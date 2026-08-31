//! `hek fmt`: canonical hek.
//!
//! `docs/fmt.md` is the contract and `cli/tests/fmt.rs` is that document as executable
//! tests. Three layers: [`doc`] decides layout without knowing any hek, the printer lowers
//! a tree-sitter tree into a document, and [`crate::grammar`] supplies the tree.

pub mod doc;
pub mod print;

/// One module, canonically. `None` when the source does not parse, which is the only
/// thing that stops `fmt`: the grammar accepts everything the language does and more, so a
/// program `hek check` rejects still formats. Checking is a different question and `check`
/// is where it is asked.
pub fn format(source: &str) -> Option<String> {
    let tree = crate::grammar::parse(source)?;
    let printer = print::Printer::new(source);
    let rendered = doc::render(&printer.file(tree.root_node()), print::WIDTH);
    if rendered.is_empty() {
        // A module holding nothing but whitespace is a file holding nothing, rather than
        // a file holding one empty line.
        return Some(String::new());
    }
    Some(format!("{rendered}\n"))
}
