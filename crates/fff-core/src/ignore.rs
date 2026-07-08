use std::path::Path;

/// Directories excluded when walking a non-git root. Entries are `cfg`-gated
/// so a single iteration covers standard + platform-specific overrides.
pub(crate) const IGNORED_DIRS: &[&str] = &[
    "node_modules",
    "__pycache__",
    "venv",
    ".venv",
    // Rust (glob-only patterns for non_git_repo_overrides; is_non_code_directory
    // matches the "target" component separately).
    "target/debug",
    "target/release",
    "target/rust-analyzer",
    "target/criterion",
    #[cfg(target_os = "macos")]
    "Library/Application Support",
    #[cfg(target_os = "macos")]
    "Library/Caches",
    // App-group sandbox storage — used by iMessage, Photos, Notes, Calendar,
    // Electron apps, etc. for SQLite-WAL, LevelDB, protobuf files. These are
    // almost entirely extension-less binary files (~80k on a typical $HOME)
    // that never need to appear in a fuzzy or grep search.
    #[cfg(target_os = "macos")]
    "Library/Group Containers",
    #[cfg(target_os = "macos")]
    "Library/Containers",
    #[cfg(target_os = "windows")]
    "bin/Debug",
    #[cfg(target_os = "windows")]
    "bin/Release",
    #[cfg(target_os = "windows")]
    "Program Files",
    #[cfg(target_os = "windows")]
    "Program Files (x86)",
    #[cfg(target_os = "windows")]
    "AppData/Local",
    #[cfg(target_os = "windows")]
    "AppData/Roaming",
];

#[cfg(all(not(feature = "zlob"), feature = "ripgrep"))]
pub(crate) fn non_git_repo_overrides(base_path: &Path) -> Option<ignore::overrides::Override> {
    use ignore::overrides::OverrideBuilder;

    let mut builder = OverrideBuilder::new(base_path);
    for dir in IGNORED_DIRS {
        let pattern = format!("!**/{dir}/");
        if let Err(e) = builder.add(&pattern) {
            tracing::warn!("failed to add ignore pattern {pattern}: {e}");
        }
    }

    builder.build().ok()
}

pub(crate) fn is_non_code_directory(path: &Path) -> bool {
    let path_str = path.as_os_str().to_str().unwrap_or("");
    IGNORED_DIRS.iter().any(|&dir| {
        #[cfg(target_os = "windows")]
        let dir = dir.replace('/', std::path::MAIN_SEPARATOR_STR);
        #[cfg(target_os = "windows")]
        return path_str.contains(dir.as_str());

        #[cfg(not(target_os = "windows"))]
        path_str.contains(dir)
    })
}
