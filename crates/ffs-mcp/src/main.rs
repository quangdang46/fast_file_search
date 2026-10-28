//! ffs MCP Server — high-performance file finder for AI code assistants.
//!
//! Drop-in replacement for AI code assistant file search tools (Glob/Grep).
//! Provides frecency-ranked, fuzzy-matched, git-aware file finding and
//! code search via the Model Context Protocol (MCP).
//!
//! Uses `ffs-core` directly (zero FFI overhead) for all search operations.

mod cursor;
mod engine_tools;
mod healthcheck;
mod mention_tools;
mod output;
mod server;
mod update_check;

use crate::engine_tools::EngineHolder;
use clap::Parser;
use ffs::file_picker::FilePicker;
use ffs::frecency::FrecencyTracker;
use ffs::{FfsMode, SharedFilePicker, SharedFrecency};
use git2::Repository;
use mimalloc::MiMalloc;
use rmcp::{ServiceExt, transport::stdio};
use server::FfsServer;
use std::sync::Arc;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

pub const MCP_INSTRUCTIONS: &str = concat!(
    "ffs: fast, frecency-ranked file and content search.\n",
    "\n",
    "## Tools\n",
    "- ffs_grep (default): search file contents for one bare identifier.\n",
    "- ffs_find: find files/modules by name when you lack an identifier.\n",
    "- ffs_multi_grep: OR over 2+ literal identifiers in one call (faster than repeated greps).\n",
    "- ffs_symbol/ffs_callers/ffs_callees/ffs_refs: AST-level navigation when available.\n",
    "\n",
    "## Rules\n",
    "1. Search bare identifiers, not code syntax or complex regex (e.g. 'ActorAuth', not 'struct ActorAuth' or 'ctx.data::<ActorAuth>').\n",
    "2. Prefer literal text; use ffs_multi_grep for variants (['ActorAuth','actor_auth']).\n",
    "3. After 2 greps, read the top result instead of grepping variations.\n",
    "\n",
    "## Constraints\n",
    "grep: inline before query ('*.rs TODO', 'quotes/ TODO', '!test/ TODO').\n",
    "multi_grep: separate 'constraints' parameter.\n",
    "Forms: '*.rs' extension, 'src/' directory, 'schema.rs' filename, '!test/' exclude.\n",
    "Bare words are NOT constraints ('quote TODO' searches literal text). Prefer broad scopes.\n",
    "\n",
    "## Output\n",
    "ffs_grep defaults to compact 'path:line:text'. Use output_mode 'content' for definition bodies or 'files_with_matches' for paths only.\n",
    "Exclude noise with '!tests/', '!*.spec.ts', '!generated/'.",
);

/// ffs MCP Server — high-performance file finder for AI code assistants.
#[derive(Parser)]
#[command(name = "ffs-mcp", version = concat!(env!("CARGO_PKG_VERSION"), " (", env!("FFS_GIT_HASH"), ")"))]
pub(crate) struct Args {
    /// Base directory to index. Defaults to the current working directory.
    #[arg(value_name = "PATH")]
    base_path: Option<String>,

    /// Path to the frecency database.
    #[arg(long = "frecency-db")]
    frecency_db_path: Option<String>,

    /// Path to the query history database.
    #[arg(long = "history-db")]
    #[allow(dead_code)]
    history_db_path: Option<String>,

    /// Path to the log file.
    #[arg(long = "log-file")]
    log_file: Option<String>,

    /// Log level (e.g. trace, debug, info, warn, error).
    #[arg(long = "log-level")]
    log_level: Option<String>,

    /// Disable automatic update checks on startup.
    #[arg(long = "no-update-check")]
    no_update_check: bool,

    /// Disable eager mmap warmup after the initial scan. Grep results will
    /// still work (files are mmap'd lazily on first access), but the first
    /// search may be slightly slower. Useful on very large repos where the
    /// warmup would consume too many kernel resources.
    #[arg(long = "no-warmup")]
    no_warmup: bool,

    /// Disable the content index built after the initial scan.
    /// This makes ffs_grep calls slower but consumes less RAM (recommended to not turn off)
    #[arg(long = "no-content-indexing")]
    no_content_indexing: bool,

    /// Explicitly enable content indexing even when `--no-warmup` is set.
    #[arg(long = "content-indexing")]
    content_indexing: bool,

    /// Disable the background file-system watcher. Files are scanned once
    /// at startup but not monitored for changes.
    #[arg(long = "no-watch")]
    no_watch: bool,

    /// Maximum number of files whose content is kept persistently in memory.
    /// Files beyond this limit are still searchable via temporary mmaps that
    /// are released after each ffs_grep call. Defaults to 30 000.
    /// Also settable via the FFS_MAX_CACHED_FILES environment variable.
    #[arg(long = "max-cached-files", env = "FFS_MAX_CACHED_FILES")]
    max_cached_files: Option<usize>,

    /// Follow symlinks during scan and watcher walks. Off by default —
    /// enabling on cyclic symlink layouts can wedge the watcher.
    #[arg(long = "follow-symlinks")]
    follow_symlinks: bool,

    /// Allow indexing the user's home directory. FFS refuses to init in `~`
    /// unless this is set. Also settable via FFS_ENABLE_HOME_SCAN=1.
    #[arg(
        long = "enable-home-scan",
        env = "FFS_ENABLE_HOME_SCAN",
        num_args = 0..=1,
        default_missing_value = "true",
        default_value_t = false,
        value_parser = clap::builder::BoolishValueParser::new()
    )]
    enable_home_scan: bool,

    /// Allow indexing the filesystem root, off by default for the same reason.
    /// Also settable via FFS_ENABLE_ROOT_SCAN=1.
    #[arg(
        long = "enable-root-scan",
        env = "FFS_ENABLE_ROOT_SCAN",
        num_args = 0..=1,
        default_missing_value = "true",
        default_value_t = false,
        value_parser = clap::builder::BoolishValueParser::new()
    )]
    enable_root_scan: bool,

    /// Exit after this many seconds of inactivity. 0 = never exit.
    /// Defaults to 900s (15 minutes). Also settable via FFS_MCP_IDLE_TIMEOUT_SECS.
    #[arg(
        long = "idle-timeout-secs",
        env = "FFS_MCP_IDLE_TIMEOUT_SECS",
        default_value_t = 900
    )]
    idle_timeout_secs: u64,

    /// Run a health check and print diagnostic information, then exit.
    #[arg(long = "healthcheck")]
    pub(crate) healthcheck: bool,
}

/// Resolve default paths for the log file.
/// Database paths (frecency, history) must be explicitly provided via flags.
fn resolve_defaults(args: &mut Args) {
    // Ensure parent directories exist for database paths when provided
    for path in [&args.frecency_db_path, &args.history_db_path]
        .into_iter()
        .flatten()
    {
        if let Some(parent) = std::path::Path::new(path).parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            eprintln!(
                "warning: failed to create directory {}: {}",
                parent.display(),
                e
            );
        }
    }

    if args.log_file.is_none() {
        let home = dirs_home();
        let is_windows = cfg!(target_os = "windows");
        args.log_file = Some(if is_windows {
            format!("{}\\AppData\\Local\\ffs_mcp.log", home)
        } else {
            format!("{}/.cache/ffs_mcp.log", home)
        });
    }
}

fn dirs_home() -> String {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| "/tmp".to_string())
}

/// Resolve the MCP workspace root when no PATH arg was given.
///
/// Priority (Cursor launches user-level mcp.json with cwd = HOME):
/// 1. `WORKSPACE_FOLDER_PATHS` — first existing entry (Cursor / VS Code MCP)
/// 2. `VSCODE_CWD`
/// 3. `std::env::current_dir()`
///
/// Multi-root workspaces only use the first existing path.
pub(crate) fn resolve_default_base_path() -> String {
    if let Some(p) = first_existing_workspace_folder() {
        return p;
    }
    if let Ok(cwd) = std::env::var("VSCODE_CWD") {
        let trimmed = cwd.trim().trim_matches('"').trim_matches('\'');
        if !trimmed.is_empty() && std::path::Path::new(trimmed).is_dir() {
            return trimmed.to_string();
        }
    }
    std::env::current_dir()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string()
}

fn first_existing_workspace_folder() -> Option<String> {
    let raw = std::env::var("WORKSPACE_FOLDER_PATHS").ok()?;
    let paths = parse_workspace_folder_paths(&raw);
    if paths.len() > 1 {
        tracing::warn!(
            count = paths.len(),
            "WORKSPACE_FOLDER_PATHS has multiple entries; indexing the first existing one only"
        );
    }
    paths.into_iter().find(|p| std::path::Path::new(p).is_dir())
}

/// Split on `;` / newlines only — never on `:`, which is the Windows drive letter.
fn parse_workspace_folder_paths(raw: &str) -> Vec<String> {
    raw.split([';', '\n', '\r'])
        .map(|s| s.trim().trim_matches('"').trim_matches('\''))
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod workspace_path_tests {
    use super::parse_workspace_folder_paths;

    #[test]
    fn single_path() {
        let paths = parse_workspace_folder_paths("/Users/me/project");
        assert_eq!(paths, vec!["/Users/me/project"]);
    }

    #[test]
    fn windows_multi_semicolon() {
        let paths = parse_workspace_folder_paths(r"C:\a;C:\b");
        assert_eq!(paths, vec![r"C:\a", r"C:\b"]);
    }

    #[test]
    fn trims_quotes_and_empties() {
        let paths = parse_workspace_folder_paths("  \"/a\" ;  ; '/b' \n");
        assert_eq!(paths, vec!["/a", "/b"]);
    }

    #[test]
    fn never_splits_on_colon() {
        // Windows drive letters must stay intact.
        let paths = parse_workspace_folder_paths(r"C:\Users\ADMIN\project");
        assert_eq!(paths, vec![r"C:\Users\ADMIN\project"]);
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = Args::parse();
    resolve_defaults(&mut args);

    if args.healthcheck {
        return healthcheck::run_healthcheck(&args);
    }

    let log_file = args.log_file.as_deref().unwrap_or("");
    if let Err(e) = ffs::log::init_tracing(log_file, args.log_level.as_deref()) {
        eprintln!("Warning: Failed to init tracing: {}", e);
    }

    // Prefer explicit PATH arg; otherwise auto-detect Cursor/VS Code workspace
    // roots so user-level mcp.json with args:["mcp"] still indexes the project
    // (not HOME). See #77.
    let base_path = args.base_path.unwrap_or_else(resolve_default_base_path);

    let base_path = match Repository::discover(&base_path) {
        Ok(repo) => {
            if let Some(workdir) = repo.workdir() {
                let git_root = workdir.to_string_lossy().to_string();
                tracing::info!("Discovered git root: {}", git_root);
                git_root
            } else {
                tracing::info!("Git repository is bare, using base path: {}", base_path);
                base_path
            }
        }
        Err(_) => {
            tracing::info!(
                "No git repository found, indexing from base path: {}",
                base_path
            );
            base_path
        }
    };

    let shared_picker = SharedFilePicker::default();
    let shared_frecency = SharedFrecency::default();
    if let Some(frecency_db_path) = args.frecency_db_path {
        match FrecencyTracker::open(&frecency_db_path) {
            Ok(tracker) => {
                let _ = shared_frecency.init(tracker);
                let _ = shared_frecency.spawn_gc(frecency_db_path);
            }
            Err(e) => {
                eprintln!("Warning: Failed to init frecency db: {}", e);
            }
        }
    }

    // Content indexing follows warmup by default (backward compat), unless
    // the user explicitly opts in via --content-indexing or out via
    // --no-content-indexing.
    let enable_content_indexing = if args.content_indexing {
        true
    } else if args.no_content_indexing {
        false
    } else {
        !args.no_warmup
    };

    // Clone root for warmup before it's consumed by FilePickerOptions
    let root_for_warmup: std::path::PathBuf = base_path.clone().into();

    // Initialize file picker (spawns background scan + watcher)
    FilePicker::new_with_shared_state(
        shared_picker.clone(),
        shared_frecency.clone(),
        ffs::FilePickerOptions {
            base_path,
            enable_mmap_cache: !args.no_warmup,
            enable_content_indexing,
            watch: !args.no_watch,
            mode: FfsMode::Ai,
            cache_budget: args
                .max_cached_files
                .map(ffs::ContentCacheBudget::new_for_repo),
            follow_symlinks: args.follow_symlinks,
            enable_home_dir_scanning: args.enable_home_scan,
            enable_fs_root_scanning: args.enable_root_scan,
        },
    )
    .map_err(|e| format!("Failed to init file picker: {}", e))?;

    if !args.no_update_check {
        update_check::spawn_update_check();
    }

    // Create the engine holder (can be pre-warmed after scan)
    let engine_holder = Arc::new(EngineHolder::new());
    let server = FfsServer::with_engine(shared_picker.clone(), engine_holder.clone());
    let last_activity = server.last_activity();
    let idle_timeout_secs = args.idle_timeout_secs;

    // Wait for initial scan in background — don't block server startup.
    // After scan completes, pre-warm the engine for zero cold-start cost.
    let picker_clone_for_scan = shared_picker.clone();
    let engine_for_warmup = engine_holder.clone();
    let warmup_root = root_for_warmup.clone();
    const WARMUP_BUDGET: u64 = 25_000;
    tokio::task::spawn_blocking(move || {
        let start = std::time::Instant::now();
        loop {
            let is_scanning = picker_clone_for_scan
                .read()
                .ok()
                .and_then(|g| g.as_ref().map(|p| p.is_scan_active()))
                .unwrap_or(true);

            if !is_scanning {
                tracing::info!("Initial scan completed in {:?}", start.elapsed());
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        // Warmup: build the engine so the first tool call is instant
        engine_for_warmup.warmup(&warmup_root, WARMUP_BUDGET);
        tracing::info!(
            "Engine warmup complete (budget={WARMUP_BUDGET}, root={})",
            warmup_root.display(),
        );
    });

    const STARTUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
    let service = match tokio::time::timeout(STARTUP_TIMEOUT, server.serve(stdio())).await {
        Ok(res) => res.map_err(|e| format!("Failed to start MCP server: {}", e))?,
        Err(_) => {
            return Err("MCP initialize handshake did not complete within 60s".into());
        }
    };

    if idle_timeout_secs > 0 {
        last_activity.store(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            std::sync::atomic::Ordering::Relaxed,
        );

        let last_activity_for_watchdog = last_activity.clone();
        tokio::spawn(async move {
            let tick = std::time::Duration::from_secs(60);
            loop {
                tokio::time::sleep(tick).await;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let last = last_activity_for_watchdog.load(std::sync::atomic::Ordering::Relaxed);
                if now.saturating_sub(last) >= idle_timeout_secs {
                    tracing::info!(
                        "Exiting after {}s of inactivity (idle_timeout_secs={})",
                        now.saturating_sub(last),
                        idle_timeout_secs
                    );
                    std::process::exit(0);
                }
            }
        });
    }

    let picker_for_shutdown = shared_picker.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        if let Ok(mut guard) = picker_for_shutdown.write()
            && let Some(ref mut picker) = *guard
        {
            picker.stop_background_monitor();
        }
        std::process::exit(0);
    });

    service.waiting().await?;

    if let Ok(mut guard) = shared_picker.write()
        && let Some(ref mut picker) = *guard
    {
        picker.stop_background_monitor();
    }

    Ok(())
}
