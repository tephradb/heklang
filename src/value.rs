use std::collections::BTreeMap;
use std::fmt;

use crate::currency::Currency;
use crate::ir::{EventPath, Ident, Type};
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
    Money(i64),
    Rounding(Rounding),
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
            Value::Money(_) => Type::Money,
            Value::Rounding(_) => Type::Rounding,
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
            Value::Money(units) => {
                scaled::write(f, *units, self.currency.scale)?;
                write!(f, " {}", self.currency.code)
            }
            Value::Rounding(mode) => write!(f, "{mode}"),
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
