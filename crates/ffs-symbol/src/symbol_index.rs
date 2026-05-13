//! Concurrent `symbol_name -> Vec<SymbolLocation>` index built from
//! tree-sitter parses across the workspace.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

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
}
