//! The tree-sitter grammar, as `hek` uses it.
//!
//! The grammar itself is the `tree-sitter-hek` crate, which compiles the committed
//! `parser.c` and hands back a `LanguageFn`. What lives here is the one thing `hek` wants
//! from it that a grammar crate has no business deciding: whether a tree is good enough to
//! print back out.

use tree_sitter::{Parser, Tree};

pub use tree_sitter_hek::LANGUAGE;

/// The tree for one module, or `None` when the source does not parse.
///
/// `None` covers an `ERROR` or a `MISSING` node anywhere in the tree, not just a failure to
/// produce one at all: tree-sitter recovers from a syntax error and hands back a tree with
/// the damage recorded in it, and a formatter that printed such a tree would write the
/// damage back out as though it were the program.
pub fn parse(source: &str) -> Option<Tree> {
    let mut parser = Parser::new();
    parser
        .set_language(&LANGUAGE.into())
        .expect("the committed parser is ABI 14 and this tree-sitter accepts 13 through 15");
    let tree = parser.parse(source, None)?;
    (!tree.root_node().has_error()).then_some(tree)
}
