//! Scry C ABI surface — additive layer on top of the existing `fff_*` exports.
//!
//! Existing FFI surface (`fff_create_instance`, `fff_destroy`, `fff_search_*`,
//! `fff_grep`, `fff_multi_grep`, …) is byte-for-byte unchanged. The new
//! `fff_scry_*` exports give C-FFI consumers (Bun, Node.js, Python, Ruby, …)
//! access to the symbol index, dispatch, and token-budgeted read APIs.
//!
//! Memory model:
//! - `fff_scry_engine_new` returns an opaque `*mut FffScryEngine`. Free with
//!   `fff_scry_engine_free`.
//! - Every function returning `*mut FffScryResponse` requires the caller to
//!   free it with `fff_free_scry_response`.

use std::ffi::{CStr, CString, c_char};
use std::path::Path;
use std::sync::Arc;

use fff_engine::dispatch::DispatchResult;
use fff_engine::{Engine, EngineConfig};

#[repr(C)]
pub struct FffScryEngine {
    inner: Arc<Engine>,
    root: std::path::PathBuf,
}

#[repr(C)]
pub struct FffScryResponse {
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

fn make_response(payload: String) -> *mut FffScryResponse {
    let payload_len = payload.len();
    let cstring = match CString::new(payload) {
        Ok(c) => c,
        Err(_) => return make_error("payload contained interior NUL"),
    };
    let raw = cstring.into_raw();
    Box::into_raw(Box::new(FffScryResponse {
        code: 0,
        payload: raw,
        payload_len,
    }))
}

fn make_error(msg: &str) -> *mut FffScryResponse {
    let cstring = CString::new(msg.to_owned()).unwrap_or_else(|_| {
        CString::new("scry_ffi: malformed error message").expect("static string")
    });
    let payload_len = cstring.as_bytes().len();
    let raw = cstring.into_raw();
    Box::into_raw(Box::new(FffScryResponse {
        code: -1,
        payload: raw,
        payload_len,
    }))
}

/// Build a new scry engine and run the unified scan over `root`.
///
/// ## Safety
/// `root` must be a NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_scry_engine_new(
    root: *const c_char,
    total_token_budget: u64,
) -> *mut FffScryEngine {
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
    Box::into_raw(Box::new(FffScryEngine {
        inner: engine,
        root: root_path,
    }))
}

/// Re-run the unified scan over the engine's root, refreshing all caches.
///
/// ## Safety
/// `engine` must be a valid pointer from `fff_scry_engine_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_scry_engine_rebuild(engine: *mut FffScryEngine) -> i32 {
    if engine.is_null() {
        return -1;
    }
    let e = unsafe { &*engine };
    e.inner.handles.symbols.clear();
    e.inner.index(&e.root);
    0
}

/// Free a `FffScryEngine`.
///
/// ## Safety
/// `engine` must be a valid pointer from `fff_scry_engine_new`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_scry_engine_free(engine: *mut FffScryEngine) {
    if engine.is_null() {
        return;
    }
    drop(unsafe { Box::from_raw(engine) });
}

/// Free a `FffScryResponse`.
///
/// ## Safety
/// `response` must be a valid pointer returned by any `fff_scry_*` call that
/// returns `*mut FffScryResponse`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_free_scry_response(response: *mut FffScryResponse) {
    if response.is_null() {
        return;
    }
    let boxed = unsafe { Box::from_raw(response) };
    if !boxed.payload.is_null() {
        drop(unsafe { CString::from_raw(boxed.payload) });
    }
}

/// Dispatch a free-form query through the scry engine. Result is JSON.
///
/// ## Safety
/// `engine` must be a valid pointer; `query` must be a NUL-terminated UTF-8 string.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_scry_dispatch(
    engine: *mut FffScryEngine,
    query: *const c_char,
) -> *mut FffScryResponse {
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
/// `fff_scry_dispatch`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_scry_symbol(
    engine: *mut FffScryEngine,
    name: *const c_char,
) -> *mut FffScryResponse {
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
/// constraints as `fff_scry_dispatch`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fff_scry_read(
    engine: *mut FffScryEngine,
    path: *const c_char,
    budget: u64,
    filter: *const c_char,
) -> *mut FffScryResponse {
    if engine.is_null() {
        return make_error("engine is null");
    }
    let Some(path_arg) = (unsafe { cstr_to_str(path) }) else {
        return make_error("path is null or non-UTF-8");
    };
    let level = unsafe { cstr_to_str(filter) }
        .and_then(|s| match s {
            "none" => Some(fff_budget::FilterLevel::None),
            "aggressive" => Some(fff_budget::FilterLevel::Aggressive),
            "minimal" => Some(fff_budget::FilterLevel::Minimal),
            _ => None,
        })
        .unwrap_or(fff_budget::FilterLevel::Minimal);
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
