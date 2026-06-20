/// Role detection for files — classifies a file's purpose based on its path.
///
/// Used by `ffs find` to apply role-based score adjustments (boost
/// implementation/auth/ui files, penalize docs/tests).
use std::path::Path;

/// The semantic role of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Role {
    /// Main source code (under `src/` or similar).
    Implementation,
    /// Test file (contains `test` in path).
    Test,
    /// Documentation (markdown, docs/ dir).
    Docs,
    /// UI/TUI component.
    Ui,
    /// Authentication/authorization logic.
    Auth,
    /// Provider/backend implementation.
    Provider,
    /// Configuration files.
    Config,
    /// Handler/router.
    Handler,
    /// Build scripts, CI configs.
    Build,
    /// Everything else.
    Generic,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Role::Implementation => "implementation",
            Role::Test => "test",
            Role::Docs => "docs",
            Role::Ui => "ui",
            Role::Auth => "auth",
            Role::Provider => "provider",
            Role::Config => "config",
            Role::Handler => "handler",
            Role::Build => "build",
            Role::Generic => "generic",
        }
    }

    /// Score adjustment for this role. Positive = boost, negative = penalty.
    pub fn score_bonus(&self) -> i32 {
        match self {
            Role::Implementation => 20,
            Role::Test => -15,
            Role::Docs => -25,
            Role::Ui => 20,
            Role::Auth => 20,
            Role::Provider => 15,
            Role::Config => 0,
            Role::Handler => 15,
            Role::Build => -10,
            Role::Generic => 0,
        }
    }
}

/// Detect the role of a file from its relative path.
pub fn detect_role(path: &Path) -> Role {
    let path_str = path.to_string_lossy().to_ascii_lowercase();
    let path_str = path_str.as_str();

    // Check file extension first
    let is_markdown = path_str.ends_with(".md") || path_str.ends_with(".mdx");
    let is_build_script = path_str.ends_with("build.rs")
        || path_str.contains("/ci/")
        || path_str.contains(".github/workflows");

    // Check directory patterns
    if is_build_script {
        return Role::Build;
    }
    if path_str.contains("/tests/")
        || path_str.contains("/test/")
        || path_str.contains("_test.")
        || path_str.contains("_spec.")
        || path_str.ends_with("_test.rs")
        || path_str.ends_with("_spec.rs")
        || path_str.ends_with("test.rs")
    {
        return Role::Test;
    }
    if is_markdown || path_str.contains("/docs/") || path_str.contains("/doc/") {
        return Role::Docs;
    }
    if path_str.contains("/tui/")
        || path_str.contains("/ui/")
        || path_str.contains("/screen/")
        || path_str.contains("/component/")
        || path_str.contains("/widget/")
    {
        return Role::Ui;
    }
    if path_str.contains("/auth") || path_str.contains("oauth") || path_str.contains("login") {
        return Role::Auth;
    }
    if path_str.contains("/provider") || path_str.contains("/backend/") {
        return Role::Provider;
    }
    if path_str.contains("/config")
        || path_str.ends_with(".toml")
        || path_str.ends_with(".json")
        || path_str.ends_with(".yaml")
        || path_str.ends_with(".yml")
    {
        return Role::Config;
    }
    if path_str.contains("/handler")
        || path_str.contains("/router")
        || path_str.contains("/middleware")
    {
        return Role::Handler;
    }
    if path_str.contains("/src/") {
        return Role::Implementation;
    }

    Role::Generic
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn detects_test_files() {
        assert_eq!(detect_role(Path::new("src/tests/mod.rs")), Role::Test);
        assert_eq!(
            detect_role(Path::new("crates/foo/src/bar_test.rs")),
            Role::Test
        );
    }

    #[test]
    fn detects_docs() {
        assert_eq!(detect_role(Path::new("README.md")), Role::Docs);
        assert_eq!(detect_role(Path::new("docs/guide.md")), Role::Docs);
    }

    #[test]
    fn detects_ui() {
        assert_eq!(detect_role(Path::new("src/tui/screen.rs")), Role::Ui);
        assert_eq!(detect_role(Path::new("src/ui/components.rs")), Role::Ui);
    }

    #[test]
    fn detects_auth() {
        assert_eq!(detect_role(Path::new("src/auth/mod.rs")), Role::Auth);
        assert_eq!(detect_role(Path::new("src/oauth.rs")), Role::Auth);
    }

    #[test]
    fn detects_implementation() {
        assert_eq!(
            detect_role(Path::new("src/lib.rs")),
            Role::Implementation
        );
        assert_eq!(
            detect_role(Path::new("crates/foo/src/bar.rs")),
            Role::Implementation
        );
    }

    #[test]
    fn score_bonus_values() {
        assert!(Role::Implementation.score_bonus() > 0);
        assert!(Role::Test.score_bonus() < 0);
        assert!(Role::Docs.score_bonus() < 0);
        assert!(Role::Generic.score_bonus() == 0);
    }
}
