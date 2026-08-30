//! `docs/effects.md` as executable tests, one test per numbered rule.

use heklang::{
    Defs, Effectful, Event, EventPath, Interpreter, Invocation, Invoked, Journal, Json, Key,
    Program, Recorded, Reply, Type, Value, parse,
};

const URL: &str = "https://mail.example/confirm";

const PRELUDE: &str = "event @order.placed {
  order_id: Uuid,
  customer_id: Int,
  email: String @subject(customer_id),
  total: Money(2),
}
event @order.cancelled { order_id: Uuid, customer_id: Int }
event @order.reviewed {
  order_id: Uuid,
  customer_id: Int,
  comment: String? @subject(customer_id),
}
event @order.notified { order_id: Uuid, notification_id: Uuid }
event @order.reconfirmed {
  order_id: Uuid,
  customer_id: Int,
  email: String @subject(customer_id),
}
event @order.audited {
  order_id: Uuid,
  auditor_id: Int,
  note: String @subject(auditor_id),
  tool: String,
}

// A tenant grouping many subjects, which is the shape a bulk erase needs: the ids come
// from a fold rather than from the event being handled.
event @tenant.redacted { tenant_id: Int }
event @tenant.member.joined {
  tenant_id: Int,
  member_id: Int,
  secret: String @subject(member_id),
}

command RecordNotified(order_id: Uuid, notification_id: Uuid) {
  guard @order.notified(order_id)

  state notified: Bool = fold false
    on @order.notified(order_id) => true

  if notified {
    return reject(\"already_notified\", \"this order was already confirmed\")
  }

  emit @order.notified { order_id, notification_id }
}
";

fn source(body: &str) -> String {
    format!("{PRELUDE}{body}\n")
}

fn program(body: &str) -> Program {
    parse(&source(body)).unwrap_or_else(|err| panic!("expected this effect to parse: {err}"))
}

fn err(body: &str) -> String {
    parse(&source(body))
        .expect_err("expected this effect to be rejected")
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

fn cancelled(seq: u32, customer_id: i64) -> Event {
    Event::new(
        EventPath::new(["order", "cancelled"]),
        [
            (
                "order_id",
                Value::uuid(format!("0190d1a1-0000-7000-8000-{seq:012}")),
            ),
            ("customer_id", Value::Int(customer_id)),
        ],
    )
}

/// An event whose subject-bound field is optional, which is the shape rule 12's
/// "an optional in, an optional out" turns on.
fn reviewed(seq: u32, customer_id: i64, comment: Option<&str>) -> Event {
    Event::new(
        EventPath::new(["order", "reviewed"]),
        [
            (
                "order_id",
                Value::uuid(format!("0190d1a1-0000-7000-8000-{seq:012}")),
            ),
            ("customer_id", Value::Int(customer_id)),
            (
                "comment",
                match comment {
                    Some(text) => Value::some(Value::str(text)),
                    None => Value::none(Type::String),
                },
            ),
        ],
    )
}

/// The same subject-bound field on a second event type, which is the shape a fold with
/// two arms under one subject takes.
fn reconfirmed(seq: u32, customer_id: i64, email: &str) -> Event {
    Event::new(
        EventPath::new(["order", "reconfirmed"]),
        [
            (
                "order_id",
                Value::uuid(format!("0190d1a1-0000-7000-8000-{seq:012}")),
            ),
            ("customer_id", Value::Int(customer_id)),
            ("email", Value::str(email)),
        ],
    )
}

/// One delivery of position 0, with `replies` scripted for the one URL these effects
/// call. Returns the interpreter so a test can read the journal, the log or the lines.
fn deliver<'a>(
    program: &'a Program,
    log: Vec<Event>,
    replies: Vec<Reply>,
    journal: &mut Journal,
) -> (Interpreter<'a>, Result<Invocation, heklang::Error>) {
    let mut interpreter = Interpreter::with_log(program, log);
    interpreter.script(URL, replies);
    let outcome = interpreter.deliver("E", 0, journal);
    (interpreter, outcome)
}

/// The body an `http.post` sent, recovered from the journal, which is where a test can
/// see what the handler actually decided.
fn posted(journal: &Journal) -> String {
    journal
        .calls()
        .find(|(call, _)| call.starts_with("http.post"))
        .map(|(call, _)| call.to_string())
        .expect("expected an http.post in the journal")
}

// ---------------------------------------------------------------------------------
// Rule 1: arms name distinct event types.

#[test]
fn two_arms_on_one_event_are_rejected() {
    let message = err("effect E {
  on @order.placed as e { log(\"first\") }
  on @order.placed as e { log(\"second\") }
}");
    assert!(
        message.contains("already has an arm on @order.placed"),
        "got: {message}"
    );
    assert!(
        message.contains("one event selects exactly one arm"),
        "expected the rule to be named, got: {message}"
    );
}

#[test]
fn one_event_selects_exactly_one_arm() {
    let program = program(
        "effect E {
  on @order.placed as e { log(\"placed\") }
  on @order.cancelled as e { log(\"cancelled\") }
}",
    );
    let mut journal = Journal::default();
    let (interpreter, outcome) = deliver(&program, vec![placed(1, 7, 2_599)], vec![], &mut journal);
    assert_eq!(outcome.expect("delivered"), Invocation::Done);
    assert_eq!(interpreter.lines(), ["placed"]);
}

// ---------------------------------------------------------------------------------
// Rule 2: state lives inside the arm.

#[test]
fn arm_state_may_filter_on_the_trigger_binding() {
    let program = program(
        "effect E {
  on @order.placed as e {
    state mine: Int = fold 0
      on @order.placed(customer_id: e.customer_id) => mine + 1

    http.post(\"https://mail.example/confirm\", { \"mine\": mine })
  }
}",
    );
    let log = vec![placed(1, 7, 100), placed(2, 9, 100), placed(3, 7, 100)];
    let mut journal = Journal::default();
    let (_, outcome) = deliver(&program, log, vec![Reply::Status(200)], &mut journal);
    outcome.expect("delivered");
    // Customer 7 placed the trigger and nothing else at or before position 0.
    assert!(
        posted(&journal).contains("\"mine\":1"),
        "{}",
        posted(&journal)
    );
}

#[test]
fn state_is_per_arm_not_per_effect() {
    let message = err("effect E {
  on @order.placed as e {
    state mine: Int = fold 0
      on @order.placed(customer_id: e.customer_id) => mine + 1

    log(\"first\")
  }
  on @order.cancelled as e { log(\"seen\" + mine) }
}");
    assert_eq!(message, "`mine` is not in scope");
}

// ---------------------------------------------------------------------------------
// Rule 3: the fold stops at the trigger's own position, inclusive.

const COUNTING: &str = "effect E {
  on @order.placed as e {
    state seen: Int = fold 0
      on @order.placed(customer_id: e.customer_id) => seen + 1

    http.post(\"https://mail.example/confirm\", { \"seen\": seen })
  }
}";

#[test]
fn the_fold_stops_at_the_trigger_position() {
    let program = program(COUNTING);
    // Three more orders for the same customer sit above the trigger. Folding to head
    // would count them, and state would depend on how far the log had run.
    let log = vec![
        placed(1, 7, 100),
        placed(2, 7, 100),
        placed(3, 7, 100),
        placed(4, 7, 100),
    ];
    let mut journal = Journal::default();
    let (_, outcome) = deliver(&program, log, vec![Reply::Status(200)], &mut journal);
    outcome.expect("delivered");
    assert!(
        posted(&journal).contains("\"seen\":1"),
        "{}",
        posted(&journal)
    );
}

#[test]
fn a_first_order_counts_itself() {
    let program = program(COUNTING);
    let mut journal = Journal::default();
    let (_, outcome) = deliver(
        &program,
        vec![placed(1, 7, 100)],
        vec![Reply::Status(200)],
        &mut journal,
    );
    outcome.expect("delivered");
    // Inclusive, so one rather than zero.
    assert!(
        posted(&journal).contains("\"seen\":1"),
        "{}",
        posted(&journal)
    );
}

// ---------------------------------------------------------------------------------
// Rule 4: `fail` is the author's terminal outcome.

const FAILING: &str = "effect E {
  on @order.placed as e {
    let response = http.post(\"https://mail.example/confirm\", { \"to\": \"x\" })
    if response.status >= 400 {
      fail(\"confirmation rejected\")
    }
    log(\"sent\")
  }
}";

#[test]
fn fail_is_terminal_and_advances() {
    let program = program(FAILING);
    let mut journal = Journal::default();
    let (interpreter, outcome) = deliver(
        &program,
        vec![placed(1, 7, 100)],
        vec![Reply::Status(422)],
        &mut journal,
    );
    assert_eq!(
        outcome.expect("a `fail` is an outcome, not an error"),
        Invocation::Failed("confirmation rejected".to_string())
    );
    // Terminal, so nothing after it ran.
    assert!(interpreter.lines().is_empty());
}

#[test]
fn author_failures_are_counted_apart_from_wedges() {
    let program = program(FAILING);
    let mut interpreter = Interpreter::with_log(&program, vec![placed(1, 7, 100)]);
    interpreter.script(URL, [Reply::Status(422)]);
    let counts = interpreter
        .drive("E")
        .expect("a `fail` advances the cursor");

    assert_eq!(counts.failed(), 1);
    assert_eq!(counts.skipped(), 0);
    assert_eq!(counts.done, 0);
    // The whole safety of `fail` rests on this staying separate.
    assert!(counts.wedged.is_none());
}

// ---------------------------------------------------------------------------------
// Rule 5: the handler sees only what it can act on.

#[test]
fn a_retryable_status_never_reaches_the_handler() {
    let program = program(FAILING);
    let mut journal = Journal::default();
    // A transport error, a 503 and a 429 all clear on their own with the same request,
    // so the runtime absorbs them and re-sends; only the 200 is a decision.
    let (interpreter, outcome) = deliver(
        &program,
        vec![placed(1, 7, 100)],
        vec![
            Reply::Transport("connection reset".to_string()),
            Reply::Status(503),
            Reply::Status(429),
            Reply::Status(200),
        ],
        &mut journal,
    );
    assert_eq!(outcome.expect("delivered"), Invocation::Done);
    assert_eq!(interpreter.lines(), ["sent"]);
    assert_eq!(interpreter.absorbed(), 3);
    // One journal entry, because one response reached the handler.
    assert_eq!(journal.len(), 1);
}

#[test]
fn a_wedge_is_invisible_to_the_script() {
    let program = program(FAILING);
    let mut journal = Journal::default();
    let (interpreter, outcome) = deliver(
        &program,
        vec![placed(1, 7, 100)],
        vec![
            Reply::Status(503),
            Reply::Status(503),
            Reply::Status(503),
            Reply::Status(503),
        ],
        &mut journal,
    );
    // The script cannot observe this, decide on it, or count the attempts.
    assert!(outcome.is_err());
    assert!(interpreter.lines().is_empty());
    assert!(journal.is_empty());
}

// ---------------------------------------------------------------------------------
// Rule 6: `invoke` returns an outcome.

const INVOKING: &str = "effect E {
  on @order.placed as e {
    let result = invoke RecordNotified {
      order_id: e.order_id,
      notification_id: Uuid.derive(e.id, \"confirmation\"),
    }
    log(result.code().unwrap_or(\"ok\"))
  }
}";

#[test]
fn invoke_returns_ok_invalid_or_reject() {
    let program = program(INVOKING);
    let mut journal = Journal::default();
    let (interpreter, outcome) = deliver(&program, vec![placed(1, 7, 100)], vec![], &mut journal);
    outcome.expect("delivered");
    assert_eq!(interpreter.lines(), ["ok"]);
    // The command really ran, so its event is in the log.
    assert_eq!(interpreter.log().len(), 2);

    // A second delivery of the same trigger hits the command's own guard state.
    let mut second = Journal::default();
    let outcome = {
        let mut interpreter = Interpreter::with_log(&program, vec![placed(1, 7, 100)]);
        interpreter.deliver("E", 0, &mut second).expect("delivered");
        interpreter
            .deliver("E", 0, &mut Journal::default())
            .expect("delivered");
        interpreter.lines().to_vec()
    };
    assert_eq!(outcome, ["ok", "already_notified"]);
}

#[test]
fn the_outcome_type_has_no_retryable_case() {
    // Exhaustive on purpose: adding a `Conflict` or `Unavailable` variant would have to
    // fail here first, which is the point of cutting them from the type rather than
    // filtering them at the boundary.
    for outcome in [
        Invoked::Ok,
        Invoked::Invalid("bad".to_string()),
        Invoked::Reject {
            code: "no".to_string(),
            message: "nope".to_string(),
        },
    ] {
        let described = match outcome {
            Invoked::Ok => "ok",
            Invoked::Invalid(_) => "invalid",
            Invoked::Reject { .. } => "reject",
        };
        assert!(["ok", "invalid", "reject"].contains(&described));
    }
}

// ---------------------------------------------------------------------------------
// Rule 7: `invoke` input is a typed struct.

#[test]
fn invoke_with_an_unknown_field_fails_at_compile_time() {
    let message = err("effect E {
  on @order.placed as e {
    invoke RecordNotified { order_id: e.order_id, notification: e.order_id }
  }
}");
    assert_eq!(
        message,
        "command `RecordNotified` has no parameter `notification`"
    );
}

#[test]
fn invoke_needs_every_required_parameter() {
    let message = err("effect E {
  on @order.placed as e {
    invoke RecordNotified { order_id: e.order_id }
  }
}");
    assert_eq!(message, "command `RecordNotified` needs `notification_id`");
}

#[test]
fn invoke_is_revalidated_at_runtime() {
    // The compile-time check covers one program version. This is what an invocation
    // straddling a deploy meets: a not-yet-journaled `invoke` against a command whose
    // signature has moved since.
    let mut program = program(INVOKING);
    let target = program
        .commands
        .iter_mut()
        .find(|command| command.name == "RecordNotified")
        .expect("declared in the prelude");
    target.params.push(heklang::Param {
        name: "channel".to_string(),
        ty: Type::String,
        slot: heklang::Slot(9),
    });

    let mut journal = Journal::default();
    let (_, outcome) = deliver(&program, vec![placed(1, 7, 100)], vec![], &mut journal);
    let err = outcome.expect_err("the runtime check is what catches the drift");
    assert_eq!(err.kind.to_string(), "missing argument `channel`");
}

#[test]
fn an_object_literal_is_not_an_invoke_input() {
    let message = err("effect E {
  on @order.placed as e {
    invoke RecordNotified { \"order_id\": e.order_id }
  }
}");
    assert!(message.contains("expected a name"), "got: {message}");

    let message = err("effect E {
  on @order.placed as e {
    let body = { \"order_id\": e.order_id }
  }
}");
    assert!(
        message.contains("an object literal is an HTTP request body"),
        "got: {message}"
    );
}

// ---------------------------------------------------------------------------------
// Rule 8: `Json`.

#[test]
fn json_accessors_return_optionals() {
    let program = program(
        "effect E {
  on @order.placed as e {
    let response = http.get(\"https://mail.example/confirm\")
    log(response.body.string(\"id\").unwrap_or(\"none\"))
    log(response.body.string(\"missing\").unwrap_or(\"none\"))
  }
}",
    );
    let mut journal = Journal::default();
    let (interpreter, outcome) = deliver(
        &program,
        vec![placed(1, 7, 100)],
        vec![Reply::Body(
            200,
            heklang::Json::obj([("id", heklang::Json::str("abc"))]),
        )],
        &mut journal,
    );
    outcome.expect("delivered");
    assert_eq!(interpreter.lines(), ["abc", "none"]);
}

#[test]
fn object_keys_are_sorted() {
    let program = program(
        "effect E {
  on @order.placed as e {
    http.post(\"https://mail.example/confirm\", {
      \"zebra\": 1,
      \"alpha\": 2,
      \"middle\": 3,
    })
  }
}",
    );
    let mut journal = Journal::default();
    let (_, outcome) = deliver(
        &program,
        vec![placed(1, 7, 100)],
        vec![Reply::Status(200)],
        &mut journal,
    );
    outcome.expect("delivered");
    // Rule 14 depends on this: the same object built twice serialises the same way.
    assert!(
        posted(&journal).ends_with("{\"alpha\":2,\"middle\":3,\"zebra\":1}"),
        "{}",
        posted(&journal)
    );
}

/// A body's values are typed by what they are, so an empty array needs no target and
/// reaches the wire as one. At any depth, because the whole literal is a body rather
/// than only its outermost brace.
#[test]
fn an_empty_array_reaches_the_body_at_any_depth() {
    let program = program(
        "effect E {
  on @order.placed as e { customer_id } {
    http.post(\"https://mail.example/confirm\", {
      \"tags\": [],
      \"ids\": [customer_id],
      \"meta\": { \"also\": [] },
      \"encoded\": Json.encode({ \"deep\": [] }),
    })
  }
}",
    );
    let mut journal = Journal::default();
    let (_, outcome) = deliver(
        &program,
        vec![placed(1, 7, 100)],
        vec![Reply::Status(200)],
        &mut journal,
    );
    outcome.expect("delivered");
    let sent = posted(&journal);
    assert!(sent.contains("\"tags\":[]"), "{sent}");
    assert!(sent.contains("\"ids\":[7]"), "{sent}");
    assert!(sent.contains("\"meta\":{\"also\":[]}"), "{sent}");
    assert!(
        sent.contains(r#""encoded":"{\"deep\":[]}""#),
        "a nested object under `Json.encode` is a body too: {sent}"
    );
}

// ---------------------------------------------------------------------------------
// Rule 9: erase last, statically enforced.

#[test]
fn a_reveal_after_an_erase_is_rejected() {
    let message = err("effect E {
  on @order.placed as e {
    erase(e.customer_id)
    log(reveal(e.email))
  }
}");
    assert!(
        message.contains("can run after the `erase`"),
        "got: {message}"
    );
}

#[test]
fn an_erase_on_a_path_that_fails_does_not_poison_the_join() {
    // The erase is lexically first and still fine, because the branch holding it does
    // not fall through. A lexical check rejects this; reachability accepts it.
    program(
        "effect E {
  on @order.placed as e {
    if e.total > 100.00 {
      erase(e.customer_id)
      fail(\"gone\")
    }
    log(reveal(e.email))
  }
}",
    );

    // The same shape without the `fail` does fall through, so it is rejected.
    let message = err("effect E {
  on @order.placed as e {
    if e.total > 100.00 {
      erase(e.customer_id)
    }
    log(reveal(e.email))
  }
}");
    assert!(
        message.contains("can run after the `erase`"),
        "got: {message}"
    );
}

/// A loop body may run again, so an `erase` in it reaches a `reveal` lexically above
/// it. Two passes reach the fixed point, which is what a lexical check gets wrong.
#[test]
fn an_erase_in_a_loop_reaches_a_reveal_above_it() {
    let message = err("effect E {
  on @order.placed as e {
    state ids: List(Int) = fold []
      on @order.placed(customer_id: e.customer_id) { customer_id } => ids.push(customer_id)

    for id in ids {
      log(reveal(e.email))
      erase(e.customer_id)
    }
  }
}");
    assert!(
        message.contains("can run after the `erase`"),
        "got: {message}"
    );

    // The same two statements outside a loop are fine in that order, which is what
    // makes the loop the thing being tested rather than the order.
    program(
        "effect E {
  on @order.placed as e {
    log(reveal(e.email))
    erase(e.customer_id)
  }
}",
    );
}

#[test]
fn an_erase_is_not_an_expression() {
    let message = err("effect E {
  on @order.placed as e {
    let gone = erase(e.customer_id)
  }
}");
    assert!(
        message.contains("`erase` is a statement rather than a value"),
        "got: {message}"
    );
}

#[test]
fn the_erase_last_error_explains_the_journal() {
    let message = err("effect E {
  on @order.placed as e {
    erase(e.customer_id)
    log(reveal(e.email))
  }
}");
    // The message is the reader's path to the contract, so it has to say why.
    assert!(
        message.contains("`erase` is journaled and `reveal` is not"),
        "got: {message}"
    );
    assert!(message.contains("replay"), "got: {message}");
    assert!(message.contains("key that is gone"), "got: {message}");
}

#[test]
fn reveal_needs_a_subject_bound_field() {
    let message = err("effect E {
  on @order.placed as e {
    log(reveal(e.order_id))
  }
}");
    assert!(
        message.contains("`reveal` takes subject-bound content and this is a plain Uuid"),
        "got: {message}"
    );
}

// ---------------------------------------------------------------------------------
// ---------------------------------------------------------------------------------
// Rule 9: naming the subject. The form a bulk erase needs, where the ids come from a
// fold and the parser has no field name to recover.

fn joined(tenant_id: i64, member_id: i64) -> Event {
    Event::new(
        EventPath::new(["tenant", "member", "joined"]),
        [
            ("tenant_id", Value::Int(tenant_id)),
            ("member_id", Value::Int(member_id)),
            ("secret", Value::str(format!("token-{member_id}"))),
        ],
    )
}

const REDACT: &str = "effect E {
  on @tenant.redacted as e { tenant_id } {
    state members: List(Int) = fold []
      on @tenant.member.joined(tenant_id) { member_id } => members.push(member_id)

    for id in members {
      erase(member_id, id)
    }
  }
}";

#[test]
fn erase_may_name_its_subject() {
    let program = program(REDACT);
    let log = vec![
        joined(1, 7),
        joined(1, 8),
        // A second tenant, so the fold's filter is doing something.
        joined(2, 9),
        Event::new(
            EventPath::new(["tenant", "redacted"]),
            [("tenant_id", Value::Int(1))],
        ),
    ];
    let mut interpreter = Interpreter::with_log(&program, log);
    let mut journal = Journal::default();
    let outcome = interpreter
        .deliver("E", 3, &mut journal)
        .expect("delivered");
    assert_eq!(outcome, Invocation::Done);

    let erased: Vec<&Effectful> = interpreter
        .trace()
        .iter()
        .filter(|entry| matches!(entry, Effectful::Erase { .. }))
        .collect();
    assert_eq!(
        erased,
        [
            &Effectful::Erase {
                subject: "member_id".to_string(),
                id: "7".to_string()
            },
            &Effectful::Erase {
                subject: "member_id".to_string(),
                id: "8".to_string()
            },
        ],
        "tenant 2's member must not be erased"
    );
}

#[test]
fn a_named_subject_must_be_declared() {
    let message = err("effect E {
  on @tenant.redacted as e { tenant_id } {
    for id in [1, 2] {
      erase(nobody, id)
    }
  }
}");
    assert!(
        message.contains("nothing is scoped to `nobody`"),
        "got: {message}"
    );
}

/// Rule 9's second rule, which only this form can reach: the inferring form takes a
/// trigger field, and a `reveal` is not one.
#[test]
fn a_named_subject_rejects_a_revealed_id() {
    let message = err("effect E {
  on @order.placed as e { customer_id } {
    erase(customer_id, reveal(e.email).len())
  }
}");
    assert!(
        message.contains("was learned by revealing"),
        "got: {message}"
    );
    assert!(
        message.contains("take a subject id from a plaintext field"),
        "expected the fix, got: {message}"
    );
}

#[test]
fn a_named_subject_checks_the_value_type() {
    let message = err("effect E {
  on @order.placed as e { customer_id } {
    erase(customer_id, e.order_id)
  }
}");
    // "an Int" and "a Uuid" in one sentence, which is the pair a plain vowel rule gets
    // half right: `Uuid` is read "you-eye-dee".
    assert!(
        message.contains("files its keys under an Int"),
        "got: {message}"
    );
    assert!(message.contains("cannot take a Uuid"), "got: {message}");
}

/// The lookahead is three tokens, not two. `erase(customer_id,)` is a bare trigger
/// field plus the trailing comma every argument list takes, so it stays one argument.
#[test]
fn a_trailing_comma_does_not_make_a_named_subject() {
    program(
        "effect E {
  on @order.placed as e { customer_id } {
    erase(customer_id,)
  }
}",
    );
}

// Rule 10: no marker on unjournaled builtins.

#[test]
fn journaled_calls_do_not_re_fire_but_reveal_and_log_do() {
    let program = program(
        "effect E {
  on @order.placed as e {
    http.post(\"https://mail.example/confirm\", { \"to\": reveal(e.email) })
    log(\"sent\")
  }
}",
    );
    let mut interpreter = Interpreter::with_log(&program, vec![placed(1, 7, 100)]);
    interpreter.script(URL, [Reply::Status(200)]);

    let mut journal = Journal::default();
    interpreter
        .deliver("E", 0, &mut journal)
        .expect("delivered");
    let performed = interpreter.http_calls();
    assert_eq!(interpreter.lines(), ["sent"]);

    // The same journal makes the second run a replay.
    interpreter.deliver("E", 0, &mut journal).expect("replayed");
    assert_eq!(
        interpreter.http_calls(),
        performed,
        "the post must not re-fire"
    );
    // `reveal` and `log` are not journaled, so both run again. Nothing in the source
    // says so, which is rule 10: the compile error teaches it where it matters.
    assert_eq!(interpreter.lines(), ["sent", "sent"]);
}

// ---------------------------------------------------------------------------------
// Rule 11: builtins.

#[test]
fn there_is_no_uuid4_or_random() {
    for name in ["uuid4", "random", "uuid5"] {
        let message = err(&format!(
            "effect E {{
  on @order.placed as e {{
    log(\"x\" + {name}())
  }}
}}"
        ));
        assert!(
            message.contains(&format!("there is no `{name}` in heklang")),
            "got: {message}"
        );
        assert!(
            message.contains("Uuid.derive(seed, name)"),
            "got: {message}"
        );
    }

    // The absence that matters is the one beside `derive`, not the missing global: an
    // author reaching for a fresh id reaches for `Uuid.new`, so that is where the
    // reason has to be.
    for member in ["new", "random", "generate", "v4"] {
        let message = err(&format!(
            "effect E {{
  on @order.placed as e {{
    log(\"x\" + Uuid.{member}())
  }}
}}"
        ));
        assert!(
            message.starts_with(&format!("`Uuid` has no `{member}`:")),
            "got: {message}"
        );
        assert!(
            message.contains(
                "a command retry and an effect replay produce the id they produced the first time"
            ),
            "got: {message}"
        );
    }

    let message = err("effect E {
  on @order.placed as e {
    log(\"x\" + Uuid.parse(\"7\"))
  }
}");
    assert_eq!(
        message,
        "`Uuid` has no `parse`; it has `derive(seed, name)`"
    );
}

/// `Uuid.derive` is the first type-qualified call, so the receiver has to behave like
/// the soft builtin names: claimed only where it is unambiguous, shadowed by a local.
#[test]
fn a_local_named_uuid_shadows_the_type() {
    let program = program(
        "effect E {
  on @order.placed as e {
    let Uuid = \"shadowed\"
    log(Uuid)
  }
}",
    );
    let mut journal = Journal::default();
    let (interpreter, outcome) = deliver(&program, vec![placed(1, 7, 100)], vec![], &mut journal);
    outcome.expect("delivered");
    assert_eq!(interpreter.lines(), ["shadowed"]);
}

/// Matching hekla's derived ids is the whole reason the `uuid` crate is a dependency,
/// so the bytes are pinned rather than left as merely deterministic. The expected value
/// is RFC 4122 v5 of that seed and name, computed outside this crate.
#[test]
fn derive_produces_the_same_bytes_as_uuid_v5() {
    let program = program(
        "effect E {
  on @order.placed as e {
    invoke RecordNotified {
      order_id: e.order_id,
      notification_id: Uuid.derive(e.order_id, \"confirmation\"),
    }
  }
}",
    );
    let mut journal = Journal::default();
    let (interpreter, outcome) = deliver(&program, vec![placed(1, 7, 100)], vec![], &mut journal);
    outcome.expect("delivered");
    assert_eq!(
        interpreter.log()[1].event.field("notification_id"),
        Some(&Value::uuid("80d7a177-654b-5d44-aaaf-f40ba9b777ac")),
    );
}

#[test]
fn now_is_absent_from_a_fold_and_a_projector() {
    let message = err("effect E {
  on @order.placed as e {
    state at: Timestamp = fold e.at
      on @order.placed(customer_id: e.customer_id) => now()

    log(\"x\")
  }
}");
    assert_eq!(message, "`state` folds the log, so it cannot read a clock");

    let message = parse(
        "event @a.b { id: Uuid }
projector P {
  entity Row { id: Uuid @key, at: Timestamp? }
  on @a.b { id } { put Row { id, at: now() } }
}
",
    )
    .expect_err("a projector has no clock")
    .text();
    assert_eq!(
        message,
        "a projector has no clock, because a rebuild must reproduce every value it writes"
    );
}

#[test]
fn two_now_calls_in_one_body_agree() {
    let program = parse(
        "event @stamp.made { first: Timestamp, second: Timestamp }
command Stamp() {
  emit @stamp.made { first: now(), second: now() }
}
",
    )
    .expect("a command body has a clock");
    let mut interpreter = Interpreter::new(&program);
    interpreter
        .run("Stamp", Vec::<(String, Value)>::new())
        .expect("ran");

    let event = &interpreter.log()[0].event;
    // Pinned once, not once per call: one slot, filled before the body runs.
    assert_eq!(event.field("first"), event.field("second"));
}

#[test]
fn a_command_that_appends_nothing_still_reads_a_clock() {
    let program = parse(
        "event @a.b { id: Uuid }
command Give() {
  let at = now()
  return reject(\"no\", \"not today\")
}
",
    )
    .expect("a command body has a clock");
    let mut interpreter = Interpreter::new(&program);
    let execution = interpreter
        .run("Give", Vec::<(String, Value)>::new())
        .expect("the clock is pinned whatever the outcome");
    assert!(matches!(execution.outcome, heklang::Outcome::Reject { .. }));
    assert!(interpreter.log().is_empty());
}

#[test]
fn now_is_journaled_in_an_effect() {
    let program = program(
        "effect E {
  on @order.placed as e {
    http.post(\"https://mail.example/confirm\", { \"at\": now() })
  }
}",
    );
    let mut journal = Journal::default();
    let (_, outcome) = deliver(
        &program,
        vec![placed(1, 7, 100)],
        vec![Reply::Status(200)],
        &mut journal,
    );
    outcome.expect("delivered");
    assert!(
        journal
            .calls()
            .any(|(call, recorded)| call == "now()" && matches!(recorded, Recorded::Now(_))),
        "expected `now()` in the journal"
    );
}

// ---------------------------------------------------------------------------------
// Rule 12: `reveal` fails terminally.

const REVEALING: &str = "effect E {
  on @order.placed as e {
    http.post(\"https://mail.example/confirm\", { \"to\": reveal(e.email) })
    log(\"sent\")
  }
}";

#[test]
fn reveal_of_an_erased_subject_skips_terminally() {
    let program = program(REVEALING);
    let mut interpreter = Interpreter::with_log(&program, vec![placed(1, 7, 100)]);
    interpreter.script(URL, [Reply::Status(200)]);
    interpreter.erase_subject("customer_id", "7");

    let outcome = interpreter
        .deliver("E", 0, &mut Journal::default())
        .expect("terminal, not a wedge");
    let Invocation::Skipped(message) = outcome else {
        panic!("expected a terminal skip, got {outcome:?}");
    };
    assert!(
        message.starts_with("reveal cannot decrypt `email`"),
        "{message}"
    );
    // Terminal means the cursor advances, and it is counted apart from a wedge.
    let mut interpreter = Interpreter::with_log(&program, vec![placed(1, 7, 100)]);
    interpreter.erase_subject("customer_id", "7");
    let counts = interpreter.drive("E").expect("advanced");
    assert_eq!(counts.skipped(), 1);
    assert_eq!(counts.failed(), 0);
    assert!(counts.wedged.is_none());
}

#[test]
fn the_skip_message_says_the_erase_may_be_non_local() {
    let program = program(REVEALING);
    let mut interpreter = Interpreter::with_log(&program, vec![placed(1, 7, 100)]);
    interpreter.erase_subject("customer_id", "7");
    let Ok(Invocation::Skipped(message)) = interpreter.deliver("E", 0, &mut Journal::default())
    else {
        panic!("expected a terminal skip");
    };
    // Without this an operator hunts for an `erase` in a file that does not have one,
    // which rule 9 guarantees is the usual case.
    assert!(
        message.contains("The erase need not be in this effect"),
        "{message}"
    );
    assert!(message.contains("concurrent invocation"), "{message}");
}

// Rule 12: what `reveal` takes. The credential is folded off an event that happened
// long before the one being handled, which is the shape every real effect has.

const FOLDING: &str = "effect E {
  on @order.reviewed as e { customer_id } {
    state contact: String? = fold none
      on @order.placed(customer_id) { email } => email
      on @order.reconfirmed(customer_id) { email } => email

    if contact.is_none() {
      log(\"no orders\")
      return
    }
    http.post(\"https://mail.example/confirm\", { \"to\": reveal(contact) })
    log(\"sent\")
  }
}";

#[test]
fn a_fold_from_two_events_with_the_same_subject_reveals() {
    let program = program(FOLDING);
    let log = vec![
        placed(1, 7, 100),
        reconfirmed(2, 7, "grace@example.com"),
        reviewed(3, 7, None),
    ];

    let mut journal = Journal::default();
    let mut interpreter = Interpreter::with_log(&program, log.clone());
    interpreter.script(URL, [Reply::Status(200)]);
    interpreter
        .deliver("E", 2, &mut journal)
        .expect("a folded credential is revealable");
    // The later arm wrote last, so this is a fold rather than a first-match lookup.
    assert!(
        posted(&journal).contains("\"to\":\"grace@example.com\""),
        "{}",
        posted(&journal)
    );
    assert_eq!(interpreter.lines(), ["sent"]);

    // The seal carries the subject of the value it is holding, so erasing some other
    // customer changes nothing.
    let mut interpreter = Interpreter::with_log(&program, log.clone());
    interpreter.script(URL, [Reply::Status(200)]);
    interpreter.erase_subject("customer_id", "3");
    assert!(matches!(
        interpreter.deliver("E", 2, &mut Journal::default()),
        Ok(Invocation::Done)
    ));

    // Erasing the right one is rule 12's terminal skip, named for the field whose
    // content is unreadable rather than for the local the source happened to reveal.
    // The seal rides on the value now, so the field is what it can name, and that is
    // what `docs/effects.md` always documented.
    let mut interpreter = Interpreter::with_log(&program, log);
    interpreter.script(URL, [Reply::Status(200)]);
    interpreter.erase_subject("customer_id", "7");
    let Ok(Invocation::Skipped(message)) = interpreter.deliver("E", 2, &mut Journal::default())
    else {
        panic!("expected a terminal skip");
    };
    assert!(
        message.starts_with("reveal cannot decrypt `email`"),
        "{message}"
    );
}

/// Narrowing and rule 12 meet here: proving a folded value present says nothing about
/// where it came from, so the `reveal` still finds the subject the fold recorded.
#[test]
fn a_narrowed_optional_can_be_revealed() {
    let program = program(
        "effect E {
  on @order.reviewed as e { customer_id } {
    state contact: String? = fold none
      on @order.placed(customer_id) { email } => email

    if contact.is_some() {
      http.post(\"https://mail.example/confirm\", { \"to\": reveal(contact) })
      log(\"sent\")
    }
  }
}",
    );

    let mut journal = Journal::default();
    let mut interpreter =
        Interpreter::with_log(&program, vec![placed(1, 7, 100), reviewed(2, 7, None)]);
    interpreter.script(URL, [Reply::Status(200)]);
    interpreter
        .deliver("E", 1, &mut journal)
        .expect("a narrowed fold reveals");
    assert!(
        posted(&journal).contains("\"to\":\"ada@example.com\""),
        "{}",
        posted(&journal)
    );

    // The subject the fold recorded is still the one the key is looked up by.
    let mut interpreter =
        Interpreter::with_log(&program, vec![placed(1, 7, 100), reviewed(2, 7, None)]);
    interpreter.script(URL, [Reply::Status(200)]);
    interpreter.erase_subject("customer_id", "7");
    assert!(matches!(
        interpreter.deliver("E", 1, &mut Journal::default()),
        Ok(Invocation::Skipped(_))
    ));

    // Nothing folded, so the branch is not taken and nothing is revealed.
    let mut interpreter = Interpreter::with_log(&program, vec![reviewed(2, 7, None)]);
    interpreter
        .deliver("E", 0, &mut Journal::default())
        .expect("no orders");
    assert!(interpreter.lines().is_empty());
}

/// The seed is not subject-bound and cannot be: it is evaluated before the fold, with
/// no event behind it. Revealing a variable that never matched hands back what the
/// author wrote, without consulting a key store that has nothing to say about it.
#[test]
fn a_non_subject_seed_is_accepted() {
    let program = program(
        "effect E {
  on @order.reviewed as e { customer_id } {
    state contact: String = fold \"nobody\"
      on @order.placed(customer_id) { email } => email

    log(reveal(contact))
  }
}",
    );

    let mut interpreter = Interpreter::with_log(&program, vec![reviewed(1, 7, None)]);
    interpreter.erase_subject("customer_id", "7");
    interpreter
        .deliver("E", 0, &mut Journal::default())
        .expect("the seed was never sealed");
    assert_eq!(interpreter.lines(), ["nobody"]);
}

/// One variable holds one subject, because `reveal` names the key by it.
#[test]
fn two_arms_with_different_subjects_are_a_conflict() {
    let message = err("effect E {
  on @order.reviewed as e { customer_id, order_id } {
    state secret: String? = fold none
      on @order.placed(customer_id) { email } => email
      on @order.audited(order_id) { note } => note

    log(reveal(secret))
  }
}");
    assert!(
        message.contains("folds under two subjects"),
        "got: {message}"
    );
    assert!(message.contains("`customer_id`"), "got: {message}");
    assert!(message.contains("`auditor_id`"), "got: {message}");
}

/// A seed may be plain, an arm may not, and the order the two are written in does not
/// change the answer.
#[test]
fn a_non_subject_arm_into_a_subject_bound_variable_is_rejected() {
    let subject_first = err("effect E {
  on @order.reviewed as e { customer_id, order_id } {
    state secret: String? = fold none
      on @order.placed(customer_id) { email } => email
      on @order.audited(order_id) { tool } => tool

    log(reveal(secret))
  }
}");
    let plain_first = err("effect E {
  on @order.reviewed as e { customer_id, order_id } {
    state secret: String? = fold none
      on @order.audited(order_id) { tool } => tool
      on @order.placed(customer_id) { email } => email

    log(reveal(secret))
  }
}");
    for message in [&subject_first, &plain_first] {
        assert!(
            message.contains("cannot fold a plain one into it"),
            "got: {message}"
        );
        assert!(
            message.contains("A seed may be plain, an arm may not."),
            "got: {message}"
        );
        assert!(message.contains("@order.audited"), "got: {message}");
    }
}

/// The transform is now caught **at the transform**, not at the `reveal` further down.
/// That is the whole gain of putting the seal in the type: the error lands on the
/// mistake rather than on the line that suffers from it.
#[test]
fn a_transform_of_sealed_content_is_rejected_where_it_is_written() {
    let message = err("effect E {
  on @order.reviewed as e { customer_id } {
    state contact: String? = fold none
      on @order.placed(customer_id) { email } => email.trim()

    log(reveal(contact))
  }
}");
    assert!(
        message.contains("`trim` reads content sealed under `customer_id`"),
        "got: {message}"
    );
    assert!(message.contains("`reveal` it first"), "got: {message}");
}

/// `unwrap_or` gets its own reason, because it is the specific mistake a port makes:
/// a plaintext sentinel standing in for content that has a key.
#[test]
fn unwrap_or_on_sealed_content_names_the_mixture() {
    let message = err("effect E {
  on @order.reviewed as e { customer_id } {
    log(reveal(e.comment.unwrap_or(\"\")))
  }
}");
    assert!(
        message.contains("plaintext default and content sealed under `customer_id` in one slot"),
        "got: {message}"
    );
}

/// Presence is not content, so the one thing that may be asked of an unrevealed
/// optional is whether it holds anything at all.
#[test]
fn a_presence_check_needs_no_reveal() {
    program(
        "effect E {
  on @order.reviewed as e { customer_id } {
    if e.comment.is_none() {
      log(\"no comment\")
      return
    }
    log(\"has one\")
  }
}",
    );
}

/// A `let` keeps the seal. This is the case the old side channels laundered: the
/// binding lived on how the expression was spelled, so one `let` lost it.
#[test]
fn a_let_keeps_the_seal() {
    let program = program(
        "effect E {
  on @order.placed as e {
    let copy = e.email
    log(reveal(copy))
  }
}",
    );
    let mut journal = Journal::default();
    let (interpreter, outcome) = deliver(&program, vec![placed(1, 7, 100)], vec![], &mut journal);
    outcome.expect("delivered");
    assert_eq!(interpreter.lines(), ["ada@example.com"]);
}

/// The rules are about the variable, so they hold in a command as well as an effect.
/// A command cannot `reveal`, so what it records is inert; keeping one rule is worth an
/// unread field.
#[test]
fn the_fold_rules_hold_in_a_command_too() {
    let good = source(
        "command Check(customer_id: Int) {
  state secret: String? = fold none
    on @order.placed(customer_id) { email } => email

  if secret.is_none() {
    return reject(\"unknown\", \"no orders\")
  }
  return
}",
    );
    parse(&good).expect("a command may fold a subject-bound value");

    let message = parse(&source(
        "command Check(customer_id: Int, order_id: Uuid) {
  state secret: String? = fold none
    on @order.placed(customer_id) { email } => email
    on @order.audited(order_id) { tool } => tool

  return
}",
    ))
    .expect_err("a plain arm is a plain arm here too")
    .text();
    assert!(
        message.contains("cannot fold a plain one into it"),
        "got: {message}"
    );
}

/// The **inferring** form stays trigger-only. Rule 12's fold path tracks the subject of
/// a value, and this is the id itself, so there is no field name to recover from a
/// folded one. The error offers the other form rather than just refusing.
#[test]
fn the_inferring_erase_stays_on_the_trigger() {
    let message = err("effect E {
  on @order.reviewed as e { customer_id } {
    state who: Int? = fold none
      on @order.placed(customer_id) { customer_id } => customer_id

    erase(who)
  }
}");
    assert!(
        message.contains("`erase` takes a field of the triggering event"),
        "got: {message}"
    );
    assert!(
        message.contains("erase(customer_id, id)"),
        "expected the named form to be offered, got: {message}"
    );
}

// Rule 12: an optional in, an optional out. These two are the pair the rule exists for:
// the same program, the same fold, one subject that never had a comment and one whose
// key is gone. Collapsing them either way is the failure this is here to prevent.

const REVIEWING: &str = "effect E {
  on @order.reviewed as e {
    if reveal(e.comment).is_none() {
      log(\"nothing to moderate\")
      return
    }
    http.post(\"https://mail.example/confirm\", { \"comment\": reveal(e.comment) })
    log(\"moderated\")
  }
}";

#[test]
fn reveal_on_an_optional_is_none_when_the_field_was_never_set() {
    let program = program(REVIEWING);
    let mut interpreter = Interpreter::with_log(&program, vec![reviewed(1, 7, None)]);
    // The key store is never consulted, so an erased subject changes nothing here: an
    // absent value was never encrypted.
    interpreter.erase_subject("customer_id", "7");

    let outcome = interpreter
        .deliver("E", 0, &mut Journal::default())
        .expect("an absent field is an ordinary condition, not a failure");
    assert!(matches!(outcome, Invocation::Done), "got {outcome:?}");
    assert_eq!(interpreter.lines(), ["nothing to moderate"]);
}

#[test]
fn reveal_on_an_optional_still_skips_terminally_on_a_shredded_key() {
    let program = program(REVIEWING);
    let mut interpreter = Interpreter::with_log(&program, vec![reviewed(1, 7, Some("rude"))]);
    interpreter.script(URL, [Reply::Status(200)]);
    interpreter.erase_subject("customer_id", "7");

    let outcome = interpreter
        .deliver("E", 0, &mut Journal::default())
        .expect("terminal, not a wedge");
    let Invocation::Skipped(message) = outcome else {
        panic!("expected a terminal skip, got {outcome:?}");
    };
    assert!(
        message.starts_with("reveal cannot decrypt `comment`"),
        "{message}"
    );
}

#[test]
fn reveal_on_a_present_optional_hands_back_the_plaintext() {
    let program = program(REVIEWING);
    let mut journal = Journal::default();
    let (interpreter, outcome) = deliver(
        &program,
        vec![reviewed(1, 7, Some("rude"))],
        vec![Reply::Status(200)],
        &mut journal,
    );
    assert!(matches!(outcome, Ok(Invocation::Done)), "got {outcome:?}");
    assert!(
        posted(&journal).contains("\"comment\":\"rude\""),
        "{}",
        posted(&journal)
    );
    assert_eq!(interpreter.lines(), ["moderated"]);
}

/// A subject id is the name a key is filed under, so it has to be a value that is
/// always there and that does not itself need a key.
#[test]
fn a_subject_id_is_a_plain_field_that_always_has_a_value() {
    let message = parse(
        "event @e.happened { id: Int?, text: String @subject(id) }
effect E { on @e.happened as e { log(\"x\") } }
",
    )
    .expect_err("an optional subject id")
    .text();
    assert!(
        message.contains("names an optional field"),
        "got: {message}"
    );

    let message = parse(
        "event @e.happened { owner: Int, id: Int @subject(owner), text: String @subject(id) }
effect E { on @e.happened as e { log(\"x\") } }
",
    )
    .expect_err("an encrypted subject id")
    .text();
    assert!(
        message.contains("names a subject-encrypted field"),
        "got: {message}"
    );

    let message = parse(
        "event @e.happened { text: String @subject(nope) }
effect E { on @e.happened as e { log(\"x\") } }
",
    )
    .expect_err("a subject id that is not a field")
    .text();
    assert!(
        message.contains("names no field of @e.happened"),
        "got: {message}"
    );
}

/// A fold arm producing a `T` lands in a `T?` state as `some(T)`. Without that,
/// `.is_none()` on a folded optional is a method call on a bare `Int`.
#[test]
fn a_fold_into_an_optional_state_holds_an_optional() {
    let program = program(
        "effect E {
  on @order.reviewed as e { order_id } {
    state customer: Int? = fold none
      on @order.placed(order_id) { customer_id } => customer_id

    if customer.is_none() {
      log(\"no order\")
    } else {
      // Narrowed by the `is_none` above, so this is an `Int` rather than an `Int?`.
      log(\"customer {customer}\")
    }
  }
}",
    );

    let mut interpreter = Interpreter::with_log(&program, vec![reviewed(1, 7, None)]);
    interpreter
        .deliver("E", 0, &mut Journal::default())
        .expect("the seed alone");
    assert_eq!(interpreter.lines(), ["no order"]);

    let mut interpreter =
        Interpreter::with_log(&program, vec![placed(1, 7, 100), reviewed(1, 7, None)]);
    interpreter
        .deliver("E", 1, &mut Journal::default())
        .expect("one matching event");
    assert_eq!(interpreter.lines(), ["customer 7"]);
}

// ---------------------------------------------------------------------------------
// Rule 13: timeouts are configuration, not syntax.

#[test]
fn a_timeout_is_not_a_call_argument() {
    let message = err("effect E {
  on @order.placed as e {
    http.post(\"https://mail.example/confirm\", { \"to\": \"x\" }, 30)
  }
}");
    assert!(
        message.contains("a timeout is configuration rather than a call argument"),
        "got: {message}"
    );
}

// ---------------------------------------------------------------------------------
// Rule 14: verify mode stays.

#[test]
fn folding_an_arm_twice_gives_the_same_state() {
    let program = program(COUNTING);
    let log = vec![placed(1, 7, 100), placed(2, 7, 100)];

    let mut first = Journal::default();
    let (_, outcome) = deliver(&program, log.clone(), vec![Reply::Status(200)], &mut first);
    outcome.expect("delivered");

    let mut second = Journal::default();
    let (_, outcome) = deliver(&program, log, vec![Reply::Status(200)], &mut second);
    outcome.expect("delivered");

    // Two independent folds of the same prefix at the same position, compared. This is
    // what verify does, and what the closed builtin set makes cheap to guarantee.
    assert_eq!(posted(&first), posted(&second));
}

// ---------------------------------------------------------------------------------
// No effect may trigger itself.

const CYCLE: &str = "event @order.placed { order_id: Uuid }

command Replace(order_id: Uuid) {
  emit @order.placed { order_id }
}

effect Retry {
  on @order.placed as e {
    invoke Replace { order_id: e.order_id }
  }
}
";

#[test]
fn an_effect_that_can_trigger_itself_is_rejected() {
    let message = parse(CYCLE)
        .expect_err("this would grow the log without end")
        .text();
    assert!(
        message.contains("this effect can trigger itself"),
        "got: {message}"
    );
}

#[test]
fn the_cycle_error_names_the_path() {
    let message = parse(CYCLE).expect_err("rejected").text();
    assert!(
        message.starts_with("@order.placed -> Retry -> Replace -> @order.placed:"),
        "got: {message}"
    );
}

// ---------------------------------------------------------------------------------
// The destructure block is optional, for arms and handlers alike.

#[test]
fn an_arm_may_omit_the_destructure_block() {
    let one = program(
        "effect E {
  on @order.placed as e { log(\"one block\") }
}",
    );
    assert!(one.effects[0].arms[0].binds.is_empty());

    let two = program(
        "effect E {
  on @order.placed as e { order_id, total } { log(\"two blocks\") }
}",
    );
    assert_eq!(two.effects[0].arms[0].binds.len(), 2);
}

#[test]
fn a_lone_destructure_block_says_the_body_is_missing() {
    let message = err("effect E {
  on @order.placed as e { order_id, total }
}");
    assert_eq!(
        message,
        "this looks like a destructure block; a handler with one needs a body block after it"
    );
}

// ---------------------------------------------------------------------------------
// Statement gating: every wrong context teaches its own rule.

fn command_err(body: &str) -> String {
    parse(&format!(
        "{PRELUDE}command C(order_id: Uuid) {{\n{body}\n}}\n"
    ))
    .expect_err("expected this command to be rejected")
    .text()
}

fn projector_err(body: &str) -> String {
    parse(&format!(
        "{PRELUDE}projector P {{
  entity Row {{ order_id: Uuid @key }}
  on @order.placed {{ order_id }} {{\n{body}\n  }}
}}\n"
    ))
    .expect_err("expected this projector to be rejected")
    .text()
}

#[test]
fn emit_in_an_effect_points_at_invoke() {
    let message = err("effect E {
  on @order.placed as e {
    emit @order.notified { order_id: e.order_id, notification_id: e.order_id }
  }
}");
    assert_eq!(
        message,
        "an effect never appends events; call a command with `invoke`, which appends under its own guard"
    );
}

#[test]
fn a_projector_write_in_an_effect_is_rejected() {
    let message = err("effect E {
  on @order.placed as e {
    delete Row[e.order_id]
  }
}");
    assert_eq!(
        message,
        "`delete` writes an entity, so it can only appear in a projector"
    );
}

#[test]
fn invoke_outside_an_effect_is_rejected() {
    assert_eq!(
        command_err("  invoke RecordNotified { order_id, notification_id: order_id }\n  return"),
        "`invoke` calls a command, so it can only appear in an effect; a command that needs another command's work emits, and an effect reacts"
    );
    assert_eq!(
        projector_err("    invoke RecordNotified { order_id, notification_id: order_id }"),
        "`invoke` calls a command, so it can only appear in an effect; a projector is a pure fold"
    );
}

#[test]
fn fail_outside_an_effect_points_at_the_right_outcome() {
    assert_eq!(
        command_err("  fail(\"no\")\n  return"),
        "`fail` is an effect's terminal outcome; a command returns `invalid(...)` or `reject(...)`"
    );
}

#[test]
fn an_http_call_in_a_projector_is_rejected() {
    assert_eq!(
        projector_err("    http.get(\"https://x.example\")"),
        "a projector is a pure fold over the log, so it cannot make an HTTP call"
    );
}

#[test]
fn reveal_outside_an_effect_is_rejected() {
    assert_eq!(
        projector_err("    put Row { order_id: reveal(order_id) }"),
        "only an effect crosses the decrypt boundary; a projector stores what the event carries"
    );
}

#[test]
fn an_effect_has_no_guard() {
    let message = err("effect E {
  on @order.placed as e {
    guard @order.placed(order_id: e.order_id)
    log(\"x\")
  }
}");
    assert_eq!(
        message,
        "an effect has no `guard`; it appends nothing, so there is no append condition to build"
    );
}

#[test]
fn a_command_outcome_is_not_an_effect_outcome() {
    let message = err("effect E {
  on @order.placed as e {
    return reject(\"no\", \"not today\")
  }
}");
    assert_eq!(
        message,
        "`reject` is a command's outcome; an effect's terminal outcome is `fail(...)`"
    );
}

/// A plain field on purpose: the suggestion is about a missing `()`, and reaching for
/// a subject-bound field here would test the decrypt boundary instead.
#[test]
fn a_parenless_field_on_a_non_response_still_suggests_the_method() {
    let message = err("effect E {
  on @order.audited as e {
    log(e.tool.trim)
  }
}");
    assert_eq!(message, "no field `trim` on String; did you mean `trim()`?");
}

/// Rule 9 with a loop, which `docs/effects.md` predicted before there was one: an
/// `erase` anywhere in a loop body is reachable from every reveal in that body,
/// including one lexically above it, because the body runs again.
#[test]
fn an_erase_in_a_loop_poisons_the_whole_body() {
    let message = err("effect E {
  on @order.placed as e {
    for x in [1, 2] {
      log(reveal(e.email))
      erase(e.customer_id)
    }
  }
}");
    assert!(
        message.contains("can run after the `erase`"),
        "the reveal is lexically first and still rejected, got: {message}"
    );

    // The same two statements outside a loop are fine in that order, which is what
    // makes the loop rule a real rule rather than a restatement of the lexical one.
    program(
        "effect E {
  on @order.placed as e {
    log(reveal(e.email))
    erase(e.customer_id)
  }
}",
    );
}

// ---------------------------------------------------------------------------------
// Rule 1, second half: an arm may list several event types.

#[test]
fn an_arm_may_list_several_event_types() {
    let program = program(
        "effect E {
  on @order.placed, @order.cancelled as e { order_id } {
    log(\"touched {order_id}\")
  }
}",
    );
    let effect = program.effect("E").expect("declared");
    assert_eq!(effect.arms.len(), 1, "one arm, not two");
    assert_eq!(effect.arms[0].events.len(), 2);

    // Either type selects that one arm, which is rule 1's invariant unchanged.
    for (position, log) in [(0, vec![placed(1, 7, 100)]), (0, vec![cancelled(1, 7)])] {
        let mut journal = Journal::default();
        let mut interpreter = Interpreter::with_log(&program, log);
        interpreter
            .deliver("E", position, &mut journal)
            .expect("delivered");
        assert_eq!(interpreter.lines().len(), 1);
    }
}

/// The invariant is unchanged: listing a path in two arms is still an error naming the
/// first, whether it was listed alone or beside others.
#[test]
fn a_path_in_two_arms_is_still_rejected() {
    let message = err("effect E {
  on @order.placed, @order.cancelled as e { order_id } {
    log(\"one\")
  }

  on @order.cancelled as e {
    log(\"two\")
  }
}");
    assert!(
        message.contains("already has an arm on @order.cancelled"),
        "got: {message}"
    );

    // And a path listed twice within one arm is caught at the arm.
    let message = err("effect E {
  on @order.placed, @order.placed as e {
    log(\"x\")
  }
}");
    assert_eq!(message, "this arm already lists @order.placed");
}

/// Only what every listed type shares, checked on name, type and `@subject`, so a
/// `reveal` through a multi-path binding cannot reach a field that is encrypted on one
/// path and plain on another.
#[test]
fn a_multi_path_binding_names_only_shared_fields() {
    // `email` is on @order.placed and not on @order.cancelled.
    let message = err("effect E {
  on @order.placed, @order.cancelled as e {
    log(reveal(e.email))
  }
}");
    assert_eq!(
        message,
        "`email` is not shared by @order.placed, @order.cancelled, so an arm listing them cannot name it; a binding names only what every listed type has, with the same type and the same `@subject`"
    );

    // `customer_id` is on both, with the same type, so it is nameable.
    program(
        "effect E {
  on @order.placed, @order.cancelled as e {
    log(\"{e.customer_id}\")
  }
}",
    );
}

/// The cycle check walks `trigger -> emitted` per arm, so one arm invoking a command
/// whose event a *different* arm triggers on is not a cycle. That distinction only
/// holds if the graph is built per arm rather than per effect.
#[test]
fn two_arms_of_one_effect_are_two_nodes() {
    program(
        "effect E {
  on @order.placed as e {
    invoke RecordNotified {
      order_id: e.order_id,
      notification_id: e.order_id,
    }
  }

  on @order.notified as e {
    log(\"noticed\")
  }
}",
    );

    // The same arm doing both is the cycle, and it is still caught.
    let message = err("effect E {
  on @order.notified as e {
    invoke RecordNotified {
      order_id: e.order_id,
      notification_id: e.order_id,
    }
  }
}");
    assert!(message.contains("can trigger itself"), "got: {message}");
}

// ---------------------------------------------------------------------------------
// Rule 8, second half: `Json` as a declarable type, and headers on a call.

const NESTED: &str = r#"{"data":{"productCreate":{"product":{"id":"gid://x/7"},"userErrors":[{"message":"bad sku"}]}}}"#;

fn graphql_body() -> Json {
    Json::obj([(
        "data",
        Json::obj([(
            "productCreate",
            Json::obj([
                ("product", Json::obj([("id", Json::str("gid://x/7"))])),
                (
                    "userErrors",
                    Json::arr([Json::obj([("message", Json::str("bad sku"))])]),
                ),
            ]),
        )]),
    )])
}

/// A GraphQL response is nested, so one step down and one step into an array, both
/// optional for the same reason rule 8's three are.
#[test]
fn json_steps_down_and_into_an_array() {
    let program = program(
        "effect E {
  on @order.placed as e {
    let response = http.post(\"https://mail.example/confirm\", { \"q\": \"\" })
    let data = response.body.json(\"data\").unwrap_or(Json.empty)
    let errors = data.json(\"productCreate\").unwrap_or(Json.empty).array(\"userErrors\").unwrap_or([])
    log(\"{errors.len()} {data.json(\"nope\").is_none()}\")
  }
}",
    );
    let mut journal = Journal::default();
    let (interpreter, outcome) = deliver(
        &program,
        vec![placed(1, 7, 100)],
        vec![Reply::Body(200, graphql_body())],
        &mut journal,
    );
    outcome.expect("delivered");
    assert_eq!(interpreter.lines(), ["1 true"]);
    assert_eq!(
        NESTED,
        graphql_body().to_string(),
        "the fixture is the shape"
    );
}

/// `Json` was in the IR and unreachable from the grammar, which meant a command could
/// not take a webhook payload at all.
#[test]
fn json_is_a_declarable_type() {
    let source = format!(
        "{PRELUDE}
fn topic_of(payload: Json) -> String {{
  return payload.string(\"topic\").unwrap_or(\"unknown\")
}}

command Receive(order_id: Uuid, payload: Json) {{
  emit @order.notified {{ order_id, notification_id: order_id }}
}}
"
    );
    let program = parse(&source).unwrap_or_else(|err| panic!("expected this to parse: {err}"));
    assert_eq!(program.command("Receive").unwrap().params[1].ty, Type::Json);
    assert_eq!(
        program.function("topic_of").unwrap().ret,
        Some(Type::String)
    );
}

/// `Response` was in the same position `Json` had been in: a real type with a real
/// value and real field access, unreachable from the grammar, so a pure helper over a
/// response could be written everywhere except in its own signature.
#[test]
fn a_response_is_declarable_in_a_fn_signature() {
    let program = program(
        "fn graphql_error(response: Response, field: String) -> String? {
  if response.status >= 400 {
    return \"status {response.status}\"
  }
  let data = response.body.json(\"data\").unwrap_or(Json.empty)
  let errors = data.json(field).unwrap_or(Json.empty).array(\"userErrors\").unwrap_or([])
  for item in errors {
    return item.string(\"message\").unwrap_or(\"unknown error\")
  }
  return none
}

effect E {
  on @order.placed as e {
    let response = http.post(\"https://mail.example/confirm\", { \"q\": \"\" })
    log(\"{graphql_error(response, \"productCreate\").unwrap_or(\"ok\")}\")
  }
}",
    );
    assert_eq!(
        program.function("graphql_error").unwrap().params[0].ty,
        Type::Response
    );

    let mut journal = Journal::default();
    let (interpreter, outcome) = deliver(
        &program,
        vec![placed(1, 7, 100)],
        vec![Reply::Body(200, graphql_body())],
        &mut journal,
    );
    outcome.expect("delivered");
    assert_eq!(
        interpreter.lines(),
        ["bad sku"],
        "the helper read both halves of the response"
    );
}

/// A `Response` is transport, not data. Reading one is pure, so a helper may take one;
/// storing one is not, so nothing else may name it, and the rejection is the ordinary
/// unknown-type message rather than a special case.
#[test]
fn a_response_is_not_declarable_anywhere_else() {
    let cases = [
        ("an event field", "event @a.b { r: Response }"),
        (
            "a record field",
            "record R { r: Response }\nevent @a.b { x: Int }",
        ),
        (
            "a command parameter",
            "event @a.b { x: Int }\ncommand C(r: Response) { emit @a.b { x: 1 } }",
        ),
        (
            "an entity column",
            "event @a.b { x: Int }\nprojector P {\n  entity E { id: Int @key, r: Response }\n  on @a.b { x } { delete E[x] }\n}",
        ),
    ];
    for (what, source) in cases {
        assert_eq!(
            parse(source).unwrap_err().text(),
            "unknown type `Response`",
            "for {what}"
        );
    }
}

/// The allowance sits above the general type parser rather than inside its recursion,
/// so a container of them is still rejected in the one position that admits a bare one.
#[test]
fn a_container_of_responses_is_rejected() {
    for ty in ["List(Response)", "Map(String, Response)"] {
        let source = format!("event @a.b {{ x: Int }}\nfn f(rs: {ty}) -> Int {{\n  return 1\n}}\n");
        assert_eq!(
            parse(&source).unwrap_err().text(),
            "unknown type `Response`",
            "for {ty}"
        );
    }
}

/// Rule 8's table pointed at a string instead of a socket, which is why it cannot
/// disagree with what a request body would have carried.
#[test]
fn json_encode_is_the_same_table_as_a_body() {
    let program = program(
        "effect E {
  on @order.placed as e {
    log(Json.encode({ \"total\": e.total, \"id\": e.order_id }))
  }
}",
    );
    let mut journal = Journal::default();
    let (interpreter, outcome) = deliver(&program, vec![placed(1, 7, 2_599)], vec![], &mut journal);
    outcome.expect("delivered");
    assert_eq!(
        interpreter.lines(),
        [r#"{"id":"0190d1a1-0000-7000-8000-000000000001","total":"25.99"}"#],
        "Money keeps its scale as a string, exactly as it does in a body"
    );

    let message = err("effect E {
  on @order.placed as e {
    log(Json.nope(1))
  }
}");
    assert_eq!(
        message,
        "`Json` has no `nope`; it has `empty` and `encode(value)`"
    );
}

/// A named argument, so the positional-third-argument error keeps teaching rule 13.
#[test]
fn headers_are_a_named_argument() {
    let program = program(
        "effect E {
  on @order.placed as e {
    let response = http.post(
      \"https://mail.example/confirm\",
      { \"to\": reveal(e.email) },
      headers = {
        \"Authorization\": \"Bearer k\",
        \"Idempotency-Key\": \"{e.id}\",
      },
    )
    log(\"{response.status}\")
  }
}",
    );
    let mut journal = Journal::default();
    let (interpreter, outcome) = deliver(
        &program,
        vec![placed(1, 7, 100)],
        vec![Reply::Status(200)],
        &mut journal,
    );
    outcome.expect("delivered");

    let sent = &interpreter.requests()[0];
    assert_eq!(sent.verb, "http.post");
    let Json::Obj(headers) = &sent.headers else {
        panic!("expected an object, got {:?}", sent.headers);
    };
    assert_eq!(headers.get("Authorization"), Some(&Json::str("Bearer k")));
    // The case that matters beyond convenience: this is what stops a second send.
    assert!(headers.contains_key("Idempotency-Key"));

    // A positional third argument is still the timeout error.
    let message = err("effect E {
  on @order.placed as e {
    let response = http.post(\"https://x\", { \"a\": 1 }, 30)
    log(\"x\")
  }
}");
    assert!(
        message.contains("a timeout is configuration rather than a call argument"),
        "got: {message}"
    );
}

/// The journal key is the verb, the URL and the body. Not the headers: a changed
/// idempotency key has to land on the entry that already suppressed the send.
#[test]
fn a_changed_header_does_not_re_fire_a_journaled_call() {
    let program = program(
        "effect E {
  on @order.placed as e {
    let response = http.post(
      \"https://mail.example/confirm\",
      { \"to\": \"x\" },
      headers = { \"Idempotency-Key\": \"{e.position}\" },
    )
    log(\"sent\")
  }
}",
    );
    let mut journal = Journal::default();
    let (mut interpreter, outcome) = deliver(
        &program,
        vec![placed(1, 7, 100)],
        vec![Reply::Status(200), Reply::Status(200)],
        &mut journal,
    );
    outcome.expect("delivered");
    let performed = interpreter.http_calls();

    interpreter.deliver("E", 0, &mut journal).expect("replayed");
    assert_eq!(
        interpreter.http_calls(),
        performed,
        "the replay is a journal hit, so nothing left the process"
    );
}

/// An object literal is a `Json` value now, so it is legal where one is expected
/// rather than only inside a body. Rule 7 still holds, because `invoke` checks its
/// fields against declared parameter types.
#[test]
fn an_object_literal_is_legal_where_a_json_is_expected() {
    let source = format!(
        "{PRELUDE}
fn auth(token: String) -> Json {{
  return {{ \"Authorization\": \"Bearer {{token}}\" }}
}}

effect E {{
  on @order.placed as e {{
    let response = http.get(\"https://x\", headers = auth(\"k\"))
    log(\"{{response.status}}\")
  }}
}}
"
    );
    parse(&source).unwrap_or_else(|err| panic!("expected this to parse: {err}"));

    // Still rejected where nothing expects a `Json`, which is what keeps rule 7's
    // "an object literal is not an invoke input".
    let message = err("effect E {
  on @order.placed as e {
    let x = { \"a\": 1 }
    log(\"x\")
  }
}");
    assert!(
        message.starts_with("an object literal is an HTTP request body"),
        "got: {message}"
    );
}

/// The effect-side half of the trailing-comma rule. `http.*` is the interesting one,
/// because rule 13's arity error is written for a third positional argument and has to
/// keep firing for one.
#[test]
fn a_trailing_comma_closes_an_effect_builtin() {
    for body in [
        "log(\"x\",)",
        "fail(\"x\",)",
        "erase(e.customer_id,)",
        "let r = http.get(\"https://mail.example/confirm\",)",
        "let r = http.post(\"https://mail.example/confirm\", { \"to\": \"x\" },)",
        "let r = http.post(\"https://mail.example/confirm\", { \"to\": \"x\" }, headers = { \"K\": \"v\" },)",
    ] {
        let source = format!("effect E {{\n  on @order.placed as e {{\n    {body}\n  }}\n}}");
        parse(&self::source(&source)).unwrap_or_else(|err| panic!("for {body}: {err}"));
    }

    let third = parse(&source(
        "effect E {
  on @order.placed as e {
    let r = http.post(\"https://mail.example/confirm\", { \"to\": \"x\" }, 5,)
  }
}",
    ))
    .expect_err("a third positional argument is still rule 13")
    .text();
    assert_eq!(
        third,
        "`http.post` takes 2 arguments; a timeout is configuration rather than a call argument"
    );
}

// ---------------------------------------------------------------------------------
// Effect-local `fn`: a helper that may call out and `invoke`, and may not `reveal` or
// `erase`. See `docs/functions.md`.

#[test]
fn an_effect_local_fn_may_call_out() {
    let program = program(
        "effect E {
  fn confirm(to: String) -> Int {
    let response = http.post(\"https://mail.example/confirm\", { \"to\": to })
    return response.status
  }

  on @order.placed as e {
    log(\"status {confirm(reveal(e.email))}\")
  }
}",
    );
    let mut journal = Journal::default();
    let (interpreter, outcome) = deliver(
        &program,
        vec![placed(1, 7, 2_599)],
        vec![Reply::Status(202)],
        &mut journal,
    );
    assert_eq!(outcome.expect("delivered"), Invocation::Done);
    assert_eq!(interpreter.lines(), ["status 202"]);
    // The call was journaled from inside the helper, so a replay finds it.
    assert!(
        posted(&journal).contains("ada@example.com"),
        "got: {}",
        posted(&journal)
    );
}

#[test]
fn an_effect_local_fn_may_invoke() {
    let program = program(
        "effect E {
  fn notify(order_id: Uuid, notification_id: Uuid) -> String {
    let result = invoke RecordNotified { order_id, notification_id }
    return result.code().unwrap_or(\"ok\")
  }

  on @order.placed as e {
    log(notify(e.order_id, Uuid.derive(e.id, \"confirmation\")))
  }
}",
    );
    let mut journal = Journal::default();
    let (interpreter, outcome) = deliver(&program, vec![placed(1, 7, 100)], vec![], &mut journal);
    assert_eq!(outcome.expect("delivered"), Invocation::Done);
    assert_eq!(interpreter.lines(), ["ok"]);
    // The command really ran from inside the helper, so its event is in the log.
    assert_eq!(interpreter.log().len(), 2);
}

#[test]
fn a_fail_inside_an_effect_local_fn_ends_the_invocation() {
    let program = program(
        "effect E {
  fn confirm(to: String) -> Int {
    let response = http.post(\"https://mail.example/confirm\", { \"to\": to })
    if response.status >= 400 {
      fail(\"mail rejected {response.status}\")
    }
    return response.status
  }

  on @order.placed as e {
    log(\"status {confirm(reveal(e.email))}\")
    log(\"unreachable\")
  }
}",
    );
    let mut journal = Journal::default();
    let (interpreter, outcome) = deliver(
        &program,
        vec![placed(1, 7, 2_599)],
        vec![Reply::Status(422)],
        &mut journal,
    );
    // The same outcome and the same trace entry a `fail` in the arm produces: only the
    // channel it travelled on differs, because a call is an expression.
    assert_eq!(
        outcome.expect("delivered"),
        Invocation::Failed("mail rejected 422".to_string())
    );
    assert!(
        interpreter
            .trace()
            .contains(&Effectful::Failed("mail rejected 422".to_string())),
        "got: {:?}",
        interpreter.trace()
    );
    assert!(
        interpreter.lines().is_empty(),
        "the arm stopped at the call"
    );
}

/// The journal counts calls per invocation, not per frame. Two identical requests, one
/// in the arm and one in the helper, are two entries and replay to their own answers.
#[test]
fn the_journal_counts_a_call_across_a_fn_boundary() {
    let program = program(
        "effect E {
  fn ping() -> Int {
    return http.post(\"https://mail.example/confirm\", { \"to\": \"ada\" }).status
  }

  on @order.placed as e {
    let first = http.post(\"https://mail.example/confirm\", { \"to\": \"ada\" }).status
    log(\"{first} then {ping()}\")
  }
}",
    );
    let mut journal = Journal::default();
    let (interpreter, outcome) = deliver(
        &program,
        vec![placed(1, 7, 100)],
        vec![Reply::Status(201), Reply::Status(202)],
        &mut journal,
    );
    outcome.expect("delivered");
    assert_eq!(interpreter.lines(), ["201 then 202"]);

    // A replay reads the same two entries back in the same order. Sharing one ordinal
    // counter is what makes that true; a fresh one per call would answer the helper
    // with the arm's own recording.
    let replayed = Interpreter::with_log(&program, vec![placed(1, 7, 100)])
        .deliver("E", 0, &mut journal)
        .expect("replayed");
    assert_eq!(replayed, Invocation::Done);
}

#[test]
fn an_effect_local_fn_may_be_declared_after_its_use() {
    let program = program(
        "effect E {
  on @order.placed as e { log(greeting(e.customer_id)) }

  fn greeting(customer_id: Int) -> String {
    return \"hello {customer_id}\"
  }
}",
    );
    let mut journal = Journal::default();
    let (interpreter, outcome) = deliver(&program, vec![placed(1, 7, 100)], vec![], &mut journal);
    outcome.expect("delivered");
    assert_eq!(interpreter.lines(), ["hello 7"]);
}

#[test]
fn an_effect_local_fn_may_call_another() {
    let program = program(
        "effect E {
  fn outer(customer_id: Int) -> String { return \"[{inner(customer_id)}]\" }
  fn inner(customer_id: Int) -> String { return \"c{customer_id}\" }

  on @order.placed as e { log(outer(e.customer_id)) }
}",
    );
    let mut journal = Journal::default();
    let (interpreter, outcome) = deliver(&program, vec![placed(1, 7, 100)], vec![], &mut journal);
    outcome.expect("delivered");
    assert_eq!(interpreter.lines(), ["[c7]"]);
}

#[test]
fn a_cycle_between_effect_local_fns_is_rejected() {
    let message = err("effect E {
  fn a(n: Int) -> Int { return b(n) }
  fn b(n: Int) -> Int { return a(n) }

  on @order.placed as e { log(\"{a(1)}\") }
}");
    assert!(
        message.contains("`a` calls `b` calls `a`"),
        "expected the cycle as a path, got: {message}"
    );
    assert!(
        message.contains("so that every call ends"),
        "got: {message}"
    );
}

#[test]
fn an_effect_local_fn_may_not_reveal() {
    let message = err("effect E {
  fn confirm(email: String) -> Int {
    return http.post(\"https://mail.example/confirm\", { \"to\": reveal(email) }).status
  }

  on @order.placed as e { log(\"{confirm(e.email)}\") }
}");
    assert!(
        message.contains("an effect-local `fn` cannot decrypt"),
        "got: {message}"
    );
    assert!(
        message.contains("pass the revealed value in as a parameter"),
        "expected the fix the port already writes, got: {message}"
    );
    assert!(
        message.contains("erase-last check inside one statement tree"),
        "expected the rule it keeps, got: {message}"
    );
}

#[test]
fn an_effect_local_fn_may_not_erase() {
    let message = err("effect E {
  fn forget(customer_id: Int) -> Bool {
    erase(customer_id)
    return true
  }

  on @order.placed as e { log(\"{forget(e.customer_id)}\") }
}");
    assert!(
        message.contains("an effect-local `fn` cannot erase a subject key"),
        "got: {message}"
    );
    assert!(
        message.contains("erase-last check inside one statement tree"),
        "got: {message}"
    );
}

#[test]
fn an_effect_local_fn_is_not_visible_from_another_effect() {
    let message = err("effect E {
  fn shared(n: Int) -> Int { return n }
  on @order.placed as e { log(\"{shared(1)}\") }
}
effect F {
  on @order.cancelled as e { log(\"{shared(1)}\") }
}");
    assert_eq!(message, "`shared` is not in scope");
}

#[test]
fn two_effects_may_each_declare_the_same_helper() {
    let program = program(
        "effect E {
  fn label(n: Int) -> String { return \"E{n}\" }
  on @order.placed as e { log(label(e.customer_id)) }
}
effect F {
  fn label(n: Int) -> String { return \"F{n}\" }
  on @order.cancelled as e { log(label(e.customer_id)) }
}",
    );
    let mut journal = Journal::default();
    let mut interpreter = Interpreter::with_log(&program, vec![placed(1, 7, 100), cancelled(2, 9)]);
    interpreter.deliver("E", 0, &mut journal).expect("E");
    interpreter.deliver("F", 1, &mut journal).expect("F");
    assert_eq!(interpreter.lines(), ["E7", "F9"]);
}

#[test]
fn an_effect_local_fn_may_not_shadow_a_module_fn() {
    let message = err("fn label(n: Int) -> String { return \"{n}\" }
effect E {
  fn label(n: Int) -> String { return \"{n}\" }
  on @order.placed as e { log(label(1)) }
}");
    assert!(
        message.contains("`label` is already a `fn` at module scope"),
        "got: {message}"
    );
    assert!(message.contains("cannot shadow it"), "got: {message}");
}

#[test]
fn one_effect_may_not_declare_a_helper_twice() {
    let message = err("effect E {
  fn label(n: Int) -> String { return \"a{n}\" }
  fn label(n: Int) -> String { return \"b{n}\" }
  on @order.placed as e { log(label(1)) }
}");
    assert_eq!(message, "`E` already declares a `fn` named `label`");
}

/// Rule 3: a fold reproduces without a journal, and this is the one helper that could
/// call out. A module `fn` is pure by construction, so a fold may still call one.
#[test]
fn a_fold_arm_may_not_call_an_effect_local_fn() {
    let message = err("effect E {
  fn bump(n: Int) -> Int { return n + 1 }

  on @order.placed as e {
    state seen: Int = fold 0
      on @order.placed(customer_id: e.customer_id) => bump(seen)
    log(\"{seen}\")
  }
}");
    assert_eq!(
        message,
        "`state` folds the log, so it cannot call `bump`, which may call out"
    );
}

#[test]
fn an_effect_local_fn_has_no_state() {
    let message = err("effect E {
  fn count(customer_id: Int) -> Int {
    state seen: Int = fold 0
      on @order.placed(customer_id) => seen + 1
    return seen
  }

  on @order.placed as e { log(\"{count(e.customer_id)}\") }
}");
    assert!(
        message.contains("an effect-local `fn` has no `state`"),
        "got: {message}"
    );
    assert!(
        message.contains("pass what it decided in as a parameter"),
        "got: {message}"
    );
}

/// Rule 11 pins the clock once per invocation, into a slot the arm fills before its
/// body runs. A helper has no such slot.
#[test]
fn an_effect_local_fn_may_not_read_the_clock() {
    let message = err("effect E {
  fn stamp() -> Timestamp { return now() }
  on @order.placed as e { log(\"{stamp()}\") }
}");
    assert!(
        message.contains("an effect-local `fn` cannot read a clock"),
        "got: {message}"
    );
    assert!(
        message.contains("read it in the arm and pass it in"),
        "got: {message}"
    );
}

#[test]
fn an_effect_local_fn_cannot_emit() {
    let message = err("effect E {
  fn append(order_id: Uuid) -> Bool {
    emit @order.notified { order_id, notification_id: order_id }
    return true
  }

  on @order.placed as e { log(\"{append(e.order_id)}\") }
}");
    assert!(
        message.contains("an effect never appends events"),
        "got: {message}"
    );
}

#[test]
fn an_effect_local_fn_may_return_nothing() {
    let program = program(
        "effect E {
  fn confirm(order_id: Uuid, to: String) {
    let response = http.post(\"https://mail.example/confirm\", { \"to\": to })
    if response.status >= 400 {
      fail(\"mail rejected {response.status}\")
    }
    invoke RecordNotified { order_id, notification_id: order_id }
    log(\"confirmed\")
  }

  on @order.placed as e {
    confirm(e.order_id, reveal(e.email))
  }
}",
    );
    let mut journal = Journal::default();
    let (interpreter, outcome) = deliver(
        &program,
        vec![placed(1, 7, 2_599)],
        vec![Reply::Status(200)],
        &mut journal,
    );
    assert_eq!(outcome.expect("delivered"), Invocation::Done);
    assert_eq!(interpreter.lines(), ["confirmed"]);
    assert_eq!(interpreter.log().len(), 2);
}

/// A `return` with no value leaves the helper, not the arm. That is what makes a void
/// helper usable as an early-exit guard the way the arm's own `return` is not.
#[test]
fn a_bare_return_leaves_the_helper_and_not_the_arm() {
    let program = program(
        "effect E {
  fn confirm(to: String) {
    let response = http.post(\"https://mail.example/confirm\", { \"to\": to })
    if response.status == 401 {
      log(\"unauthorized, skipping\")
      return
    }
    log(\"confirmed\")
  }

  on @order.placed as e {
    confirm(reveal(e.email))
    log(\"arm finished\")
  }
}",
    );
    let mut journal = Journal::default();
    let (interpreter, outcome) = deliver(
        &program,
        vec![placed(1, 7, 100)],
        vec![Reply::Status(401)],
        &mut journal,
    );
    assert_eq!(outcome.expect("delivered"), Invocation::Done);
    assert_eq!(
        interpreter.lines(),
        ["unauthorized, skipping", "arm finished"]
    );
}

#[test]
fn a_void_call_is_a_statement_rather_than_a_value() {
    let message = err("effect E {
  fn confirm(to: String) { log(to) }
  on @order.placed as e { let done = confirm(reveal(e.email)) }
}");
    assert_eq!(
        message,
        "`confirm` returns nothing, so a call to it is a statement rather than a value"
    );
}

#[test]
fn a_return_in_a_void_fn_takes_no_value() {
    let message = err("effect E {
  fn confirm(to: String) { return to }
  on @order.placed as e { confirm(reveal(e.email)) }
}");
    assert_eq!(
        message,
        "this `fn` returns nothing, so `return` takes no value"
    );
}

/// The cycle check reaches a call that is a statement, which has no `Expr::CallFn` for
/// the expression walk to find.
#[test]
fn a_cycle_between_void_helpers_is_rejected() {
    let message = err("effect E {
  fn a(n: Int) { b(n) }
  fn b(n: Int) { a(n) }

  on @order.placed as e { a(1) }
}");
    assert!(
        message.contains("`a` calls `b` calls `a`"),
        "got: {message}"
    );
}

// ---------------------------------------------------------------------------------
// Rule 12: the seal is in the type. `@subject(...)` is the authored form and
// `Type::Sealed` is what propagates from it. See `docs/effects.md`.

#[test]
fn a_subject_bound_field_has_a_sealed_type() {
    let program = program("effect E {\n  on @order.placed as e { log(\"x\") }\n}");
    let def = program
        .event(&EventPath::new(["order", "placed"]))
        .expect("the prelude declares it");

    let email = &def.field("email").expect("declared").ty;
    assert_eq!(email, &Type::sealed(Type::String, "customer_id"));
    assert_eq!(email.subject().map(String::as_str), Some("customer_id"));
    assert_eq!(email.unsealed(), Type::String);

    // A plain field carries no seal, and asking is not an error.
    let plain = &def.field("customer_id").expect("declared").ty;
    assert_eq!(plain, &Type::Int);
    assert_eq!(plain.subject(), None);
}

/// `Opt` stays outermost, so everything that looks through an optional keeps working
/// with one extra unwrap rather than two orderings to remember.
#[test]
fn an_optional_subject_bound_field_seals_inside_the_optional() {
    let program = program("effect E {\n  on @order.placed as e { log(\"x\") }\n}");
    let def = program
        .event(&EventPath::new(["order", "reviewed"]))
        .expect("the prelude declares it");

    let comment = &def.field("comment").expect("declared").ty;
    assert_eq!(
        comment,
        &Type::opt(Type::sealed(Type::String, "customer_id"))
    );
    assert_eq!(comment.subject().map(String::as_str), Some("customer_id"));
    assert_eq!(comment.unsealed(), Type::opt(Type::String));
}

// ---------------------------------------------------------------------------------
// Rule 12: content behind the seal cannot be read without `reveal`. Each of these
// passed `hek check` before the seal moved into the type, and each is broken in hekla,
// where the field is a handle. See `docs/effects.md`.

fn leaks(body: &str) -> String {
    err(&format!(
        "effect E {{\n  on @order.placed as e {{ customer_id }} {{\n    {body}\n  }}\n}}"
    ))
}

#[test]
fn sealed_content_cannot_be_sent_in_a_body() {
    let message = leaks("http.post(\"https://mail.example/confirm\", { \"email\": e.email })");
    assert!(
        message.contains("cannot be sent in a request body without `reveal`"),
        "got: {message}"
    );
    assert!(
        message.contains("sealed under `customer_id`"),
        "got: {message}"
    );
}

#[test]
fn sealed_content_cannot_be_interpolated() {
    let message = leaks("log(\"email is {e.email}\")");
    assert!(
        message.contains("cannot be interpolated into a string without `reveal`"),
        "got: {message}"
    );
}

#[test]
fn sealed_content_cannot_be_logged() {
    let message = leaks("log(e.email)");
    assert!(message.contains("and a String is not"), "got: {message}");
}

#[test]
fn sealed_content_cannot_be_passed_to_a_command() {
    let message = leaks("invoke RecordNotified { order_id: e.order_id, notification_id: e.email }");
    assert!(
        message.contains("takes it out from behind the decrypt boundary"),
        "got: {message}"
    );
}

#[test]
fn sealed_content_cannot_be_compared() {
    let message = leaks("if e.email == \"ada@example.com\" { log(\"hi\") }");
    assert!(
        message.contains("cannot be compared without `reveal`"),
        "got: {message}"
    );
}

/// The other direction is free, because it is the encrypting one: a plain value written
/// into a sealed field needs no ceremony, and sealed content written into the same seal
/// is only moving.
#[test]
fn sealed_content_may_be_written_into_the_same_seal() {
    program(
        "command Reconfirm(order_id: Uuid, customer_id: Int) {
  guard @order.reconfirmed(order_id)

  state held: String = fold \"\"
    on @order.placed(customer_id) { email } => email

  emit @order.reconfirmed { order_id, customer_id, email: held }
}",
    );
}

/// The path the port depends on: a projector may never `reveal`, and stores
/// subject-bound columns constantly. Rule 9's propagation is what makes it legal.
#[test]
fn a_projector_may_store_sealed_content_without_revealing() {
    program(
        "projector Orders {
  entity Row {
    order_id: Uuid @key,
    email: String,
  }

  on @order.placed as e { order_id, email } {
    put Row { order_id, email }
  }
}",
    );
}

/// The cascade backstop counts depth, not volume. It used to count how many events one
/// walk appended, which is a limit on how much work an effect may do rather than on
/// whether it settles: a port tripped it at seventeen sales, each of which appended one
/// event that triggered nothing.
#[test]
fn a_wide_walk_settles_rather_than_tripping_the_backstop() {
    let source = "event @tick.happened { id: Int }
event @tick.done { id: Int }

command RecordDone(id: Int) {
  emit @tick.done { id }
}

effect E {
  on @tick.happened { id } {
    invoke RecordDone { id }
  }
}";
    let program = parse(source).expect("this parses");
    let log: Vec<Event> = (0..200)
        .map(|id| {
            Event::new(
                EventPath::new(["tick", "happened"]),
                [("id", Value::Int(id))],
            )
        })
        .collect();

    let mut interpreter = Interpreter::with_log(&program, log);
    let counts = interpreter
        .drive("E")
        .expect("two hundred is a busy log, not a runaway");
    assert_eq!(counts.done, 200, "every source event was handled");
    assert!(counts.wedged.is_none());
}

// ---------------------------------------------------------------------------
// Rule 8's table read the other way: JSON as a declared type reads it.
// ---------------------------------------------------------------------------

const SHAPES: &str = "enum Tier { @default Free, Paid }
record Line { sku: String, qty: Int, price: Money(3), note: String? }";

fn shapes() -> Program {
    parse(&format!("{PRELUDE}\n{SHAPES}\n")).expect("parses")
}

/// Everything `Json::from_value` can write, read back as the type that wrote it. The
/// two directions are one table, so a value that survives this cannot be re-typed by a
/// round trip through a store.
#[test]
fn the_conversion_table_round_trips() {
    let program = shapes();
    let defs = Defs::of(&program);
    let cases: Vec<(Type, Value)> = vec![
        (Type::Bool, Value::Bool(true)),
        (Type::Int, Value::Int(-9)),
        (Type::String, Value::str("ada")),
        (
            Type::Uuid,
            Value::uuid("0190d1a1-0000-7000-8000-000000000001"),
        ),
        (Type::Timestamp, Value::Timestamp(1_700_000_000_000_000)),
        (Type::Money(2), Value::money(2_599, 2)),
        (Type::Decimal(4), Value::decimal(-12_345, 4)),
        (
            Type::Enum("Tier".to_string()),
            Value::Enum {
                ty: "Tier".to_string(),
                variant: "Paid".to_string(),
            },
        ),
        (Type::Opt(Box::new(Type::Int)), Value::none(Type::Int)),
        (Type::Opt(Box::new(Type::Int)), Value::some(Value::Int(3))),
        (
            Type::List(Box::new(Type::Int)),
            Value::list(Type::Int, [Value::Int(1), Value::Int(2)]),
        ),
        (
            Type::Map(Box::new(Type::Int), Box::new(Type::String)),
            Value::map(
                Type::Int,
                Type::String,
                [(Key::Int(7), Value::str("seven"))],
            ),
        ),
        (
            Type::Record("Line".to_string()),
            Value::record(
                "Line",
                [
                    ("sku", Value::str("A1")),
                    ("qty", Value::Int(2)),
                    ("price", Value::money(1_500, 3)),
                    ("note", Value::none(Type::String)),
                ],
            ),
        ),
    ];

    for (ty, value) in cases {
        let written = Json::from_value(&value);
        let read = Value::from_json(&written, &ty, defs).expect("reads back");
        assert_eq!(read, value, "{ty} did not survive the round trip");
    }
}

/// The same string is a different value at a different scale, which is the whole reason
/// the reader takes a type rather than inferring one from the JSON.
#[test]
fn a_scale_comes_from_the_declaration_and_not_from_the_text() {
    let program = shapes();
    let defs = Defs::of(&program);
    let written = Json::str("1.5");

    assert_eq!(
        Value::from_json(&written, &Type::Money(2), defs).expect("reads"),
        Value::money(150, 2)
    );
    assert_eq!(
        Value::from_json(&written, &Type::Money(3), defs).expect("reads"),
        Value::money(1_500, 3)
    );
}

/// More places than the target holds is a failure rather than a silent round, which is
/// the rule a written literal already follows.
#[test]
fn a_decimal_that_does_not_fit_its_scale_is_a_mismatch() {
    let program = shapes();
    let defs = Defs::of(&program);
    let err = Value::from_json(&Json::str("1.555"), &Type::Money(2), defs).expect_err("too fine");
    assert_eq!(err.expected, Type::Money(2));
}

/// A `Uuid` is checked, not merely typed. This is the one row of the table that reaches
/// past the program: it becomes a tag a host indexes on, a read-model key it paginates
/// by, and a seed `Uuid.derive` fails on, and it arrives from outside (a request body, a
/// stored record) rather than from the parser, which checks a written literal already.
#[test]
fn text_that_is_not_a_uuid_is_a_mismatch() {
    let program = shapes();
    let defs = Defs::of(&program);

    let err = Value::from_json(&Json::str("not-a-uuid"), &Type::Uuid, defs).expect_err("not one");
    assert_eq!(err.expected, Type::Uuid);
    assert_eq!(err.found, "text that is not a uuid");

    let good = "11111111-1111-1111-1111-111111111111";
    assert_eq!(
        Value::from_json(&Json::str(good), &Type::Uuid, defs).expect("a real one"),
        Value::uuid(good),
    );
}

/// The case this exists for: a field that was an `Int` when the record was written and
/// is read back under a program that has changed. The answer names where it was, so an
/// operator can see which field moved.
#[test]
fn a_stored_value_of_the_wrong_shape_names_where_it_was() {
    let program = shapes();
    let defs = Defs::of(&program);
    let written = Json::obj([
        ("sku", Json::str("A1")),
        ("qty", Json::str("two")),
        ("price", Json::str("1.500")),
        ("note", Json::Null),
    ]);

    let err = Value::from_json(&written, &Type::Record("Line".to_string()), defs)
        .expect_err("qty is not an Int");

    assert_eq!(err.path, vec!["qty".to_string()]);
    assert_eq!(err.expected, Type::Int);
    assert_eq!(err.found, "a string");
    let rendered = err.to_string();
    assert!(rendered.starts_with("qty: expected "), "{rendered}");
    assert!(rendered.ends_with("stored a string"), "{rendered}");
}

/// An absent key reads as `null`, so a missing optional is absent and a missing required
/// field is the mismatch it actually is rather than a zero quietly standing in.
#[test]
fn an_absent_key_fills_an_optional_and_fails_a_required_field() {
    let program = shapes();
    let defs = Defs::of(&program);

    let complete = Json::obj([
        ("sku", Json::str("A1")),
        ("qty", Json::int(2)),
        ("price", Json::str("1.500")),
    ]);
    let value =
        Value::from_json(&complete, &Type::Record("Line".to_string()), defs).expect("reads");
    let Value::Record { fields, .. } = value else {
        panic!("a record");
    };
    assert_eq!(fields.get("note"), Some(&Value::none(Type::String)));

    let short = Json::obj([("sku", Json::str("A1"))]);
    let err = Value::from_json(&short, &Type::Record("Line".to_string()), defs)
        .expect_err("qty is missing");
    assert_eq!(err.path, vec!["qty".to_string()]);
    assert_eq!(err.found, "null");
}

/// A seal carries the subject's id, which lives in a sibling field rather than in this
/// value, so it cannot come back out of the JSON. A host rebuilds it, and this reads the
/// content the seal was around.
#[test]
fn a_seal_is_not_in_the_json() {
    let program = shapes();
    let defs = Defs::of(&program);
    let ty = Type::Sealed(Box::new(Type::String), "customer_id".to_string());

    let value = Value::from_json(&Json::str("ada@example.com"), &ty, defs).expect("reads");
    assert_eq!(value, Value::str("ada@example.com"));
}

/// An enum is checked against its declaration, so a variant that was removed is a
/// mismatch rather than a value nothing else in the program can match on.
#[test]
fn an_unknown_variant_is_a_mismatch() {
    let program = shapes();
    let defs = Defs::of(&program);
    let err = Value::from_json(&Json::str("Trial"), &Type::Enum("Tier".to_string()), defs)
        .expect_err("Trial is not a Tier");
    assert_eq!(err.found, "a variant it does not have");
}

// ---------------------------------------------------------------------------------
// Rule 8: a number the author typed into a body is JSON's number, and a heklang value
// crossing the boundary is still the table's string.

/// Before this, `{ "amount": 10.5 }` went out as `{"amount":"10.5"}` while
/// `{ "count": 7 }` went out as `{"count":7}`, so no expression in the language produced
/// a fractional JSON number and `{"amount": 10.5}` was unreachable.
#[test]
fn a_numeric_literal_in_a_body_is_a_json_number() {
    let program = program(
        "effect E {
  on @order.placed as e { total } {
    let response = http.post(\"https://mail.example/confirm\", {
      \"whole\": 7,
      \"frac\": 10.5,
      \"neg\": -0.25,
      \"arr\": [1.5, 2],
      \"nested\": { \"deep\": 0.001 },
      \"arith\": 1 + 2,
      \"from_money\": total,
      \"quoted\": \"10.5\",
    })
    log(\"{response.status}\")
  }
}",
    );
    let mut journal = Journal::default();
    let (interpreter, outcome) = deliver(
        &program,
        vec![placed(1, 7, 1050)],
        vec![Reply::Status(200)],
        &mut journal,
    );
    outcome.expect("delivered");

    let body = interpreter.requests()[0]
        .body
        .clone()
        .expect("a post has a body");
    assert_eq!(
        body.to_string(),
        "{\"arith\":3,\"arr\":[1.5,2],\"frac\":10.5,\"from_money\":\"10.50\",\
         \"neg\":-0.25,\"nested\":{\"deep\":0.001},\"quoted\":\"10.5\",\"whole\":7}"
    );
}

/// The exact text, so nothing is rounded between the wire and a declared scale. A float
/// would lose this on the way in and there is no float in the language to lose it with.
#[test]
fn a_body_number_reads_back_as_its_exact_text() {
    let program = program(
        "effect E {
  on @order.placed as e {
    let response = http.get(\"https://mail.example/confirm\")
    log(\"{response.body.number(\"price\").unwrap_or(\"<none>\")}\")
    log(\"{response.body.number(\"whole\").unwrap_or(\"<none>\")}\")
    log(\"{response.body.int(\"price\").unwrap_or(-1)}\")
    log(\"{response.body.number(\"missing\").unwrap_or(\"<none>\")}\")
  }
}",
    );
    let mut journal = Journal::default();
    let body = Json::obj([
        ("price", Json::num("0.30000000000000004")),
        ("whole", Json::int(7)),
    ]);
    let (interpreter, outcome) = deliver(
        &program,
        vec![placed(1, 7, 100)],
        vec![Reply::Body(200, body)],
        &mut journal,
    );
    outcome.expect("delivered");

    assert_eq!(
        interpreter.lines(),
        [
            "0.30000000000000004",
            "7",
            // `int` answers only for a whole number, which is the `none` a missing key
            // gives rather than a truncation.
            "-1",
            "<none>",
        ]
    );
}
