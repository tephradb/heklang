//! The digest form: what a program does, with everything else taken away.
//!
//! `docs/digest.md` is the contract, one numbered rule per section, and this is the same
//! rules as executable tests.

use heklang::digest::VERSION;
use heklang::{Digest, Entry, Kind, parse, parse_files};

const EVENTS: &str = "\
event @order.placed { order_id: Uuid, customer_id: Int, total: Money(2) }
event @order.cancelled { order_id: Uuid, customer_id: Int }
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
        "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
           let doubled = total + total
           emit @order.placed { order_id, customer_id, total: doubled }
         }",
        "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
           let twice = total + total
           emit @order.placed { order_id, customer_id, total: twice }
         }",
        "a `let` is a slot, and a slot has no name",
    );
}

#[test]
fn comments_and_layout_are_not_in_the_form() {
    same(
        "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total }
         }",
        "// what this does, at length
         command Place(
           order_id: Uuid,
           customer_id: Int,   // the customer
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
        "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total }
         }",
        "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
           emit @order.placed { order_id: order_id, customer_id: customer_id, total: total }
         }",
        "the shorthand builds the same load the long form does",
    );
}

#[test]
fn a_written_decimal_place_is_not_the_value() {
    same(
        "command Fee(order_id: Uuid, customer_id: Int) {
           emit @order.placed { order_id, customer_id, total: 1000 }
         }",
        "command Fee(order_id: Uuid, customer_id: Int) {
           emit @order.placed { order_id, customer_id, total: 1000.00 }
         }",
        "both are a hundred thousand units at scale two",
    );
    differs(
        "command Fee(order_id: Uuid, customer_id: Int) {
           emit @order.placed { order_id, customer_id, total: 1000 }
         }",
        "command Fee(order_id: Uuid, customer_id: Int) {
           emit @order.placed { order_id, customer_id, total: 1000.01 }
         }",
        "a different amount is a different program",
    );
}

#[test]
fn an_unwritten_headers_argument_and_an_empty_one_are_one() {
    same(
        "effect Ping {
           on @order.placed as e {
             let response = http.get(\"https://ping.example/\")
             if response.status >= 400 {
               fail(\"no\")
             }
           }
         }",
        "effect Ping {
           on @order.placed as e {
             let response = http.get(\"https://ping.example/\", headers = {})
             if response.status >= 400 {
               fail(\"no\")
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
        "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total }
         }",
        "command Place(order: Uuid, customer_id: Int, total: Money(2)) {
           emit @order.placed { order_id: order, customer_id, total }
         }",
        "a command's parameter names are the request body's keys, so they leave the program",
    );
}

#[test]
fn an_event_path_is_written_out_at_every_use() {
    let digest = with_events(
        "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
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

         guard UnderLimit(customer_id: Int) {
           fold open: Int = 0
             on @order.placed(customer_id) => open + 1
             on @order.cancelled(customer_id) => open - 1

           if open >= 10 {
             return reject TooMany
           }
         }

         command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
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
        "command A(order_id: Uuid, customer_id: Int, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total }
         }
         command B(order_id: Uuid, customer_id: Int) {
           emit @order.cancelled { order_id, customer_id }
         }",
        "command B(order_id: Uuid, customer_id: Int) {
           emit @order.cancelled { order_id, customer_id }
         }
         command A(order_id: Uuid, customer_id: Int, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total }
         }",
        "declarations are a set; the entries are sorted by kind and name",
    );
}

#[test]
fn file_boundaries_do_not_matter() {
    const COMMAND: &str = "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
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
             owner: Int @index,
           }
           on @order.placed { order_id, customer_id } {
             put E { id: order_id, owner: customer_id }
           }
         }",
        "projector P {
           entity E {
             id: Uuid @key,
             owner: Int,

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
        "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
           let one = total + total
           let two = total
           emit @order.placed { order_id, customer_id, total: one }
         }",
        "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
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
        "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total }
         }",
        "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
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

      on @order.placed as e {
        let sent = http.post(\"https://ship.example/\", {
          \"b\": ping(\"https://b.example/\"),
          \"a\": ping(\"https://a.example/\"),
        })
        if sent.status >= 400 {
          fail(\"no\")
        }
      }
    }
    ";
    const TWO: &str = "effect Ship {
      fn ping(url: String) -> Int {
        let response = http.get(url)
        return response.status
      }

      on @order.placed as e {
        let sent = http.post(\"https://ship.example/\", {
          \"a\": ping(\"https://a.example/\"),
          \"b\": ping(\"https://b.example/\"),
        })
        if sent.status >= 400 {
          fail(\"no\")
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
           on @order.placed as e {
             let sent = http.post(\"https://ship.example/\", { \"a-b\": 1 })
             if sent.status >= 400 {
               fail(\"no\")
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
        "const LIMIT: Int = 5
         command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
           if customer_id >= LIMIT { return }
           emit @order.placed { order_id, customer_id, total }
         }",
        "const CAP: Int = 5
         command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
           if customer_id >= CAP { return }
           emit @order.placed { order_id, customer_id, total }
         }",
        "a const is inlined, so its name never reaches the IR",
    );
    differs(
        "const LIMIT: Int = 5
         command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
           if customer_id >= LIMIT { return }
           emit @order.placed { order_id, customer_id, total }
         }",
        "const LIMIT: Int = 6
         command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
           if customer_id >= LIMIT { return }
           emit @order.placed { order_id, customer_id, total }
         }",
        "its value is what runs, and it runs at the use site",
    );
}

#[test]
fn a_refusal_message_reaches_every_reject() {
    let before = with_events(
        "refusal Nope \"not this time\"
         command A(order_id: Uuid, customer_id: Int) { return reject Nope }
         command B(order_id: Uuid, customer_id: Int) { return reject Nope }",
    );
    let after = with_events(
        "refusal Nope \"not today\"
         command A(order_id: Uuid, customer_id: Int) { return reject Nope }
         command B(order_id: Uuid, customer_id: Int) { return reject Nope }",
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
        "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total }
         }",
        "refusal NeverSaid \"nothing rejects this\"
         command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total }
         }",
        "a refusal nothing names runs nowhere",
    );
}

#[test]
fn a_guard_is_printed_where_it_runs() {
    let digest = with_events(
        "refusal TooMany \"too many\"

         guard UnderLimit(customer_id: Int) {
           fold open: Int = 0
             on @order.cancelled(customer_id) => open + 1

           if open >= 10 {
             return reject TooMany
           }
         }

         command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
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
        "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total }
         }",
    );

    let kinds: Vec<Kind> = digest.entries().iter().map(|entry| entry.kind).collect();
    assert_eq!(
        kinds,
        vec![Kind::Event, Kind::Event, Kind::Command],
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
        "command A(order_id: Uuid, customer_id: Int, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total }
         }
         command B(order_id: Uuid, customer_id: Int) {
           emit @order.cancelled { order_id, customer_id }
         }",
    );
    let after = with_events(
        "command A(order_id: Uuid, customer_id: Int, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total: total + total }
         }
         command B(order_id: Uuid, customer_id: Int) {
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
        "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total }
         }",
    );
    let with = with_events(
        "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
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
        "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
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
        "{EVENTS}command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {{
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
    let field =
        |extra: &str| format!("event @order.placed {{ order_id: Uuid, customer_id: Int{extra} }}");
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
        "command Count(order_id: Uuid, customer_id: Int) {
           fold seen: Int = 0
             on @order.placed(customer_id) => seen + 1
           if seen > 0 { return }
           emit @order.cancelled { order_id, customer_id }
         }",
        "command Count(order_id: Uuid, customer_id: Int) {
           fold seen: Int = 1
             on @order.placed(customer_id) => seen + 1
           if seen > 0 { return }
           emit @order.cancelled { order_id, customer_id }
         }",
        "a fold's seed is where it starts",
    );

    differs(
        "projector P {
           entity E { id: Uuid @key, owner: Int }
           on @order.placed { order_id, customer_id } { put E { id: order_id, owner: customer_id } }
         }",
        "projector P {
           entity E { id: Uuid @key, owner: Int @index }
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
    const COMMAND: &str = "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
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
        "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
           emit @order.placed { order_id, customer_id, total }
         }",
    );
    let after = with_events(
        "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
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

    let base = "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
      emit @order.placed { order_id, customer_id, total }
    }";
    let retyped = "command Place(order_id: Uuid, customer_id: Int, total: Money(3)) {
      emit @order.placed { order_id, customer_id, total: 0.00 }
    }";
    assert_ne!(
        signatures(base),
        signatures(retyped),
        "a parameter's type is the request body's shape"
    );

    let renamed = "command Place(order_id: Uuid, customer: Int, total: Money(2)) {
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
         command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
           fold open: Int = 0
             on @order.placed(customer_id) => open + 1
           if open >= 10 {
             return reject TooMany
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
             return reject TooMany
           }
           return none
         }

         command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
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
