# Effects

An effect reacts to appended events with **durable side effects**: HTTP calls, invoking commands,
crypto-shredding. It is the only declaration that reaches outside the process. A command and a
projector are pure functions of the log, so replaying either is free; an effect's replay is bought
with a journal, and most of the rules below are about paying for it honestly.

```hek
effect NotifyCustomer {
  on live @order.placed as e { @key customer_id } {
    fold orders: Int = 0
      on @order.placed(customer_id: e.customer_id) => orders + 1

    let response = http.post("https://mail.example/confirm", {
      "to": reveal(e.email),
      "order_id": e.order_id,
      "first_order": orders == 1,
    })

    if response.status >= 400 {
      fail "confirmation rejected"
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
   @warranty.plan.updated as e { @key shop_id } { ... }
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

## 4. `fail "<reason>"` is the author's terminal outcome

`fail` records the position as failed and advances the cursor. It is the **only** author-invoked
failure; a runtime error wedges instead.

The reason is a written message, the way `invalid`'s is: a value reaches it through a hole, so it is
`fail "sync rejected with status {response.status}"` and never `fail why`.

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
  fail "gone"
}
reveal(e.email)                   // legal: the erase path never reaches here

for id in ids {
  log(reveal(e.email))            // rejected: the erase below reaches it on the next turn
  erase(e.customer_id)
}
```

A `for` body iterates to a fixed point, so an `erase` anywhere in one is reachable from every
`reveal` in it, including one lexically above.

### `erase` takes a value whose type is a subject

```hek
erase(e.shop)             // a Shop id, so a shop's key
erase(id)                 // `id` is a Customer, wherever it came from
```

There is one form, because the value's **type** is the namespace. It used to be two: `erase(value)`
recovered the subject by looking at the field the value was loaded from, so it reached trigger fields
and nothing else, and a folded id needed `erase(customer_id, id)` to assert a namespace heklang could
not check. `erase(customer_id, some_other_int)` compiled and destroyed the wrong subject's key. A
subject is a declared type now, so a fold is written `fold ids: List(Customer) = []` and `erase(id)`
says what it destroys.

Two things are checked: the value is a subject id (`erase(e.order_id)` is refused, and so is
`erase(7)`), and it contains no `reveal`, because a repeat request for a subject whose id you learned
by revealing then cannot be read at all.

`erase` is a statement and returns nothing, because there is nothing an author could do differently
on either answer.

## 10. Builtins, and what is journaled

| Builtin | Returns | Journaled |
| --- | --- | --- |
| `http.get(url)` | `Response` | yes |
| `http.post(url, body)`, `.put`, `.patch`, `.delete` | `Response` | yes |
| `invoke Name { ... }` | `Outcome` | yes |
| `now()` | `Timestamp` | yes, pinned once per invocation |
| `erase(value)` | nothing | yes |
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
  fn sync(shop_id: Shop, domain: String, secret: String) {          // Shop is a subject
    let response = http.post("https://{domain}/admin/api/sync", { "shop": shop_id },
      headers = { "X-Access-Token": secret })
    if response.status == 401 {
      log("shop {shop_id} rejected the token, skipping")
      return                          // leaves the helper, not the arm
    }
    if response.status >= 400 {
      fail "sync rejected with status {response.status}"
    }
  }

  on @shop.sync.requested as e { @key shop_id } { ... sync(shop_id, domain, reveal(token)) }
  on @shop.reconnected as e { @key shop_id, shop_domain, access_token } {
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

## 16. Deployment secrets

`secret NAME` declares a credential the deployment owes the program; `secret NAME?` one it may not
set. See `language.md` §9a for the declaration. Two kinds of credential, and only the second is new:

- **Per-tenant** (a shop's OAuth token) arrives as a command, lands in the log as a `@subject` field,
  and an effect folds it out and `reveal`s it. That is rule 12 and it is unchanged.
- **Per-deployment** (a Discord webhook, a Stripe key) is not a domain fact, rotates out of band and
  differs between environments. It is a `secret`, never a `const` and never an event.

One rule: **readable exactly where the network is reachable, and observable nowhere else.**

| May be | May not be |
| --- | --- |
| an `http.*` url, header value or body member | `log`, `fail`, `emit`, `put`, `invoke` |
| a `Secret` parameter or return of an effect-local `fn` | a `fold` seed, arm or filter |
| a `let`, and a string interpolation (which becomes a `Secret`) | compared, in arithmetic, or in a `List`/`Map`/record |
| asked `.is_some()` / `.is_none()` | read outside an effect arm or effect-local `fn` |

An optional one narrows through a `let`, not from a bare read: `let dsn = SENTRY_DSN` then
`if dsn.is_some()`.

**A secret carries two renderings.** The credential goes on the wire; everything else -- the journal
key, a transport failure's message, any print -- gets `{SECRET:NAME}`. So a rotation cannot move a
journal key or a digest hash, which is what keeps replay coverage on invocations already recorded.

In a test, a declared secret answers `secret:NAME` with no setup, and
`secret NAME = "value"` (or `= none`) in the setup section overrides it. See `testing.md`.

## 12. `reveal` and the seal

`@subject(buyer)` on an event field is the authored form; `Sealed(String, Customer)` is what
propagates from it, and `Sealed` is not spellable. `Opt` stays outermost, so
`String? @subject(buyer)` is `Opt(Sealed(String, Customer))`.

The annotation names a **sibling field**, and the type carries the **subject** that field's type
names. Two facts, kept apart: `@subject(buyer)` is local to one declaration, which is what lets
`from: Customer, to: Customer` be sealed under separately, and `Customer` is what a key store files
under. `@subject(x)` is refused unless `x` is a declared subject.

**A seal survives a `let`, a fold, a parameter and a column**, because it lives in the type rather
than in how an expression was spelled. A sealed value carries the field it was sealed under, the
subject, the id its key is filed under, and the content as a host stored it. heklang never reads the
content.

**`@subject(x)` must name a field of the same event whose type is a declared subject, `x` may not
itself be subject-bound, and `x` may not be optional.** A subject id is the name a key is filed
under, so a missing id is not "no key", it is no question at all.

### What may be done to sealed content

| | |
| --- | --- |
| **Move it** into a position sealed under the same subject: a `let`, a `fold`, an entity column, another event field declared `@subject(<a field of the same subject>)` | the content is never read |
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
sealed under `Customer` into a field declared `@subject(shop)` is rejected, because a key is
filed under exactly one subject. There is no way to declare a sealed `fn` parameter, a sealed record
field or a sealed container element, so `reveal` at the point of use and pass plaintext onward.

**Writing plain content into a seal is free**: a command holding an ordinary `String` may `emit` it
into a `@subject(...)` field with no ceremony. Only reading back out needs `reveal`.

### A composite seals whole

`@subject(...)` takes **any declared type**. A record, a list and a map seal as the whole JSON
document rule 8 writes, and `reveal` parses it back against the declared type, so an effect gets an
`Address` rather than its text.

```hek
subject Customer(Int)

record Address {
  line1: String @max(200),
  city: String @max(100),
}

event @order.placed {
  order_id: Uuid,
  customer_id: Customer,
  ship_to: Address @subject(customer_id),
}
```

**This is the shape to reach for.** The alternative is one sealed `String` per part, where adding a
part is a schema-evolution event on every event that carries one, and a `@subject` field takes `?`
and never `@absent`, so evolution is tighter there than anywhere else.

- **The annotation goes on the event field, never inside the record.** `@subject(x)` names a sibling
  field holding the id, and a field reached through a container has no sibling to name. A record
  *type* on a subject-bound event field is fine; `@subject` on a field of a `record` declaration is
  refused.
- **`@max` goes on the record's parts**, because the field's type is the record and there is no
  syntax for a field inside it. An over-length value reports the path (`ship_to.line1`).
- **It goes whole or not at all.** A part of one is not reachable by name: `e.ship_to.city` and
  `for item in e.items` are refused at the seal boundary the way a method call on one is, and
  `reveal` is what opens the document. Erasure is whole in the same way: an erased subject's record
  does not come back with its parts missing, the `reveal` ends the arm exactly as a scalar one does.
- **A seal is read as stored history, not as a request body**, so a part added to the record today
  reads as its `@absent` literal on every seal already written, wherever that seal is revealed. (A
  sealed read-model column is opaque text and is never read against the record, so `@absent` answers
  at the `reveal` and nowhere else.) An `@absent` literal or a `?` is the whole of what makes a
  subject-bound record safe to grow: a part added with neither wedges every `reveal` of every seal
  already written, forever, because the plaintext is behind a key and no migration is available even
  in principle.
- **A `Json` seals whole, quotes and all.** It is the one type that needs saying, because its value
  can itself be a string that looks like another: flattening `Json::Str("42")` to `42` would read
  back as a number and `.string("sku")` would answer `none` for a key that is there. The reader also
  has a depth limit the writer does not check, above what a host's own parser hands over but not
  unbounded, so a `Json` built past it would seal and never reveal.
- **A store handing back something that is not that document is a mismatch**, which is data rather
  than a broken host, and the message names the type the declaration promised.

None of the rules above move. A composite seal is moved, asked about and revealed exactly as a scalar
one is, and a `reveal` of a shredded one is still terminal and still names the field.

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

## 15. Delivery is declared, and a key names the lane

```
on [latest|live] @path[, @path]* [as name] { [@key] field, ... } { body }
```

Every arm names **at least one `@key`**, marking a destructured trigger field as the identity of the
lane the event lands in. Mandatory with no opt-out: an implicit "no key" is a default of one global
lane, which is how one unprocessable event blocks every unrelated aggregate behind it. Marking
several fields forms a composite, and the order is significant, so `{ @key a, @key b }` and
`{ @key b, @key a }` are different lanes.

A key may not be a `@subject`-bound field: choosing a lane means reading the key, and rule 12 lets
sealed content only be moved, asked about or revealed. Key by the subject id instead. A key also has
to be a type that identifies: `Int`, `String`, `Uuid`, `Timestamp` or an enum.

| Form | Delivery | May `invoke` |
| --- | --- | --- |
| `on` | every matching event gets an invocation | yes |
| `on latest` | one invocation per key per batch, at the newest matching position in it | **no** |
| `on live` | only events appended after the effect's first activation | yes |

`on latest` is **not** "skip history". History is processed; the arm runs once per key rather than
once per event, and rule 3 means that one invocation has already seen everything before it. Catching
up, the batch is the whole backlog. Live, it collapses a burst: a merchant editing six plans in a
minute gets one publish.

`on live` says history is not news. The runtime resolves it to a position once, at first activation,
so source states intent and the value cannot rot.

**`latest` may not `invoke`, anywhere it can reach, including through an effect-local `fn`.**
Collapsing keeps one invocation out of N, so a record it writes may or may not still be true, and only
the author knows which. `live` declines whole invocations, so nothing is written and the log truthfully
says nothing happened.

The cost is real: a convergent effect cannot both collapse and record what it did. Before reaching for
`on latest` on an arm that invokes, ask **whether anything reads what it records**, in this order:

1. the arm's own folds: if it reads the record back to decide whether to act, the record is the
   convergence rather than an audit trail, and the arm should stay `on` (it is already self-limiting,
   so it had no reason to collapse);
2. any other declaration (a guard, a projector, another effect);
3. the emitting command's own fold, which is how a `Record...` command usually stays idempotent.

If all three answer no, the record is an audit fact, dropping the `invoke` is unobservable, and the
arm can collapse. In a real 12-effect application one of six candidates was that shape, and it was the
one with the most to gain: fourteen trigger paths down to one publish per shop.

Modifiers and keys are per arm, not per effect, and an effect may mix them. Under `hek test`,
`deliver` is a catch-up batch, so an `on latest` arm collapses there too; an `on live` arm is
delivered as `on`, because the boundary is a runtime fact and not a property of the log.
