//! `scry callers <symbol>` — find call sites for `symbol`. With `--hops > 1`
//! the search becomes a BFS over the caller graph.

use std::collections::HashMap;
use std::path::Path;
use std::time::SystemTime;

use anyhow::Result;
use clap::{Parser, ValueEnum};
use serde::Serialize;

use fff_engine::{Engine, PreFilterStack};
use fff_symbol::lang::detect_file_type;
use fff_symbol::types::FileType;

use crate::cli::OutputFormat;
use crate::commands::callers_bfs::{
    enclosing_symbol, run_bfs, AutoHub, BfsConfig, BfsTelemetry, CandidateFile, SuspiciousHop,
};
use crate::commands::pagination::{footer, Page};

#[derive(Debug, Parser)]
pub struct Args {
    /// Symbol name to find call sites for.
    pub name: String,

    /// Maximum hits returned in this page.
    #[arg(long, default_value_t = 100)]
    pub limit: usize,

    /// Skip this many hits before starting the page.
    #[arg(long, default_value_t = 0)]
    pub offset: usize,

    /// How far to walk the caller graph. 1 = direct callers only. Capped at 5.
    #[arg(long, default_value_t = 1)]
    pub hops: u32,

    /// Skip propagation when a single name produces more than this many hits
    /// in one hop. Prevents popular helpers (clone, unwrap, …) from blowing
    /// the BFS up.
    #[arg(long, default_value_t = 50)]
    pub hub_guard: usize,

    /// Comma-separated list of hub symbols to explicitly skip during BFS.
    /// Root symbol (depth 1) is always explored regardless.
    #[arg(long, default_value = "")]
    pub skip_hubs: String,

    /// Aggregate hits into a frequency table. `caller` groups by the
    /// enclosing symbol (function/method/scope); `file` groups by the
    /// containing path; `none` (default) skips aggregation and keeps the
    /// output byte-identical.
    #[arg(long, value_enum, default_value_t = CountBy::None)]
    pub count_by: CountBy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum CountBy {
    None,
    Caller,
    File,
    Package,
}

#[derive(Debug, Serialize)]
pub struct CallerHit {
    pub path: String,
    pub line: u32,
    pub text: String,
    pub depth: u32,
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enclosing: Option<String>,
}

#[derive(Debug, Serialize)]
struct CallersOutput {
    name: String,
    hops: u32,
    hub_guard: usize,
    hits: Vec<CallerHit>,
    total: usize,
    offset: usize,
    has_more: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    aggregations: Option<Vec<Aggregation>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    suspicious_hops: Vec<SuspiciousHopOut>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    auto_hubs_promoted: Vec<AutoHubOut>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    hubs_skipped: Vec<AutoHubOut>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    proximity_suspicions: Vec<ProximitySuspicionOut>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct SuspiciousHopOut {
    pub depth: u32,
    pub name: String,
    pub roots: Vec<String>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct AutoHubOut {
    pub depth: u32,
    pub name: String,
    pub count: usize,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct ProximitySuspicionOut {
    pub depth: u32,
    pub total_edges: usize,
    pub related_edges: usize,
}

impl From<SuspiciousHop> for SuspiciousHopOut {
    fn from(s: SuspiciousHop) -> Self {
        Self {
            depth: s.depth,
            name: s.name,
            roots: s.roots,
        }
    }
}

impl From<AutoHub> for AutoHubOut {
    fn from(a: AutoHub) -> Self {
        Self {
            depth: a.depth,
            name: a.name,
            count: a.count,
        }
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct Aggregation {
    pub key: String,
    pub count: usize,
}

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let engine = Engine::default();
    engine.index(root);

    let files = super::walk_files(root);
    let candidates: Vec<CandidateFile> = files
        .iter()
        .filter_map(|path| {
            let meta = std::fs::metadata(path).ok()?;
            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let content = std::fs::read_to_string(path).ok()?;
            let lang = match detect_file_type(path) {
                FileType::Code(l) => Some(l),
                _ => None,
            };
            Some(CandidateFile {
                path: path.clone(),
                mtime,
                content,
                lang,
            })
        })
        .collect();

    let (mut hits, telemetry) = if args.hops <= 1 {
        (
            single_hop(&engine, &args.name, &candidates),
            BfsTelemetry::default(),
        )
    } else {
        let r = run_bfs(
            &engine,
            &args.name,
            &candidates,
            BfsConfig {
                max_hops: args.hops,
                hub_guard: args.hub_guard,
                skip_hubs: args.skip_hubs.clone(),
            },
            root,
        );
        (r.hits, r.telemetry)
    };

    if args.count_by == CountBy::Caller {
        populate_enclosing(&engine, &mut hits, &candidates);
    }
    let aggregations = aggregate(&hits, args.count_by);

    let page = Page::paginate(hits, args.offset, args.limit);
    let payload = CallersOutput {
        name: args.name.clone(),
        hops: args.hops.clamp(1, 5),
        hub_guard: args.hub_guard,
        total: page.total,
        offset: page.offset,
        has_more: page.has_more,
        hits: page.items,
        aggregations,
        suspicious_hops: telemetry
            .suspicious_hops
            .into_iter()
            .map(SuspiciousHopOut::from)
            .collect(),
        auto_hubs_promoted: telemetry
            .auto_hubs_promoted
            .into_iter()
            .map(AutoHubOut::from)
            .collect(),
        hubs_skipped: telemetry
            .hubs_skipped
            .into_iter()
            .map(|(depth, name)| AutoHubOut {
                depth,
                name,
                count: 0,
            })
            .collect(),
        proximity_suspicions: telemetry
            .proximity_suspicions
            .into_iter()
            .map(|p| ProximitySuspicionOut {
                depth: p.depth,
                total_edges: p.total_edges,
                related_edges: p.related_edges,
            })
            .collect(),
    };
    super::emit(format, &payload, render_text)
}

// Frequency table over hits keyed by enclosing symbol, file, or package.
// `None` mode returns `None` so callers can omit the field entirely.
pub(crate) fn aggregate(hits: &[CallerHit], mode: CountBy) -> Option<Vec<Aggregation>> {
    if mode == CountBy::None {
        return None;
    }
    let mut counts: HashMap<String, usize> = HashMap::new();
    for h in hits {
        let key = match mode {
            CountBy::Caller => h
                .enclosing
                .clone()
                .unwrap_or_else(|| "<unknown>".to_string()),
            CountBy::File => h.path.clone(),
            CountBy::Package => super::callers_bfs::package_root(
                std::path::Path::new(&h.path),
                std::path::Path::new(""),
            ),
            CountBy::None => unreachable!(),
        };
        *counts.entry(key).or_default() += 1;
    }
    let mut rows: Vec<Aggregation> = counts
        .into_iter()
        .map(|(key, count)| Aggregation { key, count })
        .collect();
    rows.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.key.cmp(&b.key)));
    Some(rows)
}

// Fill `enclosing` for hits that don't have it yet (single-hop path leaves
// it `None`). Uses the outline cache like the BFS path does.
fn populate_enclosing(engine: &Engine, hits: &mut [CallerHit], candidates: &[CandidateFile]) {
    let by_path: HashMap<&str, &CandidateFile> = candidates
        .iter()
        .map(|c| (c.path.to_str().unwrap_or(""), c))
        .collect();
    let mut outline_cache: HashMap<String, Vec<fff_symbol::types::OutlineEntry>> = HashMap::new();
    for h in hits.iter_mut() {
        if h.enclosing.is_some() {
            continue;
        }
        let Some(cf) = by_path.get(h.path.as_str()) else {
            continue;
        };
        let Some(lang) = cf.lang else {
            continue;
        };
        let outline = outline_cache.entry(h.path.clone()).or_insert_with(|| {
            engine
                .handles
                .outlines
                .get_or_compute(&cf.path, cf.mtime, &cf.content, lang)
        });
        h.enclosing = enclosing_symbol(outline, h.line);
    }
}

fn single_hop(engine: &Engine, name: &str, candidates: &[CandidateFile]) -> Vec<CallerHit> {
    let definitions = engine.handles.symbols.lookup_exact(name);
    let definition_paths: Vec<String> = definitions
        .iter()
        .map(|d| d.path.to_string_lossy().to_string())
        .collect();

    let stack = PreFilterStack::new(engine.handles.bloom.clone());
    let confirm_input: Vec<(std::path::PathBuf, SystemTime, String)> = candidates
        .iter()
        .map(|c| (c.path.clone(), c.mtime, c.content.clone()))
        .collect();
    let survivors = stack.confirm_symbol(&confirm_input, name);
    let survivor_set: std::collections::HashSet<&std::path::Path> =
        survivors.iter().map(|s| s.path.as_path()).collect();

    let mut hits: Vec<CallerHit> = Vec::new();
    for cf in candidates {
        if !survivor_set.contains(cf.path.as_path()) {
            continue;
        }
        let path_str = cf.path.to_string_lossy().to_string();
        let in_defn_file = definition_paths.contains(&path_str);
        let definition_lines: Vec<u32> = if in_defn_file {
            definitions
                .iter()
                .filter(|d| d.path.to_string_lossy() == path_str)
                .map(|d| d.line)
                .collect()
        } else {
            Vec::new()
        };

        for (lineno, line) in cf.content.lines().enumerate() {
            let lineno = (lineno + 1) as u32;
            if !line.contains(name) {
                continue;
            }
            if definition_lines.contains(&lineno) {
                continue;
            }
            hits.push(CallerHit {
                path: path_str.clone(),
                line: lineno,
                text: line.to_string(),
                depth: 1,
                target: name.to_string(),
                enclosing: None,
            });
        }
    }
    hits
}

fn render_text(p: &CallersOutput) -> String {
    let mut out = String::new();
    for h in &p.hits {
        if p.hops > 1 {
            let encl = h.enclosing.as_deref().unwrap_or("?");
            out.push_str(&format!(
                "[d{}] {}:{}: {}  (in {}, calls {})\n",
                h.depth, h.path, h.line, h.text, encl, h.target
            ));
        } else {
            out.push_str(&format!("{}:{}: {}\n", h.path, h.line, h.text));
        }
    }
    if p.total == 0 {
        out.push_str("[no callers found]\n");
    } else {
        out.push_str(&footer(p.total, p.offset, p.hits.len(), p.has_more));
    }
    if let Some(aggs) = p.aggregations.as_ref() {
        out.push_str(&render_aggregations(aggs));
    }
    if !p.suspicious_hops.is_empty() {
        out.push_str("\nSuspicious hops (name defined in multiple roots):\n");
        for s in &p.suspicious_hops {
            out.push_str(&format!(
                "  [d{}] {}  roots: {}\n",
                s.depth,
                s.name,
                s.roots.join(", ")
            ));
        }
    }
    if !p.auto_hubs_promoted.is_empty() {
        out.push_str("\nAuto hub-guard promotions (propagation stopped):\n");
        for a in &p.auto_hubs_promoted {
            out.push_str(&format!("  [d{}] {}  hits: {}\n", a.depth, a.name, a.count));
        }
    }
    if !p.hubs_skipped.is_empty() {
        out.push_str("\nHubs skipped (--skip-hubs):\n");
        for a in &p.hubs_skipped {
            out.push_str(&format!("  [d{}] {}\n", a.depth, a.name));
        }
    }
    out
}

fn render_aggregations(aggs: &[Aggregation]) -> String {
    let mut out = String::new();
    out.push_str("\nAggregated:\n");
    for a in aggs {
        out.push_str(&format!("  {:>5}  {}\n", a.count, a.key));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(path: &str, enclosing: Option<&str>) -> CallerHit {
        CallerHit {
            path: path.to_string(),
            line: 1,
            text: String::new(),
            depth: 1,
            target: "x".to_string(),
            enclosing: enclosing.map(|s| s.to_string()),
        }
    }

    #[test]
    fn aggregate_none_returns_none() {
        let hits = vec![hit("a", Some("foo"))];
        assert!(aggregate(&hits, CountBy::None).is_none());
    }

    #[test]
    fn aggregate_by_file_groups_by_path() {
        let hits = vec![
            hit("src/a.rs", Some("foo")),
            hit("src/a.rs", Some("bar")),
            hit("src/b.rs", Some("baz")),
        ];
        let aggs = aggregate(&hits, CountBy::File).unwrap();
        assert_eq!(aggs.len(), 2);
        assert_eq!(
            aggs[0],
            Aggregation {
                key: "src/a.rs".into(),
                count: 2
            }
        );
        assert_eq!(
            aggs[1],
            Aggregation {
                key: "src/b.rs".into(),
                count: 1
            }
        );
    }

    #[test]
    fn aggregate_by_caller_groups_by_enclosing() {
        let hits = vec![
            hit("src/a.rs", Some("foo")),
            hit("src/a.rs", Some("bar")),
            hit("src/b.rs", Some("baz")),
        ];
        let aggs = aggregate(&hits, CountBy::Caller).unwrap();
        assert_eq!(aggs.len(), 3);
        // Counts all 1, sorted alphabetically as tie-break.
        assert_eq!(aggs[0].key, "bar");
        assert_eq!(aggs[1].key, "baz");
        assert_eq!(aggs[2].key, "foo");
    }

    #[test]
    fn aggregate_by_caller_falls_back_for_unknown() {
        let hits = vec![hit("src/a.rs", None), hit("src/a.rs", None)];
        let aggs = aggregate(&hits, CountBy::Caller).unwrap();
        assert_eq!(aggs.len(), 1);
        assert_eq!(
            aggs[0],
            Aggregation {
                key: "<unknown>".into(),
                count: 2
            }
        );
    }

    #[test]
    fn aggregate_by_package_groups_by_parent_dirs() {
        let hits = vec![
            hit("src/a/foo.rs", None),
            hit("src/a/bar.rs", None),
            hit("src/b/baz.rs", None),
        ];
        let aggs = aggregate(&hits, CountBy::Package).unwrap();
        assert_eq!(aggs.len(), 2);
        assert_eq!(
            aggs[0],
            Aggregation {
                key: "src/a".into(),
                count: 2
            }
        );
        assert_eq!(
            aggs[1],
            Aggregation {
                key: "src/b".into(),
                count: 1
            }
        );
    }

    #[test]
    fn render_text_omits_section_when_none() {
        let payload = CallersOutput {
            name: "x".into(),
            hops: 1,
            hub_guard: 50,
            hits: vec![CallerHit {
                path: "a".into(),
                line: 1,
                text: "x".into(),
                depth: 1,
                target: "x".into(),
                enclosing: None,
            }],
            total: 1,
            offset: 0,
            has_more: false,
            aggregations: None,
            suspicious_hops: Vec::new(),
            auto_hubs_promoted: Vec::new(),
            hubs_skipped: Vec::new(),
            proximity_suspicions: Vec::new(),
        };
        let s = render_text(&payload);
        assert!(!s.contains("Aggregated"));
    }

    #[test]
    fn render_text_emits_section_when_present() {
        let payload = CallersOutput {
            name: "x".into(),
            hops: 1,
            hub_guard: 50,
            hits: vec![],
            total: 0,
            offset: 0,
            has_more: false,
            aggregations: Some(vec![Aggregation {
                key: "foo".into(),
                count: 2,
            }]),
            suspicious_hops: Vec::new(),
            auto_hubs_promoted: Vec::new(),
            hubs_skipped: Vec::new(),
            proximity_suspicions: Vec::new(),
        };
        let s = render_text(&payload);
        assert!(s.contains("Aggregated:"));
        assert!(s.contains("foo"));
    }
}
