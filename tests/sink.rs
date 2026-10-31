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

// ---------------------------------------------------------------------------

/// A key part computed from the record, which is the one shape where the sink and a
/// by-key read run the *same expression in opposite directions*: the sink evaluates it
/// and writes the row at the answer, while `fold_stored` has the asked key in the slot
/// already and keeps only the records whose answer equals it. `agrees` is what proves
/// the two meet, and a comparison written the wrong way round fails here and nowhere
/// else.
const BUCKETED: &str = "subject Shop(Int)

event @usage.recorded { shop_id: Shop, at: Timestamp, units: Int }

fn month_of(at: Timestamp) -> String {
  return \"{at.year()}-{at.month().pad(2)}\"
}

fold UsageCounted(shop_id: Shop, month: String) -> n: Int = 0 {
  on @usage.recorded(shop_id) { at, units } key month = month_of(at) => n + units
}
";

#[test]
fn a_computed_key_part_writes_the_row_the_record_belongs_to() {
    let program = program(BUCKETED);
    let mut interpreter = Interpreter::new(&program);
    for (at, units) in [
        ("2026-03-04T00:00:00Z", 3),
        ("2026-03-20T00:00:00Z", 4),
        ("2026-04-01T00:00:00Z", 9),
    ] {
        given(
            &mut interpreter,
            "usage.recorded",
            [
                ("shop_id", Value::Int(1)),
                (
                    "at",
                    Value::Timestamp(value::timestamp(at).expect("rfc 3339")),
                ),
                ("units", Value::Int(units)),
            ],
        );
    }
    // A second shop, so a wrong key would show up as a merged row rather than only as a
    // wrong total.
    given(
        &mut interpreter,
        "usage.recorded",
        [
            ("shop_id", Value::Int(2)),
            (
                "at",
                Value::Timestamp(value::timestamp("2026-03-09T00:00:00Z").expect("rfc 3339")),
            ),
            ("units", Value::Int(100)),
        ],
    );
    let cells = agrees(&program, &interpreter);

    let row = |shop, month: &str| vec![Key::Int(shop), Key::Str(month.into())];
    assert_eq!(
        cells.get("shop__month", &row(1, "2026-03"), "usage_counted"),
        Some(&Value::Int(7)),
        "both of March's records, and neither of April's or the other shop's"
    );
    assert_eq!(
        cells.get("shop__month", &row(1, "2026-04"), "usage_counted"),
        Some(&Value::Int(9))
    );
    assert_eq!(
        cells.get("shop__month", &row(2, "2026-03"), "usage_counted"),
        Some(&Value::Int(100))
    );
    assert_eq!(cells.len("shop__month"), 3, "three buckets, not one");
}

#[test]
fn a_computed_part_is_pinned_so_the_table_is_keyed_by_it() {
    let program = program(BUCKETED);
    let tables = Tables::new(&program);
    let table = tables
        .layout()
        .tables
        .iter()
        .find(|table| table.name == "shop__month")
        .expect("the fold is storable");
    assert_eq!(
        table
            .key
            .iter()
            .map(|part| part.name.as_str())
            .collect::<Vec<_>>(),
        vec!["shop", "month"],
        "the subject part leads and the computed one follows"
    );
    assert!(
        tables.layout().unstored.is_empty(),
        "a computed part supplies its key, so nothing is unstored"
    );
    // One record writes one key, so the arm is fully pinned and a host derives no range
    // index for it.
    let column = &table.columns[0];
    assert!(
        column
            .arms
            .iter()
            .all(|arm| arm.fields.iter().all(heklang::ArmMatch::pinned))
    );
}

// ---------------------------------------------------------------------------

/// `Tables::only`: the restricted sink a rebuild replays through.
///
/// A column is rebuilt by replaying the log into it, and `apply` runs every arm of every
/// column over that replay. The columns that are already correct pay for it twice: the
/// work, and the chance that a body which can `fail` from any value fails the rebuild of
/// a column it has nothing to do with. `FRAGILE` below is that second half made concrete.
const FRAGILE: &str = "event @tick.counted { box_id: Int, n: Int }

fold TickCount(box_id: Int) -> n: Int = 0 {
  on @tick.counted(box_id) => n + 1
}

fold TickDoubled(box_id: Int) -> n: Int = 1 {
  on @tick.counted(box_id) => n * 2
}
";

#[test]
fn a_restricted_apply_leaves_a_column_outside_its_closure_untouched() {
    let program = program(FRAGILE);
    let mut interpreter = Interpreter::new(&program);
    for _ in 0..3 {
        given(
            &mut interpreter,
            "tick.counted",
            [("box_id", Value::Int(1)), ("n", Value::Int(0))],
        );
    }

    let tables = Tables::new(&program);
    let mut cells = Cells::default();
    // Built once rather than per record, which is the whole reason it is a handle.
    let rebuild = tables.only(&["TickCount".to_string()]);
    assert_eq!(
        rebuild.folds(),
        ["TickCount".to_string()],
        "every arm pins the whole key, so the closure is what was asked for"
    );
    interpreter
        .host()
        .read(&tables.query(), &mut |record| {
            rebuild.apply(record, &mut cells)
        })
        .expect("the sink applies every record it is given");

    assert_eq!(
        cells.get("box", &[Key::Int(1)], "tick_count"),
        Some(&Value::Int(3))
    );
    // Not its seed and not its value: the arm never ran, so the cell was never written.
    // That is the whole of what a rebuild of one column asks for.
    assert_eq!(cells.get("box", &[Key::Int(1)], "tick_doubled"), None);
}

#[test]
fn a_restricted_apply_over_every_fold_is_the_unrestricted_one() {
    let program = program(SHOP);
    let mut interpreter = Interpreter::new(&program);
    shop_log(&mut interpreter);

    let tables = Tables::new(&program);
    let every: Vec<String> = tables
        .layout()
        .tables
        .iter()
        .flat_map(|table| &table.columns)
        .map(|column| column.fold.clone())
        .collect();
    let mut restricted = Cells::default();
    let rebuild = tables.only(&every);
    interpreter
        .host()
        .read(&tables.query(), &mut |record| {
            rebuild.apply(record, &mut restricted)
        })
        .expect("the sink applies every record it is given");

    // `agrees` has already checked the unrestricted run against a refold, so equality
    // with it carries that over rather than asserting the values again. Every arm shape
    // `layout` derives is in `SHOP`, the range arm included, which is the one that reads
    // the store back and so the one a skipped column could corrupt.
    let whole = agrees(&program, &interpreter);
    for table in &tables.layout().tables {
        let keys: Vec<Vec<Key>> = whole.rows(&table.name).map(<[Key]>::to_vec).collect();
        assert_eq!(
            restricted.len(&table.name),
            keys.len(),
            "{} holds a different number of rows",
            table.name
        );
        for key in &keys {
            for column in &table.columns {
                assert_eq!(
                    restricted.get(&table.name, key, &column.name),
                    whole.get(&table.name, key, &column.name),
                    "{}[{key:?}].{}",
                    table.name,
                    column.name
                );
            }
        }
    }
}

#[test]
fn a_restricted_apply_ignores_a_fold_no_column_is_kept_for() {
    // An unstored fold and a misspelling are one case: there is no column, so there is
    // nothing to apply. Raising instead would make a caller build the listing of what is
    // storable before it could ask, which is the derivation this module owns.
    let source = "event @tick.counted { box_id: Int, n: Int }

fold TickCount(box_id: Int) -> n: Int = 0 {
  on @tick.counted(box_id) => n + 1
}

fold TicksSince(at: Timestamp) -> n: Int = 0 {
  on @tick.counted() => n + 1
}
";
    let program = program(source);
    let mut interpreter = Interpreter::new(&program);
    given(
        &mut interpreter,
        "tick.counted",
        [("box_id", Value::Int(1)), ("n", Value::Int(0))],
    );

    let tables = Tables::new(&program);
    assert_eq!(
        tables.layout().unstored.len(),
        1,
        "`TicksSince` is not kept"
    );
    let mut cells = Cells::default();
    let rebuild = tables.only(&["TicksSince".to_string(), "TickCounted".to_string()]);
    assert!(rebuild.folds().is_empty(), "neither names a column");
    interpreter
        .host()
        .read(&tables.query(), &mut |record| {
            rebuild.apply(record, &mut cells)
        })
        .expect("a fold with no column is nothing to apply");
    assert!(cells.is_empty("box"), "`TickCount` was not among them");
}

#[test]
fn the_layout_names_the_table_and_column_one_fold_is_kept_in() {
    let program = program(SHOP);
    let layout = heklang::layout(&program);
    let (table, column) = layout.column("HoldsOfKind").expect("it is kept");
    assert_eq!(table.name, "shop__kind__order");
    assert_eq!(column.name, "holds_of_kind");
    assert!(
        layout.column("holds_of_kind").is_none(),
        "named by the fold, not by the column it derives"
    );
    assert!(layout.column("Nothing").is_none());
}

/// A range arm reads **the keys the table holds**, and other columns are what mint them,
/// so a column set is not the whole of what a restricted apply has to run.
///
/// `OrderClosed`'s arm on `@shop.uninstalled(shop_id)` leaves `order` open, which is a
/// range over every order of that shop, and the orders exist because `OrderTotal`'s arm
/// wrote a row at each. Restrict the apply to `OrderClosed` alone and the keys are never
/// minted, so the range covers nothing and the column stays at its seed: a rebuild that
/// quietly answers `false` for every order of an uninstalled shop.
///
/// `Layout::closure` is what keeps that from being the caller's problem to know about.
const RANGED: &str = "event @order.placed { shop_id: Int, order_id: Int }
event @order.cancelled { shop_id: Int, order_id: Int }
event @shop.uninstalled { shop_id: Int }

fold OrderTotal(shop_id: Int, order_id: Int) -> n: Int = 0 {
  on @order.placed(shop_id, order_id) => n + 1
}

fold OrderClosed(shop_id: Int, order_id: Int) -> Bool = false {
  on @order.cancelled(shop_id, order_id) => true
  on @shop.uninstalled(shop_id) => true
}
";

#[test]
fn a_restricted_apply_still_mints_the_keys_a_range_arm_covers() {
    let program = program(RANGED);
    let mut interpreter = Interpreter::new(&program);
    given(
        &mut interpreter,
        "order.placed",
        [("shop_id", Value::Int(1)), ("order_id", Value::Int(7))],
    );
    given(
        &mut interpreter,
        "shop.uninstalled",
        [("shop_id", Value::Int(1))],
    );

    let tables = Tables::new(&program);
    let whole = agrees(&program, &interpreter);
    let key = [Key::Int(7), Key::Int(1)];
    assert_eq!(
        whole.get("order__shop", &key, "order_closed"),
        Some(&Value::Bool(true)),
        "the order was minted before the shop was uninstalled, so the range covers it"
    );

    let mut part = Cells::default();
    let rebuild = tables.only(&["OrderClosed".to_string()]);
    interpreter
        .host()
        .read(&tables.query(), &mut |record| {
            rebuild.apply(record, &mut part)
        })
        .expect("the sink applies every record it is given");
    assert_eq!(
        part.get("order__shop", &key, "order_closed"),
        whole.get("order__shop", &key, "order_closed"),
        "a restricted rebuild of a ranged column answers what the whole sink does"
    );
}

/// What a rebuild has to be told, and the reason the closure is reachable rather than
/// private: the handle applies **more columns than were asked for**, and the sink carries
/// the accumulator in the store, so replaying onto a column that already holds its value
/// folds every record into it twice.
///
/// A store rebuilding `order_closed` alone against its live tables gets `order_total`
/// applied beside it and silently doubled. `Rebuild::folds` is the set it has to reset or
/// shadow first, and it is not the one it passed in.
#[test]
fn the_closure_names_the_columns_a_rebuild_has_to_prepare() {
    let program = program(RANGED);
    let mut interpreter = Interpreter::new(&program);
    given(
        &mut interpreter,
        "order.placed",
        [("shop_id", Value::Int(1)), ("order_id", Value::Int(7))],
    );
    given(
        &mut interpreter,
        "shop.uninstalled",
        [("shop_id", Value::Int(1))],
    );

    let tables = Tables::new(&program);
    let rebuild = tables.only(&["OrderClosed".to_string()]);
    assert!(
        rebuild.folds().contains(&"OrderTotal".to_string()),
        "the range needs the keys `OrderTotal` mints, so it is applied too"
    );

    // Already correct, which is the state a rebuild of another column starts from.
    let mut live = agrees(&program, &interpreter);
    let key = [Key::Int(7), Key::Int(1)];
    assert_eq!(
        live.get("order__shop", &key, "order_total"),
        Some(&Value::Int(1))
    );
    interpreter
        .host()
        .read(&tables.query(), &mut |record| {
            rebuild.apply(record, &mut live)
        })
        .expect("the sink applies every record it is given");
    assert_eq!(
        live.get("order__shop", &key, "order_total"),
        Some(&Value::Int(2)),
        "folded a second time onto what it already held, which is what `folds` warns about"
    );
}

#[test]
fn a_ranged_column_closes_over_the_columns_that_mint_its_keys() {
    let layout = heklang::layout(&program(RANGED));
    assert_eq!(
        layout.closure(&["OrderClosed".to_string()]),
        vec!["OrderClosed".to_string(), "OrderTotal".to_string()],
        "a range needs the keys its table holds, and the other columns mint them"
    );
    // The common case restricts to itself: every arm of `OrderTotal` pins the whole key,
    // so it reads no key set and nothing else has to run beside it.
    assert_eq!(
        layout.closure(&["OrderTotal".to_string()]),
        vec!["OrderTotal".to_string()]
    );
    assert!(layout.closure(&["Nothing".to_string()]).is_empty());
}

#[test]
fn the_closure_adds_the_columns_that_mint_and_not_the_table() {
    // What a range needs beside it is the **minting**, so a column no arm of which pins
    // the whole key is not added: it asks `Columns::keys` and writes at what comes back,
    // so it mints nothing and is no use to anyone else's range. `ShopOrders` is in
    // another table and is out either way; `OrderSeenAt` and `HoldsOfKind` are too.
    //
    // One pinned arm is enough to be added, which is why that is not the negation of
    // having a ranged one: `OrderClosed` has both.
    // Storable and yet minting nothing, which takes two arms: every part is pinned by
    // *some* arm, which is what `layout` asks, and no arm pins the whole key.
    let source = format!(
        "{SHOP}
fold OrderAnyHold(shop_id: Shop, order_id: Int) -> Bool = false {{
  on @hold.placed(shop_id) => true
  on @order.cancelled(order_id) => true
}}
"
    );
    let layout = heklang::layout(&program(&source));
    assert_eq!(
        layout.closure(&["OrderClosed".to_string()]),
        vec![
            "OrderClosed".to_string(),
            "OrderSeenAt".to_string(),
            "OrderTotal".to_string(),
        ],
        "`OrderAnyHold` ranges from every arm, so it mints nothing and is left out"
    );
    // Asked for directly it still runs, mints or not: it is what the caller is rebuilding.
    assert_eq!(
        layout.closure(&["OrderAnyHold".to_string()]),
        vec![
            "OrderAnyHold".to_string(),
            "OrderClosed".to_string(),
            "OrderSeenAt".to_string(),
            "OrderTotal".to_string(),
        ]
    );
}
