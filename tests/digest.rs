//! The digest form: what a program does, with everything else taken away.
//!
//! `docs/digest.md` is the contract, one numbered rule per section, and this is the same
//! rules as executable tests.

use heklang::digest::VERSION;
use heklang::{Digest, Entry, Kind, parse, parse_files};

const EVENTS: &str = "\
subject Customer(Int)
event @order.placed { order_id: Uuid, customer_id: Customer, total: Money(2) }
event @order.cancelled { order_id: Uuid, customer_id: Customer }
";

fn digest(source: &str) -> Digest {
    let program = parse(source).unwrap_or_else(|err| panic!("the source under test checks: {err}"));
    Digest::of(&program)
}

/// Both sources with the shared event declarations in front of them, which nearly every
/// case here needs and none of them is about.
fn with_events(body: &str) -> Digest {
    digest(&format!("{EVENTS}{body}"))
}

fn same(one: &str, two: &str, why: &str) {
    let (one, two) = (with_events(one), with_events(two));
    assert_eq!(one.packed(), two.packed(), "{why}");
    assert_eq!(one.hash(), two.hash(), "{why}, so the hashes agree");
}

fn differs(one: &str, two: &str, why: &str) {
    let (one, two) = (with_events(one), with_events(two));
    assert_ne!(one.hash(), two.hash(), "{why}");
}

/// The entry for one declaration, by name.
fn entry<'a>(digest: &'a Digest, name: &str) -> &'a heklang::Entry {
    digest
        .entries()
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("`{name}` is a declaration of this program"))
}

// ---------------------------------------------------------------------------
// Rule 1: the digest form is what runs
// ---------------------------------------------------------------------------

#[test]
fn a_local_name_is_not_in_the_form() {
    same(
        "command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           let doubled = total + total
           emit @order.placed { order_id, customer_id, total: doubled }
         }",
        "command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           let twice = total + total
           emit @order.placed { order_id, customer_id, total: twice }
         }",
        "a `let` is a slot, and a slot has no name",
    );
}

/// The lift a comparison puts on the bare side is a node, so the form says the value was
/// made into an optional rather than leaving a reader to work it out from the operands.
/// It appears only where the language grew a form it did not have, which is why no hash
/// that existed before it moved.
#[test]
fn a_lifted_comparison_is_in_the_form() {
    const LIFTED: &str =
        "command Place(order_id: Uuid, customer_id: Customer, total: Money(2), note: String?) {
           if note == \"gift\" {
             invalid \"no gifts\"
           }
           emit @order.placed { order_id, customer_id, total }
         }";
    let place = entry(&with_events(LIFTED), "Place").form.packed();
    assert!(
        place.contains("(== $3 (wrap String (str \"gift\")))"),
        "got: {place}"
    );

    differs(
        LIFTED,
        &LIFTED.replace("== \"gift\"", "== none"),
        "a lift and an absence are two different questions",
    );
}

#[test]
fn comments_and_layout_are_not_in_the_form() {
    same(
        "command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total }
         }",
        "// what this does, at length
         command Place(
           order_id: Uuid,
           customer_id: Customer,   // the customer
           total: Money(2),
         ) {

           // and here it goes
           emit @order.placed {
             order_id,
             customer_id,
             total,
           }
         }",
        "trivia never reaches the IR, so it cannot reach the digest",
    );
}

#[test]
fn the_two_spellings_of_a_field_are_one() {
    same(
        "command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total }
         }",
        "command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           emit @order.placed { order_id: order_id, customer_id: customer_id, total: total }
         }",
        "the shorthand builds the same load the long form does",
    );
}

#[test]
fn a_written_decimal_place_is_not_the_value() {
    same(
        "command Fee(order_id: Uuid, customer_id: Customer) {
           emit @order.placed { order_id, customer_id, total: 1000 }
         }",
        "command Fee(order_id: Uuid, customer_id: Customer) {
           emit @order.placed { order_id, customer_id, total: 1000.00 }
         }",
        "both are a hundred thousand units at scale two",
    );
    differs(
        "command Fee(order_id: Uuid, customer_id: Customer) {
           emit @order.placed { order_id, customer_id, total: 1000 }
         }",
        "command Fee(order_id: Uuid, customer_id: Customer) {
           emit @order.placed { order_id, customer_id, total: 1000.01 }
         }",
        "a different amount is a different program",
    );
}

#[test]
fn an_unwritten_headers_argument_and_an_empty_one_are_one() {
    same(
        "effect Ping {
           on @order.placed as e { @key order_id } {
             let response = http.get(\"https://ping.example/\")
             if response.status >= 400 {
               fail \"no\"
             }
           }
         }",
        "effect Ping {
           on @order.placed as e { @key order_id } {
             let response = http.get(\"https://ping.example/\", headers = {})
             if response.status >= 400 {
               fail \"no\"
             }
           }
         }",
        "the headers argument is in the IR either way, empty when unwritten",
    );
}

// ---------------------------------------------------------------------------
// Rule 2: declared names stay, and are repeated rather than indexed
// ---------------------------------------------------------------------------

#[test]
fn a_command_parameter_keeps_its_name_and_a_fn_parameter_does_not() {
    same(
        "fn double(amount: Money(2)) -> Money(2) { return amount + amount }",
        "fn double(value: Money(2)) -> Money(2) { return value + value }",
        "a `fn`'s arguments are positional, so its parameter names are local",
    );
    differs(
        "command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total }
         }",
        "command Place(order: Uuid, customer_id: Customer, total: Money(2)) {
           emit @order.placed { order_id: order, customer_id, total }
         }",
        "a command's parameter names are the request body's keys, so they leave the program",
    );
}

#[test]
fn an_event_path_is_written_out_at_every_use() {
    let digest = with_events(
        "command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total }
         }",
    );
    let uses = digest.packed().matches("@order.placed").count();
    assert!(
        uses >= 2,
        "the declaration and the emit both spell the path; there is no index table"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: a slot is numbered by first appearance
// ---------------------------------------------------------------------------

#[test]
fn slots_are_numbered_from_zero_with_no_gaps() {
    // A guard's slots sit at the end of its caller's frame, so this is the case where
    // raw `Slot` values would be sparse and start above zero.
    let digest = with_events(
        "refusal TooMany \"too many\"

         guard UnderLimit(customer_id: Customer) {
           fold open: Int = 0
             on @order.placed(customer_id) => open + 1
             on @order.cancelled(customer_id) => open - 1

           if open >= 10 {
             reject TooMany
           }
         }

         command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           guard @order.placed(order_id)
           guard UnderLimit { customer_id }
           emit @order.placed { order_id, customer_id, total }
         }",
    );

    let text = entry(&digest, "Place").form.packed();
    let mut seen: Vec<u32> = text
        .split('$')
        .skip(1)
        .filter_map(|rest| {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            digits.parse().ok()
        })
        .collect();
    seen.sort_unstable();
    seen.dedup();
    let expected: Vec<u32> = (0..seen.len() as u32).collect();
    assert_eq!(
        seen, expected,
        "the numbering is by first appearance, so it is dense and starts at zero"
    );
}

// ---------------------------------------------------------------------------
// Rule 4: what the language treats as a set is sorted
// ---------------------------------------------------------------------------

#[test]
fn declaration_order_within_a_file_does_not_matter() {
    same(
        "command A(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total }
         }
         command B(order_id: Uuid, customer_id: Customer) {
           emit @order.cancelled { order_id, customer_id }
         }",
        "command B(order_id: Uuid, customer_id: Customer) {
           emit @order.cancelled { order_id, customer_id }
         }
         command A(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total }
         }",
        "declarations are a set; the entries are sorted by kind and name",
    );
}

#[test]
fn file_boundaries_do_not_matter() {
    const COMMAND: &str = "command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
      emit @order.placed { order_id, customer_id, total }
    }
    ";

    let one = parse_files([("events.hk", EVENTS), ("commands.hk", COMMAND)])
        .expect("both modules are one program");
    let two = parse_files([("b/commands.hk", COMMAND), ("a/events.hk", EVENTS)])
        .expect("the same two, named and ordered differently");

    assert_eq!(
        Digest::of(&one).hash(),
        Digest::of(&two).hash(),
        "a module is a label for a diagnostic, not an identity"
    );
}

#[test]
fn an_enum_default_survives_its_variants_being_reordered() {
    same(
        "enum Tier { @default Free, Paid }",
        "enum Tier { Paid, @default Free }",
        "the variants sort and the default is printed by name, not by index",
    );
    differs(
        "enum Tier { @default Free, Paid }",
        "enum Tier { Free, @default Paid }",
        "which variant is the default is what an absent column falls back to",
    );
}

#[test]
fn an_index_written_two_ways_is_one_index() {
    same(
        "projector P {
           entity E {
             id: Uuid @key,
             owner: Customer @index,
           }
           on @order.placed { order_id, customer_id } {
             put E { id: order_id, owner: customer_id }
           }
         }",
        "projector P {
           entity E {
             id: Uuid @key,
             owner: Customer,

             index (owner),
           }
           on @order.placed { order_id, customer_id } {
             put E { id: order_id, owner: customer_id }
           }
         }",
        "`@index` on a column and an `index` clause build the same index",
    );
}

#[test]
fn swapping_two_statements_is_a_change() {
    differs(
        "command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           let one = total + total
           let two = total
           emit @order.placed { order_id, customer_id, total: one }
         }",
        "command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           let two = total
           let one = total + total
           emit @order.placed { order_id, customer_id, total: one }
         }",
        "a body is a sequence, and the order is what it means",
    );
}

// ---------------------------------------------------------------------------
// Rule 5: a declared field list is sorted, unless a value calls out
// ---------------------------------------------------------------------------

#[test]
fn reordering_the_fields_of_an_emit_changes_nothing() {
    same(
        "command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total }
         }",
        "command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           emit @order.placed { total, customer_id, order_id }
         }",
        "the event declares the fields, so which order they were written in is not observable",
    );
}

#[test]
fn a_body_holding_a_call_keeps_the_order_it_was_written_in() {
    const ONE: &str = "effect Ship {
      fn ping(url: String) -> Int {
        let response = http.get(url)
        return response.status
      }

      on @order.placed as e { @key order_id } {
        let sent = http.post(\"https://ship.example/\", {
          \"b\": ping(\"https://b.example/\"),
          \"a\": ping(\"https://a.example/\"),
        })
        if sent.status >= 400 {
          fail \"no\"
        }
      }
    }
    ";
    const TWO: &str = "effect Ship {
      fn ping(url: String) -> Int {
        let response = http.get(url)
        return response.status
      }

      on @order.placed as e { @key order_id } {
        let sent = http.post(\"https://ship.example/\", {
          \"a\": ping(\"https://a.example/\"),
          \"b\": ping(\"https://b.example/\"),
        })
        if sent.status >= 400 {
          fail \"no\"
        }
      }
    }
    ";

    let one = with_events(ONE);
    assert!(
        one.packed().find("(f \"b\" ") < one.packed().find("(f \"a\" "),
        "two calls written as sibling values are ordered, so the list is left alone"
    );
    differs(
        ONE,
        TWO,
        "swapping two calls swaps what the journal records",
    );
}

#[test]
fn a_json_key_is_quoted_because_it_is_not_an_identifier() {
    let digest = with_events(
        "effect Ship {
           on @order.placed as e { @key order_id } {
             let sent = http.post(\"https://ship.example/\", { \"a-b\": 1 })
             if sent.status >= 400 {
               fail \"no\"
             }
           }
         }",
    );
    assert!(
        digest.packed().contains("(f \"a-b\" "),
        "a JSON key is arbitrary text, so it is quoted where a field name is not"
    );
}

// ---------------------------------------------------------------------------
// Rule 6: `const`, `refusal` and `guard` are not in it
// ---------------------------------------------------------------------------

#[test]
fn a_const_is_its_value_and_not_its_name() {
    same(
        "const LIMIT: Money(2) = 5.00
         command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           if total >= LIMIT { return }
           emit @order.placed { order_id, customer_id, total }
         }",
        "const CAP: Money(2) = 5.00
         command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           if total >= CAP { return }
           emit @order.placed { order_id, customer_id, total }
         }",
        "a const is inlined, so its name never reaches the IR",
    );
    differs(
        "const LIMIT: Money(2) = 5.00
         command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           if total >= LIMIT { return }
           emit @order.placed { order_id, customer_id, total }
         }",
        "const LIMIT: Money(2) = 6.00
         command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           if total >= LIMIT { return }
           emit @order.placed { order_id, customer_id, total }
         }",
        "its value is what runs, and it runs at the use site",
    );
}

#[test]
fn a_refusal_message_reaches_every_reject() {
    let before = with_events(
        "refusal Nope \"not this time\"
         command A(order_id: Uuid, customer_id: Customer) { reject Nope }
         command B(order_id: Uuid, customer_id: Customer) { reject Nope }",
    );
    let after = with_events(
        "refusal Nope \"not today\"
         command A(order_id: Uuid, customer_id: Customer) { reject Nope }
         command B(order_id: Uuid, customer_id: Customer) { reject Nope }",
    );

    assert_ne!(
        entry(&before, "A").hash,
        entry(&after, "A").hash,
        "the message is copied into every use site, so both callers changed"
    );
    assert_ne!(entry(&before, "B").hash, entry(&after, "B").hash);
    assert!(
        entry(&before, "A")
            .form
            .packed()
            .contains("\"not this time\""),
        "the message is at the reject rather than in a declaration of its own"
    );
}

#[test]
fn an_unused_refusal_is_not_in_the_form() {
    same(
        "command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total }
         }",
        "refusal NeverSaid \"nothing rejects this\"
         command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total }
         }",
        "a refusal nothing names runs nowhere",
    );
}

#[test]
fn a_guard_is_printed_where_it_runs() {
    let digest = with_events(
        "refusal TooMany \"too many\"

         guard UnderLimit(customer_id: Customer) {
           fold open: Int = 0
             on @order.cancelled(customer_id) => open + 1

           if open >= 10 {
             reject TooMany
           }
         }

         command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           guard @order.placed(order_id)
           guard UnderLimit { customer_id }
           emit @order.placed { order_id, customer_id, total }
         }",
    );

    let place = entry(&digest, "Place").form.packed();
    assert!(
        place.contains("slice @order.cancelled"),
        "the guard's own slice is inside the command that names it"
    );
    assert!(
        place.contains("\"too_many\""),
        "and so is the refusal it decides"
    );
    assert!(
        digest
            .entries()
            .iter()
            .all(|entry| entry.name != "UnderLimit"),
        "a guard has no entry of its own; it would be the same body counted twice"
    );
}

// ---------------------------------------------------------------------------
// Rule 7: every entry carries its own hash
// ---------------------------------------------------------------------------

#[test]
fn every_entry_carries_its_own_kind_name_and_hash() {
    let digest = with_events(
        "command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total }
         }",
    );

    let kinds: Vec<Kind> = digest.entries().iter().map(|entry| entry.kind).collect();
    assert_eq!(
        kinds,
        vec![Kind::Subject, Kind::Event, Kind::Event, Kind::Command],
        "entries are sorted by kind first, in the order the kinds are declared"
    );
    let place = entry(&digest, "Place");
    assert_eq!(place.kind, Kind::Command);
    assert!(place.form.packed().starts_with("(command Place "));
    assert_ne!(
        place.hash,
        entry(&digest, "@order.placed").hash,
        "two declarations are two hashes"
    );
}

#[test]
fn changing_one_command_leaves_every_other_entry_alone() {
    let before = with_events(
        "command A(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total }
         }
         command B(order_id: Uuid, customer_id: Customer) {
           emit @order.cancelled { order_id, customer_id }
         }",
    );
    let after = with_events(
        "command A(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total: total + total }
         }
         command B(order_id: Uuid, customer_id: Customer) {
           emit @order.cancelled { order_id, customer_id }
         }",
    );

    assert_ne!(entry(&before, "A").hash, entry(&after, "A").hash);
    assert_eq!(
        entry(&before, "B").hash,
        entry(&after, "B").hash,
        "which declarations changed is a comparison of two lists, not a diff"
    );
    assert_ne!(before.hash(), after.hash());
}

// ---------------------------------------------------------------------------
// Rule 8: tests are a section of their own
// ---------------------------------------------------------------------------

#[test]
fn a_test_is_not_in_the_program_hash() {
    let without = with_events(
        "command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total }
         }",
    );
    let with = with_events(
        "command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total }
         }

         test \"an order is placed\" {
           run Place { order_id: \"0190d1a1-0000-7000-8000-000000000001\", customer_id: 1, total: 25.99 }
           expect @order.placed { order_id: \"0190d1a1-0000-7000-8000-000000000001\", customer_id: 1, total: 25.99 }
         }",
    );

    assert_eq!(
        without.hash(),
        with.hash(),
        "a `test` runs nothing in production, so it is not in the program's hash"
    );
    assert_ne!(
        without.hash_with_tests(),
        with.hash_with_tests(),
        "but writing one is still a change"
    );
    assert_eq!(with.tests().len(), 1);
    assert_eq!(with.tests()[0].kind, Kind::Test);

    // Every form makes the same cut, so a hash in a document always covers content that
    // document carries.
    let plain = with.json().to_string();
    assert!(!plain.contains("\"tests\""), "{plain}");
    assert!(!plain.contains("hash_with_tests"), "{plain}");
    let all = with.json_with_tests().to_string();
    assert!(all.contains("\"tests\""), "{all}");
    assert!(all.contains("hash_with_tests"), "{all}");
}

// ---------------------------------------------------------------------------
// Rule 9: the version line is part of the hash
// ---------------------------------------------------------------------------

#[test]
fn the_version_line_opens_the_form() {
    let digest = with_events(
        "command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total }
         }",
    );

    assert_eq!(digest.packed().lines().next(), Some(VERSION));
    assert_eq!(digest.packed_with_tests().lines().next(), Some(VERSION));
    assert!(
        digest.packed().ends_with('\n'),
        "the form is exactly the bytes the hash covers, newline and all"
    );
}

// ---------------------------------------------------------------------------
// Determinism
// ---------------------------------------------------------------------------

#[test]
fn one_program_digests_the_same_bytes_twice() {
    let source = format!(
        "{EVENTS}command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {{
           emit @order.placed {{ order_id, customer_id, total }}
         }}"
    );
    let program = parse(&source).expect("the source checks");

    assert_eq!(
        Digest::of(&program).packed(),
        Digest::of(&program).packed(),
        "nothing here reads a hash map's order or an arena's"
    );
}

/// The other half of rule 4, in one place: a change to a declaration is a change to the
/// program, whichever kind of declaration it is.
#[test]
fn a_change_to_any_declaration_reaches_the_hash() {
    let field = |extra: &str| {
        format!(
            "subject Customer(Int)\nevent @order.placed {{ order_id: Uuid, customer_id: Customer{extra} }}"
        )
    };
    assert_ne!(
        digest(&field("")).hash(),
        digest(&field(", note: String @max(20)")).hash(),
        "an event gained a field"
    );

    let bound = |max: u32| format!("record Line {{ title: String @max({max}) }}");
    assert_ne!(
        digest(&bound(20)).hash(),
        digest(&bound(21)).hash(),
        "a `@max` is checked wherever the record lands"
    );

    differs(
        "command Count(order_id: Uuid, customer_id: Customer) {
           fold seen: Int = 0
             on @order.placed(customer_id) => seen + 1
           if seen > 0 { return }
           emit @order.cancelled { order_id, customer_id }
         }",
        "command Count(order_id: Uuid, customer_id: Customer) {
           fold seen: Int = 1
             on @order.placed(customer_id) => seen + 1
           if seen > 0 { return }
           emit @order.cancelled { order_id, customer_id }
         }",
        "a fold's seed is where it starts",
    );

    differs(
        "projector P {
           entity E { id: Uuid @key, owner: Customer }
           on @order.placed { order_id, customer_id } { put E { id: order_id, owner: customer_id } }
         }",
        "projector P {
           entity E { id: Uuid @key, owner: Customer @index }
           on @order.placed { order_id, customer_id } { put E { id: order_id, owner: customer_id } }
         }",
        "an index is a read path the projector now keeps",
    );

    differs(
        "projector P {
           entity E { id: Uuid @key, seen: Int }
           on @order.placed { order_id } { patch E[order_id] { seen: .seen + 1 } }
         }",
        "projector P {
           entity E { id: Uuid @key, seen: Int }
           on @order.placed { order_id } { update E[order_id] { seen: .seen + 1 } }
         }",
        "`patch` materialises an absent row and `update` drops the write",
    );
}

/// Rule 4 as a property rather than a pair: every ordering of the same modules is the same
/// program, so every ordering is the same hash. `tests/modules.rs` states the parsing half.
#[test]
fn no_ordering_of_the_same_modules_changes_the_hash() {
    const COMMAND: &str = "command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
      emit @order.placed { order_id, customer_id, total }
    }
    ";
    const PROJECTOR: &str = "projector Orders {
      entity Order { order_id: Uuid @key, total: Money(2) }
      on @order.placed { order_id, total } { put Order { order_id, total } }
    }
    ";

    let modules = [
        ("events.hk", EVENTS),
        ("commands.hk", COMMAND),
        ("projectors.hk", PROJECTOR),
    ];
    let orders = [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ];

    let first = Digest::of(&parse_files(modules).expect("one program")).hash();
    for order in orders {
        let files = order.map(|index| modules[index]);
        let program = parse_files(files).expect("the same program, read in another order");
        assert_eq!(
            Digest::of(&program).hash(),
            first,
            "module order is not part of a program: {order:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the packed form is canonical, and the views are taken from it
// ---------------------------------------------------------------------------

/// Every `.hk` file of the demo program, which is the largest program that exists and the
/// one the round trip has to survive.
fn demo() -> Vec<(String, String)> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("hek");
    let mut files: Vec<(String, String)> = std::fs::read_dir(&root)
        .expect("the demo program is beside the crate")
        .filter_map(|entry| {
            let path = entry.expect("a directory entry").path();
            if path.extension()? != "hk" {
                return None;
            }
            Some((
                path.file_name()?.to_string_lossy().into_owned(),
                std::fs::read_to_string(&path).ok()?,
            ))
        })
        .collect();
    files.sort();
    files
}

fn demo_digest() -> Digest {
    let files = demo();
    let borrowed: Vec<(&str, &str)> = files
        .iter()
        .map(|(name, body)| (name.as_str(), body.as_str()))
        .collect();
    Digest::of(&parse_files(borrowed).expect("the demo program checks"))
}

#[test]
fn a_packed_form_reads_back_as_the_same_digest() {
    let digest = demo_digest();
    let read = Digest::from_packed(&digest.packed()).expect("its own output parses");
    assert_eq!(
        read.entries(),
        digest.entries(),
        "a stored form is the digest it came from, signatures and all"
    );
    assert_eq!(read.hash(), digest.hash());
    assert!(
        read.tests().is_empty(),
        "`packed` leaves the tests out, so reading it back finds none"
    );

    let all = Digest::from_packed(&digest.packed_with_tests()).expect("with the tests too");
    assert_eq!(all, digest, "and everything round trips");
    assert_eq!(all.hash_with_tests(), digest.hash_with_tests());
}

#[test]
fn an_entry_reads_back_from_its_row() {
    for entry in demo_digest().entries() {
        let signature = entry.signature.as_ref().map(|sexp| sexp.packed());
        let read = Entry::from_packed(&entry.form.packed(), signature.as_deref())
            .unwrap_or_else(|err| panic!("`{}` reads back: {err}", entry.name));
        assert_eq!(
            &read, entry,
            "`{}` is the row it was written as",
            entry.name
        );
    }
}

#[test]
fn the_views_come_from_the_packed_form() {
    let digest = demo_digest();
    let read = Digest::from_packed(&digest.packed()).expect("its own output parses");

    // Nothing in either view may come from the IR, or a caller holding only a stored row
    // would get a different answer from one holding the source.
    assert_eq!(read.expanded(), digest.expanded());
    assert_eq!(read.json().to_string(), digest.json().to_string());
}

#[test]
fn a_form_that_was_damaged_fails_loudly() {
    // A truncated row must not decode into a plausible wrong answer: what reads it next is
    // deciding whether a deployment is a breaking change.
    assert!(Digest::from_packed("hek-digest 2\n(event @order.placed (f x Int)").is_err());
    assert!(Entry::from_packed("(event @a.b (f x Int))", Some("(sig")).is_err());
    assert!(Entry::from_packed("(nonsense Thing)", None).is_err());
    assert!(Entry::from_packed("(event @a.b) (event @c.d)", None).is_err());
}

// ---------------------------------------------------------------------------
// Rule 8: the signature is what is visible outside the program
// ---------------------------------------------------------------------------

fn signature(digest: &Digest, name: &str) -> String {
    entry(digest, name)
        .signature
        .as_ref()
        .unwrap_or_else(|| panic!("`{name}` has a signature"))
        .packed()
}

#[test]
fn a_body_change_leaves_the_signature_alone() {
    let before = with_events(
        "command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total }
         }",
    );
    let after = with_events(
        "command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total: total + total }
         }",
    );

    assert_ne!(
        entry(&before, "Place").hash,
        entry(&after, "Place").hash,
        "what it does changed"
    );
    assert_eq!(
        entry(&before, "Place").signature_hash,
        entry(&after, "Place").signature_hash,
        "and nothing outside the program can tell"
    );
}

#[test]
fn a_signature_moves_when_something_outside_could_notice() {
    let signatures = |body: &str| {
        let digest = with_events(body);
        digest
            .entries()
            .iter()
            .filter_map(|entry| entry.signature_hash.map(|hash| (entry.name.clone(), hash)))
            .collect::<Vec<_>>()
    };

    let base = "command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
      emit @order.placed { order_id, customer_id, total }
    }";
    let retyped = "command Place(order_id: Uuid, customer_id: Customer, total: Money(3)) {
      emit @order.placed { order_id, customer_id, total: 0.00 }
    }";
    assert_ne!(
        signatures(base),
        signatures(retyped),
        "a parameter's type is the request body's shape"
    );

    let renamed = "command Place(order_id: Uuid, customer: Customer, total: Money(2)) {
      emit @order.placed { order_id, customer_id: customer, total }
    }";
    assert_ne!(
        signatures(base),
        signatures(renamed),
        "a parameter's name is one of the request body's keys"
    );
}

#[test]
fn a_command_signature_names_the_codes_it_can_answer_with() {
    let digest = with_events(
        "refusal TooMany \"too many\"
         command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           fold open: Int = 0
             on @order.placed(customer_id) => open + 1
           if open >= 10 {
             reject TooMany
           }
           emit @order.placed { order_id, customer_id, total }
         }",
    );

    assert!(
        signature(&digest, "Place").contains("(rejects too_many)"),
        "a refusal has no entry of its own, so the signature goes and gets the code: {}",
        signature(&digest, "Place")
    );
    // And the body it came from is not in the signature, because a body cannot break a
    // caller.
    assert!(!signature(&digest, "Place").contains("fold"));
}

#[test]
fn a_refusal_decided_in_a_fn_still_reaches_the_signature() {
    let digest = with_events(
        "refusal TooMany \"too many\"

         fn objection(open: Int) -> Outcome? {
           if open >= 10 {
             reject TooMany
           }
           return none
         }

         command Place(order_id: Uuid, customer_id: Customer, total: Money(2)) {
           fold open: Int = 0
             on @order.placed(customer_id) => open + 1

           let refused = objection(open)
           if refused.is_some() {
             return refused
           }
           emit @order.placed { order_id, customer_id, total }
         }",
    );

    assert!(
        signature(&digest, "Place").contains("(rejects too_many)"),
        "a `fn` may decide a refusal on a command's behalf, so the walk follows the call: {}",
        signature(&digest, "Place")
    );
}

#[test]
fn a_fn_and_a_test_have_no_signature() {
    let digest = with_events(
        "fn double(amount: Money(2)) -> Money(2) { return amount + amount }

         test \"nothing at all\" {
           project P
         }
         projector P {
           entity E { order_id: Uuid @key }
           on @order.placed { order_id } { put E { order_id } }
         }",
    );

    assert!(entry(&digest, "double").signature.is_none());
    assert!(entry(&digest, "double").signature_hash.is_none());
    assert!(digest.tests()[0].signature.is_none());
    assert!(
        entry(&digest, "P").signature.is_some(),
        "a projector's entities are the read API and do have one"
    );
}

// ---------------------------------------------------------------------------
// Rule 15 of `docs/effects.md`: delivery and the partition key
// ---------------------------------------------------------------------------

#[test]
fn a_delivery_modifier_is_what_a_program_does() {
    differs(
        "effect E {
           on @order.placed as e { @key customer_id } { log(\"x\") }
         }",
        "effect E {
           on latest @order.placed as e { @key customer_id } { log(\"x\") }
         }",
        "collapsing changes how many invocations there are",
    );
    differs(
        "effect E {
           on @order.placed as e { @key customer_id } { log(\"x\") }
         }",
        "effect E {
           on live @order.placed as e { @key customer_id } { log(\"x\") }
         }",
        "declining history changes which invocations there are",
    );
}

#[test]
fn a_partition_key_is_what_a_program_does() {
    differs(
        "effect E {
           on @order.placed as e { @key customer_id, order_id } { log(\"x\") }
         }",
        "effect E {
           on @order.placed as e { @key customer_id, @key order_id } { log(\"x\") }
         }",
        "a second key is a narrower lane",
    );
}

/// A composite is a sequence, not a set: rule 4's sorting stops at the arm's key.
#[test]
fn reordering_a_composite_key_is_a_change() {
    differs(
        "effect E {
           on @order.placed as e { @key customer_id, @key order_id } { log(\"x\") }
         }",
        "effect E {
           on @order.placed as e { @key order_id, @key customer_id } { log(\"x\") }
         }",
        "`{ @key a, @key b }` and `{ @key b, @key a }` are different lanes",
    );
}

/// The rest of the arm is unchanged by any of this: what a local is called is still not
/// in the form.
#[test]
fn a_key_does_not_make_local_names_matter() {
    same(
        "effect E {
           on latest @order.placed as e { @key customer_id } {
             let who = customer_id
             log(\"{who}\")
           }
         }",
        "effect E {
           on latest @order.placed as e { @key customer_id } {
             let whom = customer_id
             log(\"{whom}\")
           }
         }",
        "a `let` is a slot, and a slot has no name",
    );
}

/// The signature carries what a deployment has to know about how events reach an effect,
/// and nothing else from the arm: not the `as` binding, not the fields it destructures
/// for its own use.
#[test]
fn an_effect_signature_names_its_arms_delivery_and_keys() {
    let digest = with_events(
        "effect E {
           on latest @order.placed, @order.cancelled as e { @key customer_id, order_id } {
             log(\"x\")
           }
         }",
    );
    assert_eq!(
        signature(&digest, "E"),
        "(sig effect E (on (events @order.cancelled @order.placed) (delivery latest) (key customer_id)))"
    );
}

#[test]
fn an_effect_signature_moves_when_its_lanes_do() {
    let signature_of = |body: &str| entry(&with_events(body), "E").signature_hash;
    let base = "effect E {
      on @order.placed as e { @key customer_id } { log(\"x\") }
    }";
    assert_ne!(
        signature_of(base),
        signature_of(
            "effect E {
               on latest @order.placed as e { @key customer_id } { log(\"x\") }
             }"
        ),
        "a catchup policy is a deployment's business"
    );
    assert_ne!(
        signature_of(base),
        signature_of(
            "effect E {
               on @order.placed as e { @key order_id } { log(\"x\") }
             }"
        ),
        "so is which lane an event lands in"
    );
    assert_eq!(
        signature_of(base),
        signature_of(
            "effect E {
               on @order.placed as e { @key customer_id } { log(\"y\") }
             }"
        ),
        "and the body is still the effect's own business"
    );
}

/// Rule 7, met by a fourth declaration kind: a `secret` runs nothing on its own, so it
/// has no entry. Its **name** is at every read site instead, which is what buys the two
/// properties a deploy gate needs from it.
#[test]
fn a_secret_has_no_entry_and_its_name_is_at_the_read_site() {
    let form = with_events(
        "secret DISCORD_WEBHOOK
effect Alerts {
  on @order.placed as e { @key order_id } {
    let a = http.post(DISCORD_WEBHOOK, { \"id\": e.order_id })
  }
}",
    );
    assert!(
        !form.packed().contains("(secret DISCORD_WEBHOOK)\n"),
        "a declaration that runs nothing is not an entry, the same reason a `const` is not"
    );
    assert!(
        entry(&form, "Alerts")
            .form
            .packed()
            .contains("(secret DISCORD_WEBHOOK)"),
        "the name is at the read site: {}",
        entry(&form, "Alerts").form.packed()
    );

    // Reading a *different* credential is a behaviour change and moves the hash.
    differs(
        "secret A
secret B
effect E { on @order.placed as e { @key order_id } { let r = http.get(A) } }",
        "secret A
secret B
effect E { on @order.placed as e { @key order_id } { let r = http.get(B) } }",
        "an effect that starts reading a different credential decides differently",
    );

    // And renaming or moving the declaration is not, exactly as for a `const`.
    same(
        "secret A
effect E { on @order.placed as e { @key order_id } { log(\"x\") } }",
        "effect E { on @order.placed as e { @key order_id } { log(\"x\") } }",
        "an unread secret is invisible, the same as an unused const",
    );
}

/// The load-bearing half, and the reason a credential is not a `const`: `digest_hash`
/// is what a runtime records as an invocation's `script_hash`, so a hash that moved on
/// a rotation would cost replay coverage on every past invocation. A `const` holding the
/// same URL puts its text in the form and does exactly that.
#[test]
fn a_secret_holds_no_value_so_a_rotation_cannot_move_a_hash() {
    let packed = with_events(
        "secret HOOK
effect E { on @order.placed as e { @key order_id } { let r = http.get(HOOK) } }",
    )
    .packed();
    assert!(
        !packed.contains("https"),
        "there is no value in the program to put in the form"
    );

    // The contrast, spelled out: the same address as a `const` is inlined at the use
    // site, so editing it moves the effect's hash.
    differs(
        "const HOOK: String = \"https://one.example/hook\"
effect E { on @order.placed as e { @key order_id } { let r = http.get(HOOK) } }",
        "const HOOK: String = \"https://two.example/hook\"
effect E { on @order.placed as e { @key order_id } { let r = http.get(HOOK) } }",
        "a const's value is in the form, which is what a rotation must not be",
    );
}

/// The two properties the read-site atom has to carry, and the one thing it must not.
#[test]
fn a_secret_in_the_form_carries_its_shape_and_never_a_value() {
    // `secret X` and `secret X?` behave differently at a read site -- one wedges when a
    // deployment cannot answer, the other branches -- so they must not hash alike.
    // A bare read into a `let` takes either, so the two programs differ in the `?` and
    // nothing else. (Every position that *uses* the value rejects an un-narrowed
    // optional, which is the point of carrying the flag.)
    differs(
        "secret A
effect E { on @order.placed as e { @key order_id } { let a = A } }",
        "secret A?
effect E { on @order.placed as e { @key order_id } { let a = A } }",
        "the `?` is part of what runs, so a deploy gate must see the edit",
    );

    // A test's own setup line reports that it set one, never what it set. The digest is
    // a published artifact and `SecretDef` holds no value precisely so that no path puts
    // a credential into it.
    let form = digest(&format!(
        "{EVENTS}secret HOOK
effect E {{ on @order.placed as e {{ @key order_id }} {{ let r = http.get(HOOK) }} }}
test \"one\" {{
  given @order.placed {{ order_id: \"0190d1a1-0000-7000-8000-000000000001\", customer_id: 1, total: 1.00 }}
  secret HOOK = \"sk_live_TOPSECRET\"
  deliver E
  expect nothing
}}"
    ));
    let packed = form.packed_with_tests();
    assert!(
        !packed.contains("sk_live_TOPSECRET"),
        "a test's value must not reach the form: {packed}"
    );
    assert!(packed.contains("(secret HOOK set)"), "{packed}");
}

// ---------------------------------------------------------------------------
// Rule 8, continued: `@absent` is part of an event's shape
// ---------------------------------------------------------------------------

/// It renders as a child node holding a literal, the way an entity column's default
/// does, and the literal is the resolved one rather than the text that was written.
#[test]
fn an_absent_value_is_part_of_a_field_form() {
    let packed = digest(
        "event @order.placed {
           order_id: Uuid,
           note: String @absent(\"\"),
           discount: Money(2) @absent(0.00) @no_index,
         }",
    )
    .packed();

    assert!(
        packed.contains("(f note String (absent (str \"\")))"),
        "the field carries its absent value: {packed}"
    );
    assert!(
        packed.contains("(f discount (Money 2) (absent (money 2 0)) no_index)"),
        "beside the other annotations, and holding the resolved literal: {packed}"
    );
}

/// A record reached from an event is stored inside that event's payload, so its fields
/// carry the same node for the same reason.
#[test]
fn a_record_field_carries_its_absent_value_too() {
    let packed = digest(
        "record Note { kind: String, body: String @max(20) @absent(\"none given\") }
         event @order.placed { order_id: Uuid, detail: Note }",
    )
    .packed();

    assert!(
        packed.contains("(f body String (max 20) (absent (str \"none given\")))"),
        "a record field carries it: {packed}"
    );
}

/// What a reader gets out of a payload that predates the field is part of the shape, not
/// of a body: changing it changes every historical event the declaration describes, and a
/// deployment has to be told.
#[test]
fn an_absent_value_moves_the_signature_as_well_as_the_hash() {
    let event = |absent: &str| {
        digest(&format!(
            "event @order.placed {{ order_id: Uuid, note: String{absent} }}"
        ))
    };
    let plain = event("");
    let empty = event(" @absent(\"\")");
    let dash = event(" @absent(\"-\")");

    for (one, two, why) in [
        (&plain, &empty, "gaining an absent value"),
        (&empty, &dash, "changing one"),
    ] {
        assert_ne!(
            entry(one, "@order.placed").hash,
            entry(two, "@order.placed").hash,
            "{why} changes what the declaration does"
        );
        assert_ne!(
            entry(one, "@order.placed").signature_hash,
            entry(two, "@order.placed").signature_hash,
            "{why} is visible to every reader of the log"
        );
    }
}

/// The rollout property, and the reason the node is written only when it is there: a
/// declaration that does not use the annotation has to hash exactly as it did before the
/// annotation existed, or every event in every project would read as changed the first
/// time this version is deployed.
#[test]
fn a_field_without_an_absent_value_renders_as_it_always_did() {
    let packed = digest(
        "record Line { title: String @max(20) }
         event @order.placed { order_id: Uuid, total: Money(2), note: String @no_index }",
    )
    .packed();

    assert!(
        !packed.contains("absent"),
        "nothing to say, nothing said: {packed}"
    );
    assert!(packed.contains("(f note String no_index)"), "{packed}");
    assert!(packed.contains("(f title String (max 20))"), "{packed}");
    assert!(packed.contains("(f total (Money 2))"), "{packed}");

    // This used to pin `VERSION` at the number it had when `@absent` landed, as a
    // marker that an optional node is not a new digest version. The number has since
    // moved for an unrelated reason (a comprehension now carries its element type, so
    // the form it renders did change), which is what the version line is for. The
    // claim above is the real one, and the assertions above it are what hold it.
}

/// `absent` names a part of its parent rather than a value in it, so the JSON form has to
/// read it as a structure head like `max` and `default`.
#[test]
fn an_absent_node_is_a_structure_head_in_the_json_form() {
    let digest = digest("event @order.placed { order_id: Uuid, note: String @absent(\"x\") }");
    let json = entry(&digest, "@order.placed").form.json().to_string();
    assert!(
        json.contains("\"absent\""),
        "the head is a key rather than a positional value: {json}"
    );
}

/// A host holds a recorded declaration as packed text and has to ask which fields it
/// had, so the shape of an `(f ..)` node is read back here rather than picked apart by
/// whoever stored it.
#[test]
fn a_stored_event_form_says_which_fields_it_declared() {
    let made = digest(
        r#"
record Note { kind: String, weight: Int }

event @order.placed {
  order_id: Uuid,
  total: Int,
  detail: Note,
}

command Place(order_id: Uuid) {
  emit @order.placed { order_id, total: 1, detail: Note { kind: "gift", weight: 2 } }
}
"#,
    );

    let event = entry(&made, "@order.placed");
    assert_eq!(event.field_names(), ["detail", "order_id", "total"]);

    let record = entry(&made, "Note");
    assert_eq!(record.field_names(), ["kind", "weight"]);

    // Read back through the packed form, which is the way a host actually has it.
    let packed = Entry::from_packed(&event.form.packed(), None).unwrap();
    assert_eq!(packed.field_names(), ["detail", "order_id", "total"]);

    // Nothing else has fields in this sense, and answering with a guess would be worse
    // than answering with nothing.
    assert!(entry(&made, "Place").field_names().is_empty());
}

// ---------------------------------------------------------------------------
// An event a `fn` makes. The node it adds is written only when it is present, which
// is what keeps every hash already stored valid: `docs/digest.md` section 11.

const FIXTURE: &str = "\
fn t_order(order_id: Uuid) -> @order.placed {
  return @order.placed { order_id: order_id, customer_id: 1, total: 25.99 }
}
";

/// The version line did not move for this, so a `given` that spells its event out has
/// to render exactly as it did before fixtures existed. `@absent` is the precedent.
#[test]
fn a_given_without_a_fixture_renders_as_it_always_did() {
    let longhand = with_events(
        "test \"placed\" {
           given @order.placed { order_id: \"0190d1a1-0000-7000-8000-000000000001\", customer_id: 1, total: 25.99 }
           project P
         }
         projector P {
           entity Row { order_id: Uuid @key }
           on @order.placed { order_id } { put Row { order_id } }
         }",
    );
    let form = longhand.packed_with_tests();
    assert!(
        form.contains("(given @order.placed (f customer_id"),
        "got: {form}"
    );
    assert!(!form.contains("(from "), "got: {form}");
}

/// A fixture is an ordinary `fn` call in the form, so the two spellings are two
/// programs. The digest is syntactic, which is the same answer it gives for `address(12)`
/// against the record written out.
#[test]
fn a_fixture_and_a_longhand_given_are_different_forms() {
    let body = "projector P {
                  entity Row { order_id: Uuid @key }
                  on @order.placed { order_id } { put Row { order_id } }
                }";
    let longhand = with_events(&format!(
        "{FIXTURE}{body}
         test \"placed\" {{
           given @order.placed {{ order_id: \"0190d1a1-0000-7000-8000-000000000001\", customer_id: 1, total: 25.99 }}
           project P
         }}"
    ));
    let fixture = with_events(&format!(
        "{FIXTURE}{body}
         test \"placed\" {{
           given t_order(\"0190d1a1-0000-7000-8000-000000000001\")
           project P
         }}"
    ));
    assert_ne!(
        longhand.hash_with_tests(),
        fixture.hash_with_tests(),
        "a call and the fields it stands for are two spellings"
    );
    assert!(
        fixture
            .packed_with_tests()
            .contains("(given @order.placed (from (fn t_order"),
        "got: {}",
        fixture.packed_with_tests()
    );
}

/// The `fn` itself is a module declaration, so declaring one moves the program hash
/// even though only tests call it. That is already true of any helper a test shares,
/// and section 10's claim is about the `test` declarations rather than about everything
/// a test reaches.
#[test]
fn a_fixture_fn_is_in_the_program_hash() {
    let without = with_events("command Noop(order_id: Uuid) { return }");
    let with = with_events(&format!(
        "{FIXTURE}command Noop(order_id: Uuid) {{ return }}"
    ));
    assert_ne!(
        without.hash(),
        with.hash(),
        "a `fn` is a declaration whatever calls it"
    );
    assert!(
        entry(&with, "t_order")
            .form
            .packed()
            .contains("(returns (Event @order.placed))"),
        "got: {}",
        entry(&with, "t_order").form.packed()
    );
}

/// An override is the part of the form a case is about, so changing one is a change to
/// that test and to nothing else.
#[test]
fn an_override_moves_only_the_tests_hash() {
    let body = "projector P {
                  entity Row { order_id: Uuid @key }
                  on @order.placed { order_id } { put Row { order_id } }
                }";
    let one = with_events(&format!(
        "{FIXTURE}{body}
         test \"placed\" {{
           given t_order(\"0190d1a1-0000-7000-8000-000000000001\") {{ total: 1.00 }}
           project P
         }}"
    ));
    let two = with_events(&format!(
        "{FIXTURE}{body}
         test \"placed\" {{
           given t_order(\"0190d1a1-0000-7000-8000-000000000001\") {{ total: 2.00 }}
           project P
         }}"
    ));
    assert_eq!(one.hash(), two.hash(), "nothing in production moved");
    assert_ne!(one.hash_with_tests(), two.hash_with_tests());
}
