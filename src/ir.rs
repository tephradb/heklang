use std::error;
use std::fmt;

use crate::scaled::{self, Rounding};

pub type Ident = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Slot(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ExprId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SliceId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub line: u32,
    pub col: u32,
}

impl Span {
    pub fn new(line: u32, col: u32) -> Self {
        Self { line, col }
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.col)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    Bool,
    Int,
    Decimal(u8),
    String,
    Uuid,
    Timestamp,
    /// Its own type rather than a `Decimal`, because its operator table is what
    /// catches the mistakes: money plus money is fine, money plus a bare decimal is
    /// not. The scale is a storage precision floor, not a claim about any currency;
    /// currency is not in the type, the value or the config.
    Money(u8),
    Enum(Ident),
    Record(Ident),
    Rounding,
    Json,
    Response,
    Outcome,
    List(Box<Type>),
    /// The key is restricted to the types that order and hash, the same set an entity
    /// key is restricted to, which is where sorted iteration comes from.
    Map(Box<Type>, Box<Type>),
    Opt(Box<Type>),
}

impl Type {
    pub fn opt(inner: Type) -> Self {
        Type::Opt(Box::new(inner))
    }

    pub fn list(inner: Type) -> Self {
        Type::List(Box::new(inner))
    }

    pub fn map(key: Type, value: Type) -> Self {
        Type::Map(Box::new(key), Box::new(value))
    }
}

impl fmt::Display for Type {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Type::Bool => f.write_str("Bool"),
            Type::Int => f.write_str("Int"),
            Type::Decimal(scale) => write!(f, "Decimal({scale})"),
            Type::String => f.write_str("String"),
            Type::Uuid => f.write_str("Uuid"),
            Type::Timestamp => f.write_str("Timestamp"),
            Type::Money(scale) => write!(f, "Money({scale})"),
            Type::Enum(name) | Type::Record(name) => f.write_str(name),
            Type::Rounding => f.write_str("Rounding"),
            Type::Json => f.write_str("Json"),
            Type::Response => f.write_str("Response"),
            Type::Outcome => f.write_str("Outcome"),
            Type::List(inner) => write!(f, "List({inner})"),
            Type::Map(key, value) => write!(f, "Map({key}, {value})"),
            Type::Opt(inner) => write!(f, "{inner}?"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct EventPath {
    pub segments: Vec<Ident>,
}

impl EventPath {
    pub fn new(segments: impl IntoIterator<Item = impl Into<Ident>>) -> Self {
        Self {
            segments: segments.into_iter().map(Into::into).collect(),
        }
    }
}

impl fmt::Display for EventPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("@")?;
        for (i, segment) in self.segments.iter().enumerate() {
            if i > 0 {
                f.write_str(".")?;
            }
            f.write_str(segment)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Program {
    pub events: Vec<EventDef>,
    pub commands: Vec<Command>,
    pub projectors: Vec<Projector>,
    pub effects: Vec<Effect>,
    /// Module scope, shadowed inside a projector by one of its own.
    pub enums: Vec<EnumDef>,
    pub records: Vec<RecordDef>,
    pub consts: Vec<ConstDef>,
    pub functions: Vec<Function>,
}

impl Program {
    pub fn event(&self, path: &EventPath) -> Option<&EventDef> {
        self.events.iter().find(|def| &def.path == path)
    }

    pub fn command(&self, name: &str) -> Option<&Command> {
        self.commands.iter().find(|command| command.name == name)
    }

    pub fn projector(&self, name: &str) -> Option<&Projector> {
        self.projectors
            .iter()
            .find(|projector| projector.name == name)
    }

    pub fn effect(&self, name: &str) -> Option<&Effect> {
        self.effects.iter().find(|effect| effect.name == name)
    }

    pub fn record(&self, name: &str) -> Option<&RecordDef> {
        self.records.iter().find(|def| def.name == name)
    }

    pub fn constant(&self, name: &str) -> Option<&ConstDef> {
        self.consts.iter().find(|def| def.name == name)
    }

    pub fn function(&self, name: &str) -> Option<&Function> {
        self.functions.iter().find(|def| def.name == name)
    }
}

/// A pure helper. It has a command's frame and arena and none of its machinery: no
/// prologue, no slices, no state, because none of those can be pure.
#[derive(Debug, Clone)]
pub struct Function {
    pub name: Ident,
    pub module: Option<Ident>,
    pub params: Vec<Param>,
    pub ret: Type,
    pub frame: usize,
    pub exprs: Exprs,
    pub body: Vec<Stmt>,
}

/// A named product type at module scope. Unlike an entity it is an ordinary value:
/// it can be a field, a parameter, a fold's state and an element of a container.
#[derive(Debug, Clone)]
pub struct RecordDef {
    pub name: Ident,
    pub module: Option<Ident>,
    pub fields: Vec<RecordField>,
}

impl RecordDef {
    pub fn field(&self, name: &str) -> Option<&RecordField> {
        self.fields.iter().find(|field| field.name == name)
    }
}

#[derive(Debug, Clone)]
pub struct RecordField {
    pub name: Ident,
    pub ty: Type,
}

/// A module-scope constant. The value is a literal, so the declaration needs no
/// expression arena, which is the same reason an entity default is a literal.
#[derive(Debug, Clone)]
pub struct ConstDef {
    pub name: Ident,
    pub module: Option<Ident>,
    pub ty: Type,
    pub value: Literal,
}

#[derive(Debug, Clone)]
pub struct EventDef {
    pub path: EventPath,
    pub fields: Vec<FieldDef>,
}

impl EventDef {
    pub fn new(path: EventPath, fields: impl IntoIterator<Item = FieldDef>) -> Self {
        Self {
            path,
            fields: fields.into_iter().collect(),
        }
    }

    pub fn field(&self, name: &str) -> Option<&FieldDef> {
        self.fields.iter().find(|field| field.name == name)
    }
}

#[derive(Debug, Clone)]
pub struct FieldDef {
    pub name: Ident,
    pub ty: Type,
    pub subject: Option<Ident>,
    pub indexed: bool,
    pub max_len: Option<usize>,
}

impl FieldDef {
    pub fn new(name: impl Into<Ident>, ty: Type) -> Self {
        Self {
            name: name.into(),
            ty,
            subject: None,
            indexed: true,
            max_len: None,
        }
    }

    pub fn subject(mut self, field: impl Into<Ident>) -> Self {
        self.subject = Some(field.into());
        self
    }

    pub fn no_index(mut self) -> Self {
        self.indexed = false;
        self
    }

    pub fn max_len(mut self, len: usize) -> Self {
        self.max_len = Some(len);
        self
    }
}

#[derive(Debug, Clone)]
pub struct Projector {
    pub name: Ident,
    /// The module this was declared in. A projector is one braced block, so all of
    /// its handlers share it.
    pub module: Option<Ident>,
    pub enums: Vec<EnumDef>,
    pub entities: Vec<EntityDef>,
    pub handlers: Vec<Handler>,
}

impl Projector {
    pub fn entity(&self, name: &str) -> Option<&EntityDef> {
        self.entities.iter().find(|entity| entity.name == name)
    }

    pub fn enum_def(&self, name: &str) -> Option<&EnumDef> {
        self.enums.iter().find(|def| def.name == name)
    }
}

#[derive(Debug, Clone)]
pub struct EnumDef {
    pub name: Ident,
    pub variants: Vec<Ident>,
    pub default: Option<usize>,
}

impl EnumDef {
    pub fn has(&self, variant: &str) -> bool {
        self.variants.iter().any(|found| found == variant)
    }

    pub fn default_variant(&self) -> Option<&Ident> {
        self.default.and_then(|index| self.variants.get(index))
    }
}

#[derive(Debug, Clone)]
pub struct EntityDef {
    pub name: Ident,
    pub fields: Vec<EntityField>,
    pub key: usize,
    pub indexes: Vec<Index>,
}

impl EntityDef {
    pub fn field(&self, name: &str) -> Option<&EntityField> {
        self.fields.iter().find(|field| field.name == name)
    }

    pub fn key_field(&self) -> &EntityField {
        &self.fields[self.key]
    }
}

#[derive(Debug, Clone)]
pub struct EntityField {
    pub name: Ident,
    pub ty: Type,
    pub max_len: Option<usize>,
    pub default: Option<Literal>,
    pub subject: Option<Ident>,
}

impl EntityField {
    pub fn new(name: impl Into<Ident>, ty: Type) -> Self {
        Self {
            name: name.into(),
            ty,
            max_len: None,
            default: None,
            subject: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Index {
    pub fields: Vec<Ident>,
}

/// One `on @path { .. } { .. }` arm. Each handler owns its frame and arena, which
/// is what makes "handlers do not share state" structural rather than a rule.
#[derive(Debug, Clone)]
pub struct Handler {
    pub event: EventPath,
    pub binds: Vec<Bind>,
    pub envelope: Vec<EnvBind>,
    pub frame: usize,
    pub exprs: Exprs,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvField {
    At,
    Id,
    Position,
}

impl EnvField {
    pub fn lookup(name: &str) -> Option<Self> {
        Some(match name {
            "at" => EnvField::At,
            "id" => EnvField::Id,
            "position" => EnvField::Position,
            _ => return None,
        })
    }

    pub fn ty(self) -> Type {
        match self {
            EnvField::At => Type::Timestamp,
            EnvField::Id => Type::Uuid,
            EnvField::Position => Type::Int,
        }
    }
}

#[derive(Debug, Clone)]
pub struct EnvBind {
    pub field: EnvField,
    pub slot: Slot,
}

#[derive(Debug, Clone)]
pub struct Effect {
    pub name: Ident,
    pub module: Option<Ident>,
    pub arms: Vec<Arm>,
}

impl Effect {
    /// The arm one event selects. Rule 1 makes this at most one, so it is a lookup
    /// rather than a filter, even though an arm may now list several paths.
    pub fn arm(&self, event: &EventPath) -> Option<&Arm> {
        self.arms
            .iter()
            .find(|arm| arm.events.iter().any(|path| path == event))
    }
}

/// One `on @path[, @path]* [as name] [{ destructure }] { body }` of an effect. A
/// command minus its params, plus a trigger binding: the same prologue, slices and
/// arena. Several paths may share one arm; the binding then names only what they have
/// in common.
#[derive(Debug, Clone)]
pub struct Arm {
    pub events: Vec<EventPath>,
    pub binds: Vec<Bind>,
    pub envelope: Vec<EnvBind>,
    pub frame: usize,
    pub exprs: Exprs,
    pub prologue: Vec<Assign>,
    pub slices: Vec<Slice>,
    pub states: Vec<StateVar>,
    /// Rule 11: `now()` is one slot filled before the body runs, so two calls in one
    /// body are two reads of one value.
    pub now: Option<Slot>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Command {
    pub name: Ident,
    /// The module this was declared in, for error messages. `None` for an unnamed source.
    pub module: Option<Ident>,
    pub params: Vec<Param>,
    pub frame: usize,
    pub exprs: Exprs,
    /// Rule 11: the request's append time, pinned once. See [`Arm::now`].
    pub now: Option<Slot>,
    pub prologue: Vec<Assign>,
    pub slices: Vec<Slice>,
    pub states: Vec<StateVar>,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: Ident,
    pub ty: Type,
    pub slot: Slot,
}

#[derive(Debug, Clone)]
pub struct StateVar {
    pub name: Ident,
    pub ty: Type,
    pub slot: Slot,
    pub init: ExprId,
}

#[derive(Debug, Clone)]
pub struct Slice {
    pub event: EventPath,
    pub filters: Vec<Filter>,
    pub binds: Vec<Bind>,
    pub updates: Vec<Update>,
}

#[derive(Debug, Clone)]
pub struct Filter {
    pub field: Ident,
    pub value: ExprId,
}

impl Filter {
    pub fn new(field: impl Into<Ident>, value: ExprId) -> Self {
        Self {
            field: field.into(),
            value,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Bind {
    pub field: Ident,
    pub slot: Slot,
}

#[derive(Debug, Clone)]
pub struct Update {
    pub slot: Slot,
    pub value: ExprId,
}

#[derive(Debug, Clone)]
pub struct Assign {
    pub slot: Slot,
    pub value: ExprId,
}

#[derive(Debug, Clone, Default)]
pub struct Exprs {
    nodes: Vec<Expr>,
    spans: Vec<Span>,
}

impl Exprs {
    pub fn push(&mut self, expr: Expr, span: Span) -> ExprId {
        let id = ExprId(self.nodes.len() as u32);
        self.nodes.push(expr);
        self.spans.push(span);
        id
    }

    pub fn get(&self, id: ExprId) -> Option<&Expr> {
        self.nodes.get(id.0 as usize)
    }

    pub fn span(&self, id: ExprId) -> Span {
        self.spans.get(id.0 as usize).copied().unwrap_or_default()
    }

    pub fn patch(&mut self, id: ExprId, expr: Expr) {
        if let Some(node) = self.nodes.get_mut(id.0 as usize) {
            *node = expr;
        }
    }
}

#[derive(Debug, Clone)]
pub enum Expr {
    Lit(Literal),
    Load(Slot),
    Unary {
        op: UnOp,
        operand: ExprId,
    },
    Binary {
        op: BinOp,
        lhs: ExprId,
        rhs: ExprId,
    },
    Method {
        receiver: ExprId,
        method: Ident,
        args: Vec<ExprId>,
    },
    If {
        cond: ExprId,
        then: ExprId,
        otherwise: ExprId,
    },
    /// `response.status`, `response.body`. Parenless, unlike a method call.
    Field {
        receiver: ExprId,
        name: Ident,
    },
    /// A JSON object literal, for an HTTP request body only (rule 8).
    Object(Vec<(Ident, ExprId)>),
    /// An interpolated string. The literal chunks are ordinary `Lit(Str)` parts, so
    /// this is a join over one uniform list rather than two interleaved ones.
    Interp(Vec<ExprId>),
    List(Vec<ExprId>),
    Record {
        ty: Ident,
        fields: Vec<(Ident, ExprId)>,
    },
    CallFn {
        function: Ident,
        args: Vec<ExprId>,
    },
    /// `[yields for bindings in over if cond]`. The bindings are ordinary frame slots,
    /// filled once per element, so nothing about a comprehension is a new scope kind.
    Comp {
        iter: Iter,
        cond: Option<ExprId>,
        yields: ExprId,
        /// The element type when the target declared one, so an empty result still
        /// has the type it was written for rather than a guess from its first item.
        inner: Option<Type>,
    },
    Call {
        builtin: Builtin,
        args: Vec<ExprId>,
    },
    Invoke {
        command: Ident,
        args: Vec<(Ident, ExprId)>,
    },
    /// Rule 12. The subject field and value are recovered by the parser, because
    /// subject-ness is a property of the schema path rather than of the value.
    Reveal {
        value: ExprId,
        field: Ident,
        subject: Ident,
        subject_value: ExprId,
    },
}

/// What a `for` or a comprehension binds. `index` is the key over a map and the
/// position over a list, and is absent when only one name was given.
#[derive(Debug, Clone)]
pub struct Iter {
    pub index: Option<Slot>,
    pub item: Slot,
    pub over: ExprId,
}

/// The builtins that are ordinary calls. `now()` is not among them: it lowers to a
/// pre-filled slot, so it is pinned once per invocation rather than once per call.
/// `UuidDerive` is spelled `Uuid.derive`: the global namespace is closed to
/// constructors, so anything built from nothing is named by its type instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    HttpGet,
    HttpPost,
    HttpPut,
    HttpPatch,
    HttpDelete,
    UuidDerive,
    JsonEncode,
    TimestampParse,
    /// Carries the target scale, which comes from where the result lands rather than
    /// from the text: `"10.5"` is a different value at scale 2 and at scale 3.
    MoneyParse(u8),
}

impl Builtin {
    pub fn verb(name: &str) -> Option<Self> {
        Some(match name {
            "get" => Builtin::HttpGet,
            "post" => Builtin::HttpPost,
            "put" => Builtin::HttpPut,
            "patch" => Builtin::HttpPatch,
            "delete" => Builtin::HttpDelete,
            _ => return None,
        })
    }

    /// Whether this is an HTTP verb rather than one of the pure builtins.
    pub fn is_http(self) -> bool {
        !matches!(
            self,
            Builtin::UuidDerive
                | Builtin::JsonEncode
                | Builtin::TimestampParse
                | Builtin::MoneyParse(_)
        )
    }

    /// Whether this verb carries a request body.
    pub fn has_body(self) -> bool {
        matches!(
            self,
            Builtin::HttpPost | Builtin::HttpPut | Builtin::HttpPatch
        )
    }

    pub fn name(self) -> &'static str {
        match self {
            Builtin::HttpGet => "http.get",
            Builtin::HttpPost => "http.post",
            Builtin::HttpPut => "http.put",
            Builtin::HttpPatch => "http.patch",
            Builtin::HttpDelete => "http.delete",
            Builtin::UuidDerive => "Uuid.derive",
            Builtin::JsonEncode => "Json.encode",
            Builtin::TimestampParse => "Timestamp.parse",
            Builtin::MoneyParse(_) => "Money.parse",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Literal {
    Bool(bool),
    Int(i64),
    Decimal {
        units: i64,
        scale: u8,
    },
    Str(String),
    Uuid(String),
    Timestamp(i64),
    None(Type),
    Money {
        units: i64,
        scale: u8,
    },
    Enum {
        ty: Ident,
        variant: Ident,
    },
    Rounding(Rounding),
    /// A container of literals is itself a literal, so it can be a seed, a default
    /// and a const rather than only an expression.
    List {
        inner: Type,
        items: Vec<Literal>,
    },
    EmptyMap(Type, Type),
    /// `Json.empty`, an object with no members. A literal rather than an expression so
    /// it can be a seed, a default and a const.
    EmptyJson,
    Record {
        ty: Ident,
        fields: Vec<(Ident, Literal)>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Number {
    pub digits: i128,
    pub scale: u8,
}

impl Number {
    pub fn new(digits: i128, scale: u8) -> Self {
        Self { digits, scale }
    }

    /// Resolves against a target type. `Money(n)` follows exactly the same rule as
    /// `Decimal(n)`: widening is exact, and more written places than the target holds
    /// is an error rather than a silent round.
    pub fn resolve(self, ty: &Type) -> Result<Literal, NumberError> {
        let target = match ty {
            Type::Int => 0,
            Type::Decimal(scale) | Type::Money(scale) => *scale,
            other => return Err(NumberError::NotNumeric(other.clone())),
        };

        if self.scale > target {
            return Err(NumberError::TooPrecise {
                written: self.scale,
                target: ty.clone(),
            });
        }

        let digits = i64::try_from(self.digits).map_err(|_| NumberError::Overflow)?;
        let units =
            scaled::rescale(digits, self.scale, target).map_err(|_| NumberError::Overflow)?;

        Ok(match ty {
            Type::Int => Literal::Int(units),
            Type::Decimal(scale) => Literal::Decimal {
                units,
                scale: *scale,
            },
            _ => Literal::Money {
                units,
                scale: target,
            },
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NumberError {
    NotNumeric(Type),
    TooPrecise { written: u8, target: Type },
    Overflow,
}

impl fmt::Display for NumberError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NumberError::NotNumeric(ty) => write!(f, "a number cannot be a {ty}"),
            NumberError::TooPrecise { written, target } => {
                let places = if *written == 1 { "place" } else { "places" };
                write!(f, "{written} decimal {places} is too precise for {target}")
            }
            NumberError::Overflow => f.write_str("number does not fit"),
        }
    }
}

impl error::Error for NumberError {}

#[derive(Debug, Clone)]
pub enum Stmt {
    Assign {
        slot: Slot,
        value: ExprId,
    },
    If {
        cond: ExprId,
        then: Vec<Stmt>,
        otherwise: Vec<Stmt>,
    },
    Emit {
        event: EventPath,
        fields: Vec<(Ident, ExprId)>,
        span: Span,
    },
    Put {
        entity: Ident,
        fields: Vec<(Ident, ExprId)>,
        span: Span,
    },
    Patch {
        entity: Ident,
        key: ExprId,
        loads: Vec<Bind>,
        fields: Vec<(Ident, ExprId)>,
        span: Span,
    },
    Delete {
        entity: Ident,
        key: ExprId,
    },
    /// Rule 4: the author's terminal outcome, and the only one.
    Fail {
        message: ExprId,
        span: Span,
    },
    Log {
        message: ExprId,
    },
    /// Rule 9 keeps `erase` a statement, so an erase can only appear where the
    /// control-flow join is precise.
    Erase {
        subject: Ident,
        value: ExprId,
        span: Span,
    },
    For {
        iter: Iter,
        body: Vec<Stmt>,
    },
    /// A bare `invoke` or `http.*` whose result is unused. A closed rule, not general
    /// expression statements.
    Discard(ExprId),
    Return(Return),
}

#[derive(Debug, Clone)]
pub enum Return {
    Ok,
    Invalid(ExprId),
    Reject {
        code: ExprId,
        message: ExprId,
    },
    /// A `fn`'s result. Only a `fn` can produce one, and it always does: the parser
    /// rejects a body that can finish without returning.
    Value(ExprId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnOp {
    Not,
    Neg,
}

impl UnOp {
    pub fn symbol(self) -> &'static str {
        match self {
            UnOp::Not => "!",
            UnOp::Neg => "-",
        }
    }
}

impl fmt::Display for UnOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.symbol())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

impl BinOp {
    pub fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::Eq => "==",
            BinOp::Ne => "!=",
            BinOp::Lt => "<",
            BinOp::Le => "<=",
            BinOp::Gt => ">",
            BinOp::Ge => ">=",
            BinOp::And => "&&",
            BinOp::Or => "||",
        }
    }
}

impl fmt::Display for BinOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.symbol())
    }
}
