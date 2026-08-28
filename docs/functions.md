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

## A `fn` is pure

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
  construction, so "may a fold call a helper" needs no answer of its own.
- **A projector may call one.** A projector is a pure fold over the log, and a pure helper cannot
  break that.
- **Nothing needs journaling.** A `fn` produces no journal entry, so replay does not have to know it
  exists.

**Effect-local `fn` is deliberately not in this pass.** A helper declared inside an `effect`, allowed
to call out and `invoke`, is what a real port wanted for two effects that otherwise inline a 60-line
HTTP sequence into each of two arms. It is exactly the thing that makes erase-last interprocedural,
so it needs that analysis designed first. Worth knowing before that work starts: of the ten
effect-local helpers in that port, **four are already pure** and become module-scope `fn`s here, so
the case is six, not ten, and multi-path arms may shrink it further.

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
does not count as returning.

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

## What a `fn` makes possible, and is therefore not in the language

**`Timestamp.add_months(n)` is deliberately deferred.** A real port carries 33 lines of calendar
arithmetic to compute a warranty's expiry, implementing the semantics where 2026-01-31 plus one month
is 2026-02-28. Getting month-end clamping wrong is a real bug and the argument for a builtin is a
good one.

It is still the wrong place for it. **Month-end clamping is one calendar opinion among several**, and
a builtin commits the language to it forever: clamp to the last day, roll into the next month, or
refuse. Different jurisdictions and different contracts want different ones, and a language that
picks cannot be argued with.

Now that `fn` exists, that helper belongs in a shipped `lib/` where an application can read it,
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
