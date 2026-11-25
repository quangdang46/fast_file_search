//! Concurrent `symbol_name -> Vec<SymbolLocation>` index built from
//! tree-sitter parses across the workspace.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use tree_sitter::Node;

use crate::lang::detect_file_type;
use crate::outline::outline_language;
use crate::treesitter::{
    definition_weight, extract_definition_names, extract_elixir_definition_name,
    is_elixir_definition, DEFINITION_KINDS,
};
use crate::types::{FileType, Lang};
use crate::verse_spans::{verse_repair_end_line, verse_skip_spurious_definition};

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
    /// path -> names of symbols defined in that file. Reverse index so
    /// `drop_files`/`remove_path` touch only entries for the affected files
    /// instead of scanning the whole symbol map once per re-parsed file.
    by_path: DashMap<PathBuf, Vec<String>>,
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
        self.by_path.clear();
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

    /// Borrowed variant of [`SymbolIndex::lookup_exact`]: runs `f` on the
    /// stored slice without cloning the `Vec<SymbolLocation>`. Hot callers
    /// that only inspect a few fields (e.g. per-token callee scans) should
    /// prefer this — `lookup_exact` allocates a full clone per call.
    pub fn with_exact<R>(&self, name: &str, f: impl FnOnce(&[SymbolLocation]) -> R) -> Option<R> {
        self.map.get(name).map(|v| f(v.value()))
    }

    /// Glob-style prefix lookup: returns all symbols whose name starts with `prefix`.
    pub fn lookup_prefix(&self, prefix: &str) -> Vec<(String, SymbolLocation)> {
        self.lookup_prefix_limited(prefix, usize::MAX)
    }

    /// Bounded variant of [`SymbolIndex::lookup_prefix`]: stops collecting
    /// after `max_results` `(name, location)` pairs instead of materializing
    /// every match first. Interactive surfaces (MCP tool calls) should use
    /// this — on a large index `lookup_prefix("*")`-style queries otherwise
    /// allocate a Vec proportional to the whole symbol table before
    /// truncating.
    ///
    /// Like the unbounded form, results are in arbitrary `DashMap` order;
    /// bounding early returns an arbitrary subset of the same order.
    #[must_use]
    pub fn lookup_prefix_limited(
        &self,
        prefix: &str,
        max_results: usize,
    ) -> Vec<(String, SymbolLocation)> {
        let mut out = Vec::new();
        if max_results == 0 {
            return out;
        }
        'outer: for entry in self.map.iter() {
            if entry.key().starts_with(prefix) {
                for loc in entry.value().iter() {
                    out.push((entry.key().clone(), loc.clone()));
                    if out.len() >= max_results {
                        break 'outer;
                    }
                }
            }
        }
        out
    }

    /// Unique symbol names currently in the index, in arbitrary order. Useful
    /// for callers that want a flat name list (e.g. fuzzy / typo fallback)
    /// without paying the per-location duplication of `lookup_prefix("")`.
    pub fn names(&self) -> Vec<String> {
        self.names_limited(usize::MAX)
    }

    /// Bounded variant of [`SymbolIndex::names`]: stops after `max_results`
    /// names instead of cloning the entire key set.
    #[must_use]
    pub fn names_limited(&self, max_results: usize) -> Vec<String> {
        let mut out = Vec::new();
        if max_results == 0 {
            return out;
        }
        for entry in self.map.iter() {
            out.push(entry.key().clone());
            if out.len() >= max_results {
                break;
            }
        }
        out
    }

    /// Substring search across all known symbol names.
    pub fn lookup_substring(&self, needle: &str) -> Vec<(String, SymbolLocation)> {
        self.lookup_substring_limited(needle, usize::MAX)
    }

    /// Bounded variant of [`SymbolIndex::lookup_substring`] — see
    /// [`SymbolIndex::lookup_prefix_limited`] for why bounding matters.
    #[must_use]
    pub fn lookup_substring_limited(
        &self,
        needle: &str,
        max_results: usize,
    ) -> Vec<(String, SymbolLocation)> {
        let mut out = Vec::new();
        if max_results == 0 {
            return out;
        }
        'outer: for entry in self.map.iter() {
            if entry.key().contains(needle) {
                for loc in entry.value().iter() {
                    out.push((entry.key().clone(), loc.clone()));
                    if out.len() >= max_results {
                        break 'outer;
                    }
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
        // Rebuild the path -> names reverse index from the snapshot.
        let by_path: DashMap<PathBuf, Vec<String>> = DashMap::new();
        for entry in map.iter() {
            for loc in entry.value().iter() {
                by_path
                    .entry(loc.path.clone())
                    .or_default()
                    .push(entry.key().clone());
            }
        }
        Self {
            map,
            by_path,
            files,
            files_indexed: AtomicUsize::new(files_total),
            symbols_indexed: AtomicUsize::new(symbols_total),
        }
    }

    /// Drop all symbols originating from `paths` and forget their mtime
    /// entries. Used by incremental refresh.
    ///
    /// Uses the `by_path` reverse index so only symbol names that actually
    /// have locations in the dropped files are visited — O(symbols in the
    /// dropped files) instead of O(entire symbol map).
    pub fn drop_files(&self, paths: &[PathBuf]) {
        if paths.is_empty() {
            return;
        }
        let drop_set: std::collections::HashSet<&Path> =
            paths.iter().map(PathBuf::as_path).collect();
        let mut names: std::collections::HashSet<String> = std::collections::HashSet::new();
        for p in paths {
            self.files.remove(p);
            if let Some((_, ns)) = self.by_path.remove(p) {
                names.extend(ns);
            }
        }
        let mut removed = 0usize;
        for name in names {
            let mut drop_entry = false;
            if let Some(mut entry) = self.map.get_mut(&name) {
                let before = entry.value().len();
                entry
                    .value_mut()
                    .retain(|loc| !drop_set.contains(loc.path.as_path()));
                removed += before - entry.value().len();
                drop_entry = entry.value().is_empty();
            }
            if drop_entry {
                self.map.remove(&name);
            }
        }
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
        let lang = match detect_file_type(path) {
            FileType::Code(l) => l,
            _ => return 0,
        };

        let Some(language) = outline_language(lang) else {
            return 0;
        };

        // Parse before claiming the path so failures leave no `files` entry —
        // `files` then only tracks indexed files and the `files_indexed`
        // recount in `drop_files` stays consistent.
        let Some(tree) = crate::treesitter::parse_with_language(&language, content) else {
            return 0;
        };

        // Atomically check-and-set the mtime and hold the shard entry across
        // the whole update: a concurrent `index_file` on the same path either
        // sees the fresh mtime and skips, or waits — never double-indexes.
        let mut stale = false;
        let mut new_file = false;
        let _files_guard = match self.files.entry(path.to_path_buf()) {
            Entry::Occupied(mut e) => {
                if *e.get() == mtime {
                    return 0;
                }
                e.insert(mtime);
                stale = true;
                e.into_ref()
            }
            Entry::Vacant(e) => {
                new_file = true;
                e.insert(mtime)
            }
        };
        if stale {
            self.remove_path(path);
        }

        let lines: Vec<&str> = content.lines().collect();
        let root = tree.root_node();

        let mut count = 0usize;
        collect_definitions(root, &lines, lang, path, &mut |name, loc| {
            self.insert(name, loc);
            count += 1;
        });

        // `files_indexed` counts distinct files (matching `files.len()`),
        // so re-indexing an existing path does not inflate it.
        if new_file {
            self.files_indexed.fetch_add(1, Ordering::Relaxed);
        }
        self.symbols_indexed.fetch_add(count, Ordering::Relaxed);
        count
    }

    fn insert(&self, name: String, loc: SymbolLocation) {
        self.by_path
            .entry(loc.path.clone())
            .or_default()
            .push(name.clone());
        self.map.entry(name).or_default().push(loc);
    }

    /// Remove every symbol location originating from `path`. Uses the
    /// `by_path` reverse index so only the names defined in this file are
    /// visited instead of scanning the whole map.
    fn remove_path(&self, path: &Path) {
        let Some((_, names)) = self.by_path.remove(path) else {
            return;
        };
        let mut removed = 0usize;
        for name in names {
            let mut drop_entry = false;
            if let Some(mut entry) = self.map.get_mut(&name) {
                let before = entry.value().len();
                entry.value_mut().retain(|loc| loc.path != path);
                removed += before - entry.value().len();
                drop_entry = entry.value().is_empty();
            }
            if drop_entry {
                self.map.remove(&name);
            }
        }
        if removed > 0 {
            self.symbols_indexed.fetch_sub(removed, Ordering::Relaxed);
        }
    }
}

fn collect_definitions(
    node: Node<'_>,
    lines: &[&str],
    lang: Lang,
    path: &Path,
    out: &mut dyn FnMut(String, SymbolLocation),
) {
    // Iterative DFS — avoids stack overflow on deeply-nested trees that the
    // recursive version hit when rayon workers (512 KiB stacks) indexed /tmp
    // (60k+ files, many with deep YAML/JSON nesting).
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if lang == Lang::Elixir && is_elixir_definition(n, lines) {
            if let Some(name) = extract_elixir_definition_name(n, lines) {
                let loc = SymbolLocation {
                    path: path.to_path_buf(),
                    line: n.start_position().row as u32 + 1,
                    end_line: n.end_position().row as u32 + 1,
                    kind: "elixir_def".to_string(),
                    weight: 100,
                };
                out(name, loc);
            }
        } else if DEFINITION_KINDS.contains(&n.kind()) {
            if lang == Lang::Verse && verse_skip_spurious_definition(n, lines) {
                // fall through to children
            } else {
                // `export_statement`/`decorated_definition` are pure wrappers:
                // their name is extracted from a definition child that is
                // itself visited below, so emitting here would duplicate it.
                let is_redundant_wrapper =
                    matches!(n.kind(), "export_statement" | "decorated_definition")
                        && has_definition_child(n);
                if !is_redundant_wrapper {
                    let start_line = n.start_position().row as u32 + 1;
                    let ast_end = n.end_position().row as u32 + 1;
                    let end_line = if lang == Lang::Verse {
                        verse_repair_end_line(n.kind(), start_line, ast_end, lines)
                    } else {
                        ast_end
                    };
                    let weight = definition_weight(n.kind());
                    let kind = n.kind().to_string();
                    for name in extract_definition_names(n, lines) {
                        let loc = SymbolLocation {
                            path: path.to_path_buf(),
                            line: start_line,
                            end_line,
                            kind: kind.clone(),
                            weight,
                        };
                        out(name, loc);
                    }
                }
            }
        }

        // Push children in reverse so first child is processed first
        // (pre-order). Indexed access avoids allocating a `Vec<Node>` per
        // AST node — the old `children().collect()` did that for every node.
        for i in (0..n.child_count()).rev() {
            if let Some(child) = n.child(i) {
                stack.push(child);
            }
        }
    }
}

/// True when `node` has a direct child whose kind is itself a definition —
/// used to detect wrapper nodes (`export_statement`, `decorated_definition`)
/// whose symbol will be emitted when the child is visited.
fn has_definition_child(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    let found = node
        .children(&mut cursor)
        .any(|c| DEFINITION_KINDS.contains(&c.kind()));
    found
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

    #[test]
    fn indexes_verse_definitions() {
        let f = touch_file(
            "MyClass := class:\n    X:int = 0\n\nDoWork():void = Print(\"ok\")\n",
            "verse",
        );
        let idx = SymbolIndex::new();
        let mtime = std::fs::metadata(f.path()).unwrap().modified().unwrap();
        let n = idx.index_file(f.path(), mtime, &std::fs::read_to_string(f.path()).unwrap());
        assert!(n >= 2, "expected class + function symbols, got {n}");
        assert_eq!(idx.lookup_exact("MyClass").len(), 1);
        assert_eq!(idx.lookup_exact("DoWork").len(), 1);
    }

    #[test]
    fn verse_function_span_covers_if_guard_body() {
        let content = r#"game_manager := class(creative_device):
    BindBaseComponentPlots<private>() : void =
        if (Sim := GetGameSim[]):
            Plot.SetPlayerBasesSlot(Idx)
            Comp.BindPlot(Plot)
    OnBegin() : void = {}
"#;
        let f = touch_file(content, "verse");
        let idx = SymbolIndex::new();
        let mtime = std::fs::metadata(f.path()).unwrap().modified().unwrap();
        idx.index_file(f.path(), mtime, content);
        let hits = idx.lookup_exact("BindBaseComponentPlots");
        assert_eq!(hits.len(), 1, "expected BindBaseComponentPlots definition");
        assert_eq!(
            hits[0].line, 2,
            "expected header line 2, got line {}",
            hits[0].line
        );
        assert!(
            hits[0].end_line >= 5,
            "expected span through body lines, got end_line {} (start {})",
            hits[0].end_line,
            hits[0].line
        );
        assert!(
            hits[0].end_line < 6,
            "should stop before sibling OnBegin, got end_line {}",
            hits[0].end_line
        );
    }

    #[test]
    fn export_statement_does_not_duplicate_inner_definition() {
        let f = touch_file("export function foo() {}\n", "ts");
        let idx = SymbolIndex::new();
        let mtime = std::fs::metadata(f.path()).unwrap().modified().unwrap();
        idx.index_file(f.path(), mtime, &std::fs::read_to_string(f.path()).unwrap());
        assert_eq!(
            idx.lookup_exact("foo").len(),
            1,
            "export_statement wrapper + inner def must not double-emit"
        );
    }

    #[test]
    fn decorated_definition_does_not_duplicate_inner_definition() {
        let f = touch_file("@decorator\ndef foo():\n    pass\n", "py");
        let idx = SymbolIndex::new();
        let mtime = std::fs::metadata(f.path()).unwrap().modified().unwrap();
        idx.index_file(f.path(), mtime, &std::fs::read_to_string(f.path()).unwrap());
        assert_eq!(idx.lookup_exact("foo").len(), 1);
    }

    #[test]
    fn variable_declaration_indexes_all_declarators() {
        let f = touch_file("const alpha = 1, beta = 2, gamma = 3;\n", "js");
        let idx = SymbolIndex::new();
        let mtime = std::fs::metadata(f.path()).unwrap().modified().unwrap();
        idx.index_file(f.path(), mtime, &std::fs::read_to_string(f.path()).unwrap());
        for name in ["alpha", "beta", "gamma"] {
            assert_eq!(idx.lookup_exact(name).len(), 1, "missing {name}");
        }
    }

    #[test]
    fn limited_lookups_honor_zero_max_results() {
        let f = touch_file("fn alpha() {}\nfn alphabet() {}\n", "rs");
        let idx = SymbolIndex::new();
        let mtime = std::fs::metadata(f.path()).unwrap().modified().unwrap();
        idx.index_file(f.path(), mtime, &std::fs::read_to_string(f.path()).unwrap());

        assert!(idx.lookup_prefix_limited("alpha", 0).is_empty());
        assert!(idx.lookup_substring_limited("alpha", 0).is_empty());
        assert!(idx.names_limited(0).is_empty());
        assert_eq!(idx.lookup_prefix_limited("alpha", 1).len(), 1);
        assert_eq!(idx.names_limited(1).len(), 1);
    }

    #[test]
    fn non_code_file_leaves_no_files_entry() {
        let f = touch_file("just some text\n", "txt");
        let idx = SymbolIndex::new();
        let mtime = std::fs::metadata(f.path()).unwrap().modified().unwrap();
        let n = idx.index_file(f.path(), mtime, &std::fs::read_to_string(f.path()).unwrap());
        assert_eq!(n, 0);
        assert_eq!(idx.files_indexed(), 0);
        assert!(idx.indexed_paths().is_empty());
        assert!(idx.mtime_for(f.path()).is_none());
    }

    #[test]
    fn reindex_with_new_mtime_replaces_symbols() {
        let f = touch_file("fn old_name() {}\n", "rs");
        let idx = SymbolIndex::new();
        let mtime1 = std::fs::metadata(f.path()).unwrap().modified().unwrap();
        idx.index_file(f.path(), mtime1, "fn old_name() {}\n");
        let mtime2 = mtime1 + Duration::from_secs(1);
        idx.index_file(f.path(), mtime2, "fn new_name() {}\n");
        assert!(idx.lookup_exact("old_name").is_empty());
        assert_eq!(idx.lookup_exact("new_name").len(), 1);
        assert_eq!(idx.files_indexed(), 1);
    }

    #[test]
    fn deeply_nested_tree_does_not_overflow() {
        // Regression for stack overflow on rayon workers (512 KiB stacks) when
        // indexing deeply-nested files under /tmp (60k+ files). The fix makes
        // collect_definitions iterative. This test builds a synthetic deep tree
        // via 800 nested blocks — deep enough to overflow the old recursion.
        let depth = 800;
        let mut content = String::new();
        for i in 0..depth {
            content.push_str(&format!("fn f{i}() {{\n"));
        }
        content.push_str("let x = 1;\n");
        for _ in 0..depth {
            content.push('}');
            content.push('\n');
        }
        let f = touch_file(&content, "rs");
        let idx = SymbolIndex::new();
        let mtime = std::fs::metadata(f.path()).unwrap().modified().unwrap();
        // Must not stack-overflow
        let n = idx.index_file(f.path(), mtime, &content);
        assert!(n >= depth, "expected {depth} fn definitions, got {n}");
        assert_eq!(idx.lookup_exact("f0").len(), 1);
        assert_eq!(idx.lookup_exact(&format!("f{}", depth - 1)).len(), 1);
    }
}
