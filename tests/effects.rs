//! `docs/effects.md` as executable tests, one test per numbered rule.

use heklang::{
    Event, EventPath, Interpreter, Invocation, Invoked, Journal, Json, Program, Recorded, Reply,
    Type, Value, parse,
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
        .message
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
        message.contains("`order_id` is not subject-encrypted"),
        "got: {message}"
    );
}

// ---------------------------------------------------------------------------------
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
    .message;
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

    // The companion tracks the subject of the value it is holding, so erasing some
    // other customer changes nothing.
    let mut interpreter = Interpreter::with_log(&program, log.clone());
    interpreter.script(URL, [Reply::Status(200)]);
    interpreter.erase_subject("customer_id", "3");
    assert!(matches!(
        interpreter.deliver("E", 2, &mut Journal::default()),
        Ok(Invocation::Done)
    ));

    // Erasing the right one is rule 12's terminal skip, named for the variable the
    // source reveals rather than for the field it folded.
    let mut interpreter = Interpreter::with_log(&program, log);
    interpreter.script(URL, [Reply::Status(200)]);
    interpreter.erase_subject("customer_id", "7");
    let Ok(Invocation::Skipped(message)) = interpreter.deliver("E", 2, &mut Journal::default())
    else {
        panic!("expected a terminal skip");
    };
    assert!(
        message.starts_with("reveal cannot decrypt `contact`"),
        "{message}"
    );
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

/// Subject-ness is a property of the field, so anything computed from it is a new value
/// the schema says nothing about. That is the same line a trigger field is held to.
#[test]
fn a_transformed_arm_drops_the_binding() {
    let message = err("effect E {
  on @order.reviewed as e { customer_id } {
    state contact: String? = fold none
      on @order.placed(customer_id) { email } => email.trim()

    log(reveal(contact))
  }
}");
    assert!(
        message.contains("`contact` folds no subject-bound value"),
        "got: {message}"
    );
    assert!(message.contains("drops the binding"), "got: {message}");
}

/// `erase` names a subject id rather than a subject-bound value, so it stays on the
/// trigger: an id folded off an earlier event is not one this arm can be sure of.
#[test]
fn erase_still_takes_a_field_of_the_trigger() {
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
    .message;
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
    .message;
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
    .message;
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
      log(\"customer {customer.unwrap_or(0)}\")
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
        .message;
    assert!(
        message.contains("this effect can trigger itself"),
        "got: {message}"
    );
}

#[test]
fn the_cycle_error_names_the_path() {
    let message = parse(CYCLE).expect_err("rejected").message;
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
    .message
}

fn projector_err(body: &str) -> String {
    parse(&format!(
        "{PRELUDE}projector P {{
  entity Row {{ order_id: Uuid @key }}
  on @order.placed {{ order_id }} {{\n{body}\n  }}
}}\n"
    ))
    .expect_err("expected this projector to be rejected")
    .message
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

#[test]
fn a_parenless_field_on_a_non_response_still_suggests_the_method() {
    let message = err("effect E {
  on @order.placed as e {
    log(e.email.trim)
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
    assert_eq!(program.function("topic_of").unwrap().ret, Type::String);
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
