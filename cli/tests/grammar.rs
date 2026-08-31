//! The grammar and the acceptance fixtures agree.
//!
//! `tree-sitter-hek/README.md` claims the grammar parses every file in `hek/` and nothing
//! enforced it: the check was a command someone remembered to run. `hek fmt` reads through
//! the grammar, so a file it cannot parse is a file it cannot format, and this is where
//! that stops being a claim.

use std::fs;
use std::path::PathBuf;

/// Every `.hk` file heklang ships parses with no `ERROR` and no `MISSING` node.
///
/// The count is asserted so that a rename of `hek/` fails loudly rather than passing with
/// nothing read, which is the way a sweep like this usually rots.
#[test]
fn every_fixture_parses() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../hek");
    let mut parsed = Vec::new();
    for entry in fs::read_dir(&root).expect("`hek/` holds the acceptance fixtures") {
        let path = entry.expect("reading an entry of `hek/`").path();
        if path.extension().is_none_or(|ext| ext != "hk") {
            continue;
        }
        let source = fs::read_to_string(&path).expect("reading a fixture");
        let name = path.file_name().expect("a file has a name").to_owned();
        assert!(
            hek::grammar::parse(&source).is_some(),
            "{} does not parse; `tree-sitter parse` names the node",
            name.display()
        );
        parsed.push(name);
    }
    assert!(
        parsed.len() >= 8,
        "only {} fixtures were read, so the sweep found nothing to check: {parsed:?}",
        parsed.len()
    );
}
