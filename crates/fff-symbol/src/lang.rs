//! Language detection by extension and filename.

use std::path::Path;

use crate::types::{FileType, Lang};

/// Detect file type by extension, then by name.
pub fn detect_file_type(path: &Path) -> FileType {
    match path.extension().and_then(|e| e.to_str()) {
        Some("ts") => FileType::Code(Lang::TypeScript),
        Some("tsx") => FileType::Code(Lang::Tsx),
        Some("js" | "jsx" | "mjs" | "cjs") => FileType::Code(Lang::JavaScript),
        Some("py" | "pyi") => FileType::Code(Lang::Python),
        Some("rs") => FileType::Code(Lang::Rust),
        Some("go") => FileType::Code(Lang::Go),
        Some("java") => FileType::Code(Lang::Java),
        Some("scala" | "sc") => FileType::Code(Lang::Scala),
        Some("c" | "h") => FileType::Code(Lang::C),
        Some("cpp" | "hpp" | "cc" | "cxx" | "hh" | "hxx") => FileType::Code(Lang::Cpp),
        Some("rb") => FileType::Code(Lang::Ruby),
        Some("php" | "phtml") => FileType::Code(Lang::Php),
        Some("swift") => FileType::Code(Lang::Swift),
        Some("kt" | "kts") => FileType::Code(Lang::Kotlin),
        Some("cs") => FileType::Code(Lang::CSharp),
        Some("ex" | "exs") => FileType::Code(Lang::Elixir),

        Some("md" | "mdx" | "rst") => FileType::Markdown,
        Some("json" | "yaml" | "yml" | "toml" | "xml" | "ini") => FileType::StructuredData,
        Some("csv" | "tsv") => FileType::Tabular,
        Some("log") => FileType::Log,

        None => file_type_from_name(path),
        _ => FileType::Other,
    }
}

fn file_type_from_name(path: &Path) -> FileType {
    match path.file_name().and_then(|n| n.to_str()) {
        Some("Dockerfile" | "Containerfile") => FileType::Code(Lang::Dockerfile),
        Some("Makefile" | "GNUmakefile") => FileType::Code(Lang::Make),
        Some("Vagrantfile" | "Rakefile") => FileType::Code(Lang::Ruby),
        Some(n) if n.starts_with(".env") => FileType::StructuredData,
        _ => FileType::Other,
    }
}

/// Walk parents looking for the nearest manifest.
pub fn package_root(path: &Path) -> Option<&Path> {
    const MANIFESTS: &[&str] = &[
        "Cargo.toml",
        "package.json",
        "pyproject.toml",
        "setup.py",
        "go.mod",
        "pom.xml",
        "build.gradle",
        "build.sbt",
        "mix.exs",
    ];
    let mut dir = path;
    loop {
        for m in MANIFESTS {
            if dir.join(m).exists() {
                return Some(dir);
            }
        }
        dir = dir.parent()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_rust() {
        assert_eq!(
            detect_file_type(Path::new("/x/y/foo.rs")),
            FileType::Code(Lang::Rust)
        );
    }

    #[test]
    fn detect_typescript_variants() {
        assert_eq!(
            detect_file_type(Path::new("a.ts")),
            FileType::Code(Lang::TypeScript)
        );
        assert_eq!(
            detect_file_type(Path::new("a.tsx")),
            FileType::Code(Lang::Tsx)
        );
        assert_eq!(
            detect_file_type(Path::new("a.js")),
            FileType::Code(Lang::JavaScript)
        );
    }

    #[test]
    fn detect_dockerfile_by_name() {
        assert_eq!(
            detect_file_type(Path::new("/etc/Dockerfile")),
            FileType::Code(Lang::Dockerfile)
        );
    }
}
