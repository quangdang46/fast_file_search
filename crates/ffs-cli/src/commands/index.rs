use std::path::Path;

use anyhow::Result;
use clap::Parser;
use serde::Serialize;

use ffs_engine::Engine;

use crate::bigram::GrepBigram;
use crate::cache::CacheDir;
use crate::cli::OutputFormat;

#[derive(Debug, Parser)]
pub struct Args {
    /// Discard any existing `<root>/.ffs/` cache and rebuild from scratch.
    /// Use after large refactors when the file-count drift / git HEAD
    /// invalidation didn't trigger but you want a fresh index anyway.
    #[arg(short = 'f', long)]
    pub force: bool,
}

#[derive(Debug, Serialize)]
struct IndexOutput {
    files_visited: usize,
    files_indexed: usize,
    files_skipped_other: usize,
    symbols_indexed: usize,
    bigram_files: usize,
    bigram_count: usize,
    cache_path: String,
    bigram_cache_path: String,
    cache_written: bool,
    bigram_cache_written: bool,
    forced: bool,
}

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let cache = CacheDir::at(root);
    if args.force {
        // Best-effort: drop any prior payload + meta so neither the strict
        // load nor the stale-load fallback in load_or_build_engine could
        // sneak in if a downstream caller invoked it.
        let _ = std::fs::remove_file(cache.symbol_path());
        let _ = std::fs::remove_file(cache.bigram_path());
        let _ = std::fs::remove_file(cache.meta_path());
    }

    let engine = Engine::default();
    let report = engine.index(root);
    let cache_written = cache
        .write_symbol_index(&engine.handles.symbols, root)
        .is_ok();

    // Build the bigram prefilter from the same set of paths the engine
    // just walked. Reading content twice is unfortunate but keeps the
    // bigram module standalone; for typical repos the second pass is
    // dominated by FS cache hits from the first pass.
    let walk_paths = super::walk_files(root);
    let bigram = GrepBigram::build(&walk_paths);
    let bigram_cache_written = cache.write_bigram_index(&bigram).is_ok();

    let payload = IndexOutput {
        files_visited: report.files_visited,
        files_indexed: report.files_indexed,
        files_skipped_other: report.files_skipped_other,
        symbols_indexed: engine.handles.symbols.symbols_indexed(),
        bigram_files: bigram.file_count(),
        bigram_count: bigram.bigram_count(),
        cache_path: cache.symbol_path().to_string_lossy().to_string(),
        bigram_cache_path: cache.bigram_path().to_string_lossy().to_string(),
        cache_written,
        bigram_cache_written,
        forced: args.force,
    };
    super::emit(format, &payload, |p| {
        let symbol_line = if p.cache_written {
            format!("Cache: {}\n", p.cache_path)
        } else {
            "Cache: write failed\n".to_string()
        };
        let bigram_line = if p.bigram_cache_written {
            format!(
                "Bigram cache: {} ({} bigrams over {} files)\n",
                p.bigram_cache_path, p.bigram_count, p.bigram_files
            )
        } else {
            "Bigram cache: write failed\n".to_string()
        };
        let prefix = if p.forced {
            "Force-rebuilt index. "
        } else {
            ""
        };
        format!(
            "{}Indexed {} files ({} symbols, {} skipped)\n{}{}",
            prefix,
            p.files_indexed,
            p.symbols_indexed,
            p.files_skipped_other,
            symbol_line,
            bigram_line
        )
    })
}
