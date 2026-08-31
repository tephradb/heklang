use std::collections::BTreeMap;
use std::fmt::{self, Write as _};
use std::sync::Arc;

use crate::ir::{
    EntityField, EnumDef, EventPath, Ident, Literal, Number, Program, Projector, RecordDef, Type,
};
use crate::scaled::{self, Rounding};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Decimal {
        units: i64,
        scale: u8,
    },
    /// Shared rather than owned, because reading a variable copies the value out of
    /// its slot and a fold does that once per event. `Arc` rather than `Rc` so a
    /// `Program` and the values it produces stay `Send + Sync`: a host loads a program
    /// once and serves requests from several threads.
    Str(Arc<str>),
    Uuid(Arc<str>),
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
    /// Content behind the decrypt boundary: the stored form as a host keeps it, plus
    /// the subject and the id its key is filed under, so `reveal` needs no side channel
    /// to find them. Opaque to everything but `Keys::decrypt`.
    Sealed {
        /// The field the content was sealed under. A host binds its ciphertext to this
        /// name, so content moved elsewhere still decrypts under the name it was sealed
        /// with rather than under wherever it now sits.
        field: Ident,
        subject: Ident,
        id: String,
        /// Whatever the host stored. heklang never reads it.
        ///
        /// Text, because that is what a key store takes: the content type is not here
        /// but on `Expr::Reveal`, since it is the same at every run and putting a
        /// `Type` on every sealed value made `Value` a third larger for everything.
        content: Arc<str>,
    },
}

impl Value {
    pub fn str(value: impl Into<Arc<str>>) -> Self {
        Value::Str(value.into())
    }

    pub fn uuid(value: impl Into<Arc<str>>) -> Self {
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

    /// Rule 8's conversion table read the other way: JSON as the declared type reads it.
    ///
    /// [`Json::from_value`] writes and this reads, so the two are one table met twice
    /// rather than two kept in step. It takes the type rather than inferring one,
    /// because a `Money(2)` and a `Money(3)` are different types that read different
    /// values out of the same string and only a declaration knows which was written.
    pub fn from_json(json: &Json, ty: &Type, defs: Defs<'_>) -> Result<Value, Mismatch> {
        read_json(json, ty, defs, &mut Vec::new())
    }

    /// The plaintext behind a seal, read back as the type the declaration gave it.
    ///
    /// The same table again, entered from text rather than from JSON, because a key
    /// store encrypts bytes: a seal flattens whatever it held to its text form, so
    /// nothing but the declaration says whether that text was a number, a boolean or a
    /// decimal at a scale. This is the reading half of what a host does storing one,
    /// and it lives here so the two halves cannot drift apart.
    ///
    /// `ty` is the seal's content type, which `Value::Sealed` carries for this.
    pub fn from_sealed(text: &str, ty: &Type, defs: Defs<'_>) -> Result<Value, Mismatch> {
        let json = match ty {
            Type::Bool if text == "true" => Json::Bool(true),
            Type::Bool if text == "false" => Json::Bool(false),
            Type::Int | Type::Timestamp => Json::num(text),
            // A `String`, a `Uuid`, an enum variant and a scaled decimal are all text
            // already, so the seal held exactly what goes back.
            _ => Json::str(text),
        };
        Value::from_json(&json, ty, defs)
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
            // A stored seal is text, whatever the declaration says it means. Rule 12
            // keeps one out of every position that asks this, so it is reached by error
            // paths rather than by a program.
            Value::Sealed { subject, .. } => Type::sealed(Type::String, subject.clone()),
        }
    }

    /// A seal is transparent to the runtime: heklang models the key lifecycle rather
    /// than ciphertext, so a sealed position accepts the plain value as readily as a
    /// sealed one. The boundary it guards is a parse-time rule, which is what makes
    /// this safe to be lenient about. See `docs/effects.md` rule 12.
    pub fn has_type(&self, ty: &Type) -> bool {
        // A seal can sit under an `Opt` as well as at the top: an `Opt(String)` holding
        // sealed content and an `Opt(Sealed(String, x))` are the same shape to a runtime
        // that does not model ciphertext, and treating them as different made a folded
        // credential fail to fit the `state` it was folded into. `same_unsealed` is what
        // reads through both sides.
        //
        // The variants carrying a type or a name are matched here rather than left to
        // the fallback, because `ty()` builds one to answer with and that allocates.
        // This is asked on every write, so in a fold it is asked once per event.
        match (self, ty.peeled()) {
            // A seal is opaque: heklang holds the stored form and no key, so there is no
            // content type here to check one against. Rule 12's propagation checked it
            // where the write was written, which is what makes this safe to wave through
            // rather than merely convenient.
            (Value::Sealed { .. }, _) => true,
            (Value::Opt { inner, .. }, Type::Opt(want)) => inner.same_unsealed(want),
            (Value::List { inner, .. }, Type::List(want)) => inner.same_unsealed(want),
            (Value::Map { key, value, .. }, Type::Map(want_key, want_value)) => {
                key.same_unsealed(want_key) && value.same_unsealed(want_value)
            }
            (Value::Enum { ty: name, .. }, Type::Enum(want)) => name == want,
            (Value::Record { ty: name, .. }, Type::Record(want)) => name == want,
            // Everything left is a scalar, whose `ty()` is a variant with nothing in it.
            _ => self.ty().same_unsealed(ty),
        }
    }

    /// Whether two values hold the same content, reading a seal by what it stores. This
    /// is the question a test asks and the only place that asks it: `expect Shop[1] {
    /// shop_name: "Test Shop" }` names what was put in, and the column holds it sealed
    /// under the shop.
    ///
    /// Equality itself stays exact, because everywhere else a seal is part of what a value
    /// is. Here it is not: a test states an input and asks whether that is what came out,
    /// and nothing about the key lifecycle is in the question. `has_type` already answers
    /// the type half of it the same way.
    ///
    /// **A seal compares by its stored text.** heklang cannot read one without a key, so
    /// this is the only comparison available to it, and it is the right one: the harness
    /// stores content as it was given (`docs/host.md`), so a test still names what it put
    /// in. Against a real host's ciphertext it would answer false, which is what a test
    /// running there should get rather than a decrypt nothing asked for.
    ///
    /// The absent optional is why this is not only about content. An `Opt` carries its
    /// element type, and sealing one seals that type while leaving the value `None`
    /// (`docs/effects.md` rule 12: there was never a key). Two absent optionals then
    /// differed by a type nobody wrote, and both printed as `none`.
    pub fn same(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Sealed { content: left, .. }, Value::Sealed { content: right, .. }) => {
                left == right
            }
            (Value::Sealed { content, .. }, plain) | (plain, Value::Sealed { content, .. }) => {
                content.as_ref() == text(plain)
            }
            // Both absent, so only the element types could differ, and one of them may
            // carry a seal nobody wrote.
            (Value::Opt { value: None, .. }, Value::Opt { value: None, .. }) => true,
            (
                Value::Opt {
                    value: Some(left), ..
                },
                Value::Opt {
                    value: Some(right), ..
                },
            ) => left.same(right),
            (left, right) => left == right,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // Never the content. A checked program cannot print one, so this is what a
            // debug print of an intermediate frame shows rather than program output.
            Value::Sealed { subject, .. } => write!(f, "<sealed under {subject}>"),
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

impl<'a> Defs<'a> {
    /// The module's declarations, which is what an event field or a command parameter
    /// resolves against.
    pub fn of(program: &'a Program) -> Defs<'a> {
        Defs {
            local: &[],
            enums: &program.enums,
            records: &program.records,
        }
    }

    /// The same, with one projector's own enums in front of the module's, which is the
    /// precedence the parser applies to an entity column.
    pub fn in_projector(program: &'a Program, projector: &'a Projector) -> Defs<'a> {
        Defs {
            local: &projector.enums,
            ..Defs::of(program)
        }
    }
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
        Type::String => Value::Str("".into()),
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
        Literal::JsonNum(text) => Value::Json(Json::Num(text.clone())),
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
    Str(Arc<str>),
    Uuid(Arc<str>),
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

/// A decimal string read at a target scale, by exactly the rule a written literal
/// follows: widening is exact, and more places than the target holds is a failure
/// rather than a silent round. `ty` is the `Money(n)` or `Decimal(n)` being filled.
pub fn parse_scaled(text: &str, ty: &Type) -> Option<Value> {
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
    let lit = Number::new(value, places).resolve(ty).ok()?;
    Some(literal(&lit))
}

/// What a stored value was not.
///
/// Rule 8's table read backwards can fail, and it fails on **data** rather than on a
/// broken host: a record written before a field changed type reads exactly like this.
/// That is why it is its own answer and not `ErrorKind::Host`, which says the store is
/// broken when the store is fine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    /// Where in the value, outermost first: a field, then a record's field, and so on.
    /// Empty when the whole value is the wrong shape.
    pub path: Vec<Ident>,
    pub expected: Type,
    /// What was stored instead, as a shape rather than as content: a mismatch is
    /// reported to an operator, and the content may be personal.
    pub found: String,
}

impl fmt::Display for Mismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.path.is_empty() {
            write!(f, "{}: ", self.path.join("."))?;
        }
        write!(f, "expected {}, stored {}", self.expected, self.found)
    }
}

/// The shape of a JSON value, for a message that does not quote its content.
fn shape(json: &Json) -> &'static str {
    match json {
        Json::Null => "null",
        Json::Bool(_) => "a boolean",
        Json::Num(_) => "a number",
        Json::Str(_) => "a string",
        Json::Arr(_) => "an array",
        Json::Obj(_) => "an object",
    }
}

fn key_from_text(text: &str, ty: &Type, defs: Defs<'_>) -> Option<Key> {
    Some(match ty {
        Type::Int => Key::Int(text.parse().ok()?),
        Type::String => Key::Str(text.into()),
        Type::Uuid => {
            uuid::Uuid::parse_str(text).ok()?;
            Key::Uuid(text.into())
        }
        Type::Timestamp => Key::Timestamp(text.parse().ok()?),
        Type::Enum(name) => {
            let def = defs.enum_def(name)?;
            if !def.has(text) {
                return None;
            }
            Key::Enum {
                ty: name.clone(),
                variant: text.to_string(),
            }
        }
        _ => return None,
    })
}

fn read_json(
    json: &Json,
    ty: &Type,
    defs: Defs<'_>,
    path: &mut Vec<Ident>,
) -> Result<Value, Mismatch> {
    let wrong = |found: String, path: &Vec<Ident>| Mismatch {
        path: path.clone(),
        expected: ty.clone(),
        found,
    };
    Ok(match (ty, json) {
        // A sealed position reads as text, whatever it is declared to hold. What a host
        // stored is a seal's content and heklang cannot open it, so the declared inner
        // type is carried rather than applied: `reveal` is where it is used, against the
        // plaintext `Keys::decrypt` hands back.
        //
        // The seal itself is not in the JSON and cannot be, because it carries the
        // subject's id and that lives in a sibling field. `seal` rebuilds it as the
        // value enters a frame, which is the one place that has the whole event.
        (Type::Sealed(_, _), Json::Str(content)) => Value::str(content.as_str()),
        (Type::Sealed(_, _), _) => {
            return Err(wrong("a seal that is not text".into(), path));
        }

        // The one place `null` means something rather than being the wrong shape.
        (Type::Opt(inner), Json::Null) => Value::none(inner.as_ref().clone()),
        // Built by hand rather than through `Value::some`, so a declared inner type
        // survives verbatim: `Opt(Sealed(String, x))` must not come back as `Opt(String)`.
        (Type::Opt(inner), _) => Value::Opt {
            inner: inner.as_ref().clone(),
            value: Some(Box::new(read_json(json, inner, defs, path)?)),
        },

        (Type::Bool, Json::Bool(value)) => Value::Bool(*value),
        // A JSON number is text now, so an `Int` column checks that the text is one.
        // `10.5` stored where an `Int` is declared is a mismatch rather than a truncation.
        (Type::Int, Json::Num(text)) => match text.parse::<i64>() {
            Ok(value) => Value::Int(value),
            Err(_) => return Err(wrong("a number that is not a whole one".into(), path)),
        },
        (Type::String, Json::Str(value)) => Value::str(value.as_str()),
        // Checked, not merely typed. Every other shaped type in this table validates
        // (`parse_scaled` for a decimal, `def.has` for a variant), and a `Uuid` that
        // is not one reaches further than either: it becomes a tag, a read-model key,
        // and a seed `Uuid.derive` then fails on, from a value that entered through a
        // host boundary rather than through the parser.
        (Type::Uuid, Json::Str(value)) => {
            if uuid::Uuid::parse_str(value).is_err() {
                return Err(wrong("text that is not a uuid".into(), path));
            }
            Value::uuid(value.as_str())
        }
        // Microseconds, which is what a `Timestamp` is and what `Json::from_value`
        // wrote. A host whose envelope holds RFC 3339 converts with `timestamp` first.
        (Type::Timestamp, Json::Num(micros)) => match micros.parse::<i64>() {
            Ok(micros) => Value::Timestamp(micros),
            Err(_) => {
                return Err(wrong(
                    "a number that is not whole microseconds".into(),
                    path,
                ));
            }
        },
        // A string at the target scale, so nothing was lost to a float on the way out
        // and nothing is lost coming back.
        (Type::Money(_) | Type::Decimal(_), Json::Str(text)) => {
            parse_scaled(text, ty).ok_or_else(|| wrong("a decimal it cannot hold".into(), path))?
        }
        (Type::Enum(name), Json::Str(variant)) => {
            let known = defs.enum_def(name).is_some_and(|def| def.has(variant));
            if !known {
                return Err(wrong("a variant it does not have".into(), path));
            }
            Value::Enum {
                ty: name.clone(),
                variant: variant.clone(),
            }
        }
        (Type::Record(name), Json::Obj(fields)) => {
            let Some(def) = defs.records.iter().find(|def| &def.name == name) else {
                return Err(wrong("an object".into(), path));
            };
            let mut built = BTreeMap::new();
            for field in &def.fields {
                // An absent key reads as `null`, so a missing optional is absent and a
                // missing required field is the mismatch it actually is.
                let found = fields.get(&field.name).unwrap_or(&Json::Null);
                path.push(field.name.clone());
                let value = read_json(found, &field.ty, defs, path)?;
                path.pop();
                built.insert(field.name.clone(), value);
            }
            Value::Record {
                ty: name.clone(),
                fields: built,
            }
        }
        (Type::List(inner), Json::Arr(items)) => {
            let mut built = Vec::with_capacity(items.len());
            for (index, item) in items.iter().enumerate() {
                path.push(index.to_string());
                built.push(read_json(item, inner, defs, path)?);
                path.pop();
            }
            Value::list(inner.as_ref().clone(), built)
        }
        // A map's keys were strings on the way out, so they are read back as the key
        // type rather than guessed from their text.
        (Type::Map(key, value), Json::Obj(entries)) => {
            let mut built = BTreeMap::new();
            for (text, held) in entries {
                let Some(at) = key_from_text(text, key, defs) else {
                    return Err(wrong("a key it cannot hold".into(), path));
                };
                path.push(text.clone());
                let held = read_json(held, value, defs, path)?;
                path.pop();
                built.insert(at, held);
            }
            Value::map(key.as_ref().clone(), value.as_ref().clone(), built)
        }
        // Rule 8 leaves a `Json` field's shape unchecked in both directions.
        (Type::Json, _) => Value::Json(json.clone()),
        _ => return Err(wrong(shape(json).to_string(), path)),
    })
}

/// A JSON value, for HTTP bodies (rule 8). Hand-rolled rather than a dependency,
/// because the interpreter constructs responses and never parses one. `Obj` is
/// ordered, which is where rule 14's defined iteration order comes from: the same
/// object built twice serialises byte for byte the same.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Json {
    Null,
    Bool(bool),
    /// A JSON number, as the exact text it was written or received as. Not an `i64`
    /// and never an `f64`: a body carries `10.5` and `0.30000000000000004` as often as
    /// it carries `3`, and both have to survive a round trip byte for byte. One variant
    /// rather than an integer one beside it, because two spellings of `3` that compare
    /// unequal is what an `expect http.post(url, { .. })` would trip over.
    Num(String),
    Str(String),
    Arr(Vec<Json>),
    Obj(BTreeMap<String, Json>),
}

impl Json {
    pub fn str(value: impl Into<String>) -> Self {
        Json::Str(value.into())
    }

    /// A whole number, rendered once here so no call site has to remember that a
    /// `Json::Num` holds text. Every integer reaches JSON through this, which is what
    /// keeps one spelling of `3`.
    pub fn int(value: i64) -> Self {
        Json::Num(value.to_string())
    }

    /// The exact text of a number, for a host parsing a foreign body. Whatever the wire
    /// said, unrounded and unreformatted: `10.50` stays `10.50`.
    ///
    /// **The caller owes it a valid JSON number.** Nothing checks, because the text is
    /// here to be handed back byte for byte and a host that parsed a body has already
    /// lexed one. Text that is not a number serialises as itself, which is invalid JSON
    /// on the way out: the trade taken to stop `10.50` becoming `10.5`.
    pub fn num(text: impl Into<String>) -> Self {
        Json::Num(text.into())
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
            // The stored form, which is what a host wrote and what it takes back. Rule
            // 12 rejects sealed content in a request body at parse time, so the only
            // things that reach this are the writing paths and a leak that could not
            // have been checked, and neither wants the plaintext heklang has not got.
            Value::Sealed { content, .. } => Json::Str(content.to_string()),
            Value::Bool(value) => Json::Bool(*value),
            Value::Int(value) => Json::int(*value),
            Value::Decimal { units, scale } | Value::Money { units, scale } => {
                Json::Str(scaled::text(*units, *scale))
            }
            Value::Str(value) | Value::Uuid(value) => Json::Str(value.to_string()),
            Value::Timestamp(micros) => Json::int(*micros),
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
                Json::obj([("body", body.clone()), ("status", Json::int(*status))])
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
            Json::Num(value) => f.write_str(value),
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

/// A written `Timestamp`, which is a string in a Timestamp position: there is no
/// timestamp token, so the target type is what makes the string one, exactly as it
/// makes one a `Uuid`. `Timestamp.parse` is this same reading applied to text that
/// arrives at run time and may be anything, which is why one function serves both.
///
/// RFC 3339 to epoch microseconds, hand-rolled rather than a dependency: the shapes
/// that arrive on a webhook are a small set, and a calendar library is a large surface
/// and a large opinion for one function.
pub fn timestamp(text: &str) -> Option<i64> {
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

/// A moment's calendar fields in UTC: year, month, day, hour, minute, second. The
/// inverse of `days_from_civil`, by the same algorithm, so a value that goes out
/// through these and back through `from_parts` is the same value to the second.
///
/// Euclidean division rather than truncating, so a moment before 1970 lands on the day
/// it is in rather than the one after it.
pub fn parts(micros: i64) -> (i64, i64, i64, i64, i64, i64) {
    const DAY: i64 = 86_400_000_000;
    let days = micros.div_euclid(DAY);
    let seconds = micros.rem_euclid(DAY) / 1_000_000;

    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };

    (
        if month <= 2 { year + 1 } else { year },
        month,
        day,
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60,
    )
}

/// A moment from its calendar fields, or `None` when they do not name one. The range
/// checks are the same ones `timestamp` applies to written text, because a date is a
/// date however it was reached.
///
/// Sub-second precision is not a parameter and is not preserved: a moment built from
/// parts is on the second.
pub fn from_parts(
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
) -> Option<i64> {
    if !(1..=12).contains(&month)
        || day < 1
        || day > days_in_month(year, month)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=59).contains(&second)
    {
        return None;
    }
    days_from_civil(year, month, day)
        .checked_mul(86_400)?
        .checked_add(hour * 3600 + minute * 60 + second)?
        .checked_mul(1_000_000)
}
