//! Multi-module loading: declaration order across files does not matter, and an
//! error names the file it is in.

use std::fs;

use heklang::{Interpreter, Value, parse, parse_files};

const EVENTS: &str = "event @order.placed { order_id: Uuid, customer_id: Int, total: Money(2) }
event @order.shipped { order_id: Uuid, tracking: String }
";

const COMMAND: &str = "command Place(order_id: Uuid, customer_id: Int, total: Money(2)) {
  guard @order.placed(order_id)
  emit @order.placed { order_id, customer_id, total }
}
";

const PROJECTOR: &str = "projector Orders {
  entity Order {
    order_id: Uuid @key,
    total: Money(2),
    tracking: String?,
  }

  on @order.placed { order_id, total } {
    put Order { order_id, total, tracking: none }
  }

  on @order.shipped { order_id, tracking } {
    patch Order[order_id] { tracking }
  }
}
";

#[test]
fn a_module_may_use_events_declared_in_another_file() {
    let program = parse_files([
        ("commands/place.hk", COMMAND),
        ("projectors/orders.hk", PROJECTOR),
        ("events/order.hk", EVENTS),
    ])
    .expect("declarations are collected across every module first");

    assert_eq!(program.events.len(), 2);
    assert_eq!(program.commands.len(), 1);
    assert_eq!(program.projectors[0].handlers.len(), 2);
}

#[test]
fn module_order_does_not_matter() {
    let orders = [
        [
            ("events/order.hk", EVENTS),
            ("commands/place.hk", COMMAND),
            ("projectors/orders.hk", PROJECTOR),
        ],
        [
            ("projectors/orders.hk", PROJECTOR),
            ("events/order.hk", EVENTS),
            ("commands/place.hk", COMMAND),
        ],
        [
            ("commands/place.hk", COMMAND),
            ("projectors/orders.hk", PROJECTOR),
            ("events/order.hk", EVENTS),
        ],
    ];

    for files in orders {
        let names: Vec<&str> = files.iter().map(|(name, _)| *name).collect();
        let program = parse_files(files).unwrap_or_else(|err| panic!("for {names:?}: {err}"));
        assert_eq!(program.projectors[0].handlers.len(), 2, "for {names:?}");
    }
}

#[test]
fn a_syntax_error_names_its_module() {
    let broken = "command Broken(x: Int) {\n  let y = nope\n  return\n}\n";
    let err = parse_files([
        ("events/order.hk", EVENTS),
        ("commands/place.hk", COMMAND),
        ("commands/broken.hk", broken),
    ])
    .expect_err("`nope` is not in scope");

    assert_eq!(err.file.as_deref(), Some("commands/broken.hk"));
    assert_eq!(err.span.start.line, 2, "line numbers stay module-relative");
    assert_eq!(
        err.to_string(),
        "commands/broken.hk:2:11: `nope` is not in scope"
    );
}

#[test]
fn a_lex_error_names_its_module() {
    let err = parse_files([
        ("events/order.hk", EVENTS),
        ("commands/place.hk", COMMAND),
        ("commands/bad.hk", "command C(x: Int) { let y = x & 1\n}\n"),
    ])
    .expect_err("`&` alone is not a symbol");

    assert_eq!(err.file.as_deref(), Some("commands/bad.hk"));
}

#[test]
fn a_runtime_error_names_the_module_of_the_declaration_that_raised_it() {
    let program = parse_files([
        ("events/order.hk", EVENTS),
        ("commands/place.hk", COMMAND),
        (
            "commands/discount.hk",
            "command Discount(total: Money(2)) {\n  let cut = total * 0.9\n  return\n}\n",
        ),
    ])
    .expect("both commands parse");

    let mut interpreter = Interpreter::new(&program);
    let err = interpreter
        .run("Discount", [("total", Value::money(2_599, 2))])
        .expect_err("the multiplication is not exact");

    assert_eq!(err.module.as_deref(), Some("commands/discount.hk"));
    assert!(
        err.to_string().starts_with("commands/discount.hk:2:19:"),
        "got: {err}"
    );
}

#[test]
fn a_projector_runtime_error_names_its_module() {
    let projector = "projector Notes {
  entity Note {
    order_id: Uuid @key,
    tracking: String @max(4),
  }

  on @order.shipped { order_id, tracking } {
    put Note { order_id, tracking }
  }
}
";
    let program = parse_files([
        ("events/order.hk", EVENTS),
        ("commands/place.hk", COMMAND),
        ("projectors/notes.hk", projector),
    ])
    .expect("parses");

    let log = vec![heklang::Event::new(
        heklang::EventPath::new(["order", "shipped"]),
        [
            ("order_id", Value::uuid("order-1")),
            ("tracking", Value::str("TRK-0001")),
        ],
    )];
    let interpreter = Interpreter::with_log(&program, log);
    let err = interpreter
        .project("Notes")
        .expect_err("8 characters into @max(4)");

    assert_eq!(
        err.to_string(),
        "projectors/notes.hk:8:26: tracking is 8 characters, the most allowed is 4"
    );
}

#[test]
fn a_program_needs_no_header_item() {
    // There is nothing a module has to declare and nothing one module has to declare
    // for the others, so a file of plain declarations is a whole program.
    let program = parse_files([("events/order.hk", EVENTS)]).expect("declarations are enough");
    assert_eq!(program.events.len(), 2);
    assert!(program.commands.is_empty());
}

#[test]
fn a_single_source_still_parses_without_a_module_name() {
    let source = format!("{COMMAND}{EVENTS}");
    let program = parse(&source).expect("the single-source path is unchanged");
    assert_eq!(program.commands.len(), 1);

    let err = parse("command C(x: Int) {\n  let y = nope\n  return\n}\n")
        .expect_err("`nope` is not in scope");
    assert_eq!(err.file, None, "an unnamed source has no module to name");
    assert_eq!(err.to_string(), "2:11: `nope` is not in scope");
}

#[test]
fn the_demo_modules_load_as_separate_files() {
    let commands = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/hek/place_order.hk"))
        .expect("the demo command module");
    let projectors = fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/hek/orders.hk"))
        .expect("the demo projector module");

    let program = parse_files([
        ("commands/place_order.hk", commands.as_str()),
        ("projectors/orders.hk", projectors.as_str()),
    ])
    .unwrap_or_else(|err| panic!("{err}"));

    assert_eq!(program.projectors.len(), 2);
    assert_eq!(
        program.projectors[0].module.as_deref(),
        Some("projectors/orders.hk")
    );
    assert_eq!(
        program
            .command("PlaceOrder")
            .expect("declared")
            .module
            .as_deref(),
        Some("commands/place_order.hk")
    );
}

/// There is no import syntax, so a `fn` is reachable from every module. Signatures are
/// collected across every file before any body is parsed, which is what makes the file
/// order irrelevant here too.
#[test]
fn a_fn_is_callable_from_another_module() {
    const LIB: &str = "fn effective_sku(sku: String?, order_id: Uuid) -> String {
  let given = sku.unwrap_or(\"\").trim()
  if given.is_empty() {
    return \"SKU:{order_id}\"
  }
  return given
}
";
    const SKUS: &str = "event @order.skued { order_id: Uuid, sku: String }
";
    const CALLER: &str = "command Sku(order_id: Uuid, sku: String?) {
  emit @order.skued { order_id, sku: effective_sku(sku, order_id) }
}
";

    // The caller's file first, so the definition is only found because collection runs
    // over every module before any body does.
    let orders = [
        [
            ("commands/sku.hk", CALLER),
            ("events/sku.hk", SKUS),
            ("lib/sku.hk", LIB),
        ],
        [
            ("lib/sku.hk", LIB),
            ("commands/sku.hk", CALLER),
            ("events/sku.hk", SKUS),
        ],
    ];

    for files in orders {
        let names: Vec<&str> = files.iter().map(|(name, _)| *name).collect();
        let program = parse_files(files).unwrap_or_else(|err| panic!("for {names:?}: {err}"));
        assert_eq!(program.functions.len(), 1, "for {names:?}");

        let mut interpreter = Interpreter::new(&program);
        let execution = interpreter
            .run(
                "Sku",
                vec![
                    (
                        "order_id",
                        Value::uuid("0190d1a1-0000-7000-8000-000000000001"),
                    ),
                    ("sku", Value::none(heklang::Type::String)),
                ],
            )
            .unwrap_or_else(|err| panic!("for {names:?}: {err}"));
        let heklang::Outcome::Ok(events) = execution.outcome else {
            panic!("for {names:?}: expected an append");
        };
        assert_eq!(
            events[0].field("sku"),
            Some(&Value::str("SKU:0190d1a1-0000-7000-8000-000000000001")),
            "for {names:?}: the helper ran, from another module"
        );
    }
}

/// One flat space, so the same helper in two files is a collision rather than two
/// module-local helpers. The error names the second file, which is the one to change.
#[test]
fn a_fn_declared_in_two_modules_collides() {
    const LIB: &str = "fn parse_gid(gid: String) -> Int {\n  return gid.after_last(\"/\").to_int().unwrap_or(0)\n}\n";
    let err = parse_files([
        ("events/order.hk", EVENTS),
        ("lib/shopify.hk", LIB),
        ("lib/stripe.hk", LIB),
    ])
    .expect_err("one name, one function");
    assert_eq!(err.message, "fn `parse_gid` is declared twice");
    assert_eq!(err.file.as_deref(), Some("lib/stripe.hk"));
}
