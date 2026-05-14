//! Stable accessor functions for `ffs-c` FFI struct fields.
//!
//! # Why this exists
//!
//! `ffs-c` exposes its result types as plain `#[repr(C)]` structs. External
//! consumers (Emacs Lisp via `emacs-ffi`, Python `ctypes`, etc.) that access
//! fields by hardcoding byte offsets break silently whenever the struct layout
//! changes — a new field shifts every subsequent offset with no compile-time
//! warning.
//!
//! These functions turn field access into a **stable named API**: callers bind
//! to a symbol name once and are fully insulated from layout changes.
//!
//! # Usage from Emacs Lisp (example)
//!
//! ```elisp
//! (define-ffi-function ffs--grep-match-line-content
//!   "ffs_grep_match_get_line_content" :pointer [:pointer] ffs--library)
//!
//! (ffi-get-c-string (ffs--grep-match-line-content match-ptr))
//! ```
//!
//! # Array iteration
//!
//! To walk result arrays use `ffs_search_result_get_item`,
//! `ffs_grep_result_get_match`, and `ffs_search_result_get_score` — these are
//! defined in the main `lib.rs` FFI surface alongside the search functions.

use std::ffi::c_char;
use std::ptr;

use crate::ffi_types::{FfsFileItem, FfsGrepMatch, FfsGrepResult, FfsMatchRange, FfsSearchResult};

// ── FfsFileItem ──────────────────────────────────────────────────────────────

/// Returns the relative path of a file item (e.g. `"src/main.rs"`).
///
/// Returns null if `item` is null. The returned pointer is valid for the
/// lifetime of the owning `FfsSearchResult`; do not free it directly.
///
/// ## Safety
/// `item` must be a valid `FfsFileItem` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_file_item_get_relative_path(
    item: *const FfsFileItem,
) -> *const c_char {
    if item.is_null() {
        return ptr::null();
    }
    unsafe { (*item).relative_path }
}

/// Returns the file-name component of a file item (e.g. `"main.rs"`).
///
/// Returns null if `item` is null. Do not free the returned pointer.
///
/// ## Safety
/// `item` must be a valid `FfsFileItem` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_file_item_get_file_name(item: *const FfsFileItem) -> *const c_char {
    if item.is_null() {
        return ptr::null();
    }
    unsafe { (*item).file_name }
}

/// Returns the git status string for a file item (e.g. `"M "`, `"??"`)
/// or null if git is unavailable, the file is untracked, or `item` is null.
///
/// Do not free the returned pointer.
///
/// ## Safety
/// `item` must be a valid `FfsFileItem` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_file_item_get_git_status(item: *const FfsFileItem) -> *const c_char {
    if item.is_null() {
        return ptr::null();
    }
    unsafe { (*item).git_status }
}

/// Returns the file size in bytes. Returns `0` if `item` is null.
///
/// ## Safety
/// `item` must be a valid `FfsFileItem` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_file_item_get_size(item: *const FfsFileItem) -> u64 {
    if item.is_null() {
        return 0;
    }
    unsafe { (*item).size }
}

/// Returns the last-modified time as seconds since the UNIX epoch.
/// Returns `0` if `item` is null.
///
/// ## Safety
/// `item` must be a valid `FfsFileItem` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_file_item_get_modified(item: *const FfsFileItem) -> u64 {
    if item.is_null() {
        return 0;
    }
    unsafe { (*item).modified }
}

/// Returns the combined frecency score. Returns `0` if `item` is null.
///
/// ## Safety
/// `item` must be a valid `FfsFileItem` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_file_item_get_total_frecency_score(item: *const FfsFileItem) -> i64 {
    if item.is_null() {
        return 0;
    }
    unsafe { (*item).total_frecency_score }
}

/// Returns the access-based frecency score. Returns `0` if `item` is null.
///
/// ## Safety
/// `item` must be a valid `FfsFileItem` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_file_item_get_access_frecency_score(item: *const FfsFileItem) -> i64 {
    if item.is_null() {
        return 0;
    }
    unsafe { (*item).access_frecency_score }
}

/// Returns the modification-based frecency score. Returns `0` if `item` is null.
///
/// ## Safety
/// `item` must be a valid `FfsFileItem` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_file_item_get_modification_frecency_score(
    item: *const FfsFileItem,
) -> i64 {
    if item.is_null() {
        return 0;
    }
    unsafe { (*item).modification_frecency_score }
}

/// Returns `true` if the file was detected as binary. Returns `false` if `item` is null.
///
/// ## Safety
/// `item` must be a valid `FfsFileItem` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_file_item_get_is_binary(item: *const FfsFileItem) -> bool {
    if item.is_null() {
        return false;
    }
    unsafe { (*item).is_binary }
}

// ── FfsGrepMatch ─────────────────────────────────────────────────────────────

/// Returns the relative path of the file containing this grep match.
///
/// Returns null if `m` is null. Do not free the returned pointer.
///
/// ## Safety
/// `m` must be a valid `FfsGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_grep_match_get_relative_path(m: *const FfsGrepMatch) -> *const c_char {
    if m.is_null() {
        return ptr::null();
    }
    unsafe { (*m).relative_path }
}

/// Returns the file-name component of the file containing this grep match.
///
/// Returns null if `m` is null. Do not free the returned pointer.
///
/// ## Safety
/// `m` must be a valid `FfsGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_grep_match_get_file_name(m: *const FfsGrepMatch) -> *const c_char {
    if m.is_null() {
        return ptr::null();
    }
    unsafe { (*m).file_name }
}

/// Returns the git status string for the matched file (e.g. `"M "`, `"??"`)
/// or null if git is unavailable, the file is untracked, or `m` is null.
///
/// Do not free the returned pointer.
///
/// ## Safety
/// `m` must be a valid `FfsGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_grep_match_get_git_status(m: *const FfsGrepMatch) -> *const c_char {
    if m.is_null() {
        return ptr::null();
    }
    unsafe { (*m).git_status }
}

/// Returns the full text content of the matched line.
///
/// Returns null if `m` is null. Do not free the returned pointer.
///
/// ## Safety
/// `m` must be a valid `FfsGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_grep_match_get_line_content(m: *const FfsGrepMatch) -> *const c_char {
    if m.is_null() {
        return ptr::null();
    }
    unsafe { (*m).line_content }
}

/// Returns the 1-based line number of the match within its file.
/// Returns `0` if `m` is null.
///
/// ## Safety
/// `m` must be a valid `FfsGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_grep_match_get_line_number(m: *const FfsGrepMatch) -> u64 {
    if m.is_null() {
        return 0;
    }
    unsafe { (*m).line_number }
}

/// Returns the 0-based column of the match start within its line.
/// Returns `0` if `m` is null.
///
/// ## Safety
/// `m` must be a valid `FfsGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_grep_match_get_col(m: *const FfsGrepMatch) -> u32 {
    if m.is_null() {
        return 0;
    }
    unsafe { (*m).col }
}

/// Returns the byte offset of the match start from the beginning of the file.
/// Returns `0` if `m` is null.
///
/// ## Safety
/// `m` must be a valid `FfsGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_grep_match_get_byte_offset(m: *const FfsGrepMatch) -> u64 {
    if m.is_null() {
        return 0;
    }
    unsafe { (*m).byte_offset }
}

/// Returns the file size in bytes for the matched file. Returns `0` if `m` is null.
///
/// ## Safety
/// `m` must be a valid `FfsGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_grep_match_get_size(m: *const FfsGrepMatch) -> u64 {
    if m.is_null() {
        return 0;
    }
    unsafe { (*m).size }
}

/// Returns the combined frecency score for the matched file.
/// Returns `0` if `m` is null.
///
/// ## Safety
/// `m` must be a valid `FfsGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_grep_match_get_total_frecency_score(m: *const FfsGrepMatch) -> i64 {
    if m.is_null() {
        return 0;
    }
    unsafe { (*m).total_frecency_score }
}

/// Returns the access-based frecency score for the matched file.
/// Returns `0` if `m` is null.
///
/// ## Safety
/// `m` must be a valid `FfsGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_grep_match_get_access_frecency_score(m: *const FfsGrepMatch) -> i64 {
    if m.is_null() {
        return 0;
    }
    unsafe { (*m).access_frecency_score }
}

/// Returns the modification-based frecency score for the matched file.
/// Returns `0` if `m` is null.
///
/// ## Safety
/// `m` must be a valid `FfsGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_grep_match_get_modification_frecency_score(
    m: *const FfsGrepMatch,
) -> i64 {
    if m.is_null() {
        return 0;
    }
    unsafe { (*m).modification_frecency_score }
}

/// Returns the last-modified time as seconds since the UNIX epoch for the matched file.
/// Returns `0` if `m` is null.
///
/// ## Safety
/// `m` must be a valid `FfsGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_grep_match_get_modified(m: *const FfsGrepMatch) -> u64 {
    if m.is_null() {
        return 0;
    }
    unsafe { (*m).modified }
}

/// Returns the number of highlight ranges in this match. Returns `0` if `m` is null.
///
/// Use with [`ffs_grep_match_get_match_range`] to iterate the highlight spans.
///
/// ## Safety
/// `m` must be a valid `FfsGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_grep_match_get_match_ranges_count(m: *const FfsGrepMatch) -> u32 {
    if m.is_null() {
        return 0;
    }
    unsafe { (*m).match_ranges_count }
}

/// Returns a pointer to the `index`-th [`FfsMatchRange`] highlight span.
///
/// Returns null if `m` is null, `index >= match_ranges_count`, or the
/// ranges array is null. The returned pointer is valid until the owning
/// `FfsGrepResult` is freed; do not free it directly.
///
/// ## Safety
/// `m` must be a valid `FfsGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_grep_match_get_match_range(
    m: *const FfsGrepMatch,
    index: u32,
) -> *const FfsMatchRange {
    if m.is_null() {
        return ptr::null();
    }
    let m = unsafe { &*m };
    if index >= m.match_ranges_count || m.match_ranges.is_null() {
        return ptr::null();
    }
    unsafe { m.match_ranges.add(index as usize) }
}

/// Returns the number of context lines captured before the match.
/// Returns `0` if `m` is null.
///
/// Use with [`ffs_grep_match_get_context_before`] to read each line.
///
/// ## Safety
/// `m` must be a valid `FfsGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_grep_match_get_context_before_count(m: *const FfsGrepMatch) -> u32 {
    if m.is_null() {
        return 0;
    }
    unsafe { (*m).context_before_count }
}

/// Returns the `index`-th context line before the match.
///
/// Returns null if `m` is null, `index >= context_before_count`, or the
/// context array is null. Do not free the returned pointer.
///
/// ## Safety
/// `m` must be a valid `FfsGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_grep_match_get_context_before(
    m: *const FfsGrepMatch,
    index: u32,
) -> *const c_char {
    if m.is_null() {
        return ptr::null();
    }
    let m = unsafe { &*m };
    if index >= m.context_before_count || m.context_before.is_null() {
        return ptr::null();
    }
    unsafe { *m.context_before.add(index as usize) }
}

/// Returns the number of context lines captured after the match.
/// Returns `0` if `m` is null.
///
/// Use with [`ffs_grep_match_get_context_after`] to read each line.
///
/// ## Safety
/// `m` must be a valid `FfsGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_grep_match_get_context_after_count(m: *const FfsGrepMatch) -> u32 {
    if m.is_null() {
        return 0;
    }
    unsafe { (*m).context_after_count }
}

/// Returns the `index`-th context line after the match.
///
/// Returns null if `m` is null, `index >= context_after_count`, or the
/// context array is null. Do not free the returned pointer.
///
/// ## Safety
/// `m` must be a valid `FfsGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_grep_match_get_context_after(
    m: *const FfsGrepMatch,
    index: u32,
) -> *const c_char {
    if m.is_null() {
        return ptr::null();
    }
    let m = unsafe { &*m };
    if index >= m.context_after_count || m.context_after.is_null() {
        return ptr::null();
    }
    unsafe { *m.context_after.add(index as usize) }
}

/// Returns the fuzzy match score. Returns `0` if `m` is null or no fuzzy
/// score is present.
///
/// Always check [`ffs_grep_match_get_has_fuzzy_score`] first; `0` is
/// ambiguous without that flag.
///
/// ## Safety
/// `m` must be a valid `FfsGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_grep_match_get_fuzzy_score(m: *const FfsGrepMatch) -> u16 {
    if m.is_null() {
        return 0;
    }
    unsafe { (*m).fuzzy_score }
}

/// Returns `true` if this match carries a valid fuzzy score.
/// Returns `false` if `m` is null.
///
/// ## Safety
/// `m` must be a valid `FfsGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_grep_match_get_has_fuzzy_score(m: *const FfsGrepMatch) -> bool {
    if m.is_null() {
        return false;
    }
    unsafe { (*m).has_fuzzy_score }
}

/// Returns `true` if the match was identified as a symbol definition.
/// Returns `false` if `m` is null.
///
/// ## Safety
/// `m` must be a valid `FfsGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_grep_match_get_is_definition(m: *const FfsGrepMatch) -> bool {
    if m.is_null() {
        return false;
    }
    unsafe { (*m).is_definition }
}

/// Returns `true` if the matched file was detected as binary.
/// Returns `false` if `m` is null.
///
/// ## Safety
/// `m` must be a valid `FfsGrepMatch` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_grep_match_get_is_binary(m: *const FfsGrepMatch) -> bool {
    if m.is_null() {
        return false;
    }
    unsafe { (*m).is_binary }
}

// ── FfsSearchResult ──────────────────────────────────────────────────────────

/// Returns the number of items in the result. Returns `0` if `r` is null.
///
/// ## Safety
/// `r` must be a valid `FfsSearchResult` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_search_result_get_count(r: *const FfsSearchResult) -> u32 {
    if r.is_null() {
        return 0;
    }
    unsafe { (*r).count }
}

/// Returns the total number of files that matched before the result was
/// truncated to the page size. Returns `0` if `r` is null.
///
/// ## Safety
/// `r` must be a valid `FfsSearchResult` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_search_result_get_total_matched(r: *const FfsSearchResult) -> u32 {
    if r.is_null() {
        return 0;
    }
    unsafe { (*r).total_matched }
}

/// Returns the total number of indexed files considered during search.
/// Returns `0` if `r` is null.
///
/// ## Safety
/// `r` must be a valid `FfsSearchResult` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_search_result_get_total_files(r: *const FfsSearchResult) -> u32 {
    if r.is_null() {
        return 0;
    }
    unsafe { (*r).total_files }
}

// ── FfsGrepResult ─────────────────────────────────────────────────────────────

/// Returns the number of matches in the result. Returns `0` if `r` is null.
///
/// ## Safety
/// `r` must be a valid `FfsGrepResult` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_grep_result_get_count(r: *const FfsGrepResult) -> u32 {
    if r.is_null() {
        return 0;
    }
    unsafe { (*r).count }
}

/// Returns the total number of matches found across all pages.
/// Returns `0` if `r` is null.
///
/// ## Safety
/// `r` must be a valid `FfsGrepResult` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_grep_result_get_total_matched(r: *const FfsGrepResult) -> u32 {
    if r.is_null() {
        return 0;
    }
    unsafe { (*r).total_matched }
}

/// Returns the number of files actually opened and searched in this call.
/// Returns `0` if `r` is null.
///
/// ## Safety
/// `r` must be a valid `FfsGrepResult` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_grep_result_get_total_files_searched(r: *const FfsGrepResult) -> u32 {
    if r.is_null() {
        return 0;
    }
    unsafe { (*r).total_files_searched }
}

/// Returns the total number of indexed files before any filtering.
/// Returns `0` if `r` is null.
///
/// ## Safety
/// `r` must be a valid `FfsGrepResult` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_grep_result_get_total_files(r: *const FfsGrepResult) -> u32 {
    if r.is_null() {
        return 0;
    }
    unsafe { (*r).total_files }
}

/// Returns the number of files eligible for search after path/type filtering.
/// Returns `0` if `r` is null.
///
/// ## Safety
/// `r` must be a valid `FfsGrepResult` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_grep_result_get_filtered_file_count(r: *const FfsGrepResult) -> u32 {
    if r.is_null() {
        return 0;
    }
    unsafe { (*r).filtered_file_count }
}

/// Returns the file offset for the next page, or `0` if all files have been
/// searched or `r` is null. Pass this value as `file_offset` to a subsequent
/// `ffs_live_grep` or `ffs_multi_grep` call to continue pagination.
///
/// ## Safety
/// `r` must be a valid `FfsGrepResult` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_grep_result_get_next_file_offset(r: *const FfsGrepResult) -> u32 {
    if r.is_null() {
        return 0;
    }
    unsafe { (*r).next_file_offset }
}

/// Returns the regex compilation error string if the engine fell back to
/// literal matching, or null if there was no error or `r` is null.
///
/// Do not free the returned pointer.
///
/// ## Safety
/// `r` must be a valid `FfsGrepResult` pointer or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ffs_grep_result_get_regex_fallback_error(
    r: *const FfsGrepResult,
) -> *const c_char {
    if r.is_null() {
        return ptr::null();
    }
    unsafe { (*r).regex_fallback_error }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::ptr;

    // ── helpers ──────────────────────────────────────────────────────────────

    fn make_file_item(path: &str, name: &str) -> FfsFileItem {
        FfsFileItem {
            relative_path: CString::new(path).unwrap().into_raw(),
            file_name: CString::new(name).unwrap().into_raw(),
            git_status: ptr::null_mut(),
            size: 1024,
            modified: 1_700_000_000,
            access_frecency_score: 10,
            modification_frecency_score: 20,
            total_frecency_score: 30,
            is_binary: false,
        }
    }

    unsafe fn free_file_item(item: &mut FfsFileItem) {
        unsafe {
            if !item.relative_path.is_null() {
                drop(CString::from_raw(item.relative_path));
            }
            if !item.file_name.is_null() {
                drop(CString::from_raw(item.file_name));
            }
            if !item.git_status.is_null() {
                drop(CString::from_raw(item.git_status));
            }
        }
    }

    fn make_grep_match(path: &str, line: &str) -> FfsGrepMatch {
        FfsGrepMatch {
            relative_path: CString::new(path).unwrap().into_raw(),
            file_name: CString::new("file.rs").unwrap().into_raw(),
            git_status: ptr::null_mut(),
            line_content: CString::new(line).unwrap().into_raw(),
            match_ranges: ptr::null_mut(),
            context_before: ptr::null_mut(),
            context_after: ptr::null_mut(),
            size: 512,
            modified: 1_600_000_000,
            total_frecency_score: 5,
            access_frecency_score: 6,
            modification_frecency_score: 7,
            line_number: 42,
            byte_offset: 100,
            col: 8,
            match_ranges_count: 0,
            context_before_count: 0,
            context_after_count: 0,
            fuzzy_score: 0,
            has_fuzzy_score: false,
            is_binary: false,
            is_definition: true,
        }
    }

    unsafe fn free_grep_match(m: &mut FfsGrepMatch) {
        unsafe {
            if !m.relative_path.is_null() {
                drop(CString::from_raw(m.relative_path));
            }
            if !m.file_name.is_null() {
                drop(CString::from_raw(m.file_name));
            }
            if !m.line_content.is_null() {
                drop(CString::from_raw(m.line_content));
            }
        }
    }

    fn make_search_result(count: u32, total: u32, files: u32) -> FfsSearchResult {
        FfsSearchResult {
            items: ptr::null_mut(),
            scores: ptr::null_mut(),
            count,
            total_matched: total,
            total_files: files,
            location: crate::ffi_types::FfsLocation {
                tag: 0,
                line: 0,
                col: 0,
                end_line: 0,
                end_col: 0,
            },
        }
    }

    fn make_grep_result() -> FfsGrepResult {
        FfsGrepResult {
            items: ptr::null_mut(),
            count: 3,
            total_matched: 10,
            total_files_searched: 50,
            total_files: 200,
            filtered_file_count: 80,
            next_file_offset: 51,
            regex_fallback_error: ptr::null_mut(),
        }
    }

    // ── null-guard tests: every function returns its zero-value on NULL ───────

    #[test]
    fn null_file_item_returns_null_or_zero() {
        let null: *const FfsFileItem = ptr::null();
        unsafe {
            assert!(ffs_file_item_get_relative_path(null).is_null());
            assert!(ffs_file_item_get_file_name(null).is_null());
            assert!(ffs_file_item_get_git_status(null).is_null());
            assert_eq!(ffs_file_item_get_size(null), 0);
            assert_eq!(ffs_file_item_get_modified(null), 0);
            assert_eq!(ffs_file_item_get_access_frecency_score(null), 0);
            assert_eq!(ffs_file_item_get_modification_frecency_score(null), 0);
            assert_eq!(ffs_file_item_get_total_frecency_score(null), 0);
            assert!(!ffs_file_item_get_is_binary(null));
        }
    }

    #[test]
    fn null_grep_match_returns_null_or_zero() {
        let null: *const FfsGrepMatch = ptr::null();
        unsafe {
            assert!(ffs_grep_match_get_relative_path(null).is_null());
            assert!(ffs_grep_match_get_file_name(null).is_null());
            assert!(ffs_grep_match_get_git_status(null).is_null());
            assert!(ffs_grep_match_get_line_content(null).is_null());
            assert_eq!(ffs_grep_match_get_line_number(null), 0);
            assert_eq!(ffs_grep_match_get_byte_offset(null), 0);
            assert_eq!(ffs_grep_match_get_col(null), 0);
            assert_eq!(ffs_grep_match_get_size(null), 0);
            assert_eq!(ffs_grep_match_get_modified(null), 0);
            assert_eq!(ffs_grep_match_get_total_frecency_score(null), 0);
            assert_eq!(ffs_grep_match_get_access_frecency_score(null), 0);
            assert_eq!(ffs_grep_match_get_modification_frecency_score(null), 0);
            assert_eq!(ffs_grep_match_get_match_ranges_count(null), 0);
            assert_eq!(ffs_grep_match_get_context_before_count(null), 0);
            assert_eq!(ffs_grep_match_get_context_after_count(null), 0);
            assert!(!ffs_grep_match_get_has_fuzzy_score(null));
            assert_eq!(ffs_grep_match_get_fuzzy_score(null), 0);
            assert!(!ffs_grep_match_get_is_binary(null));
            assert!(!ffs_grep_match_get_is_definition(null));
            assert!(ffs_grep_match_get_context_before(null, 0).is_null());
            assert!(ffs_grep_match_get_context_after(null, 0).is_null());
            assert!(ffs_grep_match_get_match_range(null, 0).is_null());
        }
    }

    #[test]
    fn null_search_result_returns_zero() {
        let null: *const FfsSearchResult = ptr::null();
        unsafe {
            assert_eq!(ffs_search_result_get_count(null), 0);
            assert_eq!(ffs_search_result_get_total_matched(null), 0);
            assert_eq!(ffs_search_result_get_total_files(null), 0);
        }
    }

    #[test]
    fn null_grep_result_returns_zero_or_null() {
        let null: *const FfsGrepResult = ptr::null();
        unsafe {
            assert_eq!(ffs_grep_result_get_count(null), 0);
            assert_eq!(ffs_grep_result_get_total_matched(null), 0);
            assert_eq!(ffs_grep_result_get_total_files_searched(null), 0);
            assert_eq!(ffs_grep_result_get_total_files(null), 0);
            assert_eq!(ffs_grep_result_get_filtered_file_count(null), 0);
            assert_eq!(ffs_grep_result_get_next_file_offset(null), 0);
            assert!(ffs_grep_result_get_regex_fallback_error(null).is_null());
        }
    }

    // ── data correctness tests ────────────────────────────────────────────────

    #[test]
    fn file_item_getters_return_correct_values() {
        let mut item = make_file_item("src/main.rs", "main.rs");
        let p = &item as *const FfsFileItem;
        unsafe {
            let path = std::ffi::CStr::from_ptr(ffs_file_item_get_relative_path(p));
            assert_eq!(path.to_str().unwrap(), "src/main.rs");

            let name = std::ffi::CStr::from_ptr(ffs_file_item_get_file_name(p));
            assert_eq!(name.to_str().unwrap(), "main.rs");

            assert!(ffs_file_item_get_git_status(p).is_null());
            assert_eq!(ffs_file_item_get_size(p), 1024);
            assert_eq!(ffs_file_item_get_modified(p), 1_700_000_000);
            assert_eq!(ffs_file_item_get_access_frecency_score(p), 10);
            assert_eq!(ffs_file_item_get_modification_frecency_score(p), 20);
            assert_eq!(ffs_file_item_get_total_frecency_score(p), 30);
            assert!(!ffs_file_item_get_is_binary(p));

            free_file_item(&mut item);
        }
    }

    #[test]
    fn grep_match_getters_return_correct_values() {
        let mut m = make_grep_match("src/lib.rs", "fn hello()");
        let p = &m as *const FfsGrepMatch;
        unsafe {
            let path = std::ffi::CStr::from_ptr(ffs_grep_match_get_relative_path(p));
            assert_eq!(path.to_str().unwrap(), "src/lib.rs");

            let line = std::ffi::CStr::from_ptr(ffs_grep_match_get_line_content(p));
            assert_eq!(line.to_str().unwrap(), "fn hello()");

            assert_eq!(ffs_grep_match_get_line_number(p), 42);
            assert_eq!(ffs_grep_match_get_byte_offset(p), 100);
            assert_eq!(ffs_grep_match_get_col(p), 8);
            assert_eq!(ffs_grep_match_get_size(p), 512);
            assert_eq!(ffs_grep_match_get_modified(p), 1_600_000_000);
            assert_eq!(ffs_grep_match_get_total_frecency_score(p), 5);
            assert_eq!(ffs_grep_match_get_access_frecency_score(p), 6);
            assert_eq!(ffs_grep_match_get_modification_frecency_score(p), 7);
            assert_eq!(ffs_grep_match_get_match_ranges_count(p), 0);
            assert!(!ffs_grep_match_get_has_fuzzy_score(p));
            assert!(!ffs_grep_match_get_is_binary(p));
            assert!(ffs_grep_match_get_is_definition(p));

            free_grep_match(&mut m);
        }
    }

    #[test]
    fn search_result_getters_return_correct_values() {
        let r = make_search_result(5, 20, 100);
        let p = &r as *const FfsSearchResult;
        unsafe {
            assert_eq!(ffs_search_result_get_count(p), 5);
            assert_eq!(ffs_search_result_get_total_matched(p), 20);
            assert_eq!(ffs_search_result_get_total_files(p), 100);
        }
    }

    #[test]
    fn grep_result_getters_return_correct_values() {
        let r = make_grep_result();
        let p = &r as *const FfsGrepResult;
        unsafe {
            assert_eq!(ffs_grep_result_get_count(p), 3);
            assert_eq!(ffs_grep_result_get_total_matched(p), 10);
            assert_eq!(ffs_grep_result_get_total_files_searched(p), 50);
            assert_eq!(ffs_grep_result_get_total_files(p), 200);
            assert_eq!(ffs_grep_result_get_filtered_file_count(p), 80);
            assert_eq!(ffs_grep_result_get_next_file_offset(p), 51);
            assert!(ffs_grep_result_get_regex_fallback_error(p).is_null());
        }
    }
}
