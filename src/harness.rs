//! The in-memory world, and the only one until a host brings its own.
//!
//! Everything here is a stand-in for something a runtime owns: a log, a key store and
//! a network. Keeping it in its own module is what stops "what the harness does" from
//! reading as "what the language does", which is a distinction `docs/testing.md` rule 8
//! already depends on.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use crate::ir::{Builtin, Ident};
use crate::value::{Event, Json, Record};

/// 2020-01-01T00:00:00Z, so a synthesised envelope timestamp reads as a plausible
/// instant rather than the epoch.
const EPOCH_MICROS: i64 = 1_577_836_800_000_000;
const MINUTE_MICROS: i64 = 60_000_000;

/// How many attempts the runtime makes before a call wedges. Retryable statuses and
/// transport errors are absorbed here, so the handler never sees one (rule 5).
const ATTEMPTS: usize = 4;

/// One request as it left, so a test can assert what was sent rather than only what
/// came back. The `Idempotency-Key` case is why headers are worth seeing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub verb: &'static str,
    pub url: String,
    pub body: Option<Json>,
    pub headers: Json,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Status(u16),
    Body(u16, Json),
    Transport(String),
}

/// A log, a key store and a network, none of them real.
#[derive(Debug, Clone, Default)]
pub struct Harness {
    records: Vec<Record>,
    /// The key store, modelled as a lifecycle: a subject is erased or it is not. That
    /// is what rules 9 and 12 turn on. Ciphertext is not modelled; see `docs/effects.md`.
    keys: BTreeSet<(Ident, String)>,
    scripted: BTreeMap<String, VecDeque<Reply>>,
    performed: usize,
    absorbed: usize,
    sent: Vec<Request>,
}

impl Harness {
    pub fn records(&self) -> &[Record] {
        &self.records
    }

    pub fn record(&self, position: u64) -> Option<Record> {
        self.records.get(position as usize).cloned()
    }

    pub fn head(&self) -> u64 {
        self.records.len() as u64
    }

    /// Appends with a synthesised envelope, derived from the position so a run is
    /// reproducible. A real host stamps its own.
    pub fn push(&mut self, event: Event) {
        let position = self.head();
        self.records.push(Record::new(
            format!("0190d1a1-0000-7000-9000-{position:012}"),
            position,
            EPOCH_MICROS + position as i64 * MINUTE_MICROS,
            event,
        ));
    }

    /// Derived from the log's length, so two runs of one program agree. Rule 11 asks
    /// only that it be pinned, not that it be a wall clock.
    pub fn now(&self) -> i64 {
        EPOCH_MICROS + self.head() as i64 * MINUTE_MICROS
    }

    pub fn erased(&self, subject: &str, id: &str) -> bool {
        self.keys.contains(&(subject.to_string(), id.to_string()))
    }

    pub fn erase(&mut self, subject: &str, id: &str) {
        self.keys.insert((subject.to_string(), id.to_string()));
    }

    pub fn script(&mut self, url: &str, replies: impl IntoIterator<Item = Reply>) {
        self.scripted
            .entry(url.to_string())
            .or_default()
            .extend(replies);
    }

    pub fn requests(&self) -> &[Request] {
        &self.sent
    }

    pub fn performed(&self) -> usize {
        self.performed
    }

    pub fn absorbed(&self) -> usize {
        self.absorbed
    }

    /// The terminal response, or `None` when every attempt was retryable, which wedges.
    /// Rule 5 lives here: a retryable status or a transport error is absorbed and
    /// retried with the same request, so only a decidable result reaches the handler.
    pub fn call(
        &mut self,
        builtin: Builtin,
        url: &str,
        body: Option<Json>,
        headers: Json,
    ) -> Option<(i64, Json)> {
        for _ in 0..ATTEMPTS {
            self.performed += 1;
            self.sent.push(Request {
                verb: builtin.name(),
                url: url.to_string(),
                body: body.clone(),
                headers: headers.clone(),
            });
            let reply = self
                .scripted
                .get_mut(url)
                .and_then(VecDeque::pop_front)
                .unwrap_or(Reply::Status(404));
            let (status, body) = match reply {
                Reply::Status(status) => (status, Json::Null),
                Reply::Body(status, body) => (status, body),
                Reply::Transport(_) => {
                    self.absorbed += 1;
                    continue;
                }
            };
            if is_retryable(status) {
                self.absorbed += 1;
                continue;
            }
            return Some((i64::from(status), body));
        }
        None
    }
}

/// 408, 425, 429 and any 5xx each name a condition that clears on its own, with the
/// same request.
fn is_retryable(status: u16) -> bool {
    matches!(status, 408 | 425 | 429) || status >= 500
}
