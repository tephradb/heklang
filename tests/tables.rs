//! `src/tables.rs` as executable rules: the read model a program's named folds imply.
//!
//! A fold's parameters are its key, the events it folds are what changes it, and a table
//! is the folds that share a key. What is worth proving is the part a reader of the
//! source cannot check by eye: which parameter an arm actually supplies, which event
//! field fills it, and what orders a key once it has more than one part.

use heklang::{ArmMatch, Layout, Program, Value, fixed, layout, parse};

fn program(source: &str) -> Program {
    match parse(source) {
        Ok(program) => program,
        Err(err) => panic!("{err}"),
    }
}

fn read(source: &str) -> Layout {
    layout(&program(source))
}

/// One line per table, `part__part  column, column` (`()` for the keyless one), which is
/// the whole of what a layout says
/// about grouping.
fn tables(source: &str) -> Vec<String> {
    read(source)
        .tables
        .iter()
        .map(|table| {
            let columns: Vec<&str> = table
                .columns
                .iter()
                .map(|column| column.name.as_str())
                .collect();
            let key: Vec<&str> = table.key.iter().map(|part| part.name.as_str()).collect();
            let key = match key.is_empty() {
                true => "()".to_string(),
                false => key.join("__"),
            };
            format!("{key}  {}", columns.join(", "))
        })
        .collect()
}

/// One line per fold that cannot be kept, `Name  param, param`.
fn unstored(source: &str) -> Vec<String> {
    read(source)
        .unstored
        .iter()
        .map(|fold| format!("{}  {}", fold.fold, fold.unmatched.join(", ")))
        .collect()
}

const SUBJECTS: &str = "\
subject Shop(Int)
subject Customer(String) under Shop
subject Region(String)
";

const EVENTS: &str = "\
event @order.placed { shop_id: Shop, order_id: Int, total: Int }
event @order.cancelled { shop_id: Shop, order_id: Int }
event @shop.installed { shop_id: Shop }
event @shop.blocked { shop_id: Shop, code: String }
event @shop.unblocked { shop_id: Shop }
event @contact.recorded { shop_id: Shop, customer_id: Customer }
event @visit.logged { shop_id: Shop, region_id: Region }
event @transfer.recorded { from_id: Region, to_id: Region }
event @usage.seen { shop_id: Shop, on_day: String }
";

fn app(body: &str) -> String {
    format!("{SUBJECTS}{EVENTS}{body}")
}

/// A fold's parameters are its key, and the folds that share a key share a table. The two
/// here declare their parameters in opposite orders, because a key is a set of parts and
/// not the argument list a caller writes.
#[test]
fn folds_that_share_a_key_share_a_table() {
    let source = app("
fold OrderTotal(shop_id: Shop, order_id: Int) -> sum: Int = 0 {
  on @order.placed(shop_id, order_id) { total } => sum + total
}

fold OrderCancelled(order_id: Int, shop_id: Shop) -> Bool = false {
  on @order.cancelled(shop_id, order_id) => true
}
");

    assert_eq!(
        tables(&source),
        ["shop__order  order_cancelled, order_total"]
    );
}

/// A key sorts above every key that extends it, so a narrow table sits next to the one it
/// narrows rather than wherever its name happens to fall.
#[test]
fn a_key_sorts_above_every_key_that_extends_it() {
    let source = app("
fold OrderCancelled(shop_id: Shop, order_id: Int) -> Bool = false {
  on @order.cancelled(shop_id, order_id) => true
}

fold ShopInstalled(shop_id: Shop) -> Bool = false {
  on @shop.installed(shop_id) => true
}
");

    assert_eq!(
        tables(&source),
        ["shop  shop_installed", "shop__order  order_cancelled"]
    );
}

/// A subject part leads a plain one however the fold wrote them, so a table reads as the
/// subject it belongs to first.
#[test]
fn a_subject_part_leads_a_plain_part() {
    let source = app("
fold OrderCancelled(order_id: Int, shop_id: Shop) -> Bool = false {
  on @order.cancelled(shop_id, order_id) => true
}
");

    let layout = read(&source);
    assert_eq!(tables(&source), ["shop__order  order_cancelled"]);
    assert_eq!(layout.tables[0].key[0].subject.as_deref(), Some("Shop"));
    assert_eq!(layout.tables[0].key[1].subject, None);
}

/// Subjects order by containment, so a parent sits above the child whose key wraps under
/// it. Alphabetical would have given the other answer, which is the point of the fixture.
#[test]
fn subjects_order_by_containment() {
    let source = app("
fold ContactRecorded(customer_id: Customer, shop_id: Shop) -> Bool = false {
  on @contact.recorded(shop_id, customer_id) => true
}
");

    assert_eq!(tables(&source), ["shop__customer  contact_recorded"]);
}

/// Containment relates only some pairs. The rest go alphabetically, which is what makes
/// the whole thing an order rather than a preference.
#[test]
fn unrelated_subjects_order_alphabetically() {
    let source = app("
fold VisitLogged(shop_id: Shop, region_id: Region) -> Bool = false {
  on @visit.logged(shop_id, region_id) => true
}
");

    assert_eq!(tables(&source), ["region__shop  visit_logged"]);
}

/// Plain parts go alphabetically too, after every subject part.
#[test]
fn plain_parts_order_alphabetically() {
    let source = app("
fold Blocked(code: String, shop_id: Shop, order_id: Int) -> Bool = false {
  on @shop.blocked(shop_id, code) => true
  on @order.placed(shop_id, order_id) => false
}
");

    assert_eq!(tables(&source), ["shop__code__order  blocked"]);
}

/// Two parts of one key can land on one name, by repeating a subject or by spelling it
/// two ways, and neither is a mistake. **They are told apart by the parameter, never by
/// position.**
///
/// Position was the first answer and `/code-review` found it wrong twice. A suffix
/// counting declaration order made two folds declaring the same pair in opposite orders
/// into one table whose key columns meant different parameters in each, so a writer would
/// store one row's two columns at each other's addresses; and a suffix is spellable, so a
/// parameter actually called `region_1` collided with another part's suffix.
#[test]
fn two_parts_of_one_name_are_told_apart_by_the_parameter() {
    let source = app("
fold TransferSeen(to_id: Region, from_id: Region) -> Bool = false {
  on @transfer.recorded(from_id, to_id) => true
}
");

    let layout = read(&source);
    let parts: Vec<&str> = layout.tables[0]
        .key
        .iter()
        .map(|part| part.name.as_str())
        .collect();
    assert_eq!(parts, ["from", "to"]);
}

/// **The bug position caused.** Two folds declaring one pair in opposite orders are one
/// table, and the column each key part is filled from has to agree between them. Under
/// the positional suffix both derived `(region_1, region_2)` and `same` merged them, with
/// `region_1` filled from `to_id` in one fold and `from_id` in the other.
#[test]
fn transposed_parameters_of_one_pair_are_one_table_the_same_way_round() {
    let source = app("
fold TransferSeen(to_id: Region, from_id: Region) -> Bool = false {
  on @transfer.recorded(from_id, to_id) => true
}

fold TransferCount(from_id: Region, to_id: Region) -> n: Int = 0 {
  on @transfer.recorded(from_id, to_id) => n + 1
}
");

    assert_eq!(tables(&source), ["from__to  transfer_count, transfer_seen"]);

    // And both columns fill `from` from the event's `from_id`, which is the half the
    // merge got wrong: the names agreed and the parameters behind them did not.
    let layout = read(&source);
    for column in &layout.tables[0].columns {
        assert_eq!(
            column.arms[0].fields,
            [
                ArmMatch::Field("from_id".into()),
                ArmMatch::Field("to_id".into())
            ],
            "{} fills the key from the wrong fields",
            column.name
        );
    }
}

/// A part whose parameter name is what another part's suffix used to be. `region_1` is a
/// legal parameter, so a generated `region_1` could never have been safe.
#[test]
fn a_generated_name_cannot_collide_with_a_parameter() {
    let source = app("
fold Dup(from_id: Region, to_id: Region, region_1: Int) -> Bool = false {
  on @transfer.recorded(from_id, to_id) => true
  on @order.placed(order_id: region_1) => true
}
");

    let layout = read(&source);
    let parts: Vec<&str> = layout.tables[0]
        .key
        .iter()
        .map(|part| part.name.as_str())
        .collect();
    assert_eq!(parts, ["from", "to", "region_1"]);
}

/// A plain part loses one trailing `_id`: the column holds an order, and `order_id` in a
/// key named `order` says the same thing twice. So the two spellings are one table.
#[test]
fn a_plain_part_loses_one_trailing_id() {
    let source = app("
fold Blocked(shop_id: Shop, code: String) -> Bool = false {
  on @shop.blocked(shop_id, code) => true
}

fold Placed(shop_id: Shop, order_id: Int) -> Bool = false {
  on @order.placed(shop_id, order_id) => true
}

fold Cancelled(shop_id: Shop, order: Int) -> Bool = false {
  on @order.cancelled(shop_id, order_id: order) => true
}
");

    assert_eq!(
        tables(&source),
        ["shop__code  blocked", "shop__order  cancelled, placed"]
    );
}

/// A fold with no parameters is one value for the whole log, so it is kept in one row.
#[test]
fn a_fold_with_no_parameters_is_the_keyless_table() {
    let source = app("
fold OrdersPlaced() -> placed: Int = 0 {
  on @order.placed() => placed + 1
}
");

    let layout = read(&source);
    assert_eq!(tables(&source), ["()  orders_placed"]);
    assert!(layout.tables[0].key.is_empty());
}

/// **Some arm, not every arm.** An arm matching a subset of the key is a range update
/// over the parts it leaves open, so the fold is still kept. This is `SetupCodeBlocked`'s
/// shape, the live one in the ported application.
#[test]
fn an_arm_matching_part_of_the_key_still_stores() {
    let source = app("
fold CodeBlocked(code: String, shop_id: Shop) -> Bool = false {
  on @shop.blocked(shop_id, code) => true
  on @shop.unblocked(shop_id) => false
}
");

    assert_eq!(tables(&source), ["shop__code  code_blocked"]);
    assert!(unstored(&source).is_empty());

    // And the arm that leaves `code` open says so, which is how a writer knows it is a
    // range update rather than a point one.
    let layout = read(&source);
    let arms = &layout.tables[0].columns[0].arms;
    assert_eq!(
        arms[0].fields,
        [
            ArmMatch::Field("shop_id".into()),
            ArmMatch::Field("code".into())
        ]
    );
    assert_eq!(
        arms[1].fields,
        [ArmMatch::Field("shop_id".into()), ArmMatch::Open]
    );
}

/// The event's field and the parameter it binds are two names, and often two different
/// ones. Matching on the field would have put this fold in the wrong table, and twelve
/// arms in the ported application are written this way.
#[test]
fn a_filter_whose_field_differs_from_the_parameter_matches() {
    let source = app("
fold Placed(shop_id: Shop, ticket_id: Int) -> Bool = false {
  on @order.placed(shop_id, order_id: ticket_id) => true
}
");

    assert_eq!(tables(&source), ["shop__ticket  placed"]);

    // A key column called `ticket`, filled from a field called `order_id`. Nothing but
    // this mapping says so, which is why the layout carries it.
    let layout = read(&source);
    let arm = &layout.tables[0].columns[0].arms[0];
    assert_eq!(
        arm.fields,
        [
            ArmMatch::Field("shop_id".into()),
            ArmMatch::Field("order_id".into())
        ]
    );
}

/// A parameter no arm supplies is not a key anything could be written under, whether it
/// was used in the seed or nowhere at all.
#[test]
fn a_parameter_no_arm_supplies_is_not_stored() {
    let source = app("
fold OrdersSince(at: Int, shop_id: Shop) -> count: Int = at {
  on @order.placed(shop_id) => count + 1
}
");

    assert!(tables(&source).is_empty());
    assert_eq!(unstored(&source), ["OrdersSince  at"]);
}

/// A filter that computes its value reads the parameter without pinning the field to it,
/// so the key cannot be read back out of a row and the fold is not kept. A walk looking
/// for the slot anywhere in the value would have accepted this.
#[test]
fn a_computed_filter_value_does_not_match() {
    let source = app("
fn normalise(day: String) -> String {
  return day.lower()
}

fold UsageOn(shop_id: Shop, day: String) -> Bool = false {
  on @usage.seen(shop_id, on_day: normalise(day)) => true
}
");

    assert!(tables(&source).is_empty());
    assert_eq!(unstored(&source), ["UsageOn  day"]);
}

/// A column carries what a runtime needs to build it: the fold's name in snake_case with
/// its prefix kept, its type, and every event that changes it, first written first and
/// each once however many arms read it.
#[test]
fn a_column_names_its_type_and_its_events() {
    let source = app("
fold OrderTotal(shop_id: Shop, order_id: Int) -> sum: Int? = none {
  on @order.placed(shop_id, order_id) { total } => total
  on @order.cancelled(shop_id, order_id) => none
  on @order.placed(shop_id, order_id) => sum
}
");

    let layout = read(&source);
    let column = &layout.tables[0].columns[0];
    assert_eq!(column.name, "order_total");
    assert_eq!(column.fold, "OrderTotal");
    assert_eq!(column.ty.to_string(), "Int?");
    let events: Vec<String> = column.events.iter().map(ToString::to_string).collect();
    assert_eq!(events, ["@order.placed", "@order.cancelled"]);
    assert_eq!(column.arms.len(), 3, "one per arm, not one per event");
}

/// A column is a declaration, so an inline fold is not one: it has no name to be a column
/// and no parameters to be a key.
#[test]
fn an_inline_fold_is_not_a_column() {
    let source = app("
command CancelOrder(shop_id: Shop, order_id: Int) {
  fold placed: Bool = false
    on @order.placed(shop_id, order_id) => true
  if placed {
    emit @order.cancelled { shop_id, order_id }
  }
}
");

    assert!(tables(&source).is_empty());
    assert!(unstored(&source).is_empty());
}

/// Declaration order is irrelevant across modules (`docs/modules.md`), so it has to be
/// irrelevant here too. The test that catches a layout accidentally built over a hash.
#[test]
fn the_layout_does_not_depend_on_declaration_order() {
    let first = "
fold ShopInstalled(shop_id: Shop) -> Bool = false {
  on @shop.installed(shop_id) => true
}

fold OrderCancelled(shop_id: Shop, order_id: Int) -> Bool = false {
  on @order.cancelled(shop_id, order_id) => true
}

fold OrderPlaced(shop_id: Shop, order_id: Int) -> Bool = false {
  on @order.placed(shop_id, order_id) => true
}
";
    let second = "
fold OrderPlaced(shop_id: Shop, order_id: Int) -> Bool = false {
  on @order.placed(shop_id, order_id) => true
}

fold ShopInstalled(shop_id: Shop) -> Bool = false {
  on @shop.installed(shop_id) => true
}

fold OrderCancelled(shop_id: Shop, order_id: Int) -> Bool = false {
  on @order.cancelled(shop_id, order_id) => true
}
";

    assert_eq!(tables(&app(first)), tables(&app(second)));
    assert_eq!(
        tables(&app(first)),
        [
            "shop  shop_installed",
            "shop__order  order_cancelled, order_placed"
        ]
    );
}

/// Two keys alike in their part names and not their types are two tables, and their ids
/// differ because the type is in what the id hashes.
#[test]
fn two_keys_that_differ_only_by_type_do_not_share_an_id() {
    let source = app("
fold Early(day: String) -> Bool = false {
  on @usage.seen(on_day: day) => true
}

fold Late(day: Int) -> Bool = false {
  on @order.placed(order_id: day) => true
}
");

    let layout = read(&source);
    assert_eq!(layout.tables.len(), 2);
    assert_ne!(layout.tables[0].id, layout.tables[1].id);
}

/// A subject part is filed under its id type, so changing that type changes how every key of
/// the table is encoded and has to move the id, although the part's name and subject do not.
#[test]
fn a_subject_part_carries_its_id_type_into_the_table_id() {
    let id = |ty: &str| {
        let source = format!(
            "subject Shop({ty})\nevent @shop.installed {{ shop_id: Shop }}\nfold Installed(shop_id: Shop) -> Bool = false {{\n  on @shop.installed(shop_id) => true\n}}\n"
        );
        let layout = read(&source);
        layout.tables[0].id.clone()
    };
    assert_ne!(id("Int"), id("String"));
}

/// **A table's id is its own key's and nothing else's**, which is what the old derived
/// name was not: two keys that joined to one name were both renamed, so adding a fold
/// could move a table nobody touched. It is also never an author's: nothing a program
/// declares is spelled like one.
#[test]
fn a_table_id_depends_on_its_own_key_alone() {
    let alone = read(&app("
fold Placed(shop_id: Shop, order_id: Int) -> n: Int = 0 {
  on @order.placed(shop_id, order_id) => n + 1
}
"));
    let beside = read(&app("
fold Placed(shop_id: Shop, order_id: Int) -> n: Int = 0 {
  on @order.placed(shop_id, order_id) => n + 1
}

fold PlacedByCode(shop_id: Shop, order_id: String) -> n: Int = 0 {
  on @shop.blocked(shop_id, code: order_id) => n + 1
}

fold Total() -> n: Int = 0 {
  on @order.placed() => n + 1
}
"));
    let (table, _) = alone.column("Placed").expect("kept");
    let (same, _) = beside.column("Placed").expect("kept");
    assert_eq!(table.id, same.id);

    assert!(table.id.starts_with("t_"));
    assert_eq!(table.id.len(), 18);
    assert!(table.id[2..].chars().all(|c| c.is_ascii_hexdigit()));

    let ids: Vec<&str> = beside
        .tables
        .iter()
        .map(|table| table.id.as_str())
        .collect();
    let mut unique = ids.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(ids.len(), unique.len(), "three keys, three ids");
}

/// A part spelled `global` is an ordinary part. The word was reserved for the keyless table
/// while a test addressed that table by it, and nothing addresses a table any more.
#[test]
fn a_part_called_global_is_an_ordinary_part() {
    let source = app("
fold OrdersPlaced() -> placed: Int = 0 {
  on @order.placed() => placed + 1
}

fold ScopePlaced(global: Int) -> placed: Int = 0 {
  on @order.placed(order_id: global) => placed + 1
}
");

    assert_eq!(
        tables(&source),
        ["()  orders_placed", "global  scope_placed"]
    );
}

/// A key part no event carries, from `docs/folds.md` rule 13's `param == value`. The
/// part is matched, so the fold is storable, and it is matched by a **value** rather than
/// by a field, which is the third state `ArmMatch` exists for.
#[test]
fn a_key_part_an_arm_fixes_is_matched_without_a_field() {
    let source = app("
fold Counted(shop_id: Shop, kind: String) -> n: Int = 0 {
  on @order.placed(shop_id, kind == \"placed\") => n + 1
  on @order.cancelled(shop_id, kind == \"cancelled\") => n + 1
}
");

    assert_eq!(tables(&source), ["shop__kind  counted"]);
    assert!(
        unstored(&source).is_empty(),
        "the kind is supplied by every arm, so the fold is kept"
    );

    let arms = &read(&source).tables[0].columns[0].arms;
    assert!(matches!(arms[0].fields[0], ArmMatch::Field(ref field) if field == "shop_id"));
    // Fixed and not Open, which is the distinction that matters to whatever writes the
    // row: Open means range over the part, and this arm fixed it.
    assert!(matches!(arms[0].fields[1], ArmMatch::Fixed(_)));
    assert!(matches!(arms[1].fields[1], ArmMatch::Fixed(_)));
}

/// And one no arm fixes is still unstored, so the new form earns storability rather than
/// granting it: `kind` here is a parameter the seed reads and no arm names.
#[test]
fn a_key_part_no_arm_fixes_is_still_unstored() {
    let source = app("
fold Counted(shop_id: Shop, kind: String) -> n: String = kind {
  on @order.placed(shop_id) => n
}
");

    assert_eq!(unstored(&source), ["Counted  kind"]);
}

/// An arm may both narrow the event's own field and fix the key part, and the key
/// conjunct is the stronger statement, so it is what the layout records. Found by
/// `/code-review`: taking the field first told a writer to write at whatever the event
/// carried, while the read side answered the seed for every key but the fixed one.
#[test]
fn a_part_an_arm_both_binds_and_fixes_records_the_fixed_value() {
    let source = app("
fold Counted(shop_id: Shop, code: String) -> n: Int = 0 {
  on @shop.blocked(shop_id, code, code == \"plan_limit\") => n + 1
}
");

    assert_eq!(tables(&source), ["shop__code  counted"]);
    let arm = &read(&source).tables[0].columns[0].arms[0];
    assert!(matches!(arm.fields[0], ArmMatch::Field(ref field) if field == "shop_id"));
    assert!(
        matches!(arm.fields[1], ArmMatch::Fixed(_)),
        "the gate wins over the field bind, because it is what the row is written at"
    );
}

// ---------------------------------------------------------------------------
// Rule 13, from the store's side: reading the value an arm fixes a part to
// ---------------------------------------------------------------------------

#[test]
fn the_value_an_arm_fixes_a_key_part_to_is_readable() {
    let source = "subject Shop(Int)
event @hold.place.failed { shop_id: Shop, order_id: Int, attempt: Int }
event @hold.release.failed { shop_id: Shop, order_id: Int, attempt: Int }

fold RetryAttempt(shop_id: Shop, kind: String, order_id: Int) -> Int = 0 {
  on @hold.place.failed(shop_id, order_id, kind == \"hold_place\") { attempt } => attempt
  on @hold.release.failed(shop_id, order_id, kind == \"hold_release\") { attempt } => attempt
}
";
    let program = program(source);
    let layout = layout(&program);
    let table = &layout.tables[0];
    let at = table
        .key
        .iter()
        .position(|part| part.name == "kind")
        .expect("`kind` is a key part");
    let column = &table.columns[0];

    let pinned: Vec<Value> = column
        .arms
        .iter()
        .map(|arm| match arm.fields[at] {
            ArmMatch::Fixed(id) => fixed(&program, &column.fold, &table.key[at], id)
                .expect("a conjunct resolves against an empty frame"),
            ref other => panic!("expected a fixed part, got {other:?}"),
        })
        .collect();

    assert_eq!(
        pinned,
        vec![Value::str("hold_place"), Value::str("hold_release")],
        "a writer holds the event and the key it is writing and no frame, so this is the \
         only way it learns which key an arm writes"
    );
}

#[test]
fn a_fixed_value_arrives_at_the_parts_own_type() {
    let source = "subject Shop(Int)
event @order.placed { shop_id: Shop, total: Money(2) }

fold PlacedAtFloor(shop_id: Shop, floor: Money(2)) -> n: Int = 0 {
  on @order.placed(shop_id, floor == 0) => n + 1
}
";
    let program = program(source);
    let layout = layout(&program);
    let table = &layout.tables[0];
    let at = table
        .key
        .iter()
        .position(|part| part.name == "floor")
        .expect("`floor` is a key part");
    let column = &table.columns[0];
    let ArmMatch::Fixed(id) = column.arms[0].fields[at] else {
        panic!("expected a fixed part");
    };

    assert_eq!(
        fixed(&program, &column.fold, &table.key[at], id).expect("resolves"),
        Value::money(0, 2),
        "a part of Money(2) pinned to `0` is an amount, not an Int: the row is written at \
         the value the fold declares, which is the same fit the interpreter makes"
    );
}

/// Rule 20: the key **positions** each index leads with, in the table's key order, with
/// the column itself implied last. Positions rather than the parameter names the author
/// wrote, because what an index can answer is decided by the order its columns sit in and
/// that order is the table's: `@index(game_id)` written on either of these is `[0]`,
/// whichever way round the parameters were declared.
#[test]
fn an_index_is_recorded_as_key_positions() {
    let source = app("
fold First(from_id: Region, to_id: Region) -> n: Int @index(from_id) = 0 {
  on @transfer.recorded(from_id, to_id) => n + 1
}

fold Second(to_id: Region, from_id: Region) -> n: Int @index(from_id, to_id) = 0 {
  on @transfer.recorded(from_id, to_id) => n + 1
}

fold Third(from_id: Region, to_id: Region) -> n: Int @index = 0 {
  on @transfer.recorded(from_id, to_id) => n + 1
}
");

    let layout = read(&source);
    // One table, since a key is a set of parts and not an argument list.
    assert_eq!(tables(&source), ["from__to  first, second, third"]);
    let indexes = |name: &str| layout.column(name).expect("the column").1.indexes.clone();
    assert_eq!(indexes("First"), [vec![0]]);
    assert_eq!(indexes("Second"), [vec![0, 1]]);
    // The column alone, which is the whole of a bare `@index`.
    assert_eq!(indexes("Third"), [Vec::<usize>::new()]);
}
