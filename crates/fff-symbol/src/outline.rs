//! Tree-sitter driven outline extraction (functions, classes, modules, …)
//! for the languages scry supports.

use tree_sitter::{Language, Node, Parser};

use crate::treesitter::{
    elixir_arguments, extract_definition_name, extract_elixir_definition_name, find_iife_function,
    is_elixir_definition, js_function_context_name, node_text_simple,
};
use crate::types::{Lang, OutlineEntry, OutlineKind};

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
    walk_top_level(root, &lines, lang, &mut out);
    out
}

fn walk_top_level(node: Node, lines: &[&str], lang: Lang, out: &mut Vec<OutlineEntry>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(entry) = node_to_entry(child, lines, lang) {
            out.push(entry);
        }
    }
}

fn node_to_entry(node: Node, lines: &[&str], lang: Lang) -> Option<OutlineEntry> {
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
    let end_line = node.end_position().row as u32 + 1;

    let signature = extract_signature(node, lines);

    let mut children = Vec::new();
    if matches!(
        kind,
        OutlineKind::Class | OutlineKind::Struct | OutlineKind::Interface | OutlineKind::Module
    ) {
        collect_children(node, lines, lang, &mut children);
    }

    Some(OutlineEntry {
        kind,
        name,
        start_line,
        end_line,
        signature,
        children,
        doc: None,
    })
}

fn outline_kind_for(node: Node, lang: Lang) -> Option<OutlineKind> {
    if lang == Lang::Elixir && node.kind() == "call" {
        return None;
    }

    let kind = node.kind();
    let mapped = match kind {
        // Functions
        "function_declaration"
        | "function_definition"
        | "function_item"
        | "method_definition"
        | "method_declaration"
        | "function_expression"
        | "generator_function" => OutlineKind::Function,

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
        "variable_declaration" => OutlineKind::Variable,

        // Properties (Swift, Kotlin)
        "property_declaration" => OutlineKind::Property,

        // Imports
        "import_declaration"
        | "import_statement"
        | "use_declaration"
        | "import_from_statement"
        | "package_clause"
        | "use_directive" => OutlineKind::Import,

        // Exports / namespace exports (TS/JS top-level only).
        "export_statement" => OutlineKind::Export,

        _ => return None,
    };

    Some(mapped)
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

fn collect_children(node: Node, lines: &[&str], lang: Lang, out: &mut Vec<OutlineEntry>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if matches!(
            child.kind(),
            "class_body"
                | "declaration_list"
                | "block"
                | "field_declaration_list"
                | "type_definition"
        ) {
            let mut inner = child.walk();
            for grand in child.children(&mut inner) {
                if let Some(entry) = node_to_entry(grand, lines, lang) {
                    out.push(entry);
                }
            }
        } else if let Some(entry) = node_to_entry(child, lines, lang) {
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
}
