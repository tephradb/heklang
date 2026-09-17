# The standard library

Everything an author can call that is not something they declared. It is small on purpose, and this
document is the one place it appears whole.

**This is a reference, not a contract.** Every rule here is argued somewhere else, and that somewhere
else wins: `docs/strings.md`, `docs/containers.md`, `docs/optionals.md`, `docs/money.md` and
`docs/effects.md` each own their part. What was missing was a single page an author could read before
knowing which of those five to open.

In code the surface is two tables. `method_sig` in `src/types.rs` answers what a method takes and
returns, for every receiver; `Builtin` in `src/ir.rs` and the callables in `src/parse.rs` are the
free ones. A method that is not in the first table does not exist, and the error names the receiver.

## 1. Methods

The receiver's type decides what it has. There is no method every type carries, not even a text form:
a value becomes text through interpolation and that table is `docs/effects.md` rule 8.

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

`strip_prefix`, `after_last` and `truncate` return a `String` rather than an optional, and all three
defaults are deliberate. `strip_prefix` is written after a `starts_with` that already decided,
`after_last` exists so `gid.after_last("/")` is safe on something that is not a gid, and `truncate`
is written by an author who has already decided what happens to the tail. The two conversions do
return optionals, because there the failure is the point.

`truncate` counts characters, which is what `len()` reports and what `@max(n)` bounds, so
`text.truncate(n)` satisfies `@max(n)` and is what meets a bound the annotation will not meet on its
own. `docs/strings.md` has the argument, and the three things it is easy to get wrong: truncate last,
`String?` is not a receiver, and a sealed receiver is refused where a sealed destination is free.

### `Json`

| Method | Returns |
| --- | --- |
| `string(key)` | `String?` |
| `int(key)` | `Int?` |
| `bool(key)` | `Bool?` |
| `json(key)` | `Json?` |
| `array(key)` | `List(Json)?` |
| `number(key)` | `String?`, the exact text of a JSON number |

Every one is optional, because a response body is the one value nothing in the program declared. A
missing key and a key holding the wrong shape both answer `none`, so an author writes one branch
rather than two.

The one-step form is deliberate: `docs/effects.md` rule 8 rejects `body.get("id").as_string()`
because every read of an untyped body is a branch anyway and the two-step form makes the author write
two of them. There is no dynamic field access and no indexing. `Json.empty` is what makes a chain
read as one line:

```
response.body.json("data").unwrap_or(Json.empty).array("errors").unwrap_or([])
```

`number` hands back **text** rather than a number, because a `Decimal` or a `Money` needs a scale and
the scale belongs to the target rather than to the digits on the wire. Pair it with `Money.parse` or
`Decimal.parse` at a declared position and nothing is rounded on the way. `int` answers only for a
whole number, so `10.5` reads as `none` there.

`Json` is also a declarable type, so a command parameter, a `fn` parameter and a `fn` return may be
one, and an object literal is legal anywhere a `Json` is expected.

### `T?`

| Method | Returns |
| --- | --- |
| `unwrap_or(T)` | `T` |
| `is_some()`, `is_none()` | `Bool` |

Three, and there is no `expect`. `docs/optionals.md` also has narrowing, which is what removes most
of the calls an author would otherwise write.

### `List(T)`

| Method | Returns |
| --- | --- |
| `first()` | `T?` |
| `push(T)` | `List(T)` |
| `remove(T)` | `List(T)` |
| `contains(T)` | `Bool` |
| `len()` | `Int` |
| `is_empty()` | `Bool` |

`push` and `remove` build a new list rather than mutating one, which is what keeps a fold arm's
result a new state and keeps a value nothing can change once it is handed over. `remove` removes
**every** equal element, not the first, so it is idempotent the way a map's is.

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

`keys` and `values` come back sorted by key, and `docs/containers.md` explains why that is
load-bearing rather than incidental: a projector that iterates a map has to write the same rows on a
rebuild.

### `Money(n)`

| Method | Returns |
| --- | --- |
| `mul(Decimal(s), Rounding)` | `Money(n)` |
| `div(Int, Rounding)` | `Money(n)` |

The two places money is allowed to round, and the author says how. `Rounding` is `HalfUp`, `HalfEven`
or `Down`, written as a bare name. The scale stays the amount's, because a rate applied to an amount
is still that amount; the rate's own scale is whatever the author wrote, so `total.mul(0.9, HalfUp)`
takes a `Decimal(1)`.

Everything else is the operator table in `docs/money.md`, and the bare operators are errors rather
than rounders where the result is not exact. Money never rounds silently.

### `Timestamp`

| Method | Returns | |
| --- | --- | --- |
| `year()`, `month()`, `day()`, `hour()`, `minute()`, `second()` | `Int` | in UTC |
| `add_seconds(n)`, `add_minutes(n)`, `add_hours(n)`, `add_days(n)` | `Timestamp` | `n` may be negative |

The calendar fields exist so calendar arithmetic is writable as a `fn`, which is where the opinion
about month-end clamping belongs: the language gives the calendar and the author gives the rule.

**The fixed-length units are here, because they have no opinion to defer.** A minute is sixty seconds
wherever it lands and a day is twenty-four hours in UTC, so `add_minutes` has nothing to clamp and no
rule for an author to disagree with. `add_months` and `add_years` are exactly the ones the argument is
about, and they stay absent; the refusal now names the line rather than denying arithmetic outright.

Three things follow from a `Timestamp` being epoch microseconds:

- **A count, not a magnitude.** `add_minutes(0 - 30)` goes backwards, so there is no second `sub_`
  family to remember.
- **Sub-second precision survives**, which is the thing the `fn` could not do. `Timestamp.from_parts`
  is on the second and says so, so every hand-written `add_minutes` that went out through the calendar
  fields and back through `from_parts` silently truncated microseconds.
- **Overflow is an error rather than an optional**, the same answer `Int` and `Money` arithmetic give.
  `from_parts` is optional because Feb 30 is not a date; five minutes after a real moment always is
  one, so an optional there would be a branch with nothing on the other side of it.

There is still no duration type and no `format`. A moment becomes text through interpolation, and
rule 8's table renders one as epoch microseconds.

### `Int`

| Method | Returns | |
| --- | --- | --- |
| `pad(width)` | `String` | zero-filled on the left; the number's own text when it already fits |

The only `Int` method, and it is about text rather than arithmetic: the arithmetic is the operators.
It exists because interpolation has no format specifiers, so `"{y}-{mo}"` writes `2026-9`, and a
billing-period key or a bucket match that is silently wrong is worse than one that is visibly wrong.

`"{y}-{mo.pad(2)}"` is the shape it is for, which is why the receiver is the number rather than the
string: `"{mo}".pad_start(2, "0")` would make the common case a nested interpolation.

The sign comes first and the zeros after it, because `0-5` is not a number, and `width` counts the
whole rendering, so `pad(3)` answers three characters on either side of zero. A width at or below zero
pads nothing, the same way `String.truncate` at or below zero keeps nothing. It is the pair of
`truncate`: one bounds a string above and one bounds it below, and both count the characters `len`
counts.

**A width past 4096 is a runtime error**, and it is the one place in the language where a bound had to
be picked rather than derived. `pad` is the only method that makes a string longer and its width is an
ordinary expression, so without the bound `id.pad(w)` allocates whatever `w` says and a width that
reached a program from a request parameter could take the process down. A total language whose
handlers cannot crash the runtime cannot also have that. The number is the widest `@max` any real
declaration carries, and a width past the longest string a field could hold is not a field's shape any
more. The error says so, rather than blaming arithmetic. `truncate` needs no such rule, because it can
only shrink.

### `Outcome`

| Method | Returns |
| --- | --- |
| `ok()` | `Bool` |
| `code()`, `message()` | `String?` |
| `refused(<Name>)` | `Bool` |

What an `invoke` answers with, and what a `fn` declared `-> Outcome?` produces. Three variants
(`docs/effects.md` rule 6), and these methods read them without a match form. `refused` takes a
declared refusal and answers whether this is it (`docs/refusals.md`); an `invalid` carries no code,
so it is refused by nothing.

`Outcome` is spellable in a `fn` parameter and return type and nowhere else, the same allowance
`Response` has: a refusal is a decision, not data. It is the type of an `invoke`'s result and of a
`fn` that decides a refusal on a command's behalf. `reject <Name>` and `invalid "<message>"` do not
construct one: they are statements rather than values, and a `fn` that declared `Outcome` answers
with them directly (`docs/refusals.md`). `docs/functions.md` has the rule, and `docs/refusals.md`
has the declaration the name resolves to.

### `Response`

`.status` is an `Int` and `.body` is a `Json`, **without parentheses**. This is the only place in the
language where field access on a builtin type is parenless, and it is a deliberate exception rather
than a general rule: a response is the one builtin that is a record in everything but name.

## 2. Constructors

The global namespace is closed. There is no bare `uuid4()`, `random()` or `now`-like constructor
hiding in it; anything that makes a value out of nothing is qualified by the type it makes.

| Call | Returns | |
| --- | --- | --- |
| `Uuid.derive(seed, name)` | `Uuid` | a v5 UUID, a pure function of both arguments |
| `Json.empty` | `Json` | |
| `Json.encode(value)` | `String` | rule 8's table pointed at a string instead of a socket |
| `Map.empty` | `Map(K, V)` | the type comes from the target |
| `Timestamp.parse(text)` | `Timestamp?` | RFC 3339, optional fraction and zone |
| `Timestamp.from_parts(y, mo, d, h, mi, s)` | `Timestamp?` | optional because Feb 30 is not a date |
| `Money.parse(text)` | `Money(n)?` | the scale comes from where the result lands |
| `Decimal.parse(text)` | `Decimal(n)?` | the same, for a rate rather than an amount |

`Money.parse` and `Decimal.parse` are the ones whose type is not in the call: `"10.5"` is a
different value at scale 2 and at scale 3, so the target decides and a call with no target is a
compile error. `Map.empty` takes its
type the same way, and so does `[]`. `docs/containers.md` lists the positions that count as a target,
and the reason `let` is not one of them.

There is no `List.empty`, because `[]` already writes it. There is no `Map` literal, because `{ ... }`
is a JSON object.

**`Uuid.derive` is the whole identity story**, and `docs/effects.md` rule 11 spends four paragraphs on
it. An id derived from its inputs is the same id on a command retry and on an effect replay; a minted
one is not, and no amount of journaling fixes a value that was already written to the log. Writing
`uuid4()` produces an error that says so and points here.

## 3. What a handler does

These are not functions in the sense above. They reach the world, and the right-hand column is
`docs/effects.md` rule 11's, which is also the line the host seam is cut on (`docs/host.md`).

| Call | Returns | Journaled |
| --- | --- | --- |
| `http.get(url)` | `Response` | yes |
| `http.post(url, body)`, `.put`, `.patch` | `Response` | yes |
| `http.delete(url)` | `Response` | yes |
| `invoke Name { ... }` | `Outcome` | yes |
| `now()` | `Timestamp` | yes, pinned once per invocation |
| `erase(value)` / `erase(subject, value)` | nothing | yes |
| `log(message)` | nothing | **no** |
| `fail "<message>"` | nothing, terminal | n/a |
| `reveal(value)` | `String` | **no**, re-decrypts every attempt |

Every verb takes an optional named `headers = { ... }` after its other arguments. A timeout does not
go there; rule 13 puts it in configuration.

The two that are not journaled are the interesting ones. `log` is not a side effect the world can
observe, so replaying it costs nothing and recording it would grow the journal with noise. `reveal`
is not journaled because writing a decrypted value into a durable record is the one thing crypto
shredding exists to prevent, so it is re-done on every attempt instead.

`fail` is the author's terminal outcome and the only author-invoked way to stop an arm. `erase` is a
statement rather than an expression because there is nothing an author could do differently on either
answer, and `log` is a statement for the same reason.

**`fail` takes no parens and the other two do**, and the line between them is not statement against
expression: it is terminal against not. `fail` is one of the three answers, beside `reject` and
`invalid`, and saying an answer ends the declaration (`docs/refusals.md`). `log` and `erase` come
back to the next statement, so they stay calls, and parens are what a call takes. The guarantee runs
one way only, since `emit` and `put` take no parens and are not terminal; what catches a statement
written after an answer is a diagnostic rather than its shape.

## 4. Where each one is allowed

The pure surface, everything in sections 1 and 2, is available everywhere. What reaches the world is
not, and the refusals are what make a projector reproducible and a command decidable.

| | command | projector | effect arm | effect-local `fn` | module `fn` |
| --- | --- | --- | --- | --- | --- |
| `now()` | yes | no | yes | no | no |
| `http.*` | no | no | yes | yes | no |
| `invoke` | no | no | yes | yes | no |
| `log`, `fail` | no | no | yes | yes | no |
| `reveal`, `erase` | no | no | yes | no | no |

Four rules are doing all of that work:

- **A projector has no clock and no network.** A rebuild has to reproduce every value it wrote, and a
  projector's rows are its output rather than anyone's input (`docs/projectors.md` rule 4).
- **A command has a clock but cannot call out.** It decides from state, and only an effect journals a
  call.
- **A module `fn` is pure.** That is the whole of what makes it callable from a command, a projector
  and an effect alike (`docs/functions.md`).
- **An effect-local `fn` may call out but not decrypt and not read a clock.** `now()` is pinned into a
  slot the arm fills before its body runs, and a helper has no such slot, so reading it there would
  mean something different inside a call. `reveal` and `erase` stay in the arm because rule 12 tracks
  the seal on a value and a helper is where that trail would go cold.

**No fold arm calls any of them.** The four statements cannot appear in one at all, since a fold arm
is an expression, and the rest are refused by name. A fold is the definition of a state variable, and
a definition that reached the network would be a different value on every read.

**A test calls none of them.** It states inputs and expectations; `docs/testing.md` has `given`,
`respond` and `erased`, which are how a test scripts the world instead.

## 5. What is deliberately absent

Each of these is a decision with an argument behind it, not a gap waiting to be filled.

- **No `+` on strings, no `str()`, no `.to_string()`, no format specifiers.** Interpolation is the
  whole mechanism and rule 8's table is the whole text form. A second spelling could drift from it.
  `Int.pad(width)` is the one thing a specifier would have been reached for that the table cannot
  say, and it is a method rather than a syntax for exactly that reason.
- **No `sort`, `map`, `filter` or `fold` methods.** A comprehension covers map and filter, iteration
  order is already defined, and a fold over a container is a `for` inside a pure `fn`.
- **No set type and no tuple type.** `Map(K, Bool)` covers membership and a record covers two values
  that travel together. The one place a port wanted a set it wanted an ordered one, which is a list.
- **No `x.expect("reason")`.** `unwrap_or` and narrowing cover it without a panic.
- **No random, no `uuid4`, no minted identity.** `Uuid.derive` is a pure function of its arguments,
  and that is a language guarantee rather than a convention.
- **No duration type, and no *calendar* timestamp arithmetic.** `add_months` and `add_years` are the
  ones month-end clamping is an argument about, and the calendar fields plus `from_parts` make them
  writable as a `fn`. The fixed-length units are in the language, because they have no clamping
  question: see the `Timestamp` table above.
- **No regular expressions.** Nothing in a real port wanted one that `contains`, `starts_with` and
  `after_last` did not cover.
- **No `Money` conversion and no rate type.** All of it needs currency back in the type, and
  `docs/money.md` is the argument for why it is not there.

## Related

- `docs/strings.md`: interpolation, the raw form, the text table, and meeting a `@max`.
- `docs/containers.md`: iteration order, where an empty container's type comes from, comprehensions.
- `docs/optionals.md`: narrowing, and where a bare `T` fills a `T?`.
- `docs/money.md`: the operator table, and why `Money` is not `Decimal(n)`.
- `docs/effects.md`: rules 8, 11, 12 and 13, which are most of section 3.
- `docs/functions.md`: what a `fn` may call, and the two kinds of one.
- `docs/types.md`: the method table's place among the other checks.
- `docs/host.md`: the traits behind the journaled column.
