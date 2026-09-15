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
      reject SkuTaken { sku, item: other_id }
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

The code is also an API. `Invoked::code()` hands it to a caller, so `if r.code() == "sku_takn"`
compiled, never matched, and never warned.

## Declaring

```
refusal <Name>[(<field>: <Type>, ...)] "<message>"
```

**Parens declare and braces use**, the rule `docs/guards.md` states for `command Foo(...)`
against `invoke Foo { ... }`. A refusal with no fields declares no parens and is written with
no braces, which is the common case: 19 of the 23 codes in the corpus take nothing.

A use site that reaches for the declaration's parens is told so by name, in both directions:
`reject SkuTaken(sku, item)` is `` refusal `SkuTaken` takes its fields in braces `` and hints
with the declaration's own field names, and `reject ShopNotFound(x)` is `` has no fields, so
it takes no parens ``. Neither used to say that. With fields declared the report was
`expected `{``, which is true and names no rule; with none it was `unreachable`, because
`reject ShopNotFound` is already a complete answer and `(x)` parsed as the statement after
it, so the message described the shape the parser reached rather than the one that was
written.

**The message may name the refusal's own fields and nothing else.** It is not an expression
in a scope; it is text and holes, and the holes are filled at the use site. That is what makes
the message a function of the fields a caller was handed, which is the whole reason to declare
it rather than write it at each site.

A const in a message goes through a field:

```
refusal FreeLimit(limit: Int) "the free tier lists {limit} items"

reject FreeLimit { limit: FREE_LIMIT }
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

That is not only tidiness. It is what lets `reject ShopNotFound` be the last statement
in a block without the closing `}` being read as its field list.

**There is no `return` in front of it**, and there are no parens after it. Those are the same
rule twice:

> `reject X`, `invalid "..."` and `fail "..."` each say what this declaration answers, and
> saying the answer ends the declaration.

One meaning in all three places a `reject` may be written. What `return` used to add was
nothing: in statement position `reject` can only be the answer, so the word carried no
information at the site heklang writes most often.

The argument this replaces said a bare form would be "a second spelling ... the same word
meaning 'exit here' in a command and 'this is my result' in a `fn`". That mis-attributed the
overload. In `return reject X` the operand `reject X` means the same thing in every
declaration; the token whose meaning changes with the enclosing declaration is `return`, which
already means "the command's outcome" in a command and "leave this helper" in an effect-local
`fn` (`docs/functions.md`). Two constructs had shipped the disputed shape years apart:
`invoke` is both a statement and an `Outcome` expression, and `fail` is a bare terminal
statement that crosses a call boundary from a helper.

**Parens are what a call takes, and a call comes back.** That is the second half, and it is why
`invalid` and `fail` lost theirs while `log`, `erase` and `reveal` keep them: those three
return to the next statement. The guarantee only runs one way, since `emit` and `put` take no
parens and are not terminal, so the check that a statement after an answer never runs is a
diagnostic rather than a shape a reader has to spot (`docs/diagnostics.md`).

**The message is written, and a value reaches it through a hole.** `invalid "{err}"`, never
`invalid err`: the operand of `invalid` and of `fail` is a string literal and nothing else,
which is the same sentence "a message and nothing else" says below about not giving `invalid`
a declaration. A name, a call, a `const` and `"a" + b` are all `return-shape`, and the hint
names the author's own identifier back in the hole.

This is the one rule the two tools used to disagree about. `tree-sitter-hek/grammar.js` has
always spelled the message `choice($.string, $.raw_string)`, and it has to: were it an
expression, the parens of a removed `invalid("x")` would read as a grouping and `hek fmt`
would rewrite the file to `invalid ("x")`, which is output of its own that `hek check`
rejects. The parser meanwhile took any expression with a `String` hint, so `invalid err`
passed `check` and its tests while `hek fmt` declined the whole file, and a migration off
`invalid(err)` writes exactly that on the first try. `Parser::answer_message` is where the two
now agree. It checks twice, because a `const` lowers to the literal it was declared with and
so builds the same node a written message does, while a written message is also how a longer
expression starts.

**Where it may be written** is unchanged: a command, a guard, and a `fn` that declared
`Outcome`. An effect's terminal outcome is still `fail`, and a projector still has no failure
channel at all.

**`reject` is not a value.** `let objection = reject SkuTaken { sku }` is refused, the way
`log` and `fail` are refused in a value position, because saying the answer ends the
declaration and there is nothing left for a binding to hold. A `fn` that declared `Outcome`
still decides a refusal and still hands it back; it writes the same bare statement, and the
answer travels the result channel every other `fn` result travels.

## Reading one back

A bare refusal name in a `String` position is its code, which makes the consuming side
checked too:

```
let r = invoke ListItem { item_id, seller_id, sku }
if r.code() == ShopNotFound {
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

`.code()` is a `String?`, and an equality takes one against a bare value (`docs/optionals.md`), so
there is nothing to unwrap: a call that did not refuse has no code, and no code is not any name. The
`unwrap_or("")` this used to need is still legal and now says nothing, and the empty string it
invented was a sentinel standing in for absence, which is what an optional is for.

Nothing about refusals bends the optional rule to get there. The hint reaches the bare name through
the optional, exactly as it reaches the `String` parameter `refused` declares, so `ShopNotFound`
resolves to its code in both and a typo is `` `ShopNotFund` is not in scope `` in both.

## What this deliberately does not do

- **It does not put the fields on the wire.** `Outcome::Reject` still carries a code and a
  rendered message, which is what kept every host unchanged. Sending the fields as data is a
  real option and a separate one; it changes an API that reaches outside this repository.
- **It does not give `invalid` a declaration.** `docs/commands.md` argues that it carries no
  code because there is nothing to branch on when the answer is "you sent nonsense", and the
  asymmetry stays: `invalid "<message>"` is a message and nothing else.

  The symmetry is tempting, because a `refusal` exists to stop one message being written twice
  and an `invalid` message is still the unchecked string that argument is about. What says no
  is that the two are different kinds of thing, and the corpus shows it: an `invalid` message
  interpolates a const where it stands, and a refusal's may not, because a refusal's message is
  text and holes rendered at a distance from fields a caller was handed. A declared name and a
  literal string is the right pair of spellings for that difference. Revisit it if a port ever
  writes the same `invalid` message twice; that is the bar `refusal` cleared with 75 sites over
  23 codes, and this has not.

## Related

- `docs/commands.md`: the three outcomes, and why `invalid` is about the request while a
  refusal is about the world.
- `docs/guards.md`: the other half of a refusal ladder. A guard names the proposition and a
  refusal names the failure, so `CourseIsDefined` refuses with `UndefinedCourse`.
- `docs/functions.md`: a `fn` declared `-> Outcome?`, which is how two commands share one
  ladder.
- `docs/testing.md`: `expect reject <Name>`, which no longer restates the message.
- `docs/declarations.md`: the separate name spaces, and the pass a refusal is collected in.
