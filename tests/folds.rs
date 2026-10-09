//! `docs/folds.md` as executable rules. A named fold is a slice of the log and the value
//! it folds to, spliced into whatever reads it, so the three things worth proving are that
//! the value arrives, that the slices reach the append condition, and that a fold and a
//! guard stay disjoint in what they may do.

use heklang::{Event, EventPath, Interpreter, Outcome, Program, Value, parse};

/// `@a.b { field: value }`, so a `given` in prose and a `given` in a test read alike.
fn given<'a>(
    interpreter: &mut Interpreter<'_>,
    path: &str,
    fields: impl IntoIterator<Item = (&'a str, Value)>,
) {
    interpreter.append(Event::new(
        EventPath::new(path.split('.')),
        fields
            .into_iter()
            .map(|(name, value)| (name.to_string(), value)),
    ));
}

fn program(source: &str) -> Program {
    match parse(source) {
        Ok(program) => program,
        Err(err) => panic!("{err}"),
    }
}

fn error(source: &str) -> String {
    parse(source)
        .map(|_| String::new())
        .expect_err("this source should not have parsed")
        .message
}

fn hint(source: &str) -> String {
    parse(source)
        .map(|_| String::new())
        .expect_err("this source should not have parsed")
        .hint
        .unwrap_or_default()
}

const EVENTS: &str = "\
refusal TooMany \"too many open orders\"

event @order.placed { order_id: Int, customer_id: Int, total: Int }
event @order.cancelled { order_id: Int, customer_id: Int, refund: Int }
";

/// Two folds, one accumulating and one replacing, and a command that reads both.
const FOLDS: &str = "\
fold LifetimeSpend(customer_id: Int) -> spend: Int = 0 {
  on @order.placed(customer_id) { total } => spend + total
  on @order.cancelled(customer_id) { refund } => spend - refund
}

fold HasOrdered(customer_id: Int) -> Bool = false {
  on @order.placed(customer_id) => true
}

command Place(order_id: Int, customer_id: Int, total: Int) {
  fold spend = LifetimeSpend { customer_id }

  if spend > 1000 {
    reject TooMany
  }

  emit @order.placed { order_id, customer_id, total }
}
";

fn placing() -> Program {
    program(&format!("{EVENTS}{FOLDS}"))
}

fn place(interpreter: &mut Interpreter<'_>, customer: i64) -> Outcome {
    interpreter
        .run(
            "Place",
            [
                ("order_id".to_string(), Value::Int(1)),
                ("customer_id".to_string(), Value::Int(customer)),
                ("total".to_string(), Value::Int(5)),
            ],
        )
        .expect("ran")
        .outcome
}

fn placed(interpreter: &mut Interpreter<'_>, customer: i64, total: i64) {
    given(
        interpreter,
        "order.placed",
        [
            ("order_id", Value::Int(1)),
            ("customer_id", Value::Int(customer)),
            ("total", Value::Int(total)),
        ],
    );
}

// ---------------------------------------------------------------------------------
// Rule 1: declaring. A fold is named for the value and takes the ids it is kept by.

#[test]
fn a_fold_must_be_declared() {
    let message = error(&format!(
        "{EVENTS}command C(customer_id: Int) {{\n  fold n = Missing {{ customer_id }}\n  return\n}}\n"
    ));
    assert_eq!(message, "fold `Missing` is not declared");
}

#[test]
fn a_guard_named_where_a_fold_goes_says_which_it_is() {
    // The two read alike at a call site and only one hands a value back, so the hint
    // names the shape the author reached for rather than only the one they wrote.
    let source = format!(
        "{EVENTS}guard Ordered(customer_id: Int) {{\n  fold seen: Bool = false\n    on @order.placed(customer_id) => true\n  if !seen {{ reject TooMany }}\n}}\n\ncommand C(customer_id: Int) {{\n  fold n = Ordered {{ customer_id }}\n  return\n}}\n"
    );
    assert_eq!(error(&source), "fold `Ordered` is not declared");
    assert!(
        hint(&source).contains("`Ordered` is a guard"),
        "the hint should name the guard, got {:?}",
        hint(&source)
    );
}

#[test]
fn a_fold_is_declared_once() {
    let message = error(&format!(
        "{EVENTS}fold F(customer_id: Int) -> Bool = false {{\n  on @order.placed(customer_id) => true\n}}\n\nfold F(customer_id: Int) -> Bool = false {{\n  on @order.placed(customer_id) => true\n}}\n"
    ));
    assert_eq!(message, "fold `F` is declared twice");
}

#[test]
fn a_fold_with_no_arms_is_only_its_seed() {
    // The arm-less check fires before `folds_nothing` and is the sharper of the two: a
    // declaration with no arms narrows nothing, so its value is its seed at every
    // position. `folds_nothing` is the backstop for a body that declares something else.
    let source = format!("{EVENTS}fold F(customer_id: Int) -> Int = 0 {{\n}}\n");
    assert_eq!(
        error(&source),
        "fold `F` has no arms, so it is only its seed"
    );
    assert!(
        hint(&source).contains("a named fold exists to declare a slice"),
        "the hint is the fold's, not a command's `let`, got {:?}",
        hint(&source)
    );
}

/// The trap `starts_item` exists for: a declaration missing from that list does not
/// error at its own position, it goes missing from the program silently. The guard twin
/// of this is `tests/guards.rs`'s `a_const_above_a_guard_does_not_swallow_it`.
#[test]
fn a_const_above_a_fold_does_not_swallow_it() {
    let program = program(&format!(
        "{EVENTS}const LIMIT: Int = 3\nfold F(customer_id: Int) -> Bool = false {{\n  on @order.placed(customer_id) => true\n}}\n"
    ));
    assert!(
        program.fold("F").is_some(),
        "the fold below a const is still in the program"
    );
}

// ---------------------------------------------------------------------------------
// Rule 2: using. The value arrives, and the arguments are checked like a guard's.

#[test]
fn a_fold_hands_its_value_back() {
    let program = placing();
    let mut interpreter = Interpreter::new(&program);
    placed(&mut interpreter, 7, 600);
    placed(&mut interpreter, 7, 600);
    assert!(matches!(
        place(&mut interpreter, 7),
        Outcome::Reject { code, .. } if code == "too_many"
    ));
}

#[test]
fn every_arm_folds_into_the_one_value() {
    let program = placing();
    let mut interpreter = Interpreter::new(&program);
    placed(&mut interpreter, 7, 600);
    placed(&mut interpreter, 7, 600);
    // The second arm takes it back under the limit, which only holds if both arms land
    // on the same accumulator in the caller's frame.
    given(
        &mut interpreter,
        "order.cancelled",
        [
            ("order_id", Value::Int(1)),
            ("customer_id", Value::Int(7)),
            ("refund", Value::Int(600)),
        ],
    );
    assert!(matches!(place(&mut interpreter, 7), Outcome::Ok(_)));
}

#[test]
fn a_folds_filters_resolve_against_the_arguments() {
    let program = placing();
    let mut interpreter = Interpreter::new(&program);
    // Another customer's orders are another slice, so they do not reach this one.
    placed(&mut interpreter, 8, 600);
    placed(&mut interpreter, 8, 600);
    assert!(matches!(place(&mut interpreter, 7), Outcome::Ok(_)));
}

#[test]
fn a_fold_takes_every_parameter() {
    let message = error(&format!(
        "{EVENTS}{FOLDS}command D(order_id: Int) {{\n  fold n = LifetimeSpend {{ }}\n  return\n}}\n"
    ));
    assert_eq!(message, "fold `LifetimeSpend` needs `customer_id`");
}

#[test]
fn a_fold_rejects_a_parameter_it_has_not_got() {
    let message = error(&format!(
        "{EVENTS}{FOLDS}command D(customer_id: Int) {{\n  fold n = LifetimeSpend {{ customer_id, nope: 1 }}\n  return\n}}\n"
    ));
    assert_eq!(message, "fold `LifetimeSpend` has no parameter `nope`");
}

#[test]
fn a_fold_argument_cannot_read_a_fold_from_its_own_stage() {
    let message = error(&format!(
        "{EVENTS}{FOLDS}command D(customer_id: Int) {{\n  fold other: Int = 0\n    on @order.placed(customer_id) => other + 1\n  fold n = LifetimeSpend {{ customer_id: other }}\n  return\n}}\n"
    ));
    assert_eq!(
        message,
        "`customer_id` is taken from `other`, which has not folded yet"
    );
}

/// Reading the same fold twice is two names for one value and the author's business, so
/// unlike `guard` there is no check against it. The asymmetry has a reason: a second
/// `guard G { x }` decides what the first decided and does nothing.
#[test]
fn the_same_fold_read_twice_is_allowed() {
    let program = program(&format!(
        "{EVENTS}{FOLDS}command D(customer_id: Int) {{\n  fold a = LifetimeSpend {{ customer_id }}\n  fold b = LifetimeSpend {{ customer_id }}\n  if a != b {{ reject TooMany }}\n  return\n}}\n"
    ));
    let mut interpreter = Interpreter::new(&program);
    let outcome = interpreter
        .run("D", [("customer_id".to_string(), Value::Int(1))])
        .expect("ran")
        .outcome;
    assert!(matches!(outcome, Outcome::Ok(_)));
}

// ---------------------------------------------------------------------------------
// Rule 3: one accumulator, and its name is written only when it is read.

#[test]
fn an_accumulator_no_arm_reads_is_refused() {
    let source = format!(
        "{EVENTS}fold F(customer_id: Int) -> seen: Bool = false {{\n  on @order.placed(customer_id) => true\n}}\n"
    );
    assert_eq!(error(&source), "fold `F` names `seen` and no arm reads it");
    assert!(
        hint(&source).contains("drop the `seen:`"),
        "the hint should name the edit, got {:?}",
        hint(&source)
    );
}

/// The shadowing hazard the rule removes. `dry_run` is both the accumulator and the bound
/// field, and `=> dry_run` is the bind; with no name bound there is one `dry_run` in scope
/// and the question cannot be asked.
#[test]
fn a_bind_of_the_same_name_does_not_count_as_a_read() {
    let message = error(
        "event @w.opened { id: Int, dry_run: Bool }\nfold F(id: Int) -> dry_run: Bool = false {\n  on @w.opened(id) { dry_run } => dry_run\n}\n",
    );
    assert_eq!(message, "fold `F` names `dry_run` and no arm reads it");
}

#[test]
fn an_arm_reading_an_unnamed_accumulator_is_not_in_scope() {
    let message = error(&format!(
        "{EVENTS}fold F(customer_id: Int) -> Int = 0 {{\n  on @order.placed(customer_id) => n + 1\n}}\n"
    ));
    assert_eq!(message, "`n` is not in scope");
}

#[test]
fn an_accumulator_an_arm_reads_is_kept() {
    let program = program(&format!(
        "{EVENTS}fold F(customer_id: Int) -> n: Int = 0 {{\n  on @order.placed(customer_id) => n + 1\n}}\n"
    ));
    assert!(program.fold("F").is_some());
}

// ---------------------------------------------------------------------------------
// Rule 4: a fold computes; a guard refuses.

#[test]
fn a_fold_cannot_refuse() {
    // Structural rather than a message: a fold's body is arms only, so there is nowhere
    // in one to write a `reject` at all. That is the stronger form of rule 4, and it is
    // why a `fold` line at a call site is never an exit.
    let message = error(&format!(
        "{EVENTS}fold F(customer_id: Int) -> Bool = false {{\n  on @order.placed(customer_id) => true\n  reject TooMany\n}}\n"
    ));
    assert!(
        !message.is_empty(),
        "a reject inside a fold body is refused"
    );
}

#[test]
fn a_fold_appends_nothing() {
    let message = error(&format!(
        "{EVENTS}fold G(customer_id: Int) -> Bool = false {{\n  on @order.placed(customer_id) => true\n  emit @order.placed {{ order_id: 1, customer_id, total: 1 }}\n}}\n"
    ));
    assert!(!message.is_empty(), "an emit inside a fold body is refused");
}

/// A guard may still hold accumulators, and may also read a named fold. Both shapes are
/// legal, which is rule 4's recommendation rather than a rule.
#[test]
fn a_guard_may_read_a_named_fold() {
    // The command emits rather than returning bare, and that is not incidental: a bare
    // statement below a `guard` lands in the stage's prologue and runs before the guard
    // decides, which is heklang's known guard-prologue bug and reproduces with an inline
    // fold and no named one. Fixing it is a change to `guard`, not to this construct.
    let program = program(&format!(
        "{EVENTS}{FOLDS}guard Ordered(customer_id: Int) {{\n  fold seen = HasOrdered {{ customer_id }}\n  if !seen {{ reject TooMany }}\n}}\n\ncommand D(order_id: Int, customer_id: Int) {{\n  guard Ordered {{ customer_id }}\n  emit @order.placed {{ order_id, customer_id, total: 1 }}\n}}\n"
    ));
    let mut interpreter = Interpreter::new(&program);
    let outcome = interpreter
        .run(
            "D",
            [
                ("order_id".to_string(), Value::Int(1)),
                ("customer_id".to_string(), Value::Int(7)),
            ],
        )
        .expect("ran")
        .outcome;
    assert!(
        matches!(outcome, Outcome::Reject { ref code, .. } if code == "too_many"),
        "the guard refuses on what the fold returned, got {outcome:?}"
    );
}

#[test]
fn a_guard_whose_whole_body_is_a_named_fold_is_not_a_fn() {
    // `folds_nothing` counts a named read, because the slice arrives at splice time. The
    // slices-only check called this a `fn` and refused the shape rule 4 recommends.
    let program = program(&format!(
        "{EVENTS}{FOLDS}guard Ordered(customer_id: Int) {{\n  fold seen = HasOrdered {{ customer_id }}\n  if !seen {{ reject TooMany }}\n}}\n"
    ));
    assert!(program.guard("Ordered").is_some());
}

// ---------------------------------------------------------------------------------
// Rule 5: a fold's slices are the caller's boundary.

#[test]
fn a_folds_slices_reach_the_append_condition() {
    let program = placing();
    let mut interpreter = Interpreter::new(&program);
    let execution = interpreter
        .run(
            "Place",
            [
                ("order_id".to_string(), Value::Int(1)),
                ("customer_id".to_string(), Value::Int(7)),
                ("total".to_string(), Value::Int(5)),
            ],
        )
        .expect("ran");
    let events: Vec<String> = execution
        .condition
        .slices
        .iter()
        .map(|slice| slice.event.to_string())
        .collect();
    assert!(
        events.iter().any(|path| path == "@order.placed")
            && events.iter().any(|path| path == "@order.cancelled"),
        "both of the fold's arms are in the condition, got {events:?}"
    );
}

#[test]
fn a_folds_filters_are_resolved_in_the_condition() {
    let program = placing();
    let mut interpreter = Interpreter::new(&program);
    let execution = interpreter
        .run(
            "Place",
            [
                ("order_id".to_string(), Value::Int(1)),
                ("customer_id".to_string(), Value::Int(7)),
                ("total".to_string(), Value::Int(5)),
            ],
        )
        .expect("ran");
    // The predicate names the customer the command was called for, not the parameter the
    // fold declared.
    assert!(
        execution.condition.slices.iter().all(|slice| {
            slice
                .filters
                .iter()
                .all(|(field, value)| field == "customer_id" && value == &Value::Int(7))
        }),
        "every filter resolved to the caller's argument"
    );
}

#[test]
fn a_command_reading_a_fold_is_one_read_of_the_log() {
    let program = placing();
    let command = program.command("Place").expect("declared");
    assert_eq!(
        command.stages.len(),
        1,
        "a declaration run is one stage however many folds it reads"
    );
}

#[test]
fn a_command_keeps_what_it_read_for_the_boundary_listing() {
    let program = placing();
    let command = program.command("Place").expect("declared");
    let read: Vec<&str> = command
        .reads
        .iter()
        .map(|call| call.fold.as_str())
        .collect();
    assert_eq!(read, vec!["LifetimeSpend"]);
}

// ---------------------------------------------------------------------------------
// Rule 7: a sealed value crosses a signature as its own type.

#[test]
fn a_fold_may_return_a_sealed_value() {
    let program = program(
        "subject Customer(Int)\nevent @order.placed { order_id: Int, customer_id: Customer, email: String @subject(customer_id) }\nfold Email(customer_id: Customer) -> Sealed(String, Customer)? = none {\n  on @order.placed(customer_id) { email } => email\n}\n",
    );
    assert!(program.fold("Email").is_some());
}

#[test]
fn a_sealed_return_is_still_behind_the_decrypt_boundary() {
    // The caller gets a value it must `reveal`, which is the point of putting the subject
    // in the signature rather than inferring it. Presence is not content, so `.is_none()`
    // on one is fine; writing it into an event is not.
    let sealed = "subject Customer(Int)\nrefusal No \"no\"\nevent @order.placed { order_id: Int, customer_id: Customer, email: String @subject(customer_id) }\nevent @copy.made { customer_id: Customer, email: String }\nfold Email(customer_id: Customer) -> Sealed(String, Customer)? = none {\n  on @order.placed(customer_id) { email } => email\n}\n";
    let message = error(&format!(
        "{sealed}command C(customer_id: Customer) {{\n  fold mail = Email {{ customer_id }}\n  emit @copy.made {{ customer_id, email: mail.unwrap_or(\"\") }}\n}}\n"
    ));
    assert!(
        !message.is_empty(),
        "sealed content cannot be written out plain through a fold's return type"
    );
}

#[test]
fn sealed_is_not_spellable_under_a_constructor() {
    let message = error(
        "subject Customer(Int)\nevent @order.placed { order_id: Int, customer_id: Customer }\nfold F(customer_id: Customer) -> List(Sealed(String, Customer)) = [] {\n  on @order.placed(customer_id) => []\n}\n",
    );
    assert!(
        !message.is_empty(),
        "`List(Sealed(..))` stays rejected, as `List(Secret)` does"
    );
}

#[test]
fn a_sealed_return_names_a_declared_subject() {
    let message = error(
        "event @order.placed { order_id: Int, customer_id: Int }\nfold F(customer_id: Int) -> Sealed(String, Nope)? = none {\n  on @order.placed(customer_id) => none\n}\n",
    );
    assert_eq!(message, "`Nope` is not a declared subject");
}

// ---------------------------------------------------------------------------------
// Rule 8: a fold may return a record.

#[test]
fn a_fold_may_return_a_record() {
    let program = program(
        "record Totals { spend: Int, orders: Int }\nconst ZERO: Totals = Totals { spend: 0, orders: 0 }\nevent @order.placed { order_id: Int, customer_id: Int, total: Int }\nfold Rollup(customer_id: Int) -> t: Totals = ZERO {\n  on @order.placed(customer_id) { total } => Totals { spend: t.spend + total, orders: t.orders + 1 }\n}\n",
    );
    assert!(program.fold("Rollup").is_some());
}

// ---------------------------------------------------------------------------------
// Rule 9: inline or named. Promotion is a header edit, so the arms are identical.

#[test]
fn an_inline_fold_and_a_named_one_take_the_same_arms() {
    // The same arm text in both positions, which is the property that makes the
    // duplicate-fold diagnostic's advice a mechanical edit rather than a rewrite.
    let arm = "on @order.placed(customer_id) { total } => n + total";
    let inline = program(&format!(
        "{EVENTS}command C(customer_id: Int) {{\n  fold n: Int = 0\n    {arm}\n  if n > 1 {{ reject TooMany }}\n  return\n}}\n"
    ));
    let named = program(&format!(
        "{EVENTS}fold Spend(customer_id: Int) -> n: Int = 0 {{\n  {arm}\n}}\n\ncommand C(customer_id: Int) {{\n  fold n = Spend {{ customer_id }}\n  if n > 1 {{ reject TooMany }}\n  return\n}}\n"
    ));
    assert_eq!(
        inline.command("C").expect("declared").stages.len(),
        named.command("C").expect("declared").stages.len(),
        "both are one stage and one read"
    );
}

#[test]
fn an_inline_fold_is_still_legal() {
    let program = program(&format!(
        "{EVENTS}command C(customer_id: Int) {{\n  fold n: Int = 0\n    on @order.placed(customer_id) => n + 1\n  if n > 3 {{ reject TooMany }}\n  return\n}}\n"
    ));
    assert_eq!(program.command("C").expect("declared").stages.len(), 1);
}

// ---------------------------------------------------------------------------------
// Rule 10: what a fold may not do.

#[test]
fn a_fold_has_no_clock() {
    let message = error(&format!(
        "{EVENTS}fold F(customer_id: Int) -> Bool = false {{\n  on @order.placed(customer_id) => now() > 0\n}}\n"
    ));
    assert!(
        !message.is_empty(),
        "a fold decides from the log; the moment is a parameter"
    );
}

#[test]
fn a_fold_is_one_read_of_the_log() {
    // A fold's body is arms only, so a second declaration run has nowhere to go: the
    // closing brace is expected where it would start.
    let message = error(&format!(
        "{EVENTS}fold F(customer_id: Int) -> Int = 0 {{\n  on @order.placed(customer_id) => 1\n  fold other: Int = 0\n    on @order.cancelled(customer_id) => 1\n}}\n"
    ));
    assert!(!message.is_empty(), "a fold reads the log once");
}

// ---------------------------------------------------------------------------------
// Rule 11: where this diverges from the runtime, which is nowhere.

#[test]
fn a_named_fold_does_not_reach_the_runtime() {
    let program = placing();
    let command = program.command("Place").expect("declared");
    // The slices are the command's own after the splice, so the interpreter sees a
    // command with its folds written out and knows nothing about the declaration.
    assert_eq!(
        command.stages[0].slices.len(),
        2,
        "both of the fold's arms are the command's slices"
    );
    assert_eq!(
        command.stages[0].folds.len(),
        1,
        "and its accumulator is the command's"
    );
}

#[test]
fn a_fold_is_in_the_program_under_its_name() {
    let program = placing();
    assert!(program.fold("LifetimeSpend").is_some());
    assert!(program.fold("HasOrdered").is_some());
    assert!(program.fold("Nope").is_none());
}

// ---------------------------------------------------------------------------------
// An effect arm reads a fold the same way, and is where most of them live. An arm has
// no `guard`, so nothing was spliced into one before this construct.

#[test]
fn an_effect_arm_reads_a_named_fold() {
    let program = program(&format!(
        "{EVENTS}{FOLDS}effect E {{\n  on @order.placed as e {{ @key customer_id }} {{\n    fold spend = LifetimeSpend {{ customer_id: e.customer_id }}\n    log(\"spend {{spend}}\")\n  }}\n}}\n"
    ));
    let effect = program
        .effects
        .iter()
        .find(|effect| effect.name == "E")
        .expect("declared");
    let arm = &effect.arms[0];
    assert_eq!(
        arm.reads
            .iter()
            .map(|call| call.fold.as_str())
            .collect::<Vec<_>>(),
        vec!["LifetimeSpend"],
        "the arm records what it read, for the same reason a command does"
    );
    assert_eq!(
        arm.stages[0].slices.len(),
        2,
        "and the fold's two arms are the arm's own slices after the splice"
    );
    assert_eq!(arm.stages[0].folds.len(), 1);
}

#[test]
fn an_effect_arm_takes_its_arguments_from_the_trigger() {
    // The shape `docs/effects.md` rule 2 recommends: what an arm needs from `e` is
    // computed above the fold, and a read's arguments resolve there too.
    let program = program(&format!(
        "{EVENTS}{FOLDS}effect E {{\n  on @order.placed as e {{ @key customer_id }} {{\n    fold spend = LifetimeSpend {{ customer_id: e.customer_id }}\n    log(\"spend {{spend}}\")\n  }}\n}}\n"
    ));
    assert!(program.fold("LifetimeSpend").is_some());
    let arm = &program
        .effects
        .iter()
        .find(|effect| effect.name == "E")
        .expect("declared")
        .arms[0];
    // The filter is an expression in the arm's arena, resolved from the trigger rather
    // than from the fold's parameter.
    assert!(
        arm.stages[0]
            .slices
            .iter()
            .all(|slice| slice.filters.len() == 1),
        "each slice is narrowed to the one customer"
    );
}
