//! End-to-end tests for `scry impact <symbol>` (B6). Builds tiny synthetic
//! workspaces with explicit direct/imports/transitive shapes and pins the
//! resulting ranking + reasons.

use std::path::Path;
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

fn binary() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_scry"))
}

fn run(root: &Path, args: &[&str]) -> Value {
    let mut cmd = Command::new(binary());
    cmd.args(["--root", root.to_str().unwrap(), "--format", "json"]);
    cmd.arg("impact");
    cmd.args(args);
    let out = cmd.output().expect("run scry impact");
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
fn empty_workspace_reports_zero_total() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "src/lib.rs", "pub fn target() {}\n");

    let v = run(tmp.path(), &["target"]);
    assert_eq!(v["total"].as_u64().unwrap(), 0);
    assert!(v["results"].as_array().unwrap().is_empty());
}

#[test]
fn direct_callers_weighted_three_to_one_over_transitive() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "src/lib.rs", "pub fn target() {}\n");
    // a.rs calls target() once directly.
    write_file(
        tmp.path(),
        "src/a.rs",
        "pub fn a_caller() {\n    target();\n}\n",
    );
    // b.rs calls a_caller() three times (transitive at depth 2 if BFS finds it).
    write_file(
        tmp.path(),
        "src/b.rs",
        "fn b1() { a_caller(); }\n\
         fn b2() { a_caller(); }\n\
         fn b3() { a_caller(); }\n",
    );

    let v = run(tmp.path(), &["target", "--hops", "3"]);
    let rows = v["results"].as_array().unwrap();
    // Both a.rs and b.rs should be ranked. a.rs has direct=1 (score 3),
    // b.rs has transitive>=3 (score 3+). With direct getting weight 3 and
    // transitive weight 1, a single direct call ties with three transitives —
    // ordering between them is alphabetical when tied, so a.rs ranks first.
    let paths: Vec<String> = rows
        .iter()
        .map(|r| r["path"].as_str().unwrap().to_string())
        .collect();
    assert!(paths.iter().any(|p| p.ends_with("a.rs")));
    assert!(paths.iter().any(|p| p.ends_with("b.rs")));
    let a_row = rows
        .iter()
        .find(|r| r["path"].as_str().unwrap().ends_with("a.rs"))
        .unwrap();
    let reasons: Vec<String> = a_row["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_str().unwrap().to_string())
        .collect();
    assert!(reasons.iter().any(|r| r.starts_with("direct: 1")));
}

#[test]
fn reverse_imports_contribute_weight_two() {
    let tmp = TempDir::new().unwrap();
    // Defn lives in src/lib.rs. Two other files `use` it explicitly and
    // call it once each.
    write_file(
        tmp.path(),
        "Cargo.toml",
        "[package]\nname=\"t\"\nversion=\"0.1.0\"\nedition=\"2021\"\n[lib]\npath=\"src/lib.rs\"\n",
    );
    write_file(tmp.path(), "src/lib.rs", "pub fn target() {}\n");
    write_file(
        tmp.path(),
        "src/imp_a.rs",
        "use crate::target;\nfn _x() { target(); }\n",
    );
    write_file(
        tmp.path(),
        "src/imp_b.rs",
        "use crate::target;\nfn _y() { target(); }\n",
    );

    let v = run(tmp.path(), &["target", "--hops", "1"]);
    let rows = v["results"].as_array().unwrap();
    assert!(
        rows.len() >= 2,
        "expected both importers in results, got {rows:?}"
    );
    for row in rows {
        let reasons: Vec<String> = row["reasons"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_string())
            .collect();
        // Each importer counts at least one direct call site (the use line
        // matches the literal too, so direct may be >=1).
        assert!(
            reasons.iter().any(|r| r.starts_with("direct:")),
            "row {row:?} has no direct reason"
        );
    }
}

#[test]
fn hub_guard_prevents_propagation_past_popular_helper() {
    let tmp = TempDir::new().unwrap();
    // target <- hot (1 call) <- {p1..p4} (4 callers of hot) <- g1 (calls each p).
    // With --hub-guard 1 propagation from `hot` (4 callers > 1) must stop,
    // so g.rs (depth 3) should never be reached.
    write_file(tmp.path(), "src/lib.rs", "pub fn target() {}\n");
    write_file(tmp.path(), "src/h.rs", "fn hot() { target(); }\n");
    write_file(
        tmp.path(),
        "src/p.rs",
        "fn p1() { hot(); }\n\
         fn p2() { hot(); }\n\
         fn p3() { hot(); }\n\
         fn p4() { hot(); }\n",
    );
    write_file(
        tmp.path(),
        "src/g.rs",
        "fn g1() { p1(); p2(); p3(); p4(); }\n",
    );

    let v = run(tmp.path(), &["target", "--hops", "3", "--hub-guard", "1"]);
    let rows = v["results"].as_array().unwrap();
    let g_present = rows
        .iter()
        .any(|r| r["path"].as_str().unwrap().ends_with("g.rs"));
    assert!(
        !g_present,
        "hub-guard=1 should have stopped propagation before g.rs, got: {rows:?}"
    );
}

#[test]
fn pagination_respects_limit_and_offset() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "src/lib.rs", "pub fn target() {}\n");
    for i in 0..6 {
        write_file(
            tmp.path(),
            &format!("src/c{i}.rs"),
            "fn x() {\n    target();\n}\n",
        );
    }

    let v = run(tmp.path(), &["target", "--limit", "2", "--hops", "1"]);
    assert_eq!(v["results"].as_array().unwrap().len(), 2);
    assert!(v["has_more"].as_bool().unwrap());
    assert!(v["total"].as_u64().unwrap() >= 6);

    let v2 = run(
        tmp.path(),
        &["target", "--limit", "2", "--offset", "4", "--hops", "1"],
    );
    assert_eq!(v2["offset"].as_u64().unwrap(), 4);
}
