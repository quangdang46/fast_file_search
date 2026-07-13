use std::path::Path;

/// Walk `root`, find files matching `pattern`, return up to `max_files` paths.
///
/// Paths are relative to `root`. Uses zlob (SIMD glob matching with
/// gitignore support) when the `zlob` feature is enabled and the platform
/// supports it; falls back to `globset::Glob` + `ignore::WalkBuilder`.
pub fn glob_files(root: &Path, pattern: &str, max_files: usize) -> Vec<String> {
    // zlob is a C library that doesn't work on Windows (no native glob()).
    // Use the pure-Rust fallback there regardless of the feature flag.
    #[cfg(all(feature = "zlob", not(target_family = "windows")))]
    {
        let flags = zlob::ZlobFlags::BRACE
            | zlob::ZlobFlags::DOUBLESTAR_RECURSIVE
            | zlob::ZlobFlags::NOSORT
            | zlob::ZlobFlags::PERIOD;
        // Use path_utils canonicalize to avoid \\?\ prefix on Windows.
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

    // Regression for #76/#69: nested recursive patterns must match and always
    // emit forward-slash relative paths (Windows previously returned `[]` or
    // backslash paths that broke agents).
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
}
