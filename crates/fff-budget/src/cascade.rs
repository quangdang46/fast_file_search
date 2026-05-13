//! View-mode cascade: degrade from full content → outline → signatures-only
//! when a file exceeds its byte budget.

use serde::{Deserialize, Serialize};

use crate::{
    smart_truncate, AggressiveFilter, BudgetSplit, FilterLevel, FilterStrategy, MinimalFilter,
    NoFilter, TruncationOutcome,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewMode {
    Full,
    Outline,
    Signatures,
}

// Abstract outline node so cascade can render `fff_symbol::types::OutlineEntry`
// (or any other outline representation) without depending on fff-symbol.
pub trait OutlineLike {
    fn name(&self) -> &str;
    fn signature(&self) -> Option<&str>;
    fn for_each_child(&self, f: &mut dyn FnMut(&dyn OutlineLike));
}

pub struct CascadeResult {
    pub mode: ViewMode,
    pub body: String,
    pub outcome: TruncationOutcome,
}

// Try full content first; if it exceeds `budget`, fall back to outline; if still
// too large, fall back to signatures-only; last resort: truncate.
pub fn cascade_read(
    content: &str,
    outline: &[&dyn OutlineLike],
    level: FilterLevel,
    split: BudgetSplit,
) -> CascadeResult {
    let body_budget_bytes = (split.body * 4) as usize;

    // 1. Full content with filter applied.
    let filtered = apply_filter(content, level);
    if filtered.len() <= body_budget_bytes {
        let kept = filtered.len();
        let kept_lines = content.lines().count();
        return CascadeResult {
            mode: ViewMode::Full,
            body: filtered,
            outcome: TruncationOutcome {
                kept_lines,
                dropped_lines: 0,
                kept_bytes: kept,
                footer_bytes: 0,
            },
        };
    }

    // 2. Outline rendering.
    let outline_text = render_outline(outline);
    if outline_text.len() <= body_budget_bytes {
        let kept = outline_text.len();
        let kept_lines = outline_text.lines().count();
        return CascadeResult {
            mode: ViewMode::Outline,
            body: outline_text,
            outcome: TruncationOutcome {
                kept_lines,
                dropped_lines: 0,
                kept_bytes: kept,
                footer_bytes: 0,
            },
        };
    }

    // 3. Signatures only.
    let sig_text = render_signatures(outline);
    let (body, outcome) = if sig_text.len() <= body_budget_bytes {
        let kept = sig_text.len();
        let kept_lines = sig_text.lines().count();
        (
            sig_text,
            TruncationOutcome {
                kept_lines,
                dropped_lines: 0,
                kept_bytes: kept,
                footer_bytes: 0,
            },
        )
    } else {
        // 4. Truncate signatures as last resort.
        smart_truncate(&sig_text, body_budget_bytes)
    };

    CascadeResult {
        mode: ViewMode::Signatures,
        body,
        outcome,
    }
}

fn apply_filter(content: &str, level: FilterLevel) -> String {
    let filter: Box<dyn FilterStrategy> = match level {
        FilterLevel::None => Box::new(NoFilter),
        FilterLevel::Minimal => Box::new(MinimalFilter),
        FilterLevel::Aggressive => Box::new(AggressiveFilter),
    };
    filter.apply(content)
}

fn render_outline(entries: &[&dyn OutlineLike]) -> String {
    let mut out = String::new();
    for e in entries {
        render_entry(&mut out, *e, 0);
    }
    out
}

fn render_entry(out: &mut String, e: &dyn OutlineLike, depth: usize) {
    let indent = "  ".repeat(depth);
    if let Some(sig) = e.signature() {
        out.push_str(&format!("{}{} {}\n", indent, e.name(), sig));
    } else {
        out.push_str(&format!("{}{}\n", indent, e.name()));
    }
    e.for_each_child(&mut |c| render_entry(out, c, depth + 1));
}

pub fn render_signatures(entries: &[&dyn OutlineLike]) -> String {
    let mut out = String::new();
    for e in entries {
        if let Some(sig) = e.signature() {
            out.push_str(&format!("{} {}\n", e.name(), sig));
        } else {
            out.push_str(&format!("{}\n", e.name()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone)]
    struct TestEntry {
        name: String,
        signature: Option<String>,
        children: Vec<TestEntry>,
    }

    impl OutlineLike for TestEntry {
        fn name(&self) -> &str {
            &self.name
        }
        fn signature(&self) -> Option<&str> {
            self.signature.as_deref()
        }
        fn for_each_child(&self, f: &mut dyn FnMut(&dyn OutlineLike)) {
            for c in &self.children {
                f(c);
            }
        }
    }

    fn entry(name: &str, sig: Option<&str>, children: Vec<TestEntry>) -> TestEntry {
        TestEntry {
            name: name.to_string(),
            signature: sig.map(|s| s.to_string()),
            children,
        }
    }

    fn as_refs<O: OutlineLike>(entries: &[O]) -> Vec<&dyn OutlineLike> {
        entries.iter().map(|e| e as &dyn OutlineLike).collect()
    }

    #[test]
    fn full_fits_returns_full() {
        let content = "fn main() {}\n";
        let outline = vec![entry("main", Some("fn main()"), vec![])];
        let split = BudgetSplit::default_for(1000);
        let r = cascade_read(content, &as_refs(&outline), FilterLevel::None, split);
        assert_eq!(r.mode, ViewMode::Full);
        assert!(r.body.contains("main"));
    }

    #[test]
    fn falls_back_to_outline_when_full_too_long() {
        // One giant line to force budget overflow on full but fit outline.
        let big = format!("fn main() {{ {} }}\n", "x ".repeat(5000));
        let outline = vec![
            entry("main", Some("fn main()"), vec![]),
            entry("helper", Some("fn helper()"), vec![]),
        ];
        // body=10% of 1000 → 100 tokens × 4 = 400 bytes; big is ~10k, outline is ~30B.
        let split = BudgetSplit::custom(1000, 2, 10, 2);
        let r = cascade_read(&big, &as_refs(&outline), FilterLevel::None, split);
        assert_eq!(r.mode, ViewMode::Outline);
        assert!(r.body.contains("main"));
    }

    #[test]
    fn falls_back_to_signatures_when_outline_too_long() {
        let big = (0..200)
            .map(|i| format!("fn f{i}() {{}}\n"))
            .collect::<String>();
        let outline: Vec<TestEntry> = (0..200)
            .map(|i| {
                entry(
                    &format!("f{i}"),
                    Some(&format!("fn f{i}()")),
                    vec![entry("child", Some("// nested"), vec![])],
                )
            })
            .collect();
        // Tiny body so neither full nor outline (which has children) fits.
        let split = BudgetSplit::custom(10, 2, 5, 2);
        let r = cascade_read(&big, &as_refs(&outline), FilterLevel::None, split);
        assert_eq!(r.mode, ViewMode::Signatures);
    }
}
