//! `docs/declarations.md` as executable tests, for the three module-scope declarations
//! this pass added: `record`, `enum` and `const`.

use heklang::{Event, Interpreter, Key, Outcome, Store, Type, Value, parse};

const PRELUDE: &str = "enum Applicability { @default AllProducts, SpecificProducts }

record Coverage {
  kind: Applicability,
  product_ids: List(Int),
}

const NAMESPACE: Uuid = \"6ba7b810-9dad-11d1-80b4-00c04fd430c8\"
const FREE_TIER: Int = 15
const HOUSE: Coverage = Coverage { kind: AllProducts, product_ids: [] }

event @plan.created {
  plan_id: Uuid,
  covers: Coverage,
  status: Applicability,
  label: String,
}
";

const PLAN: &str = "0190d1a1-0000-7000-8000-000000000001";

fn source(params: &str, body: &str) -> String {
    format!("{PRELUDE}\ncommand Create(plan_id: Uuid{params}) {{\n{body}\n}}\n")
}

fn fired(params: &str, body: &str, args: Vec<(&str, Value)>) -> Event {
    let program =
        parse(&source(params, body)).unwrap_or_else(|err| panic!("expected this to parse: {err}"));
    let mut interpreter = Interpreter::new(&program);
    let mut all = vec![("plan_id", Value::uuid(PLAN))];
    all.extend(args);
    let execution = interpreter
        .run("Create", all)
        .unwrap_or_else(|err| panic!("expected this to run: {err}"));
    match execution.outcome {
        Outcome::Ok(events) => events.into_iter().next().expect("one event"),
        other => panic!("expected an append, got {other:?}"),
    }
}

fn err(params: &str, body: &str) -> String {
    parse(&source(params, body))
        .expect_err("expected this to be rejected")
        .message
}

/// An emit that fills every field, so a test only writes the part it cares about.
fn emitting(covers: &str, status: &str, label: &str) -> String {
    format!(
        "  emit @plan.created {{ plan_id, covers: {covers}, status: {status}, label: {label} }}"
    )
}

fn coverage(kind: &str, ids: Vec<i64>) -> Value {
    Value::record(
        "Coverage",
        [
            (
                "kind",
                Value::Enum {
                    ty: "Applicability".into(),
                    variant: kind.into(),
                },
            ),
            (
                "product_ids",
                Value::list(Type::Int, ids.into_iter().map(Value::Int)),
            ),
        ],
    )
}

// ---------------------------------------------------------------------------------
// Records.

#[test]
fn a_record_literal_builds_a_value() {
    let event = fired(
        "",
        &emitting(
            "Coverage { kind: SpecificProducts, product_ids: [7, 9] }",
            "AllProducts",
            "\"\"",
        ),
        vec![],
    );
    assert_eq!(
        event.field("covers"),
        Some(&coverage("SpecificProducts", vec![7, 9]))
    );
}

/// The same bare-name shorthand `emit` and `put` already use, so there is one rule for
/// naming a field rather than three.
#[test]
fn a_record_literal_takes_the_bare_name_shorthand() {
    let body = format!(
        "  let kind = SpecificProducts\n  let product_ids = [1]\n{}",
        emitting("Coverage { kind, product_ids }", "AllProducts", "\"\"")
    );
    let event = fired("", &body, vec![]);
    assert_eq!(
        event.field("covers"),
        Some(&coverage("SpecificProducts", vec![1]))
    );
}

#[test]
fn a_field_is_read_with_a_dot() {
    let body = emitting(
        "covers",
        "covers.kind",
        "\"{covers.product_ids.len()} products\"",
    );
    let event = fired(
        ", covers: Coverage",
        &body,
        vec![("covers", coverage("SpecificProducts", vec![4, 5, 6]))],
    );
    assert_eq!(
        event.field("status"),
        Some(&Value::Enum {
            ty: "Applicability".into(),
            variant: "SpecificProducts".into(),
        })
    );
    assert_eq!(event.field("label"), Some(&Value::str("3 products")));
}

#[test]
fn every_field_must_be_given() {
    assert_eq!(
        err(
            "",
            &emitting("Coverage { kind: AllProducts }", "AllProducts", "\"\"")
        ),
        "record `Coverage` needs `product_ids`"
    );
    assert_eq!(
        err(
            "",
            &emitting(
                "Coverage { kind: AllProducts, product_ids: [], nope: 1 }",
                "AllProducts",
                "\"\""
            )
        ),
        "record `Coverage` has no field `nope`"
    );
}

/// Rule 8's table, so a record can go straight into a request body or a message.
#[test]
fn a_record_serialises_as_a_json_object() {
    let body = emitting("covers", "AllProducts", "\"{covers}\"");
    let event = fired(
        ", covers: Coverage",
        &body,
        vec![("covers", coverage("SpecificProducts", vec![7]))],
    );
    assert_eq!(
        event.field("label"),
        Some(&Value::str(
            r#"{"kind":"SpecificProducts","product_ids":[7]}"#
        ))
    );
}

/// A record is an ordinary value, so it goes wherever a value goes. An entity column is
/// the one that needed a zero to be defined.
#[test]
fn a_record_can_be_a_container_element_and_an_entity_column() {
    let source = format!(
        "{PRELUDE}
command Create(plan_id: Uuid) {{
  emit @plan.created {{ plan_id, covers: HOUSE, status: AllProducts, label: \"\" }}
}}

projector Plans {{
  entity Plan {{
    plan_id: Uuid @key,
    covers: Coverage,
    seen: List(Int),
  }}

  on @plan.created {{ plan_id, covers }} {{
    patch Plan[plan_id] {{ covers }}
  }}
}}
"
    );
    let program = parse(&source).unwrap_or_else(|err| panic!("expected this to parse: {err}"));
    let mut interpreter = Interpreter::new(&program);
    interpreter
        .run("Create", vec![("plan_id", Value::uuid(PLAN))])
        .expect("ran");
    let store: Store = interpreter.project("Plans").expect("projected");
    let row = store
        .get("Plan", &Key::Uuid(PLAN.into()))
        .expect("the patch materialized a row");
    assert_eq!(row.field("covers"), Some(&coverage("AllProducts", vec![])));
    // A record column materializes from its fields' zeros, which is how every other
    // column already works rather than a case of its own.
    assert_eq!(row.field("seen"), Some(&Value::list(Type::Int, [])));
}

// ---------------------------------------------------------------------------------
// Module-scope enums.

/// The hole `docs/projectors.md` rule 7 named: with projector-scoped enums a bad
/// variant could not reach the read model, but could still reach the event.
#[test]
fn an_enum_is_the_same_type_on_the_event_and_the_column() {
    assert_eq!(
        err("", &emitting("HOUSE", "Archived", "\"\"")),
        "`Applicability` has no variant `Archived`"
    );

    let event = fired("", &emitting("HOUSE", "SpecificProducts", "\"\""), vec![]);
    assert_eq!(
        event.field("status"),
        Some(&Value::Enum {
            ty: "Applicability".into(),
            variant: "SpecificProducts".into(),
        })
    );
}

/// A projector's own enum shadows a module one, which is the precedence a local
/// binding already has over a builtin name.
#[test]
fn a_projector_enum_shadows_a_module_enum() {
    let source = format!(
        "{PRELUDE}
projector Plans {{
  enum Applicability {{ @default Nothing, Everything }}

  entity Plan {{
    plan_id: Uuid @key,
    shown: Applicability,
  }}

  on @plan.created {{ plan_id }} {{
    patch Plan[plan_id] {{ shown: Everything }}
  }}
}}
"
    );
    let program = parse(&source).unwrap_or_else(|err| panic!("expected this to parse: {err}"));
    let projector = program.projector("Plans").expect("declared");
    assert_eq!(projector.enums.len(), 1);
    assert_eq!(projector.enums[0].variants, ["Nothing", "Everything"]);
    // The module's is untouched, so the event still uses it.
    assert_eq!(program.enums.len(), 1);
    assert_eq!(
        program.enums[0].variants,
        ["AllProducts", "SpecificProducts"]
    );
}

/// Interpolation's enum row, which needed a module-scope enum to be reachable from a
/// command parameter at all.
#[test]
fn an_enum_interpolates_as_its_variant() {
    let event = fired(
        ", status: Applicability",
        &emitting("HOUSE", "status", "\"{status}\""),
        vec![(
            "status",
            Value::Enum {
                ty: "Applicability".into(),
                variant: "SpecificProducts".into(),
            },
        )],
    );
    assert_eq!(event.field("label"), Some(&Value::str("SpecificProducts")));
}

// ---------------------------------------------------------------------------------
// Constants.

#[test]
fn a_const_is_a_literal_wherever_it_is_named() {
    let body = emitting("HOUSE", "AllProducts", "\"under {FREE_TIER}\"");
    let event = fired("", &body, vec![]);
    assert_eq!(event.field("label"), Some(&Value::str("under 15")));
    assert_eq!(
        event.field("covers"),
        Some(&coverage("AllProducts", vec![]))
    );
}

/// heklang has no Uuid literal token, so a namespace constant was unspellable. The
/// target type is what makes this string one, and it is checked at parse time.
#[test]
fn a_string_literal_resolves_against_a_uuid_target() {
    let program = parse(&source("", &emitting("HOUSE", "AllProducts", "\"\""))).expect("parses");
    let def = program.constant("NAMESPACE").expect("declared");
    assert_eq!(def.ty, Type::Uuid);
    assert_eq!(
        heklang::value::literal(&def.value),
        Value::uuid("6ba7b810-9dad-11d1-80b4-00c04fd430c8")
    );

    let bad = parse("const N: Uuid = \"not-a-uuid\"\nevent @a.b { x: Int }\n")
        .expect_err("expected a rejection")
        .message;
    assert_eq!(bad, "`not-a-uuid` is not a Uuid");
}

#[test]
fn a_const_takes_literals_and_literal_aggregates_only() {
    let message = parse("const N: Int = 1 + 1\nevent @a.b { x: Int }\n")
        .expect_err("expected a rejection")
        .message;
    assert_eq!(
        message,
        "expected `enum`, `record`, `const`, `fn`, `event`, `command`, `projector` or `effect`, found `+`",
        "the literal ends the declaration, so what follows is read as the next item"
    );

    let message = parse("const N: Int = \"one\"\nevent @a.b { x: Int }\n")
        .expect_err("expected a rejection")
        .message;
    assert_eq!(message, "a Int const cannot be a String");
}

// ---------------------------------------------------------------------------------
// Declaration order and duplicates.

/// The passes exist so that a record field may name an enum, an event field may name a
/// record, and neither has to be declared first.
#[test]
fn declaration_order_does_not_matter() {
    let backwards = "event @plan.created { plan_id: Uuid, covers: Coverage }

record Coverage { kind: Applicability, product_ids: List(Int) }

enum Applicability { @default AllProducts, SpecificProducts }
";
    let program = parse(backwards).expect("four passes make order irrelevant");
    assert_eq!(program.records.len(), 1);
    assert_eq!(
        program.events[0].field("covers").map(|field| &field.ty),
        Some(&Type::Record("Coverage".into()))
    );
}

/// Records may reference each other, which is why pass A takes the names and pass B the
/// fields.
#[test]
fn records_may_reference_one_another() {
    let source = "record Outer { inner: Inner, tail: List(Inner) }
record Inner { n: Int }
event @a.b { x: Outer }
";
    let program = parse(source).expect("names come before fields");
    let outer = program.record("Outer").expect("declared");
    assert_eq!(outer.fields[0].ty, Type::Record("Inner".into()));
}

#[test]
fn duplicate_declarations_are_rejected() {
    for (kind, item) in [
        ("record", "record Dup { n: Int }"),
        ("enum", "enum Dup { @default A, B }"),
        ("const", "const Dup: Int = 1"),
    ] {
        let source = format!("{item}\n{item}\nevent @a.b {{ x: Int }}\n");
        assert_eq!(
            parse(&source).expect_err("declared twice").message,
            format!("{kind} `Dup` is declared twice")
        );
    }
}

#[test]
fn a_record_needs_at_least_one_field() {
    let message = parse("record Empty { }\nevent @a.b { x: Int }\n")
        .expect_err("expected a rejection")
        .message;
    assert_eq!(message, "record `Empty` declares no fields");
}

// ---------------------------------------------------------------------------------
// The `{` ambiguity, and what is out.

/// A record literal is claimed only where a `{` cannot be a block, so an `if` or `for`
/// header whose expression ends in a bare record name still reads as a header.
#[test]
fn a_record_name_in_a_header_is_not_a_literal() {
    let source = "record Flag { n: Int }
event @a.b { x: Int }

command C(x: Int) {
  if x > 0 {
    return
  }
  for i in [1] {
    return
  }
  emit @a.b { x }
}
";
    parse(source).expect("a header's block is a block");

    // Inside parentheses the restriction lifts again, because there the `{` cannot be
    // the header's block.
    let nested = "record Flag { n: Int }
event @a.b { x: Int }

command C(x: Int) {
  if (Flag { n: x }).n > 0 {
    return
  }
  emit @a.b { x }
}
";
    parse(nested).expect("a record literal inside a call in a header still works");
}

/// Record update is deliberately absent: folding one map per aspect is what a real
/// port found reads better and is structurally safer.
#[test]
fn there_is_no_record_update() {
    let body = format!(
        "  let base = HOUSE\n{}",
        emitting(
            "base with { kind: SpecificProducts }",
            "AllProducts",
            "\"\""
        )
    );
    let message = err("", &body);
    assert_eq!(message, "expected `}`, found `with`");
}

/// A `const` is the one item with no braced body, so skipping one in an earlier pass
/// cannot look for a brace: it would run past the const into the next declaration and
/// swallow it whole.
#[test]
fn a_const_does_not_swallow_the_declaration_after_it() {
    let source = "const LIMIT: Int = 2
const TAGS: List(String) = [\"a\", \"b\"]

fn doubled(n: Int) -> Int {
  return n * 2
}

event @a.b { x: Int }

command C(x: Int) {
  emit @a.b { x: doubled(x) + LIMIT }
}
";
    let program = parse(source).expect("a const ends at its literal");
    assert_eq!(
        program.functions.len(),
        1,
        "the fn after the consts survived"
    );
    assert_eq!(program.consts.len(), 2);
}
