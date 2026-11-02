# Tests

A `test` is a declaration, like `command` or `projector`, so a file of tests is an ordinary module.
It states a log, one thing to do to it, and what should come out.

```hek
test "a sale on a deleted plan does not resurrect it" {
  given @plan.created { plan_id: 1, title: "Two-year cover", price: 19.99 }
  given @plan.deleted { plan_id: 1 }
  given @plan.sold { plan_id: 1, price: 19.99 }

  project Plans

  expect no Plan[1]
  expect Sales[1] { revenue: 19.99 }
}
```

Run them with `hek test <path>`.

## 1. Shape

```
test "<name>" {
  <given>*
  <respond | erased>*
  <action>
  <expect>*
}
```

Sections appear **in that order**, and each is optional except the action. There is exactly one
action.

**The name is a string, not an identifier**, because it is what a failure report reads out. Two tests
may not share a name inside one program.

`given`, `respond`, `erased`, `secret`, `run`, `project`, `deliver`, `fill`, `expect`, `no`,
`nothing`, `skipped` and `global` are claimed **only inside a test body**, so an entity field called
`given` or a parameter called `no` stays writable elsewhere. The last three are claimed in one
position only, directly after `expect`, where each would otherwise read as an expression. `test` is
the only word this construct reserves outright. The `fold` action is the keyword a declaration already starts with, and nothing is
ambiguous: a statement's `fold` binds a local and the action does not, so `=` against `{` decides
it.

## 2. `given` is a log, and it is literal

`given @event.path { field: value, ... }` appends one event, and several `given`s make a log in the
order written. **Every field must be given**, the same rule `emit` follows, and the block takes the
same bare-name shorthand: `{ order_id }` is `{ order_id: order_id }`. A test body binds nothing, so
that only resolves inside a fixture `fn`, whose parameters are named after the fields they fill.
The values are ordinary expressions: literals, `const`s, enum variants, `fn` calls, interpolation
and containers.

A `fn` is how a suite gets a helper for near-identical events, for a field value:

```hek
fn plan_sku(n: Int) -> String {
  return "STRESS-{n}"
}
```

and for the whole event, which is what stops twenty cases repeating a 26-field block. A `fn` may
return an event, and a `given` or an `expect` takes the call with the fields that case is about
written after it:

```hek
fn t_order(order_id: Int, total: Money(3)) -> @order.placed {
  return @order.placed { order_id: order_id, total: total, status: Open, note: "" }
}

test "an edit keeps the rest of the order" {
  given t_order(9001, 84.500)
  given t_order(9002, 12.000) { note: "gift" }

  run EditItem { order_id: 9002 }

  expect t_order(9002, 12.000) { status: Edited }
}
```

The fixture gives every field and the block replaces the ones it names, so an event is still written
whole. The block may not be empty, may not name a field the event does not have, and may not name one
twice. A fixture may return what another fixture made (`return t_order(id)`), which is how a second
fixture is built on a first.

**An event field's `@max` is enforced at the `given`**, on the merged event, so an override can
correct a fixture that is past the bound or introduce one. A test that writes a value past it
**errors** rather than fails: the case could not state its world.

**Do not build a fixture by running a command.** There is no `given run PlaceOrder(...)`, on purpose:
a fixture built by the thing under test means one broken command fails every test that used it as
scenery, and the report names the wrong test. Raw events keep a failure local.

There is no way to seed a fold directly. A folded value is derived from the log.

## 3. Setup

| Statement | Meaning |
| --- | --- |
| `respond "<url>" <status>` | queue one HTTP reply for that URL |
| `respond "<url>" <status> { <json> }` | the same, with a body |
| `respond "<url>" timeout` | a transport failure, which the runtime absorbs and retries |
| `erased <subject> "<id>"` | that subject's key is already destroyed |
| `secret <NAME> = "<value>"` | this deployment holds that credential |
| `secret <NAME> = none` | this deployment does not set it |

Replies are a **queue per URL**, taken in order, so `respond url 503` then `respond url 200` is how a
test says the first attempt was absorbed. `erased` is the only way to write a shredded-key test,
since a test cannot call `erase` itself. The URL may be a `const`.

`erased` belongs to the world, not to the action: the same line makes a `deliver` `expect skipped`
where it reveals, and empties the sealed column a `project` writes, so `expect Row[k] { col: none }`
is how a test asserts the personal data left the read model. A parent's erasure takes its children:
`erased Shop "42"` shreds every `Customer` the given log puts under shop 42, and an effect's own
`erase(shop_id)` does the same, because the cascade belongs to the key store rather than to the
setup line.

A declared `secret` needs **no** setup line: it answers `secret:NAME`, so a test that does not care
about the value writes `respond "secret:HOOK" 200` and expects `http.post("secret:HOOK")`. The
directive is the override, for a value that has to parse as a URL or carry a prefix an arm branches
on, and it goes next to the `respond` that answers it. `= none` is the only way to reach the absent
branch of a `secret NAME?` or the wedge a missing required one causes.

```hek
secret CONFIRM_URL = "https://mail.example/confirm"
respond "https://mail.example/confirm" 200
```

## 4. The action

Exactly one, and it decides which expectations are legal:

| Action | Runs |
| --- | --- |
| `run Command { name: value, ... }` | the command against the given log |
| `project Projector` | the projector over the whole given log |
| `deliver Effect` | the effect over the whole given log, following it as an `invoke` lengthens it |
| `fold Fold { name: value, ... }` | the named fold over the whole given log, for that key |
| `fill` | the whole given log into the derived read model, so rows of it can be asserted |

**`fold` and `fill` run no handler, and they are the only actions that do not.** The other three
name something that decides, projects or calls out; a named fold is a value and a key, and the
derived read model is every named fold at every key, so each action is the read itself. Sections 8
and 9 are what they expect.

**`fill` is the only action with nothing to name**, because nothing declares what it fills: a
program has one derived read model and it is derived from the folds, where `project` picks one
projector out of several.

**`deliver` drives, so there is no position to write.** An effect that fires on three of the given
events makes three invocations, and the expectations are the whole trace across all of them.

**The given log is a catch-up batch**, so an `on latest` arm collapses over it: three events for one
key are one invocation, at the newest of them, and two keys are two invocations. An `on live` arm is
delivered as `on`, because its boundary is a position the runtime resolves at first activation and is
not a property of the log; a test can say what the arm does when it runs, and nothing about when it
starts running.

## 5. What `run` expects

| Expectation | Matches |
| --- | --- |
| `expect @path { field: value, ... }` | one appended event, in order |
| `expect nothing` | the command appended no events |
| `expect invalid "<message>"` | `Outcome::Invalid` |
| `expect reject <Name>` / `expect reject <Name> { field: value }` | `Outcome::Reject` |

The appended events must match the `expect @path` lines **one for one and in order**, and an event
the test did not write is a failure. **Every field of an event is matched**, so list all of them.

`expect nothing` is a real assertion: "recreating the same plan id is a no-op" is a case worth
stating, and writing no `expect` at all would say it by accident.

## 6. What `project` expects

| Expectation | Matches |
| --- | --- |
| `expect Entity[key] { field: value, ... }` | the row exists and the listed columns match |
| `expect no Entity[key]` | there is no such row |

The subscript is the whole key, so a compound-keyed entity names every column in key order:
`expect Period[42, "2026-10"] { counted: 2 }`.

**A row is matched on the listed columns only**, because a row is wide and most of its columns are
carried through untouched by the handler under test. That is the deliberate difference from an event,
which is matched on all of its fields.

`expect no` is what makes the difference between `patch` and `update` testable at all.

**A subject-bound value is matched on its plaintext**, and the rule is the same for a derived column
in section 9:

```hek
expect Shop[1] { shop_name: "Test Shop" }
```

A sealed column holds whatever the world put there, which under the harness is the text the test
wrote and against a host that really encrypts is ciphertext, so the runner **opens it first**: the
same `decrypt` call `reveal` makes, under the field, subject and id the value carries. Two
consequences worth knowing before you write one:

- **An erased subject reads as the absent optional**, which is the value rule 12 gives it, so
  `expect Shop[1] { shop_name: none }` is how an erasure is asserted.
- **A sealed column with no absent value errors rather than mismatching.** A non-optional
  `Sealed(T, S)` whose key is gone cannot be compared against anything, so the test lands in the
  **errored** bucket of section 9 with *"the column is not optional so it cannot read back
  absent"*. That is the same answer reading the column any other way gives, and the fix it names is
  to declare the column optional.

Do not write an expectation against ciphertext. There is no spelling for it and no reason to want
one.

## 7. What `deliver` expects

An effect's output is a **trace**: the ordered list of things it did to the world.

| Expectation | Matches |
| --- | --- |
| `expect http.<verb>("<url>")` | a call to that URL, body not checked |
| `expect http.<verb>("<url>", { <json> })` | the same, and the body matches the listed keys |
| `expect invoke Command { name: value, ... }` | an `invoke` with exactly those arguments |
| `expect erase(<subject>, "<id>")` | an erase, naming the subject field and the id |
| `expect log("<message>")` | one `log` line |
| `expect fail "<message>"` | an arm returned `fail` |
| `expect skipped` | an arm hit a shredded key |
| `expect nothing` | the effect did nothing observable |

Ordered and complete, like `run`'s: the trace and the expectations line up one for one, including
every `log`.

**A body is matched on the keys the test writes**, because a request body is often a large generated
document. **`invoke` arguments are matched exactly**, because a command's arguments are its whole
input.

**A number in a body is compared by its spelling.** `3` and `3.0` are the same number and not the
same JSON, so write what the wire carries.

An `Uuid.derive(seed, name)` in an expectation is evaluated the same way it is in the arm, so a
derived id is written as the same call rather than as a hard-coded uuid.

## 8. What `fold` expects

| Expectation | Matches |
| --- | --- |
| `expect <value>` | the value the fold folded to, for the key the action named |

```hek
test "lifetime spend nets the cancellation" {
  given @order.placed    { customer_id: 7, total: 400.00 }
  given @order.cancelled { customer_id: 7, refund: 150.00 }

  fold LifetimeSpend { customer_id: 7 }

  expect 250.00
}
```

**Exactly one, and that is the difference from every other action.** `run`, `project` and `deliver`
each produce a sequence and take as many expectations as it has members; a fold has one accumulator,
so a second `expect` is a second answer to one question and is refused where it is written. So is a
test with none, which would fold a value and throw it away.

The arguments are the fold's parameters, written exactly as a command reads it minus the binding,
and they **are** the key. A key no event touched folds to the **seed** rather than to nothing, which
is what makes `expect 0` an assertion about the seed and the reason there is no `expect nothing`
here.

The value is compared under the fold's declared type, so a `Money(2)` is matched on its scale.

**A query-only fold is readable here, and nowhere else** (`reference/folds.md` section 4). A test is
not a decision, and the envelope is exactly predictable under the harness: one minute per event from
`2020-01-01T00:00:00Z`, so the third `given` is at `"2020-01-01T00:02:00Z"`.

## 9. What `fill` expects

| Expectation | Matches |
| --- | --- |
| `expect <table>[key] { column: value, ... }` | the row exists and the listed columns match |
| `expect no <table>[key]` | no key like that was ever written |
| `expect global { column: value, ... }` | the single-row table's listed columns match |

```hek
test "a cancellation nets off the order's total" {
  given @order.placed    { shop_id: 1, order_id: 9001, total: 42.50 }
  given @order.cancelled { shop_id: 1, order_id: 9001, refund: 2.50 }

  fill

  expect shop__order[1, 9001] { order_total: 40.00, order_cancelled: true }
  expect no shop__order[1, 9002]
}
```

This is section 6 over a derived table, and the two are the same assertion: a row of N columns is N
fold reads, so where a projector's row test became N `fold` tests, `fill` puts it back. The subscript
is the whole key in key order and the listed columns only, exactly as section 6 has them.

**The names are the ones `hek check --tables` prints**, and both are checked when the test is
parsed, so a typo is a `hek check` error naming the tables that do exist. Run `--tables` to get them
rather than guessing; `reference/folds.md` section 2 is the derivation.

**One thing reads differently from a projector's row, and it is the fold's own rule.** An entity has
no field it was never given, and a fold over no matching events folds to **its seed**, so a column
no arm wrote still has a value and you may assert it:

```hek
expect plan[7] { plan_revenue: 0, plan_last_sold_at: none }
```

`expect nothing` is therefore refused here, as under `project` and `fold`: the question it would ask
is whether a key exists, and `expect no` asks it.

**A fold with no parameters has no key, so its row is written without one:**

```hek
expect global { store_units: 3, store_gross: 2600 }
```

Several columns in one expectation is the point: the `fold` action takes one `expect` per test, so
eight parameterless folds otherwise cost eight tests over a scenario one `fill` already sets up.

Its row is there from the start, since there is no key to mint, so `expect no global` is refused and
a log that moved none of its columns reads every one as its seed. `global` is claimed as a word
directly after `expect`, like `nothing` and `skipped`: a record type spelled `global` cannot be
asserted under the `fold` action, which is what keeps `expect Shape { a: 1 }` from being derivable
two ways.

Sealed columns and query-only columns behave as in sections 6 and 8: opened through the world's key
store, and readable here.

## 10. What a test cannot assert

- **State.** There is no `expect token == "x"`. What the arm did with a fold is already in the
  trace.
- **An arbitrary boolean.** There is no `assert <expr>`. A case is a table of inputs and outputs;
  once it can compute, it is a program with its own bugs.
- **Internals of the runtime.** Slice counts, the append condition, and how many times the fold ran
  are not outputs.

Everything the runner asserts goes through the same public API an embedder has.

## 11. Results

A test result is one of `passed`, `failed` (an expectation did not match, with what was expected and
what happened) or `errored` (the run itself raised, which is a defect in the program rather than a
mismatch). Those last two are separate on purpose: collapsing them makes a broken command look like a
wrong assertion.

Each test runs against a fresh interpreter with only its own `given` log, so tests cannot affect each
other and declaration order does not matter.

`hek check` does **not** fail on a failing test, because a test that fails is a program that parsed.
`hek test` does.

## 12. What to test

Write a case for each of these, because each is a rule the checker cannot see:

- the happy path of every command, asserting the whole event it appends;
- every refusal, with a log that reaches it, and every `invalid`, with a log that would otherwise
  succeed;
- the idempotent replay of every command that has one (`expect nothing`);
- each projector handler, including the absent-row case that distinguishes `patch` from `update`
  (`expect no Entity[key]`);
- each effect arm: the success trace, an absorbed retry (`respond url 503` then `200`), a terminal
  4xx (`expect fail "<message>"`), and a shredded subject (`erased ... ` then `expect skipped`).
