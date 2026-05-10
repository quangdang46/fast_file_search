//! Did-you-mean fallback for `scry symbol`.
//!
//! When `lookup_exact` finds zero hits, try alternate names in three layers:
//!
//! 1. Naming-convention variants (snake_case ↔ camelCase ↔ PascalCase ↔
//!    kebab-case ↔ SCREAMING_SNAKE) hit `lookup_exact` directly.
//! 2. Prefix fallback via `lookup_prefix(query)` for the case where the user
//!    typed a leading substring of a longer symbol.
//! 3. Frizbee fuzzy match against all indexed symbol names — typo tolerance.
//!
//! Each layer short-circuits the next: only run fuzzy if the cheaper layers
//! produced nothing. Output is capped to `max_suggestions` total entries.

use std::collections::BTreeSet;

use serde::Serialize;

use fff_engine::Engine;
use fff_symbol::symbol_index::SymbolLocation;

const MAX_SUGGESTIONS: usize = 5;
const FUZZY_MAX_TYPOS: u16 = 2;

#[derive(Debug, Serialize)]
pub struct Suggestion {
    pub name: String,
    pub source: &'static str,
    pub hits: Vec<SuggestionHit>,
}

#[derive(Debug, Serialize)]
pub struct SuggestionHit {
    pub path: String,
    pub line: u32,
    pub end_line: u32,
    pub kind: String,
    pub weight: u16,
}

impl SuggestionHit {
    fn from_loc(loc: SymbolLocation) -> Self {
        Self {
            path: loc.path.to_string_lossy().to_string(),
            line: loc.line,
            end_line: loc.end_line,
            kind: loc.kind,
            weight: loc.weight,
        }
    }
}

pub fn suggest(engine: &Engine, query: &str) -> Vec<Suggestion> {
    if query.is_empty() {
        return Vec::new();
    }

    let mut out: Vec<Suggestion> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    seen.insert(query.to_string());

    for variant in case_variants(query) {
        if seen.contains(&variant) {
            continue;
        }
        let hits: Vec<SuggestionHit> = engine
            .handles
            .symbols
            .lookup_exact(&variant)
            .into_iter()
            .map(SuggestionHit::from_loc)
            .collect();
        if !hits.is_empty() {
            seen.insert(variant.clone());
            out.push(Suggestion {
                name: variant,
                source: "case",
                hits,
            });
            if out.len() >= MAX_SUGGESTIONS {
                return out;
            }
        }
    }

    if out.is_empty() {
        for (name, loc) in engine.handles.symbols.lookup_prefix(query) {
            if seen.contains(&name) {
                if let Some(s) = out.iter_mut().find(|s| s.name == name) {
                    s.hits.push(SuggestionHit::from_loc(loc));
                }
                continue;
            }
            seen.insert(name.clone());
            out.push(Suggestion {
                name,
                source: "prefix",
                hits: vec![SuggestionHit::from_loc(loc)],
            });
            if out.len() >= MAX_SUGGESTIONS {
                return out;
            }
        }
    }

    if out.is_empty() {
        let names = engine.handles.symbols.names();
        for fuzzy_name in fuzzy_candidates(query, &names, MAX_SUGGESTIONS) {
            if seen.contains(&fuzzy_name) {
                continue;
            }
            let hits: Vec<SuggestionHit> = engine
                .handles
                .symbols
                .lookup_exact(&fuzzy_name)
                .into_iter()
                .map(SuggestionHit::from_loc)
                .collect();
            if hits.is_empty() {
                continue;
            }
            seen.insert(fuzzy_name.clone());
            out.push(Suggestion {
                name: fuzzy_name,
                source: "fuzzy",
                hits,
            });
            if out.len() >= MAX_SUGGESTIONS {
                return out;
            }
        }
    }

    out
}

/// Generate naming-convention variants of `name`. Always returns the input as
/// the first entry so callers can dedup against it.
pub fn case_variants(name: &str) -> Vec<String> {
    let words = split_words(name);
    if words.is_empty() {
        return vec![name.to_string()];
    }

    let mut out: Vec<String> = vec![name.to_string()];
    let mut push = |s: String| {
        if !s.is_empty() && !out.contains(&s) {
            out.push(s);
        }
    };

    push(to_snake(&words));
    push(to_kebab(&words));
    push(to_camel(&words));
    push(to_pascal(&words));
    push(to_screaming(&words));

    out
}

fn split_words(name: &str) -> Vec<String> {
    // Split on `_`, `-`, ` `, then split remaining tokens on case boundaries.
    // Acronyms are preserved as a single word: `HTTPRequest` -> ["HTTP", "Request"].
    let mut out: Vec<String> = Vec::new();
    for chunk in name.split(['_', '-', ' ', '.']) {
        if chunk.is_empty() {
            continue;
        }
        let mut current = String::new();
        let chars: Vec<char> = chunk.chars().collect();
        for i in 0..chars.len() {
            let c = chars[i];
            let prev = if i == 0 { None } else { Some(chars[i - 1]) };
            let next = chars.get(i + 1).copied();

            let upper_after_lower = matches!(prev, Some(p) if p.is_lowercase()) && c.is_uppercase();
            let acronym_end = matches!(prev, Some(p) if p.is_uppercase())
                && c.is_uppercase()
                && matches!(next, Some(n) if n.is_lowercase());

            if (upper_after_lower || acronym_end) && !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            current.push(c);
        }
        if !current.is_empty() {
            out.push(current);
        }
    }
    out
}

fn to_snake(words: &[String]) -> String {
    words
        .iter()
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join("_")
}

fn to_kebab(words: &[String]) -> String {
    words
        .iter()
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join("-")
}

fn to_screaming(words: &[String]) -> String {
    words
        .iter()
        .map(|w| w.to_uppercase())
        .collect::<Vec<_>>()
        .join("_")
}

fn to_pascal(words: &[String]) -> String {
    words.iter().map(|w| capitalize(w)).collect()
}

fn to_camel(words: &[String]) -> String {
    let mut out = String::new();
    for (i, w) in words.iter().enumerate() {
        if i == 0 {
            out.push_str(&w.to_lowercase());
        } else {
            out.push_str(&capitalize(w));
        }
    }
    out
}

fn capitalize(w: &str) -> String {
    let mut chars = w.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}

/// Top-N fuzzy candidates using `neo_frizbee::match_list` with typo tolerance.
fn fuzzy_candidates(query: &str, names: &[String], top_n: usize) -> Vec<String> {
    if names.is_empty() {
        return Vec::new();
    }
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let cfg = neo_frizbee::Config {
        max_typos: Some(FUZZY_MAX_TYPOS),
        sort: true,
        ..Default::default()
    };
    let matches = neo_frizbee::match_list(query, &refs, &cfg);

    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut out: Vec<String> = Vec::new();
    for m in matches {
        let idx = m.index as usize;
        if idx >= names.len() {
            continue;
        }
        let candidate = &names[idx];
        if candidate == query || !seen.insert(candidate.clone()) {
            continue;
        }
        out.push(candidate.clone());
        if out.len() >= top_n {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_words_handles_separators() {
        assert_eq!(split_words("foo_bar"), vec!["foo", "bar"]);
        assert_eq!(split_words("foo-bar"), vec!["foo", "bar"]);
        assert_eq!(split_words("FooBar"), vec!["Foo", "Bar"]);
        assert_eq!(split_words("fooBar"), vec!["foo", "Bar"]);
    }

    #[test]
    fn split_words_preserves_acronyms() {
        assert_eq!(split_words("HTTPRequest"), vec!["HTTP", "Request"]);
        assert_eq!(
            split_words("parseHTTPRequest"),
            vec!["parse", "HTTP", "Request"]
        );
        assert_eq!(split_words("ID"), vec!["ID"]);
    }

    #[test]
    fn case_variants_includes_all_styles() {
        let v = case_variants("myFunc");
        assert!(v.contains(&"my_func".to_string()));
        assert!(v.contains(&"my-func".to_string()));
        assert!(v.contains(&"MyFunc".to_string()));
        assert!(v.contains(&"MY_FUNC".to_string()));
        assert!(v.contains(&"myFunc".to_string()));
    }

    #[test]
    fn case_variants_idempotent_on_snake_input() {
        let v = case_variants("my_func");
        assert!(v.contains(&"myFunc".to_string()));
        assert!(v.contains(&"MyFunc".to_string()));
        assert!(v.contains(&"MY_FUNC".to_string()));
    }

    #[test]
    fn case_variants_for_acronym_input() {
        let v = case_variants("HTTPRequest");
        assert!(v.contains(&"http_request".to_string()));
        assert!(v.contains(&"HTTP_REQUEST".to_string()));
        assert!(v.contains(&"httpRequest".to_string()));
    }

    #[test]
    fn case_variants_handles_empty_and_single_char() {
        let v = case_variants("");
        assert_eq!(v, vec!["".to_string()]);
        let v = case_variants("x");
        assert!(v.contains(&"x".to_string()));
        assert!(v.contains(&"X".to_string()));
    }

    #[test]
    fn fuzzy_candidates_finds_close_matches() {
        let names = vec![
            "render_template".to_string(),
            "render_text".to_string(),
            "compute".to_string(),
        ];
        let out = fuzzy_candidates("rendr_tmplate", &names, 2);
        assert!(
            out.contains(&"render_template".to_string()),
            "fuzzy missed close typo: {out:?}"
        );
    }

    #[test]
    fn fuzzy_candidates_skips_exact_query() {
        let names = vec!["foo".to_string(), "foo_bar".to_string()];
        let out = fuzzy_candidates("foo", &names, 5);
        assert!(!out.contains(&"foo".to_string()));
    }
}
