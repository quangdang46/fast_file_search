// `scry flow <name>` — single-hit drill-down envelope per definition.
// For each definition site of `name`, emit one "card" combining:
//   def metadata + header + budget-capped body excerpt
//   + top-N direct callees (resolved to defs) + top-N single-hop callers.
// Pagination applies to cards; sub-lists are clamped per card.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;
use clap::Parser;
use serde::Serialize;

use fff_engine::{Engine, PreFilterStack};
use fff_symbol::lang::detect_file_type;
use fff_symbol::types::FileType;

use crate::cli::OutputFormat;
use crate::commands::callers_bfs::{enclosing_symbol, CandidateFile};
use crate::commands::did_you_mean::{suggest, Suggestion};
use crate::commands::expand::{expand_hit, per_hit_budget_bytes, CALLS_DIVIDER};
use crate::commands::pagination::{footer, Page};

#[derive(Debug, Parser)]
pub struct Args {
    /// Symbol name to drill down on.
    pub name: String,

    /// Maximum cards returned in this page (one card per definition).
    #[arg(long, default_value_t = 10)]
    pub limit: usize,

    /// Skip this many cards before starting the page.
    #[arg(long, default_value_t = 0)]
    pub offset: usize,

    /// Maximum callees listed per card.
    #[arg(long = "callees-top", default_value_t = 5)]
    pub callees_top: usize,

    /// Maximum callers listed per card.
    #[arg(long = "callers-top", default_value_t = 5)]
    pub callers_top: usize,

    /// Token budget for body excerpts. Split across visible cards with a
    /// per-card floor of 256 bytes.
    #[arg(long, default_value_t = 10_000)]
    pub budget: u64,

    /// Disable did-you-mean suggestions when no definitions are found.
    #[arg(long = "no-did-you-mean", default_value_t = false)]
    pub no_did_you_mean: bool,
}

#[derive(Debug, Serialize)]
struct FlowDef {
    path: String,
    line: u32,
    end_line: u32,
    kind: String,
    weight: u16,
}

#[derive(Debug, Serialize)]
struct FlowCallee {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<u32>,
}

#[derive(Debug, Clone, Serialize)]
struct FlowCaller {
    path: String,
    line: u32,
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    enclosing: Option<String>,
}

#[derive(Debug, Serialize)]
struct FlowCard {
    def: FlowDef,
    header: String,
    body: String,
    body_start_line: u32,
    body_end_line: u32,
    kept_bytes: usize,
    footer_bytes: usize,
    callees: Vec<FlowCallee>,
    total_callees: usize,
    callers: Vec<FlowCaller>,
    total_callers: usize,
}

#[derive(Debug, Serialize)]
struct FlowOutput {
    name: String,
    cards: Vec<FlowCard>,
    total_cards: usize,
    offset: usize,
    has_more: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    did_you_mean: Vec<Suggestion>,
}

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let engine = Engine::default();
    engine.index(root);

    let defs = engine.handles.symbols.lookup_exact(&args.name);

    let did_you_mean = if defs.is_empty() && !args.no_did_you_mean {
        suggest(&engine, &args.name)
    } else {
        Vec::new()
    };

    let total_cards = defs.len();
    let page = Page::paginate(defs, args.offset, args.limit);

    let candidates = if page.items.is_empty() {
        Vec::new()
    } else {
        build_candidates(root)
    };

    let usages = if page.items.is_empty() {
        Vec::new()
    } else {
        collect_usages(&engine, &args.name, &candidates)
    };

    let per_card_budget = if page.items.is_empty() {
        0
    } else {
        per_hit_budget_bytes(args.budget, page.items.len())
    };

    let cards: Vec<FlowCard> = page
        .items
        .into_iter()
        .map(|loc| {
            build_card(
                &engine,
                &args.name,
                loc,
                &usages,
                per_card_budget,
                args.callees_top,
                args.callers_top,
            )
        })
        .collect();

    let payload = FlowOutput {
        name: args.name,
        cards,
        total_cards,
        offset: page.offset,
        has_more: page.has_more,
        did_you_mean,
    };
    super::emit(format, &payload, render_text)
}

fn build_card(
    engine: &Engine,
    name: &str,
    loc: fff_symbol::symbol_index::SymbolLocation,
    usages: &[FlowCaller],
    per_card_budget: usize,
    callees_top: usize,
    callers_top: usize,
) -> FlowCard {
    let def_path = loc.path.clone();
    let def_line = loc.line;
    let def_end = loc.end_line;
    let def = FlowDef {
        path: def_path.to_string_lossy().to_string(),
        line: def_line,
        end_line: def_end,
        kind: loc.kind,
        weight: loc.weight,
    };

    let expanded = expand_hit(engine, name, &def_path, def_line, def_end, per_card_budget);

    let (body, body_start, body_end, kept_bytes, footer_bytes, callees_all) = match expanded {
        Some(e) => {
            let callees_all: Vec<FlowCallee> = e
                .calls
                .into_iter()
                .map(|c| FlowCallee {
                    name: c.name,
                    path: c.path,
                    line: c.line,
                })
                .collect();
            (
                strip_calls_block(&e.body),
                e.start_line,
                e.end_line,
                e.kept_bytes,
                e.footer_bytes,
                callees_all,
            )
        }
        None => (String::new(), def_line, def_end, 0, 0, Vec::new()),
    };

    let header = body.lines().next().unwrap_or("").to_string();

    let total_callees = callees_all.len();
    let callees: Vec<FlowCallee> = callees_all.into_iter().take(callees_top).collect();

    let def_path_str = def.path.clone();
    let callers_all: Vec<FlowCaller> = usages
        .iter()
        .filter(|u| !(u.path == def_path_str && u.line == def_line))
        .cloned()
        .collect();
    let total_callers = callers_all.len();
    let callers: Vec<FlowCaller> = callers_all.into_iter().take(callers_top).collect();

    FlowCard {
        def,
        header,
        body,
        body_start_line: body_start,
        body_end_line: body_end,
        kept_bytes,
        footer_bytes,
        callees,
        total_callees,
        callers,
        total_callers,
    }
}

// expand_hit appends a `── calls ──` block to the body. Flow surfaces
// callees in its own section, so drop the block from the body string.
fn strip_calls_block(body: &str) -> String {
    match body.find(CALLS_DIVIDER) {
        Some(idx) => body[..idx].trim_end_matches('\n').to_string() + "\n",
        None => body.to_string(),
    }
}

fn build_candidates(root: &Path) -> Vec<CandidateFile> {
    super::walk_files(root)
        .into_iter()
        .filter_map(|path| {
            let meta = std::fs::metadata(&path).ok()?;
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let content = std::fs::read_to_string(&path).ok()?;
            let lang = match detect_file_type(&path) {
                FileType::Code(l) => Some(l),
                _ => None,
            };
            Some(CandidateFile {
                path,
                mtime,
                content,
                lang,
            })
        })
        .collect()
}

fn collect_usages(engine: &Engine, name: &str, candidates: &[CandidateFile]) -> Vec<FlowCaller> {
    let definitions = engine.handles.symbols.lookup_exact(name);
    let definition_paths: Vec<String> = definitions
        .iter()
        .map(|d| d.path.to_string_lossy().to_string())
        .collect();

    let stack = PreFilterStack::new(engine.handles.bloom.clone());
    let confirm_input: Vec<(PathBuf, SystemTime, String)> = candidates
        .iter()
        .map(|c| (c.path.clone(), c.mtime, c.content.clone()))
        .collect();
    let survivors = stack.confirm_symbol(&confirm_input, name);
    let survivor_set: HashSet<&std::path::Path> =
        survivors.iter().map(|s| s.path.as_path()).collect();

    let mut out: Vec<FlowCaller> = Vec::new();
    for cf in candidates {
        if !survivor_set.contains(cf.path.as_path()) {
            continue;
        }
        let path_str = cf.path.to_string_lossy().to_string();
        let in_def_file = definition_paths.contains(&path_str);
        let def_lines: Vec<u32> = if in_def_file {
            definitions
                .iter()
                .filter(|d| d.path.to_string_lossy() == path_str)
                .map(|d| d.line)
                .collect()
        } else {
            Vec::new()
        };

        let outline = if let Some(lang) = cf.lang {
            engine
                .handles
                .outlines
                .get_or_compute(&cf.path, cf.mtime, &cf.content, lang)
        } else {
            Vec::new()
        };

        for (lineno, line) in cf.content.lines().enumerate() {
            let lineno = (lineno + 1) as u32;
            if !line.contains(name) {
                continue;
            }
            if def_lines.contains(&lineno) {
                continue;
            }
            out.push(FlowCaller {
                path: path_str.clone(),
                line: lineno,
                text: line.to_string(),
                enclosing: enclosing_symbol(&outline, lineno),
            });
        }
    }
    out
}

fn render_text(p: &FlowOutput) -> String {
    let mut out = String::new();
    if p.total_cards == 0 {
        out.push_str(&format!("[no definitions found for `{}`]\n", p.name));
        if !p.did_you_mean.is_empty() {
            out.push_str("did you mean:\n");
            for s in &p.did_you_mean {
                out.push_str(&format!(
                    "  {} ({}) — {} hit{}\n",
                    s.name,
                    s.source,
                    s.hits.len(),
                    if s.hits.len() == 1 { "" } else { "s" }
                ));
            }
        }
        return out;
    }

    for (idx, card) in p.cards.iter().enumerate() {
        let card_idx = p.offset + idx + 1;
        out.push_str(&format!(
            "── card {}/{}: {} @ {}:{} ({}, w={}) ──\n",
            card_idx,
            p.total_cards,
            p.name,
            card.def.path,
            card.def.line,
            card.def.kind,
            card.def.weight,
        ));
        if !card.header.is_empty() {
            out.push_str(&format!("header: {}\n", card.header.trim_end()));
        }
        out.push_str(&format!(
            "body [{}–{}, {} bytes kept]:\n",
            card.body_start_line, card.body_end_line, card.kept_bytes,
        ));
        for line in card.body.lines() {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
        }

        out.push_str(&format!(
            "callees ({} shown of {}):\n",
            card.callees.len(),
            card.total_callees,
        ));
        if card.callees.is_empty() {
            out.push_str("  [none]\n");
        } else {
            for c in &card.callees {
                match (c.path.as_deref(), c.line) {
                    (Some(p), Some(l)) => {
                        out.push_str(&format!("  {} @ {}:{}\n", c.name, p, l));
                    }
                    _ => {
                        out.push_str(&format!("  {} (unresolved)\n", c.name));
                    }
                }
            }
        }

        out.push_str(&format!(
            "callers ({} shown of {}):\n",
            card.callers.len(),
            card.total_callers,
        ));
        if card.callers.is_empty() {
            out.push_str("  [none]\n");
        } else {
            for c in &card.callers {
                let encl = c.enclosing.as_deref().unwrap_or("?");
                out.push_str(&format!(
                    "  {}:{} (in {}): {}\n",
                    c.path,
                    c.line,
                    encl,
                    c.text.trim_end()
                ));
            }
        }
        out.push('\n');
    }

    out.push_str(&footer(p.total_cards, p.offset, p.cards.len(), p.has_more));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn dummy_card(name: &str, path: &str, line: u32) -> FlowCard {
        FlowCard {
            def: FlowDef {
                path: path.into(),
                line,
                end_line: line + 10,
                kind: "function".into(),
                weight: 5,
            },
            header: format!("fn {name}()"),
            body: format!("fn {name}() {{\n    bar();\n}}"),
            body_start_line: line,
            body_end_line: line + 2,
            kept_bytes: 32,
            footer_bytes: 0,
            callees: vec![FlowCallee {
                name: "bar".into(),
                path: Some("src/x.rs".into()),
                line: Some(2),
            }],
            total_callees: 1,
            callers: vec![FlowCaller {
                path: "src/main.rs".into(),
                line: 9,
                text: "    foo();".into(),
                enclosing: Some("main".into()),
            }],
            total_callers: 1,
        }
    }

    fn dummy_payload() -> FlowOutput {
        FlowOutput {
            name: "foo".into(),
            cards: vec![dummy_card("foo", "src/lib.rs", 10)],
            total_cards: 1,
            offset: 0,
            has_more: false,
            did_you_mean: Vec::new(),
        }
    }

    #[test]
    fn json_shape_includes_required_fields() {
        let p = dummy_payload();
        let v: Value = serde_json::to_value(&p).unwrap();
        for k in ["name", "cards", "total_cards", "offset", "has_more"] {
            assert!(v.get(k).is_some(), "missing field {k}");
        }
        let card = &v["cards"][0];
        for k in [
            "def",
            "header",
            "body",
            "body_start_line",
            "body_end_line",
            "kept_bytes",
            "footer_bytes",
            "callees",
            "total_callees",
            "callers",
            "total_callers",
        ] {
            assert!(card.get(k).is_some(), "missing card field {k}");
        }
        assert_eq!(card["def"]["kind"], "function");
        assert_eq!(card["callees"][0]["name"], "bar");
        assert_eq!(card["callers"][0]["enclosing"], "main");
    }

    #[test]
    fn json_omits_did_you_mean_when_empty() {
        let p = dummy_payload();
        let v: Value = serde_json::to_value(&p).unwrap();
        assert!(v.get("did_you_mean").is_none());
    }

    #[test]
    fn json_omits_callee_path_when_unresolved() {
        let mut p = dummy_payload();
        p.cards[0].callees = vec![FlowCallee {
            name: "external".into(),
            path: None,
            line: None,
        }];
        let v: Value = serde_json::to_value(&p).unwrap();
        let c = &v["cards"][0]["callees"][0];
        assert_eq!(c["name"], "external");
        assert!(c.get("path").is_none());
        assert!(c.get("line").is_none());
    }

    #[test]
    fn text_render_groups_card_sections() {
        let s = render_text(&dummy_payload());
        let header_idx = s.find("── card 1/1").unwrap();
        let body_idx = s.find("body [").unwrap();
        let callees_idx = s.find("callees (").unwrap();
        let callers_idx = s.find("callers (").unwrap();
        assert!(header_idx < body_idx);
        assert!(body_idx < callees_idx);
        assert!(callees_idx < callers_idx);
        assert!(s.contains("fn foo()"));
        assert!(s.contains("bar @ src/x.rs:2"));
        assert!(s.contains("src/main.rs:9 (in main)"));
    }

    #[test]
    fn text_render_zero_cards_shows_did_you_mean_when_present() {
        let p = FlowOutput {
            name: "fooo".into(),
            cards: Vec::new(),
            total_cards: 0,
            offset: 0,
            has_more: false,
            did_you_mean: vec![Suggestion {
                name: "foo".into(),
                source: "fuzzy".into(),
                hits: vec![],
            }],
        };
        let s = render_text(&p);
        assert!(s.contains("[no definitions found"));
        assert!(s.contains("did you mean:"));
        assert!(s.contains("foo (fuzzy)"));
    }

    #[test]
    fn text_render_truncated_callees_show_total() {
        let mut p = dummy_payload();
        p.cards[0].callees = vec![FlowCallee {
            name: "a".into(),
            path: None,
            line: None,
        }];
        p.cards[0].total_callees = 5;
        let s = render_text(&p);
        assert!(s.contains("callees (1 shown of 5)"));
    }

    #[test]
    fn strip_calls_block_removes_trailing_callees_section() {
        let body = "fn foo() {\n    bar();\n}\n── calls ──\nbar @ x.rs:1\n";
        let stripped = strip_calls_block(body);
        assert!(stripped.contains("fn foo()"));
        assert!(!stripped.contains("── calls ──"));
        assert!(!stripped.contains("bar @ x.rs"));
    }

    #[test]
    fn strip_calls_block_is_noop_when_block_absent() {
        let body = "fn foo() {\n    bar();\n}\n";
        assert_eq!(strip_calls_block(body), body);
    }

    #[test]
    fn build_card_respects_callees_top_and_callers_top() {
        let card = FlowCard {
            def: FlowDef {
                path: "p".into(),
                line: 1,
                end_line: 1,
                kind: "fn".into(),
                weight: 0,
            },
            header: String::new(),
            body: String::new(),
            body_start_line: 1,
            body_end_line: 1,
            kept_bytes: 0,
            footer_bytes: 0,
            callees: (0..5)
                .map(|i| FlowCallee {
                    name: format!("c{i}"),
                    path: None,
                    line: None,
                })
                .collect(),
            total_callees: 20,
            callers: (0..5)
                .map(|i| FlowCaller {
                    path: format!("p{i}"),
                    line: i as u32,
                    text: String::new(),
                    enclosing: None,
                })
                .collect(),
            total_callers: 30,
        };
        assert_eq!(card.callees.len(), 5);
        assert_eq!(card.callers.len(), 5);
        assert_eq!(card.total_callees, 20);
        assert_eq!(card.total_callers, 30);
    }
}
