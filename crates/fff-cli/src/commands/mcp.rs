//! Minimal MCP server stub. Speaks JSON-RPC 2.0 over stdio so any MCP-aware
//! client (Claude Code, Cursor, …) can call the same handlers as the CLI.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Parser;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use fff_engine::dispatch::DispatchResult;
use fff_engine::{Engine, EngineConfig};

#[derive(Debug, Parser)]
pub struct Args {
    /// Optional total token budget propagated to `Engine`.
    #[arg(long)]
    pub budget: Option<u64>,
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
    engine.index(root);

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
        let resp = match handle_method(&engine, root, &req.method, &req.params) {
            Ok(value) => Response {
                jsonrpc: "2.0",
                id,
                result: Some(value),
                error: None,
            },
            Err(e) => Response {
                jsonrpc: "2.0",
                id,
                result: None,
                error: Some(json!({"code": -32000, "message": e.to_string()})),
            },
        };
        writeln!(out, "{}", serde_json::to_string(&resp)?)?;
    }
    Ok(())
}

fn handle_method(engine: &Engine, root: &Path, method: &str, params: &Value) -> Result<Value> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": "2024-11-05",
            "serverInfo": {"name": "scry", "version": env!("CARGO_PKG_VERSION")},
            "capabilities": {"tools": {}}
        })),
        "tools/list" => Ok(json!({
            "tools": [
                {"name": "scry_grep", "description": "Search file contents (replaces Grep)."},
                {"name": "scry_glob", "description": "Match files by glob pattern (replaces Glob)."},
                {"name": "scry_find", "description": "Fuzzy file path search."},
                {"name": "scry_read", "description": "Read a file with token-budget aware truncation (replaces Read)."},
                {"name": "scry_symbol", "description": "Look up symbol definitions across the workspace."},
                {"name": "scry_dispatch", "description": "Auto-classify a free-form query."},
            ]
        })),
        "tools/call" => {
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("missing tool name"))?;
            let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
            handle_tool(engine, root, name, &arguments)
        }
        other => Err(anyhow::anyhow!("unknown method: {other}")),
    }
}

fn handle_tool(engine: &Engine, root: &Path, name: &str, args: &Value) -> Result<Value> {
    match name {
        "scry_dispatch" => {
            let query = args
                .get("query")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("missing query"))?;
            let result = engine.dispatch(query, root);
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
            Ok(json!({"content": [{"type": "text", "text": summary.to_string()}]}))
        }
        "scry_read" => {
            let target = args
                .get("path")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("missing path"))?;
            let p = if Path::new(target).is_absolute() {
                PathBuf::from(target)
            } else {
                root.join(target)
            };
            let res = engine.read(&p);
            Ok(json!({"content": [{"type": "text", "text": res.body}]}))
        }
        "scry_symbol" => {
            let nm = args
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("missing name"))?;
            let hits = engine.handles.symbols.lookup_exact(nm);
            Ok(
                json!({"content": [{"type": "text", "text": serde_json::to_string(&symbol_locs_to_json(hits))?}]}),
            )
        }
        other => Err(anyhow::anyhow!("unknown tool: {other}")),
    }
}

fn symbol_locs_to_json(hits: Vec<fff_symbol::symbol_index::SymbolLocation>) -> Value {
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

fn symbol_glob_to_json(hits: Vec<(String, fff_symbol::symbol_index::SymbolLocation)>) -> Value {
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
