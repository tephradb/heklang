//! `docs/parsing.md` as executable tests. Every one of these reads a value out of text
//! that came from outside the program, so every one of them returns an optional or a
//! total answer rather than trapping.

use heklang::{Interpreter, Outcome, Type, Value, parse};

const PRELUDE: &str = "event @read.done {
  at: Timestamp?,
  amount: Money(3)?,
  count: Int?,
  id: Uuid?,
  text: String,
  flag: Bool,
}
";

/// Runs a command whose only job is to emit one field, and returns the event.
fn read(field: &str, expr: &str, text: &str) -> Value {
    let source = format!(
        "{PRELUDE}
command Read(text: String) {{
  emit @read.done {{
    at: none,
    amount: none,
    count: none,
    id: none,
    text: \"\",
    flag: false,
    {field}: {expr},
  }}
}}
"
    );
    let program = parse(&source).unwrap_or_else(|err| panic!("expected this to parse: {err}"));
    let mut interpreter = Interpreter::new(&program);
    let execution = interpreter
        .run("Read", vec![("text", Value::str(text))])
        .unwrap_or_else(|err| panic!("expected this to run: {err}"));
    match execution.outcome {
        Outcome::Ok(events) => events[0].field(field).cloned().expect("the field"),
        other => panic!("expected an append, got {other:?}"),
    }
}

fn err(field: &str, expr: &str) -> String {
    let source = format!(
        "{PRELUDE}
command Read(text: String) {{
  emit @read.done {{ at: none, amount: none, count: none, id: none, text: \"\", flag: false, {field}: {expr} }}
}}
"
    );
    parse(&source)
        .expect_err("expected this to be rejected")
        .message
}

// ---------------------------------------------------------------------------------
// Timestamp.parse

#[test]
fn a_timestamp_parses_from_rfc_3339() {
    // 2020-01-01T00:00:00Z is the epoch the rest of the harness uses.
    assert_eq!(
        read("at", "Timestamp.parse(text)", "2020-01-01T00:00:00Z"),
        Value::some(Value::Timestamp(1_577_836_800_000_000))
    );
    // An offset moves the instant, which is the whole reason it is required.
    assert_eq!(
        read("at", "Timestamp.parse(text)", "2020-01-01T01:00:00+01:00"),
        Value::some(Value::Timestamp(1_577_836_800_000_000))
    );
    // Fractional seconds, truncated to microseconds.
    assert_eq!(
        read(
            "at",
            "Timestamp.parse(text)",
            "2020-01-01T00:00:00.123456789Z"
        ),
        Value::some(Value::Timestamp(1_577_836_800_123_456))
    );
    // A leap day, which is where a hand-rolled calendar goes wrong.
    assert_eq!(
        read("at", "Timestamp.parse(text)", "2024-02-29T00:00:00Z"),
        Value::some(Value::Timestamp(1_709_164_800_000_000))
    );
}

#[test]
fn a_bad_timestamp_is_none_rather_than_an_error() {
    for text in [
        "",
        "not a date",
        "2020-13-01T00:00:00Z",
        "2023-02-29T00:00:00Z",
        "2020-01-01T25:00:00Z",
        // No offset at all is not RFC 3339, and guessing one is how a warranty
        // expires on the wrong day.
        "2020-01-01T00:00:00",
    ] {
        assert_eq!(
            read("at", "Timestamp.parse(text)", text),
            Value::none(Type::Timestamp),
            "for {text:?}"
        );
    }
}

/// A string in a `Timestamp` position is a `Timestamp`, read by the same function at
/// parse time. The author's text cannot be absent, so this one is not an optional.
#[test]
fn a_written_timestamp_is_read_at_parse_time() {
    assert_eq!(
        read("at", "\"2020-01-01T00:00:00Z\"", ""),
        Value::some(Value::Timestamp(1_577_836_800_000_000))
    );
    // The same reading, so the offset and the fraction behave as they do at run time.
    assert_eq!(
        read("at", "\"2020-01-01T01:00:00+01:00\"", ""),
        Value::some(Value::Timestamp(1_577_836_800_000_000))
    );
    assert_eq!(
        read("at", "\"2020-01-01T00:00:00.123456789Z\"", ""),
        Value::some(Value::Timestamp(1_577_836_800_123_456))
    );
}

/// The literal half fails at parse time where `Timestamp.parse` would give `none`, and
/// the message names the shape rather than only reporting that the text was wrong.
#[test]
fn a_written_timestamp_that_is_not_one_is_rejected() {
    for text in [
        "2020-01-01",
        "2020-01-01T00:00:00",
        "2023-02-29T00:00:00Z",
        "not a timestamp",
    ] {
        let message = err("at", &format!("\"{text}\""));
        assert!(
            message.contains(&format!("`{text}` is not a Timestamp")),
            "for {text:?}, got: {message}"
        );
        assert!(
            message.contains("RFC 3339") && message.contains("offset"),
            "expected the shape to be named, got: {message}"
        );
    }
}

// ---------------------------------------------------------------------------------
// Money.parse

#[test]
fn money_parses_against_the_target_scale() {
    assert_eq!(
        read("amount", "Money.parse(text)", "10.5"),
        Value::some(Value::money(10_500, 3)),
        "widening is exact, the same rule a written literal follows"
    );
    assert_eq!(
        read("amount", "Money.parse(text)", "0"),
        Value::some(Value::money(0, 3))
    );
    assert_eq!(
        read("amount", "Money.parse(text)", "-2.750"),
        Value::some(Value::money(-2_750, 3))
    );
    // More places than the target holds fails rather than rounding silently, which is
    // the rule `docs/money.md` already gives for a literal.
    assert_eq!(
        read("amount", "Money.parse(text)", "1.2345"),
        Value::none(Type::Money(3))
    );
    for text in ["", "abc", "1.", ".5", "1,5"] {
        assert_eq!(
            read("amount", "Money.parse(text)", text),
            Value::none(Type::Money(3)),
            "for {text:?}"
        );
    }
}

/// The scale is a property of where the amount lands, not of the text: `"10.5"` is a
/// different value at scale 2 and at scale 3.
#[test]
fn money_parse_needs_a_target_scale() {
    let message = err("text", "Money.parse(text).unwrap_or(\"\")");
    assert!(
        message.starts_with("`Money.parse` needs a target scale"),
        "got: {message}"
    );
    assert_eq!(
        err("at", "Money.nope(text)"),
        "`Money` has no `nope`; it has `parse(text)`"
    );
}

// ---------------------------------------------------------------------------------
// String

#[test]
fn a_string_converts_to_an_int_or_a_uuid() {
    assert_eq!(
        read("count", "text.to_int()", "42"),
        Value::some(Value::Int(42))
    );
    assert_eq!(
        read("count", "text.to_int()", "4 2"),
        Value::none(Type::Int)
    );
    assert_eq!(
        read(
            "id",
            "text.to_uuid()",
            "0190d1a1-0000-7000-8000-000000000001"
        ),
        Value::some(Value::uuid("0190d1a1-0000-7000-8000-000000000001"))
    );
    assert_eq!(
        read("id", "text.to_uuid()", "nope"),
        Value::none(Type::Uuid)
    );
}

#[test]
fn starts_with_and_strip_prefix_are_a_pair() {
    assert_eq!(
        read("flag", "text.starts_with(\"FW:\")", "FW:abc"),
        Value::Bool(true)
    );
    assert_eq!(
        read("flag", "text.starts_with(\"FW:\")", "abc"),
        Value::Bool(false)
    );
    assert_eq!(
        read("text", "text.strip_prefix(\"FW:\")", "FW:abc"),
        Value::str("abc")
    );
    // Unchanged rather than absent, because it is written after the `starts_with` that
    // already decided.
    assert_eq!(
        read("text", "text.strip_prefix(\"FW:\")", "abc"),
        Value::str("abc")
    );
}

#[test]
fn after_last_is_total() {
    assert_eq!(
        read("text", "text.after_last(\"/\")", "gid://shopify/Product/7"),
        Value::str("7")
    );
    // The whole string when the separator is absent, which is what makes the chain
    // `gid.after_last("/").to_int()` safe on something that is not a gid.
    assert_eq!(read("text", "text.after_last(\"/\")", "7"), Value::str("7"));
    assert_eq!(read("text", "text.after_last(\"/\")", ""), Value::str(""));
    assert_eq!(
        read("count", "text.after_last(\"/\").to_int()", "gid://x/12"),
        Value::some(Value::Int(12))
    );
}

/// The whole point of the set: read an id out of a global id without a branch per step.
#[test]
fn the_helpers_chain() {
    assert_eq!(
        read(
            "count",
            "text.trim().after_last(\"/\").to_int()",
            "  gid://shopify/ProductVariant/99  "
        ),
        Value::some(Value::Int(99))
    );
}
