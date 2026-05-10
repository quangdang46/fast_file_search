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

    /// Force whole-file mode even if `path:line` was given. Default if no
    /// flag is set, but lets you pin behaviour explicitly.
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
    let path = if Path::new(path_part).is_absolute() {
        PathBuf::from(path_part)
    } else {
        root.join(path_part)
    };

    if args.section {
        let line =
            line.ok_or_else(|| anyhow!("--section requires the target to be in `path:line` form"))?;
        let payload = read_section(&path, line, level, budget)?;
        return super::emit(format, &payload, section_text);
    }

    // --full or default: whole-file read via the engine.
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
    })
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
