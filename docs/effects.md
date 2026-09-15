# Effects

An effect reacts to appended events with **durable side effects**: HTTP calls, invoking commands,
crypto-shredding. It is the fourth declaration kind and the first one that reaches outside the
process, which is what makes its rules interesting. A command and a projector are pure functions of
the log, so replaying either is free. An effect's replay has to be bought, with a journal, and most
of what follows is about paying for it honestly.

This document is the contract. `tests/effects.rs` is the same set of rules as executable tests, one
test per numbered rule. Change the doc, the tests and the code together.

## Shape

```
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

Everything below is derived from one sentence, so it is worth stating on its own:

> **The handler sees only what it can act on; the operator sees everything.**

A result that reaches a handler is always terminal and always decidable. Retryable HTTP statuses
never arrive (rule 5), retryable command outcomes never arrive (rule 6), and a wedged invocation is
invisible to the script while being prominent in operational status. The author writes the decision;
the runtime writes the retry.

---

## 1. Arms name distinct event types

An effect is a set of `on` arms. One event selects **exactly one** arm, and two arms naming the same
event type is a compile error pointing at the first.

**An arm may list several event types**, which is a change to the shape and not to the invariant:

```
on @shop.reconnected,
   @warranty.plan.created,
   @warranty.plan.updated as e { @key shop_id } { ... }
```

Every listed path still selects that one arm, and a path appearing in two arms of one effect is
still an error naming the first. What is given up is only consequence #2 below, "an arm names one
concrete type", which was listed as a consequence rather than as the reason.

The evidence is one real effect that dispatches **thirteen** event types onto one body. With
one arm per type, that effect is thirteen copies of six `fold` declarations and a forty-line body,
and the thirteenth copy is where the drift starts. Five more effects in the same application dispatch
two to four types each. Writing the same fold thirteen times is not a rule being enforced, it is a
rule being avoided by copying.

The trigger binding then names **only the fields the listed types share**, checked at compile time. A
field counts as shared only when its type *and* its `@subject` match on every listed path, so a
`reveal` through a multi-path binding stays sound: the binding cannot name a field that is
subject-encrypted on one path and plain on another.

This is a deliberate divergence from hekla, which collects every arm whose clause matches and runs
them all in one invocation. Three things go wrong with that:

- **Declaration order becomes load-bearing for replay.** Arms run in the order they are written, so
  moving one changes which side effects are journaled before which.
- **The trigger binding becomes polymorphic.** An arm matched by two event types can only name fields
  common to both. This is the one consequence a multi-path arm does accept, and the difference is
  that the author asks for it by listing the types, so the restriction is visible where it is chosen
  rather than falling out of which clauses happened to match.
- **Every cross-arm static rule has to reason about the matched set** rather than one arm. Rule 9 is
  the concrete case: with one arm per event it is an intra-arm reachability analysis, and with
  overlapping arms it becomes a question about every order in which the matched set might run.

The counter-example, for honesty about where this stops helping: an effect posting eight different
alert shapes cannot use one multi-path arm, because its arms differ in the body and not only in the
trigger. Those eight arms still repeat the same three folds between them, and that is the case that
argues for effect-level `fold`, which this pass deliberately did not add. Per-arm state solved a
real ordering problem, and multi-path arms plus records may shrink the duplication that motivates it,
so the case is worth re-measuring before it is answered.

**Projectors legitimately want the opposite rule, and keep it.** Fanning one event out to several
read models is a real pattern, and a projector has no journal, so nothing about ordering is
dangerous there: a rebuild replays every handler in the same order and reaches the same rows. This is
a difference in the code and not only in the prose, since heklang's `fold_into` already runs every
matching handler. See "The three kinds are deliberately not unified".

## 2. State lives inside the arm

`as e` binds the trigger, and it is in scope for the arm's `fold` filters and its body. There is no
effect-level trigger binding and no effect-level state.

This is more expressive than hekla's single `query(event)`, which has to work for every subscribed
type and so in practice restricts filters to fields common to all triggers. Per-arm state has no such
constraint: each arm knows exactly one event type, so it can filter on any field of it.

An arm is `on [latest|live] @path [as name] { [@key] field, ... } { body }`, nearly the shape a
projector handler has (`docs/projectors.md`, rule 1). The first block destructures payload fields and
the second is the body. The two kinds share one construct rather than each having a slightly
different one, and rule 15 is where they part: a handler may omit the destructure and an arm may not,
because that is where the arm's partition key is written.

An arm stages like a command: a run of `fold` declarations is one read, and a statement below one
closes it, so a later run can filter on what an earlier one folded (`docs/commands.md`). Rule 3
folds every run to the trigger's own position, so a second read sees the prefix the first did and
there is no head to pin and no condition to build.

There is no `guard`, in either of its shapes, because an effect has no append condition to build and
no `Outcome` to refuse with: `docs/commands.md` has what a condition is and why a `fold` declares
it, and `docs/guards.md` has the named form. And a `let` in an arm is an ordinary statement that may
call out, because rule 2 gives an arm's filters the trigger binding rather than a hoisted value.

## 3. The fold stops at the trigger's own position, inclusive

`fold` is folded over the log up to **and including** the triggering event, never to the head of the
log. It is therefore a pure function of the log prefix and that position, so every attempt and every
replay reproduces it. Three things follow, and together they are why an effect has no read of a
projector:

- **It cannot race.** There is no other module to be behind.
- **It is not journaled.** Every attempt re-folds and gets the same answer, so there is nothing to
  record. Contrast `now()`, which is journaled precisely because it cannot reproduce itself.

  That only holds if a fold cannot do anything it would have to record, so a filter, a fold seed
  and a fold arm may not call out, invoke, decrypt or read a clock. Each is a compile error naming
  the fold rather than the builtin, because the fold is the reason. This is the same boundary hekla
  draws by evaluating `query` and `fold` without a request context.
- **It counts the trigger.** An effect folding its own trigger type sees itself, so a customer's
  first order leaves a count of one, not zero.

Folding to head instead would make state depend on how far the log had run when the handler happened
to execute, which is the same value reading differently on a retry.

## 4. `fail "<reason>"` is the author's terminal outcome

`fail` records the position as failed and advances the cursor. It is the **only** author-invoked
failure. A runtime error wedges instead, and there is no second author verb, because two would raise
"which one wedges?" as something to memorise.

Author failures and runtime terminal failures are **counted separately**, and this is what makes
`fail` safe rather than a way to lose work:

| Outcome | Meaning | Advances |
| --- | --- | --- |
| done | the arm ran to the end | yes |
| failed | the author judged this event unprocessable | yes |
| skipped | the runtime could not proceed, terminally (rule 12) | yes |
| wedged | the runtime could not proceed, and retrying might help | **no** |

The safety of `fail` rests entirely on `failed` being a first-class operational signal that never
collapses into the wedge count. An effect quietly failing a thousand events looks exactly like an
effect quietly succeeding on them unless the counter is separate and visible. hekla lumps the first
three together to a degree (it has no author verb at all, so an author who wants to give up must
raise, which wedges), and that is a wart not to inherit.

## 5. The principle, and what it rules out

Stated above. Its consequences, in order of how often they bite:

**Retryable HTTP statuses never reach the handler.** 408, 425, 429 and any 5xx each name a condition
that clears on its own, with the same request, so the runtime absorbs them with backoff. A
`status >= 400` that does reach a handler is therefore a real decide-what-to-do failure rather than
something every effect re-implements.

That split is not a convenience, it is the only place the decision can live. Every response reaching
a handler is journaled, so a handler that failed on a 429 would replay the recorded 429 on every
attempt: the request would never be re-sent and the invocation would wedge until an operator dropped
the work. Re-sending is something only the runtime can do, because only the runtime decides what
enters the journal.

**Retryable command outcomes never reach the handler**, for the same reason. See rule 6.

**A wedge is invisible to the script.** The script cannot observe that it is being retried, cannot
count attempts, and cannot behave differently on the third one.

**Rejected: author-invoked retry.** `retry(after = n)` as a fourth outcome would let an author say
"not now". It loses because retry is precisely the thing the author cannot correctly ask for: the
call is already journaled by the time they could ask, so the retry would replay the recorded failure
rather than re-send. The runtime already honours `Retry-After`, which is the same request expressed
by the party that can act on it.

## 6. `invoke` returns an outcome

Three cases:

| Case | Meaning |
| --- | --- |
| `Ok` | the command committed (possibly emitting nothing) |
| `Invalid(msg)` | the input was malformed |
| `Reject(code, msg)` | the command refused on state grounds |

This is a cut from hekla's six-variant `CommandOutcome`, and each cut is an instance of rule 5:

- **`Conflict`** means the append hit a concurrent write inside the boundary. It is retryable, the
  runtime rebuilds and retries, and an author who saw it could only retry worse.
- **`Unavailable`** means the store could not service the append transiently. Also retryable, also
  the runtime's.
- **`AlreadyCommitted`** collapses into `Ok`. It means this exact call already landed under its
  idempotency tag, which from the author's side is indistinguishable from having just landed it, and
  should be: that is what exactly-once means.

The three surviving cases are the ones an author can act on differently. `Conflict` and `Unavailable`
have **no variant in the type at all**, so "retryable outcomes never reach the handler" is
unrepresentable rather than filtered.

Read the outcome with `.ok()`, `.code()` and `.message()`. `.code()` and `.message()` are optionals
for the same reason rule 8's accessors are: every read is a branch anyway. `.refused(<Name>)` asks
the common question directly against a declared refusal (`docs/refusals.md`).

## 7. `invoke` input is a typed struct

`invoke RecordNotified { order_id: ..., notification_id: ... }` is checked at compile time against
the target command's declared parameters. An unknown field, a missing one, a duplicate, or a value of
the wrong type is a parse error with the field's own span, and each value is parsed with the
parameter's declared type as its hint, so literal inference and enum resolution work through `invoke`
exactly as they do through `emit`.

The JSON object literal (rule 8) is for HTTP bodies only and **must not leak into `invoke`**. An
object literal in invoke-argument position is an error saying so. A command's input has a schema; an
HTTP body does not, and the two should not be spelled the same.

**The runtime check stays as well**, and the reason generalises past `invoke`:

> A compile-time check covers one program version. A journaled value can be read back by a build
> other than the one that wrote it, so every boundary that crosses a journal or a deploy keeps its
> runtime check even where a static one exists.

Concretely: because `invoke` is journaled, a completed call is immune to signature drift, since the
recorded outcome is replayed rather than re-decided. But an invocation straddling a deploy hits its
first not-yet-journaled `invoke` against a command that may have changed, and no compile-time check
over one program version can cover a value produced by a different one. `invoke` is the instance, not
the rule.

## 8. `Json`

`Json` is opaque: an HTTP response body, with fallible one-step accessors.

| Accessor | Returns |
| --- | --- |
| `body.string("id")` | `String?` |
| `body.int("count")` | `Int?` |
| `body.bool("accepted")` | `Bool?` |
| `body.json("data")` | `Json?` |
| `body.array("errors")` | `List(Json)?` |

The one-step form is preferred over `body.get("id").as_string()` because every read of an untyped
body is a branch anyway, and the two-step form makes the author write two of them. There is no
dynamic field access and no indexing syntax.

`json` and `array` are the two beyond the original three, and a GraphQL response is why: the useful
value is at `data.<field>.userErrors`, three steps down and then an array. Each step stays fallible
for the reason the first three are, and `Json.empty` exists so a chain of them reads as one line
(`response.body.json("data").unwrap_or(Json.empty).array("errors").unwrap_or([])`).

**`Json` is a declarable type**, so a command parameter, a `fn` parameter and a `fn` return may be
one. It was in the IR from the start and unreachable from the grammar, which meant a command could
not take a webhook payload at all, and a real port's entry point for an entire integration could not
be spelled. An **object literal** is therefore legal anywhere a `Json` is expected, not only inside a
request body; rule 7 is unaffected, because `invoke` checks its fields against declared parameter
types and an object only reaches a parameter that is a `Json`.

**Unchecked is about a value's shape, not about whether there is one.** A `Json` position accepts
anything that is *there*, `null` included. A key that is not there at all is a different question, and
`Json` answers it exactly as every other type does: a missing key is a mismatch unless the field is
optional or declares `@absent` (`docs/declarations.md`). That makes a `Json` field inside a record
behave like its siblings, where it used to be the one type whose absence read as `null` and therefore
the one type that could quietly lose the difference between "the producer sent null" and "there was no
such field when this was written".

**`Json.encode(value) -> String`** is the table below pointed at a string instead of a socket. A
Shopify metafield of type `json` takes its value as a *string*, and the original carried a whole
hand-written encoder for that one call. Because it is the same table, a value encoded here and the
same value in a request body cannot disagree, which a second encoder could not promise.

A **JSON object literal** `{ "key": expr, ... }` builds a request body. Values convert by a total
table, which is part of the contract because it decides what a remote service actually receives:

| heklang | JSON |
| --- | --- |
| `Bool` | boolean |
| `Int` | number |
| `Decimal(n)` | string at scale `n`, e.g. `"0.0825"` |
| `Money(n)` | string at scale `n`, e.g. `"25.99"` |
| `String` | string |
| `Uuid` | string |
| `Timestamp` | number, epoch microseconds |
| an enum | string, the variant name |
| a record | object, one key per field |
| `List(T)` | array of whatever `T` converts to |
| `Map(K, V)` | object, keys as their text form |
| `none` | `null` |
| `some(x)` | whatever `x` converts to |

`Money` and `Decimal` become strings rather than numbers so no precision is lost to a float on the
far side, which is the same reason they are scaled integers here. Neither carries a currency; a
program that needs one sends the field it declared for it (`docs/money.md`).

A map's keys become strings because that is what a JSON object can hold, and the order is already the
map's, so nothing is decided at the boundary. `Map.empty` is the one thing that cannot be written
straight into a body, and the reason is a target rather than a table: an object literal is written
`{ ... }`, so an empty map has no declaration to have come from (`docs/containers.md`).

### A number the author typed is a JSON number

The table above is about **a heklang value crossing the boundary**. It is not about what may appear
inside a `Json`, which this rule separately calls unchecked passthrough in both directions. A numeric
literal written straight into a body is the second thing, and it stays a JSON number:

```
{ "count": 7, "amount": 10.5, "rates": [0.1, 0.25], "owed": total }
```

```json
{"amount":10.5,"count":7,"owed":"10.50","rates":[0.1,0.25]}
```

`total` is a `Money(2)`, so it is quoted by the table. `10.5` is not a heklang value being exported,
it is a number the author typed into a foreign document, so it goes out as one. This holds inside
arrays and nested objects, and for a negative literal.

**This is a distinction, and it will surprise someone.** Replacing the literal `10.5` with a variable
holding a `Money(2)` changes the wire form from `10.5` to `"10.50"`. That is intended, because the
two are different things, but it is worth knowing before a refactor.

Only a bare literal is affected. `{ "n": 1 + 2 }` is arithmetic that produces an `Int`, and an `Int`
is a JSON number by the table anyway.

**Before this, there was no way to send `{"amount": 10.5}` at all.** `Json` could hold an integer and
could not hold anything else, so a fractional literal fell through to `Decimal(n)` and the table
quoted it, while `7` went out bare. The boundary was not "numerics are quoted", it was "the ones the
representation could not hold were", which is not a rule anyone chose.

A `Json` number is carried as **the exact text**, never an `f64`. `0.30000000000000004` survives a
round trip byte for byte, and no float arithmetic exists anywhere in the language.

**So two numbers are equal when they are spelled the same, not when they are worth the same.** `3`
and `3.0` are both three and they do not compare equal, because collapsing them is the same operation
that would collapse `10.50` to `10.5`. Fidelity is the point, and it does not get to apply only to
the digits that matter. Where an author meets this is a test: `expect http.post(url, { "n": 3 })` against
a body that passed `3.0` through from a response fails with `expected 3, got 3.0`, which says what to
write.

**Object keys are sorted.** That is rule 14's defined iteration order (see below), and it is why the
same object built twice serialises byte-identically.

### The table read backwards

`Value::from_json(&json, &ty, defs)` is the same table inbound, and a host needs it: every field of
every stored record, every read-model column and every command argument arrives as JSON and has to
become a value of a declared type. One table met twice, rather than two kept in step.

**It takes the type rather than inferring one.** `"1.5"` is 1.50 at `Money(2)` and 1.500 at
`Money(3)`, and only the declaration says which was written. A `Timestamp` and an `Int` are both
numbers, and an enum variant is checked against its declaration rather than taken on trust.

**A number read out of a response body comes back as text**, through `body.number("amount")`, and
`Money.parse` or `Decimal.parse` finishes the job against a declared target:

```
invoke Record {
  price: Money.parse(response.body.number("price").unwrap_or("0")).unwrap_or(0.00),
  rate: Decimal.parse(response.body.number("rate").unwrap_or("0")).unwrap_or(0.0000),
}
```

Text rather than a typed value for the reason above: the scale belongs to where it lands, and the
digits on the wire do not know it. `19.99` reaches a `Money(2)` as exactly 19.99, with nothing
rounded and no float in between.

`body.int("n")` still answers for a whole number and answers `none` for `10.5`, which is the same
`none` a missing key gives.

**A `null` fills an optional and nothing else.** An absent object key reads as `null`, so a missing
optional is absent and a missing required field is an error rather than a zero quietly standing in.

**A seal is not in the JSON.** `Sealed(T, subject)` reads as its content, because a seal carries the
subject's id and that lives in a sibling field. A host that stores sealed content rebuilds the seal,
and `Value::Sealed` is public for exactly that; `docs/host.md` section 7 has the read-model side.

**A mismatch is data, not a broken host.** A record written before a field changed type reads back
wrong, and that gets its own answer: `Mismatch` names the path, the declared type and the shape that
was stored, and it arrives as `ErrorKind::Mismatch` rather than `ErrorKind::Host`. Saying `Host`
would blame a store that is working exactly as asked.

**An array crosses in both directions, and only one of them is typed.** `Value::from_json` fills a
declared `List(T)` or `Map(K, V)` from an array or an object, because the declaration says what the
elements are. A response body has no declaration, so `body.array("errors")` answers `List(Json)?` and
each element is a `Json` read with the same one-step accessors as the body itself, one branch per
step. `docs/containers.md` has the list and the `for` that walks it.

## 9. Erase last, statically enforced

A `reveal` **reachable after** an `erase` within one arm is a compile error.

The reason is the whole contract in one line, and the error message says it: `erase` is journaled and
`reveal` is not. A replay skips the erase, because it is recorded as done, and then re-runs the
reveal against a key that is gone. Calls journaled before the erase stay done, so nothing re-fires,
but everything after the reveal does not run on that replay.

This is a **reachability analysis over the arm's control flow**, not a lexical ordering check. The
difference is visible in one line:

```
if x { erase(e.customer_id); fail "gone" }
reveal(e.email)                              // legal: the erase path never reaches here
```

The erase is lexically first and is still fine, because the branch containing it does not fall
through. A lexical check rejects that program; a reachability analysis accepts it and stays correct
when the code is rearranged.

The analysis is **exact**: a program is rejected if and only if a reveal is reachable from an erase.
It stays exact because `erase` is a statement rather than an expression, so every reveal within one
statement is checked against the same incoming state and the order of reveals inside a statement
cannot matter.

**Rejected: `erase` returning a bool.** hekla's returns `true` if a key was actually deleted. It
loses on rule 5: there is nothing an author can do differently on either answer, because the subject
is gone by the time they read it. Dropping the result costs nothing and keeps `erase` out of
expression position, which is what keeps this analysis exact rather than conservative.

**A `for` body iterates to a fixed point**, because it may run again: an `erase` anywhere in one is
reachable from every reveal in it, including a reveal lexically *above* it. Two passes reach the
fixed point, since the lattice has two elements. That is exactly the case a lexical check gets
silently wrong, which is why the analysis is written as reachability:

```
for id in ids {
  log(reveal(e.email))       // rejected: the erase below reaches it on the next turn
  erase(e.customer_id)
}
```

**It stays one arm wide, and that is a decision now rather than a happy accident.** An effect may
declare its own helpers, and one of those may call out. It may not `reveal` or `erase`: those stay in
the arm, so this analysis never has to follow a call, never needs a summary per function, and never
has to explain a path through code the author was not looking at. See `docs/functions.md`.

### Naming the subject

`erase(value)` recovers the subject from the value: it must be a field of the triggering event, and
the `@subject(...)` declaration on that event says which key namespace it names. When the id does
not come from the trigger there is no name to recover, so the second form supplies it:

```
on @shop.redact.received as e { @key shop_id } {
  fold customers: List(Int) = []
    on @order.paid(shop_id) { customer_id } => customers.push(customer_id)

  for id in customers {
    erase(customer_id, id)          // the subject name, then the value
  }
  erase(shop_id)                    // the inferring form, unchanged
}
```

This is the shape a mandatory GDPR shop-redact has: one webhook, and every customer the shop holds
data for. The ids come from a fold of plaintext tags, which is as deterministic and replayable as a
trigger field; what a fold cannot supply is the **name**.

`docs/testing.md` already spells the matching expectation `expect erase(<subject>, "<id>")`, so the
statement and the expectation now read the same.

**What it gives up, said plainly.** heklang cannot check that the value really is a `customer_id`,
only that the author said so: `erase(customer_id, some_other_int)` compiles. Erasing a key that does
not exist is a no-op, but erasing the **wrong namespace** destroys the wrong subject's key. That is
why the inferring form stays the default and this one is for the case it cannot reach.

Three things are still checked:

- the name is a declared subject, the same check the inferring form makes;
- the value's type is the type of the field the keys are filed under, so
  `erase(customer_id, e.email)` is rejected, and so is an optional id;
- **the value contains no `reveal`**, which is rule 9's second rule below. The inferring form cannot
  reach it, because a `reveal` is not a trigger field load, so it becomes reachable exactly here and
  arrives with this form.

**Rejected: inferring the name through the fold.** It would mean proving every element of
`customers` is a `customer_id`: element-level provenance through `List.push`, inside a fold arm,
through an `if`/`else`. That is more analysis than anything else in the language, rule 12's "a
transformed arm drops the binding" already points against it, and it would have to be *exact*,
because a wrong namespace destroys the wrong key. The choice is between exact inference and an
explicit name, and there is no third option that is both safe and cheap.

**The second rule holds where it can be reached.** hekla also says do not erase a subject whose id
you learned by revealing, because a repeat request for an already-erased subject then cannot be read
at all. The inferring form cannot express it: a `reveal` is not a trigger field load. The named form
can, so it checks the value for one and rejects it by span. What is still not caught is an id that
round-trips through an HTTP response, which needs data flow this pass does not build. Take subject
ids from a plaintext field.

## 10. No marker on unjournaled builtins

`reveal` and `log` re-execute on replay. Everything else is journaled. **This is not marked in the
syntax.**

A per-call sigil on a two-member set whose members are distinctively named taxes every correct use to
teach something the compile error already teaches at the point of violation, and it would appear on
half the lines of every privacy-relevant handler. It is the same judgement hekla's `fold_error` and
`handle_error` make: teach the contract where it is violated, do not tax correct use.

**`log` is deliberately not journaled**, and may therefore appear twice across a crash. That is a
decision, not an omission: journaling log lines would double the operational cost of the cheapest
diagnostic in the language, and a duplicated log line is the least harmful thing that can be
duplicated.

**Write down when to revisit.** This rule expires if the unjournaled set reaches three or four
members, or sooner if one arrives whose name does not announce that it is special. The argument above
is about a small set of well-named things, and it stops holding when either half stops being true.

**Reading a `secret` is the third, and it is the first that is not a call.** Rule 16's credentials
re-execute on a replay for the same reason `reveal` does: the value is not a fact about the world
that happened, and a replay after a rotation should send the new credential rather than the old one.
It does not push this rule over, because the expiry clause is about *builtins* an author might
mistake for journaled ones. A bare name is not a call, there is no per-call sigil it could carry,
and the question a marker would answer -- "is this recorded?" -- has the same answer for every read
of every secret. What it does do is put a third entry in rule 14's list of what verify still covers.

## 11. Builtins

| Builtin | Returns | Journaled |
| --- | --- | --- |
| `http.get(url)` | `Response` | yes |
| `http.post(url, body)` | `Response` | yes |
| `http.put` / `http.patch` / `http.delete` | `Response` | yes |
| `invoke Name { ... }` | `Outcome` | yes |
| `now()` | `Timestamp` | yes |
| `Uuid.derive(seed, name)` | `Uuid` | pure |
| `log(message)` | nothing | **no** |
| `reveal(field)` | `String` | **no**, re-decrypts every attempt |
| `erase(value)` / `erase(subject, value)` | nothing | yes |

**There is no `Uuid.new`, no `Uuid.random` and no `random`, anywhere in the language.** Not in
effects, not in commands, not in projectors. "Never mint a random id" is therefore unrepresentable
rather than documented: a command retry and an effect replay both have to derive the same id they
derived the first time, and the only way to guarantee that is to have no other option.
`Uuid.derive(seed, name)` derives one from an identity that already exists, and `e.id` is the seed
most handlers want. This holds below the grammar too: the `uuid` dependency is built without its `v4`
feature.

Putting `derive` on the type is what makes that absence **visible**. An author who wants a fresh id
types `Uuid.` and sees one member; the thing they were reaching for is missing from the place they
looked, and asking for it by name gets the reason rather than "not in scope". A missing global could
not manage that: nobody scans a namespace they have no reason to think contains what they want, so
`uuid4` being absent taught nothing to the author who never typed it. The rejected spellings are
still recognised (`Uuid.new`, `Uuid.random`, `Uuid.generate`, `Uuid.v4`, and the globals `uuid4`,
`random` and `uuid5`), each pointing at `derive`.

`derive` also refuses to spell RFC 4122's version number. `uuid5` names the algorithm; `derive` names
the purpose, in a language that already hides `i64` behind `Int` and `Money` and does not otherwise
ask an author to know a wire format. The seed argument is a `seed`, not a `namespace`, for the same
reason.

**The clock rule**, ported from hekla and better than "effects only": a clock exists where its result
is pinned or journaled, and is absent where replay demands determinism.

| Where | `now()` |
| --- | --- |
| a command body | available, pinned once per request |
| an effect arm | available, journaled |
| a `fold` (either kind) | absent |
| a projector | absent |

`now()` is pinned **once**, not per call: it lowers to a single slot filled before the body runs, so
two calls in one body are two reads of the same value. In a command that value is the timestamp the
request would append at, computed on entry, so it is well defined even for a command that appends
nothing: one returning `invalid` or `reject` still read a real instant, it was simply never stamped
on anything. A command emitting two events reads the first one's append time.

## An effect-local `fn`

An `effect` may declare its own helpers, and unlike a module `fn` one of these may call out:

```
effect SyncShop {
  fn sync(shop_id: Int, domain: String, secret: String) {
    let response = http.post("https://{domain}/admin/api/sync", { "shop": shop_id },
      headers = { "X-Access-Token": secret })
    if response.status >= 400 {
      fail "sync rejected with status {response.status}"
    }
    log("synced shop {shop_id} at {domain}")
  }

  on @shop.sync.requested as e { @key shop_id } { ... sync(shop_id, domain, reveal(token)) }
  on @shop.reconnected as e { @key shop_id } { ... sync(shop_id, domain, reveal(token)) }
}
```

`docs/functions.md` has the rule and the evidence. What matters here is the three things it does not
disturb:

- **Rule 9 is unchanged.** A helper may not `reveal` or `erase`, so the erase-last analysis still runs
  over one arm's statement tree. That is the restriction the whole design is built around, and it is
  free: no helper in the port that motivated this contains either.
- **Rule 3 is unchanged.** A `fold` may not call one, because a fold has to reproduce without a
  journal and this is the first helper that could call out.
- **Rule 11 is unchanged.** `now()` stays pinned once per invocation, into a slot the arm fills, so a
  helper may not read the clock: read it in the arm and pass it in.

Rules 4, 6, 8 and 10 carry into a helper as they are. A `fail` there is the arm's terminal outcome
and produces the same trace entry, an `invoke` there appends under the command's own guard, and both
the journal's ordering and its ordinals are per invocation rather than per call frame, so two
identical requests, one in the arm and one in a helper it calls, are two entries and replay to their
own answers.

## The global namespace is closed to constructors

`Uuid.derive` is the language's first type-qualified call, and the rule it establishes is the reason
it was worth introducing a syntactic form for one member:

> The global namespace holds **actions with no natural receiver**. Anything constructed from nothing
> is named by its type.

`log`, `fail`, `now`, `reveal`, `erase`, `invoke` and `http.*` are all things a handler *does*, and
none of them has a receiver to hang from. A constructor is not one of those: it has an obvious owner,
and `Json.parse` and `Timestamp.from_micros` will want the same shape as soon as they exist.
Establishing the form now with a single member is cheaper than adding a second global and retrofitting
later, when the retrofit would be a breaking change to two call sites instead of one.

The receiver behaves like the soft builtin names (rule 10): `Uuid` is claimed only in the position
where it is unambiguous, which is immediately before a `.`, so a local binding named `Uuid` still
shadows it and an enum variant called `Uuid` still resolves. Nothing about this makes `Uuid` a value:
there is no bare `Uuid` expression, only the qualified call.

## 12. `reveal`

`reveal` takes a subject-bound value and hands back the plaintext. It decides nothing about it;
everything below is about the two ways it can hand back something else.

### The seal is in the type

`@subject(customer_id)` on an event field is the authored form; `Sealed(String, customer_id)` is what
propagates from it. `Opt` stays outermost, so `String? @subject(x)` is `Opt(Sealed(String, x))` and
everything that already looks through an optional keeps working with one extra unwrap.

`Sealed` is not spellable. An author writes the annotation and never the type, which is what keeps
one place able to create a seal.

**A seal survives a `let`, and that is the point.** Subject-ness used to be a property of how an
expression was *spelled*, recovered by looking at the slot an expression loaded from. One `let`
laundered it:

```
let copy = e.email
log(reveal(copy))          // used to be: `reveal` takes a subject-bound value...
```

A type composes where a spelling does not, so this now works, and so does folding it, passing it and
storing it.

### What it takes: a field of the trigger, or a fold of one

```
on @shop.sync.requested as e { @key shop_id } {
  fold token: String? = none
    on @shop.connected(shop_id) { access_token } => access_token
    on @shop.reconnected(shop_id) { access_token } => access_token

  let secret = reveal(token)
```

A credential is almost never on the event being handled. It was appended when the shop connected,
long before, so **the seal propagates through a `fold`**: the variable's declared type is what
the author wrote, and folding sealed content onto it seals it. That is the same propagation
`docs/projectors.md` rule 9 performs through a projector write.

An arm seals the variable when its result **is** sealed content. A transformed one
(`=> access_token.trim()`) is not, and it no longer needs a rule of its own: reading content through
a method is rejected where it is written, which is a better place for the error than the `reveal`
further down that used to suffer from it.

### The subject rides on the value

A sealed value carries the field, the subject and the **id** its key is filed under, beside the
content as a host stored it. That is what `reveal` reads, and it is why nothing has to be recovered
from the parse tree:

```rust
Value::Sealed { field, subject, id, content }
```

It is made in one place, where an event field enters a frame, because that is the only place with
the whole event: the id lives in a sibling field, so a value alone can never say what key it is
filed under. **Nothing takes it off again.** A seal crosses into the log and the store as it is, and
`Keys::decrypt` at a `reveal` is the only thing that opens one.

**`content` is text, and heklang never reads it.** That is what a key store encrypts, and it is why
the content *type* is not here but on the `reveal` node: a seal cannot say whether its text was a
number, only the declaration can, and a `Type` on every sealed value cost `Value` a third of its
size for something identical at every run.

**`field` is where it was sealed, not where it now sits.** A host binds its ciphertext to that name,
so content moved into another position still decrypts under the name it was sealed with. Moving it
is the one thing rule 12 allows without a key, and this is what makes that safe rather than lucky.

**This replaced a companion fold.** Each subject-bound variable used to get a second, hidden state
variable that folded the subject id alongside the value, because the id could not come from the
slice's filter: a fold of `customer_name @subject(customer_id)` filtered on `warranty_id` is an
ordinary shape. Carrying the id on the value deletes that machinery, along with `FoldSubject`,
`StateVar.subject` and `Parser::subject_source`.

**One thing changed shape in the error.** A terminal reveal now names the **field** whose content is
unreadable rather than the local the source happened to reveal, because the field is what the value
carries. That is what this document always showed, and it is the more useful of the two now that a
value can travel.

### What may be done to sealed content

Three things, and the port is where the list comes from: across 32 `reveal` sites it reveals at the
point of use and passes plaintext onward, and not one moves sealed content into a container, a record
or a `fn`.

| | Why it is safe |
| --- | --- |
| **Move it** into a position sealed under the same subject: a `let`, a `fold`, an entity column, another event field | the content is never read |
| **Ask if it is there**: `.is_some()` / `.is_none()` | presence is not content |
| **`reveal` it** | the boundary itself |

Everything else is a compile error, and each of these used to pass:

```
http.post(url, { "email": e.email })     // cannot be sent in a request body
log("email is {e.email}")                // cannot be interpolated into a string
log(e.email)                             // a String is not sealed content
invoke RecordCopy { note: e.email }      // takes it out from behind the boundary
if e.email == "x" { }                    // cannot be compared
e.email.trim()                           // `trim` reads content sealed under `customer_id`
e.email.unwrap_or("")                    // a plaintext default and sealed content in one slot
```

`unwrap_or` gets its own reason because it is the mistake a real port makes: a sentinel standing in
for content that has a key. It is the same argument `mixed_fold` makes about a fold, one level down.

**Writing plain content into a seal is free**, because that is the encrypting direction. A command
holding an ordinary `String` may `emit` it into a `@subject(...)` field with no ceremony; only reading
back out needs `reveal`.

**Two positions propagate instead of reading**: a `fold` and an entity column. Both take sealed
content and become sealed themselves, which is why a projector can store a credential it may never
`reveal`. That is `docs/projectors.md` rule 9, and it is what the port's read models depend on.

**Rejected: tainting slots instead of typing values.** The check would live on the slot an expression
loaded from, which is where subject-ness used to live. One `let`, one record field or one list
launders it, and an unsound security check is worse than a documented gap, because it reads as a
guarantee.

**Rejected: reveal-before-use, with no exceptions.** It is the shorter rule and it makes a projector
impossible: a projector may never `reveal`, and storing personal data into a read model is most of
what the port's projectors do.

#### One variable, one subject

Two arms folding subject-bound values under **different** subject fields into one variable is an
error naming both. Two arms under the same subject is the common case (`@shop.connected` and
`@shop.reconnected` are both keyed by `shop_id`) and is exactly what this is for.

> `token` folds under two subjects, `shop_id` from @shop.connected and `customer_id` from
> @warranty.sold; one variable holds one subject, because `reveal` names the key by it

**Rejected: allow several and take the last writer's at reveal time.** It makes the key's name a
runtime property, so the terminal message would name a subject a reader cannot predict from the
source, which is the one thing that message exists to make predictable.

#### A plain seed is fine, a plain arm is not

The asymmetry is not obvious, so both halves are stated:

- **The seed is never subject-bound, and that is fine.** It is evaluated before the fold, with no
  event behind it, so it is a value the author wrote rather than one that came out of the log.
  `fold token: String? = none` seeds with nothing and folds credentials into it.
- **An arm folding a non-subject-bound value into a variable another arm makes subject-bound is an
  error**, in either declaration order, naming the arm to change. Otherwise plaintext and a value
  that needs a key share one slot, with nothing static to say which one is in it.

**Rejected: allow the mix and treat the variable as plain when a plain arm wrote last.** The same
defect as above, one level worse: whether `reveal` is required at all would become a runtime
property.

The rules are about the variable, so they hold in a command as well as an effect. Only an effect can
`reveal`, so a command's recorded subject is inert; one rule is worth an unread field.


### `@subject(...)` names a field that always has a value

`@subject(x)` must name a field of the same event, `x` may not itself be subject-bound, and **`x` may
not be optional**. A subject id is the name a key is filed under, so a missing id is not "no key", it
is no question at all: there is nothing to look up and nothing for `erase` to remove. Checked where
the annotation is written, which is where the mistake is, and it costs one check to keep absence from
having to mean two things.

### An optional in, an optional out

`reveal(x: T?)` is `T?`, and `reveal(x: T)` is `T`. Three states are distinguishable and two of them
must not collapse:

| Held value | Key | Result |
| --- | --- | --- |
| absent | irrelevant | `none` |
| present | exists | the plaintext |
| present | shredded | terminal, below |

The first row **does not consult the key store at all**, and that is the rule rather than an
optimisation: a value that is absent was never encrypted, so no key can be missing for it.

`none` there is an ordinary condition an author branches on, not a failure. **"Never set" and "key
destroyed" are different facts**: one is recoverable by supplying the value and one is not, and an
author does different things about them. Collapsing them is wrong in a specific direction either way.
Returning `none` for both turns a shredded key into a quiet success and gives up the whole of this
rule; failing terminally for both wedges every subject that simply has no value yet.

**Rejected: `reveal` unwraps with a zero.** That is a sentinel, which the zero-value table in
`docs/projectors.md` exists to argue against, and it is the workaround a real port had to write
before this rule existed: `fold token: String = ""` and then `token.is_empty()`, where an
absent credential and an empty one are the same string.

### Failing is terminal

`reveal` of an **erased** subject fails terminally. It does not become a `none`, which is the one
case where returning an optional would be the wrong shape: it would force a branch at every call site
whose only sane arm is to give up, which is what terminal already does, one level up and once. No
retry can recover erased data, so wedging would be wrong too: the invocation is skipped, counted
apart from wedges (rule 4), and the cursor advances.

The message must say the erase may be **non-local**:

> reveal cannot decrypt `email`: subject `customer_id` = `7` has been erased. The erase need not be
> in this effect; another effect or a concurrent invocation can erase a subject between the original
> run and a replay, and nothing static catches that.

Rule 9 makes a local erase-then-reveal impossible, so by construction every terminal reveal an
operator sees is caused by something else. Without that second sentence they hunt for an `erase` in a
file that does not contain one.

## Headers

```
http.post(url, body, headers = { "Authorization": "Bearer {token}" })
```

A **named** argument, not a third positional one, because the third positional slot already has an
error attached to it that teaches rule 13, and that error should keep firing for someone who writes a
timeout there.

The case that matters beyond convenience is the `Idempotency-Key`. It is what stops a transactional
email being sent twice, so it has to survive the same replay the journal is protecting against, which
settles a question the journal key would otherwise raise:

**The journal key is the verb, the URL and the body, and deliberately not the headers.** A replay
that recomputes a different idempotency key, or a deploy that adds one, must still land on the entry
that already recorded the send. Keying on headers would turn "the same call with a new key" into a
new call, which is exactly the second send the key exists to prevent.

**Rule 16 is the case that argument did not cover**, and it gets the same answer by a different
route. A credential can be *in* the url or the body, where this rule does key, so a rotated webhook
would move every past entry onto a string that no longer exists. The url and the body therefore
enter the key in their redacted rendering: `{SECRET:DISCORD_WEBHOOK}` rather than the address. So a
rotation moves no key, and the key never holds a credential in the first place, which matters
because it is a readable description by design.

Headers are always present in the IR and empty when unwritten, so the interpreter reads one shape
rather than two.

## 13. Timeouts are configuration, not syntax

No language change. Recorded because the gap is real: hekla hardcodes a 10s connect and 30s global
timeout, which leaves an author with a legitimately slow endpoint no recourse at all. The fix is
per-effect or per-call configuration, in `hekla.toml` or an effect-level declaration, not a fourth
argument at the call site. A timeout named at the call site would also be a number the author has to
keep in sync with a service they do not operate.

## 14. Verify mode stays

Folding twice and comparing catches nondeterminism that types cannot. It stays.

But the point of typing things is to shrink what is left to it, so here is what has been removed and
what remains.

**Removed by the language:**

- *Iteration order.* Object keys are sorted and every map in the interpreter is ordered, so the same
  object built twice serialises identically. Previously this was a real source of divergence.
- *The clock.* `now()` is pinned once and journaled, so it cannot differ between a run and its
  replay, nor between two reads in one body.
- *Randomness.* There is none to be had (rule 11).
- *Reads of mutable state.* An effect has no read of a projector (rule 3), and a projector has no
  general read (`docs/projectors.md`, rule 4).

**What verify still covers**, being the causes outside the language:

- *A subject re-keyed between a run and its replay.* `reveal` is not journaled by design, so its
  result is whatever the key store says at the time. Nothing static catches this and nothing should.
- *A deployment secret rotated between a run and its replay* (rule 16). The same shape one level
  over, and the same answer: a credential is not journaled, so a replay sends whatever the
  deployment holds now. This is why the journal key is built from the redaction rather than the
  value -- the key has to stay put across a rotation even though what goes on the wire does not.
- *A journal read back by a different program version* (rule 7). Replay equivalence is exactly the
  check that notices a handler no longer making the calls it recorded.
- *Anything a future builtin adds.* The list above is short because the builtin set is closed. It
  should be re-derived whenever it stops being.

The goal is that verify keeps covering only causes not yet nameable. It should keep shrinking.

## 15. Delivery is declared, and a key names the lane

An arm says which events it answers. Until this rule it said nothing about **how many invocations
that should be**, and two things followed from the silence.

*Every effect had one lane.* Events were processed strictly in log order, so one unprocessable event
blocked every unrelated aggregate behind it. This is not hypothetical: in a production deployment one
oversized order event stalled a warranty-recording effect for every merchant on the platform for
eight hours, because that effect had no key and therefore one lane.

*Every effect replayed from position 0.* A new effect has no watermark, so adding one to a running
deployment fires its side effects across the whole history. For a notification effect that is an
email to every customer in the log. For a convergent one it is N redundant API calls where one would
do.

Authors worked around both by reading the log at head inside the effect ("skip unless this is the
newest event for this key"), which is exactly what rule 3 forbids and should keep forbidding: a fold
being a pure function of the log prefix and the trigger's position is what buys replay equivalence
and verify mode. So the fix goes in the declaration.

Everything below serves one sentence:

> **A catchup policy may change how much work happens. It must never change what the log says.**

```
arm := "on" [ "latest" | "live" ]
       path ("," path)*
       ["as" ident]
       "{" [ "@key" ] ident ("," [ "@key" ] ident)* [","] "}"
       "{" body "}"
```

### `@key`

Marks a destructured binding as part of this arm's partition key. At least one per arm, and marking
several forms a composite, ordered as written.

**Mandatory, with no opt-out.** An implicit "no key" default is a default of one global lane and
maximum blast radius, which is the failure this rule exists for. If a genuinely global-ordering
effect turns up later, an explicit opt-out is additive.

It is an annotation on a binding rather than a clause of its own, and the alternatives are worth
recording:

- **Rejected: a header clause,** `effect Foo by shop_id`. It makes the same token resolve as a
  field-name lookup in one place and as a binding in another, and it hides the key from the arm that
  uses it.
- **Rejected: an expression form,** `by <expr>`. A partition key is a domain identity, not a computed
  value. Key functions returning an arbitrary value exist in other runtimes and nothing real uses the
  freedom. If a case appears, `@key` on a `let` above the body is the additive extension.
- It reuses an annotation whose meaning already matches: `@key` on an entity field marks identity, and
  `@key` on a trigger binding marks the identity that defines the lane. The two positions are
  disjoint, so nothing has to tell them apart.
- Renames are free: `{ @key shop }` for a legacy event whose field is not called `shop_id`.
- It costs five characters. `by <expr>` costs eleven, which measurably pushes arm headers past
  `hek fmt`'s 90-column wrap: on a real eight-arm effect, `by shop_id` wraps four of the eight headers
  into six-line blocks and `@key` wraps one.

**A key may not be sealed.** Choosing a lane means reading the key, and rule 12 lets sealed content
only be moved, asked about or revealed. The subject id it is bound to is not sealed and is usually the
key that was wanted anyway. A key also has to be a type that identifies, which is the same set an
entity key takes: an `Int`, a `String`, a `Uuid`, a `Timestamp` or an enum.

**Composite order is significant.** `{ @key a, @key b }` and `{ @key b, @key a }` are different lanes,
so the digest does not sort them (`docs/digest.md` rule 4).

### The modifiers

| Form | Delivery | May `invoke` |
| --- | --- | --- |
| `on` | every matching event gets an invocation | yes |
| `on latest` | one invocation per key per batch, at the newest matching position in it | **no** |
| `on live` | only events appended after the effect's first activation | yes |

`on` is the default and is unchanged: existing behaviour, existing programs.

**`on latest` is not "skip history".** History is processed; the arm runs once per key instead of once
per event, and because rule 3 stops a fold at the trigger's own position inclusive, that one
invocation has already seen every event before it. What varies is batch size. Catching up, the batch
is the whole backlog, so thirty historical plan edits for one shop produce one invocation. Live, a
batch is normally one event, so `latest` only collapses when several events for one key are pending
while an earlier invocation is still running. That burst behaviour matters beyond migrations: a
merchant editing six plans in a minute gets one publish, permanently.

**`on live` declares that history is not news.** It is not a position constant in source. The runtime
resolves it to a concrete position once, at first activation, per data directory. Source states
intent, so dev, staging and production each resolve correctly, the value cannot rot, and `hek digest`
does not move when an operator makes an operational decision.

`latest` and `live` are **soft**, claimed only between `on` and the first path, so a field or
parameter called `live` stays writable. This is the same device the effect builtins use (rule 10).

### Why `latest` forbids `invoke` and `live` does not

The asymmetry is load-bearing, so it is enforced *and* written down.

`on live` declines whole invocations. No HTTP, no `invoke`, no record. The one-to-one relationship
between an invocation and what it wrote survives, and the log truthfully says nothing happened,
because nothing did.

`on latest` keeps one invocation out of N. That breaks one-to-one, and whether the surviving record
still tells the truth depends on whether it described *the invocation* ("I published the metafield")
or *the events* ("this webhook means the shop uninstalled"). Those spell identically and only the
author knows which.

The check is interprocedural within the effect, because an effect-local `fn` may `invoke`; a walk that
stopped at the arm's own statements would let the same program through by moving one line.

Two rejected alternatives, recorded so they are not revisited:

- **Rejected: "safe if the invoke's input uses only the key."** Unsound. `invoke DisconnectShop
  { shop_id }` has key-only input, but a shop that uninstalled, reinstalled and uninstalled again has
  two triggering webhooks, and collapsing them emits one `@shop.uninstalled` where reality had two.
  The difference is not in the data.
- **Rejected: "safe if no fold or projector reads the emitted event type."** Sound for known cases,
  but it makes the meaning of `latest` depend on what the rest of the program happens to read today,
  so an unrelated fold added elsewhere silently changes an effect's licence. It also lets a scheduling
  hint change the log's contents whenever nobody is looking, which is the one sentence this rule is
  for.

**The cost is real.** A convergent effect cannot both collapse and record a durable fact in the log.
There is no workaround inside one effect, since rule 1 forbids two arms naming one event type, and
none across two, since the record is conditional on the call having succeeded and a second effect
cannot observe that. The author picks one.

**Where it lands in practice, measured rather than assumed.** The port has six effects that look like
`on latest` candidates, and all six invoke a `Record...` command. Migrated, one takes the modifier
and five do not, for three different reasons, and the split is the useful part:

- **Three fold the event they emit** (`master_product_id != 0`, and two `enabled` facts). There the
  record is not an audit trail at all, it is the convergence state: the arm reads it back to decide
  whether to act. Dropping it costs the convergence. But they also lose nothing to the ban, because
  folding the record is exactly what makes a redundant run a no-op, so they are already self-limiting
  and had no reason to collapse.
- **One records real domain state** read by a guard, a projector and another effect
  (`@shop.subscription.synced`). Not a record of the invocation at all; it happens to be emitted by
  one.
- **One records the emitting command's own idempotence memory**
  (`@shop.webhook.registration.completed`, folded by the command that emits it, per topic). Nothing
  else reads it, but dropping it would make that command emit on every call.
- **One is the case this rule was written around.** `sync-shop-plans-metafield` emits
  `@shop.plans_metafield.synced` through a command that is a bare `emit`: no guard, no fold, and a
  comment saying the journal covers replay so the event is purely an audit fact. It appears in three
  places in the whole program, all of them its own declaration and emission. Dropping the `invoke` is
  unobservable, and it is the effect with the most to gain: **fourteen trigger paths collapsing to one
  publish per shop.**

So the correction to make here is narrower than it first looked. "Such records are usually redundant
with the runtime's invocation journal" was too glib, and three of six are the opposite. But the ban
costs those three nothing, and `on latest` does have a user. Corpus support across the port, once
migrated: `@key` on 23 of 23 arms, `on live` on 9, `on latest` on 1.

**The question to ask about a candidate** is not "does it record?" but "does anything read what it
records?" Three readers to check, in order: the arm's own folds, another declaration's, and the
emitting command's own. If all three answer no, the record is an audit fact and collapsing is free.

### Per arm, not per effect

An effect may mix modifiers, and keys may differ between its arms. The evidence is a real
`disconnect-shop` effect whose first arm derives a domain fact and must replay, while its second is
convergent and collapses. Arms of one effect keying by genuinely different identities usually means it
should have been two effects, since lanes are mutual exclusion and two arms touching one remote
resource want one lane space; that is a lint waiting on a warning channel rather than an error, since
the checker cannot know whether the two resources are disjoint. See "Known gaps".

## 16. A deployment secret is readable where the network is

heklang could hold one kind of credential and not the other, and the two get confused, so the split
comes first.

**Per-tenant credentials are already solved.** A shop's OAuth token arrives as a command, lands in
the log as a `@subject` field, and an effect folds it out and `reveal`s it. Rule 12 is built around
exactly that. Nothing here changes it.

**Per-deployment credentials had no answer.** A Discord webhook, a Stripe key, a SendGrid key. They
are not domain facts, they rotate out of band, they differ between dev, staging and production, and
there is one of each per deployment rather than one per tenant. The only way to write one was
`const WEBHOOK: String = "https://..."`, which is in git, in every surface that renders a
declaration, and **in the digest hash**, because a `const` is inlined at its use site and its text
goes into the packed form. A runtime records that hash as an invocation's `script_hash`, so
rotating the credential would cost replay coverage on every past invocation.

Putting the second kind in the log is the wrong log: "ops pasted a new Stripe key" is not something
that happened in the domain, it is permanent once written, it costs a fold per invocation, and an
empty database bootstraps to nothing, so "deploy is restart" becomes "deploy is restart plus a
runbook".

Everything below serves one sentence:

> **A secret is readable exactly where the network is reachable, and it may not be observed
> anywhere else.**

That is one rule covering both halves of the split, and it happens to be the `http.*` row of rule
11's table, so no new capability is introduced. A secret with nowhere to send it is pointless, and a
secret that can reach a log line, an event, a read model or a fold is a leak or a determinism break.

### The declaration

```
secret DISCORD_WEBHOOK
secret STRIPE_KEY
secret SENTRY_DSN?          // this deployment may not set it
```

**No type annotation**, because every real credential is text: a URL, a bearer token, an API key.
An annotation would be noise at best and would invite `secret PORT: Int`, which needs parsing and a
boot-time error path for something nothing wants. The declaration names a thing; its type is always
`Secret`.

**`secret` is soft**, claimed only where a top-level item begins. This document declares
`fn sync(shop_id: Int, domain: String, secret: String)` a few sections up and the language's own
tests declare an event field called `secret`; a hard keyword breaks both, for nothing. Nothing else
at top level is a bare word followed by a name, so the position is unambiguous.

**`secret NAME?` is an optional**, and non-optional is the default because a missing credential
should fail the deploy rather than be discovered at 3am. `Opt` composes for free, so the effect
branches:

```
let dsn = SENTRY_DSN
if dsn.is_some() { alert(dsn, "...") }
```

**The `let` is not decoration.** Narrowing (`docs/optionals.md`) is a property of a *slot*, and a
secret read is not one, the same way an inlined `const` is not. Read it into a `let` and the branch
narrows as it does for everything else.

Screaming case is convention, not enforcement, matching `const`.

### `Secret` is a taint, not a wall

This is the decision everything else follows from, and it is the deliberate difference from
`Sealed`.

`Sealed` is a wall: content may only be moved, asked about or `reveal`ed, because a runtime
genuinely cannot read it without a key. A secret is not like that. The runtime holds the plaintext
and is meant to. What must be true is that it never leaves through an **observable** channel. So the
rule is about sinks rather than about operations, and it is met wherever a type meets a value.

| Position | `Secret` |
| --- | --- |
| an `http.*` url argument | **yes**, a webhook address is itself a credential |
| an `http.*` header value | **yes** |
| an `http.*` body, at a member of the **literal** object, at any depth | **yes** |
| a `Secret` parameter or return of an effect-local `fn` | yes |
| a `let` in an arm or in an effect-local `fn` | yes |
| string interpolation | **yes, and the whole interpolation becomes a `Secret`** |
| `.is_some()` / `.is_none()` on a `secret NAME?` | yes, presence is not content |
| an *un-narrowed* `secret NAME?`, anywhere but those two | no |
| a boolean operand, a comprehension's yielded element | no |
| a `List`, a `Map`, a record field, an array inside a body | no |
| `log(..)`, `fail ..` | no |
| `emit`, `put` / `patch` / `update`, `invoke` | no |
| a `fold` seed, arm or filter | **no**, and this one is correctness |
| `==`, `!=`, ordering, arithmetic | no |
| any method on one, and `Json.encode` | no |
| a `const`, an entity column, a command parameter | no |
| read outside an effect arm or an effect-local `fn` | no |

**Interpolation taint is what makes it usable.** `"Bearer {STRIPE_KEY}"` is the single most common
shape a credential takes, and a wall would forbid it, leaving `Authorization` unwritable. Taint also
gives `"https://{host}/hooks/{HOOK_PATH}"` for free, and the result is still a `Secret`, so it still
reaches no sink. It survives a `let`, which is the same argument rule 12 makes for a seal: a type
composes where a spelling does not.

**"At any depth" means the literal object, and the allowance travels nowhere else.** A body is
written as an object, and an object written inside it is part of the same document, so a
credential may sit at either. Anything else in a member position is an ordinary declared
position that happens to be written inside a body: an `invoke` argument, a `fn` argument, a
`Json.encode`. Letting those inherit the allowance is not a near-miss, it is the whole rule
failing at once, because `invoke C { doc: { "k": KEY } }` inside a body puts the credential in
the **log**, where it is permanent and no rotation can reach it. So the allowance is granted to
an object literal in the argument itself, and re-granted only to an object literal directly
inside one.

**An optional must be narrowed before it is used for anything but presence.** An absent one has
no rendering, and the conversion table would give the literal text `null`: an interpolation
would build a url pointing somewhere else, silently. `.is_some()` and `.is_none()` ask about
presence and are always fine; every other position takes the `Secret` a narrowing proves, which
is the `let` the declaration section already shows.

**The `fold` prohibition is the sharp one, and it is the only sink whose reason is correctness
rather than disclosure.** A fold is a pure function of the log prefix and the trigger's own
position, which is what buys replay equivalence and verify mode (rules 3 and 14). A secret is not in
the log. A fold whose seed or arm read one would fold differently after a rotation, and `verify`
would be right to call that corruption. It is rejected where it is written, with rule 3's message
naming the fold rather than the read.

**A container may not hold one, and that is a decision rather than an omission.** The simplest rule
would be that containers carry the taint and every sink walks a value. It loses on a specific
ground: `Secret` is spellable in a `fn` signature and nowhere else, exactly as `Response` and
`Outcome` are, so `List(Secret)` cannot be written. Letting one be *inferred* would mean a
diagnostic that reads `expected List(String), found List(Secret)`, naming a type its reader has no
way to write. Rule 12 made the same call on measured evidence, and it holds here: no real credential
is one of several.

**`reveal` is deliberately not tainted.** A revealed email is meant to be compared, interpolated and
sent as data, and this document shows `log(reveal(copy))` as a working example. `Sealed` protects
content at rest and after erasure; `Secret` protects content in observable output. Different
threats, different mechanisms. The consequence is real and is in Known gaps: an author can still put
a revealed email in a log line, and nothing here changes that.

**Rejected: tainting `reveal`'s result.** Symmetric, and wrong for the reason above, and a breaking
change to every effect that reveals.

### The value carries its own redaction

A `Secret` holds **two renderings rather than one**: `plain` is what goes on the wire, and
`redacted` is what anything else is allowed to see, spelled `{SECRET:STRIPE_KEY}`. Interpolation
builds both in one walk, so an ordinary hole renders as itself in each and only the credential is
named.

The payoff is that the list of consumers is short and nobody has to remember a rule:

| Consumer | Reads |
| --- | --- |
| the wire, through `Http::send` | `plain` |
| the journal key | `redacted` |
| a transport failure's message | `redacted` |
| a `Display`, a diagnostic, a runtime error | `redacted` |

**Redaction is a property of the value, so every printing surface is fixed at once, including ones
nobody has written yet.** The obvious alternative, a host holding a list of credential strings and
scrubbing its own output, is O(secrets × output), fails on any encoded form, and needs a
minimum-length guard so a short credential does not redact half the log. It is a reasonable backstop
for a runtime to add; it is not the mechanism.

**The journal key is what this buys.** The Headers section below already argued that a replay which
recomputes a different idempotency key must still land on the entry that recorded the send. A
credential in the url or the body is the case that argument did not yet cover: rotate a Discord
webhook and every past entry keys on a string that no longer exists, so a crash-replay misses and
re-fires the POST, and every historical invocation reports a replay divergence. The key is built
from the redaction, so it is stable across a rotation. It never holds plaintext either, which
matters because the key is a readable description by design and a host may store it.

**A request therefore crosses the host seam in two renderings**, as separate fields rather than one
plus a convention, so a host sends one and prints the other and cannot pick wrong without the
compiler saying so. `docs/host.md` has the rule.

**Nothing is zeroized, and that is said rather than implied.** A `Value` shares its strings so a
program can be parsed once and served from many threads, and a shared buffer cannot be wiped. A
partial guarantee here would be worse than an honest absence, because it would read as a guarantee.
A host may zeroize the source it resolved from; heklang holds the plaintext for the life of the
invocation.

### What a host owes, and what it does not

One trait beside `Keys`, answering a value rather than a source: where credentials come from is a
host's business the way a key store is, and this document says nothing about environments, files or
vaults. `Program::secrets` is the deployment surface, so a runtime can answer "does this deploy need
a credential the last one did not" without decoding a digest.

A required secret a host cannot answer is a **wedge** that names the declaration, not a panic and
not a skip. A host is expected to make it unreachable by refusing to start; this is the backstop for
when it fails at that, and it wedges rather than skipping because unlike an erased subject it is
recoverable, and what recovers it is an operator setting the value.

**There is no `_PREVIOUS` list**, and this is worth saying out loud because a master key needs one
and someone will copy the shape. A master key wraps stored data, so both must be readable during a
rotation. A secret wraps nothing, so create-new, deploy, revoke-old is handled by a restart alone.

### Rejected alternatives

- **An `env("NAME")` builtin returning `String`.** One builtin, one host method, no type work, and
  wrong. It returns a plain string, so `log(env("X"))` compiles; it fixes neither the journal key
  nor the error text; the set of credentials is undeclared, so no host can fail fast and no plan can
  report; and a typo is a runtime wedge instead of a check error. In a language whose thesis is that
  the restrictions are the point, this is the shape that has none.
- **A `[config]` table in the runtime's own config file.** Everything above, plus the value is in a
  committed file.
- **Credentials in the event log under a reserved subject.** Genuinely tempting: it reuses
  `@subject`, erasure, rotation and the master key, gives an audit trail for free, and rotates
  without a restart. It is also already the right answer for the *per-tenant* kind. For deploy
  secrets it is the wrong log, per the split at the top of this rule. Keep both, and keep the split
  written down, because this is the alternative that will keep coming back.
- **A generic `config` declaration covering non-secret deployment values** (a base URL, a feature
  flag). A different requirement: those should be loggable, and lumping them in would make the type
  either over-redact or under-redact. If a case appears, `config NAME: String` is the additive
  sibling.
- **Secrets readable in a command.** The motivating case is verifying an inbound webhook HMAC, and
  the honest reasons to say no are not about determinism, since a command is not replayed. They are:
  a command's whole surface is HTTP-facing and a credential one typo from a refusal message is a bad
  place to be; there is no crypto in heklang for the case to be written against; and "readable
  exactly where the network is reachable" keeps this rule to one sentence. An effect verifies, then
  `invoke`s.


---

## The three kinds are deliberately not unified

Commands, projectors and effects all destructure an event and run a body, and it is tempting to make
`on` mean one thing. `docs/commands.md` and `docs/projectors.md` cover the other two in full; three
places where they must differ:

| | command | projector | effect |
| --- | --- | --- | --- |
| two arms on one event | n/a | **allowed**, fan-out is the point | **rejected** (rule 1) |
| writes | `emit`, under a guard | `put` / `patch` / `delete` | `invoke` only |
| clock | pinned | absent | journaled |

The first row is the one that matters. A projector has no journal and rebuilds from position 0, so
running four handlers for one event in declaration order is safe and useful. An effect journals, so
the same shape makes declaration order load-bearing for replay. The rule differs because the
consequence differs, not because the kinds were designed separately.

## No effect may trigger itself

An arm that invokes a command that emits the arm's own trigger type is an unbounded event stream. So
after parsing, heklang builds a directed graph over event types with an edge `trigger -> emitted` for
every (arm, invoked command, emitted event) and rejects a cycle, naming the path:

```
@order.placed -> NotifyCustomer -> RecordNotified -> @order.placed:
this effect can trigger itself, so the log would grow without end
```

hekla has no guard against this at all. An emit inside an `if` may never fire, so the check rejects a
program that *can* loop rather than one that provably does; that is the safe direction, and the
message names the loop rather than describing a symptom.

This is heklang's first whole-program check, and it belongs to the checker rather than the parser.
See "Checker obligations".

**The runtime backstop counts depth, not volume.** A `drive` follows the log as an `invoke` lengthens
it, and gives up if any chain of triggered events runs more than 32 deep. Depth is the measure
because it is the one that separates the two cases: an effect handling a thousand events appends a
thousand, every one of them one step from an event that was already in the log, while a runaway
appends one at a time forever, each a step further out than the last. It used to count the total,
which made it a limit on how much work one effect could do, and a port tripped it at seventeen sales.
Tripping it still means the static check above has a hole, which is the only thing it is for.

## What `reveal` models

The interpreter models the **key lifecycle**: a subject is erased or it is not, `erase` moves it one
way, and `reveal` fails once it has moved. That is what rules 9 and 12 actually turn on, and it is
enough to test both honestly.

**It holds no key and reads no content.** A sealed value is whatever a host stored, and `reveal` is
one call to `Keys::decrypt` with the coordinates the value carries. So heklang can say what a
program is allowed to do with content it cannot read, which is the whole of rules 9 and 12, without
being able to read any.

That is also why a fold costs nothing to walk. **A key is used once per `reveal`, not once per
record.** A boundary of twenty thousand events with a subject-bound field on every one asks a host
for one key, for the one value the handler actually revealed. hekla measured the other way round
before this: eager decryption at the read cost 3.5µs a record and made a fold four times its own
cost, for content nothing looked at.

The harness's own key store is the lifecycle and nothing else: content behind a live key reads back
as it was stored, and content behind a destroyed one does not read back at all. Opacity is a
property of a real host, which is where the bytes are.

What **is** enforced now is the boundary itself: content behind a seal cannot be read without
`reveal`, so a program that would hand a `CipherHandle` to hekla and expect a `String` no longer
passes `hek check`. That was the gap this document used to record here, and it mattered for a
specific reason: **green did not mean runnable.** A checker for hekla whose green light does not
imply hekla can run the program is not doing its job.

## Where this diverges from the runtime

These are places the *language* differs from hekla. `docs/host.md` is the other question, what the
*seam* asks of a host: rules 3, 5, 10, 11 and 12 are all reasons it is cut where it is.

| Topic | heklang | hekla today |
| --- | --- | --- |
| arms | one arm per event type (rule 1) | every matching arm runs, in declaration order |
| author failure | `fail`, counted separately (rule 4) | no author verb; giving up means raising, which wedges |
| `invoke` outcome | three cases (rule 6) | six-variant `CommandOutcome`, two of them retryable |
| `invoke` input | typed struct, checked at compile time (rule 7) | a Starlark dict, checked at dispatch |
| `now()` | pinned once per invocation, one journal entry | journaled per call, so two calls can disagree |
| self-triggering | rejected statically | unguarded |
| `erase` | a statement, no result (rule 9) | an expression returning a bool |
| delivery | declared per arm (rule 15) | one global lane, replaying from position 0 |
| deploy credentials | declared, typed and unobservable (rule 16) | a `const` in the source, or nothing |

## What the runtime owes rule 15

The language surface is a declaration and nothing else. These four are the dispatcher's, and they are
what make the declaration mean anything.

**They do not all have to land together, and the corpus says which order.** Lanes and the live
boundary carry 23 and 9 arms respectively, are well understood, and are what the eight-hour stall and
the replay-from-zero problem actually need. Batch collapse is the most complex of the four, because
recording the collapsed positions is what keeps replay equivalence true, and it has **one** customer.
So 1 and 2 first, then ship; 3 goes with them since it is the only way back from a `live` boundary;
4 can follow. Nothing is wasted by deferring it: `latest` is already coherent in the language, carried
in the digest and in the signature, and modelled by `hek test`, so the runtime half is the only piece
outstanding and an author who writes it today gets a checked program and a truthful test.

1. **Lanes.** Events with the same key are processed in log order. Events with different keys may be
   processed concurrently. This is an ordering guarantee the author chooses, not a parallelism hint,
   and it should be documented that way: two warranty plans in one shop write variants onto the same
   remote product, so they must share a lane even though per-plan parallelism looks tempting.

2. **The live boundary is a separate number from the watermark.** Do *not* implement `on live` by
   initialising the watermark to head. An effect may mix `on` and `on live` arms, and one cursor
   cannot start at both 0 and head. Store one additional integer per effect, resolved to the log head
   at first activation. The cursor still starts at 0; during catch-up the dispatcher simply does not
   invoke `live` arms for positions below the boundary.

3. **A rewind command, CLI only, against a stopped process.** It is the only way back to history for a
   `live` effect: flipping the arm to `on` and redeploying does not help, because the boundary is
   already persisted. It must **not** be an HTTP endpoint. A `live` effect is by definition one whose
   author declared that history must not fire, and those are exactly the effects where an accidental
   rewind re-sends every notification the log has ever seen. The journal will not save you: it is
   swept on a retention window and bounded by the watermark having moved.

4. **Batching for `latest`.** One invocation per key per dispatch batch, at the newest matching
   position in that batch. Record which positions were collapsed into the invocation, alongside the
   existing journal rows, so a re-run reproduces the same grouping and replay equivalence still holds.

## What the port wrote before rule 12 grew

Recorded because the workaround is the evidence, and because it is what the port's files still say
until they are changed. `reveal` used to require a field of the triggering event, which blocked eight
of that port's eleven effects outright: every credential there is folded off a `@shop.connected` that
happened long before the event being handled. With nowhere for a `String?` to go, the port wrote `""`
and `0` as absent-credential sentinels, in a language whose zero-value table in
`docs/projectors.md` exists to argue against exactly that:

```
fold token: String = ""          becomes    fold token: String? = none
if token.is_empty() { ... }            becomes    if token.is_none() { ... }
=> customer_name.unwrap_or("")         becomes    => customer_name
```

The last line is the one worth noticing: the `unwrap_or` was there to reach a `String`, and it was
also what dropped the subject binding, so removing the sentinel and gaining the propagation are the
same edit.

## Consuming an optional the code has proved present

`docs/optionals.md` has the rule. A branch that proves an optional present makes it its inner type
for as long as the proof holds, which is what deletes the port's `NO_..._FACTS` constants: whole
records of zeros that existed only to satisfy an `unwrap_or` on a branch three lines below the
`is_some()` that had already proved it could not be taken.

It matters here because the same shape reaches `reveal`: `record-warranty-sale` reveals `String?`
trigger fields whose guard is three lines up and invisible to the type. Rule 12's optional-in,
optional-out covers the fold, and narrowing covers the guard.

## Checker obligations

**Nothing is deferred any more.** The `@max` tightening invariant recorded in `docs/projectors.md`
was the last one and it now runs after the passes. Seven checks are **implemented**, and the first of
them has a reason to be where it is rather than only a history:

0. **The type check** (`docs/types.md`). It has to run while the program is lowered, because a
   numeric literal needs its scale before its IR node exists and a narrowed optional lowers to a
   different node than a plain one. Its tables live in `src/types.rs` with no parser state in them,
   so a checker elsewhere can reuse them.

The other six live in the parser only because nothing else exists yet:

1. **Erase-last reachability** (rule 9). Needs one arm's body, so the parser can host it, but it is a
   flow analysis and belongs with the others.
2. **The self-trigger cycle check.** Needs the whole program, and is the first thing heklang has that
   does. When the checker splits out, this one moves first.
3. **The fold subject checks** (rule 12): one variable holds one subject, and a plain arm may not
   join subject-bound ones. Needs one declaration, so the parser can host it.
4. **The decrypt boundary** (rule 12): sealed content may only be moved, asked about, or revealed.
   This one is a type rule rather than an analysis, so it is the one with the best claim to stay
   where it is; it fires wherever a type meets a value, which is everywhere the parser already looks.
5. **The `@max` invariant** (`docs/projectors.md`), over an entity column and over an `emit`. Needs
   the whole program, because the two declarations it compares can be in any two files, so it moves
   with check 2. The `emit` half is the one rule 12 makes load-bearing: a seal holds what a host
   stored, so a bound on moved content has no runtime left to check it.
6. **`on latest` never invokes** (rule 15). Interprocedural, but only within one effect, since an
   effect-local `fn` is invisible outside its braces. The same walk is what check 2 now follows, which
   closed a hole it had: an `invoke` inside a helper used to be invisible to the cycle check.
7. **The credential boundary** (rule 16). A type rule like check 4 and in the same place, running
   just before it at every position that declares a type, plus the handful that declare none: a
   comparison, an array element, an object member outside a request. It is the mirror of the decrypt
   boundary rather than a copy of it, and the asymmetry is the point: a seal may be **written**
   wherever the same seal is declared and may not be read, while a credential may be **read** freely
   and may not be written anywhere that could observe it. The one position that escapes is an
   `http.*` url, which is checked on its own rather than through a flag, because a flag would be
   inherited by everything nested inside a body and `http.post(url, { "x": leak(KEY) })` would
   launder through a helper's parameter.

The projector half of check 3 landed with it, so `docs/projectors.md` rule 9 no longer records a
no-op.

**Narrowing** (`docs/optionals.md`) is deliberately not on this list. It is a property of the
statement tree the parser is already walking, and it has to be known while lowering rather than after
it, because a narrowed load lowers differently.

## Known gaps

- **Response headers.** Request headers are `http.post(url, body, headers = { ... })`; nothing reads
  the response's yet.
- **Retry configuration** (rule 13).
- **Encrypting.** `Keys::decrypt` reads a seal and nothing writes one: a plain value emitted into a
  `@subject(...)` field crosses as plaintext and a host seals it, because that is the direction
  where the content is in hand. heklang holds no key in either direction, so this is a seam
  question rather than an absence.
- **The second erase rule** (rule 9), for an id that round-trips through an HTTP response. The
  `reveal` case is now checked where it can be written.
- **The journal key is a readable description**, not a content hash. It is stable and it prints,
  which is what a harness wants; a real host hashes it.
- **A revealed value is not a credential** (rule 16). `reveal` is deliberately untainted, so an
  author can still put a revealed email in a log line. That is rule 12's boundary working as
  designed rather than rule 16's failing: the two protect different things, and tainting `reveal`
  would be a breaking change to every effect that reveals for no gain in either.
- **A credential cannot be one of several** (rule 16). A `List(Secret)` is not spellable and not
  inferrable, so `{ "keys": [A, B] }` in a body is rejected, and so is a comprehension that would
  yield one. Nothing real wants it, and allowing it would put a type in diagnostics that no author
  could have written -- and `.contains` over such a list would answer, in a loggable `Bool`,
  whether two credentials are equal.
- **A `const` named `secret` collides with the declaration** (rule 16). `secret` is a soft word
  claimed by two tokens, so `const B: Int = secret` immediately above a `secret FOO` reads as a
  declaration and the parse stops with an unhelpful message. It is loud rather than silent, and it
  needs a const named `secret` in a language whose consts are screaming case, so it is recorded
  rather than fixed.
- **Journaled response bodies are stored in the clear.** An OAuth token exchange writes its access
  token into whatever a host keeps as the recorded result. That is a host's retention question and
  not a language one, and it is listed here so it is not mistaken for something rule 16 covers.
- **The disagreeing-keys lint** (rule 15). Arms of one effect keying by different identities is
  legal and sometimes necessary, and usually means it should have been two effects. It wants a
  `Severity::Warning`, and heklang has no warning producer yet: `settled` returns the first recorded
  diagnostic as an error and `check_files` has no channel on the `Ok` path. `docs/diagnostics.md`
  section 12 is where that lands. It must not become an error: the checker cannot know whether the
  two remote resources are disjoint.
- **Testing the live boundary** (rule 15). `deliver` runs an effect over the whole given log, which
  is all history, so an `on live` arm delivered faithfully would fire for nothing and be untestable.
  The harness therefore delivers `live` as `on` and says so in `docs/testing.md`. If a real case
  turns up, the additive move is an `activate <Effect>` statement usable between `given`s, splitting
  the log into history and live. Nothing needs it yet, and a real case should shape it.
