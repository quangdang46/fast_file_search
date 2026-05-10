//! `--expand` body inlining for `scry symbol`.
//!
//! Given the locations returned by `lookup_exact`, walk each one back to its
//! containing structural node via the outline cache, slice the body bytes,
//! and append a `── calls ──` block listing direct callees resolved to
//! their definition sites.

use std::path::PathBuf;
use std::time::SystemTime;

use serde::Serialize;

use fff_budget::{smart_truncate, TruncationOutcome};
use fff_engine::Engine;
use fff_symbol::lang::detect_file_type;
use fff_symbol::types::{FileType, Lang, OutlineEntry};

use crate::commands::callees_resolve::collect_callees;

pub const CALLS_DIVIDER: &str = "── calls ──";

#[derive(Debug, Serialize)]
pub struct Expanded {
    pub body: String,
    pub start_line: u32,
    pub end_line: u32,
    pub calls: Vec<ExpandedCall>,
    pub kept_bytes: usize,
    pub footer_bytes: usize,
}

#[derive(Debug, Serialize)]
pub struct ExpandedCall {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

pub fn expand_hit(
    engine: &Engine,
    self_name: &str,
    path: &PathBuf,
    line: u32,
    end_line: u32,
    per_hit_byte_budget: usize,
) -> Option<Expanded> {
    let lang = match detect_file_type(path) {
        FileType::Code(l) => l,
        _ => return None,
    };

    let content = std::fs::read_to_string(path).ok()?;
    let mtime = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let outline = engine
        .handles
        .outlines
        .get_or_compute(path, mtime, &content, lang);

    let (start_line, end_line) = entry_for(&outline, line)
        .map(|e| (e.start_line, e.end_line))
        .unwrap_or((line, end_line));

    let body_slice = slice_lines(&content, start_line, end_line);
    let calls = direct_callees(engine, self_name, &content, lang, start_line, end_line);

    let calls_block = render_calls_block(&calls);
    let combined = format!("{body_slice}{calls_block}");

    let (final_body, outcome) = budgeted(&combined, per_hit_byte_budget);

    Some(Expanded {
        body: final_body,
        start_line,
        end_line,
        calls,
        kept_bytes: outcome.kept_bytes,
        footer_bytes: outcome.footer_bytes,
    })
}

fn entry_for(entries: &[OutlineEntry], line: u32) -> Option<OutlineEntry> {
    let mut best: Option<OutlineEntry> = None;
    for e in entries {
        if e.start_line <= line && line <= e.end_line {
            if let Some(child) = entry_for(&e.children, line) {
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
        if i >= end {
            break;
        }
        if i >= start {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn direct_callees(
    engine: &Engine,
    self_name: &str,
    content: &str,
    lang: Lang,
    start_line: u32,
    end_line: u32,
) -> Vec<ExpandedCall> {
    let names = match collect_callees(content, lang, start_line, end_line) {
        Some(s) => s,
        None => return Vec::new(),
    };
    let mut out: Vec<ExpandedCall> = Vec::new();
    for name in names {
        if name == self_name {
            continue;
        }
        let mut locs = engine.handles.symbols.lookup_exact(&name);
        if let Some(loc) = locs.pop() {
            out.push(ExpandedCall {
                name,
                path: Some(loc.path.to_string_lossy().to_string()),
                line: Some(loc.line),
            });
        } else {
            out.push(ExpandedCall {
                name,
                path: None,
                line: None,
            });
        }
    }
    out
}

fn render_calls_block(calls: &[ExpandedCall]) -> String {
    if calls.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    out.push_str(CALLS_DIVIDER);
    out.push('\n');
    for c in calls {
        match (c.path.as_deref(), c.line) {
            (Some(p), Some(l)) => out.push_str(&format!("{} @ {p}:{l}\n", c.name)),
            _ => out.push_str(&format!("{} (unresolved)\n", c.name)),
        }
    }
    out
}

fn budgeted(body: &str, max_bytes: usize) -> (String, TruncationOutcome) {
    if max_bytes == 0 {
        let total_lines = body.lines().count();
        let outcome = TruncationOutcome {
            kept_lines: 0,
            dropped_lines: total_lines,
            kept_bytes: 0,
            footer_bytes: 0,
        };
        return (String::new(), outcome);
    }
    smart_truncate(body, max_bytes)
}

/// Per-hit byte budget = total body budget split evenly across visible hits,
/// with a 256-byte floor so even very small budgets emit something useful.
pub fn per_hit_budget_bytes(total_token_budget: u64, hit_count: usize) -> usize {
    let split = fff_budget::BudgetSplit::default_for(total_token_budget);
    let body_bytes = (split.body * 4) as usize;
    if hit_count == 0 {
        return body_bytes;
    }
    (body_bytes / hit_count).max(256)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fff_symbol::types::OutlineKind;

    fn entry(kind: OutlineKind, name: &str, start: u32, end: u32) -> OutlineEntry {
        OutlineEntry {
            kind,
            name: name.to_string(),
            start_line: start,
            end_line: end,
            signature: None,
            children: vec![],
            doc: None,
        }
    }

    #[test]
    fn slice_lines_returns_inclusive_range() {
        let s = "L1\nL2\nL3\nL4\n";
        assert_eq!(slice_lines(s, 2, 3), "L2\nL3\n");
        assert_eq!(slice_lines(s, 1, 1), "L1\n");
    }

    #[test]
    fn entry_for_picks_deepest_match() {
        let outline = vec![OutlineEntry {
            kind: OutlineKind::Class,
            name: "Cls".to_string(),
            start_line: 1,
            end_line: 50,
            signature: None,
            children: vec![entry(OutlineKind::Function, "method", 10, 30)],
            doc: None,
        }];
        let inner = entry_for(&outline, 20).expect("found");
        assert_eq!(inner.name, "method");
        let outer = entry_for(&outline, 5).expect("found");
        assert_eq!(outer.name, "Cls");
    }

    #[test]
    fn render_calls_block_formats_resolved_and_unresolved() {
        let calls = vec![
            ExpandedCall {
                name: "foo".to_string(),
                path: Some("a.rs".to_string()),
                line: Some(10),
            },
            ExpandedCall {
                name: "missing".to_string(),
                path: None,
                line: None,
            },
        ];
        let out = render_calls_block(&calls);
        assert!(out.starts_with(CALLS_DIVIDER));
        assert!(out.contains("foo @ a.rs:10"));
        assert!(out.contains("missing (unresolved)"));
    }

    #[test]
    fn render_calls_block_empty_for_no_calls() {
        assert_eq!(render_calls_block(&[]), "");
    }

    #[test]
    fn budgeted_truncates_when_over() {
        let body = (0..200).map(|i| format!("line{i}\n")).collect::<String>();
        let (out, oc) = budgeted(&body, 200);
        assert!(oc.dropped_lines > 0);
        assert!(out.len() <= 200);
    }

    #[test]
    fn budgeted_zero_budget_emits_empty() {
        let (out, oc) = budgeted("hello\nworld\n", 0);
        assert!(out.is_empty());
        assert_eq!(oc.kept_bytes, 0);
        assert_eq!(oc.dropped_lines, 2);
    }

    #[test]
    fn per_hit_budget_has_floor() {
        // Tiny budget across many hits still yields ≥ 256 bytes per hit.
        assert!(per_hit_budget_bytes(10, 100) >= 256);
    }

    #[test]
    fn per_hit_budget_splits_evenly() {
        let one = per_hit_budget_bytes(10_000, 1);
        let two = per_hit_budget_bytes(10_000, 2);
        assert!(one >= two);
    }
}
