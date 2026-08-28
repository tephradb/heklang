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
  on @order.placed as e {
    state orders: Int = fold 0
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
   @warranty.plan.updated as e { shop_id } { ... }
```

Every listed path still selects that one arm, and a path appearing in two arms of one effect is
still an error naming the first. What is given up is only consequence #2 below, "an arm names one
concrete type", which was listed as a consequence rather than as the reason.

The evidence is one real effect that dispatches **thirteen** event types onto one body. With
one arm per type, that effect is thirteen copies of six `state` declarations and a forty-line body,
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
argues for effect-level `state`, which this pass deliberately did not add. Per-arm state solved a
real ordering problem, and multi-path arms plus records may shrink the duplication that motivates it,
so the case is worth re-measuring before it is answered.

**Projectors legitimately want the opposite rule, and keep it.** Fanning one event out to several
read models is a real pattern, and a projector has no journal, so nothing about ordering is
dangerous there: a rebuild replays every handler in the same order and reaches the same rows. This is
a difference in the code and not only in the prose, since heklang's `fold_into` already runs every
matching handler. See "The three kinds are deliberately not unified".

## 2. State lives inside the arm

`as e` binds the trigger, and it is in scope for the arm's `state` filters and its body. There is no
effect-level trigger binding and no effect-level state.

This is more expressive than hekla's single `query(event)`, which has to work for every subscribed
type and so in practice restricts filters to fields common to all triggers. Per-arm state has no such
constraint: each arm knows exactly one event type, so it can filter on any field of it.

An arm is `on @path [as name] [{ destructure }] { body }`, the same shape a projector handler has
(`docs/projectors.md`, rule 1). With two blocks the first destructures payload fields and the second
is the body; with one there is nothing to destructure. The two kinds share one construct rather than
each having a slightly different one.

An arm's prologue is **`state` alone**. A command hoists a leading `let` so that a filter can name
it; an arm's filters have the trigger binding instead, so a `let` in an arm is an ordinary body
statement and may call out. There is no `guard` either, because an effect has no append condition to
build.

## 3. The fold stops at the trigger's own position, inclusive

`state` is folded over the log up to **and including** the triggering event, never to the head of the
log. It is therefore a pure function of the log prefix and that position, so every attempt and every
replay reproduces it. Three things follow, and together they are why an effect has no read of a
projector:

- **It cannot race.** There is no other module to be behind.
- **It is not journaled.** Every attempt re-folds and gets the same answer, so there is nothing to
  record. Contrast `now()`, which is journaled precisely because it cannot reproduce itself.

  That only holds if a fold cannot do anything it would have to record, so a filter, a `state` seed
  and a fold arm may not call out, invoke, decrypt or read a clock. Each is a compile error naming
  the fold rather than the builtin, because the fold is the reason. This is the same boundary hekla
  draws by evaluating `query` and `fold` without a request context.
- **It counts the trigger.** An effect folding its own trigger type sees itself, so a customer's
  first order leaves a count of one, not zero.

Folding to head instead would make state depend on how far the log had run when the handler happened
to execute, which is the same value reading differently on a retry.

## 4. `fail("reason")` is the author's terminal outcome

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
for the same reason rule 8's accessors are: every read is a branch anyway.

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
| `none` | `null` |
| `some(x)` | whatever `x` converts to |

`Money` and `Decimal` become strings rather than numbers so no precision is lost to a float on the
far side, which is the same reason they are scaled integers here. Neither carries a currency; a
program that needs one sends the field it declared for it (`docs/money.md`).

**Object keys are sorted.** That is rule 14's defined iteration order (see below), and it is why the
same object built twice serialises byte-identically.

Iteration over JSON arrays is deferred, and there is no list type. An array can arrive in a response
body and be carried around; nothing in the language takes it apart yet.

## 9. Erase last, statically enforced

A `reveal` **reachable after** an `erase` within one arm is a compile error.

The reason is the whole contract in one line, and the error message says it: `erase` is journaled and
`reveal` is not. A replay skips the erase, because it is recorded as done, and then re-runs the
reveal against a key that is gone. Calls journaled before the erase stay done, so nothing re-fires,
but everything after the reveal does not run on that replay.

This is a **reachability analysis over the arm's control flow**, not a lexical ordering check. The
difference is visible in one line:

```
if x { erase(e.customer_id); fail("gone") }
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

**If a loop is ever added**, this analysis has to change shape, and it is worth writing down what to:
iterate to a fixed point, and treat an erase anywhere in a loop body as poisoning the whole body,
including statements lexically above it. That is exactly the case a lexical check gets silently
wrong, which is why the analysis is written as reachability now, while the control flow is still a
tree.

**It stays one arm wide, and that is a decision now rather than a happy accident.** An effect may
declare its own helpers, and one of those may call out. It may not `reveal` or `erase`: those stay in
the arm, so this analysis never has to follow a call, never needs a summary per function, and never
has to explain a path through code the author was not looking at. See `docs/functions.md`.

**Known gap: the second rule is not implemented.** hekla also says do not erase a subject whose id
you learned by revealing, because a repeat request for an already-erased subject then cannot be read
at all. Enforcing it needs data flow that round-trips through an HTTP response, which is more than
this pass builds. Take subject ids from a plaintext field or a read model.

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
| `erase(subject_value)` | nothing | yes |

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
| a `state` fold (either kind) | absent |
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
  fn sync(shop_id: Int, domain: String, secret: String) -> Bool {
    let response = http.post("https://{domain}/admin/api/sync", { "shop": shop_id },
      headers = { "X-Access-Token": secret })
    if response.status >= 400 {
      fail("sync rejected with status {response.status}")
    }
    return true
  }

  on @shop.sync.requested as e { shop_id } { ... sync(shop_id, domain, reveal(token)) }
  on @shop.reconnected as e { shop_id } { ... sync(shop_id, domain, reveal(token)) }
}
```

`docs/functions.md` has the rule and the evidence. What matters here is the three things it does not
disturb:

- **Rule 9 is unchanged.** A helper may not `reveal` or `erase`, so the erase-last analysis still runs
  over one arm's statement tree. That is the restriction the whole design is built around, and it is
  free: no helper in the port that motivated this contains either.
- **Rule 3 is unchanged.** A `state` fold may not call one, because a fold has to reproduce without a
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

### What it takes: a field of the trigger, or a fold of one

```
on @shop.sync.requested as e { shop_id } {
  state token: String? = fold none
    on @shop.connected(shop_id) { access_token } => access_token
    on @shop.reconnected(shop_id) { access_token } => access_token

  let secret = reveal(token)
```

A credential is almost never on the event being handled. It was appended when the shop connected,
long before, so **subject binding propagates through a `state` fold**: a variable folded from a
`@subject(...)` field is still subject-bound and can be revealed. That is the same propagation
`docs/projectors.md` rule 9 already performs through a projector write, and it works the same way,
by recovering a schema fact in the parser rather than by carrying anything on the value.

An arm makes the variable subject-bound when its result **is** the field, a bare load of a
destructured `@subject(...)` name. A transformed one (`=> access_token.trim()`) is not: the result is
a new value, and the schema says nothing about it. This is the same line `reveal` already draws for a
trigger field, and it is why the fold above holds `String?` rather than folding an
`unwrap_or("")` into a `String`.

**The subject id is folded too.** It cannot come from the slice's filter, because a fold is not
always filtered on its subject: a fold of `customer_name @subject(customer_id)` filtered on
`warranty_id` is an ordinary shape. So each subject-bound variable gets a companion the author never
writes, folded by the same arms, holding the subject of the value currently held. It is absent
exactly when the fold never matched, which is what makes the seed distinguishable from a real value
below.

**Rejected: an opaque handle carried on the value.** A subject-bound field would be a distinct type
all the way through, and the decrypt boundary would be enforced instead of documented. That is the
right end state and it is recorded under "What `reveal` models" as a known gap; it touches every
value path, including JSON, projector writes, `@max` and interpolation, and it is a larger pass than
this one.

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
  `state token: String? = fold none` seeds with nothing and folds credentials into it.
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
before this rule existed: `state token: String = fold ""` and then `token.is_empty()`, where an
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
- *A journal read back by a different program version* (rule 7). Replay equivalence is exactly the
  check that notices a handler no longer making the calls it recorded.
- *Anything a future builtin adds.* The list above is short because the builtin set is closed. It
  should be re-derived whenever it stops being.

The goal is that verify keeps covering only causes not yet nameable. It should keep shrinking.

---

## The three kinds are deliberately not unified

Commands, projectors and effects all destructure an event and run a body, and it is tempting to make
`on` mean one thing. Three places where they must differ:

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

## What `reveal` models

The interpreter models the **key lifecycle**: a subject is erased or it is not, `erase` moves it one
way, and `reveal` fails once it has moved. That is what rules 9 and 12 actually turn on, and it is
enough to test both honestly.

It does not model ciphertext. A subject-bound event field is an ordinary `String` here, so **nothing
forces a value through `reveal` before it leaves the process**. In hekla the same field is a
`CipherHandle`, opaque until revealed, so the decrypt boundary is enforced rather than documented.
Closing the gap means a distinct type carried through the whole pipeline, including the projector
write path where `docs/projectors.md` rule 9 propagates subjects today. Recorded as a known gap, not
a bug.

## Where this diverges from the runtime

| Topic | heklang | hekla today |
| --- | --- | --- |
| arms | one arm per event type (rule 1) | every matching arm runs, in declaration order |
| author failure | `fail`, counted separately (rule 4) | no author verb; giving up means raising, which wedges |
| `invoke` outcome | three cases (rule 6) | six-variant `CommandOutcome`, two of them retryable |
| `invoke` input | typed struct, checked at compile time (rule 7) | a Starlark dict, checked at dispatch |
| `now()` | pinned once per invocation, one journal entry | journaled per call, so two calls can disagree |
| self-triggering | rejected statically | unguarded |
| `erase` | a statement, no result (rule 9) | an expression returning a bool |

## What the port wrote before rule 12 grew

Recorded because the workaround is the evidence, and because it is what the port's files still say
until they are changed. `reveal` used to require a field of the triggering event, which blocked eight
of that port's eleven effects outright: every credential there is folded off a `@shop.connected` that
happened long before the event being handled. With nowhere for a `String?` to go, the port wrote `""`
and `0` as absent-credential sentinels, in a language whose zero-value table in
`docs/projectors.md` exists to argue against exactly that:

```
state token: String = fold ""          becomes    state token: String? = fold none
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

The two recorded in `docs/projectors.md` still stand (the `@max` tightening invariant and rule 9's
subject checks). Three checks are **implemented** and live in the parser only because nothing
else exists yet:

1. **Erase-last reachability** (rule 9). Needs one arm's body, so the parser can host it, but it is a
   flow analysis and belongs with the others.
2. **The self-trigger cycle check.** Needs the whole program, and is the first thing heklang has that
   does. When the checker splits out, this one moves first.
3. **The fold subject checks** (rule 12): one variable holds one subject, and a plain arm may not
   join subject-bound ones. Needs one declaration, so the parser can host it, but it is the same
   check `docs/projectors.md` rule 9 states for a projector write and still does not perform.
   Whichever of the two moves out first should take the other with it.

**Narrowing** (`docs/optionals.md`) is deliberately not on this list. It is a property of the
statement tree the parser is already walking, and it has to be known while lowering rather than after
it, because a narrowed load lowers differently.

## Known gaps

- **Response headers.** Request headers are `http.post(url, body, headers = { ... })`; nothing reads
  the response's yet.
- **Retry configuration** (rule 13).
- **Opaque subject values**, above.
- **The second erase rule** (rule 9).
- **Arrays** in a JSON body can be carried but not taken apart (rule 8).
- **The journal key is a readable description**, not a content hash. It is stable and it prints,
  which is what a harness wants; a real host hashes it.
