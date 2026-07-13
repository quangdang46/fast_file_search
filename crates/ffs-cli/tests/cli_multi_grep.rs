//! End-to-end tests for `ffs multi-grep`.

use std::path::Path;
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

fn binary() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_ffs"))
}

fn run_json(root: &Path, args: &[&str]) -> (bool, Value, String) {
    let mut cmd = Command::new(binary());
    cmd.args(["--root", root.to_str().unwrap(), "--format", "json"]);
    cmd.arg("multi-grep");
    cmd.args(args);
    let out = cmd.output().expect("run ffs multi-grep");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let ok = out.status.success();
    let v = if stdout.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&stdout).unwrap_or(Value::Null)
    };
    if !ok {
        return (false, v, stderr);
    }
    (true, v, stderr)
}

fn write_file(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, body).unwrap();
}

#[test]
fn multi_grep_or_matches_any_pattern() {
    let tmp = TempDir::new().unwrap();
    write_file(
        tmp.path(),
        "a.rs",
        "pub enum GrepMode {\n    PlainText,\n}\n",
    );
    write_file(
        tmp.path(),
        "b.rs",
        "struct PlainTextMatcher {\n    needle: Vec<u8>,\n}\n",
    );
    write_file(tmp.path(), "c.rs", "fn main() { println!(\"hi\"); }\n");

    let (ok, v, err) = run_json(
        tmp.path(),
        &["GrepMode", "PlainTextMatcher", "--limit", "50"],
    );
    assert!(ok, "stderr: {err}");
    assert_eq!(v["mode"], "multi-literal-or");
    let patterns = v["patterns"].as_array().unwrap();
    assert_eq!(patterns.len(), 2);
    let hits = v["hits"].as_array().unwrap();
    assert!(hits.len() >= 2, "hits={hits:?}");

    let texts: Vec<&str> = hits.iter().filter_map(|h| h["text"].as_str()).collect();
    assert!(
        texts.iter().any(|t| t.contains("GrepMode")),
        "missing GrepMode in {texts:?}"
    );
    assert!(
        texts.iter().any(|t| t.contains("PlainTextMatcher")),
        "missing PlainTextMatcher in {texts:?}"
    );

    // c.rs has neither pattern
    assert!(
        !hits
            .iter()
            .any(|h| h["path"].as_str().unwrap_or("").ends_with("c.rs")),
        "c.rs should not match"
    );
}

#[test]
fn multi_grep_empty_patterns_fails() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "a.rs", "fn x() {}\n");
    let mut cmd = Command::new(binary());
    cmd.args(["--root", tmp.path().to_str().unwrap(), "multi-grep"]);
    let out = cmd.output().expect("run");
    assert!(!out.status.success());
}

#[test]
fn multi_grep_e_flag() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "x.rs", "alpha and beta\n");
    let (ok, v, err) = run_json(tmp.path(), &["-e", "alpha", "-e", "beta"]);
    assert!(ok, "stderr: {err}");
    let hits = v["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0]["text"].as_str().unwrap().contains("alpha"));
}

#[test]
fn multigrep_alias_works() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "x.rs", "needle_here\n");
    let mut cmd = Command::new(binary());
    cmd.args([
        "--root",
        tmp.path().to_str().unwrap(),
        "--format",
        "json",
        "multigrep",
        "needle_here",
    ]);
    let out = cmd.output().expect("run");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(!v["hits"].as_array().unwrap().is_empty());
}
