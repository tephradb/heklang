//! `docs/refusals.md` as executable tests. A refusal is declared so that its code and
//! its message have exactly one home: the drift this replaces was one code carrying two
//! different messages in one application, which nothing could catch.

use heklang::{Interpreter, MessagePart, Outcome, Program, Value, parse};

fn program(source: &str) -> Program {
    parse(source).unwrap_or_else(|err| panic!("expected this to parse: {err}"))
}

fn err(source: &str) -> String {
    parse(source)
        .expect_err("expected this to be rejected")
        .text()
}

// ---------------------------------------------------------------------------------
// Declaring.

#[test]
fn a_refusal_declares_a_code_and_a_message() {
    let program = program("refusal ShopNotFound \"shop does not exist\"\n");
    let def = program.refusal("ShopNotFound").expect("declared");
    assert_eq!(def.code, "shop_not_found");
    assert!(def.params.is_empty());
    assert_eq!(
        def.message,
        vec![MessagePart::Text("shop does not exist".to_string())]
    );
}

/// The code is what reaches `Outcome::Reject` and so what a caller outside the program
/// branches on. Deriving it rather than writing it is what keeps the two from drifting,
/// and every code in the two ported applications already had exactly this shape.
#[test]
fn a_code_is_the_name_in_snake_case() {
    let program = program(
        "refusal ShopNotFound \"a\"
refusal SkuTaken \"b\"
refusal TooManyOpen \"c\"
refusal NoActiveSubscription \"d\"
",
    );
    let codes: Vec<&str> = program
        .refusals
        .iter()
        .map(|def| def.code.as_str())
        .collect();
    assert_eq!(
        codes,
        [
            "shop_not_found",
            "sku_taken",
            "too_many_open",
            "no_active_subscription"
        ]
    );
}

/// Parens declare and braces use, the same split `command Foo(..)` and `invoke Foo { .. }`
/// have. The message holds the fields as holes rather than as text, so a use site fills
/// them from its own scope.
#[test]
fn a_refusal_takes_fields_its_message_names() {
    let program = program(
        "refusal SkuTaken(sku: String, item: Uuid) \"sku {sku} already belongs to item {item}\"\n",
    );
    let def = program.refusal("SkuTaken").expect("declared");
    assert_eq!(def.params.len(), 2);
    assert_eq!(def.params[0].name, "sku");
    assert_eq!(def.params[1].name, "item");
    assert_eq!(
        def.message,
        vec![
            MessagePart::Text("sku ".to_string()),
            MessagePart::Param(0),
            MessagePart::Text(" already belongs to item ".to_string()),
            MessagePart::Param(1),
        ]
    );
}

#[test]
fn a_refusal_counts_in_what_check_reports() {
    let program = program("refusal ShopNotFound \"a\"\nrefusal SkuTaken \"b\"\n");
    assert_eq!(program.refusals.len(), 2);
}

// ---------------------------------------------------------------------------------
// What a refusal may not be.

/// A message reads its own fields and nothing else. It has no scope to evaluate an
/// expression in, and a message that could name a const or a local would stop being a
/// function of the fields the caller was handed.
#[test]
fn a_message_names_only_its_own_fields() {
    let message = err("refusal SkuTaken(sku: String) \"sku {nope} is taken\"\n");
    assert!(
        message.contains("refusal `SkuTaken` has no field `nope`"),
        "got: {message}"
    );
    assert!(
        message.contains("names its own fields and nothing else"),
        "got: {message}"
    );
}

#[test]
fn a_refusal_needs_a_message() {
    let message = err("refusal ShopNotFound\nevent @a.b { id: Uuid }\n");
    assert!(
        message.contains("refusal `ShopNotFound` needs a message"),
        "got: {message}"
    );
}

#[test]
fn a_refusal_is_declared_once() {
    let message = err("refusal ShopNotFound \"one\"\nrefusal ShopNotFound \"two\"\n");
    assert!(
        message.contains("refusal `ShopNotFound` is declared twice"),
        "got: {message}"
    );
}

/// A refusal's name is the one name in heklang whose spelling leaves the program, since
/// it derives the code a caller switches on. Both rules exist to keep that derivation
/// reversible: with them, two names can never arrive as one code.
#[test]
fn a_refusal_is_named_like_a_type() {
    let message = err("refusal shopNotFound \"gone\"\n");
    assert!(
        message.contains("named like a type, so `shopNotFound` starts with a capital"),
        "got: {message}"
    );
    assert!(
        message.contains("`ShopNotFound` is `shop_not_found`"),
        "got: {message}"
    );
}

#[test]
fn a_name_holds_no_underscore() {
    let message = err("refusal Shop_Not_Found \"gone\"\n");
    assert!(
        message.contains("has no `_`, and `Shop_Not_Found` has one"),
        "got: {message}"
    );
}

/// Not a lint. A field the message never names would be evaluated at the use site into
/// an expression nothing references, so every check that walks the tree, the seal rules
/// among them, would step straight over it.
#[test]
fn every_field_is_named_by_the_message() {
    let message = err("refusal SkuTaken(sku: String, item: Uuid) \"sku {sku} is taken\"\n");
    assert!(
        message.contains("refusal `SkuTaken` declares `item` and never says it"),
        "got: {message}"
    );
}

// ---------------------------------------------------------------------------------
// Where a refusal ends. It has no braces, so the passes that skip it have to stop at
// the message the way they stop at a `const`'s literal; getting this wrong swallows the
// next declaration silently, which is what happened to `guard` in commit 6535551.

#[test]
fn a_const_above_a_refusal_does_not_swallow_it() {
    let program = program(
        "const LIMIT: Int = 2
refusal FreeLimit(limit: Int) \"the free tier lists {limit} items\"
event @a.b { id: Uuid }
",
    );
    assert!(program.refusal("FreeLimit").is_some());
    assert_eq!(program.consts.len(), 1);
    assert_eq!(program.events.len(), 1);
}

#[test]
fn a_refusal_does_not_swallow_what_follows_it() {
    let program = program(
        "event @a.b { id: Uuid }
refusal ShopNotFound \"gone\"
const LIMIT: Int = 2
command One(id: Uuid) { emit @a.b { id } }
",
    );
    assert!(program.refusal("ShopNotFound").is_some());
    assert_eq!(program.events.len(), 1);
    assert_eq!(program.consts.len(), 1);
    assert_eq!(program.commands.len(), 1);
}

/// A refusal is nameable before it is declared, like everything else, which is why it is
/// collected in a pass above the bodies that name it.
#[test]
fn order_on_the_page_is_irrelevant() {
    let program = program(
        "command One(id: Uuid) { emit @a.b { id } }
event @a.b { id: Uuid }
refusal ShopNotFound \"gone\"
",
    );
    assert!(program.refusal("ShopNotFound").is_some());
    assert_eq!(program.commands.len(), 1);
}

// ---------------------------------------------------------------------------------
// Using. `reject <Name>` is the only way to refuse, and the code and message it lands
// with are the declaration's.

const USED: &str = "refusal ShopNotFound \"shop does not exist\"
refusal SkuTaken(sku: String, item: Uuid) \"sku {sku} already belongs to item {item}\"
event @item.listed { item_id: Uuid, seller_id: Int, sku: String }
";

fn used(body: &str) -> String {
    format!("{USED}command List(item_id: Uuid, seller_id: Int, sku: String) {{\n{body}\n}}\n")
}

const ITEM: &str = "0190d1a1-0000-7000-8000-000000000001";

fn refused(body: &str) -> Outcome {
    let program = program(&used(body));
    let mut interpreter = Interpreter::new(&program);
    interpreter
        .run(
            "List",
            [
                ("item_id", Value::uuid(ITEM)),
                ("seller_id", Value::Int(1)),
                ("sku", Value::str("a")),
            ],
        )
        .expect("ran")
        .outcome
}

/// The wire code is the derivation, not the name. Every code in the two applications
/// ported so far already had this shape, so nothing a caller switches on moved.
#[test]
fn the_code_that_reaches_a_caller_is_the_derived_one() {
    let outcome = refused("  return reject ShopNotFound");
    let Outcome::Reject { code, message } = outcome else {
        panic!("expected a refusal, got {outcome:?}");
    };
    assert_eq!(code, "shop_not_found");
    assert_eq!(message, "shop does not exist");
}

/// Braces use, so the fields are written the way `emit` and `invoke` write them, and the
/// message is built from what the caller passed rather than restated at the site.
#[test]
fn the_message_is_built_from_the_fields_the_caller_gave() {
    let outcome = refused("  return reject SkuTaken { sku, item: item_id }");
    let Outcome::Reject { code, message } = outcome else {
        panic!("expected a refusal, got {outcome:?}");
    };
    assert_eq!(code, "sku_taken");
    assert_eq!(message, format!("sku a already belongs to item {ITEM}"));
}

#[test]
fn a_refusal_must_be_declared() {
    let message = err(&used("  return reject Nmae"));
    assert!(
        message.contains("refusal `Nmae` is not declared"),
        "got: {message}"
    );
}

#[test]
fn a_refusal_takes_every_field_and_no_others() {
    let missing = err(&used("  return reject SkuTaken { sku }"));
    assert!(
        missing.contains("refusal `SkuTaken` needs `item`"),
        "got: {missing}"
    );

    let unknown = err(&used(
        "  return reject SkuTaken { sku, item: item_id, nope: 1 }",
    ));
    assert!(
        unknown.contains("refusal `SkuTaken` has no field `nope`"),
        "got: {unknown}"
    );

    let twice = err(&used(
        "  return reject SkuTaken { sku, sku: \"b\", item: item_id }",
    ));
    assert!(twice.contains("`sku` is given twice"), "got: {twice}");
}

/// A refusal with no fields takes no braces, which is also what lets `return reject Name`
/// be the last statement in a block without the closing `}` being read as its fields.
#[test]
fn a_refusal_with_no_fields_takes_no_braces() {
    let message = err(&used("  return reject ShopNotFound { x: 1 }"));
    assert!(
        message.contains("refusal `ShopNotFound` has no fields, so it takes no braces"),
        "got: {message}"
    );
    assert!(matches!(
        refused("  if true {\n    return reject ShopNotFound\n  }\n  return"),
        Outcome::Reject { .. }
    ));
}

/// The form this replaces. People will type it, so the message names the declaration to
/// write, reading the literals it was given to say it concretely.
#[test]
fn the_old_call_form_says_what_to_write_instead() {
    let message = err(&used(
        "  return reject(\"customer_blocked\", \"this customer cannot place orders\")",
    ));
    assert!(
        message.contains("`reject` names a declared refusal, so it takes no code and no message"),
        "got: {message}"
    );
    assert!(
        message.contains(
            "declare `refusal CustomerBlocked \"this customer cannot place orders\"` at module scope"
        ),
        "got: {message}"
    );
    assert!(
        message.contains("then write `reject CustomerBlocked`"),
        "got: {message}"
    );
}

/// The reason a refusal is worth declaring on the consuming side too: a bare name in a
/// `String` position is the code, so a comparison against one is checked where the string
/// it replaces was checked by nobody.
#[test]
fn a_name_in_a_string_position_is_the_code() {
    let source = "refusal ShopNotFound \"shop does not exist\"
event @order.placed { order_id: Uuid, customer_id: Int }
command Inner(order_id: Uuid, customer_id: Int) {
  return reject ShopNotFound
}
effect E {
  on @order.placed as e { order_id, customer_id } {
    let r = invoke Inner { order_id, customer_id }
    if r.code().unwrap_or(\"\") == ShopNotFound {
      log(\"refused\")
    }
  }
}
";
    program(source);

    let typo = source.replace("== ShopNotFound", "== ShopNotFund");
    let message = err(&typo);
    assert!(
        message.contains("`ShopNotFund` is not in scope"),
        "got: {message}"
    );
}
