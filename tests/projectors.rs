//! `docs/projectors.md` as executable tests, one test per numbered rule.

use std::fs;

use heklang::ir::Stmt;
use heklang::{Event, EventPath, Interpreter, Key, Store, Value, parse};

const EVENTS: &str = "event @order.placed {
  order_id: Uuid,
  customer_id: Int,
  email: String @subject(customer_id) @max(200),
  total: Money(2),
}
event @order.shipped { order_id: Uuid, tracking: String }
event @order.purged { order_id: Uuid }
";

/// A projector with the two entities most of these tests write to.
fn source(body: &str) -> String {
    format!(
        "{EVENTS}projector P {{
  enum Status {{ @default Placed, Shipped }}

  entity Order {{
    order_id: Uuid @key,
    customer_id: Int @index,
    total: Money(2),
    status: Status,
    tracking: String?,
  }}

  entity Customer {{
    customer_id: Int @key,
    order_count: Int,
    lifetime_spend: Money(2),
  }}

{body}
}}
"
    )
}

fn placed(seq: u32, customer_id: i64, total: i64) -> Event {
    Event::new(
        EventPath::new(["order", "placed"]),
        [
            ("order_id", Value::uuid(format!("order-{seq}"))),
            ("customer_id", Value::Int(customer_id)),
            ("email", Value::str("ada@example.com")),
            ("total", Value::money(total, 2)),
        ],
    )
}

fn shipped(seq: u32, tracking: &str) -> Event {
    Event::new(
        EventPath::new(["order", "shipped"]),
        [
            ("order_id", Value::uuid(format!("order-{seq}"))),
            ("tracking", Value::str(tracking)),
        ],
    )
}

fn purged(seq: u32) -> Event {
    Event::new(
        EventPath::new(["order", "purged"]),
        [("order_id", Value::uuid(format!("order-{seq}")))],
    )
}

fn err(body: &str) -> String {
    parse(&source(body))
        .expect_err("expected this projector to be rejected")
        .text()
}

/// Runs a projector over `log` and returns its read models.
fn project(body: &str, log: Vec<Event>) -> Store {
    let source = source(body);
    let program = parse(&source).unwrap_or_else(|err| panic!("{err}"));
    let interpreter = Interpreter::with_log(&program, log);
    interpreter
        .project("P")
        .unwrap_or_else(|err| panic!("{err}"))
}

const PLACE: &str = "  on @order.placed { order_id, customer_id, total } {
    put Order {
      order_id, customer_id, total,
      status: Placed,
      tracking: none,
    }
    patch Customer[customer_id] {
      order_count: .order_count + 1,
      lifetime_spend: .lifetime_spend + total,
    }
  }";

// Rule 1: handler form.

#[test]
fn a_handler_without_as_cannot_reach_the_envelope() {
    let message = err("  on @order.placed { order_id } {\n    delete Order[e.id]\n  }");
    assert_eq!(message, "`e` is not in scope");
}

#[test]
fn as_binds_at_id_and_position() {
    let store = project(
        "  on @order.placed as e { order_id } {
    put Order {
      order_id: e.id,
      customer_id: e.position,
      total: e.total,
      status: Placed,
      tracking: none,
    }
  }",
        vec![placed(9, 7, 2_599), placed(1, 7, 500)],
    );

    // The second event, so position 1 and the id the interpreter stamped for it.
    let row = store
        .get(
            "Order",
            &Key::Uuid("0190d1a1-0000-7000-9000-000000000001".into()),
        )
        .expect("`e.id` became the key");
    assert_eq!(row.field("customer_id"), Some(&Value::Int(1)));
    assert_eq!(
        row.field("total"),
        Some(&Value::money(500, 2)),
        "`e.total` reads a payload field that was never destructured"
    );
}

#[test]
fn the_envelope_reaches_at_as_a_timestamp() {
    let source = format!(
        "{EVENTS}projector P {{
  entity Stamp {{ order_id: Uuid @key, at: Timestamp? }}
  on @order.placed as e {{ order_id }} {{
    put Stamp {{ order_id, at: e.at }}
  }}
}}
"
    );
    let program = parse(&source).unwrap_or_else(|err| panic!("{err}"));
    let interpreter = Interpreter::with_log(&program, vec![placed(1, 7, 2_599)]);
    let store = interpreter
        .project("P")
        .unwrap_or_else(|err| panic!("{err}"));
    let row = store
        .get("Stamp", &Key::Uuid("order-1".into()))
        .expect("the put ran");
    assert_eq!(
        row.field("at"),
        Some(&Value::some(Value::Timestamp(1_577_836_800_000_000))),
        "`e.at` is epoch microseconds, and a Timestamp? field wraps it"
    );
}

#[test]
fn a_payload_field_reached_through_as_stays_out_of_scope() {
    let message = err("  on @order.placed as e { order_id } {
    patch Order[order_id] { total: e.total }
    patch Order[order_id] { total: total }
  }");
    assert_eq!(
        message, "`total` is not in scope",
        "`e.total` binds a slot but does not put `total` in scope"
    );
}
#[test]
fn the_envelope_name_alone_is_not_a_value() {
    let message = err(
        "  on @order.placed as e { order_id } {\n    let x = e\n    delete Order[order_id]\n  }",
    );
    assert!(message.contains("is the event envelope"), "got: {message}");
}

// Rule 2: statements.

#[test]
fn put_writes_every_field_patch_only_the_listed_ones() {
    let store = project(
        &format!(
            "{PLACE}\n\n  on @order.shipped {{ order_id, tracking }} {{\n    patch Order[order_id] {{ status: Shipped, tracking }}\n  }}"
        ),
        vec![placed(1, 7, 2_599), shipped(1, "TRK-1")],
    );

    let row = store
        .get("Order", &Key::Uuid("order-1".into()))
        .expect("the row exists");
    assert_eq!(
        row.field("total"),
        Some(&Value::money(2_599, 2)),
        "a field the patch did not list is left alone"
    );
    assert_eq!(
        row.field("status"),
        Some(&Value::Enum {
            ty: "Status".into(),
            variant: "Shipped".into()
        })
    );
}

/// Rule 5: `put` never reads the zero table, so a column it leaves out has no value to
/// fall back on. The runtime held this and the checker did not, which meant a projector
/// that missed a column checked clean and failed on the first event it saw.
#[test]
fn put_must_write_every_field() {
    let source = source("  on @order.placed { order_id } {\n    put Order { order_id }\n  }");
    let err = parse(&source).expect_err("a partial `put`");
    assert_eq!(
        err.text(),
        "`put Order` needs `customer_id`; a `put` writes the whole row, so it never reads a default"
    );
}

#[test]
fn delete_removes_the_row() {
    let store = project(
        &format!(
            "{PLACE}\n\n  on @order.purged {{ order_id }} {{\n    delete Order[order_id]\n  }}"
        ),
        vec![placed(1, 7, 2_599), placed(2, 7, 1_000), purged(1)],
    );

    assert!(store.get("Order", &Key::Uuid("order-1".into())).is_none());
    assert!(store.get("Order", &Key::Uuid("order-2".into())).is_some());
}

// Rule 3: stored-value reference.

#[test]
fn a_leading_dot_reads_the_stored_value_a_bare_name_does_not() {
    // `total` is the event's field; `.total` is the one already in the row.
    let store = project(
        &format!(
            "{PLACE}\n\n  on @order.placed {{ order_id, total }} {{\n    patch Order[order_id] {{ total: .total + total }}\n  }}"
        ),
        vec![placed(1, 7, 2_599)],
    );

    let row = store
        .get("Order", &Key::Uuid("order-1".into()))
        .expect("the row exists");
    assert_eq!(
        row.field("total"),
        Some(&Value::money(5_198, 2)),
        "the put stored 2599, then the patch added the event's 2599 to it"
    );
}

#[test]
fn a_dot_field_outside_a_patch_is_an_error() {
    let message = err("  on @order.placed { order_id, customer_id, total } {
    put Order {
      order_id, customer_id,
      total: .total,
      status: Placed,
      tracking: none,
    }
  }");
    assert!(
        message.contains("only a `patch` or `update` value can do"),
        "got: {message}"
    );

    let message = err(
        "  on @order.placed { order_id } {\n    let x = .total\n    delete Order[order_id]\n  }",
    );
    assert!(
        message.contains("only a `patch` or `update` value can do"),
        "got: {message}"
    );
}

#[test]
fn a_dot_field_must_name_a_field_of_the_patched_entity() {
    let message =
        err("  on @order.placed { order_id } {\n    patch Order[order_id] { total: .nope }\n  }");
    assert_eq!(message, "entity `Order` has no field `nope`");
}

#[test]
fn each_stored_field_is_loaded_once_per_patch() {
    let source = source(
        "  on @order.placed { order_id } {\n    patch Order[order_id] { total: .total + .total }\n  }",
    );
    let program = parse(&source).unwrap_or_else(|err| panic!("{err}"));
    let handler = &program.projector("P").expect("declared").handlers[0];
    let Stmt::Patch { loads, .. } = &handler.body[0] else {
        panic!("expected a patch");
    };
    assert_eq!(loads.len(), 1, "`.total` twice is one stored load");
    assert_eq!(loads[0].field, "total");
}

// Rule 4: no general reads.

#[test]
fn a_handler_cannot_read_another_entity() {
    // There is no syntax for it: an entity name is not a value.
    let message = err(
        "  on @order.placed { order_id } {\n    let x = Customer\n    delete Order[order_id]\n  }",
    );
    assert_eq!(message, "`Customer` is not in scope");
}

#[test]
fn a_patch_cannot_read_a_different_row() {
    // `.field` always means the row being patched; there is no way to name another.
    let message = err(
        "  on @order.placed { order_id } {\n    patch Order[order_id] { total: Customer.lifetime_spend }\n  }",
    );
    assert_eq!(message, "`Customer` is not in scope");
}

// Rule 5: zero values.

#[test]
fn a_patch_materializes_a_missing_row_from_zeros() {
    let store = project(
        "  on @order.placed { order_id, total } {
    patch Order[order_id] { total }
  }",
        vec![placed(1, 7, 2_599)],
    );

    let row = store
        .get("Order", &Key::Uuid("order-1".into()))
        .expect("the patch materialized the row");
    assert_eq!(row.field("customer_id"), Some(&Value::Int(0)));
    assert_eq!(
        row.field("status"),
        Some(&Value::Enum {
            ty: "Status".into(),
            variant: "Placed".into()
        }),
        "an enum starts at its @default variant"
    );
    assert_eq!(
        row.field("tracking"),
        Some(&Value::none(heklang::Type::String))
    );
    assert_eq!(
        row.field("order_id"),
        Some(&Value::Uuid("order-1".into())),
        "the key comes from the subscript, not from a zero"
    );
}

#[test]
fn a_deleted_row_re_materializes_on_the_next_patch() {
    let store = project(
        &format!(
            "{PLACE}\n\n  on @order.purged {{ order_id }} {{\n    delete Order[order_id]\n  }}\n\n  on @order.shipped {{ order_id, tracking }} {{\n    patch Order[order_id] {{ status: Shipped, tracking }}\n  }}"
        ),
        vec![placed(1, 7, 2_599), purged(1), shipped(1, "TRK-1")],
    );

    let row = store
        .get("Order", &Key::Uuid("order-1".into()))
        .expect("the patch brought the row back");
    assert_eq!(
        row.field("total"),
        Some(&Value::money(0, 2)),
        "the re-materialized row starts from zeros, not from what was deleted"
    );
}

#[test]
fn update_applies_to_a_row_that_exists() {
    let store = project(
        &format!(
            "{PLACE}\n\n  on @order.shipped {{ order_id, tracking }} {{\n    update Order[order_id] {{ status: Shipped, tracking }}\n  }}"
        ),
        vec![placed(1, 7, 2_599), shipped(1, "TRK-1")],
    );

    let row = store
        .get("Order", &Key::Uuid("order-1".into()))
        .expect("the row was put before the update");
    assert_eq!(
        row.field("status"),
        Some(&Value::Enum {
            ty: "Status".into(),
            variant: "Shipped".into()
        })
    );
    assert_eq!(
        row.field("tracking"),
        Some(&Value::some(Value::str("TRK-1"))),
        "an update writes exactly what a patch would have"
    );
}

#[test]
fn update_on_an_absent_row_creates_nothing() {
    let store = project(
        "  on @order.shipped { order_id, tracking } {
    update Order[order_id] { status: Shipped, tracking }
  }",
        vec![shipped(1, "TRK-1")],
    );

    assert_eq!(
        store.get("Order", &Key::Uuid("order-1".into())),
        None,
        "no row, not a row whose columns happen to be unchanged"
    );
    assert_eq!(store.len("Order"), 0);
}

/// The story rule 3 tells about `update`: the load can only have come from a real row,
/// because an absent one never reaches the value expressions.
#[test]
fn a_dot_field_inside_an_update_reads_the_stored_value() {
    let store = project(
        &format!(
            "{PLACE}\n\n  on @order.shipped {{ order_id }} {{\n    update Order[order_id] {{ total: .total + 1.00 }}\n  }}"
        ),
        vec![placed(1, 7, 2_599), shipped(1, "TRK-1")],
    );

    let row = store
        .get("Order", &Key::Uuid("order-1".into()))
        .expect("the row exists");
    assert_eq!(row.field("total"), Some(&Value::money(2_699, 2)));
}

/// The side-by-side the rule exists for: one handler, one absent key, two entities, and
/// the answer differs because what absent means differs.
#[test]
fn update_and_patch_on_the_same_absent_key_differ() {
    let store = project(
        "  on @order.placed { order_id, customer_id } {
    update Order[order_id] { status: Shipped }
    patch Customer[customer_id] { order_count: .order_count + 1 }
  }",
        vec![placed(1, 7, 2_599)],
    );

    assert_eq!(
        store.get("Order", &Key::Uuid("order-1".into())),
        None,
        "an identity: absent means the order does not exist"
    );
    let counter = store
        .get("Customer", &Key::Int(7))
        .expect("a counter: absent means zero of it");
    assert_eq!(counter.field("order_count"), Some(&Value::Int(1)));
}

#[test]
fn a_deleted_row_stays_deleted_under_update() {
    let store = project(
        &format!(
            "{PLACE}\n\n  on @order.purged {{ order_id }} {{\n    delete Order[order_id]\n  }}\n\n  on @order.shipped {{ order_id, tracking }} {{\n    update Order[order_id] {{ status: Shipped, tracking }}\n  }}"
        ),
        vec![placed(1, 7, 2_599), purged(1), shipped(1, "TRK-1")],
    );

    assert_eq!(
        store.get("Order", &Key::Uuid("order-1".into())),
        None,
        "the half of `delete` is not a tombstone that a statement can close"
    );
}

#[test]
fn update_outside_a_projector_says_where_it_belongs() {
    let source = format!(
        "{EVENTS}command Ship(order_id: Uuid) {{
  update Order[order_id] {{ status: Shipped }}
}}
"
    );
    let message = parse(&source)
        .expect_err("only a projector writes a read model")
        .text();
    assert_eq!(
        message,
        "`update` writes an entity, so it can only appear in a projector"
    );
}

/// The zero table is read by a materializing `patch` and by nothing else, so this is
/// where the demand for a zero comes from.
#[test]
fn uuid_and_timestamp_have_no_zero() {
    for ty in ["Uuid", "Timestamp"] {
        let source = format!(
            "{EVENTS}projector P {{
  entity Thing {{
    id: Int @key,
    stamp: {ty},
    seen: Int,
  }}
  on @order.placed {{ order_id }} {{
    patch Thing[1] {{ seen: .seen + 1 }}
  }}
}}
"
        );
        let message = parse(&source)
            .expect_err("a patched entity needs a zero for every column")
            .text();
        assert_eq!(
            message,
            format!(
                "this `patch` materializes a `Thing`, and `stamp` is a {ty} with no zero value; give it a default, make it `{ty}?`, or make this an `update`"
            )
        );
    }
}

/// The error above offers three escapes, so all three have to exist. The default is the
/// one a `Timestamp` column could not take: there was no way to write a moment down, so
/// the advice named a door that was not there.
#[test]
fn every_escape_the_zero_error_offers_is_writable() {
    for (ty, literal) in [
        ("Uuid", "\"6ba7b810-9dad-11d1-80b4-00c04fd430c8\""),
        ("Timestamp", "\"2026-01-01T00:00:00Z\""),
    ] {
        for column in [format!("stamp: {ty} = {literal}"), format!("stamp: {ty}?")] {
            let source = format!(
                "{EVENTS}projector P {{
  entity Thing {{
    id: Int @key,
    {column},
    seen: Int,
  }}
  on @order.placed {{ order_id }} {{
    patch Thing[1] {{ seen: .seen + 1 }}
  }}
}}
"
            );
            parse(&source).unwrap_or_else(|err| panic!("for `{column}`: {err}"));
        }
    }
}

/// And the default is read, not merely accepted: a materializing `patch` puts it in the
/// row, which is the whole reason the zero table exists.
#[test]
fn a_written_timestamp_default_materializes_a_row() {
    let source = format!(
        "{EVENTS}projector P {{
  entity Thing {{
    id: Int @key,
    stamp: Timestamp = \"2020-01-01T00:00:00Z\",
    seen: Int,
  }}
  on @order.placed {{ order_id }} {{
    patch Thing[1] {{ seen: .seen + 1 }}
  }}
}}
"
    );
    let program = parse(&source).unwrap_or_else(|err| panic!("expected this to parse: {err}"));
    let interpreter = Interpreter::with_log(&program, vec![placed(1, 7, 1000)]);
    let store = interpreter
        .project("P")
        .unwrap_or_else(|err| panic!("expected this to project: {err}"));
    let row = store.get("Thing", &Key::Int(1)).expect("the row");
    assert_eq!(
        row.field("stamp"),
        Some(&Value::Timestamp(1_577_836_800_000_000))
    );
    assert_eq!(row.field("seen"), Some(&Value::Int(1)));
}

/// The same entity, written only by `put`, `update` and `delete`. Nothing can
/// materialize it, so nothing ever reads a zero for `stamp`, and demanding one would
/// be demanding a sentinel: exactly what the table refuses to make a zero.
#[test]
fn an_unpatched_entity_needs_no_zero() {
    for ty in ["Uuid", "Timestamp"] {
        let source = format!(
            "{EVENTS}projector P {{
  entity Thing {{
    id: Int @key,
    stamp: {ty},
    seen: Int,
  }}
  on @order.placed {{ order_id }} {{
    update Thing[1] {{ seen: .seen + 1 }}
    delete Thing[2]
  }}
}}
"
        );
        parse(&source).unwrap_or_else(|err| panic!("for {ty}: {err}"));
    }
}

/// A `put` requires every field to be written and never consults a zero, so it is the
/// write itself that proves the column is populated. The runtime check that enforces
/// that is `src/interp.rs`'s missing-field error, which is why dropping a field here
/// is still caught.
#[test]
fn a_put_only_entity_needs_no_zero() {
    let source = format!(
        "{EVENTS}projector P {{
  entity Thing {{
    id: Uuid @key,
    origin: Uuid,
  }}
  on @order.placed {{ order_id }} {{
    put Thing {{ id: order_id, origin: order_id }}
  }}
}}
"
    );
    let program = parse(&source).unwrap_or_else(|err| panic!("{err}"));
    let store = Interpreter::with_log(&program, vec![placed(1, 7, 100)])
        .project("P")
        .unwrap_or_else(|err| panic!("{err}"));
    let row = store
        .get("Thing", &Key::Uuid("order-1".into()))
        .expect("the put landed");
    assert_eq!(row.field("origin"), Some(&Value::uuid("order-1")));
}

/// The requirement is per entity, not per patch: a second entity in the same projector
/// is unaffected by the first one being patched.
#[test]
fn the_zero_requirement_follows_the_entity_that_is_patched() {
    let source = format!(
        "{EVENTS}projector P {{
  entity Counter {{ id: Int @key, seen: Int }}
  entity Identity {{ id: Int @key, stamp: Uuid }}
  on @order.placed {{ order_id }} {{
    patch Counter[1] {{ seen: .seen + 1 }}
    update Identity[2] {{ stamp: order_id }}
  }}
}}
"
    );
    parse(&source).unwrap_or_else(|err| panic!("{err}"));
}

#[test]
fn a_default_or_a_zero_always_has_the_declared_type() {
    let source = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/hek/place_order.hk"))
        .expect("the demo command source")
        + "\n"
        + &fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/hek/orders.hk"))
            .expect("the demo projector source");
    let program = parse(&source).unwrap_or_else(|err| panic!("{err}"));

    for projector in &program.projectors {
        for entity in &projector.entities {
            for (index, field) in entity.fields.iter().enumerate() {
                if index == entity.key {
                    continue;
                }
                let defs = heklang::value::Defs {
                    local: &projector.enums,
                    enums: &program.enums,
                    records: &program.records,
                };
                let value = heklang::value::initial(field, defs).unwrap_or_else(|| {
                    panic!(
                        "{}.{} has neither default nor zero",
                        entity.name, field.name
                    )
                });
                // `has_type` rather than `==`: a sealed column stores a plain value,
                // because the store holds plaintext and the seal is a parse-time
                // rule. See `docs/effects.md` rule 12.
                assert!(
                    value.has_type(&field.ty),
                    "{}.{} starts at {} rather than {}",
                    entity.name,
                    field.name,
                    value.ty(),
                    field.ty
                );
            }
        }
    }
}

#[test]
fn a_money_default_resolves_at_the_declared_scale() {
    for scale in [0u8, 2, 4] {
        let source = format!(
            "event @a.b {{ x: Int }}
projector P {{
  entity Thing {{ id: Int @key, spend: Money({scale}) = 0 }}
  on @a.b {{ x }} {{ delete Thing[x] }}
}}
"
        );
        let program = parse(&source).unwrap_or_else(|err| panic!("scale {scale}: {err}"));
        let entity = &program.projectors[0].entities[0];
        assert_eq!(
            entity.fields[1].default,
            Some(heklang::Literal::Money { units: 0, scale }),
            "for Money({scale})"
        );
    }

    // Widening is exact; more written places than the field holds is an error rather
    // than a silent round, exactly as for `Decimal`.
    let source = "event @a.b { x: Int }
projector P {
  entity Thing { id: Int @key, spend: Money(0) = 0.50 }
  on @a.b { x } { delete Thing[x] }
}
";
    assert_eq!(
        parse(source)
            .expect_err("Money(0) holds no decimal places")
            .text(),
        "2 decimal places is too precise for Money(0)"
    );
}

/// `= none` is the one spelling refused, because an optional column already starts
/// absent. A present default is the ordinary rule every declared position follows.
#[test]
fn an_optional_field_takes_no_none_default() {
    let message = err_entity("tracking2: String? = none");
    assert!(
        message.contains("is optional, so it is already `none` by default"),
        "got: {message}"
    );
}

#[test]
fn an_optional_field_takes_a_present_default() {
    let store = project(
        "  entity Thing {
    id: Uuid @key,
    note: String? = \"held\",
    seen: Int,
  }

  on @order.placed { order_id } {
    patch Thing[order_id] { seen: .seen + 1 }
  }",
        vec![placed(1, 1, 1_000)],
    );
    let row = store
        .get("Thing", &Key::Uuid("order-1".into()))
        .expect("the patch materialized");
    assert_eq!(
        row.field("note"),
        Some(&Value::some(Value::str("held"))),
        "a bare String fills the String? column, wrapped once"
    );
}

// Rule 6: enum defaults.

/// An enum with no `@default` has no zero, so the requirement reaches it the same way
/// it reaches a `Uuid`, and for the same reason: only a `patch` reads one.
#[test]
fn an_enum_field_needs_a_default_variant() {
    let source = format!(
        "{EVENTS}projector P {{
  enum Status {{ Placed, Shipped }}
  entity Order {{ order_id: Uuid @key, status: Status, seen: Int }}
  on @order.placed {{ order_id }} {{ patch Order[order_id] {{ seen: .seen + 1 }} }}
}}
"
    );
    let message = parse(&source).expect_err("no @default variant").text();
    assert_eq!(
        message,
        "this `patch` materializes a `Order`, and `status` is a `Status` with no `@default` variant; give the enum one, give the field a default, or make this an `update`"
    );

    let unpatched = source.replace("patch Order", "update Order");
    parse(&unpatched).unwrap_or_else(|err| panic!("nothing materializes an Order: {err}"));
}

#[test]
fn an_enum_field_may_skip_the_default_when_optional() {
    let source = format!(
        "{EVENTS}projector P {{
  enum Status {{ Placed, Shipped }}
  entity Order {{ order_id: Uuid @key, status: Status? }}
  on @order.placed {{ order_id }} {{ delete Order[order_id] }}
}}
"
    );
    parse(&source).expect("an optional enum field starts at none");
}

#[test]
fn an_enum_declares_at_most_one_default() {
    let source = format!(
        "{EVENTS}projector P {{
  enum Status {{ @default Placed, @default Shipped }}
  entity Order {{ order_id: Uuid @key, status: Status }}
  on @order.placed {{ order_id }} {{ delete Order[order_id] }}
}}
"
    );
    assert_eq!(
        parse(&source).expect_err("two defaults").text(),
        "`Status` has more than one `@default` variant"
    );
}

// Rule 7: enum literals.

#[test]
fn enum_variants_resolve_from_the_target_type() {
    let source = source(
        "  on @order.placed { order_id } {\n    patch Order[order_id] { status: Shipped }\n  }",
    );
    let program = parse(&source).unwrap_or_else(|err| panic!("{err}"));
    let handler = &program.projector("P").expect("declared").handlers[0];
    let Stmt::Patch { fields, .. } = &handler.body[0] else {
        panic!("expected a patch");
    };
    let node = handler.exprs.get(fields[0].1).expect("the value");
    assert!(
        matches!(
            node,
            heklang::Expr::Lit(heklang::Literal::Enum { ty, variant })
                if ty == "Status" && variant == "Shipped"
        ),
        "got: {node:?}"
    );
}

#[test]
fn a_variant_the_target_enum_lacks_is_an_error() {
    let message = err(
        "  on @order.placed { order_id } {\n    patch Order[order_id] { status: Cancelled }\n  }",
    );
    assert_eq!(message, "`Status` has no variant `Cancelled`");
}

#[test]
fn an_ambiguous_variant_names_the_candidates() {
    let source = format!(
        "{EVENTS}projector P {{
  enum Status {{ @default Placed, Shipped }}
  enum Leg {{ @default Placed, Flown }}
  entity Order {{ order_id: Uuid @key, status: Status }}
  on @order.placed {{ order_id }} {{
    let x = Placed
    patch Order[order_id] {{ status: x }}
  }}
}}
"
    );
    let message = parse(&source)
        .expect_err("two enums declare `Placed`")
        .text();
    assert!(
        message.contains("`Placed` is a variant of Status and Leg"),
        "got: {message}"
    );
}

#[test]
fn an_unambiguous_variant_needs_no_target() {
    let source = source(
        "  on @order.placed { order_id } {
    let s = Shipped
    patch Order[order_id] { status: s }
  }",
    );
    parse(&source).expect("`Shipped` is a variant of exactly one enum");
}

// Rule 8: indexes.

#[test]
fn indexes_are_recorded_in_the_ir() {
    let source = format!(
        "{EVENTS}projector P {{
  enum Status {{ @default Placed, Shipped }}
  entity Order {{
    order_id: Uuid @key,
    customer_id: Int @index,
    status: Status,

    index (customer_id, status)
  }}
  on @order.placed {{ order_id }} {{ delete Order[order_id] }}
}}
"
    );
    let program = parse(&source).unwrap_or_else(|err| panic!("{err}"));
    let entity = &program.projectors[0].entities[0];
    assert_eq!(entity.indexes.len(), 2);
    assert_eq!(entity.indexes[0].fields, ["customer_id"]);
    assert_eq!(
        entity.indexes[1].fields,
        ["customer_id", "status"],
        "a compound index keeps its order"
    );
}

#[test]
fn an_index_must_name_declared_fields() {
    let message = err_entity_body("order_id: Uuid @key,\n    total: Money(2),\n\n    index (nope)");
    assert_eq!(message, "entity `Order` has no field `nope` to index");
}

#[test]
fn index_is_still_usable_as_a_field_name() {
    let source = format!(
        "{EVENTS}projector P {{
  entity Order {{ order_id: Uuid @key, index: Int }}
  on @order.placed {{ order_id }} {{ delete Order[order_id] }}
}}
"
    );
    parse(&source).expect("`index` is a soft keyword");
}

// Rule 10: scoping.

#[test]
fn handlers_do_not_share_slots() {
    let source = source(
        "  on @order.placed { order_id, total } {
    let mine = total
    patch Order[order_id] { total: mine }
  }

  on @order.shipped { order_id, tracking } {
    patch Order[order_id] { tracking }
  }",
    );
    let program = parse(&source).unwrap_or_else(|err| panic!("{err}"));
    let handlers = &program.projector("P").expect("declared").handlers;
    assert_eq!(handlers.len(), 2);
    assert!(
        handlers[1].frame < handlers[0].frame,
        "the second handler starts its own frame rather than continuing the first"
    );
}

#[test]
fn a_name_from_one_handler_is_not_in_scope_in_another() {
    let message = err("  on @order.placed { order_id, total } {
    let mine = total
    patch Order[order_id] { total: mine }
  }

  on @order.shipped { order_id } {
    patch Order[order_id] { total: mine }
  }");
    assert_eq!(message, "`mine` is not in scope");
}

// Rule 9: subject propagation. The propagation is live; the checks are not, so
// these are the tests that turn on when `check_subjects` grows a body.

#[test]
#[ignore = "rule 9: the subject conflict check is not implemented yet"]
fn two_handlers_with_different_subjects_conflict() {
    unimplemented!("assert that writing `email` under two subjects is rejected")
}

#[test]
#[ignore = "rule 9: the subject discard check is not implemented yet"]
fn discarding_a_subject_binding_is_an_error() {
    unimplemented!("assert that a subject-bound value cannot land in an unbound field")
}

// Declaration collection: a projector may precede the events it uses.

#[test]
fn a_projector_may_precede_the_events_it_uses() {
    let source = "projector P {
  entity Order { order_id: Uuid @key, total: Money(2) }
  on @order.placed { order_id, total } {
    put Order { order_id, total }
  }
}

event @order.placed { order_id: Uuid, total: Money(2) }
";
    let program = parse(source).expect("events are collected before handler bodies are parsed");
    assert_eq!(program.projectors[0].handlers.len(), 1);
}

#[test]
fn an_entity_may_precede_the_enum_it_names() {
    let source = "event @a.b { x: Int }
projector P {
  entity Thing { id: Int @key, status: Status }
  enum Status { @default On, Off }
  on @a.b { x } { delete Thing[x] }
}
";
    let program = parse(source).expect("enums are collected before entities are parsed");
    assert_eq!(
        program.projectors[0].entities[0].fields[1].ty,
        heklang::Type::Enum("Status".into())
    );
}

#[test]
fn duplicate_projector_declarations_are_rejected() {
    let source = "event @a.b { x: Int }
projector P {
  entity Thing { id: Int @key }
  on @a.b { x } { delete Thing[x] }
}
projector P {
  entity Other { id: Int @key }
  on @a.b { x } { delete Other[x] }
}
";
    assert_eq!(
        parse(source).expect_err("duplicate projector").text(),
        "projector `P` is declared twice"
    );
}

#[test]
fn entities_are_scoped_to_their_projector() {
    let source = "event @a.b { x: Int }
projector P {
  entity Thing { id: Int @key }
  on @a.b { x } { delete Thing[x] }
}
projector Q {
  entity Other { id: Int @key }
  on @a.b { x } { delete Thing[x] }
}
";
    assert_eq!(
        parse(source).expect_err("`Thing` belongs to P").text(),
        "entity `Thing` is not declared"
    );
}

// Keys.

#[test]
fn a_uuid_key_and_a_string_key_are_distinct() {
    let same = "same-text";
    assert_ne!(
        Key::from_value(&Value::uuid(same)),
        Key::from_value(&Value::str(same)),
        "the key discriminant survives, so a Uuid never collides with a String"
    );
    assert_eq!(Key::from_value(&Value::money(1, 2)), None);
    assert_eq!(Key::from_value(&Value::Bool(true)), None);
}

#[test]
fn a_key_must_be_an_orderable_scalar() {
    for (ty, literal) in [("Money(2)", "Money(2)"), ("Bool", "Bool")] {
        let source = format!(
            "event @a.b {{ x: Int }}
projector P {{
  entity Thing {{ id: {ty} @key }}
  on @a.b {{ x }} {{ delete Thing[x] }}
}}
"
        );
        assert_eq!(
            parse(&source).expect_err("{ty} cannot be a key").text(),
            format!("`id` is a {literal}, which cannot be an entity key")
        );
    }
}

#[test]
fn an_entity_needs_exactly_one_key() {
    let message = err_entity_body("order_id: Uuid, total: Money(2)");
    assert_eq!(message, "entity `Order` has no `@key` field");

    let message = err_entity_body("order_id: Uuid @key, total: Money(2) @key");
    assert_eq!(message, "entity `Order` has more than one `@key`");
}

// Statement gating.

#[test]
fn emit_is_a_command_statement_and_the_writes_are_not() {
    let message =
        err("  on @order.placed { order_id } {\n    emit @order.purged { order_id }\n  }");
    assert!(
        message.contains("only appear in a command"),
        "got: {message}"
    );

    for keyword in ["put", "patch", "delete"] {
        let source = format!(
            "event @a.b {{ x: Int }}
command C(x: Int) {{
  {keyword} Thing
  return
}}
"
        );
        let message = parse(&source).expect_err("a write in a command").text();
        assert!(
            message.contains("only appear in a projector"),
            "for `{keyword}`, got: {message}"
        );
    }
}

// Helpers that build a one-entity projector to exercise entity-level errors.

fn err_entity(field: &str) -> String {
    err_entity_body(&format!("order_id: Uuid @key,\n    {field}"))
}

fn err_entity_body(body: &str) -> String {
    let source = format!(
        "{EVENTS}projector P {{
  entity Order {{
    {body}
  }}
  on @order.placed {{ order_id }} {{ delete Order[order_id] }}
}}
"
    );
    parse(&source)
        .expect_err("expected this entity to be rejected")
        .text()
}

// Spans: a write-time failure points at the expression that produced the value.

#[test]
fn a_max_violation_reports_a_line_and_column() {
    // `notes` carries no `@max`, so writing it into a field that has one is the
    // schema bug the `@max` invariant forbids. Until the checker exists, this is
    // where it surfaces.
    let source = "event @order.placed { order_id: Uuid, notes: String }
projector P {
  entity Note {
    order_id: Uuid @key,
    note: String @max(8),
  }
  on @order.placed { order_id, notes } {
    put Note { order_id, note: notes }
  }
}
";
    let program = parse(source).unwrap_or_else(|err| panic!("{err}"));
    let log = vec![Event::new(
        EventPath::new(["order", "placed"]),
        [
            ("order_id", Value::uuid("order-1")),
            ("notes", Value::str("leave at the door")),
        ],
    )];
    let interpreter = Interpreter::with_log(&program, log);
    let err = interpreter
        .project("P")
        .expect_err("17 characters into @max(8)");

    assert_eq!(
        err.to_string(),
        "8:32: note is 17 characters, the most allowed is 8"
    );
    let line = source.lines().nth(7).expect("line 8");
    assert_eq!(
        &line[31..36],
        "notes",
        "column 32 is the expression that produced the value"
    );
}

#[test]
fn a_command_reports_the_same_violation_as_an_outcome() {
    // The same annotation, the other failure mode: a command has a validation
    // channel to route it through, so it is `Invalid` rather than an error.
    let source = "event @order.placed { order_id: Uuid, notes: String @max(8) }
command C(order_id: Uuid, notes: String) {
  emit @order.placed { order_id, notes }
}
";
    let program = parse(source).unwrap_or_else(|err| panic!("{err}"));
    let mut interpreter = Interpreter::new(&program);
    let execution = interpreter
        .run(
            "C",
            [
                ("order_id", Value::uuid("order-1")),
                ("notes", Value::str("leave at the door")),
            ],
        )
        .expect("an over-length field is not an error in a command");

    assert_eq!(
        execution.outcome,
        heklang::Outcome::Invalid("notes is 17 characters, the most allowed is 8".into())
    );
    assert!(
        interpreter.log().is_empty(),
        "a rejected command appends nothing"
    );
}

#[test]
fn a_subject_propagates_from_the_event_into_the_entity_field() {
    // `email` is `@subject(customer_id)` on the event. The entity does not restate
    // it; the binding arrives with the value.
    let source = source(
        "  on @order.placed { order_id, customer_id, email, total } {
    put Order {
      order_id, customer_id, total,
      status: Placed,
      tracking: none,
    }
    patch Customer[customer_id] { note: email }
  }",
    )
    .replace(
        "    order_count: Int,",
        "    order_count: Int,\n    note: String,",
    );
    let program = parse(&source).unwrap_or_else(|err| panic!("{err}"));
    let customer = program
        .projector("P")
        .expect("declared")
        .entity("Customer")
        .expect("declared");

    assert_eq!(
        customer.field("note").expect("declared").subject.as_deref(),
        Some("customer_id"),
        "the subject propagated from @order.placed.email"
    );
    assert_eq!(
        customer
            .field("order_count")
            .expect("declared")
            .subject
            .as_deref(),
        None,
        "a field written from an unbound value carries no subject"
    );
}

#[test]
fn a_handler_may_omit_the_destructure_block() {
    // One block is the body. The same form an effect arm uses, so the two kinds do not
    // differ on the same construct.
    let store = project(
        "  on @order.placed as e {
    put Order {
      order_id: e.order_id,
      customer_id: e.customer_id,
      total: e.total,
      status: Placed,
      tracking: none,
    }
  }",
        vec![placed(1, 7, 2_599)],
    );
    assert_eq!(store.len("Order"), 1);

    let message = err("  on @order.placed { order_id, total }");
    assert_eq!(
        message,
        "this looks like a destructure block; a handler with one needs a body block after it"
    );
}

/// Rule 9's second check, which was the literal empty function
/// `fn check_subject(_target: &EntityField, _incoming: &Ident) {}` until the seal
/// became a type. A column is filed under one key, so it can hold one subject.
#[test]
fn two_handlers_cannot_seal_one_column_under_two_subjects() {
    let message = parse(
        "event @order.placed { order_id: Uuid, customer_id: Int, email: String @subject(customer_id) }
event @shop.noted { order_id: Uuid, shop_id: Int, note: String @subject(shop_id) }

projector P {
  entity Row { order_id: Uuid @key, text: String }

  on @order.placed as e { order_id, email } { put Row { order_id, text: email } }
  on @shop.noted as e { order_id, note } { put Row { order_id, text: note } }
}",
    )
    .expect_err("one column, one subject")
    .text();
    assert!(
        message.contains("`Row.text` already holds content sealed under `customer_id`"),
        "got: {message}"
    );
    assert!(
        message.contains("one column holds one subject"),
        "got: {message}"
    );
}

/// The same column written by two handlers under the **same** subject is the ordinary
/// case, and is what a read model of a shop's credentials looks like.
#[test]
fn two_handlers_may_seal_one_column_under_one_subject() {
    parse(
        "event @order.placed { order_id: Uuid, customer_id: Int, email: String @subject(customer_id) }
event @order.reconfirmed { order_id: Uuid, customer_id: Int, email: String @subject(customer_id) }

projector P {
  entity Row { order_id: Uuid @key, email: String }

  on @order.placed as e { order_id, email } { put Row { order_id, email } }
  on @order.reconfirmed as e { order_id, email } { put Row { order_id, email } }
}",
    )
    .unwrap_or_else(|err| panic!("expected this to parse: {err}"));
}

/// And writes each column once, in a `patch` as well as a `put`: a column written twice
/// in one block is a mistake whichever statement it is in.
#[test]
fn a_write_gives_each_column_once() {
    let source = source(
        "  on @order.placed { order_id, total } {\n    patch Order[order_id] { total, total }\n  }",
    );
    let err = parse(&source).expect_err("`total` is written twice");
    assert_eq!(err.text(), "`total` is given twice");
}
