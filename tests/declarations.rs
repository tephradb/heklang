use heklang::{Code, Command, Pos, parse};

/// Every slice a command declares, across its stages. A command whose declarations are
/// all at the top is one stage, which is every command in this file.
fn slice_count(command: &Command) -> usize {
    command.stages.iter().map(|stage| stage.slices.len()).sum()
}

#[test]
fn a_command_may_precede_the_events_it_uses() {
    let source = "command PlaceOrder(order_id: Uuid, customer_id: Int) {
  guard @order.placed(order_id)

  fold open: Int = 0
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
    assert_eq!(slice_count(&program.commands[0]), 2);
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
        assert_eq!(slice_count(&program.commands[0]), 2);
    }
}

#[test]
fn a_filter_naming_a_later_let_points_at_the_definition() {
    let source = "event @customer.blocked { customer_id: Int }
command C(customer_id: Int) {
  fold blocked: Bool = false
    on @customer.blocked(customer_id: customer) => true

  let customer = customer_id
  return
}
";
    let err = parse(source).expect_err("`customer` is defined below the declarations");
    assert!(
        err.text().contains("is defined below these declarations"),
        "expected the rule, got: {}",
        err.text()
    );
    assert!(
        err.text()
            .contains("reads the log before the statements below it"),
        "expected the staging rule to be explained, got: {}",
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

/// `state` was the keyword a fold used to be declared with, and it bought nothing the
/// parser could not supply: `fold` followed `=` unconditionally. Collapsing the two into
/// one keyword freed the word, and a field called `state` is the kind of name a domain
/// actually wants.
#[test]
fn state_is_an_ordinary_name() {
    let source = "event @a.b { x: Int, state: String }
command C(y: Int, state: String) {
  fold seen: Bool = false
    on @a.b(x: y) => true

  emit @a.b { x: y, state }
}
";
    parse(source).expect("`state` is a name like any other");
}

/// A fold with no arms narrows nothing, so it declares no slice and adds nothing to the
/// append condition. With the keyword at the front the line reads as a binding, which is
/// exactly what it is, so it has to say so.
#[test]
fn a_fold_with_no_arms_is_rejected() {
    let source = "event @a.b { x: Int }
command C(y: Int) {
  fold seen: Bool = false

  emit @a.b { x: y }
}
";
    let err = parse(source).expect_err("a fold with no arms folds nothing");
    assert_eq!(err.code, Code::EmptyDeclaration);
    assert_eq!(
        err.text(),
        "fold `seen` has no arms, so it is only its seed; a fold with no arms declares no \
         slice and adds nothing to the append condition; write `let seen = <seed>`"
    );
}

/// The structural error wins, and is the only one. A seed is checked against the type its
/// fold declares, so a declaration with no arms was answering a question it does not ask:
/// both used to be reported, seed first, which put the message about the half that was
/// not wrong in front of the author.
#[test]
fn an_arm_less_fold_reports_the_shape_rather_than_the_seed() {
    let source = "event @a.b { x: Int }
command C(y: Int, text: String) {
  fold seen: Int = text

  emit @a.b { x: y }
}
";
    let err = parse(source).expect_err("no arms, and a seed of the wrong type");
    assert_eq!(err.code, Code::EmptyDeclaration);
    assert!(
        !err.text().contains("expected Int, found String"),
        "the seed's type is not the mistake to lead with; got: {}",
        err.text()
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
    assert_eq!(
        command.stages.len(),
        1,
        "no statement splits the declarations, so this is one staged read"
    );
    let statements: usize = command
        .stages
        .iter()
        .map(|stage| stage.pre.len() + stage.post.len())
        .sum();
    assert_eq!(statements, 1, "the whole dispatch is one statement");
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
