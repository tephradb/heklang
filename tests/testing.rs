//! `docs/testing.md` as executable tests, one test per numbered rule.
//!
//! These test the test construct: each case parses a small program that declares its
//! own `test`s, runs them, and asserts on the verdicts.

use std::cell::Cell;
use std::rc::Rc;

use heklang::{
    Error, Event, Harness, Ident, Key, Reply, Row, Rows, TestOutcome, TestResult, World, parse,
    run_tests, run_tests_in,
};

const PRELUDE: &str = "event @plan.created { plan_id: Int, title: String, price: Money(2) }
event @plan.sold { plan_id: Int, price: Money(2) }
event @plan.deleted { plan_id: Int }
event @plan.synced { plan_id: Int }

refusal Already \"already synced\"
command RecordSync(plan_id: Int) {
  guard @plan.synced(plan_id)
  fold synced: Bool = false
    on @plan.synced(plan_id) => true
  if synced {
    reject Already
  }
  if plan_id < 0 {
    invalid \"a plan id is not negative\"
  }
  emit @plan.synced { plan_id }
}

projector Plans {
  entity Plan { plan_id: Int @key, title: String, sold: Int }
  entity Sales { plan_id: Int @key, revenue: Money(2) }

  on @plan.created { plan_id, title } { put Plan { plan_id, title, sold: 0 } }
  on @plan.deleted { plan_id } { delete Plan[plan_id] }
  on @plan.sold { plan_id, price } {
    update Plan[plan_id] { sold: .sold + 1 }
    patch Sales[plan_id] { revenue: .revenue + price }
  }
}
";

const EFFECTS: &str = "subject Shop(Int)
event @shop.connected { shop_id: Shop, domain: String, token: String @subject(shop_id) }
event @shop.sync.requested { shop_id: Shop }

effect SyncShop {
  on @shop.sync.requested { @key shop_id } {
    fold domain: String = \"\"
      on @shop.connected(shop_id) { domain } => domain
    fold token: String? = none
      on @shop.connected(shop_id) { token } => token

    let secret = reveal(token)
    if secret.is_none() {
      log(\"shop {shop_id} has never connected\")
      return
    }
    http.post(\"https://{domain}/sync\", { \"shop\": shop_id })
    // A literal, because a shop id is not a plan id and the types say so now. What this
    // fixture is here for is an effect reaching `invoke` at all.
    invoke RecordSync { plan_id: 1 }
  }
}
";

/// A subject with a parent, a projector that stores a column sealed under the child,
/// and an effect that reveals the same content. Two ways to reach one seal, which is
/// what rule 3's `erased` has to answer the same way for both.
const SEALED: &str = "subject Shop(Int)
subject Buyer(Int) under Shop
event @contact.recorded {
  shop_id: Shop,
  buyer_id: Buyer,
  email: String? @subject(buyer_id),
  note: String @subject(buyer_id),
  at: Timestamp,
}

projector Contacts {
  entity Contact {
    buyer_id: Buyer @key,
    email: String?,
    at: Timestamp,
  }

  on @contact.recorded { buyer_id, email, at } { put Contact { buyer_id, email, at } }
}

projector Required {
  entity Note {
    buyer_id: Buyer @key,
    note: String,
  }

  on @contact.recorded { buyer_id, note } { put Note { buyer_id, note } }
}

effect Mail {
  on @contact.recorded { @key buyer_id } {
    fold email: String? = none
      on @contact.recorded(buyer_id) { email } => email

    let to = reveal(email)
    if to.is_none() {
      return
    }
    http.post(\"https://mail.example/send\", { \"to\": to })
  }
}
";

/// Rule 2's fixture: an event wide enough for an override to be about something, a
/// bound on one field so the `@max` cases have somewhere to land, and a command that
/// appends one so a `run` has an expectation to match.
const FIXTURES: &str = "event @item.added { item_id: Int, note: String @max(8), tag: String }

command AddItem(item_id: Int, tag: String) {
  emit @item.added { item_id, note: \"\", tag }
}

fn t_item(item_id: Int) -> @item.added {
  return @item.added { item_id: item_id, note: \"\", tag: \"plain\" }
}

fn t_long(item_id: Int) -> @item.added {
  return @item.added { item_id: item_id, note: \"far too long\", tag: \"plain\" }
}

fn a_name(item_id: Int) -> String {
  return \"item {item_id}\"
}
";

/// Parses `PRELUDE` plus `body` and runs every test in it.
fn verdicts(body: &str) -> Vec<TestResult> {
    let source = format!("{PRELUDE}\n{body}");
    let program = parse(&source).unwrap_or_else(|err| panic!("expected this to parse: {err}"));
    run_tests(&program)
}

/// The same, with the effect declarations in scope too.
fn effect_verdicts(body: &str) -> Vec<TestResult> {
    let source = format!("{PRELUDE}\n{EFFECTS}\n{body}");
    let program = parse(&source).unwrap_or_else(|err| panic!("expected this to parse: {err}"));
    run_tests(&program)
}

/// The same, against the sealed-column declarations.
fn sealed_verdicts(body: &str) -> Vec<TestResult> {
    let source = format!("{PRELUDE}\n{SEALED}\n{body}");
    let program = parse(&source).unwrap_or_else(|err| panic!("expected this to parse: {err}"));
    run_tests(&program)
}

fn only(results: &[TestResult]) -> &TestResult {
    assert_eq!(results.len(), 1, "expected one test");
    &results[0]
}

fn why(result: &TestResult) -> String {
    match &result.outcome {
        TestOutcome::Passed => panic!("expected this test to fail, it passed"),
        TestOutcome::Failed(why) => why.clone(),
        TestOutcome::Errored(why) => panic!("expected a mismatch, got an error: {why}"),
    }
}

fn err(body: &str) -> String {
    let source = format!("{PRELUDE}\n{EFFECTS}\n{body}");
    parse(&source)
        .expect_err("expected this test declaration to be rejected")
        .text()
}

/// The same pair, against the fixture declarations.
fn fixture_verdicts(body: &str) -> Vec<TestResult> {
    let source = format!("{PRELUDE}\n{FIXTURES}\n{body}");
    let program = parse(&source).unwrap_or_else(|err| panic!("expected this to parse: {err}"));
    run_tests(&program)
}

fn fixture_err(body: &str) -> String {
    let source = format!("{PRELUDE}\n{FIXTURES}\n{body}");
    parse(&source)
        .expect_err("expected this test declaration to be rejected")
        .text()
}

fn errored(result: &TestResult) -> String {
    match &result.outcome {
        TestOutcome::Errored(why) => why.clone(),
        other => panic!("expected an error, got {other:?}"),
    }
}

// Rule 1: shape.

#[test]
fn a_test_is_a_declaration_like_any_other() {
    let results = verdicts(
        "test \"a created plan starts at zero sold\" {
  given @plan.created { plan_id: 1, title: \"Cover\", price: 19.99 }
  project Plans
  expect Plan[1] { title: \"Cover\", sold: 0 }
}",
    );
    assert!(only(&results).passed(), "{}", only(&results));
    assert_eq!(only(&results).name, "a created plan starts at zero sold");
}

#[test]
fn two_tests_may_not_share_a_name() {
    let message = err(
        "test \"same\" { run RecordSync { plan_id: 1 } expect @plan.synced { plan_id: 1 } }
test \"same\" { run RecordSync { plan_id: 2 } expect @plan.synced { plan_id: 2 } }",
    );
    assert_eq!(message, "test \"same\" is declared twice");
}

/// Order is irrelevant for a test exactly as it is for every other declaration, and a
/// test above the command it names is the case that proves the extra pass is real.
#[test]
fn a_test_may_be_declared_above_what_it_names() {
    let source = "test \"the command runs\" {
  run Later { n: 1 }
  expect @a.b { n: 1 }
}

event @a.b { n: Int }

command Later(n: Int) {
  emit @a.b { n }
}
";
    let program = parse(source).expect("a test names rather than declares");
    let results = run_tests(&program);
    assert!(only(&results).passed(), "{}", only(&results));
}

// Rule 2: `given`.

#[test]
fn given_builds_the_log_in_the_order_written() {
    let results = verdicts(
        "test \"the second sale counts too\" {
  given @plan.created { plan_id: 1, title: \"Cover\", price: 19.99 }
  given @plan.sold { plan_id: 1, price: 19.99 }
  given @plan.sold { plan_id: 1, price: 19.99 }
  project Plans
  expect Plan[1] { sold: 2 }
  expect Sales[1] { revenue: 39.98 }
}",
    );
    assert!(only(&results).passed(), "{}", only(&results));
}

#[test]
fn a_given_event_is_written_whole() {
    let message = err("test \"partial\" {
  given @plan.created { plan_id: 1 }
  project Plans
  expect no Plan[1]
}");
    assert_eq!(
        message,
        "`given @plan.created` needs `title`; an event is written whole"
    );
}

/// Rule 2: a `fn` is how a test gets a helper, with no test-only construct for it.
#[test]
fn a_test_value_may_call_a_fn() {
    let source = "event @a.b { label: String }

fn label(n: Int) -> String {
  return \"plan {n}\"
}

command Name(label: String) {
  emit @a.b { label }
}

test \"a helper builds the value\" {
  run Name { label: label(7) }
  expect @a.b { label: \"plan 7\" }
}
";
    let program = parse(source).expect("a fn call is an ordinary expression");
    assert!(only(&run_tests(&program)).passed());
}

/// Rule 2's rejected alternative, enforced: a test cannot append except through `given`.
#[test]
fn a_test_cannot_emit() {
    let message = err("test \"emitting\" {
  emit @plan.synced { plan_id: 1 }
  project Plans
}");
    assert_eq!(
        message,
        "a test writes its log with `given`, which appends the event directly"
    );
}

// Rule 2, the other half: a `fn` may make the whole event.

#[test]
fn a_fixture_gives_the_whole_event() {
    let results = fixture_verdicts(
        "test \"a fixture is a log line\" {
  given t_item(1)
  run AddItem { item_id: 2, tag: \"plain\" }
  expect @item.added { item_id: 2, note: \"\", tag: \"plain\" }
}",
    );
    assert!(only(&results).passed(), "{}", only(&results));
}

#[test]
fn one_fixture_called_twice_is_two_events() {
    let results = fixture_verdicts(
        "test \"two orders\" {
  given t_item(1)
  given t_item(2)
  run AddItem { item_id: 3, tag: \"plain\" }
  expect t_item(3)
}",
    );
    assert!(only(&results).passed(), "{}", only(&results));
}

#[test]
fn an_override_replaces_only_the_fields_it_names() {
    let results = fixture_verdicts(
        "test \"an override\" {
  given t_item(1) { tag: \"gift\", note: \"short\" }
  run AddItem { item_id: 1, tag: \"plain\" }
  expect t_item(1)
}",
    );
    assert!(only(&results).passed(), "{}", only(&results));
}

#[test]
fn an_expectation_can_be_a_fixture_with_an_override() {
    let results = fixture_verdicts(
        "test \"an expectation\" {
  run AddItem { item_id: 4, tag: \"gift\" }
  expect t_item(4) { tag: \"gift\" }
}",
    );
    assert!(only(&results).passed(), "{}", only(&results));
}

/// A fixture names every field, so what an expectation compares is still the whole
/// event: the field the override did not correct is the one reported.
#[test]
fn a_fixture_expectation_reports_the_field_that_differed() {
    let results = fixture_verdicts(
        "test \"a mismatch\" {
  run AddItem { item_id: 4, tag: \"gift\" }
  expect t_item(4)
}",
    );
    assert_eq!(
        why(only(&results)),
        "@item.added.tag: expected \"plain\", got \"gift\""
    );
}

/// The shape the 26-field and 24-field pair in the port wants: one fixture standing on
/// another. It is why the `return` dispatch keys on the token rather than on the
/// declared type.
#[test]
fn a_fixture_can_be_built_from_another_fixture() {
    let results = fixture_verdicts(
        "fn t_gift(item_id: Int) -> @item.added {
  return t_item(item_id)
}

test \"delegation\" {
  given t_gift(1)
  run AddItem { item_id: 1, tag: \"plain\" }
  expect t_gift(1)
}",
    );
    assert!(only(&results).passed(), "{}", only(&results));
}

#[test]
fn an_override_names_at_least_one_field() {
    let message = fixture_err(
        "test \"empty\" {
  given t_item(1) {}
  project Plans
}",
    );
    assert_eq!(
        message,
        "an override names at least one field; `{}` changes nothing, so write the call on its own"
    );
}

#[test]
fn an_override_is_checked_against_the_event() {
    assert_eq!(
        fixture_err("test \"unknown\" {\n  given t_item(1) { nope: 1 }\n  project Plans\n}"),
        "@item.added has no field `nope`"
    );
    assert_eq!(
        fixture_err(
            "test \"twice\" {\n  given t_item(1) { tag: \"a\", tag: \"b\" }\n  project Plans\n}"
        ),
        "`tag` is given twice"
    );
}

#[test]
fn a_given_takes_an_event_and_not_any_other_answer() {
    assert_eq!(
        fixture_err("test \"a string\" {\n  given a_name(1)\n  project Plans\n}"),
        "`a_name` returns a String, and `given` takes an event; a fixture declares `-> @some.event` and returns one"
    );
    assert_eq!(
        fixture_err("test \"nothing declared\" {\n  given nope(1)\n  project Plans\n}"),
        "`nope` is not a declared `fn`"
    );
}

/// The action decides which expectations are legal, and neither of the other two ever
/// reaches the event branch.
#[test]
fn a_fixture_expectation_belongs_to_a_run() {
    assert!(
        fixture_err("test \"projected\" {\n  project Plans\n  expect t_item(1) { tag: \"a\" }\n}")
            .contains("projector `Plans` has no entity `t_item`"),
    );
    assert!(
        fixture_err("test \"delivered\" {\n  deliver SyncShop\n  expect t_item(1)\n}")
            .contains("effect `SyncShop` is not declared"),
    );
}

/// A `given` used to skip the bound an `emit` enforces, so a test could state a world
/// the runtime could not produce. It errors rather than fails: the case could not say
/// what it was about, which is not the same as asserting the wrong thing.
#[test]
fn a_given_past_a_max_cannot_state_its_world() {
    let results = fixture_verdicts(
        "test \"too long\" {
  given @item.added { item_id: 1, note: \"far too long\", tag: \"a\" }
  project Plans
}",
    );
    assert_eq!(
        errored(only(&results)),
        "`given @item.added`: note is 12 characters, the most allowed is 8"
    );
}

/// The bound is asked of the event that is actually appended, which is the merged one.
/// So an override can correct a fixture that is past it, and can introduce one.
#[test]
fn the_bound_is_checked_after_the_override() {
    let fixed = fixture_verdicts(
        "test \"corrected\" {
  given t_long(1) { note: \"short\" }
  run AddItem { item_id: 1, tag: \"plain\" }
  expect t_item(1)
}",
    );
    assert!(only(&fixed).passed(), "{}", only(&fixed));

    let broken = fixture_verdicts(
        "test \"introduced\" {
  given t_item(1) { note: \"far too long\" }
  project Plans
}",
    );
    assert_eq!(
        errored(only(&broken)),
        "`given @item.added`: note is 12 characters, the most allowed is 8"
    );

    let from_fixture = fixture_verdicts(
        "test \"from the fixture\" {
  given t_long(1)
  project Plans
}",
    );
    assert_eq!(
        errored(only(&from_fixture)),
        "`given @item.added`: note is 12 characters, the most allowed is 8"
    );
}

// Rule 3: setup.

#[test]
fn erased_makes_a_shredded_key_writable() {
    let results = effect_verdicts(
        "test \"a shredded key skips terminally\" {
  given @shop.connected { shop_id: 3, domain: \"three.example\", token: \"shpat\" }
  given @shop.sync.requested { shop_id: 3 }
  erased Shop \"3\"
  deliver SyncShop
  expect skipped
}",
    );
    assert!(only(&results).passed(), "{}", only(&results));
}

/// `erased` is a property of the world and not of the action taken in it. A projector
/// **moves** sealed content into a column without ever revealing it
/// (`docs/projectors.md` rule 9), so a read model holds the only copy of the personal
/// data outside the log, and an erase that stopped at the decrypt boundary would leave
/// it there to read. The two tests differ in one line, which is the point.
#[test]
fn a_shredded_key_empties_the_sealed_column_a_projection_wrote() {
    let results = sealed_verdicts(
        "test \"the read model has it too\" {
  given @contact.recorded { shop_id: 1, buyer_id: 7, email: \"ada@example.com\", note: \"n\", at: \"2019-12-31T12:00:00Z\" }
  erased Buyer \"7\"
  project Contacts
  expect Contact[7] { buyer_id: 7, email: none, at: \"2019-12-31T12:00:00Z\" }
}",
    );
    assert!(only(&results).passed(), "{}", only(&results));
}

#[test]
fn a_shredded_key_skips_the_effect_that_would_reveal_it() {
    let results = sealed_verdicts(
        "test \"the decrypt boundary has it\" {
  given @contact.recorded { shop_id: 1, buyer_id: 7, email: \"ada@example.com\", note: \"n\", at: \"2019-12-31T12:00:00Z\" }
  erased Buyer \"7\"
  deliver Mail
  expect skipped
}",
    );
    assert!(only(&results).passed(), "{}", only(&results));
}

/// A child's key is wrapped in its parent's, so one row delete takes every key beneath
/// it (`docs/effects.md` rule 12). The sibling shop is what says this is the wrapping
/// and not "any erase empties every column of that type".
#[test]
fn erasing_a_parent_empties_a_column_sealed_under_the_child() {
    let results = sealed_verdicts(
        "test \"erasing the shop takes its buyers\" {
  given @contact.recorded { shop_id: 1, buyer_id: 7, email: \"ada@example.com\", note: \"n\", at: \"2019-12-31T12:00:00Z\" }
  given @contact.recorded { shop_id: 2, buyer_id: 8, email: \"bob@example.com\", note: \"n\", at: \"2019-12-31T12:00:00Z\" }
  erased Shop \"1\"
  project Contacts
  expect Contact[7] { email: none }
  expect Contact[8] { email: \"bob@example.com\" }
}",
    );
    assert!(only(&results).passed(), "{}", only(&results));
}

/// The property the design rests on: a reader of the read model cannot tell a subject
/// that was erased from one whose optional was never written. If it could, the row
/// would still be saying something about the person it no longer holds anything about.
#[test]
fn a_column_never_written_reads_back_as_a_shredded_one() {
    let results = sealed_verdicts(
        "test \"absent and erased are one column\" {
  given @contact.recorded { shop_id: 1, buyer_id: 7, email: none, note: \"n\", at: \"2019-12-31T12:00:00Z\" }
  given @contact.recorded { shop_id: 1, buyer_id: 8, email: \"ada@example.com\", note: \"n\", at: \"2019-12-31T12:00:00Z\" }
  erased Buyer \"8\"
  project Contacts
  expect Contact[7] { email: none }
  expect Contact[8] { email: none }
}",
    );
    assert!(only(&results).passed(), "{}", only(&results));
}

/// The one column that cannot answer. An emptied column holds the absent optional,
/// which is the value rule 12 gives an erased subject; a required column has no such
/// value, so the row is refused rather than written holding either the plaintext or a
/// zero that reads like data. The same thing `@absent` is told where it is written.
#[test]
fn a_required_sealed_column_cannot_hold_a_shredded_key() {
    let results = sealed_verdicts(
        "test \"a required column has no absent\" {
  given @contact.recorded { shop_id: 1, buyer_id: 7, email: none, note: \"ada\", at: \"2019-12-31T12:00:00Z\" }
  erased Buyer \"7\"
  project Required
  expect Note[7] { note: \"ada\" }
}",
    );
    let TestOutcome::Errored(why) = &only(&results).outcome else {
        panic!("expected an error, got {}", only(&results));
    };
    assert!(
        why.contains("`Note.note` holds content sealed under `Buyer` = `7`"),
        "got: {why}"
    );
    assert!(why.contains("Declare the column optional"), "got: {why}");
}

/// The cascade belongs to the key store, not to the setup line: an effect's own
/// `erase` destroys the same key row `erased` does, so it has to take the same keys
/// with it. Expanding the parent into its children at the `erased` line instead left
/// these two worlds answering differently, and the one written with `erase` was the one
/// that revealed a shredded buyer's email and sent it.
#[test]
fn an_effect_erasing_a_parent_shreds_the_same_keys_the_setup_line_does() {
    const WIPE: &str = "event @wipe.requested { shop_id: Shop }
effect Wipe {
  on @wipe.requested { @key shop_id } { erase(shop_id) }

  on @contact.recorded as e { @key buyer_id } {
    fold email: String? = none
      on @contact.recorded(buyer_id: e.buyer_id) { email } => email
    let to = reveal(email)
    if to.is_none() {
      return
    }
    http.post(\"https://mail.example/send\", { \"to\": to })
  }
}
";
    let given = "given @contact.recorded { shop_id: 1, buyer_id: 7, email: \"ada@example.com\", note: \"n\", at: \"2019-12-31T12:00:00Z\" }";
    let erased = sealed_verdicts(&format!(
        "{WIPE}
test \"the setup line\" {{
  {given}
  erased Shop \"1\"
  deliver Wipe
  expect skipped
}}"
    ));
    assert!(only(&erased).passed(), "{}", only(&erased));

    let ran = sealed_verdicts(&format!(
        "{WIPE}
test \"the effect's own erase\" {{
  given @wipe.requested {{ shop_id: 1 }}
  {given}
  deliver Wipe
  expect erase(Shop, \"1\")
  expect skipped
}}"
    ));
    assert!(only(&ran).passed(), "{}", only(&ran));
}

/// Two fields of the ancestor's type and nothing an author wrote says which one minted
/// the key, so the chain stops rather than taking the first declared: which shop a
/// buyer's key hangs from must not depend on field order. A runtime stops short for the
/// same reason, and refuses the project rather than guessing.
#[test]
fn an_ancestor_carried_twice_mints_no_wrapping() {
    let results = sealed_verdicts(
        "event @contact.moved { from_shop: Shop, to_shop: Shop, buyer_id: Buyer, email: String? @subject(buyer_id) }
projector Moves {
  entity Move { buyer_id: Buyer @key, email: String? }
  on @contact.moved { buyer_id, email } { put Move { buyer_id, email } }
}
test \"neither shop claims the buyer\" {
  given @contact.moved { from_shop: 1, to_shop: 2, buyer_id: 7, email: \"ada@example.com\" }
  erased Shop \"1\"
  project Moves
  expect Move[7] { email: \"ada@example.com\" }
}",
    );
    assert!(only(&results).passed(), "{}", only(&results));
}

/// A queue per URL, so a 503 then a 200 is how a test says the first attempt was
/// absorbed by rule 5 of `docs/effects.md` and the arm never saw it.
#[test]
fn respond_queues_replies_in_order() {
    let results = effect_verdicts(
        "test \"an absorbed retry is invisible to the arm\" {
  given @shop.connected { shop_id: 1, domain: \"one.example\", token: \"shpat\" }
  given @shop.sync.requested { shop_id: 1 }
  respond \"https://one.example/sync\" 503
  respond \"https://one.example/sync\" 200
  deliver SyncShop
  expect http.post(\"https://one.example/sync\")
  expect invoke RecordSync { plan_id: 1 }
}",
    );
    assert!(only(&results).passed(), "{}", only(&results));
}

// Rule 4: the action.

#[test]
fn a_test_does_exactly_one_thing() {
    let message = err("test \"two actions\" {
  run RecordSync { plan_id: 1 }
  project Plans
}");
    assert_eq!(
        message,
        "a test does one thing; a second action is a second test, or a `given`"
    );
}

#[test]
fn an_action_must_name_something_declared() {
    let message = err("test \"nope\" { project Nope }");
    assert_eq!(message, "projector `Nope` is not declared");
}

/// Rule 4: `deliver` drives, so an effect that fires on two of the given events makes
/// two invocations and the trace covers both.
#[test]
fn deliver_drives_the_whole_log() {
    let results = effect_verdicts(
        "test \"two requests sync twice\" {
  given @shop.connected { shop_id: 1, domain: \"one.example\", token: \"shpat\" }
  given @shop.sync.requested { shop_id: 1 }
  given @shop.sync.requested { shop_id: 1 }
  respond \"https://one.example/sync\" 200
  respond \"https://one.example/sync\" 200
  deliver SyncShop
  expect http.post(\"https://one.example/sync\")
  expect invoke RecordSync { plan_id: 1 }
  expect http.post(\"https://one.example/sync\")
  expect invoke RecordSync { plan_id: 1 }
}",
    );
    assert!(only(&results).passed(), "{}", only(&results));
}

// Rule 5: what `run` expects.

#[test]
fn run_matches_the_appended_events_one_for_one() {
    let results = verdicts(
        "test \"a first sync appends\" {
  run RecordSync { plan_id: 1 }
  expect @plan.synced { plan_id: 1 }
}",
    );
    assert!(only(&results).passed(), "{}", only(&results));
}

#[test]
fn an_extra_appended_event_is_a_failure() {
    let results = verdicts(
        "test \"expects none\" {
  run RecordSync { plan_id: 1 }
  expect nothing
}",
    );
    assert_eq!(why(only(&results)), "expected nothing, got @plan.synced");
}

#[test]
fn expect_nothing_passes_when_the_command_appended_none() {
    let results = verdicts(
        "test \"a repeat sync is a no-op\" {
  given @plan.synced { plan_id: 1 }
  run RecordSync { plan_id: 1 }
  expect reject Already
}",
    );
    assert!(only(&results).passed(), "{}", only(&results));
}

#[test]
fn run_matches_invalid_and_reject() {
    let results = verdicts(
        "test \"a negative id is invalid\" {
  run RecordSync { plan_id: -1 }
  expect invalid \"a plan id is not negative\"
}",
    );
    assert!(only(&results).passed(), "{}", only(&results));
}

#[test]
fn a_wrong_field_names_the_field_and_both_values() {
    let results = verdicts(
        "test \"wrong id\" {
  run RecordSync { plan_id: 1 }
  expect @plan.synced { plan_id: 2 }
}",
    );
    assert_eq!(
        why(only(&results)),
        "@plan.synced.plan_id: expected 2, got 1"
    );
}

// Rule 6: what `project` expects.

#[test]
fn a_row_is_matched_on_the_listed_columns_only() {
    let results = verdicts(
        "test \"only what was asked for\" {
  given @plan.created { plan_id: 1, title: \"Cover\", price: 19.99 }
  project Plans
  expect Plan[1] { sold: 0 }
}",
    );
    assert!(
        only(&results).passed(),
        "`title` was not listed and is not checked: {}",
        only(&results)
    );
}

/// Rule 6: `expect no` is what makes the `patch` and `update` difference testable.
#[test]
fn expect_no_row_distinguishes_update_from_patch() {
    let results = verdicts(
        "test \"a sale on a deleted plan does not resurrect it\" {
  given @plan.created { plan_id: 1, title: \"Cover\", price: 19.99 }
  given @plan.deleted { plan_id: 1 }
  given @plan.sold { plan_id: 1, price: 19.99 }
  project Plans
  expect no Plan[1]
  expect Sales[1] { revenue: 19.99 }
}",
    );
    assert!(only(&results).passed(), "{}", only(&results));
}

#[test]
fn a_present_row_that_should_be_absent_says_so() {
    let results = verdicts(
        "test \"wrongly absent\" {
  given @plan.created { plan_id: 1, title: \"Cover\", price: 19.99 }
  project Plans
  expect no Plan[1]
}",
    );
    assert_eq!(why(only(&results)), "Plan[1] is present");
}

#[test]
fn a_column_the_entity_lacks_is_rejected_at_parse_time() {
    let message = err("test \"nope\" {
  project Plans
  expect Plan[1] { nope: 1 }
}");
    assert_eq!(message, "entity `Plan` has no field `nope`");
}

// Rule 7: what `deliver` expects.

#[test]
fn the_trace_is_ordered_and_complete() {
    let results = effect_verdicts(
        "test \"out of order\" {
  given @shop.connected { shop_id: 1, domain: \"one.example\", token: \"shpat\" }
  given @shop.sync.requested { shop_id: 1 }
  respond \"https://one.example/sync\" 200
  deliver SyncShop
  expect invoke RecordSync { plan_id: 1 }
  expect http.post(\"https://one.example/sync\")
}",
    );
    assert_eq!(
        why(only(&results)),
        "expected invoke RecordSync, got http.post https://one.example/sync"
    );
}

/// Rule 7: a body is matched on the keys the test wrote, because a request body is
/// often a large generated document.
#[test]
fn a_body_is_matched_on_the_keys_written() {
    let results = effect_verdicts(
        "test \"the body carried the shop\" {
  given @shop.connected { shop_id: 1, domain: \"one.example\", token: \"shpat\" }
  given @shop.sync.requested { shop_id: 1 }
  respond \"https://one.example/sync\" 200
  deliver SyncShop
  expect http.post(\"https://one.example/sync\", { \"shop\": 1 })
  expect invoke RecordSync { plan_id: 1 }
}",
    );
    assert!(only(&results).passed(), "{}", only(&results));
}

#[test]
fn a_wrong_body_key_names_the_path() {
    let results = effect_verdicts(
        "test \"wrong shop in the body\" {
  given @shop.connected { shop_id: 1, domain: \"one.example\", token: \"shpat\" }
  given @shop.sync.requested { shop_id: 1 }
  respond \"https://one.example/sync\" 200
  deliver SyncShop
  expect http.post(\"https://one.example/sync\", { \"shop\": 2 })
  expect invoke RecordSync { plan_id: 1 }
}",
    );
    assert_eq!(why(only(&results)), "body.shop: expected 2, got 1");
}

/// Rule 7: `log` is in the trace, which is how a test pins the branch an arm took when
/// its only other output is a decision not to call out.
#[test]
fn a_log_line_is_part_of_the_trace() {
    let results = effect_verdicts(
        "test \"a shop that never connected takes the none branch\" {
  given @shop.sync.requested { shop_id: 2 }
  deliver SyncShop
  expect log(\"shop 2 has never connected\")
}",
    );
    assert!(only(&results).passed(), "{}", only(&results));
}

#[test]
fn expect_nothing_covers_an_effect_that_did_nothing() {
    let results = effect_verdicts(
        "test \"an unrelated event does nothing\" {
  given @plan.deleted { plan_id: 1 }
  deliver SyncShop
  expect nothing
}",
    );
    assert!(only(&results).passed(), "{}", only(&results));
}

// Rule 8: what a test cannot assert.

#[test]
fn a_test_cannot_reach_the_world() {
    let message = err("test \"calling out\" {
  given @plan.synced { plan_id: 1 }
  respond \"https://x.example\" 200
  project Plans
  expect Plan[1] { sold: now() }
}");
    assert_eq!(
        message,
        "a test states inputs and expectations, so it cannot read a clock"
    );
}

// Rule 9: running.

/// A mismatch and an error are different facts, and collapsing them would make a broken
/// program look like a wrong assertion.
#[test]
fn a_failing_run_is_an_error_not_a_mismatch() {
    let source = "event @a.b { n: Int }

command Boom(n: Int) {
  emit @a.b { n: n / 0 }
}

test \"dividing by zero\" {
  run Boom { n: 1 }
  expect @a.b { n: 1 }
}
";
    let program = parse(source).expect("this parses; it fails at run time");
    let results = run_tests(&program);
    assert!(
        matches!(only(&results).outcome, TestOutcome::Errored(_)),
        "{}",
        only(&results)
    );
}

/// Each test gets its own interpreter, so one test's log cannot reach another's.
#[test]
fn tests_do_not_share_state() {
    let results = verdicts(
        "test \"first\" {
  given @plan.synced { plan_id: 1 }
  run RecordSync { plan_id: 1 }
  expect reject Already
}

test \"second\" {
  run RecordSync { plan_id: 1 }
  expect @plan.synced { plan_id: 1 }
}",
    );
    assert_eq!(results.len(), 2);
    assert!(results.iter().all(TestResult::passed), "{:?}", results);
}

/// The words a test body uses are claimed only inside one, so a construct only tests
/// use costs no name anywhere else.
#[test]
fn the_test_words_are_soft_outside_a_test() {
    let source = "event @a.b { given: Int, expect: String, no: Bool }

command Names(given: Int, expect: String, no: Bool) {
  let deliver = given + 1
  emit @a.b { given: deliver, expect, no }
}
";
    parse(source).expect("`given`, `expect`, `no` and `deliver` are ordinary names");
}

/// A test's expected value is a declared position like any other, so a bare `T` fills a
/// `T?`. Without this the report reads `expected "TRK-1", got "TRK-1"`, which is the
/// worst kind of failure message. See `docs/optionals.md`.
#[test]
fn an_expected_value_coerces_against_the_declared_type() {
    let source = "event @a.b { note: String? }

projector P {
  entity Row { n: Int @key, note: String? }
  on @a.b { note } { patch Row[1] { note } }
}

test \"a bare value fills an optional column\" {
  given @a.b { note: \"hello\" }
  project P
  expect Row[1] { note: \"hello\" }
}
";
    let program = parse(source).expect("this parses");
    let results = run_tests(&program);
    assert!(only(&results).passed(), "{}", only(&results));
}

/// The same rule on the way in: a `given` field is declared too.
#[test]
fn a_given_value_coerces_against_the_event_field() {
    let source = "event @a.b { note: String? }

projector P {
  entity Row { n: Int @key, present: Bool }
  on @a.b { note } { patch Row[1] { present: note.is_some() } }
}

test \"a bare value fills an optional event field\" {
  given @a.b { note: \"hello\" }
  project P
  expect Row[1] { present: true }
}
";
    let program = parse(source).expect("this parses");
    let results = run_tests(&program);
    assert!(only(&results).passed(), "{}", only(&results));
}

/// `docs/declarations.md`: there is no Uuid literal token, so the target type is what
/// makes a string one. That held for a `const` and an entity default and nowhere else,
/// which a test suite full of ids finds immediately.
#[test]
fn a_string_resolves_against_a_uuid_target_in_a_test() {
    let source = "event @a.b { id: Uuid }

command Make(id: Uuid) {
  emit @a.b { id }
}

test \"an inline uuid\" {
  run Make { id: \"0190d1a1-0000-7000-8000-000000000001\" }
  expect @a.b { id: \"0190d1a1-0000-7000-8000-000000000001\" }
}
";
    let program = parse(source).expect("a string in a Uuid position is a Uuid");
    assert!(only(&run_tests(&program)).passed());
}

#[test]
fn a_string_that_is_not_a_uuid_says_so() {
    let source = "event @a.b { id: Uuid }

command Make(id: Uuid) {
  emit @a.b { id }
}

test \"a bad uuid\" {
  run Make { id: \"not-a-uuid\" }
  expect @a.b { id: \"not-a-uuid\" }
}
";
    let message = parse(source).expect_err("this is not a Uuid").text();
    assert_eq!(message, "`not-a-uuid` is not a Uuid");
}

/// A port found this by writing its first test against a subject-bound field: the report
/// read `expected none, got none`, which is the failure a reader cannot act on. An `Opt`
/// carries its element type, sealing one seals that type, and an absent optional keeps
/// the seal because there was never a key (`docs/effects.md` rule 12). So two absent
/// optionals differed by a type nobody wrote and both printed the same.
#[test]
fn an_absent_subject_bound_optional_matches_an_absent_one() {
    let source = "subject Owner(Int)
event @a.b { owner: Owner, note: String? @subject(owner) }

command Make(owner: Owner, note: String?) {
  emit @a.b { owner, note }
}

test \"absent\" {
  run Make { owner: 1, note: none }
  expect @a.b { owner: 1, note: none }
}

test \"present\" {
  run Make { owner: 1, note: \"hello\" }
  expect @a.b { owner: 1, note: \"hello\" }
}
";
    let program = parse(source).expect("this parses");
    for result in run_tests(&program) {
        assert!(result.passed(), "{}: {:?}", result.name, result.outcome);
    }
}

/// `docs/projectors.md`: writing sealed content into a column seals the column, so a
/// projector storing a subject-bound field holds content rather than plaintext. A test
/// names what it put in and asks whether that is what came out, which is a question about
/// content and not about the key, so the two meet unsealed.
#[test]
fn a_sealed_column_can_be_asserted_by_its_content() {
    let source = "subject Shop(Int)
event @shop.connected { shop_id: Shop, shop_name: String @subject(shop_id) }

projector Shops {
  entity Shop {
    shop_id: Shop @key,
    shop_name: String,
  }

  on @shop.connected { shop_id, shop_name } {
    put Shop { shop_id, shop_name }
  }
}

test \"a personal column is readable\" {
  given @shop.connected { shop_id: 1, shop_name: \"Test Shop\" }
  project Shops
  expect Shop[1] { shop_name: \"Test Shop\" }
}
";
    let program = parse(source).expect("this parses");
    assert!(only(&run_tests(&program)).passed());
}

/// And the other direction, because a check is worth what it rejects: unsealing to compare
/// must not make every value match every other one.
#[test]
fn a_sealed_column_holding_something_else_still_fails() {
    let source = "subject Shop(Int)
event @shop.connected { shop_id: Shop, shop_name: String @subject(shop_id) }

projector Shops {
  entity Shop {
    shop_id: Shop @key,
    shop_name: String,
  }

  on @shop.connected { shop_id, shop_name } {
    put Shop { shop_id, shop_name }
  }
}

test \"a personal column is readable\" {
  given @shop.connected { shop_id: 1, shop_name: \"Test Shop\" }
  project Shops
  expect Shop[1] { shop_name: \"Some Other Shop\" }
}
";
    let program = parse(source).expect("this parses");
    let results = run_tests(&program);
    let result = only(&results);
    let TestOutcome::Failed(why) = &result.outcome else {
        panic!("expected a mismatch, got {:?}", result.outcome);
    };
    assert!(
        why.contains("expected \"Some Other Shop\", got \"Test Shop\""),
        "and it names both sides in the clear: {why}"
    );
}

// ---------------------------------------------------------------------------
// Rule 8, the other half: the same expectations against a world the crate does not own.
// ---------------------------------------------------------------------------

/// Read models that are not a `Store`, counting what reached them so a test can tell a
/// projection that went through this from one that quietly went somewhere else.
#[derive(Default)]
struct Ledger {
    rows: Vec<(String, Key, Row)>,
    writes: Rc<Cell<usize>>,
}

impl Rows for Ledger {
    fn row(&self, entity: &str, key: &Key) -> Result<Option<Row>, Error> {
        Ok(self
            .rows
            .iter()
            .find(|(name, at, _)| name == entity && at == key)
            .map(|(_, _, row)| row.clone()))
    }

    fn put(&mut self, entity: &Ident, key: Key, row: Row) -> Result<(), Error> {
        self.writes.set(self.writes.get() + 1);
        self.rows
            .retain(|(name, at, _)| !(name == entity && at == &key));
        self.rows.push((entity.clone(), key, row));
        Ok(())
    }

    fn delete(&mut self, entity: &Ident, key: &Key) -> Result<(), Error> {
        self.writes.set(self.writes.get() + 1);
        self.rows
            .retain(|(name, at, _)| !(name == entity && at == key));
        Ok(())
    }
}

/// A world assembled here rather than taken from the crate. The log is still the
/// harness, because what is being proved is that the runner does not require a
/// `Sandbox`, not that a second log is possible.
#[derive(Default)]
struct Elsewhere {
    harness: Harness,
    writes: Rc<Cell<usize>>,
}

impl World for Elsewhere {
    type Host = Harness;
    type Rows = Ledger;

    fn given(&mut self, event: Event) -> Result<(), Error> {
        self.harness.push(event);
        Ok(())
    }

    fn respond(&mut self, url: &str, reply: Reply) -> Result<(), Error> {
        self.harness.script(url, [reply]);
        Ok(())
    }

    fn erased(&mut self, subject: &str, id: &str) -> Result<(), Error> {
        self.harness.erase_subject(subject, id);
        Ok(())
    }

    fn open(self) -> Result<(Harness, Ledger), Error> {
        let writes = Rc::clone(&self.writes);
        Ok((
            self.harness,
            Ledger {
                rows: Vec::new(),
                writes,
            },
        ))
    }
}

const SUITE: &str = "test \"a sale lands on both entities\" {
  given @plan.created { plan_id: 1, title: \"Pro\", price: 10.00 }
  given @plan.sold { plan_id: 1, price: 10.00 }
  project Plans
  expect Plan[1] { title: \"Pro\", sold: 1 }
  expect Sales[1] { revenue: 10.00 }
}

test \"a command still runs\" {
  run RecordSync { plan_id: 4 }
  expect @plan.synced { plan_id: 4 }
}

test \"a deleted plan is gone\" {
  given @plan.created { plan_id: 1, title: \"Pro\", price: 10.00 }
  given @plan.deleted { plan_id: 1 }
  project Plans
  expect no Plan[1]
}
";

/// One definition of `expect`, two worlds: the same suite, the same verdicts, against
/// read models the crate did not supply.
#[test]
fn a_suite_runs_against_a_world_the_crate_does_not_own() {
    let program = parse(&format!("{PRELUDE}{SUITE}")).expect("parses");

    let mine: Vec<TestResult> = run_tests(&program);
    let theirs: Vec<TestResult> = run_tests_in(&program, &mut || Ok(Elsewhere::default()));

    assert_eq!(mine.len(), theirs.len());
    for (mine, theirs) in mine.iter().zip(&theirs) {
        assert_eq!(mine.name, theirs.name);
        assert!(
            mine.passed() && theirs.passed(),
            "{}: {mine} then {theirs}",
            mine.name
        );
    }
}

/// The rows a projection wrote really went through the world's, rather than through an
/// in-memory one the runner kept to itself.
#[test]
fn a_projection_writes_through_the_worlds_own_read_models() {
    let program = parse(&format!("{PRELUDE}{SUITE}")).expect("parses");
    let writes = Rc::new(Cell::new(0));

    let results = run_tests_in(&program, &mut || {
        Ok(Elsewhere {
            harness: Harness::default(),
            writes: Rc::clone(&writes),
        })
    });

    assert!(results.iter().all(TestResult::passed));
    assert!(writes.get() > 0, "no write reached the world's read models");
}

/// A world that cannot be built is not the program being wrong, so it reads as an error
/// rather than as every expectation failing.
#[test]
fn a_world_that_cannot_be_built_errors_rather_than_fails() {
    let program = parse(&format!("{PRELUDE}{SUITE}")).expect("parses");
    let results = run_tests_in(&program, &mut || -> Result<Elsewhere, Error> {
        Err(Error::new(heklang::ErrorKind::Host("no disk".to_string())))
    });

    assert!(!results.is_empty());
    assert!(
        results.iter().all(
            |result| matches!(result.outcome, TestOutcome::Errored(ref why) if why == "no disk")
        ),
        "{results:?}"
    );
}

/// A seal is transparent to a literal, in an expectation as much as anywhere else.
///
/// `Number::resolve` already unseals what it is handed, but the hint that reaches it
/// peeled only the optional, so a sealed numeric column never received its declared
/// type: the literal defaulted to `Decimal` and then failed to fill the field it was
/// written into. A sealed column has to be optional, so `Opt(Sealed(Money(2), org))` is
/// the shape every one of them has and the only shape this could be found in.
#[test]
fn a_literal_takes_its_type_through_a_seal() {
    let program = parse(
        "subject Org(Int)
event @t.happened {
  id: Uuid,
  org: Org,
  amount: Money(2) @subject(org),
  count: Int @subject(org),
}
projector P {
  entity Row {
    id: Uuid @key,
    org: Org,
    amount: Money(2)?,
    count: Int?,
  }
  on @t.happened { id, org, amount, count } { put Row { id, org, amount, count } }
}
test \"a sealed numeric column takes a bare literal\" {
  given @t.happened {
    id: \"11111111-1111-1111-1111-111111111111\",
    org: 1,
    amount: 5.00,
    count: 3,
  }
  project P
  expect Row[\"11111111-1111-1111-1111-111111111111\"] { org: 1, amount: 5.00, count: 3 }
}
",
    );
    assert!(
        program.is_ok(),
        "a bare literal in a sealed numeric position must parse: {}",
        program.err().map(|err| err.text()).unwrap_or_default()
    );
}

// Rule 3: the fourth lever. `docs/effects.md` rule 16 is the language side.

const CREDENTIALS: &str = "secret ALERT_HOOK
secret SENTRY_DSN?

effect Alerts {
  on @plan.sold { @key plan_id } {
    let sent = http.post(ALERT_HOOK, { \"plan\": plan_id })
    let dsn = SENTRY_DSN
    if dsn.is_none() {
      log(\"no dsn\")
    }
  }
}
";

fn credential_verdicts(body: &str) -> Vec<TestResult> {
    let source = format!("{PRELUDE}\n{CREDENTIALS}\n{body}");
    let program = parse(&source).unwrap_or_else(|err| panic!("expected this to parse: {err}"));
    run_tests(&program)
}

/// The default: a declared credential resolves to `secret:NAME`, so an effect that reads
/// one runs with no setup line and the expectation reads as itself.
#[test]
fn a_secret_needs_no_setup() {
    let results = credential_verdicts(
        "test \"an alert goes out\" {
  given @plan.sold { plan_id: 1, price: 19.99 }
  respond \"secret:ALERT_HOOK\" 200
  deliver Alerts
  expect http.post(\"secret:ALERT_HOOK\", { \"plan\": 1 })
}",
    );
    assert!(
        matches!(only(&results).outcome, TestOutcome::Passed),
        "{:?}",
        only(&results).outcome
    );
}

/// The override, for a value whose shape matters, and the reason it is a setup line:
/// it sits beside the `respond` that answers it rather than above the whole log.
#[test]
fn a_secret_directive_supplies_the_value() {
    let results = credential_verdicts(
        "test \"an alert goes to the configured hook\" {
  given @plan.sold { plan_id: 1, price: 19.99 }

  secret ALERT_HOOK = \"https://example.test/hook\"
  respond \"https://example.test/hook\" 200

  deliver Alerts

  expect http.post(\"https://example.test/hook\", { \"plan\": 1 })
}",
    );
    assert!(
        matches!(only(&results).outcome, TestOutcome::Passed),
        "{:?}",
        only(&results).outcome
    );
}

/// `= none` is a deployment that did not set it, and the only way to reach the absent
/// branch of an optional: every other name answers with the stand-in.
#[test]
fn none_is_a_deployment_that_did_not_set_one() {
    let results = credential_verdicts(
        "test \"an unset dsn is a branch\" {
  given @plan.sold { plan_id: 1, price: 19.99 }

  secret SENTRY_DSN = none
  respond \"secret:ALERT_HOOK\" 200

  deliver Alerts

  expect http.post(\"secret:ALERT_HOOK\", { \"plan\": 1 })
  expect log(\"no dsn\")
}",
    );
    assert!(
        matches!(only(&results).outcome, TestOutcome::Passed),
        "{:?}",
        only(&results).outcome
    );

    // And the stand-in answers when nothing says otherwise, so the branch is not taken.
    let results = credential_verdicts(
        "test \"a set dsn is not\" {
  given @plan.sold { plan_id: 1, price: 19.99 }
  respond \"secret:ALERT_HOOK\" 200
  deliver Alerts
  expect http.post(\"secret:ALERT_HOOK\", { \"plan\": 1 })
}",
    );
    assert!(
        matches!(only(&results).outcome, TestOutcome::Passed),
        "{:?}",
        only(&results).outcome
    );
}

/// A required one the deployment did not set is the wedge `docs/effects.md` rule 16
/// describes, and it reads as **errored** rather than failed: the program could not run
/// at all, which is not the same as an expectation not matching.
#[test]
fn a_required_secret_set_to_none_wedges() {
    let results = credential_verdicts(
        "test \"nothing set it\" {
  given @plan.sold { plan_id: 1, price: 19.99 }
  secret ALERT_HOOK = none
  deliver Alerts
  expect nothing
}",
    );
    let TestOutcome::Errored(why) = &only(&results).outcome else {
        panic!("expected an error, got {:?}", only(&results).outcome);
    };
    assert!(
        why.contains("this deployment has not set `ALERT_HOOK`"),
        "the operator has to be told which one, got {why}"
    );
}

/// The name is checked against the declarations, so a typo is a check error rather than
/// a test that silently scripts nothing.
#[test]
fn the_directive_names_a_declared_secret() {
    let source = format!(
        "{PRELUDE}\n{CREDENTIALS}\ntest \"typo\" {{
  given @plan.sold {{ plan_id: 1, price: 19.99 }}
  secret ALERT_HOK = \"https://example.test/hook\"
  deliver Alerts
  expect nothing
}}"
    );
    let message = parse(&source)
        .expect_err("an undeclared credential is not scriptable")
        .text();
    assert_eq!(message, "secret `ALERT_HOK` is not declared");
}

/// The word stays soft in a test body too, so the construct costs no name: a local
/// called `secret` in an effect is what the `EFFECTS` prelude above already writes.
#[test]
fn secret_is_soft_in_a_test_body() {
    let results = effect_verdicts(
        "test \"a shop that never connected is skipped\" {
  given @shop.sync.requested { shop_id: 1 }
  deliver SyncShop
  expect log(\"shop 1 has never connected\")
}",
    );
    assert!(
        matches!(only(&results).outcome, TestOutcome::Passed),
        "{:?}",
        only(&results).outcome
    );
}
