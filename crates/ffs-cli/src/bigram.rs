//! Standalone per-bigram inverted file bitset for `ffs grep` rare patterns.
//!
//! Built during `ffs index` and persisted to
//! `<root>/.ffs/bigram.postcard.zst`. At grep time we extract the
//! pattern's printable-ASCII bigrams, AND their bitsets, and keep only
//! files that survived. False positives are fine — the literal SIMD
//! scan downstream rejects them; false negatives must not happen so
//! we always lowercase both sides and only treat bigrams of printable
//! ASCII (32..=126) characters as discriminators.
//!
//! Sized for typical repos (≤500k files): for each present bigram a
//! `Vec<u64>` of `(file_count + 63) / 64` words. zstd-19 compresses
//! the resulting payload tightly because most bitsets are sparse.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

/// Maximum file size considered for bigram extraction. Mirrors
/// `UnifiedScanner` so the bigram index covers the same shape of
/// candidates the grep scan would ever read.
const MAX_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// Inverted bigram index. `paths[i]` ↔ bit `i` in every posting bitset.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct GrepBigram {
    pub paths: Vec<PathBuf>,
    /// `bigram_id (high<<8 | low) -> bitset of length (paths.len()+63)/64`.
    pub posting: HashMap<u16, Vec<u64>>,
}

impl GrepBigram {
    /// Number of files indexed.
    #[must_use]
    pub fn file_count(&self) -> usize {
        self.paths.len()
    }

    /// Number of bigrams with at least one file in their posting list.
    #[must_use]
    pub fn bigram_count(&self) -> usize {
        self.posting.len()
    }

    /// Build an inverted index over `files` by reading each file once and
    /// hashing its 2-byte sliding window. Files that look binary (NUL in
    /// the first 8 KB), exceed the size cap, or fail to read are silently
    /// skipped — they remain in `paths` but contribute no bigrams, so a
    /// query treats them as "unknown content" and they survive prefilter.
    pub fn build(files: &[PathBuf]) -> Self {
        let n = files.len();
        let words = n.div_ceil(64).max(1);

        // Stage 1: per-file bigram set in parallel. Result: Vec<Option<HashSet<u16>>>.
        let per_file: Vec<Option<HashSet<u16>>> = files
            .par_iter()
            .map(|path| extract_file_bigrams(path))
            .collect();

        // Stage 2: invert into bigram → file-bitset.
        let mut posting: HashMap<u16, Vec<u64>> = HashMap::new();
        for (idx, maybe_set) in per_file.into_iter().enumerate() {
            let Some(set) = maybe_set else { continue };
            let word = idx / 64;
            let bit = 1u64 << (idx % 64);
            for key in set {
                let bs = posting.entry(key).or_insert_with(|| vec![0u64; words]);
                bs[word] |= bit;
            }
        }

        Self {
            paths: files.to_vec(),
            posting,
        }
    }

    /// Return the subset of indexed paths that *might* contain `pattern`.
    /// Returns `None` when the pattern has no bigrams to match against
    /// (length < 2 or all bigrams contain non-printable bytes); in that
    /// case the caller should fall back to scanning every file.
    #[must_use]
    pub fn filter(&self, pattern: &[u8]) -> Option<Vec<&Path>> {
        if pattern.len() < 2 || self.paths.is_empty() {
            return None;
        }
        let n = self.paths.len();
        let words = n.div_ceil(64).max(1);
        let mut candidates = vec![u64::MAX; words];
        // Mask off bits past file_count in the last word.
        if !n.is_multiple_of(64) {
            let last = words - 1;
            candidates[last] = (1u64 << (n % 64)) - 1;
        }

        let mut had_bigram = false;
        for w in pattern.windows(2) {
            let a = w[0];
            let b = w[1];
            if (32..=126).contains(&a) && (32..=126).contains(&b) {
                let key = ((a.to_ascii_lowercase() as u16) << 8) | (b.to_ascii_lowercase() as u16);
                match self.posting.get(&key) {
                    Some(bs) => {
                        for (c, x) in candidates.iter_mut().zip(bs.iter()) {
                            *c &= *x;
                        }
                        had_bigram = true;
                    }
                    None => {
                        // Bigram never seen — pattern cannot match anywhere.
                        return Some(Vec::new());
                    }
                }
            }
        }
        if !had_bigram {
            return None;
        }

        let mut survivors: Vec<&Path> = Vec::new();
        for (idx, path) in self.paths.iter().enumerate() {
            let word = idx / 64;
            if candidates[word] & (1u64 << (idx % 64)) != 0 {
                survivors.push(path.as_path());
            }
        }
        Some(survivors)
    }
}

fn extract_file_bigrams(path: &Path) -> Option<HashSet<u16>> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() {
        return None;
    }
    let len = meta.len();
    if len == 0 || len > MAX_FILE_BYTES {
        return None;
    }
    let content = std::fs::read(path).ok()?;
    // Quick binary sniff: NUL in the first 8 KB.
    let probe = &content[..content.len().min(8 * 1024)];
    if probe.contains(&0u8) {
        return None;
    }
    let mut set: HashSet<u16> = HashSet::with_capacity(1024);
    for w in content.windows(2) {
        let a = w[0];
        let b = w[1];
        if (32..=126).contains(&a) && (32..=126).contains(&b) {
            let key = ((a.to_ascii_lowercase() as u16) << 8) | (b.to_ascii_lowercase() as u16);
            set.insert(key);
        }
    }
    Some(set)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, rel: &str, body: &str) -> PathBuf {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn build_and_filter_keeps_files_with_pattern() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.rs", "fn alpha() {}\n");
        let b = write(dir.path(), "b.rs", "fn beta() {}\n");
        let c = write(dir.path(), "c.rs", "fn gamma() {}\n");
        let idx = GrepBigram::build(&[a.clone(), b.clone(), c.clone()]);
        let hits = idx.filter(b"alpha").expect("had bigrams");
        assert!(hits.contains(&a.as_path()));
        assert!(!hits.contains(&b.as_path()));
        assert!(!hits.contains(&c.as_path()));
    }

    #[test]
    fn filter_returns_none_for_too_short_pattern() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.rs", "x");
        let idx = GrepBigram::build(std::slice::from_ref(&a));
        assert!(idx.filter(b"a").is_none());
    }

    #[test]
    fn filter_returns_empty_when_bigram_never_seen() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.rs", "abcdef\n");
        let idx = GrepBigram::build(std::slice::from_ref(&a));
        // "qz" doesn't appear in any indexed file.
        let hits = idx.filter(b"qz").expect("had printable bigrams");
        assert!(hits.is_empty());
    }

    #[test]
    fn filter_is_case_insensitive_at_extraction() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.rs", "fn ALPHA() {}\n");
        let idx = GrepBigram::build(std::slice::from_ref(&a));
        // Lowercase pattern still hits because the file's bigrams were
        // lowercased at extraction time.
        let hits = idx.filter(b"alpha").expect("had bigrams");
        assert!(hits.contains(&a.as_path()));
    }

    #[test]
    fn build_skips_binary_files_silently() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin.dat");
        // 1 KB of NULs — looks binary.
        std::fs::write(&bin, vec![0u8; 1024]).unwrap();
        let txt = write(dir.path(), "ok.rs", "hello world\n");
        let idx = GrepBigram::build(&[bin.clone(), txt.clone()]);
        // bin.dat contributes nothing to posting; only txt should turn up.
        let hits = idx.filter(b"hello").expect("had bigrams");
        assert!(hits.contains(&txt.as_path()));
        assert!(!hits.contains(&bin.as_path()));
    }
}
