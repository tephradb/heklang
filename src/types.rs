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

use crate::ir::{BinOp, Ident, Literal, Number, Type};
use crate::scaled;

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

/// What `+`, `-`, `*`, `/` and `%` produce, or `None` when the pair is not in the
/// table and the expression is a mistake.
///
/// `docs/money.md` is most of this, and it is the entire reason `Money` is its own type
/// rather than a `Decimal(n)`: an amount times a rate is an amount, an amount over an
/// amount is a rate, and an amount plus a rate is a mistake. Collapse the two types and
/// every one of those becomes legal.
///
/// Scales never widen here. Two `Decimal`s add at one scale and `Money(2) + Money(3)`
/// is an error, the same rule in both, because a silent rescale is how a total loses a
/// cent. A literal is the one thing that widens, and it does so before it is a value at
/// all (`docs/literal-inference.md`).
pub fn arithmetic(op: BinOp, lhs: &Type, rhs: &Type) -> Option<Type> {
    use BinOp::{Add, Div, Mul, Rem, Sub};
    Some(match (lhs, op, rhs) {
        (Type::Int, Add | Sub | Mul | Div | Rem, Type::Int) => Type::Int,

        (Type::Decimal(a), Add | Sub, Type::Decimal(b)) if a == b => Type::Decimal(*a),
        (Type::Decimal(a), Mul | Div, Type::Int) => Type::Decimal(*a),
        (Type::Int, Mul, Type::Decimal(b)) => Type::Decimal(*b),

        (Type::Money(a), Add | Sub, Type::Money(b)) if a == b => Type::Money(*a),
        // An amount over an amount is a ratio, and a ratio is not money. This is the
        // row `type_of` used to get wrong, reporting `Money(n)` where the value is a
        // `Decimal(6)`, which is why the check had to wait for the table.
        (Type::Money(a), Div, Type::Money(b)) if a == b => Type::Decimal(scaled::RATIO_SCALE),
        (Type::Money(a), Mul | Div, Type::Int) => Type::Money(*a),
        (Type::Int, Mul, Type::Money(b)) => Type::Money(*b),
        (Type::Money(a), Mul, Type::Decimal(_)) => Type::Money(*a),
        (Type::Decimal(_), Mul, Type::Money(b)) => Type::Money(*b),

        _ => return None,
    })
}

/// Whether two types may be compared. Equality is any two of the same type; ordering
/// also needs one that orders. Scales do not meet here either, for the reason they do
/// not meet in `arithmetic`: `Money(2)` and `Money(3)` are different types, and a
/// comparison that rescaled one silently would answer a question nobody asked.
pub fn comparable(op: BinOp, lhs: &Type, rhs: &Type) -> bool {
    if lhs != rhs {
        return false;
    }
    match op {
        BinOp::Eq | BinOp::Ne => true,
        // `Timestamp` orders because a moment does, and because an application that
        // works with them asks "before" and "after" constantly. It is the same order
        // that makes one an entity key.
        _ => matches!(
            lhs,
            Type::Int | Type::Decimal(_) | Type::Money(_) | Type::String | Type::Timestamp
        ),
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
    /// One entry per argument, in order, so the length is the arity. The type is what
    /// the argument resolves against, and without it `xs.push([])` and
    /// `m.get(id).unwrap_or(0)` have no target. `None` is a parameter the table cannot
    /// fix: it still counts toward the arity, it just declares nothing.
    pub params: Vec<Option<Type>>,
    pub ret: Type,
}

/// A method's signature on a known receiver. One table rather than the two that used to
/// answer these questions separately, because a return type and an argument hint that
/// can disagree is a bug waiting for the first method whose shape changes.
///
/// `None` means the receiver has no such method. The tables are `docs/strings.md`,
/// `docs/containers.md`, `docs/optionals.md`, `docs/money.md` and `docs/effects.md`.
pub fn method_sig(receiver: &Type, method: &str) -> Option<Sig> {
    let sig = |params: Vec<Type>, ret: Type| {
        Some(Sig {
            params: params.into_iter().map(Some).collect(),
            ret,
        })
    };
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

        // Where the bare operator would have to round, `docs/money.md` makes the author
        // say how. The scale stays the amount's, because a rate applied to an amount is
        // still that amount.
        (Type::Money(scale), "mul") => Some(Sig {
            // The rate's scale is the author's rather than the amount's:
            // `docs/literal-inference.md` resolves `total.mul(0.9, HalfUp)` as a
            // `Decimal(1)`. So this parameter declares nothing and takes the default.
            params: vec![None, Some(Type::Rounding)],
            ret: Type::Money(*scale),
        }),
        (Type::Money(scale), "div") => sig(vec![Type::Int, Type::Rounding], Type::Money(*scale)),

        // A moment's calendar fields in UTC. These and `Timestamp.from_parts` are what
        // make calendar arithmetic writable as a `fn`, which is where the opinion about
        // month-end clamping belongs: the language gives the calendar and the author
        // gives the rule. Without them the deferral had nowhere to defer to.
        (Type::Timestamp, "year" | "month" | "day" | "hour" | "minute" | "second") => {
            sig(Vec::new(), Type::Int)
        }

        (Type::Outcome, "ok") => sig(Vec::new(), Type::Bool),
        (Type::Outcome, "code" | "message") => sig(Vec::new(), Type::opt(Type::String)),

        _ => None,
    }
}
