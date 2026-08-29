# Parsing

Seven small builtins for reading values out of text that came from outside the program: a webhook
body, a GraphQL response, a merchant-typed field. They are in their own document, and landed in their
own commit, because a set of conveniences is exactly the kind of surface that grows without anyone
deciding to grow it.

| Builtin | Returns |
| --- | --- |
| `Timestamp.parse(text)` | `Timestamp?` |
| `Money.parse(text)` | `Money(n)?` |
| `text.to_int()` | `Int?` |
| `text.to_uuid()` | `Uuid?` |
| `text.starts_with(prefix)` | `Bool` |
| `text.strip_prefix(prefix)` | `String` |
| `text.after_last(separator)` | `String` |

All pure, so all callable from a command, a projector, an effect, a `fn` and a `state` fold arm.

## Everything fallible returns an optional

The four that can fail return `T?`, never a trap and never a sentinel. The text came from a remote
service, so failing is the ordinary case rather than the exceptional one, and an author who has to
handle it is an author who was going to have to handle it anyway.

The three that cannot fail return a total answer instead, which is a decision worth stating because
the alternative reads better in the abstract and worse in use:

- **`strip_prefix` returns the string unchanged** when the prefix is absent, rather than `String?`.
  It is written after a `starts_with` that has already decided, so an optional there would be an
  `unwrap_or` on a branch that cannot be taken, which is exactly the shape
  `docs/optionals.md`'s narrowing rule is for. The pair is the API: `starts_with` asks,
  `strip_prefix` acts.
- **`after_last` returns the whole string** when the separator is absent. That is what makes
  `gid.after_last("/").to_int()` safe on something that is not a global id: the chain still ends in
  an optional, and it ends there once rather than twice.

## `Money.parse` takes its scale from the target

```
price: Money.parse(item.string("price").unwrap_or("0")).unwrap_or(0)
```

The scale is a property of **where the amount lands**, not of the text, because `"10.5"` is a
different value at scale 2 and at scale 3. So the target type decides it, the same way it decides a
written literal's, and `Money.parse` with no target is a compile error naming the places one comes
from.

Given a scale, the rule is exactly the literal rule from `docs/money.md`: **widening is exact, and
more written places than the target holds is a failure rather than a silent round.** `"10.5"` into
`Money(3)` is `10.500`; `"1.2345"` into `Money(3)` is `none`. Rounding here would be the language
quietly deciding what a merchant's price is.

## `Timestamp.parse` is RFC 3339, and requires an offset

Hand-rolled rather than a dependency. The shapes that actually arrive are a small set, and a calendar
library is a large surface and a large set of opinions to take on for one function.

**A local time with no offset is rejected.** `"2020-01-01T00:00:00"` is not RFC 3339, and the
alternative to rejecting it is guessing a zone, which is how a warranty ends up expiring on the wrong
day. `Z`, `z` and `±HH:MM` are accepted; fractional seconds are truncated to microseconds, which is
the precision a `Timestamp` has.

Ranges are checked, so `2023-02-29` is `none` and `2024-02-29` is a date. That is worth a test rather
than a claim, since a hand-rolled calendar is where leap years go wrong.

### It shares its reading with the literal

A string in a `Timestamp` position **is** a `Timestamp` (`docs/declarations.md`), checked by this same
function at parse time. So there is one reading of RFC 3339 and two ways in, split by where the text
came from:

| Written | Read | Gives |
| --- | --- | --- |
| `at: "2026-01-01T00:00:00Z"` | at parse time, by the target type | `Timestamp`, or an error naming the shape |
| `Timestamp.parse(body.string("at").unwrap_or(""))` | at run time | `Timestamp?` |

The author's text is checked now and cannot be absent; a webhook's is checked then and can be
anything. `Uuid` and `text.to_uuid()` are the same pair, which is why this needs no rule of its own.

## What is deliberately absent

- **No `Timestamp.add_months`**, and no calendar arithmetic at all. `docs/functions.md` has the
  reason: month-end clamping is one opinion among several, and now that `fn` exists it belongs in a
  shipped `lib/` an application can disagree with.
- **No formatting.** `docs/strings.md` gives every value one text form, through interpolation. A
  second one here would be a second thing to keep in step.
- **No `to_decimal`.** `Money.parse` covers the case that came up; a `Decimal` from text has not
  been needed, and adding it now would be adding it because the table looks asymmetric.
- **No regular expressions.** Every read here is a prefix, a suffix or a whole value, which is what
  a well-shaped identifier gives you. A pattern language is a language.
