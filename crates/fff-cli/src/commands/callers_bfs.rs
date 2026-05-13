//! Multi-hop BFS over the caller graph for `scry callers --hops N`.
//!
//! Each hop:
//! 1. For every name in the current frontier, find direct call-site lines
//!    using the same bloom-narrowed text scan that single-hop callers uses.
//! 2. Map each hit to its enclosing symbol (function/method/class) via the
//!    outline cache.
//! 3. Add those enclosing symbols to the next frontier, modulo `hub_guard`:
//!    if a single name produces more than `hub_guard` hits at this hop, its
//!    enclosing symbols don't propagate further (but the hits still surface).
//!
//! The frontier is name-keyed, so a symbol that's defined in many places
//! (e.g. trait methods) is searched once per hop regardless of definition
//! count.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use fff_engine::{Engine, PreFilterStack};
use fff_symbol::types::{Lang, OutlineEntry};

use crate::commands::callers::CallerHit;
use crate::commands::session::Session;

// Minimum hit count at a depth >= 2 before proximity heuristic activates.
const PROXIMITY_MIN_HITS: usize = 50;
// Fraction of hits whose parent dir must overlap the previous depth's dirs;
// below this, the hop is flagged as suspicious cross-package collisions.
const PROXIMITY_RELATED_RATIO_NUM: usize = 1;
const PROXIMITY_RELATED_RATIO_DEN: usize = 5;

pub struct BfsConfig {
    pub max_hops: u32,
    pub hub_guard: usize,
    pub skip_hubs: String,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct BfsTelemetry {
    pub suspicious_hops: Vec<SuspiciousHop>,
    pub auto_hubs_promoted: Vec<AutoHub>,
    pub hubs_skipped: Vec<(u32, String)>,
    pub proximity_suspicions: Vec<ProximitySuspicion>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct SuspiciousHop {
    pub depth: u32,
    pub name: String,
    pub roots: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ProximitySuspicion {
    pub depth: u32,
    pub total_edges: usize,
    pub related_edges: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub struct AutoHub {
    pub depth: u32,
    pub name: String,
    pub count: usize,
}

pub struct BfsResult {
    pub hits: Vec<CallerHit>,
    pub telemetry: BfsTelemetry,
}

// First two components of the relative path of `path` rooted at `root`, joined
// with `/`. Falls back to the leftmost component(s) of the absolute path when
// `path` is not actually rooted at `root` (e.g. resolved via canonicalize).
pub(crate) fn package_root(path: &Path, root: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    let mut comps = rel.components();
    let mut buf = PathBuf::new();
    if let Some(c) = comps.next() {
        buf.push(c.as_os_str());
    }
    if let Some(c) = comps.next() {
        // If the second component is the actual file basename, keep only the
        // first directory level — a file at `<root>/src/a.rs` reports `src`,
        // not `src/a.rs`.
        if comps.next().is_some() {
            buf.push(c.as_os_str());
        }
    }
    let s = buf.to_string_lossy().replace('\\', "/");
    if s.is_empty() {
        rel.to_string_lossy().replace('\\', "/")
    } else {
        s
    }
}

pub struct CandidateFile {
    pub path: PathBuf,
    pub mtime: SystemTime,
    pub content: String,
    pub lang: Option<Lang>,
}

pub fn run_bfs(
    engine: &Engine,
    initial: &str,
    candidates: &[CandidateFile],
    cfg: BfsConfig,
    root: &Path,
) -> BfsResult {
    let stack = PreFilterStack::new(engine.handles.bloom.clone());
    let mut visited: HashSet<String> = HashSet::new();
    visited.insert(initial.to_string());
    let mut frontier: Vec<String> = vec![initial.to_string()];
    let mut all_hits: Vec<CallerHit> = Vec::new();
    let mut telemetry = BfsTelemetry::default();

    let confirm_input: Vec<(PathBuf, SystemTime, String)> = candidates
        .iter()
        .map(|c| (c.path.clone(), c.mtime, c.content.clone()))
        .collect();
    let session = Session::new();

    let max_hops = cfg.max_hops.clamp(1, 5);
    let user_hubs: HashSet<String> = if cfg.skip_hubs.is_empty() {
        HashSet::new()
    } else {
        cfg.skip_hubs
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect()
    };

    for depth in 1..=max_hops {
        if frontier.is_empty() {
            break;
        }

        let mut next_set: HashSet<String> = HashSet::new();
        let mut hub_for_name: HashMap<String, usize> = HashMap::new();

        for name in &frontier {
            // Explicit user hub skip: root symbol always explored regardless.
            if depth > 1 && user_hubs.contains(name) {
                telemetry.hubs_skipped.push((depth, name.clone()));
                continue;
            }
            let definitions = engine.handles.symbols.lookup_exact(name);
            let definition_paths: HashSet<String> = definitions
                .iter()
                .map(|d| d.path.to_string_lossy().to_string())
                .collect();
            let mut def_roots: BTreeSet<String> = definitions
                .iter()
                .map(|d| package_root(&d.path, root))
                .collect();
            def_roots.remove("");

            let survivors = stack.confirm_symbol(&confirm_input, name);
            let survivor_set: HashSet<&std::path::Path> =
                survivors.iter().map(|s| s.path.as_path()).collect();

            let mut hits_for_name = 0usize;
            let mut enclosings_this_name: Vec<String> = Vec::new();

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
                    if definition_lines.contains(&lineno) {
                        continue;
                    }
                    // Dedup across BFS depths: each (path, line) reported once.
                    if session.is_expanded(&cf.path, lineno) {
                        continue;
                    }
                    session.record_expand(&cf.path, lineno);
                    let enclosing = enclosing_symbol(&outline, lineno);
                    if let Some(ref e) = enclosing {
                        enclosings_this_name.push(e.clone());
                    }
                    all_hits.push(CallerHit {
                        path: path_str.clone(),
                        line: lineno,
                        text: line.to_string(),
                        depth,
                        target: name.clone(),
                        enclosing,
                    });
                    hits_for_name += 1;
                }
            }

            hub_for_name.insert(name.clone(), hits_for_name);
            if hits_for_name <= cfg.hub_guard {
                for e in enclosings_this_name {
                    next_set.insert(e);
                }
            } else {
                telemetry.auto_hubs_promoted.push(AutoHub {
                    depth,
                    name: name.clone(),
                    count: hits_for_name,
                });
            }
            if def_roots.len() >= 2 {
                telemetry.suspicious_hops.push(SuspiciousHop {
                    depth,
                    name: name.clone(),
                    roots: def_roots.into_iter().collect(),
                });
            }
        }

        next_set.retain(|n| !visited.contains(n));
        for n in &next_set {
            visited.insert(n.clone());
        }
        frontier = next_set.into_iter().collect();
    }

    telemetry
        .suspicious_hops
        .sort_by(|a, b| a.depth.cmp(&b.depth).then_with(|| a.name.cmp(&b.name)));
    telemetry.auto_hubs_promoted.sort_by(|a, b| {
        a.depth
            .cmp(&b.depth)
            .then_with(|| b.count.cmp(&a.count))
            .then_with(|| a.name.cmp(&b.name))
    });
    telemetry
        .hubs_skipped
        .sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    telemetry.proximity_suspicions = compute_proximity_suspicions(&all_hits);

    BfsResult {
        hits: all_hits,
        telemetry,
    }
}

// Flag depth-N hops whose hits scatter into unrelated directories vs depth-(N-1).
pub fn compute_proximity_suspicions(hits: &[CallerHit]) -> Vec<ProximitySuspicion> {
    use std::collections::{BTreeMap, HashSet};

    let mut by_depth: BTreeMap<u32, Vec<&CallerHit>> = BTreeMap::new();
    for h in hits {
        by_depth.entry(h.depth).or_default().push(h);
    }

    let mut out = Vec::new();
    for (&depth, list) in &by_depth {
        if depth < 2 {
            continue;
        }
        let total = list.len();
        if total < PROXIMITY_MIN_HITS {
            continue;
        }
        let prev = match by_depth.get(&(depth - 1)) {
            Some(p) if !p.is_empty() => p,
            _ => continue,
        };
        let prev_dirs: HashSet<&Path> = prev
            .iter()
            .filter_map(|h| {
                let p = Path::new(&h.path);
                p.parent()
            })
            .collect();
        if prev_dirs.is_empty() {
            continue;
        }
        let related = list
            .iter()
            .filter(|h| {
                Path::new(&h.path)
                    .parent()
                    .is_some_and(|p| prev_dirs.contains(p))
            })
            .count();
        if related * PROXIMITY_RELATED_RATIO_DEN < total * PROXIMITY_RELATED_RATIO_NUM {
            out.push(ProximitySuspicion {
                depth,
                total_edges: total,
                related_edges: related,
            });
        }
    }
    out
}

pub fn enclosing_symbol(entries: &[OutlineEntry], line: u32) -> Option<String> {
    fn walk(entries: &[OutlineEntry], line: u32) -> Option<&OutlineEntry> {
        for e in entries {
            if line < e.start_line || line > e.end_line {
                continue;
            }
            if let Some(child) = walk(&e.children, line) {
                return Some(child);
            }
            return Some(e);
        }
        None
    }
    walk(entries, line).map(|e| e.name.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fff_symbol::types::OutlineKind;

    #[test]
    fn package_root_keeps_top_two_dirs_for_nested_files() {
        let root = Path::new("/repo");
        let p = Path::new("/repo/crates/fff-cli/src/main.rs");
        assert_eq!(package_root(p, root), "crates/fff-cli");
    }

    #[test]
    fn package_root_keeps_single_dir_for_shallow_files() {
        let root = Path::new("/repo");
        let p = Path::new("/repo/src/a.rs");
        assert_eq!(package_root(p, root), "src");
    }

    #[test]
    fn package_root_handles_paths_outside_root() {
        let root = Path::new("/elsewhere");
        let p = Path::new("/repo/src/a/foo.rs");
        // Falls back to first two leading components of the absolute path.
        let got = package_root(p, root);
        assert!(got.ends_with("repo") || got.ends_with("src"));
    }

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
    fn enclosing_returns_innermost_symbol() {
        let outer = entry(
            OutlineKind::Class,
            "Foo",
            10,
            40,
            vec![entry(OutlineKind::Function, "bar", 20, 30, vec![])],
        );
        let outline = vec![outer];
        assert_eq!(enclosing_symbol(&outline, 25), Some("bar".to_string()));
    }

    #[test]
    fn enclosing_returns_outer_when_outside_inner() {
        let outer = entry(
            OutlineKind::Class,
            "Foo",
            10,
            40,
            vec![entry(OutlineKind::Function, "bar", 20, 30, vec![])],
        );
        let outline = vec![outer];
        assert_eq!(enclosing_symbol(&outline, 35), Some("Foo".to_string()));
    }

    #[test]
    fn enclosing_returns_none_when_outside_all() {
        let outline = vec![entry(OutlineKind::Function, "bar", 20, 30, vec![])];
        assert_eq!(enclosing_symbol(&outline, 5), None);
        assert_eq!(enclosing_symbol(&outline, 50), None);
    }
}
