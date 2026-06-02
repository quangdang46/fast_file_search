//! Param shapes and a small in-process helper for the `ffs_mention_search`
//! MCP tool. The handler itself lives in `server.rs` (it needs access to
//! `FfsServer::picker`); this module keeps the schema and the JSON
//! serialization in one place so the C ABI surface and the MCP surface
//! can share the resolved-payload shape.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ffs_budget::FilterLevel;
use ffs_engine::mention::{resolve_mentions, ResolveOptions, ResolvedMention};
use serde::Serialize;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct MentionSearchParams {
    /// Input string. Split on whitespace; each token becomes a substring
    /// candidate query against the workspace, then resolved by Phase B.
    pub input: String,
    /// Maximum tokens of body allowed per resolved mention (default 50_000,
    /// mirrors `ResolveOptions::default`).
    #[serde(rename = "maxTokens")]
    pub max_tokens: Option<f64>,
    /// Optional line-range slice applied before `smart_truncate`. Two
    /// 1-based line numbers, inclusive.
    #[serde(rename = "lineRange")]
    pub line_range: Option<Vec<f64>>,
    /// Filter intensity: `"none"`, `"minimal"` (default), or `"aggressive"`.
    #[serde(rename = "filterLevel")]
    pub filter_level: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MentionSearchOutput {
    pub input: String,
    pub candidates: Vec<String>,
    pub total_candidates: usize,
    pub mentions: Vec<ResolvedMention>,
    pub schema: &'static str,
}

/// Build the `ResolveOptions` from a `MentionSearchParams`. Pulled out of
/// the handler so the test in `mod tests` can exercise the conversion
/// without going through the full MCP plumbing.
pub fn build_resolve_options(
    max_tokens: Option<f64>,
    line_range: Option<Vec<f64>>,
    filter_level: Option<&str>,
) -> ResolveOptions {
    let max_tokens = max_tokens
        .filter(|v| v.is_finite() && *v > 0.0)
        .map(|v| v.round() as u32)
        .unwrap_or(50_000);
    let filter_level = match filter_level {
        Some("none") => FilterLevel::None,
        Some("aggressive") => FilterLevel::Aggressive,
        _ => FilterLevel::Minimal,
    };
    let line_range = line_range.and_then(|v| {
        if v.len() == 2 {
            let s = v[0].round().max(1.0) as u32;
            let e = v[1].round().max(s as f64) as u32;
            Some((s, e))
        } else {
            None
        }
    });
    ResolveOptions {
        max_tokens,
        filter_level,
        line_range,
    }
}

/// Run the mention pipeline: walk `root`, substring-filter by the
/// whitespace-separated tokens of `input`, hand the candidates to the
/// Phase B resolver, and serialize the result. The function is shared
/// between the MCP handler and the tests.
pub fn run_mention_pipeline(input: &str, root: &Path, opts: &ResolveOptions) -> MentionSearchOutput {
    let candidates = collect_candidates(root, input);
    let paths: Vec<PathBuf> = candidates.iter().map(PathBuf::from).collect();
    let mentions = resolve_mentions(&paths, opts);
    MentionSearchOutput {
        input: input.to_string(),
        total_candidates: candidates.len(),
        candidates,
        mentions,
        schema: "v1",
    }
}

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
    for entry in walk_files_simple(root) {
        let Some(s) = entry.to_str() else { continue };
        if tokens.iter().any(|t| s.contains(t)) && seen.insert(s.to_string()) {
            out.push(s.to_string());
        }
    }
    out.sort();
    out
}

fn walk_files_simple(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .standard_filters(true)
        .follow_links(false)
        .threads(2)
        .build();
    for entry in walker.flatten() {
        if entry.file_type().is_some_and(|t| t.is_file()) {
            out.push(entry.into_path());
        }
    }
    out
}

/// Serialize an `Arc<str>`-like owned `MentionSearchOutput` to a JSON
/// string. Returns an error string on failure so the MCP handler can wrap
/// it in `ErrorData` without unwrapping the JSON path.
pub fn output_to_json(out: &MentionSearchOutput) -> Result<String, String> {
    serde_json::to_string(out).map_err(|e| format!("serialize MentionSearchOutput: {e}"))
}

// Suppress dead-code warnings for items kept for the forthcoming Phase D
// provider hookup (mentioned in the v2 plan).
#[allow(dead_code)]
fn _phase_d_anchor(_: Arc<()>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    fn write_files(dir: &Path) {
        std::fs::write(dir.join("alpha.rs"), b"fn alpha() {}\n").unwrap();
        std::fs::write(dir.join("beta.rs"), b"fn beta() {}\n").unwrap();
        std::fs::create_dir_all(dir.join("nested")).unwrap();
        std::fs::write(dir.join("nested/gamma.rs"), b"fn gamma() {}\n").unwrap();
    }

    #[test]
    fn params_parse_minimal() {
        let p: MentionSearchParams = serde_json::from_value(json!({ "input": "alpha" })).unwrap();
        assert_eq!(p.input, "alpha");
        assert!(p.max_tokens.is_none());
        assert!(p.line_range.is_none());
        assert!(p.filter_level.is_none());
    }

    #[test]
    fn params_parse_full() {
        let p: MentionSearchParams = serde_json::from_value(json!({
            "input": "alpha",
            "maxTokens": 2500.0,
            "lineRange": [10.0, 20.0],
            "filterLevel": "aggressive",
        }))
        .unwrap();
        assert_eq!(p.max_tokens, Some(2500.0));
        assert_eq!(p.line_range.as_ref().unwrap().len(), 2);
        assert_eq!(p.filter_level.as_deref(), Some("aggressive"));
    }

    #[test]
    fn params_rejects_missing_input() {
        let r: Result<MentionSearchParams, _> =
            serde_json::from_value(json!({ "maxTokens": 1.0 }));
        assert!(r.is_err());
    }

    #[test]
    fn build_resolve_options_defaults() {
        let o = build_resolve_options(None, None, None);
        assert_eq!(o.max_tokens, 50_000);
        assert_eq!(o.filter_level, FilterLevel::Minimal);
        assert_eq!(o.line_range, None);
    }

    #[test]
    fn build_resolve_options_handles_zero_max_tokens() {
        // 0 must NOT become 0 tokens in the budget; falls back to default.
        let o = build_resolve_options(Some(0.0), None, None);
        assert_eq!(o.max_tokens, 50_000);
    }

    #[test]
    fn build_resolve_options_filter_levels() {
        assert_eq!(
            build_resolve_options(None, None, Some("none")).filter_level,
            FilterLevel::None
        );
        assert_eq!(
            build_resolve_options(None, None, Some("aggressive")).filter_level,
            FilterLevel::Aggressive
        );
        assert_eq!(
            build_resolve_options(None, None, Some("garbage")).filter_level,
            FilterLevel::Minimal
        );
    }

    #[test]
    fn run_mention_pipeline_returns_resolved_mentions() {
        let td = tempdir().unwrap();
        write_files(td.path());
        let opts = build_resolve_options(Some(1000.0), None, None);
        let out = run_mention_pipeline("alpha", td.path(), &opts);
        assert_eq!(out.total_candidates, 1);
        assert!(out.mentions[0].content.as_deref().unwrap().contains("alpha"));
    }

    #[test]
    fn run_mention_pipeline_empty_input() {
        let td = tempdir().unwrap();
        write_files(td.path());
        let opts = build_resolve_options(None, None, None);
        let out = run_mention_pipeline("   ", td.path(), &opts);
        assert_eq!(out.total_candidates, 0);
        assert!(out.mentions.is_empty());
    }

    #[test]
    fn output_to_json_round_trips() {
        let td = tempdir().unwrap();
        write_files(td.path());
        let opts = build_resolve_options(None, None, None);
        let out = run_mention_pipeline("alpha", td.path(), &opts);
        let json = output_to_json(&out).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed["candidates"].is_array());
        assert!(parsed["mentions"].is_array());
        assert_eq!(parsed["schema"], "v1");
    }
}
