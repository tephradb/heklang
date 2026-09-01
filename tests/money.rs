//! `docs/money.md` as executable tests. The operator table is the whole reason `Money`
//! is a distinct type from `Decimal`, so it is the part worth pinning down.

use heklang::{Interpreter, Outcome, Program, Value, parse};

/// A command whose body is `let a = <expr>`, so a type error surfaces as a runtime
/// error with the operands named.
fn probe(params: &str, body: &str) -> String {
    format!("command Probe({params}) {{\n  let a = {body}\n  return\n}}\n")
}

fn parsed(params: &str, body: &str) -> Program {
    parse(&probe(params, body)).unwrap_or_else(|err| panic!("expected this to parse: {err}"))
}

/// The probe's parse error. The operator table is checked before the program runs, so
/// a row that is not in it never reaches an interpreter.
fn rejected(params: &str, body: &str) -> String {
    parse(&probe(params, body))
        .expect_err("expected the operator table to reject this")
        .text()
}

/// Runs the probe and returns the error, or `None` when the expression was fine.
fn evaluate(params: &str, body: &str, args: Vec<(&str, Value)>) -> Option<String> {
    let program = parsed(params, body);
    let mut interpreter = Interpreter::new(&program);
    match interpreter.run("Probe", args) {
        Ok(execution) => {
            assert!(matches!(execution.outcome, Outcome::Ok(_)));
            None
        }
        Err(err) => Some(err.kind.to_string()),
    }
}

fn money(units: i64) -> Value {
    Value::money(units, 2)
}

#[test]
fn two_amounts_add_and_subtract() {
    assert_eq!(
        evaluate(
            "a: Money(2), b: Money(2)",
            "a + b",
            vec![("a", money(2_599)), ("b", money(100))],
        ),
        None
    );
    assert_eq!(
        evaluate(
            "a: Money(2), b: Money(2)",
            "a - b",
            vec![("a", money(2_599)), ("b", money(100))],
        ),
        None
    );
}

#[test]
fn a_rate_scales_an_amount() {
    // Exact, so the bare operator is allowed.
    assert_eq!(
        evaluate(
            "a: Money(2), rate: Decimal(2)",
            "a * rate",
            vec![("a", money(2_000)), ("rate", Value::decimal(50, 2))],
        ),
        None
    );
    // Not exact, so it is an error naming the way out rather than a silent round.
    assert_eq!(
        evaluate(
            "a: Money(2), rate: Decimal(2)",
            "a * rate",
            vec![("a", money(2_599)), ("rate", Value::decimal(90, 2))],
        )
        .as_deref(),
        Some("`*` on Money is not exact here, use `mul` with an explicit rounding mode")
    );
    // With a rounding mode it is fine, and the result keeps the amount's scale.
    assert_eq!(
        evaluate(
            "a: Money(2), rate: Decimal(2)",
            "a.mul(rate, HalfUp)",
            vec![("a", money(2_599)), ("rate", Value::decimal(90, 2))],
        ),
        None
    );
}

#[test]
fn an_amount_divided_by_an_amount_is_a_ratio() {
    assert_eq!(
        evaluate(
            "a: Money(2), b: Money(2)",
            "a / b",
            vec![("a", money(2_000)), ("b", money(1_000))],
        ),
        None
    );
}

#[test]
fn two_amounts_multiplied_is_a_type_error() {
    let message = rejected("a: Money(2), b: Money(2)", "a * b");
    assert!(
        message.starts_with("cannot apply `*` to Money(2) and Money(2)"),
        "got: {message}"
    );
    assert!(
        message.contains("two amounts multiplied is not an amount"),
        "the message names the mistake, not only the missing row: {message}"
    );
}

#[test]
fn an_amount_plus_a_bare_decimal_is_a_type_error() {
    // The real-world bug: adding a tax rate to a total.
    let message = rejected("total: Money(2), rate: Decimal(4)", "total + rate");
    assert!(
        message.starts_with("cannot apply `+` to Money(2) and Decimal(4)"),
        "got: {message}"
    );
    assert!(
        message.contains("adds a rate to an amount"),
        "got: {message}"
    );
}

#[test]
fn two_scales_do_not_mix() {
    for (expr, op) in [("a + b", "+"), ("a > b", ">"), ("a == b", "==")] {
        let message = rejected("a: Money(2), b: Money(3)", expr);
        assert!(
            message.starts_with(&format!("cannot apply `{op}` to Money(2) and Money(3)")),
            "for `{expr}`, got: {message}"
        );
    }
    // A comparison is held to the table the arithmetic is, because rescaling one side
    // to answer it would be answering a different question.
    assert!(
        rejected("a: Money(2), b: Money(3)", "a > b").contains("two amounts meet at one scale"),
    );
}

#[test]
fn the_result_keeps_the_amounts_scale() {
    let program = parse(
        "event @paid.out { amount: Money(3) }
command Split(total: Money(3)) {
  emit @paid.out { amount: total / 4 }
}
",
    )
    .expect("parses");
    let mut interpreter = Interpreter::new(&program);
    interpreter
        .run("Split", [("total", Value::money(1_000, 3))])
        .expect("exact");
    assert_eq!(
        interpreter.log()[0].event.field("amount"),
        Some(&Value::money(250, 3))
    );
}

#[test]
fn a_scale_is_a_storage_floor_not_a_currency() {
    // A zero-fraction amount in a `Money(2)` field is exact; nothing about the scale
    // claims which currency it is.
    let program = parse(
        "event @paid.out { amount: Money(2), currency: String }
command Pay(amount: Money(2), currency: String) {
  emit @paid.out { amount, currency }
}
",
    )
    .expect("currency is an ordinary field");
    let mut interpreter = Interpreter::new(&program);
    interpreter
        .run(
            "Pay",
            [
                ("amount", Value::money(500_000, 2)),
                ("currency", Value::str("JPY")),
            ],
        )
        .expect("ran");
    assert_eq!(
        interpreter.log()[0].event.field("amount"),
        Some(&Value::money(500_000, 2))
    );
}

#[test]
fn money_has_no_currency_in_its_display() {
    assert_eq!(money(2_599).to_string(), "25.99");
    assert_eq!(Value::money(2_599, 3).to_string(), "2.599");
    assert_eq!(Value::money(-50, 2).to_string(), "-0.50");
}

#[test]
fn there_is_no_currency_item() {
    let message = parse("currency USD\nevent @a.b { x: Int }\n")
        .expect_err("`currency` is not a declaration")
        .text();
    assert_eq!(
        message,
        "expected `enum`, `record`, `const`, `fn`, `event`, `refusal`, `command`, `guard`, \
         `projector`, `effect` or `test`, found `currency`"
    );
}

/// The units are an `i64`, so the scale has an end. Past it the type can hold no whole
/// units at all, and past *that* rendering one overflows its own divisor. The parser is
/// where it stops, because a program the checker accepts must not be able to take the
/// process down when a value is written out.
#[test]
fn a_scale_wider_than_the_units_is_refused_at_the_declaration() {
    for scale in [0, 2, 9, 18] {
        parse(&format!("event @e.happened {{ amount: Money({scale}) }}\n"))
            .unwrap_or_else(|err| panic!("Money({scale}) should be declarable: {err}"));
    }
    for scale in [19, 20, 30, 255] {
        let err = parse(&format!("event @e.happened {{ amount: Money({scale}) }}\n"))
            .expect_err(&format!("Money({scale}) cannot hold a whole unit"));
        assert!(
            err.text().contains("18 places is the most one can hold"),
            "{}",
            err.text()
        );
    }
    // `Decimal` is the same scaled integer and stops in the same place.
    parse("event @e.happened { ratio: Decimal(18) }\n").expect("Decimal(18) is declarable");
    parse("event @e.happened { ratio: Decimal(19) }\n").expect_err("Decimal(19) is not");
}

/// The widest scale still holds a whole unit, and renders it. This is the other half of
/// the bound: it is where the type stops being useful, not somewhere short of it.
#[test]
fn the_widest_scale_still_renders_a_whole_unit() {
    assert_eq!(
        heklang::scaled::text(9_000_000_000_000_000_000, 18),
        "9.000000000000000000"
    );
    assert_eq!(heklang::scaled::MAX_SCALE, 18);
}
