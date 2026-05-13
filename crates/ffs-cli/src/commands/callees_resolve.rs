use std::collections::BTreeSet;

use tree_sitter::{Node, Parser};

use ffs_symbol::outline_language;
use ffs_symbol::types::Lang;

// Returns the set of identifier names referenced as call-targets inside lines
// `[start_line, end_line]` of `content` parsed under `lang`.
//
// "Call target" covers the AST nodes most languages use for a call expression,
// e.g. `call_expression`, `call`, `method_invocation`, `invocation_expression`,
// `member_call_expression`, plus `macro_invocation` for Rust. The callee
// identifier is taken from the conventional field (`function`, `name`,
// `callee`) and reduced to its rightmost name segment, so `mod::Foo::bar()`
// and `obj.bar()` both yield `bar` — the form the symbol index keys on.
pub fn collect_callees(
    content: &str,
    lang: Lang,
    start_line: u32,
    end_line: u32,
) -> Option<BTreeSet<String>> {
    let language = outline_language(lang)?;
    let mut parser = Parser::new();
    parser.set_language(&language).ok()?;
    let tree = parser.parse(content, None)?;
    let source = content.as_bytes();

    let mut out: BTreeSet<String> = BTreeSet::new();
    walk(tree.root_node(), source, start_line, end_line, &mut out);
    Some(out)
}

fn walk(node: Node, src: &[u8], start: u32, end: u32, out: &mut BTreeSet<String>) {
    let row = node.start_position().row as u32 + 1;
    if row > end {
        return;
    }

    if node_is_call(node) && row >= start {
        if let Some(name) = extract_callee_name(node, src) {
            out.insert(name);
        }
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, src, start, end, out);
    }
}

fn node_is_call(node: Node) -> bool {
    matches!(
        node.kind(),
        "call_expression"
            | "call"
            | "method_invocation"
            | "method_call"
            | "function_call_expression"
            | "member_call_expression"
            | "invocation_expression"
            | "new_expression"
            | "object_creation_expression"
            | "macro_invocation"
            | "scoped_call"
            | "function_call"
    )
}

fn extract_callee_name(call: Node, src: &[u8]) -> Option<String> {
    if call.kind() == "macro_invocation" {
        if let Some(target) = call
            .child_by_field_name("macro")
            .or_else(|| call.child_by_field_name("name"))
        {
            return rightmost_name(target, src);
        }
    }
    for field in ["function", "name", "callee", "method"] {
        if let Some(target) = call.child_by_field_name(field) {
            return rightmost_name(target, src);
        }
    }
    // Fallback: first named child.
    call.named_child(0).and_then(|c| rightmost_name(c, src))
}

fn rightmost_name(node: Node, src: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier"
        | "type_identifier"
        | "field_identifier"
        | "property_identifier"
        | "shorthand_property_identifier"
        | "shorthand_property_identifier_pattern"
        | "constant"
        | "name"
        | "simple_identifier" => text_of(node, src),
        "member_expression"
        | "member_access_expression"
        | "field_expression"
        | "field_access"
        | "scoped_identifier"
        | "scoped_type_identifier"
        | "selector_expression"
        | "qualified_name"
        | "path"
        | "scoped_call_expression"
        | "subscript_expression" => {
            for field in ["name", "property", "field", "selector"] {
                if let Some(child) = node.child_by_field_name(field) {
                    return rightmost_name(child, src);
                }
            }
            (0..node.named_child_count())
                .rev()
                .find_map(|i| node.named_child(i))
                .and_then(|c| rightmost_name(c, src))
        }
        _ => {
            if node.named_child_count() > 0 {
                node.named_child(node.named_child_count() - 1)
                    .and_then(|c| rightmost_name(c, src))
            } else {
                text_of(node, src)
            }
        }
    }
}

fn text_of(node: Node, src: &[u8]) -> Option<String> {
    node.utf8_text(src).ok().map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_collects_simple_call() {
        let src = r#"
fn outer() {
    foo();
    bar(1, 2);
}
"#;
        let names = collect_callees(src, Lang::Rust, 2, 5).expect("parsed");
        assert!(names.contains("foo"));
        assert!(names.contains("bar"));
    }

    #[test]
    fn rust_collects_method_and_path() {
        let src = r#"
fn outer() {
    obj.do_thing();
    crate::module::helper();
}
"#;
        let names = collect_callees(src, Lang::Rust, 2, 5).expect("parsed");
        assert!(names.contains("do_thing"), "got: {names:?}");
        assert!(names.contains("helper"), "got: {names:?}");
    }

    #[test]
    fn rust_macros_are_collected() {
        let src = r#"
fn outer() {
    println!("hi");
    vec![1, 2];
}
"#;
        let names = collect_callees(src, Lang::Rust, 2, 5).expect("parsed");
        assert!(names.contains("println"), "got: {names:?}");
        assert!(names.contains("vec"), "got: {names:?}");
    }

    #[test]
    fn line_window_excludes_calls_outside_range() {
        let src = r#"
fn outer() {
    inside_a();
}

fn other() {
    outside_b();
}
"#;
        let names = collect_callees(src, Lang::Rust, 2, 4).expect("parsed");
        assert!(names.contains("inside_a"));
        assert!(!names.contains("outside_b"));
    }

    #[test]
    fn python_method_invocation_resolves_to_rightmost() {
        let src = r#"
def outer():
    obj.do_thing()
    pkg.mod.helper(1)
"#;
        let names = collect_callees(src, Lang::Python, 2, 4).expect("parsed");
        assert!(names.contains("do_thing"), "got: {names:?}");
        assert!(names.contains("helper"), "got: {names:?}");
    }

    #[test]
    fn typescript_call_and_new_collected() {
        let src = r#"
function outer() {
    foo();
    new Bar();
    obj.qux();
}
"#;
        let names = collect_callees(src, Lang::TypeScript, 2, 6).expect("parsed");
        assert!(names.contains("foo"), "got: {names:?}");
        assert!(names.contains("Bar"), "got: {names:?}");
        assert!(names.contains("qux"), "got: {names:?}");
    }
}
