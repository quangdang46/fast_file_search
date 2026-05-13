// Forward-graph BFS for `scry callees --depth N`. Each hop:
// 1. For every name in the current frontier, look up definitions and run
//    `collect_callees` over each definition's body to get the set of called
//    identifiers.
// 2. Resolve each identifier back to all known definition sites; emit one
//    hit per (caller, target_def) pair tagged with the current depth and the
//    enclosing symbol that produced it (`from`).
// 3. Add the called identifiers to the next frontier, modulo `hub_guard`:
//    if a single name produces more than `hub_guard` callee hits at this
//    hop, its propagation is skipped (but the hits still surface).
//
// Frontier is name-keyed and visited-tracked, so a symbol with many
// definition sites is expanded once per hop.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use ffs_engine::Engine;
use ffs_symbol::batch::batch_lookup;
use ffs_symbol::lang::detect_file_type;
use ffs_symbol::types::FileType;

use crate::commands::callees_resolve::collect_callees;

pub struct BfsConfig {
    pub max_hops: u32,
    pub hub_guard: usize,
}

#[derive(Debug, Serialize, Clone)]
pub struct CalleeHit {
    pub name: String,
    pub path: String,
    pub line: u32,
    pub depth: u32,
    pub from: String,
}

pub fn run_bfs(engine: &Engine, initial: &str, cfg: BfsConfig) -> Vec<CalleeHit> {
    let mut visited: HashSet<String> = HashSet::new();
    visited.insert(initial.to_string());
    let mut frontier: Vec<String> = vec![initial.to_string()];
    let mut all_hits: Vec<CalleeHit> = Vec::new();

    let max_hops = cfg.max_hops.clamp(1, 5);

    for depth in 1..=max_hops {
        if frontier.is_empty() {
            break;
        }

        let mut next_set: HashSet<String> = HashSet::new();
        let mut hub_for_name: HashMap<String, usize> = HashMap::new();

        for name in &frontier {
            let mut hits_for_name = 0usize;
            let mut idents_this_name: Vec<String> = Vec::new();

            for def in engine.handles.symbols.lookup_exact(name) {
                let lang = match detect_file_type(&def.path) {
                    FileType::Code(l) => l,
                    _ => continue,
                };
                let Ok(content) = std::fs::read_to_string(&def.path) else {
                    continue;
                };
                let Some(idents) = collect_callees(&content, lang, def.line, def.end_line) else {
                    continue;
                };
                let lookup_names: Vec<&str> = idents
                    .iter()
                    .filter(|i| i.as_str() != name)
                    .map(String::as_str)
                    .collect();
                for result in batch_lookup(&engine.handles.symbols, &lookup_names) {
                    if result.locations.is_empty() {
                        continue;
                    }
                    idents_this_name.push(result.symbol.clone());
                    for loc in result.locations {
                        all_hits.push(CalleeHit {
                            name: result.symbol.clone(),
                            path: loc.path.to_string_lossy().to_string(),
                            line: loc.line,
                            depth,
                            from: name.clone(),
                        });
                        hits_for_name += 1;
                    }
                }
            }

            hub_for_name.insert(name.clone(), hits_for_name);
            if hits_for_name <= cfg.hub_guard {
                for i in idents_this_name {
                    next_set.insert(i);
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

#[cfg(test)]
mod tests {
    use super::*;
    use ffs_engine::Engine;
    use std::fs;
    use tempfile::TempDir;

    fn write(dir: &std::path::Path, name: &str, body: &str) {
        fs::write(dir.join(name), body).unwrap();
    }

    fn engine_for(root: &std::path::Path) -> Engine {
        let e = Engine::default();
        e.index(root);
        e
    }

    #[test]
    fn depth_one_returns_only_direct_callees() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "lib.rs",
            r#"
fn alpha() { beta(); }
fn beta() { gamma(); }
fn gamma() {}
"#,
        );
        let e = engine_for(tmp.path());
        let hits = run_bfs(
            &e,
            "alpha",
            BfsConfig {
                max_hops: 1,
                hub_guard: 50,
            },
        );
        let names: Vec<&str> = hits.iter().map(|h| h.name.as_str()).collect();
        assert!(names.contains(&"beta"));
        assert!(!names.contains(&"gamma"));
        assert!(hits.iter().all(|h| h.depth == 1));
        assert!(hits.iter().all(|h| h.from == "alpha"));
    }

    #[test]
    fn depth_two_reaches_grandchildren() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "lib.rs",
            r#"
fn alpha() { beta(); }
fn beta() { gamma(); }
fn gamma() {}
"#,
        );
        let e = engine_for(tmp.path());
        let hits = run_bfs(
            &e,
            "alpha",
            BfsConfig {
                max_hops: 2,
                hub_guard: 50,
            },
        );
        let beta = hits
            .iter()
            .find(|h| h.name == "beta")
            .expect("beta missing");
        let gamma = hits
            .iter()
            .find(|h| h.name == "gamma")
            .expect("gamma missing");
        assert_eq!(beta.depth, 1);
        assert_eq!(beta.from, "alpha");
        assert_eq!(gamma.depth, 2);
        assert_eq!(gamma.from, "beta");
    }

    #[test]
    fn hub_guard_blocks_propagation_but_keeps_hits() {
        let tmp = TempDir::new().unwrap();
        // alpha calls hub once; hub calls a, b, c — three hits at hub. With
        // hub_guard=2 the hub's body is still scanned and hits surface, but
        // a/b/c don't enter the next frontier.
        write(
            tmp.path(),
            "lib.rs",
            r#"
fn alpha() { hub(); }
fn hub() { a(); b(); c(); }
fn a() { d(); }
fn b() {}
fn c() {}
fn d() {}
"#,
        );
        let e = engine_for(tmp.path());
        let hits = run_bfs(
            &e,
            "alpha",
            BfsConfig {
                max_hops: 3,
                hub_guard: 2,
            },
        );
        let names: HashSet<&str> = hits.iter().map(|h| h.name.as_str()).collect();
        assert!(names.contains("hub"));
        assert!(names.contains("a"));
        assert!(names.contains("b"));
        assert!(names.contains("c"));
        // d is two hops past the hub-guarded frontier; without the guard it
        // would have surfaced at depth 3.
        assert!(!names.contains("d"));
    }

    #[test]
    fn cycle_terminates_without_repeating_expansion() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "lib.rs",
            r#"
fn alpha() { beta(); }
fn beta() { alpha(); }
"#,
        );
        let e = engine_for(tmp.path());
        let hits = run_bfs(
            &e,
            "alpha",
            BfsConfig {
                max_hops: 5,
                hub_guard: 50,
            },
        );
        // beta surfaces at depth 1 (alpha calls beta). Calling alpha back
        // from beta is a legitimate hit at depth 2 — what `visited` prevents
        // is re-expanding alpha into a third pass. Each (name, depth) appears
        // at most once.
        assert!(hits.iter().any(|h| h.name == "beta" && h.depth == 1));
        let alpha_at_2: Vec<&CalleeHit> = hits
            .iter()
            .filter(|h| h.name == "alpha" && h.depth == 2)
            .collect();
        assert_eq!(alpha_at_2.len(), 1);
        assert!(hits.iter().all(|h| h.depth <= 2));
    }

    #[test]
    fn max_hops_clamps_to_five() {
        let tmp = TempDir::new().unwrap();
        write(
            tmp.path(),
            "lib.rs",
            r#"
fn alpha() {}
"#,
        );
        let e = engine_for(tmp.path());
        // No callees at any depth — BFS must terminate quickly even when the
        // caller asks for 100 hops.
        let hits = run_bfs(
            &e,
            "alpha",
            BfsConfig {
                max_hops: 100,
                hub_guard: 50,
            },
        );
        assert!(hits.is_empty());
    }
}
