//! `docs/host.md` as executable tests. The point of the seam is that something other
//! than the harness can drive the interpreter, so these use a host the crate does not
//! ship: if any of it needed a `Harness`, the seam would not be one.

use std::cell::RefCell;

use heklang::{
    AppendCondition, Attempt, Clock, Error, Event, EventPath, Http, Interpreter, Keys, Log,
    Outcome, Predicate, Program, Query, Record, Request, Value, parse,
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
    /// reached the host rather than being applied after a full scan.
    asked: RefCell<Vec<Query>>,
    shredded: Vec<(String, String)>,
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

    fn append(&mut self, events: &[Event], _condition: &AppendCondition) -> Result<(), Error> {
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
    fn erased(&self, subject: &str, id: &str) -> Result<bool, Error> {
        Ok(self
            .shredded
            .iter()
            .any(|(one, other)| one == subject && other == id))
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
    let mut interpreter = Interpreter::with_host(&program, Elsewhere::default());
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
    assert_eq!(query.upto, None, "a command folds to the head");
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

/// A second run folds the first one's event, which is only true if the host's append
/// and the host's read are the same log.
#[test]
fn a_second_run_folds_what_the_first_appended() {
    let program = program(
        "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
  state open: Int = fold 0
    on @order.placed(customer_id) => open + 1

  if open > 0 { return reject(\"one_per_customer\", \"already ordered\") }
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
