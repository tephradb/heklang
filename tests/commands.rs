//! `docs/commands.md` as executable tests. The append condition is the part worth
//! pinning down: it is what makes a `state` a read declaration rather than a binding,
//! and it is the whole reason the keyword is not `let`.

use heklang::{Event, EventPath, Interpreter, Outcome, Program, Value, parse};

const PRELUDE: &str = "event @order.placed {
  order_id: Uuid,
  customer_id: Int,
  email: String @subject(customer_id) @max(200),
  total: Money(2),
}
event @order.cancelled { order_id: Uuid, customer_id: Int }
event @customer.blocked { customer_id: Int, reason: String }
";

const ORDER: &str = "0190d1a1-0000-7000-8000-000000000001";

fn program(body: &str) -> Program {
    parse(&format!("{PRELUDE}{body}\n"))
        .unwrap_or_else(|err| panic!("expected this command to parse: {err}"))
}

fn err(body: &str) -> String {
    parse(&format!("{PRELUDE}{body}\n"))
        .expect_err("expected this command to be rejected")
        .text()
}

fn placed(seq: u32, customer_id: i64, total: i64) -> Event {
    Event::new(
        EventPath::new(["order", "placed"]),
        [
            (
                "order_id",
                Value::uuid(format!("0190d1a1-0000-7000-8000-{seq:012}")),
            ),
            ("customer_id", Value::Int(customer_id)),
            ("email", Value::str("ada@example.com")),
            ("total", Value::money(total, 2)),
        ],
    )
}

// ---------------------------------------------------------------------------------
// `state` is a read declaration: the slices it names are the append condition.

const COUNTING: &str = "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
  state open: Int = fold 0
    on @order.placed(customer_id) => open + 1
    on @order.cancelled(customer_id) => open - 1

  emit @order.placed { order_id, customer_id, email: \"x@example.com\", total }
}";

#[test]
fn a_state_puts_its_slices_in_the_append_condition() {
    let program = program(COUNTING);
    let mut interpreter = Interpreter::new(&program);
    let execution = interpreter
        .run(
            "Place",
            [
                ("order_id", Value::uuid(ORDER)),
                ("customer_id", Value::Int(7)),
                ("total", Value::money(2_599, 2)),
            ],
        )
        .expect("ran");
    // One fold, two arms, so two slices: the boundary is per event type.
    assert_eq!(execution.condition.slices.len(), 2);
}

#[test]
fn a_let_puts_nothing_in_the_append_condition() {
    let program = program(
        "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
  let doubled = total + total

  emit @order.placed { order_id, customer_id, email: \"x@example.com\", total: doubled }
}",
    );
    let mut interpreter = Interpreter::new(&program);
    let execution = interpreter
        .run(
            "Place",
            [
                ("order_id", Value::uuid(ORDER)),
                ("customer_id", Value::Int(7)),
                ("total", Value::money(100, 2)),
            ],
        )
        .expect("ran");
    assert!(
        execution.condition.slices.is_empty(),
        "a `let` is a step, not a read declaration"
    );
}

/// `guard` is the same call as `state` with no binds and no updates, so it lands in the
/// condition the same way and is the only reason to write one.
#[test]
fn a_guard_is_a_slice_that_binds_nothing() {
    let program = program(
        "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
  guard @order.placed(order_id), @order.cancelled(order_id)

  emit @order.placed { order_id, customer_id, email: \"x@example.com\", total }
}",
    );
    let mut interpreter = Interpreter::new(&program);
    let execution = interpreter
        .run(
            "Place",
            [
                ("order_id", Value::uuid(ORDER)),
                ("customer_id", Value::Int(7)),
                ("total", Value::money(100, 2)),
            ],
        )
        .expect("ran");
    assert_eq!(execution.condition.slices.len(), 2);
}

/// A refusal still read the log, so a host that traces or caches the decision can see
/// what it depended on. This is why the condition is built after the body rather than
/// on the success path.
#[test]
fn the_condition_comes_back_for_every_outcome() {
    let program = program(
        "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
  state blocked: Bool = fold false
    on @customer.blocked(customer_id) => true

  state open: Int = fold 0
    on @order.placed(customer_id) => open + 1

  if total <= 0.00 {
    return invalid(\"a total must be positive\")
  }
  if blocked {
    return reject(\"blocked\", \"this customer cannot place orders\")
  }

  emit @order.placed { order_id, customer_id, email: \"x@example.com\", total }
}",
    );

    let mut interpreter = Interpreter::new(&program);
    let invalid = interpreter
        .run(
            "Place",
            [
                ("order_id", Value::uuid(ORDER)),
                ("customer_id", Value::Int(7)),
                ("total", Value::money(0, 2)),
            ],
        )
        .expect("ran");
    assert!(matches!(invalid.outcome, Outcome::Invalid(_)));
    assert_eq!(
        invalid.condition.slices.len(),
        2,
        "a refusal still declares what it read"
    );

    let mut interpreter = Interpreter::with_log(
        &program,
        vec![Event::new(
            EventPath::new(["customer", "blocked"]),
            [
                ("customer_id", Value::Int(7)),
                ("reason", Value::str("fraud")),
            ],
        )],
    );
    let rejected = interpreter
        .run(
            "Place",
            [
                ("order_id", Value::uuid(ORDER)),
                ("customer_id", Value::Int(7)),
                ("total", Value::money(100, 2)),
            ],
        )
        .expect("ran");
    assert!(matches!(rejected.outcome, Outcome::Reject { .. }));
    assert_eq!(rejected.condition.slices.len(), 2);
}

/// `after` is taken before the fold, so the condition means "nothing new in these
/// slices since the position I started reading at".
#[test]
fn after_is_the_log_length_before_the_fold() {
    let program = program(COUNTING);
    let log = vec![placed(1, 7, 100), placed(2, 7, 200)];
    let mut interpreter = Interpreter::with_log(&program, log);
    let execution = interpreter
        .run(
            "Place",
            [
                ("order_id", Value::uuid(ORDER)),
                ("customer_id", Value::Int(7)),
                ("total", Value::money(2_599, 2)),
            ],
        )
        .expect("ran");
    assert_eq!(execution.condition.after, 2);
}

// ---------------------------------------------------------------------------------
// Execution order: filters resolve before the fold, so they see the prologue only.

#[test]
fn a_filter_may_name_a_hoisted_let() {
    let program = program(
        "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
  let who = customer_id

  state blocked: Bool = fold false
    on @customer.blocked(customer_id: who) => true

  emit @order.placed { order_id, customer_id, email: \"x@example.com\", total }
}",
    );
    let mut interpreter = Interpreter::with_log(
        &program,
        vec![Event::new(
            EventPath::new(["customer", "blocked"]),
            [
                ("customer_id", Value::Int(7)),
                ("reason", Value::str("fraud")),
            ],
        )],
    );
    let execution = interpreter
        .run(
            "Place",
            [
                ("order_id", Value::uuid(ORDER)),
                ("customer_id", Value::Int(7)),
                ("total", Value::money(100, 2)),
            ],
        )
        .expect("ran");
    assert!(matches!(execution.outcome, Outcome::Ok(_)));
}

/// The error names the `let` and says to move it, rather than saying "not in scope" for
/// a name the reader can see three lines below.
#[test]
fn a_filter_naming_a_later_let_says_to_move_it() {
    let message = err(
        "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
  state blocked: Bool = fold false
    on @customer.blocked(customer_id: who) => true

  let who = customer_id
  emit @order.placed { order_id, customer_id, email: \"x@example.com\", total }
}",
    );
    assert!(message.contains("below the declarations"), "got: {message}");
    assert!(message.contains("move that `let` up"), "got: {message}");
}

#[test]
fn state_and_guard_must_precede_the_first_statement() {
    let message = err(
        "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
  emit @order.placed { order_id, customer_id, email: \"x@example.com\", total }

  state open: Int = fold 0
    on @order.placed(customer_id) => open + 1
}",
    );
    assert_eq!(
        message,
        "`state` and `guard` must come before the first statement"
    );
}

// ---------------------------------------------------------------------------------
// What a command may not do. Each message names the rule where it is broken.

fn body_is_rejected(body: &str) -> String {
    err(&format!(
        "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {{\n  {body}\n  return\n}}"
    ))
}

#[test]
fn a_command_cannot_call_out() {
    let message = body_is_rejected("let r = http.get(\"https://out.example\")");
    assert!(
        message.contains("only an effect can call out"),
        "got: {message}"
    );
}

#[test]
fn a_command_cannot_invoke() {
    let message = body_is_rejected("invoke Place { order_id, customer_id, total }");
    assert!(
        message.contains("`invoke` calls a command, so it can only appear in an effect"),
        "got: {message}"
    );
}

#[test]
fn a_command_cannot_decrypt() {
    let message = err(
        "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
  state held: String = fold \"\"
    on @order.placed(customer_id) { email } => email

  let seen = reveal(held)
  return
}",
    );
    assert!(
        message.contains("a command decides from state without reaching personal data"),
        "got: {message}"
    );
}

#[test]
fn a_command_cannot_write_a_read_model() {
    let message = body_is_rejected("delete Row[order_id]");
    assert!(
        message.contains("can only appear in a projector"),
        "got: {message}"
    );
}

#[test]
fn a_command_cannot_fail() {
    let message = body_is_rejected("fail(\"nope\")");
    assert!(
        message.contains("a command returns `invalid(...)` or `reject(...)`"),
        "got: {message}"
    );
}

/// Moving sealed content is not reading it, so a command may fold a subject-bound field
/// and emit it into a field sealed under the same subject. See `docs/effects.md` rule 12.
#[test]
fn a_command_may_move_sealed_content_without_revealing() {
    program(
        "command Replace(order_id: Uuid, customer_id: Int, total: Money(2)) {
  state held: String = fold \"\"
    on @order.placed(customer_id) { email } => email

  emit @order.placed { order_id, customer_id, email: held, total }
}",
    );
}

// ---------------------------------------------------------------------------------
// Outcomes.

#[test]
fn falling_off_the_end_is_ok_with_what_was_emitted() {
    let program = program(COUNTING);
    let mut interpreter = Interpreter::new(&program);
    let execution = interpreter
        .run(
            "Place",
            [
                ("order_id", Value::uuid(ORDER)),
                ("customer_id", Value::Int(7)),
                ("total", Value::money(2_599, 2)),
            ],
        )
        .expect("ran");
    let Outcome::Ok(events) = execution.outcome else {
        panic!("expected Ok");
    };
    assert_eq!(events.len(), 1);
}

#[test]
fn a_command_may_commit_having_emitted_nothing() {
    let program = program(
        "command Touch(order_id: Uuid) {
  guard @order.placed(order_id)
  return
}",
    );
    let mut interpreter = Interpreter::new(&program);
    let execution = interpreter
        .run("Touch", [("order_id", Value::uuid(ORDER))])
        .expect("ran");
    assert_eq!(execution.outcome, Outcome::Ok(Vec::new()));
    // The read set is still declared, which is the point: an empty append under a
    // condition is how a command asserts that something has not happened.
    assert_eq!(execution.condition.slices.len(), 1);
}

/// An event is written whole, the rule `given` and a record literal already held to.
/// `emit` did not run it: a command that omitted a field checked clean and raised
/// `event @order.cancelled is missing field ...` at the append instead, so an untested
/// branch shipped broken.
#[test]
fn an_emit_gives_every_field() {
    let message = err("command Cancel(order_id: Uuid) {
  emit @order.cancelled { order_id }
}");
    assert_eq!(
        message,
        "`emit @order.cancelled` needs `customer_id`; an event is written whole"
    );
}

/// And gives each of them once, which the same gap allowed.
#[test]
fn an_emit_gives_each_field_once() {
    let message = err("command Cancel(order_id: Uuid, customer_id: Int) {
  emit @order.cancelled { order_id, order_id, customer_id }
}");
    assert_eq!(message, "`order_id` is given twice");
}
