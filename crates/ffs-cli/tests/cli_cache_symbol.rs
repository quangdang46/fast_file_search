//! End-to-end test: `ffs index` writes a cache, and the next `ffs symbol`
//! invocation loads it. Verifies the cache file shows up under `.ffs/` and
//! that subsequent reads stay correct.

use std::path::Path;
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

fn binary() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_ffs"))
}

fn run(root: &Path, args: &[&str]) -> (Value, std::time::Duration) {
    let mut cmd = Command::new(binary());
    cmd.args(["--root", root.to_str().unwrap(), "--format", "json"]);
    cmd.args(args);
    let start = std::time::Instant::now();
    let out = cmd.output().expect("spawn ffs");
    let elapsed = start.elapsed();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).expect("parse json");
    (v, elapsed)
}

fn write_file(dir: &Path, rel: &str, body: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, body).unwrap();
}

#[test]
fn ffs_index_writes_cache_and_symbol_reuses_it() {
    let tmp = TempDir::new().unwrap();
    write_file(
        tmp.path(),
        "src/lib.rs",
        "pub fn alpha() {}\npub fn beta() {}\n",
    );
    write_file(tmp.path(), "src/extra.rs", "pub fn gamma() {}\n");

    // Index pass: should produce .ffs/symbol_index.postcard.zst.
    let (v, _) = run(tmp.path(), &["index"]);
    assert!(v["cache_written"].as_bool().unwrap());
    let cache_file = tmp.path().join(".ffs/symbol_index.postcard.zst");
    let meta_file = tmp.path().join(".ffs/meta.json");
    assert!(
        cache_file.exists(),
        "cache file should exist after `ffs index`"
    );
    assert!(
        meta_file.exists(),
        "meta.json should exist after `ffs index`"
    );

    // First symbol query — cache hit (since index just wrote it).
    let (v1, _) = run(tmp.path(), &["symbol", "alpha"]);
    assert_eq!(v1["total"].as_u64().unwrap(), 1);

    // Cold path with no cache: delete .ffs/, run symbol again. The query
    // must still succeed (build-on-miss path) and recreate the cache.
    std::fs::remove_dir_all(tmp.path().join(".ffs")).unwrap();
    let (v2, _) = run(tmp.path(), &["symbol", "alpha"]);
    assert_eq!(v2["total"].as_u64().unwrap(), 1);
    assert!(
        cache_file.exists(),
        "symbol command should rewrite cache on miss"
    );
}

#[test]
fn ffs_symbol_works_without_prior_index() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "main.rs", "fn solo() {}\n");

    // No `ffs index` ever ran — symbol must still find the def via the
    // build-on-miss branch.
    let (v, _) = run(tmp.path(), &["symbol", "solo"]);
    assert_eq!(v["total"].as_u64().unwrap(), 1);
    let hits = v["hits"].as_array().unwrap();
    assert_eq!(hits[0]["name"].as_str().unwrap(), "solo");
}

#[test]
fn cache_invalidates_when_file_count_changes_substantially() {
    let tmp = TempDir::new().unwrap();
    // Tiny seed — tolerance is max(file_count/20, 10) so we need a big jump.
    write_file(tmp.path(), "a.rs", "fn a() {}\n");
    let (_, _) = run(tmp.path(), &["index"]);

    // Add 30 more files; drift exceeds the ±10 tolerance.
    for i in 0..30 {
        write_file(
            tmp.path(),
            &format!("gen_{i}.rs"),
            &format!("fn g{i}() {{}}\n"),
        );
    }

    // symbol should still work (cache miss → rebuild → hit on new symbol).
    let (v, _) = run(tmp.path(), &["symbol", "g7"]);
    assert_eq!(v["total"].as_u64().unwrap(), 1);
}

#[test]
fn ffs_index_writes_bigram_cache_and_grep_uses_it() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "src/lib.rs", "fn alpha() {}\nfn beta() {}\n");
    write_file(tmp.path(), "src/extra.rs", "fn gamma() {}\n");

    let (v, _) = run(tmp.path(), &["index"]);
    assert!(v["bigram_cache_written"].as_bool().unwrap());
    assert!(v["bigram_count"].as_u64().unwrap() > 0);
    let bigram_file = tmp.path().join(".ffs/bigram.postcard.zst");
    assert!(
        bigram_file.exists(),
        "bigram cache should exist after `ffs index`"
    );

    // Literal pattern that only appears in lib.rs — bigram filter must
    // narrow the candidate list and still produce the correct hit.
    let (v, _) = run(tmp.path(), &["grep", "alpha"]);
    let hits = v["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1, "expected single hit for 'alpha'");
    assert_eq!(hits[0]["line"].as_u64().unwrap(), 1);

    // Pattern that doesn't appear anywhere — bigram filter rules out
    // every file.
    let (v, _) = run(tmp.path(), &["grep", "deltagram"]);
    assert_eq!(v["hits"].as_array().unwrap().len(), 0);
}

#[test]
fn ffs_grep_works_without_bigram_cache() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "src/lib.rs", "fn solo() {}\n");
    // No `ffs index` ran — bigram cache absent, grep must fall back to
    // walk_files and still find the match.
    let (v, _) = run(tmp.path(), &["grep", "solo"]);
    let hits = v["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
}
