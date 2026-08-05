//! Regression test for the Windows stress-test CI failure:
//!
//! `git commit` rewrites `.git/index` in place. The watcher reacts to that
//! one-shot Modify event by calling `refresh_git_status`, which reads the
//! index via libgit2. If the event fires *while* `git commit` still holds
//! `.git/index.lock` (index half-written), a naive read snapshots stale
//! status — e.g. `INDEX_NEW` for files the commit just cleared — and there
//! is no second event to fix it.
//!
//! The fix: `refresh_git_status` re-reads while the index lock is present
//! (bounded retry) so it never commits a status read against a half-written
//! index. This test holds `.git/index.lock` for longer than the legacy
//! 500 ms give-up window and asserts that `refresh_git_status` *actually
//! waited* for the lock to clear (its contract) rather than reading early.

use ffs_search::file_picker::FilePicker;
use ffs_search::{FilePickerOptions, SharedFilePicker, SharedFrecency};
use git2::Status;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use tempfile::TempDir;

/// How long we hold `.git/index.lock`. Deliberately longer than the legacy
/// 500 ms give-up so the pre-fix code returns early (and reads a half-written
/// index), while the fixed code waits for the lock to clear.
const LOCK_HOLD_MS: u64 = 1_200;

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .envs([
            ("GIT_AUTHOR_NAME", "test"),
            ("GIT_AUTHOR_EMAIL", "test@test.test"),
            ("GIT_COMMITTER_NAME", "test"),
            ("GIT_COMMITTER_EMAIL", "test@test.test"),
        ])
        .output()
        .unwrap_or_else(|e| panic!("git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Return the picker's recorded git_status for `relative` (forward slashes),
/// or `None` if the file isn't in the picker.
fn picker_status(shared: &SharedFilePicker, relative: &str) -> Option<Status> {
    let guard = shared.read().ok()?;
    let picker = guard.as_ref()?;
    let parser = ffs_search::QueryParser::default();
    let parsed = parser.parse(relative);
    let result = picker.fuzzy_search(
        &parsed,
        None,
        ffs_search::FuzzySearchOptions {
            max_threads: 1,
            pagination: ffs_search::PaginationArgs {
                offset: 0,
                limit: 10,
            },
            ..Default::default()
        },
    );
    result
        .items
        .iter()
        .find(|f| f.relative_path(picker).replace('\\', "/") == relative)
        .and_then(|f| f.git_status)
}

#[test]
fn refresh_git_status_waits_for_index_lock_release() {
    let tmp = TempDir::new().unwrap();
    let base = tmp.path().canonicalize().unwrap();

    // Seed repo with one tracked file.
    std::fs::write(base.join("a.rs"), "fn a() {}\n").unwrap();
    git(&base, &["init", "-b", "main"]);
    git(&base, &["config", "user.email", "test@test.test"]);
    git(&base, &["config", "user.name", "test"]);
    git(&base, &["add", "-A"]);
    git(&base, &["commit", "-m", "seed", "--no-gpg-sign"]);

    let (shared, _frecency) = {
        let shared = SharedFilePicker::default();
        let frecency = SharedFrecency::noop();
        FilePicker::new_with_shared_state(
            shared.clone(),
            frecency.clone(),
            FilePickerOptions {
                base_path: base.to_string_lossy().to_string(),
                watch: true,
                ..Default::default()
            },
        )
        .expect("picker");
        (shared, frecency)
    };
    assert!(shared.wait_for_scan(Duration::from_secs(30)));
    assert!(shared.wait_for_watcher(Duration::from_secs(30)));

    // Stage a change and commit it so the post-commit truth is "CURRENT".
    std::fs::write(base.join("a.rs"), "fn a() { /* edited */ }\n").unwrap();
    git(&base, &["add", "-A"]);
    git(&base, &["commit", "-m", "second", "--no-gpg-sign"]);

    // Hold `.git/index.lock` longer than the legacy 500 ms give-up window
    // while a background thread calls `refresh_git_status`.
    let lock = base.join(".git/index.lock");
    std::fs::write(&lock, b"simulated in-flight git write").unwrap();

    let shared2 = shared.clone();
    let frec2 = SharedFrecency::noop();
    let handle = std::thread::spawn(move || {
        let started = std::time::Instant::now();
        let n = shared2.refresh_git_status(&frec2).unwrap_or(0);
        (started.elapsed(), n)
    });

    std::thread::sleep(Duration::from_millis(LOCK_HOLD_MS));
    std::fs::remove_file(&lock).unwrap();

    let (elapsed, _count) = handle.join().expect("refresh thread panicked");

    // The fix's contract: refresh must not read the index while the lock is
    // held — it waits for the lock to clear. The legacy code gave up after
    // 500 ms and read a half-written index.
    assert!(
        elapsed >= Duration::from_millis(LOCK_HOLD_MS),
        "refresh_git_status returned before the lock cleared (elapsed={elapsed:?}, \
         held lock for {LOCK_HOLD_MS} ms). A refresh that reads while the index \
         is locked can snapshot a half-written index, leaving stale git_status."
    );

    // Sanity: the picker still converges to CURRENT (clean) for the file.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut last = None;
    while std::time::Instant::now() < deadline {
        last = picker_status(&shared, "a.rs");
        if last == Some(Status::CURRENT) || last.is_none() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    panic!("a.rs did not converge to CURRENT after index.lock release; last status = {last:?}");
}
