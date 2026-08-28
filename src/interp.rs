use std::collections::BTreeMap;
use std::error;
use std::fmt;

use crate::currency::Currency;
use crate::ir::{
    Assign, BinOp, Command, EntityDef, EnvField, EventPath, Expr, ExprId, Exprs, Ident, Program,
    Projector, Return, Slice, SliceId, Slot, Span, Stmt, Type, UnOp,
};
use crate::scaled::{self, Rounding};
use crate::value::{self, Event, Key, Record, Value};

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
    DivisionByZero,
    Overflow,
    Inexact,
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorKind::UnknownCommand(name) => write!(f, "unknown command `{name}`"),
            ErrorKind::UnknownProjector(name) => write!(f, "unknown projector `{name}`"),
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
    Invalid(String),
    Reject { code: String, message: String },
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
}

impl<'a> Interpreter<'a> {
    pub fn new(program: &'a Program) -> Self {
        Self {
            program,
            log: Vec::new(),
        }
    }

    pub fn with_log(program: &'a Program, log: impl IntoIterator<Item = Event>) -> Self {
        let mut interpreter = Self::new(program);
        for event in log {
            interpreter.append(event);
        }
        interpreter
    }

    pub fn currency(&self) -> &Currency {
        &self.program.currency
    }

    pub fn log(&self) -> &[Record] {
        &self.log
    }

    /// Appends with a synthesised envelope. The id and timestamp are derived from the
    /// position so a run is reproducible; a real host stamps its own.
    pub fn append(&mut self, event: Event) {
        let position = self.log.len() as u64;
        self.log.push(Record::new(
            format!("0190d1a1-0000-7000-9000-{position:012}"),
            position,
            EPOCH_MICROS + position as i64 * MINUTE_MICROS,
            event,
        ));
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
        self.execute(command, args)
            .map_err(|err| err.in_module(module))
    }

    fn execute(
        &mut self,
        command: &Command,
        args: impl IntoIterator<Item = (impl Into<Ident>, Value)>,
    ) -> Result<Execution, Error> {
        let program = self.program;
        let mut frame = Frame::new(command.frame);
        let mut args: BTreeMap<Ident, Value> = args
            .into_iter()
            .map(|(name, value)| (name.into(), value))
            .collect();
        bind_params(command, &mut args, &mut frame)?;
        run_assigns(&command.exprs, &command.prologue, &mut frame)?;

        let filters = resolve_filters(&command.exprs, &command.slices, &frame)?;
        for state in &command.states {
            let value = eval(&command.exprs, &frame, state.init)?;
            frame.set(state.slot, value)?;
        }

        let after = self.log.len() as u64;
        fold(command, &filters, &self.log, &mut frame)?;

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
        };

        // Emitted events are already validated at the emit site, where each field
        // still has an expression to point a span at.
        if let Outcome::Ok(events) = &outcome {
            for event in events {
                self.append(event.clone());
            }
        }

        Ok(Execution { outcome, condition })
    }
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

fn run_assigns(exprs: &Exprs, assigns: &[Assign], frame: &mut Frame) -> Result<(), Error> {
    for assign in assigns {
        let value = eval(exprs, frame, assign.value)?;
        frame.set(assign.slot, value)?;
    }
    Ok(())
}

fn resolve_filters(
    exprs: &Exprs,
    slices: &[Slice],
    frame: &Frame,
) -> Result<Vec<Vec<Value>>, Error> {
    slices
        .iter()
        .map(|slice| {
            slice
                .filters
                .iter()
                .map(|filter| eval(exprs, frame, filter.value))
                .collect()
        })
        .collect()
}

fn fold(
    command: &Command,
    filters: &[Vec<Value>],
    log: &[Record],
    frame: &mut Frame,
) -> Result<(), Error> {
    for record in log {
        let event = &record.event;
        for (index, slice) in command.slices.iter().enumerate() {
            if slice.event != event.path || !matches(slice, &filters[index], event)? {
                continue;
            }

            for bind in &slice.binds {
                let value = field(event, &bind.field)?.clone();
                frame.set(bind.slot, value)?;
            }
            for update in &slice.updates {
                let value = eval(&command.exprs, frame, update.value)?;
                frame.set(update.slot, value)?;
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
    Emit(&'a mut Vec<Event>),
    Write {
        projector: &'a Projector,
        store: &'a mut Store,
    },
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
            let value = eval(exprs, frame, *value)?;
            frame.set(*slot, value)?;
            Ok(Flow::Next)
        }
        Stmt::If {
            cond,
            then,
            otherwise,
        } => {
            let branch = if eval_bool(exprs, frame, *cond)? {
                then
            } else {
                otherwise
            };
            exec_block(exprs, branch, frame, program, sink)
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
                let value = coerce(eval(exprs, frame, *value)?, &declared.ty);
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
                let value = eval_field(exprs, frame, def, entity, name, *value)?;
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
            let key_value = eval(exprs, frame, *key)?;
            let (projector, store) = write_sink(sink, *span)?;
            let def = entity_def(projector, entity, *span)?;
            let key = key_of(&key_value, exprs.span(*key))?;

            // Rule 5: a missing row materializes from zeros, so a patch always has
            // a prior value for `.field` to read.
            let mut row = match store.get(entity, &key) {
                Some(row) => row.clone(),
                None => materialize(def, projector, &key, *span)?,
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
                let value = eval_field(exprs, frame, def, entity, name, *value)?;
                row.0.insert(name.clone(), value);
            }

            store.put(entity, key, row);
            Ok(Flow::Next)
        }
        Stmt::Delete { entity, key } => {
            let span = exprs.span(*key);
            let key_value = eval(exprs, frame, *key)?;
            let (projector, store) = write_sink(sink, span)?;
            entity_def(projector, entity, span)?;
            let key = key_of(&key_value, span)?;
            store.remove(entity, &key);
            Ok(Flow::Next)
        }
        Stmt::Return(ret) => {
            let ret = match ret {
                Return::Ok => Ret::Ok,
                Return::Invalid(message) => Ret::Invalid(eval_string(exprs, frame, *message)?),
                Return::Reject { code, message } => Ret::Reject {
                    code: eval_string(exprs, frame, *code)?,
                    message: eval_string(exprs, frame, *message)?,
                },
            };
            Ok(Flow::Return(ret))
        }
    }
}

fn write_sink<'s, 'a>(
    sink: &'s mut Sink<'a>,
    span: Span,
) -> Result<(&'a Projector, &'s mut Store), Error> {
    match sink {
        Sink::Write { projector, store } => Ok((projector, store)),
        Sink::Emit(_) => Err(Error::at(ErrorKind::MalformedIr, span)),
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
    exprs: &Exprs,
    frame: &Frame,
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
    let value = coerce(eval(exprs, frame, value)?, &declared.ty);
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
    key: &Key,
    span: Span,
) -> Result<Row, Error> {
    let mut row = Row::default();
    for field in &def.fields {
        let value = if field.name == def.key_field().name {
            key_value(key)
        } else {
            value::initial(field, &projector.enums).ok_or_else(|| {
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

fn eval(exprs: &Exprs, frame: &Frame, id: ExprId) -> Result<Value, Error> {
    let span = exprs.span(id);
    let at = |kind: ErrorKind| Error::at(kind, span);

    match exprs.get(id).ok_or_else(|| at(ErrorKind::MalformedIr))? {
        Expr::Lit(lit) => Ok(value::literal(lit)),
        Expr::Load(slot) => frame.get(*slot).cloned().map_err(at),
        Expr::Unary { op, operand } => {
            let value = eval(exprs, frame, *operand)?;
            unary(*op, value).map_err(at)
        }
        Expr::Binary { op, lhs, rhs } => match op {
            BinOp::And => {
                if eval_bool(exprs, frame, *lhs)? {
                    Ok(Value::Bool(eval_bool(exprs, frame, *rhs)?))
                } else {
                    Ok(Value::Bool(false))
                }
            }
            BinOp::Or => {
                if eval_bool(exprs, frame, *lhs)? {
                    Ok(Value::Bool(true))
                } else {
                    Ok(Value::Bool(eval_bool(exprs, frame, *rhs)?))
                }
            }
            op => {
                let lhs = eval(exprs, frame, *lhs)?;
                let rhs = eval(exprs, frame, *rhs)?;
                binary(*op, lhs, rhs).map_err(at)
            }
        },
        Expr::Method {
            receiver,
            method,
            args,
        } => {
            let receiver = eval(exprs, frame, *receiver)?;
            let args = args
                .iter()
                .map(|arg| eval(exprs, frame, *arg))
                .collect::<Result<Vec<_>, _>>()?;
            call_method(receiver, method, args).map_err(at)
        }
        Expr::If {
            cond,
            then,
            otherwise,
        } => {
            let taken = eval_bool(exprs, frame, *cond)?;
            eval(exprs, frame, if taken { *then } else { *otherwise })
        }
    }
}

fn eval_bool(exprs: &Exprs, frame: &Frame, id: ExprId) -> Result<bool, Error> {
    match eval(exprs, frame, id)? {
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

fn eval_string(exprs: &Exprs, frame: &Frame, id: ExprId) -> Result<String, Error> {
    match eval(exprs, frame, id)? {
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
        (UnOp::Neg, Value::Money(value)) => Ok(Value::Money(scaled::neg(value)?)),
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
                (Value::Money(a), Value::Money(b)) => a.cmp(b),
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
            (Value::Money(a), Value::Money(b)) if matches!(op, BinOp::Add | BinOp::Sub) => {
                arith(op, *a, *b).map(Value::Money)
            }
            (Value::Money(a), Value::Money(b)) if op == BinOp::Div => Ok(Value::Decimal {
                units: scaled::ratio(*a, *b, scaled::RATIO_SCALE)?,
                scale: scaled::RATIO_SCALE,
            }),
            (Value::Money(amount), Value::Int(factor)) if op == BinOp::Mul => {
                arith(op, *amount, *factor).map(Value::Money)
            }
            (Value::Int(factor), Value::Money(amount)) if op == BinOp::Mul => {
                arith(op, *factor, *amount).map(Value::Money)
            }
            (Value::Money(amount), Value::Int(divisor)) if op == BinOp::Div => {
                let units = scaled::div_exact(*amount, *divisor).map_err(inexact(op, "div"))?;
                Ok(Value::Money(units))
            }
            (Value::Money(amount), Value::Decimal { units, scale }) if op == BinOp::Mul => {
                let units =
                    scaled::mul_ratio_exact(*amount, *units, *scale).map_err(inexact(op, "mul"))?;
                Ok(Value::Money(units))
            }
            (Value::Decimal { units, scale }, Value::Money(amount)) if op == BinOp::Mul => {
                let units =
                    scaled::mul_ratio_exact(*amount, *units, *scale).map_err(inexact(op, "mul"))?;
                Ok(Value::Money(units))
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
        (Value::Money(amount), "mul") => {
            expect_arity(method, 2, &args)?;
            let (units, scale) = match &args[0] {
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
            Ok(Value::Money(scaled::mul_ratio(
                *amount, units, scale, rounding,
            )?))
        }
        (Value::Money(amount), "div") => {
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
            Ok(Value::Money(scaled::div_round(*amount, divisor, rounding)?))
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
        _ => Err(ErrorKind::UnknownMethod {
            ty: receiver.ty(),
            method: method.to_string(),
        }),
    }
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
