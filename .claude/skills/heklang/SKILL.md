---
name: heklang
description: Write, review and debug heklang (.hk) modules, the total event-sourced module language for the hekla runtime. Covers events, commands, guards, refusals, projectors, effects, fn helpers, consts and tests, plus the hek check/test/fmt/digest tool. Use for any .hk file, for anything mentioning heklang, hekla, hek check, or event-sourcing handlers written as commands, projectors and effects.
---

# heklang

heklang is a small, **total** language for event-sourced application logic, and the module
language for [hekla](https://git.tqwewe.com/tephra/hekla). Five declaration kinds do the work:

- a **command** replays the history its decision depends on and appends events;
- a **projector** consumes events into a read model;
- an **effect** reacts to appended events with durable side effects, and is the only kind that
  reaches the network;
- a **guard** is a named proposition about the log that several commands share;
- a **refusal** is a named reason one said no.

**The restrictions are the point.** A command cannot call out, read a clock in a fold, or decrypt.
A projector has no failure channel and no general read. A fold cannot observe anything but the log.
Each holds because of what kind of declaration it is, not because something checks at run time.
Every program terminates: there is no `while`, recursion is rejected statically, and a `for` runs
once per element of a finite container.

## Run the checker. Always.

`hek` is the checker, test runner, formatter and digest tool, and it is the authority on every rule
below. Every
static check the language has lives in one pass, so "parses" and "checks" are the same thing, and it
reports every mistake rather than only the first.

```sh
hek check path/      # parse every .hk file under path as one program
hek test  path/      # the same, then run every `test` declaration
hek                  # both, on the current directory
hek fmt   path/      # rewrite canonically; --check makes it a gate
hek check --boundaries   # one line per command naming what it guards, transitively
hek digest --hash path/  # a hash of what the program does, for "did this meaningfully change?"
```

Install with `cargo binstall hek`, `cargo install hek`, or
`nix run git+https://git.tqwewe.com/tephra/heklang`. From a heklang checkout it is
`cargo run -p hek -- check path/`.

**Workflow for every change:** write or edit the `.hk` files, run `hek check`, fix what it names,
add or update a `test`, run `hek test`, then `hek fmt`. Never claim a module is correct without
having run it. Do not invent syntax; if a construct is not in this skill, it does not exist.

`hek digest` writes nothing and changes nothing: it renders what the program does with local names,
layout, comments, file boundaries and declaration order taken out, so two versions that behave the
same hash the same. Use it to answer whether an edit was a refactor or a change, never as a
substitute for `hek check`. `--packed` is the canonical form a tool stores; the default output is
the readable one.

**A green `hek check` is necessary and not sufficient.** What it cannot see is data-dependent: a
`Money` operation that cannot be answered exactly type-checks and then fails at run time naming
`mul` or `div`. So every command, projector handler and effect arm wants at least one `test`, and
`hek test` is the gate, not `hek check`.

## One directory is one program

Every `.hk` file under the path is one program. There is no import syntax, no manifest, no header
item, and **declaration order does not matter** anywhere, across files or within one. Event paths,
command, projector, effect, guard, refusal, record, enum, const, `fn` and test names are all global.
A module name is a label for diagnostics, not a namespace.

The only scoped names: entities and enums declared inside a projector belong to that projector, and
a `fn` declared inside an effect is visible only in that effect.

Directories beginning with `.` and any `target` directory are skipped.

## Pick the declaration first

| I need to | Write |
| --- | --- |
| record a fact that happened | `event` |
| decide from history whether a fact may be recorded, then record it | `command` |
| share one proposition-plus-refusal across commands | `guard` |
| name a reason a command refuses | `refusal` |
| maintain a queryable shape from the log | `projector` |
| call an HTTP service, invoke a command, or crypto-shred, in reaction to an event | `effect` |
| share pure logic between any of them | `fn` at module scope |
| share impure logic between arms of one effect | `fn` inside that `effect` |
| a named product type that travels as a value | `record` |
| a closed set of values | `enum` |
| a literal named once | `const` |
| state a case: a log, one action, expectations | `test` |

## What each kind may do

| | command | guard | projector | effect arm | effect-local `fn` | module `fn` | fold arm |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `fold` / `guard` decls | yes | yes (one read) | no | yes | no | no | n/a |
| `emit` | yes | no | no | no | no | no | no |
| `put`/`patch`/`update`/`delete` | no | no | yes | no | no | no | no |
| `http.*`, `invoke` | no | no | no | yes | yes | no | no |
| `log`, `fail` | no | no | no | yes | yes | no | no |
| `reveal`, `erase` | no | no | no | yes | **no** | no | no |
| `now()` | yes, pinned once | no | no | yes, journaled | no | no | no |
| `reject` / `invalid` | yes | yes, only these | no | no | no | only if `-> Outcome` | no |
| call a module `fn` | yes | yes | yes | yes | yes | yes | yes |
| call an effect-local `fn` | no | no | no | yes | yes | no | **no** |

A module `fn` is pure by construction. That is what lets a fold arm, a projector and a command all
call one. An effect-local `fn` may call out, which is why a fold arm may not call it.

## Syntax

### Types

`Bool`, `Int`, `Decimal(n)`, `Money(n)`, `String`, `Uuid`, `Timestamp`, `Json`, an `enum` name, a
`record` name, `List(T)`, `Map(K, V)`, and `T?` for the only absence there is.

`Response` and `Outcome` exist and are spellable **only** in a `fn` parameter or return type.
`Rounding` is spellable nowhere: it reaches `.mul` and `.div` as the bare words `HalfUp`, `HalfEven`
and `Down`. `Sealed(T, subject)` is derived from `@subject(...)` and is never written. A `Map` key
must be `Int`, `String`, `Uuid`, `Timestamp` or an enum. `Money(n)` and `Decimal(n)` cap at scale 18.

Type equality is exact and structural: `Money(2)` is not `Decimal(2)` is not `Money(3)`.

### Declarations

```hek
enum Status { @default Draft, Active, Archived }

record LineItem {
  sku: String @max(64),
  price: Money(2),
  tags: List(String),
}

const LIMIT: Int = 15
const NAMESPACE: Uuid = "6ba7b810-9dad-11d1-80b4-00c04fd430c8"
const LAUNCH: Timestamp = "2026-01-01T00:00:00Z"
const NO_SKU: String? = none

event @order.placed {
  order_id: Uuid,
  customer_id: Int,
  email: String @subject(customer_id) @max(200),
  total: Money(2),
  notes: String @no_index,
}

refusal TooManyOpen "too many open orders"
refusal SkuTaken(sku: String, item: Uuid) "sku {sku} already belongs to item {item}"

fn effective_sku(sku: String?, item_id: Uuid) -> String {
  let given = sku.unwrap_or("").trim()
  if given.is_empty() {
    return "CATALOG:{item_id}"
  }
  return given
}

guard UnderOpenOrderLimit(customer_id: Int) {
  fold open: Int = 0
    on @order.placed(customer_id) => open + 1
    on @order.cancelled(customer_id) => open - 1

  if open >= LIMIT {
    return reject TooManyOpen
  }
}

command PlaceOrder(order_id: Uuid, customer_id: Int, email: String, total: Money(2)) {
  guard UnderOpenOrderLimit { customer_id }

  emit @order.placed { order_id, customer_id, email, total, notes: "" }
}

projector Orders {
  entity Order {
    order_id: Uuid @key,
    customer_id: Int @index,
    total: Money(2),
    status: Status,
    tracking: String?,

    index (customer_id, status),
  }

  on @order.placed as e { order_id, customer_id, total } {
    put Order { order_id, customer_id, total, status: Draft, tracking: none }
  }
}

effect NotifyCustomer {
  fn send(url: String, to: String) {
    let response = http.post(url, { "to": to })
    if response.status >= 400 {
      fail("rejected with status {response.status}")
    }
  }

  on @order.placed as e { order_id, email } {
    fold orders: Int = 0
      on @order.placed(customer_id: e.customer_id) => orders + 1

    send("https://mail.example/confirm", reveal(email))
    log("confirmed order {order_id}")
    invoke RecordNotified { order_id, notification_id: Uuid.derive(e.id, "confirmation") }
  }
}

test "a first order is appended as written" {
  run PlaceOrder { order_id: ORDER, customer_id: 1, email: "a@b.com", total: 25.99 }

  expect @order.placed {
    order_id: ORDER,
    customer_id: 1,
    email: "a@b.com",
    total: 25.99,
    notes: "",
  }
}
```

Annotations, exhaustively: an **event field** takes `@subject(field)`, `@max(n)` and `@no_index`; an
**entity field** takes `@key`, `@index` and `@max(n)`, plus `= <literal>` for a default and an
entity-level `index (a, b)`; a **record field** takes `@max(n)`; an **enum variant** takes
`@default`. `@max` applies to `String` and `String?` and nothing else.

### Statements

```hek
let x = <expr>                          // no type annotation, immutable, no `var`
fold s: T = <seed>
  on @path(filters) { destructure } => <expr>
  on @other.path(filters) => <expr>
guard Name { arg }                      // named proposition
guard @order.placed(id), @order.cancelled(id)   // raw slices, binds nothing
if c { .. } else if d { .. } else { .. }
for item in list { .. }
for key, value in map { .. }            // or index, item over a list
return / return <expr> / return reject Name / return reject Name { field } / return invalid("msg")
emit @path { field, other: value }
put Entity { .. } / patch Entity[key] { .. } / update Entity[key] { .. } / delete Entity[key]
log("...") / fail("...") / erase(value) / erase(subject, value)
invoke Command { field: value }
helper(args)                            // a call to a void effect-local fn is a statement
```

### Expressions

```hek
42  10.50  true  none  "text {hole}"  """raw, no escapes, no interpolation"""
"6ba7b810-9dad-11d1-80b4-00c04fd430c8"    // a Uuid, when the target says so
"2026-01-01T00:00:00Z"                    // a Timestamp, when the target says so
Placed                                    // an enum variant, bare, resolved from the target
[a, b]   [x.title for k, x in m if x.live]   { "key": value }   Record { field: value }
if c { a } else { b }                     // value position
-x  !x  a + b  a == b  a && b  a < b
x.method(arg)  record.field  response.status  response.body  .stored_column
Uuid.derive(seed, name)  Json.empty  Json.encode(v)  Map.empty
Timestamp.parse(t)  Timestamp.from_parts(y, mo, d, h, mi, s)  Money.parse(t)  Decimal.parse(t)
http.get(url)  http.post(url, body, headers = { "K": "v" })  now()  reveal(x)
invoke C { .. }  reject Name  invalid("msg")
```

**Statements are separated by newlines. There are no semicolons anywhere in the language.**

There is no string `+`, no `str()`, no format specifiers, no indexing, no `while`, no `break`, no
`continue`, no set, no tuple, no closures, no generics, no overloading, no default or named
arguments to a `fn`, no `unwrap`, no `?` operator, no random and no `uuid4`.

A comment is `//` to the end of the line. Put one on its own line, leading whatever it describes:
`hek fmt` keeps a trailing comment where it is, but nothing in the corpus writes one.

## Rules that are easy to break

1. **`fold` is a read declaration, not a variable.** It names a slice of the log, and the slices a
   command folded *are* the condition its append is checked against. Anything not folded is a `let`.
2. **A stage is one read.** A run of `fold` and `guard` declarations is one pass over the log; any
   statement below them closes it. A seed or a filter may not name a `fold` beside it. Put a `let`
   between the two runs to make a second stage.
3. **Everything is written whole.** `emit`, `put`, `given`, `invoke`, a record literal, `reject` with
   fields and `guard Name { .. }` all require every declared field, once each. `{ order_id }` is
   shorthand for `{ order_id: order_id }` everywhere except in a `given`, which spells every field
   out.
4. **`patch` materializes, `update` drops.** A `patch` on an absent row builds it from zeros, which
   is right for counters and wrong for identities: use `update` for anything the read model treats as
   a thing that exists. `Uuid` and `Timestamp` have no zero.
5. **Money never rounds silently.** `Money(2) + Money(3)` and `Money(2) + Decimal(2)` are compile
   errors, and an inexact `total * rate` fails at run time naming `mul`; write
   `total.mul(rate, HalfUp)`. `Int / Int` and `Decimal(s) / Int` truncate instead, with no error.
6. **`T?` does not fill `T`.** Use `unwrap_or(x)`, or a branch that proves it present (narrowing). A
   bare `T` does fill a `T?` at every declared position, wrapping once at the outside.
7. **Sealed content may only be moved, asked about, or revealed.** A field with `@subject(...)`
   cannot be interpolated, compared, sent in a body, passed to `invoke`, `unwrap_or`ed or read
   through a method. Move it into a same-subject position, ask `.is_some()`, or `reveal(x)` in an
   effect arm.
8. **Only an effect calls out**, and only an effect arm may `reveal` or `erase`. A helper takes the
   already-revealed value as a parameter.
9. **No minted identity.** `Uuid.derive(seed, name)` is the whole story, usually
   `Uuid.derive(e.id, "purpose")`, so a retry and a replay produce the same id.
10. **One arm per event type in an effect.** List several paths on one arm
    (`on @a, @b as e { shared_field } { .. }`) rather than writing two arms. A projector is the
    opposite: fanning one event out to several handlers is the point.
11. **An effect may not be able to trigger itself**, through any chain of invoked commands and the
    events they emit.
12. **`reject` is about the world and carries a code; `invalid` is about the request and does not.**
    A blank address is `invalid` whoever sends it; a blocked customer is `reject`.
13. **A guard names a proposition, not an entity**: `CourseIsDefined`, not `Course`. It may only
    `return reject <Name>` or `return invalid(...)`, folds at least one slice, reads the log once,
    and hands nothing back to its caller.
14. **An idempotent no-op is not a guard.** If a replay must answer `ok`, the check stays inline as a
    `fold` and an `if`, and so does every refusal below it.
15. **A refusal's message may name its own fields and nothing else, and must name all of them.** The
    code is the name in snake_case, so the name must be capitalised and carry no `_`.

## Reference

Load the file that matches the work. Each is the rules only, distilled from the language
specification in `docs/` of the heklang repository, which carries the reasoning behind them.

| File | Covers |
| --- | --- |
| `reference/language.md` | types, literals and inference, optionals and narrowing, money, strings, containers, `fn`, `const`, `record`, `enum`, modules |
| `reference/commands.md` | commands, folds, stages, the append condition, guards, refusals |
| `reference/projectors.md` | projectors, entities, `put`/`patch`/`update`/`delete`, zeros, indexes |
| `reference/effects.md` | effect arms, the journal, `http`, `Json`, `invoke`, sealing, `reveal`, `erase` |
| `reference/stdlib.md` | every method and builtin, and where each may be called |
| `reference/testing.md` | `test` declarations end to end |
| `reference/errors.md` | every diagnostic code, what causes it, and how to fix it |

`example/` is a complete seven-file program (events, refusals, guards, commands, projector, effect,
tests) that passes `hek check`, `hek test` and `hek fmt --check`. Read it for the shape of an
application before writing a new one.
