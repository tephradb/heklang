//! `docs/host.md` as executable tests. The point of the seam is that something other
//! than the harness can drive the interpreter, so these use a host the crate does not
//! ship: if any of it needed a `Harness`, the seam would not be one.

use std::cell::RefCell;

use heklang::{
    AppendCondition, Attempt, Calls, Clock, Error, ErrorKind, Event, EventPath, Harness, Http,
    Ident, Interpreter, Key, Keys, Log, Outcome, Predicate, Program, Query, Record, Recorded,
    Reply, Request, Row, Rows, Span, Value, parse,
};

const PRELUDE: &str = "event @order.placed {
  order_id: Uuid,
  customer_id: Int,
  total: Money(2),
}
";

const ORDER: &str = "0190d1a1-0000-7000-8000-000000000001";

fn program(body: &str) -> Program {
    parse(&format!("{PRELUDE}{body}\n")).expect("parses")
}

/// A log that stamps its own envelopes, a clock that is not the log's length, and a
/// network that answers one thing. Nothing here is the harness.
#[derive(Default)]
struct Elsewhere {
    records: Vec<Record>,
    /// Every query the interpreter asked with, so a test can assert that the predicate
    /// and the range reached the host rather than being applied after a full scan.
    asked: RefCell<Vec<Query>>,
    shredded: Vec<(String, String)>,
    /// Every seal this host was asked to open, so a test can assert that a key is used
    /// once per `reveal` rather than once per record a fold walks.
    opened: RefCell<Vec<(String, String)>>,
    /// How many more appends to beat to the log before letting one through. Each one
    /// lands a rival event inside the slice first, which is the shape a real store's
    /// condition check produces rather than an error invented for the test.
    beat: usize,
    /// Whose order the rival places, so it falls inside the slice under test.
    rival: i64,
}

impl Log for Elsewhere {
    fn head(&self) -> Result<u64, Error> {
        Ok(self.records.len() as u64)
    }

    fn record(&self, position: u64) -> Result<Option<Record>, Error> {
        Ok(self.records.get(position as usize).cloned())
    }

    fn read(
        &self,
        query: &Query,
        visit: &mut dyn FnMut(&Record) -> Result<(), Error>,
    ) -> Result<(), Error> {
        self.asked.borrow_mut().push(query.clone());
        let last = query.upto.unwrap_or(u64::MAX);
        for record in &self.records {
            if record.position > last {
                break;
            }
            if record.position < query.from {
                continue;
            }
            if query
                .slices
                .iter()
                .any(|slice| slice.event == record.event.path)
            {
                visit(record)?;
            }
        }
        Ok(())
    }

    fn append(&mut self, events: &[Event], condition: &AppendCondition) -> Result<(), Error> {
        if self.beat > 0 {
            self.beat -= 1;
            let position = self.records.len() as u64;
            self.records.push(order(position, self.rival));
            return Err(ErrorKind::Conflict {
                after: condition.after,
            }
            .into());
        }
        for event in events {
            let position = self.records.len() as u64;
            self.records.push(Record::new(
                format!("elsewhere-{position}"),
                position,
                1_700_000_000_000_000 + position as i64,
                event.clone(),
            ));
        }
        Ok(())
    }
}

impl Clock for Elsewhere {
    fn now(&self) -> i64 {
        1_700_000_000_000_000
    }
}

impl Keys for Elsewhere {
    /// A store that really does keep something other than the plaintext. The transform
    /// is a reversal rather than a cipher, which is enough to prove the seam: heklang
    /// hands back what this returns, so it cannot have been reading the stored form.
    fn decrypt(
        &self,
        subject: &str,
        id: &str,
        _field: &str,
        content: &str,
    ) -> Result<Option<String>, Error> {
        self.opened
            .borrow_mut()
            .push((subject.to_string(), id.to_string()));
        if self
            .shredded
            .iter()
            .any(|(one, other)| one == subject && other == id)
        {
            return Ok(None);
        }
        Ok(Some(content.chars().rev().collect()))
    }

    fn erase(&mut self, subject: &str, id: &str) -> Result<(), Error> {
        self.shredded.push((subject.to_string(), id.to_string()));
        Ok(())
    }
}

impl Http for Elsewhere {
    fn send(&mut self, _request: &Request) -> Attempt {
        Attempt::Response {
            status: 200,
            body: heklang::Json::Null,
        }
    }
}

const COUNTING: &str = "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
  state open: Int = fold 0
    on @order.placed(customer_id) => open + 1

  emit @order.placed { order_id, customer_id, total }
}";

fn placed(customer: i64) -> Vec<(&'static str, Value)> {
    vec![
        ("order_id", Value::uuid(ORDER)),
        ("customer_id", Value::Int(customer)),
        ("total", Value::money(2_599, 2)),
    ]
}

#[test]
fn a_command_runs_against_a_host_the_crate_does_not_ship() {
    let program = program(COUNTING);
    let mut interpreter = Interpreter::with_host(&program, Elsewhere::default());
    let execution = interpreter.run("Place", placed(7)).expect("ran");
    assert!(matches!(execution.outcome, Outcome::Ok(ref events) if events.len() == 1));
}

/// The host stamps the envelope, so what the log holds is its identity and its instant,
/// not one derived from the position by the interpreter.
#[test]
fn the_host_stamps_what_it_appended() {
    let program = program(COUNTING);
    let mut interpreter = Interpreter::with_host(&program, Elsewhere::default());
    interpreter.run("Place", placed(7)).expect("ran");
    let record = interpreter
        .host()
        .record(0)
        .expect("read")
        .expect("one event landed");
    assert_eq!(record.id, "elsewhere-0");
    assert_eq!(record.at, 1_700_000_000_000_000);
}

/// The whole point of resolving a filter: the host is handed the values, so it can
/// narrow from an index instead of being handed every event and told to scan.
#[test]
fn the_predicate_reaches_the_host() {
    let program = program(COUNTING);
    let seeded = Elsewhere {
        records: vec![order(0, 7), order(1, 9)],
        ..Elsewhere::default()
    };
    let mut interpreter = Interpreter::with_host(&program, seeded);
    interpreter.run("Place", placed(7)).expect("ran");

    let asked = interpreter.host().asked.borrow();
    let [query] = &asked[..] else {
        panic!("one fold, one read, got {}", asked.len());
    };
    assert_eq!(
        query.slices,
        [Predicate::new(
            EventPath::new(["order", "placed"]),
            vec![("customer_id".to_string(), Value::Int(7))],
        )]
    );
    assert_eq!(query.from, 0, "a first attempt folds from the start");
    assert_eq!(
        query.upto,
        Some(1),
        "a command folds to the head it took `after` from, and no further: reading \
         past it would decide on events the condition is about to refuse"
    );
}

/// An empty log has nothing in any slice, so the read is not made at all. Worth
/// asserting rather than leaving to chance: it is what the bounded range costs, and a
/// host that counted its reads would otherwise see the number move for no reason.
#[test]
fn a_command_against_an_empty_log_does_not_read() {
    let program = program(COUNTING);
    let mut interpreter = Interpreter::with_host(&program, Elsewhere::default());
    let execution = interpreter.run("Place", placed(7)).expect("ran");

    assert!(matches!(execution.outcome, Outcome::Ok(_)));
    assert!(
        interpreter.host().asked.borrow().is_empty(),
        "there is no position to visit"
    );
}

/// A run that refuses still read the log, so a host that wants to cache or trace the
/// decision is told what it depended on.
#[test]
fn the_condition_comes_back_resolved() {
    let program = program(COUNTING);
    let mut interpreter = Interpreter::with_host(&program, Elsewhere::default());
    let execution = interpreter.run("Place", placed(7)).expect("ran");
    assert_eq!(execution.condition.after, 0);
    assert_eq!(
        execution.condition.slices[0].filters,
        [("customer_id".to_string(), Value::Int(7))]
    );
}

const CAPPED: &str = "refusal AtCapacity \"two is the limit\"
command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
  state open: Int = fold 0
    on @order.placed(customer_id) => open + 1

  if open >= 2 { return reject AtCapacity }
  emit @order.placed { order_id, customer_id, total }
}";

/// Section 5, the part a host cannot do for itself. One event is seeded and a rival
/// lands a second one at the append, so the retry has exactly one event to catch up on.
///
/// Two properties, and both fail loudly rather than quietly:
///
/// - **The retry reads only the delta.** `from` is where the last attempt stopped, so a
///   conflict on a boundary of any depth costs the events that beat it. A host looping
///   over `run` would ask for `from: 0` twice.
/// - **The delta lands on the state that attempt built, not on the seed.** Folding one
///   rival event onto `0` gives `open = 1` and commits a third order past a cap of two.
///   Folding it onto the `1` the first attempt reached gives `2` and refuses.
#[test]
fn a_retry_folds_the_delta_onto_what_the_last_attempt_built() {
    let program = program(CAPPED);
    let host = Elsewhere {
        records: vec![order(0, 7)],
        beat: 1,
        rival: 7,
        ..Elsewhere::default()
    };
    let mut interpreter = Interpreter::with_host(&program, host);
    let mut conflicts = Vec::new();
    let execution = interpreter
        .run_retrying("Place", placed(7), &mut |attempt| {
            conflicts.push(attempt);
            true
        })
        .expect("ran");

    assert_eq!(conflicts, [0], "one conflict, reported once, zero-based");
    assert!(
        matches!(&execution.outcome, Outcome::Reject { code, .. } if code == "at_capacity"),
        "the retry decided on the event that beat it: {:?}",
        execution.outcome
    );
    assert_eq!(
        execution.condition.after, 2,
        "and conditioned on the head it folded to, not the one the first attempt took"
    );

    let asked = interpreter.host().asked.borrow();
    let [first, second] = &asked[..] else {
        panic!("one read per attempt, got {}", asked.len());
    };
    assert_eq!((first.from, first.upto), (0, Some(0)));
    assert_eq!(
        (second.from, second.upto),
        (1, Some(1)),
        "the retry asks for what landed since and nothing it already folded"
    );
}

/// The default is still one attempt. `run` is what every caller that has no retry policy
/// uses, and a conflict is its error rather than something it silently absorbs.
#[test]
fn run_raises_a_conflict_rather_than_retrying_it() {
    let program = program(CAPPED);
    let host = Elsewhere {
        beat: 1,
        rival: 7,
        ..Elsewhere::default()
    };
    let mut interpreter = Interpreter::with_host(&program, host);
    let err = interpreter.run("Place", placed(7)).expect_err("conflicts");
    assert!(
        matches!(err.kind, ErrorKind::Conflict { after: 0 }),
        "{err}"
    );
}

/// A second run folds the first one's event, which is only true if the host's append
/// and the host's read are the same log.
#[test]
fn a_second_run_folds_what_the_first_appended() {
    let program = program(
        "refusal OnePerCustomer \"already ordered\"
command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
  state open: Int = fold 0
    on @order.placed(customer_id) => open + 1

  if open > 0 { return reject OnePerCustomer }
  emit @order.placed { order_id, customer_id, total }
}",
    );
    let mut interpreter = Interpreter::with_host(&program, Elsewhere::default());
    interpreter.run("Place", placed(7)).expect("ran");

    let second = interpreter.run("Place", placed(7)).expect("ran");
    assert!(
        matches!(second.outcome, Outcome::Reject { ref code, .. } if code == "one_per_customer")
    );
    assert_eq!(second.condition.after, 1, "the head moved");
}

// ---------------------------------------------------------------------------------
// `Keys`, and the one thing a seal is for.

const SEALED: &str = "event @shop.connected {
  shop_id: Int,
  token: String @subject(shop_id),
}
effect Use {
  on @order.placed as e {
    state token: String? = fold none
      on @shop.connected(shop_id: e.customer_id) { token } => token

    log(reveal(token).unwrap_or(\"nothing\"))
  }
}";

/// A seal is opaque, and the proof is that heklang hands back what the host said rather
/// than what the log held. `Elsewhere` reverses its content, which no plaintext path
/// could produce.
#[test]
fn a_seal_is_read_through_the_host_and_not_off_the_value() {
    let program = program(SEALED);
    let host = Elsewhere {
        records: vec![connected(0, 7, "s3cret"), order(1, 7)],
        ..Elsewhere::default()
    };
    let mut interpreter = Interpreter::with_host(&program, host);
    let mut journal = heklang::Journal::default();
    interpreter.deliver("Use", 1, &mut journal).expect("ran");

    assert_eq!(
        interpreter.lines(),
        ["terc3s"],
        "the host's answer, reversed, rather than the stored text"
    );
}

/// The whole point of the change. A fold walks every record in its boundary and a
/// key is used once, for the one `reveal` that asked: eager decryption cost a key use
/// per record, for content nothing read.
#[test]
fn a_fold_opens_only_the_seal_it_reveals() {
    let program = program(SEALED);
    let mut records = vec![];
    for position in 0..20 {
        records.push(connected(position, 7, "s3cret"));
    }
    records.push(order(20, 7));
    let host = Elsewhere {
        records,
        ..Elsewhere::default()
    };
    let mut interpreter = Interpreter::with_host(&program, host);
    let mut journal = heklang::Journal::default();
    interpreter.deliver("Use", 20, &mut journal).expect("ran");

    let opened = interpreter.host().opened.borrow();
    assert_eq!(
        opened.len(),
        1,
        "twenty records folded, one `reveal`, one key used: {opened:?}"
    );
    assert_eq!(opened[0], ("shop_id".to_string(), "7".to_string()));
}

/// An absent optional does not consult the key store at all (`docs/effects.md` rule
/// 12): it was never sealed, so there is nothing to open and no key that could be
/// missing for it.
#[test]
fn an_absent_optional_opens_nothing() {
    let program = program(SEALED);
    let host = Elsewhere {
        records: vec![order(0, 7)],
        ..Elsewhere::default()
    };
    let mut interpreter = Interpreter::with_host(&program, host);
    let mut journal = heklang::Journal::default();
    interpreter.deliver("Use", 0, &mut journal).expect("ran");

    assert_eq!(interpreter.lines(), ["nothing"]);
    assert!(
        interpreter.host().opened.borrow().is_empty(),
        "no seal, so no key"
    );
}

/// A destroyed key is `None` from the host and a terminal skip in the program, which is
/// the row rule 12's table keeps apart from the absent one above. The host is still
/// asked, because only it can tell the two apart.
#[test]
fn a_destroyed_key_is_terminal_rather_than_absent() {
    let program = program(SEALED);
    let host = Elsewhere {
        records: vec![connected(0, 7, "s3cret"), order(1, 7)],
        shredded: vec![("shop_id".to_string(), "7".to_string())],
        ..Elsewhere::default()
    };
    let mut interpreter = Interpreter::with_host(&program, host);
    let mut journal = heklang::Journal::default();
    let outcome = interpreter.deliver("Use", 1, &mut journal).expect("ran");

    let heklang::Invocation::Skipped(why) = outcome else {
        panic!("expected a terminal skip, got {outcome:?}");
    };
    assert!(why.contains("has been erased"), "got: {why}");
    assert_eq!(
        interpreter.host().opened.borrow().len(),
        1,
        "the host is what knows the key is gone, so it is still asked"
    );
}

fn connected(position: u64, shop: i64, token: &str) -> Record {
    Record::new(
        format!("r{position}"),
        position,
        0,
        Event::new(
            EventPath::new(["shop", "connected"]),
            [("shop_id", Value::Int(shop)), ("token", Value::str(token))],
        ),
    )
}

// ---------------------------------------------------------------------------------
// The journal, as somebody else's store.

/// A journal that is not `Journal`. Flat rather than keyed, so nothing about it is
/// borrowed from the harness's shape.
#[derive(Default)]
struct Ledger {
    rows: Vec<(String, u32, Recorded)>,
}

impl Calls for Ledger {
    fn recorded(&self, call: &str, ordinal: u32) -> Result<Option<Recorded>, Error> {
        Ok(self
            .rows
            .iter()
            .find(|(seen, at, _)| seen == call && *at == ordinal)
            .map(|(_, _, recorded)| recorded.clone()))
    }

    fn record(&mut self, call: &str, ordinal: u32, recorded: Recorded) -> Result<(), Error> {
        self.rows.push((call.to_string(), ordinal, recorded));
        Ok(())
    }
}

const NOTIFY: &str = "effect Notify {
  on @order.placed as e {
    let response = http.post(\"https://mail.example/confirm\", { \"order\": e.order_id })
    if response.status >= 400 { fail(\"rejected\") }
  }
}";

/// The whole of durable execution: the second delivery finds the call recorded and does
/// not perform it again. That the journal is the host's store rather than heklang's is
/// exactly what has to work for an effect to survive a restart.
#[test]
fn a_journal_the_host_owns_makes_the_second_delivery_a_replay() {
    let program = program(NOTIFY);
    let mut interpreter = Interpreter::new(&program);
    interpreter.append(Event::new(
        EventPath::new(["order", "placed"]),
        [
            ("order_id".to_string(), Value::uuid(ORDER)),
            ("customer_id".to_string(), Value::Int(7)),
            ("total".to_string(), Value::money(1, 2)),
        ],
    ));
    interpreter.script("https://mail.example/confirm", [Reply::Status(200)]);

    let mut ledger = Ledger::default();
    interpreter.deliver("Notify", 0, &mut ledger).expect("ran");
    assert_eq!(interpreter.http_calls(), 1);
    assert_eq!(ledger.rows.len(), 1, "one call, one row");

    interpreter
        .deliver("Notify", 0, &mut ledger)
        .expect("replayed");
    assert_eq!(
        interpreter.http_calls(),
        1,
        "the recorded response answered it, so nothing left twice"
    );
}

// ---------------------------------------------------------------------------------
// The condition, as a question a host answers. Pure values, no interpreter: this is
// the definition every host has to agree with.

fn order(position: u64, customer: i64) -> Record {
    Record::new(
        format!("r{position}"),
        position,
        0,
        Event::new(
            EventPath::new(["order", "placed"]),
            [
                ("order_id".to_string(), Value::uuid(ORDER)),
                ("customer_id".to_string(), Value::Int(customer)),
                ("total".to_string(), Value::money(1, 2)),
            ],
        ),
    )
}

fn narrowed_to(customer: i64) -> AppendCondition {
    AppendCondition {
        after: 2,
        slices: vec![Predicate::new(
            EventPath::new(["order", "placed"]),
            vec![("customer_id".to_string(), Value::Int(customer))],
        )],
    }
}

#[test]
fn a_slice_event_at_the_boundary_conflicts() {
    assert!(
        narrowed_to(7).conflicts(&[order(2, 7)]),
        "`after` is the head the run folded, so an event there is one it did not see"
    );
}

#[test]
fn an_event_the_run_already_folded_does_not_conflict() {
    assert!(!narrowed_to(7).conflicts(&[order(1, 7)]));
}

#[test]
fn an_event_outside_the_filter_does_not_conflict() {
    assert!(
        !narrowed_to(7).conflicts(&[order(9, 8)]),
        "another customer's order is not in this slice, however late it lands"
    );
}

#[test]
fn an_event_of_another_type_does_not_conflict() {
    let elsewhere = Record::new(
        "r9",
        9,
        0,
        Event::new(
            EventPath::new(["order", "cancelled"]),
            [("order_id".to_string(), Value::uuid(ORDER))],
        ),
    );
    assert!(!narrowed_to(7).conflicts(&[elsewhere]));
}

#[test]
fn a_command_that_read_nothing_conflicts_with_nothing() {
    let condition = AppendCondition {
        after: 0,
        slices: Vec::new(),
    };
    assert!(
        !condition.conflicts(&[order(0, 7), order(1, 8)]),
        "a command with no `state` declared no slice, so nothing can beat it to one"
    );
}

/// The harness enforces it, which is what makes the condition a rule rather than a
/// value nobody reads.
#[test]
fn a_stale_condition_is_refused() {
    let mut harness = Harness::with_log([Event::new(
        EventPath::new(["order", "placed"]),
        [
            ("order_id".to_string(), Value::uuid(ORDER)),
            ("customer_id".to_string(), Value::Int(7)),
            ("total".to_string(), Value::money(1, 2)),
        ],
    )]);
    let stale = AppendCondition {
        after: 0,
        slices: vec![Predicate::new(
            EventPath::new(["order", "placed"]),
            vec![("customer_id".to_string(), Value::Int(7))],
        )],
    };
    let refused = harness
        .append(&[], &stale)
        .expect_err("the log moved under it");
    assert_eq!(
        refused.to_string(),
        "the log moved under this run: something it read landed at or after position 0"
    );
}

// ---------------------------------------------------------------------------
// Read models, which are the seam a projector writes through
// ---------------------------------------------------------------------------

const TALLY: &str = "projector Orders {
  entity Tally {
    customer_id: Int @key,
    orders: Int,
    last: Money(2),
  }

  on @order.placed { customer_id, total } {
    patch Tally[customer_id] {
      orders: .orders + 1,
      last: total,
    }
  }
}

projector Strict {
  entity Running {
    customer_id: Int @key,
    orders: Int,
  }

  on @order.placed { customer_id } {
    update Running[customer_id] { orders: .orders + 1 }
  }
}";

const TWICE: &str = "projector Twice {
  entity Count {
    customer_id: Int @key,
    hits: Int,
  }

  on @order.placed { customer_id } {
    patch Count[customer_id] { hits: .hits + 1 }
  }

  on @order.placed { customer_id } {
    patch Count[customer_id] { hits: .hits + 1 }
  }
}";

/// Read models that are not a `Store`: a flat list, so nothing here can accidentally be
/// the interpreter's own map.
#[derive(Default)]
struct Shelf {
    rows: Vec<(String, Key, Row)>,
    /// Fails every write, so a host's own failure can be watched arriving.
    broken: bool,
}

impl Rows for Shelf {
    fn row(&self, entity: &str, key: &Key) -> Result<Option<Row>, Error> {
        Ok(self
            .rows
            .iter()
            .find(|(name, at, _)| name == entity && at == key)
            .map(|(_, _, row)| row.clone()))
    }

    fn put(&mut self, entity: &Ident, key: Key, row: Row) -> Result<(), Error> {
        if self.broken {
            return Err(Error::new(ErrorKind::Host(
                "the shelf fell over".to_string(),
            )));
        }
        self.rows
            .retain(|(name, at, _)| !(name == entity && at == &key));
        self.rows.push((entity.clone(), key, row));
        Ok(())
    }

    fn delete(&mut self, entity: &Ident, key: &Key) -> Result<(), Error> {
        self.rows
            .retain(|(name, at, _)| !(name == entity && at == key));
        Ok(())
    }
}

/// One `@order.placed` per customer named, in the order named.
fn logged(customers: &[i64]) -> Elsewhere {
    let mut host = Elsewhere::default();
    for (index, customer) in customers.iter().enumerate() {
        let position = index as u64;
        host.records.push(Record::new(
            format!("elsewhere-{position}"),
            position,
            1_700_000_000_000_000 + position as i64,
            Event::new(
                EventPath::new(["order", "placed"]),
                [
                    ("order_id", Value::uuid(ORDER)),
                    ("customer_id", Value::Int(*customer)),
                    ("total", Value::money(2_599, 2)),
                ],
            ),
        ));
    }
    host
}

#[test]
fn a_projection_writes_into_read_models_the_crate_does_not_ship() {
    let program = program(TALLY);
    let interpreter = Interpreter::with_host(&program, logged(&[7, 7, 9]));
    let mut shelf = Shelf::default();
    interpreter
        .project_into("Orders", &mut shelf)
        .expect("projected");

    assert_eq!(shelf.rows.len(), 2, "one row per customer");
    let seven = shelf
        .row("Tally", &Key::Int(7))
        .expect("read")
        .expect("row");
    assert_eq!(seven.field("orders"), Some(&Value::Int(2)));
    assert_eq!(seven.field("last"), Some(&Value::money(2_599, 2)));
}

#[test]
fn the_query_a_projection_reads_with_is_one_predicate_per_path() {
    let program = program(TALLY);
    let host = logged(&[7]);
    let interpreter = Interpreter::with_host(&program, host);
    let mut shelf = Shelf::default();
    interpreter
        .project_into("Orders", &mut shelf)
        .expect("projected");

    let asked = interpreter.host().asked.borrow();
    assert_eq!(asked.len(), 1);
    assert_eq!(
        asked[0].slices,
        vec![Predicate::new(
            EventPath::new(["order", "placed"]),
            Vec::new()
        )]
    );
    assert_eq!(asked[0].upto, None, "a projector reads to the head");
}

#[test]
fn patch_materializes_an_absent_row_and_update_drops_the_write() {
    let program = program(TALLY);
    let interpreter = Interpreter::with_host(&program, logged(&[7]));

    let mut patched = Shelf::default();
    interpreter
        .project_into("Orders", &mut patched)
        .expect("projected");
    assert_eq!(patched.rows.len(), 1, "a patch materializes from zeros");

    let mut updated = Shelf::default();
    interpreter
        .project_into("Strict", &mut updated)
        .expect("projected");
    assert!(updated.rows.is_empty(), "an update drops the write");
}

#[test]
fn two_handlers_on_one_record_read_each_others_writes() {
    let program = program(TWICE);
    let interpreter = Interpreter::with_host(&program, logged(&[7]));
    let mut shelf = Shelf::default();
    interpreter
        .project_into("Twice", &mut shelf)
        .expect("projected");

    let row = shelf
        .row("Count", &Key::Int(7))
        .expect("read")
        .expect("row");
    assert_eq!(
        row.field("hits"),
        Some(&Value::Int(2)),
        "the second handler folded the first handler's write, not the zero row"
    );
}

#[test]
fn a_read_model_failure_arrives_as_a_host_error_at_the_statement() {
    let program = program(TALLY);
    let interpreter = Interpreter::with_host(&program, logged(&[7]));
    let mut shelf = Shelf {
        broken: true,
        ..Shelf::default()
    };
    let err = interpreter
        .project_into("Orders", &mut shelf)
        .expect_err("the shelf fell over");

    assert!(matches!(err.kind, ErrorKind::Host(ref why) if why == "the shelf fell over"));
    assert_ne!(
        err.span,
        Span::default(),
        "a host knows what went wrong and not where it was asked from, so the statement fills it in"
    );
}
