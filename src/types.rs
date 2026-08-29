//! The type rules, in one place.
//!
//! heklang checks types while it lowers rather than in a pass afterwards, because two
//! things need a type before the IR node exists: a numeric literal has to know its
//! scale to be built at all (`docs/literal-inference.md`), and a narrowed load lowers
//! to a different node than a plain one (`docs/optionals.md`). So the parser does the
//! walking. What it walks over is here: the tables and the relations, with no parser
//! state in them, which is what lets a checker outside the parser reuse them later.
//!
//! `docs/types.md` is the contract and `tests/types.rs` is the same rules as executable
//! tests. Change the doc, the tests and the code together.

use crate::ir::{Ident, Literal, Number, Type};

/// Looks through `T?` to the `T` a literal in that position is really making.
pub fn inner_of(ty: &Type) -> &Type {
    match ty {
        Type::Opt(inner) => inner,
        _ => ty,
    }
}

/// Whether a value of type `found` may be written where `ty` is declared: the same
/// type, or a bare `T` filling a `T?`. See `docs/optionals.md`.
pub fn fills(found: &Type, ty: &Type) -> bool {
    found == ty || matches!(ty, Type::Opt(inner) if inner.as_ref() == found)
}

/// One level, at the outside of the declared type, so a `List(String)` still does not
/// become a `List(String?)`. Only ever called where `fills` already holds.
pub fn wrap(lit: Literal, found: &Type, ty: &Type) -> Literal {
    match ty {
        Type::Opt(_) if found != ty => Literal::Some {
            inner: found.clone(),
            value: Box::new(lit),
        },
        _ => lit,
    }
}

/// The type a numeric literal takes when nothing declares one: scale 0 is a whole
/// number, anything else is a decimal at the scale written. Never `Money`, which is
/// only ever reached from an annotation (`docs/money.md`).
pub fn default_type(number: Number) -> Type {
    if number.scale == 0 {
        Type::Int
    } else {
        Type::Decimal(number.scale)
    }
}

/// `Opt` stays outermost, so an optional subject-bound field is an optional whose
/// content is sealed rather than a sealed optional. Everything that looks through an
/// optional (`inner_of`, narrowing, `fills`) keeps working with one extra unwrap.
pub fn seal(ty: Type, subject: Ident) -> Type {
    match ty {
        Type::Opt(inner) => Type::opt(seal(*inner, subject)),
        other => Type::sealed(other, subject),
    }
}

/// The two fields a `Response` carries. Parenless field access exists for these and
/// nothing else, so this doubles as the check.
pub fn response_field(name: &str) -> Option<Type> {
    Some(match name {
        "status" => Type::Int,
        "body" => Type::Json,
        _ => return None,
    })
}

/// What a method takes and what it gives back.
pub struct Sig {
    /// The declared type of each argument, in order. It is what an argument resolves
    /// against, so without it `xs.push([])` and `m.get(id).unwrap_or(0)` have no target.
    pub params: Vec<Type>,
    pub ret: Type,
}

/// A method's signature on a known receiver. One table rather than the two that used to
/// answer these questions separately, because a return type and an argument hint that
/// can disagree is a bug waiting for the first method whose shape changes.
///
/// `None` means the receiver has no such method. The tables are `docs/strings.md`,
/// `docs/containers.md`, `docs/optionals.md`, `docs/money.md` and `docs/effects.md`.
pub fn method_sig(receiver: &Type, method: &str) -> Option<Sig> {
    let sig = |params: Vec<Type>, ret: Type| Some(Sig { params, ret });
    match (receiver, method) {
        (Type::String, "trim" | "lower" | "upper") => sig(Vec::new(), Type::String),
        (Type::String, "strip_prefix" | "after_last") => sig(vec![Type::String], Type::String),
        (Type::String, "len") => sig(Vec::new(), Type::Int),
        (Type::String, "is_empty") => sig(Vec::new(), Type::Bool),
        (Type::String, "contains" | "starts_with") => sig(vec![Type::String], Type::Bool),
        (Type::String, "to_int") => sig(Vec::new(), Type::opt(Type::Int)),
        (Type::String, "to_uuid") => sig(Vec::new(), Type::opt(Type::Uuid)),

        (Type::Json, "string") => sig(vec![Type::String], Type::opt(Type::String)),
        (Type::Json, "int") => sig(vec![Type::String], Type::opt(Type::Int)),
        (Type::Json, "bool") => sig(vec![Type::String], Type::opt(Type::Bool)),
        (Type::Json, "json") => sig(vec![Type::String], Type::opt(Type::Json)),
        (Type::Json, "array") => sig(vec![Type::String], Type::opt(Type::list(Type::Json))),

        (Type::Opt(inner), "unwrap_or") => {
            sig(vec![inner.as_ref().clone()], inner.as_ref().clone())
        }
        (Type::Opt(_), "is_some" | "is_none") => sig(Vec::new(), Type::Bool),

        (Type::List(inner), "first") => sig(Vec::new(), Type::opt(inner.as_ref().clone())),
        (Type::List(inner), "push" | "remove") => {
            sig(vec![inner.as_ref().clone()], receiver.clone())
        }
        (Type::List(inner), "contains") => sig(vec![inner.as_ref().clone()], Type::Bool),
        (Type::List(_), "len") => sig(Vec::new(), Type::Int),
        (Type::List(_), "is_empty") => sig(Vec::new(), Type::Bool),

        (Type::Map(key, value), "get") => sig(
            vec![key.as_ref().clone()],
            Type::opt(value.as_ref().clone()),
        ),
        (Type::Map(key, value), "set") => sig(
            vec![key.as_ref().clone(), value.as_ref().clone()],
            receiver.clone(),
        ),
        (Type::Map(key, _), "remove") => sig(vec![key.as_ref().clone()], receiver.clone()),
        (Type::Map(key, _), "contains") => sig(vec![key.as_ref().clone()], Type::Bool),
        (Type::Map(key, _), "keys") => sig(Vec::new(), Type::list(key.as_ref().clone())),
        (Type::Map(_, value), "values") => sig(Vec::new(), Type::list(value.as_ref().clone())),
        (Type::Map(..), "len") => sig(Vec::new(), Type::Int),
        (Type::Map(..), "is_empty") => sig(Vec::new(), Type::Bool),

        _ => None,
    }
}
