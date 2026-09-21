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

/// The same, for a `Make` taking two optionals. A compound condition needs two to say
/// anything, since the interesting part is which of them a connective can speak for.
fn run_with(body: &str, text: Option<&str>, tag: Option<&str>) -> Outcome {
    let program = source(body);
    let program = parse(&program).unwrap_or_else(|err| panic!("expected this to parse: {err}"));
    let arg = |value: Option<&str>| match value {
        Some(value) => Value::some(Value::str(value)),
        None => Value::none(Type::String),
    };
    Interpreter::new(&program)
        .run(
            "Make",
            vec![
                ("id", Value::uuid(ID)),
                ("text", arg(text)),
                ("tag", arg(tag)),
            ],
        )
        .unwrap_or_else(|err| panic!("expected this to run: {err}"))
        .outcome
}

fn made_with(body: &str, text: Option<&str>, tag: Option<&str>) -> Value {
    match run_with(body, text, tag) {
        Outcome::Ok(events) => events[0].field("text").cloned().expect("the field"),
        other => panic!("expected an append, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------------
// Comparing one

/// The answer the whole rule rests on: an absent value is unequal to every present one.
/// Run rather than parsed, because the type rule passing says nothing about which way
/// the branch went.
#[test]
fn an_absent_value_equals_nothing_present() {
    const MAKE: &str = "command Make(id: Uuid, text: String?) {
  if text == \"hello\" {
    invalid \"equal\"
  }
  emit @note.made { id, text: \"not equal\" }
}";
    assert!(matches!(run(MAKE, Some("hello")), Outcome::Invalid(_)));
    assert_eq!(made(MAKE, "other"), Value::str("not equal"));
    // The one that used to need an `unwrap_or` and a sentinel to ask.
    assert!(matches!(run(MAKE, None), Outcome::Ok(_)));
}

/// The reading that is easiest to get wrong, so it is written down as a test as well as
/// in the doc: `!=` is **true** for an absent value.
#[test]
fn an_absent_value_is_unequal_under_ne() {
    const MAKE: &str = "command Make(id: Uuid, text: String?) {
  if text != \"hello\" {
    invalid \"different\"
  }
  emit @note.made { id, text: \"same\" }
}";
    assert_eq!(made(MAKE, "hello"), Value::str("same"));
    assert!(matches!(run(MAKE, Some("other")), Outcome::Invalid(_)));
    assert!(matches!(run(MAKE, None), Outcome::Invalid(_)));
}

/// Which side is bare decides nothing. The lift is on whichever operand is not the
/// optional, so both spellings are the same comparison.
#[test]
fn either_operand_may_be_the_bare_one() {
    const MAKE: &str = "command Make(id: Uuid, text: String?) {
  if \"hello\" == text {
    invalid \"equal\"
  }
  emit @note.made { id, text: \"not equal\" }
}";
    assert!(matches!(run(MAKE, Some("hello")), Outcome::Invalid(_)));
    assert!(matches!(run(MAKE, None), Outcome::Ok(_)));
}

/// Asking for absence directly still works and still means what it meant. It needs no
/// lift, both sides being optionals already, which is what keeps every digest that has
/// one unchanged.
#[test]
fn an_optional_still_compares_against_none() {
    const MAKE: &str = "command Make(id: Uuid, text: String?) {
  if text == none {
    invalid \"absent\"
  }
  emit @note.made { id, text: \"present\" }
}";
    assert_eq!(made(MAKE, "hello"), Value::str("present"));
    assert!(matches!(run(MAKE, None), Outcome::Invalid(_)));
}

/// A container on both sides, which is where the lift has a type to get wrong: the `[]`
/// resolves through the optional hint, so the element type on the bare side is the one the
/// comparison was written for rather than a guess from an empty list.
///
/// It does not reach the case the node's `inner` field exists for. An `Expr::Comp` that
/// yields nothing evaluates to `List(Json)` while its static type says otherwise, and that
/// disagreement is older than this rule and reaches an `emit` the same way; see the note
/// in `interp.rs`.
#[test]
fn the_lift_takes_its_type_from_the_comparison() {
    const MAKE: &str = "fn listed(t: String?) -> List(String)? {
  if t.is_none() {
    return none
  }
  return []
}

command Make(id: Uuid, text: String?) {
  if listed(text) == [] {
    invalid \"empty\"
  }
  emit @note.made { id, text: \"absent\" }
}";
    assert!(matches!(run(MAKE, Some("hello")), Outcome::Invalid(_)));
    assert!(matches!(run(MAKE, None), Outcome::Ok(_)));
}

/// `T??` is unspellable, but `List(T?).first()` answers one, so the rule has to hold a
/// level down as well. It does without a case of its own: the lift is one level at the
/// outside, the same place the wrap has always been.
#[test]
fn an_optional_of_an_optional_compares_the_same_way() {
    const MAKE: &str = "command Make(id: Uuid, text: String?) {
  let xs = [text]
  if xs.first() == text {
    invalid \"head is text\"
  }
  emit @note.made { id, text: \"head is not text\" }
}";
    // Present or absent, the head of a one-item list is that item: the lift nested the
    // bare side rather than flattening it.
    assert!(matches!(run(MAKE, Some("hello")), Outcome::Invalid(_)));
    assert!(matches!(run(MAKE, None), Outcome::Invalid(_)));
}

/// A comparison proves the value present in the branch it guards, and deliberately does
/// not say so. `docs/optionals.md` has the argument; this is the shape it costs.
#[test]
fn an_equality_does_not_narrow() {
    let program = source(
        "command Make(id: Uuid, text: String?) {
  if text == \"hello\" {
    emit @note.made { id, text }
  }
}",
    );
    let message = parse(&program)
        .expect_err("an equality narrows nothing")
        .text();
    assert!(
        message.contains("expected String, found String?"),
        "got: {message}"
    );
}

// ---------------------------------------------------------------------------------
// The two forms

/// The early-return shape. Reaching past the `if` means the condition was false,
/// because the branch it guards never falls through.
#[test]
fn an_early_return_narrows_the_remainder() {
    const MAKE: &str = "command Make(id: Uuid, text: String?) {
  if text.is_none() {
    invalid \"no text\"
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
  invalid \"no text\"
}";
    assert_eq!(made(MAKE, "hello"), Value::str("hello"));
    assert!(matches!(run(MAKE, None), Outcome::Invalid(_)));
}

/// `!` swaps which branch the proof is about, and nothing else changes.
#[test]
fn a_negated_test_narrows_the_other_way() {
    const EARLY: &str = "command Make(id: Uuid, text: String?) {
  if !text.is_some() {
    invalid \"no text\"
  }
  emit @note.made { id, text }
}";
    assert_eq!(made(EARLY, "hello"), Value::str("hello"));

    const BRANCH: &str = "command Make(id: Uuid, text: String?) {
  if !text.is_none() {
    emit @note.made { id, text }
    return
  }
  invalid \"no text\"
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
    invalid \"has text\"
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
    invalid \"no text\"
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
      invalid \"no text\"
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
    invalid \"never\"
  } else if text.is_none() {
    invalid \"no text\"
  }
  emit @note.made { id, text: text.unwrap_or(\"\") }
}",
    );
    parse(&program).expect("an else if narrows nothing beyond itself");
}

/// A conjunction carries what its halves prove where it is **true**, and a
/// disjunction where it is **false**. The pair below is the whole rule: the same two
/// tests, the same branch, opposite answers.
#[test]
fn a_conjunction_and_a_disjunction_prove_opposite_sides() {
    // `||` false means both halves were false, so reaching past the early exit proves
    // both present. This is the shape a port wrote as two nested `if`s.
    const EITHER: &str = "command Make(id: Uuid, text: String?, tag: String?) {
  if text.is_none() || tag.is_none() {
    invalid \"needs both\"
  }
  emit @note.made { id, text: \"{text}{tag}\" }
}";
    assert_eq!(
        made_with(EITHER, Some("a"), Some("b")),
        Value::str("ab"),
        "past the exit both are present"
    );
    assert!(matches!(
        run_with(EITHER, None, Some("b")),
        Outcome::Invalid(_)
    ));
    assert!(matches!(
        run_with(EITHER, Some("a"), None),
        Outcome::Invalid(_)
    ));

    // `&&` true means both halves were true, so the body is where both are present.
    const BOTH: &str = "command Make(id: Uuid, text: String?, tag: String?) {
  if text.is_some() && tag.is_some() {
    emit @note.made { id, text: \"{text}{tag}\" }
    return
  }
  invalid \"needs both\"
}";
    assert_eq!(made_with(BOTH, Some("a"), Some("b")), Value::str("ab"));
    assert!(matches!(
        run_with(BOTH, None, Some("b")),
        Outcome::Invalid(_)
    ));
}

/// The half that has to stay refused. A true disjunction does not say which side made
/// it true, and a false conjunction does not say which side failed, so neither proves
/// anything and `unwrap_or` is still the only way through.
#[test]
fn neither_connective_proves_the_side_it_cannot_speak_for() {
    for cond in [
        "text.is_some() || tag.is_some()",
        "!(text.is_none() && tag.is_none())",
    ] {
        let program = source(&format!(
            "command Make(id: Uuid, text: String?, tag: String?) {{
  if {cond} {{
    emit @note.made {{ id, text }}
    return
  }}
  invalid \"needs one\"
}}"
        ));
        let message = parse(&program)
            .expect_err(&format!("{cond} proves nothing in its then branch"))
            .text();
        assert!(
            message.starts_with("expected String, found String?"),
            "for {cond}: {message}"
        );
    }

    // And the sides they *can* speak for are still the other way round: the `else` of
    // an `&&` and the body of a `||` both learn nothing.
    let program = source(
        "command Make(id: Uuid, text: String?, tag: String?) {
  if text.is_some() && tag.is_some() {
    invalid \"has both\"
  } else {
    emit @note.made { id, text: text.unwrap_or(\"\") }
  }
}",
    );
    parse(&program).expect("the else of an && proves nothing, so unwrap_or is still there");
}

/// The right operand of a `&&` runs only when the left was true, so it is parsed with
/// the left's proof in hand. Without this `if x.is_some() && x > y` is refused for
/// ordering an optional, which is the one the port hit most.
#[test]
fn an_operand_sees_what_the_operand_before_it_proved() {
    const ORDERED: &str = "command Make(id: Uuid, text: String?, tag: String?) {
  if text.is_some() && text > \"m\" {
    emit @note.made { id, text }
    return
  }
  invalid \"no\"
}";
    assert_eq!(made_with(ORDERED, Some("z"), None), Value::str("z"));
    // Short-circuit is what makes it sound: with `text` absent the comparison is never
    // reached, so the unwrap a narrowed load lowers to is never evaluated.
    assert!(matches!(run_with(ORDERED, None, None), Outcome::Invalid(_)));

    // The mirror: a `||`'s right operand runs only when the left was false.
    const MIRRORED: &str = "command Make(id: Uuid, text: String?, tag: String?) {
  if text.is_none() || text < \"m\" {
    invalid \"no\"
  }
  emit @note.made { id, text }
}";
    assert_eq!(made_with(MIRRORED, Some("z"), None), Value::str("z"));
    assert!(matches!(
        run_with(MIRRORED, None, None),
        Outcome::Invalid(_)
    ));
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
    invalid \"no text\"
  }}
  invalid \"{{text.{method}}}\"
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

// ---------------------------------------------------------------------------------
// The receiver

/// A narrowing rewrites the type of a slot, so the receiver has to be a name. A field
/// access proves the very thing the rule is about and narrows nothing, and a `let` is
/// the whole of the workaround.
#[test]
fn a_field_access_does_not_narrow() {
    const DECLS: &str = "record Item { plan_id: Uuid? }
event @item.synced { id: Uuid, plan_id: Uuid }
";

    let through_the_field = format!(
        "{DECLS}command Sync(id: Uuid, item: Item) {{
  if item.plan_id.is_some() {{
    emit @item.synced {{ id, plan_id: item.plan_id }}
  }}
}}
"
    );
    let message = parse(&through_the_field)
        .expect_err("a field access is not a slot")
        .text();
    assert!(
        message.starts_with("expected Uuid, found Uuid?"),
        "{message}"
    );

    let through_a_let = format!(
        "{DECLS}command Sync(id: Uuid, item: Item) {{
  let plan_id = item.plan_id
  if plan_id.is_some() {{
    emit @item.synced {{ id, plan_id }}
  }}
}}
"
    );
    parse(&through_a_let)
        .unwrap_or_else(|err| panic!("a `let` binds a slot the branch narrows: {err}"));
}
