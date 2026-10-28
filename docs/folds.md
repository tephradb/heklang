# Named folds

A `fold` is a slice of the log and the value it folds to, declared once and read by name:

```
fold OpenProposal(shop_id: Shop, order_id: Int) -> Uuid? = none {
  on @edit.proposed(shop_id, order_id) { proposal_id } => proposal_id
  on @edit.cancelled(shop_id, order_id) => none
  on @edit.invalidated(shop_id, order_id) => none
  on @edit.confirmed(shop_id, order_id) => none
}

command ConfirmItemEdit(shop_id: Shop, order_id: Int, proposal_id: Uuid) {
  let at = now()

  guard WindowIsOpen { shop_id, order_id, at }
  fold open = OpenProposal { shop_id, order_id }

  if open != proposal_id {
    reject ProposalNotOpen
  }

  emit @edit.confirmed { shop_id, order_id, proposal_id }
}
```

This document is the contract. `tests/folds.rs` is the same set of rules as executable tests, one
section per numbered rule. Change the doc, the tests and the code together.

Three of the rules below say what building it corrected, because the design was written against a
port rather than against an implementation and was wrong in three places: rule 3 on where the
accumulator lives, rule 6 on whether a fold may read a fold, and rule 11 on the digest. Each
correction is recorded where the rule is, rather than quietly applied.

`docs/commands.md` is the inline `fold` and the stage model it lives in, and every rule there still
holds: this document adds a second **position** for the same construct and nothing else.
`docs/guards.md` is the other half of the pair, and rule 4 below is the line between them.

## The gap it closes

`guard` factored out a proposition and deliberately handed nothing back, so a fold whose *value* a
command needed could not be factored at all. `docs/guards.md` rule 6 says so outright, and the
sentence it gives as the reason is the one this document disagrees with:

> Handing values back would make a guard an object with fields, which is the aggregate rule 1 is
> about.

The two halves of that are separable. A value handed back is one value named for the value; the
aggregate is the *grouping* of several under an identity. Refusing the grouping and keeping the
value costs nothing, and rule 1's naming rule transfers across unchanged.

**The evidence is a port of a real application**, 97 modules and 25,347 lines, migrated to this
construct before it was built:

| | |
| --- | --- |
| inline `fold` declarations | 425, of which 258 distinct |
| **byte-identical copies** | **167 (39%)** |
| replaced by a named fold | 288 call sites across 80 declarations |
| named folds with two or more callers | 68 of 80 (85%) |
| `test` declarations needing a change | **0 of 265** |

The worst single case repeated `fold shop_domain: String = "" / on @shop.installed(shop_id: e.shop_id)
{ shop_domain } => shop_domain` **nineteen times across thirteen files**, with
`fold token` and `fold order_gid` fourteen times each beside it. One module, `effects/email.hk`,
carried 57 folds that were 11 distinct ones.

What drifts when those fall out of step is the append condition, and `docs/testing.md` §8 keeps that
out of a test on purpose:

> Slice counts, **the append condition**, and how many times the fold ran are not outputs.

So the failure mode is a command that reads a stale value, decides on it, and appends anyway,
because the arm somebody added to twelve copies of a fold was not added to the thirteenth. Nothing
static and nothing dynamic reports it.

**It also answers a gap `docs/effects.md` left open.** Rule 1 there names eight arms that "still
repeat the same three folds between them, and that is the case that argues for effect-level `fold`,
which this pass deliberately did not add". 178 of the 425 folds above are in effect arms. A named
fold answers them with no new scoping rule, so effect-level `fold` is not needed and should not be
built.

---

## 1. Declaring

```
fold <Name>(<name>: <Type>, ...) -> [<acc>:] <Type> = <seed> {
  on <event>(<filters>) [{ <binds> }] => <expr>
  ...
}
```

A name, parameters, the value's type and seed, and one arm per event type it reads. **Exactly one
accumulator** (rule 3), and no `return`.

**A fold is named for the VALUE, never for the entity.** `OrderName`, `ShopAccessToken`,
`OpenProposal`, `QuoteFailureCount`. Not `Order`, not `Shop`, not `Proposal`.

This is `docs/guards.md` rule 1 read for values instead of propositions, and it is the whole of what
keeps the construct from growing aggregates. One value out means there is nothing to put fields on,
so a command wanting two facts about an order names two folds and conflicts on exactly those two
slices. `Order` with a `.name` and a `.gid` would be one read whose boundary is the union of
everything any caller wanted, which is an aggregate with the serial numbers filed off.

**Parens declare and braces use**, the rule `docs/guards.md` rule 2 states for `command Foo(...)`
against `invoke Foo { ... }` and for `guard` against `guard`. One word for the declaration and the
use, because this is one concept.

**The parameters are what the filters and the seed need, and nothing else.** An arm's expression is
self-contained in practice: of the 425 folds in the port, **not one** read a binding from the body
that enclosed it. So a fold's parameters are the ids it is kept by, which is also why the migration
was mechanical.

## 2. Using

```
fold <local> = <Name> { <name>: <value>, ... }
```

A declaration, in the declaration run beside `fold` and `guard`, with the bare-name shorthand every
other block takes, so `fold open = OpenProposal { shop_id, order_id }` is
`{ shop_id: shop_id, order_id: order_id }`.

**It is a declaration and not an expression**, and that is load-bearing. A read written inside an
`if` or a `for` would be a log read that may or may not happen, which `docs/commands.md` refuses
because it has nothing to say in an append condition. Keeping the use in the declaration run means
every rule about stages applies to it unchanged: a run of them is one read, a statement below them
closes the stage, and the arguments resolve at step 2.2 before the stage folds.

**The caller names the local, so the local carries the role and the declaration carries the value.**
This reads better than the inline form it replaces:

```
fold before = OrderShipTo { shop_id, order_id }
fold after = RequestedShipTo { request_id }
```

**An argument may not read a fold from its own stage**, the rule `docs/guards.md` rule 7 already
states for a guard's arguments and for the same reason: an argument is assigned above the
declarations it sits with, so it would read that fold's seed rather than what it folds to. A fold an
earlier stage folded is fine, and a statement between the two runs is what closes the first.

**A read closes the stage's prologue**, so a statement written below one is in the stage's second
half and runs after the fold. That is what lets the statement below a read be the one that uses it,
which is the common case and the whole point.

A `guard` closes it too, and for one release it did not: a statement below a `guard` landed in the
prologue and ran before the guard decided, so a bare `return` under one skipped it silently. Writing
this rule is what found that, because a fold could not be left that way without reading a slot the
splice assigns afterwards. `docs/guards.md` rule 3 had always said the guards run "each before the
statements below them"; the implementation now agrees, at the cost of `hek-digest 5`.

## 3. One accumulator, and its name is written only when it is read

A fold has exactly one accumulator, because it returns exactly one value. There is no `return`: the
accumulator *is* the value.

**The name is written when an arm reads the running value, and omitted when none does.** Both are
checked, so a fold has one legal spelling:

```
fold WindowIsOpened(shop_id: Shop, order_id: Int) -> Bool = false {
  on @edit.window.opened(shop_id, order_id) => true
}

fold QuoteFailureCount(shop_id: Shop, order_id: Int, proposal_id: Uuid) -> failures: Int = 0 {
  on @edit.quote.failed(shop_id, order_id, proposal_id) => failures + 1
}
```

An arm reading a value no name was bound for is an ordinary scope error. A name no arm reads is
`unused-accumulator`:

> this accumulator is never read; a fold's value is its declaration's name, so write `-> Bool = false`

**The asymmetry with an inline fold is the point rather than an inconsistency.** Inline, the
accumulator name is the only name the value has and the command reads it below the declaration, so
it is always written. Named, the value already has two names, the declaration's and the caller's
local, and a third is a synonym: `fold OrderName(...) -> name: String` says "name" twice. In the
port, **2 of 41** hand-written named folds read their accumulator.

**It also removes a shadowing hazard.** The accumulator is usually named after the field it folds,
and then the two collide:

```
fold WindowIsDryRun(...) -> dry_run: Bool = false {
  on @edit.window.opened(shop_id, order_id) { dry_run } => dry_run
}                                                          // the bind, shadowing the accumulator
```

Both spellings mean the same thing *in that arm*, which is what makes it hard to see: the question
"which `dry_run`" has no consequence here and every consequence in an arm that computes.

That example is refused, and by this rule rather than by a second one. No arm reads the accumulator,
so the name is `unused-accumulator` and the fold is written `-> Bool = false`, which leaves one
`dry_run` in scope and the question cannot be asked at all.

**The two-arm case needs its own rule, and has one.** Where one arm reads the running value and
another binds a field of the accumulator's name, `unused-accumulator` is satisfied by the first arm
and says nothing about the second:

```
fold Spend(customer_id: Customer) -> spend: Money(2) = 0 {
  on @order.placed(customer_id) { total } => spend + total
  on @order.adjusted(customer_id) { spend } => spend + 1.00   // the event's spend, not the total
}
```

The second arm resets the lifetime total to one event's field plus one. That checked clean and ran,
and the digest showed it plainly: the arm's update read the bind's slot rather than the
accumulator's. It is now `declared-twice` at the bind:

> `spend` is `Spend`'s accumulator and another arm reads it, so this arm cannot bind a field of
> that name

**Only when some arm reads the accumulator**, which is the whole condition. A fold whose every arm
*replaces* the value has nothing to shadow, and
`fold token: String? = none / on @shop.connected { token } => token` is the common spelling of
exactly that: refusing the bind there would force a rename for no gain. The inline form shares the
arm loop, so it shares the rule; it cannot drop the name, but it can rename it.

**The check belongs to `check` and not to `fmt`.** Telling the accumulator from an identically-named
bind needs name resolution, and `docs/fmt.md` reads the grammar. A token scan gets this wrong, which
is worth stating because it got it wrong three times while this construct was being measured.

**Rejected: several accumulators and a `return`.** It was the first shape proposed, to cover a
running maximum needing both a current and a peak. Nothing in the port needs it: **no fold of the
425 reads a sibling fold's accumulator**, and all 41 hand-written named folds have exactly one. The
case it was for is expressible anyway, because an accumulator may be a record:

```
record Peak { current: Int, highest: Int }

fold CoursePeak(course: String) -> p: Peak = Peak { current: 0, highest: 0 } {
  on @student.subscribed(course) =>
    Peak {
      current: p.current + 1,
      highest: if p.current + 1 > p.highest { p.current + 1 } else { p.highest },
    }
  on @student.unsubscribed(course) => Peak { current: p.current - 1, highest: p.highest }
}
```

One slice set either way, so it costs nothing in boundary width, and the caller reads `.highest`.
Multiple accumulators would have bought a shorter spelling for a shape that occurs zero times in
25,347 lines, at the price of a second form of the declaration and a `return` keyword a reader has
to look for.

**Rejected: `self`, `this` or `currentValue`.** Three reasons, and the first is the one that
decides it.

heklang has **no implicit bindings anywhere**. An effect's trigger is author-named (`as e`), event
fields are bound explicitly in the arm's block, and there is no `event.data` equivalent. `self`
would be the language's first magic name, introduced for what turns out to be one declaration in
twenty.

It would also break the promotion path in rule 9. An inline fold names its accumulator because the
command reads it below, so inline arms reference a name; a named form referencing `self` instead
would mean rewriting every arm on promotion, which fights the diagnostic whose whole job is to tell
an author to promote. The port's 288 call sites were migrated without one arm being touched.

And `self` reads as "the object", which is the mental model rule 1 exists to prevent. A keyword
nudging a reader toward "a fold is a thing that has state" works against the only thing keeping this
construct from being an aggregate. `currentValue` avoids that and still fails the first reason.

## 4. A fold computes; a guard refuses

| | folds the log | returns a value | may refuse | may emit |
| --- | --- | --- | --- | --- |
| `fold Name(..) -> T` | yes | one | **no** | no |
| `guard Name(..)` | yes | no | yes | no |
| `command Name(..)` | via either | no | yes | yes |

**A fold may not `reject`, `invalid` or `fail`.** This is what keeps the two declarations from
overlapping, and it is a capability rule rather than a convention about what they contain.

The reason is at the call site. `guard` tells a reader that a refusal can happen *here*, which is
what makes `docs/guards.md` rule 3's precedence readable off the page. If a fold could refuse, every
`fold` line would be a possible exit and a reader would have to look up each declaration to know the
command's control flow.

> a `fold` computes a value and cannot refuse; move the refusal to a `guard`, which is what one is
> for

**A guard may still hold its own accumulators**, exactly as it does today, and this is the one place
the construct adds nothing. What changes is what a guard *reaches for* first. In the port all six
guards ended up naming folds rather than declaring them, and `lib/guards.hk` went from 126 lines to
97 with every guard reduced to a list of reads and a ladder:

```
guard ShopIsConnected(shop_id: Shop) {
  fold installed = ShopIsInstalled { shop_id }
  fold authenticated = ShopIsAuthenticated { shop_id }

  if !installed || !authenticated {
    reject ShopNotConnected
  }
}
```

**The doubling this looks like did not happen, and that is why naming is not required.** A
proposition needing a fold that nothing else wants would cost two declarations for one idea. In the
port it never came up, because the folds were already wanted independently: `ShopIsInstalled` and
`ShopIsAuthenticated` are read directly by `shop-token.hk` and `record-shop-authenticated.hk` as
well as through this guard. Where it does come up, the guard keeps the fold inline, which is rule 9
applied to a guard rather than to a command.

**Rejected: forbidding an accumulator in a guard.** It is the more orthogonal rule, and it would put
accumulators in exactly one construct. It contradicts rule 9: a single-use proposition would be
forced to spawn a named fold, which is the forced naming that rule refuses for a command. The
asymmetry would also have no argument behind it, since a guard and a command are the same body with
one `emit` between them.

## 5. A fold's slices are the caller's boundary

What a command folded is what it conflicts on, and a named fold folds on the command's behalf, so
its slices arrive in the `AppendCondition` exactly as if they had been written inline. The filters
resolve against the arguments the caller passed, so a predicate names the order the command was
called for rather than the parameter the fold declared.

**A fold is copied, not called.** `src/inline.rs` splices it into whatever names it, before the
interpreter sees either, so a command reaches the fold with one arena, one frame, one slice list and
one read of the log however many folds and guards it names. The splice already moves a guard's
accumulators into the caller's stage as `FoldVar`s; a fold is the same splice with the accumulator's
slot bound to the caller's local instead of discarded.

**An explicit return type is what the splice costs.** It happens after parsing, so a caller's body
is lowered before the fold's body exists and the parser needs the declared type to give the local
one. This is the same reason a `fn` declares its return type.

**`hek check --boundaries` lists reads beside guards**, and this is where the construct pays for
itself twice. Before, the listing printed guard names and every inline fold was invisible:

```
  GrantLoyaltyTier guards HasOrderedBefore
                    reads CustomerHasOrdered, LifetimeSpend, OpenOrderCount
```

Two lines rather than one list, because they are different claims: a guard can refuse the command
and a fold cannot, and that is the thing the names alone do not say. A fold reached *through* a
guard joins the `reads` list rather than getting a line of its own, because that is what it is: the
guard is where the refusal lives and the fold is only a slice in the boundary.

It is an **upper bound**, for the reason `docs/guards.md` rule 5 gives: a command that returns above
a declaration run never reaches what is below it. And it is only as complete as the program is
named, which is the honest cost of rule 9.

## 6. A fold names nothing, so it is always a leaf

A fold's body is **arms and nothing else**: no statements, no declarations, no `guard` and no second
fold. That is not a restriction added on top of the shape, it *is* the shape, because rule 3 moved
the accumulator into the signature and left the braces holding only `on` lines.

Three things follow, and the third is why this is a rule rather than an observation:

- a fold cannot refuse (rule 4) because there is nowhere in one to write a `reject`
- a fold cannot read a fold, so there is no fold graph, no `recursive-fold` code, and nothing to
  order before the splice
- `hek check --boundaries` is exact for a fold in a way it is not for a guard. `docs/guards.md`
  rule 5 pays for composition by admitting the boundary "stops being readable off the page"; a
  fold's never leaves it, because what a fold reads is the arms under it.

**This document said the opposite while it was a design**, and the correction came from building it.
"A fold may read a fold to any depth" was written by analogy with guard composition, and the
analogy does not hold: a guard's body is a command's body, where a declaration run is grammatical,
and a fold's body is an arm list. Nothing in the 25,347-line port that the construct was measured
against reads a fold from a fold, so the feature would have been grammar for a case that does not
arise.

**What takes its place.** A value computed from two folds is computed where both are in scope, which
is the caller:

```
fold count = CourseSubscriptionCount { course_id }
fold capacity = CourseCapacity { course_id }

let full = count >= capacity
```

That is DCB's `derived` projection written as a `let`, and it costs nothing: both slices are in the
boundary either way, and the `let` is one line at the one site that wanted it.

## 7. A sealed value crosses a signature as its own type

`Sealed(T, Subject)` is spellable in a fold's **return type** and nowhere else:

```
fold MerchantEmail(shop_id: Shop) -> Sealed(String, Shop)? = none {
  on @shop.installed(shop_id) { merchant_email } => merchant_email
}

fold OrderCustomerEmail(shop_id: Shop, order_id: Int) -> Sealed(String, Customer)? = none {
  on @order.contact.recorded(shop_id, order_id) { email } => email
}
```

This is the one position added to the four `docs/types.md` section 1 already lists, each spellable in
exactly one place: `Response`, `Secret`, an event type, `Outcome`. It creates no seal, because only
`@subject` on an event field does that, so `docs/effects.md`'s "one place able to create a seal"
holds unchanged. `Opt` stays outermost, as everywhere else.

**Without this the construct has a hole exactly where it is worth most.** The four most duplicated
folds in the port are all sealed: `MerchantEmail` nineteen times, `ShopAccessToken` fourteen,
`OrderCustomerEmail` six, and `OrderShipTo`.

**Rejected: `-> String? @subject(shop_id)`, reusing the authored form.** It was the first attempt
and it works for a value sealed under a parameter. It cannot express the commoner case: `@subject`
names a sibling field holding the subject **id**, and a customer-sealed fold takes its id from the
event's own `customer_id` field, which is not in scope at a signature. Adding a parameter for it
does not help either, because the caller does not know the customer id: it folds that from the same
event, in the same stage, so rule 2 forbids passing one into the other. A type needs the namespace
and nothing more; the id rides in the value, which is what `Value::Sealed` already carries.

## 8. A fold may return a record

There is no rule against it, and the rule that was proposed would not have done what it claimed.

The hazard is a fold whose **slices** are wider than its caller needs, and that has nothing to do
with records: `EditIsClaimed` reads four event types and is entirely correct, because the
proposition needs all four. Banning records would not have prevented it.

What prevents it is rule 1's naming rule and `--boundaries`, which is the same answer
`docs/guards.md` rule 5 reaches for guard composition: the cost is real, it is not representable in
the type system, and it is paid for by being printed on request.

**A record is right when the record is the value.** Seventeen fields of one event, folded under one
filter, are one slice and one predicate whichever way they are written. A record is wrong when its
fields come from different slices, which is the aggregate, and which the naming rule already refuses.

## 9. Inline or named

**Write a fold inline. When a second declaration needs the same one, `hek check` makes you name it.**

`duplicate-fold` fires on a normalized fold appearing in two declarations, keyed on the slice set,
the binds, the seed and the arms:

> this fold is also declared at `quote-outcome.hk:68`; name it, so the two cannot drift apart

It is an **error** rather than a warning. A rule about the append condition that an author may
ignore is not a rule, and `docs/testing.md` §8 means no test will catch what ignoring it costs.

**Two *bodies*, not two declarations.** An effect's arm counts as one, and that is not a detail: the
worst case the rule was measured on is nineteen arms of **one** effect folding the same thing
nineteen times, and keying on the declaration would have missed every one. The message names a file
and a line rather than a body for the same reason, which is also what a reader navigates to.

**The trigger has no false positives by construction.** Byte-identical folds cannot differ in
meaning. It fires 167 times on the port, and the near-miss pairs that look alike all survive it:
the FR-11 window snapshot (`@edit.window.opened`, seeded `false`) and the live policy
(`@edit.policy.configured`, seeded `true`) differ in both event and seed.

**Compared as written, including the accumulator's own name**, which is what makes that claim exact
rather than nearly so. Normalising the name would equate
`fold a: Bool = false / on @w.opened(id) { a } => a` with the same shape over a field called `b`,
and those fold two different fields of one event. The cost is that a pair differing *only* in its
local name goes unreported, which is the safe half of the trade: the check under-counts and never
misfires. Comparing the lowered form instead would have the opposite problem, equating two folds an
author wrote differently on purpose.

**It found one the migration's own tooling missed.** The script that named 80 folds across the port
keyed on the slice set and gave up on any fold binding more than one field, so a two-arm
`captured_currency` stayed duplicated across two effects. `hek check` reported it on the first run
after the check landed, which is the argument for the check in one sentence.

**There is no reverse check**, and a named fold with one caller is not a diagnostic. Twelve of the
port's eighty had one, and several were right: naming one of a family for symmetry with three
siblings, and naming a fold precisely so it stops resembling the one it is not.

### Why inline folds are kept

The alternative was to eliminate them, so that every fold is named and the author never chooses.
That was measured on the port and it is worse.

`commands/ingest/record-order-placed.hk` folds **seventeen fields of one event** to snapshot a
merchant's policy onto the window. All seventeen are single-use, one slice, one predicate, one pass
over the log. Named, they are seventeen global declarations used once each, roughly eighty-five
lines, and the reader loses the locality that makes the block legible as one act. The record
alternative is no better: `record` has no update form, deliberately
(`docs/declarations.md`), so the arm lists all seventeen field names twice, once to bind and once to
construct, and the `emit` still copies them back out.

Two more costs, both measured. Naming everything takes the program from 80 global fold names to
about 160, and auto-naming the tail produced outright **collisions**: `@refund.issue.failed` and
`@shipping.refund.issue.failed` both want to be `IssueFailed`. And `docs/modules.md` makes every
declaration global with no file-scoped visibility, so there is no cheap way to give a one-off a name
without paying for it in the one namespace there is.

The 137 folds the port leaves inline are single-use, 47 of them idempotence checks
(`=> true` followed by a `return`). A fold used once **cannot drift**, so forbidding it taxes the
case with no hazard to fix the case that has one.

### Why this is not a choice the author has to get right

The objection to keeping both is that someone writes a fold inline and the next person copies it
rather than promoting it. `duplicate-fold` is that objection answered mechanically, in the only case
where it matters.

And the choice is reversible at no cost, in both directions, which is what separates it from the
kind of decision a language should settle for its authors. Promotion is a header edit:

```
// inline
  fold failures: Int = 0
    on @edit.quote.failed(shop_id, order_id, proposal_id) => failures + 1

// promoted: the arm is byte-identical, and the caller keeps the local name,
// so nothing below the declaration changes either
fold QuoteFailureCount(shop_id: Shop, order_id: Int, proposal_id: Uuid) -> failures: Int = 0 {
  on @edit.quote.failed(shop_id, order_id, proposal_id) => failures + 1
}

  fold failures = QuoteFailureCount { shop_id, order_id, proposal_id }
```

The port's migration is the evidence: 288 call sites across 38 modules, and **not one arm was
edited**. This is also the property rule 3 protects by keeping the accumulator named rather than
spelling it `self`.

It is the same choice `guard` has always offered between an inline `if ... { reject X }` and a named
proposition, and `docs/guards.md` rule 6 is a page of guidance on which to reach for. A language
that asks an author to decide where a value lives but not where a refusal lives would be the
stranger one.

## 10. What a fold may not do

Most of this list is rule 6 rather than a check of its own: a fold's body holds arms, so every
statement below is unwritable rather than refused. The table says which is which, because an author
reaching for one wants to know whether to look for a message.

| | why | how it is refused |
| --- | --- | --- |
| `reject`, `invalid`, `fail` | rule 4: a fold computes, a guard refuses | unwritable (rule 6) |
| `emit` | only a command appends | unwritable |
| `put`, `patch`, `update`, `delete` | a fold reads | unwritable |
| `invoke`, `http.*`, `erase`, `log` | an effect's, and a fold runs inside whatever reads it | unwritable |
| `return` | rule 3: the accumulator is the value | unwritable |
| name a `guard` or a fold | rule 6 | unwritable |
| read the log twice | one declaration run, as a guard has | unwritable |
| `now()` | a fold decides from the log; take the moment as a parameter | `fold-restriction`, in an arm |
| `reveal` | the fold hands back the seal and the caller reveals it (rule 7) | `fold-restriction`, in an arm |
| read a `secret` | a secret is not in the log, so rotating one would change a replay | `fold-restriction`, in an arm |
| read a `fold` in a filter | a stage's filters resolve before it folds | `stage-shape`, in an arm |
| fold nothing | a value computed from arguments alone is a `fn` | `empty-declaration` |

**An arm is the one place a message is needed**, because an arm is an ordinary expression and
`now()` and `reveal` are ordinary calls in it. Both are already refused there for the inline fold,
by the same `fold-restriction` check and for the same reason, so a named fold inherits them rather
than adding anything.

**`fold-restriction` has to beat `wrong-context`, and did not.** A fold is spliced into whatever
reads it, so the context gate answers for one as though it were the command it lands in. Asking the
gate first meant `reveal` in a fold's arm reported *"only an effect crosses the decrypt boundary / a
command decides from state without reaching personal data"* — a rule about a declaration the author
is not in, under a code this table does not list, and wrong on its face for a fold read from an
effect arm. The fold check runs first now, in both `reveal` and a `secret` read.

**The filter row is a fold's own stage, checked where a command's is not.** A command's body closes
each stage through `decl_body`, which calls `stage_check`; a fold's body is arms only, so there is
no statement to close anything and nothing was calling it. `on @a.b(id: acc)` therefore passed
`hek check` and died at run time with *"slot read before it was set"*, which is the one shape
`Builder::fold_call` calls the thing a checker must not allow.

`empty-declaration` has two spellings and the sharper one wins. A body with no arms is
"fold `F` has no arms, so it is only its seed", which is about the declaration; `folds_nothing` is
the backstop and reports "fold `F` folds nothing".

## 11. Where this diverges from the runtime

Nothing, because a named fold does not reach the runtime. Like a guard it is a parse-time construct
that `src/inline.rs` splices out before `Program` is finished, so a host, the interpreter and
`docs/host.md`'s traits see a command with its folds written out. There is no named fold in the
append condition, in the journal or in a trace.

**The digest gains nothing, and the construct itself moved no hash.** `src/digest.rs` gives the
rule: `const`, `refusal` and `guard` are absent from the digest form *because the parser inlines
all three, so a declaration of any of them runs nothing and its content is already at every use
site*. A named fold is spliced identically, so it is the fourth. `docs/digest.md` rule 7 gains one
word and nothing else, and a program with no named folds hashes exactly as it did.

Two **bug fixes** on the same branch did move hashes, which is what `hek-digest 5` is for: a
statement below a guard moved from a stage's `(pre ..)` to its `(post ..)`, and a fold whose return
type already spells its seal stopped being sealed a second time. The second is the sharper one: a
declared `Sealed(T, S)?` was lowered to `Opt(Sealed(Sealed(T, S), S))`, a type no source can write,
disagreeing with the `Update::ty` of its own arms, and hashed in that shape. It also meant the
declared subject was never checked against the one the arms carry, so
`-> Sealed(String, Shop)? ` over an arm folding content sealed under `Customer` checked clean. Both
are now right and both are in one unreleased version.

That is the opposite of what this document said while it was still a design, and the correction is
worth keeping rather than quietly fixing: "a fold is a declaration now, so it earns an entry" is an
argument from the shape of the source, and the digest is a record of **what a program does**. A
declaration that is copied into its callers and then deleted does nothing, so there is nothing to
record. `subject` is the contrast that makes the line clear: it earns an entry because its parent
appears nowhere else in the program, which is not true of anything a splice leaves behind.

A consequence worth stating, because it is a real loss rather than a saving: a fold's change is
visible only inside whatever folded it, so renaming one or editing an arm moves the hash of every
command that reads it and nothing identifies the fold as the cause. That is already true of a guard
and of a `const`, and `hek digest --expanded` is where an author goes to see which.

**It also makes a fold testable.** `docs/testing.md` §8 refuses to assert state on the grounds that
"a folded value is an intermediate", which stops being true of a declaration with a signature. What
that section rules out stays ruled out; a named fold is simply no longer in it.

**Rejected: `read` as the keyword.** It was the first proposal, and it is a second name for a
concept that already has one. A named fold folds, the accumulator and arms are the same syntax
inline or named, and `docs/guards.md` rule 2's argument against a second keyword for one idea
applies here word for word. The construct adds one position, not one word.

## 12. An arm may bind the envelope, and that makes the fold query-only

`docs/commands.md` keeps the envelope out of a fold: an event's `at`, `id` and `position` are not
foldable, so an event a decision has to time carries its own `Timestamp` field. **That rule stays
for decisions**, for two reasons that have nothing to do with taste. A test writes payload fields
and the append time is the runtime's, so a decision over `e.at` cannot be pinned in one. And an
imported or backfilled event's append time is when it was imported, so every decision over it is
silently wrong about when the thing happened.

Neither objection is about storage. So an arm may bind the envelope with the same `as` a projector
handler uses, and a fold that does is **query-only**: stored and queried, never folded by a command.

```hek
fold UsageLastRecordedAt(shop_id: Shop, period: String) -> Timestamp? = none {
  on @usage.recorded as e (shop_id, period) => e.at
}
```

`docs/projectors.md` defines what the binding reaches, and it is the same here: `.at`, `.id` and
`.position`. The slots are filled from the event being folded, so two arms naming one field share
one slot and an arm that names none is unaffected.

Five things are refused, each because it would otherwise mean something the construct cannot do.

**A command or a guard that folds one.** This is the whole rule, and the message says which:

```
`UsageLastRecordedAt` binds the envelope (`as`), so it can be queried but not folded by a command
```

Checked across the whole program rather than at the call site: a fold's signature is read in an
earlier pass than its body, so whether an arm binds anything is not known until every body is in.

**An inline fold that binds one.** An inline fold lives in the declaration that reads it, so the
two together are a contradiction rather than a restriction; the fold has to move to module scope
before the envelope can mean anything.

**The binding inside a filter.** A filter resolves once, before the fold runs, and the envelope
belongs to each event the fold reads, so the two cannot be one value. This is where a computed key
would go and there is no such thing yet, so the message says why rather than just no.

**A binding no arm reads.** Rule 3's rule for the accumulator, and sharper here: an unread binding
would decide on its own that the whole fold is query-only, so a word with no meaning would cost a
command its read. Four arms of a converted application wrote `as e` and then used a declared time
instead, which is the right thing to write and the wrong thing to announce.

**A binding that hides a name already in scope.** The accumulator, or a parameter. Resolution
consults the envelope binding before the scopes, so a name that is both wins as the envelope and
whatever it hid becomes unreachable for that arm: an accumulator called `last` beside `as last`
read as the running value in one arm and as the record being folded in the next, and checked
clean. Everywhere else in the language a name cannot hide an enclosing one, and this is no
exception.

**Two classes of fold is a real cost**, and it is the cheaper side by a wide margin. The alternative
is a declared `Timestamp` to fold instead, and in the application that measured this, the read
models stamp or bucket by append time in 97 places across 20 columns while **13 of the 19 event
types involved declare no timestamp at all**. Taking the alternative means adding one to thirteen
event types no decision reads, which inverts the rule that an event declares a timestamp when a
decision has to time it. Keeping the exception narrow matters more than removing it.

**Only the envelope makes a fold query-only.** Nothing else about a fold does, and nothing else
should: the restriction exists for the two objections above, and neither applies to anything else
a fold can write.

## Seen from the read side

Not a rule, because it constrains nothing about writing a fold: `hek check --tables`
derives a read model from the declarations in which every named fold is a column, its
parameters are the key, and the folds that share a key share a table. A parameter no arm
supplies is not a key anything could be written under, so a fold with one is listed as not
stored. That is a listing and not a diagnostic; the language does not require a fold to be
storable, and a fold a command reads is doing its job either way. `docs/cli.md` has the
listing and what orders a key.
