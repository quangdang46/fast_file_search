//! Shared tree-sitter helpers used by symbol search and outline extraction.

/// Definition node kinds recognized across tree-sitter grammars.
pub const DEFINITION_KINDS: &[&str] = &[
    "function_declaration",
    "function_definition",
    "function_item",
    "function_expression",
    "generator_function",
    "method_definition",
    "method_declaration",
    "class_declaration",
    "class_definition",
    "struct_item",
    "object_declaration",
    "interface_declaration",
    "trait_declaration",
    "type_alias_declaration",
    "type_item",
    "enum_item",
    "enum_declaration",
    "lexical_declaration",
    "variable_declaration",
    "const_item",
    "const_declaration",
    "static_item",
    "property_declaration",
    "trait_item",
    "impl_item",
    "mod_item",
    "namespace_definition",
    "decorated_definition",
    "type_declaration",
    "export_statement",
];

/// Extract the name defined by a tree-sitter definition node.
pub fn extract_definition_name(node: tree_sitter::Node, lines: &[&str]) -> Option<String> {
    for field in &["name", "identifier", "declarator"] {
        if let Some(child) = node.child_by_field_name(field) {
            if child.kind().contains("declarator") {
                if let Some(name) = extract_declarator_name(child, lines) {
                    return Some(name);
                }
            }
            let text = node_text_simple(child, lines);
            if !text.is_empty() {
                return Some(text);
            }
        }
    }

    if node.kind() == "export_statement" || node.kind() == "decorated_definition" {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if DEFINITION_KINDS.contains(&child.kind()) {
                return extract_definition_name(child, lines);
            }
        }
    }

    if matches!(node.kind(), "lexical_declaration" | "variable_declaration") {
        return extract_first_variable_declarator_name(node, lines);
    }

    None
}

pub fn extract_variable_declarator_name(node: tree_sitter::Node, lines: &[&str]) -> Option<String> {
    if let Some(name) = node.child_by_field_name("name") {
        let text = node_text_simple(name, lines);
        if !text.is_empty() {
            return Some(text);
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind().contains("identifier") {
            let text = node_text_simple(child, lines);
            if !text.is_empty() {
                return Some(text);
            }
        }
    }

    None
}

fn extract_first_variable_declarator_name(
    node: tree_sitter::Node,
    lines: &[&str],
) -> Option<String> {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "variable_declarator" {
            return extract_variable_declarator_name(child, lines);
        }
    }
    None
}

pub fn is_js_function_expression_kind(kind: &str) -> bool {
    matches!(
        kind,
        "function_expression" | "generator_function" | "arrow_function"
    )
}

pub fn is_iife_function(node: tree_sitter::Node) -> bool {
    if !is_js_function_expression_kind(node.kind()) {
        return false;
    }

    if let Some(parent) = node.parent() {
        if parent.kind() == "call_expression" && call_invokes_node(parent, node) {
            return true;
        }

        if parent.kind() == "parenthesized_expression" {
            if let Some(grandparent) = parent.parent() {
                return grandparent.kind() == "call_expression"
                    && call_invokes_node(grandparent, parent);
            }
        }
    }

    false
}

fn call_invokes_node(call: tree_sitter::Node, function_node: tree_sitter::Node) -> bool {
    call.child_by_field_name("function")
        .is_some_and(|function| function.id() == function_node.id())
}

pub fn js_function_context_name(node: tree_sitter::Node, lines: &[&str]) -> Option<String> {
    if let Some(name) = node.child_by_field_name("name") {
        let text = node_text_simple(name, lines);
        if !text.is_empty() {
            return Some(text);
        }
    }

    let mut current = node.parent();
    for _ in 0..6 {
        let Some(parent) = current else { break };
        if parent.kind() == "variable_declarator" {
            return extract_variable_declarator_name(parent, lines);
        }
        current = parent.parent();
    }

    if is_iife_function(node) {
        return Some(format!("<iife@{}>", node.start_position().row + 1));
    }

    None
}

pub fn find_iife_function(node: tree_sitter::Node) -> Option<tree_sitter::Node> {
    find_iife_function_inner(node, 0)
}

fn find_iife_function_inner(node: tree_sitter::Node, depth: usize) -> Option<tree_sitter::Node> {
    if depth > 6 {
        return None;
    }
    if is_iife_function(node) {
        return Some(node);
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(iife) = find_iife_function_inner(child, depth + 1) {
            return Some(iife);
        }
    }

    None
}

pub fn extract_declarator_name(node: tree_sitter::Node, lines: &[&str]) -> Option<String> {
    let kind = node.kind();
    if matches!(
        kind,
        "identifier" | "field_identifier" | "type_identifier" | "operator_name"
    ) || kind.ends_with("identifier")
    {
        let text = node_text_simple(node, lines);
        return (!text.is_empty()).then_some(text);
    }

    for field in ["declarator", "name", "identifier"] {
        if let Some(child) = node.child_by_field_name(field) {
            if let Some(name) = extract_declarator_name(child, lines) {
                return Some(name);
            }
            let text = node_text_simple(child, lines);
            if !text.is_empty() {
                return Some(text);
            }
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        let child_kind = child.kind();
        if matches!(
            child_kind,
            "parameter_list"
                | "parameter_declaration"
                | "compound_statement"
                | "declaration_list"
                | "type"
                | "primitive_type"
        ) {
            continue;
        }
        if child_kind.contains("declarator")
            || child_kind.contains("identifier")
            || matches!(child_kind, "qualified_identifier" | "scoped_identifier")
        {
            if let Some(name) = extract_declarator_name(child, lines) {
                return Some(name);
            }
        }
    }

    None
}

/// Return the text slice of a single-line node, or first line for multi-line.
pub fn node_text_simple(node: tree_sitter::Node, lines: &[&str]) -> String {
    let row = node.start_position().row;
    let col_start = node.start_position().column;
    let end_row = node.end_position().row;
    if row < lines.len() && row == end_row {
        let col_end = node.end_position().column.min(lines[row].len());
        // Walk back to char boundary safety: tree-sitter columns are byte
        // counts, but we still must guard against invalid UTF-8 splits.
        let line = lines[row];
        let safe_start = adjust_to_char_boundary(line, col_start);
        let safe_end = adjust_to_char_boundary(line, col_end);
        line[safe_start..safe_end].to_string()
    } else if row < lines.len() {
        let line = lines[row];
        let safe_start = adjust_to_char_boundary(line, col_start);
        line[safe_start..].to_string()
    } else {
        String::new()
    }
}

fn adjust_to_char_boundary(s: &str, mut idx: usize) -> usize {
    if idx > s.len() {
        idx = s.len();
    }
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// Extract trait name from Rust `impl Trait for Type` node.
pub fn extract_impl_trait(node: tree_sitter::Node, lines: &[&str]) -> Option<String> {
    let trait_node = node.child_by_field_name("trait")?;
    Some(node_text_simple(trait_node, lines))
}

/// Extract implementing type from Rust `impl ... for Type` node.
pub fn extract_impl_type(node: tree_sitter::Node, lines: &[&str]) -> Option<String> {
    let type_node = node.child_by_field_name("type")?;
    Some(node_text_simple(type_node, lines))
}

/// Extract implemented interfaces from TS/Java class declarations.
pub fn extract_implemented_interfaces(node: tree_sitter::Node, lines: &[&str]) -> Vec<String> {
    let mut interfaces = Vec::new();
    collect_implemented_interface_clauses(node, lines, &mut interfaces);
    interfaces
}

fn collect_implemented_interface_clauses(
    node: tree_sitter::Node,
    lines: &[&str],
    out: &mut Vec<String>,
) {
    if node.kind() == "implements_clause" || node.kind() == "super_interfaces" {
        collect_identifier_texts(node, lines, out);
        return;
    }
    if node.kind() == "class_body" {
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_implemented_interface_clauses(child, lines, out);
    }
}

fn collect_identifier_texts(node: tree_sitter::Node, lines: &[&str], out: &mut Vec<String>) {
    if node.kind().contains("identifier") {
        let text = node_text_simple(node, lines);
        if !text.is_empty() {
            out.push(text);
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_identifier_texts(child, lines, out);
    }
}

// ---------------------------------------------------------------------------
// Elixir-specific helpers
// ---------------------------------------------------------------------------

const ELIXIR_DEFINITION_TARGETS: &[&str] = &[
    "defmodule",
    "def",
    "defp",
    "defmacro",
    "defmacrop",
    "defguard",
    "defguardp",
    "defdelegate",
    "defstruct",
    "defexception",
    "defprotocol",
    "defimpl",
];

pub fn elixir_arguments(node: tree_sitter::Node) -> Option<tree_sitter::Node> {
    let mut cursor = node.walk();
    let result = node.children(&mut cursor).find(|c| c.kind() == "arguments");
    result
}

pub fn is_elixir_definition(node: tree_sitter::Node, lines: &[&str]) -> bool {
    if node.kind() != "call" {
        return false;
    }
    let Some(target) = node.child_by_field_name("target") else {
        return false;
    };
    let kw = node_text_simple(target, lines);
    ELIXIR_DEFINITION_TARGETS.contains(&kw.as_str())
}

pub fn extract_elixir_definition_name(node: tree_sitter::Node, lines: &[&str]) -> Option<String> {
    let target = node.child_by_field_name("target")?;
    let kw = node_text_simple(target, lines);
    let args = elixir_arguments(node)?;

    match kw.as_str() {
        "defmodule" | "defprotocol" | "defimpl" => {
            let mut cursor = args.walk();
            for child in args.children(&mut cursor) {
                if child.is_named() {
                    return Some(node_text_simple(child, lines));
                }
            }
            None
        }
        "def" | "defp" | "defmacro" | "defmacrop" | "defguard" | "defguardp" | "defdelegate" => {
            let mut cursor = args.walk();
            for child in args.children(&mut cursor) {
                if !child.is_named() {
                    continue;
                }
                return elixir_extract_func_head_name(child, lines);
            }
            None
        }
        "defstruct" | "defexception" => Some(kw.clone()),
        _ => None,
    }
}

pub fn elixir_extract_func_head_name(node: tree_sitter::Node, lines: &[&str]) -> Option<String> {
    match node.kind() {
        "call" => node
            .child_by_field_name("target")
            .map(|t| node_text_simple(t, lines)),
        "identifier" => Some(node_text_simple(node, lines)),
        "binary_operator" => {
            let left = node.child_by_field_name("left")?;
            elixir_extract_func_head_name(left, lines)
        }
        _ => None,
    }
}

/// Semantic weight for definition kinds. Primary declarations rank highest.
pub fn definition_weight(kind: &str) -> u16 {
    match kind {
        "function_declaration"
        | "function_definition"
        | "function_item"
        | "function_expression"
        | "generator_function"
        | "method_definition"
        | "method_declaration"
        | "class_declaration"
        | "class_definition"
        | "struct_item"
        | "interface_declaration"
        | "trait_declaration"
        | "trait_item"
        | "enum_item"
        | "enum_declaration"
        | "type_item"
        | "type_declaration"
        | "decorated_definition" => 100,
        "impl_item" | "object_declaration" => 90,
        "const_item" | "const_declaration" | "static_item" => 80,
        "mod_item" | "namespace_definition" | "property_declaration" => 70,
        "lexical_declaration" | "variable_declaration" => 40,
        "export_statement" => 30,
        _ => 50,
    }
}
