//! End-to-end tests for the rg-parity flags added to `ffs grep`:
//! `-i`, `-v`, `-o`, `-g`, `-m`, `--files-without-match`, `--hidden`,
//! `--no-ignore`, `-a`.

use std::path::Path;
use std::process::Command;

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

fn grep_json(root: &Path, args: &[&str]) -> Value {
    let mut cmd = Command::new(binary());
    cmd.args(["--root", root.to_str().unwrap(), "--format", "json", "grep"]);
    cmd.args(args);
    let out = cmd.output().expect("run ffs grep");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("valid json")
}

#[test]
fn ignore_case_forces_insensitive_over_smart_case() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "a.rs", "TODO here\ntodo there\n");
    // Without -i, an uppercase pattern is smart-case sensitive and would
    // only match line 1. With -i it matches both.
    let v = grep_json(tmp.path(), &["-i", "TODO"]);
    assert_eq!(v["hits"].as_array().unwrap().len(), 2);
}

#[test]
fn invert_match_selects_non_matching_lines() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "a.rs", "keep this\nfoo line\nkeep that\n");
    let v = grep_json(tmp.path(), &["-v", "foo"]);
    let hits = v["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0]["text"], "keep this");
    assert_eq!(hits[1]["text"], "keep that");
}

#[test]
fn only_matching_emits_one_row_per_match_on_shared_line() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "a.rs", "foo bar foo\n");
    let v = grep_json(tmp.path(), &["-o", "foo"]);
    let hits = v["hits"].as_array().unwrap();
    // Two occurrences of "foo" on the same line -> two rows, not one merged row.
    assert_eq!(hits.len(), 2);
    for h in hits {
        assert_eq!(h["text"], "foo");
    }
}

#[test]
fn glob_filter_includes_only_matching_extension() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "a.rs", "needle\n");
    write_file(tmp.path(), "b.txt", "needle\n");
    let v = grep_json(tmp.path(), &["-g", "*.rs", "needle"]);
    let hits = v["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0]["path"].as_str().unwrap().ends_with("a.rs"));
}

#[test]
fn glob_filter_exclude_pattern() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "a.rs", "needle\n");
    write_file(tmp.path(), "b.rs", "needle\n");
    let v = grep_json(tmp.path(), &["-g", "!b.rs", "needle"]);
    let hits = v["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0]["path"].as_str().unwrap().ends_with("a.rs"));
}

#[test]
fn max_count_per_file_caps_hits_per_file() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "a.rs", "needle\nneedle\nneedle\n");
    let v = grep_json(tmp.path(), &["-m", "2", "needle"]);
    assert_eq!(v["hits"].as_array().unwrap().len(), 2);
}

#[test]
fn files_without_match_lists_non_matching_files_only() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "a.rs", "needle\n");
    write_file(tmp.path(), "b.rs", "nothing\n");
    let v = grep_json(tmp.path(), &["--files-without-match", "needle"]);
    let hits = v["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0]["path"].as_str().unwrap().ends_with("b.rs"));
}

#[test]
fn hidden_flag_reveals_dotfiles() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), ".hidden.rs", "needle\n");
    let without = grep_json(tmp.path(), &["needle"]);
    assert_eq!(without["hits"].as_array().unwrap().len(), 0);
    let with_hidden = grep_json(tmp.path(), &["--hidden", "needle"]);
    assert_eq!(with_hidden["hits"].as_array().unwrap().len(), 1);
}

fn grep_text(root: &Path, args: &[&str]) -> String {
    let mut cmd = Command::new(binary());
    cmd.args(["--root", root.to_str().unwrap(), "grep"]);
    cmd.args(args);
    let out = cmd.output().expect("run ffs grep");
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("utf8")
}

#[test]
fn compact_emits_relative_rg_style_rows() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "sub/a.rs", "livekit here\n");
    let text = grep_text(tmp.path(), &["--compact", "--limit", "5", "livekit"]);
    let first: Vec<String> = text
        .lines()
        .filter(|l| !l.starts_with('['))
        .map(|l| l.replace('\\', "/"))
        .collect();
    assert_eq!(first.len(), 1);
    assert!(
        first[0].starts_with("sub/a.rs:1: "),
        "expected relative rg-style row, got {:?}",
        first[0]
    );
    assert!(first[0].contains("livekit here"));
}

#[test]
fn compact_files_with_matches_emits_relative_paths() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "sub/a.rs", "livekit here\n");
    let text = grep_text(tmp.path(), &["--compact", "-l", "livekit"]);
    let rows: Vec<String> = text
        .lines()
        .filter(|l| !l.starts_with('['))
        .map(|l| l.replace('\\', "/"))
        .collect();
    assert_eq!(rows, vec!["sub/a.rs"]);
}

#[test]
fn files_with_matches_streams_large_files_without_full_read() {
    // File exceeds the CLI's streaming threshold (8 MiB) — `-l` must still
    // find a needle placed near the end without OOMing on read_to_end.
    let tmp = TempDir::new().unwrap();
    let mut data = vec![b'x'; 9 * 1024 * 1024];
    let tail = b"UNIQUE_STREAM_NEEDLE";
    let pos = data.len() - tail.len() - 10;
    data[pos..pos + tail.len()].copy_from_slice(tail);
    write_file(tmp.path(), "big.log", &String::from_utf8_lossy(&data));

    let v = grep_json(tmp.path(), &["-l", "UNIQUE_STREAM_NEEDLE"]);
    let hits = v["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0]["path"].as_str().unwrap().ends_with("big.log"));
}

#[test]
fn files_with_matches_streams_large_files_no_match() {
    let tmp = TempDir::new().unwrap();
    let data = vec![b'x'; 9 * 1024 * 1024];
    write_file(tmp.path(), "big.log", &String::from_utf8_lossy(&data));

    let v = grep_json(tmp.path(), &["-l", "NOT_PRESENT_ANYWHERE"]);
    let hits = v["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 0);
}

#[test]
fn ignore_case_no_false_positive_on_control_byte() {
    // Same regression as ffs-core's grep_integration test, for the CLI's
    // independent CaseInsensitiveLiteralIter implementation.
    let tmp = TempDir::new().unwrap();
    let mut line = b"axxxxxxx".to_vec();
    line.push(0x10); // DLE, 0x20 away from the needle's trailing '0' (0x30)
    line.push(b'\n');
    write_file(tmp.path(), "a.txt", &String::from_utf8_lossy(&line));

    let v = grep_json(tmp.path(), &["-i", "axxxxxxx0"]);
    assert_eq!(
        v["hits"].as_array().unwrap().len(),
        0,
        "DLE must not false-positive-match '0' under -i"
    );
}

#[test]
fn files_with_matches_skips_binary_content() {
    // The -l fast path rejects binaries from a leading 512B probe (rg
    // contract, see #122); a NUL byte inside the window must skip the file
    // even when the needle is present as literal text further in.
    // `-a/--text` must override that.
    let tmp = TempDir::new().unwrap();
    let p = tmp.path().join("bin.dat");
    let mut data = b"prefix ".to_vec();
    data.push(0u8); // NUL inside the 512B window
    data.extend_from_slice(b"NEEDLE_AFTER_NUL\n");
    std::fs::write(&p, &data).unwrap();

    let v = grep_json(tmp.path(), &["-l", "NEEDLE_AFTER_NUL"]);
    assert_eq!(
        v["hits"].as_array().unwrap().len(),
        0,
        "binary file must be skipped by -l"
    );

    let v_text = grep_json(tmp.path(), &["-l", "-a", "NEEDLE_AFTER_NUL"]);
    assert_eq!(
        v_text["hits"].as_array().unwrap().len(),
        1,
        "-a/--text must search binaries"
    );
}

#[test]
fn files_with_matches_nul_past_probe_window_is_text() {
    // rg contract (issue 122): only the first 512 bytes decide
    // binary-vs-text. A NUL at offset 600 is an ordinary byte — the file is
    // searched as text, exactly like rg.
    let tmp = TempDir::new().unwrap();
    let mut data = vec![b'A'; 600];
    data.push(0u8); // NUL past the 512B window
    data.extend_from_slice(b"NEEDLE_AFTER_NUL\n");
    std::fs::write(tmp.path().join("late-nul.bin"), &data).unwrap();

    let v = grep_json(tmp.path(), &["-l", "NEEDLE_AFTER_NUL"]);
    let hits = v["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1, "NUL past 512B must not skip the file");
    assert!(hits[0]["path"].as_str().unwrap().ends_with("late-nul.bin"));
}

#[test]
fn files_with_matches_finds_needle_past_the_probe_chunk() {
    // A match beyond the 512B probe window must not be missed: the probe only
    // rejects binaries, it never decides "no match" for a larger file.
    let tmp = TempDir::new().unwrap();
    let mut data = vec![b'x'; 32 * 1024];
    let tail = b"NEEDLE_PAST_PROBE";
    let pos = data.len() - tail.len() - 1;
    data[pos..pos + tail.len()].copy_from_slice(tail);
    std::fs::write(tmp.path().join("big.txt"), &data).unwrap();

    let v = grep_json(tmp.path(), &["-l", "NEEDLE_PAST_PROBE"]);
    let hits = v["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1, "match past the probe chunk must be found");
    assert!(hits[0]["path"].as_str().unwrap().ends_with("big.txt"));
}

#[test]
fn content_mode_skips_binary_and_finds_large_text() {
    // Content (non -l) mode shares the probe: binaries skipped, large text
    // files still searched in full.
    let tmp = TempDir::new().unwrap();
    let mut bin = b"aaa".to_vec();
    bin.push(0u8);
    bin.extend_from_slice(b"\nNEEDLE_CONTENT\n");
    std::fs::write(tmp.path().join("bin.dat"), &bin).unwrap();

    let mut big = vec![b'y'; 40 * 1024];
    big.extend_from_slice(b"\nNEEDLE_CONTENT\n");
    std::fs::write(tmp.path().join("big.txt"), &big).unwrap();

    let v = grep_json(tmp.path(), &["NEEDLE_CONTENT"]);
    let hits = v["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1, "only the text file should match");
    assert!(hits[0]["path"].as_str().unwrap().ends_with("big.txt"));
}

#[test]
fn word_regexp_filters_partial_matches_on_literal_path() {
    // `-w` on a pattern with no metacharacters takes the Literal matcher —
    // the flag must still be honored (issue 126: it was silently dropped).
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "a.rs", "foo-bar foo_bar foo\n");
    // Plain: all three occurrences.
    let v = grep_json(tmp.path(), &["-o", "foo"]);
    assert_eq!(v["hits"].as_array().unwrap().len(), 3);
    // Whole-word: `foo-bar` (left boundary, `-` is non-word) + trailing
    // standalone `foo`; `foo_bar` is excluded (`_` is a word char).
    let v = grep_json(tmp.path(), &["-w", "-o", "foo"]);
    assert_eq!(v["hits"].as_array().unwrap().len(), 2);
}

#[test]
fn word_regexp_unicode_mode_treats_multibyte_as_word() {
    // `café` is one word: the `caf` prefix must not match whole-word.
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "u.txt", "café cafe\n");
    let v = grep_json(tmp.path(), &["-w", "-o", "caf"]);
    assert_eq!(v["hits"].as_array().unwrap().len(), 0);
    let v = grep_json(tmp.path(), &["-w", "-o", "cafe"]);
    assert_eq!(v["hits"].as_array().unwrap().len(), 1);
}

#[test]
fn word_regexp_ascii_mode_treats_non_ascii_as_boundary() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "u.txt", "café cafe\n");
    // Byte-level: the 0xC3 after `caf` is a boundary, so the prefix counts.
    let v = grep_json(tmp.path(), &["-w", "-o", "caf", "--word-boundary", "ascii"]);
    assert_eq!(v["hits"].as_array().unwrap().len(), 1);
}

#[test]
fn word_regexp_works_in_files_with_matches_mode() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "a.rs", "foo_bar\n");
    write_file(tmp.path(), "b.rs", "a foo here\n");
    let v = grep_json(tmp.path(), &["-w", "-l", "foo"]);
    let hits = v["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert!(hits[0]["path"].as_str().unwrap().ends_with("b.rs"));
}

#[test]
fn invert_match_clean_file_emits_all_lines() {
    // Fast path: a file with zero matches must emit every line (the
    // matched-set would be empty, so skipping its construction must not
    // change output).
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "a.rs", "aaa\nbbb\nccc\n");
    write_file(tmp.path(), "b.rs", "foo here\nbar\n");
    let v = grep_json(tmp.path(), &["-v", "foo"]);
    let hits = v["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 4);
    let texts: Vec<&str> = hits.iter().map(|h| h["text"].as_str().unwrap()).collect();
    assert_eq!(texts, vec!["aaa", "bbb", "ccc", "bar"]);
}

#[test]
fn invert_match_respects_max_count() {
    let tmp = TempDir::new().unwrap();
    write_file(tmp.path(), "a.rs", "aaa\nbbb\nccc\n");
    let v = grep_json(tmp.path(), &["-v", "-m", "2", "foo"]);
    assert_eq!(v["hits"].as_array().unwrap().len(), 2);
}
