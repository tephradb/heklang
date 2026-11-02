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

/// The third rule from the same derivation. A column sits in the table its key is kept
/// for and a key part is named for a parameter, so a fold named after what it is keyed by
/// asks for one table with two columns of one name, and `--tables` printed exactly that.
#[test]
fn a_folds_column_cannot_be_named_after_a_part_of_its_own_key() {
    let subject = "\
subject Shop(Int)

event @a.one { shop_id: Shop }

fold Shop(shop_id: Shop) -> Bool = false {
  on @a.one(shop_id) => true
}
";
    assert_eq!(
        error(subject),
        "fold `Shop` derives a column called `shop`, and its key already has a part of that name"
    );
    assert!(
        hint(subject).contains("named for `shop_id`"),
        "the hint names the parameter the part came from, got {:?}",
        hint(subject)
    );

    // A plain part, where the name is the parameter with one `_id` taken off rather than
    // the subject's.
    let plain = "\
event @a.one { day: String }

fold Day(day: String) -> Bool = false {
  on @a.one(day) => true
}
";
    assert_eq!(
        error(plain),
        "fold `Day` derives a column called `day`, and its key already has a part of that name"
    );

    // And through the fallback two parts of one name take, which is the case that makes
    // this one derivation shared with `tables` rather than a second reading of the rules.
    let shared = "\
subject Region(Int)

event @a.one { from_id: Region, to_id: Region }

fold From(from_id: Region, to_id: Region) -> Bool = false {
  on @a.one(from_id, to_id) => true
}
";
    assert_eq!(
        error(shared),
        "fold `From` derives a column called `from`, and its key already has a part of that name"
    );

    // The author's fix, which is the one the other two rules already ask for: name the
    // fold for what the value is. A fold with no key has no part to collide with.
    program(
        "\
subject Shop(Int)

event @a.one { shop_id: Shop }

fold ShopInstalled(shop_id: Shop) -> Bool = false {
  on @a.one(shop_id) => true
}

fold Shop() -> Int = 0 {
  on @a.one() => 1
}
",
    );
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
    // A head and not a position: the number a store records is the one `Log::head` hands
    // it, so `1` is the first event alone and the arithmetic is not the caller's.
    assert_eq!(
        interpreter
            .fold_stored("LifetimeSpend", key(), 1)
            .expect("folded"),
        Value::money(1_000, 2),
        "a head of 1 is one event"
    );
    assert_eq!(
        interpreter
            .fold_stored("LifetimeSpend", key(), 2)
            .expect("folded"),
        Value::money(2_000, 2),
    );
    // The whole reason the bound is a head. An empty log has no position to pass, so an
    // inclusive bound left a store with nothing to fill a column from, and the only number
    // it had would have folded the first event into a column recorded as holding none of
    // it.
    assert_eq!(
        interpreter
            .fold_stored("LifetimeSpend", key(), 0)
            .expect("folded"),
        Value::money(0, 2),
        "a head of 0 is the seed"
    );
    assert_eq!(
        interpreter
            .fold_stored("LifetimeSpend", key(), 3)
            .expect("folded"),
        interpreter.fold("LifetimeSpend", key()).expect("folded"),
        "the head of a log this long is what `fold` reads to"
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
        .fold_stored("LatestEmail", key(), 1)
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
            interpreter.fold_stored("LatestEmail", key(), 1),
            Ok(Value::Sealed { .. })
        ),
        "a column filled after its subject was erased holds what is on disk"
    );
}

/// Rule 14: a key part computed from the record, which is the third way an arm can write
/// one and the second way a fold becomes query-only.
#[test]
fn an_arm_can_compute_a_key_part_from_the_record() {
    let source = "\
event @usage.recorded { shop_id: Int, at: Timestamp, units: Int }

fold UsageCounted(shop_id: Int, month: String) -> n: Int = 0 {
  on @usage.recorded(shop_id) { at, units } key month = \"{at.year()}\" => n + units
}
";
    program(source);

    // It makes the fold query-only, for the reason the envelope binding does: a computed
    // key is per record and a slice is narrowed once, so a command has no query to read
    // it with and nothing to conflict on. The message names the part.
    let read = format!(
        "{source}
command C(shop_id: Int, month: String) {{
  fold n = UsageCounted {{ shop_id, month }}
  emit @usage.recorded {{ shop_id, at: \"2026-01-01T00:00:00Z\", units: n }}
}}
"
    );
    assert_eq!(
        error(&read),
        "`UsageCounted` computes the key part `month`, so it can be queried but not folded by a command"
    );
}

#[test]
fn a_computed_key_part_is_refused_where_there_is_no_key() {
    // An inline fold is not stored, so it has no key to write a part of. A guard's slice
    // has no destructure and no `=>`, so the clause cannot be written there at all.
    let inline = "\
event @a.one { id: Int, n: Int }

command C(id: Int) {
  fold total: Int = 0
    on @a.one(id) { n } key id = n => total + n
  emit @a.one { id, n: total }
}
";
    assert_eq!(
        error(inline),
        "`key` names a key part, and an inline fold has no key"
    );
}

#[test]
fn a_computed_key_part_names_a_parameter_and_not_a_subject() {
    let missing = "\
event @a.one { id: Int, n: Int }

fold F(id: Int) -> Int = 0 {
  on @a.one(id) { n } key bucket = n => n
}
";
    assert_eq!(error(missing), "this fold has no parameter `bucket`");

    // A subject's ids are minted and filed by the key store, so a computed one would
    // address a row under a key nothing issued while `KeyPart::subject` reported it as
    // that subject all the same.
    let subject = "\
subject Shop(Int)

event @a.one { shop_id: Shop, n: Int }

fold F(shop_id: Shop) -> Int = 0 {
  on @a.one() { n } key shop_id = shop_id => n
}
";
    assert_eq!(
        error(subject),
        "`shop_id` keys the subject `Shop`, so it is read through a fold or not computed at all"
    );
}

/// One value per part per arm, whichever of the three ways it is written, for the reason
/// two rule 13 conjuncts on one part are refused: both cannot hold, so the arm folds
/// nothing while `--tables` still calls the column storable.
#[test]
fn a_key_part_is_written_once_per_arm_however_it_is_written() {
    let source = "\
event @a.one { id: Int, kind: String, n: Int }

fold F(id: Int, kind: String) -> Int = 0 {
  on @a.one(id, kind == \"x\") { n } key kind = \"y\" => n
}
";
    assert_eq!(error(source), "`kind` is matched twice in this arm");

    // A field filter is refused too, and by the rule about what a filter may read rather
    // than by this one, because a filter writes a key part by its **value**: the next
    // test has that half, and the two together are "whichever of the three ways wrote it".
    let field = "\
event @a.one { id: Int, kind: String, n: Int }

fold F(id: Int, kind: String) -> Int = 0 {
  on @a.one(id, kind) { n } key kind = \"y\" => n
}
";
    assert_eq!(
        error(field),
        "`kind` is a key part this arm computes, so the filter on `kind` cannot read it"
    );
}

/// Rule 14's fifth refusal, and the one the other four were hiding: a computed part may
/// not read the key it is computing or the accumulator.
///
/// **It is the two drivers disagreeing, which is the one thing rule 14 rests on.** A
/// computed part decides which row the record belongs to, so the sink evaluates it before
/// there is a key to bind the parameters from and before there is a row to read the
/// accumulator out of; a by-key read has both in its frame and answers. So both shapes
/// below used to check clean and then fail a `fill` with `slot read before it was set`,
/// which is a message about the interpreter rather than about the program.
#[test]
fn a_computed_key_part_reads_the_record_and_nothing_of_the_key() {
    let accumulator = "\
event @usage.recorded { shop_id: Int, units: Int }

fold UsageCounted(shop_id: Int, bucket: Int) -> n: Int = 0 {
  on @usage.recorded(shop_id) { units } key bucket = n => n + units
}
";
    assert_eq!(
        error(accumulator),
        "`n` is the accumulator, so a computed key part cannot read it"
    );

    // A parameter, including one this same arm pins some other way. Binding the pinned
    // parts before computing the rest is a feature and not this one: it would make what a
    // part may read depend on what else the arm pinned and on the order two computed parts
    // were written in, where this rule is one sentence.
    let part = "\
event @usage.recorded { shop_id: Int, units: Int }

fold UsageCounted(shop_id: Int, tag: String, bucket: Int) -> n: Int = 0 {
  on @usage.recorded(shop_id, tag == \"a\") { units } key bucket = tag.len() => n + units
}
";
    assert_eq!(
        error(part),
        "`tag` is a part of the key this arm is computing, so a computed key part cannot read it"
    );
}

/// The same rule read the other way, which both drivers need.
///
/// A filter resolves **once** for the whole slice, against the key its caller asked for,
/// and a computed part is per record with no such key, so a filter reading one has
/// nothing to resolve against. `(label: month)` writes the part twice over, since
/// `tables::pins` reads a filter's value rather than its name; `(label: month.upper())`
/// pins nothing, so the column stayed storable while the filter quietly narrowed the log
/// to the records whose field already equalled what the part computed and dropped every
/// other one from the read model.
///
/// It is also what lets the sink check an arm's narrowing before it evaluates a computed
/// part, which is `tests/sink.rs`'s half of this: with it refused, no filter the sink has
/// to resolve depends on a part it has not computed yet.
#[test]
fn a_filter_cannot_read_a_key_part_the_arm_computes() {
    let source = |filter: &str| {
        format!(
            "\
event @usage.recorded {{ shop_id: Int, label: String, units: Int }}

fold UsageCounted(shop_id: Int, month: String) -> n: Int = 0 {{
  on @usage.recorded(shop_id, {filter}) {{ units, label }} key month = label.upper() => n + units
}}
"
        )
    };
    let wanted = "`month` is a key part this arm computes, so the filter on `label` cannot read it";
    assert_eq!(error(&source("label: month")), wanted);
    assert_eq!(error(&source("label: month.upper()")), wanted);

    // The shorthand is the same case, because `(kind)` is `(kind: kind)` and its value is
    // the parameter.
    let shorthand = "\
event @a.one { id: Int, kind: String, n: Int }

fold F(id: Int, kind: String) -> Int = 0 {
  on @a.one(id, kind) { n } key kind = \"y\" => n
}
";
    assert_eq!(
        error(shorthand),
        "`kind` is a key part this arm computes, so the filter on `kind` cannot read it"
    );

    // **And a filter is about the field, so sharing a name with a parameter is nothing.**
    // `(month: 3)` pins no part: the filter narrows an event field that happens to be
    // spelled like one. Comparing names refused this, which is an ordinary program.
    let alike = "\
event @usage.recorded { shop_id: Int, month: Int, at: Timestamp }

fold Counted(shop_id: Int, month: String) -> n: Int = 0 {
  on @usage.recorded(shop_id, month: 3) { at } key month = \"{at.year()}\" => n + 1
}
";
    program(alike);
}

/// An arm that computes a key part is checked before the keys a range covers are asked
/// for, so a conjunct reading a parameter the arm leaves **open** is the one it could not
/// check. The sink would reach the computed part first and run it for records the arm
/// never folds.
#[test]
fn a_filter_of_a_computing_arm_cannot_read_an_unpinned_parameter() {
    let source = "\
event @usage.recorded { shop_id: Int, label: Int, units: Int }

fold Counted(shop_id: Int, order_id: Int, bucket: Int) -> n: Int = 0 {
  on @usage.recorded(shop_id, label: order_id + 1) { units } key bucket = units => n + 1
  on @usage.recorded(shop_id, order_id) => n
}
";
    assert_eq!(
        error(source),
        "the filter on `label` reads `order_id`, which no conjunct of this arm pins"
    );

    // Pinned by the parameter alone, which is what makes it a key part rather than a
    // range, so the sink has it from the record.
    let pinned = "\
event @usage.recorded { shop_id: Int, label: Int, units: Int }

fold Counted(shop_id: Int, order_id: Int, bucket: Int) -> n: Int = 0 {
  on @usage.recorded(shop_id, label: order_id) { units } key bucket = units => n + 1
}
";
    program(pinned);
}

#[test]
fn a_computed_key_part_may_read_everything_the_record_offers() {
    // The other side of the refusal above, and a comprehension is why it is written as a
    // deny-list: the binding it allocates is a slot of the fold's frame like any other,
    // so an allow-list of the record's own names would refuse one.
    let source = "\
const OFFSET: Int = 100

event @usage.recorded { shop_id: Int, units: Int, tags: List(String) }

fn month_of(at: Timestamp) -> String {
  return \"{at.year()}-{at.month().pad(2)}\"
}

fold UsageCounted(shop_id: Int, month: String) -> n: Int = 0 {
  on @usage.recorded as e (shop_id) { units } key month = month_of(e.at) => n + units
}

fold TaggedUnits(shop_id: Int, bucket: Int) -> n: Int = 0 {
  on @usage.recorded(shop_id) { units, tags }
    key bucket = units + OFFSET + [t.len() for t in tags].len() => n + units
}
";
    let program = program(source);
    let mut interpreter = Interpreter::new(&program);
    given(
        &mut interpreter,
        "usage.recorded",
        [
            ("shop_id", Value::Int(1)),
            ("units", Value::Int(5)),
            (
                "tags",
                Value::list(Type::String, [Value::str("ab"), Value::str("c")]),
            ),
        ],
    );
    // The by-key driver, which is half of what checking clean is worth: the refused
    // shapes checked clean and answered here too, and failed only in the sink.
    // `tests/sink.rs` runs this same program through both and asserts they agree, which
    // is the half that catches what this one cannot.
    assert_eq!(
        interpreter
            .fold(
                "TaggedUnits",
                [("shop_id", Value::Int(1)), ("bucket", Value::Int(107))]
            )
            .expect("the bucket is 5 + 100 + 2"),
        Value::Int(5)
    );
}

/// **A key is plaintext, and this is the one position inside an arm where a seal is
/// wrong.** Rule 12 lets an arm fold sealed content into a sealed column, which is why
/// the ordinary seal check stands down inside a fold, and erasure empties a column but
/// cannot reach a primary key: a row filed under decrypted content would be personal data
/// nothing could erase.
///
/// It also could not run. The seal is a `Value::Sealed` at the sink, which will not reduce
/// to a key, and the by-key read compares the plaintext it was asked for against the
/// ciphertext and folds nothing. So `hek check --tables` called the column stored, `fill`
/// raised `malformed ir`, and a `fold` read answered the seed.
#[test]
fn a_computed_key_part_cannot_be_sealed_content() {
    let source = "\
subject Customer(Int)

event @order.placed { order_id: Int, customer_id: Customer, email: String @subject(customer_id) }

fold Placed(customer_id: Customer, bucket: String) -> n: Int = 0 {
  on @order.placed(customer_id) { email, order_id } key bucket = email => n + order_id
}
";
    assert_eq!(
        error(source),
        "a key part is plaintext, and this is content sealed under `Customer`"
    );

    // **At every position inside the expression, not only over its result.** Checking the
    // result type alone was the first answer and a `fn`'s declared return type launders
    // the subject away: this checked clean, `--tables` reported the column stored, and a
    // by-key read compared the plaintext it was asked for against the ciphertext and
    // answered the seed.
    let laundered = "\
subject Customer(Int)

event @order.placed { order_id: Int, customer_id: Customer, email: String @subject(customer_id) }

fn tag(s: String) -> String { return s }

fold Placed(customer_id: Customer, bucket: String) -> n: Int = 0 {
  on @order.placed(customer_id) { email, order_id } key bucket = tag(email) => n + order_id
}
";
    assert_eq!(
        error(laundered),
        "a key part is plaintext, and this is content sealed under `Customer`"
    );

    // The same content as a column of its own is rule 7 and stays legal, which is the fix
    // the hint names: the seal crosses the signature as its own type.
    let column = "\
subject Customer(Int)

event @order.placed { order_id: Int, customer_id: Customer, email: String @subject(customer_id) }

fold LatestEmail(customer_id: Customer) -> Sealed(String, Customer)? = none {
  on @order.placed(customer_id) { email } => email
}
";
    program(column);
}

/// Rule 15: a key part read out of **another named fold**, as it stood when the record
/// being folded was appended.
///
/// The spelling is the one a command uses to read a fold, which is the point: `GameGenre {
/// game_id }` means the same thing in both places. To tree-sitter it is a `record_literal`
/// either way, so the grammar needed no rule and the parser decides, which it may because
/// a lookup is reachable only inside a `key` clause.
const KEYED: &str = "\
event @game.added { game_id: Int, genre: String }
event @game.purchased { receipt_id: Int, game_id: Int }
event @game.refunded { receipt_id: Int }

fold GameGenre(game_id: Int) -> String = \"unknown\" {
  on @game.added(game_id) { genre } => genre
}

fold ReceiptGame(receipt_id: Int) -> Int = 0 {
  on @game.purchased(receipt_id) { game_id } => game_id
}
";

#[test]
fn a_key_part_can_be_read_through_another_fold() {
    let source = format!(
        "{KEYED}
fold GenreUnits(genre: String) -> n: Int = 0 {{
  on @game.purchased() {{ game_id }} key genre = GameGenre {{ game_id }} => n + 1
  on @game.refunded() {{ receipt_id }}
    key genre = GameGenre {{ game_id: ReceiptGame {{ receipt_id }} }} => n - 1
}}
"
    );
    let program = program(&source);

    // The chain is two lookups in evaluation order, inner first, because each is recorded
    // after its own arguments are parsed.
    let fold = program.fold("GenreUnits").expect("declared");
    let chain = &fold.stage.slices[1].lookups;
    assert_eq!(
        chain.iter().map(|at| at.fold.as_str()).collect::<Vec<_>>(),
        vec!["ReceiptGame", "GameGenre"]
    );

    // It makes the fold query-only, as the envelope and a computed part do, and the
    // message names the lookup because it is the sharper reason: it is *why* the part
    // cannot be had without reading the log.
    let read = format!(
        "{source}
command C(genre: String) {{
  fold n = GenreUnits {{ genre }}
  emit @game.refunded {{ receipt_id: n }}
}}
"
    );
    assert_eq!(
        error(&read),
        "`GenreUnits` keys through `GameGenre`, so it can be queried but not folded by a command"
    );
}

#[test]
fn only_a_stored_fold_can_be_keyed_through() {
    // A lookup is answered from the looked-up fold's own column, so a fold no column is
    // kept for has nothing to answer with. Checked over the whole program, because what
    // makes a fold storable is that *some* arm of it supplies each key part.
    let source = format!(
        "{KEYED}
fold Since(at: Timestamp) -> Int = 0 {{
  on @game.added() => 1
}}

fold Odd(n: Int) -> Int = 0 {{
  on @game.added() {{ game_id }} key n = Since {{ at: \"2020-01-01T00:00:00Z\" }} => game_id
}}
"
    );
    assert_eq!(
        error(&source),
        "`Since` is not stored, so a key cannot be read through it: no event supplies `at`, so nothing keeps a column for it"
    );
}

#[test]
fn a_lookup_chain_cannot_come_back_round() {
    let source = "\
event @a.b { x: Int, y: Int }

fold A(x: Int) -> Int = 0 {
  on @a.b() { y } key x = B { y } => y
}

fold B(y: Int) -> Int = 0 {
  on @a.b() { x } key y = A { x } => x
}
";
    assert_eq!(
        error(source),
        "`A` keys through `B` keys through `A`: a fold keyed through itself has no value to key by"
    );

    // A fold looking itself up is the one-link case and lands in the same walk.
    let itself = "\
event @a.b { x: Int, y: Int }

fold A(x: Int) -> Int = 0 {
  on @a.b() { y } key x = A { x: y } => y
}
";
    assert_eq!(
        error(itself),
        "`A` keys through `A`: a fold keyed through itself has no value to key by"
    );

    // **Each link says which kind it is.** A cycle made only of rule 16 value reads has no
    // `key` clause anywhere in it, and saying "keys through" sent the reader looking for
    // one; a cycle with any key link keeps the sharper verb and the sharper reason.
    let values = "\
event @a.b { x: Int }

fold A(x: Int) -> n: Int = 0 {
  on @a.b(x) { x } => if ((B { x }) > 0) { n } else { n + 1 }
}

fold B(x: Int) -> n: Int = 0 {
  on @a.b(x) { x } => if ((A { x }) > 0) { n } else { n + 1 }
}
";
    assert_eq!(
        error(values),
        "`A` reads `B` reads `A`: a fold that reads itself has no value to read"
    );

    let mixed = "\
event @a.b { x: Int, y: Int }

fold A(x: Int) -> n: Int = 0 {
  on @a.b() { x, y } key x = B { x: y } => n + 1
}

fold B(x: Int) -> n: Int = 0 {
  on @a.b(x) { x } => if ((A { x }) > 0) { n } else { n + 1 }
}
";
    assert_eq!(
        error(mixed),
        "`A` keys through `B` reads `A`: a fold keyed through itself has no value to key by"
    );
}

#[test]
fn a_looked_up_fold_may_not_return_sealed_content() {
    // The argument rule 14 makes for a computed part, one step along: a key is plaintext
    // and an erase cannot reach one. Refused on the **declared return type**, which is
    // where a fold publishes its seal (rule 7), so there is no `fn` to launder it through.
    let source = "\
subject Customer(Int)

event @order.placed { order_id: Int, customer_id: Customer, email: String @subject(customer_id) }

fold LatestEmail(customer_id: Customer) -> Sealed(String, Customer)? = none {
  on @order.placed(customer_id) { email } => email
}

fold ByEmail(email: String) -> n: Int = 0 {
  on @order.placed() { customer_id } key email = LatestEmail { customer_id } => n + 1
}
";
    assert_eq!(
        error(source),
        "`LatestEmail` returns content sealed under `Customer`, so a key part cannot be read through it"
    );
}

#[test]
fn a_lookup_is_only_a_lookup_where_a_record_is_in_hand() {
    // A **filter** stays refused, and structurally: a filter resolves once for the whole
    // slice and is parsed before the list that collects a lookup exists, so the name is
    // not a value there at all. Everywhere outside a fold arm the same silence keeps
    // `Name { .. }` the record literal it has always been.
    let filter = format!(
        "{KEYED}
fold Odd(game_id: Int) -> n: Int = 0 {{
  on @game.purchased(game_id, receipt_id: GameGenre {{ game_id }}) => n + 1
}}
"
    );
    assert_eq!(error(&filter), "`GameGenre` is not in scope");

    // An **inline fold's arm** too, for the reason rule 14 refuses a `key` clause there:
    // it is not stored, so it is inside the command that could not then read it.
    let inline = format!(
        "{KEYED}
command C(game_id: Int) {{
  fold total: Int = 0
    on @game.purchased(game_id) => total + GameGenre {{ game_id }}.len()
  emit @game.refunded {{ receipt_id: total }}
}}
"
    );
    assert_eq!(error(&inline), "`GameGenre` is not in scope");

    // And its arguments are checked as a command's read of the same fold is.
    let wrong = format!(
        "{KEYED}
fold Odd(genre: String) -> n: Int = 0 {{
  on @game.purchased() {{ receipt_id }} key genre = GameGenre {{ receipt_id }} => n + 1
}}
"
    );
    assert_eq!(
        error(&wrong),
        "fold `GameGenre` has no parameter `receipt_id`"
    );

    let missing = format!(
        "{KEYED}
fold Odd(genre: String) -> n: Int = 0 {{
  on @game.purchased() {{ receipt_id }} key genre = GameGenre {{ }} => n + 1
}}
"
    );
    assert_eq!(error(&missing), "fold `GameGenre` needs `game_id`");
}

/// Rule 14's rule, inherited: **a lookup's arguments read the record and nothing of the
/// key.** The check has to ask for them separately, because `fold_lookup` leaves an
/// `Expr::Load` of the slot behind and keeps the arguments off the expression tree, so the
/// walk over the key expression sees nothing of them.
///
/// It is the most natural shape there is, which is why it mattered: passing the key part
/// the arm already pins. It checked clean and failed the fill.
#[test]
fn a_lookups_arguments_read_the_record_and_nothing_of_the_key() {
    let source = |args: &str| {
        format!(
            "\
event @game.added {{ shop_id: Int, game_id: Int, genre: String }}
event @game.purchased {{ shop_id: Int, game_id: Int }}

fold GameGenre(shop_id: Int, game_id: Int) -> String = \"unknown\" {{
  on @game.added(shop_id, game_id) {{ genre }} => genre
}}

fold GenreUnits(shop_id: Int, genre: String) -> n: Int = 0 {{
  on @game.purchased(shop_id) {{ {args} }} key genre = GameGenre {{ shop_id, game_id }}
    => n + 1
}}
"
        )
    };
    assert_eq!(
        error(&source("game_id")),
        "`shop_id` is a part of this fold's key, so `GameGenre` cannot be looked up by it"
    );

    // And the fix the hint names: destructure the field, so the lookup reads the record.
    // The filter still narrows, and the arm still pins the part from it.
    program(&source("shop_id, game_id"));
}

/// A lookup is **hoisted** out of the key expression onto the slice and evaluated once per
/// record, which is what lets one chain into another. A binder's name does not exist at
/// that point, so a lookup under one is refused rather than read before its argument is
/// set.
#[test]
fn a_lookup_cannot_be_written_inside_a_comprehension() {
    let source = "\
event @a.b { x: Int, ids: List(Int) }

fold One(x: Int) -> Int = 0 {
  on @a.b(x) => 1
}

fold Counted(p: Int) -> n: Int = 0 {
  on @a.b() { ids } key p = [One { x: i } for i in ids].len() => n + 1
}
";
    assert_eq!(
        error(source),
        "`One` is read once per record, so it cannot be read inside a comprehension"
    );

    // Outside the comprehension it is an ordinary lookup, and the comprehension may read
    // the slot it filled.
    let outside = "\
event @a.b { x: Int, ids: List(Int) }

fold One(x: Int) -> Int = 0 {
  on @a.b(x) => 1
}

fold Counted(p: Int) -> n: Int = 0 {
  on @a.b() { x, ids } key p = One { x } + [i for i in ids].len() => n + 1
}
";
    program(outside);
}

/// Rule 16: a **sealed** result is allowed in an arm's value, where rule 15 refuses one in
/// a key.
///
/// A key is plaintext and an erase cannot reach one; an arm's value lands in a column,
/// which `docs/projectors.md` rule 9 makes an erase empty. So a looked-up seal is ordinary
/// sealed content and rule 7 takes it from there, with no code of its own: the slot carries
/// the declared `Sealed(..)`, so the arm's value has that type and the fold has to declare
/// it. That rule 7's refusal is the one that fires is the argument for allowing it at all.
#[test]
fn a_looked_up_seal_may_be_a_value_if_the_fold_declares_it() {
    let source = |ret: &str| {
        format!(
            "\
subject Customer(Int)

event @order.placed {{ order_id: Int, customer_id: Customer, email: String @subject(customer_id) }}
event @order.shipped {{ order_id: Int, customer_id: Customer }}

fold LatestEmail(customer_id: Customer) -> Sealed(String, Customer)? = none {{
  on @order.placed(customer_id) {{ email }} => email
}}

fold ShippedTo(order_id: Int) -> {ret} = none {{
  on @order.shipped(order_id) {{ customer_id }} => LatestEmail {{ customer_id }}
}}
"
        )
    };
    assert_eq!(
        error(&source("String?")),
        "`ShippedTo` folds content sealed under `Customer` on @order.shipped, and its return type does not say so"
    );

    // Declared, it is a sealed column like any other, and the key it is filed under names
    // no subject: rule 7's usual shape.
    let program = program(&source("Sealed(String, Customer)?"));
    let layout = heklang::layout(&program);
    let (table, column) = layout.column("ShippedTo").expect("kept");
    assert_eq!(table.name, "order");
    assert_eq!(column.reads, vec!["LatestEmail".to_string()]);
    assert_eq!(
        column.ty,
        Type::opt(Type::sealed(Type::String, "Customer")),
        "the column carries the seal, so an erase can empty it"
    );
}

/// **Rule 16's gate widened the sink's, so it had to widen the parser's too.**
///
/// The sink checks an arm's narrowing before it evaluates a computed part *or* a lookup,
/// and it may check the whole predicate only because the parser has refused the conjuncts
/// it could not resolve. That refusal ran for a `key` clause alone, so an arm with a value
/// lookup and a filter reading an unpinned parameter slipped past it: `hek check --tables`
/// called the column storable and the fill aborted with `slot read before it was set`,
/// which is the two-drivers-disagree failure these rules exist to prevent.
#[test]
fn an_arm_that_reads_a_fold_is_held_to_the_filter_rule_too() {
    let source = "\
subject Shop(Int)

event @order.placed { shop_id: Shop, order_id: Int }
event @shop.swept { shop_id: Shop, mark: Int }

fold Flag(shop_id: Shop, order_id: Int) -> Bool = false {
  on @order.placed(shop_id, order_id) => true
}

fold Counter(shop_id: Shop, order_id: Int) -> n: Int = 0 {
  on @order.placed(shop_id, order_id) => n + 1
  on @shop.swept(shop_id, mark: order_id + 0) { shop_id, mark }
    => if (Flag { shop_id, order_id: mark }) { n } else { n + 1 }
}
";
    assert_eq!(
        error(source),
        "the filter on `mark` reads `order_id`, which no conjunct of this arm pins"
    );

    // Without the lookup the same arm is fine, because `folded` resolves that conjunct
    // per ranged key and the sink never has to get ahead of it.
    let plain = source.replace(
        "if (Flag { shop_id, order_id: mark }) { n } else { n + 1 }",
        "if (mark > 0) { n } else { n + 1 }",
    );
    program(&plain);
}

/// A named fold is spelled like a type, so it resolves like one: `Name { .. }` reads the
/// fold inside a fold arm and builds the record everywhere else, and one name meaning both
/// would make one declaration read two ways. Refused where it is declared, because a use
/// site is not where the author can fix it.
#[test]
fn a_named_fold_cannot_share_a_name_with_a_record_or_an_enum() {
    let shape = |kind: &str| {
        format!(
            "\
subject Shop(Int)

{kind}

event @order.placed {{ shop_id: Shop, order_id: Int }}

fold Tally(shop_id: Shop, order_id: Int) -> Int = 0 {{
  on @order.placed(shop_id, order_id) => 1
}}
"
        )
    };
    assert_eq!(
        error(&shape("record Tally { n: Int, m: Int }")),
        "`Tally` is already the name of a record"
    );
    assert_eq!(
        error(&shape("enum Tally { @default One, Two }")),
        "`Tally` is already the name of an enum"
    );
}

/// **An arm leaves an empty filter list out, and a guard does not.** Both spellings of an
/// arm are one slice, so the IR and the digest cannot tell them apart; `hek fmt` writes the
/// shorter one. The port measured the shape at 59 of 270 arms, every one in a fold no
/// command reads, where the slice is not an append condition and `()` claims nothing.
///
/// A guard keeps the list for the grammar's reason rather than taste's: a guard's slice is
/// the whole statement, so a `(` after it is the start of the next statement just as
/// readily as its own list, and `(x).len()` is one.
#[test]
fn an_arm_leaves_an_empty_filter_list_out_and_a_guard_does_not() {
    let arm = |slice: &str| {
        format!(
            "\
event @game.purchased {{ game_id: Int }}

fold Units(game_id: Int) -> n: Int = 0 {{
  on {slice} => n + 1
}}
"
        )
    };
    // One slice, two spellings, and nothing downstream can tell: the filter list is the
    // same empty vector either way.
    let bare = program(&arm("@game.purchased { game_id }"));
    let empty = program(&arm("@game.purchased() { game_id }"));
    assert_eq!(
        format!("{:?}", bare.fold("Units").expect("declared").stage.slices),
        format!("{:?}", empty.fold("Units").expect("declared").stage.slices),
    );

    // An inline fold's arm shares the loop, so it leaves it out too.
    program(
        "\
event @shop.connected { shop_id: Int, token: String }
event @shop.pinged { shop_id: Int }

command Ping(shop_id: Int) {
  fold token: String? = none
    on @shop.connected { token } => token

  emit @shop.pinged { shop_id }
}
",
    );

    // And a guard is refused, with its own reason rather than an arm's.
    let guard = "\
event @shop.connected { shop_id: Int, token: String }
event @shop.pinged { shop_id: Int }

command Ping(shop_id: Int) {
  guard @shop.connected
  emit @shop.pinged { shop_id }
}
";
    assert_eq!(
        error(guard),
        "a guard's slice keeps its filter list, so `@shop.connected` needs one"
    );
    assert!(
        hint(guard).contains("the next one may start with `(`"),
        "the hint gives the grammar's reason: {}",
        hint(guard)
    );
    program(&guard.replace("@shop.connected\n", "@shop.connected()\n"));
}

/// Rule 15 **keys through** the fold it reads and rule 16 just **reads** it, and the
/// query-only refusal says which. It said "keys through" for both, which sent a reader
/// looking for a `key` clause that a value lookup does not have.
#[test]
fn the_query_only_refusal_tells_a_key_lookup_from_a_value_one() {
    const PRELUDE: &str = "\
event @game.wishlisted { user_id: Int, game_id: Int }
event @game.purchased { user_id: Int, game_id: Int }

fold Wished(user_id: Int, game_id: Int) -> Bool = false {
  on @game.wishlisted(user_id, game_id) => true
}
";
    let caller = |reader: &str, args: &str| {
        format!(
            "{PRELUDE}
{reader}
command Buy(game_id: Int, user_id: Int) {{
  fold seen = Reader {{ {args} }}
  emit @game.purchased {{ user_id, game_id: seen }}
}}
"
        )
    };

    // Rule 16: the arm's value reads the fold, and no key part comes from it.
    assert_eq!(
        error(&caller(
            "\
fold Reader(game_id: Int) -> n: Int = 0 {
  on @game.purchased(game_id) { user_id, game_id } =>
    if (Wished { user_id, game_id }) { n } else { n + 1 }
}
",
            "game_id"
        )),
        "`Reader` reads `Wished`, so it can be queried but not folded by a command"
    );

    // Rule 15: the key part itself is read through it, which is the sharper reason and
    // the one a fold with both should name.
    assert_eq!(
        error(&caller(
            "\
fold Reader(game_id: Int, wished: Bool) -> n: Int = 0 {
  on @game.wishlisted(game_id, wished == true) { user_id, game_id } =>
    if (Wished { user_id, game_id }) { n } else { n + 1 }
  on @game.purchased(game_id) { user_id, game_id }
    key wished = Wished { user_id, game_id } => n + 1
}
",
            "game_id, wished: true"
        )),
        "`Reader` keys through `Wished`, so it can be queried but not folded by a command"
    );
}

/// **Several paths on one arm are the arms written out**, not merely equivalent to them.
/// The arm's tokens are read once per path so each slice allocates its own bind slots,
/// which is what keeps `hek digest` from calling a rewrite a change: the port has 53 folds
/// with a `@game.purchased` arm and a `@game.gifted` arm identical apart from the path.
#[test]
fn several_paths_on_one_arm_lower_to_the_arms_written_out() {
    let program = program(
        "\
event @game.purchased { game_id: Int, price_cents: Int }
event @game.gifted { game_id: Int, price_cents: Int }

fold GameGross(game_id: Int) -> cents: Int = 0 {
  on @game.purchased, @game.gifted (game_id) { price_cents } => cents + price_cents
}
",
    );
    let fold = program.fold("GameGross").expect("declared");
    let events: Vec<String> = fold
        .stage
        .slices
        .iter()
        .map(|slice| slice.event.to_string())
        .collect();
    assert_eq!(events, ["@game.purchased", "@game.gifted"]);
    // Its own slot per slice, which is the half that makes the digest agree.
    let slots: Vec<_> = fold
        .stage
        .slices
        .iter()
        .flat_map(|slice| &slice.binds)
        .map(|bind| bind.slot)
        .collect();
    assert_eq!(slots.len(), 2, "one bind per slice");
    assert_ne!(slots[0], slots[1], "and a slot of its own");
}

/// A name the arm uses has to be a field of every path, with the same type, `@subject` and
/// `absent`, which is `common_fields` and the check a projector handler already passes.
/// Listing a path twice is refused rather than deduplicated.
#[test]
fn an_arms_paths_must_share_every_name_it_uses() {
    let arm = |events: &str, body: &str| {
        format!(
            "\
{events}
fold F(game_id: Int) -> n: Int = 0 {{
  on {body} => n + 1
}}
"
        )
    };
    let two = "\
event @game.purchased { game_id: Int, price_cents: Int }
event @game.gifted { game_id: Int, wrapped: Bool }
";
    assert_eq!(
        error(&arm(
            two,
            "@game.purchased, @game.gifted (game_id) { price_cents }"
        )),
        "`price_cents` is not shared by @game.purchased, @game.gifted, so an arm listing them cannot name it"
    );
    assert_eq!(
        error(&arm(two, "@game.purchased, @game.purchased (game_id)")),
        "this arm already lists @game.purchased"
    );
    // A filter says the same thing about the same mistake, and names the paths: `def` is
    // their intersection, so naming only the first blamed an event that has the field.
    assert_eq!(
        error(&arm(two, "@game.purchased, @game.gifted (price_cents: 1)")),
        "`price_cents` is not shared by @game.purchased, @game.gifted, so an arm listing them cannot name it"
    );
    // And a name on no path at all still gets the ordinary answer.
    assert_eq!(
        error(&arm(two, "@game.purchased, @game.gifted (nothing: 1)")),
        "@game.purchased, @game.gifted has no field `nothing`"
    );

    // The shared `game_id` is enough for an arm that names only it, and an arm that names
    // nothing at all is fine even when the paths share no field: a fold may just count.
    program(&arm(two, "@game.purchased, @game.gifted (game_id)"));
    program(&arm(
        "\
event @a.failed { why: String }
event @b.failed { code: Int }
",
        "@a.failed, @b.failed",
    ));
}

/// And the same field under a different `@subject` is not the same field, so an arm cannot
/// bind one name that is sealed on one path and plain on the other.
#[test]
fn an_arms_paths_must_agree_on_the_subject_too() {
    let source = "\
subject Customer(Int)

event @a.seen { game_id: Int, customer_id: Customer, note: String @subject(customer_id) }
event @b.seen { game_id: Int, note: String }

fold F(game_id: Int) -> String = \"\" {
  on @a.seen, @b.seen (game_id) { note } => note
}
";
    assert_eq!(
        error(source),
        "`note` is not shared by @a.seen, @b.seen, so an arm listing them cannot name it"
    );
}

/// A repeat pass reports nothing twice. The arm is read once per path, so a diagnostic
/// from its body would be reported once per path; `common_fields` is what makes dropping
/// the repeats sound, since every pass is handed the same field set and so reaches the
/// same diagnostics. A type mismatch in an arm's value is a note rather than a stop, which
/// is how it got as far as being reported twice.
#[test]
fn a_multi_path_arm_reports_its_diagnostics_once() {
    let source = "\
event @game.purchased { game_id: Int, name: String }
event @game.gifted { game_id: Int, name: String }

fold Bad(game_id: Int) -> Int = 0 {
  on @game.purchased, @game.gifted (game_id) { name } => name
}
";
    let found = heklang::check_files([("a.hk", source)]).expect_err("this source is meant to fail");
    let mismatches = found
        .iter()
        .filter(|at| at.code == heklang::Code::TypeMismatch)
        .count();
    assert_eq!(mismatches, 1, "{found:#?}");
}

/// **Rule 17: a fold may not invent a subject id.** A literal is the one way a
/// subject-typed value appears from nothing: every other route is already refused by the
/// type, and what makes a literal different is that it resolves against a subject so no
/// position has to learn a coercion rule. An inlined `const` is a literal node too.
///
/// It was found while checking the invariant rule 18 wanted to rest on, that a subject id
/// can only come from an event field of that type. It cannot, and a seed is where the port
/// was leaning on it: a `""` seed on a looked-up fold hands a sale a developer nobody
/// announced.
#[test]
fn a_fold_cannot_invent_a_subject_id() {
    let source = |fold: &str| {
        format!(
            "\
subject Developer(Int)

event @game.announced {{ game_id: Int, developer_id: Developer }}

{fold}
"
        )
    };
    assert_eq!(
        error(&source(
            "fold Seeded(game_id: Int) -> Developer = 7 {\n  on @game.announced(game_id) { developer_id } => developer_id\n}"
        )),
        "`Developer` is a subject, so this seed cannot be a written id"
    );
    assert_eq!(
        error(&source(
            "fold Armed(game_id: Int) -> Developer? = none {\n  on @game.announced(game_id) => 99\n}"
        )),
        "`Developer` is a subject, so this arm's value cannot be a written id"
    );
    // An inline fold's seed is the same position.
    assert_eq!(
        error(&source(
            "event @noise { game_id: Int }\n\ncommand C(game_id: Int) {\n  fold who: Developer = 7\n    on @game.announced(game_id) { developer_id } => developer_id\n\n  emit @noise { game_id }\n}"
        )),
        "`Developer` is a subject, so this seed cannot be a written id"
    );

    // The honest spelling, and the three positions this does **not** reach.
    program(&source(
        "fold Honest(game_id: Int) -> Developer? = none {\n  on @game.announced(game_id) { developer_id } => developer_id\n}",
    ));
    // A filter compares records rather than writing one, so a literal narrows and invents
    // nothing.
    program(&source(
        "fold Filtered(game_id: Int) -> n: Int = 0 {\n  on @game.announced(game_id, developer_id: 7) => n + 1\n}",
    ));
    // And a command may write one: that is where ids enter the system, its caller may send
    // the same number, and the key store mints a key for whatever first appears.
    program(&source(
        "command C(game_id: Int) {\n  emit @game.announced { game_id, developer_id: 42 }\n}",
    ));
}

/// Rule 17 asks about the value's **result** positions, not about every literal under it.
/// Walking the whole subtree refused four programs that write no subject at all, including
/// the honest spelling the rule itself recommends. An `if` is the one expression whose
/// result is somewhere else, so its arms are the recursion.
#[test]
fn rule_17_asks_what_the_value_writes_and_not_what_it_reads() {
    let source = |fold: &str| {
        format!(
            "\
subject Shop(Int)
subject Developer(Int)

const HOUSE: Shop = 1

event @game.announced {{ game_id: Int, shop_id: Shop, developer_id: Developer }}

fn label(shop_id: Shop) -> String {{
  return \"{{shop_id}}\"
}}

{fold}
"
        )
    };
    // A comparison, a `fn` argument, and an `if` whose arms are both honest.
    program(&source(
        "fold HouseCount(game_id: Int) -> n: Int = 0 {\n  on @game.announced(game_id) { shop_id } => if shop_id == 5 { n + 1 } else { n }\n}",
    ));
    program(&source(
        "fold Labelled(game_id: Int) -> String = \"\" {\n  on @game.announced(game_id) { shop_id } => label(HOUSE) \n}",
    ));
    program(&source(
        "fold NotHouse(game_id: Int) -> Developer? = none {\n  on @game.announced(game_id) { developer_id } =>\n    if developer_id == 1 { none } else { developer_id }\n}",
    ));

    // And an `if` arm that *does* write one is still refused, which is why the recursion
    // has to go through an `if` rather than stop at it.
    assert_eq!(
        error(&source(
            "fold Sneaky(game_id: Int) -> Developer? = none {\n  on @game.announced(game_id) { developer_id } =>\n    if developer_id == 1 { 99 } else { developer_id }\n}"
        )),
        "`Developer` is a subject, so this arm's value cannot be a written id"
    );

    // The subject named is the one at the offending node. It came from an arbitrary map
    // value before, so this file reported `Shop` or `Developer` depending on the run; a
    // `HashMap`'s order is fixed within a process, so this pins the right answer rather
    // than reproducing the old flakiness, which one run cannot.
    assert_eq!(
        error(&source(
            "fold Armed(game_id: Int) -> Developer? = none {\n  on @game.announced(game_id, shop_id: 5) => 99\n}"
        )),
        "`Developer` is a subject, so this arm's value cannot be a written id"
    );
}

/// **Rules 18 and 19 are one feature.** A subject key part may be read through a fold, and
/// `else skip` is what makes that reachable: rule 17 leaves a subject-returning fold no
/// legal non-optional seed (a literal is refused and every other route is refused by the
/// type), so such a fold is always `T? = none` and a key part is always absent sometimes.
///
/// Rule 17 is also what makes rule 18 sound. A read of a stored fold is an id something
/// claimed, where a computed one is an id nothing did, and the port's `""` seed was the
/// hole: it handed every unannounced game the same invented developer.
#[test]
fn a_subject_key_part_is_read_through_a_fold_with_an_explicit_skip() {
    let source = |arm: &str| {
        format!(
            "\
subject Developer(Int)

event @game.announced {{ game_id: Int, developer_id: Developer }}
event @game.purchased {{ game_id: Int, dev: Developer, price_cents: Int }}

fn pick(d: Developer?) -> Developer? {{
  return d
}}

fold GameDeveloper(game_id: Int) -> Developer? = none {{
  on @game.announced(game_id) {{ developer_id }} => developer_id
}}

fold DeveloperGross(developer_id: Developer) -> cents: Int = 0 {{
  {arm}
}}
"
        )
    };
    // The whole shape, which is what the port converts to.
    let program = program(&source(
        "on @game.purchased { game_id, price_cents }\n    key developer_id = GameDeveloper { game_id } else skip => cents + price_cents",
    ));
    let key = program
        .fold("DeveloperGross")
        .expect("declared")
        .stage
        .slices
        .first()
        .expect("one arm")
        .keys
        .first()
        .expect("one computed part");
    assert!(key.skip, "the part carries its own `else skip`");

    // A part computed any other way is refused: an expression over a lookup is a
    // derivation again, and a field of the event is one nothing claimed as a key.
    for arm in [
        "on @game.purchased { dev, price_cents } key developer_id = dev => cents + price_cents",
        "on @game.purchased { game_id, price_cents }\n    key developer_id = pick(GameDeveloper { game_id }) else skip => cents + price_cents",
    ] {
        assert_eq!(
            error(&source(arm)),
            "`developer_id` keys the subject `Developer`, so it is read through a fold or not computed at all"
        );
    }

    // Without `else skip` the optional is refused as it always was, which is the type
    // rule rather than a new one: a key is present or it is not a key.
    assert_eq!(
        error(&source(
            "on @game.purchased { game_id, price_cents }\n    key developer_id = GameDeveloper { game_id } => cents + price_cents"
        )),
        "expected Developer, found Developer?"
    );
}

/// `else skip` has to be able to fire, for the reason rule 3 refuses an accumulator's name
/// that no arm reads: a word with no meaning is one a reader has to work out is meaningless.
/// It is allowed on any computed part and not only on a lookup, because both are the same
/// question to both drivers and refusing one would cost a check rather than save one.
#[test]
fn else_skip_belongs_on_a_part_that_can_be_absent() {
    let source = |keys: &str| {
        format!(
            "\
event @game.purchased {{ game_id: Int, note: String?, price_cents: Int }}

fn bucket(note: String?) -> String? {{
  return note
}}

fold Monthly(month: String) -> cents: Int = 0 {{
  on @game.purchased {{ note, price_cents }} key {keys} => cents + price_cents
}}
"
        )
    };
    // A computed part, no lookup in sight, and it skips.
    program(&source("month = bucket(note) else skip"));
    assert_eq!(
        error(&source("month = \"2026-01\" else skip")),
        "`month` is never absent here, so `else skip` cannot fire"
    );
    // And an absent value with no `else skip` is the ordinary type rule.
    assert_eq!(
        error(&source("month = bucket(note)")),
        "expected String, found String?"
    );
}

/// **The two routes a written id reached the log by anyway.** Rule 17 asked only about an
/// enumerated set of pass-through expressions and about literals in this arena, and both
/// assumptions were wrong: `unwrap_or`'s argument is a result position, and a `fn` may
/// declare the subject itself, so its `return 42` is a literal at a subject-typed position
/// in the function's own arena and this walk never sees it.
///
/// Both put a row in the read model keyed by an id no event carried, which is the whole
/// hole rules 17 and 18 exist to close: a key store mints a key for whatever id first
/// appears in an event, so an id a fold makes up is one no key was ever minted for, and an
/// erasure that reaches every row the subject owns still leaves it.
#[test]
fn a_fold_cannot_launder_a_subject_id_through_a_method_or_a_fn() {
    let source = |arm: &str| {
        format!(
            "\
subject Developer(Int)

event @game.announced {{ game_id: Int, developer_id: Developer? }}

fn house() -> Developer {{
  return 42
}}

fn passthru(d: Developer?) -> Developer? {{
  return d
}}

fold Held(game_id: Int) -> Developer? = none {{
  on @game.announced(game_id) {{ developer_id }} => {arm}
}}
"
        )
    };
    assert_eq!(
        error(&source("developer_id.unwrap_or(99)")),
        "`Developer` is a subject, so this arm's value cannot be a written id"
    );
    assert_eq!(
        error(&source("house()")),
        "`Developer` is a subject, so this arm's value cannot be a `fn`'s result"
    );
    // A `fn` that only passes one through is refused too, and deliberately: this walk
    // cannot see into a body, so a result it cannot account for is one it will not take.
    // Over-strict in the safe direction, and inlining the read is the fix.
    assert_eq!(
        error(&source("passthru(developer_id)")),
        "`Developer` is a subject, so this arm's value cannot be a `fn`'s result"
    );

    // What the record gives is still fine, through a method as much as directly.
    program(&source("developer_id"));
    program(&source(
        "if developer_id.is_some() { developer_id } else { none }",
    ));
}

/// `else skip` is looked for past the value, so an `if` in the value must not end the
/// scan: returning at the first `else` read `key m = if c { X } else { Y } else skip` as
/// having no skip, parsed the value at the wrong position, and gave three spurious
/// mismatches and an `expected =>`.
#[test]
fn an_if_in_a_key_parts_value_does_not_hide_its_else_skip() {
    program(
        "\
event @game.purchased { game_id: Int, note: String?, price_cents: Int }

fn bucket(note: String?) -> String? {
  return note
}

fold Monthly(month: String) -> cents: Int = 0 {
  on @game.purchased { note, game_id, price_cents }
    key month = if game_id > 1 { bucket(note) } else { none } else skip => cents + price_cents
}
",
    );
}

/// **A box is not a wall.** Rule 17 pruned its walk on "is this node's own type the
/// subject", so a record or container literal, whose own type is the record or the
/// container, was never entered and a written id inside one escaped: `Pick { who: 42 }.who`
/// checked clean and a `fill` then filed a row under developer 42, an id no event claimed
/// and no key was minted for. Which is the whole of what rule 17 exists to prevent.
///
/// The walk is entered on **containment** and accuses on **identity**, so a comparison
/// inside a subject-typed subtree is still pruned and the honest spellings still check.
#[test]
fn rule_17_looks_inside_a_box_that_holds_the_subject() {
    let source = |decls: &str| {
        format!(
            "\
subject Game(Int)
subject Developer(Int)

event @game.announced {{ game_id: Game, developer_id: Developer }}

record Pick {{
  who: Developer,
}}

record Box {{
  ids: List(Developer),
}}

{decls}
"
        )
    };
    for (what, arm) in [
        ("a record field", "Pick { who: 42 }.who"),
        ("a list inside a record", "Box { ids: [42] }.ids.first()"),
    ] {
        let message = error(&source(&format!(
            "fold Through(game_id: Game) -> Developer? = none {{\n  on @game.announced(game_id) {{ game_id }} => {arm}\n}}"
        )));
        assert_eq!(
            message, "`Developer` is a subject, so this arm's value cannot be a written id",
            "for {what}"
        );
    }

    // A `fn` whose result merely *holds* the subject is refused too, because this walk
    // cannot see into one either: its `42` is a literal in the function's own arena.
    assert_eq!(
        error(&source(
            "fn made() -> Pick {\n  return Pick { who: 42 }\n}\n\nfold Through(game_id: Game) -> Developer? = none {\n  on @game.announced(game_id) { game_id } => made().who\n}"
        )),
        "`Developer` is a subject, so this arm's value cannot be a `fn`'s result"
    );

    // And the honest spelling through the same box still checks: the id came out of a
    // field of the record, and a literal compared against is not a literal written.
    program(&source(
        "fold Honest(game_id: Game) -> Developer? = none {\n  on @game.announced(game_id) { developer_id } => Pick { who: developer_id }.who\n}\n\nfold Counted(game_id: Game) -> n: Int = 0 {\n  on @game.announced(game_id) { developer_id } => if developer_id == 5 { n + 1 } else { n }\n}",
    ));
}

/// **The declared type is asked by containment too**, which is the other half of the box
/// finding and was missed with it: the check returned early unless the *declared* type was
/// itself a subject, so a fold declaring a record or a list that holds one skipped rule 17
/// outright and stored an id no event claimed.
///
/// And the walk is over **positions** rather than over nodes. `[developer_id, 42]` takes
/// its element type from the first element, so the `42` after it is lowered as a plain
/// `Int` and resolved against nothing: asking the node refused nothing, and chained into
/// rule 18 a `fill` keyed a row by developer 42. The position answers where the node
/// cannot.
#[test]
fn rule_17_asks_the_position_a_literal_sits_at() {
    let source = |decls: &str| {
        format!(
            "\
subject Game(Int)
subject Developer(Int)
subject Publisher(Int)

event @game.announced {{ game_id: Game, developer_id: Developer }}

record Snap {{
  who: Developer,
  n: Int,
}}

record Pair {{
  dev: Developer,
  owner: Publisher,
}}

{decls}
"
        )
    };
    for (what, fold) in [
        (
            "a record the fold declares",
            "fold Latest(game_id: Game) -> Snap? = none {\n  on @game.announced(game_id) { game_id } => Snap { who: 42, n: 1 }\n}",
        ),
        (
            "a list the fold declares",
            "fold Ids(game_id: Game) -> List(Developer) = [] {\n  on @game.announced(game_id) { game_id } => [99]\n}",
        ),
        (
            "an element typed by its neighbour",
            "fold GameDeveloper(game_id: Game) -> Developer? = none {\n  on @game.announced(game_id) { developer_id } =>\n    [developer_id, 42].remove(developer_id).first()\n}",
        ),
    ] {
        let message = error(&source(fold));
        assert!(
            message.contains("is a subject, so this arm's value cannot be a written id"),
            "for {what}: {message}"
        );
    }

    // The subject named is the one the offending **position** is, not the first the type
    // holds: a record with two cannot be checked against one of them.
    assert_eq!(
        error(&source(
            "fold Both(game_id: Game) -> Pair? = none {\n  on @game.announced(game_id) { developer_id } =>\n    Pair { dev: developer_id, owner: 7 }\n}"
        )),
        "`Publisher` is a subject, so this arm's value cannot be a written id"
    );

    // And an honest value through every one of those shapes still checks. The `n: 1` field
    // is the case a rule that refused every literal under a subject-holding value would
    // have broken.
    program(&source(
        "fold Latest(game_id: Game) -> Snap? = none {\n  on @game.announced(game_id) { developer_id } => Snap { who: developer_id, n: 1 }\n}\n\nfold Ids(game_id: Game) -> List(Developer) = [] {\n  on @game.announced(game_id) { developer_id } => [developer_id]\n}",
    ));
}
