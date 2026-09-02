# Effects

An effect reacts to appended events with **durable side effects**: HTTP calls, invoking commands,
crypto-shredding. It is the only declaration that reaches outside the process. A command and a
projector are pure functions of the log, so replaying either is free; an effect's replay is bought
with a journal, and most of the rules below are about paying for it honestly.

```hek
effect NotifyCustomer {
  on @order.placed as e {
    fold orders: Int = 0
      on @order.placed(customer_id: e.customer_id) => orders + 1

    let response = http.post("https://mail.example/confirm", {
      "to": reveal(e.email),
      "order_id": e.order_id,
      "first_order": orders == 1,
    })

    if response.status >= 400 {
      fail("confirmation rejected")
    }

    invoke RecordNotified {
      order_id: e.order_id,
      notification_id: Uuid.derive(e.id, "confirmation"),
    }
  }
}
```

## The principle

> **The handler sees only what it can act on; the operator sees everything.**

A result that reaches a handler is always terminal and always decidable. Retryable HTTP statuses
never arrive, retryable command outcomes never arrive, and a wedged invocation is invisible to the
script while being prominent in operational status. The author writes the decision; the runtime
writes the retry.

## 1. One arm per event type

An effect is a set of `on` arms, and one event selects **exactly one**. Two arms naming the same
event type is a compile error pointing at the first. (A projector is the opposite: fan-out is the
point there.)

**An arm may list several event types:**

```hek
on @shop.reconnected,
   @warranty.plan.created,
   @warranty.plan.updated as e { shop_id } { ... }
```

The trigger binding then names **only the fields the listed types share**, and a field counts as
shared only when its type *and* its `@subject` match on every listed path. That keeps a `reveal`
through a multi-path binding sound.

Use a multi-path arm rather than copying a body. Where arms genuinely differ in the body and not only
in the trigger, they stay separate arms.

## 2. State lives inside the arm

`as e` binds the trigger and is in scope for the arm's `fold` filters and its body. There is **no**
effect-level trigger binding and no effect-level state.

An arm stages exactly like a command body: a run of `fold` declarations is one read, a statement
below one closes it, and a later run may filter on what an earlier one folded. There is **no
`guard`** in an effect, in either shape: an effect has no append condition to build and no `Outcome`
to refuse with.

A `let` in an arm is an ordinary statement and may call out.

## 3. The fold stops at the trigger's own position, inclusive

`fold` is folded over the log up to **and including** the triggering event, never to the head. It is
therefore a pure function of the log prefix and that position, so every attempt and every replay
reproduces it. Three consequences:

- **it cannot race**, so an effect has no read of a projector;
- **it is not journaled**, so a filter, a fold seed and a fold arm may not call out, invoke,
  decrypt or read a clock. Each is a compile error naming the fold rather than the builtin;
- **it counts the trigger**, so an effect folding its own trigger type sees itself, and a customer's
  first order leaves a count of one, not zero.

## 4. `fail("reason")` is the author's terminal outcome

`fail` records the position as failed and advances the cursor. It is the **only** author-invoked
failure; a runtime error wedges instead.

| Outcome | Meaning | Advances |
| --- | --- | --- |
| done | the arm ran to the end | yes |
| failed | the author judged this event unprocessable | yes |
| skipped | the runtime could not proceed, terminally (a shredded key) | yes |
| wedged | the runtime could not proceed, and retrying might help | **no** |

`fail` is safe precisely because `failed` is a first-class operational signal that never collapses
into the wedge count.

## 5. What never reaches the handler

**Retryable HTTP statuses.** 408, 425, 429 and any 5xx are absorbed by the runtime with backoff. A
`status >= 400` that does reach a handler is a real decide-what-to-do failure. Do not write retry
logic: every response reaching a handler is journaled, so a handler that failed on a 429 would replay
the recorded 429 forever. Re-sending is something only the runtime can do.

**Retryable command outcomes.** `Conflict` and `Unavailable` have no variant in the type at all.

**A wedge.** The script cannot observe that it is being retried, cannot count attempts, and cannot
behave differently on the third one. There is no `retry(...)`.

## 6. `invoke` returns an `Outcome`

| Case | Meaning |
| --- | --- |
| `Ok` | the command committed, possibly emitting nothing |
| `Invalid(msg)` | the input was malformed |
| `Reject(code, msg)` | the command refused on state grounds |

Read it with `.ok()`, `.code() -> String?`, `.message() -> String?` and
`.refused(<RefusalName>) -> Bool`. An already-committed call under its idempotency tag collapses into
`Ok`, which is what exactly-once means.

## 7. `invoke` input is a typed struct

```hek
invoke RecordNotified { order_id: e.order_id, notification_id: Uuid.derive(e.id, "confirmation") }
```

Checked at compile time against the target command's declared parameters: an unknown field, a missing
one, a duplicate, or a value of the wrong type is an error with the field's own span, and each value
is parsed with the parameter's declared type as its hint.

**A JSON object literal must not leak into `invoke`.** A command's input has a schema; an HTTP body
does not.

## 8. `Json` and the wire

`Json` is opaque, with fallible one-step accessors. Every one is optional, because a body is the one
value nothing in the program declared, and a missing key and a wrong shape both answer `none`.

| Accessor | Returns |
| --- | --- |
| `body.string(key)` | `String?` |
| `body.int(key)` | `Int?` |
| `body.bool(key)` | `Bool?` |
| `body.json(key)` | `Json?` |
| `body.array(key)` | `List(Json)?` |
| `body.number(key)` | `String?`, the exact text of a JSON number |

There is no dynamic field access and no indexing. `Json.empty` is what makes a chain read as one
line:

```hek
response.body.json("data").unwrap_or(Json.empty).array("errors").unwrap_or([])
```

`Json` is a **declarable type**, so a command parameter, a `fn` parameter and a `fn` return may be
one, and an object literal is legal anywhere a `Json` is expected.

### The conversion table

A JSON object literal is `{ "key": expr, ... }`. Values convert by a total table, which is part of
the contract because it decides what a remote service receives:

| heklang | JSON |
| --- | --- |
| `Bool` | boolean |
| `Int` | number |
| `Decimal(n)` | **string** at scale `n`, e.g. `"0.0825"` |
| `Money(n)` | **string** at scale `n`, e.g. `"25.99"` |
| `String` | string |
| `Uuid` | string |
| `Timestamp` | number, epoch microseconds |
| an enum | string, the variant name |
| a record | object, one key per field |
| `List(T)` | array |
| `Map(K, V)` | object, keys as their text form, sorted |
| `none` | `null` |
| `some(x)` | whatever `x` converts to |

`Money` and `Decimal` become strings so no precision is lost to a float on the far side. Object keys
are sorted, so the same object built twice serialises byte-identically.

**A number the author typed stays a JSON number.** `{ "amount": 10.5 }` sends `10.5`, while a
`Money(2)` variable holding the same amount sends `"10.50"`. Replacing a literal with a variable
changes the wire form, which is intended and worth knowing before a refactor. A `Json` number is
carried as exact text, never an `f64`, so two numbers are equal when they are spelled the same:
`3` and `3.0` do not compare equal.

**A number read out of a body comes back as text**, and the scale belongs to where it lands:

```hek
invoke Record {
  price: Money.parse(response.body.number("price").unwrap_or("0")).unwrap_or(0.00),
  rate: Decimal.parse(response.body.number("rate").unwrap_or("0")).unwrap_or(0.0000),
}
```

`Json.encode(value) -> String` is the same table pointed at a string instead of a socket, for an API
that takes a JSON document as a string field.

### Headers

```hek
http.post(url, body, headers = { "Authorization": "Bearer {token}" })
```

A **named** argument, after the others, on every verb. The case that matters is an
`Idempotency-Key`: the journal key is the verb, the URL and the body, and deliberately **not** the
headers, so a replay that recomputes a different key still lands on the entry that recorded the send.

Timeouts are configuration, not syntax. There is no third positional argument.

## 9. Erase last, statically enforced

A `reveal` **reachable after** an `erase` within one arm is a compile error. `erase` is journaled and
`reveal` is not, so a replay skips the erase and then re-runs the reveal against a key that is gone.

It is a reachability analysis over the arm's control flow, not a lexical check:

```hek
if x {
  erase(e.customer_id)
  fail("gone")
}
reveal(e.email)                   // legal: the erase path never reaches here

for id in ids {
  log(reveal(e.email))            // rejected: the erase below reaches it on the next turn
  erase(e.customer_id)
}
```

A `for` body iterates to a fixed point, so an `erase` anywhere in one is reachable from every
`reveal` in it, including one lexically above.

### The two forms of `erase`

```hek
erase(e.shop_id)          // inferring: the value must be a field of the triggering event
erase(customer_id, id)    // named: the subject name, then the value
```

`erase(value)` recovers the subject from the value, which must be a trigger field whose
`@subject(...)` declaration says which key namespace it names. When the id comes from a fold there is
no name to recover, so the second form supplies it. That is the shape a mandatory shop-wide or
tenant-wide redaction has.

The named form gives up the check that the value really is a `customer_id`, so **the inferring form
stays the default**. Three things are still checked: the name is a declared subject, the value's type
matches the field the keys are filed under, and the value contains no `reveal`.

`erase` is a statement and returns nothing, because there is nothing an author could do differently
on either answer.

## 10. Builtins, and what is journaled

| Builtin | Returns | Journaled |
| --- | --- | --- |
| `http.get(url)` | `Response` | yes |
| `http.post(url, body)`, `.put`, `.patch`, `.delete` | `Response` | yes |
| `invoke Name { ... }` | `Outcome` | yes |
| `now()` | `Timestamp` | yes, pinned once per invocation |
| `erase(value)` / `erase(subject, value)` | nothing | yes |
| `Uuid.derive(seed, name)` | `Uuid` | pure |
| `log(message)` | nothing | **no**, so it may appear twice across a crash |
| `reveal(value)` | the sealed type, unsealed | **no**, re-decrypts every attempt |

Nothing marks the unjournaled two in the syntax.

**There is no `Uuid.new`, no `Uuid.random` and no `random`, anywhere in the language.** A command
retry and an effect replay both have to derive the same id they derived the first time, and the only
way to guarantee that is to have no other option. `Uuid.derive(seed, name)` derives one from an
identity that already exists, and `e.id` is the seed most handlers want. The rejected spellings are
recognised and each points at `derive`.

**The clock rule.** `now()` is available in a command body (pinned once per request) and in an effect
arm (journaled); it is absent in a `fold` of either kind, in a projector, in a module `fn` and
in an effect-local `fn`. It is pinned **once**, not per call, so two calls in one body read the same
value.

## 11. An effect-local `fn`

```hek
effect SyncShop {
  fn sync(shop_id: Int, domain: String, secret: String) {
    let response = http.post("https://{domain}/admin/api/sync", { "shop": shop_id },
      headers = { "X-Access-Token": secret })
    if response.status == 401 {
      log("shop {shop_id} rejected the token, skipping")
      return                          // leaves the helper, not the arm
    }
    if response.status >= 400 {
      fail("sync rejected with status {response.status}")
    }
  }

  on @shop.sync.requested as e { shop_id } { ... sync(shop_id, domain, reveal(token)) }
  on @shop.reconnected as e { shop_id, shop_domain, access_token } {
    sync(shop_id, shop_domain, reveal(access_token))
  }
}
```

Visible inside its own effect and nowhere else. It **may** `http.*`, `invoke`, `log` and `fail`.

It **may not**:

- `reveal` or `erase`. Those stay in the arm, which is what keeps the erase-last analysis over one
  statement tree. **Pass the already-revealed value in as a parameter.**
- declare a `fold`. A fold belongs to the arm; pass what it decided in.
- read `now()`. The clock is pinned into a slot the arm fills; read it in the arm and pass it in.
- `emit` or write a read model.
- shadow the name of a module `fn`.

It **may omit its return type**, the only signature in the language that may, because a helper with
effects and no result is honest. A call to a void one is a **statement**, never an expression. A
`fail` anywhere ends the invocation; a bare `return` leaves only the helper.

**A fold arm may not call an effect-local `fn`**, because a fold must reproduce without a journal.

## 12. `reveal` and the seal

`@subject(customer_id)` on an event field is the authored form; `Sealed(String, customer_id)` is what
propagates from it, and `Sealed` is not spellable. `Opt` stays outermost, so
`String? @subject(x)` is `Opt(Sealed(String, x))`.

**A seal survives a `let`, a fold, a parameter and a column**, because it lives in the type rather
than in how an expression was spelled. A sealed value carries the field it was sealed under, the
subject, the id its key is filed under, and the content as a host stored it. heklang never reads the
content.

**`@subject(x)` must name a field of the same event, `x` may not itself be subject-bound, and `x` may
not be optional.** A subject id is the name a key is filed under, so a missing id is not "no key", it
is no question at all.

### What may be done to sealed content

| | |
| --- | --- |
| **Move it** into a position sealed under the same subject: a `let`, a `fold`, an entity column, another event field declared `@subject(<same name>)` | the content is never read |
| **Ask if it is there**: `.is_some()` / `.is_none()` | presence is not content |
| **`reveal` it**, in an effect arm | the boundary itself |

Everything else is a compile error. Each of these is rejected:

```hek
http.post(url, { "email": e.email })     // cannot be sent in a request body
log("email is {e.email}")                // cannot be interpolated
log(e.email)                             // a String is not sealed content
invoke RecordCopy { note: e.email }      // takes it out from behind the boundary
if e.email == "x" { }                    // cannot be compared
e.email.trim()                           // `trim` reads content sealed under `customer_id`
e.email.unwrap_or("")                    // a plaintext default and sealed content in one slot
keep(e.email)                            // a `fn` parameter is a plain type, so it is not a move
```

A destination is sealed only when it is declared under the **same subject name**: moving content
sealed under `customer_id` into a field declared `@subject(shop_id)` is rejected, because a key is
filed under exactly one subject. There is no way to declare a sealed `fn` parameter, a sealed record
field or a sealed container element, so `reveal` at the point of use and pass plaintext onward.

**Writing plain content into a seal is free**: a command holding an ordinary `String` may `emit` it
into a `@subject(...)` field with no ceremony. Only reading back out needs `reveal`.

### Folding a credential out of the log

A credential is almost never on the event being handled, so the seal propagates through a `fold`
fold:

```hek
fold token: String? = none
  on @shop.connected(shop_id) { access_token } => access_token
  on @shop.reconnected(shop_id) { access_token } => access_token

let secret = reveal(token)
if secret.is_none() {
  log("shop {shop_id} has never connected, nothing to sync")
  return
}
sync(shop_id, domain, secret)          // `secret` is a String here, proved by the branch
```

An arm seals the variable when its result **is** sealed content. A transformed arm
(`=> access_token.trim()`) is rejected where it is written.

**One variable, one subject.** Two arms folding under different subject fields into one variable is
an error naming both. **A plain seed is fine, a plain arm is not**: an arm folding a non-sealed value
into a variable another arm makes sealed is an error in either declaration order.

### An optional in, an optional out

`reveal(x: T?)` is `T?` and `reveal(x: T)` is `T`. Three states, and two of them must not collapse:

| Held value | Key | Result |
| --- | --- | --- |
| absent | irrelevant | `none`, an ordinary condition to branch on |
| present | exists | the plaintext |
| present | shredded | **terminal**: the invocation is skipped, counted apart from wedges |

"Never set" and "key destroyed" are different facts, so never use `""` or `0` as an absent-credential
sentinel. A shredded key fails terminally rather than returning `none`, and the failure may be
non-local: another effect or a concurrent invocation can erase a subject between a run and a replay.

## 13. No effect may trigger itself

heklang builds a graph over event types with an edge `trigger -> emitted` for every (arm, invoked
command, emitted event) and rejects a cycle, naming the path:

```
@order.placed -> NotifyCustomer -> RecordNotified -> @order.placed:
this effect can trigger itself, so the log would grow without end
```

It rejects a program that *can* loop rather than one that provably does, which is the safe direction.

## 14. Determinism, and what verify mode still covers

Removed by the language: iteration order (map keys and object keys are sorted), the clock (pinned and
journaled), randomness (there is none), and reads of mutable state (an effect has no read of a
projector; a projector has no general read).

Still covered by folding twice and comparing: a subject re-keyed between a run and its replay, a
journal read back by a different program version, and anything a future builtin adds.
