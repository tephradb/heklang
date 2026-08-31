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
