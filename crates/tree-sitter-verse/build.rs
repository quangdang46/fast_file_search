use std::path::Path;

fn main() {
    let src_dir = Path::new("src");
    cc::Build::new()
        .include(src_dir)
        .file(src_dir.join("parser.c"))
        .file(src_dir.join("scanner.c"))
        .warnings(false)
        .compile("tree-sitter-verse");
}
