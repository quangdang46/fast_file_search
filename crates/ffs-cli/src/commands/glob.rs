use std::path::Path;

use anyhow::Result;
use clap::Parser;
use serde::Serialize;

use crate::cli::OutputFormat;

#[derive(Debug, Parser)]
pub struct Args {
    /// Glob pattern (e.g. `**/*.rs`).
    pub pattern: String,

    /// Limit number of results emitted.
    #[arg(long, default_value_t = 200)]
    pub limit: usize,
}

#[derive(Debug, Serialize)]
struct GlobResult {
    pattern: String,
    matches: Vec<String>,
}

/// Delegate to the shared core glob function which handles Windows correctly
/// (falling back to `globset::Glob` + `ignore::WalkBuilder` on unsupported
/// platforms) and respects gitignore rules consistently.
fn glob_files(root: &Path, pattern: &str, limit: usize) -> Vec<String> {
    ffs_search::glob_matcher::glob_files(root, pattern, limit)
        .into_iter()
        .map(|rel| root.join(&rel).to_string_lossy().to_string())
        .collect()
}

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let matches = glob_files(root, &args.pattern, args.limit);
    let payload = GlobResult {
        pattern: args.pattern,
        matches,
    };
    super::emit(format, &payload, |p| {
        let mut out = String::new();
        for m in &p.matches {
            out.push_str(m);
            out.push('\n');
        }
        out
    })
}
