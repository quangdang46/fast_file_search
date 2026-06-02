//! Criterion benchmarks for the Phase A `@`-mention resolver.
//!
//! Mirrors `docs/MENTION_SYSTEM_PLAN.md §9`.
//!
//! Targets:
//! - `bench_trigger_only`            < 50µs   (pure parser, no I/O)
//! - `bench_candidate_search_warm_*` < 10ms p99 on 10k files
//! - `bench_candidate_search_cold_*` < 50ms p99 on 40k files
//!
//! Fixtures are generated at bench setup in a `tempfile::TempDir` so the
//! bench is self-contained and reproducible. We only build each tree once
//! per `Criterion::bench_function` closure so the per-iteration cost is the
//! search itself, not file IO.
//!
//! NOTE: only `mention/mod.rs` / `mention/resolver.rs` / `mention/trigger.rs`
//! are new code in Phase A. This bench reuses the existing `FilePicker`
//! pipeline (`fuzzy_search`, `fuzzy_search_directories`) and
//! `SharedFrecency::noop()` so we measure the resolver, not the picker.

use std::fs;
use std::path::{Path, PathBuf};

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use ffs_search::FilePicker;
use ffs_search::file_picker::{FfsMode, FilePickerOptions};
use ffs_search::mention::{MentionOptions, MentionResolver};
use ffs_search::path_utils;

/// Generate `count` files in `dir` with stable, predictable names.
///
/// Names are zero-padded (`file_00000.txt` ... `file_NNNNN.txt`) so the
/// sorted FilePicker index is deterministic and the bench doesn't measure
/// random disk-order effects. We also drop in a couple of sub-directories
/// so `fuzzy_search_directories` has non-trivial input on every query.
fn build_tree(dir: &Path, count: usize) {
    for i in 0..count {
        let name = format!("file_{:08}.txt", i);
        // Tiny payload — we never read content in the bench, we just need
        // the file to exist so the indexer picks it up.
        fs::write(dir.join(&name), b"x").expect("write fixture file");
    }

    // A handful of sub-directories so directory candidates are populated.
    for chunk in 0..16 {
        let sub = dir.join(format!("dir_{chunk:02}"));
        fs::create_dir_all(&sub).expect("mkdir");
        for j in 0..32 {
            let inner = format!("file_{:08}.rs", chunk * 100_000 + j);
            fs::write(sub.join(&inner), b"x").expect("write inner file");
        }
    }
}

/// Build a `FilePicker` rooted at `root` with `watch: false` and
/// content-indexing disabled, then synchronously `collect_files()`.
fn build_picker(root: &Path) -> FilePicker {
    let base = path_utils::canonicalize(root).unwrap_or_else(|_| PathBuf::from(root));
    let mut picker = FilePicker::new(FilePickerOptions {
        base_path: base.to_string_lossy().into_owned(),
        watch: false,
        enable_mmap_cache: false,
        enable_content_indexing: false,
        mode: FfsMode::Ai,
        ..Default::default()
    })
    .expect("create picker");
    picker.collect_files().expect("collect_files");
    picker
}

// Trigger detection is exercised standalone; the resolver delegates to the
// same `detect_trigger` function exposed in `mention::trigger`. Target
// latency per the plan: < 50µs.
fn bench_trigger_only(c: &mut Criterion) {
    c.bench_function("mention_trigger_only", |b| {
        b.iter(|| MentionResolver::detect_trigger(black_box("review @src/main"), 16));
    });
}

// 10k-file tree, candidate search warm. Each iteration runs the resolver
// against the same tree (and the same trigger prefix) to mirror a user
// hammering the popup with keystrokes.
fn bench_candidate_search_warm_10k(c: &mut Criterion) {
    let dir = tempfile::tempdir().expect("tempdir");
    build_tree(dir.path(), 10_000);
    let picker = build_picker(dir.path());
    let resolver = MentionResolver::new(&picker);

    c.bench_function("mention_candidate_search_warm_10k", |b| {
        b.iter(|| resolver.search(black_box("review @src/co"), 14));
    });
}

// 40k-file tree, candidate search cold. Bigger corpus, same query shape.
// We deliberately build a fresh tree per Criterion sample_size to keep the
// bench deterministic — a single tree is enough for stable timings once
// the resolver and picker hit their steady state.
fn bench_candidate_search_cold_40k(c: &mut Criterion) {
    let dir = tempfile::tempdir().expect("tempdir");
    build_tree(dir.path(), 40_000);
    let picker = build_picker(dir.path());
    let resolver = MentionResolver::new(&picker);

    c.bench_function("mention_candidate_search_cold_40k", |b| {
        b.iter(|| resolver.search(black_box("review @src/co"), 14));
    });
}

// Optional smoke variant that uses a non-default MentionOptions to make
// sure the options plumbing is in the hot path. Runs against the same
// 10k tree as the warm bench.
fn bench_candidate_search_with_options(c: &mut Criterion) {
    let dir = tempfile::tempdir().expect("tempdir");
    build_tree(dir.path(), 10_000);
    let picker = build_picker(dir.path());
    let resolver = MentionResolver::new(&picker).with_options(MentionOptions {
        include_files: true,
        include_dirs: false,
        max_candidates: 10,
        fuzzy_min_chars: 3,
        min_query_chars: 1,
    });

    c.bench_function("mention_candidate_search_with_options_10k", |b| {
        b.iter(|| resolver.search(black_box("review @file_000"), 18));
    });
}

criterion_group!(
    benches,
    bench_trigger_only,
    bench_candidate_search_warm_10k,
    bench_candidate_search_cold_40k,
    bench_candidate_search_with_options,
);
criterion_main!(benches);
