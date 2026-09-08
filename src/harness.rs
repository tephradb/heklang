//! The in-memory world, and the only one until a host brings its own.
//!
//! Everything here stands in for something a runtime owns: a log, a key store and a
//! network. Keeping it in its own module is what stops "what the harness does" from
//! reading as "what the language does", a distinction `docs/testing.md` rule 8 already
//! depends on.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use crate::host::{
    AppendCondition, Attempt, Calls, Clock, Http, Keys, Log, Query, Recorded, Request, Secrets,
};
use crate::interp::{Error, ErrorKind, Store};
use crate::ir::Ident;
use crate::testing::World;
use crate::value::{Event, Json, Record};

/// 2020-01-01T00:00:00Z, so a synthesised envelope timestamp reads as a plausible
/// instant rather than the epoch.
const EPOCH_MICROS: i64 = 1_577_836_800_000_000;
const MINUTE_MICROS: i64 = 60_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Status(u16),
    Body(u16, Json),
    Transport(String),
}

/// Durable execution's memory, in memory: an impure call looks itself up here first and
/// performs the real call only when nothing is recorded.
///
/// One per invocation, which is why `deliver` takes it rather than the interpreter
/// holding one. A host that wants a replay to survive a restart implements [`Calls`]
/// over its own store instead.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Journal {
    entries: BTreeMap<(String, u32), Recorded>,
}

impl Journal {
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn calls(&self) -> impl Iterator<Item = (&str, &Recorded)> {
        self.entries
            .iter()
            .map(|((call, _), recorded)| (call.as_str(), recorded))
    }
}

impl Calls for Journal {
    fn recorded(&self, call: &str, ordinal: u32) -> Result<Option<Recorded>, Error> {
        Ok(self.entries.get(&(call.to_string(), ordinal)).cloned())
    }

    fn record(&mut self, call: &str, ordinal: u32, recorded: Recorded) -> Result<(), Error> {
        self.entries.insert((call.to_string(), ordinal), recorded);
        Ok(())
    }
}

/// A log, a key store and a network, none of them real.
#[derive(Debug, Clone, Default)]
pub struct Harness {
    records: Vec<Record>,
    /// The key store, modelled as a lifecycle: a subject is erased or it is not. That
    /// is what rules 9 and 12 turn on. Ciphertext is not modelled; see `docs/effects.md`.
    keys: BTreeSet<(Ident, String)>,
    scripted: BTreeMap<String, VecDeque<Reply>>,
    /// Deployment credentials a test supplied. Empty is the common case: a name nothing
    /// mentions answers `secret:NAME` rather than nothing, so no test needs setup to run
    /// an effect that reads one.
    ///
    /// The value is itself an `Option` so that "a test gave this one a value" and "a
    /// test said this deployment does not set it" are one entry rather than two
    /// collections with an invariant between them. See `docs/testing.md`.
    secrets: BTreeMap<String, Option<Arc<str>>>,
}

impl Harness {
    /// A log of events, each stamped as an append would stamp it.
    pub fn with_log(log: impl IntoIterator<Item = Event>) -> Self {
        let mut harness = Self::default();
        for event in log {
            harness.push(event);
        }
        harness
    }

    /// A log of records that already carry their envelopes, which is how a host hands
    /// over one it did not synthesise.
    pub fn with_records(records: impl IntoIterator<Item = Record>) -> Self {
        Self {
            records: records.into_iter().collect(),
            ..Self::default()
        }
    }

    pub fn records(&self) -> &[Record] {
        &self.records
    }

    /// Appends with a synthesised envelope, derived from the position so a run is
    /// reproducible. A real host stamps its own.
    pub fn push(&mut self, event: Event) {
        let position = self.records.len() as u64;
        self.records.push(Record::new(
            format!("0190d1a1-0000-7000-9000-{position:012}"),
            position,
            EPOCH_MICROS + position as i64 * MINUTE_MICROS,
            event,
        ));
    }

    /// Marks a subject erased without an effect having done it, which is the case rule
    /// 12's message is about: the erase is usually not local.
    pub fn erase_subject(&mut self, subject: &str, id: &str) {
        self.keys.insert((subject.to_string(), id.to_string()));
    }

    /// Gives one deployment credential the value a test wrote, overriding the
    /// `secret:NAME` stand-in. For when the shape matters: a value that has to parse as
    /// a URL, or carry a prefix an arm branches on.
    pub fn set_secret(&mut self, name: &str, value: impl Into<Arc<str>>) {
        self.secrets.insert(name.to_string(), Some(value.into()));
    }

    /// Declares that this deployment does not set one, which is how a test reaches the
    /// absent branch of a `secret NAME?` that the stand-in would otherwise always
    /// answer, and the missing-credential backstop for a required one.
    pub fn unset_secret(&mut self, name: &str) {
        self.secrets.insert(name.to_string(), None);
    }

    /// Queues the replies one URL will answer with.
    pub fn script(&mut self, url: &str, replies: impl IntoIterator<Item = Reply>) {
        self.scripted
            .entry(url.to_string())
            .or_default()
            .extend(replies);
    }
}

impl Log for Harness {
    fn head(&self) -> Result<u64, Error> {
        Ok(self.records.len() as u64)
    }

    fn record(&self, position: u64) -> Result<Option<Record>, Error> {
        Ok(self.records.get(position as usize).cloned())
    }

    /// A scan, which is what an in-memory log has. The predicate is still applied here
    /// rather than left to the caller, so the harness answers the same question a store
    /// answers from an index.
    fn read(
        &self,
        query: &Query,
        visit: &mut dyn FnMut(&Record) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let last = query.upto.unwrap_or(u64::MAX);
        for record in &self.records {
            if record.position > last {
                break;
            }
            // A real store seeks here. Scanning to the cursor is what makes this the
            // stand-in it is, and it is still the same set of records.
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

    /// Nothing single-threaded can trip the condition from inside a run, and it is
    /// checked anyway: there is one definition of what the condition means, and a host
    /// that has to implement it deserves somewhere to read it.
    fn append(&mut self, events: &[Event], condition: &AppendCondition) -> Result<(), Error> {
        if condition.conflicts(&self.records) {
            return Err(ErrorKind::Conflict {
                after: condition.after,
            }
            .into());
        }
        for event in events {
            self.push(event.clone());
        }
        Ok(())
    }
}

impl Clock for Harness {
    /// Derived from the log's length, so two runs of one program agree. Rule 11 asks
    /// only that it be pinned, not that it be a wall clock.
    fn now(&self) -> i64 {
        EPOCH_MICROS + self.records.len() as i64 * MINUTE_MICROS
    }
}

/// The stand-in a declared credential resolves to when nothing supplied one, so no test
/// needs setup to run an effect that reads a secret. Deterministic and obviously not a
/// real value, and it is what a `respond` and an `expect http.*` in such a test name.
impl Secrets for Harness {
    fn secret(&self, name: &str) -> Option<Arc<str>> {
        match self.secrets.get(name) {
            Some(supplied) => supplied.clone(),
            None => Some(Arc::from(format!("secret:{name}"))),
        }
    }
}

impl Keys for Harness {
    /// The harness's ciphertext is its plaintext. It models the key lifecycle and not
    /// crypto, so "decrypt" is the lifecycle question and nothing else: content behind
    /// a live key reads back as it was stored, and content behind a destroyed one does
    /// not read back at all.
    fn decrypt(
        &self,
        subject: &str,
        id: &str,
        _field: &str,
        content: &str,
    ) -> Result<Option<String>, Error> {
        if self.keys.contains(&(subject.to_string(), id.to_string())) {
            return Ok(None);
        }
        Ok(Some(content.to_string()))
    }

    fn erase(&mut self, subject: &str, id: &str) -> Result<(), Error> {
        self.erase_subject(subject, id);
        Ok(())
    }
}

impl Http for Harness {
    /// The next scripted reply for that URL, taken in order. An unscripted URL answers
    /// 404, which is what a test that forgot to declare a response should see.
    fn send(&mut self, request: &Request) -> Attempt {
        let reply = self
            .scripted
            .get_mut(&request.wire.url)
            .and_then(VecDeque::pop_front)
            .unwrap_or(Reply::Status(404));
        match reply {
            Reply::Status(status) => Attempt::Response {
                status,
                body: Json::Null,
            },
            Reply::Body(status, body) => Attempt::Response { status, body },
            Reply::Transport(why) => Attempt::Transport(why),
        }
    }
}

/// The harness as a world a test runs in: this log, these read models, these scripted
/// replies. The in-memory answer to `docs/testing.md` section 3, and the one
/// `run_tests` uses when an embedder does not bring its own.
#[derive(Debug, Clone, Default)]
pub struct Sandbox {
    harness: Harness,
    store: Store,
}

impl World for Sandbox {
    type Host = Harness;
    type Rows = Store;

    fn given(&mut self, event: Event) -> Result<(), Error> {
        self.harness.push(event);
        Ok(())
    }

    fn secret(&mut self, name: &str, value: Option<&str>) -> Result<(), Error> {
        match value {
            Some(value) => self.harness.set_secret(name, value),
            None => self.harness.unset_secret(name),
        }
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

    fn open(self) -> Result<(Harness, Store), Error> {
        Ok((self.harness, self.store))
    }
}
