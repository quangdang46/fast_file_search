//! Heuristic query classification.
//!
//! Decides which sub-engine should handle a free-form user query without
//! requiring the user to specify a subcommand.

use std::path::Path;

use ffs_symbol::types::QueryType;

/// A classified query, retaining the original input plus any normalized form.
#[derive(Debug, Clone)]
pub struct ClassifiedQuery {
    pub raw: String,
    pub query: QueryType,
}

/// Classify a query string into one of the [`QueryType`] arms.
///
/// Order of checks:
/// 1. `path/to/file:line` → `FilePathLine`
/// 2. Exists on disk under `cwd` → `FilePath`
/// 3. Glob with `**`/`?`/`*` → `Glob` (or `SymbolGlob` if no path separator)
/// 4. Single identifier (CamelCase or snake_case, no whitespace) → `Symbol`
/// 5. Lowercase phrase → `Concept`
/// 6. Otherwise → `Fallthrough`
#[must_use]
pub fn classify_query(raw: &str, cwd: &Path) -> ClassifiedQuery {
    let trimmed = raw.trim();

    if let Some((path_part, line_str)) = parse_path_line(trimmed) {
        if let Ok(line) = line_str.parse::<usize>() {
            return ClassifiedQuery {
                raw: raw.to_string(),
                query: QueryType::FilePathLine(cwd.join(path_part), line),
            };
        }
    }

    if has_path_separator(trimmed) {
        let p = cwd.join(trimmed);
        if p.exists() {
            return ClassifiedQuery {
                raw: raw.to_string(),
                query: QueryType::FilePath(p),
            };
        }
    }

    if is_glob_pattern(trimmed) {
        let q = if trimmed.contains('/') {
            QueryType::Glob(trimmed.to_string())
        } else {
            QueryType::SymbolGlob(trimmed.to_string())
        };
        return ClassifiedQuery {
            raw: raw.to_string(),
            query: q,
        };
    }

    if is_likely_symbol(trimmed) {
        return ClassifiedQuery {
            raw: raw.to_string(),
            query: QueryType::Symbol(trimmed.to_string()),
        };
    }

    if is_likely_concept(trimmed) {
        return ClassifiedQuery {
            raw: raw.to_string(),
            query: QueryType::Concept(trimmed.to_string()),
        };
    }

    ClassifiedQuery {
        raw: raw.to_string(),
        query: QueryType::Fallthrough(trimmed.to_string()),
    }
}

fn parse_path_line(s: &str) -> Option<(&str, &str)> {
    // Match `path:line` but only when `path` looks path-like (has dot or slash).
    let last_colon = s.rfind(':')?;
    let (path, line) = s.split_at(last_colon);
    let line = &line[1..];
    if line.is_empty() || !line.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if !path.contains('/') && !path.contains('.') {
        return None;
    }
    Some((path, line))
}

fn has_path_separator(s: &str) -> bool {
    s.contains('/') || s.contains('\\')
}

fn is_glob_pattern(s: &str) -> bool {
    s.contains("**") || s.contains('*') || s.contains('?') || s.contains('[')
}

fn is_likely_symbol(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    if s.contains(char::is_whitespace) {
        return false;
    }
    if s.contains(['/', '\\']) {
        return false;
    }
    let alnum_or_underscore = s.chars().all(|c| c.is_alphanumeric() || c == '_');
    if !alnum_or_underscore {
        return false;
    }
    let has_upper = s.chars().any(|c| c.is_uppercase());
    let has_underscore = s.contains('_');
    has_upper || has_underscore || s.len() >= 4
}

fn is_likely_concept(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    s.split_whitespace().count() >= 2 && s.chars().all(|c| c.is_alphanumeric() || c.is_whitespace())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_file_path_line() {
        let cwd = std::env::current_dir().unwrap();
        let cq = classify_query("src/lib.rs:42", &cwd);
        assert!(matches!(cq.query, QueryType::FilePathLine(_, 42)));
    }

    #[test]
    fn classify_glob() {
        let cwd = std::env::current_dir().unwrap();
        let cq = classify_query("**/*.rs", &cwd);
        assert!(matches!(cq.query, QueryType::Glob(_)));
    }

    #[test]
    fn classify_camelcase_symbol() {
        let cwd = std::env::current_dir().unwrap();
        let cq = classify_query("MyClass", &cwd);
        assert!(matches!(cq.query, QueryType::Symbol(_)));
    }

    #[test]
    fn classify_snake_case_symbol() {
        let cwd = std::env::current_dir().unwrap();
        let cq = classify_query("my_function", &cwd);
        assert!(matches!(cq.query, QueryType::Symbol(_)));
    }

    #[test]
    fn classify_concept_phrase() {
        let cwd = std::env::current_dir().unwrap();
        let cq = classify_query("find symbols quickly", &cwd);
        assert!(matches!(cq.query, QueryType::Concept(_)));
    }
}
