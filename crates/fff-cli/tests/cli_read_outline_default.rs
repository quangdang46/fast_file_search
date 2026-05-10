//! End-to-end tests for the B0 default-routing of `scry read`:
//! a no-flag invocation returns the agent-style outline, not the full body.

use std::process::Command;

use tempfile::TempDir;

const SAMPLE: &str = "\
use std::collections::HashMap;
use anyhow::Result;

pub struct Foo {
    pub bar: u32,
}

impl Foo {
    pub fn new() -> Self {
        Self { bar: 0 }
    }

    pub fn bar(&self) -> u32 {
        self.bar
    }
}

pub fn entrypoint() -> Result<()> {
    let _ = Foo::new();
    Ok(())
}
";

fn binary() -> std::path::PathBuf {
    let exe = env!("CARGO_BIN_EXE_scry");
    std::path::PathBuf::from(exe)
}

fn write_sample(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("sample.rs");
    std::fs::write(&path, SAMPLE).expect("write sample");
    path
}

#[test]
fn read_with_no_flags_emits_agent_outline_for_code_files() {
    let tmp = TempDir::new().expect("tempdir");
    let _ = write_sample(tmp.path());

    let out = Command::new(binary())
        .args(["--root", tmp.path().to_str().unwrap()])
        .args(["read", "sample.rs"])
        .output()
        .expect("run scry read");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).expect("utf8");

    // Header line: `# <path> (<lines> lines, ~<tokens> tokens) [outline]`.
    assert!(stdout.starts_with("# sample.rs ("), "missing header line:\n{stdout}");
    assert!(stdout.contains("[outline]"), "missing [outline] tag");
    // Bundled imports row.
    assert!(stdout.contains("imports: std, anyhow"), "missing bundled imports:\n{stdout}");
    // Definitions present.
    assert!(stdout.contains("struct Foo"));
    assert!(stdout.contains("function entrypoint"));
    // Footer hint.
    assert!(stdout.contains("> Next: drill into a symbol"));
    // Should NOT include raw body (e.g. the function bodies).
    assert!(!stdout.contains("self.bar = 0"));
}

#[test]
fn read_with_full_flag_returns_raw_body() {
    let tmp = TempDir::new().expect("tempdir");
    let _ = write_sample(tmp.path());

    let out = Command::new(binary())
        .args(["--root", tmp.path().to_str().unwrap()])
        .args(["read", "sample.rs", "--full"])
        .output()
        .expect("run scry read --full");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).expect("utf8");

    // Raw body must include source lines.
    assert!(stdout.contains("pub struct Foo {"));
    assert!(stdout.contains("pub fn entrypoint() -> Result<()>"));
    // No outline header in `--full` mode.
    assert!(!stdout.starts_with("# "));
    assert!(!stdout.contains("[outline]"));
}

#[test]
fn read_with_line_suffix_drills_into_section() {
    let tmp = TempDir::new().expect("tempdir");
    let _ = write_sample(tmp.path());

    let out = Command::new(binary())
        .args(["--root", tmp.path().to_str().unwrap()])
        .args(["read", "sample.rs:18"])
        .output()
        .expect("run scry read sample.rs:18");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).expect("utf8");

    // Section banner is emitted; the body of the function we land on is
    // included; only the `entrypoint` body is rendered.
    assert!(stdout.starts_with("// "), "expected section banner:\n{stdout}");
    assert!(stdout.contains("pub fn entrypoint() -> Result<()>"));
    assert!(stdout.contains("Foo::new()"));
    assert!(!stdout.contains("pub struct Foo"));
}

#[test]
fn outline_default_style_is_agent() {
    let tmp = TempDir::new().expect("tempdir");
    let _ = write_sample(tmp.path());

    let out = Command::new(binary())
        .args(["--root", tmp.path().to_str().unwrap()])
        .args(["outline", "sample.rs"])
        .output()
        .expect("run scry outline");
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    let stdout = String::from_utf8(out.stdout).expect("utf8");

    assert!(stdout.starts_with("# sample.rs ("), "default outline style should be agent:\n{stdout}");
    assert!(stdout.contains("> Next: drill into a symbol"));
}

#[test]
fn outline_legacy_styles_still_work() {
    let tmp = TempDir::new().expect("tempdir");
    let _ = write_sample(tmp.path());

    for style in ["markdown", "structured", "tabular"] {
        let out = Command::new(binary())
            .args(["--root", tmp.path().to_str().unwrap()])
            .args(["outline", "sample.rs", "--style", style])
            .output()
            .unwrap_or_else(|e| panic!("run scry outline --style {style}: {e}"));
        assert!(
            out.status.success(),
            "style {style} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8(out.stdout).expect("utf8");
        assert!(!stdout.is_empty(), "style {style} produced empty output");
        // Agent-only header must NOT appear under legacy styles.
        assert!(
            !stdout.starts_with("# sample.rs ("),
            "style {style} unexpectedly emitted agent header"
        );
    }
}
