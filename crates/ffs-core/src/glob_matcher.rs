use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;

/// Walk `root`, find files matching `pattern`, return up to `max_files` paths.
///
/// Paths are relative to `root` with `/` separators. Matching semantics are
/// gitignore-style: a pattern with no `/` matches the file *name* at any
/// depth (`*.rs`, `main.rs`, `*in*`), while a pattern containing `/` matches
/// the full relative path (`src/**/*.ts`, `docs/*.md`). `**` crosses
/// directories, `*`/`?`/`{a,b}`/`[…]` behave per `globset`.
///
/// Hidden files and `.gitignore`/`.git/info/exclude`/global-exclude rules are
/// respected exactly like `ffs find`/`ffs grep` — `**` no longer leaks into
/// `.git`, `node_modules`, or ignored directories. Only real files are
/// returned (directories and symlinks are skipped), and results are sorted
/// for deterministic output.
///
/// The walk always visits the whole tree before truncating: early-quitting at
/// `max_files` returned an arbitrary subset chosen by parallel arrival order,
/// which made `--offset` paging and `--format json` output nondeterministic.
pub fn glob_files(root: &Path, pattern: &str, max_files: usize) -> Vec<String> {
    if max_files == 0 {
        return Vec::new();
    }
    // No '/' → basename match at any depth; otherwise full-path match where
    // `*` stays within a component and `**` crosses components.
    let basename_mode = !pattern.contains('/');
    let Ok(glob) = globset::GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
    else {
        return Vec::new();
    };
    let matcher = glob.compile_matcher();

    let matches = Arc::new(Mutex::new(Vec::new()));
    let root_owned = root.to_path_buf();

    struct GlobVisitor {
        matcher: globset::GlobMatcher,
        basename_mode: bool,
        root: std::path::PathBuf,
        matches: Arc<Mutex<Vec<String>>>,
    }

    impl GlobVisitor {
        #[inline]
        fn rel_string(&self, path: &Path) -> Option<String> {
            let rel = path.strip_prefix(&self.root).unwrap_or(path);
            #[cfg(windows)]
            {
                Some(rel.to_string_lossy().replace('\\', "/"))
            }
            #[cfg(not(windows))]
            {
                rel.to_str().map(str::to_string)
            }
        }
    }

    impl ignore::ParallelVisitor for GlobVisitor {
        fn visit(&mut self, result: Result<ignore::DirEntry, ignore::Error>) -> ignore::WalkState {
            let entry = match result {
                Ok(e) => e,
                Err(_) => return ignore::WalkState::Skip,
            };
            // is_file() is false for directories AND symlinks — a dangling
            // symlink can never be returned regardless of the pattern.
            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                return ignore::WalkState::Continue;
            }
            let candidate = if self.basename_mode {
                entry.file_name().to_str().map(str::to_string)
            } else {
                self.rel_string(entry.path())
            };
            let Some(candidate) = candidate else {
                return ignore::WalkState::Continue;
            };
            if self.matcher.is_match(&candidate)
                && let Some(rel) = self.rel_string(entry.path())
            {
                self.matches.lock().push(rel);
            }
            ignore::WalkState::Continue
        }
    }

    struct GlobVisitorBuilder {
        matcher: globset::GlobMatcher,
        basename_mode: bool,
        root: std::path::PathBuf,
        matches: Arc<Mutex<Vec<String>>>,
    }

    impl<'a> ignore::ParallelVisitorBuilder<'a> for GlobVisitorBuilder {
        fn build(&mut self) -> Box<dyn ignore::ParallelVisitor + 'a> {
            Box::new(GlobVisitor {
                matcher: self.matcher.clone(),
                basename_mode: self.basename_mode,
                root: self.root.clone(),
                matches: Arc::clone(&self.matches),
            })
        }
    }

    let walker = ignore::WalkBuilder::new(root)
        .standard_filters(true)
        // Apply .gitignore even outside a git checkout (temp dirs, exported
        // trees) — `ffs find`/`grep` behave the same way.
        .require_git(false)
        .follow_links(false)
        .build_parallel();

    walker.visit(&mut GlobVisitorBuilder {
        matcher,
        basename_mode,
        root: root_owned,
        matches: Arc::clone(&matches),
    });

    let mut result = Arc::try_unwrap(matches).unwrap().into_inner();
    // Parallel walk order is nondeterministic — sort so `--format json` and
    // text output are stable across runs.
    result.sort_unstable();
    result.truncate(max_files);
    result
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
    fn test_glob_files_basename_matches_any_depth() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("main.rs"), "").unwrap();
        let sub = dir.path().join("src").join("deep");
        fs::create_dir_all(&sub).unwrap();
        fs::write(sub.join("main.rs"), "").unwrap();
        fs::write(sub.join("other.txt"), "").unwrap();

        // `main.rs` has no '/' → basename match at any depth.
        let results = glob_files(dir.path(), "main.rs", 100);
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|p| p == "main.rs"));
        assert!(results.iter().any(|p| p == "src/deep/main.rs"));

        // `*in*` likewise matches basenames at any depth.
        let results = glob_files(dir.path(), "*in*", 100);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_glob_files_brace_expansion() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.rs"), "").unwrap();
        fs::write(dir.path().join("b.py"), "").unwrap();
        fs::write(dir.path().join("c.txt"), "").unwrap();

        let results = glob_files(dir.path(), "*.{rs,py}", 100);
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|p| p == "a.rs"));
        assert!(results.iter().any(|p| p == "b.py"));
    }

    #[test]
    fn test_glob_files_respects_gitignore_and_hidden() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".gitignore"), "ignored_dir/\n").unwrap();
        fs::write(dir.path().join("keep.rs"), "").unwrap();
        fs::create_dir_all(dir.path().join("ignored_dir")).unwrap();
        fs::write(dir.path().join("ignored_dir").join("skip.rs"), "").unwrap();
        fs::create_dir_all(dir.path().join(".hidden")).unwrap();
        fs::write(dir.path().join(".hidden").join("dot.rs"), "").unwrap();
        fs::write(dir.path().join(".dotfile.rs"), "").unwrap();

        let results = glob_files(dir.path(), "**/*.rs", 100);
        assert_eq!(results, vec!["keep.rs".to_string()]);
    }

    #[test]
    fn test_glob_files_star_returns_files_not_dirs() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("subdir")).unwrap();
        fs::write(dir.path().join("a.txt"), "").unwrap();
        fs::write(dir.path().join("subdir").join("b.txt"), "").unwrap();

        let results = glob_files(dir.path(), "*", 100);
        assert_eq!(
            results,
            vec!["a.txt".to_string(), "subdir/b.txt".to_string()]
        );
    }

    #[test]
    fn test_glob_files_skips_dangling_symlink() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("real.rs"), "").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(
            dir.path().join("nonexistent.rs"),
            dir.path().join("dangling.rs"),
        )
        .unwrap();

        let results = glob_files(dir.path(), "*.rs", 100);
        assert_eq!(results, vec!["real.rs".to_string()]);
    }

    #[test]
    fn test_glob_files_results_sorted() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["z.rs", "a.rs", "m.rs"] {
            fs::write(dir.path().join(name), "").unwrap();
        }
        let results = glob_files(dir.path(), "*.rs", 100);
        assert_eq!(results, vec!["a.rs", "m.rs", "z.rs"]);
    }
}
