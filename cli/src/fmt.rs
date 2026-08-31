//! `hek fmt`: canonical hek.
//!
//! `docs/fmt.md` is the contract and `cli/tests/fmt.rs` is that document as executable
//! tests. Three layers: [`doc`] decides layout without knowing any hek, the printer lowers
//! a tree-sitter tree into a document, and [`crate::grammar`] supplies the tree.

pub mod doc;
