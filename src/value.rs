use std::collections::BTreeMap;
use std::fmt::{self, Write as _};

use crate::currency::Currency;
use crate::ir::{EntityField, EnumDef, EventPath, Ident, Literal, Type};
use crate::scaled::{self, Rounding};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Decimal {
        units: i64,
        scale: u8,
    },
    Str(String),
    Uuid(String),
    Timestamp(i64),
    Money(i64),
    Enum {
        ty: Ident,
        variant: Ident,
    },
    Rounding(Rounding),
    Json(Json),
    Response {
        status: i64,
        body: Json,
    },
    Invoked(Invoked),
    Opt {
        inner: Type,
        value: Option<Box<Value>>,
    },
}

impl Value {
    pub fn str(value: impl Into<String>) -> Self {
        Value::Str(value.into())
    }

    pub fn uuid(value: impl Into<String>) -> Self {
        Value::Uuid(value.into())
    }

    pub fn decimal(units: i64, scale: u8) -> Self {
        Value::Decimal { units, scale }
    }

    pub fn some(value: Value) -> Self {
        Value::Opt {
            inner: value.ty(),
            value: Some(Box::new(value)),
        }
    }

    pub fn none(inner: Type) -> Self {
        Value::Opt { inner, value: None }
    }

    pub fn ty(&self) -> Type {
        match self {
            Value::Bool(_) => Type::Bool,
            Value::Int(_) => Type::Int,
            Value::Decimal { scale, .. } => Type::Decimal(*scale),
            Value::Str(_) => Type::String,
            Value::Uuid(_) => Type::Uuid,
            Value::Timestamp(_) => Type::Timestamp,
            Value::Money(_) => Type::Money,
            Value::Enum { ty, .. } => Type::Enum(ty.clone()),
            Value::Rounding(_) => Type::Rounding,
            Value::Json(_) => Type::Json,
            Value::Response { .. } => Type::Response,
            Value::Invoked(_) => Type::Outcome,
            Value::Opt { inner, .. } => Type::opt(inner.clone()),
        }
    }

    pub fn has_type(&self, ty: &Type) -> bool {
        &self.ty() == ty
    }

    pub fn display<'a>(&'a self, currency: &'a Currency) -> ValueDisplay<'a> {
        ValueDisplay {
            value: self,
            currency,
        }
    }
}

pub struct ValueDisplay<'a> {
    value: &'a Value,
    currency: &'a Currency,
}

impl fmt::Display for ValueDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.value {
            Value::Bool(value) => write!(f, "{value}"),
            Value::Int(value) => write!(f, "{value}"),
            Value::Decimal { units, scale } => scaled::write(f, *units, *scale),
            Value::Str(value) => write!(f, "{value:?}"),
            Value::Uuid(value) => write!(f, "{value}"),
            Value::Timestamp(micros) => write!(f, "{micros}"),
            Value::Enum { variant, .. } => f.write_str(variant),
            Value::Money(units) => {
                scaled::write(f, *units, self.currency.scale)?;
                write!(f, " {}", self.currency.code)
            }
            Value::Rounding(mode) => write!(f, "{mode}"),
            Value::Json(json) => write!(f, "{json}"),
            Value::Response { status, .. } => write!(f, "<{status}>"),
            Value::Invoked(outcome) => write!(f, "{outcome}"),
            Value::Opt { value, .. } => match value {
                Some(value) => write!(f, "{}", value.display(self.currency)),
                None => f.write_str("none"),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub path: EventPath,
    pub fields: BTreeMap<Ident, Value>,
}

impl Event {
    pub fn new(
        path: EventPath,
        fields: impl IntoIterator<Item = (impl Into<Ident>, Value)>,
    ) -> Self {
        Self {
            path,
            fields: fields
                .into_iter()
                .map(|(name, value)| (name.into(), value))
                .collect(),
        }
    }

    pub fn field(&self, name: &str) -> Option<&Value> {
        self.fields.get(name)
    }

    pub fn display<'a>(&'a self, currency: &'a Currency) -> EventDisplay<'a> {
        EventDisplay {
            event: self,
            currency,
        }
    }
}

pub struct EventDisplay<'a> {
    event: &'a Event,
    currency: &'a Currency,
}

impl fmt::Display for EventDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {{ ", self.event.path)?;
        for (i, (name, value)) in self.event.fields.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{name}: {}", value.display(self.currency))?;
        }
        f.write_str(" }")
    }
}

/// A log entry with its envelope. Commands read only `event`; a projector handler
/// reaches `id`, `position` and `at` through its `as` binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub id: String,
    pub position: u64,
    pub at: i64,
    pub event: Event,
}

impl Record {
    pub fn new(id: impl Into<String>, position: u64, at: i64, event: Event) -> Self {
        Self {
            id: id.into(),
            position,
            at,
            event,
        }
    }
}

/// The initial value a `patch` materializes a missing row from. `None` for `Uuid`
/// and `Timestamp`, whose nil and epoch-zero values are real data rather than an
/// absence, which is what makes a field of either type without a default an error.
pub fn zero(ty: &Type, enums: &[EnumDef]) -> Option<Value> {
    Some(match ty {
        Type::Bool => Value::Bool(false),
        Type::Int => Value::Int(0),
        Type::Decimal(scale) => Value::decimal(0, *scale),
        Type::String => Value::Str(String::new()),
        Type::Money => Value::Money(0),
        Type::Enum(name) => {
            let def = enums.iter().find(|def| &def.name == name)?;
            let variant = def.default_variant()?;
            Value::Enum {
                ty: name.clone(),
                variant: variant.clone(),
            }
        }
        Type::Opt(inner) => Value::none(inner.as_ref().clone()),
        Type::Uuid | Type::Timestamp | Type::Rounding => return None,
        // Never reachable from a declaration: there is no syntax that writes one of
        // these as an entity field type.
        Type::Json | Type::Response | Type::Outcome => return None,
    })
}

/// The value a field starts at: its declared default, or its zero. `None` only when
/// the parser let through a field with neither, which it does not.
pub fn initial(field: &EntityField, enums: &[EnumDef]) -> Option<Value> {
    match &field.default {
        Some(lit) => Some(literal(lit)),
        None => zero(&field.ty, enums),
    }
}

pub fn literal(lit: &Literal) -> Value {
    match lit {
        Literal::Bool(value) => Value::Bool(*value),
        Literal::Int(value) => Value::Int(*value),
        Literal::Decimal { units, scale } => Value::Decimal {
            units: *units,
            scale: *scale,
        },
        Literal::Str(value) => Value::Str(value.clone()),
        Literal::Uuid(value) => Value::Uuid(value.clone()),
        Literal::Timestamp(micros) => Value::Timestamp(*micros),
        Literal::Money(units) => Value::Money(*units),
        Literal::Enum { ty, variant } => Value::Enum {
            ty: ty.clone(),
            variant: variant.clone(),
        },
        Literal::None(inner) => Value::none(inner.clone()),
        Literal::Rounding(mode) => Value::Rounding(*mode),
    }
}

/// The subset of [`Value`] that can be an entity key: everything that orders and
/// hashes. `Value` itself can do neither, because `Decimal` and `Money` compare
/// across scales. The discriminant is kept, so a `Uuid` key and a `String` key
/// spelled the same are distinct keys.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Key {
    Int(i64),
    Str(String),
    Uuid(String),
    Timestamp(i64),
    Enum { ty: Ident, variant: Ident },
}

impl Key {
    pub fn from_value(value: &Value) -> Option<Key> {
        Some(match value {
            Value::Int(value) => Key::Int(*value),
            Value::Str(value) => Key::Str(value.clone()),
            Value::Uuid(value) => Key::Uuid(value.clone()),
            Value::Timestamp(micros) => Key::Timestamp(*micros),
            Value::Enum { ty, variant } => Key::Enum {
                ty: ty.clone(),
                variant: variant.clone(),
            },
            _ => return None,
        })
    }

    pub fn ty(&self) -> Type {
        match self {
            Key::Int(_) => Type::Int,
            Key::Str(_) => Type::String,
            Key::Uuid(_) => Type::Uuid,
            Key::Timestamp(_) => Type::Timestamp,
            Key::Enum { ty, .. } => Type::Enum(ty.clone()),
        }
    }
}

/// Whether a type may be an entity key. Matches the runtime's requirement that a
/// key be an orderable scalar, since it doubles as the read API's pagination cursor.
pub fn can_key(ty: &Type) -> bool {
    matches!(
        ty,
        Type::Int | Type::String | Type::Uuid | Type::Timestamp | Type::Enum(_)
    )
}

/// A JSON value, for HTTP bodies (rule 8). Hand-rolled rather than a dependency,
/// because the interpreter constructs responses and never parses one. `Obj` is
/// ordered, which is where rule 14's defined iteration order comes from: the same
/// object built twice serialises byte for byte the same.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Json {
    Null,
    Bool(bool),
    Int(i64),
    Str(String),
    Arr(Vec<Json>),
    Obj(BTreeMap<String, Json>),
}

impl Json {
    pub fn str(value: impl Into<String>) -> Self {
        Json::Str(value.into())
    }

    pub fn obj(fields: impl IntoIterator<Item = (impl Into<String>, Json)>) -> Self {
        Json::Obj(
            fields
                .into_iter()
                .map(|(name, value)| (name.into(), value))
                .collect(),
        )
    }

    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(fields) => fields.get(key),
            _ => None,
        }
    }

    /// Rule 8's conversion table, total so that an object literal always serialises.
    /// `Money` and `Decimal` become strings at their scale rather than numbers, so no
    /// precision is lost to a float on the far side.
    pub fn from_value(value: &Value, currency: &Currency) -> Json {
        match value {
            Value::Bool(value) => Json::Bool(*value),
            Value::Int(value) => Json::Int(*value),
            Value::Decimal { units, scale } => Json::Str(scaled::text(*units, *scale)),
            Value::Money(units) => Json::Str(scaled::text(*units, currency.scale)),
            Value::Str(value) | Value::Uuid(value) => Json::Str(value.clone()),
            Value::Timestamp(micros) => Json::Int(*micros),
            Value::Enum { variant, .. } => Json::Str(variant.clone()),
            Value::Rounding(mode) => Json::Str(mode.to_string()),
            Value::Json(json) => json.clone(),
            Value::Response { status, body } => {
                Json::obj([("body", body.clone()), ("status", Json::Int(*status))])
            }
            Value::Invoked(outcome) => Json::obj([
                ("ok", Json::Bool(outcome.ok())),
                ("code", outcome.code().map_or(Json::Null, Json::str)),
                ("message", outcome.message().map_or(Json::Null, Json::str)),
            ]),
            Value::Opt { value, .. } => match value {
                Some(value) => Json::from_value(value, currency),
                None => Json::Null,
            },
        }
    }
}

impl fmt::Display for Json {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Json::Null => f.write_str("null"),
            Json::Bool(value) => write!(f, "{value}"),
            Json::Int(value) => write!(f, "{value}"),
            Json::Str(value) => write_json_str(f, value),
            Json::Arr(items) => {
                f.write_str("[")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        f.write_str(",")?;
                    }
                    write!(f, "{item}")?;
                }
                f.write_str("]")
            }
            Json::Obj(fields) => {
                f.write_str("{")?;
                for (i, (name, value)) in fields.iter().enumerate() {
                    if i > 0 {
                        f.write_str(",")?;
                    }
                    write_json_str(f, name)?;
                    write!(f, ":{value}")?;
                }
                f.write_str("}")
            }
        }
    }
}

fn write_json_str(f: &mut fmt::Formatter<'_>, value: &str) -> fmt::Result {
    f.write_str("\"")?;
    for c in value.chars() {
        match c {
            '"' => f.write_str("\\\"")?,
            '\\' => f.write_str("\\\\")?,
            '\n' => f.write_str("\\n")?,
            '\r' => f.write_str("\\r")?,
            '\t' => f.write_str("\\t")?,
            c if (c as u32) < 0x20 => write!(f, "\\u{:04x}", c as u32)?,
            c => f.write_char(c)?,
        }
    }
    f.write_str("\"")
}

/// What an `invoke` returns (rule 6): hekla's six-variant `CommandOutcome` cut to the
/// three an author can act on differently. Distinct from [`crate::interp::Outcome`],
/// which also carries the emitted events; the difference between the two is the cut.
/// `Conflict` and `Unavailable` have no variant here at all, so a retryable outcome is
/// unrepresentable rather than filtered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invoked {
    Ok,
    Invalid(String),
    Reject { code: String, message: String },
}

impl Invoked {
    pub fn ok(&self) -> bool {
        matches!(self, Invoked::Ok)
    }

    pub fn code(&self) -> Option<&str> {
        match self {
            Invoked::Reject { code, .. } => Some(code),
            _ => None,
        }
    }

    pub fn message(&self) -> Option<&str> {
        match self {
            Invoked::Ok => None,
            Invoked::Invalid(message) => Some(message),
            Invoked::Reject { message, .. } => Some(message),
        }
    }
}

impl fmt::Display for Invoked {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Invoked::Ok => f.write_str("ok"),
            Invoked::Invalid(message) => write!(f, "invalid: {message}"),
            Invoked::Reject { code, message } => write!(f, "reject {code}: {message}"),
        }
    }
}
