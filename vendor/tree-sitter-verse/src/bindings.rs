use tree_sitter_language::LanguageFn;

unsafe extern "C" {
    fn tree_sitter_verse() -> *const ();
}

/// Tree-sitter [`LanguageFn`] for Verse (`.verse`).
pub const LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_verse) };
