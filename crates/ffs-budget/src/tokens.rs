//! Token estimation and budget allocation helpers.
//!
//! All allocations use integer math (no floats) to match the embedded targets
//! and keep results deterministic across platforms.

/// Default header allocation as a percentage of total budget.
pub const DEFAULT_PERCENT_HEADER: u8 = 2;
/// Default body allocation as a percentage of total budget.
pub const DEFAULT_PERCENT_BODY: u8 = 85;
/// Default footer allocation as a percentage of total budget.
pub const DEFAULT_PERCENT_FOOTER: u8 = 10;
/// Reserve always kept aside for late-arriving error/completion lines.
pub const EMERGENCY_RESERVE_PCT: u8 = 3;

/// Approximate token estimate (≈ bytes / 4, ceil division).
#[must_use]
pub fn estimate_tokens(byte_len: u64) -> u64 {
    byte_len.div_ceil(4)
}

/// Convert a token budget back into a byte budget.
#[must_use]
pub fn tokens_to_bytes(tokens: u64) -> u64 {
    tokens.saturating_mul(4)
}

/// Allocate `total` tokens as `pct%`, rounding down.
#[must_use]
pub fn percent_budget(total: u64, pct: u8) -> u64 {
    (total / 100).saturating_mul(u64::from(pct))
}

/// Header / body / footer / emergency split based on `EMERGENCY_RESERVE_PCT` plus the user split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetSplit {
    pub header: u64,
    pub body: u64,
    pub footer: u64,
    pub emergency: u64,
}

impl BudgetSplit {
    /// Default split: 2 / 85 / 10 / 3 of `total`.
    #[must_use]
    pub fn default_for(total: u64) -> Self {
        Self::custom(
            total,
            DEFAULT_PERCENT_HEADER,
            DEFAULT_PERCENT_BODY,
            DEFAULT_PERCENT_FOOTER,
        )
    }

    /// Custom split with explicit percentages. The remainder beyond
    /// `header+body+footer` becomes the emergency reserve.
    #[must_use]
    pub fn custom(total: u64, header_pct: u8, body_pct: u8, footer_pct: u8) -> Self {
        let header = percent_budget(total, header_pct);
        let body = percent_budget(total, body_pct);
        let footer = percent_budget(total, footer_pct);
        let used = header.saturating_add(body).saturating_add(footer);
        let emergency = total.saturating_sub(used);
        Self {
            header,
            body,
            footer,
            emergency,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_basic() {
        assert_eq!(estimate_tokens(0), 0);
        assert_eq!(estimate_tokens(4), 1);
        assert_eq!(estimate_tokens(5), 2);
    }

    #[test]
    fn split_proportions() {
        let s = BudgetSplit::default_for(10_000);
        assert_eq!(s.header, 200);
        assert_eq!(s.body, 8500);
        assert_eq!(s.footer, 1000);
        assert_eq!(s.emergency, 300);
    }

    #[test]
    fn custom_split_zero_total() {
        let s = BudgetSplit::default_for(0);
        assert_eq!(s.header, 0);
        assert_eq!(s.body, 0);
        assert_eq!(s.footer, 0);
        assert_eq!(s.emergency, 0);
    }
}
