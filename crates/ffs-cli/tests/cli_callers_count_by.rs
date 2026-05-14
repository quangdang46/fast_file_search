//! End-to-end tests for `ffs callers --count-by` (B3). Verifies that the
//! aggregation field is opt-in (omitted by default → byte-identical legacy
//! behaviour) and that `caller` / `file` group keys produce sensible counts
//! on a synthetic three-call graph spread across two files.

use std::path::Path;
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

const TARGET_DEF: &str = "\
pub fn target() {}
";

// Two distinct callers (alpha, beta) inside src/a.rs, each calling target() once.
const A_RS: &str = "\
pub fn alpha() {
    target();
}

pub fn beta() {
    target();
}
";

// One additional caller (gamma) inside src/b.rs.
const B_RS: &str = "\
pub fn gamma() {
    target();
}
";

fn binary() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_ffs"))
}

fn write_workspace() -> TempDir {
    let tmp = TempDir::new().expect("tempdir");
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("target.rs"), TARGET_DEF).unwrap();
    std::fs::write(src.join("a.rs"), A_RS).unwrap();
    std::fs::write(src.join("b.rs"), B_RS).unwrap();
    tmp
}

fn run(root: &Path, extra: &[&str]) -> Value {
    let mut cmd = Command::new(binary());
    cmd.args(["--root", root.to_str().unwrap(), "--format", "json"]);
    cmd.args(["callers", "target"]);
    cmd.args(extra);
    let out = cmd.output().expect("run ffs callers");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("parse json")
}

#[test]
fn count_by_default_is_none_and_field_is_absent() {
    let tmp = write_workspace();
    let v = run(tmp.path(), &[]);
    // 3 distinct call sites surfaced as hits.
    assert_eq!(v["total"].as_u64().unwrap(), 3);
    // No aggregations field when count-by is omitted.
    assert!(v.get("aggregations").is_none());
}

#[test]
fn count_by_file_groups_by_path() {
    let tmp = write_workspace();
    let v = run(tmp.path(), &["--count-by", "file"]);
    let aggs = v["aggregations"].as_array().expect("aggregations array");
    // a.rs has 2 hits, b.rs has 1 — two distinct paths.
    assert_eq!(aggs.len(), 2);
    let counts: Vec<u64> = aggs.iter().map(|a| a["count"].as_u64().unwrap()).collect();
    assert_eq!(counts, vec![2, 1]);
    assert!(aggs[0]["key"].as_str().unwrap().ends_with("a.rs"));
    assert!(aggs[1]["key"].as_str().unwrap().ends_with("b.rs"));
}

#[test]
fn count_by_caller_groups_by_enclosing_symbol() {
    let tmp = write_workspace();
    let v = run(tmp.path(), &["--count-by", "caller"]);
    let aggs = v["aggregations"].as_array().expect("aggregations array");
    // alpha / beta / gamma — three distinct enclosing functions.
    let mut keys: Vec<String> = aggs
        .iter()
        .map(|a| a["key"].as_str().unwrap().to_string())
        .collect();
    keys.sort();
    assert_eq!(keys, vec!["alpha", "beta", "gamma"]);
    for entry in aggs {
        assert_eq!(entry["count"].as_u64().unwrap(), 1);
    }
}

#[test]
fn count_by_text_appends_aggregated_section() {
    let tmp = write_workspace();
    let out = Command::new(binary())
        .args(["--root", tmp.path().to_str().unwrap()])
        .args(["callers", "target", "--count-by", "file"])
        .output()
        .expect("run ffs callers --count-by file");
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    assert!(stdout.contains("Aggregated:"));
    assert!(stdout.contains("a.rs"));
    assert!(stdout.contains("b.rs"));
}
