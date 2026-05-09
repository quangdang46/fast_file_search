use std::path::Path;

use anyhow::Result;
use clap::Parser;
use serde::Serialize;

use crate::cli::OutputFormat;

#[derive(Debug, Parser)]
pub struct Args {
    /// Path-substring or fuzzy needle.
    pub needle: String,

    /// Limit number of results emitted.
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
}

#[derive(Debug, Serialize)]
struct FindResult {
    needle: String,
    matches: Vec<String>,
}

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let files = super::walk_files(root);
    let needle_lower = args.needle.to_lowercase();
    let mut matches: Vec<String> = files
        .iter()
        .filter_map(|p| p.to_str().map(|s| s.to_string()))
        .filter(|p| p.to_lowercase().contains(&needle_lower))
        .take(args.limit)
        .collect();
    matches.sort();

    let payload = FindResult {
        needle: args.needle,
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
