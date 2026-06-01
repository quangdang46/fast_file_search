# Implementation Plan: Faster FilePicker Init for Interactive Pickers

> Generated via the feature-planning skill (reference-repo research + measurement).
> Goal: make `ffs_search::FilePicker` usable for `@`-path autocomplete with
> sub-100ms time-to-first-result, even on large repos and cold caches.

---

## 1. Executive Summary

jcode wraps `ffs_search::FilePicker` for `@`-path autocomplete (`jcode-tui/.../at_picker.rs`).
`AtPicker::search()` returns **empty** until `FilePicker`'s background scan finishes
(`is_scan_active() == false`). The scan's readiness flips only after the *entire*
filesystem walk completes, so time-to-first-result scales linearly with file count and
is dominated by cold-cache `stat` I/O.

Two improvements:

1. **(Shipped, jcode side)** Warm up the picker eagerly at the start of the run loop
   instead of lazily on the first `@` keystroke — the scan now overlaps session startup.
2. **(ffs side, this plan)** Reduce the scan's own time-to-ready via (A) incremental
   readiness (serve partial results during the walk) and/or (B) skipping the per-file
   `stat` for names-only pickers.

---

## 2. Measurements (baseline)

Harness: `FilePicker::new_with_shared_state` with jcode's exact config
(`mode: Ai, enable_mmap_cache: false, enable_content_indexing: false, watch: true`),
polling `is_scan_active()` to first-ready. Linux x86-64.

| Repo | Files | Time-to-ready (cold) | Time-to-ready (warm) |
|------|------:|---------------------:|---------------------:|
| jcode (`target/` gitignored) | 2,422 | ~538 ms | 31–71 ms |
| synthetic | 40,000 | ~1.18 s | ~320 ms |
| ffs itself | 186 | ~128 ms | <40 ms |

Findings:
- `target/` is correctly excluded (gitignore-aware walk) — not the problem.
- `git status` (~520 ms on jcode) runs **after** the readiness flip
  (`scan.rs`: `signals.scanning.store(false)` precedes git-status apply), so it does
  **not** gate `search()`.
- Readiness is gated entirely by `FileSync::walk_filesystem` (the parallel walk +
  two `par_sort`s + a serial dir-extraction pass). Cost is ~linear in file count and
  cold-dominated by one `entry.metadata()` (`stat`) syscall per file.

---

## 3. Architecture Decision

### Chosen approach (layered, ship in order)

1. **jcode eager warm-up** — zero ffs change, immediately effective on `ffs-search 0.1.3`.
2. **ffs Option B: skip per-file `stat` for names-only pickers** — biggest single win
   on large repos (40k warm: 320 ms → ~91 ms measured in a prototype), but requires
   making grep tolerant of unknown size (see §5.2). Medium risk.
3. **ffs Option A: incremental readiness** — the robust long-term win; serve partial
   results while the walk runs so time-to-first-result is constant regardless of repo
   size. Largest change; deferred behind a benchmark gate.

### Alternatives considered

| Approach | Pros | Cons | Decision |
|----------|------|------|----------|
| jcode eager warm-up | Trivial, no ffs release needed | Doesn't lower scan cost; just hides it behind startup | **Ship now** |
| Skip `stat` for names-only | 3.5× faster warm walk on 40k | `file.size` is load-bearing in grep filtering (`prepare_files_to_search` filters `f.size > 0`); needs grep changes | **Plan / gated** |
| Incremental readiness | Constant time-to-first-result; scales to 500k | Walk currently commits once into an arena-backed store; partial commit is non-trivial | **Plan / gated** |
| Make git status block readiness off | n/a | Already off the readiness path | Rejected (no-op) |

Research signal (feature-planning reference repos): the proven pattern for interactive
file finders is **stream results as the walk progresses** (fzf/telescope/ripgrep), and
`pi-agent-rust` sets the bar at `<100ms` startup via prewarm — both point at Option A
(stream) + Option 1 (prewarm) rather than a faster all-or-nothing walk.

---

## 4. Pseudocode

### Option B — skip stat for names-only pickers
```
walk_filesystem(..., collect_metadata: bool):
  for each file entry (parallel):
    if collect_metadata: md = stat(entry)      # size + mtime
    else:                md = None              # size defaults to 0
    push FileItem::new_from_walk(path, base, None, md)

# callers:
#   ScanJob.run:     collect_metadata = config.warmup || config.content_indexing
#   collect_files:   collect_metadata = enable_mmap_cache || enable_content_indexing
```

### grep must tolerate unknown size (the prerequisite that makes B safe)
```
prepare_files_to_search:
  keep file if: !deleted && !binary && size <= max && (size > 0 || size_unknown)
        # i.e. don't drop a file just because size==0 when metadata was skipped

get_content_for_search:
  if size == 0:                       # unknown OR genuinely empty
     read_to_end(file) into buf       # handles both; empty -> None
  else:
     existing fast paths (cache / fresh-mmap / read_exact)
```

### Option A — incremental readiness (sketch)
```
ScanJob.run:
  walk in batches; every N files OR T ms:
     commit a searchable snapshot of files-so-far
     flip scanning=false after the FIRST batch    # search() now returns partial
  after full walk: final commit (complete set), keep watcher/git-status as today
```

---

## 5. Implementation Notes

### 5.1 jcode eager warm-up (DONE)
`crates/jcode-tui/src/tui/app/run_shell.rs` — at the top of `run()` and `run_remote()`:
```rust
if let Some(dir) = self.session.working_dir.as_deref() {
    let _ = self.at_picker.borrow_mut().ensure(Some(dir));
}
```
`ensure()` is non-blocking + idempotent; the guard avoids the `working_dir == None`
case (which would mark the slot `Failed` and disable `@` autocomplete).

### 5.2 ffs skip-stat — why it is NOT a 2-line change
A prototype that threaded `collect_metadata` into `walk_filesystem` measured big wins
(40k warm 320 ms → 91 ms) but **broke 58 tests**, because `file.size` is consumed
beyond content reading:
- `grep::prepare_files_to_search` filters candidates with `f.size > 0` *before* any
  content access — names-only files (size 0) are silently dropped.
- `FileItem::get_content_for_search` early-returns on `self.size == 0`.

So Option B must ship **with** the grep size-tolerance changes in §4. These touch
`pub` grep entry points' internals (not signatures) and the hot path, so they need the
full grep integration suite green before merge.

---

## 6. Repo References

| Aspect | Location |
|--------|----------|
| Readiness gate | `crates/ffs-core/src/file_picker.rs::is_scan_active` |
| Scan orchestration / early readiness flip | `crates/ffs-core/src/scan.rs::ScanJob::run` |
| The walk (stat per file, sorts) | `crates/ffs-core/src/file_picker.rs::walk_filesystem` |
| size-based grep filter | `crates/ffs-core/src/grep.rs::prepare_files_to_search` |
| content read | `crates/ffs-core/src/types.rs::FileItem::get_content_for_search` |
| jcode integration | `jcode/crates/jcode-tui/src/tui/app/at_picker.rs` |

---

## 7. Test Cases

- `walk_filesystem(collect_metadata=false)` populates files with `size == 0` and a
  names-only picker still returns correct `fuzzy_search_mixed` results (names/paths).
- grep over a names-only picker finds matches in non-empty files and skips empty ones
  (regression guard for §5.2) — extend `tests/grep_integration.rs`.
- Incremental readiness: `search()` returns a non-empty subset before `wait_for_scan`
  completes on a >=10k-file fixture; final result equals the all-at-once result.
- Existing suites must stay green: `grep_integration`, `path_separator_constraint_test`,
  `bigram_overlay_*`, watcher lifecycle tests.

---

## 8. Benchmarks

| Metric | Baseline | Target | How |
|--------|----------|--------|-----|
| time-to-ready, 40k warm | 320 ms | <120 ms | harness in §2 |
| time-to-ready, 40k cold | 1.18 s | <600 ms | drop caches, harness |
| time-to-**first-result**, 200k (Option A) | = full scan | <100 ms | harness, poll first non-empty `search()` |
| grep p50 over names-only picker | n/a (broken) | within 1.2× of indexed | `tests/grep_integration` timing |

---

## 9. Migration / Rollout

- jcode pins `ffs-search = "=0.1.3"`. ffs-side wins (B, A) require a crates.io publish
  and a jcode pin bump; the eager warm-up (§5.1) works against 0.1.3 today.
- No public API breaks: Option B changes only `pub(crate) walk_filesystem` + grep
  internals; Option A changes scan orchestration only. `FilePickerOptions`,
  `FilePicker`, and the C/MCP surfaces are untouched.

---

## 10. Known Limitations / Future Work

- [ ] Option B requires grep size-tolerance (§5.2) before it is safe to enable.
- [ ] Option A partial-commit must rebuild the arena-backed `ChunkedPathStore`
      incrementally or maintain a cheap "first-paint" overlay; design TBD.
- [ ] Consider a `FilePickerOptions` opt-in (`names_only`) instead of inferring from
      `mmap || content_indexing` — but adding a field is a breaking change for
      exhaustive struct literals (jcode constructs it exhaustively), so gate behind a
      major-version bump or `#[non_exhaustive]`.

---

## 11. Success Criteria

- [x] Root cause identified and measured.
- [x] jcode `@` autocomplete is responsive at first use (eager warm-up).
- [ ] ffs time-to-ready (40k warm) under target with all tests green.
- [ ] ffs incremental readiness yields sub-100ms first result on a 200k-file repo.
