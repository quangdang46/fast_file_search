use std::path::Path;

use anyhow::Result;
use clap::Parser;
use serde::Serialize;

use ffs_engine::Engine;

use crate::cache::CacheDir;
use crate::cli::OutputFormat;

#[derive(Debug, Parser)]
pub struct Args {}

#[derive(Debug, Serialize)]
struct IndexOutput {
    files_visited: usize,
    files_indexed: usize,
    files_skipped_other: usize,
    symbols_indexed: usize,
    cache_path: String,
    cache_written: bool,
}

pub fn run(_args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let engine = Engine::default();
    let report = engine.index(root);

    let cache = CacheDir::at(root);
    let cache_written = cache
        .write_symbol_index(&engine.handles.symbols, root)
        .is_ok();

    let payload = IndexOutput {
        files_visited: report.files_visited,
        files_indexed: report.files_indexed,
        files_skipped_other: report.files_skipped_other,
        symbols_indexed: engine.handles.symbols.symbols_indexed(),
        cache_path: cache.symbol_path().to_string_lossy().to_string(),
        cache_written,
    };
    super::emit(format, &payload, |p| {
        let suffix = if p.cache_written {
            format!("\nCache: {}\n", p.cache_path)
        } else {
            "\nCache: write failed\n".to_string()
        };
        format!(
            "Indexed {} files ({} symbols, {} skipped){}",
            p.files_indexed, p.symbols_indexed, p.files_skipped_other, suffix
        )
    })
}
