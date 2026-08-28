# Testing

A `test` is a declaration, like `command` or `projector`. It states a log, one thing to do to it, and
what should come out:

```
test "a sale on a deleted plan does not resurrect it" {
  given @plan.created { plan_id: 1, title: "Two-year cover", price: 19.99 }
  given @plan.deleted { plan_id: 1 }
  given @plan.sold { plan_id: 1, price: 19.99 }

  project Plans

  expect no Plan[1]
  expect Sales[1] { revenue: 19.99 }
}
```

This document is the contract. `tests/testing.rs` is the same set of rules as executable tests.
Change the doc, the tests and the code together.

The gap it closes is the largest one a real port found: heklang had tests for the language and
nothing an application author could write, which is the difference between a language and a language
you can ship an application in. The port it came from carries 2,671 lines of them in the system
heklang replaces, and could bring none of them across.

---

## 1. Shape

```
test "<name>" {
  <given>*
  <setup>*
  <action>
  <expect>*
}
```

Sections appear in that order and each is optional except the action. There is exactly one action.

**The name is a string, not an identifier.** It is a sentence, because it is what a failure report
reads out, and `creates_a_plan_for_an_onboarded_shop` is a worse sentence than the one it was made
from. Two tests may not share a name inside one program, so a report can name the test uniquely.

**Rejected: a separate file kind or a `suite` block.** The system heklang replaces puts its cases in
their own files that `load(...)` the events they need. heklang has no `load`, on purpose: a module is
a diagnostic label rather than a namespace (`docs/modules.md`), and every declaration is global. So a
file of tests is already an ordinary module, and a separate kind would need an import mechanism that
exists for nothing else.

### An expectation is spelled like the thing it asserts

`expect reject("sku_taken", "...")` beside the `return reject("sku_taken", "...")` it is about;
`expect invoke RecordSync { shop_id: 1 }` beside the `invoke RecordSync { shop_id: shop_id }`;
`expect log("...")` beside the `log("...")`. Nothing here is a second dialect for describing a call,
so the vocabulary is one line long: write the call.

`given` and the setup statements are directives rather than calls, and read as such
(`given @plan.sold { ... }`, `respond "url" 200`), because there is nothing in a program they mirror.
`expect skipped` is the one expectation with no call behind it, for the same reason: rule 12's
terminal skip is something that happens to an arm rather than something the arm did.

Named fields are a brace block everywhere in heklang, so a command's arguments are one too, in the
action and in `expect invoke` alike. Parentheses stay positional.

### `test` is the only word this construct reserves

`given`, `respond`, `erased`, `run`, `project`, `deliver`, `expect`, `no`, `nothing` and `skipped`
are claimed **only inside a test body**. A construct that only tests use should cost no name anywhere
else, and an entity field called `given` or a command parameter called `no` stays writable. This is
the same device the soft builtin names use (`docs/effects.md` rule 11).


## 2. `given` is a log, and it is literal

`given @event.path { field: value, ... }` appends one event. Several `given`s make a log, in the
order written. Every field must be given, the same rule `emit` and `put` follow, and the values are
ordinary expressions: literals, `const`s, enum variants, `fn` calls, interpolation, containers.

**A `fn` is how a test gets a helper.** The suite this construct was designed against leans on
helper functions to build near-identical events, and `fn` already covers it:

```
fn plan_sku(n: Int) -> String {
  return "STRESS-{n}"
}

test "..." {
  given @plan.created { plan_id: 1, title: "Plan", price: 19.99, sku: plan_sku(1) }
  ...
}
```

**Rejected: running a command to build the log.** `given run PlaceOrder(...)` reads well and is
wrong: a fixture built by the thing under test means one broken command fails every test that used it
as scenery, and the report names the wrong test. Raw events keep a failure local to the case that
found it.

**Rejected: reaching for state directly.** There is no `given state total = 5`. A fold's state is
derived from the log, and a test that could set it would be asserting about a value the runtime never
produces.

## 3. Setup

| Statement | Meaning |
| --- | --- |
| `respond "<url>" <status>` | queue one HTTP reply for that URL |
| `respond "<url>" <status> { <json> }` | the same, with a body |
| `respond "<url>" timeout` | a transport failure, which rule 5 of `docs/effects.md` absorbs and retries |
| `erased <subject> "<id>"` | that subject's key is already destroyed |

Replies are a queue per URL, taken in order, so scripting a 503 then a 200 is how a test says "the
first attempt was absorbed". `erased` is the only way to write a shredded-key test, since a test
cannot call `erase` itself.

## 4. The action

Exactly one, and it decides which expectations are legal:

| Action | Runs |
| --- | --- |
| `run Command { name: value, ... }` | the command against the given log |
| `project Projector` | the projector over the whole given log |
| `deliver Effect` | the effect over the whole given log, following it as an `invoke` lengthens it |

**`deliver` drives, so there is no position to write.** A test says which effect, not which event: an
effect that fires on three of the given events makes three invocations, and the expectations are the
whole trace across all of them. That is the shape the ported suite needs, and it removes a number
from the surface that an author would otherwise have to keep in step with the `given` list.

## 5. What `run` expects

| Expectation | Matches |
| --- | --- |
| `expect @path { field: value, ... }` | one appended event, in order |
| `expect nothing` | the command appended no events |
| `expect invalid("<message>")` | `Outcome::Invalid` |
| `expect reject("<code>", "<message>")` | `Outcome::Reject` |

The appended events must match the `expect @path` lines **one for one and in order**. An event the
test did not write is a failure, which is what makes "this command appends exactly this" checkable
rather than "this command appends at least this".

`expect nothing` is a real assertion rather than an empty test: "recreating the same plan id is a
no-op" is a case the ported suite states explicitly, and writing no `expect` at all would say the
same thing by accident.

## 6. What `project` expects

| Expectation | Matches |
| --- | --- |
| `expect Entity[key] { field: value, ... }` | the row exists and the listed columns match |
| `expect no Entity[key]` | there is no such row |

**A row is matched on the listed columns only, and an event is matched on all of them.** The
difference is not an inconsistency. An event is small and every field is part of what the command
decided, so listing all of them is the assertion. A row is wide, most of its columns are carried
through untouched by the handler under test, and a test that had to restate all of them would change
every time an unrelated column was added.

`expect no` is what makes `update` (rule 5 of `docs/projectors.md`) testable at all, since the
difference between it and `patch` is precisely whether a row is there.

## 7. What `deliver` expects

An effect's output is a **trace**: the ordered list of things it did to the world.

| Expectation | Matches |
| --- | --- |
| `expect http.<verb>("<url>")` | a call to that URL, body not checked |
| `expect http.<verb>("<url>", { <json> })` | the same, and the body matches the listed keys |
| `expect invoke Command { name: value, ... }` | an `invoke` of that command with exactly those arguments |
| `expect erase(<subject>, "<id>")` | rule 9's erase, naming the subject field and the id |
| `expect log("<message>")` | one `log` line |
| `expect fail("<message>")` | an arm returned `fail` |
| `expect skipped` | an arm hit a shredded key, rule 12's terminal skip |
| `expect nothing` | the effect did nothing observable |

Ordered and complete, like `run`'s: the trace and the expectations line up one for one.

**A body is matched on the keys the test writes.** A request body is often a large generated
document, and the assertion an author wants is "it carried this id", not a copy of the payload.
`invoke` arguments are matched exactly, because a command's arguments **are** its whole input, so
there is nothing there to be uninterested in.

**`log` is in the trace.** It is not journaled (rule 10 of `docs/effects.md`), so it runs again on a
replay, and being able to assert on the line is how a test pins the branch an arm took when the arm's
only other output is a decision not to call out.

## 8. What a test cannot assert

- **State.** There is no `expect state token == "x"`. A fold's state is an intermediate, and the
  reason to have it is what the arm did with it, which is already in the trace.
- **Anything through an arbitrary boolean.** There is no `assert <expr>`. A case is a table of
  inputs and outputs; once it can compute, it is a program with its own bugs, and the thing that
  fails is no longer legible in a report.
- **Internals of the runtime.** Slice counts, the append condition, and how many times the fold ran
  are not outputs. What the append condition guarantees is tested by the language's own suite, not by
  an application's.

Everything the runner asserts goes through the same public API an embedder has, which is what keeps
this list honest: a test cannot see anything a program cannot.

## 9. Running

`run_tests(&program)` returns one result per test, in declaration order, each carrying the test's
name, its module and its outcome. A result is one of:

| Outcome | Meaning |
| --- | --- |
| passed | every expectation matched |
| failed | an expectation did not match, with what was expected and what happened |
| errored | the run itself raised, which is a defect in the program rather than a mismatch |

**Failed and errored are separate.** A mismatch is the test doing its job; an error is the program
being unable to run at all, and collapsing them makes a broken command look like a wrong assertion.

A test runs against a fresh interpreter with only its own `given` log, so tests cannot affect each
other and the order they are declared in does not matter, which is the same property
`docs/modules.md` claims for every other declaration.
