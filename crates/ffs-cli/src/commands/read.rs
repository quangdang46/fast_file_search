use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{anyhow, Result};
use clap::Parser;
use serde::Serialize;

use ffs_budget::cascade::{cascade_read, CascadeResult, OutlineLike, ViewMode};
use ffs_budget::{
    apply_preserving_footer, smart_truncate, AggressiveFilter, BudgetSplit, FilterLevel,
    FilterStrategy, MinimalFilter, NoFilter, TruncationOutcome,
};
use ffs_engine::{Engine, EngineConfig};
use ffs_symbol::detection;
use ffs_symbol::lang::detect_file_type;
use ffs_symbol::outline::get_outline_entries;
use ffs_symbol::types::{FileType, OutlineEntry};

// Adapter so `ffs_symbol::types::OutlineEntry` can be passed to cascade.
struct SymOutline<'a>(&'a OutlineEntry);

impl<'a> OutlineLike for SymOutline<'a> {
    fn name(&self) -> &str {
        &self.0.name
    }
    fn signature(&self) -> Option<&str> {
        self.0.signature.as_deref()
    }
    fn for_each_child(&self, f: &mut dyn FnMut(&dyn OutlineLike)) {
        for c in &self.0.children {
            f(&SymOutline(c));
        }
    }
}

fn as_outline_refs<'a>(entries: &'a [OutlineEntry]) -> Vec<Box<dyn OutlineLike + 'a>> {
    entries
        .iter()
        .map(|e| Box::new(SymOutline(e)) as Box<dyn OutlineLike + 'a>)
        .collect()
}

use crate::cli::OutputFormat;
use crate::commands::outline as outline_cmd;

#[derive(Debug, Parser)]
pub struct Args {
    /// File path to read; may be of the form `path:line` to focus a span.
    pub target: String,

    /// Token budget for the output (default 25000).
    /// Effective byte cap = tokens × ~85% body × 4 bytes/token. The remaining
    /// ~15% is reserved for the response envelope and the truncation footer.
    #[arg(long)]
    pub budget: Option<u64>,

    /// Filter intensity for `--full` reads: `none` keeps the file as-is,
    /// `minimal` (default) strips full-line comments, `aggressive` also
    /// strips block comments and collapses blank lines. Has no visible
    /// effect in the default outline mode (the outline contains no
    /// comments to strip).
    #[arg(long, default_value = "minimal")]
    pub filter: String,

    /// Return only the structural section (function/class/struct) that
    /// contains the line in `path:line`. Mutually exclusive with `--full`.
    #[arg(long, default_value_t = false, conflicts_with = "full")]
    pub section: bool,

    /// Force whole-file mode and return raw contents. Without this flag the
    /// ffs default is the agent-style outline.
    #[arg(long, default_value_t = false)]
    pub full: bool,

    /// Return only function/class signatures (no bodies).
    #[arg(long, default_value_t = false, conflicts_with = "full")]
    pub signatures: bool,

    /// JS/TS artifact mode: extract export anchors (ESM/CJS/AMD/UMD) for
    /// bundled or minified files. No-op for non-JS/TS targets.
    #[arg(long, default_value_t = false)]
    pub artifact: bool,
}

#[derive(Debug, Serialize)]
struct ReadOutput {
    path: String,
    mode: &'static str,
    body: String,
    kept_bytes: usize,
    footer_bytes: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    section: Option<SectionMeta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolved_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    view: Option<&'static str>,
}

#[derive(Debug, Serialize)]
struct ReadCandidates {
    needle: String,
    mode: &'static str,
    candidates: Vec<String>,
    total: usize,
}

#[derive(Debug, Serialize)]
struct SectionMeta {
    kind: String,
    name: String,
    start_line: u32,
    end_line: u32,
}

#[derive(Debug, Serialize)]
struct ArtifactOutput {
    path: String,
    mode: &'static str,
    total_lines: usize,
    anchors: Vec<ArtifactAnchorDto>,
}

#[derive(Debug, Serialize)]
struct ArtifactAnchorDto {
    line: u32,
    kind: &'static str,
    name: String,
}

fn artifact_text(p: &ArtifactOutput) -> String {
    let mut out = String::new();
    out.push_str(&format!("{} ({} lines)\n", p.path, p.total_lines));
    for a in &p.anchors {
        out.push_str(&format!("L{} [{}] {}\n", a.line, a.kind, a.name));
    }
    if p.anchors.is_empty() {
        out.push_str("[no artifact anchors found]\n");
    }
    out
}

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let level = filter_level(&args.filter);
    let budget = args.budget.unwrap_or(25_000);

    let (path_part, line) = parse_target(&args.target);
    let initial_path = if Path::new(path_part).is_absolute() {
        PathBuf::from(path_part)
    } else {
        root.join(path_part)
    };

    // Bare-filename auto-pick: when the literal path doesn't resolve and the
    // target has no separator, search the workspace by basename. Exactly one
    // exact hit → drill into it; more than one → emit a candidates list so
    // the caller can disambiguate.
    let (path, resolved_from) = match maybe_resolve_bare(root, path_part, &initial_path) {
        BareResolve::Found(p) => (p, Some(path_part.to_string())),
        BareResolve::Ambiguous(candidates) => {
            let payload = ReadCandidates {
                needle: path_part.to_string(),
                mode: "candidates",
                total: candidates.len(),
                candidates: candidates.into_iter().take(5).collect(),
            };
            return super::emit(format, &payload, candidates_text);
        }
        BareResolve::Skip => (initial_path, None),
    };

    // Generated-file guard: auto-switch to outline with a note unless user
    // explicitly asked for --full.
    let generated = is_generated_file(&path);
    let force_outline = generated && !args.full;

    // Routing:
    //   --section or `path:N`  → structural section read.
    //   --full                  → whole-file body (legacy default).
    //   no flags, no `:N`       → outline (B0 default for the engine layer).
    if args.section || (line.is_some() && !args.full) {
        let line =
            line.ok_or_else(|| anyhow!("--section requires the target to be in `path:line` form"))?;
        let mut payload = read_section(&path, line, level, budget)?;
        payload.resolved_from = resolved_from;
        return super::emit(format, &payload, section_text);
    }

    // --artifact: JS/TS export anchor extraction.
    if args.artifact {
        if let FileType::Code(lang) = detect_file_type(&path) {
            use ffs_symbol::types::Lang;
            if matches!(lang, Lang::JavaScript | Lang::TypeScript | Lang::Tsx) {
                let content = std::fs::read_to_string(&path)
                    .map_err(|e| anyhow!("failed to read {}: {e}", path.display()))?;
                let (anchors, total_lines) =
                    ffs_symbol::artifact::extract_artifact_anchors(&content);
                let payload = ArtifactOutput {
                    path: path.to_string_lossy().to_string(),
                    mode: "artifact",
                    total_lines,
                    anchors: anchors
                        .into_iter()
                        .map(|a| ArtifactAnchorDto {
                            line: a.line,
                            kind: a.kind,
                            name: a.name,
                        })
                        .collect(),
                };
                return super::emit(format, &payload, artifact_text);
            }
        }
    }

    // --signatures: signatures-only outline via the cascade renderer.
    if args.signatures {
        if let FileType::Code(lang) = detect_file_type(&path) {
            let content = std::fs::read_to_string(&path)
                .map_err(|e| anyhow!("failed to read {}: {e}", path.display()))?;
            let entries = get_outline_entries(&content, lang);
            let refs = as_outline_refs(&entries);
            let refs_slice: Vec<&dyn OutlineLike> = refs.iter().map(|b| b.as_ref()).collect();
            let body = ffs_budget::cascade::render_signatures(&refs_slice);
            let kept_bytes = body.len();
            let payload = ReadOutput {
                path: path.to_string_lossy().to_string(),
                mode: "signatures",
                body,
                kept_bytes,
                footer_bytes: 0,
                line: None,
                section: None,
                resolved_from,
                view: Some("signatures"),
            };
            return super::emit(format, &payload, |p| p.body.clone());
        }
    }

    if !args.full || force_outline {
        // Outline default (only for code files; non-code falls through to
        // full-body read so things like Markdown / JSON still emit content).
        if matches!(detect_file_type(&path), FileType::Code(_)) {
            let mut body = outline_cmd::render_agent(&path, path_part)?;
            if generated {
                body.insert_str(0, "[generated file]\n");
            }

            // Bug 12: honor `--budget` in outline mode. Apply the same
            // body-budget formula the engine uses for `--full`, falling back
            // to a signatures-only view when the outline doesn't fit. For
            // very small budgets (< ~100 tokens) `percent_budget` rounds
            // down to 0; treat that as "the user wants the smallest possible
            // payload" and aim for `total * 4` bytes instead.
            let split = BudgetSplit::default_for(budget);
            let body_budget_bytes = if split.body > 0 {
                (split.body * 4) as usize
            } else {
                (budget * 4) as usize
            };

            let (final_body, kept_bytes, footer_bytes, view) = if body.len() <= body_budget_bytes
            {
                let kept = body.len();
                (body, kept, 0usize, None)
            } else if let FileType::Code(lang) = detect_file_type(&path) {
                // Outline overflowed — try signatures-only.
                let content = std::fs::read_to_string(&path)
                    .map_err(|e| anyhow!("failed to read {}: {e}", path.display()))?;
                let entries = get_outline_entries(&content, lang);
                let refs = as_outline_refs(&entries);
                let refs_slice: Vec<&dyn OutlineLike> =
                    refs.iter().map(|b| b.as_ref()).collect();
                let sig_text = ffs_budget::cascade::render_signatures(&refs_slice);
                if sig_text.len() <= body_budget_bytes {
                    let kept = sig_text.len();
                    (sig_text, kept, 0usize, Some("signatures"))
                } else {
                    // Last-resort: clip with a truncation footer.
                    let footer = "[truncated to budget]\n";
                    let mut buf = String::new();
                    let outcome = apply_preserving_footer(
                        &mut buf,
                        body_budget_bytes,
                        footer,
                        |target, b| {
                            let take = body.len().min(b);
                            // Round down to a UTF-8 char boundary.
                            let take = (0..=take)
                                .rev()
                                .find(|i| body.is_char_boundary(*i))
                                .unwrap_or(0);
                            target.push_str(&body[..take]);
                            take
                        },
                    );
                    (buf, outcome.kept_bytes, outcome.footer_bytes, None)
                }
            } else {
                let kept = body.len();
                (body, kept, 0usize, None)
            };

            let payload = ReadOutput {
                path: path.to_string_lossy().to_string(),
                mode: "outline",
                body: final_body,
                kept_bytes,
                footer_bytes,
                line: None,
                section: None,
                resolved_from,
                view,
            };
            return super::emit(format, &payload, |p| p.body.clone());
        }
    }

    // --full or non-code target: whole-file read via the engine.
    let cfg = EngineConfig {
        filter_level: level,
        total_token_budget: budget,
        ..EngineConfig::default()
    };
    let engine = Engine::new(cfg);
    let res = engine.read(&path);

    // Bug 13: surface read errors on stderr with a non-zero exit code rather
    // than printing them on stdout and exiting 0.
    if res.is_error {
        return Err(anyhow!(
            "ffs read {}: {}",
            path.display(),
            res.body.trim_start_matches("[error reading file: ").trim_end_matches(']')
        ));
    }

    // For oversized code files, degrade to outline/signatures via cascade so the
    // agent gets useful structure instead of a truncated middle. Skip when the
    // file was detected as binary — there's nothing structural to render.
    let mut view: Option<&'static str> = None;
    let mut mode: &'static str = if res.is_binary { "binary" } else { "full" };
    let (mut body, kept_bytes, footer_bytes) = (
        res.body.clone(),
        res.outcome.kept_bytes,
        res.outcome.footer_bytes,
    );
    if !res.is_binary && res.outcome.dropped_lines > 0 {
        if let FileType::Code(lang) = detect_file_type(&path) {
            if let Ok(content) = std::fs::read_to_string(&path) {
                let entries = get_outline_entries(&content, lang);
                let refs = as_outline_refs(&entries);
                let refs_slice: Vec<&dyn OutlineLike> = refs.iter().map(|b| b.as_ref()).collect();
                let split = BudgetSplit::default_for(budget);
                let casc: CascadeResult = cascade_read(&content, &refs_slice, level, split);
                view = Some(match casc.mode {
                    ViewMode::Full => "full",
                    ViewMode::Outline => "outline",
                    ViewMode::Signatures => "signatures",
                });
                if !matches!(casc.mode, ViewMode::Full) {
                    body = casc.body;
                    mode = "cascade";
                }
            }
        }
    }
    if generated {
        body.insert_str(0, "[generated file]\n");
    }

    let payload = ReadOutput {
        path: res.path.to_string_lossy().to_string(),
        mode,
        body,
        kept_bytes,
        footer_bytes,
        line,
        section: None,
        resolved_from,
        view,
    };
    super::emit(format, &payload, |p| {
        let mut out = String::new();
        if let Some(l) = p.line {
            out.push_str(&format!("// {}:{}\n", p.path, l));
        }
        out.push_str(&p.body);
        out
    })
}

fn is_generated_file(path: &Path) -> bool {
    if path
        .file_name()
        .and_then(|n| n.to_str())
        .map(detection::is_generated_by_name)
        .unwrap_or(false)
    {
        return true;
    }
    // The generated-marker convention places markers in the file header; reading
    // a small prefix is sufficient and avoids streaming megabyte-sized files.
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    use std::io::Read;
    let mut buf = [0u8; 512];
    let n = file.read(&mut buf).unwrap_or(0);
    detection::is_generated_by_content(&buf[..n])
}

fn filter_level(s: &str) -> FilterLevel {
    match s {
        "none" => FilterLevel::None,
        "aggressive" => FilterLevel::Aggressive,
        _ => FilterLevel::Minimal,
    }
}

fn parse_target(target: &str) -> (&str, Option<u32>) {
    if let Some((p, rest)) = target.rsplit_once(':') {
        if let Ok(n) = rest.parse::<u32>() {
            if n > 0 {
                return (p, Some(n));
            }
        }
    }
    (target, None)
}

fn read_section(path: &Path, line: u32, level: FilterLevel, budget: u64) -> Result<ReadOutput> {
    let lang = match detect_file_type(path) {
        FileType::Code(l) => l,
        _ => return Err(anyhow!("not a code file: {}", path.display())),
    };

    let max_bytes_default = EngineConfig::default().max_bytes_per_result;
    let cfg = EngineConfig {
        filter_level: level,
        total_token_budget: budget,
        ..EngineConfig::default()
    };
    let engine = Engine::new(cfg);

    let content =
        std::fs::read_to_string(path).map_err(|e| anyhow!("read {}: {e}", path.display()))?;
    let mtime = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let outline = engine
        .handles
        .outlines
        .get_or_compute(path, mtime, &content, lang);

    let entry = deepest_containing(&outline, line)
        .ok_or_else(|| anyhow!("no structural section contains line {line}"))?;

    let slice = slice_lines(&content, entry.start_line, entry.end_line);

    let filter: Box<dyn FilterStrategy> = match level {
        FilterLevel::None => Box::new(NoFilter),
        FilterLevel::Minimal => Box::new(MinimalFilter),
        FilterLevel::Aggressive => Box::new(AggressiveFilter),
    };
    let filtered = filter.apply(&slice);

    let split = BudgetSplit::default_for(budget);
    let body_budget_bytes = (split.body * 4) as usize;
    let max_bytes = body_budget_bytes.min(max_bytes_default);

    let (body, outcome) = budgeted(&filtered, max_bytes);

    Ok(ReadOutput {
        path: path.to_string_lossy().to_string(),
        mode: "section",
        body,
        kept_bytes: outcome.kept_bytes,
        footer_bytes: outcome.footer_bytes,
        line: Some(line),
        section: Some(SectionMeta {
            kind: format!("{:?}", entry.kind).to_lowercase(),
            name: entry.name.clone(),
            start_line: entry.start_line,
            end_line: entry.end_line,
        }),
        resolved_from: None,
        view: None,
    })
}

enum BareResolve {
    Found(PathBuf),
    Ambiguous(Vec<String>),
    Skip,
}

const NON_PROD_SEGMENTS: &[&str] = &[
    "test",
    "tests",
    "__tests__",
    "spec",
    "specs",
    "vendor",
    "node_modules",
    "dist",
    "build",
    "target",
    ".git",
    "coverage",
    "tmp",
    "temp",
];

fn pick_primary(candidates: &[PathBuf], root: &Path) -> Option<PathBuf> {
    fn score(p: &Path, root: &Path) -> i32 {
        let rel = p.strip_prefix(root).unwrap_or(p);
        let depth = rel.components().count() as i32;
        let mut score = 0i32;
        let rel_str = rel.to_string_lossy();
        for bad in NON_PROD_SEGMENTS {
            if rel_str.contains(bad) {
                score -= 50;
            }
        }
        if rel_str.starts_with("src/") || rel_str.starts_with("lib/") {
            score += 20;
        }
        score += (10 - depth).max(0);
        score
    }
    if candidates.len() < 2 {
        return candidates.first().cloned();
    }
    let mut scored: Vec<(i32, &PathBuf)> = candidates.iter().map(|p| (score(p, root), p)).collect();
    scored.sort_by_key(|s| std::cmp::Reverse(s.0));
    let best = scored[0];
    if scored.len() > 1 {
        let gap = best.0 - scored[1].0;
        if gap < 10 {
            return None;
        }
    }
    Some(best.1.clone())
}

// Trigger only when target has no path separator and the literal path does
// not resolve. Exact-case basename match preferred; fall back to
// case-insensitive only when exact-case yields zero.
fn maybe_resolve_bare(root: &Path, target: &str, initial: &Path) -> BareResolve {
    if target.contains('/') || target.contains('\\') {
        return BareResolve::Skip;
    }
    if initial.exists() {
        return BareResolve::Skip;
    }
    if target.is_empty() {
        return BareResolve::Skip;
    }
    let files = super::walk_files(root);
    let mut exact: Vec<PathBuf> = files
        .iter()
        .filter(|p| p.file_name().and_then(|n| n.to_str()) == Some(target))
        .cloned()
        .collect();
    if exact.is_empty() {
        let lower = target.to_lowercase();
        exact = files
            .iter()
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.to_lowercase() == lower)
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
    }
    match exact.len() {
        0 => BareResolve::Skip,
        1 => BareResolve::Found(exact.remove(0)),
        _ => {
            // Deterministic ordering for ambiguous matches: shortest path
            // (likely the closest to root) first, then alphabetical.
            exact.sort_by(|a, b| {
                a.as_os_str()
                    .len()
                    .cmp(&b.as_os_str().len())
                    .then_with(|| a.cmp(b))
            });
            // Try primary pick heuristic before falling back to ambiguous list.
            if let Some(primary) = pick_primary(&exact, root) {
                return BareResolve::Found(primary);
            }
            BareResolve::Ambiguous(
                exact
                    .into_iter()
                    .map(|p| p.to_string_lossy().to_string())
                    .collect(),
            )
        }
    }
}

fn candidates_text(p: &ReadCandidates) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "ambiguous: `{}` matches {} files; pass one as the full path or pin with `path:line`.\n",
        p.needle, p.total
    ));
    for c in &p.candidates {
        out.push_str("  ");
        out.push_str(c);
        out.push('\n');
    }
    if p.total > p.candidates.len() {
        out.push_str(&format!(
            "  … {} more not shown\n",
            p.total - p.candidates.len()
        ));
    }
    out
}

fn deepest_containing(entries: &[OutlineEntry], line: u32) -> Option<OutlineEntry> {
    let mut best: Option<OutlineEntry> = None;
    for e in entries {
        if e.start_line <= line && line <= e.end_line {
            // Prefer a deeper child if one also contains the line.
            if let Some(child) = deepest_containing(&e.children, line) {
                best = Some(child);
            } else {
                best = Some(e.clone());
            }
        }
    }
    best
}

fn slice_lines(content: &str, start_line: u32, end_line: u32) -> String {
    let start = start_line.saturating_sub(1) as usize;
    let end = end_line as usize;
    let mut out = String::new();
    for (i, line) in content.lines().enumerate() {
        if i >= start && i < end {
            out.push_str(line);
            out.push('\n');
        }
        if i >= end {
            break;
        }
    }
    out
}

fn budgeted(filtered: &str, max_bytes: usize) -> (String, TruncationOutcome) {
    let mut buf = String::new();
    let footer = "[truncated to budget]\n";
    let outcome = if filtered.len() <= max_bytes {
        let (out, oc) = smart_truncate(filtered, max_bytes);
        buf.push_str(&out);
        oc
    } else {
        apply_preserving_footer(&mut buf, max_bytes, footer, |target, budget| {
            let take = filtered.len().min(budget);
            target.push_str(&filtered[..take]);
            take
        })
    };
    (buf, outcome)
}

fn section_text(p: &ReadOutput) -> String {
    let mut out = String::new();
    if let Some(s) = &p.section {
        out.push_str(&format!(
            "// {} {} (lines {}-{})\n",
            s.kind, s.name, s.start_line, s.end_line
        ));
    } else if let Some(l) = p.line {
        out.push_str(&format!("// {}:{}\n", p.path, l));
    }
    out.push_str(&p.body);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use ffs_symbol::types::OutlineKind;

    fn entry(
        kind: OutlineKind,
        name: &str,
        start: u32,
        end: u32,
        children: Vec<OutlineEntry>,
    ) -> OutlineEntry {
        OutlineEntry {
            kind,
            name: name.to_string(),
            start_line: start,
            end_line: end,
            signature: None,
            children,
            doc: None,
        }
    }

    #[test]
    fn bare_resolve_skips_when_target_has_separator() {
        let td = tempfile::tempdir().unwrap();
        let initial = td.path().join("dir/foo.rs");
        let r = maybe_resolve_bare(td.path(), "dir/foo.rs", &initial);
        assert!(matches!(r, BareResolve::Skip));
    }

    #[test]
    fn bare_resolve_skips_when_initial_exists() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("foo.rs"), "// hi").unwrap();
        let initial = td.path().join("foo.rs");
        let r = maybe_resolve_bare(td.path(), "foo.rs", &initial);
        assert!(matches!(r, BareResolve::Skip));
    }

    #[test]
    fn bare_resolve_single_exact_match_returns_found() {
        let td = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(td.path().join("src")).unwrap();
        std::fs::write(td.path().join("src/uniq.rs"), "fn x() {}").unwrap();
        let initial = td.path().join("uniq.rs");
        match maybe_resolve_bare(td.path(), "uniq.rs", &initial) {
            BareResolve::Found(p) => {
                assert!(p.ends_with("src/uniq.rs"));
            }
            _ => panic!("expected Found"),
        }
    }

    #[test]
    fn bare_resolve_multi_match_returns_ambiguous_sorted_shortest_first() {
        let td = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(td.path().join("a/b/c")).unwrap();
        std::fs::create_dir_all(td.path().join("z")).unwrap();
        std::fs::write(td.path().join("a/b/c/dup.rs"), "fn x() {}").unwrap();
        std::fs::write(td.path().join("z/dup.rs"), "fn x() {}").unwrap();
        let initial = td.path().join("dup.rs");
        match maybe_resolve_bare(td.path(), "dup.rs", &initial) {
            BareResolve::Ambiguous(v) => {
                assert_eq!(v.len(), 2);
                // shortest path first (length ordering is OS-agnostic).
                assert!(v[0].len() < v[1].len());
                assert!(Path::new(&v[0]).ends_with(Path::new("z/dup.rs")));
                assert!(Path::new(&v[1]).ends_with(Path::new("a/b/c/dup.rs")));
            }
            _ => panic!("expected Ambiguous"),
        }
    }

    // Case-insensitive fallback only meaningfully exercises when the literal
    // path doesn't resolve, which requires a case-sensitive filesystem. macOS
    // (APFS) and Windows (NTFS) default to case-insensitive: `initial.exists()`
    // already returns true and the auto-pick branch correctly short-circuits
    // to `Skip`, so we gate the fallback assertion to Linux only.
    #[cfg(target_os = "linux")]
    #[test]
    fn bare_resolve_case_insensitive_fallback_when_exact_zero() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("README.md"), "# hi").unwrap();
        let initial = td.path().join("readme.md");
        match maybe_resolve_bare(td.path(), "readme.md", &initial) {
            BareResolve::Found(p) => {
                assert_eq!(p.file_name().and_then(|n| n.to_str()), Some("README.md"));
            }
            _ => panic!("expected case-insensitive match"),
        }
    }

    #[test]
    fn bare_resolve_skip_when_no_matches() {
        let td = tempfile::tempdir().unwrap();
        let initial = td.path().join("missing.rs");
        let r = maybe_resolve_bare(td.path(), "missing.rs", &initial);
        assert!(matches!(r, BareResolve::Skip));
    }

    #[test]
    fn candidates_text_lists_paths_and_caps_at_five() {
        let p = ReadCandidates {
            needle: "x".into(),
            mode: "candidates",
            total: 7,
            candidates: (0..5).map(|i| format!("a/{i}.rs")).collect(),
        };
        let s = candidates_text(&p);
        assert!(s.contains("ambiguous: `x` matches 7 files"));
        assert!(s.contains("a/0.rs"));
        assert!(s.contains("a/4.rs"));
        assert!(s.contains("2 more not shown"));
    }

    #[test]
    fn parses_path_and_line_when_present() {
        assert_eq!(parse_target("foo.rs:42"), ("foo.rs", Some(42)));
        assert_eq!(parse_target("a/b/foo.rs:1"), ("a/b/foo.rs", Some(1)));
    }

    #[test]
    fn parses_path_only_when_no_colon_or_non_numeric_suffix() {
        assert_eq!(parse_target("foo.rs"), ("foo.rs", None));
        assert_eq!(parse_target("foo.rs:bar"), ("foo.rs:bar", None));
        assert_eq!(parse_target("foo.rs:0"), ("foo.rs:0", None));
    }

    #[test]
    fn deepest_containing_returns_innermost_match() {
        let outline = vec![entry(
            OutlineKind::Class,
            "Cls",
            1,
            50,
            vec![entry(OutlineKind::Function, "method", 10, 30, vec![])],
        )];
        let inner = deepest_containing(&outline, 20).expect("found");
        assert_eq!(inner.name, "method");
        let outer = deepest_containing(&outline, 5).expect("found");
        assert_eq!(outer.name, "Cls");
    }

    #[test]
    fn deepest_containing_returns_none_when_no_match() {
        let outline = vec![entry(OutlineKind::Function, "f", 10, 20, vec![])];
        assert!(deepest_containing(&outline, 5).is_none());
        assert!(deepest_containing(&outline, 25).is_none());
    }

    #[test]
    fn slice_lines_is_inclusive_of_both_ends() {
        let content = "L1\nL2\nL3\nL4\nL5\n";
        assert_eq!(slice_lines(content, 2, 4), "L2\nL3\nL4\n");
        assert_eq!(slice_lines(content, 1, 1), "L1\n");
        assert_eq!(slice_lines(content, 5, 5), "L5\n");
    }

    #[test]
    fn slice_lines_clamps_when_end_exceeds_file() {
        let content = "L1\nL2\n";
        assert_eq!(slice_lines(content, 1, 99), "L1\nL2\n");
    }

    #[test]
    fn generated_file_detects_by_name() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("bundle.min.js"), "// some code").unwrap();
        assert!(is_generated_file(&td.path().join("bundle.min.js")));
    }

    #[test]
    fn generated_file_detects_by_content() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("foo.rs"), "// @generated by tool\nfn x() {}").unwrap();
        assert!(is_generated_file(&td.path().join("foo.rs")));
    }

    #[test]
    fn generated_file_skips_non_generated() {
        let td = tempfile::tempdir().unwrap();
        std::fs::write(td.path().join("main.rs"), "fn main() {}").unwrap();
        assert!(!is_generated_file(&td.path().join("main.rs")));
    }
}
