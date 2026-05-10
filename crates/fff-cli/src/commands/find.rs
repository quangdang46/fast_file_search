use std::path::Path;

use anyhow::Result;
use clap::Parser;
use serde::Serialize;

use crate::cli::OutputFormat;
use crate::commands::pagination::{footer, Page};

#[derive(Debug, Parser)]
pub struct Args {
    /// Path-substring or fuzzy needle.
    pub needle: String,

    /// Maximum results returned in this page.
    #[arg(long, default_value_t = 50)]
    pub limit: usize,

    /// Skip this many results before starting the page.
    #[arg(long, default_value_t = 0)]
    pub offset: usize,
}

#[derive(Debug, Serialize)]
struct FindResult {
    needle: String,
    matches: Vec<String>,
    total: usize,
    offset: usize,
    has_more: bool,
}

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let files = super::walk_files(root);
    let needle_lower = args.needle.to_lowercase();
    let mut all: Vec<String> = files
        .iter()
        .filter_map(|p| p.to_str().map(|s| s.to_string()))
        .filter(|p| p.to_lowercase().contains(&needle_lower))
        .collect();
    all.sort();

    let page = Page::paginate(all, args.offset, args.limit);
    let payload = FindResult {
        needle: args.needle,
        total: page.total,
        offset: page.offset,
        has_more: page.has_more,
        matches: page.items,
    };
    super::emit(format, &payload, |p| {
        let mut out = String::new();
        for m in &p.matches {
            out.push_str(m);
            out.push('\n');
        }
        if p.total > 0 {
            out.push_str(&footer(p.total, p.offset, p.matches.len(), p.has_more));
        }
        out
    })
}
