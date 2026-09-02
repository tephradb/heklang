# Declarations

What a module may declare, and the name spaces those declarations live in. The rules about *loading*
several modules are in `docs/modules.md`; this file is about what a single declaration means. What
each handler kind then *does* is in `docs/commands.md`, `docs/projectors.md` and `docs/effects.md`.

## The three handler kinds have separate name spaces

`command`, `projector` and `effect` each have their own space. One program may hold a command, a
projector and an effect all called `Same`, and each kind still rejects its own duplicate.

This is not an accident of the implementation, so it is worth stating as a rule: **a name is looked
up in exactly one kind, or in none.**

| Reference | Resolves to |
| --- | --- |
| `invoke Name { .. }` | a command, and only a command |
| `guard Name { .. }` | a guard, and only a guard |
| `reject Name` | a refusal, and only a refusal |
| an event path | an event, never a handler |
| a projector's name | nothing in the language; it names a read model to a host |
| an effect's name | nothing in the language; it names a subscription to a host |

Only one of those three is reachable from source at all. A shared space would therefore buy no
disambiguation, because there is nothing to disambiguate: it would only force renames on programs
whose command and effect are two halves of the same feature and want the same name.

A real port hit this twice, renaming an effect to `TeardownShop` and another to `RecordWarrantySales`
purely to avoid a collision it assumed existed. Both renames were unnecessary. That is the cost of
leaving a rule implicit even when the implementation already has it right, which is why there is now
a test that fails if the spaces are ever merged.

A `fn` is in a fourth space, and it is the only one with a scope narrower than the program: a `fn`
declared inside an `effect` is visible in that effect and nowhere else, and may not take the name of
a module `fn`, which is in scope inside every effect. See `docs/functions.md`.

A `guard` is in a fifth, and it is the second one reachable from source. `guard Name { .. }` resolves
to a guard and nothing else, so a command and a guard may share a name for the same reason a command
and an effect may. See `docs/guards.md`.

A `refusal` is in a sixth, and the third reachable from source. `reject Name` resolves to a refusal
and nothing else. It is also the only declared name whose spelling leaves the program, since the
code a caller switches on is derived from it, which is why it is the only one whose shape is
constrained. See `docs/refusals.md`.

**Rejected: one flat space for all three.** The argument for it is that a reader seeing `Same` should
not have to ask which kind it is. But a reader never sees a bare `Same`: they see `invoke Same`, or a
`command Same` declaration, or an `effect Same` declaration. The kind is at every use site already.

## `else if`

An `else` may be followed by another `if` rather than a block, so a multi-way dispatch is a chain:

```
if kind == 1 {
  return
} else if kind == 2 {
  return invalid("two")
} else {
  emit @order.routed { order_id, kind }
}
```

Before this, the statement form required `{` after `else`, so the same dispatch nested one level per
arm and a six-way one was six levels deep at the last case. The expression form (`if c { a } else { b
}`) was already chain-friendly, so this only aligns the statement form with it.

The whole chain is **one** statement in the IR, which matters for the erase-last analysis in
`docs/effects.md` rule 9: an `else if` is a nested `Stmt::If` in the `otherwise` branch, so the join
that analysis performs is exactly the join the reader sees.

## `record`

```
record ProductApplicability {
  kind: Applicability,
  shopify_product_ids: List(Int),
}
```

A named product type at module scope. Unlike an entity it is an ordinary value: it can be a `fold`
type, an event field, a command or `fn` parameter, a return type, and the element of a `List` or
`Map`. Records serialise to JSON objects under `docs/effects.md` rule 8, so one goes straight into a
request body.

The literal is `Name { field: value }`, with the same bare-name shorthand `emit` and `put` already
use, and a field is read with `.field`. A field declared `T?` takes a bare `T` and wraps it, the same
rule that applies at every other declared position (`docs/optionals.md`). **Every field must be
given.** A record with a hole would need a zero for the missing one, and filling in part of a record
is what record update exists for, which is out.

Entities were the only product type before this, and they are the wrong shape for the job: an entity
is scoped to a projector and reachable only through `put` and `patch`, so a `Map(Uuid, Plan)` had
nothing to hold.

### `@max` on a record field

A `String` field takes `@max(n)`, the same annotation an event field and an entity column take:

```
record LineItem {
  id: Int,
  title: String @max(255),
  variant_title: String? @max(255),
}
```

It is checked wherever the record lands: at `emit`, where an over-length value is `Outcome::Invalid`,
and at a projector write, where it is a hard error, exactly as for a bare `String` field in either
place. The check reaches through containers, so a `List(LineItem)` bounds every element's `title`,
and the error names the path (`line_items[0].title`) rather than the record.

**This is the only place the bound can go, which is why it belongs here.** An entity column of record
type has nothing to bound: `@max` on it would apply to the row's whole record value rather than to a
string inside it. So without this a `String` inside a record is unbounded everywhere, and there is no
other declaration to move the constraint to.

`@max` names a length, so it applies to `String` and `String?` and nothing else. `@max` on an `Int`,
a container or a record is an error naming the type. That check is new here and applies to event
fields and entity columns too, where it used to be accepted and silently do nothing.

**Rejected: `@max` on the entity column instead.** It is where the constraint is enforced for a bare
string, so it looks like the consistent choice. It cannot express this one: the column's type is
`LineItem`, and there is no syntax for "the `title` inside it", nor should there be, because that is
the record's business rather than the column's.

**`@subject` is deliberately not here yet.** A record field cannot be subject-bound, so a record
cannot carry personal data through the crypto-shredding path. That is a real restriction rather than
an oversight of the same shape as `@max`: `docs/effects.md` rule 12 recovers subject-ness from the
schema path, and a record reached through a container has no path the parser can name. Nothing needs
it yet, so it stays recorded rather than designed.

### Reading `Name {` as a literal

A record literal is claimed only when `Name` is a declared record, is not shadowed by a local, and no
`if` or `for` header is waiting for its block. That last condition is the whole ambiguity: in
`if plan { ... }` the `{` opens a block, and in `let p = Plan { ... }` it opens a literal. The parser
tracks which it is, the same way it already tracks that a `{` is an HTTP body (`in_body`) and that
`.field` reads a stored row (`stored`). Inside parentheses the restriction lifts again, because there
a `{` cannot be the header's block.

**Rejected: a sigil** (`#Name { .. }`, `Name::{ .. }`). It removes the ambiguity outright, at the cost
of a character on every literal in a language that already reads `Name { .. }` in `emit` and `put`.
Three spellings of "here are some fields" would be worse than one rule about headers.

### Record update is deliberately absent

There is no `base with { field: value }`.

The evidence is from a real port, and it is the interesting kind: **not having it forced a better
design.** Where the original folded a dict of mutable per-plan fields, the port folds one map per
aspect (`plans`, `plan_status`, `archived`, `variants`). Each event writes only the aspect it
carries, so nothing read-modify-writes a record it did not build, which is the exact hazard hekla's
`deep-boundaries` test exists to catch. Four folds read better than one and are structurally safer.

If record update existed, the first version of that fold would have been the read-modify-write one,
and it would have looked fine.

## Module-scope `enum`

`enum` was previously collected only inside a projector, and `self.enums` was empty while event
declarations and command signatures were parsed. So an event field and a command parameter could not
have an enum type, and every set of allowed values had to be a `String`.

That is a hole `docs/projectors.md` rule 7 already names: the runtime validates membership on command
input but not on a projector write, and "that hole cannot exist here" was true of the read model and
false of the **event**. With `status: PlanStatus` the same type on the event, the parameter and the
column, a bad variant cannot reach any of the three.

A projector may still declare its own, and its own shadows the module's. That is the precedence a
local binding already has over a builtin name, so it is one rule rather than a new one.

## `const`

```
const WEBHOOK_ADDRESS: String = "https://webhooks.example.com"
const FREE_TIER_LIMIT: Int = 15
const NAMESPACE: Uuid = "6ba7b810-9dad-11d1-80b4-00c04fd430c8"
```

`const NAME: Type = <literal>`, restricted to literals and literal aggregates: a scalar, a list of
them, an empty map, an empty `Json` object, or a record whose fields are literals. Not an
expression.

The reason is the one an entity default already gives: **no expression arena hangs off a
declaration.** A declaration is read in an early pass, before any body exists, and a const holding an
expression would need a frame and an evaluation order that nothing else at that level has. The
restriction has a second benefit, which is that a const is inlined at every use rather than being a
runtime lookup.

`Map.empty` and `Json.empty` are spellable here for that same reason: both are literals in the IR
rather than calls, so they construct nothing and read nothing. It is also why both may be a `fold`
seed and an entity default.

### A string literal resolves against a `Uuid` or `Timestamp` target

There is no Uuid literal token, because a bare word of hex and dashes is not one and quoting it is
the only sane spelling. So the **target type** is what makes `"6ba7b810-..."` a `Uuid`, and it is
validated at parse time.

Without this a namespace constant was unspellable, and so was a nil uuid. It is the same
literal-inference rule `docs/literal-inference.md` already applies to numbers, applied to one more
literal shape: one token, typed by where it lands.

The rule holds at **every** position with a target type, not only in a `const` or an entity default,
which is where it used to stop. A `test` is what found that: a suite is mostly ids, and
`run Ship { order_id: "0190d1a1-..." }` had no way to be written.

**`Timestamp` follows it, for a reason that was found the same way.** A moment had no literal at all:
the only ways to reach one were `now()`, `e.at` and `Timestamp.parse(text)`, and the last returns a
`Timestamp?` that nothing can unwrap into a required field. So an event carrying a `processed_at:
Timestamp` could not be written in a `given`, a `Timestamp` column could not be given the default
that `docs/projectors.md`'s zero rule tells it to have, and the advice that rule prints named an
escape the language did not offer.

```
const LAUNCH: Timestamp = "2026-01-01T00:00:00Z"

given @order.paid { order_id: 1, processed_at: "2026-02-02T02:02:02Z" }
```

The string is RFC 3339 and carries an offset, checked by the same reading `Timestamp.parse` does. The
two are one function met twice: a literal is text the author wrote, so it is checked now; `parse` is
text a webhook sent, so it is checked then and returns an optional. That is exactly the split `Uuid`
already has between a literal and `text.to_uuid()`.

### A const may name another const

```
const NAMESPACE: Uuid = "6ba7b810-9dad-11d1-80b4-00c04fd430c8"
const NO_PLAN: Uuid   = NAMESPACE
```

Order is irrelevant, in the sense it is irrelevant for every other declaration: the second may be
written above the first, or in another file entirely. What is inlined at the use site is still a
`Literal`, so nothing about "no expression arena hangs off a declaration" changes. A name here
resolves to a value at parse time rather than standing for a lookup at run time.

That takes one pass more than a const used to need. **C0** reads every const's name, its type and
the position of its value, and nothing else. The values are then resolved on demand, so parsing one
const's value may pause to parse another's and come back, and memoising on the way out is what makes
a const named ten times cost one parse.

A const that names itself, directly or through any chain, is rejected by naming the chain:

```
`A` names `B` names `A`: a `const` cannot name itself, directly or through another,
so that every const has a value
```

**Rejected: resolving in declaration order.** The smaller change, and the one that does not work. A
const's dependencies are only visible once its value has been parsed, so there is no order to sort
into before reading, and reading is the thing that needs the order. A port of a real application is
where this showed: the file naming a constant sorted twenty-two files ahead of the one declaring it,
and nothing in either file said so.

**Rejected: an expression arena for constants.** It would make `const B: Int = A + 1` legal along
with the plain reference, and it gives up both properties the literal restriction buys. A reference
is the part authors reach for; arithmetic over constants is not.

### An optional const is written `none` or a value

```
const NO_SKU: String?    = none
const HOUSE_SKU: String? = "house"
```

Both spellings work. The second is the one rule `docs/optionals.md` states for every position
that declares a type: a bare `T` fills a `T?`, wrapped once at the outside.

Until this an optional-typed const had **no writable value at all**, neither `none` nor a present
one, which meant no record with an optional field could be a const either, since a record const
resolves each field against its declared type. `Literal::None` had been in the IR the whole time
with nothing able to reach it.

An entity field is the one place `= none` stays rejected, and the reason does not carry over: an
optional column already starts at `none`, so writing it is a second spelling of the zero. A present
default there (`note: String? = "x"`) is accepted like anywhere else.

## Six passes

Declaration order does not matter anywhere, which now takes six passes over the token stream rather
than two. Each does only what the pass before it made possible:

| Pass | Reads | Because |
| --- | --- | --- |
| A | `enum` bodies, `record` names | a record field may name an enum, and a record may name a record |
| B | `record` fields | every type they might name now has a name |
| C0 | `const` names and types, and where each value starts | a const value may name a const declared later |
| C | `event`, `refusal`, `projector` shells, `command` and `guard` signatures | these name enums, records and consts |
| D | `command`, `guard` and `projector` bodies, `effect` helpers and arms | these name everything |
| E | `test` bodies | a test names commands, projectors and effects rather than declaring any |

Two boundaries are about something other than types. **C0** is about a declaration whose value can
name a sibling, so every name has to exist before any value is read, and the values are then
resolved on demand rather than in order. **E** is about a whole declaration: a `test` states what a
command does, which projector to fold and which effect to drive, so it needs all three collected
before it can resolve one, and pass D is still collecting them while it runs.

An `effect` is the one item that sweeps its own body twice inside pass D: once for its helpers'
signatures and once for their bodies and its arms, so a helper may be declared below the arm that
calls it. A `projector` does the same for its own enums in pass C. Both are the local form of the
same rule: order does not matter, so nothing may need to have been read yet.

Skipping an item is cheap, so six passes cost little, and each boundary has one reason rather than
being where the code happened to stop. `docs/modules.md` covers what this means across files.
