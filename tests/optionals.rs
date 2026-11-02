//! `docs/optionals.md` as executable tests. Narrowing is observable two ways and both
//! are used here: a narrowed value satisfies a non-optional target, and reading an
//! optional's methods off one is rejected with a message that says why.

use heklang::{Event, EventPath, Interpreter, Outcome, Type, Value, parse};

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

// ---------------------------------------------------------------------------------
// A filter is a declared position too

/// **A bare value filtering an optional field has to be fitted at run time, or the
/// slice folds nothing.** The parser accepts `on @a.failed(fulfillment_order_id)` where
/// the field is declared `Int?`, because that is the coercion rule at a declared
/// position. The predicate the fold then reads with used to carry the unwrapped value,
/// so it compared `Int` against the `Int?` the event holds, matched no record and
/// silently answered the seed.
///
/// Silent is what makes it worth its own test: the arm reads as written, `hek check`
/// passes, and the only symptom is a value that is always the seed. It was found by
/// restoring a converted application's read-model tests, where one arm of one fold
/// existed solely to clear a deadline on a terminal failure and never fired.
#[test]
fn a_filter_fits_a_bare_value_to_an_optional_field() {
    let source = "event @hold.failed { order_id: Int, fulfillment_order_id: Int?, retryable: Bool }

fold FailedFor(fulfillment_order_id: Int) -> n: Int = 0 {
  on @hold.failed(fulfillment_order_id) => n + 1
}
";
    let program = parse(source).unwrap_or_else(|err| panic!("expected this to parse: {err}"));
    let mut interpreter = Interpreter::new(&program);
    for id in [Value::some(Value::Int(8001)), Value::none(Type::Int)] {
        interpreter.append(Event::new(
            EventPath::new(["hold", "failed"]),
            [
                ("order_id".to_string(), Value::Int(9001)),
                ("fulfillment_order_id".to_string(), id),
                ("retryable".to_string(), Value::Bool(false)),
            ],
        ));
    }

    let folded = interpreter
        .fold(
            "FailedFor",
            [("fulfillment_order_id".to_string(), Value::Int(8001))],
        )
        .expect("folded");
    assert_eq!(
        folded,
        Value::Int(1),
        "the present one matches and the absent one does not"
    );
}

/// The shape the gap was found on, twice: a fold arm carrying the accumulator forward or
/// replacing it with the envelope's append time. Both arms are the value, and until the
/// `if` lifted each of them this had to be split across two `return`s in a `fn`.
#[test]
fn a_fold_arm_may_choose_between_the_bare_value_and_the_optional() {
    let source = "subject Shop(Int)
const FULL: Int = 100

event @usage.threshold.reached { shop_id: Shop, threshold: Int }

fold CapReachedAt(shop_id: Shop) -> at: Timestamp? = none {
  on @usage.threshold.reached as e (shop_id) { threshold } =>
    if at.is_some() || threshold < FULL { at } else { e.at }
}
";
    let program = parse(source).unwrap_or_else(|err| panic!("expected this to parse: {err}"));
    let mut interpreter = Interpreter::new(&program);
    for threshold in [80, 100, 100] {
        interpreter.append(Event::new(
            EventPath::new(["usage", "threshold", "reached"]),
            [
                ("shop_id".to_string(), Value::Int(1)),
                ("threshold".to_string(), Value::Int(threshold)),
            ],
        ));
    }

    let folded = interpreter
        .fold("CapReachedAt", [("shop_id".to_string(), Value::Int(1))])
        .expect("folded");
    // The second event, which is the first at or past the cap: one minute per event from
    // the harness's epoch, and the accumulator holds against the third.
    assert_eq!(folded, Value::some(Value::Timestamp(1_577_836_860_000_000)));
}

/// **An absent value is "no value yet", and selecting with one has an answer.** Ordering
/// an optional is refused because an absent value is neither before nor after anything,
/// but `max` is not asking which is bigger: it is asking which to keep, and with one side
/// absent there is one value to keep. The result is a bare value precisely because the
/// argument is present, which is what lets a "latest so far" arm fold into a `T?` and
/// never narrow.
///
/// The shape is a real port's, and the backdated record is why it is not a replace: a log
/// is not ordered by the times its events carry, so the second record here is older than
/// the first.
#[test]
fn a_selection_on_an_optional_treats_absent_as_no_value_yet() {
    let source = "subject User(Int)

event @game.played { user_id: User, played_at: Timestamp }

fold UserLastSeen(user_id: User) -> seen: Timestamp? = none {
  on @game.played(user_id) { played_at } => seen.max(played_at)
}

fold UserFirstSeen(user_id: User) -> seen: Timestamp? = none {
  on @game.played(user_id) { played_at } => seen.min(played_at)
}
";
    let program = parse(source).unwrap_or_else(|err| panic!("expected this to parse: {err}"));
    let mut interpreter = Interpreter::new(&program);
    let newer = 1_893_456_000_000_000;
    let older = 1_577_836_800_000_000;
    for at in [newer, older] {
        interpreter.append(Event::new(
            EventPath::new(["game", "played"]),
            [
                ("user_id".to_string(), Value::Int(1)),
                ("played_at".to_string(), Value::Timestamp(at)),
            ],
        ));
    }

    let folded = |fold: &str, user: i64| {
        interpreter
            .fold(fold, [("user_id".to_string(), Value::Int(user))])
            .expect("folded")
    };
    // The seed took the first record's value with nothing to compare it against, and the
    // older second record did not displace it.
    assert_eq!(
        folded("UserLastSeen", 1),
        Value::some(Value::Timestamp(newer))
    );
    assert_eq!(
        folded("UserFirstSeen", 1),
        Value::some(Value::Timestamp(older))
    );
    // A key no event touched folds to the seed, and the seed is still absent: nothing
    // here invents a moment to compare against.
    assert_eq!(folded("UserLastSeen", 9), Value::none(Type::Timestamp));
}

/// A present receiver answers what the bare method answers, which is the property that
/// makes narrowing invisible here: `at.max(x)` means one thing whether or not a branch
/// above it has proved `at` present. Both spellings are folded and compared, because a
/// table that typed them the same while the interpreter answered differently would pass
/// every check this file otherwise makes.
#[test]
fn a_selection_reads_the_same_through_a_narrowing() {
    let arm = |body: &str| {
        format!(
            "event @x.happened {{ at: Timestamp }}

fold Latest() -> seen: Timestamp? = none {{
  on @x.happened {{ at }} => {body}
}}
"
        )
    };
    let folded = |body: &str| {
        let source = arm(body);
        let program = parse(&source).unwrap_or_else(|err| panic!("for `{body}`: {err}"));
        let mut interpreter = Interpreter::new(&program);
        for at in [1_893_456_000_000_000i64, 1_577_836_800_000_000] {
            interpreter.append(Event::new(
                EventPath::new(["x", "happened"]),
                [("at".to_string(), Value::Timestamp(at))],
            ));
        }
        interpreter
            .fold("Latest", Vec::<(String, Value)>::new())
            .expect("folded")
    };

    let through_the_optional = folded("seen.max(at)");
    let through_a_narrowing = folded("if seen != none { seen.max(at) } else { at }");
    let the_long_way = folded("seen.unwrap_or(at).max(at)");
    assert_eq!(through_the_optional, through_a_narrowing);
    assert_eq!(through_the_optional, the_long_way);
    assert_eq!(
        through_the_optional,
        Value::some(Value::Timestamp(1_893_456_000_000_000))
    );
}

/// The argument is the bare type and not the optional one, so two optionals is a type
/// error with the way out on it. Deliberate rather than an omission: a selection between
/// two values that may both be absent answers an optional, and an arm taking one would be
/// back to narrowing, which is the thing this removes.
#[test]
fn a_selection_on_an_optional_still_takes_a_bare_argument() {
    let source = "event @x.happened { at: Timestamp?, other: Timestamp? }

fold Latest() -> seen: Timestamp? = none {
  on @x.happened { at, other } => at.max(other)
}
";
    let message = parse(source)
        .expect_err("expected this to be rejected")
        .text();
    assert!(
        message.contains("expected Timestamp, found Timestamp?"),
        "got: {message}"
    );
    assert!(message.contains("unwrap_or"), "got: {message}");
}

/// Every type the selection exists on, through an optional, because the argument is fitted
/// against the receiver's **inner** type and that is a different code path from the bare
/// method's. The two subject flavours are here for the same reason: a subject orders
/// through the scalar its ids are, so its values arrive as that scalar and the fit has to
/// read through the declaration to accept one.
#[test]
fn an_optional_selection_covers_every_type_that_orders() {
    let source = "subject Shop(Int)
subject Tag(String)

event @sale.made { shop_id: Shop, tag_id: Tag, total: Money(2), rate: Decimal(4) }

fold TopSale(shop_id: Shop) -> most: Money(2)? = none {
  on @sale.made(shop_id) { total } => most.max(total)
}

fold LowRate(shop_id: Shop) -> least: Decimal(4)? = none {
  on @sale.made(shop_id) { rate } => least.min(rate)
}

fold HighestShop() -> which: Shop? = none {
  on @sale.made { shop_id } => which.max(shop_id)
}

fold LastTag() -> which: Tag? = none {
  on @sale.made { tag_id } => which.max(tag_id)
}
";
    let program = parse(source).unwrap_or_else(|err| panic!("expected this to parse: {err}"));
    let mut interpreter = Interpreter::new(&program);
    for (shop, tag, total, rate) in [(1i64, "a", 1000i64, 5000i64), (4, "z", 250, 9000)] {
        interpreter.append(Event::new(
            EventPath::new(["sale", "made"]),
            [
                ("shop_id".to_string(), Value::Int(shop)),
                ("tag_id".to_string(), Value::str(tag)),
                ("total".to_string(), Value::money(total, 2)),
                ("rate".to_string(), Value::decimal(rate, 4)),
            ],
        ));
    }

    let folded = |fold: &str, args: Vec<(String, Value)>| {
        interpreter
            .fold(fold, args)
            .unwrap_or_else(|err| panic!("{fold}: {err}"))
    };
    let shop = vec![("shop_id".to_string(), Value::Int(1))];
    // The scale in is the scale out, which is the whole reason the argument is declared as
    // the receiver's own type rather than widened to it.
    assert_eq!(
        folded("TopSale", shop.clone()),
        Value::some(Value::money(1000, 2))
    );
    assert_eq!(
        folded("LowRate", shop),
        Value::some(Value::decimal(5000, 4))
    );
    // `same` rather than `==` for the two subjects, which is the comparison a test
    // expectation itself makes: the accumulator is declared `Shop?`, so it holds an `Opt`
    // whose inner type is the subject while its content is the scalar the ids are.
    assert!(
        folded("HighestShop", Vec::new()).same(&Value::some(Value::Int(4))),
        "the highest shop id"
    );
    assert!(
        folded("LastTag", Vec::new()).same(&Value::some(Value::str("z"))),
        "the last tag"
    );
}
