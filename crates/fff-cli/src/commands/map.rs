//! `scry map` — render the workspace as a tree annotated with file count and
//! estimated token weight at each directory.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;
use clap::Parser;
use ignore::WalkBuilder;
use serde::Serialize;

use crate::cli::OutputFormat;

#[derive(Debug, Parser)]
pub struct Args {
    /// Maximum tree depth to render. Beyond this, directories show as a
    /// single summary line with the total file count and token estimate.
    #[arg(long, default_value_t = 3)]
    pub depth: usize,

    /// Skip files larger than this many bytes when computing the token
    /// estimate (still counted in file totals). Default = 1 MiB.
    #[arg(long, default_value_t = 1_048_576)]
    pub max_file_bytes: u64,

    /// Approximate bytes-per-token used to convert raw file size into an
    /// LLM token estimate. Default = 4 (a common rule of thumb for English
    /// + code mixed corpora).
    #[arg(long, default_value_t = 4)]
    pub bytes_per_token: u64,
}

#[derive(Debug, Serialize, Clone)]
pub struct MapNode {
    pub name: String,
    pub is_dir: bool,
    pub bytes: u64,
    pub est_tokens: u64,
    pub file_count: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<MapNode>,
    /// True when children were elided because we hit `--depth`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

#[derive(Debug, Serialize)]
struct MapOutput {
    root: String,
    total_files: usize,
    total_bytes: u64,
    total_est_tokens: u64,
    tree: MapNode,
}

pub fn run(args: Args, root: &Path, format: OutputFormat) -> Result<()> {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let bytes_per_token = args.bytes_per_token.max(1);
    let tree = build_tree(&root, &args, bytes_per_token);
    let payload = MapOutput {
        root: root.to_string_lossy().to_string(),
        total_files: tree.file_count,
        total_bytes: tree.bytes,
        total_est_tokens: tree.est_tokens,
        tree,
    };
    super::emit(format, &payload, render_text)
}

fn build_tree(root: &Path, args: &Args, bytes_per_token: u64) -> MapNode {
    let mut by_dir: BTreeMap<std::path::PathBuf, DirAcc> = BTreeMap::new();
    by_dir.insert(root.to_path_buf(), DirAcc::default());

    for entry in WalkBuilder::new(root)
        .standard_filters(true)
        .follow_links(false)
        .build()
        .flatten()
    {
        let Some(ftype) = entry.file_type() else {
            continue;
        };
        if !ftype.is_file() {
            if ftype.is_dir() {
                by_dir.entry(entry.path().to_path_buf()).or_default();
            }
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let size = meta.len();
        let counted_bytes = size.min(args.max_file_bytes);
        let tokens = counted_bytes.div_ceil(bytes_per_token);
        let path = entry.path();
        let parent = path.parent().unwrap_or(root).to_path_buf();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();

        let acc = by_dir.entry(parent).or_default();
        acc.files.push(FileEntry {
            name,
            bytes: size,
            est_tokens: tokens,
        });
    }

    fold_tree(root, &mut by_dir, args.depth, 0)
}

#[derive(Debug, Default)]
struct DirAcc {
    files: Vec<FileEntry>,
}

#[derive(Debug, Clone)]
struct FileEntry {
    name: String,
    bytes: u64,
    est_tokens: u64,
}

fn fold_tree(
    dir: &Path,
    by_dir: &mut BTreeMap<std::path::PathBuf, DirAcc>,
    max_depth: usize,
    cur_depth: usize,
) -> MapNode {
    let acc = by_dir.remove(dir).unwrap_or_default();
    let dir_name = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| dir.to_string_lossy().to_string());

    let mut total_bytes = 0u64;
    let mut total_tokens = 0u64;
    let mut total_files = 0usize;
    let mut child_files: Vec<MapNode> = Vec::new();
    for f in &acc.files {
        total_bytes += f.bytes;
        total_tokens += f.est_tokens;
        total_files += 1;
        child_files.push(MapNode {
            name: f.name.clone(),
            is_dir: false,
            bytes: f.bytes,
            est_tokens: f.est_tokens,
            file_count: 1,
            children: Vec::new(),
            truncated: false,
        });
    }

    let subdir_paths: Vec<std::path::PathBuf> = by_dir
        .keys()
        .filter(|p| p.parent() == Some(dir))
        .cloned()
        .collect();

    let mut child_dirs: Vec<MapNode> = Vec::new();
    for sd in subdir_paths {
        let node = fold_tree(&sd, by_dir, max_depth, cur_depth + 1);
        total_bytes += node.bytes;
        total_tokens += node.est_tokens;
        total_files += node.file_count;
        child_dirs.push(node);
    }

    let mut children: Vec<MapNode> = Vec::new();
    let truncated = cur_depth >= max_depth;
    if !truncated {
        children.append(&mut child_dirs);
        children.append(&mut child_files);
        children.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.cmp(&b.name),
        });
    }

    MapNode {
        name: dir_name,
        is_dir: true,
        bytes: total_bytes,
        est_tokens: total_tokens,
        file_count: total_files,
        children,
        truncated,
    }
}

fn render_text(p: &MapOutput) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{}  ({} files, {} bytes, ~{} tokens)\n",
        p.root,
        p.total_files,
        humanize_bytes(p.total_bytes),
        humanize_tokens(p.total_est_tokens)
    ));
    let last = p.tree.children.len().saturating_sub(1);
    for (i, child) in p.tree.children.iter().enumerate() {
        render_node(child, "", i == last, &mut out);
    }
    out
}

fn render_node(node: &MapNode, prefix: &str, is_last: bool, out: &mut String) {
    let connector = if is_last { "└── " } else { "├── " };
    let suffix = if node.is_dir {
        let core = format!(
            "{}/  ({} files, ~{} tokens)",
            node.name,
            node.file_count,
            humanize_tokens(node.est_tokens)
        );
        if node.truncated {
            format!("{core}  …")
        } else {
            core
        }
    } else {
        format!(
            "{}  (~{} tokens)",
            node.name,
            humanize_tokens(node.est_tokens)
        )
    };
    out.push_str(&format!("{prefix}{connector}{suffix}\n"));

    if node.truncated {
        return;
    }
    let extension = if is_last { "    " } else { "│   " };
    let child_prefix = format!("{prefix}{extension}");
    let last = node.children.len().saturating_sub(1);
    for (i, child) in node.children.iter().enumerate() {
        render_node(child, &child_prefix, i == last, out);
    }
}

fn humanize_bytes(b: u64) -> String {
    const K: u64 = 1024;
    if b < K {
        format!("{b}B")
    } else if b < K * K {
        format!("{:.1}K", b as f64 / K as f64)
    } else if b < K * K * K {
        format!("{:.1}M", b as f64 / (K * K) as f64)
    } else {
        format!("{:.1}G", b as f64 / (K * K * K) as f64)
    }
}

fn humanize_tokens(t: u64) -> String {
    if t < 1_000 {
        t.to_string()
    } else if t < 1_000_000 {
        format!("{:.1}k", t as f64 / 1_000.0)
    } else {
        format!("{:.1}M", t as f64 / 1_000_000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(path: &Path, body: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("dirs");
        }
        fs::write(path, body).expect("write");
    }

    #[test]
    fn map_aggregates_bytes_and_tokens_by_directory() {
        let dir = tempfile::tempdir().expect("tmp");
        let root = dir.path();
        write(&root.join("a/foo.rs"), "fn foo() {}\n");
        write(&root.join("a/b/bar.rs"), "fn bar() {}\nfn baz() {}\n");
        write(&root.join("c/qux.rs"), "fn qux() {}\n");

        let args = Args {
            depth: 5,
            max_file_bytes: 1_048_576,
            bytes_per_token: 4,
        };
        let tree = build_tree(root, &args, 4);

        assert!(tree.is_dir);
        assert_eq!(tree.file_count, 3);
        assert!(tree.bytes > 0);
        let total: u64 = ["a/foo.rs", "a/b/bar.rs", "c/qux.rs"]
            .iter()
            .map(|p| fs::metadata(root.join(p)).unwrap().len())
            .sum();
        assert_eq!(tree.bytes, total);
    }

    #[test]
    fn map_directories_sort_before_files() {
        let dir = tempfile::tempdir().expect("tmp");
        let root = dir.path();
        write(&root.join("z_dir/inside.rs"), "x");
        write(&root.join("a_file.rs"), "x");

        let args = Args {
            depth: 5,
            max_file_bytes: 1_048_576,
            bytes_per_token: 4,
        };
        let tree = build_tree(root, &args, 4);
        assert!(tree.children.len() >= 2);
        assert!(tree.children[0].is_dir);
        assert!(!tree.children[1].is_dir);
    }

    #[test]
    fn map_depth_limit_truncates_children() {
        let dir = tempfile::tempdir().expect("tmp");
        let root = dir.path();
        write(&root.join("a/b/c/d.rs"), "x");

        let args = Args {
            depth: 1,
            max_file_bytes: 1_048_576,
            bytes_per_token: 4,
        };
        let tree = build_tree(root, &args, 4);
        let a = tree
            .children
            .iter()
            .find(|c| c.name == "a")
            .expect("a/ exists");
        assert!(a.truncated);
        assert!(a.children.is_empty(), "should be elided at depth=1");
        assert_eq!(a.file_count, 1, "but file_count still aggregates totals");
    }

    #[test]
    fn map_max_file_bytes_caps_token_estimate() {
        let dir = tempfile::tempdir().expect("tmp");
        let root = dir.path();
        let big = "x".repeat(8192);
        write(&root.join("big.rs"), &big);

        let args = Args {
            depth: 5,
            max_file_bytes: 1024,
            bytes_per_token: 4,
        };
        let tree = build_tree(root, &args, 4);
        let big_node = tree
            .children
            .iter()
            .find(|c| c.name == "big.rs")
            .expect("big.rs in tree");
        // Estimate is computed from min(size, max_file_bytes), so 1024/4 = 256.
        assert_eq!(big_node.est_tokens, 256);
        // But the raw byte count reflects the real on-disk size.
        assert_eq!(big_node.bytes, 8192);
    }
}
