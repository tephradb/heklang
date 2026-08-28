//! `docs/functions.md` as executable tests: what a `fn` is, what purity buys, and the
//! two static checks that make a call terminate and produce a value.

use heklang::{Event, Interpreter, Outcome, Value, parse};

const PRELUDE: &str = "const PREFIX: String = \"SKU:\"

event @plan.created {
  plan_id: Uuid,
  sku: String,
  months: Int,
}

fn effective_sku(sku: String?, plan_id: Uuid) -> String {
  let given = sku.unwrap_or(\"\").trim()
  if given.is_empty() {
    return \"{PREFIX}{plan_id}\"
  }
  return given
}
";

const PLAN: &str = "0190d1a1-0000-7000-8000-000000000001";

fn source(decls: &str, body: &str) -> String {
    format!(
        "{PRELUDE}{decls}\ncommand Create(plan_id: Uuid, sku: String?, months: Int) {{\n{body}\n}}\n"
    )
}

fn fired(decls: &str, body: &str, sku: Option<&str>) -> Event {
    let program =
        parse(&source(decls, body)).unwrap_or_else(|err| panic!("expected this to parse: {err}"));
    let mut interpreter = Interpreter::new(&program);
    let sku = match sku {
        Some(text) => Value::some(Value::str(text)),
        None => Value::none(heklang::Type::String),
    };
    let execution = interpreter
        .run(
            "Create",
            vec![
                ("plan_id", Value::uuid(PLAN)),
                ("sku", sku),
                ("months", Value::Int(24)),
            ],
        )
        .unwrap_or_else(|err| panic!("expected this to run: {err}"));
    match execution.outcome {
        Outcome::Ok(events) => events.into_iter().next().expect("one event"),
        other => panic!("expected an append, got {other:?}"),
    }
}

fn err(decls: &str, body: &str) -> String {
    parse(&source(decls, body))
        .expect_err("expected this to be rejected")
        .message
}

const EMIT: &str = "  emit @plan.created { plan_id, sku: effective_sku(sku, plan_id), months }";

// ---------------------------------------------------------------------------------
// The shape.

/// The helper that motivated the whole item: one rule about SKUs, spelled once and
/// used from six places, where copying it is where the bug lives.
#[test]
fn a_fn_is_callable_and_returns_its_declared_type() {
    let given = fired("", EMIT, Some("MINE-1"));
    assert_eq!(given.field("sku"), Some(&Value::str("MINE-1")));

    let derived = fired("", EMIT, None);
    assert_eq!(
        derived.field("sku"),
        Some(&Value::str(format!("SKU:{PLAN}")))
    );

    // A blank one falls back too, which is the branch a copy would get wrong.
    let blank = fired("", EMIT, Some("   "));
    assert_eq!(blank.field("sku"), Some(&Value::str(format!("SKU:{PLAN}"))));
}

/// Signatures are collected before any body, so a call may name a `fn` declared below
/// it or in another module.
#[test]
fn a_fn_may_be_declared_after_its_caller() {
    let source = "event @a.b { x: Int }

command C(x: Int) {
  emit @a.b { x: double(x) }
}

fn double(n: Int) -> Int {
  return n * 2
}
";
    let program = parse(source).expect("signatures come before bodies");
    assert_eq!(program.functions.len(), 1);
}

#[test]
fn a_call_is_checked_against_the_declared_parameters() {
    assert_eq!(
        err(
            "",
            "  emit @plan.created { plan_id, sku: effective_sku(sku), months }"
        ),
        "`effective_sku` needs `plan_id`"
    );
    assert_eq!(
        err(
            "",
            "  emit @plan.created { plan_id, sku: effective_sku(sku, plan_id, 1), months }"
        ),
        "`effective_sku` takes 2 arguments"
    );
    // A parameter's type is the argument's hint, so inference works through a call the
    // way it works through `emit`.
    assert_eq!(
        err(
            "",
            "  emit @plan.created { plan_id, sku: effective_sku(sku, 1), months }"
        ),
        "a number cannot be a Uuid"
    );
}

/// A local wins, the way it already does over a builtin name.
#[test]
fn a_local_shadows_a_fn_name() {
    let decls = "\nfn label(n: Int) -> String {\n  return \"n={n}\"\n}\n";
    let body = "  let label = \"shadowed\"\n  emit @plan.created { plan_id, sku: label, months }";
    let event = fired(decls, body, None);
    assert_eq!(event.field("sku"), Some(&Value::str("shadowed")));
}

// ---------------------------------------------------------------------------------
// Purity.

/// The gating, one row per thing a `fn` cannot do, and the message says which rule the
/// restriction keeps rather than calling it a style.
#[test]
fn a_fn_is_pure() {
    let cases = [
        ("now()", "read a clock"),
        ("http.get(\"https://x\")", "call out"),
        (
            "invoke C { plan_id: p, sku: none, months: 1 }",
            "call a command",
        ),
        ("reveal(p)", "decrypt"),
    ];
    for (call, what) in cases {
        let decls =
            format!("\nfn helper(p: Uuid) -> String {{\n  let x = {call}\n  return \"\"\n}}\n");
        let message = err(&decls, EMIT);
        assert_eq!(
            message,
            format!(
                "a `fn` is pure, so it cannot {what}; that is what keeps the erase-last check inside one arm instead of following calls"
            ),
            "for {call}"
        );
    }

    for (stmt, what) in [
        (
            "emit @plan.created { plan_id: p, sku: \"\", months: 1 }",
            "append events",
        ),
        ("erase(p)", "erase a subject key"),
        ("log(\"x\")", "log"),
        ("fail(\"x\")", "fail"),
    ] {
        let decls = format!("\nfn helper(p: Uuid) -> String {{\n  {stmt}\n  return \"\"\n}}\n");
        assert!(
            err(&decls, EMIT).starts_with(&format!("a `fn` is pure, so it cannot {what}")),
            "for {stmt}: {}",
            err(&decls, EMIT)
        );
    }
}

#[test]
fn a_fn_has_no_state() {
    let decls =
        "\nfn helper(p: Uuid) -> String {\n  state seen: Bool = fold false\n  return \"\"\n}\n";
    assert_eq!(
        err(decls, EMIT),
        "a `fn` has no `state`; it is a pure function of its arguments"
    );
}

#[test]
fn a_fn_returns_a_value_not_an_outcome() {
    let decls = "\nfn helper(p: Uuid) -> String {\n  return invalid(\"no\")\n}\n";
    assert_eq!(
        err(decls, EMIT),
        "`invalid` is a command's outcome; a `fn` returns a value, so a caller decides what a bad one means"
    );
}

/// Purity is what makes this need no rule of its own: a fold must reproduce without a
/// journal, and a pure helper cannot break that.
#[test]
fn a_fold_arm_and_a_projector_may_call_a_fn() {
    let source = "event @plan.created { plan_id: Uuid, months: Int }

fn years(months: Int) -> Int {
  return months / 12
}

command C(plan_id: Uuid, months: Int) {
  state total: Int = fold 0
    on @plan.created(plan_id) { months } => total + years(months)

  emit @plan.created { plan_id, months }
}

projector Plans {
  entity Plan { plan_id: Uuid @key, years: Int }

  on @plan.created { plan_id, months } {
    put Plan { plan_id, years: years(months) }
  }
}
";
    let program = parse(source).expect("a pure helper is legal in a fold and a projector");
    let mut interpreter = Interpreter::new(&program);
    interpreter
        .run(
            "C",
            vec![("plan_id", Value::uuid(PLAN)), ("months", Value::Int(36))],
        )
        .expect("ran");
    let store = interpreter.project("Plans").expect("projected");
    let row = store
        .get("Plan", &heklang::Key::Uuid(PLAN.into()))
        .expect("a row");
    assert_eq!(row.field("years"), Some(&Value::Int(3)));
}

// ---------------------------------------------------------------------------------
// Recursion.

#[test]
fn a_fn_cannot_call_itself() {
    let decls =
        "\nfn down(n: Int) -> Int {\n  if n == 0 {\n    return 0\n  }\n  return down(n - 1)\n}\n";
    assert_eq!(
        err(decls, EMIT),
        "`down` calls `down`: a `fn` cannot call itself, directly or through another, so that every call ends"
    );
}

/// Indirect recursion too, and the message names the cycle as a path rather than just
/// saying one exists.
#[test]
fn the_recursion_error_names_the_cycle() {
    let decls = "
fn a(n: Int) -> Int {
  return b(n)
}

fn b(n: Int) -> Int {
  return c(n)
}

fn c(n: Int) -> Int {
  return a(n)
}
";
    let message = err(decls, EMIT);
    assert!(
        message.starts_with("`a` calls `b` calls `c` calls `a`:"),
        "got: {message}"
    );
}

/// A shared helper called from two places is not a cycle, which is the case a naive
/// visited-set check gets wrong.
#[test]
fn a_diamond_is_not_a_cycle() {
    let source = "event @a.b { x: Int }

fn leaf(n: Int) -> Int { return n + 1 }
fn left(n: Int) -> Int { return leaf(n) }
fn right(n: Int) -> Int { return leaf(n) }

command C(x: Int) {
  emit @a.b { x: left(x) + right(x) }
}
";
    parse(source).expect("two paths to one helper is a diamond, not a cycle");
}

// ---------------------------------------------------------------------------------
// Every path returns.

#[test]
fn a_fn_must_return_on_every_path() {
    let decls = "\nfn pick(n: Int) -> Int {\n  if n > 0 {\n    return 1\n  }\n}\n";
    assert_eq!(
        err(decls, EMIT),
        "`pick` can finish without returning a Int; every path out of a `fn` returns one"
    );

    // An `if` with both branches returning does count.
    let both = "\nfn pick(n: Int) -> Int {\n  if n > 0 {\n    return 1\n  } else {\n    return 0\n  }\n}\n";
    parse(&source(both, EMIT)).expect("both branches return");
}

/// The case a reader is most likely to get wrong, and exactly the shape a search
/// helper has: the container can be empty, so the loop can run zero times.
#[test]
fn a_for_body_does_not_count_as_returning() {
    let decls = "\nfn first(xs: List(Int)) -> Int {\n  for x in xs {\n    return x\n  }\n}\n";
    assert_eq!(
        err(decls, EMIT),
        "`first` can finish without returning a Int; every path out of a `fn` returns one"
    );

    let with_fallback =
        "\nfn first(xs: List(Int)) -> Int {\n  for x in xs {\n    return x\n  }\n  return 0\n}\n";
    parse(&source(with_fallback, EMIT)).expect("a fallback after the loop returns");
}

#[test]
fn a_bare_return_in_a_fn_says_it_needs_a_value() {
    let decls = "\nfn pick(n: Int) -> Int {\n  return\n}\n";
    assert_eq!(
        err(decls, EMIT),
        "this `fn` returns Int, so `return` needs a value"
    );
}

#[test]
fn a_fn_is_declared_once() {
    let decls = "\nfn twice(n: Int) -> Int { return n }\nfn twice(n: Int) -> Int { return n }\n";
    assert_eq!(err(decls, EMIT), "fn `twice` is declared twice");
}

// ---------------------------------------------------------------------------------
// Trailing commas.

/// Every comma-separated list took a trailing comma except a fixed-arity builtin's
/// argument list, whose parser read `arg`, `,`, `arg`, `)` literally. A port found it
/// by writing a long `reject` across three lines.
#[test]
fn a_trailing_comma_closes_an_argument_list() {
    let cases = [
        (
            "reject",
            "  return reject(\n    \"code\",\n    \"a long message\",\n  )",
        ),
        ("invalid", "  return invalid(\"m\",)"),
        (
            "fn call",
            "  emit @plan.created { plan_id, sku: effective_sku(sku, plan_id,), months }",
        ),
        (
            "method",
            "  emit @plan.created { plan_id, sku, months: months.min(1,) }",
        ),
        (
            "Uuid.derive",
            "  let u = Uuid.derive(plan_id, \"s\",)\n  emit @plan.created { plan_id, sku, months }",
        ),
        (
            "Timestamp.parse",
            "  let t = Timestamp.parse(\"2026-01-01T00:00:00Z\",)\n  emit @plan.created { plan_id, sku, months }",
        ),
    ];
    for (what, body) in cases {
        parse(&source("", body)).unwrap_or_else(|err| panic!("for {what}: {err}"));
    }
}

/// The lists that already took one, so the rule is now the same everywhere rather
/// than the same in most places.
#[test]
fn a_trailing_comma_closes_every_other_list() {
    let decls = "\nrecord Pair { a: Int, b: Int, }\nfn pair(a: Int, b: Int,) -> Int {\n  let xs = [a, b,]\n  let p = Pair { a: a, b: b, }\n  return p.a + xs.len()\n}\n";
    let body = "  emit @plan.created { plan_id, sku, months: pair(1, 2), }";
    parse(&source(decls, body)).unwrap_or_else(|err| panic!("{err}"));
}

/// A comma is trailing only when the closing paren is next, so a call that takes no
/// arguments has no last item for one to follow, and a real extra argument still
/// reaches the error written for it.
#[test]
fn a_trailing_comma_needs_a_last_argument() {
    let bare = parse(&source(
        "",
        "  let t = now(,)\n  emit @plan.created { plan_id, sku, months }",
    ))
    .expect_err("there is no argument for the comma to follow")
    .message;
    assert_eq!(bare, "expected `)`, found `,`");

    let short = parse(&source("", "  return reject(\"code\",)"))
        .expect_err("a missing argument is still missing")
        .message;
    assert_eq!(short, "expected a value, found `)`");
}
