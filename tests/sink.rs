//! `docs/folds.md` section 11's per-record sink as executable rules.
//!
//! One claim carries the file: **applying a record through `Tables::apply` leaves every
//! column holding what refolding it would have returned.** A sink carries the
//! accumulator in the store rather than re-deriving it from the log, so that equality is
//! the whole of its correctness, and `agrees` asserts it for every column at every key
//! after every program here.

use heklang::{
    Cells, Event, EventPath, Interpreter, Key, Log, Program, Tables, Value, parse, value,
};

/// `@a.b { field: value }`, so a `given` in prose and a `given` in a test read alike.
fn given<'a>(
    interpreter: &mut Interpreter<'_>,
    path: &str,
    fields: impl IntoIterator<Item = (&'a str, Value)>,
) {
    interpreter.append(Event::new(
        EventPath::new(path.split('.')),
        fields
            .into_iter()
            .map(|(name, value)| (name.to_string(), value)),
    ));
}

fn program(source: &str) -> Program {
    match parse(source) {
        Ok(program) => program,
        Err(err) => panic!("{err}"),
    }
}

/// Every record through the sink, and then the claim.
///
/// The records arrive through `Tables::query` rather than straight off the harness, so a
/// subscription that selects nothing fails here instead of looking like a caught-up
/// store. That is the failure mode the first real store hit, and it is invisible to any
/// test that feeds the sink its records directly.
fn agrees(program: &Program, interpreter: &Interpreter<'_>) -> Cells {
    let tables = Tables::new(program);
    let mut cells = Cells::default();
    let query = tables.query();
    interpreter
        .host()
        .read(&query, &mut |record| tables.apply(record, &mut cells))
        .expect("the sink applies every record it is given");

    let head = interpreter.host().records().len() as u64;
    for table in &tables.layout().tables {
        let keys: Vec<Vec<Key>> = cells.rows(&table.name).map(<[Key]>::to_vec).collect();
        for key in &keys {
            for column in &table.columns {
                let args: Vec<(String, Value)> = column
                    .args
                    .iter()
                    .cloned()
                    .zip(key.iter().map(value::from_key))
                    .collect();
                let refolded = interpreter
                    .fold_stored(&column.fold, args.clone(), head)
                    .expect("the column refolds");
                match cells.get(&table.name, key, &column.name) {
                    Some(held) => assert!(
                        held.same(&refolded),
                        "{}[{key:?}].{} held {held} and refolds to {refolded}",
                        table.name,
                        column.name
                    ),
                    // A column another column's key minted and this one never wrote is at
                    // its seed, which is what a fold over no matching events folds to. A
                    // head of zero is that seed, so the two answers are compared rather
                    // than one of them being assumed.
                    None => {
                        let seed = interpreter
                            .fold_stored(&column.fold, args, 0)
                            .expect("the seed folds");
                        assert!(
                            seed.same(&refolded),
                            "{}[{key:?}].{} is unwritten, so it reads as the seed {seed}, but refolds to {refolded}",
                            table.name,
                            column.name
                        );
                    }
                }
            }
        }
    }
    cells
}

// ---------------------------------------------------------------------------

/// Four key signatures, and between them every arm shape `layout` can derive: an
/// accumulating arm and a replacing one on the same column, a range arm that leaves a key
/// part open, a rule 13 conjunct that writes a key part the event does not carry, and an
/// envelope binding that makes a column query-only.
const SHOP: &str = "subject Shop(Int)

event @order.placed { shop_id: Shop, order_id: Int, total: Money(2) }
event @order.cancelled { shop_id: Shop, order_id: Int, refund: Money(2) }
event @shop.uninstalled { shop_id: Shop }
event @hold.placed { shop_id: Shop, order_id: Int, kind: String }

fold OrderTotal(shop_id: Shop, order_id: Int) -> sum: Money(2) = 0 {
  on @order.placed(shop_id, order_id) { total } => sum + total
  on @order.cancelled(shop_id, order_id) { refund } => sum - refund
}

fold OrderClosed(shop_id: Shop, order_id: Int) -> Bool = false {
  on @order.cancelled(shop_id, order_id) => true
  on @shop.uninstalled(shop_id) => true
}

fold OrderSeenAt(shop_id: Shop, order_id: Int) -> Timestamp? = none {
  on @order.placed as e (shop_id, order_id) => e.at
}

fold HoldsOfKind(shop_id: Shop, order_id: Int, kind: String) -> n: Int = 0 {
  on @hold.placed(shop_id, order_id, kind, kind == \"fixed\") => n + 1
}

fold ShopOrders(shop_id: Shop) -> n: Int = 0 {
  on @order.placed(shop_id) => n + 1
}
";

fn shop_log(interpreter: &mut Interpreter<'_>) {
    given(
        interpreter,
        "order.placed",
        [
            ("shop_id", Value::Int(1)),
            ("order_id", Value::Int(7)),
            ("total", Value::money(40_000, 2)),
        ],
    );
    given(
        interpreter,
        "order.placed",
        [
            ("shop_id", Value::Int(1)),
            ("order_id", Value::Int(9)),
            ("total", Value::money(15_000, 2)),
        ],
    );
    given(
        interpreter,
        "order.cancelled",
        [
            ("shop_id", Value::Int(1)),
            ("order_id", Value::Int(7)),
            ("refund", Value::money(10_000, 2)),
        ],
    );
    given(
        interpreter,
        "hold.placed",
        [
            ("shop_id", Value::Int(1)),
            ("order_id", Value::Int(7)),
            ("kind", Value::str("fixed")),
        ],
    );
    given(
        interpreter,
        "hold.placed",
        [
            ("shop_id", Value::Int(1)),
            ("order_id", Value::Int(7)),
            ("kind", Value::str("manual")),
        ],
    );
    given(
        interpreter,
        "order.placed",
        [
            ("shop_id", Value::Int(2)),
            ("order_id", Value::Int(7)),
            ("total", Value::money(500, 2)),
        ],
    );
}

#[test]
fn a_record_applied_leaves_every_column_where_refolding_would() {
    let program = program(SHOP);
    let mut interpreter = Interpreter::new(&program);
    shop_log(&mut interpreter);
    let cells = agrees(&program, &interpreter);

    // Spot the values as well as their agreement, so a sink and a refold that are wrong
    // together still fail.
    let order = |shop, id| vec![Key::Int(shop), Key::Int(id)];
    assert_eq!(
        cells.get("shop__order", &order(1, 7), "order_total"),
        Some(&Value::money(30_000, 2)),
        "400.00 placed and 100.00 refunded"
    );
    assert_eq!(
        cells.get("shop__order", &order(1, 7), "order_closed"),
        Some(&Value::Bool(true))
    );
    assert_eq!(
        cells.get("shop__order", &order(1, 9), "order_total"),
        Some(&Value::money(15_000, 2))
    );
    assert_eq!(
        cells.get("shop", &[Key::Int(1)], "shop_orders"),
        Some(&Value::Int(2)),
        "two of the shop's three placed orders are its own"
    );
    // Rule 13: the conjunct writes the key part and the bare `kind` beside it narrows the
    // event, so the `manual` hold is in no row at all rather than in a row keyed `manual`.
    assert_eq!(
        cells.get(
            "shop__kind__order",
            &[Key::Int(1), Key::Str("fixed".into()), Key::Int(7)],
            "holds_of_kind"
        ),
        Some(&Value::Int(1))
    );
    assert_eq!(cells.len("shop__kind__order"), 1);
}

#[test]
fn a_key_conjunct_alone_writes_the_key_and_narrows_nothing() {
    // The pair in `SHOP` is `(kind, kind == "fixed")` and both halves do work: the
    // conjunct says which key part the arm writes, and the bare `kind` beside it is an
    // ordinary field filter. Written without that second half the arm still writes at
    // `"fixed"` and now counts *every* hold into it, which is `docs/folds.md` rule 13 as
    // specified and is easy to mistake for a bug in the sink. It is asserted here so the
    // fixture above reads as a choice.
    let source = "event @hold.placed { order_id: Int, kind: String }

fold HoldsOfKind(order_id: Int, kind: String) -> n: Int = 0 {
  on @hold.placed(order_id, kind == \"fixed\") => n + 1
}
";
    let program = program(source);
    let mut interpreter = Interpreter::new(&program);
    for kind in ["fixed", "manual"] {
        given(
            &mut interpreter,
            "hold.placed",
            [("order_id", Value::Int(7)), ("kind", Value::str(kind))],
        );
    }
    let cells = agrees(&program, &interpreter);
    assert_eq!(
        cells.get(
            "kind__order",
            &[Key::Str("fixed".into()), Key::Int(7)],
            "holds_of_kind"
        ),
        Some(&Value::Int(2)),
        "nothing narrowed by the event's own `kind`, so both holds counted"
    );
}

#[test]
fn a_range_arm_covers_a_key_this_same_record_minted() {
    // **The ordering claim, and it needs this exact shape to be a claim at all.** One
    // record has to both mint a key through a fully pinned arm and range over it through
    // another, so `@order.placed` writes `order_total` at a new key while
    // `orders_touched`'s second arm ranges over every order of the shop. Applying the
    // range first leaves the new key out of it, and nothing later corrects a column that
    // was already written.
    let source = "subject Shop(Int)

event @order.placed { shop_id: Shop, order_id: Int, total: Money(2) }
event @order.cancelled { shop_id: Shop, order_id: Int, refund: Money(2) }

fold OrderTotal(shop_id: Shop, order_id: Int) -> sum: Money(2) = 0 {
  on @order.placed(shop_id, order_id) { total } => sum + total
}

fold OrdersTouched(shop_id: Shop, order_id: Int) -> n: Int = 0 {
  on @order.cancelled(shop_id, order_id) => n + 1
  on @order.placed(shop_id) => n + 1
}
";
    let program = program(source);
    let mut interpreter = Interpreter::new(&program);
    // Mints (1, 7) before the record under test, so the range has an older key too and a
    // sink that covered only the new one would fail just as loudly.
    given(
        &mut interpreter,
        "order.cancelled",
        [
            ("shop_id", Value::Int(1)),
            ("order_id", Value::Int(7)),
            ("refund", Value::money(100, 2)),
        ],
    );
    given(
        &mut interpreter,
        "order.placed",
        [
            ("shop_id", Value::Int(1)),
            ("order_id", Value::Int(9)),
            ("total", Value::money(2_500, 2)),
        ],
    );
    let cells = agrees(&program, &interpreter);

    assert_eq!(
        cells.get("shop__order", &[Key::Int(1), Key::Int(9)], "orders_touched"),
        Some(&Value::Int(1)),
        "the key this record minted is in the range it ranged over"
    );
    assert_eq!(
        cells.get("shop__order", &[Key::Int(1), Key::Int(7)], "orders_touched"),
        Some(&Value::Int(2)),
        "one cancellation of its own, and one placement over the shop"
    );
}

#[test]
fn a_sealed_column_is_handed_through_rather_than_shredded() {
    // The fill path, so it behaves as `fold_stored` does: the store holds the ciphertext
    // and shreds on its own read, which is what makes an erasure after the fill
    // observable without refolding the column.
    let source = "subject Customer(Int)

event @order.placed { customer_id: Customer, email: String @subject(customer_id) }

fold LatestEmail(customer_id: Customer) -> Sealed(String, Customer)? = none {
  on @order.placed(customer_id) { email } => email
}
";
    let program = program(source);
    let mut interpreter = Interpreter::new(&program);
    given(
        &mut interpreter,
        "order.placed",
        [
            ("customer_id", Value::Int(7)),
            ("email", Value::str("buyer@example.com")),
        ],
    );
    let cells = agrees(&program, &interpreter);

    let held = cells
        .get("customer", &[Key::Int(7)], "latest_email")
        .expect("the column is filled");
    assert!(
        matches!(
            held,
            Value::Opt { value: Some(inner), .. } if matches!(**inner, Value::Sealed { .. })
        ),
        "got {held:?}"
    );
}

#[test]
fn a_record_no_arm_matches_writes_nothing() {
    // The half a host cannot do for itself. `ColumnArm` carries the key parts an arm pins
    // and not the field filters it also wrote, so a host choosing columns to refold must
    // over-approximate; the sink has the filters and checks them, and `agrees` would not
    // catch this on its own because refolding an untouched column returns what it held.
    let source = "event @a.happened { id: Int, kind: String }

fold Interesting(id: Int) -> n: Int = 0 {
  on @a.happened(id, kind: \"yes\") => n + 1
}
";
    let program = program(source);
    let mut interpreter = Interpreter::new(&program);
    given(
        &mut interpreter,
        "a.happened",
        [("id", Value::Int(1)), ("kind", Value::str("no"))],
    );
    let cells = agrees(&program, &interpreter);
    assert!(
        cells.is_empty("id"),
        "the filter failed, so there is no row at all"
    );

    given(
        &mut interpreter,
        "a.happened",
        [("id", Value::Int(1)), ("kind", Value::str("yes"))],
    );
    let cells = agrees(&program, &interpreter);
    assert_eq!(
        cells.get("id", &[Key::Int(1)], "interesting"),
        Some(&Value::Int(1))
    );
}

#[test]
fn the_subscription_is_every_event_any_column_folds_and_each_once() {
    let program = program(SHOP);
    let tables = Tables::new(&program);
    let query = tables.query();
    let mut paths: Vec<String> = query
        .slices
        .iter()
        .map(|slice| slice.event.to_string())
        .collect();
    let seen = paths.len();
    paths.sort();
    paths.dedup();
    assert_eq!(paths.len(), seen, "each event once, not once per column");
    assert_eq!(
        paths,
        vec![
            "@hold.placed",
            "@order.cancelled",
            "@order.placed",
            "@shop.uninstalled"
        ]
    );
    // No filters, because two columns on one event filter it differently and a
    // subscription is their union. `HoldsOfKind` narrows `@hold.placed` to one `kind` and
    // the subscription must not.
    assert!(
        query.slices.iter().all(|slice| slice.filters.is_empty()),
        "a subscription narrows by nothing"
    );
    assert_eq!((query.from, query.upto), (0, None));
}

#[test]
fn a_global_table_takes_one_row_under_an_empty_key() {
    let source = "event @a.happened { id: Int }

fold Happenings() -> n: Int = 0 {
  on @a.happened() => n + 1
}
";
    let program = program(source);
    let mut interpreter = Interpreter::new(&program);
    given(&mut interpreter, "a.happened", [("id", Value::Int(1))]);
    given(&mut interpreter, "a.happened", [("id", Value::Int(2))]);
    let cells = agrees(&program, &interpreter);
    assert_eq!(cells.get("global", &[], "happenings"), Some(&Value::Int(2)));
    assert_eq!(cells.len("global"), 1, "a keyless fold has one value");
}
