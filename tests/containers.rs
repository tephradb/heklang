//! `docs/containers.md` as executable tests: `List`, `Map`, `for`, comprehensions, and
//! the two things deliberately absent (mutable bindings, insertion order).

use heklang::{Event, Interpreter, Key, Outcome, Type, Value, parse};

const PRELUDE: &str = "event @item.added {
  basket_id: Uuid,
  item: Int,
  tag: String,
}

event @basket.filled {
  basket_id: Uuid,
  items: List(Int),
  label: String,
}
";

const BASKET: &str = "0190d1a1-0000-7000-8000-000000000001";

fn source(params: &str, body: &str) -> String {
    format!("{PRELUDE}\ncommand Fill(basket_id: Uuid{params}) {{\n{body}\n}}\n")
}

/// Runs a command over a fixed log and returns the `@basket.filled` it appended.
fn fired(params: &str, body: &str, args: Vec<(&str, Value)>) -> Event {
    let program =
        parse(&source(params, body)).unwrap_or_else(|err| panic!("expected this to parse: {err}"));
    let mut interpreter = Interpreter::with_log(&program, added());
    let mut all = vec![("basket_id", Value::uuid(BASKET))];
    all.extend(args);
    let execution = interpreter
        .run("Fill", all)
        .unwrap_or_else(|err| panic!("expected this to run: {err}"));
    match execution.outcome {
        Outcome::Ok(events) => events.into_iter().next().expect("one event"),
        other => panic!("expected an append, got {other:?}"),
    }
}

fn added() -> Vec<Event> {
    [(30, "c"), (10, "a"), (20, "b")]
        .into_iter()
        .map(|(item, tag)| {
            Event::new(
                heklang::EventPath::new(["item", "added"]),
                [
                    ("basket_id", Value::uuid(BASKET)),
                    ("item", Value::Int(item)),
                    ("tag", Value::str(tag)),
                ],
            )
        })
        .collect()
}

fn items(event: &Event) -> Vec<i64> {
    match event.field("items") {
        Some(Value::List { items, .. }) => items
            .iter()
            .map(|value| match value {
                Value::Int(value) => *value,
                other => panic!("expected an Int, got {other:?}"),
            })
            .collect(),
        other => panic!("expected a list, got {other:?}"),
    }
}

fn label(event: &Event) -> String {
    match event.field("label") {
        Some(Value::Str(text)) => text.to_string(),
        other => panic!("expected a string, got {other:?}"),
    }
}

fn err(params: &str, body: &str) -> String {
    parse(&source(params, body))
        .expect_err("expected this to be rejected")
        .text()
}

/// A body that emits `items` and a fixed label, so a test only writes the expression
/// it cares about.
fn emitting(expr: &str) -> String {
    format!("  emit @basket.filled {{ basket_id, items: {expr}, label: \"\" }}")
}

fn describing(expr: &str) -> String {
    format!("  emit @basket.filled {{ basket_id, items: no_items, label: {expr} }}")
}

/// A `List(Int)` parameter that is always empty, so `describing` has a target type for
/// its `items` field without every test restating one.
const NO_ITEMS: &str = ", no_items: List(Int)";

fn empty_list() -> (&'static str, Value) {
    ("no_items", Value::list(Type::Int, []))
}

fn letters() -> Value {
    Value::map(
        Type::Int,
        Type::String,
        [
            (Key::Int(30), Value::str("c")),
            (Key::Int(10), Value::str("a")),
            (Key::Int(20), Value::str("b")),
        ],
    )
}

// ---------------------------------------------------------------------------------
// List.

#[test]
fn a_list_literal_holds_its_elements() {
    let event = fired("", &emitting("[3, 1, 2]"), vec![]);
    assert_eq!(items(&event), [3, 1, 2], "order is the order written");
    assert_eq!(
        event.field("items"),
        Some(&Value::list(
            Type::Int,
            [Value::Int(3), Value::Int(1), Value::Int(2)]
        ))
    );
}

#[test]
fn list_methods_read_without_changing() {
    let body = format!(
        "  let xs = [3, 1, 2]\n{}",
        describing(r#""{xs.len()} {xs.is_empty()} {xs.contains(1)} {xs.first()}""#)
    );
    assert_eq!(
        label(&fired(NO_ITEMS, &body, vec![empty_list()])),
        "3 false true 3"
    );

    // `first()` on an empty list is `none`, not a trap, which is why there is no
    // indexing to ask the out-of-bounds question about.
    let empty = describing(r#""{no_items.len()} {no_items.is_empty()} {no_items.first()}""#);
    assert_eq!(
        label(&fired(NO_ITEMS, &empty, vec![empty_list()])),
        "0 true null"
    );
}

/// The property a fold arm depends on: nothing mutates, so the old value is still the
/// old value after a `push`.
#[test]
fn push_and_remove_return_new_lists() {
    let body = format!(
        "  let xs = [1, 2]\n  let ys = xs.push(3)\n{}",
        describing(r#""{xs.len()} {ys.len()}""#)
    );
    assert_eq!(label(&fired(NO_ITEMS, &body, vec![empty_list()])), "2 3");

    let event = fired("", &emitting("[1, 2].push(3)"), vec![]);
    assert_eq!(items(&event), [1, 2, 3]);
}

/// Every equal element, not the first, which is what makes running a fold arm twice
/// the same as running it once.
#[test]
fn remove_drops_every_equal_element() {
    let event = fired("", &emitting("[1, 2, 1, 3].remove(1)"), vec![]);
    assert_eq!(items(&event), [2, 3]);
    let twice = fired("", &emitting("[1, 2, 1].remove(1).remove(1)"), vec![]);
    assert_eq!(items(&twice), [2], "removing again changes nothing");
}

// ---------------------------------------------------------------------------------
// Map.

#[test]
fn a_map_is_built_with_set_and_read_with_get() {
    let body = describing(r#""{m.len()} {m.get(10)} {m.get(9)} {m.contains(20)}""#);
    assert_eq!(
        label(&fired(
            &format!("{NO_ITEMS}, m: Map(Int, String)"),
            &body,
            vec![empty_list(), ("m", letters())],
        )),
        "3 a null true"
    );
}

/// Sorted, not insertion order. This is what lets verify mode claim that the same
/// object built twice serialises identically.
#[test]
fn map_iteration_is_sorted_by_key() {
    let body = describing(r#""{m.keys()} {m.values()} {m}""#);
    assert_eq!(
        label(&fired(
            &format!("{NO_ITEMS}, m: Map(Int, String)"),
            &body,
            vec![empty_list(), ("m", letters())],
        )),
        r#"[10,20,30] ["a","b","c"] {"10":"a","20":"b","30":"c"}"#,
        "insertion was 30, 10, 20"
    );
}

#[test]
fn a_map_key_must_be_a_type_that_orders() {
    let message = err(", m: Map(Money(2), Int)", &emitting("[]"));
    assert_eq!(
        message,
        "a Money(2) cannot be a map key, for the reason it cannot be an entity key: it does not order"
    );
}

#[test]
fn map_empty_needs_a_target_type() {
    let message = err("", "  let m = Map.empty\n  return");
    assert!(
        message.starts_with("`Map.empty` needs a target type"),
        "got: {message}"
    );
    let message = err("", "  let m = Map.nope\n  return");
    assert_eq!(
        message,
        "`Map` has no `nope`; it has `empty`, and everything else is a method on a map value"
    );
}

#[test]
fn an_empty_list_needs_a_target_type() {
    let message = err("", "  let xs = []\n  return");
    assert!(
        message.starts_with("an empty list needs a target type"),
        "got: {message}"
    );
}

/// A `fn` returning a `Json`, which is an object literal without needing an effect.
fn returning_json(value: &str) -> String {
    format!("{PRELUDE}\nfn payload(item: Int) -> Json {{\n  return {value}\n}}\n")
}

/// Inside an object literal there is no target and none is needed: a body's values are
/// typed by what they are rather than by where they land. The rule holds at any depth
/// and wherever an object literal is legal, so a `Json.encode` argument is on the list
/// too. Only the outermost brace used to have a target, which made `{ "tags": [] }` an
/// error about declarations a body has none of.
#[test]
fn an_empty_list_in_a_body_needs_no_target_type() {
    for value in [
        "{ \"tags\": [] }",
        "{ \"meta\": { \"tags\": [] }, \"ids\": [item] }",
        "{ \"encoded\": Json.encode({ \"deep\": { \"tags\": [] } }) }",
    ] {
        parse(&returning_json(value)).unwrap_or_else(|err| panic!("for `{value}`: {err}"));
    }
}

/// `Map.empty` is not on that list, because a JSON object is written `{ ... }`: a map
/// never reaches a body without a declared type to have come from.
#[test]
fn an_empty_map_in_a_body_still_needs_a_target_type() {
    let message = parse(&returning_json("{ \"m\": Map.empty }"))
        .expect_err("a map has no place in a body")
        .text();
    assert!(
        message.starts_with("`Map.empty` needs a target type"),
        "got: {message}"
    );
}

// ---------------------------------------------------------------------------------
// `for`.

#[test]
fn for_binds_one_name_over_a_list() {
    let body = format!(
        "  let xs = [1, 2, 3]\n  for x in xs {{\n    if x == 2 {{\n      return invalid(\"found two\")\n    }}\n  }}\n{}",
        emitting("[]")
    );
    let program = parse(&source("", &body)).expect("parses");
    let mut interpreter = Interpreter::new(&program);
    let execution = interpreter
        .run("Fill", vec![("basket_id", Value::uuid(BASKET))])
        .expect("ran");
    assert_eq!(execution.outcome, Outcome::Invalid("found two".into()));
}

#[test]
fn for_binds_an_index_and_an_item_over_a_list() {
    let body =
        "  for i, x in [10, 20] {\n    if i == 1 {\n      return invalid(\"{i}:{x}\")\n    }\n  }\n"
            .to_string() + &emitting("[]");
    let program = parse(&source("", &body)).expect("parses");
    let mut interpreter = Interpreter::new(&program);
    let execution = interpreter
        .run("Fill", vec![("basket_id", Value::uuid(BASKET))])
        .expect("ran");
    assert_eq!(execution.outcome, Outcome::Invalid("1:20".into()));
}

#[test]
fn for_binds_a_key_and_a_value_over_a_map() {
    let body =
        "  for key, value in m {\n    if key == 10 {\n      return invalid(value)\n    }\n  }\n"
            .to_string()
            + &emitting("[]");
    let program = parse(&source(", m: Map(Int, String)", &body)).expect("parses");
    let mut interpreter = Interpreter::new(&program);
    let execution = interpreter
        .run(
            "Fill",
            vec![("basket_id", Value::uuid(BASKET)), ("m", letters())],
        )
        .expect("ran");
    assert_eq!(execution.outcome, Outcome::Invalid("a".into()));
}

/// There is no pair type, so "the element of a map" is not a thing to bind.
#[test]
fn one_name_over_a_map_says_to_write_two() {
    let body = format!("  for entry in m {{\n    return\n  }}\n{}", emitting("[]"));
    assert_eq!(
        err(", m: Map(Int, String)", &body),
        "a map yields a key beside its value, so `for` over one binds two names; write `for key, value in ...`"
    );
}

#[test]
fn for_over_a_scalar_is_rejected() {
    let body = format!(
        "  for x in basket_id {{\n    return\n  }}\n{}",
        emitting("[]")
    );
    assert_eq!(
        err("", &body),
        "`for` walks a List or a Map, and this is a Uuid"
    );
}

// ---------------------------------------------------------------------------------
// Comprehensions.

#[test]
fn a_comprehension_maps_and_filters() {
    let event = fired(
        "",
        &emitting("[x * 2 for x in [1, 2, 3] if x != 2]"),
        vec![],
    );
    assert_eq!(items(&event), [2, 6]);
}

#[test]
fn a_comprehension_over_a_map_binds_both_names() {
    let body = describing(r#""{[value for key, value in m if key > 10]}""#);
    assert_eq!(
        label(&fired(
            &format!("{NO_ITEMS}, m: Map(Int, String)"),
            &body,
            vec![empty_list(), ("m", letters())],
        )),
        r#"["b","c"]"#
    );
}

/// The element type comes from the target, so a comprehension that matches nothing is
/// still a list of the right thing rather than a list of nothing in particular.
#[test]
fn an_empty_comprehension_keeps_its_declared_element_type() {
    let event = fired("", &emitting("[x for x in [1, 2] if x > 9]"), vec![]);
    assert_eq!(items(&event), [] as [i64; 0]);
    assert_eq!(event.field("items"), Some(&Value::list(Type::Int, [])));
}

// ---------------------------------------------------------------------------------
// A fold arm accumulating a container, which is the shape the port needed.

#[test]
fn a_fold_arm_accumulates_a_container() {
    let body = "  state seen: List(Int) = fold []\n    on @item.added(basket_id) { item } => seen.push(item)\n\n"
        .to_string()
        + &emitting("seen");
    let event = fired("", &body, vec![]);
    assert_eq!(items(&event), [30, 10, 20], "log order, not sorted");

    let map_body = "  state tags: Map(Int, String) = fold Map.empty\n    on @item.added(basket_id) { item, tag } => tags.set(item, tag)\n\n"
        .to_string()
        + &describing(r#""{tags}""#);
    assert_eq!(
        label(&fired(NO_ITEMS, &map_body, vec![empty_list()])),
        r#"{"10":"a","20":"b","30":"c"}"#,
        "the map sorts what the log delivered out of order"
    );
}

// ---------------------------------------------------------------------------------
// What is absent.

/// Recorded as tested rather than assumed: 3,186 lines of ported application code
/// needed no mutable binding, so there is no `var`.
#[test]
fn there_are_no_mutable_bindings() {
    let body = format!("  var total = 0\n{}", emitting("[]"));
    assert_eq!(err("", &body), "expected a statement, found `var`");

    // What replaces it: a comprehension for an accumulation, and a `for` with an early
    // `return` for a search. Both are already covered above; this pins that `var` is
    // absent rather than merely unused.
}

#[test]
fn there_is_no_break_or_continue() {
    for word in ["break", "continue"] {
        let body = format!("  for x in [1] {{\n    {word}\n  }}\n{}", emitting("[]"));
        assert_eq!(
            err("", &body),
            format!("expected a statement, found `{word}`")
        );
    }
}

/// A declared element type coerces, the same rule every other declared position
/// applies. All four ways an element is produced, because each is its own site and one
/// holding the rule says nothing about the others.
///
/// The probe is `.first().unwrap_or(none)`, which is what a `List(T?)` reads back
/// through: two levels, so a bare element collapses them into one and `.is_some()`
/// arrives at a `String` that has no such method. That is the shape of the bug, several
/// statements from the write that caused it.
#[test]
fn a_bare_value_fills_an_optional_element() {
    let source = "event @a.b { listed: Bool, built: Bool, pushed: Bool, mapped: Bool }

fn listed(tag: String) -> List(String?) {
  return [tag]
}

fn built(tag: String, seed: List(String?)) -> List(String?) {
  return [tag for x in seed]
}

command C(tag: String, seed: List(String?), spare: List(String?), table: Map(Int, String?)) {
  emit @a.b {
    listed: listed(tag).first().unwrap_or(none).is_some(),
    built: built(tag, seed).first().unwrap_or(none).is_some(),
    pushed: spare.push(tag).first().unwrap_or(none).is_some(),
    mapped: table.set(1, tag).get(1).unwrap_or(none).is_some(),
  }
}
";
    let program = parse(source).expect("a bare String fills a String? element");
    let mut interpreter = Interpreter::new(&program);
    let execution = interpreter
        .run(
            "C",
            vec![
                ("tag", Value::str("a")),
                (
                    "seed",
                    Value::list(Type::opt(Type::String), [Value::none(Type::String)]),
                ),
                ("spare", Value::list(Type::opt(Type::String), [])),
                ("table", Value::map(Type::Int, Type::opt(Type::String), [])),
            ],
        )
        .unwrap_or_else(|err| panic!("{err}"));

    let event = match execution.outcome {
        Outcome::Ok(events) => events.into_iter().next().expect("one event"),
        other => panic!("expected an append, got {other:?}"),
    };
    for field in ["listed", "built", "pushed", "mapped"] {
        assert_eq!(
            event.fields.get(field),
            Some(&Value::Bool(true)),
            "`{field}` held a bare element rather than a wrapped one"
        );
    }
}
