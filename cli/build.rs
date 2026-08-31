//! Compiles the tree-sitter parser into the binary.
//!
//! `tree-sitter-hek/src/` is generated and committed (`tree-sitter-hek/README.md` has why:
//! nix's `buildGrammar` compiles `parser.c` and never runs `generate`). That is what lets
//! this need a C compiler rather than the tree-sitter CLI and node. The obligation it
//! creates is the one the README already states: regenerate and commit `src/` with any
//! grammar change, or the formatter parses a language nobody is writing.

use std::path::Path;

fn main() {
    let src = Path::new("../tree-sitter-hek/src");
    println!("cargo::rerun-if-changed={}", src.join("parser.c").display());
    cc::Build::new()
        .include(src)
        .file(src.join("parser.c"))
        // Generated, and its warnings are not ours to answer.
        .warnings(false)
        .compile("tree-sitter-hek");
}
