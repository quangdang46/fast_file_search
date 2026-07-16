//! High-performance grep engine for live content search.
//!
//! Module layout inspired by upstream fff `grep/` (b4590ca).
//! `classify` is extracted; remaining engine stays in `grep` until a
//! follow-up lands fuzzy_grep/utils extraction without visibility churn.

mod classify;

#[allow(clippy::module_inception)]
mod grep;
pub use classify::*;
pub use grep::*;
