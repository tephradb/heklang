use std::error;
use std::fmt;

use crate::currency::Currency;
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
    Money,
    Enum(Ident),
    Rounding,
    Opt(Box<Type>),
}

impl Type {
    pub fn opt(inner: Type) -> Self {
        Type::Opt(Box::new(inner))
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
            Type::Money => f.write_str("Money"),
            Type::Enum(name) => f.write_str(name),
            Type::Rounding => f.write_str("Rounding"),
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
    pub currency: Currency,
    pub events: Vec<EventDef>,
    pub commands: Vec<Command>,
    pub projectors: Vec<Projector>,
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
pub struct Command {
    pub name: Ident,
    /// The module this was declared in, for error messages. `None` for an unnamed source.
    pub module: Option<Ident>,
    pub params: Vec<Param>,
    pub frame: usize,
    pub exprs: Exprs,
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Literal {
    Bool(bool),
    Int(i64),
    Decimal { units: i64, scale: u8 },
    Str(String),
    Uuid(String),
    Timestamp(i64),
    None(Type),
    Money(i64),
    Enum { ty: Ident, variant: Ident },
    Rounding(Rounding),
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

    pub fn resolve(self, ty: &Type, currency: &Currency) -> Result<Literal, NumberError> {
        let target = match ty {
            Type::Int => 0,
            Type::Decimal(scale) => *scale,
            Type::Money => currency.scale,
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
            _ => Literal::Money(units),
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
    Return(Return),
}

#[derive(Debug, Clone)]
pub enum Return {
    Ok,
    Invalid(ExprId),
    Reject { code: ExprId, message: ExprId },
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
