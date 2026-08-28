use std::collections::BTreeMap;
use std::fmt::{self, Write as _};

use crate::ir::{EntityField, EnumDef, EventPath, Ident, Literal, RecordDef, Type};
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
    Money {
        units: i64,
        scale: u8,
    },
    Enum {
        ty: Ident,
        variant: Ident,
    },
    Record {
        ty: Ident,
        fields: BTreeMap<Ident, Value>,
    },
    Rounding(Rounding),
    Json(Json),
    Response {
        status: i64,
        body: Json,
    },
    Invoked(Invoked),
    List {
        inner: Type,
        items: Vec<Value>,
    },
    /// Keyed by [`Key`] rather than by `Value`, which is what makes iteration sorted
    /// and therefore what makes verify mode's determinism hold.
    Map {
        key: Type,
        value: Type,
        entries: BTreeMap<Key, Value>,
    },
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

    pub fn money(units: i64, scale: u8) -> Self {
        Value::Money { units, scale }
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

    pub fn list(inner: Type, items: impl IntoIterator<Item = Value>) -> Self {
        Value::List {
            inner,
            items: items.into_iter().collect(),
        }
    }

    pub fn record(
        ty: impl Into<Ident>,
        fields: impl IntoIterator<Item = (impl Into<Ident>, Value)>,
    ) -> Self {
        Value::Record {
            ty: ty.into(),
            fields: fields
                .into_iter()
                .map(|(name, value)| (name.into(), value))
                .collect(),
        }
    }

    pub fn map(key: Type, value: Type, entries: impl IntoIterator<Item = (Key, Value)>) -> Self {
        Value::Map {
            key,
            value,
            entries: entries.into_iter().collect(),
        }
    }

    pub fn ty(&self) -> Type {
        match self {
            Value::Bool(_) => Type::Bool,
            Value::Int(_) => Type::Int,
            Value::Decimal { scale, .. } => Type::Decimal(*scale),
            Value::Str(_) => Type::String,
            Value::Uuid(_) => Type::Uuid,
            Value::Timestamp(_) => Type::Timestamp,
            Value::Money { scale, .. } => Type::Money(*scale),
            Value::Enum { ty, .. } => Type::Enum(ty.clone()),
            Value::Record { ty, .. } => Type::Record(ty.clone()),
            Value::Rounding(_) => Type::Rounding,
            Value::Json(_) => Type::Json,
            Value::Response { .. } => Type::Response,
            Value::Invoked(_) => Type::Outcome,
            Value::List { inner, .. } => Type::list(inner.clone()),
            Value::Map { key, value, .. } => Type::map(key.clone(), value.clone()),
            Value::Opt { inner, .. } => Type::opt(inner.clone()),
        }
    }

    /// A seal is transparent here: heklang models the key lifecycle rather than
    /// ciphertext, so a sealed position holds an ordinary value of its inner type.
    /// The boundary it guards is a parse-time rule. See `docs/effects.md` rule 12.
    pub fn has_type(&self, ty: &Type) -> bool {
        match ty {
            Type::Sealed(inner, _) => self.has_type(inner),
            _ => &self.ty() == ty,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Bool(value) => write!(f, "{value}"),
            Value::Int(value) => write!(f, "{value}"),
            Value::Decimal { units, scale } => scaled::write(f, *units, *scale),
            Value::Str(value) => write!(f, "{value:?}"),
            Value::Uuid(value) => write!(f, "{value}"),
            Value::Timestamp(micros) => write!(f, "{micros}"),
            Value::Enum { variant, .. } => f.write_str(variant),
            Value::Record { .. } => write!(f, "{}", Json::from_value(self)),
            // No currency code: an amount carries a scale and nothing else, so a
            // program that needs one declares an ordinary field beside it.
            Value::Money { units, scale } => scaled::write(f, *units, *scale),
            Value::Rounding(mode) => write!(f, "{mode}"),
            Value::Json(json) => write!(f, "{json}"),
            Value::Response { status, .. } => write!(f, "<{status}>"),
            Value::Invoked(outcome) => write!(f, "{outcome}"),
            Value::List { .. } | Value::Map { .. } => write!(f, "{}", Json::from_value(self)),
            Value::Opt { value, .. } => match value {
                Some(value) => write!(f, "{value}"),
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
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {{ ", self.path)?;
        for (i, (name, value)) in self.fields.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{name}: {value}")?;
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

/// The declarations a zero value has to consult: an enum for its default variant, a
/// record for its fields. A projector's own enums shadow the module's, which is the
/// same precedence the parser applies.
#[derive(Debug, Clone, Copy)]
pub struct Defs<'a> {
    pub local: &'a [EnumDef],
    pub enums: &'a [EnumDef],
    pub records: &'a [RecordDef],
}

impl Defs<'_> {
    pub fn enum_def(&self, name: &str) -> Option<&EnumDef> {
        self.local
            .iter()
            .chain(self.enums)
            .find(|def| def.name == name)
    }
}

/// The initial value a `patch` materializes a missing row from. `None` for `Uuid`
/// and `Timestamp`, whose nil and epoch-zero values are real data rather than an
/// absence, which is what makes a field of either type without a default an error.
pub fn zero(ty: &Type, defs: Defs<'_>) -> Option<Value> {
    Some(match ty {
        // A seal is transparent: the zero of sealed content is the content's own.
        Type::Sealed(inner, _) => zero(inner, defs)?,
        Type::Bool => Value::Bool(false),
        Type::Int => Value::Int(0),
        Type::Decimal(scale) => Value::decimal(0, *scale),
        Type::String => Value::Str(String::new()),
        Type::Money(scale) => Value::money(0, *scale),
        Type::Enum(name) => {
            let def = defs.enum_def(name)?;
            let variant = def.default_variant()?;
            Value::Enum {
                ty: name.clone(),
                variant: variant.clone(),
            }
        }
        // A record's zero is its fields' zeros, so a record column materializes the
        // same way every other column does rather than being a special case.
        Type::Record(name) => {
            let def = defs.records.iter().find(|def| &def.name == name)?;
            let mut fields = BTreeMap::new();
            for field in &def.fields {
                fields.insert(field.name.clone(), zero(&field.ty, defs)?);
            }
            Value::Record {
                ty: name.clone(),
                fields,
            }
        }
        Type::Opt(inner) => Value::none(inner.as_ref().clone()),
        Type::List(inner) => Value::list(inner.as_ref().clone(), []),
        Type::Map(key, value) => Value::map(key.as_ref().clone(), value.as_ref().clone(), []),
        Type::Uuid | Type::Timestamp | Type::Rounding => return None,
        // Never reachable from a declaration: there is no syntax that writes one of
        // these as an entity field type.
        Type::Json | Type::Response | Type::Outcome => return None,
    })
}

/// The value a field starts at: its declared default, or its zero. `None` only when
/// the parser let through a field with neither, which it does not.
pub fn initial(field: &EntityField, defs: Defs<'_>) -> Option<Value> {
    match &field.default {
        Some(lit) => Some(literal(lit)),
        None => zero(&field.ty, defs),
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
        Literal::Money { units, scale } => Value::money(*units, *scale),
        Literal::Enum { ty, variant } => Value::Enum {
            ty: ty.clone(),
            variant: variant.clone(),
        },
        Literal::None(inner) => Value::none(inner.clone()),
        Literal::Some { inner, value } => Value::Opt {
            inner: inner.clone(),
            value: Some(Box::new(literal(value))),
        },
        Literal::Rounding(mode) => Value::Rounding(*mode),
        Literal::List { inner, items } => Value::list(inner.clone(), items.iter().map(literal)),
        Literal::EmptyMap(key, value) => Value::map(key.clone(), value.clone(), []),
        Literal::EmptyJson => Value::Json(Json::Obj(BTreeMap::new())),
        Literal::Record { ty, fields } => Value::Record {
            ty: ty.clone(),
            fields: fields
                .iter()
                .map(|(name, value)| (name.clone(), literal(value)))
                .collect(),
        },
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

/// A key as a value, for a `for` binding and for a map's `keys()`.
pub fn from_key(key: &Key) -> Value {
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

fn key_text(key: &Key) -> String {
    text(&from_key(key))
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

    pub fn arr(items: impl IntoIterator<Item = Json>) -> Self {
        Json::Arr(items.into_iter().collect())
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
    pub fn from_value(value: &Value) -> Json {
        match value {
            Value::Bool(value) => Json::Bool(*value),
            Value::Int(value) => Json::Int(*value),
            Value::Decimal { units, scale } | Value::Money { units, scale } => {
                Json::Str(scaled::text(*units, *scale))
            }
            Value::Str(value) | Value::Uuid(value) => Json::Str(value.clone()),
            Value::Timestamp(micros) => Json::Int(*micros),
            Value::Enum { variant, .. } => Json::Str(variant.clone()),
            Value::Record { fields, .. } => Json::Obj(
                fields
                    .iter()
                    .map(|(name, value)| (name.clone(), Json::from_value(value)))
                    .collect(),
            ),
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
            Value::List { items, .. } => Json::Arr(items.iter().map(Json::from_value).collect()),
            // A map's keys become strings, which is what a JSON object can hold. The
            // ordering is already the map's, so this is a copy rather than a decision.
            Value::Map { entries, .. } => Json::Obj(
                entries
                    .iter()
                    .map(|(key, value)| (key_text(key), Json::from_value(value)))
                    .collect(),
            ),
            Value::Opt { value, .. } => match value {
                Some(value) => Json::from_value(value),
                None => Json::Null,
            },
        }
    }
}

/// A value's text form, for string interpolation. Rule 8's JSON table rather than
/// `Display`, which quotes a `String`: one answer for how a value looks when it leaves
/// the process, so a message and a request body cannot disagree about it.
pub fn text(value: &Value) -> String {
    match Json::from_value(value) {
        Json::Str(text) => text,
        other => other.to_string(),
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
