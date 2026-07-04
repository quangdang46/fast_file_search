//! Engine C ABI surface — additive layer on top of the existing `ffs_*` exports.
//!
//! Existing FFI surface (`ffs_create_instance`, `ffs_destroy`, `ffs_search_*`,
//! `ffs_grep`, `ffs_multi_grep`, …) is byte-for-byte unchanged. The new
//! `ffs_engine_*` exports give C-FFI consumers (Bun, Node.js, Python, Ruby, …)
//! access to the symbol index, dispatch, and token-budgeted read APIs.
//!
//! Memory model:
//! - `ffs_engine_new` returns an opaque `*mut FfsEngine`. Free with
//!   `ffs_engine_free`.
//! - Every function returning `*mut FfsResponse` requires the caller to
//!   free it with `ffs_engine_free_response`.

use std::ffi::{CStr, CString, c_char};
use std::path::Path;
use std::sync::Arc;

use ffs_engine::PreFilterStack;
use ffs_engine::dispatch::DispatchResult;
use ffs_engine::{Engine, EngineConfig};
use ffs_symbol::lang::detect_file_type;
use ffs_symbol::types::FileType;
use std::time::SystemTime;

#[repr(C)]
pub struct FfsEngine {
    inner: Arc<Engine>,
    root: std::path::PathBuf,
}

#[repr(C)]
pub struct FfsResponse {
    /// 0 on success; non-zero error code.
    pub code: i32,
    /// Owned UTF-8 NUL-terminated payload. Always non-null on success.
    pub payload: *mut c_char,
    /// Length of `payload` in bytes (excluding NUL).
    pub payload_len: usize,
}

unsafe fn cstr_to_str<'a>(s: *const c_char) -> Option<&'a str> {
    if s.is_null() {
        return None;
    }
    unsafe { CStr::from_ptr(s).to_str().ok() }
}

fn make_response(payload: String) -> *mut FfsResponse {
    let payload_len = payload.len();
    let cstring = match CString::new(payload) {
        Ok(c) => c,
        Err(_) => return make_error("payload contained interior NUL"),
    };
    let raw = cstring.into_raw();
    Box::into_raw(Box::new(FfsResponse {
        code: 0,
        payload: raw,
        payload_len,
    }))
}

fn make_error(msg: &str) -> *mut FfsResponse {
    let cstring = CString::new(msg.to_owned()).unwrap_or_else(|_| {
        CString::new("engine_ffi: malformed error message").expect("static string")
    });
    let payload_len = cstring.as_bytes().len();
    let raw = cstring.into_raw();
    Box::into_raw(Box::new(FfsResponse {
        code: -1,
        payload: raw,
        payload_len,
    }))
}

/// Build a new engine and run the unified scan over `root`.
///
/// ## Safety
/// `root` must be a NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_engine_new(
    root: *const c_char,
    total_token_budget: u64,
) -> *mut FfsEngine {
    let Some(root_str) = (unsafe { cstr_to_str(root) }) else {
        return std::ptr::null_mut();
    };
    let cfg = EngineConfig {
        total_token_budget: if total_token_budget == 0 {
            25_000
        } else {
            total_token_budget
        },
        ..EngineConfig::default()
    };
    let engine = Arc::new(Engine::new(cfg));
    let root_path = Path::new(root_str).to_path_buf();
    engine.index(&root_path);
    Box::into_raw(Box::new(FfsEngine {
        inner: engine,
        root: root_path,
    }))
}

/// Re-run the unified scan over the engine's root, refreshing all caches.
///
/// ## Safety
/// `engine` must be a valid pointer from `ffs_engine_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_engine_rebuild(engine: *mut FfsEngine) -> i32 {
    if engine.is_null() {
        return -1;
    }
    let e = unsafe { &*engine };
    e.inner.handles.symbols.clear();
    e.inner.index(&e.root);
    0
}

/// Free a `FfsEngine`.
///
/// ## Safety
/// `engine` must be a valid pointer from `ffs_engine_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_engine_free(engine: *mut FfsEngine) {
    if engine.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(engine) });
}

/// Free a `FfsResponse`.
///
/// ## Safety
/// `response` must be a valid pointer returned by any `ffs_engine_*` call that
/// returns `*mut FfsResponse`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_engine_free_response(response: *mut FfsResponse) {
    if response.is_null() {
        return;
    }
    let boxed = unsafe { Box::from_raw(response) };
    if !boxed.payload.is_null() {
        drop(unsafe { CString::from_raw(boxed.payload) });
    }
}

/// Dispatch a free-form query through the engine. Result is JSON.
///
/// ## Safety
/// `engine` must be a valid pointer; `query` must be a NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_engine_dispatch(
    engine: *mut FfsEngine,
    query: *const c_char,
) -> *mut FfsResponse {
    if engine.is_null() {
        return make_error("engine is null");
    }
    let Some(q) = (unsafe { cstr_to_str(query) }) else {
        return make_error("query is null or non-UTF-8");
    };
    let e = unsafe { &*engine };
    let result = e.inner.dispatch(q, &e.root);
    let json = match result {
        DispatchResult::Symbol { hits, classified } => serde_json::json!({
            "kind": "symbol",
            "raw": classified.raw,
            "hits": hits.iter().map(|h| serde_json::json!({
                "path": h.path.display().to_string(),
                "line": h.line,
                "kind": h.kind,
            })).collect::<Vec<_>>(),
        }),
        DispatchResult::SymbolGlob { hits, classified } => serde_json::json!({
            "kind": "symbol_glob",
            "raw": classified.raw,
            "hits": hits.iter().map(|(name, h)| serde_json::json!({
                "name": name,
                "path": h.path.display().to_string(),
                "line": h.line,
            })).collect::<Vec<_>>(),
        }),
        DispatchResult::Glob {
            classified,
            pattern,
        } => serde_json::json!({
            "kind": "glob",
            "raw": classified.raw,
            "pattern": pattern,
        }),
        DispatchResult::FilePath { classified, path } => serde_json::json!({
            "kind": "file_path",
            "raw": classified.raw,
            "path": path.display().to_string(),
        }),
        DispatchResult::ContentFallback { classified } => serde_json::json!({
            "kind": "content_fallback",
            "raw": classified.raw,
        }),
    };
    make_response(json.to_string())
}

/// Look up a symbol by exact name or by prefix (suffix `*`). Result is JSON.
///
/// ## Safety
/// `engine` and `name` must satisfy the same constraints as
/// `ffs_engine_dispatch`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_engine_symbol(
    engine: *mut FfsEngine,
    name: *const c_char,
) -> *mut FfsResponse {
    if engine.is_null() {
        return make_error("engine is null");
    }
    let Some(name) = (unsafe { cstr_to_str(name) }) else {
        return make_error("name is null or non-UTF-8");
    };
    let e = unsafe { &*engine };
    let json = if let Some(prefix) = name.strip_suffix('*') {
        let hits = e.inner.handles.symbols.lookup_prefix(prefix);
        serde_json::json!({
            "mode": "prefix",
            "hits": hits.iter().map(|(s, h)| serde_json::json!({
                "name": s,
                "path": h.path.display().to_string(),
                "line": h.line,
                "kind": h.kind,
            })).collect::<Vec<_>>(),
        })
    } else {
        let hits = e.inner.handles.symbols.lookup_exact(name);
        serde_json::json!({
            "mode": "exact",
            "name": name,
            "hits": hits.iter().map(|h| serde_json::json!({
                "path": h.path.display().to_string(),
                "line": h.line,
                "kind": h.kind,
            })).collect::<Vec<_>>(),
        })
    };
    make_response(json.to_string())
}

/// Read a file with token-budget aware truncation. Result is JSON
/// `{ path, body }`.
///
/// ## Safety
/// `engine`, `path`, and `filter` (when non-null) must satisfy the same
/// constraints as `ffs_engine_dispatch`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_engine_read(
    engine: *mut FfsEngine,
    path: *const c_char,
    budget: u64,
    filter: *const c_char,
) -> *mut FfsResponse {
    if engine.is_null() {
        return make_error("engine is null");
    }
    let Some(path_arg) = (unsafe { cstr_to_str(path) }) else {
        return make_error("path is null or non-UTF-8");
    };
    let level = unsafe { cstr_to_str(filter) }
        .and_then(|s| match s {
            "none" => Some(ffs_budget::FilterLevel::None),
            "aggressive" => Some(ffs_budget::FilterLevel::Aggressive),
            "minimal" => Some(ffs_budget::FilterLevel::Minimal),
            _ => None,
        })
        .unwrap_or(ffs_budget::FilterLevel::Minimal);
    let total_token_budget = if budget == 0 { 25_000 } else { budget };

    let e = unsafe { &*engine };
    let path_part = path_arg
        .rsplit_once(':')
        .filter(|(_, ln)| !ln.is_empty() && ln.chars().all(|c| c.is_ascii_digit()))
        .map(|(p, _)| p)
        .unwrap_or(path_arg);
    let candidate = Path::new(path_part);
    let abs = if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        e.root.join(candidate)
    };

    let cfg = EngineConfig {
        filter_level: level,
        total_token_budget,
        ..EngineConfig::default()
    };
    let local = Engine::new(cfg);
    let res = local.read(&abs);
    let json = serde_json::json!({
        "path": res.path.display().to_string(),
        "body": res.body,
    });
    make_response(json.to_string())
}

/// Run `ffs refs <name>` against the engine's root. JSON payload follows the
/// CLI's `RefsOutput` shape (`definitions[]`, `usages[]`, pagination).
///
/// ## Safety
/// `engine` must be a valid pointer; `name` must be a NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_engine_refs(
    engine: *mut FfsEngine,
    name: *const c_char,
    limit: u64,
    offset: u64,
) -> *mut FfsResponse {
    if engine.is_null() {
        return make_error("engine is null");
    }
    let Some(n) = (unsafe { cstr_to_str(name) }) else {
        return make_error("name is null or non-UTF-8");
    };
    let e = unsafe { &*engine };
    let limit = if limit == 0 { 100 } else { limit as usize };
    let offset = offset as usize;

    // 1. Definitions from symbol index
    let definitions = e.inner.handles.symbols.lookup_exact(n);
    let definition_line_set: std::collections::HashSet<(String, u32)> = definitions
        .iter()
        .map(|d| (d.path.to_string_lossy().to_string(), d.line))
        .collect();

    // 2. Walk code files
    let mut candidates: Vec<(std::path::PathBuf, SystemTime, String)> = Vec::new();
    for entry in ignore::WalkBuilder::new(&e.root)
        .standard_filters(true)
        .follow_links(false)
        .build()
        .flatten()
    {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.into_path();
        if !matches!(detect_file_type(&path), FileType::Code(_)) {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let Ok(content) = ffs::bom::read_file(&path) else {
            continue;
        };
        candidates.push((path, mtime, content));
    }

    // 3. Bloom filter confirmation
    let stack = PreFilterStack::new(e.inner.handles.bloom.clone());
    let confirm_input: Vec<_> = candidates
        .iter()
        .map(|(p, m, c)| (p.clone(), *m, c.clone()))
        .collect();
    let survivors = stack.confirm_symbol(&confirm_input, n);
    let survivor_set: std::collections::HashSet<&std::path::Path> =
        survivors.iter().map(|s| s.path.as_path()).collect();

    // 4. Text scan for usages
    let mut usages: Vec<serde_json::Value> = Vec::new();
    for (path, _mtime, content) in &candidates {
        if !survivor_set.contains(path.as_path()) {
            continue;
        }
        let path_str = path.to_string_lossy().to_string();
        for (lineno, line) in content.lines().enumerate() {
            let lineno = (lineno + 1) as u32;
            if !line.contains(n) {
                continue;
            }
            if definition_line_set.contains(&(path_str.clone(), lineno)) {
                continue;
            }
            usages.push(serde_json::json!({
                "path": path_str,
                "line": lineno,
                "text": line,
            }));
        }
    }

    // 5. Pagination
    let total_usages = usages.len();
    let has_more = offset + limit < total_usages;
    let page: Vec<_> = if offset < total_usages {
        usages.drain(offset..).collect()
    } else {
        Vec::new()
    };
    let page = &page[..page.len().min(limit)];

    // 6. JSON output matching CLI format
    let defs_json: Vec<serde_json::Value> = definitions
        .iter()
        .map(|d| {
            serde_json::json!({
                "path": d.path.to_string_lossy(),
                "line": d.line,
                "end_line": d.end_line,
                "kind": d.kind,
                "weight": d.weight,
            })
        })
        .collect();

    let payload = serde_json::json!({
        "name": n,
        "definitions": defs_json,
        "usages": page,
        "total_usages": total_usages,
        "offset": offset,
        "has_more": has_more,
    });

    make_response(payload.to_string())
}

/// Run `ffs flow <name>` against the engine's root. JSON payload follows the
/// CLI's `FlowOutput` shape (one card per definition).
///
/// ## Safety
/// `engine` must be a valid pointer; `name` must be a NUL-terminated UTF-8 string.
pub unsafe extern "C" fn ffs_engine_flow(
    engine: *mut FfsEngine,
    name: *const c_char,
    limit: u64,
    offset: u64,
    callees_top: u64,
    callers_top: u64,
    _budget: u64,
) -> *mut FfsResponse {
    if engine.is_null() {
        return make_error("engine is null");
    }
    let Some(n) = (unsafe { cstr_to_str(name) }) else {
        return make_error("name is null or non-UTF-8");
    };
    let e = unsafe { &*engine };
    let limit = if limit == 0 { 10 } else { limit as usize };
    let offset = offset as usize;
    let _callees_top = if callees_top == 0 {
        5
    } else {
        callees_top as usize
    };
    let _callers_top = if callers_top == 0 {
        5
    } else {
        callers_top as usize
    };

    let definitions = e.inner.handles.symbols.lookup_exact(n);
    let total = definitions.len();
    let page: Vec<_> = definitions.into_iter().skip(offset).take(limit).collect();

    let mut cards: Vec<serde_json::Value> = Vec::new();
    for def in &page {
        let body_text = ffs::bom::read_file(&def.path).unwrap_or_default();
        let body_lines: Vec<&str> = body_text.lines().collect();
        let start = (def.line.saturating_sub(1) as usize).min(body_lines.len());
        let end = (def.end_line as usize).min(body_lines.len());
        let body_snippet: Vec<&str> = body_lines[start..end].to_vec();

        cards.push(serde_json::json!({
            "def": {
                "path": def.path.to_string_lossy(),
                "line": def.line,
                "end_line": def.end_line,
                "kind": def.kind,
                "weight": def.weight,
            },
            "body": body_snippet.join("\n"),
            "body_start_line": def.line,
            "body_end_line": def.end_line,
        }));
    }

    let payload = serde_json::json!({
        "name": n,
        "cards": cards,
        "total_cards": total,
        "offset": offset,
        "has_more": offset + page.len() < total,
    });
    make_response(payload.to_string())
}

/// Run `ffs impact <name>` against the engine's root. JSON payload follows
/// the CLI's `ImpactOutput` shape (ranked `results[]`).
///
/// ## Safety
/// `engine` must be a valid pointer; `name` must be a NUL-terminated UTF-8 string.
pub unsafe extern "C" fn ffs_engine_impact(
    engine: *mut FfsEngine,
    name: *const c_char,
    limit: u64,
    offset: u64,
    _hops: u32,
    _hub_guard: u64,
) -> *mut FfsResponse {
    if engine.is_null() {
        return make_error("engine is null");
    }
    let Some(n) = (unsafe { cstr_to_str(name) }) else {
        return make_error("name is null or non-UTF-8");
    };
    let e = unsafe { &*engine };
    let limit = if limit == 0 { 20 } else { limit as usize };
    let offset = offset as usize;

    // Walk code files to find callers via bloom + contains
    let stack = PreFilterStack::new(e.inner.handles.bloom.clone());
    let mut candidates: Vec<(std::path::PathBuf, String)> = Vec::new();
    for entry in ignore::WalkBuilder::new(&e.root)
        .standard_filters(true)
        .follow_links(false)
        .build()
        .flatten()
    {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.into_path();
        if !matches!(detect_file_type(&path), FileType::Code(_)) {
            continue;
        }
        let Ok(content) = ffs::bom::read_file(&path) else {
            continue;
        };
        candidates.push((path, content));
    }
    let confirm_input: Vec<_> = candidates
        .iter()
        .map(|(p, c)| (p.clone(), SystemTime::UNIX_EPOCH, c.clone()))
        .collect();
    let survivors = stack.confirm_symbol(&confirm_input, n);
    let survivor_set: std::collections::HashSet<&std::path::Path> =
        survivors.iter().map(|s| s.path.as_path()).collect();

    let mut results: Vec<serde_json::Value> = Vec::new();
    let definitions = e.inner.handles.symbols.lookup_exact(n);
    let def_line_set: std::collections::HashSet<(String, u32)> = definitions
        .iter()
        .map(|d| (d.path.to_string_lossy().to_string(), d.line))
        .collect();

    for (path, content) in &candidates {
        if !survivor_set.contains(path.as_path()) {
            continue;
        }
        let path_str = path.to_string_lossy().to_string();
        let mut direct = 0u32;
        for (lineno, line) in content.lines().enumerate() {
            let lineno = (lineno + 1) as u32;
            if !line.contains(n) {
                continue;
            }
            if def_line_set.contains(&(path_str.clone(), lineno)) {
                continue;
            }
            direct += 1;
        }
        if direct > 0 {
            results.push(serde_json::json!({
                "path": path_str,
                "score": direct * 3,
                "direct": direct,
            }));
        }
    }

    results.sort_by(|a, b| {
        b["score"]
            .as_u64()
            .cmp(&a["score"].as_u64())
            .then_with(|| a["path"].as_str().cmp(&b["path"].as_str()))
    });
    let total = results.len();
    let page: Vec<_> = results.into_iter().skip(offset).take(limit).collect();

    let payload = serde_json::json!({
        "name": n,
        "results": page,
        "total": total,
        "offset": offset,
        "has_more": offset + page.len() < total,
    });
    make_response(payload.to_string())
}
