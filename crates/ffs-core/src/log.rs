//! Shared logging utilities for ffs crates.
//!
//! Provides file-based tracing initialization and crash handlers (panic hook
//! + SIGSEGV signal handler) that write diagnostics to both stderr and the
//! configured log file.
//!
//! The SIGSEGV handler uses a pre-opened file descriptor instead of opening
//! the log file inside the signal handler, because open(2) is not mandated
//! async-signal-safe (it may allocate for path resolution).

use std::io;
use std::os::fd::IntoRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI32, Ordering};
use tracing_appender::non_blocking;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

static TRACING_INITIALIZED: std::sync::OnceLock<tracing_appender::non_blocking::WorkerGuard> =
    std::sync::OnceLock::new();

static CRASH_HANDLERS_INSTALLED: std::sync::OnceLock<()> = std::sync::OnceLock::new();

/// The log file path set by `init_tracing`. Crash handlers append to this file.
static LOG_FILE_PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Pre-opened log file descriptor for async-signal-safe crash writes.
static LOG_FD: AtomicI32 = AtomicI32::new(-1);

fn write_crash_report(header: &str, body: &str) {
    let msg = format!(
        "\n=== CRASH {} ===\n{}\n=== CRASH END {} ===\n",
        header, body, header
    );

    let _ = std::io::Write::write_all(&mut std::io::stderr(), msg.as_bytes());

    let fd = LOG_FD.load(Ordering::Relaxed);
    if fd >= 0 {
        unsafe {
            libc::write(fd, msg.as_ptr() as *const libc::c_void, msg.len());
        }
    }
}

/// Pre-open the log file for signal-safe crash writes. Must be called
/// *before* install_panic_hook so the fd is available in the signal handler.
fn init_log_fd(path: &Path) {
    if let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let prev = LOG_FD.swap(file.into_raw_fd(), Ordering::Relaxed);
        if prev >= 0 {
            unsafe { libc::close(prev) };
        }
    }
}

extern "C" fn sigsegv_handler(sig: libc::c_int) {
    let bt = std::backtrace::Backtrace::force_capture();
    write_crash_report("SIGSEGV", &format!("signal {}\n{}", sig, bt));

    unsafe {
        libc::signal(sig, libc::SIG_DFL);
        libc::raise(sig);
    }
}

/// Install both the panic hook and the SIGSEGV signal handler.
pub fn install_panic_hook() {
    CRASH_HANDLERS_INSTALLED.get_or_init(|| {
        let default_panic = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |panic_info| {
            let message = if let Some(s) = panic_info.payload().downcast_ref::<&str>() {
                s.to_string()
            } else if let Some(s) = panic_info.payload().downcast_ref::<String>() {
                s.clone()
            } else {
                "Unknown panic payload".to_string()
            };

            let location = panic_info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "unknown location".to_string());

            tracing::error!(
                panic.message = %message,
                panic.location = %location,
                "PANIC occurred in ffs"
            );

            write_crash_report(
                "RUST PANIC",
                &format!("Message: {}\nLocation: {}", message, location),
            );
            default_panic(panic_info);
        }));

        unsafe {
            libc::signal(
                libc::SIGSEGV,
                sigsegv_handler as *const () as libc::sighandler_t,
            );
        }
    });
}

/// Parse a log level string into a `tracing::Level`.
pub fn parse_log_level(level: Option<&str>) -> tracing::Level {
    match level.as_ref().map(|s| s.trim().to_lowercase()).as_deref() {
        Some("trace") => tracing::Level::TRACE,
        Some("debug") => tracing::Level::DEBUG,
        Some("info") => tracing::Level::INFO,
        Some("warn") => tracing::Level::WARN,
        Some("error") => tracing::Level::ERROR,
        _ => tracing::Level::INFO,
    }
}

/// Initialize tracing with a single log file.
pub fn init_tracing(log_file_path: &str, log_level: Option<&str>) -> Result<String, io::Error> {
    let log_path = Path::new(log_file_path);
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let _ = LOG_FILE_PATH.set(log_path.to_path_buf());
    init_log_fd(log_path);
    install_panic_hook();

    let file_appender = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true) // truncates a file on restart (instead of appending)
        .open(log_path)?;

    let level = parse_log_level(log_level);

    TRACING_INITIALIZED.get_or_init(|| {
        let (non_blocking_appender, guard) = non_blocking(file_appender);

        let subscriber = tracing_subscriber::registry()
            .with(
                fmt::layer()
                    .with_writer(non_blocking_appender)
                    .with_target(true)
                    .with_thread_ids(false)
                    .with_thread_names(false)
                    .with_ansi(false)
                    .with_span_events(FmtSpan::NEW | FmtSpan::CLOSE),
            )
            .with(
                EnvFilter::builder()
                    .with_default_directive(level.into())
                    .from_env_lossy(),
            );

        if let Err(e) = tracing::subscriber::set_global_default(subscriber) {
            eprintln!("Failed to set tracing subscriber: {}", e);
        } else {
            tracing::info!(
                "ffs tracing initialized with log file: {}",
                log_path.display()
            );
        }

        guard
    });

    Ok(log_file_path.to_string())
}
