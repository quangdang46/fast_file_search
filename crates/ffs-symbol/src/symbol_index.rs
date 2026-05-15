//! Concurrent `symbol_name -> Vec<SymbolLocation>` index built from
//! tree-sitter parses across the workspace.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tree_sitter::{Node, Parser};

use crate::lang::detect_file_type;
use crate::outline::outline_language;
use crate::treesitter::{
    definition_weight, extract_definition_name, extract_elixir_definition_name,
    is_elixir_definition, DEFINITION_KINDS,
};
use crate::types::{FileType, Lang};

/// One occurrence of a symbol definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SymbolLocation {
    pub path: PathBuf,
    pub line: u32,
    pub end_line: u32,
    pub kind: String,
    pub weight: u16,
}

/// Plain, fully-owned snapshot of a `SymbolIndex`. Used by on-disk caches —
/// `SystemTime` becomes millis-since-epoch so postcard / serde can round-trip
/// it without needing extra crates.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SymbolIndexSnapshot {
    pub map: HashMap<String, Vec<SymbolLocation>>,
    pub files: HashMap<PathBuf, u128>,
}

/// Concurrent map: symbol name -> all known definition sites.
#[derive(Default)]
pub struct SymbolIndex {
    map: DashMap<String, Vec<SymbolLocation>>,
    files: DashMap<PathBuf, SystemTime>,
    files_indexed: AtomicUsize,
    symbols_indexed: AtomicUsize,
}

impl SymbolIndex {
    /// Empty index.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of unique symbol names tracked.
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    /// Whether the index has no entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Files indexed counter.
    #[must_use]
    pub fn files_indexed(&self) -> usize {
        self.files_indexed.load(Ordering::Relaxed)
    }

    /// Total symbol occurrences (includes duplicates across files).
    #[must_use]
    pub fn symbols_indexed(&self) -> usize {
        self.symbols_indexed.load(Ordering::Relaxed)
    }

    /// Drop everything.
    pub fn clear(&self) {
        self.map.clear();
        self.files.clear();
        self.files_indexed.store(0, Ordering::Relaxed);
        self.symbols_indexed.store(0, Ordering::Relaxed);
    }

    /// Look up all definitions of `name` (exact match).
    pub fn lookup_exact(&self, name: &str) -> Vec<SymbolLocation> {
        self.map
            .get(name)
            .map(|v| v.value().clone())
            .unwrap_or_default()
    }

    /// Glob-style prefix lookup: returns all symbols whose name starts with `prefix`.
    pub fn lookup_prefix(&self, prefix: &str) -> Vec<(String, SymbolLocation)> {
        let mut out = Vec::new();
        for entry in self.map.iter() {
            if entry.key().starts_with(prefix) {
                for loc in entry.value() {
                    out.push((entry.key().clone(), loc.clone()));
                }
            }
        }
        out
    }

    /// Unique symbol names currently in the index, in arbitrary order. Useful
    /// for callers that want a flat name list (e.g. fuzzy / typo fallback)
    /// without paying the per-location duplication of `lookup_prefix("")`.
    pub fn names(&self) -> Vec<String> {
        self.map.iter().map(|e| e.key().clone()).collect()
    }

    /// Substring search across all known symbol names.
    pub fn lookup_substring(&self, needle: &str) -> Vec<(String, SymbolLocation)> {
        let mut out = Vec::new();
        for entry in self.map.iter() {
            if entry.key().contains(needle) {
                for loc in entry.value() {
                    out.push((entry.key().clone(), loc.clone()));
                }
            }
        }
        out
    }

    /// Build a fully-owned snapshot suitable for serialization. Walks both the
    /// `map` and the `files` table once; `SystemTime` is normalised to
    /// millis-since-epoch so the snapshot can travel through serde back-ends
    /// that lack `SystemTime` support (postcard, ron, …).
    #[must_use]
    pub fn snapshot(&self) -> SymbolIndexSnapshot {
        let map: HashMap<String, Vec<SymbolLocation>> = self
            .map
            .iter()
            .map(|e| (e.key().clone(), e.value().clone()))
            .collect();
        let files: HashMap<PathBuf, u128> = self
            .files
            .iter()
            .map(|e| {
                let ms = e
                    .value()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_millis())
                    .unwrap_or(0);
                (e.key().clone(), ms)
            })
            .collect();
        SymbolIndexSnapshot { map, files }
    }

    /// Re-hydrate a `SymbolIndex` from a snapshot. Counters are recomputed
    /// from the data so `files_indexed()` / `symbols_indexed()` stay in sync.
    #[must_use]
    pub fn from_snapshot(snap: SymbolIndexSnapshot) -> Self {
        let map: DashMap<String, Vec<SymbolLocation>> = DashMap::new();
        let mut symbols_total = 0usize;
        for (k, v) in snap.map {
            symbols_total += v.len();
            map.insert(k, v);
        }
        let files: DashMap<PathBuf, SystemTime> = DashMap::new();
        for (k, ms) in snap.files {
            // u128 -> u64 is safe here: ms fits in u64 for the next ~580M
            // years past UNIX_EPOCH.
            let dur = Duration::from_millis(ms.min(u128::from(u64::MAX)) as u64);
            files.insert(k, UNIX_EPOCH + dur);
        }
        let files_total = files.len();
        Self {
            map,
            files,
            files_indexed: AtomicUsize::new(files_total),
            symbols_indexed: AtomicUsize::new(symbols_total),
        }
    }

    /// Drop all symbols originating from `paths` and forget their mtime
    /// entries. Used by incremental refresh.
    pub fn drop_files(&self, paths: &[PathBuf]) {
        if paths.is_empty() {
            return;
        }
        let drop_set: std::collections::HashSet<&Path> =
            paths.iter().map(PathBuf::as_path).collect();
        for p in paths {
            self.files.remove(p);
        }
        let mut removed = 0usize;
        self.map.iter_mut().for_each(|mut entry| {
            let before = entry.value().len();
            entry
                .value_mut()
                .retain(|loc| !drop_set.contains(loc.path.as_path()));
            removed += before - entry.value().len();
        });
        self.map.retain(|_, v| !v.is_empty());
        if removed > 0 {
            self.symbols_indexed.fetch_sub(removed, Ordering::Relaxed);
        }
        let new_files = self.files.len();
        self.files_indexed.store(new_files, Ordering::Relaxed);
    }

    /// Snapshot the set of indexed file paths. Used by incremental refresh
    /// to spot deletions (paths in the index that no longer exist on disk).
    #[must_use]
    pub fn indexed_paths(&self) -> Vec<PathBuf> {
        self.files.iter().map(|e| e.key().clone()).collect()
    }

    /// Look up the recorded mtime for `path`, if any. Mostly useful for
    /// callers that want to short-circuit re-parsing themselves.
    #[must_use]
    pub fn mtime_for(&self, path: &Path) -> Option<SystemTime> {
        self.files.get(path).map(|e| *e.value())
    }

    /// Extract symbol definitions from a file's content and add them to the index.
    /// Skips re-indexing if mtime matches a previously seen entry.
    pub fn index_file(&self, path: &Path, mtime: SystemTime, content: &str) -> usize {
        if let Some(prev) = self.files.get(path) {
            if *prev == mtime {
                return 0;
            }
        }

        if let Some(prev_mtime) = self.files.insert(path.to_path_buf(), mtime) {
            if prev_mtime != mtime {
                self.remove_path(path);
            }
        }

        let lang = match detect_file_type(path) {
            FileType::Code(l) => l,
            _ => return 0,
        };

        let Some(language) = outline_language(lang) else {
            return 0;
        };

        let mut parser = Parser::new();
        if parser.set_language(&language).is_err() {
            return 0;
        }
        let Some(tree) = parser.parse(content, None) else {
            return 0;
        };

        let lines: Vec<&str> = content.lines().collect();
        let root = tree.root_node();

        let mut count = 0usize;
        collect_definitions(root, &lines, lang, path, &mut |name, loc| {
            self.insert(name, loc);
            count += 1;
        });

        self.files_indexed.fetch_add(1, Ordering::Relaxed);
        self.symbols_indexed.fetch_add(count, Ordering::Relaxed);
        count
    }

    fn insert(&self, name: String, loc: SymbolLocation) {
        self.map.entry(name).or_default().push(loc);
    }

    fn remove_path(&self, path: &Path) {
        self.map
            .iter_mut()
            .for_each(|mut entry| entry.value_mut().retain(|loc| loc.path != path));
        let removed = self.map.iter().filter(|e| e.value().is_empty()).count();
        self.map.retain(|_, v| !v.is_empty());
        if removed > 0 {
            self.symbols_indexed.fetch_sub(removed, Ordering::Relaxed);
        }
    }
}

fn collect_definitions(
    node: Node,
    lines: &[&str],
    lang: Lang,
    path: &Path,
    out: &mut impl FnMut(String, SymbolLocation),
) {
    if lang == Lang::Elixir && is_elixir_definition(node, lines) {
        if let Some(name) = extract_elixir_definition_name(node, lines) {
            let loc = SymbolLocation {
                path: path.to_path_buf(),
                line: node.start_position().row as u32 + 1,
                end_line: node.end_position().row as u32 + 1,
                kind: "elixir_def".to_string(),
                weight: 100,
            };
            out(name, loc);
        }
    } else if DEFINITION_KINDS.contains(&node.kind()) {
        if let Some(name) = extract_definition_name(node, lines) {
            let loc = SymbolLocation {
                path: path.to_path_buf(),
                line: node.start_position().row as u32 + 1,
                end_line: node.end_position().row as u32 + 1,
                kind: node.kind().to_string(),
                weight: definition_weight(node.kind()),
            };
            out(name, loc);
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_definitions(child, lines, lang, path, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch_file(content: &str, ext: &str) -> tempfile::NamedTempFile {
        let path = tempfile::Builder::new()
            .suffix(&format!(".{ext}"))
            .tempfile()
            .unwrap();
        std::fs::write(path.path(), content).unwrap();
        path
    }

    #[test]
    fn indexes_rust_functions() {
        let f = touch_file("fn alpha() {}\nfn beta() {}\n", "rs");
        let idx = SymbolIndex::new();
        let mtime = std::fs::metadata(f.path()).unwrap().modified().unwrap();
        let n = idx.index_file(f.path(), mtime, &std::fs::read_to_string(f.path()).unwrap());
        assert_eq!(n, 2);
        assert_eq!(idx.lookup_exact("alpha").len(), 1);
        assert_eq!(idx.lookup_exact("beta").len(), 1);
    }

    #[test]
    fn skips_reindex_for_same_mtime() {
        let f = touch_file("fn alpha() {}\n", "rs");
        let idx = SymbolIndex::new();
        let mtime = std::fs::metadata(f.path()).unwrap().modified().unwrap();
        let content = std::fs::read_to_string(f.path()).unwrap();
        idx.index_file(f.path(), mtime, &content);
        let n = idx.index_file(f.path(), mtime, &content);
        assert_eq!(n, 0);
        assert_eq!(idx.lookup_exact("alpha").len(), 1);
    }

    #[test]
    fn lookup_prefix_returns_matching_symbols() {
        let f = touch_file("fn process_request() {}\nfn process_response() {}\n", "rs");
        let idx = SymbolIndex::new();
        let mtime = std::fs::metadata(f.path()).unwrap().modified().unwrap();
        idx.index_file(f.path(), mtime, &std::fs::read_to_string(f.path()).unwrap());
        let res = idx.lookup_prefix("process_");
        assert_eq!(res.len(), 2);
    }

    #[test]
    fn snapshot_roundtrips_symbols_and_files() {
        let f = touch_file("fn alpha() {}\nfn beta() {}\n", "rs");
        let idx = SymbolIndex::new();
        let mtime = std::fs::metadata(f.path()).unwrap().modified().unwrap();
        idx.index_file(f.path(), mtime, &std::fs::read_to_string(f.path()).unwrap());

        let snap = idx.snapshot();
        let restored = SymbolIndex::from_snapshot(snap);
        assert_eq!(restored.lookup_exact("alpha").len(), 1);
        assert_eq!(restored.lookup_exact("beta").len(), 1);
        assert_eq!(restored.files_indexed(), 1);
        assert_eq!(restored.symbols_indexed(), 2);
    }

    #[test]
    fn drop_files_removes_symbols_and_updates_counters() {
        let f1 = touch_file("fn alpha() {}\n", "rs");
        let f2 = touch_file("fn beta() {}\n", "rs");
        let idx = SymbolIndex::new();
        let mtime1 = std::fs::metadata(f1.path()).unwrap().modified().unwrap();
        let mtime2 = std::fs::metadata(f2.path()).unwrap().modified().unwrap();
        idx.index_file(
            f1.path(),
            mtime1,
            &std::fs::read_to_string(f1.path()).unwrap(),
        );
        idx.index_file(
            f2.path(),
            mtime2,
            &std::fs::read_to_string(f2.path()).unwrap(),
        );
        assert_eq!(idx.files_indexed(), 2);

        idx.drop_files(&[f1.path().to_path_buf()]);
        assert!(idx.lookup_exact("alpha").is_empty());
        assert_eq!(idx.lookup_exact("beta").len(), 1);
        assert_eq!(idx.files_indexed(), 1);
    }
}
