//! Integration tests for hashline (#L10-20) functionality.
//!
//! Tests use real tmp folders and exercise both the trigger parser
//! (Phase A: ffs-core) and the mention resolver (Phase B: ffs-engine).

use std::fs;
use std::io::Write;
use std::path::PathBuf;
use tempfile::tempdir;

use ffs_engine::mention::{resolve_mentions, MentionKind, MentionResolverCache, ResolveOptions};

// ─── Helpers ────────────────────────────────────────────────────────

fn write_files(spec: &[(&str, &[u8])]) -> (tempfile::TempDir, Vec<PathBuf>) {
    let dir = tempdir().expect("tempdir");
    let mut paths = Vec::new();
    for (name, body) in spec {
        let p = dir.path().join(name);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).expect("mkdir -p");
        }
        let mut f = fs::File::create(&p).expect("create");
        f.write_all(body).expect("write");
        paths.push(p);
    }
    (dir, paths)
}

fn canonical_dir(dir: &tempfile::TempDir) -> PathBuf {
    fs::canonicalize(dir.path()).expect("canonicalize tempdir")
}

/// Build a file with N lines (numbered), each ending with \n.
fn n_lines_file(n: usize) -> String {
    (1..=n).map(|i| format!("line {i}\n")).collect()
}

/// Check that resolved mention has the expected lines.
fn assert_lines_eq(m: &ffs_engine::mention::ResolvedMention, expected: &[&str]) {
    let body = m.content.as_deref().expect("expected content");
    let actual: Vec<&str> = body.trim_end().split('\n').collect();
    assert_eq!(
        actual, expected,
        "line mismatch for path={:?} line_range={:?}",
        m.path, m.line_range
    );
}

// ─── Test suite ─────────────────────────────────────────────────────

#[test]
fn hashline_basic_range() {
    let (dir, paths) = write_files(&[("x.txt", n_lines_file(20).as_bytes())]);
    let opts = ResolveOptions {
        line_range: Some((5, 7)),
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    let m = &out[0];
    assert_lines_eq(m, &["line 5", "line 6", "line 7"]);
    assert_eq!(m.line_range, Some((5, 7)));
    assert!(m.path.starts_with(canonical_dir(&dir)));
}

#[test]
fn hashline_first_lines() {
    let (dir, paths) = write_files(&[("x.txt", n_lines_file(20).as_bytes())]);
    let opts = ResolveOptions {
        line_range: Some((1, 3)),
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    assert_lines_eq(&out[0], &["line 1", "line 2", "line 3"]);
    assert!(out[0].path.starts_with(canonical_dir(&dir)));
}

#[test]
fn hashline_last_lines() {
    let (dir, paths) = write_files(&[("x.txt", n_lines_file(20).as_bytes())]);
    let opts = ResolveOptions {
        line_range: Some((18, 20)),
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    assert_lines_eq(&out[0], &["line 18", "line 19", "line 20"]);
    assert!(out[0].path.starts_with(canonical_dir(&dir)));
}

#[test]
fn hashline_single_line() {
    let (_dir, paths) = write_files(&[("x.txt", n_lines_file(20).as_bytes())]);
    let opts = ResolveOptions {
        line_range: Some((10, 10)),
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    assert_lines_eq(&out[0], &["line 10"]);
}

#[test]
fn hashline_single_line_first() {
    let (_dir, paths) = write_files(&[("x.txt", n_lines_file(20).as_bytes())]);
    let opts = ResolveOptions {
        line_range: Some((1, 1)),
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    assert_lines_eq(&out[0], &["line 1"]);
}

#[test]
fn hashline_single_line_last() {
    let (_dir, paths) = write_files(&[("x.txt", n_lines_file(20).as_bytes())]);
    let opts = ResolveOptions {
        line_range: Some((20, 20)),
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    assert_lines_eq(&out[0], &["line 20"]);
}

#[test]
fn hashline_range_exceeds_file() {
    // line_range beyond file length should clamp to EOF
    let (_dir, paths) = write_files(&[("x.txt", n_lines_file(5).as_bytes())]);
    let opts = ResolveOptions {
        line_range: Some((1, 100)),
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    assert_lines_eq(&out[0], &["line 1", "line 2", "line 3", "line 4", "line 5"]);
}

#[test]
fn hashline_start_beyond_file() {
    // start > EOF => empty content
    let (_dir, paths) = write_files(&[("x.txt", n_lines_file(5).as_bytes())]);
    let opts = ResolveOptions {
        line_range: Some((50, 60)),
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    let body = out[0].content.as_deref().unwrap();
    assert!(
        body.trim().is_empty(),
        "start beyond EOF should give empty content, got: {body:?}"
    );
}

#[test]
fn hashline_start_gt_end_swapped() {
    // Inverted range is now normalized by swapping start<->end,
    // so `#L20-10` on a 20-line file returns lines 10-20.
    let (_dir, paths) = write_files(&[("x.txt", n_lines_file(20).as_bytes())]);
    let opts = ResolveOptions {
        line_range: Some((20, 10)),
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    assert_lines_eq(
        &out[0],
        &[
            "line 10", "line 11", "line 12", "line 13", "line 14", "line 15", "line 16", "line 17",
            "line 18", "line 19", "line 20",
        ],
    );
}

#[test]
fn hashline_empty_file() {
    let (_dir, paths) = write_files(&[("empty.txt", b"")]);
    let opts = ResolveOptions {
        line_range: Some((1, 1)),
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    let m = &out[0];
    assert_eq!(m.kind, MentionKind::File);
    assert!(!m.is_binary);
    // Empty file with line range should still produce content (empty string)
    assert_eq!(m.content.as_deref(), Some(""));
}

#[test]
fn hashline_single_line_file_range() {
    // 1-line file with various ranges
    let (_dir, paths) = write_files(&[("single.txt", b"the only line\n")]);

    let opts = ResolveOptions {
        line_range: Some((1, 1)),
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    assert_lines_eq(&out[0], &["the only line"]);

    let opts = ResolveOptions {
        line_range: Some((1, 5)),
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    assert_lines_eq(&out[0], &["the only line"]);
}

#[test]
fn hashline_no_trailing_newline() {
    // File without trailing \n
    let (_dir, paths) = write_files(&[("x.txt", b"line 1\nline 2\nline 3")]);
    let opts = ResolveOptions {
        line_range: Some((2, 3)),
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    assert_lines_eq(&out[0], &["line 2", "line 3"]);
}

#[test]
fn hashline_with_truncation() {
    // line_range applied BEFORE truncation. So slicing first should
    // give a smaller body that doesn't need truncation, or at most
    // truncation applies to the already-sliced body.
    let body: String = (1..=200).map(|i| format!("line {i}\n")).collect();
    let (_dir, paths) = write_files(&[("big.txt", body.as_bytes())]);
    let opts = ResolveOptions {
        line_range: Some((1, 3)),
        max_tokens: 10, // very small budget
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    let m = &out[0];
    let c = m.content.as_deref().unwrap();
    assert!(
        c.contains("line 1") || c.contains("more lines]"),
        "expected at least line 1 or truncation footer: {c}"
    );
    // Should NEVER contain line 100 (line range was 1-3)
    assert!(
        !c.contains("line 100"),
        "line 100 should be excluded by range"
    );
}

#[test]
fn hashline_with_filter() {
    // line_range + Aggressive filter: strip comments from the sliced range
    let body = b"/// keep me\n// strip me\nfn x() {}\n// more\n";
    let (_dir, paths) = write_files(&[("x.rs", &body[..])]);
    let opts = ResolveOptions {
        line_range: Some((1, 3)),
        filter_level: ffs_budget::FilterLevel::Aggressive,
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    let m = &out[0];
    let c = m.content.as_deref().unwrap();
    // Line 1 (///) is stripped by aggressive, line 2 (//) stripped, line 3 kept
    assert!(!c.contains("keep me"));
    assert!(!c.contains("strip me"));
    assert!(c.contains("fn x"));
}

#[test]
fn hashline_line_range_none_resolves_full() {
    // line_range=None should return the full file content
    let (_dir, paths) = write_files(&[("x.txt", n_lines_file(10).as_bytes())]);
    let opts = ResolveOptions::default();
    let out = resolve_mentions(&paths, &opts);
    let m = &out[0];
    assert_eq!(m.line_range, None);
    let body = m.content.as_deref().unwrap();
    assert!(body.contains("line 1"));
    assert!(body.contains("line 10"));
}

#[test]
fn hashline_dedup_same_range() {
    // Same path + same line_range resolved twice in same turn uses cache
    let (_dir, paths) = write_files(&[("a.txt", n_lines_file(20).as_bytes())]);
    let p = paths[0].clone();
    let opts = ResolveOptions {
        line_range: Some((5, 10)),
        ..ResolveOptions::default()
    };
    let mut cache = MentionResolverCache::new();
    let r1 = cache.resolve_cached(&p, &opts, 1);
    let r2 = cache.resolve_cached(&p, &opts, 1);
    assert_eq!(r1.content, r2.content);
}

#[test]
fn hashline_different_ranges_different_cache_entries() {
    // FIX: cache key now includes line_range, so different ranges
    // for the same path produce different cache entries.
    let (_dir, paths) = write_files(&[("a.txt", n_lines_file(20).as_bytes())]);
    let p = paths[0].clone();
    let mut cache = MentionResolverCache::new();
    let opts1 = ResolveOptions {
        line_range: Some((1, 5)),
        ..ResolveOptions::default()
    };
    let opts2 = ResolveOptions {
        line_range: Some((10, 15)),
        ..ResolveOptions::default()
    };
    let r1 = cache.resolve_cached(&p, &opts1, 1);
    let r2 = cache.resolve_cached(&p, &opts2, 1);
    // r1 has lines 1-5
    assert_lines_eq(&r1, &["line 1", "line 2", "line 3", "line 4", "line 5"]);
    // r2 has lines 10-15 (NOT the cached r1 content)
    assert_lines_eq(
        &r2,
        &[
            "line 10", "line 11", "line 12", "line 13", "line 14", "line 15",
        ],
    );
    // Two distinct cache entries (one per line_range)
    assert_eq!(
        cache.len(),
        2,
        "different line_ranges should be 2 cache entries"
    );
}

#[test]
fn hashline_batch_multiple_ranges() {
    // Resolve multiple paths with different line_ranges in one batch call
    let mut spec = Vec::new();
    for i in 0..5 {
        spec.push((
            format!("file_{i}.txt"),
            n_lines_file(100).as_bytes().to_vec(),
        ));
    }
    let file_specs: Vec<(&str, &[u8])> = spec
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .collect();
    let (dir, paths) = write_files(&file_specs);

    // Each file gets a different line_range
    let results: Vec<_> = paths
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let start = (i * 10 + 1) as u32;
            let end = (i * 10 + 5) as u32;
            let opts = ResolveOptions {
                line_range: Some((start, end)),
                ..ResolveOptions::default()
            };
            resolve_mentions(&[p.clone()], &opts)
        })
        .collect();

    for (i, res) in results.iter().enumerate() {
        let m = &res[0];
        assert_eq!(m.kind, MentionKind::File);
        let body = m.content.as_deref().unwrap();
        let start_line = i * 10 + 1;
        let end_line = i * 10 + 5;
        assert!(
            body.contains(&format!("line {}", start_line)),
            "file_{i} should contain line {start_line}"
        );
        assert!(
            body.contains(&format!("line {}", end_line)),
            "file_{i} should contain line {end_line}"
        );
        assert!(m.path.starts_with(canonical_dir(&dir)));
    }
}

#[test]
fn hashline_with_crlf() {
    // Windows-style line endings
    let body: String = (1..=10).map(|i| format!("line {i}\r\n")).collect();
    let (_dir, paths) = write_files(&[("crlf.txt", body.as_bytes())]);
    let opts = ResolveOptions {
        line_range: Some((3, 5)),
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    let m = &out[0];
    let c = m.content.as_deref().unwrap();
    assert!(c.contains("line 3"));
    assert!(c.contains("line 4"));
    assert!(c.contains("line 5"));
    assert!(!c.contains("line 2"));
    assert!(!c.contains("line 6"));
}

#[test]
fn hashline_preserves_content_order() {
    // Verify that content is in the correct order when slicing
    let body = b"first\nsecond\nthird\nfourth\nfifth\n";
    let (_dir, paths) = write_files(&[("ordered.txt", &body[..])]);
    let opts = ResolveOptions {
        line_range: Some((2, 4)),
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    let c = out[0].content.as_deref().unwrap();
    let lines: Vec<&str> = c.trim().split('\n').collect();
    assert_eq!(lines, &["second", "third", "fourth"]);
}

#[test]
fn hashline_very_large_range_small_file() {
    // Small file (3 lines) with huge range request
    let (_dir, paths) = write_files(&[("small.txt", b"a\nb\nc\n")]);
    let opts = ResolveOptions {
        line_range: Some((1, 999_999)),
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    assert_lines_eq(&out[0], &["a", "b", "c"]);
}

#[test]
fn hashline_start_zero_is_ignored() {
    // start=0 is technically invalid (1-based), but should not crash.
    // The guard in resolve_text is `start > 0`, so this falls through to
    // full content without slicing.
    let body = n_lines_file(10);
    let (_dir, paths) = write_files(&[("x.txt", body.as_bytes())]);
    let opts = ResolveOptions {
        line_range: Some((0, 5)),
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    let m = &out[0];
    // guard `start > 0` fails → full content is returned (no slice)
    let c = m.content.as_deref().unwrap();
    assert!(
        c.contains("line 1"),
        "start=0 should fall through to full content, got: {c:?}"
    );
}

#[test]
fn hashline_unicode_content() {
    // Unicode content within line range
    let body = "α line\nβ line\nγ line\nδ line\nε line\n";
    let (_dir, paths) = write_files(&[("unicode.txt", body.as_bytes())]);
    let opts = ResolveOptions {
        line_range: Some((2, 4)),
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    assert_lines_eq(&out[0], &["β line", "γ line", "δ line"]);
}

#[test]
fn hashline_very_long_lines() {
    // Very long line that exceeds token budget after slicing
    let long_line = "A".repeat(10_000);
    let mut body = String::new();
    for i in 1..=10 {
        body.push_str(&format!("{long_line} // line {i}\n"));
    }
    let (_dir, paths) = write_files(&[("long.txt", body.as_bytes())]);
    let opts = ResolveOptions {
        line_range: Some((3, 7)),
        max_tokens: 500,
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    let m = &out[0];
    let c = m.content.as_deref().unwrap();
    // Should have some content from lines 3-7, possibly truncated
    assert!(m.truncation.is_some() || c.len() < 50_000);
}

#[test]
fn hashline_kind_is_file_not_image() {
    // Even with line_range, a file should still be a File, not Image
    let (_dir, paths) = write_files(&[("data.png", b"not a real png\nsecond line\n")]);
    let opts = ResolveOptions {
        line_range: Some((1, 1)),
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    // .png extension → kind=Image → content=None
    // line_range doesn't apply to images
    let m = &out[0];
    assert_eq!(m.kind, MentionKind::Image);
    assert!(m.content.is_none());
}

#[test]
fn hashline_nonexistent_path_returns_error() {
    let bogus = PathBuf::from("/does/not/exist/hashline_test.txt");
    let opts = ResolveOptions {
        line_range: Some((1, 10)),
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&[bogus.clone()], &opts);
    let m = &out[0];
    assert_eq!(m.kind, MentionKind::File);
    assert!(m.content.is_none());
    assert!(m.audit.error.is_some());
    assert_eq!(m.path, bogus);
}

#[test]
fn hashline_file_with_only_newlines() {
    // File containing only \n characters
    let body = "\n\n\n\n\n"; // 5 newlines
    let (_dir, paths) = write_files(&[("blank.txt", body.as_bytes())]);
    let opts = ResolveOptions {
        line_range: Some((2, 4)),
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    // split('\n') on "\n\n\n\n\n" gives ["", "", "", "", "", ""] (6 elements)
    // Actually... let me check: "\n\n\n\n\n".split('\n') → ["", "", "", "", "", ""]
    // length = 6
    // s = 1, e = 4 → lines[1..4] = ["", "", ""]
    // join "\n" → "\n\n"
    // After trim -> ""
    let c = out[0].content.as_deref().unwrap();
    // Just verify it doesn't panic
    assert!(
        c.len() <= 3,
        "expected at most 3 newlines, got len={}",
        c.len()
    );
}

#[test]
fn hashline_different_ranges_same_path_no_cache_cross_talk() {
    // When line_range is not part of cache key, ensuring no cache cross-talk
    // across different turns
    let (_dir, paths) = write_files(&[("a.txt", n_lines_file(50).as_bytes())]);
    let p = paths[0].clone();
    let mut cache = MentionResolverCache::new();

    // Turn 1: range (1, 5)
    let opts1 = ResolveOptions {
        line_range: Some((1, 5)),
        ..ResolveOptions::default()
    };
    let r1 = cache.resolve_cached(&p, &opts1, 1);
    assert_lines_eq(&r1, &["line 1", "line 2", "line 3", "line 4", "line 5"]);

    // Turn 2: range (10, 15) — different turn, fresh cache
    let opts2 = ResolveOptions {
        line_range: Some((10, 15)),
        ..ResolveOptions::default()
    };
    let r2 = cache.resolve_cached(&p, &opts2, 2);
    assert_lines_eq(
        &r2,
        &[
            "line 10", "line 11", "line 12", "line 13", "line 14", "line 15",
        ],
    );
}

#[test]
fn hashline_ten_files_different_ranges_batch() {
    // Stress test: 10 files, each with a different line_range
    // Enough to find overflow / off-by-one / boundary bugs
    let n_files = 10;
    let lines_per_file = 50;

    let mut specs = Vec::new();
    for i in 0..n_files {
        let name = format!("f{i}.rs");
        let body: String = (1..=lines_per_file)
            .map(|j| format!("// line {j} of file {i}\n"))
            .collect();
        specs.push((name, body.into_bytes()));
    }
    let spec_refs: Vec<(&str, &[u8])> = specs
        .iter()
        .map(|(n, b)| (n.as_str(), b.as_slice()))
        .collect();
    let (dir, paths) = write_files(&spec_refs);

    let results: Vec<_> = paths
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let start = (i * 3 + 2) as u32;
            let end = (i * 3 + 6) as u32;
            let opts = ResolveOptions {
                line_range: Some((start, end.min(lines_per_file as u32))),
                max_tokens: 1000,
                filter_level: ffs_budget::FilterLevel::None,
                ..ResolveOptions::default()
            };
            resolve_mentions(&[p.clone()], &opts)
        })
        .collect();

    for (i, res) in results.iter().enumerate() {
        let m = &res[0];
        let body = m.content.as_deref().unwrap();
        let start = i * 3 + 2;
        let end = (i * 3 + 6).min(lines_per_file);
        for line in start..=end {
            assert!(
                body.contains(&format!("line {line} of file {i}")),
                "f{i}.rs should contain 'line {line} of file {i}'"
            );
        }
        assert!(m.path.starts_with(canonical_dir(&dir)));
    }
}

#[test]
fn hashline_content_without_newline_at_end_still_counts() {
    // File ending without \n on the last line. split('\n') gives
    // [..., "last line"] — last line is counted.
    let body = "a\nb\nc\nd"; // 4 lines, d has no trailing newline
    let (_dir, paths) = write_files(&[("x.txt", body.as_bytes())]);
    let opts = ResolveOptions {
        line_range: Some((3, 4)),
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    assert_lines_eq(&out[0], &["c", "d"]);
}

// Trigger parser tests live in ffs-core's own unit tests
// (mention::trigger::detect_trigger). Here we only test the
// integration with the resolver, which is what host apps care about.

#[test]
fn hashline_trailing_newline_preserved() {
    // When a file ends with \n, slice_lines now preserves the trailing
    // newline if the last kept line is the last line of the file.
    let body = "a\nb\nc\n"; // 3 lines with trailing newline
    let (_dir, paths) = write_files(&[("nl.txt", body.as_bytes())]);
    let opts = ResolveOptions {
        line_range: Some((1, 3)),
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    let c = out[0].content.as_deref().unwrap();
    // Fix: trailing newline preserved when slice reaches EOF.
    assert_eq!(
        c.as_bytes().last().copied(),
        Some(b'\n'),
        "trailing newline should be preserved: {c:?}"
    );
    assert_eq!(c, "a\nb\nc\n");
}

#[test]
fn hashline_trailing_newline_not_added_if_no_eof() {
    // If the last kept line is NOT the file's last line, no trailing newline.
    let body = "a\nb\nc\nd\n";
    let (_dir, paths) = write_files(&[("partial.txt", body.as_bytes())]);
    let opts = ResolveOptions {
        line_range: Some((1, 2)),
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    let c = out[0].content.as_deref().unwrap();
    // Lines 1-2 of "a\nb\nc\nd\n" → "a\nb" (no trailing newline)
    assert_eq!(c, "a\nb");
}

#[test]
fn hashline_no_trailing_newline_stays_clean() {
    // File WITHOUT trailing newline → output should NOT add one.
    let body = "a\nb\nc"; // 3 lines, no trailing newline
    let (_dir, paths) = write_files(&[("clean.txt", body.as_bytes())]);
    let opts = ResolveOptions {
        line_range: Some((1, 3)),
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    let c = out[0].content.as_deref().unwrap();
    assert_eq!(c, "a\nb\nc");
    assert_eq!(
        c.as_bytes().last().copied(),
        Some(b'c'),
        "no trailing newline when input has none: {c:?}"
    );
}

#[test]
fn hashline_inverted_range_now_swaps() {
    // After fix: inverted range (8, 3) is swapped to (3, 8) → returns lines 3-8.
    let (_dir, paths) = write_files(&[("x.txt", n_lines_file(10).as_bytes())]);
    let opts = ResolveOptions {
        line_range: Some((8, 3)),
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    let m = &out[0];
    let c = m.content.as_deref().unwrap();
    assert!(
        !c.trim().is_empty(),
        "inverted range should swap, not return empty"
    );
    assert!(c.contains("line 3"), "should contain line 3 (swapped): {c}");
    assert!(c.contains("line 8"), "should contain line 8 (swapped): {c}");
    assert!(!c.contains("line 1"), "should NOT contain line 1: {c}");
}

#[test]
fn hashline_start_zero_normalizes_to_one() {
    // start=0 is invalid (1-based) → now clamped to 1, slices lines 1-3.
    let (_dir, paths) = write_files(&[("x.txt", n_lines_file(10).as_bytes())]);
    let opts = ResolveOptions {
        line_range: Some((0, 3)),
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    let m = &out[0];
    let c = m.content.as_deref().unwrap();
    // Clamped to (1, 3) → lines 1-3, NOT full file
    assert!(c.contains("line 1"), "should contain line 1: {c}");
    assert!(c.contains("line 3"), "should contain line 3: {c}");
    assert!(!c.contains("line 10"), "should NOT contain line 10: {c}");
    assert!(m.audit.error.is_none());
}

#[test]
fn hashline_both_zero_normalizes_to_one() {
    // (0, 0) → clamps to (1, 1) → returns line 1 only.
    let (_dir, paths) = write_files(&[("x.txt", n_lines_file(10).as_bytes())]);
    let opts = ResolveOptions {
        line_range: Some((0, 0)),
        ..ResolveOptions::default()
    };
    let out = resolve_mentions(&paths, &opts);
    let c = out[0].content.as_deref().unwrap();
    assert_eq!(c.trim(), "line 1", "expected only line 1: {c}");
}
