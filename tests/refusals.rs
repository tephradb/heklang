//! `docs/refusals.md` as executable tests. A refusal is declared so that its code and
//! its message have exactly one home: the drift this replaces was one code carrying two
//! different messages in one application, which nothing could catch.

use heklang::{MessagePart, Program, parse};

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
