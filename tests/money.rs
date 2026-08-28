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
    assert_eq!(
        evaluate(
            "a: Money(2), b: Money(2)",
            "a * b",
            vec![("a", money(2_599)), ("b", money(100))],
        )
        .as_deref(),
        Some("cannot apply `*` to Money(2) and Money(2)")
    );
}

#[test]
fn an_amount_plus_a_bare_decimal_is_a_type_error() {
    // The real-world bug: adding a tax rate to a total.
    assert_eq!(
        evaluate(
            "total: Money(2), rate: Decimal(4)",
            "total + rate",
            vec![("total", money(2_599)), ("rate", Value::decimal(825, 4))],
        )
        .as_deref(),
        Some("cannot apply `+` to Money(2) and Decimal(4)")
    );
}

#[test]
fn two_scales_do_not_mix() {
    assert_eq!(
        evaluate(
            "a: Money(2), b: Money(3)",
            "a + b",
            vec![("a", money(2_599)), ("b", Value::money(2_599, 3))],
        )
        .as_deref(),
        Some("cannot apply `+` to Money(2) and Money(3)")
    );
    assert_eq!(
        evaluate(
            "a: Money(2), b: Money(3)",
            "a > b",
            vec![("a", money(2_599)), ("b", Value::money(2_599, 3))],
        )
        .as_deref(),
        Some("cannot apply `>` to Money(2) and Money(3)")
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
        .message;
    assert_eq!(
        message,
        "expected `enum`, `record`, `const`, `fn`, `event`, `command`, `projector`, \
         `effect` or `test`, found `currency`"
    );
}
