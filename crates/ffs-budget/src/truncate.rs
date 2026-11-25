//! Content-preserving truncation primitives.
//!
//! `smart_truncate` keeps as many full lines as fit then appends a
//! `[N more lines]` footer.
//!
//! `apply_preserving_footer` is a stricter variant: it guarantees that — even
//! when truncating to a hard byte budget — the last appended line is a
//! `[truncated …]` footer, *not* a half-cut payload line.

use serde::{Deserialize, Serialize};

/// Result of a truncation pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TruncationOutcome {
    pub kept_lines: usize,
    pub dropped_lines: usize,
    pub kept_bytes: usize,
    pub footer_bytes: usize,
}

/// Truncate `input` so that the result fits in `max_bytes`. Adds a `[N more lines]`
/// footer if any lines were dropped. Always preserves the footer if at all possible.
pub fn smart_truncate(input: &str, max_bytes: usize) -> (String, TruncationOutcome) {
    let lines: Vec<&str> = input.lines().collect();
    let total = lines.len();

    if input.len() <= max_bytes {
        return (
            input.to_string(),
            TruncationOutcome {
                kept_lines: total,
                dropped_lines: 0,
                kept_bytes: input.len(),
                footer_bytes: 0,
            },
        );
    }

    // The footer width is monotone in the dropped-line count, so
    // "[<total> more lines]" is the widest footer this input can produce.
    // Reserving it up-front keeps the payload budget honest: previously a
    // tiny `max_bytes` could emit a footer *larger* than the budget, or a
    // bare footer with zero content lines.
    let widest_footer = format!("[{total} more lines]\n");
    let footer_reserve = widest_footer.len();

    // Degenerate budget: not even the footer fits. Hard-clip at a char
    // boundary rather than emit an over-budget footer.
    if max_bytes <= footer_reserve {
        let mut end = max_bytes.min(input.len());
        while end > 0 && !input.is_char_boundary(end) {
            end -= 1;
        }
        let out = input[..end].to_string();
        let kept_lines = out.lines().count();
        return (
            out,
            TruncationOutcome {
                kept_lines,
                dropped_lines: total.saturating_sub(kept_lines),
                kept_bytes: end,
                footer_bytes: 0,
            },
        );
    }

    let payload_budget = max_bytes - footer_reserve;
    let mut kept: Vec<&str> = Vec::with_capacity(total);
    let mut bytes_so_far = 0usize;

    for line in &lines {
        let prospective = bytes_so_far + line.len() + 1; // +1 for newline
        if prospective > payload_budget {
            break;
        }
        kept.push(*line);
        bytes_so_far = prospective;
    }

    // First-line guarantee: a budget smaller than `line1 + footer` must
    // still show the head of the file, not a bare "[N more lines]" footer.
    // Clip line 0 to the payload budget at a char boundary. The result is a
    // partial line, so it is not counted as a kept line below.
    let mut clipped_partial = false;
    if kept.is_empty() {
        if let Some(first) = lines.first() {
            let mut take = first.len().min(payload_budget.saturating_sub(1));
            while take > 0 && !first.is_char_boundary(take) {
                take -= 1;
            }
            kept.push(&first[..take]);
            bytes_so_far = take + 1;
            clipped_partial = true;
        }
    }

    // A mid-line clip keeps only a fragment of `lines[0]`: count complete
    // lines only so `dropped_lines`/`kept_lines` stay honest. Previously a
    // single-line input reported dropped_lines = 0 and emitted no footer.
    let complete_kept = kept.len().saturating_sub(usize::from(clipped_partial));
    let dropped = total.saturating_sub(complete_kept);
    let footer = if dropped > 0 {
        format!("[{dropped} more lines]\n")
    } else {
        String::new()
    };

    let mut out = String::with_capacity(bytes_so_far + footer.len());
    for l in &kept {
        out.push_str(l);
        out.push('\n');
    }
    out.push_str(&footer);

    let outcome = TruncationOutcome {
        kept_lines: complete_kept,
        dropped_lines: dropped,
        kept_bytes: bytes_so_far,
        footer_bytes: footer.len(),
    };

    (out, outcome)
}

/// Apply `producer` to fill `target` with content, preserving a `footer` even
/// when the content overflows `max_bytes`. Guarantees the final byte sequence
/// is `<truncated content><footer>` with the footer preserved verbatim.
///
/// `producer` is invoked with a mutable byte budget; it pushes payload into
/// `target` and returns the number of payload bytes appended.
///
/// Precondition: `footer.len() <= max_bytes`. The footer is always appended
/// verbatim, so an oversized footer produces output larger than `max_bytes`;
/// callers must size `max_bytes` to fit their footer.
pub fn apply_preserving_footer<F>(
    target: &mut String,
    max_bytes: usize,
    footer: &str,
    producer: F,
) -> TruncationOutcome
where
    F: FnOnce(&mut String, usize) -> usize,
{
    let footer_len = footer.len();
    let payload_budget = max_bytes.saturating_sub(footer_len);
    let pre_len = target.len();

    let _emitted = producer(target, payload_budget);
    let appended = target.len() - pre_len;

    if appended > payload_budget {
        // Walk back to a char boundary at the budget.
        let mut idx = pre_len + payload_budget;
        while idx > pre_len && !target.is_char_boundary(idx) {
            idx -= 1;
        }
        target.truncate(idx);
    }

    let final_payload = target.len() - pre_len;
    target.push_str(footer);

    TruncationOutcome {
        kept_lines: 0,
        dropped_lines: 0,
        kept_bytes: final_payload,
        footer_bytes: footer.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_truncation_when_input_fits() {
        let (out, outcome) = smart_truncate("hello\n", 100);
        assert_eq!(out, "hello\n");
        assert_eq!(outcome.dropped_lines, 0);
        assert_eq!(outcome.footer_bytes, 0);
    }

    #[test]
    fn truncates_with_footer() {
        let input: String = (0..10).map(|i| format!("l{i}\n")).collect();
        let (out, outcome) = smart_truncate(&input, 20);
        assert!(outcome.dropped_lines > 0);
        assert!(out.contains("more lines]"));
    }

    #[test]
    fn footer_byte_count_matches() {
        let input: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let (out, outcome) = smart_truncate(&input, 30);
        assert!(outcome.dropped_lines > 0);
        assert!(outcome.footer_bytes > 0);
        assert!(out.ends_with("more lines]\n"));
    }

    #[test]
    fn apply_preserving_footer_clips_payload() {
        let mut buf = String::new();
        let outcome = apply_preserving_footer(&mut buf, 20, "[truncated]\n", |buf, budget| {
            let payload = "0123456789ABCDEFGHIJ"; // 20 bytes
            let take = payload.len().min(budget + 5);
            buf.push_str(&payload[..take]);
            payload.len()
        });
        assert!(buf.ends_with("[truncated]\n"));
        assert!(outcome.footer_bytes > 0);
    }

    #[test]
    fn apply_preserving_footer_handles_empty_input() {
        let mut buf = String::new();
        let outcome = apply_preserving_footer(&mut buf, 10, "[empty]\n", |_, _| 0);
        assert_eq!(outcome.kept_bytes, 0);
        assert_eq!(buf, "[empty]\n");
    }

    #[test]
    fn mid_line_clip_reports_dropped_honestly() {
        // Single very long line, tiny budget: only a fragment fits. The
        // partial line must not count as kept — previously this reported
        // dropped_lines = 0 and emitted no footer at all.
        let input = "x".repeat(200);
        let (out, outcome) = smart_truncate(&input, 30);
        assert!(out.ends_with("more lines]\n"));
        assert_eq!(outcome.kept_lines, 0);
        assert_eq!(outcome.dropped_lines, 1);
        assert!(out.len() <= 30);
    }

    #[test]
    fn mid_line_clip_multi_line_counts_partial_as_dropped() {
        let input = format!("{}\nline2\nline3\n", "x".repeat(200));
        let (_out, outcome) = smart_truncate(&input, 30);
        assert_eq!(outcome.kept_lines, 0);
        assert_eq!(outcome.dropped_lines, 3);
    }

    #[test]
    fn apply_preserving_footer_oversized_footer_still_appended() {
        // Documents the precondition boundary: footer > max_bytes is appended
        // verbatim and the result exceeds max_bytes (caller must size budget).
        let mut buf = String::new();
        let outcome = apply_preserving_footer(&mut buf, 5, "[a much longer footer]\n", |_, _| 0);
        assert_eq!(buf, "[a much longer footer]\n");
        assert_eq!(outcome.footer_bytes, "[a much longer footer]\n".len());
    }

    #[test]
    fn apply_preserving_footer_no_overflow() {
        let mut buf = String::new();
        let outcome = apply_preserving_footer(&mut buf, 100, "[end]\n", |buf, _budget| {
            buf.push_str("ok\n");
            3
        });
        assert_eq!(outcome.kept_bytes, 3);
        assert!(buf.ends_with("[end]\n"));
    }
}
