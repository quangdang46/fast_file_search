//! Shared candidate-file collection for the navigation backends.
//!
//! `ffs-cli`, `ffs-mcp`, and `ffs-c` each need the same primitive: walk `root`
//! honoring ignore rules, keep readable code files, and hand back
//! `(path, mtime, content)`. Each surface used to carry its own copy; this is
//! the single implementation so the three cannot drift.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
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
        Self::walk(root, true).0
    }

    /// Like [`CodeFile::collect`] but reads **every** readable file, not only
    /// code. The CLI navigation commands need this: they scan all files for
    /// call sites, so a hit inside a `.md`/`.txt` file is still reported.
    /// Callers derive the language themselves and treat non-code as `None`.
    #[must_use]
    pub fn collect_all(root: &Path) -> Vec<Self> {
        Self::collect_all_with_errors(root).0
    }

    /// [`CodeFile::collect`] plus the number of walk errors (unreadable
    /// directories, permission failures) that `ignore` reported and we
    /// skipped. `collect`/`collect_all` discard the count for compatibility;
    /// callers that surface diagnostics should use the `_with_errors` forms.
    #[must_use]
    pub fn collect_with_errors(root: &Path) -> (Vec<Self>, usize) {
        Self::walk(root, true)
    }

    /// [`CodeFile::collect_all`] plus the walk-error count — see
    /// [`CodeFile::collect_with_errors`].
    #[must_use]
    pub fn collect_all_with_errors(root: &Path) -> (Vec<Self>, usize) {
        Self::walk(root, false)
    }

    /// Parallel variant of [`CodeFile::collect`]: same gitignore-aware walk
    /// and code-only filter, but the walk runs on `ignore`'s thread pool and
    /// file reads run under rayon. Output is sorted by path (deterministic),
    /// unlike the serial walker whose order is directory order.
    #[must_use]
    pub fn collect_parallel(root: &Path, code_only: bool) -> Vec<Self> {
        let (paths, _walk_errors) = walk_paths_sorted(root);
        use rayon::prelude::*;
        paths
            .par_iter()
            .filter(|p| !code_only || matches!(detect_file_type(p), FileType::Code(_)))
            .filter_map(|p| Self::read(p))
            .collect()
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
    /// Returns `(files, walk_errors)` — walk errors are counted, not
    /// swallowed, so callers can report partially-covered scans.
    fn walk(root: &Path, code_only: bool) -> (Vec<Self>, usize) {
        use ignore::WalkBuilder;

        let mut walk_errors = 0usize;
        let files = Self::from_paths(
            WalkBuilder::new(root)
                .standard_filters(true)
                .follow_links(false)
                .build()
                .filter_map(|entry| match entry {
                    Ok(e) => Some(e),
                    Err(_) => {
                        walk_errors += 1;
                        None
                    }
                })
                .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
                .map(|e| e.into_path()),
            code_only,
        );
        (files, walk_errors)
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
    ///
    /// This clones every file's content. When the triples are only fed to
    /// the bloom confirm, prefer `PreFilterStack::confirm_symbol_ref` with
    /// `(f.path.as_path(), f.mtime, f.content.as_str())` — it borrows the
    /// bodies instead of duplicating the whole candidate set in memory.
    #[must_use]
    pub fn into_confirm_input(files: &[Self]) -> Vec<(PathBuf, SystemTime, String)> {
        files
            .iter()
            .map(|f| (f.path.clone(), f.mtime, f.content.clone()))
            .collect()
    }
}

/// Parallel walk of `root` returning every file path, sorted for
/// deterministic downstream ordering. Same filters as [`CodeFile::collect`]
/// (`standard_filters`, no symlink following); walk errors are counted and
/// returned alongside.
fn walk_paths_sorted(root: &Path) -> (Vec<PathBuf>, usize) {
    use ignore::WalkState;

    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .min(8);

    let paths: parking_lot::Mutex<Vec<PathBuf>> = parking_lot::Mutex::new(Vec::new());
    let walk_errors = std::sync::atomic::AtomicUsize::new(0);
    let walker = ignore::WalkBuilder::new(root)
        .standard_filters(true)
        .follow_links(false)
        .threads(threads)
        .build_parallel();
    walker.run(|| {
        let paths = &paths;
        let walk_errors = &walk_errors;
        Box::new(move |entry| match entry {
            Ok(e) => {
                if e.file_type().is_some_and(|t| t.is_file()) {
                    paths.lock().push(e.into_path());
                }
                WalkState::Continue
            }
            Err(_) => {
                walk_errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                WalkState::Continue
            }
        })
    });

    let mut paths = paths.into_inner();
    // Sort so hit truncation stays deterministic across calls — the parallel
    // walker yields paths in arrival order.
    paths.sort_unstable();
    (
        paths,
        walk_errors.load(std::sync::atomic::Ordering::Relaxed),
    )
}

/// Shared snapshot of code files: `Arc`-wrapped entries so unchanged files
/// are reused across refreshes without cloning their contents.
pub type CodeFileSnapshot = Arc<Vec<Arc<CodeFile>>>;

struct CachedCodeFiles {
    root: PathBuf,
    files: CodeFileSnapshot,
    /// path -> (mtime, byte_len) observed at snapshot time; the staleness key.
    manifest: HashMap<PathBuf, (SystemTime, u64)>,
}

/// Incremental, mtime-aware cache of code-file contents for long-lived
/// callers (the MCP `engine_*` tools).
///
/// [`CodeFileCache::get`] still walks the tree on every call — in parallel —
/// and stats each file, but only files whose `(mtime, len)` changed since the
/// previous snapshot are re-read; unchanged files reuse the cached `String`
/// via `Arc`. The first call pays the full read; later calls pay
/// walk+stat+diff only. A different `root` invalidates the whole snapshot.
///
/// Single-flight: concurrent callers serialize on the write lock and the
/// second observes the refreshed snapshot, so results are always consistent.
#[derive(Default)]
pub struct CodeFileCache {
    inner: parking_lot::RwLock<Option<CachedCodeFiles>>,
}

impl CodeFileCache {
    /// Fresh (or incrementally refreshed) snapshot of every readable code
    /// file under `root`, sorted by path.
    #[must_use]
    pub fn get(&self, root: &Path) -> CodeFileSnapshot {
        use rayon::prelude::*;

        let mut guard = self.inner.write();

        // Phase 1: parallel walk, then parallel code-filter + stat.
        let (paths, _walk_errors) = walk_paths_sorted(root);
        let statted: Vec<(PathBuf, SystemTime, u64)> = paths
            .par_iter()
            .filter(|p| matches!(detect_file_type(p), FileType::Code(_)))
            .filter_map(|p| {
                let meta = std::fs::metadata(p).ok()?;
                let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                Some((p.clone(), mtime, meta.len()))
            })
            .collect();
        // par_iter preserves the sorted path order.

        // Phase 2: diff against the previous snapshot; re-read only changes.
        let old = guard.take().filter(|c| c.root == root);
        let old_files: HashMap<PathBuf, Arc<CodeFile>> = old
            .as_ref()
            .map(|c| {
                c.files
                    .iter()
                    .map(|f| (f.path.clone(), Arc::clone(f)))
                    .collect()
            })
            .unwrap_or_default();

        let mut files: Vec<Arc<CodeFile>> = Vec::with_capacity(statted.len());
        let mut manifest: HashMap<PathBuf, (SystemTime, u64)> =
            HashMap::with_capacity(statted.len());
        for (path, mtime, len) in statted {
            let unchanged = old
                .as_ref()
                .and_then(|c| c.manifest.get(&path))
                .is_some_and(|&(om, ol)| om == mtime && ol == len);
            if unchanged {
                if let Some(f) = old_files.get(&path) {
                    manifest.insert(path, (mtime, len));
                    files.push(Arc::clone(f));
                    continue;
                }
            }
            if let Some(f) = CodeFile::read(&path) {
                manifest.insert(f.path.clone(), (mtime, len));
                files.push(Arc::new(f));
            }
        }

        let snapshot: CodeFileSnapshot = Arc::new(files);
        *guard = Some(CachedCodeFiles {
            root: root.to_path_buf(),
            files: Arc::clone(&snapshot),
            manifest,
        });
        snapshot
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
