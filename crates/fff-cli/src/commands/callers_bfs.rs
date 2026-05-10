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

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::SystemTime;

use fff_engine::{Engine, PreFilterStack};
use fff_symbol::types::{Lang, OutlineEntry};

use crate::commands::callers::CallerHit;

pub struct BfsConfig {
    pub max_hops: u32,
    pub hub_guard: usize,
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
) -> Vec<CallerHit> {
    let stack = PreFilterStack::new(engine.handles.bloom.clone());
    let mut visited: HashSet<String> = HashSet::new();
    visited.insert(initial.to_string());
    let mut frontier: Vec<String> = vec![initial.to_string()];
    let mut all_hits: Vec<CallerHit> = Vec::new();

    let confirm_input: Vec<(PathBuf, SystemTime, String)> = candidates
        .iter()
        .map(|c| (c.path.clone(), c.mtime, c.content.clone()))
        .collect();

    let max_hops = cfg.max_hops.clamp(1, 5);

    for depth in 1..=max_hops {
        if frontier.is_empty() {
            break;
        }

        let mut next_set: HashSet<String> = HashSet::new();
        let mut hub_for_name: HashMap<String, usize> = HashMap::new();

        for name in &frontier {
            let definitions = engine.handles.symbols.lookup_exact(name);
            let definition_paths: HashSet<String> = definitions
                .iter()
                .map(|d| d.path.to_string_lossy().to_string())
                .collect();

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
            }
        }

        next_set.retain(|n| !visited.contains(n));
        for n in &next_set {
            visited.insert(n.clone());
        }
        frontier = next_set.into_iter().collect();
    }

    all_hits
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
