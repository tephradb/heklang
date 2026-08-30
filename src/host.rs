//! The seam: what an interpreter needs from the world outside it.
//!
//! Nothing here holds interpreter state, so a host implementor reads one file. The cut
//! between [`Log`] and the other three is the one `docs/effects.md` rule 11 already
//! makes: reading the log is redone on every attempt and must be, while a side effect
//! or an unrepeatable observation is done once and remembered.

use crate::interp::Error;
use crate::ir::{EventPath, Ident};
use crate::value::{Event, Invoked, Json, Record, Value};

/// One resolved read: an event path and the values its filters narrowed it to.
///
/// A `state` declares a slice, and what a command folded is what it conflicts on, so
/// the query a host reads with and the condition it appends against are one shape.
/// `docs/commands.md` has the argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Predicate {
    pub event: EventPath,
    /// Each field and the value it must equal. Empty is the whole event type.
    ///
    /// Sorted by field name, so one slice is one predicate however it was written:
    /// `(shop_id, topic)` and `(topic, shop_id)` narrow the same set of events and
    /// have no business comparing unequal.
    pub filters: Vec<(Ident, Value)>,
}

impl Predicate {
    pub fn new(event: EventPath, mut filters: Vec<(Ident, Value)>) -> Self {
        filters.sort_by(|(one, _), (other, _)| one.cmp(other));
        Self { event, filters }
    }

    /// Whether this event is in the slice.
    ///
    /// A filter naming a field the event does not carry answers `false`: the event is
    /// outside the slice, which is a narrower answer than no answer at all. The fold's
    /// own check is stricter and raises instead, because a log missing a declared field
    /// is a broken host rather than a narrower read.
    pub fn holds(&self, event: &Event) -> bool {
        self.event == event.path
            && self
                .filters
                .iter()
                .all(|(name, want)| event.field(name) == Some(want))
    }
}

/// What one fold reads: the union of its slices, bounded above.
///
/// Three obligations, each of them load-bearing. Every record matching any predicate is
/// visited, or a fold silently loses events. Each is visited **once**, or `open + 1`
/// counts one event twice. In ascending position order, because a fold is an
/// order-dependent expression.
///
/// Visiting a record that matches nothing is harmless, because the fold re-checks each
/// slice itself. A store that can only narrow approximately is still correct, only
/// slower: over-delivering is a cost and under-delivering is a bug.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    pub slices: Vec<Predicate>,
    /// The last position to visit, inclusive. `None` reads to the head.
    /// `docs/effects.md` rule 3: an effect's fold stops at the trigger's own position.
    pub upto: Option<u64>,
}

/// What a run read, and therefore what it can be beaten to.
///
/// Returned with every outcome rather than only a commit: a refusal still read the log,
/// and a host that wants to cache or trace the decision needs to know what it depended
/// on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppendCondition {
    /// The head the run folded against. Another writer landing in one of `slices` at or
    /// after this position is what makes the append stale.
    pub after: u64,
    pub slices: Vec<Predicate>,
}

impl AppendCondition {
    /// Whether anything in the read set landed at or after `after`.
    ///
    /// This is the definition, written once. A host that can answer it from an index
    /// should, and this is the question it would be answering.
    pub fn conflicts(&self, records: &[Record]) -> bool {
        records.iter().any(|record| {
            record.position >= self.after
                && self.slices.iter().any(|slice| slice.holds(&record.event))
        })
    }
}

/// The event log.
pub trait Log {
    /// The position the next append lands at.
    fn head(&self) -> Result<u64, Error>;

    /// One position, or `None` when the log is not that long.
    fn record(&self, position: u64) -> Result<Option<Record>, Error>;

    /// Every record the query selects, in position order, each of them once. A host
    /// answers this from an index; the harness scans.
    fn read(
        &self,
        query: &Query,
        visit: &mut dyn FnMut(&Record) -> Result<(), Error>,
    ) -> Result<(), Error>;

    /// Appends whole, stamping each event with its own envelope. All of the events or
    /// none of them.
    fn append(&mut self, events: &[Event], condition: &AppendCondition) -> Result<(), Error>;
}

/// `docs/effects.md` rule 11. Microseconds since the Unix epoch, which is what a
/// `Timestamp` is; a host whose clock is a wall-clock string converts here.
pub trait Clock {
    fn now(&self) -> i64;
}

/// The key store, as a lifecycle rather than as ciphertext. `docs/effects.md` rule 12:
/// a subject is erased or it is not, and that is the whole of what heklang models.
pub trait Keys {
    fn erased(&self, subject: &str, id: &str) -> Result<bool, Error>;
    fn erase(&mut self, subject: &str, id: &str) -> Result<(), Error>;
}

/// One request as it left, so a test can assert what was sent rather than only what
/// came back. The `Idempotency-Key` case is why headers are worth seeing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub verb: &'static str,
    pub url: String,
    pub body: Option<Json>,
    pub headers: Json,
}

/// What one attempt came to. A transport failure is not an error: it is the retryable
/// outcome, and rule 5 absorbs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attempt {
    Response { status: u16, body: Json },
    Transport(String),
}

/// One attempt with one request.
///
/// The retry policy is heklang's, not the host's. Rule 5 says only a decidable result
/// reaches the handler, and that stops being a language rule the moment two hosts can
/// answer the same program differently.
pub trait Http {
    fn send(&mut self, request: &Request) -> Attempt;
}

/// A recorded impure call. `reveal` and `log` are absent, which is rule 10's
/// unjournaled set being a property of the type rather than a marker in the syntax.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recorded {
    Response { status: i64, body: Json },
    Invoked(Invoked),
    Now(i64),
    Erased,
}

/// Durable execution's memory for one invocation.
///
/// Separate from [`Host`] because it is per invocation rather than per world: `deliver`
/// takes one, and nothing carries between positions. The key describes the call and
/// prints, plus an ordinal for repeated identical calls; a host that would rather store
/// a hash hashes exactly that string, which is what keeps the hash a host's business
/// and the key the language's.
pub trait Calls {
    fn recorded(&self, call: &str, ordinal: u32) -> Result<Option<Recorded>, Error>;
    fn record(&mut self, call: &str, ordinal: u32, recorded: Recorded) -> Result<(), Error>;
}

/// Everything the interpreter needs from the world outside it, apart from the journal:
/// that is per invocation and arrives at `deliver`.
///
/// One bundle because `Effects` holds one trait object, and four traits because the
/// seams have genuinely different shapes: a `Clock` is three lines and a `Keys` is a
/// key management service.
pub trait Host: Log + Clock + Keys + Http {}

impl<T: Log + Clock + Keys + Http> Host for T {}
