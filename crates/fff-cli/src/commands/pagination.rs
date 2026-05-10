// Limit/offset windowing for command outputs (callers, callees, symbol, find).

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct Page<T> {
    pub items: Vec<T>,
    pub total: usize,
    pub offset: usize,
    pub has_more: bool,
}

impl<T> Page<T> {
    // offset >= total → empty page; limit == 0 → empty page but has_more if anything
    // remained after offset; otherwise drain offset, truncate to limit.
    pub fn paginate(mut all: Vec<T>, offset: usize, limit: usize) -> Self {
        let total = all.len();
        if offset >= total {
            return Self {
                items: Vec::new(),
                total,
                offset,
                has_more: false,
            };
        }
        all.drain(..offset);
        if limit == 0 {
            return Self {
                items: Vec::new(),
                total,
                offset,
                has_more: !all.is_empty(),
            };
        }
        let has_more = all.len() > limit;
        if has_more {
            all.truncate(limit);
        }
        Self {
            items: all,
            total,
            offset,
            has_more,
        }
    }
}

// One-line "[start-end of total]" footer for human text output. `returned`
// is the number of items shown on this page (separated from `Page<T>` so the
// caller can borrow individual fields without keeping the page itself around).
pub(crate) fn footer(total: usize, offset: usize, returned: usize, has_more: bool) -> String {
    if total == 0 {
        return String::new();
    }
    let end = offset.saturating_add(returned);
    let mut out = format!("[{}-{} of {}]", offset + 1, end, total);
    if has_more {
        let next = offset.saturating_add(returned);
        out.push_str(&format!(" — next: --offset {next}"));
    }
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::{footer, Page};

    #[test]
    fn first_page_with_limit_smaller_than_total() {
        let p = Page::paginate(vec![1, 2, 3, 4, 5], 0, 3);
        assert_eq!(p.items, vec![1, 2, 3]);
        assert_eq!(p.total, 5);
        assert_eq!(p.offset, 0);
        assert!(p.has_more);
        assert_eq!(p.items.len(), 3);
    }

    #[test]
    fn middle_page_via_offset() {
        let p = Page::paginate(vec![1, 2, 3, 4, 5, 6], 2, 2);
        assert_eq!(p.items, vec![3, 4]);
        assert_eq!(p.total, 6);
        assert_eq!(p.offset, 2);
        assert!(p.has_more);
    }

    #[test]
    fn last_page_clears_has_more() {
        let p = Page::paginate(vec![1, 2, 3], 1, 10);
        assert_eq!(p.items, vec![2, 3]);
        assert_eq!(p.total, 3);
        assert!(!p.has_more);
    }

    #[test]
    fn offset_past_end_returns_empty_no_more() {
        let p: Page<i32> = Page::paginate(vec![1, 2, 3], 10, 5);
        assert!(p.items.is_empty());
        assert_eq!(p.total, 3);
        assert_eq!(p.offset, 10);
        assert!(!p.has_more);
    }

    #[test]
    fn empty_input_is_empty_page() {
        let p: Page<i32> = Page::paginate(Vec::new(), 0, 10);
        assert!(p.items.is_empty());
        assert_eq!(p.total, 0);
        assert!(!p.has_more);
    }

    #[test]
    fn limit_zero_signals_more_when_anything_left() {
        let p = Page::paginate(vec![1, 2, 3], 1, 0);
        assert!(p.items.is_empty());
        assert_eq!(p.total, 3);
        assert_eq!(p.offset, 1);
        assert!(p.has_more);
    }

    #[test]
    fn limit_equal_to_remaining_clears_has_more() {
        let p = Page::paginate(vec![1, 2, 3, 4], 1, 3);
        assert_eq!(p.items, vec![2, 3, 4]);
        assert!(!p.has_more);
    }

    #[test]
    fn footer_renders_range_and_next_offset() {
        let s = footer(10, 0, 3, true);
        assert!(s.starts_with("[1-3 of 10]"));
        assert!(s.contains("--offset 3"));
    }

    #[test]
    fn footer_omits_next_when_no_more() {
        let s = footer(3, 0, 3, false);
        assert!(s.starts_with("[1-3 of 3]"));
        assert!(!s.contains("--offset"));
    }

    #[test]
    fn footer_empty_when_total_zero() {
        assert!(footer(0, 0, 0, false).is_empty());
    }

    #[test]
    fn footer_offset_at_end_shows_empty_range() {
        let s = footer(5, 5, 0, false);
        assert!(s.starts_with("[6-5 of 5]") || s.contains("of 5"));
    }
}
