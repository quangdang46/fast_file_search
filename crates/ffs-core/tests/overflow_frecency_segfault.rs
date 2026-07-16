use ffs_search::file_picker::{FfsMode, FilePicker};
use ffs_search::{
    FilePickerOptions, FileSearchConfig, FrecencyTracker, FuzzySearchOptions, QueryParser,
    SharedFilePicker, SharedFrecency,
};
use std::fs;
use std::path::Path;
use std::time::Duration;
use tempfile::TempDir;

// regression pinning test: if base count=0 updating frecency should not panic
// Port of upstream 0ee4ada / fff#569.
#[test]
fn update_single_file_frecency_on_overflow_file_does_not_segfault() {
    let base = TempDir::new().expect("mktemp base");
    let db = TempDir::new().expect("mktemp db");

    // EMPTY base tree. The initial scan indexes zero files, so `base_count == 0`
    // and the base path arena is an empty store whose pointer is the dangling,
    // 16-byte-aligned `0x10`. This is the exact state observed at the crash.
    let base_path = base.path().canonicalize().expect("canonicalize base");

    let shared_picker = SharedFilePicker::default();
    let shared_frecency = SharedFrecency::default();

    let tracker = FrecencyTracker::open(db.path().join("frecency.mdb")).expect("open frecency db");
    shared_frecency.init(tracker).expect("init frecency");

    FilePicker::new_with_shared_state(
        shared_picker.clone(),
        shared_frecency.clone(),
        FilePickerOptions {
            base_path: base_path.to_string_lossy().to_string(),
            enable_mmap_cache: false,
            enable_content_indexing: false,
            mode: FfsMode::Ai,
            watch: false,
            ..Default::default()
        },
    )
    .expect("FilePicker::new_with_shared_state");

    assert!(
        shared_picker.wait_for_scan(Duration::from_secs(10)),
        "initial scan did not complete"
    );

    // Create ONE new file after the (empty) scan. It is added to the overflow
    // arena (`FileItem::is_overflow()` is true).
    let new_file = base_path.join("created_after_scan.txt");
    fs::write(&new_file, "hello\n").unwrap();

    {
        let mut guard = shared_picker.write().expect("picker write lock");
        let picker: &mut FilePicker = guard.as_mut().expect("picker initialized");
        picker.handle_create_or_modify(&new_file);
    }

    // simulate watcher events
    {
        let frecency_guard = shared_frecency.read().expect("frecency read lock");
        let frecency = frecency_guard.as_ref().expect("frecency initialized");

        let mut guard = shared_picker.write().expect("picker write lock");
        let picker: &mut FilePicker = guard.as_mut().expect("picker initialized");

        let _ = frecency.track_access(new_file.as_path());
        let _ = picker.update_single_file_frecency(new_file.as_path(), frecency);
    }

    // Reaching here means the overflow file was read through the correct arena.
    let _: &Path = base_path.as_path();

    {
        let mut guard = shared_picker.write().expect("picker write lock");
        let picker: &mut FilePicker = guard.as_mut().expect("picker initialized");
        let results = picker.fuzzy_search(
            &QueryParser::new(FileSearchConfig).parse("created_after_scan *.txt"),
            None,
            FuzzySearchOptions::default(),
        );

        assert_eq!(results.total_matched, 1);
    }

    drop(shared_picker);
    drop(shared_frecency);
}
