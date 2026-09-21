# The standard library

Everything callable that an author did not declare. It is small on purpose, and this is the whole of
it: **a method that is not in these tables does not exist**, and the error names the receiver.

## 1. Methods, by receiver

### `String`

| Method | Returns | |
| --- | --- | --- |
| `trim()`, `lower()`, `upper()` | `String` | |
| `len()` | `Int` | |
| `is_empty()` | `Bool` | |
| `contains(s)`, `starts_with(s)` | `Bool` | |
| `strip_prefix(s)` | `String` | the string unchanged when the prefix is absent |
| `after_last(s)` | `String` | the whole string when the separator is absent or empty |
| `truncate(n)` | `String` | the first `n` characters, the string itself when it already fits |
| `to_int()` | `Int?` | |
| `to_uuid()` | `Uuid?` | |

`strip_prefix` is written after a `starts_with` that already decided, and `after_last` exists so that
`gid.after_last("/")` is safe on something that is not a global id. The two conversions return
optionals, because there the failure is the point.

`truncate` counts characters, the unit `len()` reports and `@max(n)` bounds, so `text.truncate(n)`
satisfies `@max(n)` for every input and a count at or below zero gives `""`. It is how a value from
outside is made to fit a bounded field: `body_html: html.truncate(500)` at the `emit`. `@max` never
truncates on its own, because silent truncation at a schema edge is the string version of a silent
round. A truncated value is computed rather than read, so the `max-tightening` check does not fire on
it. Four things to get right:

- **Truncate last.** The guarantee covers what `truncate` returns, and `upper()`/`lower()` can grow a
  count (`ß` uppercases to `SS`), so `body.truncate(3).upper()` may be six characters. Put the bound
  at the end of the chain.
- **It is on `String`, not `String?`.** `body.truncate(n)` on an optional is `no method truncate on
  String?`. Write `body.unwrap_or("").truncate(n)`, or narrow first and truncate the narrowed value.
- **A sealed destination is fine; a sealed receiver is not.** Writing plaintext into a
  `@subject(...)` field is the encrypting direction, so `email: email.truncate(200)` from an ordinary
  `String` checks clean. `e.email.truncate(200)` on content folded out of the log is `seal-boundary`,
  the same as `e.email.trim()` (rule 7 of this skill, `effects.md` rule 12).
- **Nothing checks a literal count against a literal bound.** `body.truncate(80)` into `@max(8)`
  checks clean and then fails at run time, hard in a projector. Match the two by hand.

### `Json`

| Method | Returns |
| --- | --- |
| `string(key)` | `String?` |
| `int(key)` | `Int?` |
| `bool(key)` | `Bool?` |
| `json(key)` | `Json?` |
| `array(key)` | `List(Json)?` |
| `number(key)` | `String?`, the exact text of a JSON number |

Every one is optional: a missing key and a key holding the wrong shape both answer `none`, so an
author writes one branch rather than two. There is no `get`, no dynamic field access and no indexing.
`int` answers `none` for `10.5`.

### `T?`

| Method | Returns |
| --- | --- |
| `unwrap_or(T)` | `T` |
| `is_some()`, `is_none()` | `Bool` |

Three, and there is no `unwrap` and no `expect`. Narrowing removes most of the calls an author would
otherwise write.

### `List(T)`

| Method | Returns |
| --- | --- |
| `first()` | `T?` |
| `push(T)` | `List(T)` |
| `remove(T)` | `List(T)` |
| `contains(T)` | `Bool` |
| `len()` | `Int` |
| `is_empty()` | `Bool` |

`push` and `remove` build a new list rather than mutating one. `remove` removes **every** equal
element, so it is idempotent. There is no indexing.

### `Map(K, V)`

| Method | Returns |
| --- | --- |
| `get(K)` | `V?` |
| `set(K, V)` | `Map(K, V)` |
| `remove(K)` | `Map(K, V)` |
| `contains(K)` | `Bool` |
| `keys()` | `List(K)` |
| `values()` | `List(V)` |
| `len()` | `Int` |
| `is_empty()` | `Bool` |

`keys` and `values` come back **sorted by key**, which is load-bearing for rebuild and replay
determinism.

### `Money(n)`

| Method | Returns |
| --- | --- |
| `mul(Decimal(s), Rounding)` | `Money(n)` |
| `div(Int, Rounding)` | `Money(n)` |

The two places money is allowed to round, and the author says how. `Rounding` is `HalfUp`,
`HalfEven` or `Down`, written as a bare name. The scale stays the amount's; the rate's own scale is
whatever the author wrote, so `total.mul(0.9, HalfUp)` takes a `Decimal(1)`.

### `Timestamp`

| Method | Returns | |
| --- | --- | --- |
| `year()`, `month()`, `day()`, `hour()`, `minute()`, `second()` | `Int` | in UTC |
| `add_seconds(n)`, `add_minutes(n)`, `add_hours(n)`, `add_days(n)` | `Timestamp` | `n` may be negative |

**The fixed-length units are here; the calendar ones are not.** A minute is sixty seconds and a UTC
day is twenty-four hours, so `add_minutes` has nothing to clamp. `add_months` and `add_years` are the
opinion, so they stay absent and calendar arithmetic is written as a `fn` over the calendar fields
above plus `Timestamp.from_parts` (under Constructors).

- **A count, not a magnitude.** `add_minutes(-30)` goes backwards, so there is no `sub_` family.
- **Sub-second precision survives**, which a `fn` over the calendar fields could not manage:
  `from_parts` is on the second, so every hand-written `add_minutes` silently dropped microseconds.
- **Overflow is an error**, not an optional, the same answer `Int` and `Money` arithmetic give.

There is still no duration type and no `format`.

### `Int`

| Method | Returns | |
| --- | --- | --- |
| `pad(width)` | `String` | zero-filled on the left; the receiver's own text when it already fits |

The only `Int` method, and it is about text: the arithmetic is the operators. Interpolation has no
format specifiers, so `"{y}-{mo}"` writes `2026-9` and a billing-period key is silently wrong for
nine months of the year. `"{y}-{mo.pad(2)}"` is the shape it exists for, and the receiver is the
number so that padding one needs no nested interpolation.

The sign comes first and the zeros after it, and `width` counts the whole rendering. A width at or
below zero pads nothing. **A width past 4096 is a runtime error**: `pad` is the only method that makes
a string longer, so an unbounded width reaching it from a request would allocate whatever it says.

It is `truncate`'s pair: one bounds a string above, one below, both count the characters `len` counts,
and neither reports that the value already fitted. `truncate` hands the string straight back; `pad`
writes the number's own text, and its result is a `String` either way.

### `Outcome`

| Method | Returns |
| --- | --- |
| `ok()` | `Bool` |
| `code()`, `message()` | `String?` |
| `refused(<RefusalName>)` | `Bool` |

`refused` takes a declared refusal name and answers whether this is it. An `invalid` carries no code,
so it is refused by nothing.

### `Response`

`.status` is an `Int` and `.body` is a `Json`, **without parentheses**. This is the only parenless
field access on a builtin type in the language.

## 2. Constructors

The global namespace is closed: anything built from nothing is named by its type.

| Call | Returns | |
| --- | --- | --- |
| `Uuid.derive(seed, name)` | `Uuid` | a pure function of both arguments |
| `Json.empty` | `Json` | |
| `Json.encode(value)` | `String` | the JSON table pointed at a string |
| `Map.empty` | `Map(K, V)` | the type comes from the target |
| `Timestamp.parse(text)` | `Timestamp?` | RFC 3339, offset required |
| `Timestamp.from_parts(y, mo, d, h, mi, s)` | `Timestamp?` | optional, because Feb 30 is not a date |
| `Money.parse(text)` | `Money(n)?` | the scale comes from where the result lands |
| `Decimal.parse(text)` | `Decimal(n)?` | the same, for a rate rather than an amount |

`Money.parse`, `Decimal.parse`, `Map.empty` and `[]` all take their type from the target, and a call
with no target is a compile error naming the places one comes from. There is no `List.empty`, because
`[]` already writes it, and no `Map` literal, because `{ ... }` is a JSON object.

`Money.parse` widens exactly and answers `none` for text with more places than the target holds:
`"10.5"` into `Money(3)` is `10.500`, and `"1.2345"` into `Money(3)` is `none`.

## 3. What a handler does

| Call | Returns | Journaled |
| --- | --- | --- |
| `http.get(url)` | `Response` | yes |
| `http.post(url, body)`, `.put`, `.patch` | `Response` | yes |
| `http.delete(url)` | `Response` | yes |
| `invoke Name { ... }` | `Outcome` | yes |
| `now()` | `Timestamp` | yes, pinned once per invocation |
| `erase(value)` | nothing | yes |
| `log(message)` | nothing | **no** |
| `fail "<message>"` | nothing, terminal | n/a |
| `reveal(value)` | the sealed type, unsealed | **no**, re-decrypts every attempt |

Every `http` verb takes an optional `headers = { ... }` named argument after its other arguments. A
timeout does not go there; it is configuration.

## 4. Where each may be called

The pure surface, sections 1 and 2, is available everywhere. What reaches the world is not.

| | command | guard | projector | effect arm | effect-local `fn` | module `fn` | fold arm |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `now()` | yes | no | no | yes | no | no | no |
| `http.*` | no | no | no | yes | yes | no | no |
| `invoke` | no | no | no | yes | yes | no | no |
| `log`, `fail` | no | no | no | yes | yes | no | no |
| `reveal`, `erase` | no | no | no | yes | **no** | no | no |
| `emit` | yes | no | no | no | no | no | no |
| `put`/`patch`/`update`/`delete` | no | no | yes | no | no | no | no |
| `reject`, `invalid` | yes | yes | no | no | no | with `-> Outcome` | no |

Four rules do all of that work:

- **A projector has no clock and no network**, because a rebuild has to reproduce every value it
  wrote.
- **A command has a clock but cannot call out**, because only an effect journals a call.
- **A module `fn` is pure**, which is the whole of what makes it callable from a command, a projector
  and a fold arm alike.
- **An effect-local `fn` may call out but not decrypt and not read a clock**, because `now()` is
  pinned into a slot the arm fills and `reveal`/`erase` stay in the arm for the erase-last analysis.

**No fold arm calls any of them.** A fold is the definition of a state variable, and a definition
that reached the network would be a different value on every read.

**A test calls none of them.** It states inputs and expectations; `given`, `respond` and `erased`
script the world instead.

## 5. Deliberately absent

Each of these is a decision, not a gap.

- **No `+` on strings, no `str()`, no `.to_string()`, no format specifiers.** Interpolation is the
  whole mechanism and the JSON text table is the whole text form. `Int.pad(width)` is the one thing a
  specifier would have been reached for that the table cannot say, and it is a method rather than a
  specifier so nothing about the string syntax changes.
- **No `sort`, `map`, `filter` or `fold` methods.** A comprehension covers map and filter, iteration
  order is already defined. `fold` is the one that is genuinely missing: a total over a container has
  no spelling, because a `let` cannot accumulate across a `for` body and a `fn` is not recursive.
- **No set type and no tuple type.** `Map(K, Bool)` covers membership and a record covers two values
  that travel together.
- **No `x.expect("reason")`.** `unwrap_or` and narrowing cover it without a panic.
- **No random, no `uuid4`, no minted identity.** `Uuid.derive` is a pure function of its arguments.
- **No duration type, and no `add_months` or `add_years`.** The fixed-length units are in the
  language because they carry no opinion; the calendar ones are the opinion, so the calendar fields
  plus `from_parts` make them writable as a `fn`, which is where the clamping rule belongs.
- **No regular expressions.** `contains`, `starts_with` and `after_last` cover what came up.
- **No `Money` conversion and no rate type.** Both need currency back in the type, and currency is
  deliberately not in the type: declare an ordinary field beside the amount.
- **No `while`, no `break`, no `continue`, no recursion, no mutable bindings, no closures, no
  generics, no overloading, no default or named `fn` arguments.**
