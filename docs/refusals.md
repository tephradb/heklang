# Refusals

A `refusal` is a named reason a command said no, with the fields its message needs and the
message itself:

```
refusal ShopNotFound "shop does not exist"
refusal SkuTaken(sku: String, item: Uuid) "sku {sku} already belongs to item {item}"

command ListItem(item_id: Uuid, seller_id: Int, sku: String) {
  guard ShopIsConnected { seller_id }

  fold items: Map(Uuid, Item) = Map.empty
    on @item.listed(seller_id) { item_id, item } => items.set(item_id, item)

  for other_id, other in items {
    if other.sku == sku {
      return reject SkuTaken { sku, item: other_id }
    }
  }

  emit @item.listed { item_id, seller_id, sku }
}
```

This document is the contract. `tests/refusals.rs` is the same set of rules as executable
tests. Change the doc, the tests and the code together.

## The gap it closes

`reject("code", "message")` was two strings, positional and unchecked. Nothing stopped the
arguments being swapped, nothing caught a typo, and nothing kept one code's message the same
in two places. Across the three applications ported so far, 75 refusals carried 23 distinct
codes, and the predicted damage was already there: `plan_not_found` carried both "warranty
plan does not exist" and "warranty plan not found", because it was written out five separate
times.

Every code was a string literal, so nothing was bought by them being strings. Four messages
of the 75 interpolated anything, so nothing was bought by them being expressions.

The code is also an API. `Invoked::code()` hands it to a caller, so
`if r.code().unwrap_or("") == "sku_takn"` compiled, never matched, and never warned.

## Declaring

```
refusal <Name>[(<field>: <Type>, ...)] "<message>"
```

**Parens declare and braces use**, the rule `docs/guards.md` states for `command Foo(...)`
against `invoke Foo { ... }`. A refusal with no fields declares no parens and is written with
no braces, which is the common case: 19 of the 23 codes in the corpus take nothing.

**The message may name the refusal's own fields and nothing else.** It is not an expression
in a scope; it is text and holes, and the holes are filled at the use site. That is what makes
the message a function of the fields a caller was handed, which is the whole reason to declare
it rather than write it at each site.

A const in a message goes through a field:

```
refusal FreeLimit(limit: Int) "the free tier lists {limit} items"

return reject FreeLimit { limit: FREE_LIMIT }
```

One clause at one site, and it says where the number came from. The alternative, allowing a
const directly, would give the message a third input that neither the declaration's parens nor
the use site's braces mention.

**Every field must be named by the message**, and that is an error rather than a lint:

> refusal `SkuTaken` declares `item` and never says it
> = the message is the only thing a refusal's fields feed, so one the message does not name
>   could never be read

The message is the only thing a field feeds, so a field the message skips would be evaluated
at the use site into an expression nothing references, and every check that walks the tree,
the seal rules among them, would step over it. Unreachable rather than unused.

## The code is derived

`ShopNotFound` is `"shop_not_found"`. Insert `_` before each capital after the first,
lowercase the rest.

This is the one name in heklang whose spelling leaves the program: it becomes a string a
client switches on. Every other name is internal. So two rules keep the derivation reversible,
and with them two names can never arrive as one code:

> a refusal is named like a type, so `shopNotFound` starts with a capital
> a refusal's name has no `_`, and `Shop_Not_Found` has one

Deriving rather than writing is what stops the name and the code drifting. It was also free:
all 23 codes in the two ported applications are already exactly the derivation of their
natural name, so `Outcome::Reject { code, .. }` carries the same strings it always did and no
caller outside the program saw this change at all.

## Using

```
reject <Name>
reject <Name> { <field>: <value>, ... }
```

Braces take the same bare-name shorthand every other block does, so `reject SkuTaken { sku }`
is `{ sku: sku }`. Every field, always, checked against the declaration:

> refusal `SkuTaken` needs `item`
> refusal `SkuTaken` has no field `nope`
> `sku` is given twice

A refusal with no fields takes no braces:

> refusal `ShopNotFound` has no fields, so it takes no braces

That is not only tidiness. It is what lets `return reject ShopNotFound` be the last statement
in a block without the closing `}` being read as its field list.

**`return` stays.** `fail` is written bare in an effect because an effect arm has no result
channel and there is nothing to return; a command has one, and `return` is it. `reject` also
has to remain a value, because a `fn` declared `-> Outcome?` decides a refusal and hands it
back (`docs/functions.md`), so a bare statement form would be a second spelling rather than a
replacement: the same word meaning "exit here" in a command and "this is my result" in a `fn`.

**Where it may be written** is unchanged: a command, a guard, and a `fn` that declared
`Outcome`. An effect's terminal outcome is still `fail`, and a projector still has no failure
channel at all.

## Reading one back

A bare refusal name in a `String` position is its code, which makes the consuming side
checked too:

```
let r = invoke ListItem { item_id, seller_id, sku }
if r.code().unwrap_or("") == ShopNotFound {
  log("the shop went away")
}
```

Or, asking the question directly:

```
if r.refused(ShopNotFound) {
  log("the shop went away")
}
```

Either way a typo is now `` `ShopNotFund` is not in scope ``, where the string form was checked
by nobody.

`refused` declares a `String` parameter, and that is the whole mechanism: the method table's
parameter type is the hint every argument is parsed against, so a bare refusal name resolves to
its code there exactly as it does in a comparison, and nothing in the parser knows this method
exists. It also means a literal is still accepted and still unchecked. What the name buys is that
a misspelled one is a parse error rather than a branch that never runs.

**`invalid` is refused by nothing.** It carries no code, and the question `refused` asks is "did
it refuse with this one"; a malformed request did not refuse at all, so the answer is `false`
whichever refusal is named.

`.code()` is a `String?` and `T?` does not fill a `T` (`docs/types.md`), so the `unwrap_or` is
load-bearing rather than habit: `r.code() == ShopNotFound` is
`` cannot apply `==` to String? and String ``, exactly as `r.code() == "shop_not_found"` was
before this existed. Nothing about refusals bends the optional rule.

## What this deliberately does not do

- **It does not put the fields on the wire.** `Outcome::Reject` still carries a code and a
  rendered message, which is what kept every host unchanged. Sending the fields as data is a
  real option and a separate one; it changes an API that reaches outside this repository.
- **It does not touch `invalid`.** `docs/commands.md` argues that it carries no code because
  there is nothing to branch on when the answer is "you sent nonsense", and the asymmetry
  stays: `invalid(message)` is still a message and nothing else.

## Related

- `docs/commands.md`: the three outcomes, and why `invalid` is about the request while a
  refusal is about the world.
- `docs/guards.md`: the other half of a refusal ladder. A guard names the proposition and a
  refusal names the failure, so `CourseIsDefined` refuses with `UndefinedCourse`.
- `docs/functions.md`: a `fn` declared `-> Outcome?`, which is how two commands share one
  ladder.
- `docs/testing.md`: `expect reject <Name>`, which no longer restates the message.
- `docs/declarations.md`: the separate name spaces, and the pass a refusal is collected in.
