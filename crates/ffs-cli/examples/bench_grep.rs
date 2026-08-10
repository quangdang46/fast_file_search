//! End-to-end grep benchmark: current CLI self-scan vs engine `FilePicker`.
//!
//! Usage:
//!   cargo run -p ffs-cli --example bench_grep -- <repo> <needle> [--regex] [--warm]
//!
//! Measures wall-clock latency of the full user-visible operation (walk +
//! prefilter + search + collect) for three configurations:
//!   1. cli-selfscan     — current `ffs grep` path: walk_files + optional
//!                         bigram cache prefilter + rayon SIMD scan.
//!   2. engine           — FilePicker::new + collect_files + picker.grep (no
//!                         content index / bigram).
//!   3. engine+index     — same, but with enable_content_indexing so the
//!                         engine builds its own bigram during scan.
//!
//! Output: per-config median of N runs, plus which is fastest. The decision
//! gate (owner): switch CLI to engine ONLY if engine ≤ cli-selfscan (ideally
//! clearly faster). Bigram must NOT be dropped.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ffs_search::grep::{GrepMode, GrepSearchOptions, parse_grep_query};
use ffs_search::file_picker::{FilePicker, FilePickerOptions};

/// Load the on-disk bigram cache (`<root>/.ffs/bigram.postcard.zst`) and
/// filter `needle` down to candidate files. Returns None when the cache is
/// absent/unreadable/invalid or the pattern has no discriminative bigrams —
/// caller falls back to a full walk. Replicates `CacheDir::load_bigram_index`
/// + `GrepBigram::filter` (the modules are bin-private, so inline here).
fn load_bigram_candidates(root: &Path, needle: &[u8]) -> Option<Vec<PathBuf>> {
    if needle.len() < 2 {
        return None;
    }
    let path = root.join(".ffs").join("bigram.postcard.zst");
    let bytes = std::fs::read(&path).ok()?;
    let decompressed = zstd::stream::decode_all(&bytes[..]).ok()?;
    // GrepBigram is serde; decode as a map-like struct. We can't name the
    // bin-private type, so decode via the public serde layout used by the
    // CLI cache: { paths: Vec<PathBuf>, posting: HashMap<u16, Vec<u64>> }.
    #[derive(serde::Deserialize)]
    struct RawBigram {
        paths: Vec<PathBuf>,
        posting: std::collections::HashMap<u16, Vec<u64>>,
    }
    let idx: RawBigram = postcard::from_bytes(&decompressed).ok()?;
    if idx.paths.is_empty() {
        return None;
    }
    let n = idx.paths.len();
    let words = n.div_ceil(64).max(1);
    let mut candidates = vec![u64::MAX; words];
    if !n.is_multiple_of(64) {
        let last = words - 1;
        candidates[last] = (1u64 << (n % 64)) - 1;
    }
    let mut had_bigram = false;
    for w in needle.windows(2) {
        let a = w[0];
        let b = w[1];
        if (32..=126).contains(&a) && (32..=126).contains(&b) {
            let key = ((a.to_ascii_lowercase() as u16) << 8) | (b.to_ascii_lowercase() as u16);
            match idx.posting.get(&key) {
                Some(bs) => {
                    for (c, x) in candidates.iter_mut().zip(bs.iter()) {
                        *c &= *x;
                    }
                    had_bigram = true;
                }
                None => return Some(Vec::new()),
            }
        }
    }
    if !had_bigram {
        return None;
    }
    let mut survivors: Vec<PathBuf> = Vec::new();
    for (i, p) in idx.paths.iter().enumerate() {
        let word = i / 64;
        if candidates[word] & (1u64 << (i % 64)) != 0 {
            survivors.push(p.clone());
        }
    }
    Some(survivors)
}

/// The CLI's own self-scan grep (copied from commands/grep.rs's hot path,
/// minus output formatting) so we measure the search itself, not printing.
///
/// Mirrors the real CLI: literal patterns try the on-disk bigram cache
/// (`<root>/.ffs/bigram.postcard.zst`) as a prefilter; on miss it falls back
/// to scanning every walked file.
fn cli_selfscan(root: &Path, needle: &str, use_regex: bool) -> usize {
    let matcher = build_cli_matcher(needle, use_regex);
    let files: Vec<PathBuf> = match &matcher {
        CliMatcher::Literal { needle, .. } => load_bigram_candidates(root, needle)
            .unwrap_or_else(|| crate_walk_files(root)),
        _ => crate_walk_files(root),
    };
    let mut hits = 0usize;
    for path in files {
        let Ok(content) = std::fs::read(&path) else { continue };
        let probe = &content[..content.len().min(8 * 1024)];
        if probe.contains(&0u8) {
            continue;
        }
        for (off, _end) in matcher.find_iter(&content) {
            let _ = off;
            hits += 1;
            // Stop early like the real CLI does at limit=200? No — count all
            // for a fair cost comparison; the real CLI early-exits but the
            // engine also caps via max_matches_per_file. Keep counting.
        }
    }
    hits
}

/// Build a matcher equivalent to commands/grep.rs's Matcher.
enum CliMatcher {
    Literal { needle: Vec<u8>, case_insensitive: bool },
    Regex(regex::bytes::Regex),
}

impl CliMatcher {
    fn find_iter<'a>(
        &'a self,
        haystack: &'a [u8],
    ) -> Box<dyn Iterator<Item = (usize, usize)> + 'a> {
        match self {
            CliMatcher::Literal { needle, case_insensitive } => {
                if *case_insensitive {
                    let needle = needle.clone();
                    let lower: Vec<u8> =
                        haystack.iter().map(|b| b.to_ascii_lowercase()).collect();
                    let nlen = needle.len();
                    let finder = memchr::memmem::Finder::new(&needle).into_owned();
                    let positions: Vec<(usize, usize)> =
                        finder.find_iter(&lower).map(|p| (p, p + nlen)).collect();
                    Box::new(positions.into_iter())
                } else {
                    let nlen = needle.len();
                    let finder = memchr::memmem::Finder::new(needle.as_slice());
                    Box::new(
                        finder
                            .find_iter(haystack)
                            .map(|p| (p, p + nlen))
                            .collect::<Vec<_>>()
                            .into_iter(),
                    )
                }
            }
            CliMatcher::Regex(re) => Box::new(
                re.find_iter(haystack)
                    .map(|m| (m.start(), m.end()))
                    .collect::<Vec<_>>()
                    .into_iter(),
            ),
        }
    }
}

fn build_cli_matcher(needle: &str, use_regex: bool) -> CliMatcher {
    let smart_case_sensitive = needle.chars().any(|c| c.is_uppercase());
    if use_regex {
        let re = regex::bytes::RegexBuilder::new(needle)
            .case_insensitive(!smart_case_sensitive)
            .multi_line(true)
            .build()
            .expect("regex");
        CliMatcher::Regex(re)
    } else {
        let needle_bytes = if smart_case_sensitive {
            needle.as_bytes().to_vec()
        } else {
            needle.to_lowercase().into_bytes()
        };
        CliMatcher::Literal {
            needle: needle_bytes,
            case_insensitive: !smart_case_sensitive,
        }
    }
}

/// Walk files with the same ignore-based walker the CLI uses.
fn crate_walk_files(root: &Path) -> Vec<PathBuf> {
    use ignore::WalkState;
    use std::sync::Mutex;
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(2)
        .min(8);
    let out: Mutex<Vec<PathBuf>> = Mutex::new(Vec::with_capacity(1024));
    let walker = ignore::WalkBuilder::new(root)
        .standard_filters(true)
        .follow_links(false)
        .threads(threads)
        .build_parallel();
    walker.run(|| {
        let out = &out;
        Box::new(move |entry| {
            if let Ok(e) = entry {
                if e.file_type().is_some_and(|t| t.is_file()) {
                    if let Ok(mut guard) = out.lock() {
                        guard.push(e.into_path());
                    }
                }
            }
            WalkState::Continue
        })
    });
    out.into_inner().unwrap_or_default()
}

fn engine_grep(
    root: &Path,
    needle: &str,
    use_regex: bool,
    content_indexing: bool,
) -> usize {
    let mut picker = FilePicker::new(FilePickerOptions {
        base_path: root.to_string_lossy().into_owned(),
        watch: false,
        enable_content_indexing: content_indexing,
        ..Default::default()
    })
    .expect("picker");
    picker.collect_files().expect("collect");

    let q = parse_grep_query(needle);
    let mode = if use_regex { GrepMode::Regex } else { GrepMode::PlainText };
    let options = GrepSearchOptions {
        max_file_size: 10 * 1024 * 1024,
        max_matches_per_file: 0,
        smart_case: true,
        file_offset: 0,
        page_limit: usize::MAX,
        mode,
        time_budget_ms: 0,
        before_context: 0,
        after_context: 0,
        classify_definitions: false,
        trim_whitespace: false,
        abort_signal: None,
    };
    let result = picker.grep(&q, &options);
    result.matches.len()
}

fn median(v: &mut [Duration]) -> Duration {
    v.sort();
    v[v.len() / 2]
}

fn run_n<F>(n: usize, f: F) -> Vec<Duration>
where
    F: Fn() -> usize,
{
    // Warm up once (FS cache), then measure.
    let _ = f();
    let mut times = Vec::with_capacity(n);
    for _ in 0..n {
        let t = Instant::now();
        let _ = f();
        times.push(t.elapsed());
    }
    times
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: bench_grep <repo> <needle> [--regex] [--warm] [--compare-regex]");
        std::process::exit(2);
    }
    let root = PathBuf::from(&args[0]);
    let needle = &args[1];
    let use_regex = args.iter().any(|a| a == "--regex");
    let warm = args.iter().any(|a| a == "--warm");
    let compare_regex = args.iter().any(|a| a == "--compare-regex");
    let runs = if warm { 10 } else { 3 };

    eprintln!(
        "bench_grep: repo={} needle={:?} regex={} compare_regex={} runs={}",
        root.display(),
        needle,
        use_regex,
        compare_regex,
        runs
    );

    // Regex vs plaintext mode comparison (Track B addendum / task #3): the same
    // needle driven through the CLI's two matchers — literal (memmem SIMD) vs
    // regex (regex::bytes::Regex over the whole buffer, no candidate-line
    // prefilter). This measures the gap that a literal prefilter could close.
    if compare_regex {
        // Literal: escaped needle so both match the same text.
        let literal_needle = regex::escape(needle);
        let mut lit = run_n(runs, || cli_selfscan(&root, &literal_needle, false));
        let mut re = run_n(runs, || cli_selfscan(&root, needle, true));
        let l = median(&mut lit);
        let r = median(&mut re);
        println!("matcher          median     min        max");
        println!("literal(memmem)  {:>9}  {:>9}  {:>9}", fmt(l), fmt(lit[0]), fmt(*lit.iter().max().unwrap()));
        println!("regex            {:>9}  {:>9}  {:>9}", fmt(r), fmt(re[0]), fmt(*re.iter().max().unwrap()));
        let ratio = l.as_secs_f64().max(1e-9);
        println!("\nregex/literal: {:.2}x (regex {}x slower)", r.as_secs_f64() / ratio, (r.as_secs_f64() / ratio));
        return;
    }

    // Config 1: CLI self-scan (current path).
    let mut cli = run_n(runs, || cli_selfscan(&root, needle, use_regex));
    // Config 2: engine, no content index.
    let mut eng = run_n(runs, || engine_grep(&root, needle, use_regex, false));
    // Config 3: engine with content indexing (builds bigram).
    let mut eng_idx = run_n(runs, || engine_grep(&root, needle, use_regex, true));

    let c = median(&mut cli);
    let e = median(&mut eng);
    let ei = median(&mut eng_idx);
    let (cmin, cmax) = (cli[0], *cli.iter().max().unwrap());
    let (emin, emax) = (eng[0], *eng.iter().max().unwrap());
    let (eimin, eimax) = (eng_idx[0], *eng_idx.iter().max().unwrap());

    println!("config           median     min        max");
    println!("cli-selfscan     {:>9}  {:>9}  {:>9}", fmt(c), fmt(cmin), fmt(cmax));
    println!("engine           {:>9}  {:>9}  {:>9}", fmt(e), fmt(emin), fmt(emax));
    println!("engine+index     {:>9}  {:>9}  {:>9}", fmt(ei), fmt(eimin), fmt(eimax));

    let base = c.as_secs_f64().max(1e-9);
    println!("\nengine vs cli:       {:.2}x", e.as_secs_f64() / base);
    println!("engine+index vs cli: {:.2}x", ei.as_secs_f64() / base);
    if e <= c {
        println!("GATE: engine <= cli-selfscan → engine switch viable");
    } else {
        println!("GATE: engine > cli-selfscan → keep CLI self-scan (no switch)");
    }
}

fn fmt(d: Duration) -> String {
    if d.as_secs_f64() >= 1.0 {
        format!("{:.3}s", d.as_secs_f64())
    } else {
        format!("{:.2}ms", d.as_secs_f64() * 1000.0)
    }
}
