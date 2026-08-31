use heklang::{Pos, parse};

#[test]
fn a_command_may_precede_the_events_it_uses() {
    let source = "command PlaceOrder(order_id: Uuid, customer_id: Int) {
  guard @order.placed(order_id)

  state open: Int = fold 0
    on @order.placed(customer_id) => open + 1

  emit @order.placed { order_id, customer_id }
}

event @order.placed {
  order_id: Uuid,
  customer_id: Int,
}
";
    let program = parse(source).expect("events are collected before command bodies are parsed");
    assert_eq!(program.commands.len(), 1);
    assert_eq!(program.commands[0].slices.len(), 2);
}

#[test]
fn events_from_two_files_are_order_independent() {
    let customer_first = "event @customer.blocked { customer_id: Int }
event @order.placed { order_id: Uuid, customer_id: Int }
command C(order_id: Uuid, customer_id: Int) {
  guard @customer.blocked(customer_id), @order.placed(order_id)
  return
}
";
    let order_first = "event @order.placed { order_id: Uuid, customer_id: Int }
event @customer.blocked { customer_id: Int }
command C(order_id: Uuid, customer_id: Int) {
  guard @customer.blocked(customer_id), @order.placed(order_id)
  return
}
";
    for source in [customer_first, order_first] {
        let program = parse(source).expect("file ordering must not matter");
        assert_eq!(program.commands[0].slices.len(), 2);
    }
}

#[test]
fn a_filter_naming_a_later_let_points_at_the_definition() {
    let source = "event @customer.blocked { customer_id: Int }
command C(customer_id: Int) {
  state blocked: Bool = fold false
    on @customer.blocked(customer_id: customer) => true

  let customer = customer_id
  return
}
";
    let err = parse(source).expect_err("`customer` is defined below the declarations");
    assert!(
        err.text().contains("is defined below the declarations"),
        "expected the rule, got: {}",
        err.text()
    );
    assert!(
        err.text().contains("run before the body"),
        "expected the prologue rule to be explained, got: {}",
        err.text()
    );
    // The definition site is a place rather than a sentence, so it is a related
    // location an editor can follow rather than a `6:7` inside the message.
    let [defined] = err.related.as_slice() else {
        panic!("expected one related location, got: {:?}", err.related)
    };
    assert_eq!(defined.span.start, Pos::new(6, 7));
}

#[test]
fn a_body_reference_to_a_later_let_says_so() {
    let source = "event @a.b { x: Int }
command C(y: Int) {
  if y > 0 {
    let a = later
  }
  let later = y
  return
}
";
    let err = parse(source).expect_err("`later` is not bound yet");
    assert!(
        err.text().contains("not in scope yet"),
        "got: {}",
        err.text()
    );
    let [defined] = err.related.as_slice() else {
        panic!("expected one related location, got: {:?}", err.related)
    };
    assert_eq!(defined.span.start, Pos::new(6, 7));
}

#[test]
fn an_unknown_name_stays_a_plain_error() {
    let source = "event @a.b { x: Int }
command C(y: Int) {
  let a = nope
  return
}
";
    let err = parse(source).expect_err("`nope` is never defined");
    assert_eq!(err.text(), "`nope` is not in scope");
}

#[test]
fn duplicate_declarations_are_rejected() {
    let events = "event @a.b { x: Int }
event @a.b { x: Int }
";
    assert_eq!(
        parse(events).expect_err("duplicate event").text(),
        "event @a.b is declared twice"
    );

    let commands = "command C(y: Int) { return }
command C(y: Int) { return }
";
    assert_eq!(
        parse(commands).expect_err("duplicate command").text(),
        "command `C` is declared twice"
    );
}

#[test]
fn a_state_without_fold_is_rejected() {
    let source = "event @a.b { x: Int }
command C(y: Int) {
  state seen: Bool = false
    on @a.b(x: y) => true

  return
}
";
    let err = parse(source).expect_err("`state` needs `fold` before its seed");
    assert_eq!(
        err.text(),
        "`seen` is a fold over the log, so `=` introduces a seed rather than a value; \
         write `= fold <seed>`"
    );
}

#[test]
fn an_effect_may_precede_the_command_it_invokes() {
    let source = "effect Notify {
  on @order.placed as e {
    invoke Record { order_id: e.order_id }
  }
}

command Record(order_id: Uuid) {
  emit @order.recorded { order_id }
}

event @order.placed { order_id: Uuid }
event @order.recorded { order_id: Uuid }
";
    let program =
        parse(source).expect("command signatures are collected before effect bodies are parsed");
    assert_eq!(program.effects.len(), 1);
    assert_eq!(program.effects[0].arms.len(), 1);
}

/// `else if` is a chain rather than a block, so a multi-way dispatch does not nest one
/// level per arm. The expression form always required `else`; the statement form used to
/// require `{` after it as well.
#[test]
fn else_if_chains_without_nesting() {
    let source = "event @order.placed { order_id: Uuid, kind: Int }
refusal Three \"no\"

command Route(order_id: Uuid, kind: Int) {
  if kind == 1 {
    return
  } else if kind == 2 {
    return invalid(\"two\")
  } else if kind == 3 {
    return reject Three
  } else {
    emit @order.placed { order_id, kind }
  }
}
";
    let program = parse(source).expect("`else if` continues the chain");
    let command = &program.commands[0];
    assert_eq!(command.body.len(), 1, "the whole dispatch is one statement");
}

/// The three declaration kinds are separate name spaces. `invoke` reaches only commands
/// and nothing reaches an effect by name, so a shared space would only force renames.
#[test]
fn the_three_kinds_have_separate_namespaces() {
    let source = "event @order.placed { order_id: Uuid }

command Same(order_id: Uuid) {
  emit @order.placed { order_id }
}

projector Same {
  entity Row { order_id: Uuid @key }

  on @order.placed { order_id } {
    put Row { order_id }
  }
}

effect Same {
  on @order.placed as e {
    log(\"placed\")
  }
}
";
    let program = parse(source).expect("one name per kind does not collide across kinds");
    assert_eq!(program.command("Same").map(|c| c.params.len()), Some(1));
    assert_eq!(program.projector("Same").map(|p| p.entities.len()), Some(1));
    assert_eq!(program.effect("Same").map(|e| e.arms.len()), Some(1));

    // Each kind still rejects its own duplicate.
    for (kind, item) in [
        ("command", "command Dup(order_id: Uuid) { return }"),
        ("projector", "projector Dup { entity R { x: Int @key } }"),
        (
            "effect",
            "effect Dup { on @order.placed as e { log(\"x\") } }",
        ),
    ] {
        let doubled = format!("event @order.placed {{ order_id: Uuid }}\n{item}\n{item}\n");
        let message = parse(&doubled)
            .expect_err("the same kind twice is still an error")
            .text();
        assert_eq!(message, format!("{kind} `Dup` is declared twice"));
    }
}
