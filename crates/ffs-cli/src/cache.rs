//! On-disk cache for the tree-sitter `SymbolIndex`.
//!
//! Layout under `<repo-root>/.ffs/`:
//!
//! ```text
//! .ffs/
//!   meta.json                    -> CacheMeta (schema, version, git head, …)
//!   symbol_index.postcard.zst    -> postcard(SymbolIndexSnapshot) | zstd
//! ```
//!
//! Invalidation strategy: the cache is treated as fresh when the schema
//! version, git HEAD, and file count all match. Anything else triggers a
//! full rebuild on the next code-navigation command.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use ffs_engine::{Engine, EngineConfig};
use ffs_symbol::{SymbolIndex, SymbolIndexSnapshot};

const SCHEMA_VERSION: &str = "v1";
const FFS_VERSION: &str = env!("CARGO_PKG_VERSION");
const META_FILE: &str = "meta.json";
const SYMBOL_FILE: &str = "symbol_index.postcard.zst";
const ZSTD_LEVEL: i32 = 19;

/// Persisted alongside each cache payload — used to decide whether the cache
/// can be reused on a subsequent run.
#[derive(Debug, Serialize, Deserialize)]
struct CacheMeta {
    schema_version: String,
    ffs_version: String,
    git_head: Option<String>,
    file_count: usize,
    generated_at_ms: u128,
}

/// Handle to a `<root>/.ffs/` directory. Construction is cheap and infallible.
pub struct CacheDir {
    dir: PathBuf,
}

impl CacheDir {
    /// Locate the cache directory for `root`. Does not touch the filesystem.
    #[must_use]
    pub fn at(root: &Path) -> Self {
        Self {
            dir: root.join(".ffs"),
        }
    }

    /// Create `<root>/.ffs/` if it does not already exist.
    pub fn ensure(&self) -> Result<()> {
        fs::create_dir_all(&self.dir).with_context(|| format!("create {:?}", self.dir))
    }

    /// Path to the symbol-index payload.
    #[must_use]
    pub fn symbol_path(&self) -> PathBuf {
        self.dir.join(SYMBOL_FILE)
    }

    /// Path to the meta.json file.
    #[must_use]
    pub fn meta_path(&self) -> PathBuf {
        self.dir.join(META_FILE)
    }

    /// Load the cached symbol index for `root`, returning `None` if the cache
    /// is missing, corrupt, or invalidated by a metadata mismatch.
    pub fn load_symbol_index(&self, root: &Path) -> Option<SymbolIndex> {
        let meta = self.read_meta().ok()?;
        if meta.schema_version != SCHEMA_VERSION {
            return None;
        }
        if !head_matches(root, meta.git_head.as_deref()) {
            return None;
        }
        if !file_count_within_tolerance(root, meta.file_count) {
            return None;
        }
        let bytes = fs::read(self.symbol_path()).ok()?;
        let decompressed = zstd::stream::decode_all(&bytes[..]).ok()?;
        let snap: SymbolIndexSnapshot = postcard::from_bytes(&decompressed).ok()?;
        Some(SymbolIndex::from_snapshot(snap))
    }

    /// Snapshot `idx` to `<root>/.ffs/symbol_index.postcard.zst`, atomically
    /// replacing any previous payload. Also rewrites `meta.json`.
    pub fn write_symbol_index(&self, idx: &SymbolIndex, root: &Path) -> Result<()> {
        self.ensure()?;
        let snap = idx.snapshot();
        let payload = postcard::to_allocvec(&snap).context("postcard serialize symbol index")?;
        let mut compressed = Vec::with_capacity(payload.len() / 4);
        zstd::stream::copy_encode(&payload[..], &mut compressed, ZSTD_LEVEL)
            .context("zstd compress symbol index")?;
        atomic_write(&self.symbol_path(), &compressed)?;

        let meta = CacheMeta {
            schema_version: SCHEMA_VERSION.to_string(),
            ffs_version: FFS_VERSION.to_string(),
            git_head: read_git_head(root),
            file_count: idx.files_indexed(),
            generated_at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or(Duration::ZERO)
                .as_millis(),
        };
        let meta_bytes = serde_json::to_vec_pretty(&meta).context("serialize cache meta")?;
        atomic_write(&self.meta_path(), &meta_bytes)?;
        Ok(())
    }

    fn read_meta(&self) -> Result<CacheMeta> {
        let raw = fs::read(self.meta_path()).context("read cache meta")?;
        serde_json::from_slice(&raw).context("parse cache meta")
    }
}

/// Best-effort: build an `Engine` for `root`, reusing the on-disk symbol
/// index when fresh and rebuilding it (plus rewriting the cache) on miss.
///
/// The bloom + outline caches always start empty and rebuild lazily on
/// demand, since the heavy cost being amortized is the tree-sitter parse.
pub fn load_or_build_engine(root: &Path) -> Engine {
    let cache = CacheDir::at(root);
    if let Some(idx) = cache.load_symbol_index(root) {
        return Engine::with_symbols(EngineConfig::default(), Arc::new(idx));
    }
    let engine = Engine::default();
    engine.index(root);
    let _ = cache.write_symbol_index(&engine.handles.symbols, root);
    engine
}

fn head_matches(root: &Path, expected: Option<&str>) -> bool {
    read_git_head(root).as_deref() == expected
}

/// Allow up to ±5% drift in file count (or ±10 files for tiny repos) before
/// invalidating the cache. This keeps small edits / new tests from forcing a
/// 79-second rebuild on the kernel-sized workspace.
fn file_count_within_tolerance(root: &Path, expected: usize) -> bool {
    let now = crate::commands::walk_files(root).len();
    let tolerance = (expected / 20).max(10) as i64;
    (now as i64 - expected as i64).abs() <= tolerance
}

fn read_git_head(root: &Path) -> Option<String> {
    let head = fs::read_to_string(root.join(".git/HEAD")).ok()?;
    let trimmed = head.trim();
    if let Some(rest) = trimmed.strip_prefix("ref: ") {
        let ref_path = root.join(".git").join(rest);
        fs::read_to_string(ref_path)
            .ok()
            .map(|s| s.trim().to_string())
    } else {
        Some(trimmed.to_string())
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {parent:?}"))?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut f = fs::File::create(&tmp).with_context(|| format!("create tmp {tmp:?}"))?;
        f.write_all(bytes).context("write cache payload")?;
        f.sync_all().context("fsync cache payload")?;
    }
    fs::rename(&tmp, path).with_context(|| format!("rename {tmp:?} -> {path:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn build_index(content: &str, name: &str) -> (tempfile::TempDir, SymbolIndex) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        std::fs::write(&path, content).unwrap();
        let idx = SymbolIndex::new();
        let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
        idx.index_file(&path, mtime, content);
        (dir, idx)
    }

    #[test]
    fn roundtrips_symbol_index_through_disk() {
        let (dir, idx) = build_index("fn alpha() {}\nfn beta() {}\n", "lib.rs");
        let cache = CacheDir::at(dir.path());
        cache.write_symbol_index(&idx, dir.path()).unwrap();

        let loaded = cache.load_symbol_index(dir.path()).expect("cache hit");
        assert_eq!(loaded.lookup_exact("alpha").len(), 1);
        assert_eq!(loaded.lookup_exact("beta").len(), 1);
        assert_eq!(loaded.symbols_indexed(), 2);
    }

    #[test]
    fn invalidate_on_schema_mismatch() {
        let (dir, idx) = build_index("fn alpha() {}\n", "lib.rs");
        let cache = CacheDir::at(dir.path());
        cache.write_symbol_index(&idx, dir.path()).unwrap();

        // Tamper with meta to bump schema_version.
        let stale = CacheMeta {
            schema_version: "v0".to_string(),
            ffs_version: FFS_VERSION.to_string(),
            git_head: read_git_head(dir.path()),
            file_count: idx.files_indexed(),
            generated_at_ms: 0,
        };
        atomic_write(&cache.meta_path(), &serde_json::to_vec(&stale).unwrap()).unwrap();

        assert!(cache.load_symbol_index(dir.path()).is_none());
    }

    #[test]
    fn invalidate_on_head_change() {
        let (dir, idx) = build_index("fn alpha() {}\n", "lib.rs");
        let cache = CacheDir::at(dir.path());
        cache.write_symbol_index(&idx, dir.path()).unwrap();

        // Forge a fake .git/HEAD pointing at a different commit-ish than
        // whatever was captured at write time (which was None for this temp
        // dir, since .git/ doesn't exist there yet).
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/HEAD"), "deadbeefdeadbeefdeadbeef\n").unwrap();

        assert!(cache.load_symbol_index(dir.path()).is_none());
    }

    #[test]
    fn cache_dir_paths_are_under_dot_ffs() {
        let cache = CacheDir::at(Path::new("/tmp/example"));
        assert!(cache
            .symbol_path()
            .ends_with(".ffs/symbol_index.postcard.zst"));
        assert!(cache.meta_path().ends_with(".ffs/meta.json"));
    }

    #[test]
    fn load_or_build_engine_writes_cache_on_miss() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("lib.rs"), "fn alpha() {}\n").unwrap();

        // Cold: no cache yet.
        let engine = load_or_build_engine(dir.path());
        assert_eq!(engine.handles.symbols.lookup_exact("alpha").len(), 1);

        // Cache file should now exist.
        let cache = CacheDir::at(dir.path());
        assert!(cache.symbol_path().exists());

        // Warm: re-running should hit the cache (no panic, same answer).
        let engine = load_or_build_engine(dir.path());
        assert_eq!(engine.handles.symbols.lookup_exact("alpha").len(), 1);
        let _ = SystemTime::now();
    }
}
