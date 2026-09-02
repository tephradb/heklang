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

`given`, `respond`, `erased`, `run`, `project`, `deliver`, `expect`, `no`, `nothing` and `skipped`
are claimed **only inside a test body**, so an entity field called `given` or a parameter called `no`
stays writable elsewhere. `test` is the only word this construct reserves outright.

## 2. `given` is a log, and it is literal

`given @event.path { field: value, ... }` appends one event, and several `given`s make a log in the
order written. **Every field must be given**, the same rule `emit` follows, and unlike `emit` a
`given` takes no bare-name shorthand: write `field: value` for every one. The values are ordinary
expressions: literals, `const`s, enum variants, `fn` calls, interpolation, containers.

A `fn` is how a suite gets a helper for near-identical events:

```hek
fn plan_sku(n: Int) -> String {
  return "STRESS-{n}"
}
```

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

Replies are a **queue per URL**, taken in order, so `respond url 503` then `respond url 200` is how a
test says the first attempt was absorbed. `erased` is the only way to write a shredded-key test,
since a test cannot call `erase` itself. The URL may be a `const`.

## 4. The action

Exactly one, and it decides which expectations are legal:

| Action | Runs |
| --- | --- |
| `run Command { name: value, ... }` | the command against the given log |
| `project Projector` | the projector over the whole given log |
| `deliver Effect` | the effect over the whole given log, following it as an `invoke` lengthens it |

**`deliver` drives, so there is no position to write.** An effect that fires on three of the given
events makes three invocations, and the expectations are the whole trace across all of them.

## 5. What `run` expects

| Expectation | Matches |
| --- | --- |
| `expect @path { field: value, ... }` | one appended event, in order |
| `expect nothing` | the command appended no events |
| `expect invalid("<message>")` | `Outcome::Invalid` |
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

**A row is matched on the listed columns only**, because a row is wide and most of its columns are
carried through untouched by the handler under test. That is the deliberate difference from an event,
which is matched on all of its fields.

`expect no` is what makes the difference between `patch` and `update` testable at all.

**A subject-bound value is matched on its content.** A test writes the plaintext and the two meet on
what the seal stores:

```hek
expect Shop[1] { shop_name: "Test Shop" }
```

The harness stores content as it was given, so the comparison works there. Against a host that really
encrypts it answers false, which is the right answer: a test asserting on content it cannot read
would be asserting on nothing.

## 7. What `deliver` expects

An effect's output is a **trace**: the ordered list of things it did to the world.

| Expectation | Matches |
| --- | --- |
| `expect http.<verb>("<url>")` | a call to that URL, body not checked |
| `expect http.<verb>("<url>", { <json> })` | the same, and the body matches the listed keys |
| `expect invoke Command { name: value, ... }` | an `invoke` with exactly those arguments |
| `expect erase(<subject>, "<id>")` | an erase, naming the subject field and the id |
| `expect log("<message>")` | one `log` line |
| `expect fail("<message>")` | an arm returned `fail` |
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

## 8. What a test cannot assert

- **State.** There is no `expect token == "x"`. What the arm did with a fold is already in the
  trace.
- **An arbitrary boolean.** There is no `assert <expr>`. A case is a table of inputs and outputs;
  once it can compute, it is a program with its own bugs.
- **Internals of the runtime.** Slice counts, the append condition, and how many times the fold ran
  are not outputs.

Everything the runner asserts goes through the same public API an embedder has.

## 9. Results

A test result is one of `passed`, `failed` (an expectation did not match, with what was expected and
what happened) or `errored` (the run itself raised, which is a defect in the program rather than a
mismatch). Those last two are separate on purpose: collapsing them makes a broken command look like a
wrong assertion.

Each test runs against a fresh interpreter with only its own `given` log, so tests cannot affect each
other and declaration order does not matter.

`hek check` does **not** fail on a failing test, because a test that fails is a program that parsed.
`hek test` does.

## 10. What to test

Write a case for each of these, because each is a rule the checker cannot see:

- the happy path of every command, asserting the whole event it appends;
- every refusal, with a log that reaches it, and every `invalid`, with a log that would otherwise
  succeed;
- the idempotent replay of every command that has one (`expect nothing`);
- each projector handler, including the absent-row case that distinguishes `patch` from `update`
  (`expect no Entity[key]`);
- each effect arm: the success trace, an absorbed retry (`respond url 503` then `200`), a terminal
  4xx (`expect fail(...)`), and a shredded subject (`erased ... ` then `expect skipped`).
