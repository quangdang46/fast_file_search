//! Token-budget aware filtering and truncation.
//!
//! Three primary services:
//!
//! * [`filter`] — character-level / line-level comment & whitespace stripping
//!   (None / Minimal / Aggressive levels).
//! * [`truncate`] — `smart_truncate` and `apply_preserving_footer` for
//!   token-budgeted output that always shows a `[N more lines]` footer.
//! * [`stream`] — `BlockStreamFilter` for line-oriented streaming output.

pub mod cascade;
pub mod filter;
pub mod stream;
pub mod tokens;
pub mod truncate;

pub use filter::{
    detect_filter_level, AggressiveFilter, FilterLevel, FilterStrategy, MinimalFilter, NoFilter,
};
pub use stream::{BlockHandler, BlockStreamFilter, StreamFilter};
pub use tokens::{
    estimate_tokens, percent_budget, tokens_to_bytes, BudgetSplit, DEFAULT_PERCENT_BODY,
    DEFAULT_PERCENT_FOOTER, DEFAULT_PERCENT_HEADER, EMERGENCY_RESERVE_PCT,
};
pub use truncate::{apply_preserving_footer, smart_truncate, TruncationOutcome};
