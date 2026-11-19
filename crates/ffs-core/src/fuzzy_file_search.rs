//! Ad-hoc fuzzy file search — walk a directory tree and find files by
//! case-insensitive substring matching against filenames and full paths.
//!
//! This module provides [`fuzzy_search_files`] for quick file lookups without
//! setting up the indexed search pipeline. Results are scored: filename matches
//! rank higher than path-only matches, and more specific matches rank higher.
//!
//! # Example
//!
//! ```no_run
//! use std::path::Path;
//! use ffs_search::fuzzy_file_search::{fuzzy_search_files, FuzzySearchOptions};
//!
//! let results = fuzzy_search_files(
//!     Path::new("/tmp"),
//!     "hello",
//!     FuzzySearchOptions { max_results: 50, ..Default::default() },
//! );
//! for m in &results {
//!     println!("{}(score={})", m.path, m.score);
//! }
//! ```

use std::path::Path;

/// A single match from [`fuzzy_search_files`].
#[derive(Debug, Clone)]
pub struct FuzzyFileMatch {
    /// Relative path from the search root.
    pub path: String,
    /// Match quality: higher = better. 2 = filename match, 1 = path-only match.
    pub score: usize,
    /// Whether the matched entry is a directory.
    pub is_dir: bool,
}

/// Options for [`fuzzy_search_files`].
#[derive(Debug, Clone)]
pub struct FuzzySearchOptions {
    /// Maximum number of results to return.
    pub max_results: usize,
    /// Whether to respect `.gitignore` rules during the walk.
    pub git_ignore: bool,
    /// When true, matches in the filename score higher (2) than path-only (1).
    pub filename_boost: bool,
}

impl Default for FuzzySearchOptions {
    fn default() -> Self {
        Self {
            max_results: 100,
            git_ignore: true,
            filename_boost: true,
        }
    }
}

/// Walk `root` and find files matching `query` by case-insensitive substring
/// matching against filenames and paths.
pub fn fuzzy_search_files(
    root: &Path,
    query: &str,
    options: FuzzySearchOptions,
) -> Vec<FuzzyFileMatch> {
    fuzzy_search_files_with_errors(root, query, options).0
}

/// [`fuzzy_search_files`] plus the number of directory-walk errors `ignore`
/// reported (permission denied, unreadable dirs). The walk errors used to be
/// dropped by `let-else-continue`, which made partially-covered scans look
/// complete.
#[must_use]
pub fn fuzzy_search_files_with_errors(
    root: &Path,
    query: &str,
    options: FuzzySearchOptions,
) -> (Vec<FuzzyFileMatch>, usize) {
    if query.is_empty() || options.max_results == 0 {
        return (Vec::new(), 0);
    }

    let query_lower = query.to_ascii_lowercase();

    let mut walker = ignore::WalkBuilder::new(root);
    walker
        .hidden(false)
        .git_ignore(options.git_ignore)
        .git_exclude(options.git_ignore)
        .git_global(options.git_ignore);

    // Parallel walk + per-entry match. The final sort makes collection order
    // irrelevant; the old collect_limit early-exit is gone (a parallel walk
    // can't stop cheaply), so matching is bounded by repo size, not results.
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .min(8);
    walker.threads(threads);

    use ignore::WalkState;
    use parking_lot::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let entries: Mutex<Vec<(std::path::PathBuf, bool)>> = Mutex::new(Vec::new());
    let walk_errors = AtomicUsize::new(0);
    walker.build_parallel().run(|| {
        let entries = &entries;
        let walk_errors = &walk_errors;
        Box::new(move |result| match result {
            Ok(entry) => {
                let is_dir = entry.file_type().is_some_and(|ft| ft.is_dir());
                let path = entry.into_path();
                // Skip known-binary files (no useful filename to fuzzy-match).
                if !is_dir && crate::file_picker::is_known_binary_extension(&path) {
                    return WalkState::Continue;
                }
                entries.lock().push((path, is_dir));
                WalkState::Continue
            }
            Err(_) => {
                walk_errors.fetch_add(1, Ordering::Relaxed);
                WalkState::Continue
            }
        })
    });

    use rayon::prelude::*;
    let mut results: Vec<FuzzyFileMatch> = entries
        .into_inner()
        .into_par_iter()
        .filter_map(|(path, is_dir)| {
            let rel = path.strip_prefix(root).unwrap_or(&path);
            let rel_str = rel.to_string_lossy();
            #[cfg(windows)]
            let path_str: std::borrow::Cow<'_, str> =
                std::borrow::Cow::Owned(rel_str.replace("\\", "/"));
            #[cfg(not(windows))]
            let path_str: std::borrow::Cow<'_, str> = rel_str.clone();

            // Zero-alloc ASCII case-insensitive substring check (needle is
            // already lowercased). Replaces per-entry to_ascii_lowercase.
            if !crate::case_insensitive_memmem::search(rel_str.as_bytes(), query_lower.as_bytes()) {
                return None;
            }

            let score = if options.filename_boost {
                let filename = rel
                    .file_name()
                    .map(|n| n.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default();
                if filename.contains(&query_lower) {
                    2
                } else {
                    1
                }
            } else {
                1
            };

            Some(FuzzyFileMatch {
                path: path_str.into_owned(),
                score,
                is_dir,
            })
        })
        .collect();

    // Sort: higher score first, directories after files, then alphabetically.
    results.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then(a.is_dir.cmp(&b.is_dir))
            .then(a.path.cmp(&b.path))
    });
    results.truncate(options.max_results);
    (results, walk_errors.load(Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_fuzzy_filename_match() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("hello_world.rs"), "").unwrap();
        fs::write(dir.path().join("goodbye.rs"), "").unwrap();
        fs::create_dir_all(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub/hello_there.rs"), "").unwrap();

        let results = fuzzy_search_files(
            dir.path(),
            "hello",
            FuzzySearchOptions {
                max_results: 100,
                ..Default::default()
            },
        );
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|m| m.path == "hello_world.rs"));
        assert!(results.iter().any(|m| m.path == "sub/hello_there.rs"));
    }

    #[test]
    fn test_fuzzy_case_insensitive() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("HelloWorld.rs"), "").unwrap();

        let results = fuzzy_search_files(dir.path(), "helloworld", FuzzySearchOptions::default());
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].path, "HelloWorld.rs");
    }

    #[test]
    fn test_fuzzy_scoring_filename_boost() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("core.rs"), "").unwrap();
        fs::create_dir_all(dir.path().join("src/lib")).unwrap();
        fs::write(dir.path().join("src/lib/core.rs"), "").unwrap();

        let results = fuzzy_search_files(dir.path(), "core", FuzzySearchOptions::default());
        assert_eq!(results.len(), 2);
        // Filename match (core.rs) should come first
        assert!(results[0].path == "core.rs");
        assert!(results[0].score >= results[1].score);
    }

    #[test]
    fn test_fuzzy_no_match() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("main.rs"), "").unwrap();

        let results = fuzzy_search_files(dir.path(), "zzzznotfound", FuzzySearchOptions::default());
        assert!(results.is_empty());
    }

    #[test]
    fn test_fuzzy_respects_limit() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..20 {
            fs::write(dir.path().join(format!("file_{i}.rs")), "").unwrap();
        }

        let results = fuzzy_search_files(
            dir.path(),
            "file",
            FuzzySearchOptions {
                max_results: 5,
                ..Default::default()
            },
        );
        assert_eq!(results.len(), 5);
    }

    #[test]
    fn test_fuzzy_empty_query() {
        let dir = tempfile::tempdir().unwrap();
        let results = fuzzy_search_files(dir.path(), "", FuzzySearchOptions::default());
        assert!(results.is_empty());
    }

    #[test]
    fn test_fuzzy_skips_binary() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("image.png"), "").unwrap();

        let results = fuzzy_search_files(dir.path(), "image", FuzzySearchOptions::default());
        // .png is binary; should be skipped even though name matches
        assert!(results.iter().all(|m| m.path != "image.png"));
    }

    #[test]
    fn test_fuzzy_zero_max() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("file.rs"), "").unwrap();

        let results = fuzzy_search_files(
            dir.path(),
            "file",
            FuzzySearchOptions {
                max_results: 0,
                ..Default::default()
            },
        );
        assert!(results.is_empty());
    }
}
