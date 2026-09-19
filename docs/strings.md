# Strings

Two additions: interpolation, and a raw multi-line form. Neither adds a type; both produce an
ordinary `String`.

## Interpolation

```
"sku {wanted} is already used by another plan in this shop"
"{duration_months / 12}-Year Warranty"
"gid://shopify/Product/{product_id}"
```

A `{` inside a string opens a hole; the expression inside it is an ordinary expression, and its value
is converted to text. A literal `{` is written `\{`. A `}` in string content is never a delimiter, so
it needs no escape, but `\}` is accepted anyway: an author who has just learned `\{` reaches for its
pair, and rejecting that would be a puzzle rather than a lesson.

There is no string `+`. Interpolation is the whole mechanism, which is why it has to carry arbitrary
expressions rather than just names.

### The lexer nests; interpolation is not restricted to a plain path

This was the one open question, and the evidence settles it. A real port builds roughly ninety
strings, and the expressions in them include `duration_months / 12` and
`variants.get(plan_id).unwrap_or(0)`. A plain-path restriction would carry neither, and the port
would go back to a helper per shape.

Nesting a string literal inside a hole works, which is the case that motivated the restriction in the
first place:

```
"productCreate failed: {err.unwrap_or("")}"
```

Before this, that could not be written, and a real port carried a `message_of(err: String?) -> String`
helper whose entire job was to move the inner `""` out of the braces. A wart that makes authors write
a function to work around it is not a wart to keep.

**How it works**, because the mechanism is what makes nesting free rather than special-cased: the
lexer keeps a stack of open interpolations, each holding the brace depth inside its hole. A `{` in
expression position deepens the top entry; a `}` at depth one closes the hole and resumes string
scanning. A string literal inside a hole simply re-enters the string scanner, which pushes its own
entry. So the nesting is the stack, not a rule.

The token stream is flat, which is what keeps the parser a flat recursive-descent one: an
interpolated string lexes to `TextOpen`, then the hole's tokens, then `TextPart` or `TextClose`,
alternating. `primary` reads that shape directly into one node whose parts are ordinary expressions.

**Rejected: restricting a hole to a path expression** (`{a}`, `{a.b}`). It is a smaller lexer, and
the restriction is invisible until the day it bites. But it bites on arithmetic and on any method
call, both of which are common in the middle of a message, and the workaround is a named helper per
site, which is worse than the thing being avoided.

### A value's text form is rule 8's JSON table

Not `Display`, which quotes a `String`. The table in `docs/effects.md` rule 8 already had to decide
how every value looks when it leaves the process, and a second answer would be a second thing to get
wrong:

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

`Money(3)` giving `10.500` rather than `10.5` is the point of the shared table: the scale is part of
the value, and a message that drops it is a message that lies about precision.

### A width is the one thing the table cannot say, and `Int.pad` says it

There are no format specifiers, and that stays: a hole holds an expression, and a second little
language inside the braces is a second thing to learn and a second thing to get wrong. What the
decision cost was narrower than it looked, and it was one shape:

```
"{y}-{mo}"          // 2026-9, and a billing-period key that is silently wrong
"{y}-{mo.pad(2)}"   // 2026-09
```

`Int` renders as its digits and nothing pads them, so a key built out of calendar fields comes out a
character short for nine months of the year and matches nothing. That is a wrong value rather than a
wrong-looking one, which is the kind of thing worth a method.

`Int.pad(width)` is a total method (`docs/stdlib.md`) and it is deliberately not a specifier: it is
written in the expression where the number is, so nothing about the string syntax changes and there is
no second place where a value's text form is decided. It is on the number rather than on the string
because the receiver always is one: a `String.pad_start(2, "0")` would make `"{y}-{"{mo}".pad_start(2,
"0")}"` the way to write a two-digit month, which is a nested interpolation to pad a number.

It is `truncate`'s pair. One bounds a string above and one bounds it below, both count the characters
`len` counts, and neither reports that the value already fitted: `truncate` hands the string straight
back, and `pad` writes the number's own text, which is a `String` either way because that is what
`pad` returns.

## Raw multi-line strings

```
const PRODUCT_CREATE_MUTATION: String = """
mutation productCreate($input: ProductInput!) {
  productCreate(input: $input) {
    product { id }
    userErrors { field message }
  }
}
"""
```

Everything between the delimiters is the value, verbatim. **No escapes, no interpolation, no
indentation stripping.**

GraphQL settles both halves of that, and GraphQL is the reason the form exists at all:

- **No interpolation**, because a GraphQL document is brace-dense. `{ edges { node { id } } }` is
  ordinary content, and a form that read those as holes would demand a backslash on almost every line
  of every document. The multi-line form is exactly where interpolation is least wanted.
- **No indentation stripping**, because GraphQL does not care about leading whitespace, so the
  feature would buy nothing while forcing a rule about tabs against spaces and about what the closing
  delimiter's column means. Swift and Kotlin both have that rule and both have errata about it.

If a document needs a value spliced into it, that is what GraphQL variables are for, and the
variables object is an ordinary interpolated or JSON value.

**Rejected: one string form with a flag.** Making `"""` interpolate unless marked, or `"` raw when
marked, keeps one syntax at the cost of an author having to remember which mode they are in. Two
delimiters that each do one thing are two things to learn once, rather than one thing to check every
time.

## `truncate` is how a string meets a `@max`

`@max(n)` is a hard boundary on what may enter the log, and `len()` only says where a value sits
relative to it. `truncate(n)` is the other half of that pair: the first `n` characters, and the
string itself when it already fits.

```
emit @product.described { product_id, body_html: html.truncate(500) }
```

It counts characters, which is the unit `@max(n)` bounds in and the unit `len()` reports in, so the
value `text.truncate(n)` hands back satisfies `@max(n)` for every input there is. A count at or
below zero keeps nothing, which is the same sentence read at its edge. `strip_prefix` is the shape
it copies: total, the string unchanged when there is nothing to do, and no optional for a decision
the author has already made.

**Truncate last.** The guarantee is about the value `truncate` hands back, so a method after it is
outside the guarantee, and one of them can undo it: `upper()` and `lower()` may return more
characters than they took, because `ß` uppercases to `SS` and `İ` lowercases to two. So
`body.truncate(3).upper()` is six characters on `"ßßßßß"` and `body.upper().truncate(3)` is three.
Write the bound at the end of the chain.

**It is on `String`, not on `String?`.** An optional has three methods and this is not one of them
(`docs/optionals.md`), so a `String? @max(n)` field is met by getting to the string first:
`text.unwrap_or("").truncate(n)` when absent may become empty, and narrowing then truncating when it
may not.

**Why an author writes it rather than `@max` doing it at the boundary.** Silent truncation at a
schema edge reads fine and mangles data: a description that arrives forty characters over is stored
forty characters short, and nothing in the program said so. That is `docs/money.md`'s argument in a
second place, where an inexact `mul` fails rather than rounds until the author names a rounding. So
the annotation stays a boundary and the method is what meets it, written at the `emit` where the
decision is being made and visible to whoever reads it next.

The two runtime channels are unchanged and still carry the values nobody truncated: over-length at an
`emit` is `Outcome::Invalid`, and in a projector it is a hard error (`docs/projectors.md`). A
truncated value is computed rather than read, so it is not what the `max-tightening` invariant is
about either; that check is still two declarations disagreeing.

**Which means the count and the bound are the author's to match.** `body.truncate(80)` into a
`@max(8)` column checks clean and then fails at run time, hard, because `max-tightening` reads a
declaration and this is an expression. Comparing the two literals would be decidable, and it is the
narrow end of a question `docs/projectors.md` declines whole: reasoning about the length of an
expression is a different check with a different answer, and one that stops at literals would look
like a guarantee it is not.

**A sealed field is bounded from the plaintext side.** Writing into a `@subject(...)` field is the
encrypting direction and needs no ceremony (`docs/effects.md` rule 12), so a command holding an
ordinary `String` meets the bound the ordinary way: `email: email.truncate(200)` checks clean. What
is refused is a sealed **receiver**. `e.email.truncate(200)` on content folded out of the log is
`seal-boundary`, for the same reason `e.email.trim()` is, so content that is being *moved* rather
than freshly written cannot be reshaped to fit and the `max-tightening` check is the whole of what
can be said about its length.

**Rejected: `truncate` returning `String?`**, absent when it cut something off. The information is
real, and every call site would spend an `unwrap_or` throwing it away: an author who wrote
`truncate(500)` against a `@max(500)` has already decided what happens to the tail. `len() > 500`
asks the question for the one caller who wants to branch on the answer.

## What is deliberately absent

- **No `+` on strings.** Interpolation covers it, reads better in the cases that matter (a message
  with several holes), and having one way avoids the question of what `"a" + 1` means.
- **No `str()` or `.to_string()`.** The text form is defined by the table above and reached through
  interpolation, so there is no second spelling that could drift from it.
- **No format specifiers** (`{x:.2}`). A `Money(3)` already knows it has three places; a width or a
  precision in the hole would be a second source of truth about a value's shape, and the one case it
  would serve (padding for aligned output) is not something a handler does.
