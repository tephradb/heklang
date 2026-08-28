# Declarations

What a module may declare, and the name spaces those declarations live in. The rules about *loading*
several modules are in `docs/modules.md`; this file is about what a single declaration means.

## The three handler kinds have separate name spaces

`command`, `projector` and `effect` each have their own space. One program may hold a command, a
projector and an effect all called `Same`, and each kind still rejects its own duplicate.

This is not an accident of the implementation, so it is worth stating as a rule: **a name is looked
up in exactly one kind, or in none.**

| Reference | Resolves to |
| --- | --- |
| `invoke Name { .. }` | a command, and only a command |
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

A named product type at module scope. Unlike an entity it is an ordinary value: it can be a `state`
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
them, an empty map, or a record whose fields are literals. Not an expression.

The reason is the one an entity default already gives: **no expression arena hangs off a
declaration.** A declaration is read in an early pass, before any body exists, and a const holding an
expression would need a frame and an evaluation order that nothing else at that level has. The
restriction has a second benefit, which is that a const is inlined at every use rather than being a
runtime lookup.

### A string literal resolves against a `Uuid` target

There is no Uuid literal token, because a bare word of hex and dashes is not one and quoting it is
the only sane spelling. So the **target type** is what makes `"6ba7b810-..."` a `Uuid`, and it is
validated at parse time.

Without this a namespace constant was unspellable, and so was a nil uuid. It is the same
literal-inference rule `docs/literal-inference.md` already applies to numbers, applied to one more
literal shape: one token, typed by where it lands.

## Four passes

Declaration order does not matter anywhere, which now takes four passes over the token stream rather
than two. Each does only what the pass before it made possible:

| Pass | Reads | Because |
| --- | --- | --- |
| A | `enum` bodies, `record` names | a record field may name an enum, and a record may name a record |
| B | `record` fields | every type they might name now has a name |
| C | `event`, `projector` shells, `command` signatures, `const` | these name enums and records |
| D | `command` bodies, `projector` handlers, `effect` arms | these name everything |

Skipping an item is cheap, so four passes cost little, and each boundary has one reason rather than
being where the code happened to stop. `docs/modules.md` covers what this means across files.
