//! Minimal MCP server stub. Speaks JSON-RPC 2.0 over stdio so any MCP-aware
//! client (Claude Code, Cursor, …) can call the same handlers as the CLI.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Parser;
use ignore::gitignore::GitignoreBuilder;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use ffs_engine::dispatch::DispatchResult;
use ffs_engine::{Engine, EngineConfig};

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
        "tools/list" => Ok(json!({
            "tools": [
                {
                    "name": "ffs_grep",
                    "description": "Search file contents (replaces Grep).",
                    "inputSchema": object_schema(json!({
                        "query": {"type": "string", "description": "Text to find in file contents."},
                        "maxResults": {"type": "number", "description": "Maximum matching lines to return."}
                    }), &["query"])
                },
                {
                    "name": "ffs_glob",
                    "description": "Match files by glob pattern (replaces Glob).",
                    "inputSchema": object_schema(json!({
                        "pattern": {"type": "string", "description": "Glob pattern, for example src/**/*.rs."},
                        "maxResults": {"type": "number", "description": "Maximum matching paths to return."}
                    }), &["pattern"])
                },
                {
                    "name": "ffs_find",
                    "description": "Fuzzy file path search.",
                    "inputSchema": object_schema(json!({
                        "query": {"type": "string", "description": "Path substring or fuzzy filename query."},
                        "maxResults": {"type": "number", "description": "Maximum matching paths to return."}
                    }), &["query"])
                },
                {
                    "name": "ffs_read",
                    "description": "Read a file with token-budget aware truncation (replaces Read).",
                    "inputSchema": object_schema(json!({
                        "path": {"type": "string", "description": "Relative or absolute file path."}
                    }), &["path"])
                },
                {
                    "name": "ffs_symbol",
                    "description": "Look up symbol definitions across the workspace.",
                    "inputSchema": object_schema(json!({
                        "name": {"type": "string", "description": "Exact symbol name to look up."}
                    }), &["name"])
                },
                {
                    "name": "ffs_dispatch",
                    "description": "Auto-classify a free-form query.",
                    "inputSchema": object_schema(json!({
                        "query": {"type": "string", "description": "Free-form search or navigation query."}
                    }), &["query"])
                },
            ]
        })),
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

fn object_schema(properties: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn handle_tool(state: &mut McpState, root: &Path, name: &str, args: &Value) -> Result<Value> {
    match name {
        "ffs_grep" => {
            let query = get_string(args, "query")?;
            let limit = get_limit(args, 20);
            let hits = grep_files(root, query, limit);
            Ok(text_json(serde_json::to_string(&hits)?))
        }
        "ffs_glob" => {
            let pattern = get_string(args, "pattern")?;
            let limit = get_limit(args, 50);
            let hits = glob_files(root, pattern, limit)?;
            Ok(text_json(serde_json::to_string(&hits)?))
        }
        "ffs_find" => {
            let query = get_string(args, "query")?;
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
            let query = get_string(args, "query")?;
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
        "ffs_symbol" => {
            let nm = get_string(args, "name")?;
            state.ensure_indexed(root);
            let hits = state.engine.handles.symbols.lookup_exact(nm);
            Ok(text_json(serde_json::to_string(&symbol_locs_to_json(
                hits,
            ))?))
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

fn get_limit(args: &Value, default: usize) -> usize {
    args.get("maxResults")
        .and_then(Value::as_f64)
        .filter(|v| v.is_finite() && *v > 0.0)
        .map(|v| v.round() as usize)
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

        assert_eq!(tools.len(), 6);
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
