use std::fs;

use ffs_search::file_picker::FilePicker;
use ffs_search::grep::{GrepMode, GrepSearchOptions};
use ffs_search::{AiGrepConfig, FilePickerOptions, QueryParser};
use tempfile::TempDir;

// bug pinning https://github.com/dmtrKovalenko/fff/issues/618
// Port of upstream 8e8b09f — overflow-file path constraints must not
// read chunks from the base arena (segfault / debug precondition).
#[test]
fn grep_path_constraint_on_overflow_file_does_not_segfault() {
    let tmp = TempDir::new().expect("tempdir");
    let base = tmp.path();
    let spec_dir = base.join("specs");
    fs::create_dir_all(&spec_dir).expect("mkdir specs");

    let file = spec_dir.join("annotation-plan.md");
    fs::write(&file, "dependency\n").expect("write test file");

    // Intentionally do NOT call `collect_files()`. This leaves the base path
    // arena unset/null. `handle_create_or_modify` then adds the file as an
    // overflow file whose path chunks live in the overflow arena.
    let mut picker = FilePicker::new(FilePickerOptions {
        base_path: base.to_string_lossy().to_string(),
        enable_mmap_cache: false,
        watch: false,
        ..Default::default()
    })
    .expect("create picker");

    let is_overflow = picker
        .handle_create_or_modify(&file)
        .expect("add overflow file")
        .is_overflow();
    assert!(is_overflow, "file must be added to the overflow arena");

    // Mirrors the `ffgrep({ pattern: "dependency", path: "specs/annotation-plan.md" })`
    // call that crashed: in AI grep mode the path token becomes a FilePath
    // constraint and `dependency` becomes the grep text.
    let parsed = QueryParser::new(AiGrepConfig).parse("specs/annotation-plan.md dependency");

    let opts = GrepSearchOptions {
        mode: GrepMode::PlainText,
        page_limit: 20,
        smart_case: true,
        ..Default::default()
    };

    // Debug builds typically abort with Rust's unsafe precondition check at
    // simd_path.rs. Release builds may SIGSEGV from a null arena.
    let result = picker.grep(&parsed, &opts);

    assert_eq!(result.files.len(), 1, "the overflow file should match");
    assert_eq!(result.matches.len(), 1, "`dependency` should match once");
}
