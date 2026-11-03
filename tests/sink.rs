//! `docs/folds.md` section 11's per-record sink as executable rules.
//!
//! One claim carries the file: **applying a record through `Tables::apply` leaves every
//! column holding what refolding it would have returned.** A sink carries the
//! accumulator in the store rather than re-deriving it from the log, so that equality is
//! the whole of its correctness, and `agrees` asserts it for every column at every key
//! after every program here.
//!
//! **One exception, and it is a boundary rather than a bug**: a range arm covers the keys
//! the table holds when the record arrives, so a key minted later never sees it, while a
//! refold applies every ranged record to every key. The sink's answer is the definition,
//! because it is the implementable one;
//! `a_key_minted_after_a_ranged_record_does_not_see_it` states the difference and
//! `docs/folds.md` section 11 has the argument. Every fixture here keeps its ranged
//! records after the keys they cover, which is what lets `agrees` hold over all of them.

use heklang::{
    Cells, Event, EventPath, Interpreter, Key, Log, Program, Tables, Type, Value, parse, value,
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
fn a_bare_key_part_over_an_optional_field_skips_an_absent_one() {
    // The sink's half of a slice filter fitted to an optional field: a by-key read at
    // `fo: 3` folds `some(3)` and never `none`, so the sink unwraps one and writes nothing
    // for the other. It raised on both, and a sink error stops every table in a host.
    // `ByKind` is the arm `narrowed` checks first, which bound the field the same way.
    let source = "subject Shop(Int)
event @x.failed { shop_id: Shop, fo: Int?, kind: String }

fold FoFailures(shop_id: Shop, fo: Int) -> n: Int = 0 {
  on @x.failed(shop_id, fo) => n + 1
}

fold ByKind(shop_id: Shop, fo: Int, k: String) -> n: Int = 0 {
  on @x.failed(shop_id, fo) { kind } key k = kind => n + 1
}
";
    let program = program(source);
    let mut interpreter = Interpreter::new(&program);
    for fo in [Value::some(Value::Int(3)), Value::none(Type::Int)] {
        given(
            &mut interpreter,
            "x.failed",
            [
                ("shop_id", Value::Int(1)),
                ("fo", fo),
                ("kind", Value::str("a")),
            ],
        );
    }
    let cells = agrees(&program, &interpreter);
    assert_eq!(
        cells.get("shop__fo", &[Key::Int(1), Key::Int(3)], "fo_failures"),
        Some(&Value::Int(1))
    );
    assert_eq!(
        cells.get(
            "shop__fo__k",
            &[Key::Int(1), Key::Int(3), Key::Str("a".into())],
            "by_kind"
        ),
        Some(&Value::Int(1))
    );
    assert_eq!(cells.rows("shop__fo").count(), 1, "none mints no key");
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

// ---------------------------------------------------------------------------

/// Rule 14 through both drivers, which is where a computed key part's correctness lives:
/// the sink writes the row at what the expression evaluates to, and a by-key read
/// compares, so `agrees` is the only place the two can be caught disagreeing.
///
/// Everything a computed part may read is here, and the comprehension is why the rule is
/// written as a deny-list of the key rather than an allow-list of the record: the binding
/// it allocates is a slot of the fold's frame like any other.
const COMPUTED: &str = "const OFFSET: Int = 100

event @usage.recorded { shop_id: Int, kind: String, units: Int, tags: List(String) }

fn month_of(at: Timestamp) -> String {
  return \"{at.year()}-{at.month().pad(2)}\"
}

fold UsageCounted(shop_id: Int, month: String) -> n: Int = 0 {
  on @usage.recorded as e (shop_id) { units } key month = month_of(e.at) => n + units
}

fold TaggedUnits(shop_id: Int, bucket: Int) -> n: Int = 0 {
  on @usage.recorded(shop_id) { units, tags }
    key bucket = units + OFFSET + [t.len() for t in tags].len() => n + units
}

fold Rated(shop_id: Int, rate: Int) -> n: Int = 0 {
  on @usage.recorded(shop_id, kind: \"paid\") { units } key rate = 100 / units => n + units
}

fold Matching(shop_id: Int, kind: String, rate: Int) -> n: Int = 0 {
  on @usage.recorded(shop_id, kind, tags: [kind]) { units } key rate = 100 / units => n + 1
}
";

fn usage(interpreter: &mut Interpreter<'_>, kind: &str, units: i64) {
    given(
        interpreter,
        "usage.recorded",
        [
            ("shop_id", Value::Int(1)),
            ("kind", Value::str(kind)),
            ("units", Value::Int(units)),
            (
                "tags",
                Value::list(heklang::Type::String, [Value::str("ab"), Value::str("c")]),
            ),
        ],
    );
}

#[test]
fn a_computed_key_part_reaches_the_same_row_through_both_drivers() {
    let program = program(COMPUTED);
    let mut interpreter = Interpreter::new(&program);
    usage(&mut interpreter, "paid", 5);
    let cells = agrees(&program, &interpreter);

    // The envelope binding, through a pure `fn`: the harness stamps 2020-01-01.
    assert_eq!(
        cells.get(
            "month__shop",
            &[Key::Str("2020-01".into()), Key::Int(1)],
            "usage_counted"
        ),
        Some(&Value::Int(5))
    );
    // A `const`, a bind and a comprehension over a bound list: 5 + 100 + 2.
    assert_eq!(
        cells.get(
            "bucket__shop",
            &[Key::Int(107), Key::Int(1)],
            "tagged_units"
        ),
        Some(&Value::Int(5))
    );
}

/// **The arm's narrowing is checked before the key is evaluated**, because a by-key read
/// never runs the expression for a record its predicate rejects.
///
/// `Rated` divides by `units` and only counts paid usage, so a free record with no units
/// is one the arm does not match. The sink used to compute the key first and fail the
/// whole replay with `division by zero`, where the same fold read by key answered: a
/// program that checks clean and that one driver cannot run at all.
#[test]
fn the_sink_does_not_evaluate_a_key_for_a_record_the_arm_rejects() {
    let program = program(COMPUTED);
    let mut interpreter = Interpreter::new(&program);
    usage(&mut interpreter, "paid", 5);
    usage(&mut interpreter, "free", 0);
    let cells = agrees(&program, &interpreter);

    assert_eq!(
        cells.get("rate__shop", &[Key::Int(20), Key::Int(1)], "rated"),
        Some(&Value::Int(5)),
        "the paid record alone, at 100 / 5"
    );
    assert_eq!(cells.len("rate__shop"), 1, "the free record is in no row");

    // **The whole predicate, not the conjuncts that read nothing.** `Matching` pins
    // `kind` from the field of that name and then narrows again on `tags: [kind]`, which
    // reads the parameter the first conjunct pinned. Skipping it as unresolvable left the
    // key evaluated for every record of the event, which is this program's whole bug: the
    // free record's tags are not `["free"]`, so the arm does not match it, and the sink
    // divided by its zero units anyway.
    assert!(
        cells.is_empty("kind__rate__shop"),
        "no record has tags equal to its own kind"
    );
}

// ---------------------------------------------------------------------------

/// **One column's arms apply in the order they were written**, which the two-pass sink did
/// not do and which is the file's headline claim.
///
/// `applying` runs every fully pinned arm of every column before every ranged one, because
/// a range over a key this record mints has to see it. For a column with one of each on the
/// same event that put them in *pass* order: the pinned arm wrote `1` and the ranged arm
/// then overwrote it with `0`, where a refold applies the slices in source order and
/// answers `1`. `agrees` could not see it, because no other fixture has a column whose
/// pinned and ranged arms share an event path.
const ORDERED: &str = "event @x.happened { shop_id: Int, order_id: Int }

fold Seen(shop_id: Int, order_id: Int) -> n: Int = 0 {
  on @x.happened(shop_id) => 0
  on @x.happened(shop_id, order_id) => n + 1
}
";

#[test]
fn one_columns_arms_apply_in_written_order() {
    let program = program(ORDERED);
    let mut interpreter = Interpreter::new(&program);
    given(
        &mut interpreter,
        "x.happened",
        [("shop_id", Value::Int(1)), ("order_id", Value::Int(7))],
    );
    let cells = agrees(&program, &interpreter);
    assert_eq!(
        cells.get("order__shop", &[Key::Int(7), Key::Int(1)], "seen"),
        Some(&Value::Int(1)),
        "the ranged arm resets and the pinned arm counts, in that order"
    );
}

/// The same shape with the pinned arm written first, so the reset wins. Both orders, because
/// a fix that applied one column's arms in *reverse* would pass the test above alone.
#[test]
fn the_written_order_is_the_order_and_not_a_fixed_one() {
    let source = "event @x.happened { shop_id: Int, order_id: Int }

fold Seen(shop_id: Int, order_id: Int) -> n: Int = 0 {
  on @x.happened(shop_id, order_id) => n + 1
  on @x.happened(shop_id) => 0
}
";
    let program = program(source);
    let mut interpreter = Interpreter::new(&program);
    given(
        &mut interpreter,
        "x.happened",
        [("shop_id", Value::Int(1)), ("order_id", Value::Int(7))],
    );
    let cells = agrees(&program, &interpreter);
    assert_eq!(
        cells.get("order__shop", &[Key::Int(7), Key::Int(1)], "seen"),
        Some(&Value::Int(0)),
        "the pinned arm counts and the ranged arm resets, in that order"
    );
}

/// `Columns::keys` is held to `Log::read`'s cut: **over-delivering is a cost and
/// under-delivering is a bug**, so a host may answer with more keys than the arm writes
/// and the sink drops the rest.
///
/// A store with no index for a range answers with every row of the table, which is the
/// cheapest correct implementation and the one the contract exists to allow. It used to
/// abort the record: a part the arm pinned with a rule 13 conjunct is bound from whatever
/// key came back, so the conjunct's gate went dead, and a dead gate was read as a
/// disagreement between `layout` and the arm rather than as a key to skip.
#[derive(Default)]
struct Everything(Cells);

impl heklang::Columns for Everything {
    fn value(
        &self,
        table: &str,
        key: &[Key],
        column: &str,
    ) -> Result<Option<Value>, heklang::Error> {
        self.0.value(table, key, column)
    }

    fn set(
        &mut self,
        table: &str,
        key: Vec<Key>,
        column: &str,
        value: Value,
    ) -> Result<(), heklang::Error> {
        self.0.set(table, key, column, value)
    }

    fn keys(&self, table: &str, _pinned: &[(usize, Key)]) -> Result<Vec<Vec<Key>>, heklang::Error> {
        Ok(self.0.rows(table).map(<[Key]>::to_vec).collect())
    }
}

#[test]
fn a_host_may_over_deliver_the_keys_a_range_covers() {
    let source = "event @y.happened { shop_id: Int, order_id: Int, kind: String }

fold OfKind(shop_id: Int, order_id: Int, kind: String) -> n: Int = 0 {
  on @y.happened(shop_id, order_id, kind, kind == \"a\") => n + 1
  on @y.happened(shop_id, kind, kind == \"a\") => n + 100
}

fold OtherKind(shop_id: Int, order_id: Int, kind: String) -> n: Int = 0 {
  on @y.happened(shop_id, order_id, kind == \"b\") => n + 1
}
";
    let program = program(source);
    let mut interpreter = Interpreter::new(&program);
    // The `"b"` record is what puts a row in the table that `OfKind`'s range does not
    // write, and the third is what makes the range run while that row is there.
    for (order, kind) in [(7, "a"), (8, "b"), (9, "a")] {
        given(
            &mut interpreter,
            "y.happened",
            [
                ("shop_id", Value::Int(1)),
                ("order_id", Value::Int(order)),
                ("kind", Value::str(kind)),
            ],
        );
    }

    let tables = Tables::new(&program);
    let mut cells = Everything::default();
    interpreter
        .host()
        .read(&tables.query(), &mut |record| {
            tables.apply(record, &mut cells)
        })
        .expect("an over-delivered key is skipped rather than aborting the record");

    let key = |order, kind: &str| vec![Key::Str(kind.into()), Key::Int(order), Key::Int(1)];
    assert_eq!(
        cells.0.get("kind__order__shop", &key(7, "a"), "of_kind"),
        Some(&Value::Int(201)),
        "its own arm once, and the range of both `\"a\"` records"
    );
    assert_eq!(
        cells.0.get("kind__order__shop", &key(9, "a"), "of_kind"),
        Some(&Value::Int(101)),
        "minted by the third record, so only its own range reaches it"
    );
    // Offered to the range and not written by it: the key it carries is not the one the
    // arm pins.
    assert_eq!(
        cells.0.get("kind__order__shop", &key(8, "b"), "of_kind"),
        None
    );
    assert_eq!(
        cells.0.get("kind__order__shop", &key(8, "b"), "other_kind"),
        Some(&Value::Int(1))
    );
}

/// **A range arm covers the keys the table holds when the record arrives, and a key minted
/// later never sees it.** The one place the sink and a refold are *meant* to differ, pinned
/// here because it is easy to meet and nothing said so.
///
/// `docs/folds.md` section 11 had the right boundary and the wrong reason for its safety:
/// "the keys a range arm can need are exactly the keys the pinning arms mint" is about
/// *which* keys exist and not about *when*. A refold is a function of a key and the whole
/// log, so it applies every ranged record to every key; a sink walks forward and can only
/// apply one to the rows that are there.
///
/// Both answers are defensible and only one is implementable, so the sink's is the
/// definition and this is what it costs. An author who needs the other does not write a
/// range: an arm pinning the whole key is the same under both.
#[test]
fn a_key_minted_after_a_ranged_record_does_not_see_it() {
    let source = "event @x.happened { shop_id: Int, order_id: Int }

fold Seen(shop_id: Int, order_id: Int) -> n: Int = 0 {
  on @x.happened(shop_id) => n + 10
  on @x.happened(shop_id, order_id) => n + 1
}
";
    let program = program(source);
    let mut interpreter = Interpreter::new(&program);
    for order in [7, 8] {
        given(
            &mut interpreter,
            "x.happened",
            [("shop_id", Value::Int(1)), ("order_id", Value::Int(order))],
        );
    }

    let tables = Tables::new(&program);
    let mut cells = Cells::default();
    interpreter
        .host()
        .read(&tables.query(), &mut |record| {
            tables.apply(record, &mut cells)
        })
        .expect("the sink applies every record it is given");

    // Order 7 exists for both records, so it takes both ranges and its own count.
    assert_eq!(
        cells.get("order__shop", &[Key::Int(7), Key::Int(1)], "seen"),
        Some(&Value::Int(21))
    );
    // Order 8 is minted by the second record, so the first record's range passed it by.
    assert_eq!(
        cells.get("order__shop", &[Key::Int(8), Key::Int(1)], "seen"),
        Some(&Value::Int(11))
    );
    // And this is the difference, stated rather than discovered: a refold applies both.
    assert_eq!(
        interpreter
            .fold_stored(
                "Seen",
                [("shop_id", Value::Int(1)), ("order_id", Value::Int(8))],
                2
            )
            .expect("the column refolds"),
        Value::Int(21),
        "a refold is a function of the key and the whole log, so both ranges reach it"
    );
}

// ---------------------------------------------------------------------------

/// Rule 15 through both drivers: a key part read out of **another named fold**, as it
/// stood when the record being folded was appended.
///
/// `@game.refunded` carries neither the game nor the genre, which is the whole point of
/// the feature: without it the event would have to copy them, and `Refund` would fold
/// twelve folds to stamp values onto an event its decision never reads.
const KEYED: &str = "event @game.added { game_id: Int, genre: String }
event @game.purchased { receipt_id: Int, game_id: Int }
event @game.refunded { receipt_id: Int }

fold GameGenre(game_id: Int) -> String = \"unknown\" {
  on @game.added(game_id) { genre } => genre
}

fold ReceiptGame(receipt_id: Int) -> Int = 0 {
  on @game.purchased(receipt_id) { game_id } => game_id
}

fold GenreUnits(genre: String) -> n: Int = 0 {
  on @game.purchased() { game_id } key genre = GameGenre { game_id } => n + 1
  on @game.refunded() { receipt_id }
    key genre = GameGenre { game_id: ReceiptGame { receipt_id } } => n - 1
}
";

#[test]
fn a_key_read_through_another_fold_agrees_with_a_refold() {
    let program = program(KEYED);
    let mut interpreter = Interpreter::new(&program);
    given(
        &mut interpreter,
        "game.added",
        [("game_id", Value::Int(1)), ("genre", Value::str("rpg"))],
    );
    given(
        &mut interpreter,
        "game.purchased",
        [("receipt_id", Value::Int(50)), ("game_id", Value::Int(1))],
    );
    // **The record that both moves the looked-up fold and is counted through it**, which
    // is the case the handoff asked to be tested either way the question was answered. The
    // sale above is an rpg and the one below a shooter, because a lookup is as of the
    // record and strictly before it: a genre change between two sales splits them.
    given(
        &mut interpreter,
        "game.added",
        [("game_id", Value::Int(1)), ("genre", Value::str("shooter"))],
    );
    given(
        &mut interpreter,
        "game.purchased",
        [("receipt_id", Value::Int(51)), ("game_id", Value::Int(1))],
    );
    // A refund keyed through two folds, from an event carrying only the receipt.
    given(
        &mut interpreter,
        "game.refunded",
        [("receipt_id", Value::Int(50))],
    );

    let cells = agrees(&program, &interpreter);
    assert_eq!(
        cells.get("genre", &[Key::Str("rpg".into())], "genre_units"),
        Some(&Value::Int(1)),
        "the first sale, under the genre the game had when it sold"
    );
    // **And the refund lands here, not under `\"rpg\"`**, which is as-of doing exactly what
    // it says: the record being folded is the *refund*, so `GameGenre` is read as of the
    // refund and the game is a shooter by then. The sale it reverses was an rpg.
    //
    // That is a modelling trap and not a defect, and it is worth knowing before a port
    // leans on it: a refund keyed through a **mutable** fold decrements the bucket the
    // thing is in now rather than the one it was sold under. An author who needs the
    // other keys through a fold that captures the sale, so that what the lookup answers
    // cannot move afterwards.
    assert_eq!(
        cells.get("genre", &[Key::Str("shooter".into())], "genre_units"),
        Some(&Value::Int(0)),
        "the second sale, and the refund of the first"
    );
}

#[test]
fn a_looked_up_fold_is_a_column_and_a_dependency() {
    let layout = heklang::layout(&program(KEYED));
    let (_, column) = layout.column("GenreUnits").expect("it is kept");
    assert_eq!(
        column.reads,
        vec!["GameGenre".to_string(), "ReceiptGame".to_string()],
        "transitive and sorted: the chain's inner link counts too"
    );
    // Nothing keys through these, so they carry no dependency of their own.
    assert!(layout.column("GameGenre").expect("kept").1.reads.is_empty());

    // **A rebuild of the dependent has to replay the dependencies**, because a lookup
    // wants their value as of each past record and only maintaining them alongside gives
    // that. `Tables::only` closes over them for the same reason it closes a range over its
    // table's minters.
    assert_eq!(
        layout.closure(&["GenreUnits".to_string()]),
        vec![
            "GameGenre".to_string(),
            "GenreUnits".to_string(),
            "ReceiptGame".to_string(),
        ]
    );
    assert_eq!(
        layout.closure(&["GameGenre".to_string()]),
        vec!["GameGenre".to_string()],
        "a dependency does not drag its dependents in"
    );
}

#[test]
fn a_restricted_replay_of_a_keyed_column_is_the_whole_sink() {
    let program = program(KEYED);
    let mut interpreter = Interpreter::new(&program);
    given(
        &mut interpreter,
        "game.added",
        [("game_id", Value::Int(1)), ("genre", Value::str("rpg"))],
    );
    given(
        &mut interpreter,
        "game.purchased",
        [("receipt_id", Value::Int(50)), ("game_id", Value::Int(1))],
    );

    let tables = Tables::new(&program);
    let rebuild = tables.only(&["GenreUnits".to_string()]);
    let mut part = Cells::default();
    interpreter
        .host()
        .read(&tables.query(), &mut |record| {
            rebuild.apply(record, &mut part)
        })
        .expect("the sink applies every record it is given");

    // The dependency was replayed alongside, so the lookup had a value to read. Without
    // the closure it would have read `GameGenre`'s seed and filed the sale under
    // `"unknown"`, which is the silent wrong answer the closure exists to prevent.
    assert_eq!(
        part.get("genre", &[Key::Str("rpg".into())], "genre_units"),
        Some(&Value::Int(1))
    );
    assert!(
        part.get("genre", &[Key::Str("unknown".into())], "genre_units")
            .is_none()
    );
}

/// **A record that both moves the looked-up fold and is counted through it**, which is the
/// case that makes the as-of position observable at all, and which the handoff asked for
/// either way the question was answered.
///
/// One event feeds both folds here, so the lookup is reading a column this same record is
/// about to change. Strictly before it, the listing counts under the seed; at and
/// including it, under the genre it carries. Everything else in this file is indifferent
/// to the choice, because `@game.added` and `@game.purchased` are different events and a
/// purchase never moves `GameGenre`.
///
/// **Strictly before is the answer, and the sink is why it is not a preference.** The sink
/// works out every key before it writes any of them, so the column it reads holds the
/// value from before this record whatever anyone would prefer; a by-key read that included
/// the record would answer differently and the two drivers would part company. It is also
/// what a command appending the record would have read, and it leaves the answer
/// independent of the order the arms happen to be applied in.
const SELF_MOVING: &str = "event @game.listed { game_id: Int, genre: String }

fold GameGenre(game_id: Int) -> String = \"unknown\" {
  on @game.listed(game_id) { genre } => genre
}

fold GenreListings(genre: String) -> n: Int = 0 {
  on @game.listed() { game_id } key genre = GameGenre { game_id } => n + 1
}
";

#[test]
fn a_lookup_reads_the_log_strictly_before_the_record() {
    let program = program(SELF_MOVING);
    let mut interpreter = Interpreter::new(&program);
    for genre in ["rpg", "rpg"] {
        given(
            &mut interpreter,
            "game.listed",
            [("game_id", Value::Int(1)), ("genre", Value::str(genre))],
        );
    }

    let cells = agrees(&program, &interpreter);
    assert_eq!(
        cells.get("genre", &[Key::Str("unknown".into())], "genre_listings"),
        Some(&Value::Int(1)),
        "the first listing is counted before it has a genre to be counted under"
    );
    assert_eq!(
        cells.get("genre", &[Key::Str("rpg".into())], "genre_listings"),
        Some(&Value::Int(1)),
        "the second reads what the first left"
    );
}

// ---------------------------------------------------------------------------

/// Rule 16: the lookup is in the arm's **value** rather than its key, which is what lets a
/// counter read a fact about another key instead of carrying a copy of it.
///
/// `EditsRequested` counts edits and leaves test orders out. Before this, "is this a test
/// order?" had to be a membership set on the counter's own row, written by every event
/// that could place one, because a fold is one value out and could not read a sibling.
/// Now it is its own fold, looked up.
const GATED: &str = "subject Shop(Int)

event @order.placed { shop_id: Shop, order_id: Int, test_order: Bool }
event @edit.requested { shop_id: Shop, order_id: Int }

fold TestOrder(shop_id: Shop, order_id: Int) -> Bool = false {
  on @order.placed(shop_id, order_id) { test_order } => test_order
}

fold EditsRequested(shop_id: Shop) -> n: Int = 0 {
  on @edit.requested(shop_id) { shop_id, order_id }
    => if (TestOrder { shop_id, order_id }) { n } else { n + 1 }
}
";

#[test]
fn a_value_read_through_another_fold_agrees_with_a_refold() {
    let program = program(GATED);
    let mut interpreter = Interpreter::new(&program);
    for (order, test) in [(7, false), (8, true)] {
        given(
            &mut interpreter,
            "order.placed",
            [
                ("shop_id", Value::Int(1)),
                ("order_id", Value::Int(order)),
                ("test_order", Value::Bool(test)),
            ],
        );
    }
    for order in [7, 8] {
        given(
            &mut interpreter,
            "edit.requested",
            [("shop_id", Value::Int(1)), ("order_id", Value::Int(order))],
        );
    }

    let cells = agrees(&program, &interpreter);
    assert_eq!(
        cells.get("shop", &[Key::Int(1)], "edits_requested"),
        Some(&Value::Int(1)),
        "the real order's edit, and not the test order's"
    );
    // The counter is keyed by shop alone: nothing about the gate reached its key.
    assert_eq!(
        heklang::layout(&program)
            .column("EditsRequested")
            .expect("kept")
            .1
            .reads,
        vec!["TestOrder".to_string()],
        "a value lookup is a dependency like a key lookup"
    );
}

/// **As of the record, in the value position too**, which is the same rule and the same
/// reason: `Tables::arm` reads every lookup for a record before anything is written for
/// it, so a record that moves the looked-up fold reads what it held before.
#[test]
fn a_value_lookup_reads_the_log_strictly_before_the_record() {
    let source = "event @a.happened { x: Int, flag: Bool }

fold Flag(x: Int) -> Bool = false {
  on @a.happened(x) { flag } => flag
}

fold Counted(x: Int) -> n: Int = 0 {
  on @a.happened(x) { x } => if (Flag { x }) { n + 10 } else { n + 1 }
}
";
    let program = program(source);
    let mut interpreter = Interpreter::new(&program);
    for _ in 0..2 {
        given(
            &mut interpreter,
            "a.happened",
            [("x", Value::Int(1)), ("flag", Value::Bool(true))],
        );
    }

    let cells = agrees(&program, &interpreter);
    assert_eq!(
        cells.get("x", &[Key::Int(1)], "counted"),
        Some(&Value::Int(11)),
        "the first record sets the flag and is counted before it: 1, then 10"
    );
}

/// **A ranged arm's lookups are read once, not once per key**, which is what lets the
/// values be carried as one list per arm.
///
/// It is sound because a lookup's arguments read the record and never the key (rules 15
/// and 16 both), so every key the range covers would compute the same value. An arm that
/// wanted a lookup per key would be asking for one whose argument came from the key, and
/// that is the thing those rules refuse.
#[test]
fn a_ranged_arms_lookup_is_read_once_for_every_key_it_covers() {
    let source = "subject Shop(Int)

event @order.placed { shop_id: Shop, order_id: Int }
event @shop.swept { shop_id: Shop, swept: Bool }

fold Swept(shop_id: Shop) -> Bool = false {
  on @shop.swept(shop_id) { swept } => swept
}

fold Counted(shop_id: Shop, order_id: Int) -> n: Int = 0 {
  on @order.placed(shop_id, order_id) => n + 1
  on @shop.swept(shop_id) { shop_id } => if (Swept { shop_id }) { n + 100 } else { n + 10 }
}
";
    let program = program(source);
    let mut interpreter = Interpreter::new(&program);
    for order in [7, 8] {
        given(
            &mut interpreter,
            "order.placed",
            [("shop_id", Value::Int(1)), ("order_id", Value::Int(order))],
        );
    }
    given(
        &mut interpreter,
        "shop.swept",
        [("shop_id", Value::Int(1)), ("swept", Value::Bool(true))],
    );

    let cells = agrees(&program, &interpreter);
    // The sweep ranges over both orders and reads `Swept` as of itself, which is `false`
    // because it is the record setting it: `+ 10` at each, not `+ 100`.
    for order in [7, 8] {
        assert_eq!(
            cells.get("shop__order", &[Key::Int(1), Key::Int(order)], "counted"),
            Some(&Value::Int(11)),
            "one placement and one sweep, at order {order}"
        );
    }
}

// ---------------------------------------------------------------------------

/// Rule 19: `else skip` and absent, so the record belongs under **no key**. The two drivers
/// have to say that the same way, and they say it differently: the sink writes nothing at
/// all for the record, and a by-key read passes it over under every key. `agrees` is what
/// checks those are one answer.
///
/// Rule 18's shape, and the port's: a sale is filed under the game's developer, and a game
/// nobody announced has none. Before this the fold needed a seed to be a key part, so every
/// unannounced game's sale landed under the same invented developer.
const SKIPPED: &str = "subject Developer(Int)

event @game.announced { game_id: Int, developer_id: Developer }
event @game.purchased { game_id: Int, price_cents: Int }

fold GameDeveloper(game_id: Int) -> Developer? = none {
  on @game.announced(game_id) { developer_id } => developer_id
}

fold DeveloperGross(developer_id: Developer) -> cents: Int = 0 {
  on @game.purchased { game_id, price_cents }
    key developer_id = GameDeveloper { game_id } else skip => cents + price_cents
}
";

#[test]
fn a_skipped_record_is_written_under_no_key_by_either_driver() {
    let program = program(SKIPPED);
    let mut interpreter = Interpreter::new(&program);
    given(
        &mut interpreter,
        "game.announced",
        [("game_id", Value::Int(1)), ("developer_id", Value::Int(7))],
    );
    given(
        &mut interpreter,
        "game.purchased",
        [("game_id", Value::Int(1)), ("price_cents", Value::Int(500))],
    );
    // The game nobody announced. Its sale has no developer to be filed under.
    given(
        &mut interpreter,
        "game.purchased",
        [
            ("game_id", Value::Int(99)),
            ("price_cents", Value::Int(900)),
        ],
    );

    let cells = agrees(&program, &interpreter);
    assert_eq!(
        cells.get("developer", &[Key::Int(7)], "developer_gross"),
        Some(&Value::Int(500)),
        "the announced game's sale is filed under its developer"
    );
    // **No row at all**, which is the claim: not a zero, not a row under the seed.
    let rows: Vec<Vec<Key>> = cells.rows("developer").map(<[Key]>::to_vec).collect();
    assert_eq!(rows, [vec![Key::Int(7)]], "and the skipped sale wrote none");

    // And the by-key driver has to agree about a key the sink never minted, which is the
    // half `agrees` cannot reach: it compares at the keys the sink wrote, and the whole
    // point here is that it wrote none. Asked directly, developer 0 is the key an absent
    // part would have collapsed to.
    let head = interpreter.host().records().len() as u64;
    let zero = interpreter
        .fold_stored("DeveloperGross", [("developer_id", Value::Int(0))], head)
        .expect("the column refolds");
    assert_eq!(
        zero,
        Value::Int(0),
        "a skipped record counts under no key, not under the one an absent part reduces to"
    );
}

// ---------------------------------------------------------------------------------
// A fold's history: the accumulator after each record.

/// **The last step is the stored value**, which is the obligation that makes a history read
/// worth anything: a reader showing a timeline and a reader showing the current value have
/// to agree. Asserted against `fold_stored` at the same bound rather than against a
/// hand-written number, so the two cannot be right about different things.
#[test]
fn the_last_step_is_what_the_column_holds() {
    let source = "event @wallet.topped_up { user_id: Int, cents: Int }
event @wallet.spent { user_id: Int, cents: Int }

fold WalletBalance(user_id: Int) -> held: Int = 0 {
  on @wallet.topped_up(user_id) { cents } => held + cents
  on @wallet.spent(user_id) { cents } => held - cents
}
";
    let program = program(source);
    let mut interpreter = Interpreter::new(&program);
    for (path, cents) in [
        ("wallet.topped_up", 1000),
        ("wallet.spent", 250),
        ("wallet.topped_up", 500),
    ] {
        given(
            &mut interpreter,
            path,
            [("user_id", Value::Int(1)), ("cents", Value::Int(cents))],
        );
    }

    let head = interpreter.host().records().len() as u64;
    let mut steps: Vec<(u64, Value)> = Vec::new();
    let last = interpreter
        .fold_steps(
            "WalletBalance",
            [("user_id", Value::Int(1))],
            head,
            &mut |record, value| {
                steps.push((record.position, value.clone()));
                Ok(())
            },
        )
        .expect("the steps fold");

    assert_eq!(
        steps,
        vec![
            (0, Value::Int(1000)),
            (1, Value::Int(750)),
            (2, Value::Int(1250)),
        ]
    );
    let stored = interpreter
        .fold_stored("WalletBalance", [("user_id", Value::Int(1))], head)
        .expect("the column refolds");
    assert_eq!(steps.last().map(|(_, value)| value.clone()), Some(stored));
    assert_eq!(last, Value::Int(1250));

    // A head of zero is a log with nothing in it, so there is nothing to step through and
    // the answer is the seed. `fold_stored` takes the same number the same way.
    let mut none = 0;
    let seed = interpreter
        .fold_steps(
            "WalletBalance",
            [("user_id", Value::Int(1))],
            0,
            &mut |_, _| {
                none += 1;
                Ok(())
            },
        )
        .expect("the seed folds");
    assert_eq!((none, seed), (0, Value::Int(0)));
}

/// **A record that moved nothing is still a step**, and that is deliberate: a reader diffing
/// consecutive values is what decides which positions are changes, and "the value after this
/// record" is a fact about every record the query delivered. Deciding it here would decide it
/// for every caller, and a caller wanting only the changes has the values to compare.
#[test]
fn a_record_that_changed_nothing_is_still_reported() {
    let source = "event @wallet.topped_up { user_id: Int, cents: Int }

fold WalletBalance(user_id: Int) -> held: Int = 0 {
  on @wallet.topped_up(user_id) { cents } => held + cents
}
";
    let program = program(source);
    let mut interpreter = Interpreter::new(&program);
    for cents in [1000, 0, 500] {
        given(
            &mut interpreter,
            "wallet.topped_up",
            [("user_id", Value::Int(1)), ("cents", Value::Int(cents))],
        );
    }

    let mut steps: Vec<Value> = Vec::new();
    interpreter
        .fold_steps(
            "WalletBalance",
            [("user_id", Value::Int(1))],
            3,
            &mut |_, value| {
                steps.push(value.clone());
                Ok(())
            },
        )
        .expect("the steps fold");
    assert_eq!(
        steps,
        vec![Value::Int(1000), Value::Int(1000), Value::Int(1500)],
        "the zero top-up is a step whose value did not move"
    );
}

/// **A ranged column is refused**, because this is exactly where the two drivers are defined
/// to disagree: `a_key_minted_after_a_ranged_record_does_not_see_it` above has the sink
/// answering 11 and a refold answering 21 for one key over one log. A by-key history would
/// report the refold's steps, so it would show a move the stored column never made, which is
/// the wrongness a history read exists to remove. `Column::ranges` is the same question
/// asked once.
#[test]
fn a_ranged_column_has_no_history() {
    let source = "event @shop.uninstalled { shop_id: Int }
event @order.placed { shop_id: Int, order_id: Int }

fold OrderClosed(shop_id: Int, order_id: Int) -> Bool = false {
  on @order.placed(shop_id, order_id) => false
  on @shop.uninstalled(shop_id) => true
}

fold OrderSeen(shop_id: Int, order_id: Int) -> n: Int = 0 {
  on @order.placed(shop_id, order_id) => n + 1
}
";
    let program = program(source);
    let interpreter = Interpreter::new(&program);
    let err = interpreter
        .fold_steps(
            "OrderClosed",
            [("shop_id", Value::Int(1)), ("order_id", Value::Int(9))],
            0,
            &mut |_, _| Ok(()),
        )
        .expect_err("a ranged column has no history");
    let message = err.to_string();
    assert!(message.contains("leaves `order` open"), "got: {message}");
    assert!(
        message.contains("steps the stored column never took"),
        "the reason is the disagreement, not the cost: {message}"
    );

    // And the column beside it, on the same key, does have one: the refusal is per column
    // and not per table.
    interpreter
        .fold_steps(
            "OrderSeen",
            [("shop_id", Value::Int(1)), ("order_id", Value::Int(9))],
            0,
            &mut |_, _| Ok(()),
        )
        .expect("every arm pins the whole key");

    // The same question a host asks once, rather than per request.
    let tables = Tables::new(&program);
    let ranges = |fold: &str| tables.layout().column(fold).expect("the column").1.ranges();
    assert!(ranges("OrderClosed"));
    assert!(!ranges("OrderSeen"));
}

/// **A computed key and a lookup are allowed**, which is the other half of the ranged-arm
/// refusal and deliberately not symmetrical with it. Those two *agree* between the drivers,
/// which is the claim this whole file exists to make, so what they cost is time and not
/// correctness: the lookup is folded again at every candidate record. A caller can decide
/// about a cost. It cannot decide about a wrong answer, which is why the one case where the
/// drivers differ is the one case refused.
#[test]
fn a_computed_key_and_a_lookup_both_have_a_history() {
    let source = "event @workshop.opened { workshop_id: Int, title: String }
event @seat.booked { workshop_id: Int, at_month: String, paid: Int }

fold WorkshopTitle(workshop_id: Int) -> String = \"untitled\" {
  on @workshop.opened(workshop_id) { title } => title
}

fold MonthlyRevenue(workshop_id: Int, month: String) -> taken: Int = 0 {
  on @seat.booked(workshop_id) { at_month, paid } key month = at_month => taken + paid
}

fold TitleRevenue(title: String) -> taken: Int = 0 {
  on @seat.booked { workshop_id, paid } key title = WorkshopTitle { workshop_id } =>
    taken + paid
}
";
    let program = program(source);
    let mut interpreter = Interpreter::new(&program);
    given(
        &mut interpreter,
        "workshop.opened",
        [
            ("workshop_id", Value::Int(1)),
            ("title", Value::str("Rust")),
        ],
    );
    for paid in [40, 60] {
        given(
            &mut interpreter,
            "seat.booked",
            [
                ("workshop_id", Value::Int(1)),
                ("at_month", Value::str("2020-01")),
                ("paid", Value::Int(paid)),
            ],
        );
    }

    let head = interpreter.host().records().len() as u64;
    let steps = |fold: &str, args: Vec<(&'static str, Value)>| {
        let mut seen: Vec<Value> = Vec::new();
        let last = interpreter
            .fold_steps(fold, args, head, &mut |_, value| {
                seen.push(value.clone());
                Ok(())
            })
            .unwrap_or_else(|err| panic!("{fold}: {err}"));
        (seen, last)
    };

    // The computed part is compared per record, so only this bucket's records are steps.
    let (computed, last) = steps(
        "MonthlyRevenue",
        vec![
            ("workshop_id", Value::Int(1)),
            ("month", Value::str("2020-01")),
        ],
    );
    assert_eq!(computed, vec![Value::Int(40), Value::Int(100)]);
    assert_eq!(last, Value::Int(100));

    // And the lookup is answered as of each record, which is what makes the first booking
    // land under the title the workshop already had.
    let (looked_up, last) = steps("TitleRevenue", vec![("title", Value::str("Rust"))]);
    assert_eq!(looked_up, vec![Value::Int(40), Value::Int(100)]);
    assert_eq!(last, Value::Int(100));

    // Both agree with the stored read at the same bound, which is the obligation.
    for (fold, args) in [
        (
            "MonthlyRevenue",
            vec![
                ("workshop_id", Value::Int(1)),
                ("month", Value::str("2020-01")),
            ],
        ),
        ("TitleRevenue", vec![("title", Value::str("Rust"))]),
    ] {
        let stored = interpreter
            .fold_stored(fold, args, head)
            .unwrap_or_else(|err| panic!("{fold}: {err}"));
        assert_eq!(stored, Value::Int(100), "{fold}");
    }
}

/// A **query-only** column has a history like any other, and a port's price timeline is one:
/// an arm binding the envelope with `as e` is what makes a column hold when something
/// happened, and that is exactly the column a reader wants a timeline of. The read is not a
/// decision, which is the same allowance a test's `fold` action has.
#[test]
fn a_query_only_column_has_a_history() {
    let source = "event @game.priced { game_id: Int, cents: Int }

fold GamePricedAt(game_id: Int) -> Timestamp? = none {
  on @game.priced as e (game_id) => e.at
}
";
    let program = program(source);
    let mut interpreter = Interpreter::new(&program);
    for cents in [1000, 1200] {
        given(
            &mut interpreter,
            "game.priced",
            [("game_id", Value::Int(1)), ("cents", Value::Int(cents))],
        );
    }

    let mut steps: Vec<Value> = Vec::new();
    let last = interpreter
        .fold_steps(
            "GamePricedAt",
            [("game_id", Value::Int(1))],
            2,
            &mut |_, value| {
                steps.push(value.clone());
                Ok(())
            },
        )
        .expect("a query-only column steps");
    // The harness stamps one minute per event from 2020-01-01T00:00:00Z, so these are the
    // first two positions and the last step is what the column holds.
    assert_eq!(
        steps,
        vec![
            Value::some(Value::Timestamp(1_577_836_800_000_000)),
            Value::some(Value::Timestamp(1_577_836_860_000_000)),
        ]
    );
    assert_eq!(steps.last().cloned(), Some(last));
}

/// **And a column whose key is read through a ranged one has no history either**, which is
/// the half that asking `Column::ranges` alone missed. Every arm of `NotesByDiscount` pins
/// its whole key, so it looked exact; its key comes out of `OrderDiscounts`, which ranges,
/// and a lookup is answered by **refolding** the looked-up fold. A refold applies every
/// ranged record to every key, so the key this computes is one the store never wrote a row
/// at: the sink filed the note under `0` and a by-key read finds it under `10`.
///
/// Asserted both ways round, because the point is not that the read is slow. It is that the
/// two answers are different, and a timeline of a row that does not exist is worse than no
/// timeline.
#[test]
fn a_column_keyed_through_a_ranged_one_has_no_history() {
    let source = "event @shop.discounted { why: String }
event @order.placed { order_id: Int }
event @note.added { order_id: Int }

fold OrderDiscounts(order_id: Int) -> n: Int = 0 {
  on @order.placed(order_id) => n
  on @shop.discounted => n + 10
}

fold NotesByDiscount(taken: Int) -> n: Int = 0 {
  on @note.added { order_id } key taken = OrderDiscounts { order_id } => n + 1
}
";
    let program = program(source);
    let mut interpreter = Interpreter::new(&program);
    given(
        &mut interpreter,
        "shop.discounted",
        [("why", Value::str("sale"))],
    );
    given(
        &mut interpreter,
        "order.placed",
        [("order_id", Value::Int(9))],
    );
    given(
        &mut interpreter,
        "note.added",
        [("order_id", Value::Int(9))],
    );

    // The column itself does not range, which is what made this look exact.
    let tables = Tables::new(&program);
    let column = |fold: &str| {
        tables
            .layout()
            .column(fold)
            .unwrap_or_else(|| panic!("{fold} is kept"))
            .1
    };
    assert!(!column("NotesByDiscount").ranges());
    assert!(column("OrderDiscounts").ranges());
    // The closure is the question, and it names the fold that has to change.
    assert_eq!(
        tables
            .layout()
            .ranged_through("NotesByDiscount")
            .map(|at| at.to_string()),
        Some("OrderDiscounts".to_string())
    );

    let err = interpreter
        .fold_steps(
            "NotesByDiscount",
            [("taken", Value::Int(10))],
            3,
            &mut |_, _| Ok(()),
        )
        .expect_err("a column keyed through a ranged one has no history");
    let message = err.to_string();
    assert!(
        message.contains("`NotesByDiscount` reads `OrderDiscounts`"),
        "it names the fold that has to change: {message}"
    );
    assert!(
        message.contains("leaves `order` open"),
        "and the part that has to be pinned: {message}"
    );

    // The disagreement the refusal is about: the sink wrote the row at `0` and the by-key
    // read finds the note under `10`.
    let mut cells = Cells::default();
    interpreter
        .host()
        .read(&tables.query(), &mut |record| {
            tables.apply(record, &mut cells)
        })
        .expect("the sink applies every record");
    assert_eq!(
        cells.get("taken", &[Key::Int(0)], "notes_by_discount"),
        Some(&Value::Int(1)),
        "the sink used the discount the store held"
    );
    assert_eq!(
        interpreter
            .fold_stored("NotesByDiscount", [("taken", Value::Int(10))], 3)
            .expect("the refold answers"),
        Value::Int(1),
        "and a refold uses one it never held"
    );
}
