//! Rule 8's table, met from both ends, over generated values.
//!
//! `tests/effects.rs` states the same table as a hand-written list of thirteen cases,
//! which is the readable version and stays. This is the exhaustive one: it draws a type
//! and a value of that type and asserts the two directions agree, so a row nobody
//! thought to write down is covered anyway.
//!
//! **The generator draws a type and derives the value from it**, never the two
//! independently. Every conversion here is keyed on a declaration, so an independently
//! drawn pair would spend its budget on shapes no program can declare.
//!
//! A failing case writes its seed to `tests/roundtrip.proptest-regressions`, which is
//! worth committing: a counterexample that is not pinned is a check that fired once. A
//! seed is only meaningful while the strategy that consumed it is unchanged, so prune
//! the file when a generator changes rather than letting it replay something else.
//!
//! **Edge lists beat uniform draws.** `any::<i64>()` reaches `i64::MIN` with probability
//! 2^-64, and a conversion's bugs live at its boundaries: the widest integer, the empty
//! string, a scale of zero, the last scale that holds a whole unit. Each type mixes a
//! hand-written edge list with a uniform tail, weighted toward the edges.

use std::sync::LazyLock;

use heklang::ir::Type;
use heklang::scaled::MAX_SCALE;
use heklang::{Defs, Json, Key, Mismatch, Program, Value, parse};
use proptest::prelude::*;

/// The declarations a generated value resolves against. The same shapes
/// `tests/effects.rs` uses, so the two versions of this table speak about one program.
const SHAPES: &str = "enum Tier { @default Free, Paid }
record Line { sku: String, qty: Int, price: Money(3), note: String? }
event @thing.happened { id: Uuid, tier: Tier }
";

/// Parsed once: a property runs hundreds of cases, and re-parsing the fixture for each
/// would cost more than the conversions under test.
static SHAPES_PROGRAM: LazyLock<Program> =
    LazyLock::new(|| parse(SHAPES).expect("the fixture parses"));

fn shapes() -> &'static Program {
    &SHAPES_PROGRAM
}

// --- the generators --------------------------------------------------------

/// Every type a value can be written to JSON from and read back as.
///
/// `Rounding`, `Response` and `Outcome` are absent because there is no reader for them,
/// which is a property of its own below rather than a gap here. `Sealed` is absent
/// because it is not spellable and cannot be rebuilt from JSON alone: the subject id
/// lives in a sibling field, so only `interp::seal` has what it takes.
fn any_type() -> impl Strategy<Value = Type> {
    let leaf = prop_oneof![
        Just(Type::Bool),
        Just(Type::Int),
        Just(Type::String),
        Just(Type::Uuid),
        Just(Type::Timestamp),
        Just(Type::Json),
        Just(Type::Enum("Tier".to_owned())),
        Just(Type::Record("Line".to_owned())),
        (0u8..=MAX_SCALE).prop_map(Type::Money),
        (0u8..=MAX_SCALE).prop_map(Type::Decimal),
    ];
    leaf.prop_recursive(2, 8, 2, |inner| {
        prop_oneof![
            // An `Opt` never wraps an `Opt`. `String?` is `Opt(String)` and there is no
            // `String??` to declare, so generating one would fail a property about a
            // shape no program has.
            2 => inner.clone().prop_map(|ty| match ty {
                Type::Opt(_) => ty,
                other => Type::opt(other),
            }),
            1 => inner.clone().prop_map(Type::list),
            1 => inner.prop_map(|value| Type::map(Type::String, value)),
        ]
    })
}

fn value_for(ty: &Type) -> BoxedStrategy<Value> {
    match ty {
        Type::Bool => any::<bool>().prop_map(Value::Bool).boxed(),
        Type::Int => int().prop_map(Value::Int).boxed(),
        Type::String => text().prop_map(Value::str).boxed(),
        Type::Uuid => uuid().prop_map(Value::uuid).boxed(),
        Type::Timestamp => int().prop_map(Value::Timestamp).boxed(),
        Type::Money(scale) => {
            let scale = *scale;
            int()
                .prop_map(move |units| Value::money(units, scale))
                .boxed()
        }
        Type::Decimal(scale) => {
            let scale = *scale;
            int()
                .prop_map(move |units| Value::decimal(units, scale))
                .boxed()
        }
        Type::Enum(name) => {
            let name = name.clone();
            prop_oneof![Just("Free".to_owned()), Just("Paid".to_owned())]
                .prop_map(move |variant| Value::Enum {
                    ty: name.clone(),
                    variant,
                })
                .boxed()
        }
        Type::Record(name) => record(name.clone()).boxed(),
        Type::Json => json().prop_map(Value::Json).boxed(),
        Type::Opt(inner) => {
            let inner = (**inner).clone();
            let empty = inner.clone();
            prop_oneof![
                // A JSON null inside a `Json?` is the one value an optional cannot be
                // told from: both write `null`, and the reader answers `none`. Left out
                // here and pinned by name below.
                1 => Just(Value::none(empty)),
                3 => value_for(&inner)
                    .prop_filter("a json null cannot be told from none", |value| {
                        !matches!(value, Value::Json(Json::Null))
                    })
                    .prop_map(Value::some),
            ]
            .boxed()
        }
        Type::List(inner) => {
            let inner = (**inner).clone();
            let declared = inner.clone();
            prop::collection::vec(value_for(&inner), 0..3)
                .prop_map(move |items| Value::list(declared.clone(), items))
                .boxed()
        }
        Type::Map(key, value) => {
            // Keys come from a small pool: a map writes itself keyed by rendered text,
            // so two keys that render alike would lose an entry and the property would
            // fail for a reason that is not a bug.
            let key_ty = (**key).clone();
            let value_ty = (**value).clone();
            prop::collection::btree_map(
                prop_oneof![
                    Just("a".to_owned()),
                    Just("b".to_owned()),
                    Just("c".to_owned())
                ],
                value_for(&value_ty),
                0..3,
            )
            .prop_map(move |entries| {
                Value::map(
                    key_ty.clone(),
                    value_ty.clone(),
                    entries
                        .into_iter()
                        .map(|(key, value)| (Key::Str(key.into()), value)),
                )
            })
            .boxed()
        }
        other => panic!("no generator for {other}"),
    }
}

fn typed_value() -> impl Strategy<Value = (Type, Value)> {
    any_type().prop_flat_map(|ty| {
        let declared = ty.clone();
        value_for(&ty).prop_map(move |value| (declared.clone(), value))
    })
}

fn int() -> impl Strategy<Value = i64> {
    prop_oneof![
        4 => prop_oneof![
            Just(i64::MIN),
            Just(i64::MAX),
            Just(-1i64),
            Just(0),
            Just(1),
            Just(i64::from(i32::MAX) + 1),
        ],
        1 => any::<i64>(),
    ]
}

fn text() -> impl Strategy<Value = String> {
    prop_oneof![
        4 => prop_oneof![
            Just(String::new()),
            Just(" ".to_owned()),
            Just("\"".to_owned()),
            Just("\\".to_owned()),
            Just("\n\r\t".to_owned()),
            Just("\u{0}".to_owned()),
            Just("日本語".to_owned()),
            // One grapheme, several code points.
            Just("\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}".to_owned()),
            // Text that looks like another JSON type.
            Just("42".to_owned()),
            Just("true".to_owned()),
            Just("null".to_owned()),
        ],
        1 => ".{0,32}",
    ]
}

fn uuid() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("00000000-0000-0000-0000-000000000000".to_owned()),
        Just("ffffffff-ffff-ffff-ffff-ffffffffffff".to_owned()),
        Just("FFFFFFFF-FFFF-FFFF-FFFF-FFFFFFFFFFFF".to_owned()),
        Just("0190d1a1-0000-7000-8000-000000000001".to_owned()),
    ]
}

fn record(name: String) -> impl Strategy<Value = Value> {
    (text(), int(), int(), prop::option::of(text())).prop_map(move |(sku, qty, price, note)| {
        Value::record(
            name.clone(),
            [
                ("sku", Value::str(sku)),
                ("qty", Value::Int(qty)),
                ("price", Value::money(price, 3)),
                (
                    "note",
                    match note {
                        Some(text) => Value::some(Value::str(text)),
                        None => Value::none(Type::String),
                    },
                ),
            ],
        )
    })
}

fn json() -> impl Strategy<Value = Json> {
    let leaf = prop_oneof![
        Just(Json::Null),
        any::<bool>().prop_map(Json::Bool),
        int().prop_map(|value| Json::num(value.to_string())),
        text().prop_map(Json::Str),
    ];
    leaf.prop_recursive(3, 12, 3, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..3).prop_map(Json::Arr),
            prop::collection::btree_map("[a-z]{1,3}", inner, 0..3).prop_map(Json::Obj),
        ]
    })
}

// --- the table -------------------------------------------------------------

proptest! {
    /// Everything `Json::from_value` can write, read back as the type that wrote it.
    /// The reader takes the declared type rather than inferring one from the JSON, so
    /// this is the property that says the two directions agree about what a type means.
    #[test]
    fn the_conversion_table_round_trips((ty, value) in typed_value()) {
        let written = Json::from_value(&value);
        let read = Value::from_json(&written, &ty, Defs::of(shapes()));
        prop_assert_eq!(read, Ok(value));
    }

    /// The seal path reads the same table from text rather than from JSON, because a key
    /// store hands back a string and only the declaration says what that string was.
    /// Scalars are the whole of what a seal can hold.
    #[test]
    fn a_sealed_scalar_reads_back_from_its_text(
        (ty, value) in typed_value().prop_filter("a seal holds a scalar", |(ty, _)| {
            matches!(
                ty,
                Type::Bool | Type::Int | Type::String | Type::Uuid | Type::Timestamp
            )
        })
    ) {
        let defs = Defs::of(shapes());
        let text = heklang::value::text(&value);
        prop_assert_eq!(Value::from_sealed(&text, &ty, defs), Ok(value));
    }
}

/// A scale is the declaration's, not the text's. Widening is exact and silent; more
/// written places than the target holds is an error rather than a round.
#[test]
fn a_scale_comes_from_the_declaration_and_widens_exactly() {
    let defs = Defs::of(shapes());
    assert_eq!(
        Value::from_json(&Json::str("1.5"), &Type::Money(2), defs),
        Ok(Value::money(150, 2))
    );
    assert_eq!(
        Value::from_json(&Json::str("1.5"), &Type::Money(3), defs),
        Ok(Value::money(1_500, 3))
    );
    let err = Value::from_json(&Json::str("1.555"), &Type::Money(2), defs)
        .expect_err("three places do not fit two");
    assert_eq!(err.expected, Type::Money(2));
}

/// The types with no reader. Each is written by `Json::from_value` and each fails on the
/// way back, which is what stops a helpful arm being added later and letting a response
/// body or a rounding mode reach a stored position.
#[test]
fn the_types_that_only_go_one_way_fail_on_the_way_back() {
    let defs = Defs::of(shapes());
    for ty in [Type::Rounding, Type::Response, Type::Outcome] {
        assert!(
            Value::from_json(&Json::Null, &ty, defs).is_err(),
            "{ty} must have no reader"
        );
        assert!(
            Value::from_json(&Json::str("anything"), &ty, defs).is_err(),
            "{ty} must have no reader"
        );
    }
}

/// A seal writes the content a host stored and reads back as a bare string, because the
/// subject and the id it is filed under are not in the JSON: they live in a sibling
/// field, and only the place that has the whole event can put them back.
#[test]
fn a_seal_does_not_round_trip_through_json_alone() {
    let defs = Defs::of(shapes());
    let sealed = Value::Sealed {
        field: "email".to_owned(),
        subject: "customer_id".to_owned(),
        id: "7".to_owned(),
        content: "Y2lwaGVy".into(),
    };
    assert_eq!(Json::from_value(&sealed), Json::str("Y2lwaGVy"));

    let ty = Type::sealed(Type::String, "customer_id".to_owned());
    assert_eq!(
        Value::from_json(&Json::str("Y2lwaGVy"), &ty, defs),
        Ok(Value::str("Y2lwaGVy")),
        "a seal reads back as its content, not as a seal"
    );
}

/// The one value an optional cannot be told from absence: both write `null`, and the
/// reader has to pick one. It picks `none`. Inherent rather than fixable, and the single
/// exception to the property above.
#[test]
fn a_json_null_inside_an_optional_reads_back_as_absent() {
    let defs = Defs::of(shapes());
    let value = Value::some(Value::Json(Json::Null));
    assert_eq!(Json::from_value(&value), Json::Null);
    assert_eq!(
        Value::from_json(&Json::Null, &Type::opt(Type::Json), defs),
        Ok(Value::none(Type::Json))
    );
}

/// A `Uuid` and an enum variant are checked, not merely typed. Both arrive from outside
/// the program (a request body, a stored record), where the parser's own check on a
/// written literal cannot reach.
#[test]
fn a_uuid_and_a_variant_are_checked_rather_than_taken() {
    let defs = Defs::of(shapes());
    assert!(Value::from_json(&Json::str("not-a-uuid"), &Type::Uuid, defs).is_err());
    let tier = Type::Enum("Tier".to_owned());
    assert_eq!(
        Value::from_json(&Json::str("Paid"), &tier, defs),
        Ok(Value::Enum {
            ty: "Tier".to_owned(),
            variant: "Paid".to_owned()
        })
    );
    let err: Mismatch = Value::from_json(&Json::str("Platinum"), &tier, defs)
        .expect_err("an undeclared variant is not a Tier");
    assert_eq!(err.expected, tier);
}
