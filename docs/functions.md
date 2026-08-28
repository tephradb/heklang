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
pure helper over a response could be written everywhere except in its own signature. The section
above counts four of a port's ten effect-local helpers as "already pure, and therefore module-scope
`fn`s here". Two of those four take a `Response`, so the count that argued effect-local `fn` down
from ten to six was assuming this worked.

**Rejected: teaching the general type parser.** One arm on `type_ref` reaches every position at once,
including the event field, and an accidental rejection elsewhere (an entity column has no zero value
for a `Response`, so it fails for an unrelated reason) is not the same as a rule.

**Rejected: passing `.body` at the call site.** The port's two helpers read only `body`, so both
could take a `Json` instead, and it needs no language change. It does not generalise: `.status` is
read at twelve sites, five of them the same unauthorized check, and the first helper that wants one
is back here.

`Outcome`, which `invoke` returns, has the same shape and is deliberately left unspellable. Nothing
in either tree names it, and the rule above would extend to it unchanged the day something does.

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
