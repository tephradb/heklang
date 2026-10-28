//! `src/tables.rs` as executable rules: the read model a program's named folds imply.
//!
//! A fold's parameters are its key, the events it folds are what changes it, and a table
//! is the folds that share a key. What is worth proving is the part a reader of the
//! source cannot check by eye: which parameter an arm actually supplies, which event
//! field fills it, and what orders a key once it has more than one part.

use heklang::{Layout, Program, layout, parse};

fn program(source: &str) -> Program {
    match parse(source) {
        Ok(program) => program,
        Err(err) => panic!("{err}"),
    }
}

fn read(source: &str) -> Layout {
    layout(&program(source))
}

/// One line per table, `name  column, column`, which is the whole of what a layout says
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
            format!("{}  {}", table.name, columns.join(", "))
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

/// Two parts of one key can land on one name. Neither spelling is a mistake, so they are
/// told apart by position, counted in the order the fold declares them. No fold in the
/// ported application does this, so this is the only thing holding the rule up.
#[test]
fn two_parts_of_one_name_become_positional() {
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
    assert_eq!(parts, ["region_1", "region_2"]);
    assert_eq!(layout.tables[0].name, "region_1__region_2");
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
fn a_fold_with_no_parameters_is_the_global_table() {
    let source = app("
fold OrdersPlaced() -> placed: Int = 0 {
  on @order.placed() => placed + 1
}
");

    let layout = read(&source);
    assert_eq!(tables(&source), ["global  orders_placed"]);
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
        [Some("shop_id".into()), Some("code".into())]
    );
    assert_eq!(arms[1].fields, [Some("shop_id".into()), None]);
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
        [Some("shop_id".into()), Some("order_id".into())]
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

/// Two key signatures can derive one name, because a part is told apart by name and type
/// and a name is only the names joined. No program has done it, and a runtime keying on
/// the name would serve one table for both, so each takes the type into its name.
#[test]
fn two_keys_that_differ_only_by_type_do_not_share_a_name() {
    let source = app("
fold Early(day: String) -> Bool = false {
  on @usage.seen(on_day: day) => true
}

fold Late(day: Int) -> Bool = false {
  on @order.placed(order_id: day) => true
}
");

    let layout = read(&source);
    let names: Vec<&str> = layout
        .tables
        .iter()
        .map(|table| table.name.as_str())
        .collect();
    assert_eq!(names, ["day_int", "day_string"]);
}
