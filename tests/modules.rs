//! Multi-module loading: declaration order across files does not matter, and an
//! error names the file it is in.

use heklang::{Interpreter, Value, parse, parse_files};

const EVENTS: &str = "event @order.placed { order_id: Uuid, customer_id: Int, total: Money }
event @order.shipped { order_id: Uuid, tracking: String }
";

const COMMAND: &str = "currency USD
command Place(order_id: Uuid, customer_id: Int, total: Money) {
  guard @order.placed(order_id)
  emit @order.placed { order_id, customer_id, total }
}
";

const PROJECTOR: &str = "projector Orders {
  entity Order {
    order_id: Uuid @key,
    total: Money,
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
    assert_eq!(err.line, 2, "line numbers stay module-relative");
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
            "command Discount(total: Money) {\n  let cut = total * 0.9\n  return\n}\n",
        ),
    ])
    .expect("both commands parse");

    let mut interpreter = Interpreter::new(&program);
    let err = interpreter
        .run("Discount", [("total", Value::Money(2_599))])
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

// `currency` is an item like any other, so no module has to be the first one.

#[test]
fn currency_may_live_in_any_module() {
    let program = parse_files([
        ("events/order.hk", EVENTS),
        (
            "commands/place.hk",
            COMMAND
                .strip_prefix("currency USD\n")
                .expect("has the header"),
        ),
        ("config.hk", "currency BHD\n"),
    ])
    .expect("currency is found wherever it sits");

    assert_eq!(program.currency.code, "BHD");
    assert_eq!(program.currency.scale, 3);
}

#[test]
fn currency_must_be_declared_exactly_once() {
    let message = parse_files([
        ("events/order.hk", EVENTS),
        ("commands/place.hk", COMMAND),
        ("config.hk", "currency BHD\n"),
    ])
    .expect_err("two currency declarations")
    .message;
    assert_eq!(
        message, "currency is already declared at commands/place.hk:1:1",
        "the second declaration points at the first, across modules"
    );

    let message = parse_files([("events/order.hk", EVENTS)])
        .expect_err("no currency at all")
        .message;
    assert_eq!(
        message,
        "no `currency` declaration; one module must declare it"
    );
}

#[test]
fn a_single_source_still_parses_without_a_module_name() {
    let source = format!("{COMMAND}{EVENTS}");
    let program = parse(&source).expect("the single-source path is unchanged");
    assert_eq!(program.commands.len(), 1);

    let err = parse("currency USD\ncommand C(x: Int) {\n  let y = nope\n  return\n}\n")
        .expect_err("`nope` is not in scope");
    assert_eq!(err.file, None, "an unnamed source has no module to name");
    assert_eq!(err.to_string(), "3:11: `nope` is not in scope");
}

#[test]
fn the_demo_modules_load_as_separate_files() {
    let commands =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/hek/place_order.hk"))
            .expect("the demo command module");
    let projectors = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/hek/orders.hk"))
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
