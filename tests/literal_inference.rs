use heklang::ir::{Expr, ExprId, Literal};
use heklang::parse;

const PRELUDE: &str = "event @order.placed { order_id: Uuid, customer_id: Int, total: Money(2) }
command Probe(total: Money(2), spend: Money(2), count: Int, rate: Decimal(4)) {
";

fn literals(body: &str) -> Result<Vec<String>, String> {
    let source = format!("{PRELUDE}{body}\n}}\n");
    let program = parse(&source).map_err(|err| err.message)?;
    let command = &program.commands[0];

    let mut found = Vec::new();
    let mut index = 0u32;
    while let Some(node) = command.exprs.get(ExprId(index)) {
        if let Expr::Lit(lit) = node {
            found.push(describe(lit));
        }
        index += 1;
    }
    Ok(found)
}

fn describe(lit: &Literal) -> String {
    match lit {
        Literal::Bool(value) => format!("Bool({value})"),
        Literal::Int(value) => format!("Int({value})"),
        Literal::Decimal { units, scale } => format!("Decimal({units}, scale {scale})"),
        Literal::Str(value) => format!("Str({value:?})"),
        Literal::Uuid(value) => format!("Uuid({value})"),
        Literal::Timestamp(micros) => format!("Timestamp({micros})"),
        Literal::None(inner) => format!("None({inner})"),
        Literal::Some { inner, value } => format!("Some({inner}, {})", describe(value)),
        Literal::Money { units, scale } => format!("Money({units}, scale {scale})"),
        Literal::Enum { ty, variant } => format!("{ty}.{variant}"),
        Literal::Rounding(mode) => format!("Rounding({mode})"),
        Literal::List { inner, items } => format!("List({inner}, {})", items.len()),
        Literal::Record { ty, fields } => format!("{ty}({} fields)", fields.len()),
        Literal::EmptyJson => "EmptyJson".to_string(),
        Literal::EmptyMap(key, value) => format!("EmptyMap({key}, {value})"),
    }
}

fn check(body: &str, expected: &[&str]) {
    match literals(body) {
        Ok(found) => assert_eq!(found, expected, "for `{body}`"),
        Err(err) => panic!("`{body}` failed to parse: {err}"),
    }
}

fn check_error(body: &str, expected: &str) {
    match literals(body) {
        Ok(found) => panic!("`{body}` unexpectedly parsed, giving {found:?}"),
        Err(err) => assert_eq!(err, expected, "for `{body}`"),
    }
}

#[test]
fn annotations_drive_resolution() {
    check("state open: Int = fold 0\nreturn", &["Int(0)"]);
    check(
        "state spent: Money(2) = fold 0\nreturn",
        &["Money(0, scale 2)"],
    );
    check(
        "state fee: Decimal(4) = fold 0.0825\nreturn",
        &["Decimal(825, scale 4)"],
    );
}

#[test]
fn addition_and_comparison_cross_hint() {
    check("let a = count + 1\nreturn", &["Int(1)"]);
    check("let a = spend + 1\nreturn", &["Money(100, scale 2)"]);
    check(
        "let a = spend > 1000.00\nreturn",
        &["Money(100000, scale 2)"],
    );
    check(
        "let a = 1000.00 < spend\nreturn",
        &["Money(100000, scale 2)"],
    );
    check("let a = count >= 10\nreturn", &["Int(10)"]);
}

/// A binary result is the operator table's answer, not the left operand's type. The
/// left operand is what it used to be, and `Money(n) / Money(n)` is the row that made
/// it wrong: the value is a ratio, so the literal beside it is one too.
#[test]
fn an_amount_over_an_amount_hints_a_ratio() {
    check(
        "let a = spend / total + 1\nreturn",
        &["Decimal(1000000, scale 6)"],
    );
}

/// A method's result reaches the literal beside it, including for the two `Money`
/// methods the table used to be missing entirely.
#[test]
fn a_money_method_hints_the_literal_beside_it() {
    check(
        "let a = total.mul(rate, HalfUp) + 1\nreturn",
        &["Rounding(HalfUp)", "Money(100, scale 2)"],
    );
    check(
        "let a = total.div(count, Down) - 1\nreturn",
        &["Rounding(Down)", "Money(100, scale 2)"],
    );
    // The rate's own scale is still the author's: this row is `docs/money.md`'s, and
    // `mul` declares no type for it precisely so that it stays that way.
    check(
        "let a = total.mul(0.9, HalfUp)\nreturn",
        &["Decimal(9, scale 1)", "Rounding(HalfUp)"],
    );
}

/// A target that cannot hold a number is not a target, so the literal keeps its own
/// type and the real mistake is the one reported. Resolving against it instead said
/// "a number cannot be a String" about the `0` in `email > 0`, which is true and is not
/// what is wrong with that line.
#[test]
fn a_non_numeric_target_is_not_a_target() {
    // The literal keeps its own type, and the position it is in says what it wanted.
    check_error(
        "let a = total.mul(rate, 1)\nreturn",
        "expected Rounding, found Int",
    );
    // The case that prompted this: the target came from the other operand, and
    // "a number cannot be a String" is true of the `0` and is not what is wrong here.
    check_error(
        "let a = \"{count}\" > 0\nreturn",
        "cannot apply `>` to String and Int",
    );
}

/// A `Bool` target describes the comparison rather than its operands, so it must not
/// reach them. It used to, and the row above was unwritable inside an `if`: the literal
/// hit `Bool` before the other operand could type it, and reported "a number cannot be
/// a Bool" about a number that was never meant to be one.
#[test]
fn a_bool_target_does_not_reach_the_operands() {
    check("if 5 > count {\n  return\n}\nreturn", &["Int(5)"]);
    check(
        "if 1000.00 < spend {\n  return\n}\nreturn",
        &["Money(100000, scale 2)"],
    );
    check(
        "if count >= 10 && 0.5 < rate {\n  return\n}\nreturn",
        &["Int(10)", "Decimal(5000, scale 4)"],
    );
}

#[test]
fn defaulted_literals_settle_toward_more_places() {
    check(
        "let a = 1 + 0.5\nreturn",
        &["Decimal(10, scale 1)", "Decimal(5, scale 1)"],
    );
    check(
        "let a = 0.5 + 1\nreturn",
        &["Decimal(5, scale 1)", "Decimal(10, scale 1)"],
    );
}

#[test]
fn bare_literals_take_their_written_scale() {
    check("let a = 0.9\nreturn", &["Decimal(9, scale 1)"]);
    check("let a = 9\nreturn", &["Int(9)"]);
}

#[test]
fn multiplication_and_division_never_cross_hint() {
    check("let a = total * 0.9\nreturn", &["Decimal(9, scale 1)"]);
    check(
        "let a = total.mul(0.9, HalfUp)\nreturn",
        &["Decimal(9, scale 1)", "Rounding(HalfUp)"],
    );
    check("let a = total / 3\nreturn", &["Int(3)"]);
}

#[test]
fn over_precision_is_an_error_not_a_round() {
    check_error(
        "state fee: Decimal(2) = fold 0.0825\nreturn",
        "4 decimal places is too precise for Decimal(2)",
    );
    check_error(
        "state open: Int = fold 10.5\nreturn",
        "1 decimal place is too precise for Int",
    );
    check_error(
        "let a = count + 0.5\nreturn",
        "1 decimal place is too precise for Int",
    );
}

#[test]
fn money_literals_follow_the_declared_scale() {
    // Currency is not in the type, the value or the config, so a money literal
    // resolves against the field's own scale exactly as a decimal one does.
    let cases = [
        (2, "1000.00", "Money(100000, scale 2)"),
        (3, "1000.00", "Money(1000000, scale 3)"),
        (4, "1000", "Money(10000000, scale 4)"),
        (0, "1000", "Money(1000, scale 0)"),
    ];
    for (scale, literal, expected) in cases {
        let source = format!(
            "command Probe(spend: Money({scale})) {{\n  let a = spend > {literal}\n  return\n}}\n"
        );
        let program = parse(&source).unwrap_or_else(|err| panic!("Money({scale}): {err}"));
        let node = program.commands[0].exprs.get(ExprId(1));
        match node {
            Some(Expr::Lit(lit)) => assert_eq!(describe(lit), expected, "for Money({scale})"),
            other => panic!("Money({scale}): expected a literal, found {other:?}"),
        }
    }

    let source = "command Probe(spend: Money(0)) {\n  let a = spend > 1000.00\n  return\n}\n";
    let err = parse(source).expect_err("Money(0) holds no decimal places");
    assert_eq!(err.message, "2 decimal places is too precise for Money(0)");
}
