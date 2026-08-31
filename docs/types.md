# Types

Every type heklang has, what an expression's type is, and what may be written where.

Until now there was no such document, and the rules were spread across four: the optional
coercion in `docs/optionals.md`, the numeric literal in `docs/literal-inference.md`, the operator
table in `docs/money.md`, the method tables in `docs/containers.md` and `docs/strings.md`. Each of
those is still where its rule is argued. This is where they are one system.

This document is the contract. `tests/types.rs` is the same set of rules as executable tests, one
test per numbered rule. Change the doc, the tests and the code together.

The gap it closes is the one a real port found by being run for the first time. `hek check` passed
this:

```
command C(id: Int, text: String) {
  emit @thing.happened { id, name: text.to_int() }   // an Int? into a String field
}
```

and only a `test` that ran it reported `expected String, found Int?`. The runtime has always checked;
nothing static did. A checker whose green light does not mean "this can run" is not doing its job,
which is the same thing `docs/effects.md` rule 12 says about the decrypt boundary.

---

## 1. The types

| Type | Written | Notes |
| --- | --- | --- |
| `Bool` | `Bool` | |
| `Int` | `Int` | 64-bit |
| `Decimal(n)` | `Decimal(2)` | a scaled integer at `n` places |
| `Money(n)` | `Money(3)` | a scaled integer, and a **distinct** type: `docs/money.md` |
| `String` | `String` | |
| `Uuid` | `Uuid` | written as a string literal (`docs/declarations.md`) |
| `Timestamp` | `Timestamp` | written as an RFC 3339 string literal |
| an enum | its name | declared by `enum` |
| a record | its name | declared by `record` |
| `Json` | `Json` | opaque; read with the accessors in `docs/effects.md` rule 8 |
| `List(T)` | `List(Int)` | `docs/containers.md` |
| `Map(K, V)` | `Map(Uuid, String)` | `K` must order |
| `T?` | `String?` | the only absence: `docs/optionals.md` |

Three more exist and **cannot be written in a type position**:

- `Response`, spellable in a `fn` signature and nowhere else (`docs/functions.md`), because a
  transport result is not data an event or a column may hold.
- `Outcome`, the result of an `invoke`. It has no spelling outside a `fn` signature; it is
  otherwise only consumed by `.ok()`, `.code()` and `.message()` on the expression that produced
  it. `.code()` is a `String?`, and a declared refusal name is that code, so
  `r.refused(ShopNotFound)` and `r.code().unwrap_or("") == ShopNotFound` are both checked
  (`docs/refusals.md`).
- `Rounding`, the mode a `mul` or `div` takes. Its values are the bare words `HalfUp`, `HalfEven`
  and `Down`.

And one is **derived rather than written**: `Sealed(T, subject)`, which an event field gets from
`@subject(...)`. `docs/effects.md` rule 12 is the whole of it. `Opt` stays outermost, so
`String? @subject(x)` is `Opt(Sealed(String, x))`.

**Type equality is exact and structural.** `Decimal(2)` is not `Decimal(3)`, `Money(2)` is not
`Decimal(2)`, and content sealed under one subject is not content sealed under another. Nothing
widens, nothing coerces numerically, and there is no subtyping except the one rule in section 3.

## 2. Synthesis: what an expression's type is

| Expression | Type |
| --- | --- |
| a literal | the literal's, after `docs/literal-inference.md` has resolved it |
| a name | the slot's declared type |
| `-x` | the operand's |
| `!x` | `Bool`, whatever the operand was |
| `a + b`, `a - b`, `a * b`, `a / b`, `a % b` | the operator table (below) |
| `a == b`, `a < b`, `a && b` | `Bool` |
| `x.method(...)` | the method table's return, for a receiver whose type is known |
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

### Unknown is an answer, and it is not an error

Synthesis returns "unknown" wherever it cannot be sure, and **an unknown type is never checked**. It
is not a hole in the language: it is what keeps this check safe to run everywhere at once. A check
that guessed would reject correct programs, which is strictly worse than one that stays quiet.

Unknown propagates through a `let`, because a `let` takes its binding's type from its value
(`docs/containers.md` explains why `let` has no annotation). So one unknown makes every later use of
that name unchecked. That is the honest cost, and it is why section 2's table is worth keeping
complete: every row filled in is a body's worth of checking gained.

### The operator table

`docs/money.md` argues this; here it is as a table.

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

Anything else, with both types known, is a compile error naming both operands. **Scales never meet**:
`Money(2) + Money(3)` and `Decimal(2) + Decimal(4)` are errors, because a silent rescale is how a
total loses a cent. A literal is the one thing that widens, and it does so before it is a value at all.

A comparison follows the same table: `==` and `!=` on any two of the same type, and `< <= > >=` on
`Int`, `Decimal(s)`, `Money(n)`, `String` and `Timestamp`. `Money(2) > Money(3)` is an error for the
reason `Money(2) + Money(3)` is.

**Arithmetic on sealed content is rejected.** A sum of it is plaintext derived from it, so `reveal`
comes first, the same rule that already covers interpolation and comparison.

## 3. Filling: what may be written where

Every position that declares a type funnels through one relation. A value of type `F` fills a
declared `T` when:

- **`F` is `T`.** Exactly, by the equality in section 1.
- **`T` is `F?`.** A bare value fills an optional and wraps. One level, at the outside of the
  declared type, so a `List(String)` still does not fill a `List(String?)`. `docs/optionals.md` has
  the argument and the exhaustive list of positions.
- **`T` is `F` sealed.** Writing plaintext into a subject-bound field is the encrypting direction and
  needs no ceremony. The other direction needs `reveal`, and is rejected by rule 12 with its own
  message rather than by this one.

Nothing else. In particular: `T?` does not fill `T` (that is what `unwrap_or` and narrowing are for),
`Int` does not fill `Money(n)` or `Decimal(n)` (a *literal* resolves directly to either; a typed `Int`
value does not), and no type fills `Json` except a `Json`.

### The positions

Every one of them, because the rule is only worth something if it holds at all of them:

an `emit` field, a `put`/`patch`/`update` column and its key, a command parameter at `run` and at
`invoke`, a `fn` argument and its return, a `state` seed and every fold arm, a slice filter value, a
record literal field, a list element and a comprehension's yield, a method argument, an entity
default and a `const`, a `given` field and every `expect` value, an `if` condition and both operands
of `&&`, `||` and `!` (which is where `if owner_email` stops being a program).

Almost all of them are one call site in the parser, so adding a position cannot forget the rule.

**The boolean operands are the exception, and were the one entry this list claimed without
honouring.** They are not written through that call site: the ladder that parses `&&` and `||` sits
*inside* it, so `Bool` reached them as an inference hint and nothing below ever compared anything to
it. `if ok && id` passed. They now check each operand where the ladder builds it, which is the only
place that has one operand rather than the whole expression.

The `!` case is worth its own sentence, because it looked like it worked. `if !id` was rejected, but
by the check on the whole condition rather than on the operand, and only because `!x` used to
synthesise the operand's type. Put the same `!id` inside a `&&` and the condition synthesised `Bool`,
so nothing was left to notice. Fixing the synthesis alone would have made `if !id` pass; the two go
together.

### Where the check happens, and why not in a pass of its own

In the parser, while it lowers. Two things need a type *before* the IR node exists: a numeric literal
has to know its scale to be built at all, and a load of a narrowed optional lowers to a different node
than a plain one. So the type rules cannot be deferred to a walk over a finished program, and putting
half of them there would put them in two places.

What is separable is separated: `src/types.rs` holds the tables and the relations with no parser state
in them, so a checker outside the parser can use the same ones. `docs/cli.md` has the rest of that
story.

## 4. What is deliberately not checked

- **Anything whose type is unknown.** Section 2.
- **A `fn`'s body against its own callers.** There are no generics and no inference across a
  signature; a `fn` declares its parameter types and its return, and both are checked at the boundary.
- **A `Json`'s shape.** `Json` is opaque by design: `.string("k")` returns a `String?` whether the key
  is absent or holds a number, because the document came from outside and its shape is not a promise
  the language can keep.
- **Currency.** `docs/money.md` states this cost plainly: `Money(2)` in USD and `Money(2)` in AUD are
  one type, and adding them is the author's invariant to keep.
- **Two values under different subjects being compared or combined.** Comparison of sealed content is
  rejected outright, so the question does not arise.

## 5. Known gaps

- **`Uuid` does not order.** `<` and `>` accept `Int`, `Decimal(n)`, `Money(n)`, `String` and
  `Timestamp`. A `Uuid` may be an entity key, so it orders somewhere, but that order is for
  pagination rather than for asking which id is larger, and no program has wanted to ask.
- **A record's fields are not checked against the record.** `Record { .. }` synthesises that record's
  type without confirming the literal built it correctly; the field-by-field check happens where the
  literal is parsed, which covers the same ground by a different route.
- **A moment built from parts is on the second.** `Timestamp.from_parts` takes no sub-second
  argument, so calendar arithmetic through it drops any fraction the original carried. The written
  form and `Timestamp.parse` both keep microseconds.

## Methods

The receiver's type decides which methods exist, so where the receiver is known a method that is not
on it is caught where it is written rather than when it runs. So is the count of its arguments; their
types check themselves on the way in, because the hint each one resolves against comes from the same
table.

The message names the way out for the confusion this sees most, which is one confusion from two
sides: `is_empty()` asked of a `String?` and `is_none()` asked of a `String`. A real port made that
edit by hand in eight files, having found each one by running it.

### A `Timestamp` can be taken apart

```
at.year()  at.month()  at.day()  at.hour()  at.minute()  at.second()   -> Int
Timestamp.from_parts(year, month, day, hour, minute, second)           -> Timestamp?
```

UTC, and the constructor is fallible because six numbers do not always name a date. These exist so
that calendar arithmetic can be written as a `fn`, which is where `docs/functions.md` argues it
belongs: month-end clamping is one opinion among several, so the language supplies the calendar and
the author supplies the rule. Before them that argument had nowhere to send anyone.
