//! Engine Lua bindings — additive layer on top of the existing ffs-nvim API.
//!
//! Existing exports (`init_db`, `init_file_picker`, `scan_files`,
//! `fuzzy_search_files`, `live_grep`, etc.) are byte-for-byte unchanged.
//! These bindings expose the new `ffs-engine` (symbol index, dispatch,
//! token-budgeted read) under the `engine_*` namespace.
//!
//! Lifecycle:
//!   - `engine_init(root)` builds the engine and runs the initial unified scan.
//!   - `engine_dispatch / engine_symbol / engine_grep / engine_read` query the warm
//!     caches.
//!   - `engine_rebuild()` re-runs the unified scan against the same root.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mlua::prelude::*;
use once_cell::sync::Lazy;
use parking_lot::RwLock;

use ffs_budget::FilterLevel;
use ffs_engine::dispatch::DispatchResult;
use ffs_engine::{Engine, EngineConfig};

struct EngineState {
    engine: Arc<Engine>,
    root: PathBuf,
}

static ENGINE_STATE: Lazy<RwLock<Option<EngineState>>> = Lazy::new(|| RwLock::new(None));

fn parse_filter_level(raw: Option<String>) -> FilterLevel {
    match raw.as_deref() {
        Some("none") => FilterLevel::None,
        Some("aggressive") => FilterLevel::Aggressive,
        _ => FilterLevel::Minimal,
    }
}

fn ensure_state() -> Result<EngineState, mlua::Error> {
    ENGINE_STATE
        .read()
        .as_ref()
        .map(|s| EngineState {
            engine: s.engine.clone(),
            root: s.root.clone(),
        })
        .ok_or_else(|| mlua::Error::RuntimeError("engine_init() not called yet".to_string()))
}

pub fn engine_init(_: &Lua, (root, opts): (String, Option<LuaTable>)) -> LuaResult<bool> {
    let total_token_budget: u64 = opts
        .as_ref()
        .and_then(|t| t.get::<u64>("total_token_budget").ok())
        .unwrap_or(25_000);

    let cfg = EngineConfig {
        total_token_budget,
        ..EngineConfig::default()
    };
    let engine = Arc::new(Engine::new(cfg));
    let root_path = PathBuf::from(&root);
    engine.index(&root_path);
    *ENGINE_STATE.write() = Some(EngineState {
        engine,
        root: root_path,
    });
    Ok(true)
}

pub fn engine_rebuild(_: &Lua, _: ()) -> LuaResult<bool> {
    let state = ensure_state()?;
    state.engine.handles.symbols.clear();
    state.engine.index(&state.root);
    Ok(true)
}

pub fn engine_dispatch(lua: &Lua, query: String) -> LuaResult<LuaTable> {
    let state = ensure_state()?;
    let result = state.engine.dispatch(&query, &state.root);
    let tbl = lua.create_table()?;
    match result {
        DispatchResult::Symbol { hits, classified } => {
            tbl.set("kind", "symbol")?;
            tbl.set("raw", classified.raw)?;
            let arr = lua.create_table()?;
            for (i, h) in hits.iter().enumerate() {
                let row = lua.create_table()?;
                row.set("path", h.path.display().to_string())?;
                row.set("line", h.line)?;
                row.set("kind", h.kind.as_str())?;
                arr.set(i + 1, row)?;
            }
            tbl.set("hits", arr)?;
        }
        DispatchResult::SymbolGlob { hits, classified } => {
            tbl.set("kind", "symbol_glob")?;
            tbl.set("raw", classified.raw)?;
            let arr = lua.create_table()?;
            for (i, (name, h)) in hits.iter().enumerate() {
                let row = lua.create_table()?;
                row.set("name", name.as_str())?;
                row.set("path", h.path.display().to_string())?;
                row.set("line", h.line)?;
                arr.set(i + 1, row)?;
            }
            tbl.set("hits", arr)?;
        }
        DispatchResult::Glob {
            classified,
            pattern,
        } => {
            tbl.set("kind", "glob")?;
            tbl.set("raw", classified.raw)?;
            tbl.set("pattern", pattern)?;
        }
        DispatchResult::FilePath { classified, path } => {
            tbl.set("kind", "file_path")?;
            tbl.set("raw", classified.raw)?;
            tbl.set("path", path.display().to_string())?;
        }
        DispatchResult::ContentFallback { classified } => {
            tbl.set("kind", "content_fallback")?;
            tbl.set("raw", classified.raw)?;
        }
    }
    Ok(tbl)
}

pub fn engine_symbol(lua: &Lua, name: String) -> LuaResult<LuaTable> {
    let state = ensure_state()?;
    let arr = lua.create_table()?;
    if let Some(prefix) = name.strip_suffix('*') {
        let hits = state.engine.handles.symbols.lookup_prefix(prefix);
        for (i, (sym, h)) in hits.iter().enumerate() {
            let row = lua.create_table()?;
            row.set("name", sym.as_str())?;
            row.set("path", h.path.display().to_string())?;
            row.set("line", h.line)?;
            row.set("kind", h.kind.as_str())?;
            arr.set(i + 1, row)?;
        }
    } else {
        let hits = state.engine.handles.symbols.lookup_exact(&name);
        for (i, h) in hits.iter().enumerate() {
            let row = lua.create_table()?;
            row.set("name", name.as_str())?;
            row.set("path", h.path.display().to_string())?;
            row.set("line", h.line)?;
            row.set("kind", h.kind.as_str())?;
            arr.set(i + 1, row)?;
        }
    }
    Ok(arr)
}

pub fn engine_grep(lua: &Lua, pattern: String) -> LuaResult<LuaTable> {
    let state = ensure_state()?;
    let mut out = Vec::new();
    let walker = ignore::WalkBuilder::new(&state.root)
        .standard_filters(true)
        .follow_links(false)
        .build();
    let needle = pattern.as_str();
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.into_path();
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (idx, line) in content.lines().enumerate() {
            if line.contains(needle) {
                out.push((path.clone(), (idx + 1) as u32, line.to_string()));
                if out.len() >= 500 {
                    break;
                }
            }
        }
        if out.len() >= 500 {
            break;
        }
    }
    let arr = lua.create_table()?;
    for (i, (p, ln, text)) in out.into_iter().enumerate() {
        let row = lua.create_table()?;
        row.set("path", p.display().to_string())?;
        row.set("line", ln)?;
        row.set("text", text)?;
        arr.set(i + 1, row)?;
    }
    Ok(arr)
}

// Invoke the ffs CLI (current_exe) and return stdout as a string. Used by
// the additive `engine_refs` / `engine_flow` / `engine_impact` Lua exports — those
// reimplementations would be too heavy here, so we shell out to the same
// binary that loaded this `ffs_nvim` module.
fn run_engine_subprocess(subcommand: &str, root: &Path, args: &[String]) -> mlua::Result<String> {
    let exe = std::env::current_exe().map_err(|e| mlua::Error::RuntimeError(e.to_string()))?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--root")
        .arg(root)
        .arg("--format")
        .arg("json")
        .arg(subcommand);
    for a in args {
        cmd.arg(a);
    }
    let out = cmd
        .output()
        .map_err(|e| mlua::Error::RuntimeError(e.to_string()))?;
    if !out.status.success() {
        return Err(mlua::Error::RuntimeError(format!(
            "ffs {subcommand} exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

pub fn engine_refs(
    _: &Lua,
    (name, limit, offset): (String, Option<u64>, Option<u64>),
) -> LuaResult<String> {
    let state = ensure_state()?;
    let mut args = vec![name];
    if let Some(n) = limit {
        args.push("--limit".into());
        args.push(n.to_string());
    }
    if let Some(n) = offset {
        args.push("--offset".into());
        args.push(n.to_string());
    }
    run_engine_subprocess("refs", &state.root, &args)
}

pub fn engine_flow(_: &Lua, (name, opts): (String, Option<LuaTable>)) -> LuaResult<String> {
    let state = ensure_state()?;
    let mut args = vec![name];
    if let Some(t) = opts {
        for (flag, key) in [
            ("--limit", "limit"),
            ("--offset", "offset"),
            ("--callees-top", "callees_top"),
            ("--callers-top", "callers_top"),
            ("--budget", "budget"),
        ] {
            if let Ok(v) = t.get::<u64>(key) {
                args.push(flag.into());
                args.push(v.to_string());
            }
        }
    }
    run_engine_subprocess("flow", &state.root, &args)
}

pub fn engine_impact(_: &Lua, (name, opts): (String, Option<LuaTable>)) -> LuaResult<String> {
    let state = ensure_state()?;
    let mut args = vec![name];
    if let Some(t) = opts {
        for (flag, key) in [
            ("--limit", "limit"),
            ("--offset", "offset"),
            ("--hops", "hops"),
            ("--hub-guard", "hub_guard"),
        ] {
            if let Ok(v) = t.get::<u64>(key) {
                args.push(flag.into());
                args.push(v.to_string());
            }
        }
    }
    run_engine_subprocess("impact", &state.root, &args)
}

pub fn engine_read(
    lua: &Lua,
    (target, budget, filter): (String, Option<u64>, Option<String>),
) -> LuaResult<LuaTable> {
    let state = ensure_state()?;
    let level = parse_filter_level(filter);
    let total_token_budget = budget.unwrap_or(25_000);

    let path_part = target
        .rsplit_once(':')
        .filter(|(_, ln)| !ln.is_empty() && ln.chars().all(|c| c.is_ascii_digit()))
        .map(|(p, _)| p)
        .unwrap_or(&target);
    let path = Path::new(path_part);
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        state.root.join(path)
    };

    let cfg = EngineConfig {
        filter_level: level,
        total_token_budget,
        ..EngineConfig::default()
    };
    let engine = Engine::new(cfg);
    let res = engine.read(&abs);
    let tbl = lua.create_table()?;
    tbl.set("path", res.path.display().to_string())?;
    tbl.set("body", res.body)?;
    Ok(tbl)
}
