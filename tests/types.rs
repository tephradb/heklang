//! `docs/types.md` as executable tests, one test per numbered rule.
//!
//! The check under test is the one that runs at every declared position, so most of
//! these are a program that used to pass `hek check` and fail only when something ran
//! it. The interesting half is section 2's "unknown is not an error": a type checker is
//! worth what it rejects, and worth nothing if it rejects a correct program.

use heklang::{Interpreter, Outcome, Program, Value, parse};

const PRELUDE: &str = "enum Status { @default Draft, Active }

record Facts {
  note: String,
  count: Int,
}

event @thing.happened {
  id: Int,
  name: String,
  maybe: String?,
  at: Timestamp,
  total: Money(2),
  rate: Decimal(4),
  status: Status,
  tags: List(String),
  facts: Facts,
  blob: Json,
}

event @thing.touched { id: Int }

fn maybe(t: String) -> String? {
  if t.is_empty() {
    return none
  }
  return t
}
";

fn source(body: &str) -> String {
    format!("{PRELUDE}{body}\n")
}

fn program(body: &str) -> Program {
    parse(&source(body)).unwrap_or_else(|err| panic!("expected this to parse: {err}"))
}

fn err(body: &str) -> String {
    parse(&source(body))
        .expect_err("expected this to be rejected")
        .message
}

/// A command that emits one event, with every field written out and `field` overridden.
fn emitting(params: &str, field: &str, value: &str) -> String {
    let mut fields = vec![
        ("id", "id"),
        ("name", "\"n\""),
        ("maybe", "none"),
        ("at", "\"2026-01-01T00:00:00Z\""),
        ("total", "0"),
        ("rate", "0"),
        ("status", "Draft"),
        ("tags", "[]"),
        ("facts", "Facts { note: \"n\", count: 0 }"),
        ("blob", "Json.empty"),
    ];
    for entry in &mut fields {
        if entry.0 == field {
            entry.1 = value;
        }
    }
    let written: Vec<String> = fields
        .iter()
        .map(|(name, value)| format!("{name}: {value}"))
        .collect();
    format!(
        "command C(id: Int{params}) {{\n  emit @thing.happened {{ {} }}\n}}",
        written.join(", ")
    )
}

/// The event a command appended, so a test can check that a value that passed the type
/// check is also the value the runtime stored.
fn fired(params: &str, field: &str, value: &str, args: Vec<(&str, Value)>) -> Value {
    let program = program(&emitting(params, field, value));
    let mut interpreter = Interpreter::new(&program);
    let mut all = vec![("id", Value::Int(1))];
    all.extend(args);
    let execution = interpreter
        .run("C", all)
        .unwrap_or_else(|err| panic!("expected this to run: {err}"));
    match execution.outcome {
        Outcome::Ok(events) => events[0].field(field).cloned().expect("the field"),
        other => panic!("expected an append, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------------
// Rule 1: the types, and equality is exact.

#[test]
fn equality_is_exact_and_structural() {
    // Two scales are two types, in both of the scaled families.
    for (declared, value) in [
        ("d: Decimal(2)", "rate: d"),
        ("m: Money(3)", "total: m"),
        ("s: String", "id: s"),
        ("n: Int", "name: n"),
    ] {
        let (field, expr) = value.split_once(": ").expect("a field and an expression");
        let message = err(&emitting(&format!(", {declared}"), field, expr));
        assert!(
            message.starts_with("expected "),
            "for `{declared}` into `{field}`, got: {message}"
        );
    }
}

/// `Response`, `Outcome` and `Rounding` have no spelling in a type position, so nothing
/// may declare one. `Response` is the exception `docs/functions.md` carves out, and it
/// is a `fn` signature only.
#[test]
fn the_unwritable_types_cannot_be_declared() {
    for ty in ["Outcome", "Rounding", "Response"] {
        let message = err(&format!(
            "command C(id: Int, x: {ty}) {{\n  emit @thing.touched {{ id }}\n}}"
        ));
        assert!(
            message.contains(&format!("unknown type `{ty}`")),
            "for {ty}, got: {message}"
        );
    }
    // A `fn` may name one, and only a `fn`.
    program(
        "fn code(r: Response) -> Int {\n  return r.status\n}\n\ncommand C(id: Int) {\n  emit @thing.touched { id }\n}",
    );
}

// ---------------------------------------------------------------------------------
// Rule 2: synthesis.

/// The case the port found. Every one of these passed `hek check` and failed only when
/// something ran it.
#[test]
fn an_optional_does_not_fill_a_required_position() {
    for (params, value) in [
        (", text: String", "text.to_int()"),
        (", text: String", "text.to_uuid()"),
        (", text: String", "Timestamp.parse(text)"),
        (", m: Map(Int, String)", "m.get(id)"),
        (", xs: List(String)", "xs.first()"),
    ] {
        let message = err(&emitting(params, "name", value));
        assert!(
            message.starts_with("expected String, found "),
            "for `{value}`, got: {message}"
        );
    }
}

/// A `fn` boundary is the same rule reached from two more directions: an argument
/// against a parameter, and a `return` against the declared result. Both directions
/// wrap a bare value into an optional and neither unwraps one.
#[test]
fn a_fn_boundary_is_checked_in_both_directions() {
    // `return t` where the result is `String?`: the bare value wraps.
    program(&emitting(", text: String", "maybe", "maybe(text)"));
    // An argument that is an optional where the parameter is not.
    let message = err(&emitting(", text: String", "name", "maybe(maybe(text))"));
    assert_eq!(
        message,
        "expected String, found String?; `unwrap_or` gives it a fallback, or a branch that proves it present makes it a String without one"
    );
}

/// The message carries the way out, because in this shape the author knows what they
/// meant and only needs the spelling.
#[test]
fn the_optional_message_names_the_way_out() {
    let message = err(&emitting(", text: String", "name", "maybe(text)"));
    assert!(
        message.contains("`unwrap_or` gives it a fallback"),
        "got: {message}"
    );
    assert!(
        message.contains("a branch that proves it present"),
        "got: {message}"
    );
}

/// And a branch that proves it present really does satisfy the check, which is what
/// makes the advice above true rather than only encouraging.
#[test]
fn a_narrowed_optional_fills_a_required_position() {
    let body = "command C(id: Int, text: String?) {
  if text.is_none() {
    return invalid(\"no text\")
  }
  emit @thing.happened {
    id, name: text, maybe: none, at: \"2026-01-01T00:00:00Z\", total: 0, rate: 0,
    status: Draft, tags: [], facts: Facts { note: \"n\", count: 0 }, blob: Json.empty,
  }
}";
    let program = program(body);
    let mut interpreter = Interpreter::new(&program);
    let execution = interpreter
        .run(
            "C",
            vec![
                ("id", Value::Int(1)),
                ("text", Value::some(Value::str("here"))),
            ],
        )
        .expect("ran");
    match execution.outcome {
        Outcome::Ok(events) => {
            assert_eq!(events[0].field("name"), Some(&Value::str("here")));
        }
        other => panic!("expected an append, got {other:?}"),
    }
}

/// Unknown is an answer and it is never checked. A `let` bound to something synthesis
/// cannot type carries that forward, and nothing downstream is accused.
#[test]
fn an_unknown_type_is_not_an_accusation() {
    // `.mul` on a `Money` is known; `Json.encode` of one is a String; the interesting
    // case is a value whose type nothing can name, which is a Json accessor's payload.
    program(&emitting(
        ", blob_in: Json",
        "name",
        "blob_in.string(\"k\").unwrap_or(\"\")",
    ));
}

/// Both arms of a value-position `if` are the value, so its type is theirs only when
/// they agree, and two that disagree is a value with no type at all.
#[test]
fn an_if_expression_takes_both_arms() {
    let value = fired(
        ", flag: Bool",
        "name",
        "if flag { \"yes\" } else { \"no\" }",
        vec![("flag", Value::Bool(true))],
    );
    assert_eq!(value, Value::str("yes"));

    // Where a target declared a type, each arm meets it where it is written, so the
    // wrong one is named rather than the `if`.
    let message = err(&emitting(
        ", flag: Bool, n: Int",
        "name",
        "if flag { \"yes\" } else { n }",
    ));
    assert_eq!(message, "expected String, found Int");

    // Where nothing declared one, this is the only place the disagreement can be said.
    let message = err("command C(id: Int, flag: Bool) {
  let x = if flag { 1 } else { \"two\" }
  emit @thing.touched { id }
}");
    assert!(
        message.contains("give a Int and a String") && message.contains("both arms are the value"),
        "got: {message}"
    );
}

// ---------------------------------------------------------------------------------
// Rule 2: the operator table.

#[test]
fn a_pair_with_no_row_is_rejected() {
    for (params, value, expected) in [
        (
            ", a: Money(2), b: Money(2)",
            "a * b",
            "Money(2) and Money(2)",
        ),
        (
            ", a: Money(2), r: Decimal(4)",
            "a + r",
            "Money(2) and Decimal(4)",
        ),
        (
            ", a: Decimal(2), b: Decimal(4)",
            "a + b",
            "Decimal(2) and Decimal(4)",
        ),
        (", a: String, b: String", "a + b", "String and String"),
    ] {
        let message = err(&emitting(params, "total", value));
        assert!(message.contains(expected), "for `{value}`, got: {message}");
        assert!(message.starts_with("cannot apply"), "got: {message}");
    }
}

/// A comparison is held to the same table, so a scale mismatch is caught under `>` for
/// the reason it is caught under `+`.
#[test]
fn a_comparison_meets_at_one_scale() {
    let message = err("command C(a: Money(2), b: Money(3), id: Int) {
  if a > b {
    return
  }
  emit @thing.touched { id }
}");
    assert!(
        message.starts_with("cannot apply `>` to Money(2) and Money(3)"),
        "got: {message}"
    );
}

/// Rule 12 already covers interpolation and comparison. Arithmetic reads its operands
/// the same way: a sum of sealed content is plaintext derived from it.
#[test]
fn arithmetic_on_sealed_content_is_rejected() {
    let message = parse(
        "event @order.paid { order_id: Int, customer_id: Int, tip: Money(2) @subject(customer_id) }

effect E {
  on @order.paid as e { tip } {
    log(\"{reveal(tip) + tip}\")
  }
}
",
    )
    .expect_err("a sum of sealed content reads it")
    .message;
    assert!(
        message.contains("sealed under `customer_id`"),
        "got: {message}"
    );
    assert!(message.contains("arithmetic"), "got: {message}");
}

// ---------------------------------------------------------------------------------
// Rule 3: filling.

/// A bare value fills an optional and wraps, at one level and no further.
#[test]
fn a_bare_value_fills_an_optional_but_does_not_recurse() {
    assert_eq!(
        fired("", "maybe", "\"here\"", Vec::new()),
        Value::some(Value::str("here"))
    );
    // One level, at the outside. A `List(String?)` is not a `List(String)`, which is
    // the case `docs/optionals.md` names to say the wrap does not recurse.
    let message = err(&emitting(", xs: List(String?)", "tags", "xs"));
    assert_eq!(message, "expected List(String), found List(String?)");
}

/// A typed `Int` is not a `Money`, however freely an `Int` *literal* becomes one. The
/// literal resolves before it is a value; a value has a type and keeps it.
#[test]
fn an_int_value_does_not_fill_a_money_field() {
    // The literal is fine, and it is fine because it was never an `Int`.
    assert_eq!(fired("", "total", "10", Vec::new()), Value::money(1_000, 2));
    let message = err(&emitting(", n: Int", "total", "n"));
    assert_eq!(message, "expected Money(2), found Int");
}

/// A condition is a declared position like any other. This is the shape a real port
/// carried into production: a `String` where the branch wanted a question about it.
#[test]
fn a_condition_is_a_declared_position() {
    let message = err("command C(id: Int, text: String) {
  if text {
    return
  }
  emit @thing.touched { id }
}");
    assert_eq!(message, "expected Bool, found String");
}

/// Every position, because the rule is only worth something if it holds at all of them.
#[test]
fn the_rule_holds_at_every_declared_position() {
    let cases = [
        // a state seed and a fold arm
        (
            "command C(id: Int, text: String) {
  state seen: Int = fold text
  emit @thing.touched { id }
}",
            "expected Int, found String",
        ),
        // a record literal field
        (
            "command C(id: Int, text: String) {
  let f = Facts { note: 1, count: 0 }
  emit @thing.touched { id }
}",
            "expected String, found Int",
        ),
        // a list element, against the element type the target declared
        (
            "command C(id: Int, n: Int) {
  state xs: List(String) = fold [n]
  emit @thing.touched { id }
}",
            "expected String, found Int",
        ),
        // an entity key
        (
            "projector P {
  entity Row { id: Int @key, seen: Int }
  on @thing.touched { id } {
    patch Row[\"one\"] { seen: .seen + 1 }
  }
}",
            "expected Int, found String",
        ),
        // a projector column
        (
            "projector P {
  entity Row { id: Int @key, seen: Int }
  on @thing.happened { id, name } {
    put Row { id, seen: name }
  }
}",
            "expected Int, found String",
        ),
    ];
    for (body, expected) in cases {
        let message = err(body);
        assert!(
            message.contains(expected),
            "expected {expected:?}, got: {message}"
        );
    }
}

/// A test's `given` and `expect` are declared positions too, which is what stops a
/// suite from asserting something the program could never have produced.
#[test]
fn a_tests_values_are_checked_too() {
    let message = err("test \"a touch\" {
  given @thing.touched { id: \"one\" }
  project P
}

projector P {
  entity Row { id: Int @key, seen: Int }
  on @thing.touched { id } {
    put Row { id, seen: 0 }
  }
}");
    assert!(message.contains("expected Int"), "got: {message}");
}

// ---------------------------------------------------------------------------------
// Rule 3: methods.

/// The table that types a method's result knows whether there is one to type, so a
/// method the receiver does not have is caught where it is written.
#[test]
fn a_method_must_exist_on_its_receiver() {
    for (params, value, expected) in [
        (
            ", text: String",
            "text.frobnicate()",
            "no method `frobnicate` on String",
        ),
        (", n: Int", "n.min(1)", "no method `min` on Int"),
        (
            ", xs: List(String)",
            "xs.set(1, \"a\")",
            "no method `set` on List(String)",
        ),
        (
            ", m: Map(Int, String)",
            "m.push(\"a\")",
            "no method `push` on Map(Int, String)",
        ),
    ] {
        let message = err(&emitting(params, "name", value));
        assert!(
            message.starts_with(expected),
            "for `{value}`, got: {message}"
        );
    }
}

/// The pair this sees most is one confusion from either side, so each names the other.
/// It is exactly the edit a real port had to make by hand in eight files.
#[test]
fn the_emptiness_and_presence_questions_name_each_other() {
    let message = err(&emitting(", text: String?", "name", "text.is_empty()"));
    assert!(
        message.contains("an optional is asked `is_none()`"),
        "got: {message}"
    );
    let message = err(&emitting(", text: String", "name", "text.is_none()"));
    assert!(
        message.contains("a String is always there"),
        "got: {message}"
    );
    let message = err(&emitting(", text: String", "name", "text.unwrap_or(\"\")"));
    assert!(
        message.contains("is already there, so there is nothing to fall back to"),
        "got: {message}"
    );
}

/// `docs/functions.md` argues that calendar arithmetic is one opinion among several and
/// belongs in a `fn`. That decision is only visible if reaching for it says so, and a
/// real port reached for it.
#[test]
fn calendar_arithmetic_says_why_it_is_absent() {
    let message = err(&emitting(
        ", at_in: Timestamp",
        "at",
        "at_in.add_months(12)",
    ));
    assert!(
        message.contains("no method `add_months` on Timestamp"),
        "got: {message}"
    );
    assert!(
        message.contains("month-end clamping is one opinion among several"),
        "got: {message}"
    );
}

/// And the decision has somewhere to send an author, which is what makes it a decision
/// rather than a hole with a rationale attached. A moment comes apart into its calendar
/// fields and goes back together from them.
#[test]
fn a_timestamp_comes_apart_and_goes_back_together() {
    for (method, expected) in [
        ("year", 2026),
        ("month", 3),
        ("day", 15),
        ("hour", 9),
        ("minute", 30),
        ("second", 45),
    ] {
        assert_eq!(
            fired(
                ", at_in: Timestamp",
                "id",
                &format!("at_in.{method}()"),
                vec![("at_in", Value::Timestamp(1_773_567_045_000_000))],
            ),
            Value::Int(expected),
            "for `{method}`"
        );
    }
    // Back again, to the same moment. Fallible, because six numbers are not always a
    // date, and that optional is where an author's clamping rule goes.
    assert_eq!(
        fired(
            "",
            "at",
            "Timestamp.from_parts(2026, 3, 15, 9, 30, 45).unwrap_or(\"2000-01-01T00:00:00Z\")",
            Vec::new(),
        ),
        Value::Timestamp(1_773_567_045_000_000)
    );
    assert_eq!(
        fired(
            "",
            "id",
            "if Timestamp.from_parts(2026, 2, 30, 0, 0, 0).is_none() { 1 } else { 0 }",
            Vec::new(),
        ),
        Value::Int(1),
        "February has no thirtieth"
    );
}

/// A moment orders, because an application that works with them asks "before" and
/// "after" constantly, and nothing could answer either until now.
#[test]
fn two_moments_compare() {
    assert_eq!(
        fired(
            ", a: Timestamp, b: Timestamp",
            "id",
            "if a < b { 1 } else { 0 }",
            vec![("a", Value::Timestamp(1)), ("b", Value::Timestamp(2))],
        ),
        Value::Int(1)
    );
}

/// Arity is the table's too, and the argument types checked themselves on the way in
/// through the hint the same table gave them.
#[test]
fn a_methods_arity_and_arguments_are_checked() {
    let message = err(&emitting(", text: String", "name", "text.trim(1)"));
    assert_eq!(message, "`trim` takes 0 arguments, and this gives 1");

    let message = err(&emitting(", m: Map(Int, String)", "name", "m.get()"));
    assert_eq!(message, "`get` takes 1 argument, and this gives 0");

    // The key's type comes from the map, so a wrong one is a type error rather than an
    // arity one.
    let message = err(&emitting(", m: Map(Int, String)", "name", "m.get(\"one\")"));
    assert_eq!(message, "expected Int, found String");
}

// ---------------------------------------------------------------------------------
// Rule 4: what is deliberately not checked.

/// `Json` is opaque on purpose: its shape came from outside and is not a promise the
/// language can keep, so an accessor answers `none` rather than the checker answering
/// anything.
#[test]
fn a_jsons_shape_is_not_checked() {
    program(&emitting(
        ", blob_in: Json",
        "name",
        "blob_in.string(\"anything at all\").unwrap_or(\"\")",
    ));
}

/// Section 3's list of positions names both operands of `&&`, `||` and `!`, and until now
/// it was the one entry the code did not honour. `Bool` was threaded down as an inference
/// hint, and nothing below `expr` ever compared anything to it, so `if ok && id` passed.
#[test]
fn both_operands_of_a_boolean_operator_must_be_bool() {
    for (params, condition, expected) in [
        (", id: Int", "ok && id", "expected Bool, found Int"),
        (", id: Int", "id && ok", "expected Bool, found Int"),
        (", id: Int", "ok || id", "expected Bool, found Int"),
        (", id: Int", "id || ok", "expected Bool, found Int"),
        (
            ", text: String",
            "ok && text",
            "expected Bool, found String",
        ),
        (", id: Int", "!id", "expected Bool, found Int"),
        // The `!` was reached only because synthesis passed its operand's type through,
        // so it went quiet the moment it sat inside a `&&`. Both are closed here.
        (", id: Int", "ok && !id", "expected Bool, found Int"),
        (", id: Int", "ok && id && ok", "expected Bool, found Int"),
    ] {
        let body = format!(
            "command C(ok: Bool{params}) {{\n  if {condition} {{\n    return\n  }}\n  emit @thing.touched {{ id: 1 }}\n}}"
        );
        let message = err(&body);
        assert!(
            message.starts_with(expected),
            "`{condition}`: expected {expected:?}, got {message:?}"
        );
    }
}

/// And the other half, because a check is worth what it rejects and worth nothing if it
/// rejects a correct program. Every one of these is a boolean the checker has to see as
/// one, including the two that only synthesis can answer for: a comparison and a method.
#[test]
fn a_boolean_operand_that_is_one_is_accepted() {
    for (params, condition) in [
        (", b: Bool", "ok && b"),
        (", id: Int", "ok && id == 1"),
        (", id: Int", "id > 1 || id < 0"),
        (", text: String", "ok && text.is_empty()"),
        (", maybe: String?", "ok && maybe.is_some()"),
        (", b: Bool", "ok && b || !ok"),
        (", b: Bool, c: Bool", "ok && b && c && !ok"),
        (", b: Bool", "(ok || b) && !b"),
    ] {
        let body = format!(
            "command C(ok: Bool{params}) {{\n  if {condition} {{\n    return\n  }}\n  emit @thing.touched {{ id: 1 }}\n}}"
        );
        program(&body);
    }
}

/// Section 2's synthesis table: `!x` is a `Bool` whatever it was handed. It used to report
/// the operand's type, which is what made `if !id` fail with "found Int" for the wrong
/// reason, and it would have started passing the moment the operand was checked properly.
#[test]
fn a_negation_synthesises_bool() {
    let body = "command C(b: Bool) {\n  emit @thing.happened { id: !b, name: \"n\", maybe: none, at: \"2026-01-01T00:00:00Z\", total: 0, rate: 0, status: Draft, tags: [], facts: Facts { note: \"n\", count: 0 }, blob: Json.empty }\n}";
    assert_eq!(err(body), "expected Int, found Bool");
}
