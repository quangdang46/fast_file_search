//! Indentation-based span repair for Verse definitions whose tree-sitter
//! `end_position` stops early.
//!
//! A common failure mode: `function_definition` gets an empty `indented_block`
//! body while the real statements (`if_expression`, `for_expression`, …) parse
//! as sibling class members at the same source lines. Symbol/callee/read tools
//! key off `(start_line, end_line)`, so we extend the span through indented
//! continuation lines.

use tree_sitter::Node;

use crate::treesitter::extract_definition_name;

/// Column of the first non-whitespace character (tabs expand to 8 columns).
pub fn line_indent(line: &str) -> u32 {
    let mut col = 0u32;
    for ch in line.chars() {
        match ch {
            ' ' => col += 1,
            '\t' => col = (col + 8) & !7,
            _ => break,
        }
    }
    col
}

fn is_blank_or_comment(line: &str) -> bool {
    let t = line.trim();
    t.is_empty() || t.starts_with('#') || t.starts_with("<#")
}

/// Extend `[start_line, ast_end]` through lines indented deeper than the
/// definition header. Stops at the first non-blank line whose indent is
/// `<= base_indent` (a sibling member or module-level item).
pub fn verse_member_end_line(lines: &[&str], start_line: u32, ast_end: u32) -> u32 {
    let start_idx = start_line.saturating_sub(1) as usize;
    if start_idx >= lines.len() {
        return ast_end;
    }
    let base = line_indent(lines[start_idx]);
    let mut end = ast_end.max(start_line);
    for (idx, line) in lines.iter().enumerate().skip(start_idx + 1) {
        if is_blank_or_comment(line) {
            end = (idx as u32) + 1;
            continue;
        }
        let indent = line_indent(line);
        if indent <= base {
            break;
        }
        end = (idx as u32) + 1;
    }
    end
}

/// Whether a Verse definition node likely needs span repair.
#[cfg(test)]
pub fn verse_should_repair_span(
    node_kind: &str,
    start_line: u32,
    ast_end: u32,
    lines: &[&str],
) -> bool {
    if !matches!(
        node_kind,
        "function_definition" | "extension_function_definition" | "type_definition"
    ) {
        return false;
    }
    if ast_end <= start_line {
        return true;
    }
    let start_idx = start_line.saturating_sub(1) as usize;
    if start_idx >= lines.len() {
        return false;
    }
    let base = line_indent(lines[start_idx]);
    // Header-only span, or empty body with deeper-indented continuation.
    if ast_end <= start_line + 1 {
        let next_idx = ast_end as usize;
        if next_idx < lines.len() {
            let next = lines[next_idx];
            if !is_blank_or_comment(next) && line_indent(next) > base {
                return true;
            }
        }
    }
    false
}

/// Skip `@editable` metadata lines parsed as spurious `Categories` type defs.
pub fn verse_skip_spurious_definition(node: Node, lines: &[&str]) -> bool {
    if node.kind() != "type_definition" {
        return false;
    }
    let Some(name) = extract_definition_name(node, lines) else {
        return false;
    };
    if name != "Categories" {
        return false;
    }
    let line = node.start_position().row;
    lines
        .get(line)
        .is_some_and(|l| l.contains("Categories := array{SettingsCategory}"))
}

/// Apply span repair when appropriate.
pub fn verse_repair_end_line(
    node_kind: &str,
    start_line: u32,
    ast_end: u32,
    lines: &[&str],
) -> u32 {
    if matches!(
        node_kind,
        "function_definition" | "extension_function_definition" | "type_definition"
    ) {
        verse_member_end_line(lines, start_line, ast_end)
    } else {
        ast_end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extends_if_guard_function_body() {
        let src = r#"game_manager := class(creative_device):
    BindBaseComponentPlots<private>() : void =
        if (Sim := GetGameSim[]):
            for (Idx -> Plot : PlayerBases):
                Plot.SetPlayerBasesSlot(Idx)
            if (Tz := TeleportZoneManager?):
                Pad.BindTeleportManager(Tz)
    OnBegin<override>()<suspends> : void =
        BindBaseComponentPlots()
"#;
        let lines: Vec<&str> = src.lines().collect();
        // Simulate truncated AST ending on the `if` header line (1-based line 3).
        let end = verse_repair_end_line("function_definition", 2, 3, &lines);
        assert!(
            end >= 6,
            "expected body through nested if/for, got end_line {end}"
        );
        assert!(
            end < 8,
            "should stop before sibling OnBegin at line 8, got {end}"
        );
    }

    #[test]
    fn extends_truncated_class_type_definition() {
        let src = r#"game_manager := class(creative_device):
    @editable
    PetDevice : ?pet_device = false
    OnBegin() : void = {}
(Agent : agent).AddToTeam(T : int) : void = {}
"#;
        let lines: Vec<&str> = src.lines().collect();
        let end = verse_repair_end_line("type_definition", 1, 1, &lines);
        assert!(end >= 4, "class should include members, got {end}");
        assert!(
            end < 6,
            "should stop before module-level AddToTeam, got {end}"
        );
    }

    #[test]
    fn leaves_short_function_untouched_when_ast_span_covers_body() {
        let src = "Run():void=\n    Print(\"ok\")\n";
        let lines: Vec<&str> = src.lines().collect();
        let end = verse_repair_end_line("function_definition", 1, 2, &lines);
        assert_eq!(end, 2);
    }
}
