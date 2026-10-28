//! Shared candidate-file collection for the navigation backends.
//!
//! `ffs-cli`, `ffs-mcp`, and `ffs-c` each need the same primitive: walk `root`
//! honoring ignore rules, keep readable code files, and hand back
//! `(path, mtime, content)`. Each surface used to carry its own copy; this is
//! the single implementation so the three cannot drift.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ffs_symbol::lang::detect_file_type;
use ffs_symbol::types::FileType;

/// A file that survived the walk and was read into memory.
#[derive(Debug, Clone)]
pub struct CodeFile {
    pub path: PathBuf,
    pub mtime: SystemTime,
    pub content: String,
}

impl CodeFile {
    /// Walk `root` (gitignore-aware) and read every code file.
    ///
    /// Non-code files are skipped before the read, so this never loads binary
    /// blobs. Files that fail metadata/read are skipped silently — the same
    /// contract the per-surface copies had.
    #[must_use]
    pub fn collect(root: &Path) -> Vec<Self> {
        Self::walk(root, true)
    }

    /// Like [`CodeFile::collect`] but reads **every** readable file, not only
    /// code. The CLI navigation commands need this: they scan all files for
    /// call sites, so a hit inside a `.md`/`.txt` file is still reported.
    /// Callers derive the language themselves and treat non-code as `None`.
    #[must_use]
    pub fn collect_all(root: &Path) -> Vec<Self> {
        Self::walk(root, false)
    }

    /// Read the given paths, keeping only code files when `code_only`.
    ///
    /// Separate from the walk so callers that already hold a (possibly
    /// parallel-walked) path list can reuse the read/filter logic without
    /// taking the serial walker's ordering.
    #[must_use]
    pub fn from_paths(paths: impl IntoIterator<Item = PathBuf>, code_only: bool) -> Vec<Self> {
        paths
            .into_iter()
            .filter(|p| !code_only || matches!(detect_file_type(p), FileType::Code(_)))
            .filter_map(|p| Self::read(&p))
            .collect()
    }

    /// Serial walk. Ordered, which `ffs-mcp` and `ffs-c` rely on: they truncate
    /// hits at a limit, so the walk order decides which hits are returned.
    fn walk(root: &Path, code_only: bool) -> Vec<Self> {
        use ignore::WalkBuilder;

        Self::from_paths(
            WalkBuilder::new(root)
                .standard_filters(true)
                .follow_links(false)
                .build()
                .flatten()
                .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
                .map(|e| e.into_path()),
            code_only,
        )
    }

    /// Read one file into a `CodeFile`; `None` when metadata or content fails.
    #[must_use]
    pub fn read(path: &Path) -> Option<Self> {
        let meta = std::fs::metadata(path).ok()?;
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let content = ffs_search::bom::read_file(path).ok()?;
        Some(Self {
            path: path.to_path_buf(),
            mtime,
            content,
        })
    }

    /// `(path, mtime, content)` triple, the shape `PreFilterStack` consumes.
    #[must_use]
    pub fn into_confirm_input(files: &[Self]) -> Vec<(PathBuf, SystemTime, String)> {
        files
            .iter()
            .map(|f| (f.path.clone(), f.mtime, f.content.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_only_code_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "fn a() {}\n").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "not code\n").unwrap();
        std::fs::write(dir.path().join("blob.bin"), [0u8, 1, 2, 255]).unwrap();

        let files = CodeFile::collect(dir.path());
        assert_eq!(files.len(), 1, "only the .rs file is code: {files:?}");
        assert!(files[0].path.ends_with("a.rs"));
        assert_eq!(files[0].content, "fn a() {}\n");
    }

    #[test]
    fn honors_gitignore() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".gitignore"), "ignored/\n").unwrap();
        std::fs::create_dir_all(dir.path().join("ignored")).unwrap();
        std::fs::write(dir.path().join("ignored/a.rs"), "fn a() {}\n").unwrap();
        std::fs::write(dir.path().join("b.rs"), "fn b() {}\n").unwrap();

        let names: Vec<String> = CodeFile::collect(dir.path())
            .iter()
            .map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["b.rs"]);
    }

    #[test]
    fn confirm_input_matches_file_order() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "alpha\n").unwrap();
        let files = CodeFile::collect(dir.path());
        let triples = CodeFile::into_confirm_input(&files);
        assert_eq!(triples.len(), files.len());
        assert_eq!(triples[0].2, "alpha\n");
    }
}
