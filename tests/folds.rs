//! `docs/folds.md` as executable rules. A named fold is a slice of the log and the value
//! it folds to, spliced into whatever reads it, so the three things worth proving are that
//! the value arrives, that the slices reach the append condition, and that a fold and a
//! guard stay disjoint in what they may do.

use heklang::{Event, EventPath, Filter, Interpreter, Outcome, Program, Type, Value, parse};

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

/// One arm, binding a field named like the accumulator and replacing the value with it.
/// Nothing reads the running value, so the *name* is what is wrong rather than the bind:
/// `-> Bool = false` says the same thing and leaves one `dry_run` in scope.
///
/// Reported as `unused-accumulator` and not as shadowing, because a fold whose every arm
/// replaces the value has nothing to shadow. That is the latest-value-wins fold, and
/// `fold token: String? = none / on @shop.connected { token } => token` is the common
/// spelling of it; refusing the bind there would force a rename for no gain.
#[test]
fn a_bind_of_the_same_name_does_not_count_as_a_read() {
    let message = error(
        "event @w.opened { id: Int, dry_run: Bool }\nfold F(id: Int) -> dry_run: Bool = false {\n  on @w.opened(id) { dry_run } => dry_run\n}\n",
    );
    assert_eq!(message, "fold `F` names `dry_run` and no arm reads it");
}

/// The hazard is one name with two meanings: an arm that reads the running value beside
/// an arm that binds a field of the same name. The second arm's `=> n + 1` folds the
/// event's `n` and discards the total, and this checked clean and ran before.
///
/// `unused-accumulator` cannot stand in for it, which is the point: it asks whether
/// *any* arm reads the slot, so the first arm here satisfies it.
#[test]
fn an_arm_reading_the_accumulator_forbids_another_binding_its_name() {
    let message = error(
        "event @w.opened { id: Int, n: Int }\nevent @w.closed { id: Int, n: Int }\nfold F(id: Int) -> n: Int = 0 {\n  on @w.opened(id) => n + 1\n  on @w.closed(id) { n } => n + 1\n}\n",
    );
    assert_eq!(
        message,
        "`n` is `F`'s accumulator and another arm reads it, so this arm cannot bind a field of that name"
    );
}

/// The inline form shares `fold_arms`, so it shares the rule. Its declaration name *is*
/// its accumulator's, so the message cannot name the fold without reading "`n` is `n`'s".
#[test]
fn an_inline_fold_has_the_same_rule() {
    let message = error(
        "event @w.opened { id: Int, n: Int }\nevent @w.closed { id: Int, n: Int }\ncommand C(id: Int) {\n  fold n: Int = 0\n    on @w.opened(id) => n + 1\n    on @w.closed(id) { n } => n + 1\n  return\n}\n",
    );
    assert_eq!(
        message,
        "`n` is this fold's accumulator and another arm reads it, so this arm cannot bind a field of that name"
    );
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
    // Structural rather than a message, and the asserted text says so: a fold's body is
    // arms only, so there is nowhere in one to write a `reject` at all. That is the
    // stronger form of rule 4, and it is why a `fold` line at a call site is never an
    // exit. Asserted exactly, so a fixture that stops parsing for some *other* reason
    // cannot keep the test passing.
    let message = error(&format!(
        "{EVENTS}fold F(customer_id: Int) -> Bool = false {{\n  on @order.placed(customer_id) => true\n  reject TooMany\n}}\n"
    ));
    assert_eq!(message, "expected `}`, found `reject`");
}

#[test]
fn a_fold_appends_nothing() {
    let message = error(&format!(
        "{EVENTS}fold G(customer_id: Int) -> Bool = false {{\n  on @order.placed(customer_id) => true\n  emit @order.placed {{ order_id: 1, customer_id, total: 1 }}\n}}\n"
    ));
    assert_eq!(message, "expected `}`, found `emit`");
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
    assert_eq!(
        message,
        "`unwrap_or` would put a plaintext default and content sealed under `Customer` in one slot, with nothing static to say which is in it"
    );
}

#[test]
fn sealed_is_not_spellable_under_a_constructor() {
    let message = error(
        "subject Customer(Int)\nevent @order.placed { order_id: Int, customer_id: Customer }\nfold F(customer_id: Customer) -> List(Sealed(String, Customer)) = [] {\n  on @order.placed(customer_id) => []\n}\n",
    );
    // `Sealed` is reachable only from the top of a fold's return type, so under a
    // constructor the word is an ordinary unknown type rather than a rule being broken.
    assert_eq!(message, "unknown type `Sealed`");
}

/// The declared subject is checked against what the arms carry. It was not, and the
/// double seal below is why nothing noticed: the type the arms produced and the type the
/// signature declared never had to agree.
#[test]
fn a_sealed_return_names_the_subject_its_arms_fold() {
    let message = error(
        "subject Customer(Int)\nsubject Shop(Int)\nevent @order.placed { order_id: Int, customer_id: Customer, email: String @subject(customer_id) }\nfold F(customer_id: Customer) -> Sealed(String, Shop)? = none {\n  on @order.placed(customer_id) { email } => email\n}\n",
    );
    assert_eq!(
        message,
        "`F` declares content sealed under `Shop` and the arm on @order.placed folds content sealed under `Customer`"
    );
}

/// **A declared type that says nothing is a leak, not something to patch up.** Sealing
/// the local slot is right for an *inline* fold, whose value lives in that slot in the
/// declaration that reads it, so every check downstream sees the seal. A named fold
/// publishes `ret` instead: the caller's local, the IR and the column `crate::tables`
/// types are all taken from the declared type. Sealing the slot and leaving `ret` plain
/// therefore handed the plaintext to a plain position with nothing static to say so, and
/// typed the stored column as plaintext, so nothing would ever have shredded it for an
/// erased subject.
///
/// Found by `/code-review` on the whole branch. The byte-identical inline fold was
/// correctly refused the whole time, which is what made the asymmetry invisible: the
/// shape looked covered.
#[test]
fn a_named_folds_return_type_cannot_stay_silent_about_a_seal() {
    const SEALED: &str = "subject Shop(Int)
event @shop.connected { shop_id: Shop, email: String @subject(shop_id) }
";
    let source = format!(
        "{SEALED}fold MerchantEmail(shop_id: Shop) -> String? = none {{
  on @shop.connected(shop_id) {{ email }} => email
}}
"
    );
    assert_eq!(
        error(&source),
        "`MerchantEmail` folds content sealed under `Shop` on @shop.connected, and its return type does not say so"
    );
    // The hint spells what rule 7 lets an author write, not what `Display` renders for an
    // event field, which is `String @subject(Shop)?` and is unwritable here.
    assert!(
        hint(&source).contains("`-> Sealed(String, Shop)?`"),
        "{}",
        hint(&source)
    );

    // Declaring it is accepted, and is the one position `Sealed` is spellable in.
    program(&format!(
        "{SEALED}fold MerchantEmail(shop_id: Shop) -> Sealed(String, Shop)? = none {{
  on @shop.connected(shop_id) {{ email }} => email
}}
"
    ));

    // An inline fold is unchanged: it has no signature to disagree with, so the seal
    // rides on the slot and the misuse is caught where the plaintext would escape.
    let inline = format!(
        "{SEALED}event @leaked {{ text: String }}
command C(shop_id: Shop) {{
  fold plain: String? = none
    on @shop.connected(shop_id) {{ email }} => email
  emit @leaked {{ text: plain.unwrap_or(\"none\") }}
}}
"
    );
    assert!(
        error(&inline).contains("would put a plaintext default and content sealed under `Shop`"),
        "{}",
        error(&inline)
    );
}

/// And the seal is applied once. `build::seal` is not idempotent, so a return type that
/// already spells the seal used to be sealed again, giving
/// `Opt(Sealed(Sealed(String, Customer), Customer))`: unwritable in source, disagreeing
/// with the `Update::ty` of its own arms, and hashed into the digest.
#[test]
fn a_declared_seal_is_not_applied_twice() {
    let program = program(
        "subject Customer(Int)\nevent @order.placed { order_id: Int, customer_id: Customer, email: String @subject(customer_id) }\nfold Email(customer_id: Customer) -> Sealed(String, Customer)? = none {\n  on @order.placed(customer_id) { email } => email\n}\n",
    );
    let fold = program.fold("Email").expect("declared");
    assert_eq!(
        fold.ret.to_string(),
        "String @subject(Customer)?",
        "the declared type is what the author wrote, as `Display` renders a seal"
    );
    let var = fold.stage.folds.first().expect("one accumulator");
    assert_eq!(
        var.ty, fold.ret,
        "and the accumulator holds it, rather than a seal around a seal"
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
    assert_eq!(message, "a `fold` reads the log, so it cannot read a clock");
}

/// Rule 10's table says `fold-restriction`, and it has to be that rather than
/// `wrong-context`: `gate` answers for a fold as though it were the command it is spliced
/// into, so gating first told an author in an *effect* about a rule for commands.
#[test]
fn a_fold_may_not_decrypt() {
    let message = error(
        "subject Customer(Int)\nevent @order.placed { order_id: Int, customer_id: Customer, email: String @subject(customer_id) }\nfold F(customer_id: Customer) -> String? = none {\n  on @order.placed(customer_id) { email } => reveal(email)\n}\n",
    );
    assert_eq!(message, "a `fold` reads the log, so it cannot decrypt");
}

#[test]
fn a_fold_may_not_read_a_secret() {
    let message = error(&format!(
        "{EVENTS}secret TOKEN\nfold F(customer_id: Int) -> Int = 0 {{\n  on @order.placed(customer_id) => TOKEN.length\n}}\n"
    ));
    assert_eq!(message, "a `fold` cannot read `TOKEN`");
}

/// A fold's arm filters are checked against what its own stage declares, which a
/// command's body gets from `decl_body` and a fold's body does not go through. Without
/// it this passed `hek check` and died at run time reading a slot before it was set.
#[test]
fn an_arm_filter_may_not_read_the_accumulator() {
    let message = error(&format!(
        "{EVENTS}fold F(customer_id: Int) -> acc: Int = 0 {{\n  on @order.placed(customer_id: acc) => acc + 1\n}}\n"
    ));
    assert_eq!(
        message,
        "this filter on `customer_id` reads a `fold` beside it"
    );
}

#[test]
fn a_fold_is_one_read_of_the_log() {
    // A fold's body is arms only, so a second declaration run has nowhere to go: the
    // closing brace is expected where it would start.
    let message = error(&format!(
        "{EVENTS}fold F(customer_id: Int) -> Int = 0 {{\n  on @order.placed(customer_id) => 1\n  fold other: Int = 0\n    on @order.cancelled(customer_id) => 1\n}}\n"
    ));
    assert_eq!(message, "expected `}`, found `fold`");
}

// ---------------------------------------------------------------------------------
// Rule 11: where this diverges from the runtime. Nowhere on the decision path, because
// the splice removes it; and `Interpreter::fold` on the read path, which is how a stored
// column is served.

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

/// And it is foldable by name, over the whole log, for one key. The read a stored column
/// is filled by, and the same entry point a test's `fold` action goes through.
#[test]
fn a_named_fold_folds_by_name_for_a_key() {
    let program = placing();
    let mut interpreter = Interpreter::new(&program);
    placed(&mut interpreter, 7, 400);
    placed(&mut interpreter, 7, 50);
    placed(&mut interpreter, 8, 999);
    given(
        &mut interpreter,
        "order.cancelled",
        [
            ("order_id", Value::Int(1)),
            ("customer_id", Value::Int(7)),
            ("refund", Value::Int(150)),
        ],
    );

    let folded = interpreter
        .fold(
            "LifetimeSpend",
            [("customer_id".to_string(), Value::Int(7))],
        )
        .expect("folded");
    assert_eq!(folded, Value::Int(300));

    // A key no event touched is the seed rather than an absence: a row that has not been
    // written to still has a value, which is what makes a stored column total.
    let folded = interpreter
        .fold(
            "LifetimeSpend",
            [("customer_id".to_string(), Value::Int(9))],
        )
        .expect("folded");
    assert_eq!(folded, Value::Int(0));
}

#[test]
fn folding_an_undeclared_name_says_so() {
    let program = placing();
    let interpreter = Interpreter::new(&program);
    let err = interpreter
        .fold("Turnover", [("customer_id".to_string(), Value::Int(7))])
        .expect_err("not declared");
    assert_eq!(err.to_string(), "unknown fold `Turnover`");
}

/// The argument contract is a command's, because `bind_params` is the same code: a
/// missing one, an extra one and a wrong type all answer the way `run` does.
#[test]
fn folding_checks_its_arguments() {
    let program = placing();
    let interpreter = Interpreter::new(&program);
    let err = interpreter
        .fold("LifetimeSpend", [] as [(String, Value); 0])
        .expect_err("needs the key");
    assert_eq!(err.to_string(), "missing argument `customer_id`");

    // The missing one is reported first, which is why this passes the key as well: an
    // extra argument is only the complaint once every parameter is bound.
    let err = interpreter
        .fold(
            "LifetimeSpend",
            [
                ("customer_id".to_string(), Value::Int(7)),
                ("period".to_string(), Value::Int(1)),
            ],
        )
        .expect_err("not a parameter");
    assert_eq!(err.to_string(), "unexpected argument `period`");
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

// ---------------------------------------------------------------------------------
// Rule 9's check: one inline fold written the same way in two bodies.

#[test]
fn the_same_inline_fold_in_two_bodies_is_refused() {
    let source = format!(
        "{EVENTS}command A(customer_id: Int) {{\n  fold n: Int = 0\n    on @order.placed(customer_id) => n + 1\n  if n > 1 {{ reject TooMany }}\n  return\n}}\n\ncommand B(customer_id: Int) {{\n  fold n: Int = 0\n    on @order.placed(customer_id) => n + 1\n  if n > 2 {{ reject TooMany }}\n  return\n}}\n"
    );
    assert!(
        error(&source)
            == "this fold is written the same way in two places; name it, so the two cannot drift apart",
        "got {:?}",
        error(&source)
    );
    assert!(
        hint(&source).contains("no test can see it"),
        "the hint says what drifting costs, got {:?}",
        hint(&source)
    );
}

/// Two arms of one effect count as two bodies. This is the case the rule was measured
/// on: nineteen arms of one effect folded the same thing nineteen times, and keying on
/// the declaration would have missed every one.
#[test]
fn two_arms_of_one_effect_are_two_bodies() {
    let source = format!(
        "{EVENTS}effect E {{\n  on @order.placed as e {{ @key customer_id }} {{\n    fold n: Int = 0\n      on @order.placed(customer_id: e.customer_id) => n + 1\n    log(\"{{n}}\")\n  }}\n  on @order.cancelled as e {{ @key customer_id }} {{\n    fold n: Int = 0\n      on @order.placed(customer_id: e.customer_id) => n + 1\n    log(\"{{n}}\")\n  }}\n}}\n"
    );
    assert!(
        error(&source)
            == "this fold is written the same way in two places; name it, so the two cannot drift apart",
        "got {:?}",
        error(&source)
    );
}

/// The near-miss pairs the rule names. Same slice, same seed, **different bound field**:
/// two folds over two fields of one event, which is why the accumulator's own name is
/// part of the comparison rather than normalised away.
#[test]
fn a_different_bound_field_is_a_different_fold() {
    let program = program(
        "event @w.opened { id: Int, on_a: Bool, on_b: Bool }\ncommand C(id: Int) {\n  fold a: Bool = false\n    on @w.opened(id) { on_a } => on_a\n  fold b: Bool = false\n    on @w.opened(id) { on_b } => on_b\n  if a && b { return }\n  return\n}\n",
    );
    assert_eq!(program.command("C").expect("declared").stages.len(), 1);
}

#[test]
fn a_different_seed_or_event_is_a_different_fold() {
    // The FR-11 window snapshot against the live policy: alike on the page, opposite in
    // meaning, and apart in both the event and the seed.
    let program = program(
        "event @w.opened { id: Int, on_a: Bool }\nevent @p.configured { id: Int, on_a: Bool }\ncommand C(id: Int) {\n  fold snap: Bool = false\n    on @w.opened(id) { on_a } => on_a\n  fold live: Bool = true\n    on @p.configured(id) { on_a } => on_a\n  if snap && live { return }\n  return\n}\n",
    );
    assert_eq!(program.command("C").expect("declared").stages.len(), 1);
}

/// There is no reverse check: reading one named fold from two bodies is the point of the
/// construct, not a diagnostic.
#[test]
fn reading_one_named_fold_from_two_bodies_is_fine() {
    let program = program(&format!(
        "{EVENTS}{FOLDS}command D(customer_id: Int) {{\n  fold a = HasOrdered {{ customer_id }}\n  if a {{ return }}\n  return\n}}\n\ncommand E(customer_id: Int) {{\n  fold a = HasOrdered {{ customer_id }}\n  if a {{ return }}\n  return\n}}\n"
    ));
    assert!(program.command("D").is_some() && program.command("E").is_some());
}

// ---------------------------------------------------------------------------------
// Rule 12: an arm may bind the envelope, and doing so makes the fold query-only.

/// The events the port's read models bucket and stamp by append time. Neither declares a
/// `Timestamp`, which is the whole reason the envelope has to be reachable: of the
/// nineteen event types the port's `stats` projector buckets, thirteen carry none.
const STAMPED: &str = "\
event @usage.threshold.reached { shop_id: Int, period: String, threshold: Int }
event @edit.attempt.blocked { shop_id: Int, order_id: Int, code: String }
";

#[test]
fn an_arm_may_bind_the_envelope() {
    let program = program(&format!(
        "{STAMPED}
fold BlockedAt(shop_id: Int, order_id: Int, code: String) -> Timestamp? = none {{
  on @edit.attempt.blocked as e (shop_id, order_id, code) => e.at
}}
"
    ));
    let fold = program.fold("BlockedAt").expect("declared");
    assert!(fold.query_only(), "binding the envelope is what decides it");
    assert_eq!(fold.envelope.len(), 1);
}

/// All three, because `docs/projectors.md` defines the envelope as all three and a fold
/// reaches the same one.
#[test]
fn an_arm_reaches_at_and_id_and_position() {
    let program = program(&format!(
        "{STAMPED}
fold A(shop_id: Int, order_id: Int, code: String) -> Timestamp? = none {{
  on @edit.attempt.blocked as e (shop_id, order_id, code) => e.at
}}

fold B(shop_id: Int, order_id: Int, code: String) -> Uuid? = none {{
  on @edit.attempt.blocked as e (shop_id, order_id, code) => e.id
}}

fold C(shop_id: Int, order_id: Int, code: String) -> Int = 0 {{
  on @edit.attempt.blocked as e (shop_id, order_id, code) => e.position
}}
"
    ));
    for name in ["A", "B", "C"] {
        assert!(program.fold(name).expect("declared").query_only(), "{name}");
    }
}

/// Deduplicated across the arms, because the slots are filled from the event being
/// folded and every arm sees the same ones.
#[test]
fn two_arms_reading_the_same_envelope_field_share_one_slot() {
    let program = program(&format!(
        "{STAMPED}
fold Stamped(shop_id: Int, period: String) -> Timestamp? = none {{
  on @usage.threshold.reached as e (shop_id, period) => e.at
  on @usage.threshold.reached as e (shop_id, period) => e.at
}}
"
    ));
    assert_eq!(program.fold("Stamped").expect("declared").envelope.len(), 1);
}

/// The whole point of the rule. A decision may not read an append time: a test writes
/// payload fields and cannot pin it, and a backfilled event's is when it was imported.
#[test]
fn a_command_cannot_fold_a_query_only_fold() {
    let source = format!(
        "{STAMPED}event @order.noted {{ shop_id: Int, order_id: Int }}
fold BlockedAt(shop_id: Int, order_id: Int, code: String) -> Timestamp? = none {{
  on @edit.attempt.blocked as e (shop_id, order_id, code) => e.at
}}

command Note(shop_id: Int, order_id: Int, code: String) {{
  fold at = BlockedAt {{ shop_id, order_id, code }}
  if at.is_some() {{
    emit @order.noted {{ shop_id, order_id }}
  }}
}}
"
    );
    assert_eq!(
        error(&source),
        "`BlockedAt` binds the envelope (`as`), so it can be queried but not folded by a command"
    );
}

#[test]
fn a_guard_cannot_fold_one_either() {
    let source = format!(
        "{STAMPED}refusal NotBlocked \"not blocked\"
fold BlockedAt(shop_id: Int, order_id: Int, code: String) -> Timestamp? = none {{
  on @edit.attempt.blocked as e (shop_id, order_id, code) => e.at
}}

guard HasBeenBlocked(shop_id: Int, order_id: Int, code: String) {{
  fold at = BlockedAt {{ shop_id, order_id, code }}
  if at.is_none() {{
    reject NotBlocked
  }}
}}
"
    );
    assert!(
        error(&source).contains("but not folded by a guard"),
        "{}",
        error(&source)
    );
}

/// An inline fold lives in the declaration that reads it, so binding the envelope there
/// is a contradiction rather than a restriction.
#[test]
fn an_inline_fold_cannot_bind_the_envelope() {
    let source = format!(
        "{STAMPED}event @order.noted {{ shop_id: Int, order_id: Int }}
command Note(shop_id: Int, order_id: Int, code: String) {{
  fold seen: Int = 0
    on @edit.attempt.blocked as e (shop_id, order_id, code) => e.position
  if seen > 0 {{
    emit @order.noted {{ shop_id, order_id }}
  }}
}}
"
    );
    assert_eq!(error(&source), "an inline fold cannot bind the envelope");
    assert!(
        hint(&source).contains("declare it at module scope and store it"),
        "{}",
        hint(&source)
    );
}

/// A filter resolves once per invocation and the envelope belongs to each event folded,
/// so the binding deliberately does not reach one. This is where section 8's computed
/// keys will land, and until they do the message has to say why rather than just no.
#[test]
fn the_envelope_does_not_reach_a_filter() {
    let source = format!(
        "{STAMPED}fn bucket(at: Timestamp) -> String {{
  return \"2020-01\"
}}

fold SeenOn(shop_id: Int, period: String) -> Int = 0 {{
  on @usage.threshold.reached as e (shop_id, period: bucket(e.at)) => 1
}}
"
    );
    assert_eq!(error(&source), "`e` is not in scope in a filter");
}

/// The rule is narrow: a fold that binds nothing is foldable by a command as before.
#[test]
fn a_fold_that_binds_no_envelope_is_not_query_only() {
    let program = program(&format!("{EVENTS}{FOLDS}"));
    let fold = program.fold("HasOrdered").expect("declared");
    assert!(!fold.query_only());
    assert!(fold.envelope.is_empty());
}

/// A binding no arm reads is rule 3's rule for the accumulator, applied to the other
/// name an arm can bind. Sharper here, because an unread one would silently decide that
/// the whole fold is query-only. Found four of these in a converted application.
#[test]
fn an_arm_that_binds_the_envelope_must_read_it() {
    let source = format!(
        "{STAMPED}
fold Counted(shop_id: Int, period: String) -> Int = 0 {{
  on @usage.threshold.reached as e (shop_id, period) => 1
}}
"
    );
    assert_eq!(error(&source), "this arm binds `e` and does not read it");
    assert!(hint(&source).contains("drop the `as`"), "{}", hint(&source));
}

/// Resolution consults the envelope binding before the scopes, so a name that is both
/// wins as the envelope and whatever it hid is simply unreachable for that arm. The
/// accumulator read as the running value in one arm and the record in the next.
#[test]
fn an_envelope_binding_cannot_hide_the_accumulator() {
    let source = format!(
        "{STAMPED}
fold LastSeen(shop_id: Int, period: String) -> last: Timestamp? = none {{
  on @usage.threshold.reached(shop_id, period) {{ threshold }} => last
  on @usage.threshold.reached as last (shop_id, period) => last.at
}}
"
    );
    assert_eq!(error(&source), "`last` is the accumulator");
}

#[test]
fn an_envelope_binding_cannot_hide_a_parameter() {
    let source = format!(
        "{STAMPED}
fold F(shop_id: Int, e: String) -> Timestamp? = none {{
  on @usage.threshold.reached as e (shop_id, period: e) => e.at
}}
"
    );
    assert_eq!(error(&source), "`e` is already in scope");
}

/// A payload field reached through the binding, which `/code-review` found aborts the
/// compiler: the bypass that lets an arm read its own envelope reached the handler path
/// below it, and a module-scope fold has no `self.event` for that path to unwrap.
#[test]
fn a_payload_field_through_the_binding_is_a_diagnostic_and_not_a_panic() {
    let source = format!(
        "{STAMPED}
fold LastThreshold(shop_id: Int, period: String) -> Int = 0 {{
  on @usage.threshold.reached as e (shop_id, period) => e.threshold
}}
"
    );
    assert_eq!(
        error(&source),
        "`e.threshold` is not an envelope field, and an arm's `as` binding carries only `at`, `id` and `position`"
    );
    assert!(
        hint(&source).contains("binds the event's own fields in `{ }`"),
        "{}",
        hint(&source)
    );
}

/// The other half of the shadow check, and the half the first one structurally cannot
/// see: `fold_envelope` runs before the arm's binds are allocated, so a bind of the
/// binding's own name was the one spelling it missed. Resolution puts the envelope ahead
/// of the scopes, so the bind was unreachable for that arm and checked clean.
#[test]
fn an_envelope_binding_cannot_be_rebound_as_a_field_of_its_own_arm() {
    let source = "\
event @usage.counted { shop_id: Int, e: Int }

fold LastAt(shop_id: Int) -> Timestamp? = none {
  on @usage.counted as e (shop_id) { e } => e.at
}
";
    assert_eq!(error(source), "`e` is this arm's envelope binding");
}

/// A named fold's name leaves the program as a stored column's, so it is held to the two
/// rules that keep `snake_case` reversible. Without them `Sum` and `sum` derive one
/// column and a runtime building the real table keys both folds to it.
#[test]
fn a_named_folds_name_is_spelled_like_a_type() {
    let lower = format!(
        "{EVENTS}fold lifetimeSpend(customer_id: Int) -> s: Int = 0 {{
  on @order.placed(customer_id) {{ total }} => s + total
}}
"
    );
    assert_eq!(
        error(&lower),
        "a named fold is named like a type, so `lifetimeSpend` starts with a capital"
    );

    let scored = format!(
        "{EVENTS}fold Lifetime_Spend(customer_id: Int) -> s: Int = 0 {{
  on @order.placed(customer_id) {{ total }} => s + total
}}
"
    );
    assert_eq!(
        error(&scored),
        "a named fold's name has no `_`, and `Lifetime_Spend` has one"
    );

    // An inline fold's accumulator is a local and keeps the local spelling, which is the
    // whole reason the rule sits on the signature rather than on the keyword.
    program(&format!(
        "{EVENTS}command C(customer_id: Int) {{
  fold spend: Int = 0
    on @order.placed(customer_id) {{ total }} => spend + total
  emit @order.placed {{ order_id: 1, customer_id, total: spend }}
}}
"
    ));
}

/// **One declaration has to answer one way through both drivers.** A spliced call assigns
/// its argument into the callee's parameter slot with no declared type to coerce against,
/// so a bare value in an optional parameter stayed bare; `Interpreter::fold` reads the
/// same fold through `bind_params`, which coerces. The fold action therefore passed on a
/// fold the command could not run. Found by `/code-review`; the hole pre-dated the action
/// on the `guard` path, which is why the lift is written at the call site rather than in
/// either driver.
#[test]
fn an_optional_parameter_takes_a_bare_argument_through_the_splice_too() {
    let source = "\
event @order.placed { order_id: Int, customer_id: Int, total: Int }
event @order.counted { n: Int }

fold Sum(customer_id: Int, bias: Int?) -> s: Int = 0 {
  on @order.placed(customer_id) { total } =>
    if bias.is_none() { s + total } else { s + total + 100 }
}

command C(customer_id: Int) {
  fold s = Sum { customer_id, bias: 5 }
  emit @order.counted { n: s }
}
";
    let program = program(source);
    let mut interpreter = Interpreter::new(&program);
    placed(&mut interpreter, 7, 1);

    let Outcome::Ok(events) = interpreter
        .run("C", [("customer_id".to_string(), Value::Int(7))])
        .expect("ran")
        .outcome
    else {
        panic!("expected the command to append");
    };
    assert_eq!(events[0].field("n"), Some(&Value::Int(101)));

    // And the same fold by name, which is the driver that always coerced.
    let folded = interpreter
        .fold(
            "Sum",
            [
                ("customer_id".to_string(), Value::Int(7)),
                ("bias".to_string(), Value::Int(5)),
            ],
        )
        .expect("folded");
    assert_eq!(folded, Value::Int(101));
}

// ---------------------------------------------------------------------------------
// Rule 13: an arm may match a key part the event does not carry.

/// The queue shape the rule exists for: one fold holding several kinds of work, keyed by
/// the kind alongside the ids. Without it the two arms are one value, because both are
/// keyed by the same order.
const QUEUE: &str = "\
subject Shop(Int)

event @hold.place.failed { shop_id: Shop, order_id: Int, attempt: Int }
event @hold.release.failed { shop_id: Shop, order_id: Int, attempt: Int }
event @a.done { n: Int }

fold RetryAttempt(shop_id: Shop, kind: String, order_id: Int) -> Int = 0 {
  on @hold.place.failed(shop_id, order_id, kind == \"hold_place\") { attempt } => attempt
  on @hold.release.failed(shop_id, order_id, kind == \"hold_release\") { attempt } => attempt
}
";

fn failed(interpreter: &mut Interpreter<'_>, path: &str, order: i64, attempt: i64) {
    given(
        interpreter,
        path,
        [
            ("shop_id", Value::Int(1)),
            ("order_id", Value::Int(order)),
            ("attempt", Value::Int(attempt)),
        ],
    );
}

fn queued() -> Program {
    program(QUEUE)
}

#[test]
fn a_key_part_the_event_does_not_carry_tells_two_arms_apart() {
    let program = queued();
    let mut interpreter = Interpreter::new(&program);
    failed(&mut interpreter, "hold.place.failed", 9, 2);
    failed(&mut interpreter, "hold.release.failed", 9, 7);

    let folded = |kind: &str| {
        interpreter
            .fold(
                "RetryAttempt",
                [
                    ("shop_id".to_string(), Value::Int(1)),
                    ("kind".to_string(), Value::str(kind)),
                    ("order_id".to_string(), Value::Int(9)),
                ],
            )
            .expect("folded")
    };
    assert_eq!(folded("hold_place"), Value::Int(2));
    assert_eq!(folded("hold_release"), Value::Int(7));
    // A kind no arm names folds nothing at all, so the value is the seed. Every slice is
    // dead here, which is the case that must not reach the host with an empty predicate
    // list and read the whole range instead.
    assert_eq!(folded("order_tag"), Value::Int(0));
}

/// **The conjunct is dropped from the append condition, not widened.** A dead slice
/// folded nothing, so conflicting on it would make the command lose to events it never
/// read: on this nine-kind queue that is nine times the conflict surface.
#[test]
fn a_dead_slice_leaves_the_append_condition() {
    let source = format!(
        "{QUEUE}command C(shop_id: Shop, order_id: Int) {{
  fold n = RetryAttempt {{ shop_id, order_id, kind: \"hold_place\" }}
  emit @a.done {{ n }}
}}
"
    );
    let program = program(&source);
    let mut interpreter = Interpreter::new(&program);
    failed(&mut interpreter, "hold.place.failed", 9, 2);
    failed(&mut interpreter, "hold.release.failed", 9, 7);

    let execution = interpreter
        .run(
            "C",
            [
                ("shop_id".to_string(), Value::Int(1)),
                ("order_id".to_string(), Value::Int(9)),
            ],
        )
        .expect("ran");

    let Outcome::Ok(events) = &execution.outcome else {
        panic!("expected the command to append");
    };
    assert_eq!(events[0].field("n"), Some(&Value::Int(2)));

    let read: Vec<String> = execution
        .condition
        .slices
        .iter()
        .map(|slice| slice.event.to_string())
        .collect();
    assert_eq!(
        read,
        ["@hold.place.failed"],
        "the release arm's slice folded nothing, so it is not in the condition"
    );
}

/// The left side is a **parameter**, which is a different kind of name from a field, and
/// the sigil is the whole of what says so. Both are legal when one name is both.
#[test]
fn the_two_sigils_name_two_different_things() {
    let source = "\
event @a.one { shop_id: Int, kind: String }

fold Both(shop_id: Int, kind: String) -> Int = 0 {
  on @a.one(shop_id, kind: kind) => 1
  on @a.one(shop_id, kind == \"other\") => 2
}
";
    let program = program(source);
    let fold = program.fold("Both").expect("declared");
    // The first arm narrows the event's own `kind` field; the second pins the key part
    // and puts nothing on the event at all.
    assert!(matches!(
        fold.stage.slices[0].filters[1],
        Filter::Field { .. }
    ));
    assert!(matches!(
        fold.stage.slices[1].filters[1],
        Filter::Key { .. }
    ));
}

#[test]
fn a_key_filter_names_a_parameter_and_not_anything_in_scope() {
    let source = "\
event @a.one { shop_id: Int }

fold F(shop_id: Int) -> acc: Int = 0 {
  on @a.one(shop_id, nope == \"x\") => acc + 1
}
";
    assert_eq!(error(source), "this fold has no parameter `nope`");
    assert!(
        hint(source).contains("`==` matches a parameter"),
        "{}",
        hint(source)
    );
}

/// A guard is not stored, so it has no key for a conjunct to name. Refused rather than
/// quietly accepted as a narrowing nobody could read back.
#[test]
fn a_guard_slice_has_no_key_to_match() {
    let source = "\
subject Shop(Int)
refusal No \"no\"
event @a.one { shop_id: Shop }
event @a.done { n: Int }

command C(shop_id: Shop, kind: String) {
  guard @a.one(shop_id, kind == \"x\")
  emit @a.done { n: 1 }
}
";
    assert_eq!(
        error(source),
        "`kind ==` names a key part, and a guard has no key"
    );
}

/// The right side resolves once in the caller's frame, so it cannot read the event. The
/// arm's binds are not in scope yet when a filter parses, which is what makes this an
/// ordinary "not in scope" rather than a rule of its own.
#[test]
fn a_key_filter_cannot_read_the_event_it_filters() {
    let source = "\
event @a.one { shop_id: Int, at: Timestamp }

fold F(shop_id: Int, day: String) -> Int = 0 {
  on @a.one(shop_id, day == at) => 1
}
";
    assert!(
        error(source).contains("`at` is not in scope"),
        "{}",
        error(source)
    );
}

/// The parameter's declared type is the target, so the ordinary check catches a value
/// that could never equal it.
#[test]
fn a_key_filters_value_is_typed_by_the_parameter() {
    let source = "\
event @a.one { shop_id: Int }

fold F(shop_id: Int, kind: Int) -> Int = 0 {
  on @a.one(shop_id, kind == \"x\") => 1
}
";
    assert_eq!(error(source), "expected Int, found String");
}

/// **The right side is a value, not a value of the row.** Whatever writes the row holds
/// the event and the key it is writing and no frame, so a conjunct reading a slot is a
/// key that depends on the key. Found by `/code-review`: the degenerate case is a
/// tautology, and `--tables` called the column storable on the strength of it while no
/// event supplied the part at all.
#[test]
fn a_key_filter_is_matched_against_a_value_and_not_a_slot() {
    let tautology = "\
subject Shop(Int)
event @a.one { shop_id: Shop }

fold Counted(shop_id: Shop, kind: String) -> n: Int = 0 {
  on @a.one(shop_id, kind == kind) => n + 1
}
";
    assert_eq!(
        error(tautology),
        "a key part cannot be matched against `kind`"
    );

    // The same hole with a value that is not a tautology, which is why the rule is about
    // reading a slot rather than about naming the part itself.
    let sibling = "\
subject Shop(Int)
event @a.one { shop_id: Shop, order_id: Int }

fold Counted(shop_id: Shop, order_id: Int, kind: Int) -> n: Int = 0 {
  on @a.one(shop_id, order_id, kind == order_id) => n + 1
}
";
    assert_eq!(
        error(sibling),
        "a key part cannot be matched against `order_id`"
    );

    // And the accumulator, which the same check catches where it is written rather than
    // at the stage's close.
    let accumulator = "\
event @a.one { shop_id: Int }

fold F(shop_id: Int, n: Int) -> acc: Int = 0 {
  on @a.one(shop_id, n == acc) => acc + 1
}
";
    assert_eq!(
        error(accumulator),
        "a key part cannot be matched against `acc`"
    );
}

/// Two conjuncts on one part cannot both hold unless they are the same value, so the arm
/// folds nothing at all while `--tables` still reports the column. An arm that can never
/// fire is a mistake rather than a narrowing.
#[test]
fn a_key_part_is_matched_once_per_arm() {
    let source = "\
subject Shop(Int)
event @a.one { shop_id: Shop }

fold Counted(shop_id: Shop, kind: String) -> n: Int = 0 {
  on @a.one(shop_id, kind == \"a\", kind == \"b\") => n + 1
}
";
    assert_eq!(error(source), "`kind` is matched twice in this arm");
}

/// An inline fold has no signature to declare a parameter in, which is a different
/// reason from a guard's and gets its own message: the first version of this blamed a
/// guard for a program with no guard in it.
#[test]
fn an_inline_fold_has_no_key_either_and_is_told_why() {
    let source = "\
subject Shop(Int)
event @a.one { shop_id: Shop }
event @a.done { n: Int }

command C(shop_id: Shop, kind: String) {
  fold n: Int = 0
    on @a.one(shop_id, kind == \"x\") => n + 1
  emit @a.done { n }
}
";
    assert_eq!(
        error(source),
        "`kind ==` names a key part, and an inline fold has no key"
    );
    assert!(
        hint(source).contains("no signature to declare one in"),
        "{}",
        hint(source)
    );
}

// ---------------------------------------------------------------------------
// Rule 11: the read a store fills a column from
// ---------------------------------------------------------------------------

const SPEND: &str = "subject Customer(Int)
event @order.placed { customer_id: Customer, total: Money(2) }

fold LifetimeSpend(customer_id: Customer) -> spend: Money(2) = 0 {
  on @order.placed(customer_id) { total } => spend + total
}
";

/// A sealed column, optional, which is the shape rule 12 gives an erased subject.
const EMAIL: &str = "subject Customer(Int)
event @order.placed { customer_id: Customer, email: String @subject(customer_id) }

fold LatestEmail(customer_id: Customer) -> Sealed(String, Customer)? = none {
  on @order.placed(customer_id) { email } => email
}
";

/// The same column declared non-optional, which is what `fold` has no value for once the
/// key is gone and `fold_stored` does not need one.
const EMAIL_PLAIN: &str = "subject Customer(Int)
event @order.placed { customer_id: Customer, email: String @subject(customer_id) }

fold LatestEmail(customer_id: Customer) -> Sealed(String, Customer) = \"\" {
  on @order.placed(customer_id) { email } => email
}
";

#[test]
fn a_stored_fold_stops_at_the_position_it_is_given() {
    let program = program(SPEND);
    let mut interpreter = Interpreter::new(&program);
    for _ in 0..3 {
        given(
            &mut interpreter,
            "order.placed",
            [
                ("customer_id", Value::Int(7)),
                ("total", Value::money(1_000, 2)),
            ],
        );
    }
    let key = || [("customer_id".to_string(), Value::Int(7))];

    assert_eq!(
        interpreter.fold("LifetimeSpend", key()).expect("folded"),
        Value::money(3_000, 2),
        "`fold` reads the log as it now stands"
    );
    // Inclusive, and positions count from zero, so this is the first event alone. A store
    // records one position with the table, and this is how every column reaches it.
    assert_eq!(
        interpreter
            .fold_stored("LifetimeSpend", key(), 0)
            .expect("folded"),
        Value::money(1_000, 2),
        "bounded at position 0, which is one event"
    );
    assert_eq!(
        interpreter
            .fold_stored("LifetimeSpend", key(), 1)
            .expect("folded"),
        Value::money(2_000, 2),
    );
}

#[test]
fn a_stored_fold_hands_sealed_content_through_rather_than_shredding_it() {
    let program = program(EMAIL);
    let mut interpreter = Interpreter::new(&program);
    given(
        &mut interpreter,
        "order.placed",
        [
            ("customer_id", Value::Int(7)),
            ("email", Value::str("buyer@example.com")),
        ],
    );
    interpreter.erase_subject("Customer", "7");
    let key = || [("customer_id".to_string(), Value::Int(7))];

    assert_eq!(
        interpreter.fold("LatestEmail", key()).expect("folded"),
        Value::none(Type::sealed(Type::String, "Customer".to_string())),
        "`fold` hands the value to whoever asked, so it shreds on the way out"
    );
    // The store holds the ciphertext and shreds on its own read, which is what makes an
    // erasure after the fill observable without refolding the column.
    let stored = interpreter
        .fold_stored("LatestEmail", key(), 0)
        .expect("folded");
    assert!(
        matches!(
            stored,
            Value::Opt {
                value: Some(ref held),
                ..
            } if matches!(**held, Value::Sealed { .. })
        ),
        "got {stored:?}"
    );
}

#[test]
fn filling_a_non_optional_sealed_column_survives_an_erasure_that_precedes_it() {
    let program = program(EMAIL_PLAIN);
    let mut interpreter = Interpreter::new(&program);
    given(
        &mut interpreter,
        "order.placed",
        [
            ("customer_id", Value::Int(7)),
            ("email", Value::str("buyer@example.com")),
        ],
    );
    interpreter.erase_subject("Customer", "7");
    let key = || [("customer_id".to_string(), Value::Int(7))];

    // There is no value that says "this is gone" at a non-optional position, so the read
    // raises rather than quietly holding a zero.
    let why = interpreter
        .fold("LatestEmail", key())
        .expect_err("the column is not optional");
    assert!(
        why.to_string().contains("LatestEmail"),
        "the message names the column: {why}"
    );
    // The fill must not inherit that. A projector in the same position stores its
    // ciphertext and raises at the query, and a stored column does the same.
    assert!(
        matches!(
            interpreter.fold_stored("LatestEmail", key(), 0),
            Ok(Value::Sealed { .. })
        ),
        "a column filled after its subject was erased holds what is on disk"
    );
}
