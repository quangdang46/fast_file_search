use std::path::Path;

pub(crate) const NON_GIT_IGNORED_DIRS: &[&str] = &[
    "node_modules",
    "__pycache__",
    "venv",
    ".venv",
    // Rust (these are glob-only patterns for non_git_repo_overrides,
    // is_non_code_directory matches the "target" component separately)
    "target/debug",
    "target/release",
    "target/rust-analyzer",
    "target/criterion",
];

#[cfg(target_os = "macos")]
pub(crate) const PLATFORM_IGNORED_DIRS: &[&str] = &[
    "Library/Application Support",
    "Library/Caches",
    // App-group sandbox storage — used by iMessage, Photos, Notes, Calendar,
    // Electron apps, etc. for SQLite-WAL, LevelDB, protobuf files. These are
    // almost entirely extension-less binary files (~80k on a typical $HOME)
    // that never need to appear in a fuzzy or grep search.
    "Library/Group Containers",
    "Library/Containers",
];

#[cfg(target_os = "windows")]
pub(crate) const PLATFORM_IGNORED_DIRS: &[&str] = &[
    "bin/Debug",
    "bin/Release",
    "Program Files",
    "Program Files (x86)",
    "AppData/Local",
    "AppData/Roaming",
];

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
pub(crate) const PLATFORM_IGNORED_DIRS: &[&str] = &[];

pub(crate) fn non_git_repo_overrides(base_path: &Path) -> Option<ignore::overrides::Override> {
    use ignore::overrides::OverrideBuilder;

    let mut builder = OverrideBuilder::new(base_path);
    for dir in NON_GIT_IGNORED_DIRS.iter().chain(PLATFORM_IGNORED_DIRS) {
        let pattern = format!("!**/{dir}/");
        if let Err(e) = builder.add(&pattern) {
            tracing::warn!("failed to add ignore pattern {pattern}: {e}");
        }
    }

    builder.build().ok()
}

/// True when `path` contains one of the ignored directory patterns as a
/// run of whole path components. Component-wise matching (instead of the
/// old substring `contains`) avoids false positives like `myvenv/` or
/// `targetstuff/` that merely contain an ignored name as a substring.
pub(crate) fn is_non_code_directory(path: &Path) -> bool {
    let components: Vec<&str> = path
        .components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();
    NON_GIT_IGNORED_DIRS
        .iter()
        .chain(PLATFORM_IGNORED_DIRS)
        .any(|&dir| {
            let dir_components: Vec<&str> = dir.split('/').collect();
            components
                .windows(dir_components.len())
                .any(|window| window == dir_components.as_slice())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_ignored_dir_names_on_component_boundaries() {
        assert!(is_non_code_directory(Path::new(
            "/repo/node_modules/pkg/index.js"
        )));
        assert!(is_non_code_directory(Path::new(
            "/repo/src/__pycache__/x.pyc"
        )));
        assert!(is_non_code_directory(Path::new("/repo/.venv/bin/python")));
        assert!(is_non_code_directory(Path::new(
            "/repo/foo/target/debug/build.rs"
        )));
        assert!(is_non_code_directory(Path::new("target/release/app")));
    }

    #[test]
    fn rejects_substring_false_positives() {
        // Names that merely *contain* an ignored dir as a substring must
        // not be skipped — the old `path_str.contains(dir)` matched these.
        assert!(!is_non_code_directory(Path::new("/repo/myvenv/tool.rs")));
        assert!(!is_non_code_directory(Path::new("/repo/venv_extra/x.rs")));
        assert!(!is_non_code_directory(Path::new(
            "/repo/node_modules_backup/x.rs"
        )));
        assert!(!is_non_code_directory(Path::new(
            "/repo/targetstuff/debug/x.rs"
        )));
        assert!(!is_non_code_directory(Path::new("/repo/src/main.rs")));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn matches_platform_ignored_dirs() {
        assert!(is_non_code_directory(Path::new(
            "/Users/x/Library/Caches/foo"
        )));
        assert!(!is_non_code_directory(Path::new(
            "/Users/x/Library/MyCaches/foo"
        )));
    }
}
