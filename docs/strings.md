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

## What is deliberately absent

- **No `+` on strings.** Interpolation covers it, reads better in the cases that matter (a message
  with several holes), and having one way avoids the question of what `"a" + 1` means.
- **No `str()` or `.to_string()`.** The text form is defined by the table above and reached through
  interpolation, so there is no second spelling that could drift from it.
- **No format specifiers** (`{x:.2}`). A `Money(3)` already knows it has three places; a width or a
  precision in the hole would be a second source of truth about a value's shape, and the one case it
  would serve (padding for aligned output) is not something a handler does.
