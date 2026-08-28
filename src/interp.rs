use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::error;
use std::fmt;

use uuid::Uuid;

use crate::ir::{
    Assign, BinOp, Builtin, Command, Effect, EntityDef, EnvField, EventPath, Expr, ExprId, Exprs,
    Function, Ident, Iter, Number, Program, Projector, Return, Slice, SliceId, Slot, Span, Stmt,
    Type, UnOp,
};
use crate::scaled::{self, Rounding};
use crate::value::{self, Event, Invoked, Json, Key, Record, Value};

/// 2020-01-01T00:00:00Z, so a synthesised envelope timestamp reads as a plausible
/// instant rather than the epoch.
const EPOCH_MICROS: i64 = 1_577_836_800_000_000;
const MINUTE_MICROS: i64 = 60_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppendCondition {
    pub after: u64,
    pub slices: Vec<SliceId>,
}

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
    NoSuchPosition(u64),
    /// Rule 12: terminal rather than a wedge, because no retry recovers erased data.
    /// The message names the erase as possibly non-local, which it usually is: rule 9
    /// makes a local erase-then-reveal a compile error.
    Erased {
        field: Ident,
        subject: Ident,
        id: String,
    },
    Unreachable(String),
    BadSubject(Type),
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
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorKind::UnknownCommand(name) => write!(f, "unknown command `{name}`"),
            ErrorKind::UnknownProjector(name) => write!(f, "unknown projector `{name}`"),
            ErrorKind::UnknownEffect(name) => write!(f, "unknown effect `{name}`"),
            ErrorKind::NoSuchPosition(position) => write!(f, "no event at position {position}"),
            ErrorKind::Erased { field, subject, id } => write!(
                f,
                "reveal cannot decrypt `{field}`: subject `{subject}` = `{id}` has been erased. \
                 The erase need not be in this effect; another effect or a concurrent invocation \
                 can erase a subject between the original run and a replay, and nothing static \
                 catches that"
            ),
            ErrorKind::Unreachable(url) => {
                write!(f, "{url} did not answer; every attempt was retryable")
            }
            ErrorKind::BadSubject(ty) => write!(f, "a {ty} cannot identify a subject"),
            ErrorKind::BadUuid(value) => write!(f, "`{value}` is not a uuid"),
            ErrorKind::NoSuchField { ty, field } => write!(f, "no field `{field}` on {ty}"),
            ErrorKind::Cascade { effect, events } => write!(
                f,
                "effect `{effect}` kept producing events without settling ({}); \
                 the self-trigger check should have rejected this",
                events.join(" -> ")
            ),
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
        }
    }
}

impl error::Error for ErrorKind {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error {
    pub kind: ErrorKind,
    pub span: Option<Span>,
    /// Stamped at the `run` / `project` boundary, which is the innermost place that
    /// knows which module the running declaration came from.
    pub module: Option<String>,
}

impl Error {
    pub fn new(kind: ErrorKind) -> Self {
        Self {
            kind,
            span: None,
            module: None,
        }
    }

    pub fn at(kind: ErrorKind, span: Span) -> Self {
        Self {
            kind,
            span: Some(span),
            module: None,
        }
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
        match (&self.module, self.span) {
            (Some(module), Some(span)) => write!(f, "{module}:{span}: {}", self.kind),
            (None, Some(span)) => write!(f, "{span}: {}", self.kind),
            _ => write!(f, "{}", self.kind),
        }
    }
}

impl error::Error for Error {}

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

#[derive(Debug)]
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
    entities: BTreeMap<Ident, BTreeMap<Key, Row>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Row(pub BTreeMap<Ident, Value>);

impl Row {
    pub fn field(&self, name: &str) -> Option<&Value> {
        self.0.get(name)
    }
}

impl Store {
    pub fn get(&self, entity: &str, key: &Key) -> Option<&Row> {
        self.entities.get(entity)?.get(key)
    }

    pub fn rows(&self, entity: &str) -> impl Iterator<Item = (&Key, &Row)> {
        self.entities.get(entity).into_iter().flatten()
    }

    pub fn len(&self, entity: &str) -> usize {
        self.entities.get(entity).map_or(0, BTreeMap::len)
    }

    pub fn is_empty(&self, entity: &str) -> bool {
        self.len(entity) == 0
    }

    fn put(&mut self, entity: &Ident, key: Key, row: Row) {
        self.entities
            .entry(entity.clone())
            .or_default()
            .insert(key, row);
    }

    fn remove(&mut self, entity: &str, key: &Key) {
        if let Some(rows) = self.entities.get_mut(entity) {
            rows.remove(key);
        }
    }
}

pub struct Interpreter<'a> {
    program: &'a Program,
    log: Vec<Record>,
    /// The key store, modelled as a lifecycle: a subject is erased or it is not. That
    /// is what rules 9 and 12 turn on. Ciphertext is not modelled; see `docs/effects.md`.
    erased: BTreeSet<(Ident, String)>,
    http: Http,
    lines: Vec<String>,
}

impl<'a> Interpreter<'a> {
    pub fn new(program: &'a Program) -> Self {
        Self {
            program,
            log: Vec::new(),
            erased: BTreeSet::new(),
            http: Http::default(),
            lines: Vec::new(),
        }
    }

    pub fn with_log(program: &'a Program, log: impl IntoIterator<Item = Event>) -> Self {
        let mut interpreter = Self::new(program);
        for event in log {
            interpreter.append(event);
        }
        interpreter
    }

    pub fn log(&self) -> &[Record] {
        &self.log
    }

    /// Appends with a synthesised envelope. The id and timestamp are derived from the
    /// position so a run is reproducible; a real host stamps its own.
    pub fn append(&mut self, event: Event) {
        append(&mut self.log, event);
    }

    /// Queues the replies one URL will answer with, as hekla's own test harness does.
    pub fn script(&mut self, url: &str, replies: impl IntoIterator<Item = Reply>) {
        self.http.script(url, replies);
    }

    /// Marks a subject erased without an effect having done it, which is the case rule
    /// 12's message is about: the erase is usually not local.
    pub fn erase_subject(&mut self, subject: &str, id: &str) {
        self.erased.insert((subject.to_string(), id.to_string()));
    }

    /// HTTP calls actually performed, so a replay can be shown not re-firing them.
    /// Every request that actually left, including the attempts rule 5 absorbed.
    pub fn requests(&self) -> &[Request] {
        &self.http.sent
    }

    pub fn http_calls(&self) -> usize {
        self.http.performed
    }

    /// Retryable responses the runtime absorbed, which the handler never saw (rule 5).
    pub fn absorbed(&self) -> usize {
        self.http.absorbed
    }

    /// `log` output. Not journaled (rule 10), so a replay adds to it again.
    pub fn lines(&self) -> &[String] {
        &self.lines
    }

    /// Delivers one position to one effect. A journal carried across two calls is what
    /// makes the second a replay: journaled calls return their recorded result and are
    /// not performed again, while `reveal` and `log` run every time.
    pub fn deliver(
        &mut self,
        effect: &str,
        position: u64,
        journal: &mut Journal,
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
        journal: &mut Journal,
    ) -> Result<Invocation, Error> {
        let Some(record) = self.log.get(position as usize).cloned() else {
            return Err(ErrorKind::NoSuchPosition(position).into());
        };
        // Rule 1: one event selects exactly one arm, so this is a lookup.
        let Some(arm) = effect.arm(&record.event.path) else {
            return Ok(Invocation::Ignored);
        };

        let program = self.program;
        let mut frame = Frame::new(arm.frame);
        for bind in &arm.binds {
            let value = field(&record.event, &bind.field)?.clone();
            frame.set(bind.slot, value)?;
        }
        for bind in &arm.envelope {
            frame.set(bind.slot, envelope_value(&record, bind.field))?;
        }

        run_assigns(program, &arm.exprs, &arm.prologue, &mut frame)?;
        let filters = resolve_filters(program, &arm.exprs, &arm.slices, &mut frame)?;
        for state in &arm.states {
            let value = eval(program, &arm.exprs, &mut frame, state.init, None)?;
            frame.set(state.slot, coerce(value, &state.ty))?;
        }
        // Rule 3: the fold stops at the trigger's own position, inclusive, so state is
        // a pure function of the log prefix and that position, and counts the trigger.
        let prefix = &self.log[..=position as usize];
        fold(
            program,
            &arm.exprs,
            &arm.slices,
            &filters,
            prefix,
            &mut frame,
        )?;

        let mut ctx = Effects {
            program: self.program,
            log: &mut self.log,
            journal,
            http: &mut self.http,
            erased: &mut self.erased,
            lines: &mut self.lines,
            used: BTreeMap::new(),
        };
        if let Some(slot) = arm.now {
            let at = ctx.now();
            frame.set(slot, Value::Timestamp(at))?;
        }

        let mut sink = Sink::Effect(ctx);
        let flow = exec_block(&arm.exprs, &arm.body, &mut frame, self.program, &mut sink);
        match flow {
            Ok(Flow::Return(Ret::Fail(message))) => Ok(Invocation::Failed(message)),
            Ok(_) => Ok(Invocation::Done),
            // Rule 12: terminal, so the cursor advances and this is counted apart from
            // a wedge, which does not advance.
            Err(err) if matches!(err.kind, ErrorKind::Erased { .. }) => {
                Ok(Invocation::Skipped(err.kind.to_string()))
            }
            Err(err) => Err(err),
        }
    }

    /// Runs one effect over the log, following it as an `invoke` lengthens it. A wedge
    /// stops the walk, because a wedged invocation does not advance.
    pub fn drive(&mut self, effect: &str) -> Result<Counts, Error> {
        let mut counts = Counts::default();
        let start = self.log.len();
        let mut position = 0u64;

        while (position as usize) < self.log.len() {
            if self.log.len() > start + CASCADE {
                let events = self.log[start..]
                    .iter()
                    .map(|record| record.event.path.to_string())
                    .collect();
                return Err(ErrorKind::Cascade {
                    effect: effect.to_string(),
                    events,
                }
                .into());
            }

            // One journal per invocation: it is the memory of this position's calls,
            // and nothing carries between positions.
            let mut journal = Journal::default();
            match self.deliver(effect, position, &mut journal) {
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

    /// Folds the whole log into this projector's read models. Each handler gets a
    /// fresh frame, which is what makes "handlers do not share state" structural.
    pub fn project(&self, name: &str) -> Result<Store, Error> {
        let projector = self
            .program
            .projector(name)
            .ok_or_else(|| ErrorKind::UnknownProjector(name.to_string()))?;
        self.fold_into(projector)
            .map_err(|err| err.in_module(projector.module.as_deref()))
    }

    fn fold_into(&self, projector: &Projector) -> Result<Store, Error> {
        let mut store = Store::default();
        for record in &self.log {
            for handler in &projector.handlers {
                if handler.event != record.event.path {
                    continue;
                }

                let mut frame = Frame::new(handler.frame);
                for bind in &handler.binds {
                    let value = field(&record.event, &bind.field)?.clone();
                    frame.set(bind.slot, value)?;
                }
                for bind in &handler.envelope {
                    frame.set(bind.slot, envelope_value(record, bind.field))?;
                }

                let mut sink = Sink::Write {
                    projector,
                    store: &mut store,
                };
                exec_block(
                    &handler.exprs,
                    &handler.body,
                    &mut frame,
                    self.program,
                    &mut sink,
                )?;
            }
        }
        Ok(store)
    }

    pub fn run(
        &mut self,
        name: &str,
        args: impl IntoIterator<Item = (impl Into<Ident>, Value)>,
    ) -> Result<Execution, Error> {
        let program = self.program;
        let command = program
            .command(name)
            .ok_or_else(|| ErrorKind::UnknownCommand(name.to_string()))?;
        let module = command.module.as_deref();

        // Every failure below is inside this command, so the module is stamped once here
        // rather than at each raise site.
        execute(self.program, &mut self.log, command, args).map_err(|err| err.in_module(module))
    }
}

/// Appends with a synthesised envelope, derived from the position so a run is
/// reproducible. Free rather than a method, so an effect's `invoke` can append too.
fn append(log: &mut Vec<Record>, event: Event) {
    let position = log.len() as u64;
    log.push(Record::new(
        format!("0190d1a1-0000-7000-9000-{position:012}"),
        position,
        EPOCH_MICROS + position as i64 * MINUTE_MICROS,
        event,
    ));
}

fn execute(
    program: &Program,
    log: &mut Vec<Record>,
    command: &Command,
    args: impl IntoIterator<Item = (impl Into<Ident>, Value)>,
) -> Result<Execution, Error> {
    let mut frame = Frame::new(command.frame);
    // Rule 11: the request's append time, pinned once before anything runs, so it is
    // well defined even for a command that goes on to append nothing.
    if let Some(slot) = command.now {
        let at = EPOCH_MICROS + log.len() as i64 * MINUTE_MICROS;
        frame.set(slot, Value::Timestamp(at))?;
    }

    let mut args: BTreeMap<Ident, Value> = args
        .into_iter()
        .map(|(name, value)| (name.into(), value))
        .collect();
    bind_params(command, &mut args, &mut frame)?;
    run_assigns(program, &command.exprs, &command.prologue, &mut frame)?;

    let filters = resolve_filters(program, &command.exprs, &command.slices, &mut frame)?;
    for state in &command.states {
        let value = eval(program, &command.exprs, &mut frame, state.init, None)?;
        frame.set(state.slot, coerce(value, &state.ty))?;
    }

    let after = log.len() as u64;
    fold(
        program,
        &command.exprs,
        &command.slices,
        &filters,
        log,
        &mut frame,
    )?;

    let mut emitted = Vec::new();
    let ret = {
        let mut sink = Sink::Emit(&mut emitted);
        match exec_block(
            &command.exprs,
            &command.body,
            &mut frame,
            program,
            &mut sink,
        )? {
            Flow::Return(ret) => ret,
            Flow::Next => Ret::Ok,
        }
    };

    let condition = AppendCondition {
        after,
        slices: (0..command.slices.len())
            .map(|index| SliceId(index as u32))
            .collect(),
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
    if let Outcome::Ok(events) = &outcome {
        for event in events {
            append(log, event.clone());
        }
    }

    Ok(Execution { outcome, condition })
}

fn bind_params(
    command: &Command,
    args: &mut BTreeMap<Ident, Value>,
    frame: &mut Frame,
) -> Result<(), Error> {
    for param in &command.params {
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

fn run_assigns(
    program: &Program,
    exprs: &Exprs,
    assigns: &[Assign],
    frame: &mut Frame,
) -> Result<(), Error> {
    for assign in assigns {
        let value = eval(program, exprs, frame, assign.value, None)?;
        frame.set(assign.slot, value)?;
    }
    Ok(())
}

fn resolve_filters(
    program: &Program,
    exprs: &Exprs,
    slices: &[Slice],
    frame: &mut Frame,
) -> Result<Vec<Vec<Value>>, Error> {
    slices
        .iter()
        .map(|slice| {
            slice
                .filters
                .iter()
                .map(|filter| eval(program, exprs, frame, filter.value, None))
                .collect()
        })
        .collect()
}

fn fold(
    program: &Program,
    exprs: &Exprs,
    slices: &[Slice],
    filters: &[Vec<Value>],
    log: &[Record],
    frame: &mut Frame,
) -> Result<(), Error> {
    for record in log {
        let event = &record.event;
        for (index, slice) in slices.iter().enumerate() {
            if slice.event != event.path || !matches(slice, &filters[index], event)? {
                continue;
            }

            for bind in &slice.binds {
                let value = field(event, &bind.field)?.clone();
                frame.set(bind.slot, value)?;
            }
            for update in &slice.updates {
                let value = eval(program, exprs, frame, update.value, None)?;
                frame.set(update.slot, coerce(value, &update.ty))?;
            }
        }
    }
    Ok(())
}

fn matches(slice: &Slice, filters: &[Value], event: &Event) -> Result<bool, Error> {
    for (filter, expected) in slice.filters.iter().zip(filters) {
        if field(event, &filter.field)? != expected {
            return Ok(false);
        }
    }
    Ok(true)
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
        store: &'a mut Store,
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
                if let Some(fault) = check_field(&declared.ty, declared.max_len, name, &value, at)?
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
            let (projector, store) = write_sink(sink, *span)?;
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
            store.put(entity, key, row);
            Ok(Flow::Next)
        }
        Stmt::Patch {
            entity,
            key,
            loads,
            fields,
            span,
        } => {
            let key_value = eval(program, exprs, frame, *key, None)?;
            let (projector, store) = write_sink(sink, *span)?;
            let def = entity_def(projector, entity, *span)?;
            let key = key_of(&key_value, exprs.span(*key))?;

            // Rule 5: a missing row materializes from zeros, so a patch always has
            // a prior value for `.field` to read.
            let mut row = match store.get(entity, &key) {
                Some(row) => row.clone(),
                None => materialize(def, projector, program, &key, *span)?,
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

            store.put(entity, key, row);
            Ok(Flow::Next)
        }
        Stmt::Delete { entity, key } => {
            let span = exprs.span(*key);
            let key_value = eval(program, exprs, frame, *key, None)?;
            let (projector, store) = write_sink(sink, span)?;
            entity_def(projector, entity, span)?;
            let key = key_of(&key_value, span)?;
            store.remove(entity, &key);
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
                Sink::Effect(ctx) => ctx.lines.push(message),
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
            let id = subject_id(&value, *span)?;
            match sink {
                Sink::Effect(ctx) => ctx.erase(subject, &id),
                _ => return Err(Error::at(ErrorKind::MalformedIr, *span)),
            }
            Ok(Flow::Next)
        }
        Stmt::Discard(value) => {
            eval(program, exprs, frame, *value, effects(sink))?;
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
            };
            Ok(Flow::Return(ret))
        }
    }
}

/// A subject is identified by a plaintext scalar, which is what `erase` and `reveal`
/// look the key up by.
fn subject_id(value: &Value, span: Span) -> Result<String, Error> {
    match value {
        Value::Int(id) => Ok(id.to_string()),
        Value::Str(id) | Value::Uuid(id) => Ok(id.clone()),
        other => Err(Error::at(ErrorKind::BadSubject(other.ty()), span)),
    }
}

fn write_sink<'s, 'a>(
    sink: &'s mut Sink<'a>,
    span: Span,
) -> Result<(&'a Projector, &'s mut Store), Error> {
    match sink {
        Sink::Write { projector, store } => Ok((projector, store)),
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
    if let Some(fault) = check_field(&declared.ty, declared.max_len, name, &value, at)? {
        return Err(Error::at(fault, at));
    }
    Ok(value)
}

/// Type check, then length check. A type mismatch is always an error; the caller
/// decides what an over-length value means, which is the one place commands and
/// projectors differ.
/// A bare `T` written into a `T?` field wraps, the same coercion `bind_params`
/// already applies to command arguments.
fn coerce(value: Value, ty: &Type) -> Value {
    match ty {
        Type::Opt(inner) if value.has_type(inner) => Value::some(value),
        _ => value,
    }
}

fn check_field(
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
    if let (Some(max), Value::Str(text)) = (max_len, value) {
        let len = text.chars().count();
        if len > max {
            return Ok(Some(ErrorKind::TooLong {
                field: name.clone(),
                len,
                max,
            }));
        }
    }
    Ok(None)
}

fn key_of(value: &Value, span: Span) -> Result<Key, Error> {
    Key::from_value(value).ok_or_else(|| Error::at(ErrorKind::BadKey(value.ty()), span))
}

fn row_key(def: &EntityDef, row: &Row, span: Span) -> Result<Key, Error> {
    let name = &def.key_field().name;
    let value = row.0.get(name).ok_or_else(|| {
        Error::at(
            ErrorKind::MissingEntityField {
                entity: def.name.clone(),
                field: name.clone(),
            },
            span,
        )
    })?;
    key_of(value, span)
}

fn materialize(
    def: &EntityDef,
    projector: &Projector,
    program: &Program,
    key: &Key,
    span: Span,
) -> Result<Row, Error> {
    let defs = value::Defs {
        local: &projector.enums,
        enums: &program.enums,
        records: &program.records,
    };
    let mut row = Row::default();
    for field in &def.fields {
        let value = if field.name == def.key_field().name {
            key_value(key)
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
            if ctx.is_none() {
                return Err(at(ErrorKind::MalformedIr));
            }
            let mut object = BTreeMap::new();
            for (name, value) in fields {
                let value = eval(program, exprs, frame, *value, ctx.as_deref_mut())?;
                object.insert(name.clone(), Json::from_value(&value));
            }
            Ok(Value::Json(Json::Obj(object)))
        }
        Expr::List(items) => {
            let mut values = Vec::new();
            for item in items {
                values.push(eval(program, exprs, frame, *item, ctx.as_deref_mut())?);
            }
            let inner = values.first().map_or(Type::Json, Value::ty);
            Ok(Value::List {
                inner,
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
                inner.get_or_insert_with(|| value.ty());
                items.push(value);
            }
            Ok(Value::List {
                inner: inner.unwrap_or(Type::Json),
                items,
            })
        }
        Expr::CallFn { function, args } => {
            let mut values = Vec::new();
            for arg in args {
                values.push(eval(program, exprs, frame, *arg, ctx.as_deref_mut())?);
            }
            let def = program
                .function(function)
                .ok_or_else(|| at(ErrorKind::UnknownFunction(function.clone())))?;
            call_function(program, def, values, span)
        }
        Expr::Record { ty, fields } => {
            let mut values = BTreeMap::new();
            for (name, value) in fields {
                let value = eval(program, exprs, frame, *value, ctx.as_deref_mut())?;
                values.insert(name.clone(), value);
            }
            Ok(Value::Record {
                ty: ty.clone(),
                fields: values,
            })
        }
        Expr::Interp(parts) => {
            let mut text = String::new();
            for part in parts {
                let value = eval(program, exprs, frame, *part, ctx.as_deref_mut())?;
                text.push_str(&value::text(&value));
            }
            Ok(Value::Str(text))
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
                    return Ok(Value::Str(Json::from_value(value).to_string()));
                }
                Builtin::TimestampParse | Builtin::MoneyParse(_) => {
                    let Some(Value::Str(text)) = values.first() else {
                        return Err(at(ErrorKind::MalformedIr));
                    };
                    return Ok(match builtin {
                        Builtin::TimestampParse => match parse_timestamp(text) {
                            Some(micros) => Value::some(Value::Timestamp(micros)),
                            None => Value::none(Type::Timestamp),
                        },
                        Builtin::MoneyParse(scale) => match parse_money(text, *scale) {
                            Some(value) => Value::some(value),
                            None => Value::none(Type::Money(*scale)),
                        },
                        _ => return Err(at(ErrorKind::MalformedIr)),
                    });
                }
                _ => {}
            }
            let Some(ctx) = ctx else {
                return Err(at(ErrorKind::MalformedIr));
            };
            let Some(Value::Str(url)) = values.first().cloned() else {
                return Err(at(ErrorKind::MalformedIr));
            };
            // The headers are the last argument and always present, so the shape is
            // (url, headers) or (url, body, headers).
            let headers = values.last().map(Json::from_value).unwrap_or(Json::Null);
            let body = if builtin.has_body() {
                values.get(1).map(Json::from_value)
            } else {
                None
            };
            ctx.http(*builtin, &url, body, headers).map_err(at)
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
        Expr::Reveal {
            value,
            field,
            subject,
            subject_value,
        } => {
            let plaintext = eval(program, exprs, frame, *value, ctx.as_deref_mut())?;
            // Rule 12: an absent value was never encrypted, so no key can be missing
            // for it. This is the row that must not collapse into the erased one.
            if matches!(plaintext, Value::Opt { value: None, .. }) {
                return Ok(plaintext);
            }
            let id = eval(program, exprs, frame, *subject_value, ctx.as_deref_mut())?;
            let id = subject_id(&id, span)?;
            let Some(ctx) = ctx else {
                return Err(at(ErrorKind::MalformedIr));
            };
            ctx.reveal(field, subject, &id, plaintext)
        }
    }
}

/// A pure call: a fresh frame, the parameters filled, and no sink to write through.
/// The parser proved every path returns, so falling out of the body is malformed IR
/// rather than a case with a value.
fn call_function(
    program: &Program,
    def: &Function,
    args: Vec<Value>,
    span: Span,
) -> Result<Value, Error> {
    let mut frame = Frame::new(def.frame);
    if args.len() != def.params.len() {
        return Err(Error::at(ErrorKind::MalformedIr, span));
    }
    for (param, value) in def.params.iter().zip(args) {
        // The same coercion `bind_params` applies, so a bare `T` fills a `T?`.
        frame
            .set(param.slot, coerce(value, &param.ty))
            .map_err(|kind| Error::at(kind, span))?;
    }
    let mut sink = Sink::Pure;
    match exec_block(&def.exprs, &def.body, &mut frame, program, &mut sink)? {
        Flow::Return(Ret::Value(value)) => Ok(coerce(value, &def.ret)),
        _ => Err(Error::at(ErrorKind::MalformedIr, span)),
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
        Value::Str(value) => Ok(value),
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
            let ordering = match (&lhs, &rhs) {
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
                _ => {
                    return Err(ErrorKind::BadOperands {
                        op,
                        lhs: lhs.ty(),
                        rhs: rhs.ty(),
                    });
                }
            };
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
            (Value::Str(a), Value::Str(b)) if op == BinOp::Add => Ok(Value::Str(format!("{a}{b}"))),
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
            Ok(Value::Str(value.trim().to_string()))
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
            Ok(Value::Str(value.to_lowercase()))
        }
        (Value::Str(value), "upper") => {
            expect_arity(method, 0, &args)?;
            Ok(Value::Str(value.to_uppercase()))
        }
        (Value::Str(value), "starts_with") => {
            expect_arity(method, 1, &args)?;
            match &args[0] {
                Value::Str(prefix) => Ok(Value::Bool(value.starts_with(prefix.as_str()))),
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
                Value::Str(prefix) => Ok(Value::str(
                    value.strip_prefix(prefix.as_str()).unwrap_or(value),
                )),
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
                    value
                        .rsplit_once(sep.as_str())
                        .map_or(value.as_str(), |(_, tail)| tail),
                )),
                Value::Str(_) => Ok(Value::str(value)),
                other => Err(ErrorKind::TypeMismatch {
                    expected: Type::String,
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
                Ok(_) => Value::some(Value::uuid(value)),
                Err(_) => Value::none(Type::Uuid),
            })
        }
        (Value::Str(value), "contains") => {
            expect_arity(method, 1, &args)?;
            match &args[0] {
                Value::Str(needle) => Ok(Value::Bool(value.contains(needle.as_str()))),
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
            items.push(args.into_iter().next().ok_or(ErrorKind::MalformedIr)?);
            Ok(Value::List {
                inner: inner.clone(),
                items,
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
            entries.insert(at, args.next().ok_or(ErrorKind::MalformedIr)?);
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
        _ => Err(ErrorKind::UnknownMethod {
            ty: receiver.ty(),
            method: method.to_string(),
        }),
    }
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
        EnvField::Id => Value::Uuid(record.id.clone()),
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
        (Json::Str(value), Type::String) => Some(Value::Str(value.clone())),
        (Json::Int(value), Type::Int) => Some(Value::Int(*value)),
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
/// RFC 3339 to epoch microseconds. Hand-rolled rather than a dependency: the shapes
/// that arrive on a webhook are a small set, and a calendar library is a large surface
/// and a large opinion for one function.
fn parse_timestamp(text: &str) -> Option<i64> {
    let digits = |part: Option<&str>| -> Option<i64> {
        let part = part?;
        if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        part.parse().ok()
    };
    let bytes = text.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    if !matches!(bytes[10], b'T' | b't' | b' ') || bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    let year = digits(text.get(0..4))?;
    let month = digits(text.get(5..7))?;
    let day = digits(text.get(8..10))?;
    let hour = digits(text.get(11..13))?;
    let minute = digits(text.get(14..16))?;
    let second = digits(text.get(17..19))?;
    if !(1..=12).contains(&month)
        || day < 1
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }

    let mut rest = &text[19..];
    let mut fraction = 0i64;
    if let Some(tail) = rest.strip_prefix('.') {
        let written: String = tail.chars().take_while(char::is_ascii_digit).collect();
        if written.is_empty() {
            return None;
        }
        rest = &tail[written.len()..];
        let mut micros = written.clone();
        micros.truncate(6);
        while micros.len() < 6 {
            micros.push('0');
        }
        fraction = digits(Some(&micros))?;
    }

    // A local time with no offset is not RFC 3339, and guessing one is how a warranty
    // ends up expiring on the wrong day.
    let offset = match rest {
        "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes().first() {
                Some(b'+') => 1,
                Some(b'-') => -1,
                _ => return None,
            };
            if rest.len() != 6 || rest.as_bytes()[3] != b':' {
                return None;
            }
            let hours = digits(rest.get(1..3))?;
            let minutes = digits(rest.get(4..6))?;
            if hours > 23 || minutes > 59 {
                return None;
            }
            sign * (hours * 3600 + minutes * 60)
        }
    };

    let seconds = days_from_civil(year, month, day)
        .checked_mul(86_400)?
        .checked_add(hour * 3600 + minute * 60 + second - offset)?;
    seconds.checked_mul(1_000_000)?.checked_add(fraction)
}

/// Days since 1970-01-01, by Howard Hinnant's civil-calendar algorithm.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 => 29,
        2 => 28,
        _ => 0,
    }
}

/// A decimal string at the target scale, by exactly the rule a written literal follows:
/// widening is exact, and more places than the target holds is a failure rather than a
/// silent round.
fn parse_money(text: &str, scale: u8) -> Option<Value> {
    let (negative, rest) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (whole, fraction) = rest.split_once('.').unwrap_or((rest, ""));
    if whole.is_empty() || !whole.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    if rest.contains('.') && fraction.is_empty() {
        return None;
    }
    if !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let mut written = String::from(whole);
    written.push_str(fraction);
    let value: i128 = written.parse().ok()?;
    let value = if negative { -value } else { value };
    let places = u8::try_from(fraction.len()).ok()?;
    let lit = Number::new(value, places)
        .resolve(&Type::Money(scale))
        .ok()?;
    Some(value::literal(&lit))
}

fn uuid_derive(args: &[Value]) -> Result<Value, ErrorKind> {
    let (Some(Value::Uuid(seed)), Some(Value::Str(name))) = (args.first(), args.get(1)) else {
        return Err(ErrorKind::BadArgument {
            method: "Uuid.derive".to_string(),
            expected: "a Uuid seed and a String name",
            found: args.first().map_or(Type::Uuid, Value::ty),
        });
    };
    let parsed = Uuid::parse_str(seed).map_err(|_| ErrorKind::BadUuid(seed.clone()))?;
    Ok(Value::uuid(
        Uuid::new_v5(&parsed, name.as_bytes()).to_string(),
    ))
}

/// How many events one `drive` will follow past where it started. A backstop only: the
/// parser's self-trigger check is what actually makes the walk terminate, so tripping
/// this reports a hole in that check rather than an expected limit.
const CASCADE: usize = 16;

/// How many attempts the runtime makes before a call wedges. Retryable statuses and
/// transport errors are absorbed here, so the handler never sees one (rule 5).
const ATTEMPTS: usize = 4;

/// A recorded impure call. `reveal` and `log` are absent, which is rule 10's
/// unjournaled set being a property of the type rather than a marker in the syntax.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recorded {
    Response { status: i64, body: Json },
    Invoked(Invoked),
    Now(i64),
    Erased,
}

/// Durable execution's memory: an impure call looks itself up here first and performs
/// the real call only when nothing is recorded. The key describes the call, plus an
/// ordinal for repeated identical calls, which is hekla's content-hash-and-
/// disambiguator scheme with a key that prints. A real host hashes it.
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

/// Scripted HTTP, as hekla's own test harness does it.
#[derive(Debug, Clone, Default)]
pub struct Http {
    scripted: BTreeMap<String, VecDeque<Reply>>,
    performed: usize,
    absorbed: usize,
    sent: Vec<Request>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    Status(u16),
    Body(u16, Json),
    Transport(String),
}

impl Http {
    fn script(&mut self, url: &str, replies: impl IntoIterator<Item = Reply>) {
        self.scripted
            .entry(url.to_string())
            .or_default()
            .extend(replies);
    }

    /// The terminal response, or `None` when every attempt was retryable, which wedges.
    /// Rule 5 lives here: a retryable status or a transport error is absorbed and
    /// retried with the same request, so only a decidable result reaches the handler.
    fn call(
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

/// What one delivery came to. `Ignored` is not an outcome: no arm selected the event,
/// so there was no invocation to have one.
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
    log: &'a mut Vec<Record>,
    journal: &'a mut Journal,
    http: &'a mut Http,
    erased: &'a mut BTreeSet<(Ident, String)>,
    lines: &'a mut Vec<String>,
    /// How many times each call has been made so far in this invocation, so a repeated
    /// identical call lines up with its own recording rather than the first one's.
    used: BTreeMap<String, u32>,
}

impl Effects<'_> {
    fn recorded(&mut self, call: &str) -> (u32, Option<Recorded>) {
        let counter = self.used.entry(call.to_string()).or_insert(0);
        let ordinal = *counter;
        *counter += 1;
        let found = self
            .journal
            .entries
            .get(&(call.to_string(), ordinal))
            .cloned();
        (ordinal, found)
    }

    fn record(&mut self, call: &str, ordinal: u32, recorded: Recorded) {
        self.journal
            .entries
            .insert((call.to_string(), ordinal), recorded);
    }

    /// Rule 11: journaled, and pinned once per invocation rather than once per call,
    /// which is where this diverges from hekla.
    fn now(&mut self) -> i64 {
        let (ordinal, found) = self.recorded("now()");
        if let Some(Recorded::Now(at)) = found {
            return at;
        }
        let at = EPOCH_MICROS + self.log.len() as i64 * MINUTE_MICROS;
        self.record("now()", ordinal, Recorded::Now(at));
        at
    }

    /// The journal key is the verb, the URL and the body, and deliberately **not** the
    /// headers. A changed idempotency key must land on the same entry, or a replay
    /// would re-send the request it was written to suppress.
    fn http(
        &mut self,
        builtin: Builtin,
        url: &str,
        body: Option<Json>,
        headers: Json,
    ) -> Result<Value, ErrorKind> {
        let call = match &body {
            Some(body) => format!("{} {url} {body}", builtin.name()),
            None => format!("{} {url}", builtin.name()),
        };
        let (ordinal, found) = self.recorded(&call);
        if let Some(Recorded::Response { status, body }) = found {
            return Ok(Value::Response { status, body });
        }

        let Some((status, body)) = self.http.call(builtin, url, body.clone(), headers) else {
            return Err(ErrorKind::Unreachable(url.to_string()));
        };
        self.record(
            &call,
            ordinal,
            Recorded::Response {
                status,
                body: body.clone(),
            },
        );
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
        let (ordinal, found) = self.recorded(&call);
        if let Some(Recorded::Invoked(outcome)) = found {
            return Ok(Value::Invoked(outcome));
        }

        let target = self
            .program
            .command(command)
            .ok_or_else(|| Error::new(ErrorKind::UnknownCommand(command.clone())))?;
        let execution = execute(self.program, self.log, target, args)?;
        // The cut: `Conflict` and `Unavailable` are the runtime's, and
        // `AlreadyCommitted` is indistinguishable from `Ok` from here, as it should be.
        let outcome = match execution.outcome {
            Outcome::Ok(_) => Invoked::Ok,
            Outcome::Invalid(message) => Invoked::Invalid(message),
            Outcome::Reject { code, message } => Invoked::Reject { code, message },
        };
        self.record(&call, ordinal, Recorded::Invoked(outcome.clone()));
        Ok(Value::Invoked(outcome))
    }

    /// Rule 12. Not journaled, so it re-runs on every attempt, which is exactly why
    /// rule 9 forbids reaching one after an `erase`.
    fn reveal(
        &mut self,
        field: &Ident,
        subject: &Ident,
        id: &str,
        plaintext: Value,
    ) -> Result<Value, Error> {
        if self.erased.contains(&(subject.clone(), id.to_string())) {
            return Err(ErrorKind::Erased {
                field: field.clone(),
                subject: subject.clone(),
                id: id.to_string(),
            }
            .into());
        }
        Ok(plaintext)
    }

    fn erase(&mut self, subject: &Ident, id: &str) {
        let call = format!("erase {subject}={id}");
        let (ordinal, found) = self.recorded(&call);
        if found.is_some() {
            return;
        }
        self.erased.insert((subject.clone(), id.to_string()));
        self.record(&call, ordinal, Recorded::Erased);
    }
}
