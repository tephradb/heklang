//! `docs/fmt.md` as executable tests.
//!
//! The safety property comes first, because everything else is a preference and it is not:
//! formatting a file changes its whitespace and nothing else. It is checked by lexing both
//! sides with the language's own lexer rather than by comparing programs, because a
//! `Program` carries a `Span` in dozens of places and every one of them legitimately moves
//! when text moves.

use std::fs;
use std::path::PathBuf;

use heklang::lex::{Sym, Token, lex};

fn fixtures() -> Vec<(String, String)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../hek");
    let mut found = Vec::new();
    for entry in fs::read_dir(&root).expect("`hek/` holds the acceptance fixtures") {
        let path = entry.expect("reading an entry").path();
        if path.extension().is_none_or(|ext| ext != "hk") {
            continue;
        }
        let name = path.file_name().expect("a file has a name");
        found.push((
            name.to_string_lossy().into_owned(),
            fs::read_to_string(&path).expect("reading a fixture"),
        ));
    }
    found.sort();
    assert!(found.len() >= 8, "the sweep found nothing to check");
    found
}

/// The token stream, with a comma before a closing delimiter dropped.
///
/// That comma is the one token the formatter is allowed to add and remove: it writes one
/// when a list breaks and none when it fits, and `docs/functions.md` records that both
/// parse to the same thing. Every other difference is a bug.
fn shape(source: &str) -> Vec<Token> {
    let tokens: Vec<Token> = lex(source)
        .expect("a formatted file still lexes")
        .into_iter()
        .map(|spanned| spanned.token)
        .collect();
    let closes = |token: Option<&Token>| {
        matches!(
            token,
            Some(Token::Sym(Sym::RParen | Sym::RBrace | Sym::RBracket))
        )
    };
    tokens
        .iter()
        .enumerate()
        .filter(|(at, token)| {
            !(matches!(token, Token::Sym(Sym::Comma)) && closes(tokens.get(at + 1)))
        })
        .map(|(_, token)| token.clone())
        .collect()
}

/// **Formatting changes whitespace and nothing else.**
///
/// This is the claim the rest of the formatter rests on, and the one a user has to be able
/// to take on faith to run `hek fmt` over a tree they care about.
#[test]
fn formatting_changes_only_whitespace() {
    for (name, source) in fixtures() {
        let formatted = hek::fmt::format(&source).expect("a fixture parses");
        assert_eq!(
            shape(&formatted),
            shape(&source),
            "{name} came back with a different token stream"
        );
    }
}

/// Every comment survives, counted rather than trusted. Comments are `extras` in the
/// grammar, so they are attached to no rule and are the thing a printer written against
/// the node-type schema silently drops.
#[test]
fn every_comment_survives() {
    for (name, source) in fixtures() {
        let formatted = hek::fmt::format(&source).expect("a fixture parses");
        let before: Vec<&str> = comments(&source);
        let after: Vec<&str> = comments(&formatted);
        assert_eq!(before, after, "{name} lost or gained a comment");
    }
}

fn comments(source: &str) -> Vec<&str> {
    source
        .lines()
        .filter_map(|line| line.trim().strip_prefix("//"))
        .map(str::trim_end)
        .collect()
}

/// A formatter that is not idempotent is not a formatter: `--check` would never go quiet
/// and two people running it would fight.
#[test]
fn formatting_twice_is_formatting_once() {
    for (name, source) in fixtures() {
        let once = hek::fmt::format(&source).expect("a fixture parses");
        let twice = hek::fmt::format(&once).expect("formatted output parses");
        assert_eq!(once, twice, "{name} is not a fixed point");
    }
}

/// Whatever else changed, the file still parses, and it parses as the same declarations.
#[test]
fn the_formatted_program_still_checks() {
    let files = fixtures();
    let borrowed: Vec<(String, String)> = files
        .iter()
        .map(|(name, source)| {
            (
                name.clone(),
                hek::fmt::format(source).expect("a fixture parses"),
            )
        })
        .collect();
    let pairs: Vec<(&str, &str)> = borrowed
        .iter()
        .map(|(name, source)| (name.as_str(), source.as_str()))
        .collect();
    if let Err(errors) = heklang::check_files(pairs) {
        panic!(
            "the formatted fixtures no longer check:\n{}",
            errors
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}

/// Two spaces, no tabs, no trailing whitespace, one trailing newline. The invariants that
/// hold across all 83 `.hk` files in existence with no counterexample.
#[test]
fn the_output_holds_the_invariants_the_corpus_already_held() {
    for (name, source) in fixtures() {
        let formatted = hek::fmt::format(&source).expect("a fixture parses");
        assert!(!formatted.contains('\t'), "{name} has a tab");
        assert!(
            formatted.ends_with('\n') && !formatted.ends_with("\n\n"),
            "{name} does not end in exactly one newline"
        );
        assert!(
            !formatted.contains("\n\n\n"),
            "{name} has two blank lines in a row"
        );
        for line in formatted.lines() {
            assert_eq!(line, line.trim_end(), "{name} has a line ending in space");
            let indent = line.len() - line.trim_start().len();
            assert_eq!(indent % 2, 0, "{name} has an odd indent: {line:?}");
        }
    }
}

fn fmt(source: &str) -> String {
    hek::fmt::format(source).expect("the fixture parses")
}

/// The acceptance case, and the reason to build this at all. Five files in a real
/// application carry newline-eating edit damage like this; it accounts for every line over
/// 120 columns in the corpus and every non-string run of two spaces.
#[test]
fn collapsed_lines_are_put_back() {
    let damaged = "test \"a\" {\n  given @order.placed {\n    order_id: 1,      status: 2,  }\n}\n";
    assert_eq!(
        fmt(damaged),
        "test \"a\" {\n  given @order.placed { order_id: 1, status: 2 }\n}\n"
    );
}

/// A comment inside a list opens the list, because a line comment would otherwise swallow
/// whatever followed it on the line.
#[test]
fn a_comment_in_a_list_breaks_it_and_keeps_its_place() {
    let source = "command C() {\n  emit @a.b {\n    x,\n    // why y\n    y,\n  }\n}\n";
    assert_eq!(fmt(source), source, "short enough to fit, and still broken");
}

/// Nothing in the corpus writes one. The grammar allows it, so it gets an answer.
#[test]
fn a_comment_after_code_stays_on_that_line() {
    let source = "command C() {\n  let x = 1 // why\n  let y = 2\n}\n";
    assert_eq!(fmt(source), source);
}

/// A comment with nothing after it is still emitted. This is the placement a printer loses
/// first, because there is no next sibling to attach it to.
#[test]
fn a_comment_alone_in_a_body_survives() {
    let source = "command C() {\n  // nothing to decide yet\n}\n";
    assert_eq!(fmt(source), source);
}

/// A `"""` body holds another language at its own indentation. Re-indenting one would
/// corrupt it, and the corpus has fifteen holding GraphQL at column 0.
#[test]
fn a_raw_string_is_untouched_however_it_is_indented() {
    let source = "const Q: String = \"\"\"\nquery {\n  thing { id }\n}\n\"\"\"\n";
    assert_eq!(fmt(source), source);
}

/// Blank lines are the author's: kept where written, collapsed where doubled, and never
/// invented. `const` runs are grouped by meaning and a `test` body groups its phases.
#[test]
fn blank_lines_are_kept_collapsed_and_never_added() {
    let source = "const A: Int = 1\nconst B: Int = 2\n\n\n\nconst C: Int = 3\n";
    assert_eq!(
        fmt(source),
        "const A: Int = 1\nconst B: Int = 2\n\nconst C: Int = 3\n",
        "the pair stays packed, the run of blanks becomes one"
    );
}

/// A field list is a type declaration and always breaks; a variant list is a value
/// enumeration and fits. No `record` in the corpus is on one line and every `enum` is.
#[test]
fn a_record_always_breaks_and_an_enum_does_not() {
    assert_eq!(
        fmt("record Item { sku: String }\n"),
        "record Item {\n  sku: String,\n}\n"
    );
    assert_eq!(
        fmt("enum Tier {\n  @default Free,\n  Paid,\n}\n"),
        "enum Tier { @default Free, Paid }\n"
    );
}

/// `guard_decl` is the one comma loop in `src/parse.rs` that parses another slice
/// unconditionally after eating a comma, so a trailing comma here does not parse. The list
/// is flat for that reason rather than for a layout one.
#[test]
fn a_raw_guard_list_never_takes_a_trailing_comma() {
    let source = "command C(id: Uuid) {\n  guard @order.placed(id), @order.cancelled(id)\n}\n";
    assert_eq!(fmt(source), source);
}

/// A long `on` header breaks rather than running to 328 columns, and its path list takes no
/// trailing comma: there is no closing delimiter, so a comma would be followed by `as`.
#[test]
fn a_long_on_header_breaks_its_paths_without_a_trailing_comma() {
    let source = "effect E {
  on @a.one, @a.two, @a.three, @a.four, @a.five, @a.six, @a.seven, @a.eight, @a.nine as e { id } {
    log(\"{id}\")
  }
}
";
    let formatted = fmt(source);
    assert!(
        formatted.contains("on @a.one,\n    @a.two,"),
        "the paths should break one per line at +2:\n{formatted}"
    );
    assert!(
        formatted.contains("@a.nine as e { id } {"),
        "the last path carries the rest of the header, and takes no comma:\n{formatted}"
    );
    assert!(
        formatted.lines().all(|line| line.chars().count() <= 90),
        "nothing should still be over the width:\n{formatted}"
    );
}

/// Nesting keeps its shape all the way down, and the trailing comma appears at every level
/// that broke and at none that did not.
#[test]
fn nesting_breaks_outside_in() {
    let source = "effect E {\n  on @a.b as e { id } {\n    log(\"{id}\")\n  }\n}\n";
    assert_eq!(fmt(source), source);
}

/// The acceptance fixtures are the formatter's own output, so a change to the printer shows
/// up as a diff in the repository rather than only in whatever someone happens to run it on.
#[test]
fn the_fixtures_are_their_own_formatters_output() {
    for (name, source) in fixtures() {
        assert_eq!(
            fmt(&source),
            source,
            "{name} is not formatted; `cargo run -p hek -- fmt hek/` fixes it"
        );
    }
}
