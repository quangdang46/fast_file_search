//! Fuzzy content search via neo_frizbee batch scoring.

use super::classify::is_definition_line;
use super::grep::{
    GrepMatch, GrepResult, GrepSearchOptions, char_indices_to_byte_offsets, collect_grep_results,
    truncate_display_bytes,
};
use crate::types::{ContentCacheBudget, FileItem, MmapSlot};
use ffs_grep::lines::{self, LineStep};
use rayon::prelude::*;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// Fuzzy grep search using SIMD-accelerated `neo_frizbee::match_list`.
///
/// Why this doesn't use `grep-searcher` / `GrepSink`
///
/// PlainText and Regex modes use the `grep-searcher` pipeline: a `Matcher`
/// finds candidate lines, and a `Sink` collects them one at a time. This
/// works well because memchr/regex can *skip* non-matching lines in O(n)
/// without scoring every one.
///
/// Fuzzy matching is fundamentally different. Every line is a candidate —
/// the Smith-Waterman score determines whether it passes, not a substring
/// or pattern test. The `Matcher::find_at` trait forces per-line calls to
/// the *reference* (scalar) smith-waterman, which is O(needle × line_len)
/// per line. For a 10k-line file that's 10k sequential reference calls.
///
/// `neo_frizbee::match_list` solves this by batching lines into
/// fixed-width SIMD buckets (4, 8, 12 … 512 bytes) and scoring 16+
/// haystacks per SIMD invocation. A single `match_list` call over the
/// entire file replaces 10k individual `match_indices` calls. We then
/// call `match_indices` *only* on the ~5-20 lines that pass `min_score`
/// to extract character highlight positions.
///
/// Line splitting uses `memchr::memchr` (the same SIMD-accelerated byte
/// search that `grep-searcher` and `bstr::ByteSlice::find_byte` use
/// internally) to locate `\n` terminators. This gives us the same
/// performance as the searcher's `LineStep` iterator without pulling in
/// the full searcher machinery.
///
/// For each file:
///   1. mmap the file, split lines via memchr '\n' (tracking line numbers + byte offsets)
///   2. Batch all lines through `match_list` (SIMD smith-waterman)
///   3. Filter results by `min_score`
///   4. Call `match_indices` only on passing lines to get character highlight offsets
#[allow(clippy::too_many_arguments)]
pub(super) fn fuzzy_grep_search<'a>(
    grep_text: &str,
    files_to_search: &[&'a FileItem],
    options: &GrepSearchOptions,
    total_files: usize,
    filtered_file_count: usize,
    case_insensitive: bool,
    budget: &ContentCacheBudget,
    abort_signal: &AtomicBool,
    base_path: &Path,
    arena: crate::simd_path::ArenaPtr,
    _overflow_arena: crate::simd_path::ArenaPtr,
) -> GrepResult<'a> {
    // max_typos controls how many *needle* characters can be unmatched.
    // A transposition (e.g. "shcema" → "schema") costs ~1 typo with
    // default gap penalties. We scale max_typos by needle length:
    //   1-2 chars → 0 typos (exact subsequence only)
    //   3-5 chars → 1 typo
    //   6+  chars → 2 typos
    // Cap at 2: higher values (3+) let the SIMD prefilter pass lines
    // missing key characters entirely (e.g. query "flvencodeX" matching
    // lines without 'l' or 'v'). Quality comes from the post-match filters.
    let max_typos = (grep_text.len() / 3).min(2);
    let scoring = neo_frizbee::Scoring {
        // Use default gap penalties. Higher values (e.g. 20) cause
        // smith-waterman to prefer *dropping needle chars* over paying
        // gap costs, which inflates the typo count and breaks
        // transposition matching ("shcema" → "schema" becomes 3 typos instead of 1)
        exact_match_bonus: 100,
        // gap_open_penalty: 4,
        // gap_extend_penalty: 2,
        prefix_bonus: 0,
        capitalization_bonus: if case_insensitive { 0 } else { 4 },
        ..neo_frizbee::Scoring::default()
    };

    let matcher = neo_frizbee::Matcher::new(
        grep_text,
        &neo_frizbee::Config {
            // Use the real max_typos so frizbee's SIMD prefilter actually rejects non-matching lines (~2 SIMD instructions per line vs full SW scoring).
            max_typos: Some(max_typos as u16),
            sort: false,
            scoring,
            ..Default::default()
        },
    );

    // Minimum score threshold: 50% of a perfect contiguous match.
    // With default scoring (match_score=12, matching_case_bonus=4 = 16/char),
    // a transposition costs ~5 from a gap, keeping the score well above 50%.
    let perfect_score = (grep_text.len() as u16) * 16;
    let min_score = (perfect_score * 50) / 100;

    // Target identifiers are often longer than the query due to delimiters
    // (e.g. query "flvencodepicture" → "ff_flv_encode_picture_header").
    // Allow 3x needle length to accommodate underscore/dot-separated names.
    let max_match_span = grep_text.len() * 3;
    let needle_len = grep_text.len();

    // Each delimiter (_, .) in the target creates a gap. A typical C/Rust
    // identifier like "ff_flv_encode_picture_header" has 4-5 underscores.
    // Scale generously so delimiter gaps don't reject valid matches.
    let max_gaps = (needle_len / 3).max(2);

    // File-level prefilter: collect unique needle chars (both cases) for
    // a fast memchr scan.  If a file doesn't contain enough distinct
    // needle characters, skip it entirely — no line splitting needed.
    let needle_bytes = grep_text.as_bytes();
    let mut unique_needle_chars: Vec<u8> = Vec::new();
    for &b in needle_bytes {
        let lo = b.to_ascii_lowercase();
        let hi = b.to_ascii_uppercase();
        if !unique_needle_chars.contains(&lo) {
            unique_needle_chars.push(lo);
        }
        if lo != hi && !unique_needle_chars.contains(&hi) {
            unique_needle_chars.push(hi);
        }
    }

    // How many distinct needle chars must appear in the file.
    // With max_typos allowed, we need at least (unique_count - max_typos).
    let unique_count = {
        let mut seen = [false; 256];
        for &b in needle_bytes {
            seen[b.to_ascii_lowercase() as usize] = true;
        }
        seen.iter().filter(|&&v| v).count()
    };
    let min_chars_required = unique_count.saturating_sub(max_typos);

    let time_budget = if options.time_budget_ms > 0 {
        Some(std::time::Duration::from_millis(options.time_budget_ms))
    } else {
        None
    };
    let search_start = std::time::Instant::now();
    let budget_exceeded = AtomicBool::new(false);
    let max_matches_per_file = options.max_matches_per_file;
    // Parallel phase with `map_init`: each rayon worker thread clones the
    // matcher once and gets a reusable read buffer + mmap slot. The buffer
    // holds small files; the slot holds a fresh mmap for cache-miss files.
    let per_file_results: Vec<(usize, &'a FileItem, Vec<GrepMatch>)> = files_to_search
        .par_iter()
        .enumerate()
        .map_init(
            || {
                (
                    matcher.clone(),
                    Vec::with_capacity(64 * 1024),
                    MmapSlot::default(),
                )
            },
            |(matcher, buf, mmap_slot), (idx, file)| {
                if abort_signal.load(Ordering::Relaxed) {
                    budget_exceeded.store(true, Ordering::Relaxed);
                    return None;
                }

                if let Some(budget) = time_budget
                    && search_start.elapsed() > budget
                {
                    budget_exceeded.store(true, Ordering::Relaxed);
                    return None;
                }

                let file_bytes =
                    file.get_content_for_search(buf, mmap_slot, arena, base_path, budget)?;

                // File-level prefilter: check if enough distinct needle chars
                // exist anywhere in the file bytes.  Uses memchr for speed.
                if min_chars_required > 0 {
                    let mut chars_found = 0usize;
                    for &ch in &unique_needle_chars {
                        if memchr::memchr(ch, file_bytes).is_some() {
                            chars_found += 1;
                            if chars_found >= min_chars_required {
                                break;
                            }
                        }
                    }
                    if chars_found < min_chars_required {
                        return None;
                    }
                }

                // Validate the whole file as UTF-8 once upfront. Source code
                // files are virtually always valid UTF-8; this single check
                // replaces per-line from_utf8 calls (~8% of fuzzy grep time).
                let file_is_utf8 = std::str::from_utf8(file_bytes).is_ok();

                // Reuse grep-searcher's LineStep for SIMD-accelerated line iteration.
                let mut stepper = LineStep::new(b'\n', 0, file_bytes.len());
                let estimated_lines = (file_bytes.len() / 40).max(64);
                let mut file_lines: Vec<&str> = Vec::with_capacity(estimated_lines);
                let mut line_meta: Vec<(u64, u64)> = Vec::with_capacity(estimated_lines);
                let line_term_lf = ffs_grep::LineTerminator::byte(b'\n');
                let line_term_cr = ffs_grep::LineTerminator::byte(b'\r');

                let mut line_number: u64 = 1;
                while let Some(line_match) = stepper.next_match(file_bytes) {
                    let byte_offset = line_match.start() as u64;

                    // Strip line terminators (\n, \r).
                    let trimmed = lines::without_terminator(
                        lines::without_terminator(&file_bytes[line_match], line_term_lf),
                        line_term_cr,
                    );

                    if !trimmed.is_empty() {
                        // SAFETY: when the whole file is valid UTF-8, every
                        // sub-slice split on ASCII byte boundaries (\n, \r)
                        // is also valid UTF-8.
                        let line_str = if file_is_utf8 {
                            unsafe { std::str::from_utf8_unchecked(trimmed) }
                        } else if let Ok(s) = std::str::from_utf8(trimmed) {
                            s
                        } else {
                            line_number += 1;
                            continue;
                        };
                        file_lines.push(line_str);
                        line_meta.push((line_number, byte_offset));
                    }

                    line_number += 1;
                }

                if file_lines.is_empty() {
                    return None;
                }

                // Single-pass: score + indices in one Smith-Waterman run per line.
                let matches_with_indices = matcher.match_list_indices(&file_lines);
                let mut file_matches: Vec<GrepMatch> = Vec::new();

                for mut match_indices in matches_with_indices {
                    if match_indices.score < min_score {
                        continue;
                    }

                    let idx = match_indices.index as usize;
                    let raw_line = file_lines[idx];

                    let truncated = truncate_display_bytes(raw_line.as_bytes());
                    let display_line = if truncated.len() < raw_line.len() {
                        // SAFETY: truncate_display_bytes preserves UTF-8 char boundaries
                        &raw_line[..truncated.len()]
                    } else {
                        raw_line
                    };

                    // If the line was truncated, re-compute indices on the shorter string.
                    if display_line.len() < raw_line.len() {
                        let Some(re_indices) = matcher
                            .match_list_indices(&[display_line])
                            .into_iter()
                            .next()
                        else {
                            continue;
                        };
                        match_indices = re_indices;
                    }

                    // upstream returns indices in reverse order, sort ascending
                    match_indices.indices.sort_unstable();

                    // Minimum matched chars: at least (needle_len - max_typos)
                    // characters must appear. This is consistent with the typo
                    // budget: each typo can drop one needle char from the alignment.
                    let min_matched = needle_len.saturating_sub(max_typos).max(1);
                    if match_indices.indices.len() < min_matched {
                        continue;
                    }

                    let indices = &match_indices.indices;

                    if let (Some(&first), Some(&last)) = (indices.first(), indices.last()) {
                        // Span check: reject widely scattered matches.
                        let span = last - first + 1;
                        if span > max_match_span {
                            continue;
                        }

                        // Density check: matched chars / span must be dense enough.
                        // Relaxed for perfect subsequence matches (all needle chars
                        // present), slightly relaxed for typo matches to handle
                        // delimiter-heavy targets (e.g. "ff_flv_encode_picture_header"
                        // has span inflated by underscores → density ~68%).
                        let density = (indices.len() * 100) / span;
                        let min_density = if indices.len() >= needle_len {
                            45 // Perfect subsequence — relaxed (delimiters inflate span)
                        } else {
                            65 // Has typos — moderately strict
                        };
                        if density < min_density {
                            continue;
                        }

                        // Gap count check: count discontinuities in the indices.
                        let gap_count = indices.windows(2).filter(|w| w[1] != w[0] + 1).count();
                        if gap_count > max_gaps {
                            continue;
                        }
                    }

                    let (ln, bo) = line_meta[idx];
                    let match_byte_offsets =
                        char_indices_to_byte_offsets(display_line, &match_indices.indices);
                    let col = match_byte_offsets
                        .first()
                        .map(|r| r.0 as usize)
                        .unwrap_or(0);

                    file_matches.push(GrepMatch {
                        file_index: 0,
                        line_number: ln,
                        col,
                        byte_offset: bo,
                        is_definition: options.classify_definitions
                            && is_definition_line(display_line),
                        line_content: display_line.to_string(),
                        match_byte_offsets,
                        fuzzy_score: Some(match_indices.score),
                        context_before: Vec::new(),
                        context_after: Vec::new(),
                    });

                    if max_matches_per_file != 0 && file_matches.len() >= max_matches_per_file {
                        break;
                    }
                }

                if file_matches.is_empty() {
                    return None;
                }

                Some((idx, *file, file_matches))
            },
        )
        .flatten()
        .collect();

    collect_grep_results(
        per_file_results,
        files_to_search.len(),
        options,
        total_files,
        filtered_file_count,
        budget_exceeded.load(Ordering::Relaxed),
    )
}
