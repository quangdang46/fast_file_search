use std::path::Path;
use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use serde::Serialize;

use ffs_engine::Engine;
use ffs_symbol::symbol_index::SymbolLocation;

use crate::cli::OutputFormat;
use crate::commands::did_you_mean::{suggest, Suggestion};
use crate::commands::expand::{expand_hit, per_hit_budget_bytes, Expanded};
use crate::commands::facets::Facets;
use crate::commands::pagination::{footer, Page};

#[derive(Debug, Parser)]
pub struct Args {
    /// Symbol name to look up. Glob patterns ending with `*` are treated as prefixes.
    /// Comma-separated list (e.g. `"a,b,c"`) emits one group per name.
    pub name: String,

    /// Maximum hits returned in this page.
    #[arg(long, default_value_t = 100)]
    pub limit: usize,

    /// Skip this many hits before starting the page.
    #[arg(long, default_value_t = 0)]
    pub offset: usize,

    /// Inline each definition's body and a `── calls ──` block listing direct
    /// callees. Token-budget capped via `--budget`.
    #[arg(long, default_value_t = false)]
    pub expand: bool,

    /// Disable did-you-mean suggestions when no exact match is found.
    /// Suggestions are on by default and only emit when there are zero hits.
    #[arg(long = "no-did-you-mean", default_value_t = false)]
    pub no_did_you_mean: bool,

    /// Token budget for `--expand`. Split across visible hits with a per-hit
    /// floor of 256 bytes. Ignored when `--expand` is not set.
    #[arg(long, default_value_t = 10_000)]
    pub budget: u64,
}

#[derive(Debug, Serialize)]
struct SymbolOutput {
    query: String,
    hits: Vec<SymbolHit>,
    total: usize,
    offset: usize,
    has_more: bool,
    facets: Facets,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    did_you_mean: Vec<Suggestion>,
}

#[derive(Debug, Serialize)]
struct MultiSymbolOutput {
    query: String,
    groups: Vec<SymbolOutput>,
    total_groups: usize,
}

#[derive(Debug, Serialize)]
struct SymbolHit {
    name: String,
    path: String,
    line: u32,
    end_line: u32,
    kind: String,
    weight: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    expanded: Option<Expanded>,
}

fn loc_to_hit(name: &str, loc: SymbolLocation) -> SymbolHit {
    SymbolHit {
        name: name.to_string(),
        path: loc.path.to_string_lossy().to_string(),
        line: loc.line,
        end_line: loc.end_line,
        kind: loc.kind,
        weight: loc.weight,
        expanded: None,
    }
}

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let engine = Engine::default();
    engine.index(root);

    let names = parse_names(&args.name);
    if names.len() <= 1 {
        let payload = build_group(
            &engine,
            &args.name,
            args.offset,
            args.limit,
            args.expand,
            args.budget,
            args.no_did_you_mean,
        );
        return super::emit(format, &payload, render_single);
    }

    // Multi-symbol: budget shared across visible hits in every group.
    let mut groups: Vec<SymbolOutput> = names
        .iter()
        .map(|n| {
            build_group(
                &engine,
                n,
                args.offset,
                args.limit,
                false, // expand applied after we know total visible hit count.
                args.budget,
                args.no_did_you_mean,
            )
        })
        .collect();

    if args.expand {
        let total_visible: usize = groups.iter().map(|g| g.hits.len()).sum();
        if total_visible > 0 {
            let per_hit = per_hit_budget_bytes(args.budget, total_visible);
            for group in groups.iter_mut() {
                for hit in group.hits.iter_mut() {
                    let path = PathBuf::from(&hit.path);
                    hit.expanded =
                        expand_hit(&engine, &hit.name, &path, hit.line, hit.end_line, per_hit);
                }
            }
        }
    }

    let payload = MultiSymbolOutput {
        total_groups: groups.len(),
        groups,
        query: args.name,
    };
    super::emit(format, &payload, render_multi)
}

fn parse_names(raw: &str) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for part in raw.split(',') {
        let t = part.trim();
        if t.is_empty() {
            continue;
        }
        if !seen.iter().any(|s| s == t) {
            seen.push(t.to_string());
        }
    }
    seen
}

fn build_group(
    engine: &Engine,
    name: &str,
    offset: usize,
    limit: usize,
    expand: bool,
    budget: u64,
    no_did_you_mean: bool,
) -> SymbolOutput {
    let all_hits: Vec<SymbolHit> = if let Some(prefix) = name.strip_suffix('*') {
        engine
            .handles
            .symbols
            .lookup_prefix(prefix)
            .into_iter()
            .map(|(n, l)| loc_to_hit(&n, l))
            .collect()
    } else {
        engine
            .handles
            .symbols
            .lookup_exact(name)
            .into_iter()
            .map(|l| loc_to_hit(name, l))
            .collect()
    };

    let did_you_mean = if all_hits.is_empty() && !no_did_you_mean {
        suggest(engine, name)
    } else {
        Vec::new()
    };

    let facets = Facets::from_kinds(all_hits.iter().map(|h| h.kind.as_str()));
    let mut page = Page::paginate(all_hits, offset, limit);

    if expand && !page.items.is_empty() {
        let per_hit = per_hit_budget_bytes(budget, page.items.len());
        for hit in page.items.iter_mut() {
            let path = PathBuf::from(&hit.path);
            hit.expanded = expand_hit(engine, &hit.name, &path, hit.line, hit.end_line, per_hit);
        }
    }

    SymbolOutput {
        query: name.to_string(),
        total: page.total,
        offset: page.offset,
        has_more: page.has_more,
        hits: page.items,
        facets,
        did_you_mean,
    }
}

fn render_single(p: &SymbolOutput) -> String {
    let mut out = String::new();
    push_group_lines(&mut out, p, false);
    out
}

fn render_multi(p: &MultiSymbolOutput) -> String {
    let mut out = String::new();
    for (i, g) in p.groups.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str(&format!("## {}\n", g.query));
        push_group_lines(&mut out, g, true);
    }
    out
}

fn push_group_lines(out: &mut String, p: &SymbolOutput, indent: bool) {
    let prefix = if indent { "  " } else { "" };
    for h in &p.hits {
        out.push_str(prefix);
        out.push_str(&format!(
            "{} @ {}:{} ({})\n",
            h.name, h.path, h.line, h.kind
        ));
        if let Some(exp) = &h.expanded {
            for line in exp.body.lines() {
                out.push_str(prefix);
                out.push_str("  ");
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    if p.total == 0 {
        out.push_str(prefix);
        out.push_str("[no symbols found]\n");
        if !p.did_you_mean.is_empty() {
            out.push_str(prefix);
            out.push_str("did you mean:\n");
            for s in &p.did_you_mean {
                out.push_str(prefix);
                out.push_str(&format!(
                    "  {} ({}) — {} hit{}\n",
                    s.name,
                    s.source,
                    s.hits.len(),
                    if s.hits.len() == 1 { "" } else { "s" }
                ));
            }
        }
    } else {
        let foot = footer(p.total, p.offset, p.hits.len(), p.has_more);
        for line in foot.lines() {
            out.push_str(prefix);
            out.push_str(line);
            out.push('\n');
        }
        if !p.facets.by_kind.is_empty() {
            let parts: Vec<String> = p
                .facets
                .by_kind
                .iter()
                .map(|(k, n)| format!("{k}: {n}"))
                .collect();
            out.push_str(prefix);
            out.push_str(&format!("by kind: {}\n", parts.join(", ")));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_names_returns_singleton_for_plain_name() {
        assert_eq!(parse_names("foo"), vec!["foo".to_string()]);
    }

    #[test]
    fn parse_names_trims_and_filters_empty() {
        assert_eq!(
            parse_names(" a , b , , c "),
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
        );
    }

    #[test]
    fn parse_names_dedups_preserving_first_occurrence() {
        assert_eq!(
            parse_names("a,b,a,c,b"),
            vec!["a".to_string(), "b".to_string(), "c".to_string()],
        );
    }

    #[test]
    fn parse_names_preserves_prefix_glob_marker() {
        // Multi-symbol entries each keep their own `*` suffix.
        assert_eq!(
            parse_names("foo*,bar"),
            vec!["foo*".to_string(), "bar".to_string()],
        );
    }

    fn empty_group(name: &str) -> SymbolOutput {
        SymbolOutput {
            query: name.to_string(),
            hits: Vec::new(),
            total: 0,
            offset: 0,
            has_more: false,
            facets: Facets::from_kinds(std::iter::empty::<&str>()),
            did_you_mean: Vec::new(),
        }
    }

    #[test]
    fn render_multi_separates_groups_with_blank_line_and_header() {
        let payload = MultiSymbolOutput {
            query: "a,b".into(),
            total_groups: 2,
            groups: vec![empty_group("a"), empty_group("b")],
        };
        let text = render_multi(&payload);
        assert!(text.starts_with("## a\n"));
        assert!(text.contains("\n\n## b\n"));
        assert!(text.matches("[no symbols found]").count() == 2);
    }

    #[test]
    fn render_single_omits_group_header_and_indent() {
        let group = empty_group("a");
        let text = render_single(&group);
        assert!(!text.contains("## a"));
        assert!(text.starts_with("[no symbols found]"));
    }
}
