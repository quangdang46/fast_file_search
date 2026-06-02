//! `ffs mention-search <input>` — Phase C surface for the @-mention system.
//!
//! Phase B (`ffs_engine::mention::resolve_mentions`) resolves filesystem
//! resources into `ResolvedMention` payloads. Phase C wires three host
//! surfaces onto that resolver:
//!
//!   1. The `ffs mention-search` subcommand (this file).
//!   2. The `ffs_mention_search_json` C ABI (in `crates/ffs-c`).
//!   3. The `ffs_mention_search` MCP tool (in `crates/ffs-mcp`).
//!
//! This subcommand takes a single input string, splits it into tokens, runs
//! a tiny substring-based candidate search against the repo (no FilePicker
//! — that would couple us to the indexed path DB, which is overkill for a
//! mention surface), and then calls `resolve_mentions` for each candidate.
//!
//! The candidate selection is intentionally simple: each non-empty whitespace
//! token becomes a substring query, all hits are unioned, deduped, and
//! passed to the resolver. Phase A (lives on main, not in this worktree)
//! will replace the substring pass with a proper candidate pipeline; the
//! surface stays the same.

use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Parser;
use serde::Serialize;

use ffs_budget::FilterLevel;
use ffs_engine::mention::{resolve_mentions, ResolveOptions, ResolvedMention};

use crate::cli::OutputFormat;

#[derive(Debug, Parser)]
pub struct Args {
    /// Input string. Tokens are split on whitespace; each token becomes a
    /// substring candidate query that is resolved by Phase B.
    pub input: String,

    /// Cursor into the input (1-based, informational only — surfaces
    /// alignment with the MCP `cursor` parameter for future Phase D work).
    #[arg(long)]
    pub cursor: Option<usize>,

    /// Output format. Defaults to JSON for stability across hosts.
    #[arg(long, default_value = "json")]
    pub format: String,

    /// Maximum tokens of body allowed per resolved mention. Default 50_000
    /// (mirrors `ResolveOptions::default`).
    #[arg(long, default_value_t = 50000)]
    pub max_tokens: u32,
}

#[derive(Debug, Serialize)]
struct MentionSearchOutput {
    input: String,
    cursor: Option<usize>,
    candidates: Vec<String>,
    total_candidates: usize,
    mentions: Vec<ResolvedMention>,
    schema: &'static str,
}

/// Run the mention-search subcommand. Walks `root`, finds files whose path
/// contains any of the input's whitespace-separated tokens (case-sensitive
/// substring), then hands the candidate paths to the Phase B resolver.
///
/// `format` is supplied as a string (per the spec's `format` arg). Anything
/// other than `"text"` is treated as JSON for forward compatibility with new
/// formats (yaml, toon, …).
pub fn run(args: Args, root: &Path, default_format: OutputFormat) -> Result<()> {
    let format = match args.format.to_ascii_lowercase().as_str() {
        "text" => OutputFormat::Text,
        _ => OutputFormat::Json,
    };
    // `default_format` is the global `--format` flag from the CLI; the
    // subcommand's own `--format` flag wins when present (and it always is,
    // since we set a default). Kept in the signature for symmetry with
    // other commands and to make it easy to flip the precedence later.
    let _ = default_format;

    let candidates = collect_candidates(root, &args.input);
    let paths: Vec<PathBuf> = candidates.iter().map(PathBuf::from).collect();

    let opts = ResolveOptions {
        max_tokens: args.max_tokens,
        filter_level: FilterLevel::Minimal,
        line_range: None,
    };
    let mentions = resolve_mentions(&paths, &opts);

    let payload = MentionSearchOutput {
        input: args.input,
        cursor: args.cursor,
        total_candidates: candidates.len(),
        candidates,
        mentions,
        schema: "v1",
    };

    super::emit(format, &payload, |p| render_text(p))
}

fn render_text(p: &MentionSearchOutput) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# ffs mention-search ({} candidates, schema {})\n",
        p.total_candidates, p.schema
    ));
    for (i, _c) in p.candidates.iter().enumerate() {
        let m = &p.mentions[i];
        let body = m.content.as_deref().unwrap_or("<none>");
        out.push_str(&format!(
            "\n--- {} [{}] ({} tokens) ---\n{}\n",
            m.path.display(),
            match m.kind {
                ffs_engine::mention::MentionKind::File => "file",
                ffs_engine::mention::MentionKind::Directory => "directory",
                ffs_engine::mention::MentionKind::Image => "image",
            },
            m.token_cost,
            body
        ));
        if let Some(err) = &m.audit.error {
            out.push_str(&format!("! error: {err}\n"));
        }
    }
    out
}

/// Walk `root` and return every absolute path whose stringified form
/// contains any of the whitespace-separated tokens in `input` (case-sensitive
/// substring). Order is arrival order from the parallel walker; we sort +
/// dedup so JSON output is deterministic.
fn collect_candidates(root: &Path, input: &str) -> Vec<String> {
    use std::collections::HashSet;

    let tokens: Vec<&str> = input
        .split_whitespace()
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.is_empty() {
        return Vec::new();
    }

    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();
    for path in super::walk_files(root) {
        let Some(s) = path.to_str() else { continue };
        if tokens.iter().any(|t| s.contains(t)) && seen.insert(s.to_string()) {
            out.push(s.to_string());
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_files(dir: &Path) {
        std::fs::write(dir.join("alpha.rs"), "fn alpha() {}\n").unwrap();
        std::fs::write(dir.join("beta.rs"), "fn beta() {}\n").unwrap();
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        std::fs::write(dir.join("nested/gamma.rs"), "fn gamma() {}\n").unwrap();
    }

    #[test]
    fn collect_candidates_substring_match() {
        let td = tempfile::tempdir().unwrap();
        make_files(td.path());
        let hits = collect_candidates(td.path(), "alpha");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].ends_with("alpha.rs"));
    }

    #[test]
    fn collect_candidates_multiple_tokens_unioned() {
        let td = tempfile::tempdir().unwrap();
        make_files(td.path());
        let hits = collect_candidates(td.path(), "alpha beta");
        assert_eq!(hits.len(), 2);
    }

    #[test]
    fn collect_candidates_empty_input_returns_empty() {
        let td = tempfile::tempdir().unwrap();
        make_files(td.path());
        let hits = collect_candidates(td.path(), "   \t  ");
        assert!(hits.is_empty());
    }

    #[test]
    fn collect_candidates_dedups_when_same_token_repeated() {
        let td = tempfile::tempdir().unwrap();
        make_files(td.path());
        let hits = collect_candidates(td.path(), "alpha alpha");
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn run_produces_resolved_mention_for_matched_file() {
        let td = tempfile::tempdir().unwrap();
        make_files(td.path());
        let args = Args {
            input: "alpha".into(),
            cursor: None,
            format: "json".into(),
            max_tokens: 1000,
        };
        run(args, td.path(), OutputFormat::Json).unwrap();
    }

    #[test]
    fn run_handles_text_format() {
        let td = tempfile::tempdir().unwrap();
        make_files(td.path());
        let args = Args {
            input: "alpha".into(),
            cursor: Some(0),
            format: "text".into(),
            max_tokens: 1000,
        };
        run(args, td.path(), OutputFormat::Text).unwrap();
    }

    #[test]
    fn run_handles_unknown_format_as_json() {
        let td = tempfile::tempdir().unwrap();
        make_files(td.path());
        let args = Args {
            input: "alpha".into(),
            cursor: None,
            format: "yaml".into(),
            max_tokens: 1000,
        };
        run(args, td.path(), OutputFormat::Json).unwrap();
    }

    #[test]
    fn run_returns_no_mentions_for_no_match() {
        let td = tempfile::tempdir().unwrap();
        make_files(td.path());
        let args = Args {
            input: "this_file_does_not_exist_zzz".into(),
            cursor: None,
            format: "json".into(),
            max_tokens: 1000,
        };
        // No candidates => empty Vec<ResolvedMention>. No error.
        run(args, td.path(), OutputFormat::Json).unwrap();
    }
}
