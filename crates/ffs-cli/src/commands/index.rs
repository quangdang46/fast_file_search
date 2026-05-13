use std::path::Path;

use anyhow::Result;
use clap::Parser;
use serde::Serialize;

use ffs_engine::Engine;

use crate::cli::OutputFormat;

#[derive(Debug, Parser)]
pub struct Args {}

#[derive(Debug, Serialize)]
struct IndexOutput {
    files_visited: usize,
    files_indexed: usize,
    files_skipped_other: usize,
    symbols_indexed: usize,
}

pub fn run(_args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let engine = Engine::default();
    let report = engine.index(root);
    let payload = IndexOutput {
        files_visited: report.files_visited,
        files_indexed: report.files_indexed,
        files_skipped_other: report.files_skipped_other,
        symbols_indexed: engine.handles.symbols.symbols_indexed(),
    };
    super::emit(format, &payload, |p| {
        format!(
            "Indexed {} files ({} symbols, {} skipped)\n",
            p.files_indexed, p.symbols_indexed, p.files_skipped_other
        )
    })
}
