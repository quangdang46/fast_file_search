//! Glob pattern file search — walk a directory tree and return matching paths.
//!
//! This module provides [`glob_files`] which walks a directory tree respecting
//! `.gitignore` and returns file paths matching the given glob pattern.
//!
//! Under the hood it delegates to `zlob` (Zig-compiled C library) when the
//! `zlob` feature is enabled, otherwise falls back to `globset` + `ignore`.
//!
//! # Example
//!
//! ```no_run
//! use std::path::Path;
//! use ffs_search::glob_matcher::glob_files;
//!
//! let files = glob_files(Path::new("/tmp"), "*.rs", 50);
//! for f in &files {
//!     println!("{f}");
//! }
//! ```

use std::path::Path;

/// Walk `root`, find files matching `pattern`, return up to `max_files` paths.
///
/// Paths are relative to `root`. Uses zlob (SIMD glob matching with
/// gitignore support) when the `zlob` feature is enabled; falls back to
/// `globset::Glob` + `ignore::WalkBuilder`.
pub fn glob_files(root: &Path, pattern: &str, max_files: usize) -> Vec<String> {
    #[cfg(feature = "zlob")]
    {
        let flags = zlob::ZlobFlags::BRACE
            | zlob::ZlobFlags::DOUBLESTAR_RECURSIVE
            | zlob::ZlobFlags::NOSORT
            | zlob::ZlobFlags::PERIOD;
        // Canonicalize to resolve symlinks (/var -> /private/var on macOS).
        let canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        let base = canon.to_string_lossy();
        match zlob::zlob_at(&base, pattern, flags) {
            Ok(Some(result)) => result
                .iter()
                .take(max_files)
                .map(|s| s.to_string())
                .collect(),
            Ok(None) => Vec::new(),
            Err(_) => Vec::new(),
        }
    }

    #[cfg(not(feature = "zlob"))]
    {
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
                    rel.to_str().map(|s| s.to_string())
                } else {
                    None
                }
            })
            .take(max_files)
            .collect()
    }
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
}
