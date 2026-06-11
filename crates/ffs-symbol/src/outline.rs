//! Tree-sitter driven outline extraction (functions, classes, modules, …)
//! for the languages ffs supports.

use tree_sitter::{Language, Node, Parser};

use crate::treesitter::{
    elixir_arguments, extract_definition_name, extract_elixir_definition_name, find_iife_function,
    is_elixir_definition, js_function_context_name, node_text_simple,
};
use crate::types::{Lang, OutlineEntry, OutlineKind};
use crate::verse_spans::{verse_repair_end_line, verse_skip_spurious_definition};

/// Map a [`Lang`] to its tree-sitter [`Language`] descriptor.
pub fn outline_language(lang: Lang) -> Option<Language> {
    let l: Language = match lang {
        Lang::Rust => tree_sitter_rust::LANGUAGE.into(),
        Lang::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        Lang::Tsx => tree_sitter_typescript::LANGUAGE_TSX.into(),
        Lang::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
        Lang::Python => tree_sitter_python::LANGUAGE.into(),
        Lang::Go => tree_sitter_go::LANGUAGE.into(),
        Lang::Java => tree_sitter_java::LANGUAGE.into(),
        Lang::Scala => tree_sitter_scala::LANGUAGE.into(),
        Lang::C => tree_sitter_c::LANGUAGE.into(),
        Lang::Cpp => tree_sitter_cpp::LANGUAGE.into(),
        Lang::Ruby => tree_sitter_ruby::LANGUAGE.into(),
        Lang::Php => tree_sitter_php::LANGUAGE_PHP.into(),
        Lang::Swift => tree_sitter_swift::LANGUAGE.into(),
        Lang::Kotlin => tree_sitter_kotlin_ng::LANGUAGE.into(),
        Lang::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
        Lang::Elixir => tree_sitter_elixir::LANGUAGE.into(),
        Lang::Verse => tree_sitter_verse::LANGUAGE.into(),
        Lang::Dockerfile | Lang::Make => return None,
    };
    Some(l)
}

/// Extract a flat outline (top-level definitions only, plus their immediate
/// children for class-like nodes) from `content` parsed under `lang`.
pub fn get_outline_entries(content: &str, lang: Lang) -> Vec<OutlineEntry> {
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
    let lines: Vec<&str> = content.lines().collect();
    let root = tree.root_node();
    let mut out = Vec::new();
    walk_top_level(root, &lines, content, lang, &mut out);
    out
}

fn walk_top_level(
    node: Node,
    lines: &[&str],
    content: &str,
    lang: Lang,
    out: &mut Vec<OutlineEntry>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(entry) = node_to_entry(child, lines, content, lang) {
            out.push(entry);
        }
    }
}

fn node_to_entry(node: Node, lines: &[&str], content: &str, lang: Lang) -> Option<OutlineEntry> {
    if lang == Lang::Verse && verse_skip_spurious_definition(node, lines) {
        return None;
    }
    let kind = outline_kind_for(node, lang)?;

    let name = match (lang, node.kind()) {
        (Lang::Elixir, "call") if is_elixir_definition(node, lines) => {
            extract_elixir_definition_name(node, lines)?
        }
        (Lang::JavaScript | Lang::TypeScript | Lang::Tsx, k) if k.contains("function") => {
            js_function_context_name(node, lines).unwrap_or_else(|| "<anonymous>".to_string())
        }
        _ => extract_definition_name(node, lines).unwrap_or_else(|| "<anonymous>".to_string()),
    };

    let start_line = node.start_position().row as u32 + 1;
    let ast_end = node.end_position().row as u32 + 1;
    let end_line = if lang == Lang::Verse {
        verse_repair_end_line(node.kind(), start_line, ast_end, lines)
    } else {
        ast_end
    };

    let signature = extract_signature(node, lines);

    let mut children = Vec::new();
    if matches!(
        kind,
        OutlineKind::Class | OutlineKind::Struct | OutlineKind::Interface | OutlineKind::Module
    ) {
        collect_children(node, lines, content, lang, &mut children);
    }

    let doc = extract_doc_comment(node, content, lang);

    Some(OutlineEntry {
        kind,
        name,
        start_line,
        end_line,
        signature,
        children,
        doc,
    })
}

/// Extract a doc comment string from the AST node, language-dependently.
fn extract_doc_comment(node: Node, content: &str, lang: Lang) -> Option<String> {
    match lang {
        Lang::Rust => extract_rust_doc_comment(node, content),
        Lang::JavaScript | Lang::TypeScript | Lang::Tsx => extract_js_doc_comment(node, content),
        Lang::Python => extract_python_doc_comment(node, content),
        _ => None,
    }
}

/// Collect consecutive Rust `line_comment` siblings before a definition,
/// each containing a `doc_comment` child, and join them.
fn extract_rust_doc_comment(node: Node, content: &str) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut current = node.prev_sibling();
    while let Some(sibling) = current {
        if sibling.kind() != "line_comment" {
            break;
        }
        let mut cursor = sibling.walk();
        for child in sibling.children(&mut cursor) {
            if child.kind() == "doc_comment" {
                if let Ok(text) = child.utf8_text(content.as_bytes()) {
                    let stripped = text.trim();
                    parts.push(stripped.to_string());
                }
            }
        }
        current = sibling.prev_sibling();
    }
    if parts.is_empty() {
        return None;
    }
    parts.reverse();
    Some(parts.join("\n"))
}

/// Collect consecutive `comment` siblings that start with `/**` before
/// a JS/TS definition and strip the JSDoc delimiters.
fn extract_js_doc_comment(node: Node, content: &str) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    let mut current = node.prev_sibling();
    while let Some(sibling) = current {
        if sibling.kind() != "comment" {
            break;
        }
        if let Ok(text) = sibling.utf8_text(content.as_bytes()) {
            let trimmed = text.trim();
            if trimmed.starts_with("/**") {
                let cleaned = trimmed
                    .trim_start_matches("/**")
                    .trim_end_matches("*/")
                    .trim();
                parts.push(cleaned.to_string());
            }
        }
        current = sibling.prev_sibling();
    }
    if parts.is_empty() {
        return None;
    }
    parts.reverse();
    Some(parts.join("\n"))
}

/// Extract the first `expression_statement` containing a string node from
/// the body of a Python function/class definition — this is the docstring.
fn extract_python_doc_comment(node: Node, content: &str) -> Option<String> {
    let body = node.child_by_field_name("body")?;
    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        if child.kind() != "expression_statement" {
            continue;
        }
        let mut inner = child.walk();
        for expr in child.children(&mut inner) {
            if expr.kind() == "string" {
                return expr
                    .utf8_text(content.as_bytes())
                    .ok()
                    .map(|s| s.to_string());
            }
        }
        // Only check the first expression statement.
        break;
    }
    None
}

fn outline_kind_for(node: Node, lang: Lang) -> Option<OutlineKind> {
    if lang == Lang::Elixir && node.kind() == "call" {
        return None;
    }

    let kind = node.kind();
    let mapped = match kind {
        "type_definition" if lang == Lang::Verse => return verse_type_definition_kind(node),

        // Functions
        "function_declaration"
        | "function_definition"
        | "function_item"
        | "method_definition"
        | "method_declaration"
        | "function_expression"
        | "generator_function"
        | "extension_function_definition" => OutlineKind::Function,

        // Decorated definitions in Python — recurse into inner def.
        "decorated_definition" => return decorated_definition_kind(node),

        // Classes / structs / interfaces / traits / enums
        "class_declaration" | "class_definition" => OutlineKind::Class,
        "struct_item" | "object_declaration" => OutlineKind::Struct,
        "interface_declaration" | "trait_declaration" | "trait_item" => OutlineKind::Interface,
        "type_alias_declaration" | "type_item" | "type_declaration" => OutlineKind::TypeAlias,
        "enum_item" | "enum_declaration" => OutlineKind::Enum,

        // Modules / namespaces
        "mod_item" | "namespace_definition" | "module_declaration" => OutlineKind::Module,

        // Constants / variables
        "const_item" | "const_declaration" | "static_item" => OutlineKind::Constant,
        "lexical_declaration" => OutlineKind::Variable,
        "variable_declaration" | "var_declaration" => OutlineKind::Variable,

        // Properties (Swift, Kotlin)
        "property_declaration" => OutlineKind::Property,

        // Imports
        "import_declaration"
        | "import_statement"
        | "use_declaration"
        | "import_from_statement"
        | "package_clause"
        | "use_directive"
        | "using_declaration" => OutlineKind::Import,

        // Exports / namespace exports (TS/JS top-level only).
        "export_statement" => OutlineKind::Export,

        _ => return None,
    };

    Some(mapped)
}

fn verse_type_definition_kind(node: Node) -> Option<OutlineKind> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let mapped = match child.kind() {
            "class_definition" => OutlineKind::Class,
            "struct_definition" => OutlineKind::Struct,
            "enum_definition" => OutlineKind::Enum,
            "interface_definition" => OutlineKind::Interface,
            "module_definition" => OutlineKind::Module,
            _ => continue,
        };
        return Some(mapped);
    }
    Some(OutlineKind::TypeAlias)
}

fn decorated_definition_kind(node: Node) -> Option<OutlineKind> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(k) = outline_kind_for(child, Lang::Python) {
            return Some(k);
        }
    }
    None
}

fn collect_children(
    node: Node,
    lines: &[&str],
    content: &str,
    lang: Lang,
    out: &mut Vec<OutlineEntry>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(
            child.kind(),
            "class_body"
                | "declaration_list"
                | "block"
                | "field_declaration_list"
                | "type_definition"
                | "indented_block"
                | "colon_block"
        ) {
            let mut inner = child.walk();
            for grand in child.children(&mut inner) {
                if let Some(entry) = node_to_entry(grand, lines, content, lang) {
                    out.push(entry);
                }
            }
        } else if let Some(entry) = node_to_entry(child, lines, content, lang) {
            out.push(entry);
        }
    }
}

// Extract a single-line signature for the node — first line trimmed to ~120 chars.
fn extract_signature(node: Node, lines: &[&str]) -> Option<String> {
    if node.kind().contains("function") || node.kind().contains("method") {
        if let Some(iife) = find_iife_function(node) {
            let line = iife.start_position().row;
            if line < lines.len() {
                return Some(crate::types::truncate_str(lines[line].trim(), 120).to_string());
            }
        }
    }
    if node.kind() == "call" && elixir_arguments(node).is_some() {
        let line = node.start_position().row;
        if line < lines.len() {
            return Some(crate::types::truncate_str(lines[line].trim(), 120).to_string());
        }
    }

    let line = node.start_position().row;
    if line < lines.len() {
        let trimmed = lines[line].trim();
        if !trimmed.is_empty() {
            return Some(crate::types::truncate_str(trimmed, 120).to_string());
        }
    }
    let _ = node_text_simple(node, lines);
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find_outline_entry<'a>(entries: &'a [OutlineEntry], name: &str) -> Option<&'a OutlineEntry> {
        for entry in entries {
            if entry.name == name {
                return Some(entry);
            }
            if let Some(found) = find_outline_entry(&entry.children, name) {
                return Some(found);
            }
        }
        None
    }

    #[test]
    fn extracts_rust_function() {
        let code = "fn hello() {\n    println!(\"hi\");\n}\n";
        let entries = get_outline_entries(code, Lang::Rust);
        assert!(entries
            .iter()
            .any(|e| e.name == "hello" && matches!(e.kind, OutlineKind::Function)));
    }

    #[test]
    fn extracts_typescript_class() {
        let code = "class Foo {\n  bar(): void {}\n}\n";
        let entries = get_outline_entries(code, Lang::TypeScript);
        let foo = entries
            .iter()
            .find(|e| e.name == "Foo")
            .expect("expected Foo class");
        assert!(matches!(foo.kind, OutlineKind::Class));
        assert!(foo.children.iter().any(|c| c.name == "bar"));
    }

    #[test]
    fn handles_python_decorated_function() {
        let code = "@decorator\ndef hello():\n    pass\n";
        let entries = get_outline_entries(code, Lang::Python);
        assert!(entries
            .iter()
            .any(|e| e.name == "hello" && matches!(e.kind, OutlineKind::Function)));
    }

    // --- doc comment tests ---

    #[test]
    fn rust_doc_comment_on_function() {
        let code =
            "/// Does foo.\n///\n/// # Examples\n/// ```\n/// foo();\n/// ```\nfn foo() {}\n";
        let entries = get_outline_entries(code, Lang::Rust);
        let foo = entries
            .iter()
            .find(|e| e.name == "foo")
            .expect("expected foo");
        let doc = foo.doc.as_deref().expect("expected doc comment");
        assert!(doc.contains("Does foo."));
        assert!(doc.contains("Examples"));
    }

    #[test]
    fn rust_doc_comment_on_struct() {
        let code = "/// A point in 2D space.\nstruct Point {\n    x: i32,\n    y: i32,\n}\n";
        let entries = get_outline_entries(code, Lang::Rust);
        let pt = entries
            .iter()
            .find(|e| e.name == "Point")
            .expect("expected Point");
        let doc = pt.doc.as_deref().expect("expected doc comment");
        assert!(doc.contains("A point in 2D space."));
    }

    #[test]
    fn rust_no_doc_comment_returns_none() {
        let code = "fn bar() {}\n";
        let entries = get_outline_entries(code, Lang::Rust);
        let bar = entries
            .iter()
            .find(|e| e.name == "bar")
            .expect("expected bar");
        assert!(bar.doc.is_none());
    }

    #[test]
    fn rust_multiline_doc_comment() {
        let code = "/// Line one\n/// Line two\n/// Line three\nfn multi() {}\n";
        let entries = get_outline_entries(code, Lang::Rust);
        let multi = entries
            .iter()
            .find(|e| e.name == "multi")
            .expect("expected multi");
        let doc = multi.doc.as_deref().expect("expected doc comment");
        assert!(doc.contains("Line one"));
        assert!(doc.contains("Line two"));
        assert!(doc.contains("Line three"));
    }

    #[test]
    fn js_jsdoc_on_function() {
        let code = "/**\n * Adds two numbers.\n * @param {number} a\n * @param {number} b\n */\nfunction add(a, b) {}\n";
        let entries = get_outline_entries(code, Lang::JavaScript);
        let add = entries
            .iter()
            .find(|e| e.name == "add")
            .expect("expected add");
        let doc = add.doc.as_deref().expect("expected jsdoc");
        assert!(doc.contains("Adds two numbers."));
        assert!(doc.contains("@param"));
    }

    #[test]
    fn ts_jsdoc_on_function() {
        let code = "/** Greets the user. */\nfunction greet(name: string): void {}\n";
        let entries = get_outline_entries(code, Lang::TypeScript);
        let greet = entries
            .iter()
            .find(|e| e.name == "greet")
            .expect("expected greet");
        let doc = greet.doc.as_deref().expect("expected jsdoc");
        assert!(doc.contains("Greets the user."));
    }

    #[test]
    fn python_docstring_on_function() {
        let code = "def hello():\n    \"\"\"Greet the caller.\"\"\"\n    pass\n";
        let entries = get_outline_entries(code, Lang::Python);
        let hello = entries
            .iter()
            .find(|e| e.name == "hello")
            .expect("expected hello");
        let doc = hello.doc.as_deref().expect("expected docstring");
        assert!(doc.contains("Greet the caller."));
    }

    #[test]
    fn python_docstring_on_class() {
        let code = "class MyClass:\n    \"\"\"A class that does things.\"\"\"\n    def method(self):\n        pass\n";
        let entries = get_outline_entries(code, Lang::Python);
        let cls = entries
            .iter()
            .find(|e| e.name == "MyClass")
            .expect("expected MyClass");
        let doc = cls.doc.as_deref().expect("expected docstring");
        assert!(doc.contains("A class that does things."));
    }

    #[test]
    fn python_no_docstring_returns_none() {
        let code = "def no_doc():\n    pass\n";
        let entries = get_outline_entries(code, Lang::Python);
        let e = entries
            .iter()
            .find(|e| e.name == "no_doc")
            .expect("expected no_doc");
        assert!(e.doc.is_none());
    }

    #[test]
    fn extracts_verse_class_and_function() {
        let code = r#"using { /Verse.org/Verse }

player_character := class:
    Name:string = ""

RunExample<public>()<suspends>:void =
    Print("hi")
"#;
        let entries = get_outline_entries(code, Lang::Verse);
        assert!(
            entries.iter().any(|e| e.name == "player_character"),
            "expected player_character class"
        );
        assert!(
            entries.iter().any(|e| e.name == "RunExample"),
            "expected RunExample function"
        );
    }

    #[test]
    fn verse_if_guard_function_outline_span() {
        let code = r#"game_manager := class(creative_device):
    BindBaseComponentPlots<private>() : void =
        if (Sim := GetGameSim[]):
            Plot.SetPlayerBasesSlot(Idx)
    OnBegin() : void = {}
"#;
        let entries = get_outline_entries(code, Lang::Verse);
        let bind = find_outline_entry(&entries, "BindBaseComponentPlots")
            .expect("expected BindBaseComponentPlots in outline");
        assert!(
            bind.end_line >= 4,
            "outline end_line should cover if body, got {}",
            bind.end_line
        );
        assert!(bind.end_line < 5, "should stop before OnBegin");
    }
}
