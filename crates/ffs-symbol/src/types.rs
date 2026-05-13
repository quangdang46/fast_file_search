//! Core data types shared between symbol indexing, outlines and downstream
//! engine layers. Self-contained — no dependencies outside `std`.

use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// What kind of query the user issued.
#[derive(Debug, Clone)]
pub enum QueryType {
    FilePath(PathBuf),
    FilePathLine(PathBuf, usize),
    Glob(String),
    SymbolGlob(String),
    Symbol(String),
    /// Broad concept query — single lowercase word or multi-word phrase.
    Concept(String),
    /// Path-like or unclassified query — try symbol, then content fallback.
    Fallthrough(String),
}

/// Programming language carried through the type system so detection happens once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Lang {
    Rust,
    TypeScript,
    Tsx,
    JavaScript,
    Python,
    Go,
    Java,
    Scala,
    C,
    Cpp,
    Ruby,
    Php,
    Swift,
    Kotlin,
    CSharp,
    Elixir,
    Dockerfile,
    Make,
}

/// File type as detected by extension or filename.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileType {
    Code(Lang),
    Markdown,
    StructuredData,
    Tabular,
    Log,
    Other,
}

/// What the output contains — shown in the header bracket.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewMode {
    Full,
    Outline,
    /// Outline emitted because a `--full` request would exceed budget.
    OutlineCascade,
    /// Top-level signatures only — second cascade step when even outline overflows.
    Signatures,
    Keys,
    HeadTail,
    Empty,
    Generated,
    Binary,
    Error,
    Section,
    /// Outline of a section that exceeded the section token threshold.
    SectionOutline,
}

impl std::fmt::Display for ViewMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Full => write!(f, "full"),
            Self::Outline => write!(f, "outline"),
            Self::OutlineCascade => write!(f, "outline (full requested, over budget)"),
            Self::Signatures => write!(f, "signatures (full requested, over budget)"),
            Self::Keys => write!(f, "keys"),
            Self::HeadTail => write!(f, "head+tail"),
            Self::Empty => write!(f, "empty"),
            Self::Generated => write!(f, "generated — skipped"),
            Self::Binary => write!(f, "skipped"),
            Self::Error => write!(f, "error"),
            Self::Section => write!(f, "section"),
            Self::SectionOutline => write!(f, "section, outline (over limit)"),
        }
    }
}

/// A single search match, carrying enough context for ranking and display.
#[derive(Debug, Clone)]
pub struct Match {
    pub path: PathBuf,
    pub line: u32,
    pub text: String,
    pub is_definition: bool,
    pub exact: bool,
    pub file_lines: u32,
    pub mtime: SystemTime,
    /// Line range of the enclosing definition node (for expand).
    pub def_range: Option<(u32, u32)>,
    /// The defined symbol name (populated from AST during definition detection).
    pub def_name: Option<String>,
    /// Semantic weight for definition kinds. 0 for usages.
    pub def_weight: u16,
    /// For impl/implements matches: the trait or interface being implemented.
    pub impl_target: Option<String>,
    /// For neutral base-list matches such as C# `class X : Y`.
    pub base_target: Option<String>,
    /// Whether this match sits inside a comment or doc-comment node.
    pub in_comment: bool,
}

/// Assembled search results before formatting.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub query: String,
    pub scope: PathBuf,
    pub matches: Vec<Match>,
    pub total_found: usize,
    pub definitions: usize,
    pub usages: usize,
    pub comments: usize,
    pub has_more: bool,
    pub offset: usize,
}

/// A single entry in a code outline.
#[derive(Debug, Clone)]
pub struct OutlineEntry {
    pub kind: OutlineKind,
    pub name: String,
    pub start_line: u32,
    pub end_line: u32,
    pub signature: Option<String>,
    pub children: Vec<OutlineEntry>,
    pub doc: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutlineKind {
    Import,
    Function,
    Class,
    Struct,
    Interface,
    TypeAlias,
    Enum,
    Constant,
    Variable,
    ImmutableVariable,
    Export,
    Property,
    Module,
    TestSuite,
    TestCase,
}

/// Detect test files by path patterns.
pub fn is_test_file(path: &std::path::Path) -> bool {
    let s = path.to_string_lossy();
    s.contains(".test.") || s.contains(".spec.") || s.contains("__tests__/")
}

/// Tokens ≈ bytes / 4. Ceiling division, no float.
#[must_use]
pub fn estimate_tokens(byte_len: u64) -> u64 {
    byte_len.div_ceil(4)
}

/// UTF-8 safe string truncation. Never panics on multi-byte characters.
#[must_use]
pub fn truncate_str(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        // Walk back to a char boundary.
        let mut idx = max;
        while idx > 0 && !s.is_char_boundary(idx) {
            idx -= 1;
        }
        &s[..idx]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_str_handles_multibyte() {
        let s = "héllo";
        // First char 'h' is 1 byte, then 'é' is 2 bytes. Asking for 2 must
        // walk back to byte 1 to avoid splitting é.
        assert_eq!(truncate_str(s, 2), "h");
        assert_eq!(truncate_str(s, 3), "hé");
    }

    #[test]
    fn estimate_tokens_ceil() {
        assert_eq!(estimate_tokens(0), 0);
        assert_eq!(estimate_tokens(1), 1);
        assert_eq!(estimate_tokens(4), 1);
        assert_eq!(estimate_tokens(5), 2);
    }

    #[test]
    fn is_test_file_patterns() {
        assert!(is_test_file(std::path::Path::new("foo.test.ts")));
        assert!(is_test_file(std::path::Path::new("__tests__/bar.ts")));
        assert!(!is_test_file(std::path::Path::new("src/lib.rs")));
    }
}
