//! `docs/diagnostics.md` as executable tests: where a diagnostic says it is, and how
//! wide. One test per numbered rule.

use heklang::{Pos, Span, SyntaxError, parse};

const EVENTS: &str = "event @thing.happened { id: Int, name: String }
";

/// The one thing every test here needs: the error a source raises, with its span. The
/// events go in front, so every line number below is one more than it reads.
fn error(source: &str) -> SyntaxError {
    parse(&format!("{EVENTS}{source}")).expect_err("this source is meant to fail")
}

/// A span written the way a reader says it: line, column, through to line, column.
fn at(start_line: u32, start_col: u32, end_line: u32, end_col: u32) -> Span {
    Span::new(Pos::new(start_line, start_col), Pos::new(end_line, end_col))
}

/// Rule 1: 1-based, both of them, so the first character of a file is 1:1 and not 0:0.
#[test]
fn the_first_character_of_a_file_is_one_one() {
    let err = parse("nope Foo { }\n").expect_err("`nope` is not a declaration");
    assert_eq!(err.span.start, Pos::new(1, 1));
}

/// Rule 2: `end` is one past the last character, so a token's width on one line is the
/// difference between the columns. `nope` is four characters at column 1.
#[test]
fn a_span_ends_one_past_its_last_character() {
    let err = parse("nope Foo { }\n").expect_err("`nope` is not a declaration");
    assert_eq!(err.span, at(1, 1, 1, 5));
    assert_eq!(err.span.end.col - err.span.start.col, 4);
}

/// Rule 2: the rendered form is the start alone, unchanged by the extent existing. This
/// is what keeps every message in the rest of the suite reading the way it did.
#[test]
fn the_rendered_form_is_the_start_alone() {
    let err = error("command A(id: Int) {\n  emit @thing.happened { id, name: missing }\n}\n");
    assert_ne!(err.span.end, err.span.start, "it has an extent");
    assert_eq!(
        err.to_string(),
        "3:36: `missing` is not in scope",
        "and none of it reaches the text"
    );
}

/// Rule 4: a token the parser did not expect is covered, not pointed at. `command` is
/// seven characters where a `{` was wanted.
#[test]
fn an_unexpected_token_is_covered_whole() {
    let err = error("command A(id: Int)\ncommand B(id: Int) {\n}\n");
    assert_eq!(err.message, "expected `{`, found `command`");
    assert_eq!(err.span, at(3, 1, 3, 8));
}

/// Rule 4: a name that is not in scope covers the name.
#[test]
fn a_name_out_of_scope_covers_the_name() {
    let err = error("command A(id: Int) {\n  emit @thing.happened { id, name: missing }\n}\n");
    assert_eq!(err.message, "`missing` is not in scope");
    assert_eq!(err.span, at(3, 36, 3, 43), "`missing` is seven characters");
}

/// Rule 3: the end comes off the token itself, so a two-character symbol is two wide.
/// Nothing here is derived from the token's kind or guessed from its text.
#[test]
fn a_symbol_is_as_wide_as_it_is_written() {
    let err = error("command A(id: Int) {\n  let x = id => 1\n}\n");
    assert_eq!(err.message, "expected a statement, found `=>`");
    assert_eq!(err.span, at(3, 14, 3, 16));
}

/// Rule 6: a string may hold a newline, so a span may end on a later line than it began.
/// Anything drawing one has to expect that, which is why it is asserted rather than
/// assumed.
#[test]
fn a_span_may_end_on_a_later_line() {
    let err = error(
        "command A(id: Int) {\n  emit @thing.happened { id: \"\"\"one\ntwo\"\"\", name: \"x\" }\n}\n",
    );
    assert_eq!(err.message, "expected Int, found String");
    assert_eq!(err.span, at(3, 30, 4, 7));
    assert!(err.span.end.line > err.span.start.line);
}

/// Rule 5: running out of input has no token to point at, so it reports at the sentinel.
/// `0:0` is the position of nothing, and it is a position rather than an absence, so
/// every diagnostic has one and nothing has to test for its absence.
#[test]
fn the_end_of_the_file_has_no_extent() {
    let err = error("command A(id: Int) {\n");
    assert_eq!(err.message, "unclosed `{`");
    assert_eq!(err.span, Span::default());
    assert_eq!(err.span.start, err.span.end, "and it holds nothing");
}

/// Rule 2: `Span::point` is the constructor for that case, and an empty span is what it
/// makes: the two ends together.
#[test]
fn a_point_is_a_span_with_nothing_in_it() {
    let span = Span::point(Pos::new(4, 9));
    assert_eq!(span.start, span.end);
    assert_eq!(
        span.to_string(),
        "4:9",
        "and it renders as the one position"
    );
}

/// Rule 4: a value in a declared position is about the whole value, so the span covers
/// all of it. Reporting at its first token pointed at `text` and said "found Int?",
/// which is a claim about what the call returns rather than about the name.
#[test]
fn a_type_mismatch_covers_the_whole_expression() {
    let err = error(
        "command A(id: Int, text: String) {\n  emit @thing.happened { id, name: text.to_int() }\n}\n",
    );
    assert_eq!(err.message, "expected String, found Int?");
    assert_eq!(err.span, at(3, 36, 3, 49), "`text.to_int()` is thirteen");
}

/// Rule 4: and so is a chain of them. The span runs from the first operand to the last,
/// which is the extent the operator table read to reject it.
#[test]
fn an_arithmetic_mismatch_covers_both_operands() {
    let err = error(
        "command A(id: Int, text: String) {\n  emit @thing.happened { id: id + 1 + text, name: \"x\" }\n}\n",
    );
    assert_eq!(err.message, "cannot apply `+` to Int and String");
    assert_eq!(err.span, at(3, 30, 3, 43), "`id + 1 + text`, not the `+`");
}

/// Rule 4: a comparison covers the pair. The operator alone is where the mistake is
/// spelled, and the pair is what the mistake is about; an editor can only underline one
/// of them, and the pair is the one that says which two things did not meet.
#[test]
fn a_comparison_covers_the_pair_rather_than_the_operator() {
    let err = error("command A(a: Money(2), b: Money(3)) {\n  if a > b {\n    return\n  }\n}\n");
    assert!(
        err.message
            .starts_with("cannot apply `>` to Money(2) and Money(3)")
    );
    assert_eq!(err.span, at(3, 6, 3, 11), "`a > b`");
}

/// Rule 4: a field the event does not have covers the field's name. It used to report
/// at the cursor, which by then had moved past the name onto the `:`.
#[test]
fn an_unknown_field_covers_the_field_name() {
    let err = error("command A(id: Int) {\n  emit @thing.happened { id, nope: \"x\" }\n}\n");
    assert_eq!(err.message, "@thing.happened has no field `nope`");
    assert_eq!(err.span, at(3, 30, 3, 34));
}

/// Rule 3: a node carries the extent it was parsed from, not the token the builder's
/// cursor happened to hold. A runtime error reads that extent, so it now covers the same
/// text a static error over the same expression would.
#[test]
fn a_runtime_error_covers_its_expression() {
    let program = parse(
        "event @thing.happened { id: Int, name: String }\ncommand A(total: Money(2)) {\n  let cut = total * 0.9\n  return\n}\n",
    )
    .expect("this parses; the multiplication only fails when it runs");

    let mut interpreter = heklang::Interpreter::new(&program);
    let err = interpreter
        .run("A", [("total", heklang::Value::money(2_599, 2))])
        .expect_err("the multiplication is not exact");

    assert_eq!(
        err.span,
        at(3, 13, 3, 24),
        "`total * 0.9`, not the `*` the builder's cursor was on"
    );
}

/// Rule 2, over every expression in the demo programs: a span runs forwards. A production
/// given the wrong start shows up here and nowhere else, because a backwards span still
/// renders as a plausible position.
#[test]
fn every_span_runs_forwards() {
    let mut sources = Vec::new();
    for entry in std::fs::read_dir("hek").expect("the demo programs are in `hek/`") {
        let path = entry.expect("a readable entry").path();
        if path.extension().is_some_and(|ext| ext == "hk") {
            let name = path.to_string_lossy().into_owned();
            sources.push((name, std::fs::read_to_string(&path).expect("readable")));
        }
    }
    assert!(!sources.is_empty(), "the demo programs are the corpus");

    let program = heklang::parse_files(sources.iter().map(|(n, s)| (n.as_str(), s.as_str())))
        .expect("the demo programs check clean");

    let arenas = program
        .commands
        .iter()
        .map(|c| &c.exprs)
        .chain(
            program
                .projectors
                .iter()
                .flat_map(|p| p.handlers.iter().map(|h| &h.exprs)),
        )
        .chain(
            program
                .effects
                .iter()
                .flat_map(|e| e.arms.iter().map(|a| &a.exprs)),
        )
        .chain(program.functions.iter().map(|f| &f.exprs));

    let mut checked = 0;
    for exprs in arenas {
        let mut id = 0;
        while exprs.get(heklang::ExprId(id)).is_some() {
            let span = exprs.span(heklang::ExprId(id));
            assert!(
                span.start <= span.end,
                "expression {id} runs backwards: {span:?}"
            );
            checked += 1;
            id += 1;
        }
    }
    assert!(
        checked > 300,
        "only {checked} expressions; too few to mean much"
    );
}

/// Rule 4, for the lexer's own errors. They used to report at the cursor, which a scanner
/// leaves *past* the character it gave up on, so `unexpected character` pointed one to
/// the right of the character it named.
#[test]
fn an_unexpected_character_covers_that_character() {
    let err = error("command A(id: Int) {\n  let x = #\n}\n");
    assert_eq!(err.message, "unexpected character `#`");
    assert_eq!(err.span, at(3, 11, 3, 12), "the `#`, one wide");
}

/// And the one this matters most for: an unterminated string ran to the end of the file,
/// so the cursor was arbitrarily far from the quote that opened it. The extent now starts
/// where the string did.
#[test]
fn an_unterminated_string_starts_at_its_quote() {
    let err = error("command A(id: Int) {\n  let x = \"abc\n}\n");
    assert_eq!(err.message, "unterminated string");
    assert_eq!(err.span.start, Pos::new(3, 11), "the opening quote");
}

/// Rule 4: an annotation is one `@name` token, and a diagnostic about one covers it. All
/// four of these reported at the cursor, which by then had moved past the annotation onto
/// whatever followed, so `@subject(shop_id)` was reported at its `(`.
#[test]
fn an_unknown_annotation_covers_the_annotation() {
    for (source, line, col, width) in [
        ("event @a.b { id: Int, note: String @nope }\n", 1, 36, 5),
        ("record R { note: String @nope }\n", 1, 25, 5),
        ("enum E { @nope A, B }\n", 1, 10, 5),
        (
            "event @a.b { id: Int }\nprojector P {\n  entity Row {\n    id: Int @key,\n    text: String @nope,\n  }\n  on @a.b { id } { put Row { id, text: \"x\" } }\n}\n",
            5,
            18,
            5,
        ),
    ] {
        let err = parse(source).expect_err("`@nope` is not an annotation");
        assert_eq!(err.message, "unknown annotation `@nope`");
        assert_eq!(err.span, at(line, col, line, col + width), "{source}");
    }
}

/// The same for the name that is not one name. `@a.b` is four characters and the span is
/// four wide, which is the token rather than anything counted from the text.
#[test]
fn a_dotted_annotation_name_covers_the_whole_path() {
    let err = parse("event @a.b { id: Int, note: String @a.b }\n")
        .expect_err("an annotation name is one segment");
    assert_eq!(err.message, "an annotation name cannot contain `.`");
    assert_eq!(err.span, at(1, 36, 1, 40));
}
