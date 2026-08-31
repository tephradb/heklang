# Commands

```
refusal TooManyOpen "too many open orders"

command PlaceOrder(order_id: Uuid, customer_id: Int, email: String, total: Money(2)) {
  guard @order.placed(order_id), @order.cancelled(order_id)

  state open_orders: Int = fold 0
    on @order.placed(customer_id) => open_orders + 1
    on @order.cancelled(customer_id) => open_orders - 1

  if open_orders >= 10 {
    return reject TooManyOpen
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

A `let` compiles to an assignment. It produces no slice and contributes nothing to the condition. So
the two keywords are not two spellings of one idea:

| | `state x = fold ...` | `let x = ...` |
| --- | --- | --- |
| declares a slice | **yes**, and it is in the append condition | no |
| re-runs on a retry | yes, re-folded against the new log | yes, but from the same inputs |
| may read a clock | no | yes, the pinned one |
| may call out | no | no in a command; yes in an effect arm |
| position | anywhere in the body, and a run of them is one read | anywhere in the body |

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
would have to read every binding in the body and know which right-hand sides were folds.

**Rejected: an explicit `reads @order.placed(id)` list.** It separates the declaration of the read
set from the value that comes out of it, so the two can drift: a fold could name a slice the list
forgot. Deriving the condition from the folds themselves makes the drift unrepresentable.

## Execution order

A run of `state` and `guard` declarations is a **stage**: one read of the log, folded in one pass.
A statement written below a stage's declarations closes it, so the next run is a stage of its own.
A command is a sequence of stages, and the order is fixed:

1. **parameters** are bound into the frame, each coerced to its declared type;
2. then, for each **stage** in the order written:
   1. the statements above its declarations run, and one that returns is the command's outcome;
   2. its **filters** are evaluated once, so a filter may name a parameter, a `let` above it, or a
      `state` an *earlier* stage folded;
   3. its **`state` seeds** are evaluated and coerced;
   4. `after` is taken, on the first stage that reads, and every later stage reads to it;
   5. its **fold** runs, one pass over the log, applying every matching slice per record in
      declaration order;
   6. the statements below its declarations run, the guards' decisions first;
3. the outcome and the `AppendCondition` are returned together, the condition naming what the
   stages that actually ran read.

One pass per stage, not one per `state`: ten folds in one run read the log once, and a guard adds
folds rather than reads. A command whose declarations are all at the top -- which is most of them --
is one stage and one read.

**`after` is pinned once and every stage reads to it.** This is the rule that keeps staging off the
host's contract. Were each stage to take its own head, the append would have to assert two things at
two positions, and one `AppendCondition` cannot say that. Reading every stage to the head the first
one took makes the stages one consistent view of the log, so the condition stays a single `after`
and a flat slice list. It is rule 11 applied to the log rather than to the clock, and `docs/host.md`
section 5 has what it costs.

**A stage is unconditional.** A `state` or a `guard` may not be written inside an `if` or a `for`,
because a read that may or may not happen has nothing to say in an append condition:

> `state` and `guard` must come before the first statement

Step 2.6 is why a `guard` is a declaration rather than a statement. A guard's folds happen at its
stage's step 2.5 with every other in that run, its decision at 2.6, and neither straddles the two.

**A `let` runs where it is written.** One above a declaration run is in that stage's first half, so
a filter can name it; one below the declarations is in the second half, so it can read what they
folded.

```
state open: Int = fold 0
  on @order.placed(customer_id) => open + 1
state shut: Int = fold 0
  on @order.cancelled(customer_id) => shut + 1

let live = open - shut          // below the declarations, so after the fold
```

**A seed may not read a `state` in its own stage**, because every seed in a run is evaluated before
that run folds, so `fold open` would read `open`'s own seed and never what it folds to:

```
state open: Int = fold 0
  on @order.placed(customer_id) => open + 1
state seen: Int = fold open      // rejected: `seen` is seeded from `open`
```

There is nothing to defer it to, because a seed has to exist before the fold it seeds. And this one
used to **check clean and answer with the wrong number**: three matching events left `open` at 3 and
`seen` seeded at 0, which is a plausible answer and a silent one.

The way out is a statement between them, which is also the general answer to everything in this
section: it closes the first stage, so the second reads a log the first has already folded.

```
state open: Int = fold 0
  on @order.placed(customer_id) => open + 1

let so_far = open                // closes the stage above

state seen: Int = fold so_far    // a second stage, reading what the first folded
  on @order.cancelled(customer_id) => seen + 1
```

The same rule and the same escape apply to a filter, which is the case that pays for staging:

```
state open: Int = fold 0
  on @order.placed(customer_id) => open + 1

let who = open

state blocked: Bool = fold false
  on @customer.blocked(customer_id: who) => true
```

Written without the `let` between them, both folds are one stage and the filter asks that stage for
a value it has not produced, which is refused:

> this filter on `customer_id` is folded from a `state` beside it; a stage's filters are evaluated
> once, before it folds, so they can name a parameter, a `let`, or a `state` an earlier declaration
> run has already folded

`after` is subtle and load-bearing: it is taken before any fold, so the condition means "nothing new
in these slices since the position I started reading at".

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

**A field with a `@max` is checked against where its value came from**, when that is a `state` folded
off another event field: emitting into a field bounded tighter than the one folded into it is
`max-tightening`, and `docs/projectors.md` has the invariant. An over-length *value* is still
`Outcome::Invalid` at run time, which is a different thing: one is a bad input and this is two
declarations disagreeing.

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

**A decision that read nothing comes back with an empty condition**, and `after` at zero rather than
at a head it never asked for. That is every command with no `state`, and now also any path that
returns above the first declaration run. It is not a hole: `AppendCondition::conflicts` is false for
every record when there are no slices, which is correct, because a decision that depended on nothing
cannot be invalidated by anything. `after` is meaningful only alongside a non-empty `slices`.

**So an `emit` on a path that returns before any declaration is appended unconditionally.** A
command may do that deliberately -- answering from its arguments and appending without consulting
the world -- and the `emit` sits visibly above the `guard` lines when it does. What it does not get
is the protection of the guards below it, because they did not run.

**An emitted event is not in the log a later stage folds.** Emits go to a buffer and the append
happens once, at the end, so a `state` in a second stage counting `.placed` counts everything
but the one this command is about to write. It cannot self-conflict either, since the condition is
checked against the records already there.

**`invalid` is about the request; `reject` is about the world.** A blank address is `invalid` whoever
sends it and whenever. A blocked customer is `reject`, because the same request would have succeeded
yesterday. The distinction matters to a caller deciding whether to fix the input or to give up, which
is why `reject` carries a code and `invalid` does not: there is nothing to branch on when the answer
is "you sent nonsense".

**A command may return an outcome it did not spell.** `return reject <Name>` is unchanged,
and beside it `return <expression>` takes anything of type `Outcome`, which is what a `fn` declared
`-> Outcome?` produces. That is how two commands share one ladder without sharing a body:

```
let decision = ladder(subscribed, taken, limit)
if decision.is_some() {
  return decision
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

A fold arm is `on @path(filters) [{ destructure }] => expression`, and the expression may name the
state variable itself, which is what makes it a fold rather than a scan.

**The parens and the braces read from opposite sides.** Both sit between the path and the `=>`, and
both hold bare field names, which is the whole reason they get swapped:

```
state lifetime_spend: Money(2) = fold 0
  on @order.placed(customer_id) { total } => lifetime_spend + total
```

| | says | evaluated | reads |
| --- | --- | --- | --- |
| `(customer_id)` | which events this arm matches | once, before the fold | the command's scope |
| `{ total }` | which of its fields the arm may name | once per matching event | the event |

So `customer_id` is the command's parameter and `total` is the event's field. Only the filter
reaches the append condition, because only the filter narrows the slice: the `Predicate` above
carries `filters` and has nowhere to put a destructure. A destructure opens events the filter has
already chosen.

**A filter is `field` when a binding of that name is in scope, and `field: expression` otherwise.**
The shorthand is the common case, `@order.placed(customer_id)` beside a parameter of the same name.
The long form is for when the two sides do not share a name:

```
on @customer.blocked(customer_id: customer) => true
```

Because it runs once and before its own stage folds, a filter may not name a `state` declared beside
it, and a `let` it does name has to be above the declarations of its stage. It may name a `state` an
earlier stage folded. "Execution order" above has the errors and the escape.

**A destructure binds the event's own fields under their own names.** It is optional, and an arm
needing only the fact that an event happened leaves it off:

```
on @order.placed(customer_id) => open_orders + 1
```

There is no renaming, so `{ total: amount }` does not parse. A binding always carries the field's
name, which is what lets a reader who knows the event know what the arm has in scope without
reading back up to the `on`; a name for anything else is a `let` below the declarations rather than
an alias here. Where a parameter shares the name, the destructure shadows it for that one arm, which
is what an arm folding `total` wants and not a collision.

The name has to be a field that event declares, so an envelope is not reachable this way:

> @order.placed has no field `position`

A command has no envelope to reach for. An effect's fold sits inside a handler, so that handler's
`as` binding is in scope in a filter and in an arm, but it is the *triggering* event and does not
move as the fold runs (`docs/effects.md`).

**The scope is one arm.** Folding two event types binds two sets of names that never meet, each
checked against its own declaration:

```
state items: Map(Uuid, Item) = fold Map.empty
  on @item.listed(seller_id) { item_id, item } => items.set(item_id, item)
  on @item.delisted(seller_id) { item_id } => items.remove(item_id)
```

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
