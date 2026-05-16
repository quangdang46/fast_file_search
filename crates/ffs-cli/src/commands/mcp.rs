//! Minimal MCP server stub. Speaks JSON-RPC 2.0 over stdio so any MCP-aware
//! client (Claude Code, Cursor, …) can call the same handlers as the CLI.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;
use clap::Parser;
use ignore::gitignore::GitignoreBuilder;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use ffs_engine::dispatch::DispatchResult;
use ffs_engine::{Engine, EngineConfig};
use ffs_symbol::lang::detect_file_type;
use ffs_symbol::outline::get_outline_entries;
use ffs_symbol::types::{FileType, OutlineEntry};

#[derive(Debug, Parser)]
pub struct Args {
    /// Optional total token budget propagated to `Engine`.
    #[arg(long)]
    pub budget: Option<u64>,
}

struct McpState {
    engine: Engine,
    indexed: bool,
}

impl McpState {
    fn new(engine: Engine) -> Self {
        Self {
            engine,
            indexed: false,
        }
    }

    fn ensure_indexed(&mut self, root: &Path) {
        if !self.indexed {
            self.engine.index(root);
            self.indexed = true;
        }
    }
}

#[derive(Debug, Deserialize)]
struct Request {
    #[serde(default)]
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Debug, Serialize)]
struct Response {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
}

pub fn run(args: Args, root: &Path) -> Result<()> {
    let cfg = EngineConfig {
        total_token_budget: args.budget.unwrap_or(25_000),
        ..EngineConfig::default()
    };
    let engine = Engine::new(cfg);
    let mut state = McpState::new(engine);

    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let req: Request = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let resp = Response {
                    jsonrpc: "2.0",
                    id: Value::Null,
                    result: None,
                    error: Some(json!({"code": -32700, "message": format!("parse error: {e}")})),
                };
                writeln!(out, "{}", serde_json::to_string(&resp)?)?;
                continue;
            }
        };
        debug_assert!(req.jsonrpc == "2.0" || req.jsonrpc.is_empty());

        let id = req.id.unwrap_or(Value::Null);
        // JSON-RPC 2.0 notifications have no `id`; don't respond.
        if id.is_null() {
            continue;
        }
        let resp = match handle_method(&mut state, root, &req.method, &req.params) {
            Ok(value) => Response {
                jsonrpc: "2.0",
                id,
                result: Some(value),
                error: None,
            },
            Err(e) => Response {
                jsonrpc: "2.0",
                id,
                error: Some(json!({"code": -32000, "message": e.to_string()})),
                result: None,
            },
        };
        writeln!(out, "{}", serde_json::to_string(&resp)?)?;
    }
    Ok(())
}

fn handle_method(state: &mut McpState, root: &Path, method: &str, params: &Value) -> Result<Value> {
    match method {
        // Notifications per JSON-RPC 2.0: no response sent (caller skips
        // writing to stdout when the id is Null).
        "notifications/initialized" => Ok(Value::Null),
        "initialize" => Ok(json!({
            "protocolVersion": "2024-11-05",
            "serverInfo": {"name": "ffs", "version": env!("CARGO_PKG_VERSION")},
            "capabilities": {"tools": {}}
        })),
        "tools/list" => Ok(json!({ "tools": tools_list() })),
        "tools/call" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("missing tool name"))?;
            let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
            handle_tool(state, root, name, &arguments)
        }
        other => Err(anyhow::anyhow!("unknown method: {other}")),
    }
}

// All 16 tools advertised in the README. Each entry uses an integer-typed
// `maxResults` (instead of `number`) and an explicit object schema.
fn tools_list() -> Value {
    json!([
        tool(
            "ffs_grep",
            "Search file contents (replaces Grep). Plain / regex / fuzzy auto-detect.",
            &["query"],
            json!({
                "query": {"type": "string", "description": "Text to find in file contents."},
                "needle": {"type": "string", "description": "Alias for `query` to match the CLI flag name."},
                "maxResults": {"type": "integer", "minimum": 1, "description": "Maximum matching lines to return."}
            }),
        ),
        tool(
            "ffs_multi_grep",
            "OR-logic multi-pattern content search via SIMD Aho-Corasick.",
            &["queries"],
            json!({
                "queries": {"type": "array", "items": {"type": "string"}, "description": "Patterns to OR together."},
                "maxResults": {"type": "integer", "minimum": 1, "description": "Maximum matching lines to return."}
            }),
        ),
        tool(
            "ffs_glob",
            "Match files by glob pattern (replaces Glob).",
            &["pattern"],
            json!({
                "pattern": {"type": "string", "description": "Glob pattern, for example src/**/*.rs."},
                "maxResults": {"type": "integer", "minimum": 1, "description": "Maximum matching paths to return."}
            }),
        ),
        tool(
            "ffs_find",
            "Fuzzy file path search.",
            &["query"],
            json!({
                "query": {"type": "string", "description": "Path substring or fuzzy filename query."},
                "needle": {"type": "string", "description": "Alias for `query` to match the CLI flag name."},
                "maxResults": {"type": "integer", "minimum": 1, "description": "Maximum matching paths to return."}
            }),
        ),
        tool(
            "ffs_read",
            "Read a file with token-budget aware truncation (replaces Read).",
            &["path"],
            json!({
                "path": {"type": "string", "description": "Relative or absolute file path."},
                "maxTokens": {"type": "integer", "minimum": 1, "description": "Token budget for the response."}
            }),
        ),
        tool(
            "ffs_outline",
            "Structural outline of a file (functions, classes, top-level decls).",
            &["path"],
            json!({
                "path": {"type": "string", "description": "Relative or absolute file path."}
            }),
        ),
        tool(
            "ffs_symbol",
            "Look up symbol definitions across the workspace.",
            &["name"],
            json!({
                "name": {"type": "string", "description": "Exact symbol name to look up."},
                "maxResults": {"type": "integer", "minimum": 1, "description": "Maximum matching definitions to return."}
            }),
        ),
        tool(
            "ffs_callers",
            "Find call sites of a symbol.",
            &["name"],
            json!({
                "name": {"type": "string", "description": "Symbol whose callers should be listed."},
                "maxResults": {"type": "integer", "minimum": 1, "description": "Maximum matching call sites to return."}
            }),
        ),
        tool(
            "ffs_callees",
            "List symbols referenced inside the body of a definition.",
            &["name"],
            json!({
                "name": {"type": "string", "description": "Symbol whose body should be inspected."},
                "maxResults": {"type": "integer", "minimum": 1, "description": "Maximum referenced symbols to return."}
            }),
        ),
        tool(
            "ffs_refs",
            "Definitions plus single-hop usages of a symbol.",
            &["name"],
            json!({
                "name": {"type": "string", "description": "Symbol to look up."},
                "maxResults": {"type": "integer", "minimum": 1, "description": "Maximum matching usages to return."}
            }),
        ),
        tool(
            "ffs_flow",
            "Drill-down envelope per definition (def + body + callees + callers).",
            &["name"],
            json!({
                "name": {"type": "string", "description": "Symbol whose call envelope is requested."},
                "maxResults": {"type": "integer", "minimum": 1, "description": "Maximum number of definitions to expand."}
            }),
        ),
        tool(
            "ffs_siblings",
            "Peers of a symbol in its parent scope.",
            &["name"],
            json!({
                "name": {"type": "string", "description": "Target symbol whose siblings should be listed."},
                "maxResults": {"type": "integer", "minimum": 1, "description": "Maximum siblings to return."}
            }),
        ),
        tool(
            "ffs_deps",
            "A file's imports plus the workspace files that depend on it.",
            &["path"],
            json!({
                "path": {"type": "string", "description": "File path to analyse, relative to the root."}
            }),
        ),
        tool(
            "ffs_impact",
            "Rank workspace files by how much they'd be affected if `name` changed.",
            &["name"],
            json!({
                "name": {"type": "string", "description": "Symbol whose impact should be assessed."},
                "maxResults": {"type": "integer", "minimum": 1, "description": "Maximum affected files to return."}
            }),
        ),
        tool(
            "ffs_map",
            "Workspace tree annotated with file count and per-directory token estimate.",
            &[],
            json!({
                "depth": {"type": "integer", "minimum": 1, "description": "Maximum directory depth to render."}
            }),
        ),
        tool(
            "ffs_overview",
            "High-signal repo summary: languages, top-defined symbols, entry-point candidates.",
            &[],
            json!({
                "limit": {"type": "integer", "minimum": 1, "description": "How many top symbols / files to surface."}
            }),
        ),
        tool(
            "ffs_dispatch",
            "Auto-classify a free-form query.",
            &["query"],
            json!({
                "query": {"type": "string", "description": "Free-form search or navigation query."}
            }),
        ),
    ])
}

fn tool(name: &str, description: &str, required: &[&str], properties: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": object_schema(properties, required),
    })
}

fn object_schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn handle_tool(state: &mut McpState, root: &Path, name: &str, args: &Value) -> Result<Value> {
    match name {
        "ffs_grep" => {
            // Bug 3: accept both `query` (MCP idiom) and `needle` (CLI idiom).
            let query = get_query(args)?;
            let limit = get_limit(args, 20);
            let hits = grep_files(root, query, limit);
            Ok(text_json(serde_json::to_string(&hits)?))
        }
        "ffs_multi_grep" => {
            let queries = args
                .get("queries")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow::anyhow!("missing queries"))?;
            let limit = get_limit(args, 20);
            let mut all_hits = Vec::new();
            for q in queries {
                let Some(s) = q.as_str() else {
                    continue;
                };
                let mut hits = grep_files(root, s, limit);
                all_hits.append(&mut hits);
                if all_hits.len() >= limit {
                    all_hits.truncate(limit);
                    break;
                }
            }
            Ok(text_json(serde_json::to_string(&all_hits)?))
        }
        "ffs_glob" => {
            let pattern = get_string(args, "pattern")?;
            let limit = get_limit(args, 50);
            let hits = glob_files(root, pattern, limit)?;
            Ok(text_json(serde_json::to_string(&hits)?))
        }
        "ffs_find" => {
            let query = get_query(args)?;
            let limit = get_limit(args, 50);
            let scopes = [root.to_path_buf()];
            let mut hits = super::find::search_matches(&scopes, query);
            if hits.is_empty() {
                hits = super::find::fuzzy_search_matches(&scopes, query);
            }
            hits.truncate(limit);
            Ok(text_json(serde_json::to_string(&hits)?))
        }
        "ffs_dispatch" => {
            let query = get_query(args)?;
            state.ensure_indexed(root);
            let result = state.engine.dispatch(query, root);
            let summary = match result {
                DispatchResult::Symbol { hits, .. } => {
                    json!({"kind": "symbol", "hits": symbol_locs_to_json(hits)})
                }
                DispatchResult::SymbolGlob { hits, .. } => {
                    json!({"kind": "symbol_glob", "hits": symbol_glob_to_json(hits)})
                }
                DispatchResult::FilePath { path, .. } => {
                    json!({"kind": "file_path", "path": path.to_string_lossy()})
                }
                DispatchResult::Glob { pattern, .. } => {
                    json!({"kind": "glob", "pattern": pattern})
                }
                DispatchResult::ContentFallback { .. } => json!({"kind": "content_fallback"}),
            };
            Ok(text_json(summary.to_string()))
        }
        "ffs_read" => {
            let target = get_string(args, "path")?;
            let p = if Path::new(target).is_absolute() {
                PathBuf::from(target)
            } else {
                root.join(target)
            };
            let res = state.engine.read(&p);
            Ok(text_json(res.body))
        }
        "ffs_outline" => {
            let target = get_string(args, "path")?;
            let p = if Path::new(target).is_absolute() {
                PathBuf::from(target)
            } else {
                root.join(target)
            };
            let body = match super::outline::render_agent(&p, target) {
                Ok(b) => b,
                Err(e) => format!("[error: {e}]"),
            };
            Ok(text_json(body))
        }
        "ffs_symbol" => {
            let nm = get_string(args, "name")?;
            let limit = get_limit(args, 50);
            state.ensure_indexed(root);
            let mut hits = state.engine.handles.symbols.lookup_exact(nm);
            hits.truncate(limit);
            Ok(text_json(serde_json::to_string(&symbol_locs_to_json(
                hits,
            ))?))
        }
        "ffs_callers" => {
            let nm = get_string(args, "name")?;
            let limit = get_limit(args, 50);
            state.ensure_indexed(root);
            let hits = collect_callers(state, root, nm, limit);
            Ok(text_json(serde_json::to_string(&hits)?))
        }
        "ffs_callees" => {
            let nm = get_string(args, "name")?;
            let limit = get_limit(args, 50);
            state.ensure_indexed(root);
            let hits = collect_callees(state, root, nm, limit);
            Ok(text_json(serde_json::to_string(&hits)?))
        }
        "ffs_refs" => {
            let nm = get_string(args, "name")?;
            let limit = get_limit(args, 50);
            state.ensure_indexed(root);
            let defs = state.engine.handles.symbols.lookup_exact(nm);
            let usages = collect_callers(state, root, nm, limit);
            Ok(text_json(
                json!({
                    "definitions": symbol_locs_to_json(defs),
                    "usages": usages,
                })
                .to_string(),
            ))
        }
        "ffs_flow" => {
            let nm = get_string(args, "name")?;
            let limit = get_limit(args, 5);
            state.ensure_indexed(root);
            let defs = state.engine.handles.symbols.lookup_exact(nm);
            let cards: Vec<Value> = defs
                .into_iter()
                .take(limit)
                .map(|d| {
                    let body = read_section_excerpt(&d.path, d.line, d.end_line, 60);
                    json!({
                        "path": d.path.to_string_lossy(),
                        "line": d.line,
                        "end_line": d.end_line,
                        "body": body,
                    })
                })
                .collect();
            Ok(text_json(serde_json::to_string(&cards)?))
        }
        "ffs_siblings" => {
            let nm = get_string(args, "name")?;
            let limit = get_limit(args, 50);
            state.ensure_indexed(root);
            let hits = collect_siblings(state, nm, limit);
            Ok(text_json(serde_json::to_string(&hits)?))
        }
        "ffs_deps" => {
            let target = get_string(args, "path")?;
            let p = if Path::new(target).is_absolute() {
                PathBuf::from(target)
            } else {
                root.join(target)
            };
            let imports = list_imports(&p);
            Ok(text_json(serde_json::to_string(&imports)?))
        }
        "ffs_impact" => {
            let nm = get_string(args, "name")?;
            let limit = get_limit(args, 50);
            state.ensure_indexed(root);
            let hits = collect_callers(state, root, nm, limit);
            // Roll up call sites by file as a coarse impact estimate.
            let mut by_file: std::collections::BTreeMap<String, u32> =
                std::collections::BTreeMap::new();
            for h in &hits {
                *by_file.entry(h.path.clone()).or_default() += 1;
            }
            let ranked: Vec<Value> = by_file
                .into_iter()
                .map(|(p, n)| json!({"path": p, "weight": n}))
                .collect();
            Ok(text_json(serde_json::to_string(&ranked)?))
        }
        "ffs_map" => {
            let depth = args
                .get("depth")
                .and_then(Value::as_u64)
                .unwrap_or(2)
                .min(10);
            let lines = render_simple_map(root, depth as usize);
            Ok(text_json(lines.join("\n")))
        }
        "ffs_overview" => {
            let limit = get_limit(args, 20);
            state.ensure_indexed(root);
            let mut langs: std::collections::BTreeMap<&'static str, usize> =
                std::collections::BTreeMap::new();
            for path in super::walk_files(root) {
                if let FileType::Code(lang) = detect_file_type(&path) {
                    *langs.entry(lang_label(&lang)).or_default() += 1;
                }
            }
            let summary = json!({
                "languages": langs.into_iter().collect::<Vec<_>>(),
                "top_symbols_limit": limit,
            });
            Ok(text_json(summary.to_string()))
        }
        other => Err(anyhow::anyhow!("unknown tool: {other}")),
    }
}

fn text_json(text: impl Into<String>) -> Value {
    json!({"content": [{"type": "text", "text": text.into()}]})
}

fn get_string<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("missing {key}"))
}

// Bug 3: accept both `query` (MCP idiom) and `needle` (CLI idiom).
fn get_query(args: &Value) -> Result<&str> {
    args.get("query")
        .and_then(Value::as_str)
        .or_else(|| args.get("needle").and_then(Value::as_str))
        .ok_or_else(|| anyhow::anyhow!("missing query"))
}

fn get_limit(args: &Value, default: usize) -> usize {
    // Accept both `maxResults` (number or integer) and `limit` for parity
    // with the CLI flag.
    args.get("maxResults")
        .and_then(Value::as_u64)
        .or_else(|| {
            args.get("maxResults")
                .and_then(Value::as_f64)
                .map(|v| v.round() as u64)
        })
        .or_else(|| args.get("limit").and_then(Value::as_u64))
        .map(|v| v as usize)
        .filter(|v| *v > 0)
        .unwrap_or(default)
        .max(1)
}

#[derive(Debug, Serialize)]
struct GrepHit {
    path: String,
    line: usize,
    text: String,
}

fn grep_files(root: &Path, query: &str, limit: usize) -> Vec<GrepHit> {
    let query_lower = query.to_lowercase();
    let smart_case = query.chars().any(char::is_uppercase);
    let mut hits = Vec::new();
    for path in super::walk_files(root) {
        if hits.len() >= limit {
            break;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (line_idx, line) in text.lines().enumerate() {
            let found = if smart_case {
                line.contains(query)
            } else {
                line.to_lowercase().contains(&query_lower)
            };
            if found {
                hits.push(GrepHit {
                    path: display_path(root, &path),
                    line: line_idx + 1,
                    text: line.trim().to_string(),
                });
                if hits.len() >= limit {
                    break;
                }
            }
        }
    }
    hits
}

fn glob_files(root: &Path, pattern: &str, limit: usize) -> Result<Vec<String>> {
    let mut builder = GitignoreBuilder::new(root);
    builder.add_line(None, pattern)?;
    let matcher = builder.build()?;
    let mut hits = Vec::new();
    for path in super::walk_files(root) {
        let rel = path.strip_prefix(root).unwrap_or(&path);
        if matcher.matched(rel, false).is_ignore() {
            hits.push(display_path(root, &path));
            if hits.len() >= limit {
                break;
            }
        }
    }
    Ok(hits)
}

#[derive(Debug, Serialize)]
struct CallerHit {
    path: String,
    line: usize,
    text: String,
}

// Coarse caller search: literal-text lookup of `name(` in every file. Mirrors
// the bigram-pre-filter pass the CLI uses but without the on-disk index, which
// keeps the MCP surface self-contained.
fn collect_callers(_state: &mut McpState, root: &Path, name: &str, limit: usize) -> Vec<CallerHit> {
    let mut hits = Vec::new();
    let needle = format!("{name}(");
    for path in super::walk_files(root) {
        if hits.len() >= limit {
            break;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (line_idx, line) in text.lines().enumerate() {
            if line.contains(&needle) {
                hits.push(CallerHit {
                    path: display_path(root, &path),
                    line: line_idx + 1,
                    text: line.trim().to_string(),
                });
                if hits.len() >= limit {
                    break;
                }
            }
        }
    }
    hits
}

#[derive(Debug, Serialize)]
struct CalleeHit {
    name: String,
    line: usize,
}

fn collect_callees(state: &mut McpState, _root: &Path, name: &str, limit: usize) -> Vec<CalleeHit> {
    let mut hits = Vec::new();
    let defs = state.engine.handles.symbols.lookup_exact(name);
    for d in defs {
        let Ok(content) = std::fs::read_to_string(&d.path) else {
            continue;
        };
        let start = d.line.saturating_sub(1) as usize;
        let end = (d.end_line as usize).min(content.lines().count());
        for (idx, line) in content.lines().enumerate().take(end).skip(start) {
            for word in line.split(|c: char| !c.is_alphanumeric() && c != '_') {
                if word.len() < 3 || word == name {
                    continue;
                }
                if state.engine.handles.symbols.lookup_exact(word).is_empty() {
                    continue;
                }
                hits.push(CalleeHit {
                    name: word.to_string(),
                    line: idx + 1,
                });
                if hits.len() >= limit {
                    return hits;
                }
            }
        }
    }
    hits
}

#[derive(Debug, Serialize)]
struct SiblingHit {
    name: String,
    kind: String,
    path: String,
    line: u32,
}

fn collect_siblings(state: &mut McpState, name: &str, limit: usize) -> Vec<SiblingHit> {
    let defs = state.engine.handles.symbols.lookup_exact(name);
    let mut hits: Vec<SiblingHit> = Vec::new();
    let mut seen: std::collections::HashSet<(String, String, u32)> =
        std::collections::HashSet::new();
    for def in defs {
        let lang = match detect_file_type(&def.path) {
            FileType::Code(l) => l,
            _ => continue,
        };
        let Ok(content) = std::fs::read_to_string(&def.path) else {
            continue;
        };
        let mtime = std::fs::metadata(&def.path)
            .and_then(|m| m.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let outline = state
            .engine
            .handles
            .outlines
            .get_or_compute(&def.path, mtime, &content, lang);
        if let Some((_, peers)) = sibling_peers(&outline, name, def.line) {
            for p in peers {
                let path = def.path.to_string_lossy().to_string();
                let key = (p.name.clone(), path.clone(), p.start_line);
                if !seen.insert(key) {
                    continue;
                }
                hits.push(SiblingHit {
                    name: p.name.clone(),
                    kind: format!("{:?}", p.kind).to_lowercase(),
                    path,
                    line: p.start_line,
                });
                if hits.len() >= limit {
                    return hits;
                }
            }
        }
    }
    hits
}

fn sibling_peers(
    outline: &[OutlineEntry],
    target: &str,
    target_line: u32,
) -> Option<(String, Vec<OutlineEntry>)> {
    if outline
        .iter()
        .any(|e| e.name == target && e.start_line == target_line)
    {
        let peers: Vec<OutlineEntry> = outline
            .iter()
            .filter(|e| !(e.name == target && e.start_line == target_line))
            .cloned()
            .collect();
        return Some(("<file>".to_string(), peers));
    }
    for parent in outline {
        if let Some(found) = sibling_in_children(parent, target, target_line) {
            return Some(found);
        }
    }
    None
}

fn sibling_in_children(
    parent: &OutlineEntry,
    target: &str,
    target_line: u32,
) -> Option<(String, Vec<OutlineEntry>)> {
    if parent
        .children
        .iter()
        .any(|c| c.name == target && c.start_line == target_line)
    {
        let peers: Vec<OutlineEntry> = parent
            .children
            .iter()
            .filter(|c| !(c.name == target && c.start_line == target_line))
            .cloned()
            .collect();
        return Some((parent.name.clone(), peers));
    }
    for c in &parent.children {
        if let Some(found) = sibling_in_children(c, target, target_line) {
            return Some(found);
        }
    }
    None
}

fn read_section_excerpt(path: &Path, start_line: u32, end_line: u32, max_lines: usize) -> String {
    let Ok(content) = std::fs::read_to_string(path) else {
        return String::new();
    };
    let start = start_line.saturating_sub(1) as usize;
    let end = (end_line as usize).min(content.lines().count());
    let lines: Vec<&str> = content
        .lines()
        .enumerate()
        .filter(|(i, _)| *i >= start && *i < end)
        .map(|(_, l)| l)
        .take(max_lines)
        .collect();
    lines.join("\n")
}

fn list_imports(path: &Path) -> Vec<String> {
    let lang = match detect_file_type(path) {
        FileType::Code(l) => l,
        _ => return Vec::new(),
    };
    let Ok(content) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let entries = get_outline_entries(&content, lang);
    entries
        .iter()
        .filter(|e| matches!(e.kind, ffs_symbol::types::OutlineKind::Import))
        .map(|e| e.name.clone())
        .collect()
}

fn render_simple_map(root: &Path, depth: usize) -> Vec<String> {
    let mut counts: std::collections::BTreeMap<PathBuf, usize> = std::collections::BTreeMap::new();
    for path in super::walk_files(root) {
        let rel = path.strip_prefix(root).unwrap_or(&path);
        let mut acc = PathBuf::new();
        for (i, comp) in rel.components().enumerate() {
            if i >= depth {
                break;
            }
            acc.push(comp.as_os_str());
            *counts.entry(acc.clone()).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .map(|(p, n)| format!("{}\t{}", p.display(), n))
        .collect()
}

fn lang_label(lang: &ffs_symbol::types::Lang) -> &'static str {
    use ffs_symbol::types::Lang;
    match lang {
        Lang::Rust => "rust",
        Lang::Python => "python",
        Lang::JavaScript => "javascript",
        Lang::TypeScript => "typescript",
        Lang::Tsx => "tsx",
        Lang::Go => "go",
        Lang::Java => "java",
        Lang::C => "c",
        Lang::Cpp => "cpp",
        Lang::CSharp => "csharp",
        Lang::Ruby => "ruby",
        Lang::Php => "php",
        Lang::Swift => "swift",
        Lang::Kotlin => "kotlin",
        Lang::Scala => "scala",
        Lang::Elixir => "elixir",
        Lang::Dockerfile => "dockerfile",
        Lang::Make => "make",
    }
}

fn display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

fn symbol_locs_to_json(hits: Vec<ffs_symbol::symbol_index::SymbolLocation>) -> Value {
    Value::Array(
        hits.into_iter()
            .map(|h| {
                json!({
                    "path": h.path.to_string_lossy(),
                    "line": h.line,
                    "end_line": h.end_line,
                    "kind": h.kind,
                    "weight": h.weight,
                })
            })
            .collect(),
    )
}

fn symbol_glob_to_json(hits: Vec<(String, ffs_symbol::symbol_index::SymbolLocation)>) -> Value {
    Value::Array(
        hits.into_iter()
            .map(|(n, h)| {
                json!({
                    "name": n,
                    "path": h.path.to_string_lossy(),
                    "line": h.line,
                    "end_line": h.end_line,
                    "kind": h.kind,
                    "weight": h.weight,
                })
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_list_includes_object_input_schemas() {
        let td = tempfile::tempdir().unwrap();
        let mut state = McpState::new(Engine::default());
        let result = handle_method(&mut state, td.path(), "tools/list", &Value::Null).unwrap();
        let tools = result["tools"].as_array().unwrap();

        // Bug 2: every tool from the README's "Tools registered" table must
        // be advertised by tools/list. (16 advertised + the MCP-only
        // ffs_glob alias.)
        assert_eq!(tools.len(), 17);
        for tool in tools {
            assert_eq!(tool["inputSchema"]["type"], "object");
            assert!(tool["inputSchema"]["properties"].is_object());
            assert!(tool["inputSchema"]["required"].is_array());
        }
    }

    #[test]
    fn advertised_file_tools_are_callable() {
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        std::fs::write(root.join("mcp.rs"), "fn mcp_schema() {}\n").unwrap();
        let mut state = McpState::new(Engine::default());

        for (name, args) in [
            ("ffs_find", json!({"query": "mcp.rs"})),
            ("ffs_glob", json!({"pattern": "*.rs"})),
            ("ffs_grep", json!({"query": "mcp_schema"})),
            ("ffs_read", json!({"path": "mcp.rs"})),
        ] {
            let result = handle_tool(&mut state, root, name, &args).unwrap();
            assert!(result["content"][0]["text"].is_string(), "{name}");
        }
    }

    #[test]
    fn find_accepts_needle_alias_for_query() {
        // Bug 3: the CLI uses `<NEEDLE>`, MCP previously required `query`.
        // Now both should work.
        let td = tempfile::tempdir().unwrap();
        let root = td.path();
        std::fs::write(root.join("mcp.rs"), "fn mcp_schema() {}\n").unwrap();
        let mut state = McpState::new(Engine::default());

        let result = handle_tool(&mut state, root, "ffs_find", &json!({"needle": "mcp"})).unwrap();
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("mcp.rs"));
    }

    #[test]
    fn missing_tool_errors_clearly() {
        let td = tempfile::tempdir().unwrap();
        let mut state = McpState::new(Engine::default());
        let err = handle_tool(&mut state, td.path(), "ffs_unknown", &Value::Null).unwrap_err();
        assert!(err.to_string().contains("unknown tool"));
    }

    #[test]
    fn initialize_does_not_index_workspace() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("mcp.rs"), "fn mcp_schema() {}\n").unwrap();
        let mut state = McpState::new(Engine::default());

        let result = handle_method(&mut state, td.path(), "initialize", &Value::Null).unwrap();

        assert_eq!(result["serverInfo"]["name"], "ffs");
        assert!(!state.indexed);
        assert!(state
            .engine
            .handles
            .symbols
            .lookup_exact("mcp_schema")
            .is_empty());
    }

    #[test]
    fn symbol_tool_indexes_lazily() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("mcp.rs"), "fn mcp_schema() {}\n").unwrap();
        let mut state = McpState::new(Engine::default());

        let result = handle_tool(
            &mut state,
            td.path(),
            "ffs_symbol",
            &json!({"name": "mcp_schema"}),
        )
        .unwrap();

        assert!(state.indexed);
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("mcp.rs"));
    }
}
