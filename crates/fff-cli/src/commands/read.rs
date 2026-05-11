use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{anyhow, Result};
use clap::Parser;
use serde::Serialize;

use fff_budget::{
    apply_preserving_footer, smart_truncate, AggressiveFilter, BudgetSplit, FilterLevel,
    FilterStrategy, MinimalFilter, NoFilter, TruncationOutcome,
};
use fff_engine::{Engine, EngineConfig};
use fff_symbol::lang::detect_file_type;
use fff_symbol::types::{FileType, OutlineEntry};

use crate::cli::OutputFormat;
use crate::commands::outline as outline_cmd;

#[derive(Debug, Parser)]
pub struct Args {
    /// File path to read; may be of the form `path:line` to focus a span.
    pub target: String,

    /// Token budget for the output (default 25000).
    /// Effective byte cap = tokens × ~85% body × 4 bytes/token. The remaining
    /// ~15% is reserved for the response envelope and the truncation footer.
    #[arg(long)]
    pub budget: Option<u64>,

    /// Filter intensity: `none` keeps the file as-is, `minimal` (default)
    /// strips full-line comments, `aggressive` collapses impl/class bodies.
    #[arg(long, default_value = "minimal")]
    pub filter: String,

    /// Return only the structural section (function/class/struct) that
    /// contains the line in `path:line`. Mutually exclusive with `--full`.
    #[arg(long, default_value_t = false, conflicts_with = "full")]
    pub section: bool,

    /// Force whole-file mode and return raw contents. Without this flag the
    /// scry default is the agent-style outline.
    #[arg(long, default_value_t = false)]
    pub full: bool,
}

#[derive(Debug, Serialize)]
struct ReadOutput {
    path: String,
    mode: &'static str,
    body: String,
    kept_bytes: usize,
    footer_bytes: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    section: Option<SectionMeta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolved_from: Option<String>,
}

#[derive(Debug, Serialize)]
struct ReadCandidates {
    needle: String,
    mode: &'static str,
    candidates: Vec<String>,
    total: usize,
}

#[derive(Debug, Serialize)]
struct SectionMeta {
    kind: String,
    name: String,
    start_line: u32,
    end_line: u32,
}

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let level = filter_level(&args.filter);
    let budget = args.budget.unwrap_or(25_000);

    let (path_part, line) = parse_target(&args.target);
    let initial_path = if Path::new(path_part).is_absolute() {
        PathBuf::from(path_part)
    } else {
        root.join(path_part)
    };

    // Bare-filename auto-pick: when the literal path doesn't resolve and the
    // target has no separator, search the workspace by basename. Exactly one
    // exact hit → drill into it; more than one → emit a candidates list so
    // the caller can disambiguate.
    let (path, resolved_from) = match maybe_resolve_bare(root, path_part, &initial_path) {
        BareResolve::Found(p) => (p, Some(path_part.to_string())),
        BareResolve::Ambiguous(candidates) => {
            let payload = ReadCandidates {
                needle: path_part.to_string(),
                mode: "candidates",
                total: candidates.len(),
                candidates: candidates.into_iter().take(5).collect(),
            };
            return super::emit(format, &payload, candidates_text);
        }
        BareResolve::Skip => (initial_path, None),
    };

    // Routing:
    //   --section or `path:N`  → structural section read.
    //   --full                  → whole-file body (legacy default).
    //   no flags, no `:N`       → outline (B0 default for the scry layer).
    if args.section || (line.is_some() && !args.full) {
        let line =
            line.ok_or_else(|| anyhow!("--section requires the target to be in `path:line` form"))?;
        let mut payload = read_section(&path, line, level, budget)?;
        payload.resolved_from = resolved_from;
        return super::emit(format, &payload, section_text);
    }

    if !args.full {
        // Outline default (only for code files; non-code falls through to
        // full-body read so things like Markdown / JSON still emit content).
        if matches!(detect_file_type(&path), FileType::Code(_)) {
            let body = outline_cmd::render_agent(&path, path_part)?;
            let kept_bytes = body.len();
            let payload = ReadOutput {
                path: path.to_string_lossy().to_string(),
                mode: "outline",
                body,
                kept_bytes,
                footer_bytes: 0,
                line: None,
                section: None,
                resolved_from,
            };
            return super::emit(format, &payload, |p| p.body.clone());
        }
    }

    // --full or non-code target: whole-file read via the engine.
    let cfg = EngineConfig {
        filter_level: level,
        total_token_budget: budget,
        ..EngineConfig::default()
    };
    let engine = Engine::new(cfg);
    let res = engine.read(&path);

    let payload = ReadOutput {
        path: res.path.to_string_lossy().to_string(),
        mode: "full",
        body: res.body.clone(),
        kept_bytes: res.outcome.kept_bytes,
        footer_bytes: res.outcome.footer_bytes,
        line,
        section: None,
        resolved_from,
    };
    super::emit(format, &payload, |p| {
        let mut out = String::new();
        if let Some(l) = p.line {
            out.push_str(&format!("// {}:{}\n", p.path, l));
        }
        out.push_str(&p.body);
        out
    })
}

fn filter_level(s: &str) -> FilterLevel {
    match s {
        "none" => FilterLevel::None,
        "aggressive" => FilterLevel::Aggressive,
        _ => FilterLevel::Minimal,
    }
}

fn parse_target(target: &str) -> (&str, Option<u32>) {
    if let Some((p, rest)) = target.rsplit_once(':') {
        if let Ok(n) = rest.parse::<u32>() {
            if n > 0 {
                return (p, Some(n));
            }
        }
    }
    (target, None)
}

fn read_section(path: &Path, line: u32, level: FilterLevel, budget: u64) -> Result<ReadOutput> {
    let lang = match detect_file_type(path) {
        FileType::Code(l) => l,
        _ => return Err(anyhow!("not a code file: {}", path.display())),
    };

    let max_bytes_default = EngineConfig::default().max_bytes_per_result;
    let cfg = EngineConfig {
        filter_level: level,
        total_token_budget: budget,
        ..EngineConfig::default()
    };
    let engine = Engine::new(cfg);

    let content =
        std::fs::read_to_string(path).map_err(|e| anyhow!("read {}: {e}", path.display()))?;
    let mtime = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let outline = engine
        .handles
        .outlines
        .get_or_compute(path, mtime, &content, lang);

    let entry = deepest_containing(&outline, line)
        .ok_or_else(|| anyhow!("no structural section contains line {line}"))?;

    let slice = slice_lines(&content, entry.start_line, entry.end_line);

    let filter: Box<dyn FilterStrategy> = match level {
        FilterLevel::None => Box::new(NoFilter),
        FilterLevel::Minimal => Box::new(MinimalFilter),
        FilterLevel::Aggressive => Box::new(AggressiveFilter),
    };
    let filtered = filter.apply(&slice);

    let split = BudgetSplit::default_for(budget);
    let body_budget_bytes = (split.body * 4) as usize;
    let max_bytes = body_budget_bytes.min(max_bytes_default);

    let (body, outcome) = budgeted(&filtered, max_bytes);

    Ok(ReadOutput {
        path: path.to_string_lossy().to_string(),
        mode: "section",
        body,
        kept_bytes: outcome.kept_bytes,
        footer_bytes: outcome.footer_bytes,
        line: Some(line),
        section: Some(SectionMeta {
            kind: format!("{:?}", entry.kind).to_lowercase(),
            name: entry.name.clone(),
            start_line: entry.start_line,
            end_line: entry.end_line,
        }),
        resolved_from: None,
    })
}

enum BareResolve {
    Found(PathBuf),
    Ambiguous(Vec<String>),
    Skip,
}

// Trigger only when target has no path separator and the literal path does
// not resolve. Exact-case basename match preferred; fall back to
// case-insensitive only when exact-case yields zero.
fn maybe_resolve_bare(root: &Path, target: &str, initial: &Path) -> BareResolve {
    if target.contains('/') || target.contains('\\') {
        return BareResolve::Skip;
    }
    if initial.exists() {
        return BareResolve::Skip;
    }
    if target.is_empty() {
        return BareResolve::Skip;
    }
    let files = super::walk_files(root);
    let mut exact: Vec<PathBuf> = files
        .iter()
        .filter(|p| p.file_name().and_then(|n| n.to_str()) == Some(target))
        .cloned()
        .collect();
    if exact.is_empty() {
        let lower = target.to_lowercase();
        exact = files
            .iter()
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.to_lowercase() == lower)
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
    }
    match exact.len() {
        0 => BareResolve::Skip,
        1 => BareResolve::Found(exact.remove(0)),
        _ => {
            // Deterministic ordering for ambiguous matches: shortest path
            // (likely the closest to root) first, then alphabetical.
            exact.sort_by(|a, b| {
                a.as_os_str()
                    .len()
                    .cmp(&b.as_os_str().len())
                    .then_with(|| a.cmp(b))
            });
            BareResolve::Ambiguous(
                exact
                    .into_iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect(),
            )
        }
    }
}

fn candidates_text(p: &ReadCandidates) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "ambiguous: `{}` matches {} files; pass one as the full path or pin with `path:line`.\n",
        p.needle, p.total
    ));
    for c in &p.candidates {
        out.push_str("  ");
        out.push_str(c);
        out.push('\n');
    }
    if p.total > p.candidates.len() {
        out.push_str(&format!(
            "  … {} more not shown\n",
            p.total - p.candidates.len()
        ));
    }
    out
}

fn deepest_containing(entries: &[OutlineEntry], line: u32) -> Option<OutlineEntry> {
    let mut best: Option<OutlineEntry> = None;
    for e in entries {
        if e.start_line <= line && line <= e.end_line {
            // Prefer a deeper child if one also contains the line.
            if let Some(child) = deepest_containing(&e.children, line) {
                best = Some(child);
            } else {
                best = Some(e.clone());
            }
        }
    }
    best
}

fn slice_lines(content: &str, start_line: u32, end_line: u32) -> String {
    let start = start_line.saturating_sub(1) as usize;
    let end = end_line as usize;
    let mut out = String::new();
    for (i, line) in content.lines().enumerate() {
        if i >= start && i < end {
            out.push_str(line);
            out.push('\n');
        }
        if i >= end {
            break;
        }
    }
    out
}

fn budgeted(filtered: &str, max_bytes: usize) -> (String, TruncationOutcome) {
    let mut buf = String::new();
    let footer = "[truncated to budget]\n";
    let outcome = if filtered.len() <= max_bytes {
        let (out, oc) = smart_truncate(filtered, max_bytes);
        buf.push_str(&out);
        oc
    } else {
        apply_preserving_footer(&mut buf, max_bytes, footer, |target, budget| {
            let take = filtered.len().min(budget);
            target.push_str(&filtered[..take]);
            take
        })
    };
    (buf, outcome)
}

fn section_text(p: &ReadOutput) -> String {
    let mut out = String::new();
    if let Some(s) = &p.section {
        out.push_str(&format!(
            "// {} {} (lines {}-{})\n",
            s.kind, s.name, s.start_line, s.end_line
        ));
    } else if let Some(l) = p.line {
        out.push_str(&format!("// {}:{}\n", p.path, l));
    }
    out.push_str(&p.body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use fff_symbol::types::OutlineKind;

    fn entry(
        kind: OutlineKind,
        name: &str,
        start: u32,
        end: u32,
        children: Vec<OutlineEntry>,
    ) -> OutlineEntry {
        OutlineEntry {
            kind,
            name: name.to_string(),
            start_line: start,
            end_line: end,
            signature: None,
            children,
            doc: None,
        }
    }

    #[test]
    fn bare_resolve_skips_when_target_has_separator() {
        let td = tempfile::tempdir().unwrap();
        let initial = td.path().join("dir/foo.rs");
        let r = maybe_resolve_bare(td.path(), "dir/foo.rs", &initial);
        assert!(matches!(r, BareResolve::Skip));
    }

    #[test]
    fn bare_resolve_skips_when_initial_exists() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("foo.rs"), "// hi").unwrap();
        let initial = td.path().join("foo.rs");
        let r = maybe_resolve_bare(td.path(), "foo.rs", &initial);
        assert!(matches!(r, BareResolve::Skip));
    }

    #[test]
    fn bare_resolve_single_exact_match_returns_found() {
        let td = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(td.path().join("src")).unwrap();
        std::fs::write(td.path().join("src/uniq.rs"), "fn x() {}").unwrap();
        let initial = td.path().join("uniq.rs");
        match maybe_resolve_bare(td.path(), "uniq.rs", &initial) {
            BareResolve::Found(p) => {
                assert!(p.ends_with("src/uniq.rs"));
            }
            _ => panic!("expected Found"),
        }
    }

    #[test]
    fn bare_resolve_multi_match_returns_ambiguous_sorted_shortest_first() {
        let td = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(td.path().join("a/b/c")).unwrap();
        std::fs::create_dir_all(td.path().join("z")).unwrap();
        std::fs::write(td.path().join("a/b/c/dup.rs"), "fn x() {}").unwrap();
        std::fs::write(td.path().join("z/dup.rs"), "fn x() {}").unwrap();
        let initial = td.path().join("dup.rs");
        match maybe_resolve_bare(td.path(), "dup.rs", &initial) {
            BareResolve::Ambiguous(v) => {
                assert_eq!(v.len(), 2);
                // shortest path first
                assert!(v[0].ends_with("z/dup.rs"));
                assert!(v[1].ends_with("a/b/c/dup.rs"));
            }
            _ => panic!("expected Ambiguous"),
        }
    }

    #[test]
    fn bare_resolve_case_insensitive_fallback_when_exact_zero() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("README.md"), "# hi").unwrap();
        let initial = td.path().join("readme.md");
        match maybe_resolve_bare(td.path(), "readme.md", &initial) {
            BareResolve::Found(p) => assert!(p.ends_with("README.md")),
            _ => panic!("expected case-insensitive match"),
        }
    }

    #[test]
    fn bare_resolve_skip_when_no_matches() {
        let td = tempfile::tempdir().unwrap();
        let initial = td.path().join("missing.rs");
        let r = maybe_resolve_bare(td.path(), "missing.rs", &initial);
        assert!(matches!(r, BareResolve::Skip));
    }

    #[test]
    fn candidates_text_lists_paths_and_caps_at_five() {
        let p = ReadCandidates {
            needle: "x".into(),
            mode: "candidates",
            total: 7,
            candidates: (0..5).map(|i| format!("a/{i}.rs")).collect(),
        };
        let s = candidates_text(&p);
        assert!(s.contains("ambiguous: `x` matches 7 files"));
        assert!(s.contains("a/0.rs"));
        assert!(s.contains("a/4.rs"));
        assert!(s.contains("2 more not shown"));
    }

    #[test]
    fn parses_path_and_line_when_present() {
        assert_eq!(parse_target("foo.rs:42"), ("foo.rs", Some(42)));
        assert_eq!(parse_target("a/b/foo.rs:1"), ("a/b/foo.rs", Some(1)));
    }

    #[test]
    fn parses_path_only_when_no_colon_or_non_numeric_suffix() {
        assert_eq!(parse_target("foo.rs"), ("foo.rs", None));
        assert_eq!(parse_target("foo.rs:bar"), ("foo.rs:bar", None));
        assert_eq!(parse_target("foo.rs:0"), ("foo.rs:0", None));
    }

    #[test]
    fn deepest_containing_returns_innermost_match() {
        let outline = vec![entry(
            OutlineKind::Class,
            "Cls",
            1,
            50,
            vec![entry(OutlineKind::Function, "method", 10, 30, vec![])],
        )];
        let inner = deepest_containing(&outline, 20).expect("found");
        assert_eq!(inner.name, "method");
        let outer = deepest_containing(&outline, 5).expect("found");
        assert_eq!(outer.name, "Cls");
    }

    #[test]
    fn deepest_containing_returns_none_when_no_match() {
        let outline = vec![entry(OutlineKind::Function, "f", 10, 20, vec![])];
        assert!(deepest_containing(&outline, 5).is_none());
        assert!(deepest_containing(&outline, 25).is_none());
    }

    #[test]
    fn slice_lines_is_inclusive_of_both_ends() {
        let content = "L1\nL2\nL3\nL4\nL5\n";
        assert_eq!(slice_lines(content, 2, 4), "L2\nL3\nL4\n");
        assert_eq!(slice_lines(content, 1, 1), "L1\n");
        assert_eq!(slice_lines(content, 5, 5), "L5\n");
    }

    #[test]
    fn slice_lines_clamps_when_end_exceeds_file() {
        let content = "L1\nL2\n";
        assert_eq!(slice_lines(content, 1, 99), "L1\nL2\n");
    }
}
