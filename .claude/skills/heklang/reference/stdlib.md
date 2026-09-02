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
| `to_int()` | `Int?` | |
| `to_uuid()` | `Uuid?` | |

`strip_prefix` is written after a `starts_with` that already decided, and `after_last` exists so that
`gid.after_last("/")` is safe on something that is not a global id. The two conversions return
optionals, because there the failure is the point.

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

`year()`, `month()`, `day()`, `hour()`, `minute()`, `second()`, each an `Int`, each in UTC. There is
no `add`, no duration type and no `format`: calendar arithmetic is written as a `fn`, because
month-end clamping is one opinion among several.

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
| `reject <Name>` | `Outcome` | a declared refusal, as a value |
| `invalid(message)` | `Outcome` | the same, for a malformed request |

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
| `erase(value)` / `erase(subject, value)` | nothing | yes |
| `log(message)` | nothing | **no** |
| `fail(message)` | nothing, terminal | n/a |
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
  whole mechanism and the JSON text table is the whole text form.
- **No `sort`, `map`, `filter` or `fold` methods.** A comprehension covers map and filter, iteration
  order is already defined, and a fold over a container is a `for` inside a pure `fn`.
- **No set type and no tuple type.** `Map(K, Bool)` covers membership and a record covers two values
  that travel together.
- **No `x.expect("reason")`.** `unwrap_or` and narrowing cover it without a panic.
- **No random, no `uuid4`, no minted identity.** `Uuid.derive` is a pure function of its arguments.
- **No duration type and no timestamp arithmetic.** The calendar fields plus `from_parts` make it
  writable as a `fn`, which is where the clamping rule belongs.
- **No regular expressions.** `contains`, `starts_with` and `after_last` cover what came up.
- **No `Money` conversion and no rate type.** Both need currency back in the type, and currency is
  deliberately not in the type: declare an ordinary field beside the amount.
- **No `while`, no `break`, no `continue`, no recursion, no mutable bindings, no closures, no
  generics, no overloading, no default or named `fn` arguments.**
