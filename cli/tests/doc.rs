//! The document algebra, on its own and knowing no hek.
//!
//! Every layout decision `hek fmt` makes is one of these rules applied to a bigger
//! document, so a failure here is worth more than a failure in the printer: it says the
//! thing the printer is built out of is wrong.

use hek::fmt::doc::{Doc, render};

/// A list, in the shape every delimited construct in the language is printed as: the
/// delimiters flush, the contents indented, a trailing comma that exists only when broken.
fn list<'a>(open: &'a str, items: &[&'a str], close: &'a str) -> Doc<'a> {
    Doc::group(Doc::concat([
        Doc::text(open),
        Doc::indent(Doc::concat([
            Doc::Softline,
            Doc::join(
                Doc::concat([Doc::text(","), Doc::Line]),
                items.iter().map(|item| Doc::text(item)),
            ),
            Doc::if_break(Doc::text(","), Doc::nil()),
        ])),
        Doc::Softline,
        Doc::text(close),
    ]))
}

#[test]
fn a_group_that_fits_is_one_line() {
    let doc = list("{", &["a", "b"], "}");
    assert_eq!(render(&doc, 90), "{a, b}");
}

#[test]
fn a_group_that_does_not_fit_breaks_every_line_in_it() {
    let doc = list("{", &["alpha", "bravo", "charlie"], "}");
    assert_eq!(
        render(&doc, 10),
        "{\n  alpha,\n  bravo,\n  charlie,\n}",
        "one item per line, not as many as would fit"
    );
}

/// The whole point of `IfBreak`, and the reason `fits` may not measure source bytes: the
/// comma is absent from the flat rendering, so the width it is measured at is the width it
/// will occupy.
#[test]
fn a_trailing_comma_exists_only_when_the_list_broke() {
    let items = ["alpha", "bravo"];
    assert_eq!(render(&list("(", &items, ")"), 90), "(alpha, bravo)");
    assert_eq!(
        render(&list("(", &items, ")"), 8),
        "(\n  alpha,\n  bravo,\n)"
    );
}

#[test]
fn a_line_is_a_space_when_flat_and_a_softline_is_nothing() {
    let flat = Doc::group(Doc::concat([
        Doc::text("a"),
        Doc::Line,
        Doc::text("b"),
        Doc::Softline,
        Doc::text("c"),
    ]));
    assert_eq!(render(&flat, 90), "a bc");
}

/// A hard break reaches every group above it, so a construct that must span lines is never
/// measured and never claims to fit.
#[test]
fn a_hardline_breaks_the_group_around_it_however_short_it_is() {
    let doc = Doc::group(Doc::concat([Doc::text("a"), Doc::Hardline, Doc::text("b")]));
    assert_eq!(render(&doc, 90), "a\nb");
}

#[test]
fn a_hardline_breaks_through_a_nested_group() {
    let inner = Doc::group(Doc::concat([Doc::text("x"), Doc::Hardline, Doc::text("y")]));
    let outer = Doc::group(Doc::concat([
        Doc::text("["),
        Doc::Line,
        inner,
        Doc::text("]"),
    ]));
    assert_eq!(
        render(&outer, 90),
        "[\nx\ny]",
        "the outer group broke even though it is four characters wide"
    );
}

/// The case that makes the algebra worth having: an outer construct too wide for one line
/// whose parts are not. A rule that broke everything at the same level would put `b` and
/// `c` on separate lines too.
#[test]
fn an_inner_group_stays_flat_while_the_one_around_it_breaks() {
    let inner = list("{", &["b", "c"], "}");
    let outer = Doc::group(Doc::concat([
        Doc::text("outer("),
        Doc::indent(Doc::concat([
            Doc::Softline,
            Doc::text("aaaaaaaaaa"),
            Doc::text(","),
            Doc::Line,
            inner,
        ])),
        Doc::Softline,
        Doc::text(")"),
    ]));
    assert_eq!(render(&outer, 20), "outer(\n  aaaaaaaaaa,\n  {b, c}\n)");
}

#[test]
fn indentation_applies_to_the_lines_inside_it_and_not_the_delimiters() {
    let doc = list("{", &["alpha", "bravo"], "}");
    let nested = Doc::indent(Doc::concat([Doc::Hardline, doc]));
    assert_eq!(render(&nested, 10), "\n  {\n    alpha,\n    bravo,\n  }");
}

/// What keeps a comment written after code on the line it was written on.
#[test]
fn a_line_suffix_waits_for_the_end_of_the_line() {
    let doc = Doc::concat([
        Doc::text("let x = 1"),
        Doc::LineSuffix(" // why"),
        Doc::Hardline,
        Doc::text("let y = 2"),
    ]);
    assert_eq!(render(&doc, 90), "let x = 1 // why\nlet y = 2");
}

#[test]
fn a_line_suffix_at_the_very_end_is_still_emitted() {
    let doc = Doc::concat([Doc::text("a"), Doc::LineSuffix(" // last")]);
    assert_eq!(render(&doc, 90), "a // last");
}

/// No line the formatter writes ends in a space, including one whose content turned out to
/// be nothing at all.
#[test]
fn a_broken_line_never_ends_in_whitespace() {
    let doc = Doc::group(Doc::concat([
        Doc::text("a"),
        Doc::Line,
        Doc::text("bbbbbbbbbb"),
        Doc::Hardline,
        Doc::indent(Doc::Hardline),
        Doc::text("c"),
    ]));
    let out = render(&doc, 5);
    assert!(
        out.lines().all(|line| line == line.trim_end()),
        "a line ends in whitespace: {out:?}"
    );
}

/// Width is what a reader sees, so a character counts once however many bytes it takes.
#[test]
fn width_is_counted_in_characters_rather_than_bytes() {
    let doc = list("{", &["\"é\"", "\"é\""], "}");
    assert_eq!(render(&doc, 12), "{\"é\", \"é\"}");
}
