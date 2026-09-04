use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

/// Walk `root`, find files matching `pattern`, return up to `max_files` paths.
///
/// Paths are relative to `root`. Uses zlob (SIMD glob matching with
/// gitignore support) when the `zlob` feature is enabled and the platform
/// supports it; falls back to `globset::Glob` + `ignore::WalkBuilder`.
pub fn glob_files(root: &Path, pattern: &str, max_files: usize) -> Vec<String> {
    // Fast path: simple doublestar-extension patterns (e.g. `**/*.rs`).
    // Uses parallel directory walking which is significantly faster than
    // the single-threaded zlob/globset paths for this common case.
    if let Some(result) = fast_ext_glob(root, pattern, max_files) {
        return result;
    }

    // zlob is a C library that doesn't work on Windows (no native glob()).
    // Use the pure-Rust fallback there regardless of the feature flag.
    #[cfg(all(feature = "zlob", not(target_family = "windows")))]
    {
        let flags = zlob::ZlobFlags::BRACE
            | zlob::ZlobFlags::DOUBLESTAR_RECURSIVE
            | zlob::ZlobFlags::NOSORT
            | zlob::ZlobFlags::PERIOD;
        let canon = crate::path_utils::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        #[cfg(windows)]
        let base = std::borrow::Cow::Owned(canon.to_string_lossy().replace('\\', "/"));
        #[cfg(not(windows))]
        let base = canon.to_string_lossy();
        match zlob::zlob_at(&base, pattern, flags) {
            Ok(Some(result)) => result
                .iter()
                .take(max_files)
                .map(|s| s.to_string())
                .collect(),
            Ok(None) => Vec::new(),
            Err(_) => fallback_glob(root, pattern, max_files),
        }
    }

    #[cfg(not(all(feature = "zlob", not(target_family = "windows"))))]
    {
        fallback_glob(root, pattern, max_files)
    }
}

/// Fast path for simple extension patterns like `**/*.rs` or `*.rs`.
/// Uses parallel directory walking for a significant speedup over zlob.
fn fast_ext_glob(root: &Path, pattern: &str, max_files: usize) -> Option<Vec<String>> {
    // Match: **/*.ext  or  *.ext
    let ext = pattern
        .strip_prefix("**/*.")
        .or_else(|| pattern.strip_prefix("*."))?;

    if ext.is_empty() || ext.contains('*') || ext.contains('/') || ext.contains('{') {
        return None;
    }

    let done = Arc::new(AtomicBool::new(false));
    let matches = Arc::new(Mutex::new(Vec::with_capacity(max_files.min(512))));
    let suffix = format!(".{ext}");

    struct ExtVisitor {
        suffix: String,
        root: std::path::PathBuf,
        max_files: usize,
        done: Arc<AtomicBool>,
        matches: Arc<Mutex<Vec<String>>>,
    }

    impl ignore::ParallelVisitor for ExtVisitor {
        fn visit(&mut self, result: Result<ignore::DirEntry, ignore::Error>) -> ignore::WalkState {
            if self.done.load(Ordering::Relaxed) {
                return ignore::WalkState::Quit;
            }
            let entry = match result {
                Ok(e) => e,
                Err(_) => return ignore::WalkState::Skip,
            };
            let _ft = match entry.file_type() {
                Some(ft) if ft.is_file() => ft,
                _ => return ignore::WalkState::Continue,
            };
            let name = match entry.file_name().to_str() {
                Some(n) => n,
                None => return ignore::WalkState::Continue,
            };
            if name.len() > self.suffix.len()
                && name.ends_with(self.suffix.as_str())
                && let Ok(rel) = entry.path().strip_prefix(&self.root)
            {
                #[cfg(windows)]
                let rel_str = rel.to_string_lossy().replace('\\', "/");
                #[cfg(not(windows))]
                let rel_str = rel.to_string_lossy().to_string();
                let mut guard = self.matches.lock();
                if guard.len() < self.max_files {
                    guard.push(rel_str);
                    if guard.len() >= self.max_files {
                        self.done.store(true, Ordering::Relaxed);
                        return ignore::WalkState::Quit;
                    }
                }
            }
            ignore::WalkState::Continue
        }
    }

    struct ExtVisitorBuilder {
        suffix: String,
        root: std::path::PathBuf,
        max_files: usize,
        done: Arc<AtomicBool>,
        matches: Arc<Mutex<Vec<String>>>,
    }

    impl<'a> ignore::ParallelVisitorBuilder<'a> for ExtVisitorBuilder {
        fn build(&mut self) -> Box<dyn ignore::ParallelVisitor + 'a> {
            Box::new(ExtVisitor {
                suffix: self.suffix.clone(),
                root: self.root.clone(),
                max_files: self.max_files,
                done: Arc::clone(&self.done),
                matches: Arc::clone(&self.matches),
            })
        }
    }

    let root_owned = root.to_path_buf();
    let walker = ignore::WalkBuilder::new(root)
        .standard_filters(true)
        .follow_links(false)
        .build_parallel();

    walker.visit(&mut ExtVisitorBuilder {
        suffix,
        root: root_owned,
        max_files,
        done,
        matches: Arc::clone(&matches),
    });

    let mut result = Arc::try_unwrap(matches).unwrap().into_inner();
    result.truncate(max_files);
    Some(result)
}

/// Pure-Rust glob implementation using `globset` + `ignore::WalkBuilder`.
fn fallback_glob(root: &Path, pattern: &str, max_files: usize) -> Vec<String> {
    let Ok(glob) = globset::GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
    else {
        return Vec::new();
    };
    let matcher = glob.compile_matcher();
    ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .build()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_some_and(|ft| ft.is_file()))
        .filter_map(|e| {
            let path = e.path();
            let rel = path.strip_prefix(root).unwrap_or(path);
            if matcher.is_match(rel) {
                rel.to_str().map(|s| {
                    #[cfg(windows)]
                    {
                        s.replace('\\', "/")
                    }
                    #[cfg(not(windows))]
                    {
                        s.to_string()
                    }
                })
            } else {
                None
            }
        })
        .take(max_files)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_glob_files_basic() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("main.rs"), "").unwrap();
        fs::write(dir.path().join("lib.rs"), "").unwrap();
        fs::write(dir.path().join("readme.md"), "").unwrap();

        let results = glob_files(dir.path(), "*.rs", 100);
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|p| p == "main.rs"));
        assert!(results.iter().any(|p| p == "lib.rs"));
    }

    #[test]
    fn test_glob_files_no_match() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("main.rs"), "").unwrap();

        let results = glob_files(dir.path(), "*.py", 100);
        assert!(results.is_empty());
    }

    #[test]
    fn test_glob_files_respects_limit() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..10 {
            fs::write(dir.path().join(format!("file_{i}.rs")), "").unwrap();
        }

        let results = glob_files(dir.path(), "*.rs", 3);
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_glob_files_nested_forward_slash_pattern() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("src").join("foo");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("bar.ts"), "").unwrap();
        fs::write(dir.path().join("src").join("top.ts"), "").unwrap();
        fs::write(dir.path().join("src").join("skip.js"), "").unwrap();

        let results = glob_files(dir.path(), "src/**/*.ts", 100);
        assert_eq!(results.len(), 2, "should match nested + top-level .ts");
        assert!(results.iter().any(|p| p == "src/foo/bar.ts"));
        assert!(results.iter().any(|p| p == "src/top.ts"));
        assert!(results.iter().all(|p| !p.contains('\\')));
    }

    #[test]
    fn test_fast_ext_glob_doublestar() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "").unwrap();
        fs::write(dir.path().join("b.rs"), "").unwrap();
        fs::write(dir.path().join("c.txt"), "").unwrap();
        let sub = dir.path().join("src");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("d.rs"), "").unwrap();

        let results = glob_files(dir.path(), "**/*.rs", 100);
        assert_eq!(results.len(), 3);
    }

    #[test]
    fn test_fast_ext_glob_simple() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "").unwrap();
        fs::write(dir.path().join("b.py"), "").unwrap();

        let results = glob_files(dir.path(), "*.rs", 100);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], "a.rs");
    }
}
