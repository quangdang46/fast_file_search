use std::path::Path;

use anyhow::Result;
use clap::Parser;
use serde::Serialize;

use crate::cli::OutputFormat;

#[derive(Debug, Parser)]
pub struct Args {
    /// Substring or regex to search for in file contents.
    pub needle: String,

    /// Maximum lines emitted total across all files.
    #[arg(long, default_value_t = 200)]
    pub limit: usize,

    /// Match case sensitively (default: false).
    #[arg(short = 's', long)]
    pub case_sensitive: bool,
}

#[derive(Debug, Serialize)]
struct GrepHit {
    path: String,
    line: u32,
    text: String,
}

#[derive(Debug, Serialize)]
struct GrepResult {
    needle: String,
    hits: Vec<GrepHit>,
    total_files_searched: usize,
}

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let files = super::walk_files(root);
    let total_files = files.len();
    let needle = if args.case_sensitive {
        args.needle.clone()
    } else {
        args.needle.to_lowercase()
    };

    let mut hits = Vec::new();
    for path in &files {
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        for (lineno, line) in content.lines().enumerate() {
            let haystack = if args.case_sensitive {
                std::borrow::Cow::Borrowed(line)
            } else {
                std::borrow::Cow::Owned(line.to_lowercase())
            };
            if haystack.contains(&needle) {
                hits.push(GrepHit {
                    path: path.to_string_lossy().to_string(),
                    line: (lineno + 1) as u32,
                    text: line.to_string(),
                });
                if hits.len() >= args.limit {
                    break;
                }
            }
        }
        if hits.len() >= args.limit {
            break;
        }
    }

    let payload = GrepResult {
        needle: args.needle,
        hits,
        total_files_searched: total_files,
    };
    super::emit(format, &payload, |p| {
        let mut out = String::new();
        for h in &p.hits {
            out.push_str(&format!("{}:{}: {}\n", h.path, h.line, h.text));
        }
        if p.hits.is_empty() {
            out.push_str(&format!(
                "[no matches across {} files]\n",
                p.total_files_searched
            ));
        }
        out
    })
}
