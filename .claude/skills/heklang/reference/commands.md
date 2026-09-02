# Commands, guards and refusals

```hek
refusal TooManyOpen "too many open orders"

guard UnderOpenOrderLimit(customer_id: Int) {
  state open: Int = fold 0
    on @order.placed(customer_id) => open + 1
    on @order.cancelled(customer_id) => open - 1

  if open >= 10 {
    return reject TooManyOpen
  }
}

command PlaceOrder(order_id: Uuid, customer_id: Int, email: String, total: Money(2)) {
  guard UnderOpenOrderLimit { customer_id }

  emit @order.placed { order_id, customer_id, email, total }
}
```

A command is the only thing that appends. It reads history by folding it, decides, and emits. It
cannot call out, cannot write a read model, and cannot decrypt.

## 1. `state` is a read declaration, not a binding

A `state` declares a **slice** of the log: an event type plus the filters that narrow it. The slices
a command folded **are** the condition its append is checked against. If another writer appends into
one of those slices after the position the command started reading at, the append is rejected and
the whole command retries against the new log. That is the Dynamic Consistency Boundary: what you
read is what you conflict on.

| | `state x = fold ...` | `let x = ...` |
| --- | --- | --- |
| declares a slice in the append condition | **yes** | no |
| re-runs on a retry | yes, re-folded against the new log | yes, from the same inputs |
| may read the clock | no | yes, the pinned one |
| may call out | no | no in a command |

So: use `state` for anything folded out of the log, and `let` for everything else. Never reach for
`let` to shorten a fold, and never add a `state` you do not read, because it widens the boundary.

A slice leaves resolved rather than as a pointer: `@order.placed(customer_id)` becomes
`@order.placed` narrowed to `customer_id = 7`. Filters are sorted by field name, so one slice is one
predicate however it was written.

## 2. Stages: a run of declarations is one read

A run of adjacent `state` and `guard` declarations is a **stage**, folded in one pass over the log.
A statement written below a stage's declarations closes it, so the next run is a stage of its own.
Ten folds written together read the log once.

Order within a command:

1. parameters are bound, each coerced to its declared type;
2. for each stage in the order written:
   1. the statements above its declarations run, and one that returns is the command's outcome,
   2. its **filters** are evaluated once, before the fold,
   3. its **`state` seeds** are evaluated,
   4. the log head is pinned on the first stage that reads, and every later stage reads to it,
   5. the fold runs, one pass, applying every matching slice per record in declaration order,
   6. the statements below the declarations run, the guards' decisions first;
3. the outcome and the append condition are returned together.

**A stage is unconditional.** A `state` or a `guard` may not be written inside an `if` or a `for`.

**A seed or a filter may not name a `state` in its own stage**, because seeds and filters are
evaluated before that stage folds. The way out is a statement between them, which closes the first
stage:

```hek
state open: Int = fold 0
  on @order.placed(customer_id) => open + 1

let who = open                   // closes the stage above

state blocked: Bool = fold false
  on @customer.blocked(customer_id: who) => true
```

**A `let` runs where it is written.** One above a stage's declarations is in that stage's first half,
so a filter may name it; one below is in the second half, so it can read what the stage folded.

## 3. Inside a fold

```hek
state lifetime_spend: Money(2) = fold 0
  on @order.placed(customer_id) { total } => lifetime_spend + total
```

The arm is `on @path(filters) [{ destructure }] => expression`, and the expression may name the state
variable itself. The parens and the braces both hold bare field names and read from opposite sides:

| | says | evaluated | reads |
| --- | --- | --- | --- |
| `(customer_id)` | which events this arm matches | once, before the fold | the command's scope |
| `{ total }` | which of its fields the arm may name | once per matching event | the event |

Only the filter reaches the append condition, because only the filter narrows the slice.

**A filter is `field` when a binding of that name is in scope, and `field: expression` otherwise**
(`on @customer.blocked(customer_id: customer) => true`).

**A destructure binds the event's own fields under their own names.** There is no renaming
(`{ total: amount }` does not parse), and the name has to be a field that event declares, so an
envelope is not reachable this way. Where a parameter shares the name, the destructure shadows it
for that one arm.

**The scope is one arm.** Folding two event types binds two sets of names that never meet:

```hek
state items: Map(Uuid, Item) = fold Map.empty
  on @item.listed(seller_id) { item_id, item } => items.set(item_id, item)
  on @item.delisted(seller_id) { item_id } => items.remove(item_id)
```

**A fold arm may not read the clock, call out, invoke, or decrypt**, because a fold is not journaled:
every attempt re-folds and must get the same answer. It may call a module `fn`, which is pure, and
may not call an effect-local one.

**A seed may be plain while an arm is sealed**, and that is the ordinary shape for a credential
(`state token: String? = fold none` with two arms folding a `@subject(...)` field). What is rejected
is a mix: an arm folding a plain value into a variable another arm makes sealed, and two arms folding
under two different subjects.

## 4. `emit`

```hek
emit @order.cancelled { order_id, customer_id, refund }
```

**Every field the event declares is given, each once.** There is no partial event and no default: an
event is a fact, and a fact with a hole in it is a different fact. `{ order_id }` is shorthand for
`{ order_id: order_id }`.

An emitted event is **not** in the log a later stage folds: emits go to a buffer and the append
happens once, at the end. A command cannot self-conflict.

## 5. The three outcomes

| Outcome | Meaning |
| --- | --- |
| `Ok(events)` | committed; may be empty |
| `Invalid(message)` | the input was malformed |
| `Reject { code, message }` | the command refused on state grounds |

`return` with no value, or falling off the end, is `Ok` with whatever was emitted. There is no
`Conflict` and no `Unavailable`: those are the runtime's to retry and have no variant here.

**`invalid` is about the request; `reject` is about the world.** A blank address is `invalid`
whoever sends it and whenever. A blocked customer is `reject`, because the same request would have
succeeded yesterday. `reject` carries a code to branch on; `invalid` does not, because there is
nothing to branch on when the answer is "you sent nonsense".

**The append condition comes back with all three outcomes**, and a decision that read nothing comes
back with an empty one. So an `emit` on a path that returns above the first declaration run is
appended unconditionally, and does not get the protection of the guards below it.

**A command may return an outcome it did not spell**: `return <expression>` takes anything of type
`Outcome`, which is what a `fn` declared `-> Outcome?` produces.

## 6. Guards

```hek
guard <Name>(<name>: <Type>, ...) {
  <statement>*
  <state | guard>*
  <statement>*
}

guard <Name> { <name>: <value>, ... }         // at the call site
guard @order.placed(order_id), @order.cancelled(order_id)   // raw slices, binds nothing
```

A guard is a command body with no `emit` and one declaration run. Parameters in, folds, and a
decision; **falling off the end means the proposition holds**. There is no return type. Parens
declare and braces use, the same split `command Foo(...)` and `invoke Foo { ... }` have.

**A guard names a proposition, not an entity**: `CourseIsDefined`, `ShopIsConnected`,
`UnderOpenOrderLimit`. Not `Course`, `Shop`, `Order`. That is what keeps a command's boundary the
union of several small propositions instead of one aggregate.

**The order on the page is the precedence.** Guards run in the order written, each before the
statements below them, and the first that refuses is the command's outcome.

**A statement above a guard decides before it and reads nothing to do so.** That is where request
validation belongs, and it matters: a command that guards the log before validating its input answers
the world's question first, which turns a refusal into an existence oracle for a caller probing ids.

**Guards compose.** A guard may guard another, to any depth, and every slice reached joins the
command's condition. `hek check --boundaries` prints the transitive closure, which is the only place
a fold three levels down shows up.

**A guard is copied, not called.** It is spliced into whatever names it before the interpreter sees
either, so composition costs nothing at run time and there is no `Guard` in the append condition, the
journal or a trace.

### What a guard may not do

| | why |
| --- | --- |
| `emit` | a guard decides whether a command may run; the command appends |
| `put`, `patch`, `update`, `delete` | a guard reads |
| `invoke`, `http.*`, `reveal`, `erase`, `fail`, `log` | an effect's, and a guard runs inside a command |
| `now()` | a guard decides from the log; take the moment as a parameter |
| a bare `return`, or `return <value>` | only `return reject <Name>` and `return invalid(...)` |
| fold nothing | a decision made from arguments alone is a `fn` |
| read the log twice | a guard is one read: its declarations come before its first statement |
| bind a value back to its caller | its states are its own |
| name itself, directly or through another | a guard is copied, so a cycle has no end |
| be named twice on the same arguments in one body | the second decides what the first already did |

An early exit meaning "this holds" is spelled by not writing one: `if !defined { return reject ... }`
rather than `if defined { return }`.

### When a check is not a guard

**An idempotent no-op is not a guard.** The test is not "is this a no-op" but **"can this refusal be
reached by a request the command would have answered `ok`?"** If a replay has to answer `ok`, the
check stays inline as a `state` and an `if`, and so does every refusal below it, because a guard runs
at the front of the body and would refuse the replay:

```hek
command RecordWarrantySale(warranty_id: Uuid, shop_id: Int, premium: Bool) {
  guard ShopIsConnected { shop_id }

  state already_sold: Bool = fold false
    on @warranty.sold(warranty_id, shop_id) => true
  if already_sold {
    return
  }

  state sold: Int = fold 0
    on @warranty.sold(shop_id) => sold + 1
  if !premium && sold >= FREE_TIER_LIMIT {
    return reject FreeTierExhausted
  }

  emit @warranty.sold { warranty_id, shop_id }
}
```

**The raw slice form is the rare one.** `guard @order.placed(order_id)` adds slices and binds
nothing. A `state` fold already contributes its own slice, so writing the raw form beside a fold on
the same slice adds an identical predicate and no safety. It earns its place only when the decision
needs a slice in the boundary that nothing folds.

**Reach for `state` when the value is this command's own**, since a guard hands nothing back. A
discount computed from `lifetime_spend` is a fold, not a guard.

## 7. Refusals

```hek
refusal ShopNotFound "shop does not exist"
refusal SkuTaken(sku: String, item: Uuid) "sku {sku} already belongs to item {item}"

return reject ShopNotFound
return reject SkuTaken { sku: wanted, item: other_id }
```

`refusal <Name>[(<field>: <Type>, ...)] "<message>"`. Parens declare, braces use, with the same
bare-name shorthand. A refusal with no fields takes **no braces** at the use site, which is what lets
`return reject ShopNotFound` be the last statement in a block.

**The code is derived from the name**: `ShopNotFound` becomes `"shop_not_found"`. Insert `_` before
each capital after the first and lowercase the rest. This is the one name whose spelling leaves the
program, so the derivation is kept reversible: a refusal name **must start with a capital and must
contain no `_`**.

**The message may name the refusal's own fields and nothing else, and every field must be named by
the message.** A field the message skips is unreachable, so it is an error rather than a lint. A
const in a message goes through a field:

```hek
refusal FreeLimit(limit: Int) "the free tier lists {limit} items"
return reject FreeLimit { limit: FREE_LIMIT }
```

**Reading one back.** A bare refusal name in a `String` position is its code, so the consuming side
is checked too:

```hek
let r = invoke ListItem { item_id, seller_id, sku }
if r.refused(ShopNotFound) { log("the shop went away") }
if r.code().unwrap_or("") == ShopNotFound { log("the same question, spelled out") }
```

`.code()` is a `String?` and `T?` does not fill `T`, so the `unwrap_or` is load-bearing. An `invalid`
carries no code, so `refused` answers `false` for it whichever refusal is named.

`reject <Name>` and `invalid(msg)` may be written wherever an `Outcome` is expected: as a `return` in
a command or a guard, and as the value of a `fn` declared `-> Outcome` or `-> Outcome?`. A `fn` that
declared neither cannot write either.
