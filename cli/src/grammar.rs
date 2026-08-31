//! The tree-sitter grammar, linked in by `build.rs`.
//!
//! heklang has two front ends and this is the second one. `heklang::parse` lowers straight
//! to IR and throws comments away with the rest of the trivia (`src/lex.rs` `skip_trivia`),
//! which is right for a checker and useless for a formatter: there is no tree to print back
//! and no comment to print. The grammar keeps every byte, so `hek fmt` reads through here
//! and `hek check` does not.
//!
//! The two disagree in one direction on purpose. `grammar.js` has no idea whether it is
//! inside a command or a projector, so it accepts `put` in a command and `emit` in a `fn`:
//! nothing valid fails to parse, and some invalid programs do parse. That is the safe
//! direction for a formatter, which needs a tree for everything an author can write and is
//! not the thing that decides whether it means anything. `check` is still the gate.

use tree_sitter::{Parser, Tree};
use tree_sitter_language::LanguageFn;

unsafe extern "C" {
    fn tree_sitter_hek() -> *const ();
}

/// The compiled grammar. ABI 14, which is what Helix loads and what `tree-sitter` accepts
/// (it takes 13 through 15), so one generated parser serves the editor and this.
pub const LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_hek) };

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
