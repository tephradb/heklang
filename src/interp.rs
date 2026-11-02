use std::cmp;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::error;
use std::fmt;
use std::sync::Arc;

use uuid::Uuid;

use crate::harness::{Harness, Journal, Reply};
use crate::host::{
    AppendCondition, Attempt, Calls, Columns, Host, Keys, Log, Parts, Predicate, Query, Recorded,
    Request, Rows,
};
use crate::ir::{
    Absent, Arm, BinOp, Builtin, Command, Delivery, Effect, EntityDef, EnvBind, EnvField,
    EventPath, Expr, ExprId, Exprs, Filter, Fold, Function, Ident, Iter, Param, Program, Projector,
    Return, Slice, Slot, Span, Stmt, Type, UnOp,
};
use crate::scaled::{self, Rounding};
use crate::tables::{self, ArmMatch, Column, Layout, Table};
use crate::value::{self, Event, Invoked, Json, Key, Record, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Execution {
    pub outcome: Outcome,
    pub condition: AppendCondition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Ok(Vec<Event>),
    Invalid(String),
    Reject { code: String, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorKind {
    UnknownCommand(String),
    UnknownProjector(String),
    UnknownEffect(String),
    UnknownFold(String),
    /// [`Interpreter::fold_steps`] on a column whose closure holds a range update: itself,
    /// or a fold it keys or reads through. A by-key read of one does not agree with the
    /// stored value (`docs/folds.md` section 11), so there is no history to report rather
    /// than a slow one. `ranged` is the column that ranges and `open` its unpinned key
    /// parts, because the fold asked for is often not the one that has to change.
    NoHistory {
        fold: String,
        ranged: String,
        open: String,
    },
    NoSuchPosition(u64),
    /// Rule 12: terminal rather than a wedge, because no retry recovers erased data.
    /// The message names the erase as possibly non-local, which it usually is: rule 9
    /// makes a local erase-then-reveal a compile error.
    Erased {
        field: Ident,
        subject: Ident,
        id: String,
    },
    /// Rule 12 in a read model: a sealed column whose key is gone, in a column that
    /// cannot say so. An erased optional reads back absent; a required column has no
    /// such value, so the row cannot be written at all.
    ///
    /// `column` is `Entity.field`, built the way a bound's path is: three fields rather
    /// than four keeps `Error` inside the size every other seam here returns it at.
    ShreddedColumn {
        column: Ident,
        subject: Ident,
        id: String,
    },
    /// A required `secret` this deployment did not set (`docs/effects.md` rule 16).
    ///
    /// A **wedge**, not a skip: unlike an erased subject, this is recoverable, and it is
    /// recovered by an operator setting the credential and the invocation being retried.
    /// It is a backstop rather than a path a running deployment should reach, because a
    /// host is expected to refuse to start with one unresolved; it names the declaration
    /// so an operator can act on it either way.
    MissingSecret(Ident),
    /// Rule 4's terminal outcome, raised inside an effect-local `fn`. A call is an
    /// expression, so a `Flow` cannot carry it out; `run_arm` catches this exactly
    /// where it catches the direct `fail`, and reports the same thing.
    Failed(String),
    Unreachable(String),
    /// The host could not do what was asked. Rendered as the host wrote it: heklang has
    /// no vocabulary for a store's failures and should not invent one.
    Host(String),
    /// A stored value is not what its declaration says. The one failure on this seam
    /// that is about data rather than about a broken host, which is why it is not a
    /// `Host`: a record written before a field changed type reads like this, and an
    /// operator quarantines the reader rather than mistrusting the store.
    Mismatch(value::Mismatch),
    /// The host refused the append: something in the read set landed at or after
    /// `after`. Not an `Outcome`, because the three outcomes are the command's own
    /// answer and a conflict is the runtime's. `docs/host.md` has what an adapter
    /// turns it into.
    Conflict {
        after: u64,
    },
    BadSubject(Type),
    /// Rule 15: a `@key` field arrived holding something that cannot name a lane. The
    /// checker rejects a type that could do this, so reaching here means a host handed
    /// over a value its own event declaration disagrees with.
    BadLane {
        field: Ident,
        ty: Type,
    },
    BadUuid(String),
    NoSuchField {
        ty: Type,
        field: Ident,
    },
    Cascade {
        effect: String,
        events: Vec<String>,
    },
    UnknownEvent(EventPath),
    UnknownField {
        event: EventPath,
        field: Ident,
    },
    UnknownMethod {
        ty: Type,
        method: String,
    },
    MissingArgument(Ident),
    UnexpectedArgument(Ident),
    MissingField {
        event: EventPath,
        field: Ident,
    },
    UnsetSlot(Slot),
    MalformedIr,
    TypeMismatch {
        expected: Type,
        found: Type,
    },
    BadOperands {
        op: BinOp,
        lhs: Type,
        rhs: Type,
    },
    BadUnaryOperand {
        op: UnOp,
        ty: Type,
    },
    BadArity {
        method: String,
        expected: usize,
        found: usize,
    },
    BadArgument {
        method: String,
        expected: &'static str,
        found: Type,
    },
    InexactMoney {
        op: BinOp,
        hint: &'static str,
    },
    TooLong {
        field: Ident,
        len: usize,
        max: usize,
    },
    UnknownEntity(Ident),
    UnknownEntityField {
        entity: Ident,
        field: Ident,
    },
    MissingEntityField {
        entity: Ident,
        field: Ident,
    },
    BadKey(Type),
    NotIterable(Type),
    UnknownFunction(Ident),
    DivisionByZero,
    Overflow,
    Inexact,
    /// `Int.pad(width)` asked for a rendering wider than 4096 characters, which is the
    /// bound `docs/stdlib.md` sets on it.
    ///
    /// The number rather than a link to `MAX_PAD`: the constant is private, and this
    /// variant is not, so a link from here is one rustdoc refuses to resolve. The bound
    /// is a language contract rather than an implementation detail, so it is written
    /// where a reader of this variant is.
    ///
    /// Its own variant rather than `Overflow`, because nothing overflowed: the width is
    /// representable and the string it asks for is the problem. A handler must not be
    /// able to take the process down with an allocation, and a width that reaches a
    /// program from an event field or a request parameter otherwise could.
    PadWidth(i64),
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorKind::UnknownCommand(name) => write!(f, "unknown command `{name}`"),
            ErrorKind::UnknownProjector(name) => write!(f, "unknown projector `{name}`"),
            ErrorKind::UnknownEffect(name) => write!(f, "unknown effect `{name}`"),
            ErrorKind::UnknownFold(name) => write!(f, "unknown fold `{name}`"),
            ErrorKind::NoHistory { fold, ranged, open } if fold == ranged => write!(
                f,
                "`{fold}` has an arm that leaves `{open}` open, so it has no history: a range covers the keys the table held when each record arrived, and reading it by key would report steps the stored column never took"
            ),
            ErrorKind::NoHistory { fold, ranged, open } => write!(
                f,
                "`{fold}` reads `{ranged}`, which has an arm that leaves `{open}` open, so it has no history: a lookup is answered by refolding, and a refold of a ranged column applies every record to every key, so this column's key is one the store never wrote a row at"
            ),
            ErrorKind::NoSuchPosition(position) => write!(f, "no event at position {position}"),
            ErrorKind::Erased { field, subject, id } => write!(
                f,
                "reveal cannot decrypt `{field}`: subject `{subject}` = `{id}` has been erased. \
                 The erase need not be in this effect; another effect or a concurrent invocation \
                 can erase a subject between the original run and a replay, and nothing static \
                 catches that"
            ),
            ErrorKind::ShreddedColumn {
                column,
                subject,
                id,
            } => write!(
                f,
                "`{column}` holds content sealed under `{subject}` = `{id}`, whose key \
                 has been erased, and the column is not optional so it cannot read back absent. \
                 Declare the column optional: the erased case for sealed content is an optional, \
                 which is what `@absent` is told where it is written"
            ),
            ErrorKind::MissingSecret(name) => write!(
                f,
                "this deployment has not set `{name}`. A required secret is the \
                 deployment's to settle before the process starts, so nothing in the \
                 program can proceed without it; set it and retry, or declare it \
                 `secret {name}?` if it is genuinely optional"
            ),
            ErrorKind::Failed(message) => write!(f, "{message}"),
            ErrorKind::Unreachable(url) => {
                write!(f, "{url} did not answer; every attempt was retryable")
            }
            ErrorKind::Host(why) => write!(f, "{why}"),
            ErrorKind::Mismatch(why) => write!(f, "{why}"),
            ErrorKind::Conflict { after } => write!(
                f,
                "the log moved under this run: something it read landed at or after position {after}"
            ),
            ErrorKind::BadSubject(ty) => {
                write!(f, "{} cannot identify a subject", crate::types::a(ty))
            }
            ErrorKind::BadLane { field, ty } => {
                write!(f, "`{field}` holds {ty}, which cannot name a lane")
            }
            ErrorKind::BadUuid(value) => write!(f, "`{value}` is not a uuid"),
            ErrorKind::NoSuchField { ty, field } => write!(f, "no field `{field}` on {ty}"),
            // The tail rather than the whole slice: a walk deep enough to trip this has
            // usually appended far more events than are worth printing, and the ones that
            // say what the cycle is are the last few.
            ErrorKind::Cascade { effect, events } => {
                let tail = events.len().saturating_sub(8);
                let ending = if tail > 0 { "ending " } else { "" };
                write!(
                    f,
                    "effect `{effect}` kept producing events without settling after {} of them, \
                     {ending}{}; the self-trigger check should have rejected this",
                    events.len(),
                    events[tail..].join(" -> ")
                )
            }
            ErrorKind::UnknownEvent(path) => write!(f, "undeclared event {path}"),
            ErrorKind::UnknownField { event, field } => {
                write!(f, "event {event} has no field `{field}`")
            }
            ErrorKind::UnknownMethod { ty, method } => write!(f, "no method `{method}` on {ty}"),
            ErrorKind::MissingArgument(name) => write!(f, "missing argument `{name}`"),
            ErrorKind::UnexpectedArgument(name) => write!(f, "unexpected argument `{name}`"),
            ErrorKind::MissingField { event, field } => {
                write!(f, "event {event} is missing field `{field}`")
            }
            ErrorKind::UnsetSlot(slot) => write!(f, "slot {} read before it was set", slot.0),
            ErrorKind::MalformedIr => f.write_str("malformed ir"),
            ErrorKind::TypeMismatch { expected, found } => {
                write!(f, "expected {expected}, found {found}")
            }
            ErrorKind::BadOperands { op, lhs, rhs } => {
                write!(f, "cannot apply `{op}` to {lhs} and {rhs}")
            }
            ErrorKind::BadUnaryOperand { op, ty } => write!(f, "cannot apply `{op}` to {ty}"),
            ErrorKind::BadArity {
                method,
                expected,
                found,
            } => write!(f, "`{method}` takes {expected} arguments, got {found}"),
            ErrorKind::BadArgument {
                method,
                expected,
                found,
            } => write!(f, "`{method}` expects {expected}, got {found}"),
            ErrorKind::InexactMoney { op, hint } => write!(
                f,
                "`{op}` on Money is not exact here, use `{hint}` with an explicit rounding mode"
            ),
            ErrorKind::TooLong { field, len, max } => {
                write!(f, "{field} is {len} characters, the most allowed is {max}")
            }
            ErrorKind::UnknownEntity(name) => write!(f, "undeclared entity `{name}`"),
            ErrorKind::UnknownEntityField { entity, field } => {
                write!(f, "entity `{entity}` has no field `{field}`")
            }
            ErrorKind::MissingEntityField { entity, field } => {
                write!(f, "entity `{entity}` is missing field `{field}`")
            }
            ErrorKind::BadKey(ty) => write!(f, "{ty} cannot be an entity key"),
            ErrorKind::NotIterable(ty) => write!(f, "{ty} is not a list or a map"),
            ErrorKind::UnknownFunction(name) => write!(f, "unknown fn `{name}`"),
            ErrorKind::DivisionByZero => f.write_str("division by zero"),
            ErrorKind::Overflow => f.write_str("arithmetic overflow"),
            ErrorKind::Inexact => f.write_str("result is not exact"),
            ErrorKind::PadWidth(width) => write!(
                f,
                "`pad` was asked for a width of {width}, and {MAX_PAD} is the widest it \
                 will build. A width is a field's shape and is written as a small number; \
                 one this large has come from data"
            ),
        }
    }
}

impl error::Error for ErrorKind {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    /// Where it happened. `Span::default()` is the one that means nowhere: an error
    /// raised outside any expression, which renders without a position at all.
    pub span: Span,
    /// Stamped at the `run` / `project` boundary, which is the innermost place that
    /// knows which module the running declaration came from.
    pub module: Option<String>,
}

impl Error {
    pub fn new(kind: ErrorKind) -> Self {
        Self {
            kind,
            span: Span::default(),
            module: None,
        }
    }

    pub fn at(kind: ErrorKind, span: Span) -> Self {
        Self {
            kind,
            span,
            module: None,
        }
    }

    /// Fills the span in when the error has none, which is the case for anything a
    /// host raised: it knows what went wrong and not where it was asked from.
    fn located(mut self, span: Span) -> Self {
        if self.span == Span::default() {
            self.span = span;
        }
        self
    }

    fn in_module(mut self, module: Option<&str>) -> Self {
        if self.module.is_none() {
            self.module = module.map(str::to_string);
        }
        self
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let nowhere = self.span == Span::default();
        match (&self.module, nowhere) {
            (Some(module), false) => write!(f, "{module}:{}: {}", self.span, self.kind),
            (None, false) => write!(f, "{}: {}", self.span, self.kind),
            _ => write!(f, "{}", self.kind),
        }
    }
}

impl error::Error for Error {}

impl From<value::Mismatch> for ErrorKind {
    fn from(why: value::Mismatch) -> Self {
        ErrorKind::Mismatch(why)
    }
}

impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Self {
        Error::new(kind)
    }
}

impl From<scaled::Error> for ErrorKind {
    fn from(err: scaled::Error) -> Self {
        match err {
            scaled::Error::Overflow => ErrorKind::Overflow,
            scaled::Error::DivisionByZero => ErrorKind::DivisionByZero,
            scaled::Error::Inexact => ErrorKind::Inexact,
        }
    }
}

/// Cloneable so a command's attempt loop can keep the part that does not vary between
/// attempts (the pinned clock and the arguments) and re-run only what
/// does. A slot holds a `Value`, which is itself cheap to clone.
#[derive(Debug, Clone)]
struct Frame {
    slots: Vec<Option<Value>>,
}

impl Frame {
    fn new(size: usize) -> Self {
        Self {
            slots: vec![None; size],
        }
    }

    fn set(&mut self, slot: Slot, value: Value) -> Result<(), ErrorKind> {
        let cell = self
            .slots
            .get_mut(slot.0 as usize)
            .ok_or(ErrorKind::MalformedIr)?;
        *cell = Some(value);
        Ok(())
    }

    fn get(&self, slot: Slot) -> Result<&Value, ErrorKind> {
        self.slots
            .get(slot.0 as usize)
            .ok_or(ErrorKind::MalformedIr)?
            .as_ref()
            .ok_or(ErrorKind::UnsetSlot(slot))
    }
}

enum Flow {
    Next,
    Return(Ret),
}

enum Ret {
    Ok,
    /// A `fn`'s result. It never escapes `call_function`, which is the only caller
    /// that can produce one.
    Value(Value),
    Invalid(String),
    Reject {
        code: String,
        message: String,
    },
    /// Rule 4: the author's terminal outcome, which only an effect can reach.
    Fail(String),
}

/// In-memory read models, one map per entity. A test harness: declared indexes are
/// recorded in the IR and ignored here.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Store {
    entities: BTreeMap<Ident, BTreeMap<Vec<Key>, Row>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Row(pub BTreeMap<Ident, Value>);

impl Row {
    pub fn field(&self, name: &str) -> Option<&Value> {
        self.0.get(name)
    }
}

impl Store {
    /// One row by its whole key. A single-column key is a one-element slice:
    /// `get("Order", &[Key::Int(7)])`.
    pub fn get(&self, entity: &str, key: &[Key]) -> Option<&Row> {
        self.entities.get(entity)?.get(key)
    }

    pub fn rows(&self, entity: &str) -> impl Iterator<Item = (&[Key], &Row)> {
        self.entities
            .get(entity)
            .into_iter()
            .flatten()
            .map(|(key, row)| (key.as_slice(), row))
    }

    pub fn len(&self, entity: &str) -> usize {
        self.entities.get(entity).map_or(0, BTreeMap::len)
    }

    pub fn is_empty(&self, entity: &str) -> bool {
        self.len(entity) == 0
    }
}

/// The in-memory read models are one implementation of the seam a persistent store also
/// implements, rather than a shape a host has to mirror.
impl Rows for Store {
    fn row(&self, entity: &str, key: &[Key]) -> Result<Option<Row>, Error> {
        Ok(self.get(entity, key).cloned())
    }

    fn put(&mut self, entity: &Ident, key: Vec<Key>, row: Row) -> Result<(), Error> {
        self.entities
            .entry(entity.clone())
            .or_default()
            .insert(key, row);
        Ok(())
    }

    fn delete(&mut self, entity: &Ident, key: &[Key]) -> Result<(), Error> {
        if let Some(rows) = self.entities.get_mut(entity) {
            rows.remove(key);
        }
        Ok(())
    }
}

/// In-memory derived tables, one map per table and one value per column. The test double
/// [`Store`] is for a projector's rows, and this is the same thing for the read model a
/// program's named folds imply.
///
/// A key with no entry and a column with no value are one thing, which is [`Columns`]'
/// own rule: both mean the column is at its seed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cells {
    tables: BTreeMap<String, BTreeMap<Vec<Key>, BTreeMap<String, Value>>>,
}

impl Cells {
    pub fn get(&self, table: &str, key: &[Key], column: &str) -> Option<&Value> {
        self.tables.get(table)?.get(key)?.get(column)
    }

    /// Every key this table holds, in key order, which is the order a sequence of
    /// [`Key`] sorts in.
    pub fn rows(&self, table: &str) -> impl Iterator<Item = &[Key]> {
        self.tables
            .get(table)
            .into_iter()
            .flatten()
            .map(|(key, _)| key.as_slice())
    }

    pub fn len(&self, table: &str) -> usize {
        self.tables.get(table).map_or(0, BTreeMap::len)
    }

    pub fn is_empty(&self, table: &str) -> bool {
        self.len(table) == 0
    }
}

impl Columns for Cells {
    fn value(&self, table: &str, key: &[Key], column: &str) -> Result<Option<Value>, Error> {
        Ok(self.get(table, key, column).cloned())
    }

    fn set(&mut self, table: &str, key: Vec<Key>, column: &str, value: Value) -> Result<(), Error> {
        self.tables
            .entry(table.to_string())
            .or_default()
            .entry(key)
            .or_default()
            .insert(column.to_string(), value);
        Ok(())
    }

    fn keys(&self, table: &str, pinned: &[(usize, Key)]) -> Result<Vec<Vec<Key>>, Error> {
        Ok(self
            .rows(table)
            .filter(|key| {
                pinned
                    .iter()
                    .all(|(at, want)| key.get(*at).is_some_and(|held| held == want))
            })
            .map(<[Key]>::to_vec)
            .collect())
    }
}

/// Read models with a key store beside them: a sealed column whose key is gone is
/// emptied on the way through, and everything else is passed to `rows` untouched.
///
/// **Why a read model needs this at all.** Rule 9 of `docs/projectors.md` lets a
/// projector *move* sealed content into a column without ever revealing it, so a read
/// model holds the only copy of the personal data outside the log. An erase that
/// reached the log and not the read models would leave it there to read, which is the
/// opposite of what `erase` is for.
///
/// **Why here rather than in [`Projection`].** A projection holds no host
/// (`docs/host.md` section 7) and a `Value::Sealed` is what reaches `Rows::put`, which
/// is what lets a host store one without opening it. So this is not something the write
/// path can do on a host's behalf: whoever owns both the keys and the rows does it, and
/// a host that keeps its own read models does the same in its own `Rows`. This is the
/// one for the read models heklang keeps itself.
pub(crate) struct Shredding<'a> {
    pub keys: &'a dyn Keys,
    pub rows: &'a mut dyn Rows,
}

impl Rows for Shredding<'_> {
    fn row(&self, entity: &str, key: &[Key]) -> Result<Option<Row>, Error> {
        self.rows.row(entity, key)
    }

    fn put(&mut self, entity: &Ident, key: Vec<Key>, row: Row) -> Result<(), Error> {
        let row = shredded(self.keys, entity, row)?;
        self.rows.put(entity, key, row)
    }

    fn delete(&mut self, entity: &Ident, key: &[Key]) -> Result<(), Error> {
        self.rows.delete(entity, key)
    }
}

/// One row, with every sealed column the key store can no longer open taken out of it.
///
/// The emptied column reads back as the **absent optional**, which is the value rule 12
/// gives an erased subject and also the value a column the handler never wrote holds.
/// The two are deliberately indistinguishable: a reader that could tell them apart
/// would be reading the fact that this subject was erased off a row that no longer
/// holds anything else about it.
fn shredded(keys: &dyn Keys, entity: &Ident, mut row: Row) -> Result<Row, Error> {
    for (name, value) in row.0.iter_mut() {
        shred(keys, value, || format!("{entity}.{name}"))?;
    }
    Ok(row)
}

/// One column, emptied if it holds sealed content the key store can no longer open.
///
/// `column` is called only on the path that raises, because naming a column allocates
/// and almost no column is an erased seal.
fn shred(keys: &dyn Keys, value: &mut Value, column: impl FnOnce() -> String) -> Result<(), Error> {
    // Nothing is cloned to decide this, because most columns are not seals and a
    // `Type` clone allocates: the key store's answer is what the arms below branch
    // on, and only the erased one owns anything.
    let (subject, id) = {
        // `Opt` is outermost around a seal (`docs/effects.md` rule 12), so one peel
        // is the whole of the nesting there is.
        let sealed = match &*value {
            held @ Value::Sealed { .. } => held,
            Value::Opt {
                value: Some(held), ..
            } => &**held,
            _ => return Ok(()),
        };
        let Value::Sealed {
            field,
            subject,
            id,
            content,
        } = sealed
        else {
            return Ok(());
        };
        // The lifecycle question, not the content: a projection moves sealed
        // content and never reads it, so this asks whether the column still has
        // any rather than what it is.
        if keys.is_live(subject, id, field, content)? {
            return Ok(());
        }
        (subject.clone(), id.clone())
    };
    match value {
        Value::Opt { inner, .. } => {
            *value = Value::Opt {
                inner: inner.clone(),
                value: None,
            };
            Ok(())
        }
        // A column that is not optional has no value that says "this is gone": the
        // erased case for sealed content is an optional, which is the same thing
        // `@absent` is told where it is written. Loud here rather than a row that
        // quietly keeps the plaintext or quietly holds a zero.
        _ => Err(ErrorKind::ShreddedColumn {
            column: column(),
            subject,
            id,
        }
        .into()),
    }
}

/// One column with its sealed content **opened**, which is what a test's comparison needs
/// and nothing else does.
///
/// A stored sealed column holds the log's own ciphertext under the field, subject and id
/// it was sealed with, because that is what lets a store hold one without opening it and
/// what makes an erasure observable without refolding. A test writes the plaintext, so the
/// two meet only once this has run: it is the same `decrypt` call `reveal` makes, with the
/// same argument list, and `docs/testing.md` section 6 is the rule it serves.
///
/// **This is a comparison and not a read**, which is why it is here rather than on any
/// path a program can reach. Rule 12's boundary is about what a *program* may read and is
/// enforced at parse time; a test states an input and asks whether that is what came out.
///
/// The two outcomes for a key that is gone are [`shred`]'s, for its reasons: an optional
/// reads back absent, and a column with no absent value says so loudly rather than
/// comparing against a ciphertext nobody can check.
pub(crate) fn opened(
    keys: &dyn Keys,
    value: &mut Value,
    column: impl FnOnce() -> String,
) -> Result<(), Error> {
    // Decided under an immutable borrow and acted on after it, exactly as `shred` is, so
    // that the erased arms can rewrite the value they inspected.
    let plaintext = {
        let sealed = match &*value {
            held @ Value::Sealed { .. } => held,
            Value::Opt {
                value: Some(held), ..
            } => &**held,
            _ => return Ok(()),
        };
        let Value::Sealed {
            field,
            subject,
            id,
            content,
        } = sealed
        else {
            return Ok(());
        };
        match keys.decrypt(subject, id, field, content)? {
            Some(plaintext) => Ok(plaintext),
            None => Err((subject.clone(), id.clone())),
        }
    };
    let (subject, id) = match plaintext {
        Ok(plaintext) => {
            let held = match value {
                Value::Opt {
                    value: Some(held), ..
                } => &mut **held,
                held => held,
            };
            if let Value::Sealed { content, .. } = held {
                *content = plaintext.into();
            }
            return Ok(());
        }
        Err(gone) => gone,
    };
    match value {
        Value::Opt { inner, .. } => {
            *value = Value::Opt {
                inner: inner.clone(),
                value: None,
            };
            Ok(())
        }
        _ => Err(ErrorKind::ShreddedColumn {
            column: column(),
            subject,
            id,
        }
        .into()),
    }
}

/// One projector, ready to be applied to records a host supplies.
///
/// Holds no [`Host`]: `docs/projectors.md` rule 4 gives a projector no general read, and
/// rule 11 of `docs/effects.md` gives it no clock, so the program and the rows it writes
/// through are the whole of what it needs. That is what lets a host drive projections
/// from a thread that never touches the log reader.
pub struct Projection<'a> {
    program: &'a Program,
    projector: &'a Projector,
}

impl<'a> Projection<'a> {
    pub fn new(program: &'a Program, name: &str) -> Result<Self, Error> {
        let projector = program
            .projector(name)
            .ok_or_else(|| ErrorKind::UnknownProjector(name.to_string()))?;
        Ok(Self { program, projector })
    }

    /// The definition a host builds its schema from: entities, enums and handlers.
    pub fn projector(&self) -> &'a Projector {
        self.projector
    }

    /// What this projector reads, which is a host's subscription. One predicate per
    /// distinct handler path: two handlers may share a path, and a host looping slices
    /// outer would otherwise visit one record twice.
    pub fn query(&self) -> Query {
        let mut slices: Vec<Predicate> = Vec::new();
        for handler in &self.projector.handlers {
            let slice = Predicate::new(handler.event.clone(), Vec::new());
            if !slices.contains(&slice) {
                slices.push(slice);
            }
        }
        Query {
            slices,
            from: 0,
            upto: None,
        }
    }

    /// Applies every handler this record selects, in declaration order. Each gets a
    /// fresh frame, which is what makes "handlers do not share state" structural.
    pub fn apply(&self, record: &Record, rows: &mut dyn Rows) -> Result<(), Error> {
        for handler in &self.projector.handlers {
            if handler.event != record.event.path {
                continue;
            }

            let mut frame = Frame::new(handler.frame);
            for bind in &handler.binds {
                let value = field(&record.event, &bind.field)?.clone();
                let value = seal(self.program, &record.event, &bind.field, value)?;
                frame.set(bind.slot, value)?;
            }
            for bind in &handler.envelope {
                frame.set(bind.slot, envelope_value(record, bind.field))?;
            }

            let mut sink = Sink::Write {
                projector: self.projector,
                rows: &mut *rows,
            };
            exec_block(
                &handler.exprs,
                &handler.body,
                &mut frame,
                self.program,
                &mut sink,
            )?;
        }
        Ok(())
    }
}

/// The derived read model, ready to be applied to records a host supplies.
///
/// **This is the per-record sink, and it is what a column should cost.**
/// [`Interpreter::fold_stored`] fills one column at one key by folding the whole log,
/// which is the right read for a backfill and the wrong one for following a log: on the
/// ported application's order table the median write is two columns of 87, so a host
/// driving that per record pays two whole-log scans forever. This applies one record to
/// whatever columns its arms name, carrying the accumulator in the store instead of in a
/// frame, and is to [`Interpreter::fold_stored`] what [`Projection::apply`] is to a
/// rebuild.
///
/// Holds no [`Host`] for the reason [`Projection`] holds none: the caller owns the log
/// reader and the columns, so a host drives this from a thread that never touches the
/// log. It is also why the two halves of a fill are separable at all, since the
/// accumulator crossing the seam is what removes the need for one.
///
/// **It is more correct than refolding, not only cheaper.** A host deciding which columns
/// a record could have changed has only [`ColumnArm`](crate::ColumnArm), which carries
/// the key parts an arm pins and not the field filters it also wrote, so it must
/// over-approximate and refold columns the record does not touch. That is harmless when
/// the answer is re-derived from the log and it is not harmless here, so this re-checks
/// each arm's predicate per record, exactly as [`Projection::apply`] re-checks a
/// handler's path.
pub struct Tables<'a> {
    program: &'a Program,
    layout: Layout,
}

impl<'a> Tables<'a> {
    pub fn new(program: &'a Program) -> Self {
        Self {
            layout: tables::layout(program),
            program,
        }
    }

    /// The layout this was built from, so a host that also builds its schema from it
    /// derives it once.
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// What the whole read model reads, which is a host's subscription: one predicate per
    /// event any column folds, each once.
    ///
    /// **With no filters, and that is not laziness.** Two columns on one event filter it
    /// differently and a subscription is their union, so narrowing it by either one's
    /// conjuncts would withhold records the other needs. `docs/host.md` section 4 is the
    /// cut: over-delivering is a cost and under-delivering is a bug, and [`apply`](Self::apply)
    /// re-checks every arm anyway.
    ///
    /// **This method exists so that no host builds the query itself.** The first store to
    /// try it built the subscription out of [`EventPath`]'s `Display`, which keeps the
    /// `@` sigil that an event type does not have. The query matched nothing, so the
    /// thread followed the log, folded none of it, and advanced its checkpoint over every
    /// batch, while the status endpoint reported nothing behind. A subscription that is
    /// silently empty is indistinguishable from being caught up, so it is derived here
    /// where one test covers every host.
    pub fn query(&self) -> Query {
        let mut slices: Vec<Predicate> = Vec::new();
        for table in &self.layout.tables {
            for column in &table.columns {
                for event in &column.events {
                    let slice = Predicate::new(event.clone(), Vec::new());
                    if !slices.contains(&slice) {
                        slices.push(slice);
                    }
                }
            }
        }
        Query {
            slices,
            from: 0,
            upto: None,
        }
    }

    /// Applies one record to every column an arm of it names.
    ///
    /// **Fully pinned arms first, then the ranges.** An arm that leaves a key part open
    /// is a range update over every key carrying the parts it did pin, and a key this
    /// same record mints is one of them: a column written at a new key by one arm has to
    /// be visible to another arm's range over it. Reading the range first would miss it
    /// and nothing afterwards would correct it, because the next record's range reads a
    /// column that was already written and finds nothing wrong with it.
    ///
    /// Within one record is the whole of the ordering obligation. A key minted by an
    /// earlier record is already in the store, so a host applying records in position
    /// order needs no batch-wide pass.
    pub fn apply(&self, record: &Record, columns: &mut dyn Columns) -> Result<(), Error> {
        self.applying(record, None, columns)
    }

    /// The same sink, restricted to the columns a rebuild is replaying into.
    ///
    /// **What a rebuild wants, and `apply` is wrong for it twice over.** A column is
    /// rebuilt by replaying the log into it, and `apply` runs every arm of every column
    /// over that whole replay: the columns that are already correct do the work again for
    /// nothing, and a body that can `fail` or overflow fails the rebuild of a column it
    /// has nothing to do with. Handing those columns a seed instead of their held value
    /// avoids the overflow and not the rest, because the body still runs.
    ///
    /// Named by the **fold** rather than by the column, because a fold is the thing a
    /// caller knows: a column's name is `snake_case` of it, derived in
    /// [`crate::tables`] rather than written anywhere, and a caller spelling that
    /// derivation out to name a column is a second copy of it. A name no column is kept
    /// for is ignored, so an unstored fold and a typo both mean "nothing to apply" rather
    /// than an error about a listing the caller did not build.
    ///
    /// **A handle rather than an argument, for two reasons that are the same reason.**
    /// What runs is [`Layout::closure`] of `folds` and not `folds`, which costs a walk of
    /// the layout, and a replay calls the sink once per record: computing it there would
    /// put a sort and an allocation per record into the one API whose purpose is to make
    /// a rebuild cheaper. And because the closure is wider than what was asked for, a
    /// caller has to be able to see it: [`Rebuild::folds`] is that set, and it is the one
    /// a rebuild resets or shadows before the replay rather than the one it passed in.
    pub fn only<'t>(&'t self, folds: &[Ident]) -> Rebuild<'t, 'a> {
        Rebuild {
            tables: self,
            folds: self.layout.closure(folds),
        }
    }

    /// `None` is every column, which is the one difference between the two doors above.
    fn applying(
        &self,
        record: &Record,
        folds: Option<&[Ident]>,
        columns: &mut dyn Columns,
    ) -> Result<(), Error> {
        // **What this record writes, worked out before any of it is written, and across
        // every table.** The two passes are over *key discovery* and not over the writing,
        // which is the whole of this shape: a pinned arm mints a key and a range has to
        // cover it, so the pinned arms go first and `minted` is what a range unions in.
        // Reading the store back for them, which is what this did, is the same set by a
        // longer route and it put the writing in pass order.
        //
        // **The reads span the tables and the writes come after all of them**, which rule
        // 15 is what forces: a lookup reads the looked-up fold's column, and that column is
        // in another table. Discovering and writing one table at a time left a record's own
        // write to `game_genre` visible to the `genre_units` lookup that had to see the
        // value from *before* it, so a listing that set a game's genre counted under the
        // genre it was setting.
        let mut writes: BTreeMap<(usize, usize, Vec<Key>), Vec<usize>> = BTreeMap::new();
        // Rules 15 and 16: what each arm's lookups read, by (table, column, arm), carried
        // from the phase that ran before any write to the phase that does the writing.
        // Empty for every program that looks nothing up, which is almost all of them.
        let mut looked_up: HashMap<(usize, usize), HashMap<usize, Vec<Value>>> = HashMap::new();
        for (whose, table) in self.layout.tables.iter().enumerate() {
            // Per table, because a key belongs to one: a range over this table covers the
            // keys this table holds.
            let mut minted: BTreeSet<Vec<Key>> = BTreeSet::new();
            for pinned_only in [true, false] {
                for (which, column) in table.columns.iter().enumerate() {
                    // Sorted by `Layout::closure`, so this is a search rather than the
                    // scan per column per pass per record a `contains` would be.
                    if folds.is_some_and(|folds| folds.binary_search(&column.fold).is_err()) {
                        continue;
                    }
                    for (at, arm) in column.arms.iter().enumerate() {
                        if arm.event != record.event.path {
                            continue;
                        }
                        if arm.fields.iter().all(ArmMatch::pinned) != pinned_only {
                            continue;
                        }
                        let (keys, read) = self.arm(record, table, column, at, &minted, columns)?;
                        if !read.is_empty() {
                            looked_up
                                .entry((whose, which))
                                .or_default()
                                .insert(at, read);
                        }
                        for key in keys {
                            if pinned_only {
                                minted.insert(key.clone());
                            }
                            writes.entry((whose, which, key)).or_default().push(at);
                        }
                    }
                }
            }
        }

        // **One group per column and key, and its arms in written order.** A fold is its
        // arms applied in source order to one accumulator, and the two passes above hand
        // them out pinned-first, so a column with one arm of each kind on one event had
        // them the wrong way round: the pinned arm counted and the ranged arm then
        // overwrote it, where a refold resets and then counts. It is also one store read
        // and one write per row rather than per arm.
        let empty: HashMap<usize, Vec<Value>> = HashMap::new();
        for ((whose, which, key), mut arms) in writes {
            let table = &self.layout.tables[whose];
            arms.sort_unstable();
            self.fold_into(
                record,
                table,
                &table.columns[which],
                &arms,
                key,
                looked_up.get(&(whose, which)).unwrap_or(&empty),
                columns,
            )?;
        }
        Ok(())
    }

    /// Every key one arm of one column writes this record at, its predicate already
    /// checked: what [`applying`](Self::applying) groups by column and key before it folds
    /// anything.
    ///
    /// **Read-only, and that is what makes the grouping possible.** It asks
    /// [`Columns::keys`] for a range and nothing else, so every key the record writes is
    /// known before the first `set`, and the arms of one row can then be applied in the
    /// order they were written.
    ///
    /// It also hands back what the arm's **lookups** evaluated to (rules 15 and 16), which
    /// is why they come back rather than being read again where they are used: this is the
    /// phase that runs before any write, so it is the only place the looked-up column
    /// still holds the value from before this record. One list per arm and not per key,
    /// because a lookup's arguments read the record and never the key.
    #[allow(clippy::too_many_arguments)]
    fn arm(
        &self,
        record: &Record,
        table: &Table,
        column: &Column,
        at: usize,
        minted: &BTreeSet<Vec<Key>>,
        columns: &dyn Columns,
    ) -> Result<(Vec<Vec<Key>>, Vec<Value>), Error> {
        let arm = &column.arms[at];
        // **Once per arm per record, and passed down from here.** `Program::fold` is a
        // scan of every fold by name, and the per-key work below asks the same question:
        // resolving it there cost one scan per key of a range, in the path `Tables::only`
        // exists to make cheaper. `fold_into` keeps its own, which is once per row written
        // rather than once per key considered.
        let def = self
            .program
            .fold(&column.fold)
            .ok_or_else(|| Error::new(ErrorKind::UnknownFold(column.fold.to_string())))?;
        let module = def.module.as_deref();
        // Built on demand and reused across parts, because building it means reading and
        // sealing every bound field.
        let mut computed: Option<Frame> = None;

        // The arm's own narrowing, before a lookup is read or a computed part evaluated.
        // `matching` checks it again per key, and has to: a ranged arm has one key per row
        // rather than the one this resolved against.
        let slice = &def.stage.slices[at];
        if (!slice.keys.is_empty() || !slice.lookups.is_empty())
            && !self
                .narrowed(record, table, column, at, def)
                .map_err(|err| err.in_module(module))?
        {
            return Ok((Vec::new(), Vec::new()));
        }

        // The lookups, read here because this is the phase that runs before any write, so
        // the looked-up column still holds the value from before this record. A computed
        // part below reads the slots they fill, and so does the arm's value, which is in
        // `folded` on the other side of the writes and is why these are handed back.
        let read = match slice.lookups.is_empty() {
            true => Vec::new(),
            false => {
                let frame = match computed {
                    Some(ref mut frame) => frame,
                    None => computed.insert(
                        self.record_frame(record, def, slice, columns)
                            .map_err(|err| err.in_module(module))?,
                    ),
                };
                slice
                    .lookups
                    .iter()
                    .map(|lookup| frame.get(lookup.slot).cloned())
                    .collect::<Result<Vec<Value>, ErrorKind>>()
                    .map_err(|err| Error::new(err).in_module(module))?
            }
        };

        // The key, part by part, in the table's order. A part the arm pinned is known
        // from the record or from the arm itself; one it left open is a range, and the
        // pinned parts are what narrows it.
        let mut key: Vec<Option<Key>> = Vec::with_capacity(table.key.len());
        for (part, matched) in table.key.iter().zip(&arm.fields) {
            let value = match matched {
                ArmMatch::Field(named) => {
                    let value = field(&record.event, named)?.clone();
                    fitted_at(value, &part.ty, Span::default())?
                }
                // Rule 13: not on the event at all, and resolved against an empty frame.
                ArmMatch::Fixed(id) => tables::fixed(self.program, &column.fold, part, *id)?,
                // **The inverse of what a by-key read does with the same expression.**
                // There is no argument to compare against here, so whatever this
                // evaluates to over this record is the key the row is written at, and
                // `fold_into` binds the parameter from it below.
                ArmMatch::Computed(id) => {
                    let frame = match computed {
                        Some(ref mut frame) => frame,
                        None => computed.insert(
                            self.record_frame(record, def, slice, columns)
                                .map_err(|err| err.in_module(module))?,
                        ),
                    };
                    let value = eval(self.program, &def.exprs, frame, *id, None)
                        .map_err(|err| err.in_module(module))?;
                    // **Rule 19: `else skip` and absent, so this record has no key.** Not
                    // this part and not this arm: a key is whole or it is no key, so the
                    // arm writes nothing at all for this record and the by-key read skips
                    // it under every key, which is the two drivers giving one answer.
                    //
                    // The flag is looked up by the expression rather than carried on
                    // `ArmMatch`, which keeps hekla's DDL exactly as it was: a lookup is
                    // still a `Computed` and the layout still reports `pinned()`.
                    let skip = slice.keys.iter().any(|key| key.value == *id && key.skip);
                    match (skip, value) {
                        (true, Value::Opt { value: None, .. }) => {
                            return Ok((Vec::new(), read));
                        }
                        (
                            true,
                            Value::Opt {
                                value: Some(held), ..
                            },
                        ) => fitted_at(*held, &part.ty, def.exprs.span(*id))
                            .map_err(|err| err.in_module(module))?,
                        (_, value) => fitted_at(value, &part.ty, def.exprs.span(*id))
                            .map_err(|err| err.in_module(module))?,
                    }
                }
                ArmMatch::Open => {
                    key.push(None);
                    continue;
                }
            };
            // A key part is a scalar by construction, since `Type::can_key` is what let
            // this fold be stored at all, so a value that will not reduce is a bug in the
            // layout rather than a program a host should be told about.
            let Some(part) = Key::from_value(&value) else {
                return Err(ErrorKind::MalformedIr.into());
            };
            key.push(Some(part));
        }

        let whole: Option<Vec<Key>> = key.iter().cloned().collect();
        let mut keys = match whole {
            Some(key) => vec![key],
            None => {
                let pinned: Vec<(usize, Key)> = key
                    .iter()
                    .enumerate()
                    .filter_map(|(at, part)| part.clone().map(|part| (at, part)))
                    .collect();
                // **The store's keys and the ones this record has just minted.** A range
                // covers every key the table holds, and a key an earlier arm of this same
                // record minted is one of them: nothing has been written yet, so the store
                // cannot answer for it and `minted` is where it is. Deduplicated, because
                // a key the store already held and this record minted again is one key and
                // folding its arms twice would count the record twice.
                //
                // **Both sides go through `covers`, including the store's.**
                // `Columns::keys` is held to `Log::read`'s cut: over-delivering is a cost
                // and under-delivering is a bug, so a host may answer with more than the
                // arm writes and this is where the rest is dropped. Leaving it to the
                // predicate re-check was not the same thing: a part the arm pinned with a
                // rule 13 conjunct is bound from whatever key came back, so the gate goes
                // dead, and a dead gate here is a `malformed ir` that aborts the record
                // rather than a key to skip.
                let mut found = columns.keys(&table.name, &pinned)?;
                found.retain(|key| covers(&pinned, key));
                found.extend(minted.iter().filter(|key| covers(&pinned, key)).cloned());
                found.sort();
                found.dedup();
                found
            }
        };
        // The check a host cannot make for itself: an arm's field filters, which
        // `ColumnArm` does not carry. Here rather than at the fold, so the list is exactly
        // what the arm writes and `minted` says what the table now holds rather than what
        // it might.
        let mut writes = Vec::with_capacity(keys.len());
        for key in keys.drain(..) {
            if self.matching(record, column, at, &key, def)? {
                writes.push(key);
            }
        }
        Ok((writes, read))
    }

    /// Whether this arm's predicate holds for this record, checked **before** a computed
    /// key part is evaluated.
    ///
    /// A by-key read checks the predicate and skips the record before it ever runs the
    /// expression (`fold`'s inner loop), so the sink must not raise on a record the arm
    /// was never going to fold: `key rate = 100 / units` under `(kind: "paid")` failed a
    /// whole replay on a free record with no units, where the same fold read by key
    /// answered.
    ///
    /// **It can check the whole predicate, and rule 14 is what pays for that.** A
    /// conjunct's value reads nothing but the arm's parameters, since a filter is parsed
    /// before the destructure's scope exists and cannot read the accumulator, and in an
    /// arm with a `key` clause every parameter a conjunct reads is one the arm *pins*:
    /// one it computes is refused, and one it leaves open is refused, which is exactly so
    /// that none of them is a value this has to go and find. A pinned part is on the
    /// record or fixed by the arm, so the frame below is complete.
    ///
    /// Through `resolved_one` and `matches`, which is what `folded` resolves with, so the
    /// two cannot come to disagree about what an arm matches.
    fn narrowed(
        &self,
        record: &Record,
        table: &Table,
        column: &Column,
        at: usize,
        def: &Fold,
    ) -> Result<bool, Error> {
        let mut frame = Frame::new(def.frame);
        for ((part, matched), name) in table
            .key
            .iter()
            .zip(&column.arms[at].fields)
            .zip(&column.args)
        {
            let value = match matched {
                ArmMatch::Field(named) => field(&record.event, named)?.clone(),
                ArmMatch::Fixed(id) => tables::fixed(self.program, &column.fold, part, *id)?,
                // The two rule 14 refuses a conjunct for reading, so neither is needed.
                ArmMatch::Computed(_) | ArmMatch::Open => continue,
            };
            let Some(param) = def.params.iter().find(|param| &param.name == name) else {
                return Err(ErrorKind::MalformedIr.into());
            };
            let value = fitted_at(value, &param.ty, Span::default())?;
            frame.set(param.slot, value)?;
        }
        let slice = &def.stage.slices[at];
        let resolved = resolved_one(self.program, &def.exprs, slice, &mut frame)?;
        Ok(resolved.live && matches(&resolved.predicate, &record.event)?)
    }

    /// Whether one arm matches this record **at one key**: the check a host cannot make
    /// for itself, because [`ColumnArm`] carries the key parts an arm pins and not the
    /// field filters it also wrote.
    ///
    /// Split out of the fold, where it used to sit, so that
    /// [`applying`](Self::applying) knows every key the record writes before it writes
    /// any of them. That is what lets a row's arms be folded in the order they were
    /// written, and what makes the minted set exact: a key an arm computed but whose
    /// filters failed is not one the table now holds, and another column's range must not
    /// be told otherwise.
    ///
    /// **Resolved after the parameters, which is what makes a rule 13 gate hold.** A
    /// caller resolves a key conjunct as a *gate*, comparing what the arm writes against
    /// the argument it was given; there is no argument here, so the key came from the arm
    /// and the gate is true by construction. Going through `resolved_one` anyway is what
    /// keeps a field filter fitted identically, and a dead slice at this point is a
    /// disagreement between `layout` and the arm rather than a case to skip.
    fn matching(
        &self,
        record: &Record,
        column: &Column,
        at: usize,
        key: &[Key],
        def: &Fold,
    ) -> Result<bool, Error> {
        // Stamped over the whole of it, which is what `fold_into` does with the fold and
        // for the reason its comment gives: this is the boundary that knows which module
        // the fold came from, and a `MissingField` raised while comparing a predicate
        // against a record reached a host naming no file at all.
        let module = def.module.as_deref();
        self.matched(record, column, at, key, def)
            .map_err(|err| err.in_module(module))
    }

    /// The body of [`matching`](Self::matching), so one `map_err` covers all of it.
    fn matched(
        &self,
        record: &Record,
        column: &Column,
        at: usize,
        key: &[Key],
        def: &Fold,
    ) -> Result<bool, Error> {
        let mut frame = Frame::new(def.frame);
        self.bind_key(column, key, def, &mut frame)?;
        let slice = &def.stage.slices[at];
        let resolved = resolved_one(self.program, &def.exprs, slice, &mut frame)?;
        if !resolved.live {
            return Err(ErrorKind::MalformedIr.into());
        }
        matches(&resolved.predicate, &record.event)
    }

    /// The parameters from the key, which is the inverse of what a caller does and the
    /// reason [`Column::args`] is recorded: a key part is named for its subject or for the
    /// parameter with one `_id` off, so the part's name is not callable.
    ///
    /// [`Column::args`]: crate::Column::args
    fn bind_key(
        &self,
        column: &Column,
        key: &[Key],
        def: &Fold,
        frame: &mut Frame,
    ) -> Result<(), Error> {
        for (name, part) in column.args.iter().zip(key) {
            let Some(param) = def.params.iter().find(|param| &param.name == name) else {
                return Err(ErrorKind::MalformedIr.into());
            };
            let value = fitted_at(value::from_key(part), &param.ty, Span::default())?;
            frame.set(param.slot, value)?;
        }
        Ok(())
    }

    /// The frame a computed key part is evaluated in: this record's envelope and this
    /// slice's binds, which is exactly what `key param = ...` is allowed to read.
    ///
    /// The same two loops `fold_into` runs, lifted here because a computed part is needed
    /// before there is a key, and the key is what `fold_into` takes.
    fn record_frame(
        &self,
        record: &Record,
        def: &Fold,
        slice: &Slice,
        columns: &dyn Columns,
    ) -> Result<Frame, Error> {
        let mut frame = Frame::new(def.frame);
        for bind in &def.envelope {
            frame.set(bind.slot, envelope_value(record, bind.field))?;
        }
        for bind in &slice.binds {
            let value = field(&record.event, &bind.field)?.clone();
            let value = seal(self.program, &record.event, &bind.field, value)?;
            frame.set(bind.slot, value)?;
        }
        // **Rule 15, read before this record has written anything.** `applying` works out
        // every key before it writes any of them, so the store holds the looked-up column
        // at exactly this record's position: records arrive in order and nothing for this
        // one has landed. That is the handoff's "evaluate every lookup for the record
        // before writing anything for that record", and it is structural here rather than
        // a rule to keep, because the phase that reads is a different loop from the phase
        // that writes.
        if !slice.lookups.is_empty() {
            let asof = Asof::Stored {
                layout: &self.layout,
                columns,
            };
            lookups(self.program, &def.exprs, slice, &mut frame, &asof)?;
        }
        Ok(frame)
    }

    /// One arm of one column at one key: the fold's own body, with the accumulator read
    /// from the store and written back to it.
    #[allow(clippy::too_many_arguments)]
    fn fold_into(
        &self,
        record: &Record,
        table: &Table,
        column: &Column,
        arms: &[usize],
        key: Vec<Key>,
        // What each of this column's arms looked up, from the phase that ran before any
        // write: see `Tables::arm`. Keyed by arm index, since the values are per arm and
        // not per key.
        read: &HashMap<usize, Vec<Value>>,
        columns: &mut dyn Columns,
    ) -> Result<(), Error> {
        let def = self
            .program
            .fold(&column.fold)
            .ok_or_else(|| Error::new(ErrorKind::UnknownFold(column.fold.to_string())))?;
        let module = def.module.as_deref();
        // Stamped once here, which is the boundary that knows which module the fold came
        // from, exactly as `run` and `project` are the boundary for a handler's.
        let folded = self
            .folded(record, table, column, arms, &key, def, read, columns)
            .map_err(|err| err.in_module(module))?;
        // Not shredded. This is the fill path, so it behaves as `fold_stored` does and
        // hands a `Value::Sealed` through for the store to hold and shred on its own
        // read, which is what makes an erasure after the fill observable without
        // refolding anything.
        match folded {
            Some(folded) => columns.set(&table.name, key, &column.name, folded),
            // No arm of this column writes this key, which `applying` does not ask for.
            None => Ok(()),
        }
    }

    /// What one arm folds this record onto the column's held value, or `None` where the
    /// arm does not match it after all.
    #[allow(clippy::too_many_arguments)]
    fn folded(
        &self,
        record: &Record,
        table: &Table,
        column: &Column,
        arms: &[usize],
        key: &[Key],
        def: &Fold,
        read: &HashMap<usize, Vec<Value>>,
        columns: &dyn Columns,
    ) -> Result<Option<Value>, Error> {
        if arms.is_empty() {
            return Ok(None);
        }
        let mut frame = Frame::new(def.frame);
        self.bind_key(column, key, def, &mut frame)?;

        // The accumulator: what the column holds, or the seed where it holds nothing. A
        // key no event has touched folds to the seed, so an absent row and an absent
        // value are one thing (`docs/folds.md` section 11). Read **once** for the row,
        // not once per arm, which is what folding a row's arms together buys besides
        // getting their order right.
        let held = match columns.value(&table.name, key, &column.name)? {
            Some(value) => value,
            None => {
                let var = def
                    .stage
                    .folds
                    .iter()
                    .find(|var| var.slot == def.slot)
                    .ok_or(ErrorKind::MalformedIr)?;
                let value = eval(self.program, &def.exprs, &mut frame, var.init, None)?;
                fitted_at(value, &var.ty, def.exprs.span(var.init))?
            }
        };
        frame.set(def.slot, held)?;

        // Before the slices, because an arm's value reads these. One list for the whole
        // fold, exactly as the free `fold` driver writes them.
        for bind in &def.envelope {
            frame.set(bind.slot, envelope_value(record, bind.field))?;
        }

        // **In the order they were written**, which is the fold's own inner loop for the
        // arms that match this row. `applying` sorted them and `matching` is what decided
        // they belong here, so there is nothing left to re-check: a fold is its arms
        // applied in source order to one accumulator, and so is this.
        for &at in arms {
            let slice = &def.stage.slices[at];
            // Rules 15 and 16: what this arm's lookups read, which `Tables::arm` evaluated
            // before anything was written for this record. Put back rather than read
            // again, because by now the writing has started and the looked-up column may
            // be one of the things written: reading it here would be reading it "after"
            // the record rather than before it.
            if let Some(values) = read.get(&at) {
                for (lookup, value) in slice.lookups.iter().zip(values) {
                    frame.set(lookup.slot, value.clone())?;
                }
            }
            for bind in &slice.binds {
                let value = field(&record.event, &bind.field)?.clone();
                let value = seal(self.program, &record.event, &bind.field, value)?;
                frame.set(bind.slot, value)?;
            }
            for update in &slice.updates {
                let value = eval(self.program, &def.exprs, &mut frame, update.value, None)?;
                let value = fitted_at(value, &update.ty, def.exprs.span(update.value))?;
                frame.set(update.slot, value)?;
            }
        }
        Ok(Some(frame.get(def.slot)?.clone()))
    }
}

/// [`Tables`] restricted to the columns a rebuild is replaying into, with
/// [`Layout::closure`] of them worked out once. See [`Tables::only`].
pub struct Rebuild<'t, 'p> {
    tables: &'t Tables<'p>,
    folds: Vec<Ident>,
}

impl Rebuild<'_, '_> {
    /// Every column this applies, named by fold and sorted: the closure of what was
    /// asked for, which is **wider** where a ranged column is among them.
    ///
    /// **This is the set a rebuild has to prepare, and the reason the handle exists.** The
    /// sink carries the accumulator in the store rather than re-deriving it, so replaying
    /// the log onto a column that already holds its value folds every record into it a
    /// second time. A rebuild therefore resets or shadows each of these before it starts,
    /// and the ones it did not ask for are the ones it throws away afterwards. What it may
    /// not do is drop their writes as they go, because the keys are what the additions are
    /// *for*: recording each key and discarding the value is enough, since
    /// [`Columns::keys`] is all a ranged arm reads, and keeping both costs only an
    /// overlay.
    ///
    /// [`Columns::keys`]: crate::Columns::keys
    ///
    /// So the closure buys correctness and spends the second thing
    /// [`Tables::only`] exists for: the bodies it adds run over the whole replay, and one
    /// of them that can `fail` or overflow can still fail a rebuild of the ranged column
    /// beside it. A column whose every arm pins the whole key closes over itself and pays
    /// neither.
    pub fn folds(&self) -> &[Ident] {
        &self.folds
    }

    /// One record, into these columns. [`Tables::apply`]'s rules, unchanged.
    pub fn apply(&self, record: &Record, columns: &mut dyn Columns) -> Result<(), Error> {
        self.tables.applying(record, Some(&self.folds), columns)
    }
}

/// What left, and what rule 5 absorbed on the way. Counted here rather than on the
/// host because the retry loop is heklang's: a host performs one attempt.
#[derive(Debug, Clone, Default)]
struct Traffic {
    sent: Vec<Request>,
    performed: usize,
    absorbed: usize,
}

pub struct Interpreter<'a, H = Harness> {
    program: &'a Program,
    /// The log, the clock, the key store and the network. Everything the interpreter
    /// did not compute for itself.
    host: H,
    traffic: Traffic,
    lines: Vec<String>,
    /// What the effects did to the world, in order. `docs/testing.md` rule 7 asserts
    /// against this, and it is the whole of what an effect produces. Not part of the
    /// world: it is heklang's record of what it asked the world for.
    trace: Vec<Effectful>,
}

/// The harness's own affordances: seeding a log, scripting a reply and shredding a key
/// are how a test writes a world, and `docs/testing.md` section 3 is where the language
/// spells them. A real host is handed one that already exists.
impl<'a> Interpreter<'a, Harness> {
    pub fn new(program: &'a Program) -> Self {
        Self::with_host(program, Harness::default())
    }

    pub fn with_log(program: &'a Program, log: impl IntoIterator<Item = Event>) -> Self {
        Self::with_host(program, Harness::with_log(log))
    }

    pub fn log(&self) -> &[Record] {
        self.host.records()
    }

    /// Appends with a synthesised envelope. The id and timestamp are derived from the
    /// position so a run is reproducible; a real host stamps its own.
    pub fn append(&mut self, event: Event) {
        self.host.push(event);
    }

    /// One deployment credential, overriding the harness's `secret:NAME` stand-in. The
    /// same lever `docs/testing.md`'s `secret NAME = "..."` pulls.
    pub fn set_secret(&mut self, name: &str, value: impl Into<Arc<str>>) {
        self.host.set_secret(name, value);
    }

    /// A deployment that did not set one, which is how a required secret reaches
    /// `ErrorKind::MissingSecret` and an optional one reaches its absent branch.
    pub fn unset_secret(&mut self, name: &str) {
        self.host.unset_secret(name);
    }

    /// Queues the replies one URL will answer with.
    pub fn script(&mut self, url: &str, replies: impl IntoIterator<Item = Reply>) {
        self.host.script(url, replies);
    }

    /// Marks a subject erased without an effect having done it, which is the case rule
    /// 12's message is about: the erase is usually not local.
    pub fn erase_subject(&mut self, subject: &str, id: &str) {
        self.host.erase_subject(subject, id);
    }
}

impl<'a, H: Host> Interpreter<'a, H> {
    /// A program against a world that already exists.
    pub fn with_host(program: &'a Program, host: H) -> Self {
        Self {
            program,
            host,
            traffic: Traffic::default(),
            lines: Vec::new(),
            trace: Vec::new(),
        }
    }

    /// The world this ran against, so an embedder can read back what its own host
    /// recorded. Shared: the interpreter is still using it.
    pub fn host(&self) -> &H {
        &self.host
    }

    /// Every request that actually left, including the attempts rule 5 absorbed. Not
    /// the host's to report: the loop that made them is heklang's.
    pub fn requests(&self) -> &[Request] {
        &self.traffic.sent
    }

    pub fn http_calls(&self) -> usize {
        self.traffic.performed
    }

    /// Retryable responses the runtime absorbed, which the handler never saw (rule 5).
    pub fn absorbed(&self) -> usize {
        self.traffic.absorbed
    }

    /// `log` output. Not journaled (rule 10), so a replay adds to it again.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Everything the effects did to the world, in the order they did it. Ordered and
    /// complete, which is what lets a test say "and nothing else".
    pub fn trace(&self) -> &[Effectful] {
        &self.trace
    }

    /// Delivers one position to one effect. A journal carried across two calls is what
    /// makes the second a replay: journaled calls return their recorded result and are
    /// not performed again, while `reveal` and `log` run every time.
    pub fn deliver(
        &mut self,
        effect: &str,
        position: u64,
        journal: &mut dyn Calls,
    ) -> Result<Invocation, Error> {
        let target = self
            .program
            .effect(effect)
            .ok_or_else(|| ErrorKind::UnknownEffect(effect.to_string()))?;
        let module = target.module.clone();
        self.invoke_arm(target, position, journal)
            .map_err(|err| err.in_module(module.as_deref()))
    }

    fn invoke_arm(
        &mut self,
        effect: &'a Effect,
        position: u64,
        journal: &mut dyn Calls,
    ) -> Result<Invocation, Error> {
        let Some(record) = self.host.record(position)? else {
            return Err(ErrorKind::NoSuchPosition(position).into());
        };
        // Rule 1: one event selects exactly one arm, so this is a lookup.
        let Some(arm) = effect.arm(&record.event.path) else {
            return Ok(Invocation::Ignored);
        };
        // Rule 15: the lane is part of what the arm declared, so an event that cannot
        // produce one is an event its own declaration disagrees with, and this delivery
        // wedges rather than running against a key nobody could read. It is also what
        // lets a walk resolve its collapse set without being able to fail: `collapsed`
        // stops at such a record and leaves saying so to the delivery that reaches it.
        // A missing field would already be caught by the bind below; a field holding
        // something that cannot name a lane would not, because a frame slot takes any
        // value.
        partition_key(arm, &record.event)?;

        let program = self.program;
        let mut frame = Frame::new(arm.frame);
        for bind in &arm.binds {
            let value = field(&record.event, &bind.field)?.clone();
            let value = seal(program, &record.event, &bind.field, value)?;
            frame.set(bind.slot, value)?;
        }
        for bind in &arm.envelope {
            frame.set(bind.slot, envelope_value(&record, bind.field))?;
        }

        let mut used = BTreeMap::new();
        // Rule 11: pinned once, before anything in the arm runs, so a stage boundary
        // cannot move it and two calls in one body are two reads of one value. Unlike a
        // command's head this is journalled, which is why it is taken through `Effects`.
        if let Some(slot) = arm.now {
            let mut ctx = Effects {
                program: self.program,
                host: &mut self.host,
                journal: &mut *journal,
                traffic: &mut self.traffic,
                lines: &mut self.lines,
                trace: &mut self.trace,
                used: &mut used,
            };
            let at = ctx.now()?;
            frame.set(slot, Value::Timestamp(at))?;
        }

        // An arm's stages are a command's, minus everything about appending: rule 3
        // folds every one of them to the trigger's own position, so a later stage reads
        // the same prefix the first did and there is no head to pin and no condition to
        // build. `Effects` is rebuilt around each half because it holds the host
        // mutably and a fold wants it shared.
        let mut flow: Result<Flow, Error> = Ok(Flow::Next);
        'stages: for stage in &arm.stages {
            for half in [Half::Pre, Half::Post] {
                if half == Half::Post && (!stage.folds.is_empty() || stage.reads()) {
                    let resolved = resolve(program, &arm.exprs, &stage.slices, &mut frame)?;
                    for var in &stage.folds {
                        let value = eval(program, &arm.exprs, &mut frame, var.init, None)?;
                        let value = fitted_at(value, &var.ty, arm.exprs.span(var.init))?;
                        frame.set(var.slot, value)?;
                    }
                    if stage.reads() {
                        // Rule 3: the fold stops at the trigger's own position,
                        // inclusive, so state is a pure function of the log prefix and
                        // that position, and counts the trigger.
                        let folding = Folding {
                            slices: &stage.slices,
                            resolved: &resolved,
                            envelope: &[],
                        };
                        fold(
                            program,
                            &arm.exprs,
                            &self.host,
                            &folding,
                            0,
                            Some(position),
                            &mut frame,
                            None,
                        )?;
                    }
                }

                let part = match half {
                    Half::Pre => &stage.pre,
                    Half::Post => &stage.post,
                };
                let ctx = Effects {
                    program: self.program,
                    host: &mut self.host,
                    journal: &mut *journal,
                    traffic: &mut self.traffic,
                    lines: &mut self.lines,
                    trace: &mut self.trace,
                    used: &mut used,
                };
                let mut sink = Sink::Effect(ctx);
                flow = exec_block(&arm.exprs, part, &mut frame, self.program, &mut sink);
                if !matches!(flow, Ok(Flow::Next)) {
                    break 'stages;
                }
            }
        }
        match flow {
            // Rule 4's terminal outcome, whether the `fail` was written in the arm or
            // in an effect-local `fn` it called. A call is an expression, so a helper's
            // has to arrive as an error; the outcome and the trace entry are the same.
            Ok(Flow::Return(Ret::Fail(message)))
            | Err(Error {
                kind: ErrorKind::Failed(message),
                ..
            }) => {
                self.trace.push(Effectful::Failed(message.clone()));
                Ok(Invocation::Failed(message))
            }
            Ok(_) => Ok(Invocation::Done),
            // Rule 12: terminal, so the cursor advances and this is counted apart from
            // a wedge, which does not advance.
            Err(err) if matches!(err.kind, ErrorKind::Erased { .. }) => {
                let message = err.kind.to_string();
                self.trace.push(Effectful::Skipped(message.clone()));
                Ok(Invocation::Skipped(message))
            }
            Err(err) => Err(err),
        }
    }

    /// Runs one effect over the log, following it as an `invoke` lengthens it. A wedge
    /// stops the walk, because a wedged invocation does not advance.
    ///
    /// **This is the harness's dispatcher, and it honours one of rule 15's two modifiers.**
    /// `on latest` collapses here, because the batch it needs is the backlog this walk can
    /// see. `on live` does not: the boundary is a position a runtime resolves once at first
    /// activation and keeps, and nothing in a `Program` or a `Log` holds it, so a `live`
    /// arm is driven exactly as an `on` arm would be and history fires. A host that wants
    /// the declared meaning keeps that boundary itself and calls `deliver` per position,
    /// which is the seam `docs/host.md` describes.
    pub fn drive(&mut self, effect: &str) -> Result<Counts, Error> {
        let mut counts = Counts::default();
        let start = self.host.head()? as usize;
        // Rule 15: an `on latest` arm runs once per key per batch, at the newest matching
        // position in it. Catching up, the batch is the whole backlog, which is what is in
        // the log now; an event appended while this walk runs arrives on its own, so it is
        // a batch of one and collapses with nothing.
        let collapsed = self.collapsed(effect, start as u64);
        // One entry per event the walk visits. Everything already in the log is a step
        // zero, and an event appended while handling one at step `d` is a `d + 1`.
        let mut steps = vec![0u32; start];
        let mut position = 0u64;

        while position < self.host.head()? {
            if collapsed.contains(&position) {
                counts.collapsed += 1;
                position += 1;
                continue;
            }
            // One journal per invocation: it is the memory of this position's calls,
            // and nothing carries between positions.
            let mut journal = Journal::default();
            let before = self.host.head()?;
            let outcome = self.deliver(effect, position, &mut journal);

            if self.host.head()? > before {
                let step = steps[position as usize] + 1;
                steps.resize(self.host.head()? as usize, step);
                if step > CASCADE {
                    let mut events = Vec::new();
                    for position in start as u64..self.host.head()? {
                        if let Some(record) = self.host.record(position)? {
                            events.push(record.event.path.to_string());
                        }
                    }
                    return Err(ErrorKind::Cascade {
                        effect: effect.to_string(),
                        events,
                    }
                    .into());
                }
            }

            match outcome {
                Ok(Invocation::Done) => counts.done += 1,
                Ok(Invocation::Ignored) => counts.ignored += 1,
                Ok(Invocation::Failed(message)) => counts.failures.push(message),
                Ok(Invocation::Skipped(message)) => counts.skips.push(message),
                // A wedge does not advance, so the walk stops rather than skipping
                // work an operator has not agreed to drop.
                Err(err) => {
                    counts.wedged = Some((position, err));
                    return Ok(counts);
                }
            }
            position += 1;
        }
        Ok(counts)
    }

    /// The positions in `0..upto` an `on latest` arm selected and will not be invoked for,
    /// because a later position in the same batch names the same lane.
    ///
    /// Grouped by arm as well as by key: rule 1 makes an event select exactly one arm, and
    /// two arms of one effect may key by different fields, so "the same key" is only a
    /// question inside one arm. Two events of *different* types do collapse together when
    /// one arm lists them both, which is the case the rule exists for.
    ///
    /// **It answers rather than fails**, and it may only do that because every way it can
    /// stop early is one the walk stops on too. An unknown effect collapses nothing. A
    /// record it cannot read, a position with no record, and a key it cannot build all end
    /// the scan, so nothing past that point collapses; the walk then reaches that same
    /// position and wedges there, naming it, with everything before it delivered. Failing
    /// here instead would turn a bad record at position 900 into an error with no counts
    /// and no partial progress.
    ///
    /// The third of those is why `invoke_arm` reads the key it is not otherwise going to
    /// use: without that, a value that cannot name a lane would bind into a frame slot
    /// happily, the walk would sail past, and one bad key would quietly downgrade the rest
    /// of the log from `on latest` to `on`.
    fn collapsed(&self, effect: &str, upto: u64) -> HashSet<u64> {
        let mut out = HashSet::new();
        let Some(target) = self.program.effect(effect) else {
            return out;
        };
        if !target
            .arms
            .iter()
            .any(|arm| arm.delivery == Delivery::Latest)
        {
            return out;
        }
        // One entry per lane rather than per position: the loser of each pair is known as
        // soon as the winner replaces it, so nothing has to be kept to compare later.
        let mut newest: HashMap<(usize, Vec<Key>), u64> = HashMap::new();
        for position in 0..upto {
            // A hole stops the scan like an unreadable record does, because the walk
            // treats it the same way: `invoke_arm` answers `NoSuchPosition`, a wedge.
            let Ok(Some(record)) = self.host.record(position) else {
                break;
            };
            let Some(index) = target.arm_index(&record.event.path) else {
                continue;
            };
            let arm = &target.arms[index];
            if arm.delivery != Delivery::Latest {
                continue;
            }
            let Ok(key) = partition_key(arm, &record.event) else {
                break;
            };
            if let Some(earlier) = newest.insert((index, key), position) {
                out.insert(earlier);
            }
        }
        out
    }

    /// Folds the whole log into this projector's read models, in memory.
    ///
    /// These are heklang's own rows rather than a host's, so heklang is what shreds a
    /// sealed column whose key is gone. [`project_into`](Self::project_into) does not:
    /// the rows there belong to the caller, and `docs/host.md` section 7 is that a
    /// `Value::Sealed` reaches their `put` so a host can store one without opening it.
    pub fn project(&self, name: &str) -> Result<Store, Error> {
        let mut store = Store::default();
        let mut rows = Shredding {
            keys: &self.host,
            rows: &mut store,
        };
        self.project_into(name, &mut rows)?;
        Ok(store)
    }

    /// The same fold, into read models the host keeps. A record is applied inside the
    /// visitor rather than after it, so a projection's live heap does not have to hold
    /// its whole boundary at once, which is what `docs/host.md` rule 4 hands a visitor
    /// for in the first place.
    pub fn project_into(&self, name: &str, rows: &mut dyn Rows) -> Result<(), Error> {
        let projection = Projection::new(self.program, name)?;
        let module = projection.projector.module.clone();
        let query = projection.query();
        self.host
            .read(&query, &mut |record| projection.apply(record, rows))
            .map_err(|err| err.in_module(module.as_deref()))
    }

    /// Folds one named fold over the whole log and hands back the value it folded to.
    ///
    /// **This is the first thing that executes a named fold.** Everywhere else one runs
    /// by having been spliced into a caller by [`crate::inline`], and a fold a command
    /// reads never exists at runtime as itself. A read model is a named fold kept for
    /// every key ([`crate::tables`]), so a stored column is served by calling this with
    /// the key, and that is also what a test's `fold` action does.
    ///
    /// The whole log, with no upper bound. The two conventions a handler folds under both
    /// exist to protect something this does not have: a command stops below its pinned
    /// head so a decision is not made on events its append condition is about to reject,
    /// and an effect arm stops at its trigger so delivery is a function of a prefix. A
    /// read model has neither an append condition nor a trigger, and reads the log as it
    /// now stands, which is what [`project_into`](Self::project_into) does too.
    ///
    /// A store filling a column wants neither half of that, and calls
    /// [`fold_stored`](Self::fold_stored) instead.
    pub fn fold(
        &self,
        name: &str,
        args: impl IntoIterator<Item = (impl Into<Ident>, Value)>,
    ) -> Result<Value, Error> {
        let def = self.fold_def(name)?;
        let module = def.module.as_deref();
        let mut folded = self
            .folding(def, args, None)
            .map_err(|err| err.in_module(module))?;
        // A column holding sealed content whose key is gone reads back absent, exactly as
        // the same column does through `project`. This hands the value to whoever asked
        // for it, so there is no later step to shred in.
        shred(&self.host, &mut folded, || def.name.to_string())
            .map_err(|err| err.in_module(module))?;
        Ok(folded)
    }

    /// The same fold, bounded at a log length and with a sealed value handed through.
    ///
    /// **This is the read a store fills a column from**, and it is to [`fold`](Self::fold)
    /// what [`project_into`](Self::project_into) is to [`project`](Self::project). Two
    /// differences, and a store needs both:
    ///
    /// - **`head` is how long the log was**, which is the number [`Log::head`] answers and
    ///   therefore the position the next append lands at, not the last one folded. A store
    ///   records one of those with the whole table so that every column of a row is at the
    ///   same one and a query reads one position, and `docs/host.md` says `head` and `read`
    ///   are not atomic together, so the bound has to be the store's rather than whatever
    ///   the log has reached by the time each column is folded.
    ///
    ///   It takes a head rather than the inclusive last position `Query::upto` carries
    ///   because a head is the only number a store has: it is what the seam hands out,
    ///   what a command pins its own read to, and what a checkpoint records. Asking for
    ///   `head - 1` instead put the subtraction at every call site and left an empty log
    ///   with nothing to pass, since `0` is a position like any other and would have
    ///   folded the first event into a column recorded as holding none of it. A caller
    ///   that really does hold a position asks for `position + 1`.
    /// - **A `Value::Sealed` is handed back rather than shredded**, for the reason
    ///   `project_into` hands one to `Rows::put`: the store holds the ciphertext and
    ///   shreds on its own read, which is what makes an erasure after the fill observable
    ///   without refolding anything. Shredding here would also fail the fill for a
    ///   non-optional sealed column whose subject was erased before it, where the same
    ///   column through a projector stores its ciphertext and raises at the query.
    ///
    /// The two travel together because a store wants both, not because they are one
    /// thing. A console showing a value as of a position wants the bound and the shred,
    /// and is the caller that would split them.
    pub fn fold_stored(
        &self,
        name: &str,
        args: impl IntoIterator<Item = (impl Into<Ident>, Value)>,
        head: u64,
    ) -> Result<Value, Error> {
        let def = self.fold_def(name)?;
        let module = def.module.as_deref();
        self.folding(def, args, Some(head))
            .map_err(|err| err.in_module(module))
    }

    /// The same read as [`fold_stored`](Self::fold_stored), reporting the accumulator
    /// **after each record it folds**, which is what a fold's history is made of.
    ///
    /// `visit` is called once per record the fold's own query delivers, in log order, with
    /// that record and the value as of it. A reader diffing consecutive values gets the
    /// positions where the column changed, the value before and after, and the record that
    /// did it. Records that moved nothing are reported too: a purchase that passes through
    /// a balance's arm without changing it is a fact about that record either way, and
    /// deciding it here would be deciding for every caller.
    ///
    /// The bound and the sealed hand-back are `fold_stored`'s, for its reasons: `head` is a
    /// log length, so `0` yields no steps at all, and a `Value::Sealed` arrives untouched
    /// because the store holds the ciphertext and shreds on its own read.
    ///
    /// **There is no lower bound**, and that is not an omission: the value after a record
    /// depends on every record before it, so a fold started in the middle would report a
    /// first step that is the seed plus a tail and was never what the column held. A caller
    /// paging a history folds from zero and filters what it reports.
    ///
    /// **A column with a range arm is refused**, and that is the one restriction. A refold
    /// is a function of a key and the whole log, so it applies every ranged record to every
    /// key, while the sink walks forward and applies one only to the rows that exist: a key
    /// minted after a ranged record never sees it, and `docs/folds.md` section 11 settles
    /// that the sink's answer is the definition. So a by-key history of such a column would
    /// report a step the stored column never took, which is the wrongness a history read
    /// exists to remove rather than to introduce. [`Column::ranges`] is the same question
    /// asked once, for a host that wants to know before a request arrives.
    ///
    /// The check derives the layout per call, which is deliberate rather than overlooked: a
    /// refusal a caller has to remember to ask for is not a refusal, and the derivation is
    /// noise beside the log scan this then does. A host that wants the answer without the
    /// scan has `Column::ranges` off the layout it already holds for the sink.
    ///
    /// **A computed key and a lookup are allowed**, which is the other half of that line and
    /// not a symmetrical one: those two *agree* between the drivers, which is what
    /// `tests/sink.rs` exists to prove. They are merely quadratic, since the lookup is
    /// folded again at every candidate record, and a cost is the caller's to decide about
    /// where a wrong answer is not.
    pub fn fold_steps(
        &self,
        name: &str,
        args: impl IntoIterator<Item = (impl Into<Ident>, Value)>,
        head: u64,
        visit: OnStep<'_>,
    ) -> Result<Value, Error> {
        let def = self.fold_def(name)?;
        let module = def.module.as_deref();
        let layout = tables::layout(self.program);
        if let Some(ranged) = layout.ranged_through(name) {
            let open = layout
                .column(ranged)
                .map(|(table, column)| column.open(table))
                .unwrap_or_default();
            return Err(Error::new(ErrorKind::NoHistory {
                fold: name.to_string(),
                ranged: ranged.to_string(),
                open: open.join(", "),
            })
            .in_module(module));
        }
        folding(self.program, &self.host, def, args, Some(head), Some(visit))
            .map_err(|err| err.in_module(module))
    }

    fn fold_def(&self, name: &str) -> Result<&'a Fold, Error> {
        self.program
            .fold(name)
            .ok_or_else(|| Error::new(ErrorKind::UnknownFold(name.to_string())))
    }

    /// One named fold, folded. Shredding is the caller's, which is the whole difference
    /// between the two public doors onto this.
    ///
    /// `head` is a log length and not a position, for the reason
    /// [`fold_stored`](Self::fold_stored) takes one: `None` reads to whatever the log has
    /// reached, and `Some(n)` reads the first `n` events.
    fn folding(
        &self,
        def: &Fold,
        args: impl IntoIterator<Item = (impl Into<Ident>, Value)>,
        head: Option<u64>,
    ) -> Result<Value, Error> {
        folding(self.program, &self.host, def, args, head, None)
    }

    /// One attempt: a DCB conflict leaves as [`ErrorKind::Conflict`] for the host to do
    /// something about. See [`run_retrying`](Self::run_retrying) for doing it here.
    pub fn run(
        &mut self,
        name: &str,
        args: impl IntoIterator<Item = (impl Into<Ident>, Value)>,
    ) -> Result<Execution, Error> {
        self.run_retrying(name, args, &mut |_| false)
    }

    /// The same run, with a DCB conflict retried in place.
    ///
    /// `again` is asked after each conflict, with the zero-based number of the attempt
    /// that just had one, and a `true` decides again against the log as it now stands. A
    /// `false`, which is what [`run`](Self::run) always answers, raises the conflict.
    ///
    /// **The policy is still the host's**, which is all `docs/host.md` section 5 ever
    /// meant: how many attempts are worth spending and how long to wait between them are
    /// decisions only a runtime can make, and they arrive through the callback. What
    /// belongs on this side is the *carry*. A retry folds only what landed since its last
    /// attempt, onto the state that attempt already built, and the state lives in a frame
    /// that never leaves this module. A host looping over `run` cannot do that, and would
    /// re-read and re-fold the whole boundary for every event that beat it.
    pub fn run_retrying(
        &mut self,
        name: &str,
        args: impl IntoIterator<Item = (impl Into<Ident>, Value)>,
        again: &mut dyn FnMut(u32) -> bool,
    ) -> Result<Execution, Error> {
        let program = self.program;
        let command = program
            .command(name)
            .ok_or_else(|| ErrorKind::UnknownCommand(name.to_string()))?;
        let module = command.module.as_deref();

        // Every failure below is inside this command, so the module is stamped once here
        // rather than at each raise site.
        execute(self.program, &mut self.host, command, args, again)
            .map_err(|err| err.in_module(module))
    }
}

/// The two halves of a stage: the statements above its declarations and the ones below.
/// Named rather than a bool so the fold sits visibly between them.
#[derive(PartialEq, Eq, Clone, Copy)]
enum Half {
    Pre,
    Post,
}

fn execute(
    program: &Program,
    host: &mut dyn Host,
    command: &Command,
    args: impl IntoIterator<Item = (impl Into<Ident>, Value)>,
    again: &mut dyn FnMut(u32) -> bool,
) -> Result<Execution, Error> {
    // Step 1, which no retry repeats: the arguments and the pinned clock read the
    // request and nothing else. Everything downstream of a fold can move between
    // attempts now, so it lives inside the loop.
    let mut prepared = Frame::new(command.frame);
    // Rule 11: the request's append time, pinned once before anything runs, so it is
    // well defined even for a command that goes on to append nothing, and so a retry
    // decides at the time the request arrived rather than the time it stopped losing.
    if let Some(slot) = command.now {
        let at = host.now();
        prepared.set(slot, Value::Timestamp(at))?;
    }

    let mut args: BTreeMap<Ident, Value> = args
        .into_iter()
        .map(|(name, value)| (name.into(), value))
        .collect();
    bind_params(&command.params, &mut args, &mut prepared)?;

    // What one attempt hands the next: the state the first reading stage folded, the
    // predicates it folded with, and the position it folded through. A fold is a left
    // fold over an append-only log, so folding `[0, a)` and then `[a, b)` is the state
    // that folding `[0, b)` would have given, and a retry never has to start over. On a
    // boundary tens of thousands of events deep that is the difference between paying
    // for the events that beat you and paying for the boundary.
    //
    // Only the *first* reading stage carries, and that is not a simplification to be
    // tidied away later. A fold is a function of its predicates, its range, its seeds
    // and every other slot its seeds and arm expressions read. For the first stage that
    // last part comes from `prepared` alone, so it cannot move between attempts. For a
    // later one it can: `let bump = a` between two folds puts a value the first stage
    // folded into the second stage's arm, and `resolve` never sees it, so no comparison
    // of predicates could tell that the answer went stale. A later stage re-seeds and
    // folds the whole range instead, which is what it would have cost without a carry
    // at all.
    let mut carried: Option<Carry> = None;
    let mut attempt = 0;

    loop {
        let mut frame = prepared.clone();
        // The head every stage of this attempt reads to. Taken lazily, at the first
        // stage that reads, so a command answering from the request alone asks the host
        // nothing; and taken once, so every stage folds the same prefix rather than each
        // seeing a log that moved under the one before it.
        let mut after: Option<u64> = None;
        let mut read: Vec<Predicate> = Vec::new();
        let mut emitted = Vec::new();
        let mut ret = Ret::Ok;
        // Whether the stage about to fold is the first one this attempt reads with, and
        // so the only one whose carry is sound.
        let mut leading = true;
        let mut folded: Option<Carry> = None;

        'stages: for stage in &command.stages {
            {
                let mut sink = Sink::Emit(&mut emitted);
                if let Flow::Return(got) =
                    exec_block(&command.exprs, &stage.pre, &mut frame, program, &mut sink)?
                {
                    ret = got;
                    break 'stages;
                }
            }

            if !stage.folds.is_empty() || stage.reads() {
                // Resolved per stage and per attempt, rather than once for the command,
                // because a filter may now name a `fold` an earlier stage folded.
                let resolved = resolve(program, &command.exprs, &stage.slices, &mut frame)?;
                // A stage whose filters moved since the last attempt folded a different
                // slice, so its carry answers a different question.
                let resume = match (leading, &carried) {
                    (true, Some(carry)) if carry.slices == resolved => Some(carry),
                    _ => None,
                };
                match resume {
                    // On a retry the seed is not where the fold starts from. The state
                    // the last attempt folded is, and the delta below is what it has
                    // not seen.
                    Some(carry) => {
                        for (var, value) in stage.folds.iter().zip(&carry.folds) {
                            frame.set(var.slot, value.clone())?;
                        }
                    }
                    None => {
                        for var in &stage.folds {
                            let value = eval(program, &command.exprs, &mut frame, var.init, None)?;
                            let value = fitted_at(value, &var.ty, command.exprs.span(var.init))?;
                            frame.set(var.slot, value)?;
                        }
                    }
                }
                let from = resume.map_or(0, |carry| carry.through);

                if stage.reads() {
                    // The fold stops just below the pinned head rather than at whatever
                    // the head has become by the time the read gets there. Two reasons,
                    // and the second is the one that bites: a decision made on events the
                    // condition is about to call a conflict is wasted, and a carry whose
                    // upper bound only the store knows cannot be resumed from at all.
                    let at = match after {
                        Some(at) => at,
                        None => {
                            let at = host.head()?;
                            after = Some(at);
                            at
                        }
                    };
                    // Nothing landed since the last attempt read, so there is no delta
                    // to fold. The guard is also what keeps an empty log out of the
                    // `upto: None` case, which means read to the head.
                    if at > from {
                        let folding = Folding {
                            slices: &stage.slices,
                            resolved: &resolved,
                            envelope: &[],
                        };
                        fold(
                            program,
                            &command.exprs,
                            host,
                            &folding,
                            from,
                            at.checked_sub(1),
                            &mut frame,
                            None,
                        )?;
                    }
                    // Rule 13: the live slices only. A slice whose key gate failed folded
                    // nothing, so conflicting on it would make the command lose to events
                    // it never read.
                    read.extend(narrowing(&resolved));

                    if leading {
                        // Taken before the statements below the declarations, which is
                        // what makes the carry safe rather than merely fast: one that
                        // assigns into a `fold` changes what *it* decides on and cannot
                        // reach what the next attempt folds onto.
                        folded = Some(Carry {
                            through: at,
                            slices: resolved,
                            folds: stage
                                .folds
                                .iter()
                                .map(|var| frame.get(var.slot).cloned())
                                .collect::<Result<Vec<_>, _>>()?,
                        });
                    }
                    leading = false;
                }
            }

            {
                let mut sink = Sink::Emit(&mut emitted);
                if let Flow::Return(got) =
                    exec_block(&command.exprs, &stage.post, &mut frame, program, &mut sink)?
                {
                    ret = got;
                    break 'stages;
                }
            }
        }

        // Only replaced when this attempt reached the leading stage's fold. A path that
        // returned above it carries nothing forward, so the next attempt starts over
        // rather than resuming a range it never read.
        if folded.is_some() {
            carried = folded;
        }

        // A command that returned before any stage read depended on nothing, and an
        // empty condition is what says so: `AppendCondition::conflicts` is false for
        // every record, because there is no slice for one to land in.
        let condition = AppendCondition {
            after: after.unwrap_or(0),
            slices: read,
        };

        let outcome = match ret {
            Ret::Ok => Outcome::Ok(emitted),
            Ret::Invalid(message) => Outcome::Invalid(message),
            Ret::Reject { code, message } => Outcome::Reject { code, message },
            // The parser gates `fail` to an effect and a value return to a `fn`, so a
            // command can never carry either.
            Ret::Fail(_) | Ret::Value(_) => return Err(ErrorKind::MalformedIr.into()),
        };

        // Emitted events are already validated at the emit site, where each field still
        // has an expression to point a span at.
        if let Outcome::Ok(events) = &outcome
            && let Err(err) = host.append(events, &condition)
        {
            if matches!(err.kind, ErrorKind::Conflict { .. }) && again(attempt) {
                attempt += 1;
                continue;
            }
            return Err(err);
        }

        return Ok(Execution { outcome, condition });
    }
}

/// What one attempt hands the next for one stage: the state it folded, and the resolved
/// slices it folded with. The slices travel beside the values because a later stage's
/// filter may read an earlier stage's folded state, so unlike the pinned head they are
/// not fixed for the run: a stage whose filters moved folded a different slice and has
/// nothing to carry onto.
///
/// **A [`Resolved`] rather than a [`Predicate`], and that is load-bearing.** A rule 13
/// key gate has no representation in a predicate, so two arms on one event differing only
/// in their gates resolve to *equal* predicates; comparing predicates alone would resume
/// one attempt's delta through the other attempt's arms.
#[derive(Clone)]
struct Carry {
    /// The position the states below are folded through, exclusive. A retry folds
    /// `[through, after)` onto them rather than starting at the seed.
    through: u64,
    slices: Vec<Resolved>,
    folds: Vec<Value>,
}

/// The argument contract: optional coercion, a missing one, an extra one and a wrong
/// type. Takes the parameters rather than the declaration, because a command and a named
/// fold both have a list of them and the contract is the same for either.
fn bind_params(
    params: &[Param],
    args: &mut BTreeMap<Ident, Value>,
    frame: &mut Frame,
) -> Result<(), Error> {
    for param in params {
        let value = match (args.remove(&param.name), &param.ty) {
            (Some(value), Type::Opt(inner)) if value.has_type(inner) => Value::some(value),
            (Some(value), _) => value,
            (None, Type::Opt(inner)) => Value::none(inner.as_ref().clone()),
            (None, _) => return Err(ErrorKind::MissingArgument(param.name.clone()).into()),
        };
        if !value.has_type(&param.ty) {
            return Err(ErrorKind::TypeMismatch {
                expected: param.ty.clone(),
                found: value.ty(),
            }
            .into());
        }
        frame.set(param.slot, value)?;
    }

    match args.keys().next() {
        Some(extra) => Err(ErrorKind::UnexpectedArgument(extra.clone()).into()),
        None => Ok(()),
    }
}

/// One named fold, folded by key over a log. The body of
/// [`Interpreter::fold`](Interpreter::fold)'s two doors, free of the interpreter because
/// a rule 15 lookup calls it again for the fold it reads.
///
/// `head` is a log length and not a position: `None` reads to whatever the log has
/// reached, and `Some(n)` reads the first `n` events.
fn folding(
    program: &Program,
    log: &dyn Log,
    def: &Fold,
    args: impl IntoIterator<Item = (impl Into<Ident>, Value)>,
    head: Option<u64>,
    steps: Option<OnStep<'_>>,
) -> Result<Value, Error> {
    // A fold's body is arms only: `Builder::stmt` is the one path onto either half of a
    // stage and a fold parser never reaches it, so an IR that grew statements here is a
    // bug in the builder rather than something to quietly run.
    debug_assert!(
        def.stage.pre.is_empty() && def.stage.post.is_empty(),
        "a fold's body is arms, so its stage has no statements"
    );

    let mut frame = Frame::new(def.frame);
    let mut args: BTreeMap<Ident, Value> = args
        .into_iter()
        .map(|(name, value)| (name.into(), value))
        .collect();
    bind_params(&def.params, &mut args, &mut frame)?;

    // Resolve before seeding, which is what both other drivers do and is safe for the
    // same reason: a filter cannot read the accumulator (`Builder::filter_past_fold`).
    let resolved = resolve(program, &def.exprs, &def.stage.slices, &mut frame)?;
    for var in &def.stage.folds {
        let value = eval(program, &def.exprs, &mut frame, var.init, None)?;
        let value = fitted_at(value, &var.ty, def.exprs.span(var.init))?;
        frame.set(var.slot, value)?;
    }

    let folding = Folding {
        slices: &def.stage.slices,
        resolved: &resolved,
        envelope: &def.envelope,
    };
    // A head of zero is a log with nothing in it, and there is no inclusive bound that
    // says so: `Some(0)` is position zero. A command's read takes the same two steps
    // (`at > from`, then `at - 1`) for the same reason, since the number it pins itself
    // to is a head too.
    if head != Some(0) {
        fold(
            program,
            &def.exprs,
            log,
            &folding,
            0,
            head.map(|head| head - 1),
            &mut frame,
            // The accumulator's slot is known here and nowhere below, which is why it
            // travels with the callback rather than being looked up in the driver.
            steps.map(|visit| Steps {
                slot: def.slot,
                visit,
            }),
        )?;
    }
    // Always set, because the seed above wrote it.
    Ok(frame.get(def.slot)?.clone())
}

/// Where a rule 15 lookup's value comes from: the two drivers answer it two ways, and
/// both have to give what the fold held when the record being folded was appended.
///
/// **Strictly before the record**, which is what a command appending it would have read.
/// Not the current value, so a replay is deterministic; not including the record, because
/// one that both moves the looked-up fold and is counted through it would otherwise depend
/// on the order the arms happen to be applied in.
enum Asof<'a> {
    /// Fold it again, over the log up to the record's own position. **Quadratic**, which
    /// is what a by-key read of a lookup fold costs and is acknowledged rather than
    /// hidden: a deployment serves these from the table and never takes this path on a
    /// request, and a test is a handful of records.
    Replay { log: &'a dyn Log, upto: u64 },
    /// Read the looked-up column out of the store, which a host maintaining the dependency
    /// in the same pass holds at exactly the record's position: records arrive in order
    /// and nothing for this one has been written yet.
    Stored {
        layout: &'a Layout,
        columns: &'a dyn Columns,
    },
}

impl Asof<'_> {
    fn value(
        &self,
        program: &Program,
        name: &Ident,
        args: &[(Ident, Value)],
    ) -> Result<Value, Error> {
        let def = program
            .fold(name)
            .ok_or_else(|| Error::new(ErrorKind::UnknownFold(name.to_string())))?;
        match self {
            Asof::Replay { log, upto } => {
                folding(program, *log, def, args.iter().cloned(), Some(*upto), None)
            }
            Asof::Stored { layout, columns } => {
                // Always found: the parser refuses a lookup of a fold `layout` keeps no
                // column for, which is what makes this door answerable at all.
                let Some((table, column)) = layout.column(name) else {
                    return Err(ErrorKind::MalformedIr.into());
                };
                let mut key = Vec::with_capacity(column.args.len());
                for param in &column.args {
                    let Some((_, value)) = args.iter().find(|(given, _)| given == param) else {
                        return Err(ErrorKind::MissingArgument(param.clone()).into());
                    };
                    let Some(declared) = def.params.iter().find(|at| &at.name == param) else {
                        return Err(ErrorKind::MalformedIr.into());
                    };
                    let value = fitted_at(value.clone(), &declared.ty, def.span)?;
                    let Some(part) = Key::from_value(&value) else {
                        return Err(ErrorKind::MalformedIr.into());
                    };
                    key.push(part);
                }
                match columns.value(&table.name, &key, &column.name)? {
                    Some(value) => Ok(value),
                    // An absent row is the fold's seed, as it is everywhere else
                    // (`docs/folds.md` section 11). Its parameters are bound first,
                    // because a seed may read one.
                    None => {
                        let mut frame = Frame::new(def.frame);
                        let mut bound: BTreeMap<Ident, Value> = args.iter().cloned().collect();
                        bind_params(&def.params, &mut bound, &mut frame)?;
                        let var = def
                            .stage
                            .folds
                            .iter()
                            .find(|var| var.slot == def.slot)
                            .ok_or(ErrorKind::MalformedIr)?;
                        let value = eval(program, &def.exprs, &mut frame, var.init, None)?;
                        fitted_at(value, &var.ty, def.exprs.span(var.init))
                    }
                }
            }
        }
    }
}

/// Rules 15 and 16: every fold this slice reads, from a `key` clause or from the arm's own
/// value, into the slots they read it from.
///
/// In the list's own order, which is evaluation order: an inner link of a chain was pushed
/// before the outer one that takes it as an argument, so by the time the outer one's
/// arguments are evaluated the inner slot is filled.
fn lookups(
    program: &Program,
    exprs: &Exprs,
    slice: &Slice,
    frame: &mut Frame,
    asof: &Asof<'_>,
) -> Result<(), Error> {
    for lookup in &slice.lookups {
        let mut args: Vec<(Ident, Value)> = Vec::with_capacity(lookup.args.len());
        for (name, value) in &lookup.args {
            args.push((name.clone(), eval(program, exprs, frame, *value, None)?));
        }
        let value = asof.value(program, &lookup.fold, &args)?;
        let value = fitted_at(value, &lookup.ty, lookup.span)?;
        frame.set(lookup.slot, value)?;
    }
    Ok(())
}

/// Every slice with its filters evaluated. This runs before the fold, so what a run
/// read is known whatever the body goes on to decide, which is why the condition comes
/// back with a refusal too.
///
/// **Each value is fitted to the field's declared type**, which is what optional coercion
/// means at this position. `on @hold.release.failed(fulfillment_order_id)` against a
/// field declared `Int?` compares a bare `Int` with the optional the event holds, and
/// without the fit the filter matches nothing at all: the parser allows the bare value
/// (`docs/optionals.md`), so the slice reads as written and silently folds no events. The
/// fit has to be here rather than in [`matches`] because the predicate is also what the
/// host narrows with, and a store comparing a column to an unwrapped value has the same
/// problem one layer down.
fn resolve(
    program: &Program,
    exprs: &Exprs,
    slices: &[Slice],
    frame: &mut Frame,
) -> Result<Vec<Resolved>, Error> {
    slices
        .iter()
        .map(|slice| resolved_one(program, exprs, slice, frame))
        .collect()
}

/// One slice, resolved against one frame.
///
/// Split out of [`resolve`] for [`Tables::apply`], which resolves a slice at a time
/// because each column, arm and key it writes has a frame of its own: two arms on one
/// event differing only in their rule 13 gates bind the same parameter to two different
/// values, so one frame cannot answer for both.
fn resolved_one(
    program: &Program,
    exprs: &Exprs,
    slice: &Slice,
    frame: &mut Frame,
) -> Result<Resolved, Error> {
    let def = program.event(&slice.event);
    let mut filters = Vec::new();
    let mut live = true;
    for filter in &slice.filters {
        let value = eval(program, exprs, frame, filter.value(), None)?;
        match filter {
            Filter::Field { field, .. } => {
                let value = match def.and_then(|def| def.field(field)) {
                    Some(declared) => fitted_at(value, &declared.ty, exprs.span(filter.value()))?,
                    None => value,
                };
                filters.push((field.clone(), value));
            }
            // Rule 13: the key this arm writes, which is not a predicate on the
            // event and so never joins `filters`. Fitted to the parameter's
            // declared type for the reason a field's value is, so a bare value in
            // an optional parameter compares as the optional.
            Filter::Key { slot, ty, .. } => {
                let value = fitted_at(value, ty, exprs.span(filter.value()))?;
                live = live && frame.get(*slot)? == &value;
            }
        }
    }
    Ok(Resolved {
        predicate: Predicate::new(slice.event.clone(), filters),
        live,
    })
}

/// One slice, resolved: the predicate a host narrows with, and whether this invocation
/// reads the slice at all.
///
/// The second half is rule 13's. A `Filter::Key` conjunct is resolved once against the
/// argument the caller passed, so a slice whose conjunct fails participates in **nothing**:
/// it folds no events and joins no append condition. Dropping it rather than widening is
/// available precisely because the answer cannot move between attempts of one run, and it
/// matters: a command reading one kind of a nine-kind queue would otherwise conflict on
/// all nine.
///
/// **`live` is why this type exists rather than a bare `Predicate`.** A key conjunct has
/// no representation in a predicate, so two arms on one event differing only in their key
/// gates resolve to *equal* predicates. A carry comparing predicates alone would resume
/// one attempt's delta through the other's arms.
///
/// The predicate is built for a dead slice too. Its sibling field filters are evaluated
/// either way, so a type error in one raises whether or not the key gate held, which is
/// the determinism a resolved value should have.
#[derive(Clone, PartialEq, Eq)]
struct Resolved {
    predicate: Predicate,
    live: bool,
}

/// The three parallel lists one stage folds with, in one place so the invariant has one
/// home: a slice, the predicate it resolved to, and the envelope binds its arms wrote.
struct Folding<'a> {
    slices: &'a [Slice],
    resolved: &'a [Resolved],
    envelope: &'a [EnvBind],
}

impl Folding<'_> {
    /// The predicates a host is asked to narrow with, and the ones a command conflicts
    /// on: the live slices only.
    fn narrowing(&self) -> Vec<Predicate> {
        narrowing(self.resolved)
    }
}

fn narrowing(resolved: &[Resolved]) -> Vec<Predicate> {
    resolved
        .iter()
        .filter(|slice| slice.live)
        .map(|slice| slice.predicate.clone())
        .collect()
}

/// What [`Interpreter::fold_steps`] is called back with: one record the fold folded and the
/// accumulator as of it, in log order.
///
/// Named rather than written out at each of the three places it appears, which is also what
/// gives it somewhere to say that an `Err` from it stops the fold: a visitor that cannot take
/// the next step is the caller going away, not a fold that failed.
pub type OnStep<'a> = &'a mut dyn FnMut(&Record, &Value) -> Result<(), Error>;

/// What a history read adds to the by-key driver: the accumulator reported after each
/// record, which is the one thing a fold otherwise never says out loud.
///
/// The slot travels with the callback because the driver does not otherwise know which of
/// the frame's slots is the answer: it folds whatever the stage says and `folding` is where
/// [`Fold::slot`] is in hand.
struct Steps<'a> {
    slot: Slot,
    visit: OnStep<'a>,
}

/// The host narrows, and this re-checks, because a store that can only narrow
/// approximately is still correct.
///
/// **The query is built here, from the same resolved list the matching reads.** It used
/// to be built by each of the three callers and passed in, which made the parallelism
/// between it and the slices an invariant three places had to keep; now there is one
/// construction and nothing to assert. `from` and `upto` are the one thing a caller still
/// decides, because the range is the one latitude `docs/host.md` section 4 does not give.
#[allow(clippy::too_many_arguments)]
fn fold(
    program: &Program,
    exprs: &Exprs,
    log: &dyn Log,
    stage: &Folding<'_>,
    from: u64,
    upto: Option<u64>,
    frame: &mut Frame,
    mut steps: Option<Steps<'_>>,
) -> Result<(), Error> {
    // `zip` below truncates, so a resolved list shorter than the slices would match a
    // stage against another stage's predicates and bind the wrong events into its slots.
    // Silent, and only visible once a command has more than one stage.
    debug_assert_eq!(
        stage.resolved.len(),
        stage.slices.len(),
        "a fold's slices and their resolved predicates are one per slice, in order"
    );
    let query = Query {
        slices: stage.narrowing(),
        from,
        upto,
    };
    // Rule 13: every slice's key gate failed, so this invocation reads nothing. Returning
    // here rather than asking with an empty `slices` matters, because an empty list reads
    // as "narrow by nothing" to a host and would deliver the whole range.
    if query.slices.is_empty() {
        return Ok(());
    }
    // **A fold with a rule 15 lookup reads its records out first**, because folding one
    // means reading the log again, once per record, for the fold it keys through.
    //
    // Buffered rather than recursed into from inside the visitor, which is what keeps the
    // re-entrancy out of `Log::read`: the seam takes `&self` and says nothing about being
    // called again from its own visitor, so a host holding a cursor, a connection or a
    // `RefCell` borrow across the visit would deadlock rather than merely be slow. The
    // cost lands only on the folds that need it, which are the quadratic ones already.
    let buffered: Option<Vec<Record>> = match stage.slices.iter().any(|at| !at.lookups.is_empty()) {
        false => None,
        true => {
            let mut found = Vec::new();
            log.read(&query, &mut |record| {
                found.push(record.clone());
                Ok(())
            })?;
            Some(found)
        }
    };

    let mut visit = |record: &Record| -> Result<(), Error> {
        let event = &record.event;
        // Before the slices, because an arm's value reads these and a slice's updates
        // are that value. One list for the whole fold, so a record that matches no slice
        // writes them harmlessly.
        for bind in stage.envelope {
            frame.set(bind.slot, envelope_value(record, bind.field))?;
        }
        for (slice, resolved) in stage.slices.iter().zip(stage.resolved) {
            if !resolved.live || !matches(&resolved.predicate, event)? {
                continue;
            }

            for bind in &slice.binds {
                let value = field(event, &bind.field)?.clone();
                let value = seal(program, event, &bind.field, value)?;
                frame.set(bind.slot, value)?;
            }
            // Rule 15: the folds this arm's `key` clauses read, as of this record, which
            // here means folded again over the log before it. After the binds, because
            // their arguments read them, and before the computed parts, which read the
            // slots they land in.
            if !slice.lookups.is_empty() {
                let asof = Asof::Replay {
                    log,
                    upto: record.position,
                };
                lookups(program, exprs, slice, frame, &asof)?;
            }
            // **A computed key part, compared rather than written.** This is a by-key
            // read: the caller's argument is already in the slot, so a record whose
            // computed part is some other key belongs to some other row and is skipped.
            // `Tables::apply` takes the same expression the other way round, writing the
            // row at whatever it evaluates to, which is why one of the two has to scan.
            //
            // After the binds, because the expression reads them, and before the updates,
            // because a record that is not this key's must not reach the accumulator.
            let mut mine = true;
            for key in &slice.keys {
                let value = eval(program, exprs, frame, key.value, None)?;
                // Rule 19: `else skip` and absent, so this record belongs under no key at
                // all and is therefore not this one's. The sink writes nothing for it,
                // which is the same answer: a record skipped by every key and a record
                // written at none are one record.
                let value = match (key.skip, value) {
                    (true, Value::Opt { value: None, .. }) => {
                        mine = false;
                        break;
                    }
                    (
                        true,
                        Value::Opt {
                            value: Some(held), ..
                        },
                    ) => *held,
                    (_, value) => value,
                };
                let value = fitted_at(value, &key.ty, exprs.span(key.value))?;
                if frame.get(key.slot)? != &value {
                    mine = false;
                    break;
                }
            }
            if !mine {
                continue;
            }
            for update in &slice.updates {
                let value = eval(program, exprs, frame, update.value, None)?;
                let value = fitted_at(value, &update.ty, exprs.span(update.value))?;
                frame.set(update.slot, value)?;
            }
        }
        // **After every slice, and for every record the query delivered**, including one
        // that matched no arm or was skipped by a computed key: those are the records whose
        // step did not move the value, and a reader diffing consecutive values is what
        // decides they are not changes. Reporting them is cheaper than deciding here, and
        // "the value after this record" is a fact about every record either way.
        if let Some(steps) = steps.as_mut() {
            let held = frame.get(steps.slot)?.clone();
            (steps.visit)(record, &held)?;
        }
        Ok(())
    };

    match &buffered {
        Some(records) => records.iter().try_for_each(&mut visit),
        None => log.read(&query, &mut visit),
    }
}

fn matches(predicate: &Predicate, event: &Event) -> Result<bool, Error> {
    if predicate.event != event.path {
        return Ok(false);
    }
    for (name, expected) in &predicate.filters {
        if field(event, name)? != expected {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Rule 15's key: the values an arm's `@key` fields hold in one event, in the order the
/// arm wrote them.
///
/// **The one place a partition key is computed.** `drive` collapses an `on latest` arm with
/// it and a host assigns lanes with it, so the two cannot come to disagree about a
/// composite or about the order its parts were written in. The checker has already made
/// every name a field of every listed event type, and every type one that orders and
/// hashes, which is why this is a read rather than a decision.
///
/// **Two things are built from this and they are not the same thing.** A runtime's *lane*,
/// the unit of mutual exclusion, is the key alone: `docs/effects.md` rule 15 wants two arms
/// touching one remote resource under one shop id to share a lane, which is the whole
/// reason arms keying by different identities are worth a warning. Rule 15's *collapse
/// grouping* is narrower, the key together with the arm that produced it
/// ([`Effect::arm_index`]), because two arms have two bodies and folding one into the other
/// would drop work rather than repeat it. `drive` builds the second. A host assigning lanes
/// wants the first.
pub fn partition_key(arm: &Arm, event: &Event) -> Result<Vec<Key>, Error> {
    arm.keys
        .iter()
        .map(|name| {
            let value = field(event, name)?;
            Key::from_value(value).ok_or_else(|| {
                Error::new(ErrorKind::BadLane {
                    field: name.clone(),
                    ty: value.ty(),
                })
            })
        })
        .collect()
}

/// Whether a key carries every part a ranged arm pinned, which is what
/// [`Columns::keys`] is asked and what the minted set has to be filtered by before it is
/// unioned in.
///
/// `pinned` is a position in the table's key order and the value the arm fixed it to, so a
/// key agrees when each named position holds that value; the positions it does not name
/// are the range.
fn covers(pinned: &[(usize, Key)], key: &[Key]) -> bool {
    pinned.iter().all(|(at, part)| key.get(*at) == Some(part))
}

fn field<'a>(event: &'a Event, name: &str) -> Result<&'a Value, Error> {
    event.field(name).ok_or_else(|| {
        Error::new(ErrorKind::MissingField {
            event: event.path.clone(),
            field: name.to_string(),
        })
    })
}

/// Where a statement's writes go. One `exec_block` serves both declaration kinds;
/// the parser is what guarantees a command never reaches `Write` and a handler
/// never reaches `Emit`.
enum Sink<'a> {
    /// A `fn` body, which writes nowhere. Purity is a parse-time rule, so nothing here
    /// has to enforce it; this is what there is nothing to write through.
    Pure,
    Emit(&'a mut Vec<Event>),
    Write {
        projector: &'a Projector,
        rows: &'a mut dyn Rows,
    },
    Effect(Effects<'a>),
}

fn effects<'s, 'a>(sink: &'s mut Sink<'a>) -> Option<&'s mut Effects<'a>> {
    match sink {
        Sink::Effect(ctx) => Some(ctx),
        _ => None,
    }
}

fn exec_block(
    exprs: &Exprs,
    stmts: &[Stmt],
    frame: &mut Frame,
    program: &Program,
    sink: &mut Sink<'_>,
) -> Result<Flow, Error> {
    for stmt in stmts {
        let flow = exec_stmt(exprs, stmt, frame, program, sink)?;
        if matches!(flow, Flow::Return(_)) {
            return Ok(flow);
        }
    }
    Ok(Flow::Next)
}

fn exec_stmt(
    exprs: &Exprs,
    stmt: &Stmt,
    frame: &mut Frame,
    program: &Program,
    sink: &mut Sink<'_>,
) -> Result<Flow, Error> {
    match stmt {
        Stmt::Assign { slot, value } => {
            let value = eval(program, exprs, frame, *value, effects(sink))?;
            frame.set(*slot, value)?;
            Ok(Flow::Next)
        }
        Stmt::If {
            cond,
            then,
            otherwise,
        } => {
            let branch = if eval_bool(program, exprs, frame, *cond, effects(sink))? {
                then
            } else {
                otherwise
            };
            exec_block(exprs, branch, frame, program, sink)
        }
        Stmt::For { iter, body } => {
            for (index, item) in elements(program, exprs, frame, iter, effects(sink))? {
                bind_iter(iter, index, item, frame)?;
                let flow = exec_block(exprs, body, frame, program, sink)?;
                // A `return` inside a `for` leaves the loop and the body both, which
                // is what makes "a search is a pure fn with an early return" work.
                if matches!(flow, Flow::Return(_)) {
                    return Ok(flow);
                }
            }
            Ok(Flow::Next)
        }
        Stmt::Emit {
            event,
            fields,
            span,
        } => {
            let Sink::Emit(emitted) = sink else {
                return Err(Error::at(ErrorKind::MalformedIr, *span));
            };
            let def = program
                .event(event)
                .ok_or_else(|| Error::at(ErrorKind::UnknownEvent(event.clone()), *span))?;

            let mut values = BTreeMap::new();
            for (name, value) in fields {
                let at = exprs.span(*value);
                let Some(declared) = def.field(name) else {
                    return Err(Error::at(
                        ErrorKind::UnknownField {
                            event: event.clone(),
                            field: name.clone(),
                        },
                        at,
                    ));
                };
                let value = coerce(eval(program, exprs, frame, *value, None)?, &declared.ty);
                // An over-length value is the runtime's validation channel, so it
                // leaves as `Outcome::Invalid` rather than as an error.
                if let Some(fault) =
                    check_field(program, &declared.ty, declared.max_len, name, &value, at)?
                {
                    return Ok(Flow::Return(Ret::Invalid(fault.to_string())));
                }
                values.insert(name.clone(), value);
            }

            for declared in &def.fields {
                if !values.contains_key(&declared.name) {
                    return Err(Error::at(
                        ErrorKind::MissingField {
                            event: event.clone(),
                            field: declared.name.clone(),
                        },
                        *span,
                    ));
                }
            }

            emitted.push(Event {
                path: event.clone(),
                fields: values,
            });
            Ok(Flow::Next)
        }
        Stmt::Put {
            entity,
            fields,
            span,
        } => {
            let (projector, rows) = write_sink(sink, *span)?;
            let def = entity_def(projector, entity, *span)?;

            let mut row = Row::default();
            for (name, value) in fields {
                let value = eval_field(program, exprs, frame, def, entity, name, *value)?;
                row.0.insert(name.clone(), value);
            }
            for declared in &def.fields {
                if !row.0.contains_key(&declared.name) {
                    return Err(Error::at(
                        ErrorKind::MissingEntityField {
                            entity: entity.clone(),
                            field: declared.name.clone(),
                        },
                        *span,
                    ));
                }
            }

            let key = row_key(def, &row, *span)?;
            rows.put(entity, key, row)
                .map_err(|err| err.located(*span))?;
            Ok(Flow::Next)
        }
        Stmt::Patch {
            entity,
            keys,
            absent,
            loads,
            fields,
            span,
        } => {
            let key = subscript(program, exprs, frame, keys)?;
            let (projector, rows) = write_sink(sink, *span)?;
            let def = entity_def(projector, entity, *span)?;

            // Rule 5: a `patch` materializes from zeros, so it always has a prior value
            // for `.field` to read. An `update` drops the write instead, which is why a
            // stored load below can only ever come from a row that really exists.
            let stored = rows.row(entity, &key).map_err(|err| err.located(*span))?;
            let mut row = match (stored, absent) {
                (Some(row), _) => row,
                (None, Absent::Materialize) => materialize(def, projector, program, &key, *span)?,
                (None, Absent::Skip) => return Ok(Flow::Next),
            };
            for load in loads {
                let value = row.0.get(&load.field).cloned().ok_or_else(|| {
                    Error::at(
                        ErrorKind::UnknownEntityField {
                            entity: entity.clone(),
                            field: load.field.clone(),
                        },
                        *span,
                    )
                })?;
                frame.set(load.slot, value)?;
            }

            for (name, value) in fields {
                let value = eval_field(program, exprs, frame, def, entity, name, *value)?;
                row.0.insert(name.clone(), value);
            }

            rows.put(entity, key, row)
                .map_err(|err| err.located(*span))?;
            Ok(Flow::Next)
        }
        Stmt::Delete { entity, keys } => {
            // The first key's span, or nothing to point at. A valid program always has one
            // (`subscript` checks the arity against a key that is never empty), so an
            // empty list is malformed IR rather than a program error, and indexing it is
            // the one place the compound-key change could have panicked on one.
            let span = match keys.first() {
                Some(first) => exprs.span(*first),
                None => return Err(ErrorKind::MalformedIr.into()),
            };
            let key = subscript(program, exprs, frame, keys)?;
            let (projector, rows) = write_sink(sink, span)?;
            entity_def(projector, entity, span)?;
            rows.delete(entity, &key).map_err(|err| err.located(span))?;
            Ok(Flow::Next)
        }
        // Rule 4: `fail` is the author's terminal outcome, and only an effect has one.
        Stmt::Fail { message, span } => {
            if !matches!(sink, Sink::Effect(_)) {
                return Err(Error::at(ErrorKind::MalformedIr, *span));
            }
            let message = eval_string(program, exprs, frame, *message, effects(sink))?;
            Ok(Flow::Return(Ret::Fail(message)))
        }
        // Rule 10: not journaled, so a replay adds this line again.
        Stmt::Log { message } => {
            let message = eval_string(program, exprs, frame, *message, effects(sink))?;
            match sink {
                Sink::Effect(ctx) => {
                    ctx.lines.push(message.clone());
                    ctx.trace.push(Effectful::Log(message));
                }
                _ => return Err(ErrorKind::MalformedIr.into()),
            }
            Ok(Flow::Next)
        }
        Stmt::Erase {
            subject,
            value,
            span,
        } => {
            let value = eval(program, exprs, frame, *value, effects(sink))?;
            let Some(id) = subject_id(&value, *span)? else {
                return Err(Error::at(ErrorKind::BadSubject(value.ty()), *span));
            };
            match sink {
                Sink::Effect(ctx) => ctx.erase(subject, &id)?,
                _ => return Err(Error::at(ErrorKind::MalformedIr, *span)),
            }
            Ok(Flow::Next)
        }
        Stmt::Discard(value) => {
            eval(program, exprs, frame, *value, effects(sink))?;
            Ok(Flow::Next)
        }
        Stmt::Call {
            function,
            scope,
            args,
            span,
        } => {
            let mut values = Vec::new();
            for arg in args {
                values.push(eval(program, exprs, frame, *arg, effects(sink))?);
            }
            let def = program
                .function_in(scope.as_deref(), function)
                .ok_or_else(|| Error::at(ErrorKind::UnknownFunction(function.clone()), *span))?;
            call_void(program, def, values, *span, effects(sink))?;
            Ok(Flow::Next)
        }
        Stmt::Return(ret) => {
            let ret = match ret {
                Return::Ok => Ret::Ok,
                Return::Invalid(message) => {
                    Ret::Invalid(eval_string(program, exprs, frame, *message, None)?)
                }
                Return::Reject { code, message } => Ret::Reject {
                    code: eval_string(program, exprs, frame, *code, None)?,
                    message: eval_string(program, exprs, frame, *message, None)?,
                },
                Return::Value(value) => {
                    Ret::Value(eval(program, exprs, frame, *value, effects(sink))?)
                }
                // The decision was computed rather than spelled. Unwrapping an optional
                // here would be a second rule: the parser only builds this for an
                // expression whose type is `Outcome`, so a `none` never reaches it.
                Return::Outcome(value) => {
                    match eval(program, exprs, frame, *value, effects(sink))? {
                        Value::Invoked(Invoked::Ok) => Ret::Ok,
                        Value::Invoked(Invoked::Invalid(message)) => Ret::Invalid(message),
                        Value::Invoked(Invoked::Reject { code, message }) => {
                            Ret::Reject { code, message }
                        }
                        other => {
                            return Err(Error::at(
                                ErrorKind::TypeMismatch {
                                    expected: Type::Outcome,
                                    found: other.ty(),
                                },
                                exprs.span(*value),
                            ));
                        }
                    }
                }
            };
            Ok(Flow::Return(ret))
        }
    }
}

/// A subject is identified by a plaintext scalar, which is what `erase` and `reveal`
/// look the key up by. Absent only for a fold's companion, which holds nothing until
/// the fold matches something; rule 12 turns on telling that apart from a real id.
pub(crate) fn subject_id(value: &Value, span: Span) -> Result<Option<String>, Error> {
    match value {
        Value::Int(id) => Ok(Some(id.to_string())),
        Value::Str(id) | Value::Uuid(id) => Ok(Some(id.to_string())),
        Value::Opt {
            value: Some(id), ..
        } => subject_id(id, span),
        Value::Opt { value: None, .. } => Ok(None),
        other => Err(Error::at(ErrorKind::BadSubject(other.ty()), span)),
    }
}

fn write_sink<'s, 'a>(
    sink: &'s mut Sink<'a>,
    span: Span,
) -> Result<(&'a Projector, &'s mut (dyn Rows + 'a)), Error> {
    match sink {
        Sink::Write { projector, rows } => Ok((projector, &mut **rows)),
        Sink::Emit(_) | Sink::Effect(_) | Sink::Pure => {
            Err(Error::at(ErrorKind::MalformedIr, span))
        }
    }
}

fn entity_def<'a>(
    projector: &'a Projector,
    name: &Ident,
    span: Span,
) -> Result<&'a EntityDef, Error> {
    projector
        .entity(name)
        .ok_or_else(|| Error::at(ErrorKind::UnknownEntity(name.clone()), span))
}

/// Evaluates one `put` or `patch` field value and checks it against the declared
/// field. An over-length value is a hard error here: rule 2 gives a projector no
/// outcome an author could catch it with.
fn eval_field(
    program: &Program,
    exprs: &Exprs,
    frame: &mut Frame,
    def: &EntityDef,
    entity: &Ident,
    name: &Ident,
    value: ExprId,
) -> Result<Value, Error> {
    let at = exprs.span(value);
    let Some(declared) = def.field(name) else {
        return Err(Error::at(
            ErrorKind::UnknownEntityField {
                entity: entity.clone(),
                field: name.clone(),
            },
            at,
        ));
    };
    let value = coerce(eval(program, exprs, frame, value, None)?, &declared.ty);
    if let Some(fault) = check_field(program, &declared.ty, declared.max_len, name, &value, at)? {
        return Err(Error::at(fault, at));
    }
    Ok(value)
}

/// Type check, then length check. A type mismatch is always an error; the caller
/// decides what an over-length value means, which is the one place commands and
/// projectors differ.
/// A bare `T` written into a `T?` field wraps, the same coercion `bind_params`
/// already applies to command arguments. `docs/optionals.md` lists every position
/// this holds at, and public so a test's expected value is held to the same rule
/// rather than to a second copy of it.
pub fn coerce(value: Value, ty: &Type) -> Value {
    match ty {
        // Built by hand rather than through `Value::some`, so the declared element type
        // survives rather than being rebuilt from the value. A seal cannot say what it
        // holds, so `Value::some` around one would answer `String` and lose what the
        // declaration knew. `read_json` is built this way for the same reason.
        Type::Opt(inner) if value.has_type(inner) => Value::Opt {
            inner: inner.as_ref().clone(),
            value: Some(Box::new(value)),
        },
        // Writing a plain value into a sealed position is the encrypting direction and
        // needs no ceremony; reading content back out is what `reveal` is for.
        Type::Sealed(inner, _) => coerce(value, inner),
        _ => value,
    }
}

/// A value meeting a declared type at a write. `coerce` alone was silent about a
/// mismatch, so the wrong shape sat in the slot until some later read reported that a
/// `String` has no `is_none`, naming a symptom several statements from its cause.
/// `docs/types.md`'s check catches these before the program runs wherever it can name
/// the type; this is the backstop for wherever it cannot.
fn fitted(value: Value, ty: &Type) -> Result<Value, ErrorKind> {
    let value = coerce(value, ty);
    if value.has_type(ty) {
        return Ok(value);
    }
    Err(ErrorKind::TypeMismatch {
        expected: ty.clone(),
        found: value.ty(),
    })
}

pub(crate) fn fitted_at(value: Value, ty: &Type, span: Span) -> Result<Value, Error> {
    fitted(value, ty).map_err(|kind| Error::at(kind, span))
}

pub(crate) fn check_field(
    program: &Program,
    ty: &Type,
    max_len: Option<usize>,
    name: &Ident,
    value: &Value,
    span: Span,
) -> Result<Option<ErrorKind>, Error> {
    if !value.has_type(ty) {
        return Err(Error::at(
            ErrorKind::TypeMismatch {
                expected: ty.clone(),
                found: value.ty(),
            },
            span,
        ));
    }
    Ok(bounded(program, name, max_len, value))
}

/// A value against the `@max` its field declared, and then against every `@max`
/// declared inside it. A record carries its own bounds, so they travel with the value
/// rather than with the field it landed in; see `docs/declarations.md`.
///
/// An optional is peeled before the length test. `String? @max(200)` bounds the string
/// it holds, and that it did not was a hole a record made visible rather than one a
/// record introduced.
fn bounded(
    program: &Program,
    at: &str,
    max_len: Option<usize>,
    value: &Value,
) -> Option<ErrorKind> {
    let value = match value {
        Value::Opt {
            value: Some(held), ..
        } => held.as_ref(),
        Value::Opt { value: None, .. } => return None,
        other => other,
    };
    if let (Some(max), Value::Str(text)) = (max_len, value) {
        let len = text.chars().count();
        if len > max {
            return Some(ErrorKind::TooLong {
                field: at.to_string(),
                len,
                max,
            });
        }
    }
    // The path, not the field, so a `List(LineItem)` says which element is too long.
    match value {
        Value::Record { ty, fields } => {
            let def = program.record(ty)?;
            fields.iter().find_map(|(name, held)| {
                let max_len = def.field(name).and_then(|field| field.max_len);
                bounded(program, &format!("{at}.{name}"), max_len, held)
            })
        }
        Value::List { items, .. } => items
            .iter()
            .enumerate()
            .find_map(|(index, item)| bounded(program, &format!("{at}[{index}]"), None, item)),
        Value::Map { entries, .. } => entries.iter().find_map(|(key, held)| {
            let key = value::text(&key_value(key));
            bounded(program, &format!("{at}[{key}]"), None, held)
        }),
        _ => None,
    }
}

fn key_of(value: &Value, span: Span) -> Result<Key, Error> {
    Key::from_value(value).ok_or_else(|| Error::at(ErrorKind::BadKey(value.ty()), span))
}

/// `Entity[a, b]`: one key per column, in key order. The parser has already checked the
/// arity against the declaration, so this does not re-check it; what it cannot check is
/// that each value is one a key can be made of, which is what `key_of` answers.
fn subscript(
    program: &Program,
    exprs: &Exprs,
    frame: &mut Frame,
    keys: &[ExprId],
) -> Result<Vec<Key>, Error> {
    keys.iter()
        .map(|&id| {
            let value = eval(program, exprs, frame, id, None)?;
            key_of(&value, exprs.span(id))
        })
        .collect()
}

/// A `put`'s key, read off the row it writes: the key columns are columns like any
/// other there, which is why `put` takes no subscript.
fn row_key(def: &EntityDef, row: &Row, span: Span) -> Result<Vec<Key>, Error> {
    def.key_fields()
        .map(|field| {
            let value = row.0.get(&field.name).ok_or_else(|| {
                Error::at(
                    ErrorKind::MissingEntityField {
                        entity: def.name.clone(),
                        field: field.name.clone(),
                    },
                    span,
                )
            })?;
            key_of(value, span)
        })
        .collect()
}

fn materialize(
    def: &EntityDef,
    projector: &Projector,
    program: &Program,
    key: &[Key],
    span: Span,
) -> Result<Row, Error> {
    let defs = value::Defs {
        local: &projector.enums,
        enums: &program.enums,
        records: &program.records,
    };
    let mut row = Row::default();
    for (position, field) in def.fields.iter().enumerate() {
        // A key column's zero is never read, because the subscript holds its value.
        let value = if let Some(at) = def.keys.iter().position(|&key| key == position) {
            key_value(&key[at])
        } else {
            value::initial(field, defs).ok_or_else(|| {
                Error::at(
                    ErrorKind::MissingEntityField {
                        entity: def.name.clone(),
                        field: field.name.clone(),
                    },
                    span,
                )
            })?
        };
        row.0.insert(field.name.clone(), value);
    }
    Ok(row)
}

fn key_value(key: &Key) -> Value {
    match key {
        Key::Int(value) => Value::Int(*value),
        Key::Str(value) => Value::Str(value.clone()),
        Key::Uuid(value) => Value::Uuid(value.clone()),
        Key::Timestamp(micros) => Value::Timestamp(*micros),
        Key::Enum { ty, variant } => Value::Enum {
            ty: ty.clone(),
            variant: variant.clone(),
        },
    }
}

fn eval(
    program: &Program,
    exprs: &Exprs,
    frame: &mut Frame,
    id: ExprId,
    mut ctx: Option<&mut Effects<'_>>,
) -> Result<Value, Error> {
    let span = exprs.span(id);
    let at = |kind: ErrorKind| Error::at(kind, span);

    match exprs.get(id).ok_or_else(|| at(ErrorKind::MalformedIr))? {
        Expr::Lit(lit) => Ok(value::literal(lit)),
        Expr::Load(slot) => frame.get(*slot).cloned().map_err(at),
        // Rule 16. The `ctx` is what makes the restriction structural as well as
        // checked: a command, a projector and a fold all evaluate with `None`, so there
        // is no host to ask and no credential to be had. The redaction is built here,
        // once, and every surface downstream reads it rather than remembering a rule.
        Expr::Secret { name, optional } => {
            let Some(ctx) = ctx else {
                return Err(at(ErrorKind::MalformedIr));
            };
            match ctx.secret(name) {
                Some(plain) => {
                    let held = Value::Secret {
                        redacted: Arc::from(format!("{{SECRET:{name}}}")),
                        plain,
                    };
                    Ok(match optional {
                        true => Value::some(held),
                        false => held,
                    })
                }
                // A required one is the deployment's to settle before the process
                // starts, so this wedges rather than skipping: an operator setting it
                // is what makes the retry succeed.
                None if !*optional => Err(at(ErrorKind::MissingSecret(name.clone()))),
                None => Ok(Value::none(Type::Secret)),
            }
        }
        Expr::Unary { op, operand } => {
            let value = eval(program, exprs, frame, *operand, ctx)?;
            unary(*op, value).map_err(at)
        }
        Expr::Binary { op, lhs, rhs } => match op {
            BinOp::And => {
                if eval_bool(program, exprs, frame, *lhs, ctx.as_deref_mut())? {
                    Ok(Value::Bool(eval_bool(program, exprs, frame, *rhs, ctx)?))
                } else {
                    Ok(Value::Bool(false))
                }
            }
            BinOp::Or => {
                if eval_bool(program, exprs, frame, *lhs, ctx.as_deref_mut())? {
                    Ok(Value::Bool(true))
                } else {
                    Ok(Value::Bool(eval_bool(program, exprs, frame, *rhs, ctx)?))
                }
            }
            op => {
                let lhs = eval(program, exprs, frame, *lhs, ctx.as_deref_mut())?;
                let rhs = eval(program, exprs, frame, *rhs, ctx)?;
                binary(*op, lhs, rhs).map_err(at)
            }
        },
        Expr::Method {
            receiver,
            method,
            args,
        } => {
            let receiver = eval(program, exprs, frame, *receiver, ctx.as_deref_mut())?;
            let mut values = Vec::new();
            for arg in args {
                values.push(eval(program, exprs, frame, *arg, ctx.as_deref_mut())?);
            }
            call_method(receiver, method, values).map_err(at)
        }
        Expr::If {
            cond,
            then,
            otherwise,
        } => {
            let taken = eval_bool(program, exprs, frame, *cond, ctx.as_deref_mut())?;
            eval(
                program,
                exprs,
                frame,
                if taken { *then } else { *otherwise },
                ctx,
            )
        }
        Expr::Field { receiver, name } => {
            let value = eval(program, exprs, frame, *receiver, ctx)?;
            match (&value, name.as_str()) {
                (Value::Response { status, .. }, "status") => Ok(Value::Int(*status)),
                (Value::Response { body, .. }, "body") => Ok(Value::Json(body.clone())),
                (Value::Record { fields, .. }, field) if fields.contains_key(field) => {
                    Ok(fields[field].clone())
                }
                _ => Err(at(ErrorKind::NoSuchField {
                    ty: value.ty(),
                    field: name.clone(),
                })),
            }
        }
        Expr::Object(fields) => {
            // Rule 8's table. Sorted keys, so the same object built twice serialises
            // the same, which is one cause removed from verify's list (rule 14).
            //
            // Where an object literal may be written is the parser's `in_body`, not a
            // sink check here: `docs/testing.md` rule 7 writes an expected body with no
            // effect running behind it, and that is a body position too.
            let mut object = BTreeMap::new();
            for (name, value) in fields {
                let value = eval(program, exprs, frame, *value, ctx.as_deref_mut())?;
                object.insert(name.clone(), Json::from_value(&value));
            }
            Ok(Value::Json(Json::Obj(object)))
        }
        Expr::List { items, inner } => {
            let mut values = Vec::new();
            for item in items {
                let value = eval(program, exprs, frame, *item, ctx.as_deref_mut())?;
                values.push(match inner {
                    Some(declared) => fitted_at(value, declared, exprs.span(*item))?,
                    None => value,
                });
            }
            // Untyped only when nothing declared one, which is the `let xs = [a, b]`
            // shape; a declared element type is what a bare `T` coerces against.
            let ty = inner
                .clone()
                .or_else(|| values.first().map(Value::ty))
                .unwrap_or(Type::Json);
            Ok(Value::List {
                inner: ty,
                items: values,
            })
        }
        Expr::Comp {
            iter,
            cond,
            yields,
            inner: declared,
        } => {
            let mut items = Vec::new();
            let mut inner = declared.clone();
            for (index, item) in elements(program, exprs, frame, iter, ctx.as_deref_mut())? {
                bind_iter(iter, index, item, frame)?;
                if let Some(cond) = cond
                    && !eval_bool(program, exprs, frame, *cond, ctx.as_deref_mut())?
                {
                    continue;
                }
                let value = eval(program, exprs, frame, *yields, ctx.as_deref_mut())?;
                let value = match declared {
                    Some(declared) => fitted_at(value, declared, exprs.span(*yields))?,
                    None => value,
                };
                inner.get_or_insert_with(|| value.ty());
                items.push(value);
            }
            Ok(Value::List {
                inner: inner.unwrap_or(Type::Json),
                items,
            })
        }
        Expr::CallFn {
            function,
            scope,
            args,
        } => {
            let mut values = Vec::new();
            for arg in args {
                values.push(eval(program, exprs, frame, *arg, ctx.as_deref_mut())?);
            }
            let def = program
                .function_in(scope.as_deref(), function)
                .ok_or_else(|| at(ErrorKind::UnknownFunction(function.clone())))?;
            call_function(program, def, values, span, ctx)
        }
        Expr::Record { ty, fields } => {
            // The parser resolved the record and filled every field, so a name that
            // does not resolve here is malformed IR rather than a program error.
            let def = program
                .record(ty)
                .ok_or_else(|| at(ErrorKind::MalformedIr))?;
            let mut values = BTreeMap::new();
            for (name, written) in fields {
                let value = eval(program, exprs, frame, *written, ctx.as_deref_mut())?;
                // A declared field coerces, exactly as an entity column and an event
                // field do.
                let value = match def.field(name) {
                    Some(declared) => fitted_at(value, &declared.ty, exprs.span(*written))?,
                    None => return Err(at(ErrorKind::MalformedIr)),
                };
                values.insert(name.clone(), value);
            }
            Ok(Value::Record {
                ty: ty.clone(),
                fields: values,
            })
        }
        // The `emit` path one step earlier: the same coercion against the same
        // declaration, and deliberately not `check_field`. An over-length value is a
        // command's validation channel and an expression has no `Outcome` to hand back,
        // so the bound is checked where the event lands, at the `given` that takes it.
        Expr::Event { path, fields } => {
            let def = program
                .event(path)
                .ok_or_else(|| at(ErrorKind::MalformedIr))?;
            let mut values = BTreeMap::new();
            for (name, written) in fields {
                let value = eval(program, exprs, frame, *written, ctx.as_deref_mut())?;
                let value = match def.field(name) {
                    Some(declared) => fitted_at(value, &declared.ty, exprs.span(*written))?,
                    None => return Err(at(ErrorKind::MalformedIr)),
                };
                values.insert(name.clone(), value);
            }
            Ok(Value::Event(Event {
                path: path.clone(),
                fields: values,
            }))
        }
        // Rule 16's taint, built as the two renderings rather than recovered later.
        // Both strings are accumulated in one walk, so `"https://{host}/hooks/{PATH}"`
        // comes out spelled for the wire and named for everything else, and the result
        // is a `Secret` exactly when a hole was one.
        Expr::Interp(parts) => {
            let mut text = String::new();
            let mut redacted = String::new();
            let mut tainted = false;
            for part in parts {
                let value = eval(program, exprs, frame, *part, ctx.as_deref_mut())?;
                // Through the same table the two renderings come from, rather than a
                // `matches!` on the variant: an `Opt(Secret)` holds one and is one, and
                // the parser counts it as tainted, so a bare match let a present
                // optional through as a plain `Str` carrying the credential.
                tainted |= matches!(Json::from_value(&value), Json::Secret { .. });
                text.push_str(&value::text(&value));
                redacted.push_str(&value::redacted_text(&value));
            }
            Ok(match tainted {
                true => Value::Secret {
                    plain: Arc::from(text),
                    redacted: Arc::from(redacted),
                },
                false => Value::str(text),
            })
        }
        Expr::Call { builtin, args } => {
            let mut values = Vec::new();
            for arg in args {
                values.push(eval(program, exprs, frame, *arg, ctx.as_deref_mut())?);
            }
            match builtin {
                Builtin::UuidDerive => return uuid_derive(&values).map_err(at),
                // Rule 8's table pointed at a string instead of a socket, so a value
                // encoded here and the same value in a request body cannot disagree.
                Builtin::JsonEncode => {
                    let value = values.first().ok_or_else(|| at(ErrorKind::MalformedIr))?;
                    return Ok(Value::str(Json::from_value(value).to_string()));
                }
                Builtin::TimestampFromParts => {
                    let mut fields = [0i64; 6];
                    for (slot, value) in fields.iter_mut().zip(&values) {
                        let Value::Int(field) = value else {
                            return Err(at(ErrorKind::MalformedIr));
                        };
                        *slot = *field;
                    }
                    let [year, month, day, hour, minute, second] = fields;
                    return Ok(
                        match value::from_parts(year, month, day, hour, minute, second) {
                            Some(micros) => Value::some(Value::Timestamp(micros)),
                            None => Value::none(Type::Timestamp),
                        },
                    );
                }
                Builtin::TimestampParse | Builtin::MoneyParse(_) | Builtin::DecimalParse(_) => {
                    let Some(Value::Str(text)) = values.first() else {
                        return Err(at(ErrorKind::MalformedIr));
                    };
                    return Ok(match builtin {
                        Builtin::TimestampParse => match value::timestamp(text) {
                            Some(micros) => Value::some(Value::Timestamp(micros)),
                            None => Value::none(Type::Timestamp),
                        },
                        Builtin::MoneyParse(scale) => {
                            match value::parse_scaled(text, &Type::Money(*scale)) {
                                Some(value) => Value::some(value),
                                None => Value::none(Type::Money(*scale)),
                            }
                        }
                        Builtin::DecimalParse(scale) => {
                            match value::parse_scaled(text, &Type::Decimal(*scale)) {
                                Some(value) => Value::some(value),
                                None => Value::none(Type::Decimal(*scale)),
                            }
                        }
                        _ => return Err(at(ErrorKind::MalformedIr)),
                    });
                }
                _ => {}
            }
            let Some(ctx) = ctx else {
                return Err(at(ErrorKind::MalformedIr));
            };
            // Rule 16: a url may be a credential, so it crosses as its two renderings
            // rather than as one string. A `String` is both of them.
            // Named, not a tuple read back as `.0` and `.1`. `docs/host.md` argues that
            // two renderings a caller can confuse are identical for every program that
            // holds no secret, so a mix-up would never show up in a test; that argument
            // does not stop at the crate boundary.
            let (wire_url, shown_url) = match values.first() {
                Some(Value::Str(url)) => (url.to_string(), url.to_string()),
                Some(Value::Secret { plain, redacted }) => {
                    (plain.to_string(), redacted.to_string())
                }
                _ => return Err(at(ErrorKind::MalformedIr)),
            };
            // The headers are the last argument and always present, so the shape is
            // (url, headers) or (url, body, headers).
            let headers = values.last().map(Json::from_value).unwrap_or(Json::Null);
            let body = if builtin.has_body() {
                values.get(1).map(Json::from_value)
            } else {
                None
            };
            ctx.http(*builtin, &wire_url, &shown_url, body, headers)
                .map_err(|err| err.located(span))
        }
        Expr::Invoke { command, args } => {
            let mut values: BTreeMap<Ident, Value> = BTreeMap::new();
            for (name, value) in args {
                let value = eval(program, exprs, frame, *value, ctx.as_deref_mut())?;
                values.insert(name.clone(), value);
            }
            let Some(ctx) = ctx else {
                return Err(at(ErrorKind::MalformedIr));
            };
            ctx.invoke(command, values)
        }
        // Narrowing proved this slot present, so an absent value here means the proof
        // and the lowering disagree, which is a bug in the parser rather than in the
        // program. See `docs/optionals.md`.
        Expr::Unwrap(inner) => match eval(program, exprs, frame, *inner, ctx)? {
            Value::Opt {
                value: Some(value), ..
            } => Ok(*value),
            Value::Opt { .. } => Err(at(ErrorKind::MalformedIr)),
            other => Ok(other),
        },
        // The other side of a comparison is an optional, so this side becomes one and
        // `binary` compares two `Opt`s with the exact type test it always had.
        //
        // The check is `has_type` against the inner rather than `fitted` against the
        // optional, which is the same question one allocation cheaper: `fitted` would box
        // a `Type::opt` per evaluation, and this node sits inside conditions a fold runs
        // per event (`docs/fold-cost.md`).
        //
        // It can fail, for a reason that is not this rule's: an `Expr::Comp` that yields
        // nothing evaluates to `List(Json)` (see `inner.unwrap_or` below) while its static
        // type is what its yield says, and an `emit` of one already reports the same
        // disagreement. `binary`'s exact test would reject the pair here anyway; catching
        // it names the operand and the two types rather than the comparison and two
        // optionals.
        Expr::Wrap { value, inner } => {
            let value = eval(program, exprs, frame, *value, ctx)?;
            if !value.has_type(inner) {
                return Err(Error::at(
                    ErrorKind::TypeMismatch {
                        expected: inner.clone(),
                        found: value.ty(),
                    },
                    span,
                ));
            }
            Ok(Value::Opt {
                inner: inner.clone(),
                value: Some(Box::new(value)),
            })
        }
        Expr::Reveal { value, ty } => {
            let value = eval(program, exprs, frame, *value, ctx.as_deref_mut())?;
            let Some(ctx) = ctx else {
                return Err(at(ErrorKind::MalformedIr));
            };
            ctx.reveal(value, ty, span)
        }
        Expr::Refusal { code, message } => {
            let message = eval_string(program, exprs, frame, *message, ctx.as_deref_mut())?;
            Ok(Value::Invoked(match code {
                Some(code) => Invoked::Reject {
                    code: eval_string(program, exprs, frame, *code, ctx)?,
                    message,
                },
                None => Invoked::Invalid(message),
            }))
        }
        // The parser's poison. `check_files` fails whenever one was recorded, so a
        // program holding one never gets this far.
        Expr::Invalid => Err(at(ErrorKind::MalformedIr)),
    }
}

/// A call: a fresh frame with the parameters filled, and the caller's own context. A
/// module `fn` is pure and writes nowhere, so `Sink::Pure` costs it nothing; an
/// effect-local one needs `log` and `fail`, which only `Sink::Effect` carries. Handing
/// a pure helper an effect sink is safe because purity is a parse-time rule, which is
/// the same reason `Sink::Pure` enforces nothing.
fn enter_function(
    program: &Program,
    def: &Function,
    args: Vec<Value>,
    span: Span,
    ctx: Option<&mut Effects<'_>>,
) -> Result<Flow, Error> {
    let mut frame = Frame::new(def.frame);
    if args.len() != def.params.len() {
        return Err(Error::at(ErrorKind::MalformedIr, span));
    }
    for (param, value) in def.params.iter().zip(args) {
        // The same coercion `bind_params` applies, so a bare `T` fills a `T?`.
        let value = fitted_at(value, &param.ty, span)?;
        frame
            .set(param.slot, value)
            .map_err(|kind| Error::at(kind, span))?;
    }
    let mut sink = match ctx {
        Some(ctx) => Sink::Effect(ctx.reborrow()),
        None => Sink::Pure,
    };
    match exec_block(&def.exprs, &def.body, &mut frame, program, &mut sink)? {
        // Rule 4, on the error channel because a call is an expression. `run_arm`
        // catches it where it catches the arm's own `fail`.
        Flow::Return(Ret::Fail(message)) => Err(Error::at(ErrorKind::Failed(message), span)),
        flow => Ok(flow),
    }
}

/// A call whose value is used. The parser proved every path returns, so falling out of
/// the body is malformed IR rather than a case with a value.
fn call_function(
    program: &Program,
    def: &Function,
    args: Vec<Value>,
    span: Span,
    ctx: Option<&mut Effects<'_>>,
) -> Result<Value, Error> {
    let Some(ret) = def.ret.as_ref() else {
        return Err(Error::at(ErrorKind::MalformedIr, span));
    };
    match enter_function(program, def, args, span, ctx)? {
        Flow::Return(Ret::Value(value)) => fitted_at(value, ret, span),
        _ => Err(Error::at(ErrorKind::MalformedIr, span)),
    }
}

/// A call to a `fn` that returns nothing. Both ways out are ordinary here: a bare
/// `return`, and falling off the end.
fn call_void(
    program: &Program,
    def: &Function,
    args: Vec<Value>,
    span: Span,
    ctx: Option<&mut Effects<'_>>,
) -> Result<(), Error> {
    match enter_function(program, def, args, span, ctx)? {
        Flow::Next | Flow::Return(Ret::Ok) => Ok(()),
        _ => Err(Error::at(ErrorKind::MalformedIr, span)),
    }
}
/// Wraps a subject-bound field in its seal as it enters a frame, so `reveal` can find
/// the subject and the id on the value rather than through a side channel.
///
/// This is the only place a seal is made, because it is the only place that has the
/// whole event: the id lives in a sibling field, so a value alone can never say what
/// key it is filed under. Nothing takes a seal off again; a seal reaches the host as it
/// is, and `Keys::decrypt` at a `reveal` is the only thing that opens one. See
/// `docs/effects.md` rule 12.
fn seal(program: &Program, event: &Event, name: &Ident, value: Value) -> Result<Value, Error> {
    let Some(def) = program.event(&event.path) else {
        return Ok(value);
    };
    let Some(declared) = def.field(name) else {
        return Ok(value);
    };
    // The **field** the annotation named, which is where the id is read from, and then
    // the **subject** that field's type names, which is what the key is filed under.
    // Two facts, kept apart: `@subject(buyer)` is local to one declaration and the
    // `Customer` it resolves to is what crosses the host seam.
    let Some(id_field) = declared.subject.clone() else {
        return Ok(value);
    };
    let span = Span::default();
    let Some(Type::Subject(sub)) = def.field(&id_field).map(|def| &def.ty) else {
        return Err(Error::at(ErrorKind::MalformedIr, span));
    };
    let subject = sub.name.clone();
    let Some(id) = subject_id(field(event, &id_field)?, span)? else {
        return Err(Error::at(ErrorKind::MalformedIr, span));
    };
    // Rule 12: an absent optional was never encrypted, so there is no key behind it
    // and nothing to seal. That is the row that must not collapse into the erased one.
    Ok(match value {
        Value::Opt { value: None, inner } => Value::Opt {
            inner: Type::sealed(inner, subject),
            value: None,
        },
        Value::Opt {
            inner,
            value: Some(stored),
        } => Value::Opt {
            inner: Type::sealed(inner, subject.clone()),
            value: Some(Box::new(Value::Sealed {
                field: name.clone(),
                subject,
                id,
                content: stored_text(&stored),
            })),
        },
        // Rule 16: a secret may not be emitted, so nothing checked can reach here. It
        // refuses rather than sealing, because the catch-all below it would take
        // `stored_text` of the plaintext and file it in the log under a subject key,
        // which is the one laundering path a wrong arm here would open.
        Value::Secret { .. } => return Err(Error::at(ErrorKind::MalformedIr, span)),
        stored => Value::Sealed {
            field: name.clone(),
            subject,
            id,
            content: stored_text(&stored),
        },
    })
}

/// The text form of what a host stored, which for a real one is its ciphertext and for
/// the harness is the content as it was given (`docs/host.md`).
///
/// [`value::sealed_text`] is the rendering and this is the allocation: text is already
/// the shape a stored seal has, so the common case is a refcount bump rather than a
/// copy. This runs once per record a fold binds, which is the reason it is worth the arm.
fn stored_text(stored: &Value) -> Arc<str> {
    match stored {
        Value::Str(text) => text.clone(),
        other => value::sealed_text(other).into(),
    }
}

/// The (index, item) pairs a `for` or a comprehension walks. A map yields its key
/// beside its value; a list yields its position. Collected up front, because the body
/// may write frame slots the container was read from.
fn elements(
    program: &Program,
    exprs: &Exprs,
    frame: &mut Frame,
    iter: &Iter,
    ctx: Option<&mut Effects<'_>>,
) -> Result<Vec<(Value, Value)>, Error> {
    let span = exprs.span(iter.over);
    match eval(program, exprs, frame, iter.over, ctx)? {
        Value::List { items, .. } => Ok(items
            .into_iter()
            .enumerate()
            .map(|(index, item)| (Value::Int(index as i64), item))
            .collect()),
        Value::Map { entries, .. } => Ok(entries
            .into_iter()
            .map(|(key, value)| (value::from_key(&key), value))
            .collect()),
        other => Err(Error::at(ErrorKind::NotIterable(other.ty()), span)),
    }
}

fn bind_iter(iter: &Iter, index: Value, item: Value, frame: &mut Frame) -> Result<(), Error> {
    if let Some(slot) = iter.index {
        frame.set(slot, index)?;
    }
    frame.set(iter.item, item)?;
    Ok(())
}

fn eval_bool(
    program: &Program,
    exprs: &Exprs,
    frame: &mut Frame,
    id: ExprId,
    ctx: Option<&mut Effects<'_>>,
) -> Result<bool, Error> {
    match eval(program, exprs, frame, id, ctx)? {
        Value::Bool(value) => Ok(value),
        other => Err(Error::at(
            ErrorKind::TypeMismatch {
                expected: Type::Bool,
                found: other.ty(),
            },
            exprs.span(id),
        )),
    }
}

fn eval_string(
    program: &Program,
    exprs: &Exprs,
    frame: &mut Frame,
    id: ExprId,
    ctx: Option<&mut Effects<'_>>,
) -> Result<String, Error> {
    match eval(program, exprs, frame, id, ctx)? {
        Value::Str(value) => Ok(value.to_string()),
        other => Err(Error::at(
            ErrorKind::TypeMismatch {
                expected: Type::String,
                found: other.ty(),
            },
            exprs.span(id),
        )),
    }
}

fn unary(op: UnOp, value: Value) -> Result<Value, ErrorKind> {
    match (op, value) {
        (UnOp::Not, Value::Bool(value)) => Ok(Value::Bool(!value)),
        (UnOp::Neg, Value::Int(value)) => Ok(Value::Int(scaled::neg(value)?)),
        (UnOp::Neg, Value::Decimal { units, scale }) => Ok(Value::Decimal {
            units: scaled::neg(units)?,
            scale,
        }),
        (UnOp::Neg, Value::Money { units, scale }) => Ok(Value::money(scaled::neg(units)?, scale)),
        (op, other) => Err(ErrorKind::BadUnaryOperand { op, ty: other.ty() }),
    }
}

/// How two values order, or `None` for a pair that does not order at all.
///
/// **The one place ordering is decided**, so `<` and `>` and `max` and `min` cannot come
/// to disagree: `types::comparable` is the checker's half of this list and this is the
/// runtime's, and a type that ordered in one and not the other would be a method whose
/// signature says it works and whose body raises. Scales do not meet here, for the reason
/// they do not meet in arithmetic.
fn ordering(lhs: &Value, rhs: &Value) -> Option<cmp::Ordering> {
    Some(match (lhs, rhs) {
        (Value::Int(a), Value::Int(b)) => a.cmp(b),
        (
            Value::Decimal { units: a, scale },
            Value::Decimal {
                units: b,
                scale: other,
            },
        ) if scale == other => a.cmp(b),
        (
            Value::Money { units: a, scale },
            Value::Money {
                units: b,
                scale: other,
            },
        ) if scale == other => a.cmp(b),
        (Value::Str(a), Value::Str(b)) => a.cmp(b),
        (Value::Timestamp(a), Value::Timestamp(b)) => a.cmp(b),
        _ => return None,
    })
}

fn binary(op: BinOp, lhs: Value, rhs: Value) -> Result<Value, ErrorKind> {
    match op {
        BinOp::Eq | BinOp::Ne => {
            if lhs.ty() != rhs.ty() {
                return Err(ErrorKind::BadOperands {
                    op,
                    lhs: lhs.ty(),
                    rhs: rhs.ty(),
                });
            }
            let equal = lhs == rhs;
            Ok(Value::Bool(if op == BinOp::Eq { equal } else { !equal }))
        }
        BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            let ordering = ordering(&lhs, &rhs).ok_or_else(|| ErrorKind::BadOperands {
                op,
                lhs: lhs.ty(),
                rhs: rhs.ty(),
            })?;
            Ok(Value::Bool(match op {
                BinOp::Lt => ordering.is_lt(),
                BinOp::Le => ordering.is_le(),
                BinOp::Gt => ordering.is_gt(),
                _ => ordering.is_ge(),
            }))
        }
        BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Rem => match (&lhs, &rhs) {
            (Value::Int(a), Value::Int(b)) => arith(op, *a, *b).map(Value::Int),
            (
                Value::Decimal { units: a, scale },
                Value::Decimal {
                    units: b,
                    scale: other,
                },
            ) if scale == other && matches!(op, BinOp::Add | BinOp::Sub) => {
                arith(op, *a, *b).map(|units| Value::Decimal {
                    units,
                    scale: *scale,
                })
            }
            (Value::Decimal { units, scale }, Value::Int(factor))
                if matches!(op, BinOp::Mul | BinOp::Div) =>
            {
                arith(op, *units, *factor).map(|units| Value::Decimal {
                    units,
                    scale: *scale,
                })
            }
            (Value::Int(factor), Value::Decimal { units, scale }) if op == BinOp::Mul => {
                arith(op, *factor, *units).map(|units| Value::Decimal {
                    units,
                    scale: *scale,
                })
            }
            // Money keeps its own operator table, which is the whole reason it is not
            // a `Decimal`. Two amounts add and subtract; a rate scales an amount; two
            // amounts multiplied is a type error, as is an amount plus a bare decimal.
            (
                Value::Money { units: a, scale },
                Value::Money {
                    units: b,
                    scale: other,
                },
            ) if scale == other && matches!(op, BinOp::Add | BinOp::Sub) => {
                arith(op, *a, *b).map(|units| Value::money(units, *scale))
            }
            (
                Value::Money { units: a, scale },
                Value::Money {
                    units: b,
                    scale: other,
                },
            ) if scale == other && op == BinOp::Div => Ok(Value::Decimal {
                units: scaled::ratio(*a, *b, scaled::RATIO_SCALE)?,
                scale: scaled::RATIO_SCALE,
            }),
            (Value::Money { units, scale }, Value::Int(factor)) if op == BinOp::Mul => {
                arith(op, *units, *factor).map(|units| Value::money(units, *scale))
            }
            (Value::Int(factor), Value::Money { units, scale }) if op == BinOp::Mul => {
                arith(op, *factor, *units).map(|units| Value::money(units, *scale))
            }
            (Value::Money { units, scale }, Value::Int(divisor)) if op == BinOp::Div => {
                let units = scaled::div_exact(*units, *divisor).map_err(inexact(op, "div"))?;
                Ok(Value::money(units, *scale))
            }
            (
                Value::Money {
                    units: amount,
                    scale,
                },
                Value::Decimal { units, scale: rate },
            ) if op == BinOp::Mul => {
                let units =
                    scaled::mul_ratio_exact(*amount, *units, *rate).map_err(inexact(op, "mul"))?;
                Ok(Value::money(units, *scale))
            }
            (
                Value::Decimal { units, scale: rate },
                Value::Money {
                    units: amount,
                    scale,
                },
            ) if op == BinOp::Mul => {
                let units =
                    scaled::mul_ratio_exact(*amount, *units, *rate).map_err(inexact(op, "mul"))?;
                Ok(Value::money(units, *scale))
            }
            (Value::Str(a), Value::Str(b)) if op == BinOp::Add => Ok(Value::str(format!("{a}{b}"))),
            _ => Err(ErrorKind::BadOperands {
                op,
                lhs: lhs.ty(),
                rhs: rhs.ty(),
            }),
        },
        BinOp::And | BinOp::Or => Err(ErrorKind::BadOperands {
            op,
            lhs: lhs.ty(),
            rhs: rhs.ty(),
        }),
    }
}

fn inexact(op: BinOp, hint: &'static str) -> impl Fn(scaled::Error) -> ErrorKind {
    move |err| match err {
        scaled::Error::Inexact => ErrorKind::InexactMoney { op, hint },
        other => ErrorKind::from(other),
    }
}

fn arith(op: BinOp, lhs: i64, rhs: i64) -> Result<i64, ErrorKind> {
    let value = match op {
        BinOp::Add => scaled::add(lhs, rhs),
        BinOp::Sub => scaled::sub(lhs, rhs),
        BinOp::Mul => scaled::mul(lhs, rhs),
        BinOp::Div => scaled::div(lhs, rhs),
        _ => scaled::rem(lhs, rhs),
    };
    value.map_err(ErrorKind::from)
}

fn call_method(receiver: Value, method: &str, args: Vec<Value>) -> Result<Value, ErrorKind> {
    match (&receiver, method) {
        (Value::Str(value), "trim") => {
            expect_arity(method, 0, &args)?;
            Ok(Value::str(value.trim()))
        }
        (Value::Str(value), "len") => {
            expect_arity(method, 0, &args)?;
            Ok(Value::Int(value.chars().count() as i64))
        }
        (Value::Str(value), "is_empty") => {
            expect_arity(method, 0, &args)?;
            Ok(Value::Bool(value.is_empty()))
        }
        (Value::Str(value), "lower") => {
            expect_arity(method, 0, &args)?;
            Ok(Value::str(value.to_lowercase()))
        }
        (Value::Str(value), "upper") => {
            expect_arity(method, 0, &args)?;
            Ok(Value::str(value.to_uppercase()))
        }
        (Value::Str(value), "starts_with") => {
            expect_arity(method, 1, &args)?;
            match &args[0] {
                Value::Str(prefix) => Ok(Value::Bool(value.starts_with(&**prefix))),
                other => Err(ErrorKind::TypeMismatch {
                    expected: Type::String,
                    found: other.ty(),
                }),
            }
        }
        // Returns the string unchanged when the prefix is absent, rather than an
        // optional: it is written after a `starts_with` that already decided.
        (Value::Str(value), "strip_prefix") => {
            expect_arity(method, 1, &args)?;
            match &args[0] {
                Value::Str(prefix) => {
                    Ok(Value::str(value.strip_prefix(&**prefix).unwrap_or(value)))
                }
                other => Err(ErrorKind::TypeMismatch {
                    expected: Type::String,
                    found: other.ty(),
                }),
            }
        }
        // The whole string when the separator is absent, which is what makes
        // `gid.after_last("/")` safe on something that is not a gid.
        (Value::Str(value), "after_last") => {
            expect_arity(method, 1, &args)?;
            match &args[0] {
                Value::Str(sep) if !sep.is_empty() => Ok(Value::str(
                    value.rsplit_once(&**sep).map_or(&**value, |(_, tail)| tail),
                )),
                Value::Str(_) => Ok(Value::Str(value.clone())),
                other => Err(ErrorKind::TypeMismatch {
                    expected: Type::String,
                    found: other.ty(),
                }),
            }
        }
        // The first `n` characters, counted the way `len` and `@max` count them, and the
        // string itself when it already fits. A count at or below zero keeps nothing:
        // the answer is still a string of at most `n` characters, which is the whole
        // contract.
        (Value::Str(value), "truncate") => {
            expect_arity(method, 1, &args)?;
            match &args[0] {
                Value::Int(count) => {
                    // Clamped at both ends, and the two ends are not the same fallback:
                    // a negative count keeps nothing, and one past `usize` keeps
                    // everything. `unwrap_or(0)` would read the second as the first and
                    // empty the string on a 32-bit host.
                    let count = usize::try_from((*count).max(0)).unwrap_or(usize::MAX);
                    Ok(match value.char_indices().nth(count) {
                        Some((end, _)) => Value::str(&value[..end]),
                        None => Value::Str(value.clone()),
                    })
                }
                other => Err(ErrorKind::TypeMismatch {
                    expected: Type::Int,
                    found: other.ty(),
                }),
            }
        }
        (Value::Str(value), "to_int") => {
            expect_arity(method, 0, &args)?;
            Ok(match value.parse::<i64>() {
                Ok(parsed) => Value::some(Value::Int(parsed)),
                Err(_) => Value::none(Type::Int),
            })
        }
        (Value::Str(value), "to_uuid") => {
            expect_arity(method, 0, &args)?;
            Ok(match Uuid::parse_str(value) {
                Ok(_) => Value::some(Value::Uuid(value.clone())),
                Err(_) => Value::none(Type::Uuid),
            })
        }
        (Value::Str(value), "contains") => {
            expect_arity(method, 1, &args)?;
            match &args[0] {
                Value::Str(needle) => Ok(Value::Bool(value.contains(&**needle))),
                other => Err(ErrorKind::TypeMismatch {
                    expected: Type::String,
                    found: other.ty(),
                }),
            }
        }
        (
            Value::Money {
                units: amount,
                scale,
            },
            "mul",
        ) => {
            expect_arity(method, 2, &args)?;
            let (rate, places) = match &args[0] {
                Value::Decimal { units, scale } => (*units, *scale),
                other => {
                    return Err(ErrorKind::BadArgument {
                        method: method.to_string(),
                        expected: "a Decimal",
                        found: other.ty(),
                    });
                }
            };
            let rounding = rounding_arg(method, &args[1])?;
            Ok(Value::money(
                scaled::mul_ratio(*amount, rate, places, rounding)?,
                *scale,
            ))
        }
        (
            Value::Money {
                units: amount,
                scale,
            },
            "div",
        ) => {
            expect_arity(method, 2, &args)?;
            let divisor = match &args[0] {
                Value::Int(divisor) => *divisor,
                other => {
                    return Err(ErrorKind::BadArgument {
                        method: method.to_string(),
                        expected: "an Int",
                        found: other.ty(),
                    });
                }
            };
            let rounding = rounding_arg(method, &args[1])?;
            Ok(Value::money(
                scaled::div_round(*amount, divisor, rounding)?,
                *scale,
            ))
        }
        (Value::List { items, .. }, "len") => {
            expect_arity(method, 0, &args)?;
            Ok(Value::Int(items.len() as i64))
        }
        (Value::List { items, .. }, "is_empty") => {
            expect_arity(method, 0, &args)?;
            Ok(Value::Bool(items.is_empty()))
        }
        (Value::List { items, .. }, "contains") => {
            expect_arity(method, 1, &args)?;
            Ok(Value::Bool(items.contains(&args[0])))
        }
        (Value::List { inner, items }, "first") => {
            expect_arity(method, 0, &args)?;
            Ok(match items.first() {
                Some(first) => Value::some(first.clone()),
                None => Value::none(inner.clone()),
            })
        }
        // `push` and `remove` build a new list, so a fold arm still returns new state
        // and nothing a value was handed to can change it.
        (Value::List { inner, items }, "push") => {
            expect_arity(method, 1, &args)?;
            let mut items = items.clone();
            // The element type is declared, so a bare `T` into a `List(T?)` wraps here
            // the way it would into any other declared position.
            let pushed = args.into_iter().next().ok_or(ErrorKind::MalformedIr)?;
            items.push(fitted(pushed, inner)?);
            Ok(Value::List {
                inner: inner.clone(),
                items,
            })
        }
        // Every element added at one scale, so the answer carries that scale and never
        // rounds. The element type rather than the first element decides it, which is
        // what gives an empty list a zero to be.
        (Value::List { inner, items }, "sum") => {
            expect_arity(method, 0, &args)?;
            let mut total: i64 = 0;
            for item in items {
                let units = match (inner, item) {
                    (Type::Int, Value::Int(value)) => *value,
                    (
                        Type::Decimal(scale),
                        Value::Decimal {
                            units,
                            scale: found,
                        },
                    )
                    | (
                        Type::Money(scale),
                        Value::Money {
                            units,
                            scale: found,
                        },
                    ) if found == scale => *units,
                    _ => return Err(ErrorKind::MalformedIr),
                };
                total = scaled::add(total, units).map_err(ErrorKind::from)?;
            }
            // Named exhaustively rather than defaulting to `Int`, because the empty
            // case is the one nothing else checks: with no element to disagree with,
            // a wrong element type walks straight out as a zero of the wrong kind.
            // That is exactly how a `List(Json)` from an un-hinted empty comprehension
            // came back as `Int(0)` where a `Money(3)` was declared.
            Ok(match inner {
                Type::Int => Value::Int(total),
                Type::Decimal(scale) => Value::Decimal {
                    units: total,
                    scale: *scale,
                },
                Type::Money(scale) => Value::money(total, *scale),
                _ => return Err(ErrorKind::MalformedIr),
            })
        }
        // A new list, like `push` and for the same reason. The element type is the
        // receiver's, so a value arriving from the argument fits the same way one
        // arriving from `push` does.
        (Value::List { inner, items }, "concat") => {
            expect_arity(method, 1, &args)?;
            let Some(Value::List { items: tail, .. }) = args.into_iter().next() else {
                return Err(ErrorKind::MalformedIr);
            };
            let mut items = items.clone();
            for item in tail {
                items.push(fitted(item, inner)?);
            }
            Ok(Value::List {
                inner: inner.clone(),
                items,
            })
        }
        (Value::List { inner, items }, "distinct") => {
            expect_arity(method, 0, &args)?;
            // **The first of each, so the order is the order they were written in**, which
            // is what a comprehension over the result then reads in and what `keys` and
            // `values` already promise for a map.
            //
            // A scan rather than a set: the equality is the element type's own, the one
            // `contains` and `remove` use, and a set would need a hash or an order that
            // every value does not have. It is the same cost as the `contains` inside a
            // comprehension that this replaces, in one call instead of two helper `fn`s.
            let mut kept: Vec<Value> = Vec::with_capacity(items.len());
            for item in items {
                if !kept.contains(item) {
                    kept.push(item.clone());
                }
            }
            Ok(Value::List {
                inner: inner.clone(),
                items: kept,
            })
        }
        (Value::List { inner, items }, "remove") => {
            expect_arity(method, 1, &args)?;
            // Every equal element, not the first, which makes it idempotent the way
            // a map's `remove` is.
            let items = items
                .iter()
                .filter(|item| *item != &args[0])
                .cloned()
                .collect();
            Ok(Value::List {
                inner: inner.clone(),
                items,
            })
        }
        (Value::Map { entries, .. }, "len") => {
            expect_arity(method, 0, &args)?;
            Ok(Value::Int(entries.len() as i64))
        }
        (Value::Map { entries, .. }, "is_empty") => {
            expect_arity(method, 0, &args)?;
            Ok(Value::Bool(entries.is_empty()))
        }
        (Value::Map { entries, .. }, "contains") => {
            expect_arity(method, 1, &args)?;
            Ok(Value::Bool(entries.contains_key(&map_key(&args[0])?)))
        }
        (Value::Map { value, entries, .. }, "get") => {
            expect_arity(method, 1, &args)?;
            Ok(match entries.get(&map_key(&args[0])?) {
                Some(found) => Value::some(found.clone()),
                None => Value::none(value.clone()),
            })
        }
        (
            Value::Map {
                key,
                value,
                entries,
            },
            "set",
        ) => {
            expect_arity(method, 2, &args)?;
            let mut entries = entries.clone();
            let mut args = args.into_iter();
            let at = map_key(&args.next().ok_or(ErrorKind::MalformedIr)?)?;
            let stored = args.next().ok_or(ErrorKind::MalformedIr)?;
            entries.insert(at, fitted(stored, value)?);
            Ok(Value::Map {
                key: key.clone(),
                value: value.clone(),
                entries,
            })
        }
        (
            Value::Map {
                key,
                value,
                entries,
            },
            "remove",
        ) => {
            expect_arity(method, 1, &args)?;
            let mut entries = entries.clone();
            entries.remove(&map_key(&args[0])?);
            Ok(Value::Map {
                key: key.clone(),
                value: value.clone(),
                entries,
            })
        }
        (Value::Map { key, entries, .. }, "keys") => {
            expect_arity(method, 0, &args)?;
            Ok(Value::list(
                key.clone(),
                entries.keys().map(value::from_key),
            ))
        }
        (Value::Map { value, entries, .. }, "values") => {
            expect_arity(method, 0, &args)?;
            Ok(Value::list(value.clone(), entries.values().cloned()))
        }
        (Value::Opt { value, .. }, "is_some") => {
            expect_arity(method, 0, &args)?;
            Ok(Value::Bool(value.is_some()))
        }
        (Value::Opt { value, .. }, "is_none") => {
            expect_arity(method, 0, &args)?;
            Ok(Value::Bool(value.is_none()))
        }
        (Value::Opt { inner, value }, "unwrap_or") => {
            expect_arity(method, 1, &args)?;
            let fallback = args.into_iter().next().ok_or(ErrorKind::MalformedIr)?;
            if !fallback.has_type(inner) {
                return Err(ErrorKind::TypeMismatch {
                    expected: inner.clone(),
                    found: fallback.ty(),
                });
            }
            match value {
                Some(value) => Ok(value.as_ref().clone()),
                None => Ok(fallback),
            }
        }
        // Rule 8: one-step fallible accessors, because every read of an untyped body
        // is a branch anyway and the two-step form makes the author write two.
        (Value::Json(json), "string") => json_field(json, method, &args, Type::String),
        (Value::Json(json), "int") => json_field(json, method, &args, Type::Int),
        (Value::Json(json), "bool") => json_field(json, method, &args, Type::Bool),
        // A GraphQL response is nested, so two accessors beyond rule 8's three: one
        // step down, and one step into an array.
        (Value::Json(json), "json") => json_field(json, method, &args, Type::Json),
        (Value::Json(json), "array") => json_field(json, method, &args, Type::list(Type::Json)),
        // The exact text of a number, which is the only lossless way to hand one over:
        // the scale a `10.5` should land at comes from where it is going, so this stops
        // at the text and `Money.parse` finishes the job against a declared target.
        (Value::Json(json), "number") => {
            expect_arity(method, 1, &args)?;
            let Value::Str(key) = &args[0] else {
                return Err(ErrorKind::TypeMismatch {
                    expected: Type::String,
                    found: args[0].ty(),
                });
            };
            Ok(match json.get(key) {
                Some(Json::Num(text)) => Value::some(Value::str(text.as_str())),
                _ => Value::none(Type::String),
            })
        }
        (Value::Timestamp(micros), "year" | "month" | "day" | "hour" | "minute" | "second") => {
            expect_arity(method, 0, &args)?;
            let (year, month, day, hour, minute, second) = value::parts(*micros);
            Ok(Value::Int(match method {
                "year" => year,
                "month" => month,
                "day" => day,
                "hour" => hour,
                "minute" => minute,
                _ => second,
            }))
        }
        // A `Timestamp` is epoch microseconds, so a fixed-length unit is a multiply and
        // an add and nothing is rounded or clamped on the way. Sub-second precision
        // survives, which is the thing a `fn` written over `parts` and `from_parts`
        // cannot do: `from_parts` is on the second.
        //
        // `Overflow` rather than an optional, which is the answer `Int` and `Money`
        // arithmetic already give: `from_parts` is optional because Feb 30 is not a
        // date, and five minutes after a real moment always is one.
        (Value::Timestamp(micros), "add_seconds" | "add_minutes" | "add_hours" | "add_days") => {
            expect_arity(method, 1, &args)?;
            let Value::Int(count) = &args[0] else {
                return Err(ErrorKind::TypeMismatch {
                    expected: Type::Int,
                    found: args[0].ty(),
                });
            };
            let unit: i64 = match method {
                "add_seconds" => 1_000_000,
                "add_minutes" => 60_000_000,
                "add_hours" => 3_600_000_000,
                _ => 86_400_000_000,
            };
            count
                .checked_mul(unit)
                .and_then(|delta| micros.checked_add(delta))
                .map(Value::Timestamp)
                .ok_or(ErrorKind::Overflow)
        }
        // Zero-padded on the left to `width` characters, and the number's own text when
        // it already meets it: `truncate` pointed the other way, and both count the
        // characters `len` counts.
        //
        // The sign comes first and the zeros after it, because `-05` is the padding of
        // `-5` and `0-5` is not a number. A width at or below zero pads nothing, the
        // same way a `truncate` at or below zero keeps nothing.
        //
        // **The bound is not decoration.** This is the only method in the language that
        // makes a string longer, `width` is an ordinary expression, and a total language
        // whose handlers cannot crash the runtime cannot also let `id.pad(w)` allocate
        // whatever `w` says. `truncate` needs no such rule because it can only shrink.
        (Value::Int(value), "pad") => {
            expect_arity(method, 1, &args)?;
            let Value::Int(width) = &args[0] else {
                return Err(ErrorKind::TypeMismatch {
                    expected: Type::Int,
                    found: args[0].ty(),
                });
            };
            if *width > MAX_PAD {
                return Err(ErrorKind::PadWidth(*width));
            }
            let digits = value.unsigned_abs().to_string();
            let sign = if *value < 0 { "-" } else { "" };
            // Counted over the whole rendering, sign included, so `pad(3)` answers a
            // three-character string whatever the sign. The cast cannot truncate, since
            // the bound above is far inside `usize` on every host.
            let width = usize::try_from((*width).max(0)).unwrap_or(0);
            let zeros = width.saturating_sub(digits.len() + sign.len());
            Ok(Value::str(format!("{sign}{}{digits}", "0".repeat(zeros))))
        }
        // The optional receiver, before the general one below it: an absent value is "no
        // value yet", so the argument is the answer. That is the whole of the `fn` a port
        // wrote sixteen call sites of, and it is why the result is a bare value.
        (Value::Opt { inner, value }, "max" | "min") => {
            expect_arity(method, 1, &args)?;
            let other = args.into_iter().next().ok_or(ErrorKind::MalformedIr)?;
            if !other.has_type(inner) {
                return Err(ErrorKind::TypeMismatch {
                    expected: inner.clone(),
                    found: other.ty(),
                });
            }
            match value {
                Some(held) => selected(method, held, other),
                // **Asked of the argument, because an absent receiver has no value to ask
                // it of.** Without this the absent branch is the one that answers where
                // the checker refused, and which it did would depend on whether the row
                // happened to hold anything.
                None if ordering(&other, &other).is_some() => Ok(other),
                None => Err(ErrorKind::UnknownMethod {
                    ty: receiver.ty(),
                    method: method.to_string(),
                }),
            }
        }
        (held, "max" | "min") => {
            expect_arity(method, 1, &args)?;
            let other = args.into_iter().next().ok_or(ErrorKind::MalformedIr)?;
            selected(method, held, other)
        }
        (Value::Invoked(outcome), "ok") => {
            expect_arity(method, 0, &args)?;
            Ok(Value::Bool(outcome.ok()))
        }
        (Value::Invoked(outcome), "code") => {
            expect_arity(method, 0, &args)?;
            Ok(optional_str(outcome.code()))
        }
        (Value::Invoked(outcome), "message") => {
            expect_arity(method, 0, &args)?;
            Ok(optional_str(outcome.message()))
        }
        // `invalid` carries no code, so it is refused by nothing: the question is
        // "did it refuse with this one", and a malformed request did not refuse at all.
        (Value::Invoked(outcome), "refused") => {
            expect_arity(method, 1, &args)?;
            match &args[0] {
                Value::Str(code) => Ok(Value::Bool(outcome.code() == Some(&**code))),
                other => Err(ErrorKind::TypeMismatch {
                    expected: Type::String,
                    found: other.ty(),
                }),
            }
        }
        _ => Err(ErrorKind::UnknownMethod {
            ty: receiver.ty(),
            method: method.to_string(),
        }),
    }
}

/// `max` or `min` of two values that order, which is what both receivers come down to.
///
/// The checker has already refused a receiver that does not order and an argument that is
/// not the receiver's type, so both errors here are defences rather than paths a program
/// reaches. They are told apart by asking whether the receiver orders against *itself*: a
/// receiver that does not is the method not existing, and one that does with an argument
/// that does not is the argument's type, which is the same split `unwrap_or` makes.
fn selected(method: &str, held: &Value, other: Value) -> Result<Value, ErrorKind> {
    let Some(ordering) = ordering(held, &other) else {
        if ordering(held, held).is_none() {
            return Err(ErrorKind::UnknownMethod {
                ty: held.ty(),
                method: method.to_string(),
            });
        }
        return Err(ErrorKind::TypeMismatch {
            expected: held.ty(),
            found: other.ty(),
        });
    };
    let keep_held = match method {
        "max" => ordering.is_ge(),
        _ => ordering.is_le(),
    };
    Ok(match keep_held {
        true => held.clone(),
        false => other,
    })
}

/// A map subscript. The restriction to orderable types is checked at parse time, so
/// reaching this with anything else is a malformed program rather than bad input.
fn map_key(value: &Value) -> Result<Key, ErrorKind> {
    Key::from_value(value).ok_or_else(|| ErrorKind::BadKey(value.ty()))
}

fn rounding_arg(method: &str, value: &Value) -> Result<Rounding, ErrorKind> {
    match value {
        Value::Rounding(mode) => Ok(*mode),
        other => Err(ErrorKind::BadArgument {
            method: method.to_string(),
            expected: "a rounding mode",
            found: other.ty(),
        }),
    }
}

fn expect_arity(method: &str, expected: usize, args: &[Value]) -> Result<(), ErrorKind> {
    if args.len() == expected {
        Ok(())
    } else {
        Err(ErrorKind::BadArity {
            method: method.to_string(),
            expected,
            found: args.len(),
        })
    }
}

fn envelope_value(record: &Record, field: EnvField) -> Value {
    match field {
        EnvField::At => Value::Timestamp(record.at),
        EnvField::Id => Value::uuid(record.id.as_str()),
        EnvField::Position => Value::Int(record.position as i64),
    }
}

fn json_field(json: &Json, method: &str, args: &[Value], want: Type) -> Result<Value, ErrorKind> {
    expect_arity(method, 1, args)?;
    let Value::Str(key) = &args[0] else {
        return Err(ErrorKind::TypeMismatch {
            expected: Type::String,
            found: args[0].ty(),
        });
    };
    let found = json.get(key).and_then(|found| match (found, &want) {
        (Json::Str(value), Type::String) => Some(Value::str(value.as_str())),
        // `body.int("n")` answers only for a whole number. A fractional one is the same
        // `none` a missing key gives, which is the rule every accessor here follows.
        (Json::Num(text), Type::Int) => text.parse().ok().map(Value::Int),
        (Json::Bool(value), Type::Bool) => Some(Value::Bool(*value)),
        (Json::Obj(_), Type::Json) => Some(Value::Json(found.clone())),
        (Json::Arr(items), Type::List(_)) => Some(Value::list(
            Type::Json,
            items.iter().cloned().map(Value::Json),
        )),
        _ => None,
    });
    Ok(match found {
        Some(value) => Value::some(value),
        None => Value::none(want),
    })
}

fn optional_str(value: Option<&str>) -> Value {
    match value {
        Some(value) => Value::some(Value::str(value)),
        None => Value::none(Type::String),
    }
}

/// Rule 11. There is no `Uuid.new`, so an id is always derived from one that already
/// exists, and a retry or a replay derives the same one.
fn uuid_derive(args: &[Value]) -> Result<Value, ErrorKind> {
    let (Some(Value::Uuid(seed)), Some(Value::Str(name))) = (args.first(), args.get(1)) else {
        return Err(ErrorKind::BadArgument {
            method: "Uuid.derive".to_string(),
            expected: "a Uuid seed and a String name",
            found: args.first().map_or(Type::Uuid, Value::ty),
        });
    };
    let parsed = Uuid::parse_str(seed).map_err(|_| ErrorKind::BadUuid(seed.to_string()))?;
    Ok(Value::uuid(
        Uuid::new_v5(&parsed, name.as_bytes()).to_string(),
    ))
}

/// How far one `drive` will follow a chain of triggered events. A backstop only: the
/// parser's self-trigger check is what actually makes the walk terminate, so tripping
/// this reports a hole in that check rather than an expected limit.
///
/// Depth, not volume, which is what this used to count and why sixteen sales in one log
/// tripped it. An effect handling a thousand events appends a thousand, every one of them
/// one step from an event that was already there; a runaway appends one at a time,
/// forever, each a step further out than the last. The first is a busy log and the second
/// is the bug, and depth is what tells them apart. Volume never could.
const CASCADE: u32 = 32;

/// The widest rendering `Int.pad(width)` will build.
///
/// `pad` is the one method that makes a string longer, and its width is an ordinary
/// expression, so it is the one place a handler could ask for an allocation the size of
/// whatever a request parameter said. A total language cannot have that.
///
/// The number is the widest `@max` any real declaration carries: a 4096-character API
/// key. A width past the longest string a field is declared to hold is not a field's
/// shape any more, and a `pad` is always written for one.
const MAX_PAD: i64 = 4096;

/// How many attempts the runtime makes before a call wedges. Retryable statuses and
/// transport errors are absorbed here, so the handler never sees one (rule 5). The
/// policy is the language's and the attempt is the host's: two hosts answering one
/// program differently is exactly what rule 5 exists to rule out.
const ATTEMPTS: usize = 4;

/// 408, 425, 429 and any 5xx each name a condition that clears on its own, with the
/// same request.
fn is_retryable(status: u16) -> bool {
    matches!(status, 408 | 425 | 429) || status >= 500
}

/// What one delivery came to. `Ignored` is not an outcome: no arm selected the event,
/// so there was no invocation to have one.
///
/// Rule 15's collapsing has no variant here, and deliberately: `deliver` names one
/// position and cannot see the batch it would have to be newest in, so it could never
/// answer with one. A walk reports the positions it folded away through `Counts::collapsed`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    Done,
    Ignored,
    Failed(String),
    Skipped(String),
}

/// Rule 4: three meanings, counted apart. The safety of `fail` rests entirely on
/// `failures` never collapsing into `wedged`, because an effect quietly failing a
/// thousand events looks exactly like one quietly succeeding otherwise.
#[derive(Debug, Default)]
pub struct Counts {
    pub done: usize,
    pub ignored: usize,
    /// Rule 15: positions an `on latest` arm selected and did not invoke for, because a
    /// later position in the same batch had the same key. Counted apart from `ignored`,
    /// which is the positions no arm wanted at all.
    pub collapsed: usize,
    pub failures: Vec<String>,
    pub skips: Vec<String>,
    /// A wedge does not advance, so a walk stops at the first one.
    pub wedged: Option<(u64, Error)>,
}

impl Counts {
    pub fn failed(&self) -> usize {
        self.failures.len()
    }

    pub fn skipped(&self) -> usize {
        self.skips.len()
    }
}

/// What an effect arm's expressions can reach and a command's cannot. Threaded through
/// `eval` because an effect builtin is an expression.
struct Effects<'a> {
    program: &'a Program,
    host: &'a mut dyn Host,
    journal: &'a mut dyn Calls,
    traffic: &'a mut Traffic,
    lines: &'a mut Vec<String>,
    trace: &'a mut Vec<Effectful>,
    /// How many times each call has been made so far in this invocation, so a repeated
    /// identical call lines up with its own recording rather than the first one's.
    /// Borrowed rather than owned because `reborrow` hands it to a called effect-local
    /// `fn`: a fresh map there would give the helper's first `http.post` ordinal 0,
    /// and a replay would answer it with whatever the arm's own first call recorded.
    used: &'a mut BTreeMap<String, u32>,
}

impl<'a> Effects<'a> {
    /// A shorter-lived view of the same context, so a call can be handed a `Sink`
    /// without moving the borrows its caller still needs. Every field is a borrow,
    /// including `used`, so the journal keeps one ordinal counter per invocation
    /// rather than one per frame.
    fn reborrow(&mut self) -> Effects<'_> {
        Effects {
            program: self.program,
            host: self.host,
            journal: self.journal,
            traffic: self.traffic,
            lines: self.lines,
            trace: self.trace,
            used: self.used,
        }
    }
}

impl Effects<'_> {
    fn recorded(&mut self, call: &str) -> Result<(u32, Option<Recorded>), Error> {
        let counter = self.used.entry(call.to_string()).or_insert(0);
        let ordinal = *counter;
        *counter += 1;
        let found = self.journal.recorded(call, ordinal)?;
        Ok((ordinal, found))
    }

    fn record(&mut self, call: &str, ordinal: u32, recorded: Recorded) -> Result<(), Error> {
        self.journal.record(call, ordinal, recorded)
    }

    /// Rule 11: journaled, and pinned once per invocation rather than once per call,
    /// which is where this diverges from hekla.
    fn now(&mut self) -> Result<i64, Error> {
        let (ordinal, found) = self.recorded("now()")?;
        if let Some(Recorded::Now(at)) = found {
            return Ok(at);
        }
        let at = self.host.now();
        self.record("now()", ordinal, Recorded::Now(at))?;
        Ok(at)
    }

    /// The journal key is the verb, the URL and the body, and deliberately **not** the
    /// headers. A changed idempotency key must land on the same entry, or a replay
    /// would re-send the request it was written to suppress.
    fn http(
        &mut self,
        builtin: Builtin,
        url: &str,
        shown_url: &str,
        body: Option<Json>,
        headers: Json,
    ) -> Result<Value, Error> {
        // Rule 16: the key is built from the **redacted** rendering, which is what makes
        // it survive a rotation. Keying on the credential would make every entry for a
        // rotated webhook key on a string that no longer exists, so a crash-replay would
        // miss and re-fire the send, and `verify` would report a divergence for every
        // historical invocation. It also keeps the plaintext out of a string that is a
        // readable description by design and that a host may store.
        // Each rendering is walked once, here, and both are carried down rather than
        // rebuilt: `wire` and `shown` each deep-clone the document, and a request with a
        // body was paying for four walks.
        let wire = Parts {
            url: url.to_string(),
            body: body.as_ref().map(Json::wire),
            headers: headers.wire(),
        };
        let shown = Parts {
            url: shown_url.to_string(),
            body: body.as_ref().map(Json::shown),
            headers: headers.shown(),
        };
        let call = match &shown.body {
            Some(body) => format!("{} {shown_url} {body}", builtin.name()),
            None => format!("{} {shown_url}", builtin.name()),
        };
        let (ordinal, found) = self.recorded(&call)?;
        if let Some(Recorded::Response { status, body }) = found {
            return Ok(Value::Response { status, body });
        }

        // The trace is the harness's own observation, and a test names the value it
        // supplied with `secret NAME = "..."`, so it holds what went out. Redaction is
        // about a *runtime's* observable output; see `docs/testing.md`.
        let sent = wire.body.clone();
        let Some((status, body)) = self.send(builtin, wire, shown) else {
            // Rule 16: for a webhook the url *is* the credential, so the first outage
            // would otherwise publish it through `/status`, `/admin` and `tracing`.
            return Err(Error::new(ErrorKind::Unreachable(shown_url.to_string())));
        };
        // One entry per logical call, so the retries rule 5 absorbed do not show up as
        // calls a test has to expect.
        self.trace.push(Effectful::Http {
            verb: builtin.name(),
            url: url.to_string(),
            body: sent,
        });
        self.record(
            &call,
            ordinal,
            Recorded::Response {
                status,
                body: body.clone(),
            },
        )?;
        Ok(Value::Response { status, body })
    }

    /// Rule 6. The command really runs, so `bind_params` revalidates the input against
    /// the signature that is loaded now, which is rule 7's runtime half.
    fn invoke(&mut self, command: &Ident, args: BTreeMap<Ident, Value>) -> Result<Value, Error> {
        let rendered = Json::Obj(
            args.iter()
                .map(|(name, value)| (name.clone(), Json::from_value(value)))
                .collect(),
        );
        let call = format!("invoke {command} {rendered}");
        let (ordinal, found) = self.recorded(&call)?;
        if let Some(Recorded::Invoked(outcome)) = found {
            return Ok(Value::Invoked(outcome));
        }

        let target = self
            .program
            .command(command)
            .ok_or_else(|| Error::new(ErrorKind::UnknownCommand(command.clone())))?;
        self.trace.push(Effectful::Invoke {
            command: command.clone(),
            args: args.clone(),
        });
        // One attempt, deliberately. A conflict here wedges the invocation, and rule 4
        // replays it from the journal, so the retry that matters already exists one
        // level up and is the one that also re-reads what the arm decided on.
        let execution = execute(self.program, self.host, target, args, &mut |_| false)?;
        // The cut: `Conflict` and `Unavailable` are the runtime's, and
        // `AlreadyCommitted` is indistinguishable from `Ok` from here, as it should be.
        let outcome = match execution.outcome {
            Outcome::Ok(_) => Invoked::Ok,
            Outcome::Invalid(message) => Invoked::Invalid(message),
            Outcome::Reject { code, message } => Invoked::Reject { code, message },
        };
        self.record(&call, ordinal, Recorded::Invoked(outcome.clone()))?;
        Ok(Value::Invoked(outcome))
    }

    /// One deployment credential, asked of the host each time rather than resolved once,
    /// so where they come from stays a host's business. Unjournaled, like `reveal`: the
    /// value is not a fact about the world that happened, and a replay after a rotation
    /// should send the new one.
    fn secret(&self, name: &str) -> Option<Arc<str>> {
        self.host.secret(name)
    }

    /// Rule 12. Not journaled, so it re-runs on every attempt, which is exactly why
    /// rule 9 forbids reaching one after an `erase`.
    ///
    /// The field, the subject and the id ride on the value; `ty` comes from the
    /// `reveal` node, because a seal holds text and only the declaration says what that
    /// text was.
    ///
    /// An absent optional comes back as it went in **without** consulting the key store:
    /// it was never encrypted, and "never set" and "key destroyed" are different facts
    /// that must not collapse.
    fn reveal(&mut self, value: Value, ty: &Type, span: Span) -> Result<Value, Error> {
        match value {
            Value::Sealed {
                field,
                subject,
                id,
                content,
            } => {
                // The one call, and the one place a key is used. `None` is the key being
                // gone, which is an outcome rule 12 names rather than a host failure.
                let Some(plaintext) = self.host.decrypt(&subject, &id, &field, &content)? else {
                    return Err(Error::at(ErrorKind::Erased { field, subject, id }, span));
                };
                Value::from_sealed(&plaintext, ty, value::Defs::of(self.program))
                    .map_err(|why| Error::at(ErrorKind::Mismatch(why), span))
            }
            Value::Opt {
                inner,
                value: Some(held),
            } => Ok(Value::Opt {
                inner: inner.unsealed(),
                value: Some(Box::new(self.reveal(*held, ty, span)?)),
            }),
            // Not sealed at all. The parser rejects that, so this is a pass-through
            // rather than a case with meaning.
            other => Ok(other),
        }
    }

    fn erase(&mut self, subject: &Ident, id: &str) -> Result<(), Error> {
        let call = format!("erase {subject}={id}");
        let (ordinal, found) = self.recorded(&call)?;
        if found.is_some() {
            return Ok(());
        }
        self.host.erase(subject, id)?;
        self.trace.push(Effectful::Erase {
            subject: subject.clone(),
            id: id.to_string(),
        });
        self.record(&call, ordinal, Recorded::Erased)?;
        Ok(())
    }

    /// Rule 5: a retryable status or a transport error is absorbed and retried with the
    /// same request, so only a decidable result reaches the handler. `None` is every
    /// attempt retryable, which wedges.
    fn send(&mut self, builtin: Builtin, wire: Parts, shown: Parts) -> Option<(i64, Json)> {
        // Both renderings arrive already built, with the `Json::Secret` leaves an object
        // literal produced already taken off by `wire` and `shown`, so a host is handed
        // two ordinary documents and never meets the variant. They are identical
        // whenever the program holds no secret, which is why `Request` makes a host name
        // the one it wants rather than leaving it to a convention.
        let request = Request {
            verb: builtin.name(),
            wire,
            shown,
        };
        for _ in 0..ATTEMPTS {
            self.traffic.performed += 1;
            self.traffic.sent.push(request.clone());
            match self.host.send(&request) {
                Attempt::Response { status, body } if !is_retryable(status) => {
                    return Some((i64::from(status), body));
                }
                _ => self.traffic.absorbed += 1,
            }
        }
        None
    }
}

/// One thing an effect did to the world. `docs/testing.md` rule 7: an effect's output
/// is this list and nothing else, which is what makes an expectation checkable rather
/// than approximate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effectful {
    Http {
        verb: &'static str,
        url: String,
        body: Option<Json>,
    },
    Invoke {
        command: Ident,
        args: BTreeMap<Ident, Value>,
    },
    Erase {
        subject: Ident,
        id: String,
    },
    Log(String),
    /// Rule 4 of `docs/effects.md`: the author's terminal outcome.
    Failed(String),
    /// Rule 12: a shredded key, terminal and counted apart from a wedge.
    Skipped(String),
}

/// One value out of a declaration that has values and no statements, evaluated against
/// an empty frame with no sink. `docs/testing.md` rule 10: a test reaches nothing a
/// program could not, and having no sink is what makes that structural.
pub fn eval_pure(
    program: &Program,
    exprs: &Exprs,
    frame: usize,
    id: ExprId,
) -> Result<Value, Error> {
    let mut frame = Frame::new(frame);
    eval(program, exprs, &mut frame, id, None)
}

/// A key as a value, for a report that names the row it could not find.
pub fn key_as_value(key: &Key) -> Value {
    key_value(key)
}
