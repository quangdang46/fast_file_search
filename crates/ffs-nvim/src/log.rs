//! Logging setup for ffs-nvim — delegates to the shared ffs-core::log utilities.

pub use ffs::log::{init_tracing, install_panic_hook};
