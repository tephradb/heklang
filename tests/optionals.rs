//! `docs/optionals.md` as executable tests. Narrowing is observable two ways and both
//! are used here: a narrowed value satisfies a non-optional target, and reading an
//! optional's methods off one is rejected with a message that says why.

use heklang::{Interpreter, Outcome, Type, Value, parse};

const ID: &str = "0190d1a1-0000-7000-8000-000000000001";

const PRELUDE: &str = "event @note.made { id: Uuid, text: String }
";

fn source(body: &str) -> String {
    format!("{PRELUDE}{body}\n")
}

/// Runs `Make` with `text` present or absent, and returns what it decided. An event
/// field is `String` rather than `String?`, so an un-narrowed `text` reaching `emit`
/// is a type mismatch rather than a quietly wrapped value.
fn run(body: &str, text: Option<&str>) -> Outcome {
    let program = source(body);
    let program = parse(&program).unwrap_or_else(|err| panic!("expected this to parse: {err}"));
    let mut interpreter = Interpreter::new(&program);
    let arg = match text {
        Some(text) => Value::some(Value::str(text)),
        None => Value::none(Type::String),
    };
    interpreter
        .run("Make", vec![("id", Value::uuid(ID)), ("text", arg)])
        .unwrap_or_else(|err| panic!("expected this to run: {err}"))
        .outcome
}

fn made(body: &str, text: &str) -> Value {
    match run(body, Some(text)) {
        Outcome::Ok(events) => events[0].field("text").cloned().expect("the field"),
        other => panic!("expected an append, got {other:?}"),
    }
}

fn err(body: &str) -> String {
    parse(&source(body))
        .expect_err("expected this to be rejected")
        .text()
}

// ---------------------------------------------------------------------------------
// The two forms

/// The early-return shape. Reaching past the `if` means the condition was false,
/// because the branch it guards never falls through.
#[test]
fn an_early_return_narrows_the_remainder() {
    const MAKE: &str = "command Make(id: Uuid, text: String?) {
  if text.is_none() {
    return invalid(\"no text\")
  }
  emit @note.made { id, text }
}";
    assert_eq!(made(MAKE, "hello"), Value::str("hello"));
    assert!(matches!(run(MAKE, None), Outcome::Invalid(_)));
}

#[test]
fn an_is_some_branch_narrows_its_body() {
    const MAKE: &str = "command Make(id: Uuid, text: String?) {
  if text.is_some() {
    emit @note.made { id, text }
    return
  }
  return invalid(\"no text\")
}";
    assert_eq!(made(MAKE, "hello"), Value::str("hello"));
    assert!(matches!(run(MAKE, None), Outcome::Invalid(_)));
}

/// `!` swaps which branch the proof is about, and nothing else changes.
#[test]
fn a_negated_test_narrows_the_other_way() {
    const EARLY: &str = "command Make(id: Uuid, text: String?) {
  if !text.is_some() {
    return invalid(\"no text\")
  }
  emit @note.made { id, text }
}";
    assert_eq!(made(EARLY, "hello"), Value::str("hello"));

    const BRANCH: &str = "command Make(id: Uuid, text: String?) {
  if !text.is_none() {
    emit @note.made { id, text }
    return
  }
  return invalid(\"no text\")
}";
    assert_eq!(made(BRANCH, "hello"), Value::str("hello"));
}

/// The else branch of an `is_some` is where the value is absent, so nothing is proved
/// there. This is the direction a lexical check gets wrong.
#[test]
fn the_other_branch_is_not_narrowed() {
    let program = source(
        "command Make(id: Uuid, text: String?) {
  if text.is_some() {
    return invalid(\"has text\")
  } else {
    emit @note.made { id, text: text.unwrap_or(\"\") }
  }
}",
    );
    // `unwrap_or` is still available there, which is the proof: it is rejected only
    // where the value was narrowed.
    parse(&program).expect("the else of an is_some proves nothing");
}

// ---------------------------------------------------------------------------------
// Where a narrowing ends

/// Same expression, two nestings: rejected where the proof reaches it and accepted
/// where it does not.
#[test]
fn a_narrowing_ends_with_its_block() {
    let message = err("command Make(id: Uuid, text: String?) {
  if text.is_none() {
    return invalid(\"no text\")
  }
  emit @note.made { id, text: text.unwrap_or(\"\") }
}");
    assert!(
        message.contains("already proved this one present"),
        "got: {message}"
    );

    let program = source(
        "command Make(id: Uuid, text: String?) {
  if id == id {
    if text.is_none() {
      return invalid(\"no text\")
    }
  }
  emit @note.made { id, text: text.unwrap_or(\"\") }
}",
    );
    parse(&program).expect("the narrowing ended with the inner block");
}

/// What a chain proves as a whole depends on every arm above it, so a narrowing proved
/// inside an `else if` does not escape the chain.
#[test]
fn an_else_if_does_not_leak_its_narrowing() {
    let program = source(
        "command Make(id: Uuid, text: String?) {
  if id != id {
    return invalid(\"never\")
  } else if text.is_none() {
    return invalid(\"no text\")
  }
  emit @note.made { id, text: text.unwrap_or(\"\") }
}",
    );
    parse(&program).expect("an else if narrows nothing beyond itself");
}

/// A conjunction and a disjunction narrow in opposite directions, and getting that
/// wrong is silent, so neither narrows.
#[test]
fn a_compound_condition_narrows_nothing() {
    for cond in ["text.is_none() || id != id", "text.is_none() && id == id"] {
        let program = source(&format!(
            "command Make(id: Uuid, text: String?) {{
  if {cond} {{
    return invalid(\"no text\")
  }}
  emit @note.made {{ id, text: text.unwrap_or(\"\") }}
}}"
        ));
        parse(&program).unwrap_or_else(|err| panic!("for {cond}: {err}"));
    }
}

// ---------------------------------------------------------------------------------
// The message

/// Without this the mistake arrives at run time as "String has no method `unwrap_or`",
/// which names the symptom rather than the cause.
#[test]
fn an_optional_method_on_a_narrowed_value_says_why() {
    for method in ["unwrap_or(\"\")", "is_some()", "is_none()"] {
        let message = err(&format!(
            "command Make(id: Uuid, text: String?) {{
  if text.is_none() {{
    return invalid(\"no text\")
  }}
  return invalid(\"{{text.{method}}}\")
}}"
        ));
        assert!(
            message.contains("already proved this one present"),
            "for {method}: {message}"
        );
        assert!(
            message.contains("so it is a String here"),
            "for {method}: {message}"
        );
    }
}

// `a_narrowed_optional_can_be_revealed` lives in `tests/effects.rs`, next to the rest
// of rule 12, because it needs an effect and a subject-bound event to say anything.
