# The language

Types, literals, optionals, money, strings, containers, `fn`, `const`, `record`, `enum`, and how a
program is assembled from files. Handler kinds have their own files.

## 1. The types

| Type | Written | Notes |
| --- | --- | --- |
| `Bool` | `Bool` | |
| `Int` | `Int` | 64-bit |
| `Decimal(n)` | `Decimal(2)` | a scaled integer at `n` places |
| `Money(n)` | `Money(3)` | a scaled integer, and a **distinct** type from `Decimal(n)` |
| `String` | `String` | |
| `Uuid` | `Uuid` | written as a string literal |
| `Timestamp` | `Timestamp` | written as an RFC 3339 string literal, UTC, microsecond precision |
| an enum | its name | declared by `enum` |
| a record | its name | declared by `record` |
| `Json` | `Json` | opaque, read with the one-step accessors |
| `List(T)` | `List(Int)` | |
| `Map(K, V)` | `Map(Uuid, String)` | `K` must be `Int`, `String`, `Uuid`, `Timestamp` or an enum |
| `T?` | `String?` | the only absence in the language |

Two more are spellable **only** in a `fn` parameter or return type, and nowhere else:

- `Response`, what `http.*` returns. A response is transport, not data, so no event field, entity
  column, record field, `fold` or command parameter may name one, and neither may `List(Response)`.
- `Outcome`, what an `invoke` answers with and what `reject`/`invalid` construct. A refusal is a
  decision, not data, so the same restriction applies.

A third, `Rounding`, is spellable **nowhere**: `Rounding` as a type name is `unknown type`, and its
values reach `.mul` and `.div` as the bare words `HalfUp`, `HalfEven` and `Down`. So a rounding mode
cannot be passed through a `fn` parameter.

One more is **derived rather than written**: `Sealed(T, subject)`, which an event field gets from
`@subject(...)`. `Opt` stays outermost, so `String? @subject(x)` is `Opt(Sealed(String, x))`.

**Type equality is exact and structural.** `Decimal(2)` is not `Decimal(3)`, `Money(2)` is not
`Decimal(2)`, and content sealed under one subject is not content sealed under another. Nothing
widens and nothing coerces numerically. `Money(n)` and `Decimal(n)` refuse a scale above 18.

## 2. What may be written where

A value of type `F` fills a declared `T` when:

- **`F` is `T`**, exactly;
- **`T` is `F?`**: a bare value fills an optional and wraps, one level, at the outside of the
  declared type (so a `List(String)` still does not fill a `List(String?)`);
- **`T` is `F` sealed**: writing plaintext into a subject-bound field is the encrypting direction and
  needs no ceremony.

Nothing else. In particular `T?` does **not** fill `T`, and a typed `Int` value does not fill a
`Money(n)` or a `Decimal(n)` (a *literal* resolves directly to either; a value does not).

The positions this holds at, exhaustively: an `emit` field, a `put`/`patch`/`update` column and its
key, a command parameter at `run` and at `invoke`, a `fn` argument and its return, a fold seed and
every fold arm, a slice filter value, a record literal field, a list element and a comprehension's
yield, a method argument, an entity default, a `const`, a guard argument, a refusal field, a `given`
field and every `expect` value, an `if` condition and every operand of `&&`, `||` and `!`.

## 3. What an expression's type is

| Expression | Type |
| --- | --- |
| a literal | the literal's, after inference (section 4) |
| a name | the slot's declared type |
| `-x` | the operand's |
| `!x` | `Bool` |
| `a + b`, `a - b`, `a * b`, `a / b`, `a % b` | the operator table below |
| `a == b`, `a < b`, `a && b` | `Bool` |
| `x.method(...)` | the method table's return |
| `if c { a } else { b }` | `a`'s, and only when `b` agrees |
| `record.field` | the declared field's |
| `response.status`, `response.body` | `Int`, `Json` |
| `{ "k": v }` | `Json` |
| `"text {hole}"` | `String` |
| `[a, b]`, `[x for x in xs]` | the declared element type when the target gave one, else the first element's |
| `http.post(...)` | `Response` |
| `Uuid.derive(..)`, `Json.encode(..)` | `Uuid`, `String` |
| `Timestamp.parse(..)`, `Money.parse(..)` | `Timestamp?`, `Money(n)?` |
| `invoke C { .. }` | `Outcome` |
| `Record { .. }` | that record |
| `f(..)` | the `fn`'s declared return |
| `reveal(x)` | `x`'s, with the seal off |

**Unknown is an answer, and an unknown type is never checked.** Synthesis returns "unknown" where it
cannot be sure, and unknown propagates through a `let`. That is why a rejected value reports once
rather than at every later use.

### The operator table

| Left | Op | Right | Result |
| --- | --- | --- | --- |
| `Int` | `+ - * / %` | `Int` | `Int` |
| `Decimal(s)` | `+ -` | `Decimal(s)` | `Decimal(s)` |
| `Decimal(s)` | `* /` | `Int` | `Decimal(s)` |
| `Int` | `*` | `Decimal(s)` | `Decimal(s)` |
| `Money(n)` | `+ -` | `Money(n)` | `Money(n)` |
| `Money(n)` | `/` | `Money(n)` | `Decimal(6)`, a ratio |
| `Money(n)` | `* /` | `Int` | `Money(n)` |
| `Int` | `*` | `Money(n)` | `Money(n)` |
| `Money(n)` | `*` | `Decimal(s)` | `Money(n)` |
| `Decimal(s)` | `*` | `Money(n)` | `Money(n)` |

Anything else with both types known is a compile error naming both operands. **Scales never meet**:
`Money(2) + Money(3)` and `Decimal(2) + Decimal(4)` are errors, because a silent rescale is how a
total loses a cent. There is no `+` on `String`.

`==` and `!=` take any two values of the same type. `< <= > >=` take `Int`, `Decimal(s)`, `Money(n)`,
`String` and `Timestamp`. `Uuid` does not order. **Arithmetic on sealed content is rejected**: a sum
of it is plaintext derived from it, so `reveal` comes first.

### Where rounding happens, and where it does not

`Money` never rounds silently. `Money(n) / Int` and `Money(n) * Decimal(s)` are **run-time errors
when the result is not exact**, naming `div` or `mul` and asking for an explicit mode:

```hek
total.mul(rate, HalfUp)     // Money(n).mul(Decimal(s), Rounding) -> Money(n)
total.div(parts, Down)      // Money(n).div(Int, Rounding) -> Money(n)
```

`Int / Int` and `Decimal(s) / Int` **truncate toward zero** on the units, with no error: `1.00 / 3`
at `Decimal(2)` is `0.33`. That asymmetry is deliberate, and it is why an amount is a `Money` rather
than a `Decimal`.

## 4. Literals and inference

The lexer emits one untyped token per numeric literal, and the type comes from where the value
lands. There is no suffix and no constructor.

A literal resolves against a **target type**:

- against `Int`, it must have scale 0;
- against `Decimal(s)` or `Money(n)`, it widens from its written scale, and widening is exact;
- against anything else, the literal takes its default and the position reports what it wanted.

**More written places than the target holds is an error, never a silent round.** `0.0825` cannot
become a `Decimal(2)` and `10.5` cannot become an `Int`.

The target comes from, in priority order:

1. **an annotation**: a parameter type, a `fold` type, an event field type in a filter or an `emit`,
   a column, a `fn` parameter, a method's declared argument;
2. **the other operand of `+`, `-` or a comparison**, in either direction. `*`, `/` and `%` never
   cross-hint, because their operands are deliberately different types. A hint is never taken from a
   literal that was itself defaulted, and a `Bool` is not a target for either operand;
3. **the default**: scale 0 becomes `Int`, any other scale becomes `Decimal(n)` at the scale written.
   Two defaulted literals settle toward the one with more decimal places, so `1 + 0.5` is
   `Decimal(1) + Decimal(1)`.

A bare literal never defaults to `Money`, so money is always reached from an annotation.

**A string literal resolves against a `Uuid` or a `Timestamp` target**, at every position that has
one, validated at parse time. That is how a namespace constant, a test's ids and a `Timestamp` column
default are written. A `Timestamp` literal is RFC 3339 and must carry an offset (`Z`, `z` or
`+HH:MM`); a local time with no offset is rejected.

**An enum variant is written bare** and resolved from the target type. Where the target's type is
unknown, a variant that exactly one in-scope enum declares resolves to that one, and a variant two
enums share is an error naming both.

## 5. Optionals

`T?` is the only way a value is allowed to be absent. There is no null, no
empty-string-means-nothing, no zero-means-nothing, no `unwrap`, no `?` operator and no `.expect()`.

| Method | Returns |
| --- | --- |
| `x.is_some()` | `Bool` |
| `x.is_none()` | `Bool` |
| `x.unwrap_or(fallback)` | `T` |

### Narrowing

A branch that proves an optional present makes it its inner type for as long as the proof holds:

```hek
let plan = plans.get(plan_id)
if plan.is_none() {
  return
}
sync(plan)                    // `plan` is Plan here, not Plan?

if found.is_some() {
  use(found)                  // and here
}
```

Three lines of rule:

- `x.is_some()` narrows the **then** branch, `x.is_none()` narrows the **else** branch, and `!` swaps
  which;
- when the then branch never falls through, the test also narrows the **rest of the enclosing
  block**, which is the early-return shape above;
- a narrowing ends where its block does.

**What deliberately does not narrow**: a compound condition (`if a.is_some() && b.is_some()` narrows
neither), the value-position `if`, and an `else if`. Where narrowing does not reach, `unwrap_or`
does.

## 6. Strings

A `{` inside a string opens a hole holding an ordinary expression, including a nested string literal
and a method call. A literal `{` is `\{`. There is no string `+`, no `str()`, no `.to_string()` and
no format specifiers.

```hek
"sku {wanted} is already used by another plan"
"{duration_months / 12}-Year Warranty"
"productCreate failed: {err.unwrap_or("")}"
```

The **text form** of a value, used by interpolation, is the same table the JSON boundary uses:

| Value | Text |
| --- | --- |
| `Bool` | `true` / `false` |
| `Int` | `42` |
| `Decimal(n)`, `Money(n)` | fixed point at scale `n`, so `Money(3)` gives `10.500` |
| `String` | its characters, unquoted |
| `Uuid` | canonical form |
| `Timestamp` | epoch microseconds |
| an enum | the variant name |
| `none` | `null` |
| `some(x)` | `x`'s text |
| `Json`, `List`, `Map`, a record | its JSON text |

A **raw multi-line string** is `"""..."""`: everything between the delimiters is the value,
verbatim, with no escapes, no interpolation and no indentation stripping. It exists for GraphQL
documents and anything else brace-dense.

## 7. Containers

```hek
let ids = [first_id, second_id]
fold skus: Map(Uuid, String) = Map.empty
  on @plan.created(shop_id) { plan_id, sku } => skus.set(plan_id, sku)
```

`List(T)`: `.len()`, `.is_empty()`, `.contains(x)`, `.first() -> T?`, `.push(x) -> List(T)`,
`.remove(x) -> List(T)`. `push` and `remove` build a **new** list; nothing mutates. `remove` drops
every equal element, so it is idempotent. There is no indexing.

`Map(K, V)`: `.get(k) -> V?`, `.set(k, v)`, `.remove(k)`, `.contains(k)`, `.len()`, `.is_empty()`,
`.keys() -> List(K)`, `.values() -> List(V)`.

**Iteration is sorted by key, not insertion order.** That is load-bearing: the same object built
twice has to serialise identically for replay verification. If insertion order matters, carry a
separate `List(K)` beside the map and say so.

**Where an empty container's type comes from.** `[]` and `Map.empty` hold nothing, so the type comes
from the target: a `fold` declaration, a command or `fn` parameter, an event or entity field, or a
method argument that already knows. A `let` is **not** a target, because `let` takes no type
annotation. Inside a JSON object literal `[]` needs no target, since a body's values are typed by
what they are.

```hek
for id in ids { .. }              // one name over a list
for plan_id, plan in plans { .. } // key and value over a map
for index, item in items { .. }   // index and item over a list

[plan.title for plan_id, plan in plans if plan.status == Active]
```

A `for` runs once per element of a finite container and always terminates. There is no `while`, no
`break` and no `continue`. A `return` inside a `for` does propagate out of it, which is how a search
is written. One name over a map is an error: there is no pair type.

**There are no mutable bindings.** `let` only, no `var`. Every accumulation is a comprehension or a
fold arm returning new state, and every search is a `fn` whose `for` body returns.

## 8. `fn`

```hek
fn effective_sku(sku: String?, plan_id: Uuid) -> String {
  let given = sku.unwrap_or("").trim()
  if given.is_empty() {
    return "{RESERVED_SKU_PREFIX}{plan_id}"
  }
  return given
}
```

Module scope, a **required** return type, and `return <expr>`. Callable from a command, a guard, a
projector, an effect, another `fn` and a fold arm.

**A module `fn` is pure.** No clock, no `http.*`, no `invoke`, no `reveal`, no `erase`, no `emit`, no
read-model write. That purity is what makes it callable from a fold arm and from a projector without
a rule of its own.

**Recursion is rejected**, directly or through any chain, and the error names the cycle as a path.
**Every path must return**: an `if` with no `else` does not count, and neither does a `for` body,
because a container can be empty.

A `fn` may take and return a `Response` or an `Outcome`, and nothing else may name either type. A
`fn` declared `-> Outcome?` is how two commands share one refusal ladder:

```hek
fn ladder(subscribed: Bool, taken: Int, cap: Int) -> Outcome? {
  if subscribed { return reject AlreadySubscribed }
  if cap == 0 { return invalid("this course has no capacity set") }
  if taken >= cap { return reject CourseFull }
  return none
}

// in a command
let decision = ladder(subscribed, taken, limit)
if decision.is_some() {
  return decision
}
```

**An effect-local `fn`** is declared inside an `effect` and is the one impure helper. See
`effects.md`; the short version is that it may `http.*`, `invoke`, `log` and `fail`, may **not**
`reveal`, `erase`, `now()` or declare a `fold`, may omit its return type (the only signature that may),
and a call to a void one is a statement rather than an expression.

Arguments are positional and checked against the declared parameters, with each parameter's type as
that argument's hint. There are no default arguments, no named arguments, no closures, no function
values, no generics and no overloading. A trailing comma closes an argument list.

## 9. `const`

```hek
const WEBHOOK: String = "https://webhooks.example.com"
const FREE_TIER_LIMIT: Int = 15
const NAMESPACE: Uuid = "6ba7b810-9dad-11d1-80b4-00c04fd430c8"
const LAUNCH: Timestamp = "2026-01-01T00:00:00Z"
const NO_SKU: String? = none
const HOUSE_SKU: String? = "house"
const HOUSE_ITEM: Item = Item { sku: "house", price: 0.00, tags: [] }
const NO_PLAN: Uuid = NAMESPACE
```

`const NAME: Type = <literal>`, restricted to literals and literal aggregates: a scalar, a list of
them, `Map.empty`, `Json.empty`, a record whose fields are literals, or the name of another const.
Not an expression: `const B: Int = A + 1` is rejected. A const that names itself, directly or through
a chain, is rejected by naming the chain.

Order is irrelevant, so a const may name one declared below it or in another file. A const is inlined
at every use rather than being a runtime lookup.

## 10. `record`

```hek
record LineItem {
  id: Int,
  title: String @max(255),
  variant_title: String? @max(255),
}
```

A named product type at module scope, and an ordinary value: it can be a `fold` type, an event
field, a command or `fn` parameter, a return type, and the element of a `List` or `Map`. It
serialises to a JSON object.

The literal is `Name { field: value }`, with the same bare-name shorthand `emit` and `put` use, and a
field is read with `.field`. **Every field must be given.** There is no record update
(`base with { .. }`) and no partial literal.

`@max(n)` is the only annotation a record field takes. **A record field cannot be `@subject`**, so a
record cannot carry subject-bound personal data.

A `Name {` is read as a record literal only when `Name` is a declared record, is not shadowed by a
local, and no `if` or `for` header is waiting for its block. Inside parentheses the restriction
lifts.

## 11. `enum`

```hek
enum Status { @default Draft, Active, Archived }
```

Declared at module scope, or inside a projector, where the projector's own shadows a module one.
Variants are written bare and resolved from the target type. An enum used as a non-optional entity
field needs a `@default` variant, so that reordering variants is not a semantic change.

Because enums are types rather than a set of allowed strings, a variant that is not in the set cannot
be written at all, on an event field, a command parameter or a column.

## 12. Modules

Every `.hk` file under the checked path is one module of one program. There is no import syntax, no
manifest and no header item, and **declaration order does not matter**, within a file or across
files.

Event paths and the names of commands, guards, refusals, projectors, effects, records, enums,
consts, module `fn`s and tests are **global**. Two modules may not both declare `@order.placed` or a
command named `Place`; that is the same "declared twice" error as within one file, and it names the
module of the first declaration.

The three handler kinds have **separate name spaces**, as do guards, refusals and `fn`s, so one
program may hold a `command Same`, a `projector Same` and an `effect Same`. The kind is at every use
site (`invoke Same`, `guard Same`, `reject Same`), so nothing has to be renamed to avoid a collision
that does not exist.

The only scoped names are a projector's entities and enums, and an effect's local `fn`s.

Line and column in a diagnostic are module-relative, and errors render as
`module:line:col [code] message`.
