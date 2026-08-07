use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard, Weak};
use std::time::{Duration, Instant};

use crate::error::Error;
use crate::file_picker::FilePicker;
use crate::frecency::FrecencyTracker;
use crate::git::GitStatusCache;
use crate::query_tracker::QueryTracker;
use crate::scan::ScanJob;

/// Maximum time to retry reading git status while `.git/index.lock` is
/// held. This covers the worst case: a slow `git add -A` + `git commit`
/// in a repo with many files. 5 s is generous — the lock typically clears
/// in milliseconds — but keeps the watcher thread responsive if something
/// unusual holds the lock for a while.
const GIT_STATUS_RETRY_WINDOW: Duration = Duration::from_secs(5);

/// Poll interval between retries while the index lock is held.
const GIT_STATUS_RETRY_POLL: Duration = Duration::from_millis(50);

/// Poll `.git/index.lock` until it disappears (git write completed), giving up
/// after [`GIT_LOCK_MAX_WAIT`]. Returns `true` if the lock cleared (or was
/// never present) and `false` if it was still held when the wait gave up.
///
/// Used by [`SharedPicker::refresh_git_status`] to avoid reading a
/// half-updated index when the watcher fires mid-`git add` / `git commit`.
///
/// The wait is bounded and cheap: the lock file is typically cleared within
/// a few milliseconds of the git command exiting. Callers that see `false`
/// should re-read git status on a later pass (the `.git/index` Modify event
/// is one-shot) rather than proceed against a half-written index.
pub(crate) fn wait_for_git_index_lock_release(git_root: &Path) -> bool {
    const GIT_LOCK_POLL: Duration = Duration::from_millis(10);
    const GIT_LOCK_MAX_WAIT: Duration = Duration::from_millis(500);

    let lock = git_root.join(".git").join("index.lock");
    // Fast path: no lock present.
    if !lock.exists() {
        return true;
    }
    let deadline = Instant::now() + GIT_LOCK_MAX_WAIT;
    while lock.exists() && Instant::now() < deadline {
        std::thread::sleep(GIT_LOCK_POLL);
    }
    if lock.exists() {
        tracing::warn!(
            "Git status refresh gave up waiting on lingering \
             .git/index.lock at {} — the index may be half-written",
            lock.display()
        );
        false
    } else {
        true
    }
}

/// Thread-safe shared handle to the [`FilePicker`] instance.
/// This accumulates only asynchronous non-blocking operations against the
/// file picker: creating, triggering various rescans and so on.
///
/// For blocking access use internal picker via `.read()` or `.write()`
///
/// ```ignore
/// let shared_picker = SharedFilePicker::default();
///
/// if let Some(picker) = shared_picker.read()?.as_ref() {
///     let files = picker.fuzzy_search(&query, options);
///     println!("Found {} files", files.len());
/// } else {
///     println!("Picker not initialized");
/// }
/// ```
#[derive(Clone, Default)]
pub struct SharedFilePicker(pub(crate) Arc<SharedPickerInner>);

pub struct SharedPickerInner {
    picker: parking_lot::RwLock<Option<FilePicker>>,
}

impl Default for SharedPickerInner {
    fn default() -> Self {
        Self {
            picker: parking_lot::RwLock::new(None),
        }
    }
}

/// Non-owning handle to a [`SharedPicker`].
#[derive(Clone)]
pub(crate) struct WeakFilePicker(Weak<SharedPickerInner>);

impl WeakFilePicker {
    /// Try to promote the weak handle back to a strong [`SharedPicker`].
    ///
    /// Returns `None` once every strong `SharedPicker` clone has been
    /// dropped. Callers should treat that as "the picker is being
    /// torn down" and exit their current iteration cleanly.
    pub(crate) fn upgrade(&self) -> Option<SharedFilePicker> {
        self.0.upgrade().map(SharedFilePicker)
    }
}

impl std::fmt::Debug for SharedFilePicker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SharedPicker").field(&"..").finish()
    }
}

impl SharedFilePicker {
    pub fn read(&self) -> Result<parking_lot::RwLockReadGuard<'_, Option<FilePicker>>, Error> {
        Ok(self.0.picker.read())
    }

    pub fn write(&self) -> Result<parking_lot::RwLockWriteGuard<'_, Option<FilePicker>>, Error> {
        Ok(self.0.picker.write())
    }

    /// Produce a non-owning handle to the same inner picker.
    /// Use it if you don't need to block internal threads from dropping while owning this ref
    pub(crate) fn weaken(&self) -> WeakFilePicker {
        WeakFilePicker(Arc::downgrade(&self.0))
    }

    /// Return `true` if this is an instance of the picker that requires a complicated post-scan
    /// indexing/cache warmup job. The indexing is not crazy but it takes time.
    pub fn need_complex_rebuild(&self) -> bool {
        let guard = self.0.picker.read();
        guard
            .as_ref()
            .is_some_and(|p| p.has_mmap_cache() || p.has_content_indexing())
    }

    /// Block until the background filesystem scan finishes.
    /// Returns `true` if scan completed, `false` on timeout.
    pub fn wait_for_scan(&self, timeout: Duration) -> bool {
        let signal = {
            let guard = self.0.picker.read();
            match &*guard {
                Some(picker) => Arc::clone(&picker.signals.scanning),
                None => return true,
            }
        };

        let start = std::time::Instant::now();
        while signal.load(std::sync::atomic::Ordering::Acquire) {
            if start.elapsed() >= timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        true
    }

    /// Block until the background file watcher is ready.
    /// Returns `true` if watcher ready, `false` on timeout.
    pub fn wait_for_watcher(&self, timeout: Duration) -> bool {
        let watch_ready_signal = {
            let guard = self.0.picker.read();
            match &*guard {
                Some(picker) => Arc::clone(&picker.signals.watcher_ready),
                None => return true,
            }
        };

        let start = std::time::Instant::now();
        while !watch_ready_signal.load(std::sync::atomic::Ordering::Acquire) {
            if start.elapsed() >= timeout {
                return false;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        true
    }

    /// Trigger a full filesystem rescan without blocking the caller.
    /// Performs a safe async rescan. Guarantees only single active rescan per picker.
    /// If many rescans requested the last one guaranteed to be finished.
    pub fn trigger_full_rescan_async(&self, shared_frecency: &SharedFrecency) -> Result<(), Error> {
        match ScanJob::new_rescan(self, shared_frecency)? {
            Some(job) => {
                job.spawn();
            }
            None => {
                // we can not abort the ongoing sync, but if the events
                if let Ok(guard) = self.read()
                    && let Some(picker) = guard.as_ref()
                {
                    picker
                        .scan_signals()
                        .rescan_pending
                        .store(true, std::sync::atomic::Ordering::Release);
                    tracing::info!(
                        "Full rescan requested while another scan is active — \
                         deferred via rescan_pending flag"
                    );
                }
            }
        }
        Ok(())
    }

    /// Refresh git statuses for all indexed files.
    pub fn refresh_git_status(&self, shared_frecency: &SharedFrecency) -> Result<usize, Error> {
        use tracing::debug;

        let git_status = {
            let guard = self.read()?;
            let Some(ref picker) = *guard else {
                return Err(Error::FilePickerMissing);
            };

            let git_root = picker.git_root().map(|p| p.to_path_buf());
            drop(guard); // updating git status could take very long time, there is not risky as we
            // do not allow any mutations and deletions of files from the sync

            debug!(?git_root, "Refreshing git status for picker");

            // Re-read while `.git/index.lock` is being written. The FS event
            // for `.git/index` is one-shot: if we read a half-written index
            // during a `git add` / `git commit`, there is no second event to
            // nudge us, and the picker would keep a stale status (e.g.
            // `INDEX_NEW` for files a commit just cleaned). Bound the retry
            // to a generous deadline; the poll is cheap and converges quickly.
            //
            // A `None` read while the lock is still held is the transient
            // mid-write race — libgit2 partial-read of `.git/index` (e.g.
            // "could not read (expected 172 bytes, read 167)"). That must not
            // stop the loop, or the picker keeps its stale status for ever.
            // Only stop when the index is stable (lock cleared) and a read
            // succeeded, or once the deadline passes.
            let git_root_ref = git_root.as_deref();
            let deadline = Instant::now() + GIT_STATUS_RETRY_WINDOW;
            let mut last_good: Option<GitStatusCache> = None;
            let git_status = loop {
                let lock_clear = git_root_ref
                    .map(wait_for_git_index_lock_release)
                    .unwrap_or(true);
                let status = GitStatusCache::read_git_status(
                    git_root_ref,
                    &mut crate::git::default_status_options(),
                );
                match (status, lock_clear, git_root_ref.is_none()) {
                    // Authoritative read: lock cleared and we got a value.
                    (Some(s), true, false) => break Some(s),
                    // No repo at all — reading is futile, don't spin.
                    (_, _, true) => break None,
                    // Lock still held (or raced just after release): keep the
                    // last good read, keep polling.
                    (Some(s), _, false) => last_good = Some(s),
                    (None, _, false) => {}
                }
                if Instant::now() >= deadline {
                    break None;
                }
                debug!("Index lock still held or status read raced, retrying");
                std::thread::sleep(GIT_STATUS_RETRY_POLL);
            };
            // Past deadline with a good read captured earlier — prefer it over
            // nothing so a stuck lock can't leave the picker stale.
            git_status.or(last_good)
        };

        let mut guard = self.write()?;
        let picker = guard.as_mut().ok_or(Error::FilePickerMissing)?;

        let statuses_count = if let Some(git_status) = git_status {
            let count = git_status.statuses_len();
            picker.update_git_statuses(git_status, shared_frecency)?;
            count
        } else {
            0
        };

        Ok(statuses_count)
    }
}

/// Thread-safe shared handle to the [`FrecencyTracker`] instance.
#[derive(Clone)]
pub struct SharedFrecency {
    inner: Arc<RwLock<Option<FrecencyTracker>>>,
    enabled: bool,
}

impl Default for SharedFrecency {
    fn default() -> Self {
        Self {
            inner: Arc::new(RwLock::new(None)),
            enabled: true,
        }
    }
}

impl std::fmt::Debug for SharedFrecency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SharedFrecency").field(&"..").finish()
    }
}

impl SharedFrecency {
    /// Creates a disabled instance that silently ignores all writes.
    pub fn noop() -> Self {
        Self {
            inner: Arc::new(RwLock::new(None)),
            enabled: false,
        }
    }

    pub fn read(&self) -> Result<RwLockReadGuard<'_, Option<FrecencyTracker>>, Error> {
        self.inner.read().map_err(|_| Error::AcquireFrecencyLock)
    }

    pub fn write(&self) -> Result<RwLockWriteGuard<'_, Option<FrecencyTracker>>, Error> {
        self.inner.write().map_err(|_| Error::AcquireFrecencyLock)
    }

    /// Initialize the frecency tracker. No-op if this is a disabled instance.
    pub fn init(&self, tracker: FrecencyTracker) -> Result<(), Error> {
        if !self.enabled {
            return Ok(());
        }
        let mut guard = self.write()?;
        *guard = Some(tracker);
        Ok(())
    }

    /// Spawn a background GC thread for this frecency tracker.
    pub fn spawn_gc(&self, db_path: String) -> crate::Result<std::thread::JoinHandle<()>> {
        FrecencyTracker::spawn_gc(self.clone(), db_path)
    }

    /// Drop the in-memory tracker and delete the on-disk database directory.
    ///
    /// Acquires the write lock, ensuring all readers (including any active mmap
    /// access) are finished before the LMDB environment is closed and the files
    /// are removed.
    ///
    /// Returns `Ok(Some(path))` with the deleted path, or `Ok(None)` if no
    /// tracker was initialized.
    pub fn destroy(&self) -> Result<Option<PathBuf>, Error> {
        let mut guard = self.write()?;
        let Some(tracker) = guard.take() else {
            return Ok(None);
        };
        let db_path = tracker.db_path().to_path_buf();
        // Drop closes the LMDB env and unmaps the files
        drop(tracker);
        drop(guard);
        std::fs::remove_dir_all(&db_path).map_err(|source| Error::RemoveDbDir {
            path: db_path.clone(),
            source,
        })?;
        Ok(Some(db_path))
    }
}

/// Thread-safe shared handle to the [`QueryTracker`] instance.
#[derive(Clone)]
pub struct SharedQueryTracker {
    inner: Arc<RwLock<Option<QueryTracker>>>,
    enabled: bool,
}

impl Default for SharedQueryTracker {
    fn default() -> Self {
        Self {
            inner: Arc::new(RwLock::new(None)),
            enabled: true,
        }
    }
}

impl std::fmt::Debug for SharedQueryTracker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SharedQueryTracker").field(&"..").finish()
    }
}

impl SharedQueryTracker {
    /// Creates a disabled instance that silently ignores all writes.
    pub fn noop() -> Self {
        Self {
            inner: Arc::new(RwLock::new(None)),
            enabled: false,
        }
    }

    pub fn read(&self) -> Result<RwLockReadGuard<'_, Option<QueryTracker>>, Error> {
        self.inner.read().map_err(|_| Error::AcquireFrecencyLock)
    }

    pub fn write(&self) -> Result<RwLockWriteGuard<'_, Option<QueryTracker>>, Error> {
        self.inner.write().map_err(|_| Error::AcquireFrecencyLock)
    }

    /// Initialize the query tracker. No-op if this is a disabled instance.
    pub fn init(&self, tracker: QueryTracker) -> Result<(), Error> {
        if !self.enabled {
            return Ok(());
        }
        let mut guard = self.write()?;
        *guard = Some(tracker);
        Ok(())
    }

    /// Drop the in-memory tracker and delete the on-disk database directory.
    ///
    /// Acquires the write lock, ensuring all readers (including any active mmap
    /// access) are finished before the LMDB environment is closed and the files
    /// are removed.
    ///
    /// Returns `Ok(Some(path))` with the deleted path, or `Ok(None)` if no
    /// tracker was initialized.
    pub fn destroy(&self) -> Result<Option<PathBuf>, Error> {
        let mut guard = self.write()?;
        let Some(tracker) = guard.take() else {
            return Ok(None);
        };
        let db_path = tracker.db_path().to_path_buf();
        drop(tracker);
        drop(guard);
        std::fs::remove_dir_all(&db_path).map_err(|source| Error::RemoveDbDir {
            path: db_path.clone(),
            source,
        })?;
        Ok(Some(db_path))
    }
}
