# Functions

```
fn effective_sku(sku: String?, plan_id: Uuid) -> String {
  let given = sku.unwrap_or("").trim()
  if given.is_empty() {
    return "{RESERVED_SKU_PREFIX}{plan_id}"
  }
  return given
}
```

Module scope, a required return type, and `return <expr>`. Callable from a command, a projector, an
effect and a `state` fold arm.

A `fn` declared inside an `effect` is the one exception to everything in the next section: it may
call out, and it is scoped to that effect. It has its own section below.

## This overturns "no user-defined functions"

heklang had no function declaration of any kind, and that was a decision rather than an omission: a
language whose pitch is smallness does not add a general abstraction because it might be handy.

The evidence that changed it is specific. In a 3,186-line port, `lib/` is five files of pure helpers,
and **`effective_sku` alone is needed at six call sites**: three commands, a projector and two
effects, all of which have to agree on the same answer, because the SKU is a uniqueness key. Without
a way to spell it once it is copied six times, and the sixth copy is where the bug lives. Four more
helpers are in the same position.

That is the shape of evidence worth reversing a decision on: not "this would be nicer", but "this
rule cannot be expressed once, and it is a rule that must hold in six places".

## A module `fn` is pure

No clock, no HTTP, no `invoke`, no `reveal`, no `erase`, no `emit`, no `put` / `patch` / `delete`. All
of it is a compile error inside a `fn`, and the message says which rule is being kept.

The restriction is not tidiness. **It is what keeps rule 9's erase-last analysis inside one arm.**
`docs/effects.md` rule 9 checks that no `reveal` is reachable from an `erase`, over one arm's
statement tree. The moment a helper can contain either, that analysis has to follow calls: it becomes
interprocedural, it needs a summary per function, and the error has to explain a path through code
the author was not looking at. Keeping both at the arm level means the analysis stays exact and the
error keeps naming two spans in one body.

Purity buys three more things, each of which would otherwise be its own rule:

- **A fold arm may call one.** `docs/effects.md` rule 3 requires a `state` fold to reproduce without a
  journal, which is why a fold cannot read a clock or call out. A pure `fn` cannot do either by
  construction, so "may a fold call a module helper" needs no answer of its own. An effect-local one
  is the first that cannot make that promise, and is the first a fold may not call.
- **A projector may call one.** A projector is a pure fold over the log, and a pure helper cannot
  break that.
- **Nothing needs journaling.** A `fn` produces no journal entry, so replay does not have to know it
  exists.

## An effect-local `fn` may call out

A `fn` declared inside an `effect` is the one helper that is not pure. It may `http.*`, `invoke`,
`log` and `fail`, and it is visible only inside those braces.

```
effect CreateMasterProduct {
  fn create(shop_id: Int, shop_domain: String, access_token: String) {
    let response = http.post(admin_url(shop_domain), { ... }, headers = admin_headers(access_token))
    if response.status == 401 {
      log("productCreate got 401, retrying on next reconnect")
      return
    }
    if graphql_error(response, "productCreate").is_some() {
      fail("productCreate failed")
    }
    invoke RecordMasterProductCreated { shop_id, product_id, default_variant_id }
  }

  on @shop.onboarding.completed, @shop.reconnected as e { shop_id } {
    state token: String = fold "" on @shop.connected(shop_id) { access_token } => access_token
    ...
    create(shop_id, domain, reveal(token))
  }
}
```

The evidence is the same shape as the one that added `fn` at all. A 3,186-line port declares
**thirteen** of these, and one of them, a 60-line HTTP sequence, is called by two arms of the same
effect. Six of the thirteen are pure and lift to module scope with no language change; the other
seven are the case, and they are not separable, because the first file that stops the checker holds
two pure helpers and one impure one.

### It may not `reveal` or `erase`

That is the whole restriction, and it is what keeps rule 9's erase-last analysis where the section
above says it lives: over one arm's statement tree, exact, naming two spans in one body. A helper
that could hold either would make the analysis interprocedural.

The restriction costs nothing, which is why it is the right one. **None of the port's thirteen
helpers contains a `reveal` or an `erase`**, while its arms use them 35 times across 11 files. Every
helper takes the already-decrypted value as a parameter, and the port's own comment says why:

```
  // Effect-local, so it may call out and invoke; it takes the revealed token
  // rather than the handle, which keeps every decrypt at the arm's own level.
```

The error says the same thing, and names the fix rather than the rule alone:

```
an effect-local `fn` cannot decrypt; it stays in the arm, which is what keeps rule 9's
erase-last check inside one statement tree, so pass the revealed value in as a parameter
```

### It may return nothing

```
fn create(shop_id: Int, shop_domain: String, access_token: String) {
  ...
  invoke RecordMasterProductCreated { shop_id, product_id, default_variant_id }
}
```

This is the only signature in the language that may omit `-> Type`, and a module `fn` still must
declare one. That is not an oversight of symmetry: **a pure function that returns nothing does
nothing**, so at module scope the omission is always a mistake and is worth rejecting. An
effect-local helper has effects, so no result is the honest signature. Four of the port's seven
impure helpers are written this way.

A call to one is a **statement**, never an expression:

```
create(shop_id, domain, reveal(token))       // a statement
let done = create(...)                       // `create` returns nothing, so a call to it is a
                                             // statement rather than a value
```

Making it a statement in the IR rather than an expression whose value is discarded is what lets the
error say that, at the call, instead of surfacing several lines later as a type that will not fit.

A bare `return` leaves **the helper**, not the arm, which is what makes a void helper usable as an
early-exit guard:

```
if response.status == 401 {
  log("productCreate got 401, retrying on next reconnect")
  return                       // the arm carries on
}
```

`fail(...)` is the one that ends the invocation, wherever it is written. A helper's `fail` produces
the same outcome and the same trace entry as an arm's; only the channel it travels on differs,
because a call is an expression and cannot carry a control-flow result out.

### What else it may not do

- **`state`.** A fold belongs to the arm (`docs/effects.md` rule 2), so pass what it decided in.
- **`now()`.** Rule 11 pins the clock once per invocation, into a slot the arm fills before its body
  runs. A helper has no such slot, and giving it one would make `now()` mean something different
  inside a call than outside it. Read it in the arm and pass it in. The port reads a clock in zero
  effect files, so there is nothing to weigh against the machinery.
- **`emit` and read-model writes**, for the reasons an arm cannot do them either.

### A fold arm may not call one

`docs/effects.md` rule 3 requires a `state` fold to reproduce without a journal. A module `fn` is
pure by construction, so the section above can say a fold may call one and stop there. This is the
first helper that cannot make that promise, so it is the first that a fold may not call.

**Rejected: a purity marker per helper**, so that a fold could call an effect-local one that happens
not to call out. It is a second rule, it puts a keyword on a declaration to describe what its body
already shows, and no fold arm in the port calls a local helper at all.

### Scope

Visible inside its own effect: that effect's arms and its sibling helpers. Order is irrelevant, as
everywhere else, because signatures are collected in a sweep before any body is read. `docs/effects.md`
has the shape.

Two different effects may each declare a `fn post`, the way two projectors may each declare an
`enum Status`. Shadowing a **module** `fn` is rejected, and that is not symmetry: a module `fn` is in
scope inside every effect, so a local one of the same name silently changes which code runs at a call
site that reads identically in two files. The port has no collision of either kind.

One consequence is worth keeping: within any one call graph the names stay unambiguous, so the
recursion path below still prints bare names, and no error had to learn a qualified spelling.

## Recursion is rejected

A `fn` may not call itself, directly or through another. The check is a cycle detection over the call
graph, and the error names the cycle as a path.

So every call terminates, and it terminates for the same reason the self-trigger check in
`docs/effects.md` gives: **by construction, not by a cap.** That matters most for a fold arm, which
re-runs on every attempt of a command and must not be able to hang; and for a command retry, where a
non-terminating helper turns a conflict into an outage.

**Rejected: a depth cap.** It is one line, and it turns a program error into a runtime one: the
author learns about the recursion at the worst moment, from a message about a limit rather than about
their code. A cap also cannot say what the cycle *is*, and the whole value of the static check is
that it can.

There is no `while`, and a `for` runs once per element of a finite container, so with recursion gone
there is no way to write a loop that does not end.

## Every path must return

A `fn` that can finish without producing its return type is a compile error. The analysis is the
falls-through one rule 9 already uses: an `if` with no `else`, or with a branch that falls out of it,
does not count as returning. A `fn` that declares no return type is exempt, because there is nothing
for it to fall through to.

A `for` body does not count either, because a container can be empty and the loop can run zero times.
That is the case a reader is most likely to get wrong, and it is exactly the shape a search helper
has:

```
fn first_error(items: List(Json)) -> String {
  for item in items {
    return item.string("message").unwrap_or("unknown")
  }
  return "unknown"          // required: `items` may be empty
}
```

## A `Response` may be a parameter

```
fn graphql_error(response: Response, field: String) -> String? {
  let errors = response.body.array("errors").unwrap_or([])
  ...
}
```

A `Response` is what `http.get` and its siblings return. It is the one type a `fn` may name that no
other declaration may, and the rule behind that is a sentence: **a `Response` is transport, not
data.** Reading one is pure, so a helper may take one; storing one is not, so nothing else may name
it.

Concretely, `Response` is spellable in a `fn` parameter and in a `fn` return type, and nowhere else.
An event field, an entity column, a record field, a `state` declaration and a command parameter all
still report `unknown type`, and so do `List(Response)` and `Map(String, Response)`, because the
allowance sits above the general type parser rather than inside its recursion.

**Why storing one is the thing being prevented.** An event is the durable record, and a `Response` in
one puts a status code and a transport body in the log forever, where a replay years later folds over
whatever the remote host happened to answer. Rule 3 in `docs/effects.md` asks a fold to reproduce
without a journal; a stored `Response` is the shape that makes that impossible while looking like
ordinary data. The read model has the same problem one step later.

**This was a gap, not a decision.** `Type::Response` and `Value::Response { status, body }` were both
already in the IR, and `.status` / `.body` were already checked; only the spelling was missing, so a
pure helper over a response could be written everywhere except in its own signature. Two of the six
effect-local helpers that the section above counts as "already pure, and therefore liftable to module
scope" take a `Response`, so that count was assuming this worked.

**Rejected: teaching the general type parser.** One arm on `type_ref` reaches every position at once,
including the event field, and an accidental rejection elsewhere (an entity column has no zero value
for a `Response`, so it fails for an unrelated reason) is not the same as a rule.

**Rejected: passing `.body` at the call site.** The port's two helpers read only `body`, so both
could take a `Json` instead, and it needs no language change. It does not generalise: `.status` is
read at twelve sites, five of them the same unauthorized check, and the first helper that wants one
is back here.

## A `fn` may decide a refusal

That day arrived, and the rule extended unchanged. `Outcome` is spellable in exactly the positions
`Response` is, for a sentence with the same shape: **a refusal is a decision, not data.** Producing
one is pure, so a helper may return one; storing one is not, so nothing else may name it.

```
fn ladder(subscribed: Bool, taken: Int, cap: Int) -> Outcome? {
  if subscribed { return reject("already_subscribed", "already on the course") }
  if cap == 0   { return invalid("this course has no capacity set") }
  if taken >= cap { return reject("course_full", "the course is full") }
  return none
}

command Subscribe(course: Uuid, student: Uuid) {
  state subscribed: Bool = fold false
    on @StudentSubscribed(course, student) => true
  ...
  let refusal = ladder(subscribed, taken, limit)
  if refusal.is_some() {
    return refusal
  }
  emit @StudentSubscribed { course, student }
}
```

`none` is "no objection", so the `fn` returns `Outcome?` and the caller proves it present the way it
proves any other optional present. Nothing about `Outcome` bends the narrowing rule.

**What this replaces.** A check-then-do pair of commands shares a decision, and before this the only
way to share it was a `String` whose emptiness meant "allowed", plus a second `fn` to turn a code
into a message. That is a sentinel, in a language whose optional story exists to remove sentinels,
and the caller had to evaluate the ladder twice because there was nothing to bind before the `if`.

**`reject` and `invalid` are expressions now, not only statements.** The written forms in a command
are unchanged and are still parsed as statements, so nothing about `return reject("code", "why")`
moved. What is new is that the same two words produce a value where an `Outcome` is expected, which
is what a `fn` returns and what a command's `return` accepts.

**A `fn` that did not declare one still cannot write one**, and the error says how to:

> `invalid` is a command's outcome; declare it `-> Outcome` or `-> Outcome?` to decide a refusal the
> caller returns, or return a value the caller branches on

**An effect is unaffected.** Rule 4 keeps `fail` as its terminal outcome, and `reject`/`invalid` in
an effect or an effect-local `fn` still say so. A projector still cannot write either, because a
projector write cannot fail in a way the program observes.

**What this does not do.** Two commands can now share the decision; they still each declare their own
`state`, because a `state` declares the append condition and that has to be the command's own. The
duplication that remains is the fold block, not the logic, and a check-and-do pair is better served
by the host running the real command without appending than by a second command that copies it.

## What a `fn` makes possible, and is therefore not in the language

**`Timestamp.add_months(n)` is deliberately deferred.** A real port carries 33 lines of calendar
arithmetic to compute a warranty's expiry, implementing the semantics where 2026-01-31 plus one month
is 2026-02-28. Getting month-end clamping wrong is a real bug and the argument for a builtin is a
good one.

It is still the wrong place for it. **Month-end clamping is one calendar opinion among several**, and
a builtin commits the language to it forever: clamp to the last day, roll into the next month, or
refuse. Different jurisdictions and different contracts want different ones, and a language that
picks cannot be argued with.

**A port proved the deferral was empty, and that is what fixed it.** Reaching for `add_months` used
to fail at run time; then it failed at check time with the argument above; and neither was any use,
because nothing could write the replacement. A `Timestamp` had no methods, no arithmetic and no way
in or out but `Timestamp.parse`, so the opinion the language refused to hold had nowhere else to be
held. A decision with no escape hatch is not a decision, it is a hole with a rationale attached.

So the language supplies the calendar and the author supplies the rule. `at.year()` through
`at.second()` read a moment's fields, and `Timestamp.from_parts(...)` builds one back, returning a
`Timestamp?` because six numbers do not always name a date. **That optional is where the opinion
goes**: February has no thirtieth, and what to do about it is the thing a builtin would have decided
for everyone.

```
// Clamping, in a lib/ an application can disagree with.
fn add_months(at: Timestamp, months: Int) -> Timestamp {
  let total = at.year() * 12 + at.month() - 1 + months
  let year = total / 12
  let month = total % 12 + 1
  let last = days_in_month(year, month)
  let day = if at.day() > last { last } else { at.day() }
  return Timestamp.from_parts(
    year, month, day, at.hour(), at.minute(), at.second(),
  ).unwrap_or(at)
}
```

Rolling over instead is the same function with `day` left alone and the `unwrap_or` replaced by a
carry into the next month. Refusing is the same function returning the `Timestamp?`. All three are
ten lines and none of them is the language's to pick.

That helper belongs in a shipped `lib/` where an application can read it,
disagree with it and replace it. That is the general shape of the rule: **a `fn` is where an opinion
goes, and the language is for what has no defensible alternative.**

## Calls, order, and what is absent

A call is `name(args)`, resolved after a local binding and after the builtin names, so a local still
shadows and `log` still means `log`. Arguments are checked against the declared parameters by
position at parse time, with each parameter's type as that argument's hint, so literal inference and
enum resolution work through a call the way they work through `emit`.

Signatures are collected before any body is parsed, so a `fn` may call one declared later or in
another module. `docs/declarations.md` has the pass table.

- **No default arguments and no named arguments.** `invoke` has named fields because a command's
  input is a struct with an independent shape; a helper's arguments are positional because they are
  a short list the reader can hold.
- **No closures, no function values, no generics.** A `fn` is a name for a repeated expression, not
  a value. Every one of the five helper files in the port is that.
- **No overloading.** One name, one function, in one namespace with commands, projectors and effects
  kept separate from it for the reason in `docs/declarations.md`.
- **A trailing comma closes an argument list**, in a call to a `fn`, a method, or any builtin
  (`reject`, `invalid`, `log`, `fail`, `erase`, `Uuid.derive`, `Json.encode`, `http.*` and the
  `docs/parsing.md` set). That matches the last field of a record literal, the last element of a
  list, the last parameter of a signature and the last field of an `emit`, all of which already took
  one. A call that takes no arguments does not, because there is no last item for the comma to
  follow, so `now(,)` stays an error.

  It had been the one exception, and only for the fixed-arity builtins, whose parsers read
  `arg`, `,`, `arg`, `)` literally. A port found it by writing a long `reject` across three lines,
  where every formatter puts a comma after the last argument, and got ``expected `)` `` with no
  explanation. Rule 13's ``takes 2 arguments; a timeout is configuration`` still fires for a real
  third argument, because a trailing comma is recognised as a comma followed by the closing paren
  rather than by eating any comma that appears.
