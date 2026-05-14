//! FFI-compatible type definitions
//!
//! All result types use `#[repr(C)]` structs for direct memory access from any
//! language with C FFI support. No JSON serialization is used for search or grep
//! results — callers read struct fields directly.

use std::ffi::{CString, c_char, c_void};
use std::ptr;

use ffs::file_picker::FilePicker;
use ffs::git::format_git_status;
use ffs::{
    DirItem, DirSearchResult, FileItem, GrepMatch, GrepResult, Location, MixedItemRef,
    MixedSearchResult, Score, SearchResult,
};

/// Allocate a heap CString from a `&str`, returning a raw pointer.
fn cstring_new(s: &str) -> *mut c_char {
    CString::new(s).unwrap_or_default().into_raw()
}

/// Convert a `Vec<T>` into a raw pointer + count, leaking the memory.
fn vec_to_raw<T>(v: Vec<T>) -> (*mut T, u32) {
    if v.is_empty() {
        return (ptr::null_mut(), 0);
    }
    let count = v.len() as u32;
    let mut boxed = v.into_boxed_slice();
    let p = boxed.as_mut_ptr();
    std::mem::forget(boxed);
    (p, count)
}

/// Convert a `&[String]` into a heap-allocated array of C strings.
fn strings_to_raw(v: &[String]) -> (*mut *mut c_char, u32) {
    if v.is_empty() {
        return (ptr::null_mut(), 0);
    }
    let ptrs: Vec<*mut c_char> = v.iter().map(|s| cstring_new(s)).collect();
    vec_to_raw(ptrs)
}

/// Free a heap-allocated array of C strings.
///
/// ## Safety
/// `arr` must have been produced by `strings_to_raw`.
unsafe fn free_cstring_array(arr: *mut *mut c_char, count: u32) {
    if arr.is_null() {
        return;
    }
    unsafe {
        let ptrs = Vec::from_raw_parts(arr, count as usize, count as usize);
        for p in ptrs {
            if !p.is_null() {
                drop(CString::from_raw(p));
            }
        }
    }
}

/// A file item returned by `ffs_search`.
///
/// All string fields are heap-allocated and owned by the parent `FfsSearchResult`.
/// Free the entire result with `ffs_free_search_result`.
#[repr(C)]
pub struct FfsFileItem {
    pub relative_path: *mut c_char,
    pub file_name: *mut c_char,
    pub git_status: *mut c_char,
    pub size: u64,
    pub modified: u64,
    pub access_frecency_score: i64,
    pub modification_frecency_score: i64,
    pub total_frecency_score: i64,
    pub is_binary: bool,
}

impl FfsFileItem {
    pub fn from_item(item: &FileItem, picker: &FilePicker) -> Self {
        FfsFileItem {
            relative_path: cstring_new(&item.relative_path(picker)),
            file_name: cstring_new(&item.file_name(picker)),
            git_status: cstring_new(format_git_status(item.git_status)),
            size: item.size,
            modified: item.modified,
            access_frecency_score: item.access_frecency_score as i64,
            modification_frecency_score: item.modification_frecency_score as i64,
            total_frecency_score: item.total_frecency_score() as i64,
            is_binary: item.is_binary(),
        }
    }
}

impl FfsFileItem {
    /// ## Safety
    /// All string pointers must have been allocated by `CString::into_raw`.
    pub unsafe fn free_strings(&mut self) {
        unsafe {
            if !self.relative_path.is_null() {
                drop(CString::from_raw(self.relative_path));
            }
            if !self.file_name.is_null() {
                drop(CString::from_raw(self.file_name));
            }
            if !self.git_status.is_null() {
                drop(CString::from_raw(self.git_status));
            }
        }
    }
}

/// Score breakdown for a search result.
#[repr(C)]
pub struct FfsScore {
    pub total: i32,
    pub base_score: i32,
    pub filename_bonus: i32,
    pub special_filename_bonus: i32,
    pub frecency_boost: i32,
    pub distance_penalty: i32,
    pub current_file_penalty: i32,
    pub combo_match_boost: i32,
    pub path_alignment_bonus: i32,
    pub exact_match: bool,
    pub match_type: *mut c_char,
}

impl From<&Score> for FfsScore {
    fn from(score: &Score) -> Self {
        FfsScore {
            total: score.total,
            base_score: score.base_score,
            filename_bonus: score.filename_bonus,
            special_filename_bonus: score.special_filename_bonus,
            frecency_boost: score.frecency_boost,
            distance_penalty: score.distance_penalty,
            current_file_penalty: score.current_file_penalty,
            combo_match_boost: score.combo_match_boost,
            path_alignment_bonus: score.path_alignment_bonus,
            exact_match: score.exact_match,
            match_type: cstring_new(score.match_type),
        }
    }
}

impl FfsScore {
    /// ## Safety
    /// `match_type` must have been allocated by `CString::into_raw`.
    pub unsafe fn free_strings(&mut self) {
        unsafe {
            if !self.match_type.is_null() {
                drop(CString::from_raw(self.match_type));
            }
        }
    }
}

/// Location parsed from a query string (e.g. `"file.ts:42:10"`).
///
/// `tag` encodes the variant:
///   0 = no location,
///   1 = line only (`line` is set),
///   2 = position (`line` + `col`),
///   3 = range (`line`/`col` = start, `end_line`/`end_col` = end).
#[repr(C)]
pub struct FfsLocation {
    pub tag: u8,
    pub line: i32,
    pub col: i32,
    pub end_line: i32,
    pub end_col: i32,
}

impl From<Option<&Location>> for FfsLocation {
    fn from(loc: Option<&Location>) -> Self {
        match loc {
            None => FfsLocation {
                tag: 0,
                line: 0,
                col: 0,
                end_line: 0,
                end_col: 0,
            },
            Some(Location::Line(line)) => FfsLocation {
                tag: 1,
                line: *line,
                col: 0,
                end_line: 0,
                end_col: 0,
            },
            Some(Location::Position { line, col }) => FfsLocation {
                tag: 2,
                line: *line,
                col: *col,
                end_line: 0,
                end_col: 0,
            },
            Some(Location::Range { start, end }) => FfsLocation {
                tag: 3,
                line: start.0,
                col: start.1,
                end_line: end.0,
                end_col: end.1,
            },
        }
    }
}

/// Search result returned by `ffs_search`.
///
/// The caller must free this with `ffs_free_search_result`.
#[repr(C)]
pub struct FfsSearchResult {
    /// Pointer to a heap-allocated array of `FfsFileItem` (length = `count`).
    pub items: *mut FfsFileItem,
    /// Pointer to a heap-allocated array of `FfsScore` (length = `count`).
    pub scores: *mut FfsScore,
    /// Number of items/scores in the arrays.
    pub count: u32,
    /// Total number of files that matched the query.
    pub total_matched: u32,
    /// Total number of indexed files.
    pub total_files: u32,
    /// Location parsed from the query string.
    pub location: FfsLocation,
}

impl FfsSearchResult {
    /// Convert a core `SearchResult` into a heap-allocated `FfsSearchResult`.
    pub fn from_core(result: &SearchResult, picker: &FilePicker) -> *mut Self {
        let items: Vec<FfsFileItem> = result
            .items
            .iter()
            .map(|i| FfsFileItem::from_item(i, picker))
            .collect();
        let scores: Vec<FfsScore> = result.scores.iter().map(FfsScore::from).collect();
        let count = items.len() as u32;

        let (items_ptr, _) = vec_to_raw(items);
        let (scores_ptr, _) = vec_to_raw(scores);

        Box::into_raw(Box::new(FfsSearchResult {
            items: items_ptr,
            scores: scores_ptr,
            count,
            total_matched: result.total_matched as u32,
            total_files: result.total_files as u32,
            location: FfsLocation::from(result.location.as_ref()),
        }))
    }
}

// ---------------------------------------------------------------------------
// Grep result types
// ---------------------------------------------------------------------------

/// A byte range within a matched line, used for highlighting.
#[repr(C)]
pub struct FfsMatchRange {
    pub start: u32,
    pub end: u32,
}

/// A single grep match with file and line information.
///
/// All string fields and arrays are heap-allocated. Free the parent
/// `FfsGrepResult` with `ffs_free_grep_result` to release everything.
#[repr(C)]
pub struct FfsGrepMatch {
    // -- pointers (8 bytes each) --
    pub relative_path: *mut c_char,
    pub file_name: *mut c_char,
    pub git_status: *mut c_char,
    pub line_content: *mut c_char,
    pub match_ranges: *mut FfsMatchRange,
    pub context_before: *mut *mut c_char,
    pub context_after: *mut *mut c_char,
    // -- 8-byte numeric fields --
    pub size: u64,
    pub modified: u64,
    pub total_frecency_score: i64,
    pub access_frecency_score: i64,
    pub modification_frecency_score: i64,
    pub line_number: u64,
    pub byte_offset: u64,
    // -- 4-byte fields --
    pub col: u32,
    pub match_ranges_count: u32,
    pub context_before_count: u32,
    pub context_after_count: u32,
    // -- 2-byte fields --
    pub fuzzy_score: u16,
    // -- 1-byte fields --
    pub has_fuzzy_score: bool,
    pub is_binary: bool,
    pub is_definition: bool,
}

impl FfsGrepMatch {
    fn from_core_with_file(m: &GrepMatch, file: &FileItem, picker: &FilePicker) -> Self {
        let ranges: Vec<FfsMatchRange> = m
            .match_byte_offsets
            .iter()
            .map(|&(start, end)| FfsMatchRange { start, end })
            .collect();
        let (match_ranges, match_ranges_count) = vec_to_raw(ranges);
        let (context_before, context_before_count) = strings_to_raw(&m.context_before);
        let (context_after, context_after_count) = strings_to_raw(&m.context_after);
        let (has_fuzzy_score, fuzzy_score) = match m.fuzzy_score {
            Some(s) => (true, s),
            None => (false, 0),
        };

        FfsGrepMatch {
            relative_path: cstring_new(&file.relative_path(picker)),
            file_name: cstring_new(&file.file_name(picker)),
            git_status: cstring_new(format_git_status(file.git_status)),
            line_content: cstring_new(&m.line_content),
            match_ranges,
            context_before,
            context_after,
            size: file.size,
            modified: file.modified,
            total_frecency_score: file.total_frecency_score() as i64,
            access_frecency_score: file.access_frecency_score as i64,
            modification_frecency_score: file.modification_frecency_score as i64,
            line_number: m.line_number,
            byte_offset: m.byte_offset,
            col: m.col as u32,
            match_ranges_count,
            context_before_count,
            context_after_count,
            fuzzy_score,
            has_fuzzy_score,
            is_binary: file.is_binary(),
            is_definition: m.is_definition,
        }
    }

    /// ## Safety
    /// All pointers must have been allocated by the corresponding `from_core`.
    pub unsafe fn free_fields(&mut self) {
        unsafe {
            if !self.relative_path.is_null() {
                drop(CString::from_raw(self.relative_path));
            }
            if !self.file_name.is_null() {
                drop(CString::from_raw(self.file_name));
            }
            if !self.git_status.is_null() {
                drop(CString::from_raw(self.git_status));
            }
            if !self.line_content.is_null() {
                drop(CString::from_raw(self.line_content));
            }
            if !self.match_ranges.is_null() {
                drop(Vec::from_raw_parts(
                    self.match_ranges,
                    self.match_ranges_count as usize,
                    self.match_ranges_count as usize,
                ));
            }
            free_cstring_array(self.context_before, self.context_before_count);
            free_cstring_array(self.context_after, self.context_after_count);
        }
    }
}

/// Grep result returned by `ffs_live_grep` and `ffs_multi_grep`.
///
/// The caller must free this with `ffs_free_grep_result`.
#[repr(C)]
pub struct FfsGrepResult {
    /// Pointer to a heap-allocated array of `FfsGrepMatch` (length = `count`).
    pub items: *mut FfsGrepMatch,
    /// Number of matches in the `items` array.
    pub count: u32,
    /// Total number of matches (always equal to `count`).
    pub total_matched: u32,
    /// Number of files actually opened and searched in this call.
    pub total_files_searched: u32,
    /// Total number of indexed files (before any filtering).
    pub total_files: u32,
    /// Number of files eligible for search after filtering.
    pub filtered_file_count: u32,
    /// File offset for the next page. 0 if all files have been searched.
    pub next_file_offset: u32,
    /// Regex compilation error when falling back to literal matching. Null if none.
    pub regex_fallback_error: *mut c_char,
}

impl FfsGrepResult {
    /// Convert a core `GrepResult` into a heap-allocated `FfsGrepResult`.
    pub fn from_core(result: &GrepResult, picker: &FilePicker) -> *mut Self {
        let items: Vec<FfsGrepMatch> = result
            .matches
            .iter()
            .map(|m| {
                let file = result.files[m.file_index];
                FfsGrepMatch::from_core_with_file(m, file, picker)
            })
            .collect();
        let (items_ptr, count) = vec_to_raw(items);

        Box::into_raw(Box::new(FfsGrepResult {
            items: items_ptr,
            count,
            total_matched: result.matches.len() as u32,
            total_files_searched: result.total_files_searched as u32,
            total_files: result.total_files as u32,
            filtered_file_count: result.filtered_file_count as u32,
            next_file_offset: result.next_file_offset as u32,
            regex_fallback_error: match &result.regex_fallback_error {
                Some(e) => cstring_new(e),
                None => ptr::null_mut(),
            },
        }))
    }
}

/// Result envelope returned by all `ffs_*` functions.
///
/// Heap-allocated — the caller must free it with `ffs_free_result`.
///
/// Depending on the function, the payload is delivered through different fields:
///
/// | Function                   | Payload field | Type                          |
/// |----------------------------|---------------|-------------------------------|
/// | `ffs_create_instance`      | `handle`      | opaque instance pointer       |
/// | `ffs_search`               | `handle`      | `*mut FfsSearchResult`        |
/// | `ffs_live_grep`            | `handle`      | `*mut FfsGrepResult`          |
/// | `ffs_multi_grep`           | `handle`      | `*mut FfsGrepResult`          |
/// | `ffs_get_scan_progress`    | `handle`      | `*mut FfsScanProgress`        |
/// | `ffs_health_check`         | `handle`      | `*mut c_char` (JSON string)   |
/// | `ffs_get_historical_query` | `handle`      | `*mut c_char` (string or null)|
/// | `ffs_wait_for_scan`        | `int_value`   | 1 = completed, 0 = timed out  |
/// | `ffs_track_query`          | `int_value`   | 1 = success, 0 = failure      |
/// | `ffs_refresh_git_status`   | `int_value`   | number of files updated       |
/// | `ffs_scan_files`           | (none)        | success flag only             |
/// | `ffs_restart_index`        | (none)        | success flag only             |
///
/// On failure, `success` is false and `error` contains the message.
///
/// **Important:** `ffs_free_result` frees `error` but does **not** free `handle`.
/// The caller must free the handle with the appropriate function
/// (`ffs_destroy`, `ffs_free_search_result`, `ffs_free_grep_result`,
///  `ffs_free_string`, etc.).
#[repr(C)]
pub struct FfsResult {
    /// Whether the operation succeeded.
    pub success: bool,
    /// Error message on failure. Null on success.
    pub error: *mut c_char,
    /// Opaque pointer payload (instance handle, typed result struct, or string). May be null.
    pub handle: *mut c_void,
    /// Integer payload for simple return values (bool as 0/1, counts, etc.).
    pub int_value: i64,
}

impl FfsResult {
    /// Create a successful result with no payload, returned as heap pointer.
    pub fn ok_empty() -> *mut Self {
        Box::into_raw(Box::new(FfsResult {
            success: true,
            error: ptr::null_mut(),
            handle: ptr::null_mut(),
            int_value: 0,
        }))
    }

    /// Create a successful result with an integer value.
    pub fn ok_int(value: i64) -> *mut Self {
        Box::into_raw(Box::new(FfsResult {
            success: true,
            error: ptr::null_mut(),
            handle: ptr::null_mut(),
            int_value: value,
        }))
    }

    /// Create a successful result carrying an opaque pointer (handle, typed struct, or string).
    pub fn ok_handle(handle: *mut c_void) -> *mut Self {
        Box::into_raw(Box::new(FfsResult {
            success: true,
            error: ptr::null_mut(),
            handle,
            int_value: 0,
        }))
    }

    /// Create a successful result carrying a C string in the `handle` field.
    /// The caller must free it with `ffs_free_string`.
    pub fn ok_string(s: &str) -> *mut Self {
        let cstr = CString::new(s).unwrap_or_default().into_raw();
        Box::into_raw(Box::new(FfsResult {
            success: true,
            error: ptr::null_mut(),
            handle: cstr as *mut c_void,
            int_value: 0,
        }))
    }

    /// Create an error result, returned as heap pointer.
    pub fn err(error: &str) -> *mut Self {
        Box::into_raw(Box::new(FfsResult {
            success: false,
            error: CString::new(error).unwrap_or_default().into_raw(),
            handle: ptr::null_mut(),
            int_value: 0,
        }))
    }
}

/// A directory item returned by `ffs_search_directories`.
///
/// All string fields are heap-allocated and owned by the parent `FfsDirSearchResult`.
/// Free the entire result with `ffs_free_dir_search_result`.
#[repr(C)]
pub struct FfsDirItem {
    pub relative_path: *mut c_char,
    pub dir_name: *mut c_char,
    pub max_access_frecency: i32,
}

impl FfsDirItem {
    pub fn from_item(item: &DirItem, picker: &FilePicker) -> Self {
        FfsDirItem {
            relative_path: cstring_new(&item.relative_path(picker)),
            dir_name: cstring_new(&item.dir_name(picker)),
            max_access_frecency: item.max_access_frecency(),
        }
    }

    /// ## Safety
    /// All string pointers must have been allocated by the rust side
    pub unsafe fn free_strings(&mut self) {
        unsafe {
            if !self.relative_path.is_null() {
                drop(CString::from_raw(self.relative_path));
            }
            if !self.dir_name.is_null() {
                drop(CString::from_raw(self.dir_name));
            }
        }
    }
}

/// Directory search result returned by `ffs_search_directories`.
///
/// The caller must free this with `ffs_free_dir_search_result`.
#[repr(C)]
pub struct FfsDirSearchResult {
    /// Pointer to a heap-allocated array of `FfsDirItem` (length = `count`).
    pub items: *mut FfsDirItem,
    /// Pointer to a heap-allocated array of `FfsScore` (length = `count`).
    pub scores: *mut FfsScore,
    /// Number of items/scores in the arrays.
    pub count: u32,
    /// Total number of directories that matched the query.
    pub total_matched: u32,
    /// Total number of indexed directories.
    pub total_dirs: u32,
}

impl FfsDirSearchResult {
    /// Convert a core `DirSearchResult` into a heap-allocated `FfsDirSearchResult`.
    pub fn from_core(result: &DirSearchResult, picker: &FilePicker) -> *mut Self {
        let items: Vec<FfsDirItem> = result
            .items
            .iter()
            .map(|i| FfsDirItem::from_item(i, picker))
            .collect();
        let scores: Vec<FfsScore> = result.scores.iter().map(FfsScore::from).collect();
        let count = items.len() as u32;

        let (items_ptr, _) = vec_to_raw(items);
        let (scores_ptr, _) = vec_to_raw(scores);

        Box::into_raw(Box::new(FfsDirSearchResult {
            items: items_ptr,
            scores: scores_ptr,
            count,
            total_matched: result.total_matched as u32,
            total_dirs: result.total_dirs as u32,
        }))
    }
}

/// A single item in a mixed (files + directories) search result.
///
/// `item_type`: 0 = file, 1 = directory.
/// All string fields are heap-allocated and owned by the parent `FfsMixedSearchResult`.
#[repr(C)]
pub struct FfsMixedItem {
    /// 0 = file, 1 = directory.
    pub item_type: u8,
    pub relative_path: *mut c_char,
    /// Filename for files, last directory segment for directories.
    pub display_name: *mut c_char,
    pub git_status: *mut c_char,
    pub size: u64,
    pub modified: u64,
    /// The access frecency score for files, or max access frecency among all the immediate
    /// children for directories.
    pub access_frecency_score: i64,
    /// Always 0 for directories
    pub modification_frecency_score: i64,
    /// Always 0 for directories
    pub total_frecency_score: i64,
    /// Always 0 for directories
    pub is_binary: bool,
}

impl FfsMixedItem {
    pub fn from_mixed_ref(item: &MixedItemRef<'_>, picker: &FilePicker) -> Self {
        match item {
            MixedItemRef::File(file) => FfsMixedItem {
                item_type: 0,
                relative_path: cstring_new(&file.relative_path(picker)),
                display_name: cstring_new(&file.file_name(picker)),
                git_status: cstring_new(format_git_status(file.git_status)),
                size: file.size,
                modified: file.modified,
                access_frecency_score: file.access_frecency_score as i64,
                modification_frecency_score: file.modification_frecency_score as i64,
                total_frecency_score: file.total_frecency_score() as i64,
                is_binary: file.is_binary(),
            },
            MixedItemRef::Dir(dir) => FfsMixedItem {
                item_type: 1,
                relative_path: cstring_new(&dir.relative_path(picker)),
                display_name: cstring_new(&dir.dir_name(picker)),
                git_status: cstring_new(""),
                size: 0,
                modified: 0,
                access_frecency_score: dir.max_access_frecency() as i64,
                modification_frecency_score: 0,
                total_frecency_score: dir.max_access_frecency() as i64,
                is_binary: false,
            },
        }
    }

    /// ## Safety
    /// All string pointers must have been allocated by rust side
    pub unsafe fn free_strings(&mut self) {
        unsafe {
            if !self.relative_path.is_null() {
                drop(CString::from_raw(self.relative_path));
            }
            if !self.display_name.is_null() {
                drop(CString::from_raw(self.display_name));
            }
            if !self.git_status.is_null() {
                drop(CString::from_raw(self.git_status));
            }
        }
    }
}

/// Mixed search result returned by `ffs_search_mixed`.
///
/// The caller must free this with `ffs_free_mixed_search_result`.
#[repr(C)]
pub struct FfsMixedSearchResult {
    /// Pointer to a heap-allocated array of `FfsMixedItem` (length = `count`).
    pub items: *mut FfsMixedItem,
    /// Pointer to a heap-allocated array of `FfsScore` (length = `count`).
    pub scores: *mut FfsScore,
    /// Number of items/scores in the arrays.
    pub count: u32,
    /// Total number of items (files + dirs) that matched the query.
    pub total_matched: u32,
    /// Total number of indexed files.
    pub total_files: u32,
    /// Total number of indexed directories.
    pub total_dirs: u32,
    /// Location parsed from the query string.
    pub location: FfsLocation,
}

impl FfsMixedSearchResult {
    /// Convert a core `MixedSearchResult` into a heap-allocated `FfsMixedSearchResult`.
    pub fn from_core(result: &MixedSearchResult, picker: &FilePicker) -> *mut Self {
        let items: Vec<FfsMixedItem> = result
            .items
            .iter()
            .map(|i| FfsMixedItem::from_mixed_ref(i, picker))
            .collect();
        let scores: Vec<FfsScore> = result.scores.iter().map(FfsScore::from).collect();
        let count = items.len() as u32;

        let (items_ptr, _) = vec_to_raw(items);
        let (scores_ptr, _) = vec_to_raw(scores);

        Box::into_raw(Box::new(FfsMixedSearchResult {
            items: items_ptr,
            scores: scores_ptr,
            count,
            total_matched: result.total_matched as u32,
            total_files: result.total_files as u32,
            total_dirs: result.total_dirs as u32,
            location: FfsLocation::from(result.location.as_ref()),
        }))
    }
}

/// Scan progress returned by `ffs_get_scan_progress`.
/// The caller must free this with `ffs_free_scan_progress`.
#[repr(C)]
pub struct FfsScanProgress {
    pub scanned_files_count: u64,
    pub is_scanning: bool,
    pub is_watcher_ready: bool,
    pub is_warmup_complete: bool,
}

impl From<ffs::file_picker::ScanProgress> for FfsScanProgress {
    fn from(p: ffs::file_picker::ScanProgress) -> Self {
        Self {
            scanned_files_count: p.scanned_files_count as u64,
            is_scanning: p.is_scanning,
            is_watcher_ready: p.is_watcher_ready,
            is_warmup_complete: p.is_warmup_complete,
        }
    }
}
