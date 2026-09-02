//! The tree-sitter grammar for heklang, as a Rust crate.
//!
//! heklang has two front ends and this is the second one. `heklang::parse` lowers straight
//! to IR and throws comments away with the rest of the trivia, which is right for a checker
//! and useless for a formatter: there is no tree to print back and no comment to print. The
//! grammar keeps every byte, so `hek fmt` reads through here and `hek check` does not.
//!
//! The two disagree in one direction on purpose. `grammar.js` has no idea whether it is
//! inside a command or a projector, so it accepts `put` in a command and `emit` in a `fn`:
//! nothing valid fails to parse, and some invalid programs do parse. That is the safe
//! direction for a formatter, which needs a tree for everything an author can write and is
//! not the thing that decides whether it means anything. `check` is still the gate.

use tree_sitter_language::LanguageFn;

unsafe extern "C" {
    fn tree_sitter_hek() -> *const ();
}

/// The compiled grammar. ABI 14, which is what Helix loads and what `tree-sitter` accepts
/// (it takes 13 through 15), so one generated parser serves the editor and this.
pub const LANGUAGE: LanguageFn = unsafe { LanguageFn::from_raw(tree_sitter_hek) };

/// The node types the grammar can produce, as tree-sitter's generated JSON.
pub const NODE_TYPES: &str = include_str!("../../src/node-types.json");

/// Syntax highlighting. General to specific: an editor takes the last matching pattern,
/// so the catch-alls come first and a naming convention has to win over them.
pub const HIGHLIGHTS_QUERY: &str = include_str!("../../queries/highlights.scm");

/// Local scopes, definitions and references. Every definition capture names its scope
/// class (`@local.definition.parameter`, never a bare `@local.definition`), because that
/// class is the highlight a resolved reference is given.
pub const LOCALS_QUERY: &str = include_str!("../../queries/locals.scm");

/// Indentation.
pub const INDENTS_QUERY: &str = include_str!("../../queries/indents.scm");

/// Text objects: function, class, parameter and comment ranges.
pub const TEXTOBJECTS_QUERY: &str = include_str!("../../queries/textobjects.scm");

/// Code navigation tags.
pub const TAGS_QUERY: &str = include_str!("../../queries/tags.scm");

/// Bracket pair colouring.
pub const RAINBOWS_QUERY: &str = include_str!("../../queries/rainbows.scm");

#[cfg(test)]
mod tests {
    #[test]
    fn grammar_loads() {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&super::LANGUAGE.into())
            .expect("the committed parser is ABI 14 and this tree-sitter accepts 13 through 15");
    }
}
