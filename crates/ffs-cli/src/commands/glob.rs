use std::path::Path;

use anyhow::Result;
use clap::Parser;
use serde::Serialize;

use crate::cli::OutputFormat;
use crate::commands::pagination::footer;

#[derive(Debug, Parser)]
pub struct Args {
    /// Glob pattern (e.g. `**/*.rs`).
    pub pattern: String,

    /// Limit number of results emitted.
    #[arg(long, default_value_t = 200)]
    pub limit: usize,

    /// Skip this many matches before starting the page. Use together with
    /// `--limit` to page past a truncated result set (see the `[N-M of
    /// total]` footer).
    #[arg(long, default_value_t = 0)]
    pub offset: usize,
}

#[derive(Debug, Serialize)]
struct GlobResult {
    pattern: String,
    matches: Vec<String>,
    /// Matches skipped before this page (`--offset`).
    offset: usize,
    /// True when at least one more match exists beyond this page. Because
    /// the underlying scan stops at `offset + limit` matches (to stay fast
    /// on huge trees), `total_matches_at_least` is exact only when this is
    /// false — otherwise it's a lower bound.
    truncated: bool,
    /// Number of matches found up to the `offset + limit` cutoff. Exact
    /// total when `truncated` is false; a lower bound otherwise.
    total_matches_at_least: usize,
}

/// Delegate to the shared core glob function which handles Windows correctly
/// (falling back to `globset::Glob` + `ignore::WalkBuilder` on unsupported
/// platforms) and respects gitignore rules consistently.
fn glob_files(_root: &Path, pattern: &str, limit: usize) -> Vec<String> {
    ffs_search::glob_matcher::glob_files(_root, pattern, limit)
}

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let offset = args.offset;
    // Fetch offset+limit worth of matches so we can slice off `offset`
    // without asking the underlying scan for the whole (potentially huge)
    // result set.
    let take = offset.saturating_add(args.limit);
    let mut matches = glob_files(root, &args.pattern, take);
    let total_matches_at_least = matches.len();
    let truncated = matches.len() >= take && take > 0;
    if offset > 0 {
        matches = if offset >= matches.len() {
            Vec::new()
        } else {
            matches.split_off(offset)
        };
    }
    let payload = GlobResult {
        pattern: args.pattern,
        matches,
        offset,
        truncated,
        total_matches_at_least,
    };
    super::emit(format, &payload, render_text)
}

fn render_text(p: &GlobResult) -> String {
    let mut out = String::new();
    // "[no matches]" is only honest when nothing matched at all — an empty
    // page can also mean `--offset` skipped past the last match.
    if p.matches.is_empty() && p.total_matches_at_least == 0 {
        out.push_str(&format!("[no matches for `{}`]\n", p.pattern));
        return out;
    }
    for m in &p.matches {
        out.push_str(m);
        out.push('\n');
    }
    let returned = p.matches.len();
    let has_more = p.truncated || p.offset + returned < p.total_matches_at_least;
    out.push_str(&footer(
        p.total_matches_at_least,
        p.offset,
        returned,
        has_more,
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(matches: Vec<String>, offset: usize, total: usize, truncated: bool) -> GlobResult {
        GlobResult {
            pattern: "*.rs".to_string(),
            matches,
            offset,
            truncated,
            total_matches_at_least: total,
        }
    }

    #[test]
    fn no_matches_only_when_nothing_found() {
        let out = render_text(&payload(Vec::new(), 0, 0, false));
        assert!(out.contains("[no matches"), "got: {out:?}");
    }

    #[test]
    fn empty_page_past_offset_is_not_no_matches() {
        // 5 matches exist but --offset 10 skipped past all of them.
        let out = render_text(&payload(Vec::new(), 10, 5, false));
        assert!(!out.contains("[no matches"), "got: {out:?}");
    }

    #[test]
    fn partial_page_renders_footer() {
        let out = render_text(&payload(vec!["a.rs".into(), "b.rs".into()], 0, 5, true));
        assert!(out.contains("a.rs"));
        assert!(out.contains("[1-2 of 5]"), "got: {out:?}");
        assert!(out.contains("--offset 2"));
    }
}
