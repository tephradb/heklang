use heklang::parse;

#[test]
fn a_command_may_precede_the_events_it_uses() {
    let source = "currency USD
command PlaceOrder(order_id: Uuid, customer_id: Int) {
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
    let customer_first = "currency USD
event @customer.blocked { customer_id: Int }
event @order.placed { order_id: Uuid, customer_id: Int }
command C(order_id: Uuid, customer_id: Int) {
  guard @customer.blocked(customer_id), @order.placed(order_id)
  return
}
";
    let order_first = "currency USD
event @order.placed { order_id: Uuid, customer_id: Int }
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
    let source = "currency USD
event @customer.blocked { customer_id: Int }
command C(customer_id: Int) {
  state blocked: Bool = fold false
    on @customer.blocked(customer_id: customer) => true

  let customer = customer_id
  return
}
";
    let err = parse(source).expect_err("`customer` is defined below the declarations");
    assert!(
        err.message.contains("is defined at 7:7"),
        "expected the definition site, got: {}",
        err.message
    );
    assert!(
        err.message.contains("run before the body"),
        "expected the prologue rule to be explained, got: {}",
        err.message
    );
}

#[test]
fn a_body_reference_to_a_later_let_says_so() {
    let source = "currency USD
event @a.b { x: Int }
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
        err.message.contains("not in scope yet"),
        "got: {}",
        err.message
    );
    assert!(err.message.contains("7:7"), "got: {}", err.message);
}

#[test]
fn an_unknown_name_stays_a_plain_error() {
    let source = "currency USD
event @a.b { x: Int }
command C(y: Int) {
  let a = nope
  return
}
";
    let err = parse(source).expect_err("`nope` is never defined");
    assert_eq!(err.message, "`nope` is not in scope");
}

#[test]
fn duplicate_declarations_are_rejected() {
    let events = "currency USD
event @a.b { x: Int }
event @a.b { x: Int }
";
    assert_eq!(
        parse(events).expect_err("duplicate event").message,
        "event @a.b is declared twice"
    );

    let commands = "currency USD
command C(y: Int) { return }
command C(y: Int) { return }
";
    assert_eq!(
        parse(commands).expect_err("duplicate command").message,
        "command `C` is declared twice"
    );
}

#[test]
fn a_state_without_fold_is_rejected() {
    let source = "currency USD
event @a.b { x: Int }
command C(y: Int) {
  state seen: Bool = false
    on @a.b(x: y) => true

  return
}
";
    let err = parse(source).expect_err("`state` needs `fold` before its seed");
    assert_eq!(
        err.message,
        "`seen` is a fold over the log, so `=` introduces a seed rather than a value; \
         write `= fold <seed>`"
    );
}
