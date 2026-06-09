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

/// Directories always filtered out regardless of gitignore / zlob rules.
const ALWAYS_HIDE: &[&str] = &[".git", "node_modules", "target", ".ffs"];

#[cfg(feature = "zlob")]
fn glob_files(root: &Path, pattern: &str, limit: usize) -> Vec<String> {
    let flags = zlob::ZlobFlags::RECOMMENDED | zlob::ZlobFlags::GITIGNORE;
    let base = root.to_string_lossy();
    match zlob::zlob_at(&base, pattern, flags) {
        Ok(Some(result)) => result
            .iter()
            .filter(|s| {
                !Path::new(s).components().any(|c| {
                    c.as_os_str()
                        .to_str()
                        .map(|s| ALWAYS_HIDE.contains(&s))
                        .unwrap_or(false)
                })
            })
            .take(limit)
            .map(|s| s.to_string())
            .collect(),
        Ok(None) => Vec::new(),
        Err(_) => Vec::new(),
    }
}

#[cfg(not(feature = "zlob"))]
fn glob_files(root: &Path, pattern: &str, limit: usize) -> Vec<String> {
    let mut builder = ignore::overrides::OverrideBuilder::new(root);
    builder.add(pattern).ok();
    let Ok(overrides) = builder.build() else {
        return Vec::new();
    };

    ignore::WalkBuilder::new(root)
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
        .take(limit)
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
