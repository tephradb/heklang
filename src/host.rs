//! The seam: what an interpreter needs from the world outside it.
//!
//! Nothing here holds interpreter state, so a host implementor reads one file. The cut
//! between [`Log`] and the other three is the one `docs/effects.md` rule 11 already
//! makes: reading the log is redone on every attempt and must be, while a side effect
//! or an unrepeatable observation is done once and remembered.
//!
//! [`Calls`] and [`Rows`] sit beside the bundle rather than inside it, because a journal
//! is per invocation and a read model is per projector, while a [`Host`] is per world.

use std::sync::Arc;

use crate::interp::{Error, Row};
use crate::ir::{EventPath, Ident};
use crate::value::{Event, Invoked, Json, Key, Record, Value};

/// One resolved read: an event path and the values its filters narrowed it to.
///
/// A `fold` declares a slice, and what a command folded is what it conflicts on, so
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

/// What one fold reads: the union of its slices, over a range of positions.
///
/// Three obligations, each of them load-bearing. Every record in range matching any
/// predicate is visited, or a fold silently loses events. Each is visited **once**, or
/// `open + 1` counts one event twice. In ascending position order, because a fold is an
/// order-dependent expression.
///
/// Visiting a record that matches nothing is harmless, because the fold re-checks each
/// slice itself. A store that can only narrow approximately is still correct, only
/// slower: over-delivering is a cost and under-delivering is a bug. **The range is not
/// part of that latitude.** A slice is re-checked and a position is not, so a record
/// handed to a resumed fold that already folded it is counted twice.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Query {
    pub slices: Vec<Predicate>,
    /// The first position to visit, inclusive. `0` reads from the start of the log.
    ///
    /// A retry sets it to where its last attempt stopped, so a conflict costs the events
    /// that beat it rather than the whole boundary again. A store whose reads take a
    /// cursor answers this by seeking, which is the entire point of the field: a host
    /// that filtered instead would still read every event it then threw away.
    pub from: u64,
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
    /// The plaintext behind a seal, or `None` when it cannot be read because the
    /// subject's key is gone.
    ///
    /// `None` rather than `Err`: an erased subject is an outcome `docs/effects.md`
    /// rule 12 names and a program meets, not a host that failed. `field` is the name
    /// the content was sealed under rather than where it now sits, because a host
    /// binds its ciphertext to that name and sealed content may be moved.
    ///
    /// Called once per `reveal`, so a fold pays for the content it reads rather than
    /// for every record it walks. The other question this seam is asked is
    /// [`Keys::is_live`], which wants no content and is answered without this.
    fn decrypt(
        &self,
        subject: &str,
        id: &str,
        field: &str,
        content: &str,
    ) -> Result<Option<String>, Error>;

    /// Whether this subject's key is still there: the lifecycle question with no
    /// content attached.
    ///
    /// Asked where a sealed column is written into read models heklang owns
    /// (`docs/projectors.md` rule 9: an erase has to empty the column, and only the key
    /// store knows whether it should). A projection moves sealed content without ever
    /// revealing it, so producing the plaintext to answer a boolean would put personal
    /// data on a path built not to hold any, and would pay a key use per column per
    /// row.
    ///
    /// Defaulted to `decrypt` so that no host is broken by this arriving, and so that a
    /// host with nothing cheaper is still correct. Override it wherever the key store
    /// can answer without unwrapping the content.
    fn is_live(&self, subject: &str, id: &str, field: &str, content: &str) -> Result<bool, Error> {
        Ok(self.decrypt(subject, id, field, content)?.is_some())
    }

    fn erase(&mut self, subject: &str, id: &str) -> Result<(), Error>;
}

/// Deployment credentials, as values rather than as a source.
///
/// Where they come from is a host's business, the same way a key store is:
/// `docs/effects.md` rule 16 says which declarations may read one and says nothing
/// about environments, files or vaults. Asked lazily rather than resolved when an
/// interpreter is built, so a remote provider is a host change and not a language one;
/// a host reading an environment caches trivially.
pub trait Secrets {
    /// The value behind a declared name, or `None` when this deployment has not set it.
    ///
    /// `None` is only meant to be reachable for a `secret NAME?`. A host is expected to
    /// refuse to start when a required one is unset, so a program that reads one is
    /// answering a question the deployment already settled; heklang's backstop for a
    /// host that fails at that is `ErrorKind::MissingSecret`, which wedges the
    /// invocation and names the declaration rather than panicking.
    ///
    /// `&self` for the reason `Keys::decrypt` is: reading a credential does not spend
    /// it.
    fn secret(&self, name: &str) -> Option<Arc<str>>;
}

/// The three parts of a request that can carry a deployment credential, in one of its
/// two renderings. See [`Request`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parts {
    pub url: String,
    pub body: Option<Json>,
    pub headers: Json,
}

/// One request as it left, so a test can assert what was sent rather than only what
/// came back. The `Idempotency-Key` case is why headers are worth seeing.
///
/// **Two renderings, and reading the wrong one is a compile error rather than a leak.**
/// A `secret` (`docs/effects.md` rule 16) may sit in the url, in a header value or in
/// the body, and those three cross here. `wire` is what the credential actually is and
/// `shown` names it instead, so a host sends one and prints the other. They are separate
/// fields rather than one field plus a convention because a program holding no secret
/// makes them identical: a host that read the wrong one would be wrong only for the
/// programs that use the feature, and would never find out. The flat `url`, `body` and
/// `headers` fields are gone for exactly that reason, so the choice is made once, at
/// the compiler's insistence. `docs/host.md` has the rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub verb: &'static str,
    /// What goes on the wire. Read this to send.
    pub wire: Parts,
    /// The same request with every credential named rather than spelled. Read this to
    /// print, trace, log, or key a journal.
    pub shown: Parts,
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

/// One projector's read models, as the host stores them.
///
/// Separate from [`Host`] because it is per projector rather than per world, the way
/// [`Calls`] is per invocation. `docs/projectors.md` rule 3 is why it reads as well as
/// writes: a `patch` fills its stored loads from the row before it evaluates any of the
/// write's value expressions, so a write-only stream could not carry one.
///
/// A `patch` arrives here as a whole row rather than as the fields it named. The delta
/// is not lost, since `Stmt::Patch` still holds it, but it is deliberately not what
/// crosses: a host taking a delta while [`Store`](crate::Store) took a whole row would
/// be two write paths with nothing comparing them against each other.
pub trait Rows {
    /// The current row, including writes this projector has already made.
    fn row(&self, entity: &str, key: &Key) -> Result<Option<Row>, Error>;
    fn put(&mut self, entity: &Ident, key: Key, row: Row) -> Result<(), Error>;
    fn delete(&mut self, entity: &Ident, key: &Key) -> Result<(), Error>;
}

/// Everything the interpreter needs from the world outside it, apart from the journal:
/// that is per invocation and arrives at `deliver`.
///
/// One bundle because `Effects` holds one trait object, and four traits because the
/// seams have genuinely different shapes: a `Clock` is three lines and a `Keys` is a
/// key management service.
pub trait Host: Log + Clock + Keys + Http + Secrets {}

impl<T: Log + Clock + Keys + Http + Secrets> Host for T {}
