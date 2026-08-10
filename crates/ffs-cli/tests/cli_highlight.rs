//! End-to-end tests for ANSI highlight rendering (Track A).
//!
//! The key invariant: ANSI color is emitted ONLY when stdout is a tty and
//! `NO_COLOR` is unset. When piped (as these tests run), output must be
//! byte-clean — no `\x1b` anywhere — while `--format json` gains `match_ranges`.

use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::Value;
use tempfile::TempDir;

fn binary() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_ffs"))
}

fn write_file(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, body).unwrap();
}

/// Run `ffs grep` with stdout piped (not a tty). Returns (exit_ok, stdout_bytes, stderr).
fn run_grep_piped(root: &Path, args: &[&str]) -> (bool, Vec<u8>, String) {
    let mut cmd = Command::new(binary());
    cmd.args(["--root", root.to_str().unwrap()]);
    cmd.arg("grep");
    cmd.args(args);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let out = cmd.output().expect("run ffs grep");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    (out.status.success(), out.stdout, stderr)
}

#[test]
fn grep_json_keeps_path_line_text_and_adds_match_ranges() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "a.rs", "foo bar foo\nnothing here\n");
    let mut cmd = Command::new(binary());
    cmd.args([
        "--root",
        tmp.path().to_str().unwrap(),
        "--format",
        "json",
        "grep",
        "foo",
    ]);
    let out = cmd.output().expect("run");
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let hits = v["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    let h = &hits[0];
    assert_eq!(h["path"].as_str().unwrap().replace('\\', "/").ends_with("a.rs"), true);
    assert_eq!(h["line"], 1);
    assert_eq!(h["text"], "foo bar foo");
    // Two occurrences: bytes [0,3) and [8,11).
    let ranges = h["match_ranges"].as_array().unwrap();
    assert_eq!(ranges.len(), 2);
    assert_eq!(ranges[0][0], 0);
    assert_eq!(ranges[0][1], 3);
    assert_eq!(ranges[1][0], 8);
    assert_eq!(ranges[1][1], 11);
}

#[test]
fn grep_json_ranges_empty_in_files_with_matches_mode() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "a.rs", "foo bar\n");
    let mut cmd = Command::new(binary());
    cmd.args([
        "--root",
        tmp.path().to_str().unwrap(),
        "--format",
        "json",
        "grep",
        "-l",
        "foo",
    ]);
    let out = cmd.output().expect("run");
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let hits = v["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["line"], 0);
    assert_eq!(hits[0]["text"], "");
    // match_ranges omitted entirely (skip_serializing_if empty).
    assert!(hits[0].get("match_ranges").is_none());
}

#[test]
fn grep_json_has_no_ansi_when_piped() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "a.rs", "foo bar foo\n");
    let mut cmd = Command::new(binary());
    cmd.args([
        "--root",
        tmp.path().to_str().unwrap(),
        "--format",
        "json",
        "grep",
        "foo",
    ]);
    let out = cmd.output().expect("run");
    assert!(out.status.success());
    assert!(
        !out.stdout.contains(&0x1b),
        "JSON output must never contain ANSI escapes"
    );
    // And it parses as valid JSON.
    let _: Value = serde_json::from_slice(&out.stdout).unwrap();
}

#[test]
fn grep_text_piped_equals_plain_text() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "a.rs", "foo bar foo\n");
    let (ok, bytes, err) = run_grep_piped(tmp.path(), &["foo"]);
    assert!(ok, "stderr: {err}");
    // No ANSI escapes when piped.
    assert!(
        !bytes.contains(&0x1b),
        "piped text output leaked ANSI: {:?}",
        String::from_utf8_lossy(&bytes)
    );
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains(":1: foo bar foo"), "got: {text}");
}

#[test]
fn grep_text_no_color_env_still_plain_when_piped() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "a.rs", "foo bar foo\n");
    let mut cmd = Command::new(binary());
    cmd.args(["--root", tmp.path().to_str().unwrap(), "grep", "foo"]);
    cmd.env("NO_COLOR", "1");
    let out = cmd.output().expect("run");
    assert!(!out.stdout.contains(&0x1b));
}

#[test]
fn grep_regex_match_ranges_variable_length() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "a.rs", "aab aaab aaaaab\n");
    let mut cmd = Command::new(binary());
    cmd.args([
        "--root",
        tmp.path().to_str().unwrap(),
        "--format",
        "json",
        "grep",
        "--regex",
        "a+",
    ]);
    let out = cmd.output().expect("run");
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let hits = v["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    let ranges = hits[0]["match_ranges"].as_array().unwrap();
    // "aab aaab aaaaab" → runs [0,2) [4,7) [9,14)
    assert_eq!(ranges.len(), 3);
    assert_eq!(ranges[0][0], 0);
    assert_eq!(ranges[0][1], 2);
    assert_eq!(ranges[1][0], 4);
    assert_eq!(ranges[1][1], 7);
    assert_eq!(ranges[2][0], 9);
    assert_eq!(ranges[2][1], 14);
}

#[test]
fn multi_grep_json_match_ranges_and_matched_patterns() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "a.rs", "TODO: fix me\nall done\n");
    let mut cmd = Command::new(binary());
    cmd.args([
        "--root",
        tmp.path().to_str().unwrap(),
        "--format",
        "json",
        "multi-grep",
        "TODO",
    ]);
    let out = cmd.output().expect("run");
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let hits = v["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    let h = &hits[0];
    assert_eq!(h["text"], "TODO: fix me");
    assert_eq!(h["matched_patterns"], serde_json::json!(["TODO"]));
    assert_eq!(h["match_ranges"][0][0], 0);
    assert_eq!(h["match_ranges"][0][1], 4);
}

#[test]
fn find_fuzzy_json_unchanged() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "src/UnifiedScanner.rs", "x");
    let mut cmd = Command::new(binary());
    cmd.args([
        "--root",
        tmp.path().to_str().unwrap(),
        "--format",
        "json",
        "find",
        "--fuzzy",
        "scaner",
    ]);
    let out = cmd.output().expect("run");
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["schema"], "v1");
    // matches is an array of strings (no per-result range fields added).
    let m = v["matches"].as_array().unwrap();
    assert!(m.iter().all(|x| x.is_string()));
}

#[test]
fn find_text_piped_has_no_ansi() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "src/UnifiedScanner.rs", "x");
    let mut cmd = Command::new(binary());
    cmd.args([
        "--root",
        tmp.path().to_str().unwrap(),
        "find",
        "--fuzzy",
        "scaner",
    ]);
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let out = cmd.output().expect("run");
    assert!(
        !out.stdout.contains(&0x1b),
        "piped find leaked ANSI: {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn grep_dedups_lines_per_file() {
    let tmp = TempDir::new().unwrap();
    // Regex `a|a` matches the same line twice — must be deduped to one hit.
    write_file(tmp.path(), "a.rs", "foo a a bar\n");
    let mut cmd = Command::new(binary());
    cmd.args([
        "--root",
        tmp.path().to_str().unwrap(),
        "--format",
        "json",
        "grep",
        "--regex",
        "a|a",
    ]);
    let out = cmd.output().expect("run");
    assert!(out.status.success());
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    let hits = v["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
}
