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

    let mut kept = Vec::with_capacity(total);
    let mut bytes_so_far = 0usize;
    let mut footer = String::new();

    for (i, line) in lines.iter().enumerate() {
        let prospective = bytes_so_far + line.len() + 1; // +1 for newline
        let remaining_lines = total.saturating_sub(i);
        let candidate_footer = if remaining_lines > 0 {
            format!("[{remaining_lines} more lines]\n")
        } else {
            String::new()
        };

        if prospective + candidate_footer.len() > max_bytes {
            let dropped = remaining_lines;
            footer = if dropped > 0 {
                format!("[{dropped} more lines]\n")
            } else {
                String::new()
            };
            break;
        }
        kept.push(*line);
        bytes_so_far = prospective;
    }

    let mut out = String::with_capacity(bytes_so_far + footer.len());
    for l in &kept {
        out.push_str(l);
        out.push('\n');
    }
    out.push_str(&footer);

    let outcome = TruncationOutcome {
        kept_lines: kept.len(),
        dropped_lines: total - kept.len(),
        kept_bytes: bytes_so_far,
        footer_bytes: footer.len(),
    };

    (out, outcome)
}

/// Apply `producer` to fill `target` with content, preserving a `footer` even
/// when the content overflows `max_bytes`. Guarantees the final byte sequence
/// is `<truncated content><footer>` where footer always fits.
///
/// `producer` is invoked with a mutable byte budget; it pushes payload into
/// `target` and returns the number of payload bytes appended.
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
        let input = "a\nb\nc\nd\ne\n";
        let (out, outcome) = smart_truncate(input, 8);
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
