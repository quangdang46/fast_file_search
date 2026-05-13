//! Batch multi-symbol lookup and file scan via AhoCorasick.

use aho_corasick::AhoCorasick;

use crate::symbol_index::{SymbolIndex, SymbolLocation};

/// One symbol's confirmed locations.
#[derive(Debug, Clone)]
pub struct BatchResult {
    pub symbol: String,
    pub locations: Vec<SymbolLocation>,
}

/// Look up many symbols in the index at once. Returns one [`BatchResult`] per
/// name in the same order, deduplicated.
pub fn batch_lookup(index: &SymbolIndex, names: &[&str]) -> Vec<BatchResult> {
    let mut seen = std::collections::HashSet::new();
    names
        .iter()
        .filter(|n| !n.is_empty() && seen.insert(**n))
        .map(|s| BatchResult {
            symbol: s.to_string(),
            locations: index.lookup_exact(s),
        })
        .collect()
}

/// Scan raw bytes for any of `patterns` using AhoCorasick and report which
/// matched. Useful for pre-filtering file contents before expensive work.
pub fn scan_bytes<'a>(patterns: &'a [&'a str], haystack: &[u8]) -> Vec<&'a str> {
    if patterns.is_empty() || haystack.is_empty() {
        return Vec::new();
    }
    let ac = match AhoCorasick::new(patterns) {
        Ok(a) => a,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    let mut found = std::collections::HashSet::new();
    for m in ac.find_iter(haystack) {
        let p = patterns[m.pattern()];
        if found.insert(p) {
            out.push(p);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_lookup_groups_symbols() {
        let idx = SymbolIndex::new();
        let content = "fn alpha() {}\nfn beta() {}\n";
        let tmp = tempfile::NamedTempFile::with_suffix(".rs").unwrap();
        std::fs::write(tmp.path(), content).unwrap();
        let mtime = std::fs::metadata(tmp.path()).unwrap().modified().unwrap();
        idx.index_file(tmp.path(), mtime, content);

        let r = batch_lookup(&idx, &["alpha", "beta", "gamma"]);
        assert_eq!(r.len(), 3);
        assert_eq!(r[0].symbol, "alpha");
        assert_eq!(r[0].locations.len(), 1);
        assert_eq!(r[1].symbol, "beta");
        assert_eq!(r[1].locations.len(), 1);
        assert!(r[2].locations.is_empty());
    }

    #[test]
    fn batch_dedups_duplicates() {
        let idx = SymbolIndex::new();
        let r = batch_lookup(&idx, &["foo", "foo", ""]);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].symbol, "foo");
    }

    #[test]
    fn scan_bytes_finds_patterns() {
        let hay = b"hello world, foo bar baz";
        let mut hits = scan_bytes(&["foo", "qux", "hello"], hay);
        hits.sort();
        assert_eq!(hits, vec!["foo", "hello"]);
    }

    #[test]
    fn scan_bytes_empty_is_noop() {
        assert!(scan_bytes(&[], b"x").is_empty());
        assert!(scan_bytes(&["a"], b"").is_empty());
    }
}
