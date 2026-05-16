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

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let mut builder = ignore::overrides::OverrideBuilder::new(root);
    builder.add(&args.pattern)?;
    let overrides = builder.build()?;

    // Bug 15: respect default ignores. `WalkBuilder` checks override matches
    // *before* hidden/gitignore rules, so an explicit pattern like `**/*`
    // would otherwise pull in `.git/`. We post-filter paths whose components
    // contain a hidden directory we always want to skip.
    const ALWAYS_HIDE: &[&str] = &[".git", "node_modules", "target", ".ffs"];

    let files = ignore::WalkBuilder::new(root)
        .overrides(overrides)
        .standard_filters(true)
        .hidden(true)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .require_git(false)
        .build()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter(|e| {
            let p = e.path();
            !p.components().any(|c| {
                c.as_os_str()
                    .to_str()
                    .map(|s| ALWAYS_HIDE.contains(&s))
                    .unwrap_or(false)
            })
        })
        .filter_map(|e| e.path().to_str().map(|s| s.to_string()))
        .take(args.limit)
        .collect::<Vec<_>>();

    let payload = GlobResult {
        pattern: args.pattern,
        matches: files,
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
