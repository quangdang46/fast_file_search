//! Scry Lua bindings — additive layer on top of the existing fff-nvim API.
//!
//! Existing exports (`init_db`, `init_file_picker`, `scan_files`,
//! `fuzzy_search_files`, `live_grep`, etc.) are byte-for-byte unchanged.
//! These bindings expose the new `fff-engine` (symbol index, dispatch,
//! token-budgeted read) under the `scry_*` namespace.
//!
//! Lifecycle:
//!   - `scry_init(root)` builds the engine and runs the initial unified scan.
//!   - `scry_dispatch / scry_symbol / scry_grep / scry_read` query the warm
//!     caches.
//!   - `scry_rebuild()` re-runs the unified scan against the same root.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use mlua::prelude::*;
use once_cell::sync::Lazy;
use parking_lot::RwLock;

use fff_budget::FilterLevel;
use fff_engine::dispatch::DispatchResult;
use fff_engine::{Engine, EngineConfig};

struct ScryState {
    engine: Arc<Engine>,
    root: PathBuf,
}

static SCRY_STATE: Lazy<RwLock<Option<ScryState>>> = Lazy::new(|| RwLock::new(None));

fn parse_filter_level(raw: Option<String>) -> FilterLevel {
    match raw.as_deref() {
        Some("none") => FilterLevel::None,
        Some("aggressive") => FilterLevel::Aggressive,
        _ => FilterLevel::Minimal,
    }
}

fn ensure_state() -> Result<ScryState, mlua::Error> {
    SCRY_STATE
        .read()
        .as_ref()
        .map(|s| ScryState {
            engine: s.engine.clone(),
            root: s.root.clone(),
        })
        .ok_or_else(|| mlua::Error::RuntimeError("scry_init() not called yet".to_string()))
}

pub fn scry_init(_: &Lua, (root, opts): (String, Option<LuaTable>)) -> LuaResult<bool> {
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
    *SCRY_STATE.write() = Some(ScryState {
        engine,
        root: root_path,
    });
    Ok(true)
}

pub fn scry_rebuild(_: &Lua, _: ()) -> LuaResult<bool> {
    let state = ensure_state()?;
    state.engine.handles.symbols.clear();
    state.engine.index(&state.root);
    Ok(true)
}

pub fn scry_dispatch(lua: &Lua, query: String) -> LuaResult<LuaTable> {
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

pub fn scry_symbol(lua: &Lua, name: String) -> LuaResult<LuaTable> {
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

pub fn scry_grep(lua: &Lua, pattern: String) -> LuaResult<LuaTable> {
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

pub fn scry_read(
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
