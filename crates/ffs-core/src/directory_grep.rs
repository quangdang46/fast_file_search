//! Walk-and-grep helper: search a directory tree for lines matching a regex
//! pattern without requiring a pre-built index.
//!
//! This module provides [`grep_directory`] which uses `ignore::WalkBuilder` to
//! walk files (respecting `.gitignore`), reads each file, and searches for the
//! given regex pattern. Binary files (detected by extension) are skipped.
//! Results are collected up to `max_matches` and returned as
//! [`DirectoryGrepMatch`].

use std::path::Path;

/// A single match returned by [`grep_directory`].
#[derive(Debug, Clone)]
pub struct DirectoryGrepMatch {
    /// Absolute path to the matched file.
    pub path: String,
    /// 1-based line number of the match.
    pub line_number: u64,
    /// The matched line content (no trailing newline).
    pub line: String,
}

/// Walk `root`, search each non-binary file for `pattern` (regex), and return
/// up to `max_matches` results.
///
/// Files are walked with `ignore::WalkBuilder` so `.gitignore` / `.ignore`
/// rules are respected. Files whose extension matches the known-binary list
/// (images, archives, executables, etc.) are skipped without being read.
///
/// # Examples
///
/// ```ignore
/// use ffs_search::directory_grep::grep_directory;
/// use std::path::Path;
///
/// let matches = grep_directory(Path::new("/tmp"), r"TODO", 10);
/// for m in &matches {
///     println!("{}:{}:{}", m.path, m.line_number, m.line);
/// }
/// ```
pub fn grep_directory(root: &Path, pattern: &str, max_matches: usize) -> Vec<DirectoryGrepMatch> {
    if max_matches == 0 || pattern.is_empty() {
        return Vec::new();
    }

    let re = match regex::RegexBuilder::new(pattern).multi_line(true).build() {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let mut walk_builder = ignore::WalkBuilder::new(root);
    walk_builder
        .hidden(false)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .ignore(true)
        .follow_links(false);

    // Parallel walk (gitignore-aware) + rayon reads. Results are flattened
    // in sorted-path order so output order is deterministic; per-file match
    // lists are capped at max_matches since the global cap can never exceed it.
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .min(8);
    walk_builder.threads(threads);

    use ignore::WalkState;
    use parking_lot::Mutex;

    let paths: Mutex<Vec<std::path::PathBuf>> = Mutex::new(Vec::new());
    walk_builder.build_parallel().run(|| {
        let paths = &paths;
        Box::new(move |result| match result {
            Ok(entry) => {
                if entry.file_type().is_some_and(|ft| ft.is_file()) {
                    let path = entry.into_path();
                    if !crate::file_picker::is_known_binary_extension(&path) {
                        paths.lock().push(path);
                    }
                }
                WalkState::Continue
            }
            Err(_) => WalkState::Continue,
        })
    });

    let mut paths = paths.into_inner();
    paths.sort();

    use rayon::prelude::*;
    let per_file: Vec<Vec<DirectoryGrepMatch>> = paths
        .par_iter()
        .map(|path| {
            let Ok(content) = crate::bom::read_file(path) else {
                return Vec::new();
            };
            let path_str = path.to_string_lossy().into_owned();
            content
                .lines()
                .enumerate()
                .filter(|&(_, line)| re.is_match(line))
                .map(|(line_number, line)| DirectoryGrepMatch {
                    path: path_str.clone(),
                    line_number: line_number as u64 + 1,
                    line: line.to_string(),
                })
                .take(max_matches)
                .collect()
        })
        .collect();

    let mut results: Vec<DirectoryGrepMatch> = Vec::new();
    for file_matches in per_file {
        for m in file_matches {
            results.push(m);
            if results.len() >= max_matches {
                return results;
            }
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup_test_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("hello.rs"),
            "fn main() {\n    println!(\"hello\");\n}\n",
        )
        .unwrap();
        fs::write(
            dir.path().join("lib.rs"),
            "pub fn greet(name: &str) -> String {\n    format!(\"Hello, {name}\")\n}\n",
        )
        .unwrap();
        fs::write(dir.path().join("readme.md"), "# My Project\n\nWelcome!\n").unwrap();
        dir
    }

    #[test]
    fn test_grep_directory_basic() {
        let dir = setup_test_dir();
        let results = grep_directory(dir.path(), r"fn ", 10);
        assert_eq!(results.len(), 2, "should match both Rust functions");
        assert!(results.iter().any(|m| m.path.contains("hello.rs")));
        assert!(results.iter().any(|m| m.path.contains("lib.rs")));
    }

    #[test]
    fn test_grep_directory_max_matches() {
        let dir = setup_test_dir();
        let results = grep_directory(dir.path(), r"fn ", 1);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_grep_directory_empty_pattern() {
        let dir = setup_test_dir();
        let results = grep_directory(dir.path(), "", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_grep_directory_zero_max_matches() {
        let dir = setup_test_dir();
        let results = grep_directory(dir.path(), r"fn ", 0);
        assert!(results.is_empty());
    }

    #[test]
    fn test_grep_directory_no_match() {
        let dir = setup_test_dir();
        let results = grep_directory(dir.path(), r"ZZZZNOTFOUND", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_grep_directory_invalid_regex() {
        let dir = setup_test_dir();
        let results = grep_directory(dir.path(), r"[invalid", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_grep_directory_skips_binary_extensions() {
        let dir = setup_test_dir();
        fs::write(
            dir.path().join("image.png"),
            "this has fn text but should be skipped",
        )
        .unwrap();
        let results = grep_directory(dir.path(), r"fn ", 10);
        let png_matches: Vec<_> = results
            .iter()
            .filter(|m| m.path.ends_with(".png"))
            .collect();
        assert!(
            png_matches.is_empty(),
            "binary extension files should be skipped"
        );
    }

    #[test]
    fn test_grep_directory_nonexistent_dir() {
        let dir = std::env::temp_dir().join("ffs_nonexistent_dir_0123456789");
        let results = grep_directory(&dir, r"fn ", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_grep_directory_line_numbers() {
        let dir = setup_test_dir();
        let results = grep_directory(dir.path(), r"println", 10);
        assert_eq!(results.len(), 1);
        let m = &results[0];
        assert_eq!(m.line_number, 2, "println is on line 2 of hello.rs");
        assert!(m.path.ends_with("hello.rs"));
    }
}
