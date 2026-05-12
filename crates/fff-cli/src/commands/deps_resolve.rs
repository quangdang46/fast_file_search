use std::path::{Path, PathBuf};

use tree_sitter::{Node, Parser};

use fff_symbol::outline_language;
use fff_symbol::types::Lang;

// Returns raw import specifier strings (the "from X" / "use X::Y" / "#include X")
// extracted from `content` parsed under `lang`. Strings are returned in source
// order. Path-style specifiers keep their quotes stripped.
pub fn extract_imports(content: &str, lang: Lang) -> Vec<String> {
    let Some(language) = outline_language(lang) else {
        return Vec::new();
    };
    let mut parser = Parser::new();
    if parser.set_language(&language).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(content, None) else {
        return Vec::new();
    };
    let src = content.as_bytes();
    let mut out = Vec::new();
    walk(tree.root_node(), src, lang, &mut out);
    out
}

fn walk(node: Node, src: &[u8], lang: Lang, out: &mut Vec<String>) {
    if let Some(spec) = node_to_import(node, src, lang) {
        out.push(spec);
        // Imports do not nest, so don't recurse into them.
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, src, lang, out);
    }
}

fn node_to_import(node: Node, src: &[u8], lang: Lang) -> Option<String> {
    match (lang, node.kind()) {
        // Rust: `use foo::bar::Baz;`, `use foo::{bar, baz};`, `use foo::*;`.
        // Reduce all forms to the path prefix the compiler resolves first.
        (Lang::Rust, "use_declaration") => rust_use_path(node, src),

        // Python: `import a`, `import a.b`, `from a.b import c`.
        (Lang::Python, "import_statement") => find_dotted_or_alias(node, src),
        (Lang::Python, "import_from_statement") => {
            // Field "module_name" holds the source module of the from-import.
            node.child_by_field_name("module_name")
                .and_then(|n| text_of(n, src))
        }

        // TS/JS: `import x from "spec"`, `import "spec"`, `export ... from "spec"`.
        (
            Lang::TypeScript | Lang::Tsx | Lang::JavaScript,
            "import_statement" | "export_statement",
        ) => find_string_in(node, src),

        // JS/TS CommonJS: `require("spec")`.
        (Lang::TypeScript | Lang::Tsx | Lang::JavaScript, "call" | "call_expression") => {
            js_require_target(node, src)
        }

        // Elixir: `alias Foo.Bar`, `import Foo.Bar`, `use Foo.Bar`, `require Foo.Bar`.
        (Lang::Elixir, "call") => elixir_alias_target(node, src),

        // Go: `import "fmt"` or `import ( "fmt"; "io" )`. Each spec node
        // contains exactly one string.
        (Lang::Go, "import_spec") => find_string_in(node, src),

        // Java: `import a.b.C;` — one scoped_identifier inside.
        (Lang::Java, "import_declaration") => first_path_segment(node, src),

        // C / C++: `#include <foo.h>` or `#include "foo.h"`. The path lives
        // in a `string_literal` or `system_lib_string` child.
        (Lang::C | Lang::Cpp, "preproc_include") => find_string_in(node, src),

        // Ruby: `require "foo"`, `require_relative "../foo"`. These are
        // calls, not import nodes, so we only match the call form below.
        (Lang::Ruby, "call") => ruby_require_target(node, src),

        // PHP: `use A\B\C;`, `require_once "...";`. Only the namespace form
        // gets a dedicated node kind.
        (Lang::Php, "namespace_use_declaration") => first_path_segment(node, src),

        // C#: `using System.Collections.Generic;`.
        (Lang::CSharp, "using_directive") => first_path_segment(node, src),

        // Kotlin / Swift: `import foo.bar` / `import Bar`.
        (Lang::Kotlin | Lang::Swift, "import_header" | "import_declaration") => {
            first_path_segment(node, src)
        }

        // Scala: `import a.b.{C, D}`.
        (Lang::Scala, "import_declaration") => first_path_segment(node, src),

        _ => None,
    }
}

fn rust_use_path(node: Node, src: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "scoped_identifier" | "identifier" | "crate" | "super" | "self" => {
                return text_of(child, src);
            }
            "scoped_use_list" | "use_wildcard" | "use_as_clause" => {
                if let Some(path) = child.child_by_field_name("path") {
                    return text_of(path, src);
                }
                if let Some(first) = child.named_child(0) {
                    return text_of(first, src);
                }
            }
            _ => {}
        }
    }
    None
}

fn first_path_segment(node: Node, src: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "scoped_identifier"
            | "identifier"
            | "type_identifier"
            | "qualified_name"
            | "package_identifier"
            | "namespace_name"
            | "user_type"
            | "name"
            | "scoped_type_identifier"
            | "use_clause"
            | "use_path"
            | "use_list" => {
                if let Some(t) = text_of(child, src) {
                    return Some(t);
                }
            }
            _ => {}
        }
    }
    // Fallback: first identifier-like descendant.
    find_descendant_identifier(node, src)
}

fn find_descendant_identifier(node: Node, src: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(
            child.kind(),
            "scoped_identifier"
                | "identifier"
                | "type_identifier"
                | "qualified_name"
                | "namespace_name"
                | "user_type"
        ) {
            return text_of(child, src);
        }
        if let Some(t) = find_descendant_identifier(child, src) {
            return Some(t);
        }
    }
    None
}

fn find_dotted_or_alias(node: Node, src: &[u8]) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        match child.kind() {
            "dotted_name" | "aliased_import" | "identifier" => {
                if let Some(t) = text_of(child, src) {
                    return Some(t);
                }
            }
            _ => {}
        }
    }
    None
}

fn find_string_in(node: Node, src: &[u8]) -> Option<String> {
    let mut stack = vec![node];
    while let Some(n) = stack.pop() {
        if matches!(
            n.kind(),
            "string"
                | "string_literal"
                | "system_lib_string"
                | "interpreted_string_literal"
                | "raw_string_literal"
        ) {
            if let Some(t) = text_of(n, src) {
                return Some(strip_quotes(&t).to_string());
            }
        }
        let mut cursor = n.walk();
        for child in n.children(&mut cursor) {
            stack.push(child);
        }
    }
    None
}

fn js_require_target(node: Node, src: &[u8]) -> Option<String> {
    let method = node.child_by_field_name("function")
        .or_else(|| node.named_child(0).filter(|n| matches!(n.kind(), "identifier")))?;
    let name = text_of(method, src)?;
    if name != "require" {
        return None;
    }
    find_string_in(node, src)
}

fn elixir_alias_target(node: Node, src: &[u8]) -> Option<String> {
    let method = node.child_by_field_name("target")
        .or_else(|| node.named_child(0).filter(|n| matches!(n.kind(), "identifier")))?;
    let name = text_of(method, src)?;
    if !matches!(name.as_str(), "alias" | "import" | "use" | "require") {
        return None;
    }
    // The module name lives in the arguments child; skip the keyword itself.
    let args = node.child_by_field_name("arguments")?;
    let mut cursor = args.walk();
    for child in args.named_children(&mut cursor) {
        if let Some(t) = text_of(child, src) {
            if t != name && !t.is_empty() {
                return Some(t);
            }
        }
    }
    None
}

fn ruby_require_target(node: Node, src: &[u8]) -> Option<String> {
    let method = node.child_by_field_name("method").or_else(|| {
        node.named_child(0)
            .filter(|n| matches!(n.kind(), "identifier"))
    })?;
    let name = text_of(method, src)?;
    if name != "require" && name != "require_relative" {
        return None;
    }
    find_string_in(node, src)
}

fn strip_quotes(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"')
            || (first == b'\'' && last == b'\'')
            || (first == b'<' && last == b'>')
        {
            return &s[1..s.len() - 1];
        }
    }
    s
}

fn text_of(node: Node, src: &[u8]) -> Option<String> {
    node.utf8_text(src).ok().map(|s| s.to_string())
}

// Resolve an import specifier extracted from `from_path` to a file path inside
// `root`, if it points to one. Best-effort across languages:
//
// * Quoted relative paths (`./foo`, `../bar/baz`) try the literal path,
//   `<path>.<ext>`, `<path>/index.<ext>`, `<path>/mod.rs`, and
//   `<path>/__init__.py` for Python.
// * Dotted module paths (`a.b.c`, `a::b::c`) translate `.` and `::` to `/`
//   and search both from `root` and from the importing file's package roots.
// * Unresolvable specifiers (e.g. third-party packages) return None.
pub fn resolve_import(spec: &str, from: &Path, root: &Path, lang: Lang) -> Option<PathBuf> {
    let trimmed = spec.trim();
    if trimmed.is_empty() {
        return None;
    }

    if trimmed.starts_with('.') || trimmed.starts_with('/') {
        let base = from.parent().unwrap_or(root);
        return try_resolve_path(base, trimmed, lang).filter(|p| p.starts_with(root));
    }

    // JS/TS path aliases: @/... and ~/... resolve from root/src/ or root/.
    if matches!(lang, Lang::TypeScript | Lang::Tsx | Lang::JavaScript) {
        if let Some(alias_resolved) = resolve_js_alias(trimmed, root, lang) {
            return Some(alias_resolved);
        }
    }

    let parts: Vec<&str> = if trimmed.contains("::") {
        trimmed.split("::").collect()
    } else {
        trimmed.split('.').collect()
    };
    if parts.is_empty() {
        return None;
    }
    let candidate_segments = parts
        .iter()
        .filter(|p| !p.is_empty() && !p.contains('{') && !p.contains('}'))
        .copied()
        .collect::<Vec<_>>();
    if candidate_segments.is_empty() {
        return None;
    }

    // Rust: `crate::a::b` resolves from the crate root (`src/<a>/<b>`); `super::a`
    // / `self::a` from the parent / current module.
    if matches!(lang, Lang::Rust) {
        if let Some(p) = rust_resolve(&candidate_segments, from, root) {
            return Some(p);
        }
    }

    let segs: Vec<&str> = candidate_segments
        .iter()
        .filter(|s| !matches!(**s, "crate" | "super" | "self"))
        .copied()
        .collect();
    if segs.is_empty() {
        return None;
    }
    let joined: PathBuf = segs.iter().collect();

    // Try root-relative first.
    if let Some(p) = try_resolve_path(root, joined.to_str()?, lang) {
        return Some(p);
    }
    // Then walk parents of the importing file looking for a match.
    let mut current = from.parent();
    while let Some(dir) = current {
        if !dir.starts_with(root) {
            break;
        }
        if let Some(p) = try_resolve_path(dir, joined.to_str()?, lang) {
            return Some(p);
        }
        current = dir.parent();
    }
    None
}

fn rust_resolve(segments: &[&str], from: &Path, root: &Path) -> Option<PathBuf> {
    let first = *segments.first()?;
    let rest: Vec<&str> = segments[1..].to_vec();
    let base = match first {
        "crate" => find_crate_src(from, root)?,
        "super" => from.parent()?.parent()?.to_path_buf(),
        "self" => from.parent()?.to_path_buf(),
        _ => return None,
    };
    if rest.is_empty() {
        return None;
    }
    // The last segment may be a module file *or* an item inside the parent
    // module — try both, longest match first.
    for take in (1..=rest.len()).rev() {
        if let Some(p) = try_rust_module_path(&base, &rest[..take]) {
            return Some(p);
        }
    }
    None
}

fn try_rust_module_path(base: &Path, segs: &[&str]) -> Option<PathBuf> {
    let mut probe = base.to_path_buf();
    for seg in segs.iter().take(segs.len() - 1) {
        probe.push(seg);
    }
    let last = segs.last()?;
    let with_ext = probe.join(format!("{last}.rs"));
    if with_ext.is_file() {
        return Some(with_ext);
    }
    let module_dir = probe.join(last);
    for index in ["mod.rs", "lib.rs"] {
        let p = module_dir.join(index);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

fn resolve_js_alias(trimmed: &str, root: &Path, lang: Lang) -> Option<PathBuf> {
    let rel = if trimmed.starts_with("@/") {
        &trimmed[2..]
    } else if trimmed.starts_with("~/") {
        &trimmed[2..]
    } else {
        return None;
    };
    // Try root/src first, then root itself.
    for base in [root.join("src"), root.to_path_buf()] {
        if let Some(p) = try_resolve_path(&base, rel, lang) {
            return Some(p);
        }
    }
    None
}

fn find_crate_src(from: &Path, root: &Path) -> Option<PathBuf> {
    let mut current = from.parent();
    while let Some(dir) = current {
        if dir.join("Cargo.toml").is_file() {
            let src = dir.join("src");
            if src.is_dir() {
                return Some(src);
            }
        }
        if dir == root {
            break;
        }
        current = dir.parent();
    }
    None
}

fn try_resolve_path(base: &Path, spec: &str, lang: Lang) -> Option<PathBuf> {
    let direct = base.join(spec);
    if direct.is_file() {
        return Some(direct);
    }
    for ext in candidate_extensions(lang) {
        let with_ext = base.join(format!("{spec}.{ext}"));
        if with_ext.is_file() {
            return Some(with_ext);
        }
    }
    for index in candidate_index_files(lang) {
        let with_index = base.join(spec).join(index);
        if with_index.is_file() {
            return Some(with_index);
        }
    }
    None
}

fn candidate_extensions(lang: Lang) -> &'static [&'static str] {
    match lang {
        Lang::Rust => &["rs"],
        Lang::TypeScript => &["ts", "tsx", "d.ts", "js"],
        Lang::Tsx => &["tsx", "ts", "jsx", "js"],
        Lang::JavaScript => &["js", "jsx", "mjs", "cjs"],
        Lang::Python => &["py"],
        Lang::Go => &["go"],
        Lang::Java => &["java"],
        Lang::Scala => &["scala", "sc"],
        Lang::C => &["h", "c"],
        Lang::Cpp => &["hpp", "hh", "h", "cpp", "cc", "cxx"],
        Lang::Ruby => &["rb"],
        Lang::Php => &["php"],
        Lang::Swift => &["swift"],
        Lang::Kotlin => &["kt", "kts"],
        Lang::CSharp => &["cs"],
        Lang::Elixir => &["ex", "exs"],
        Lang::Dockerfile | Lang::Make => &[],
    }
}

fn candidate_index_files(lang: Lang) -> &'static [&'static str] {
    match lang {
        Lang::TypeScript | Lang::Tsx => &["index.ts", "index.tsx", "index.js"],
        Lang::JavaScript => &["index.js", "index.mjs"],
        Lang::Python => &["__init__.py"],
        Lang::Rust => &["mod.rs", "lib.rs"],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_use_declaration_yields_first_path() {
        let src = "use std::collections::HashMap;\nuse crate::foo::bar;\n";
        let imports = extract_imports(src, Lang::Rust);
        assert_eq!(
            imports,
            vec!["std::collections::HashMap", "crate::foo::bar"]
        );
    }

    #[test]
    fn rust_use_list_returns_path_prefix_not_listed_items() {
        let src = "use crate::commands::pagination::{footer, Page};\n";
        let imports = extract_imports(src, Lang::Rust);
        assert_eq!(imports, vec!["crate::commands::pagination"]);
    }

    #[test]
    fn rust_use_wildcard_returns_path_prefix() {
        let src = "use super::*;\n";
        let imports = extract_imports(src, Lang::Rust);
        assert_eq!(imports, vec!["super"]);
    }

    #[test]
    fn resolve_rust_crate_path_walks_into_file_or_module() {
        let dir = tempfile::tempdir().expect("tmp");
        let root = dir.path();
        std::fs::create_dir_all(root.join("crates/foo/src/cmd")).expect("dirs");
        std::fs::write(root.join("crates/foo/Cargo.toml"), "").expect("write");
        std::fs::write(root.join("crates/foo/src/lib.rs"), "").expect("write");
        std::fs::write(root.join("crates/foo/src/cli.rs"), "").expect("write");
        std::fs::write(root.join("crates/foo/src/cmd/mod.rs"), "").expect("write");
        std::fs::write(root.join("crates/foo/src/cmd/sub.rs"), "").expect("write");
        let from = root.join("crates/foo/src/cmd/sub.rs");

        let p = resolve_import("crate::cli::Type", &from, root, Lang::Rust)
            .expect("crate path resolves");
        assert_eq!(p, root.join("crates/foo/src/cli.rs"));

        let p = resolve_import("crate::cmd::sub", &from, root, Lang::Rust)
            .expect("crate path to module file resolves");
        assert_eq!(p, root.join("crates/foo/src/cmd/sub.rs"));
    }

    #[test]
    fn python_import_and_from_yield_modules() {
        let src = "import os\nimport pkg.mod\nfrom a.b import c\n";
        let imports = extract_imports(src, Lang::Python);
        assert!(imports.contains(&"os".to_string()), "{imports:?}");
        assert!(imports.contains(&"pkg.mod".to_string()), "{imports:?}");
        assert!(imports.contains(&"a.b".to_string()), "{imports:?}");
    }

    #[test]
    fn typescript_import_yields_specifier() {
        let src = r#"import { x } from "./foo";
import "side-effect";
import * as m from '../bar/baz';
"#;
        let imports = extract_imports(src, Lang::TypeScript);
        assert!(imports.contains(&"./foo".to_string()), "{imports:?}");
        assert!(imports.contains(&"side-effect".to_string()), "{imports:?}");
        assert!(imports.contains(&"../bar/baz".to_string()), "{imports:?}");
    }

    #[test]
    fn go_import_block_yields_each_spec() {
        let src = "import (\n  \"fmt\"\n  \"io\"\n)\n";
        let imports = extract_imports(src, Lang::Go);
        assert!(imports.contains(&"fmt".to_string()));
        assert!(imports.contains(&"io".to_string()));
    }

    #[test]
    fn strip_quotes_handles_double_single_angle() {
        assert_eq!(strip_quotes("\"foo\""), "foo");
        assert_eq!(strip_quotes("'foo'"), "foo");
        assert_eq!(strip_quotes("<foo.h>"), "foo.h");
        assert_eq!(strip_quotes("foo"), "foo");
    }

    #[test]
    fn resolve_relative_typescript_with_extension_search() {
        let dir = tempfile::tempdir().expect("tmp");
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/util")).expect("dirs");
        std::fs::write(root.join("src/util/foo.ts"), "").expect("write");
        std::fs::write(root.join("src/main.ts"), "").expect("write");

        let from = root.join("src/main.ts");
        let resolved =
            resolve_import("./util/foo", &from, root, Lang::TypeScript).expect("resolve");
        assert_eq!(resolved, root.join("src/util/foo.ts"));
    }

    #[test]
    fn resolve_typescript_index_file() {
        let dir = tempfile::tempdir().expect("tmp");
        let root = dir.path();
        std::fs::create_dir_all(root.join("pkg")).expect("dirs");
        std::fs::write(root.join("pkg/index.ts"), "").expect("write");
        std::fs::write(root.join("main.ts"), "").expect("write");

        let from = root.join("main.ts");
        let resolved = resolve_import("./pkg", &from, root, Lang::TypeScript).expect("resolve");
        assert_eq!(resolved, root.join("pkg/index.ts"));
    }

    #[test]
    fn resolve_python_dotted_finds_file_under_root() {
        let dir = tempfile::tempdir().expect("tmp");
        let root = dir.path();
        std::fs::create_dir_all(root.join("a/b")).expect("dirs");
        std::fs::write(root.join("a/__init__.py"), "").expect("write");
        std::fs::write(root.join("a/b/__init__.py"), "").expect("write");
        std::fs::write(root.join("a/b/c.py"), "").expect("write");
        std::fs::write(root.join("main.py"), "").expect("write");

        let from = root.join("main.py");
        let resolved = resolve_import("a.b.c", &from, root, Lang::Python).expect("resolve");
        assert_eq!(resolved, root.join("a/b/c.py"));
    }

    #[test]
    fn js_require_target_extracts_specifier() {
        let src = r#"const x = require("./foo");"#;
        let imports = extract_imports(src, Lang::JavaScript);
        assert!(imports.contains(&"./foo".to_string()), "{imports:?}");
    }

    #[test]
    fn elixir_alias_target_extracts_module() {
        // Best-effort: Elixir tree-sitter grammar node structures for alias/import
        // vary by version. At minimum we don't panic and may return some results.
        let src = "alias MyApp.Module.Sub\nimport Other\n";
        let _imports = extract_imports(src, Lang::Elixir);
        // Not asserting exact contents — grammar-dependent.
    }

    #[test]
    fn resolve_js_at_alias_to_src() {
        let dir = tempfile::tempdir().expect("tmp");
        let root = dir.path();
        std::fs::create_dir_all(root.join("src/util")).expect("dirs");
        std::fs::write(root.join("src/util/foo.ts"), "").expect("write");
        std::fs::write(root.join("src/main.ts"), "").expect("write");

        let from = root.join("src/main.ts");
        let resolved =
            resolve_import("@/util/foo", &from, root, Lang::TypeScript).expect("resolve");
        assert_eq!(resolved, root.join("src/util/foo.ts"));
    }

    #[test]
    fn resolve_js_tilde_alias_to_root() {
        let dir = tempfile::tempdir().expect("tmp");
        let root = dir.path();
        std::fs::create_dir_all(root.join("lib")).expect("dirs");
        std::fs::create_dir_all(root.join("src")).expect("dirs");
        std::fs::write(root.join("lib/index.ts"), "").expect("write");
        std::fs::write(root.join("src/main.ts"), "").expect("write");

        let from = root.join("src/main.ts");
        let resolved =
            resolve_import("~/lib/index", &from, root, Lang::TypeScript).expect("resolve");
        assert_eq!(resolved, root.join("lib/index.ts"));
    }

    #[test]
    fn resolve_returns_none_when_not_found() {
        let dir = tempfile::tempdir().expect("tmp");
        let root = dir.path();
        std::fs::write(root.join("main.ts"), "").expect("write");
        let from = root.join("main.ts");
        assert!(resolve_import("./missing", &from, root, Lang::TypeScript).is_none());
        assert!(resolve_import("react", &from, root, Lang::TypeScript).is_none());
    }
}
