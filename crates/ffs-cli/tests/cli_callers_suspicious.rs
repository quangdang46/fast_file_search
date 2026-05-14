//! End-to-end tests for `ffs callers --hops N` telemetry (B4):
//! `suspicious_hops` lights up when a name resolves to definitions in two
//! distinct package roots at the same hop. `auto_hubs_promoted` lights up
//! when a single name fires more hits in a hop than `--hub-guard` allows.
//! Both fields stay omitted from JSON when empty so the default `--hops 1`
//! output is byte-identical to the legacy contract.

use std::path::Path;
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

fn binary() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_ffs"))
}

fn run(root: &Path, extra: &[&str]) -> Value {
    let mut cmd = Command::new(binary());
    cmd.args(["--root", root.to_str().unwrap(), "--format", "json"]);
    cmd.args(["callers"]);
    cmd.args(extra);
    let out = cmd.output().expect("run ffs callers");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("parse json")
}

fn write_file(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, body).unwrap();
}

#[test]
fn default_hops_omits_new_telemetry_fields() {
    let tmp = TempDir::new().unwrap();
    write_file(
        tmp.path(),
        "src/lib.rs",
        "pub fn target() {}\npub fn caller() { target(); }\n",
    );

    let v = run(tmp.path(), &["target"]);
    // Legacy `--hops 1` path must not surface the new fields.
    assert!(v.get("suspicious_hops").is_none());
    assert!(v.get("auto_hubs_promoted").is_none());
    assert!(v["total"].as_u64().unwrap() >= 1);
}

#[test]
fn suspicious_hops_reports_name_defined_in_multiple_roots() {
    let tmp = TempDir::new().unwrap();
    // Same symbol name `dup` defined in two distinct package roots — this
    // is exactly what the suspicious-hops list is meant to flag for the
    // first hop (lookup of the initial name).
    write_file(tmp.path(), "src/a/mod.rs", "pub fn dup() {}\n");
    write_file(tmp.path(), "src/b/mod.rs", "pub fn dup() {}\n");
    write_file(
        tmp.path(),
        "src/c/use.rs",
        "fn caller() {\n    dup();\n    dup();\n}\n",
    );

    let v = run(tmp.path(), &["dup", "--hops", "2"]);
    let suspicious = v["suspicious_hops"].as_array().expect("array");
    assert!(!suspicious.is_empty(), "expected suspicious_hops entry");
    let entry = &suspicious[0];
    assert_eq!(entry["name"].as_str().unwrap(), "dup");
    assert_eq!(entry["depth"].as_u64().unwrap(), 1);
    let roots: Vec<String> = entry["roots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_str().unwrap().to_string())
        .collect();
    assert!(roots.iter().any(|r| r.ends_with("src/a")));
    assert!(roots.iter().any(|r| r.ends_with("src/b")));
}

#[test]
fn auto_hubs_promoted_fires_when_hub_guard_kicks_in() {
    let tmp = TempDir::new().unwrap();
    // `hot` is a popular helper — many callers in one file. With --hub-guard 1
    // BFS must stop propagating from `hot` and record that it did so.
    let body = "pub fn hot() {}\n".to_string()
        + "fn a() { hot(); }\n"
        + "fn b() { hot(); }\n"
        + "fn c() { hot(); }\n";
    write_file(tmp.path(), "src/hot.rs", &body);

    let v = run(tmp.path(), &["hot", "--hops", "2", "--hub-guard", "1"]);
    let promoted = v["auto_hubs_promoted"]
        .as_array()
        .expect("auto_hubs_promoted array");
    assert!(!promoted.is_empty(), "expected hub-guard promotion entry");
    let entry = &promoted[0];
    assert_eq!(entry["name"].as_str().unwrap(), "hot");
    assert_eq!(entry["depth"].as_u64().unwrap(), 1);
    assert!(entry["count"].as_u64().unwrap() >= 2);
}

#[test]
fn telemetry_renders_text_sections_when_nonempty() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "src/a/mod.rs", "pub fn dup() {}\n");
    write_file(tmp.path(), "src/b/mod.rs", "pub fn dup() {}\n");
    write_file(tmp.path(), "src/c/use.rs", "fn caller() {\n    dup();\n}\n");

    let out = Command::new(binary())
        .args(["--root", tmp.path().to_str().unwrap()])
        .args(["callers", "dup", "--hops", "2"])
        .output()
        .expect("run ffs callers --hops 2");
    assert!(out.status.success());
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(
        s.contains("Suspicious hops"),
        "expected suspicious section in text output:\n{s}"
    );
}
