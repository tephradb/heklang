# Commands

```
command PlaceOrder(order_id: Uuid, customer_id: Int, email: String, total: Money(2)) {
  guard @order.placed(order_id), @order.cancelled(order_id)

  state open_orders: Int = fold 0
    on @order.placed(customer_id) => open_orders + 1
    on @order.cancelled(customer_id) => open_orders - 1

  if open_orders >= 10 {
    return reject("too_many_open", "too many open orders")
  }

  emit @order.placed { order_id, customer_id, email, total }
}
```

A command is the only thing that appends. It reads history by folding it, decides, and emits; it
cannot call out, cannot write a read model, and cannot decrypt.

## `state` is a read declaration, not a binding

This is the one thing about a command worth understanding before anything else, and it is why the
keyword is not `let`.

A `state` declares a **slice** of the log: an event type plus the filters that narrow it. The slice
is what the runtime appends against:

```rust
pub struct AppendCondition {
    pub after: u64,
    pub slices: Vec<Predicate>,
}

pub struct Predicate {
    pub event: EventPath,
    pub filters: Vec<(Ident, Value)>,
}
```

Every run returns one of those beside its outcome, so the read set is observable to the host. That is
the Dynamic Consistency Boundary: **what you folded is what you conflict on.** If another writer
appends into one of those slices after position `after`, this append is rejected and retried against
the new log.

**A slice comes back resolved, not as a pointer.** `@order.placed(customer_id)` leaves as
`@order.placed` narrowed to `customer_id = 7`, because a filter is an expression the command
evaluated and "which slice" means nothing to a host that did not compile the program. Resolving is
what makes the condition answerable: it is the same shape a tag query has, so the host that appends
against it can also index on it.

The filters are sorted by field name, so one slice is one predicate however it was written: two
filters in either order narrow the same events and have no business comparing unequal.

A `let` compiles to an assignment in the prologue. It produces no slice and contributes nothing to
the condition. So the two keywords are not two spellings of one idea:

| | `state x = fold ...` | `let x = ...` |
| --- | --- | --- |
| declares a slice | **yes**, and it is in the append condition | no |
| re-runs on a retry | yes, re-folded against the new log | yes, but from the same inputs |
| may read a clock | no | yes, the pinned one |
| may call out | no | no in a command; yes in an effect arm |
| position | before the first statement | before the first statement, or anywhere in the body |

Naming both `let` would leave the thing that decides whether concurrent appends conflict looking
exactly like the thing that shortens an expression. The cost of keeping them apart is one word.

**A `guard` also declares slices, and usually declares folds too.** `guard <Name> { args }` names a
proposition whose own folds join this command's condition; the raw `guard @order.placed(id)` form is
a `state` that binds nothing, and is the same call:

```rust
pub fn guard(&mut self, event: EventPath, filters: Vec<Filter>) -> SliceId {
    self.slice(event, filters, Vec::new(), Vec::new())
}
```

`docs/guards.md` has both, and "`guard` names a proposition" below has when to reach for which.

**Rejected: `let name = fold ...`.** It is one keyword instead of two, and it hides the append
condition inside the same syntax as arithmetic. A reader scanning for what a command conflicts on
would have to read every binding in the prologue and know which right-hand sides were folds.

**Rejected: an explicit `reads @order.placed(id)` list.** It separates the declaration of the read
set from the value that comes out of it, so the two can drift: a fold could name a slice the list
forgot. Deriving the condition from the folds themselves makes the drift unrepresentable.

## Execution order

Fixed, and worth knowing because it explains every scoping rule below:

1. **parameters** are bound into the frame, each coerced to its declared type;
2. **hoisted `let`s** run, in order (which is those that do not read a `state`, see below);
3. **filters** are evaluated once, so a filter may name a parameter or a hoisted `let`;
4. **`state` seeds** are evaluated and coerced;
5. `after` is taken: the log length *before* the fold;
6. **the fold** runs, one pass over the log, applying every matching slice per record in declaration
   order;
7. **the guards' decisions** run, in the order the guards are written (`docs/guards.md`);
8. **the body** runs, appending into an emit buffer;
9. the outcome and the `AppendCondition` are returned together.

One pass, not one per `state`: ten folds over a million events read the log once, and a guard adds
folds rather than reads.

Step 7 is why a `guard` is a declaration rather than a statement. A guard's folds happen at step 6
with every other, its decision at step 7, and neither straddles the two.

**A `let` is hoisted only when it can be.** Step 2 runs before the fold, so a `let` whose initialiser
reads a `state` has nothing to read, and that one stays an ordinary body statement at step 8 instead.
It applies transitively: a `let` reading such a `let` stays with it.

```
state open: Int = fold 0
  on @order.placed(customer_id) => open + 1
state shut: Int = fold 0
  on @order.cancelled(customer_id) => shut + 1

let live = open - shut          // step 8, after the fold, because it reads a state
```

The two placements are indistinguishable except to a filter, because nothing between step 2 and step
8 changes what the prologue could have read. So the rule costs an author nothing to not know, which
is the point: writing `let live = open - shut` under the folds means what it looks like it means.

**A seed may not read a `state` either**, and that one is rejected rather than deferred. Step 4 runs
every seed before step 6 runs any fold, so `fold open` would read `open`'s own seed and never what it
folds to:

```
state open: Int = fold 0
  on @order.placed(customer_id) => open + 1
state seen: Int = fold open      // rejected: `seen` is seeded from `open`
```

There is nothing to defer it to, because a seed has to exist before the fold it seeds. And unlike the
`let`, this one used to **check clean and answer with the wrong number**: three matching events left
`open` at 3 and `seen` seeded at 0, which is a plausible answer and a silent one. The way out is the
`let` above, which the error names.

What a filter cannot do is name one of them, and that **is** rejected for the same reason, since it
asks the fold for the value that decides what the fold reads:

> this filter on `customer_id` is folded from a `state`; filters are evaluated once, before the fold,
> so they can name a parameter or a `let` above the declarations

Step 3 is why the prologue exists at all, and why a `let` a filter names must be **above** it. The
error says so rather than saying "not in scope":

> `customer` is defined below the declarations; `guard` and `state` run before the body, so they can
> only use names bound above them; move that `let` up

The definition site is a related location rather than a position written into the sentence, so an
editor can go to it. `docs/diagnostics.md` section 9 has why.

Step 5 is subtle and load-bearing: `after` is taken before the fold, so the condition means "nothing
new in these slices since the position I started reading at".

## `emit` writes an event whole

Every field the event declares is given, each of them once. There is no partial event and no
default: an event is a fact, and a fact with a hole in it is a different fact.

```
emit @order.cancelled { order_id }
```

> `emit @order.cancelled` needs `customer_id`; an event is written whole

This is the same rule `given` holds to (`docs/testing.md` rule 2), a record literal holds to, and an
`invoke`'s arguments hold to, and it is checked in the same place: at the write, against the
declaration. It was the runtime that held it for `emit` and for `put`, which meant a command that
omitted a field checked clean and failed at the append, so a branch no test reached shipped broken.
`docs/projectors.md` rule 5 has the `put` half.

The shorthand is `{ order_id }` for `{ order_id: order_id }`, so writing every field is usually
writing every name.

## Three outcomes, and the condition comes back with all of them

`return` with no value, or falling off the end, is `Ok` with whatever was emitted.

| Outcome | Meaning |
| --- | --- |
| `Ok(events)` | committed; may be empty |
| `Invalid(message)` | the input was malformed |
| `Reject { code, message }` | the command refused on state grounds |

`docs/effects.md` rule 6 has the full argument for why these three and not hekla's six; the short
version is that `Conflict` and `Unavailable` are the runtime's to retry and have no variant here at
all.

**The `AppendCondition` is returned even for `Invalid` and `Reject`.** A refusal still read the log,
and a host that wants to cache or trace the decision needs to know what it depended on. It is
computed after the body rather than as part of the success path for exactly that reason.

**`invalid` is about the request; `reject` is about the world.** A blank address is `invalid` whoever
sends it and whenever. A blocked customer is `reject`, because the same request would have succeeded
yesterday. The distinction matters to a caller deciding whether to fix the input or to give up, which
is why `reject` carries a code and `invalid` does not: there is nothing to branch on when the answer
is "you sent nonsense".

**A command may return an outcome it did not spell.** `return reject("code", "why")` is unchanged,
and beside it `return <expression>` takes anything of type `Outcome`, which is what a `fn` declared
`-> Outcome?` produces. That is how two commands share one ladder without sharing a body:

```
let refusal = ladder(subscribed, taken, limit)
if refusal.is_some() {
  return refusal
}
```

`docs/functions.md` has the rule and why the type is spellable in a `fn` signature and nowhere else.

## `guard` names a proposition

The commoner shape by far, and it has its own document: `docs/guards.md`.

```
guard CourseIsDefined { course }
guard StudentIsRegistered { student }
```

Each names a declared proposition about the log, folds it, and refuses if it does not hold. The
guards' slices join this command's condition, so the boundary is the union of what it guards, and
the order on the page is the precedence of the refusals.

**The raw form stays, and it is the rare one.** `guard @order.placed(order_id)` adds slices to the
boundary and binds nothing. A `state` fold already contributes its slice, so a command that folds
`@order.placed(order_id)` to decide whether an order exists has *already* declared that slice as its
conflict boundary, and writing `guard @order.placed(order_id)` beside it adds a second, identical
slice and no safety. It earns its place only when the decision needs a slice in the boundary that
nothing folds: the one counter-example in either tree guards `@order.placed(order_id)` while folding
on other fields.

So: name a guard when the proposition has a refusal, reach for `state` when the value is this
command's own, and write raw slices when you can say which slice they add that no fold covers.

## What a command may not do

- **No `http.*`, no `invoke`.** Only an effect calls out, because only an effect journals the call.
  A command that needs another command's work emits, and an effect reacts.
- **No `reveal`, no `erase`.** Only an effect crosses the decrypt boundary. A command decides from
  state without reaching personal data, which is what keeps rule 12's key handling in one place. A
  command may still *fold* sealed content and emit it into a field sealed under the same subject:
  moving it is not reading it (`docs/effects.md` rule 12).
- **No `put` / `patch` / `update` / `delete`.** A read model is a projector's output.
- **No `fail`.** That is an effect's terminal outcome; a command returns `invalid` or `reject`.

Each of those has a message naming the rule at the point of violation rather than naming a category.

## Inside a fold

A fold arm is `on @path(filters) [{ fields }] => expression`, and the expression may name the state
variable itself, which is what makes it a fold rather than a scan.

A filter is `field` when a binding of that name is in scope, or `field: expression` for anything
else. The shorthand is the common case: `@order.placed(customer_id)` beside a parameter called
`customer_id`.

A fold arm may not read the clock or call out, because **a fold is not journaled**: every attempt
re-folds and gets the same answer, which is only true if it cannot observe anything but the log
(`docs/effects.md` rule 3). It may call a module `fn`, which is pure by construction, and may not
call an effect-local one, which is not (`docs/functions.md`).

`docs/effects.md` rule 12 covers the rest: one variable holds one subject, a plain seed is fine and a
plain arm is not, and a transformed arm drops the seal.

## Related

- `docs/effects.md`: arms, `invoke`, the journal, and rules 3, 6 and 12, all of which a command
  shares or is deliberately excluded from.
- `docs/projectors.md`: the other reader of the log, and the only writer of a read model.
- `docs/declarations.md`: the separate name spaces, and the pass order that lets a command name an
  event declared below it.
- `docs/testing.md`: `run`, which asserts a command's outcome and its emitted events.
- `docs/host.md`: who answers the read, who enforces the condition, and what a conflict arrives as.
