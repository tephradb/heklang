//! `docs/strings.md` as executable tests: interpolation, its nesting, the text form
//! table, and the raw multi-line form.

use heklang::{Interpreter, Outcome, Value, parse};

const PRELUDE: &str = "event @note.written {
  note_id: Uuid,
  body: String,
}
";

/// Runs a command whose only job is to emit `body`, and returns the string it built.
fn built(params: &str, expr: &str, args: Vec<(&str, Value)>) -> String {
    let source = format!(
        "{PRELUDE}
command Write(note_id: Uuid, {params}) {{
  emit @note.written {{ note_id, body: {expr} }}
}}
"
    );
    let program = parse(&source).unwrap_or_else(|err| panic!("expected this to parse: {err}"));
    let mut interpreter = Interpreter::new(&program);
    let mut all = vec![(
        "note_id",
        Value::uuid("0190d1a1-0000-7000-8000-000000000001"),
    )];
    all.extend(args);
    let execution = interpreter
        .run("Write", all)
        .unwrap_or_else(|err| panic!("expected this to run: {err}"));
    match execution.outcome {
        Outcome::Ok(events) => match events[0].field("body") {
            Some(Value::Str(text)) => text.clone(),
            other => panic!("expected a string body, got {other:?}"),
        },
        other => panic!("expected an append, got {other:?}"),
    }
}

fn err(params: &str, expr: &str) -> String {
    let source = format!(
        "{PRELUDE}
command Write(note_id: Uuid, {params}) {{
  emit @note.written {{ note_id, body: {expr} }}
}}
"
    );
    parse(&source)
        .expect_err("expected this to be rejected")
        .text()
}

// ---------------------------------------------------------------------------------
// Interpolation.

#[test]
fn a_hole_takes_the_value_of_its_expression() {
    assert_eq!(
        built(
            "who: String",
            r#""hello {who}, and welcome""#,
            vec![("who", Value::str("ada"))],
        ),
        "hello ada, and welcome"
    );
}

#[test]
fn a_string_may_be_nothing_but_holes() {
    assert_eq!(
        built(
            "a: Int, b: Int",
            r#""{a}/{b}""#,
            vec![("a", Value::Int(7)), ("b", Value::Int(9))],
        ),
        "7/9"
    );
}

/// The reason the lexer nests rather than restricting a hole to a path: a real port
/// needed arithmetic and a method chain inside one.
#[test]
fn a_hole_takes_an_arbitrary_expression() {
    assert_eq!(
        built(
            "months: Int",
            r#""{months / 12}-Year Warranty""#,
            vec![("months", Value::Int(24))],
        ),
        "2-Year Warranty"
    );
    assert_eq!(
        built(
            "name: String?",
            r#""hello {name.unwrap_or("nobody").upper()}""#,
            vec![("name", Value::none(heklang::Type::String))],
        ),
        "hello NOBODY"
    );
}

/// The wart that made a real port carry a `message_of` helper whose only job was to
/// move a `""` out of the braces.
#[test]
fn a_string_literal_nests_inside_a_hole() {
    assert_eq!(
        built(
            "err: String?",
            r#""failed: {err.unwrap_or("")}""#,
            vec![("err", Value::none(heklang::Type::String))],
        ),
        "failed: "
    );
}

#[test]
fn holes_nest_to_any_depth() {
    assert_eq!(
        built("a: Int", r#""[{"<{a}>"}]""#, vec![("a", Value::Int(3))],),
        "[<3>]"
    );
}

/// A brace inside a hole is counted, so a block-shaped expression is not mistaken for
/// the end of the interpolation.
#[test]
fn a_braced_expression_inside_a_hole_is_counted_not_terminating() {
    assert_eq!(
        built(
            "n: Int",
            r#""n is {if n > 1 { "many" } else { "one" }}.""#,
            vec![("n", Value::Int(4))],
        ),
        "n is many."
    );
}

#[test]
fn a_literal_brace_is_escaped() {
    assert_eq!(
        built("n: Int", r#""\{{n}\}""#, vec![("n", Value::Int(1))]),
        "{1}"
    );
}

#[test]
fn an_unterminated_hole_is_rejected() {
    let message = err("n: Int", r#""a {n""#);
    assert_eq!(message, "unterminated string");
}

// ---------------------------------------------------------------------------------
// The text form is rule 8's JSON table.

#[test]
fn the_text_form_is_rule_eights_table() {
    // A `Money(n)` keeps its scale, because the scale is part of the value and a
    // message that drops it lies about precision.
    assert_eq!(
        built(
            "price: Money(3)",
            r#""{price}""#,
            vec![("price", Value::money(10_500, 3))],
        ),
        "10.500"
    );
    // The enum row of the table needs a module-scope enum to be reachable from a
    // command parameter, so it is asserted where that lands.
    // A `Uuid` is canonical, and a `String` is unquoted, which `Display` is not.
    assert_eq!(
        built(
            "id: Uuid, name: String",
            r#""{id} {name}""#,
            vec![
                ("id", Value::uuid("0190d1a1-0000-7000-8000-00000000000f")),
                ("name", Value::str("ada")),
            ],
        ),
        "0190d1a1-0000-7000-8000-00000000000f ada"
    );
    // An absent optional is `null`, matching what it serialises to.
    assert_eq!(
        built(
            "name: String?",
            r#""[{name}]""#,
            vec![("name", Value::none(heklang::Type::String))],
        ),
        "[null]"
    );
    assert_eq!(
        built(
            "flag: Bool",
            r#""{flag}""#,
            vec![("flag", Value::Bool(true))]
        ),
        "true"
    );
}

// ---------------------------------------------------------------------------------
// Raw multi-line strings.

#[test]
fn a_multi_line_string_is_verbatim() {
    let source = format!(
        "{PRELUDE}
command Write(note_id: Uuid) {{
  emit @note.written {{ note_id, body: \"\"\"
line one
  line two
\"\"\" }}
}}
"
    );
    let program = parse(&source).unwrap_or_else(|err| panic!("expected this to parse: {err}"));
    let mut interpreter = Interpreter::new(&program);
    let execution = interpreter
        .run(
            "Write",
            vec![(
                "note_id",
                Value::uuid("0190d1a1-0000-7000-8000-000000000001"),
            )],
        )
        .expect("ran");
    let Outcome::Ok(events) = execution.outcome else {
        panic!("expected an append");
    };
    assert_eq!(
        events[0].field("body"),
        Some(&Value::str("\nline one\n  line two\n")),
        "verbatim: the leading newline and the indentation are both content"
    );
}

/// The whole reason the raw form exists: a GraphQL document is brace-dense, so those
/// braces must not be holes, and a backslash must not be an escape.
#[test]
fn a_multi_line_string_does_not_interpolate_or_escape() {
    let source = format!(
        "{PRELUDE}
command Write(note_id: Uuid) {{
  emit @note.written {{ note_id, body: \"\"\"query {{ shop {{ id name }} }} \\n\"\"\" }}
}}
"
    );
    let program = parse(&source).unwrap_or_else(|err| panic!("expected this to parse: {err}"));
    let mut interpreter = Interpreter::new(&program);
    let execution = interpreter
        .run(
            "Write",
            vec![(
                "note_id",
                Value::uuid("0190d1a1-0000-7000-8000-000000000001"),
            )],
        )
        .expect("ran");
    let Outcome::Ok(events) = execution.outcome else {
        panic!("expected an append");
    };
    assert_eq!(
        events[0].field("body"),
        Some(&Value::str(r"query { shop { id name } } \n")),
    );
}

#[test]
fn an_unterminated_multi_line_string_is_rejected() {
    let source = format!(
        "{PRELUDE}
command Write(note_id: Uuid) {{
  emit @note.written {{ note_id, body: \"\"\"never closed }}
}}
"
    );
    assert_eq!(
        parse(&source).expect_err("expected a rejection").text(),
        "unterminated multi-line string"
    );
}

/// An interpolated string is not a literal, so it cannot be an entity default. The
/// message says which of the two it got.
#[test]
fn an_interpolated_string_is_not_a_literal() {
    let source = "event @note.written { note_id: Uuid, body: String }

projector Notes {
  entity Note {
    note_id: Uuid @key,
    body: String = \"a {note_id}\",
  }
}
";
    assert_eq!(
        parse(source).expect_err("expected a rejection").text(),
        "a String default cannot be an interpolated string"
    );
}
