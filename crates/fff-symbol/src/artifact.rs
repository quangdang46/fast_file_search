//! JS/TS artifact mode: export anchor extraction for bundled/compiled files.
//!
//! Identifies named exports, CommonJS exports, AMD modules, and UMD globals
//! in JS/TS code so that bundled/minified files remain navigable.

/// One export anchor inside a JS/TS file.
#[derive(Debug, Clone)]
pub struct ArtifactAnchor {
    pub line: u32,
    pub kind: &'static str,
    pub name: String,
}

/// Extract all named export anchors from JS/TS `content`.
/// Returns (anchors, total_lines) where total_lines is the number of lines in the file.
pub fn extract_artifact_anchors(content: &str) -> (Vec<ArtifactAnchor>, usize) {
    let mut anchors = Vec::new();
    let total = content.lines().count();
    for (i, line) in content.lines().enumerate() {
        let lineno = (i + 1) as u32;
        // ES module exports
        for name in es_export_names(line) {
            anchors.push(ArtifactAnchor {
                line: lineno,
                kind: "esm",
                name,
            });
        }
        // CommonJS exports
        for name in cjs_export_names(line) {
            anchors.push(ArtifactAnchor {
                line: lineno,
                kind: "cjs",
                name,
            });
        }
        // AMD defines
        for name in amd_define_names(line) {
            anchors.push(ArtifactAnchor {
                line: lineno,
                kind: "amd",
                name,
            });
        }
        // UMD globals
        for name in umd_global_names(line) {
            anchors.push(ArtifactAnchor {
                line: lineno,
                kind: "umd",
                name,
            });
        }
    }
    // Dedup by (line, name).
    anchors.sort_by(|a, b| a.line.cmp(&b.line).then_with(|| a.name.cmp(&b.name)));
    anchors.dedup_by(|a, b| a.line == b.line && a.name == b.name);
    (anchors, total)
}

/// Search anchors for any whose name contains `query`.
pub fn search_anchor_matches(content: &str, query: &str) -> Vec<ArtifactAnchor> {
    let (anchors, _) = extract_artifact_anchors(content);
    anchors
        .into_iter()
        .filter(|a| a.name.to_lowercase().contains(&query.to_lowercase()))
        .collect()
}

/// Check if a file is likely an artifact JS/TS file by extension.
pub fn is_artifact_js_ts_file(path: &std::path::Path) -> bool {
    path.extension().is_some_and(|e| {
        let s = e.to_string_lossy().to_lowercase();
        matches!(s.as_str(), "js" | "ts" | "jsx" | "tsx" | "mjs" | "cjs")
    })
}

// ES: export { x }, export default, export function x, export class X,
//     export const x, export let x, export var x
fn es_export_names(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let trimmed = line.trim();
    // export { a, b, c }
    if let Some(start) = trimmed.find("export {") {
        let inner = &trimmed[start + 8..];
        if let Some(end) = inner.find('}') {
            for name in inner[..end].split(',') {
                let clean = clean_export_name(name.trim());
                if !clean.is_empty() {
                    out.push(clean);
                }
            }
        }
    }
    // export default X
    if trimmed.starts_with("export default") {
        let name = trimmed[14..].trim();
        let clean = clean_export_name(name);
        if !clean.is_empty() {
            out.push(clean);
        }
    }
    // export function/class/const/let/var NAME
    for kw in &["export function", "export class", "export const", "export let", "export var"] {
        if trimmed.starts_with(kw) {
            let after = &trimmed[kw.len()..].trim_start();
            let clean = clean_export_name(after);
            if !clean.is_empty() {
                out.push(clean);
            }
        }
    }
    out
}

// CommonJS: module.exports.NAME = ... or exports.NAME = ...
fn cjs_export_names(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let trimmed = line.trim();
    if trimmed.starts_with("module.exports.") || trimmed.starts_with("exports.") {
        let after = if trimmed.starts_with("module.exports.") {
            &trimmed[15..]
        } else {
            &trimmed[8..]
        };
        let name = clean_export_name(after);
        if !name.is_empty() {
            out.push(name);
        }
    }
    out
}

// AMD: define("name", ...) or define("name" ...)
fn amd_define_names(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let trimmed = line.trim();
    if trimmed.starts_with("define(") || trimmed.starts_with("define (") {
        let after = if trimmed.starts_with("define(") {
            &trimmed[7..]
        } else {
            &trimmed[8..]
        };
        // Find first quoted string
        if let Some(start) = after.find('"') {
            let rest = &after[start + 1..];
            if let Some(end) = rest.find('"') {
                let name = &rest[..end];
                let clean = clean_export_name(name);
                if !clean.is_empty() {
                    out.push(clean);
                }
            }
        }
        // Try single quotes too
        if let Some(start) = after.find('\'') {
            let rest = &after[start + 1..];
            if let Some(end) = rest.find('\'') {
                let name = &rest[..end];
                let clean = clean_export_name(name);
                if !clean.is_empty() {
                    out.push(clean);
                }
            }
        }
    }
    out
}

// UMD: global assignments like globalThis.Name = Name or window.Name = Name
fn umd_global_names(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let trimmed = line.trim();
    for prefix in &["globalThis.", "global.", "window.", "self."] {
        if trimmed.starts_with(prefix) {
            let after = &trimmed[prefix.len()..];
            let name = clean_export_name(after);
            if !name.is_empty() {
                out.push(name);
            }
        }
    }
    out
}

/// Extract an identifier from the start of `text`, stopping at any
/// non-alphanumeric, underscore, dollar-sign, or hyphen character.
fn clean_export_name(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() || c == '_' || c == '$' || c == '-' {
            out.push(c);
        } else {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn es_export_names_basic() {
        let anchors = es_export_names("export { foo, bar, baz }");
        assert!(anchors.contains(&"foo".to_string()));
        assert!(anchors.contains(&"bar".to_string()));
    }

    #[test]
    fn es_export_default() {
        let anchors = es_export_names("export default MyClass");
        assert!(anchors.contains(&"MyClass".to_string()));
    }

    #[test]
    fn es_export_function() {
        let anchors = es_export_names("export function hello() {}");
        assert!(anchors.contains(&"hello".to_string()));
    }

    #[test]
    fn es_export_const() {
        let anchors = es_export_names("export const MAX = 100;");
        assert!(anchors.contains(&"MAX".to_string()));
    }

    #[test]
    fn cjs_module_exports() {
        let anchors = cjs_export_names("module.exports.foo = function() {};");
        assert_eq!(anchors, vec!["foo"]);
    }

    #[test]
    fn cjs_exports_short() {
        let anchors = cjs_export_names("exports.bar = 42;");
        assert_eq!(anchors, vec!["bar"]);
    }

    #[test]
    fn amd_define_name() {
        let anchors = amd_define_names("define('my-module', [...], function() {});");
        assert_eq!(anchors, vec!["my-module"]);
    }

    #[test]
    fn amd_define_double_quotes() {
        let anchors = amd_define_names("define(\"other\", function() {});");
        assert_eq!(anchors, vec!["other"]);
    }

    #[test]
    fn umd_global_this() {
        let anchors = umd_global_names("globalThis.MyLib = MyLib;");
        assert_eq!(anchors, vec!["MyLib"]);
    }

    #[test]
    fn umd_window() {
        let anchors = umd_global_names("window.App = App;");
        assert_eq!(anchors, vec!["App"]);
    }

    #[test]
    fn is_artifact_js_ts() {
        assert!(is_artifact_js_ts_file(std::path::Path::new("file.ts")));
        assert!(is_artifact_js_ts_file(std::path::Path::new("bundle.mjs")));
        assert!(!is_artifact_js_ts_file(std::path::Path::new("README.md")));
    }

    #[test]
    fn extract_all_from_mixed_content() {
        let content = r#"export { a, b };
export function foo() {}
module.exports.bar = 1;
define("mod", function() {});
globalThis.X = X;
"#;
        let (anchors, total) = extract_artifact_anchors(content);
        assert_eq!(total, 5);
        let names: Vec<String> = anchors.iter().map(|a| a.name.clone()).collect();
        assert!(names.contains(&"a".to_string()));
        assert!(names.contains(&"foo".to_string()));
        assert!(names.contains(&"bar".to_string()));
        assert!(names.contains(&"mod".to_string()));
        assert!(names.contains(&"X".to_string()));
    }

    #[test]
    fn search_matches_are_case_insensitive() {
        let content = "export { FooBar };\n";
        let hits = search_anchor_matches(content, "foo");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "FooBar");
    }
}
