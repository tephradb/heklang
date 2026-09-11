use heklang::{Code, Command, EventPath, Literal, Pos, parse};

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
  on @order.placed as e { @key order_id } {
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
  on @order.placed as e { @key order_id } {
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
            "effect Dup { on @order.placed as e { @key order_id } { log(\"x\") } }",
        ),
    ] {
        let doubled = format!("event @order.placed {{ order_id: Uuid }}\n{item}\n{item}\n");
        let message = parse(&doubled)
            .expect_err("the same kind twice is still an error")
            .text();
        assert_eq!(message, format!("{kind} `Dup` is declared twice"));
    }
}

// --- @absent ---------------------------------------------------------------

/// The annotation resolves its literal against the declared type at parse time, exactly
/// as an entity column's default does, so nothing unresolved reaches the IR.
#[test]
fn an_absent_value_resolves_against_the_declared_type() {
    let source = "record Note {
  kind: String,
  body: String @absent(\"none given\"),
}

event @order.placed {
  order_id: Uuid,
  note: String @absent(\"\"),
  channel: String @absent(\"web\") @max(10),
  discount: Money(2) @absent(0.00),
  tries: Int @absent(0),
  gift: Bool @absent(false),
  meta: Json @absent(Json.empty),
  detail: Note,
  placed_at: Timestamp @absent(\"2020-01-01T00:00:00Z\"),
  batch: Uuid @absent(\"6ba7b810-9dad-11d1-80b4-00c04fd430c8\"),
}
";
    let program = parse(source).expect("every literal shape resolves against its field's type");
    let event = program
        .event(&EventPath::new(["order", "placed"]))
        .expect("declared");

    assert_eq!(
        event.field("note").and_then(|field| field.absent.clone()),
        Some(Literal::Str("".into()))
    );
    assert_eq!(
        event
            .field("discount")
            .and_then(|field| field.absent.clone()),
        Some(Literal::Money { units: 0, scale: 2 })
    );
    assert_eq!(
        event
            .field("placed_at")
            .and_then(|field| field.absent.clone()),
        Some(Literal::Timestamp(1_577_836_800_000_000))
    );
    assert_eq!(
        event.field("meta").and_then(|field| field.absent.clone()),
        Some(Literal::EmptyJson)
    );
    // A record carries it too: a record reached from an event is stored inside that
    // event's payload, so it has the same history one level down.
    let note = program.record("Note").expect("declared");
    assert_eq!(
        note.field("body").and_then(|field| field.absent.clone()),
        Some(Literal::Str("none given".into()))
    );
    assert_eq!(
        note.field("kind").and_then(|field| field.absent.clone()),
        None
    );
}

/// The question a host asks, answered by the language rather than reimplemented by every
/// host that asks it.
#[test]
fn a_field_answers_absence_by_its_type_or_its_annotation() {
    let source = "event @order.placed {
  order_id: Uuid,
  note: String @absent(\"\"),
  memo: String?,
  channel: String,
}
";
    let program = parse(source).expect("parses");
    let event = program
        .event(&EventPath::new(["order", "placed"]))
        .expect("declared");
    let answers = |name: &str| event.field(name).expect("declared").answers_absence();

    assert!(answers("note"), "`@absent` answers it");
    assert!(answers("memo"), "an optional type answers it on its own");
    assert!(!answers("channel"), "a bare required field answers nothing");
    assert!(!answers("order_id"), "nor does the id");
}

/// The mirror of an entity column's refusal of `= none`: a payload that predates an
/// optional field already reads as `none`, so an absence value would be a second
/// spelling of an answer the type gives for free.
#[test]
fn an_optional_field_takes_no_absent_value() {
    for source in [
        "event @a.b { id: Int, note: String? @absent(\"\") }",
        "record R { note: String? @absent(\"\") }",
    ] {
        let message = parse(source)
            .expect_err("an optional field already reads as `none`")
            .text();
        assert_eq!(
            message,
            "`note` is optional, so a payload that predates it already reads as `none`; \
             `@absent` is for a field that stays required",
            "for: {source}"
        );
    }
}

/// Rule 12: a seal holds ciphertext and the language cannot produce any, so there is no
/// literal that could stand in for one. Order-independent, because the two annotations
/// are one declaration however they are written.
#[test]
fn a_sealed_field_takes_no_absent_value() {
    for source in [
        "event @a.b { id: Int, email: String @subject(id) @absent(\"x\") }",
        "event @a.b { id: Int, email: String @absent(\"x\") @subject(id) }",
    ] {
        let message = parse(source)
            .expect_err("sealed content has no plaintext literal")
            .text();
        assert_eq!(
            message,
            "`@absent` on `email`, which is sealed under `id`; sealed content has no plaintext \
             literal: make the field optional instead",
            "for: {source}"
        );
    }
}

/// `@max` is enforced where a value is written, and a value this annotation produces is
/// never written: it is read out of a payload that does not hold it. Parse time is the
/// only place the bound can be applied to it at all.
#[test]
fn an_absent_value_is_bounded_by_max() {
    for source in [
        "event @a.b { id: Int, note: String @max(3) @absent(\"far too long\") }",
        "event @a.b { id: Int, note: String @absent(\"far too long\") @max(3) }",
        "record R { note: String @absent(\"far too long\") @max(3) }",
    ] {
        let message = parse(source)
            .expect_err("an absent value past the field's own bound")
            .text();
        assert_eq!(
            message, "`note` is bounded at 3 and its absent value is 12 long",
            "for: {source}"
        );
    }
    parse("event @a.b { id: Int, note: String @max(12) @absent(\"just about fits\") }")
        .expect_err("fifteen characters is still past twelve");
    parse("event @a.b { id: Int, note: String @max(12) @absent(\"fits\") }")
        .expect("a value inside the bound is fine");
}

/// A literal that cannot be the field's type is the same error it is anywhere else, and
/// it names the annotation so the message is about what was written.
#[test]
fn an_absent_value_of_the_wrong_type_is_rejected() {
    let cases = [
        (
            "event @a.b { note: String @absent(none) }",
            "a String absent value cannot be `none`",
        ),
        (
            "event @a.b { note: String @absent([1]) }",
            "a String absent value cannot be a list",
        ),
        (
            "event @a.b { note: Json @absent(\"x\") }",
            "a Json absent value cannot be a String",
        ),
    ];
    for (source, expected) in cases {
        let message = parse(source)
            .expect_err("a literal that is not of the field's type")
            .text();
        assert_eq!(message, expected, "for: {source}");
    }
}

/// Two event types whose field reads back differently from a payload that predates it
/// are not one field, so an arm listing both cannot bind the name: it would get whichever
/// event happened to arrive.
#[test]
fn an_arm_over_two_events_shares_a_field_only_when_its_absent_value_agrees() {
    let shared = "event @a.one { id: Int, note: String @absent(\"x\") }
event @a.two { id: Int, note: String @absent(\"x\") }
";
    let split = "event @a.one { id: Int, note: String @absent(\"x\") }
event @a.two { id: Int, note: String @absent(\"y\") }
";
    let arm = "effect E {
  on @a.one, @a.two as e { @key id } {
    log(\"{e.note}\")
  }
}
";
    parse(&format!("{shared}{arm}")).expect("one field, so the bind resolves");
    let message = parse(&format!("{split}{arm}"))
        .expect_err("two readings of `note` are not one field")
        .text();
    assert_eq!(
        message,
        "`note` is not shared by @a.one, @a.two, so an arm listing them cannot name it; \
         a binding names only what every listed type has, and reads it the same way: the same \
         type, the same `@subject` and the same `@absent`"
    );
}

/// A record field's `@absent` is located in the pass that fills record bodies and read
/// once they are all in, so a literal here may name a record declared further down. Read
/// eagerly, a forward reference resolved against a shell with no fields yet: the
/// every-field check iterated an empty list and passed, and the stored literal would have
/// built a record value with nothing in it.
#[test]
fn a_record_field_absent_value_names_a_record_declared_later() {
    let ordered = "record Note { kind: String }
record Wrap { note: Note @absent(Note { kind: \"none\" }) }
event @a.b { id: Int, wrap: Wrap }
";
    let reversed = "record Wrap { note: Note @absent(Note { kind: \"none\" }) }
record Note { kind: String }
event @a.b { id: Int, wrap: Wrap }
";
    for source in [ordered, reversed] {
        let program = parse(source).expect("declaration order must not matter");
        let wrap = program.record("Wrap").expect("declared");
        assert_eq!(
            wrap.field("note").and_then(|field| field.absent.clone()),
            Some(Literal::Record {
                ty: "Note".into(),
                fields: vec![("kind".into(), Literal::Str("none".into()))],
            }),
            "for: {source}"
        );
    }

    // And the every-field check fires whichever way round they are written.
    for source in [
        "record Note { kind: String }\nrecord Wrap { note: Note @absent(Note {}) }",
        "record Wrap { note: Note @absent(Note {}) }\nrecord Note { kind: String }",
    ] {
        let message = parse(source)
            .expect_err("a record literal names every field")
            .text();
        assert_eq!(message, "record `Note` needs `kind`", "for: {source}");
    }
}

/// The other thing reading it eagerly could not see: a `const`'s shell is collected after
/// record bodies, so a const in a record field's `@absent` used to be reported as a type
/// error about a value the parser had not read yet. An event field never had the problem,
/// and the two must not disagree.
#[test]
fn an_absent_value_may_be_a_const_wherever_it_is_written() {
    let source = "const NONE_GIVEN: String = \"none given\"

record Note { body: String @absent(NONE_GIVEN) }

event @a.b {
  id: Int,
  note: String @absent(NONE_GIVEN),
  detail: Note,
}
";
    let program = parse(source).expect("a const resolves in either position");
    let expected = Some(Literal::Str("none given".into()));
    assert_eq!(
        program
            .record("Note")
            .and_then(|def| def.field("body"))
            .and_then(|field| field.absent.clone()),
        expected
    );
    assert_eq!(
        program
            .event(&EventPath::new(["a", "b"]))
            .and_then(|def| def.field("note"))
            .and_then(|field| field.absent.clone()),
        expected
    );
}

/// `@max` is declared per field, so a string inside a record is bounded by the record's
/// own declaration. An absent value is never written, so `interp::bounded` never sees it
/// and the nested bound would hold nowhere at all.
#[test]
fn an_absent_value_is_bounded_through_a_record_and_a_list() {
    let cases = [
        (
            "record N { kind: String @max(3) }\nevent @a.b { id: Int, n: N @absent(N { kind: \"far too long\" }) }",
            "`n.kind` is bounded at 3 and its absent value is 12 long",
        ),
        (
            "record N { kind: String @max(3) }\nevent @a.b { id: Int, n: List(N) @absent([N { kind: \"ok\" }, N { kind: \"far too long\" }]) }",
            "`n[1].kind` is bounded at 3 and its absent value is 12 long",
        ),
        (
            "record N { kind: String @max(3) }\nrecord W { n: N @absent(N { kind: \"far too long\" }) }\nevent @a.b { id: Int, w: W }",
            "`n.kind` is bounded at 3 and its absent value is 12 long",
        ),
    ];
    for (source, expected) in cases {
        let message = parse(source)
            .expect_err("a nested absent value past the nested bound")
            .text();
        assert_eq!(message, expected, "for: {source}");
    }

    parse("record N { kind: String @max(3) }\nevent @a.b { id: Int, n: N @absent(N { kind: \"ok\" }) }")
        .expect("a nested value inside the bound is fine");
}
