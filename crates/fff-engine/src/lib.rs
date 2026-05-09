//! Orchestration layer: classify queries, run the unified scanner, dispatch
//! to the right search backend, rank, and enforce the memory budget.

pub mod classify;
pub mod dispatch;
pub mod memory;
pub mod prefilter;
pub mod ranking;
pub mod scanner;

pub use classify::{classify_query, ClassifiedQuery};
pub use dispatch::{Engine, EngineConfig, EngineHandles};
pub use memory::{MemoryGuard, RepoSize};
pub use prefilter::{PreFilterStack, PreFilteredCandidate};
pub use ranking::{rank_matches, RankInputs};
pub use scanner::{IndexedFile, ScanProgress, ScanReport, UnifiedScanner};
