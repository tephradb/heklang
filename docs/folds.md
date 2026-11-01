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

What drifts when those fall out of step is the append condition, and `docs/testing.md` §10 keeps that
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

**And it is spelled like a type: an initial capital, and no `_`.** The second name in heklang whose
spelling leaves the program, the first being a `refusal`'s (`docs/declarations.md`), and held to the
same two rules for the same reason. A named fold is a stored column and the column's name is
`snake_case` of this one, so the two rules keep that derivation reversible and two folds can never
arrive as one column. Without them `fold Sum` beside `fold sum` derives `sum` twice and a runtime
building the real table keys both to it. An inline fold's accumulator is a local and keeps the local
spelling, which is why the rule is on the signature rather than on the keyword.

**And a third rule from the same derivation gives the first one teeth: the column may not be named
after a part of its own key.** `fold Shop(shop_id: Shop)` derives a column `shop` in a table whose
key column is already `shop`, which is one table with two columns of one name and nothing can
create it. `--tables` printed it without complaint, which is how it was found. Both names come out
of `tables::part_names`, so the refusal reads the same derivation the layout does rather than a
second account of the rules, and it names the parameter the part came from:

> fold `Shop` derives a column called `shop`, and its key already has a part of that name

The fix is the one the paragraph above already asks for, which is why this is the rule rather than a
renaming scheme: a key part is what the row is, and a column is what is true of it. A fold with no
parameters has no part to collide with, so `fold Shop()` over the single-row `global` table is fine.

This is `docs/guards.md` rule 1 read for values instead of propositions, and it is the whole of what
keeps the construct from growing aggregates. One value out means there is nothing to put fields on,
so a command wanting two facts about an order names two folds and conflicts on exactly those two
slices. `Order` with a `.name` and a `.gid` would be one read whose boundary is the union of
everything any caller wanted, which is an aggregate with the serial numbers filed off.

**Parens declare and braces use**, the rule `docs/guards.md` rule 2 states for `command Foo(...)`
against `invoke Foo { ... }` and for `guard` against `guard`. One word for the declaration and the
use, because this is one concept.

**A filter's left side is a field of the event, or a parameter.** `(shop_id, order_id: oid)` names
fields and `(kind == "hold_place")` names a parameter, which is the key part that arm writes. Rule
13 has it, and the sigil is the whole of the difference.

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
`fold token: String? = none / on @shop.connected() { token } => token` is the common spelling of
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

**The filter list is written even when it is empty**, because the list is what a slice is: `()` says
this arm takes every record of the event, which is a statement about the boundary and not
punctuation. `on @game.purchased() { game_id } => ..`, never `on @game.purchased { game_id }`. A
projector's `on` has no filter list at all (`docs/projectors.md`), so the paren-less spelling is
right there and wrong here, and that is the one place the two constructs do not read alike.

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

**One thing narrows it rather than widening it.** A rule 13 key conjunct resolves before anything
is folded, so an arm whose conjunct fails folded nothing and is left out of the condition entirely.
A command reading one kind of a nine-kind queue conflicts on that kind's events and no others,
which is the only place in the language where naming a fold buys a *smaller* boundary.

## 6. A fold names nothing, so it is always a leaf

A fold's body is **arms and nothing else**: no statements, no declarations, no `guard` and no second
fold. That is not a restriction added on top of the shape, it *is* the shape, because rule 3 moved
the accumulator into the signature and left the braces holding only `on` lines.

Three things follow, and the third is why this is a rule rather than an observation:

- a fold cannot refuse (rule 4) because there is nowhere in one to write a `reject`
- **a fold's value cannot be computed from another fold's value**, so no caller's boundary is ever
  wider than the arms under it
- `hek check --boundaries` is exact for a fold in a way it is not for a guard. `docs/guards.md`
  rule 5 pays for composition by admitting the boundary "stops being readable off the page"; a
  fold's never leaves it, because what a fold reads is the arms under it.

**The second bullet used to say more than it should**, and rule 15 is why it no longer does. It said
a fold cannot read a fold at all, "so there is no fold graph, no `recursive-fold` code, and nothing
to order before the splice". A fold *can* now read one, in a `key` clause, so there is a graph and
there is a `recursive-fold` code; what has not changed is the splice, because a lookup is not
spliced and a fold is still a leaf there. The rest of this rule stands as written, and it stands for
the **value** side, which is still what the port measured: nothing in those 25,347 lines computes
one fold's value from another's.

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

**A named fold must say so, and silence used to be a leak.** `-> String?` over an arm folding a
`@subject` field is refused: a named fold publishes its *declared* type, so the caller's local, the
IR and the stored column were all typed plaintext while the value was ciphertext, and nothing
downstream would ever have shredded that column for an erased subject. The same shape **inline** was
refused the whole time, because an inline fold's value lives in a slot in the declaration that reads
it and the seal rides on the slot; there is no signature for it to disagree with. That asymmetry is
why it went unnoticed, and it is the reason this rule is checked at the declaration rather than left
to the use site.

Three spellings mean one type, because `Opt` stays outermost whichever side of the parenthesis it is
written on: `Sealed(String, Shop)?`, `Sealed(String?, Shop)` and `Sealed(String?, Shop)?`.

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
ignore is not a rule, and `docs/testing.md` §10 means no test will catch what ignoring it costs.

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
| read the event in a `param ==` filter | rule 13: the right side resolves once, before any record | `not-in-scope`, in an arm |
| write `param ==` in a guard's slice or an inline fold's arm | rule 13: neither is stored, so neither has a key | `fold-restriction` |
| read a parameter or the accumulator on the right of `param ==` | rule 13: a key that depends on the key, which nothing can write | `fold-restriction`, in an arm |
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

**Nothing on the decision path.** Where a command reads a fold, it is a parse-time construct: like a
guard, `src/inline.rs` splices it out before `Program` is finished, so a host, the interpreter and
`docs/host.md`'s traits see a command with its folds written out. There is no named fold in the
append condition, in the journal or in a trace.

**It does reach the runtime on the read path.** `splice` leaves `program.folds` standing, and
`Interpreter::fold(name, args)` folds one of them over the whole log for one key and hands the value
back. That is the read a stored column is **served** by, and it is what a test's `fold` action calls
(`docs/testing.md` section 8). It is also the only way a query-only fold runs at all, since by
rule 12 no command may read one.

`Interpreter::fold_stored` is the read a column is **filled** by, and it differs in the two ways a
store needs: it stops at a log length the store gives it, so every column of a row is at one and a
query reads one position, and it hands a `Value::Sealed` back rather than shredding it, so the store
holds the ciphertext and shreds on its own read. That second half is what `project_into` does and
`project` does not, for the same reason, and without it filling a non-optional sealed column whose
subject was already erased would fail where the same column through a projector does not.

`Tables` is the third door and the one a deployment spends its life in. `Interpreter::fold` and
`fold_stored` both read the whole log for one key, which is the right shape for a backfill and the
wrong one for following a log: on the ported application's order table the median write is **two
columns of 87** across 46 event types, so a host refolding per record pays two whole-log scans
forever. `Tables::apply(record, columns)` applies one record to whatever columns its arms name,
carrying the accumulator in the store through the `Columns` seam, and is to `fold_stored` what
`Projection::apply` is to a projector rebuild. Like a projection it holds no host, so a host drives
it from a thread that never touches the log reader.

It is also **more correct** than refolding, not only cheaper. A host choosing which columns a record
could have changed has only `ColumnArm`, which carries the key parts an arm pins and not the field
filters it also wrote, so it must over-approximate and refold columns the record never touched. That
is harmless when the answer is re-derived and it is not harmless here, so `apply` re-checks each
arm's predicate per record, exactly as a projection re-checks a handler's path.

Two obligations are the sink's own, and both were learned from a store that got them wrong:

- **Fully pinned arms before ranged ones, within one record.** An arm leaving a key part open is a
  range update over every key carrying the parts it did pin, and a key this same record mints is one
  of them. Reading the range first leaves the new key out and nothing afterwards corrects it. One
  record is the whole obligation, since a key minted by an earlier record is already stored.

  **That ordering is over working out the keys, not over writing them**, and the difference is a
  defect that stood for three releases. The two passes used to fold as they went, so a column with
  one pinned and one ranged arm on the same event applied them in *pass* order: the pinned arm
  counted and the ranged arm then overwrote it, where a refold resets and then counts. A fold is its
  arms applied in **source order** to one accumulator, and the sink has to be the same fold for the
  keys it has. So the passes now only collect what the record writes, the keys they mint accumulate
  in a set a range unions in rather than being read back out of the store, and the work is then
  grouped by column and key and folded in written order. That also makes it one read and one write
  per row instead of per arm.

  **`Columns::keys` may over-deliver and the sink drops the rest**, which is the trait's own cut and
  used to be honoured only by accident. A store with no index for a range answering with every row
  of the table is the cheapest correct implementation; a key it hands back that the arm does not
  write is skipped here rather than re-checked later, because a part the arm pinned with a rule 13
  conjunct would be *bound* from that key and the conjunct's gate would go dead, which the fold
  reads as a disagreement between `layout` and the arm and not as a key to skip.
- **The subscription is derived here, not by each host.** `Tables::query` is one predicate per event
  any column folds, with no filters, because two columns on one event filter it differently and a
  subscription is their union. The first store to build its own used `EventPath`'s `Display`, which
  keeps the `@` that an event type does not have: the query matched nothing, the thread followed the
  log and folded none of it, and the checkpoint advanced over every batch while the status endpoint
  reported nothing behind. A silently empty subscription is indistinguishable from being caught up.

**`Tables::only(folds)` is the same sink for a column set**, and a rebuild wants it because `apply`
is wrong for one twice over. A column is rebuilt by replaying the log into it, and `apply` runs every
arm of every column over the replay: the columns that are already correct pay for the work again,
and a body that can `fail` or overflow fails the rebuild of a column it has nothing to do with.
Handing the others a seed instead of their held value avoids the overflow case and leaves the rest,
because the body still runs. The set is named by the **fold**, since a column's name is derived from
it and nothing writes the derivation down; a fold no column is kept for is nothing to apply.

**What it runs is `Layout::closure` of that set**, which is a third obligation of the same kind as
the two above and was found the same way. A range arm covers the keys the table holds, and those
keys are minted by the pinned arms of the table's columns rather than by the ranged one, so a replay
of a ranged column alone mints none of them: the range covers nothing, the column comes out at its
seed, and nothing says so. `OrderClosed`'s `@shop.uninstalled(shop_id)` arm above is exactly that
shape, and the orders it ranges over exist because `OrderTotal` wrote a row at each. What the
closure adds is the columns of that table that **mint**, the ones with a fully pinned arm, since a
column no arm of which pins the whole key writes only at keys that already exist.

So the restricted sink is a **handle** and the closure is **readable off it**, which is `docs/host.md`
section 7a's half of this and matters to anyone writing a rebuild: the sink carries the accumulator
in the store, so replaying onto a column that already holds its value folds every record into it
twice. `Rebuild::folds` is the set a rebuild resets or shadows before it starts, and it is wider than
the set it asked for. What the extra columns are there for is their **keys**, so a rebuild that
drops their writes outright drops the keys with them and is back where it started; recording each
key and discarding the value is enough, since `Columns::keys` is all a ranged arm reads.

**A stored column is defined over the keys the table holds**, and that is a real boundary rather
than an implementation detail. A fold is a total function of a key and a log, so `OrderClosed(shop,
order)` with an arm on `@shop.uninstalled(shop_id)` is `true` under `Interpreter::fold` for *any*
order id, including one no event ever mentioned. The store has no row there and reads the seed, so
the two disagree. The key space is unbounded and the alternative is not implementable, and a read at
a key nothing minted is a question about a row that does not exist.

**The boundary is in time as well as in space**, and this document used to get the second half
wrong. It said the keys a range arm can need "are exactly the keys the pinning arms mint", which is
about *which* keys exist and says nothing about *when*. A refold is a function of a key and the whole
log, so it applies every ranged record to every key; a sink walks forward one record at a time and
can only apply one to the rows that are there. **A key minted after a ranged record never sees it.**

```hek
fold Seen(shop_id: Int, order_id: Int) -> n: Int = 0 {
  on @x.happened(shop_id) => n + 10
  on @x.happened(shop_id, order_id) => n + 1
}
```

Two records under one shop, orders 7 and 8. The sink leaves order 8 at `11`, because it did not
exist when the first record's range ran; `Interpreter::fold` answers `21`. The same shape reaches
`OrderClosed` above with nothing exotic: an order placed *after* the uninstall is `false` in the
store and `true` on a refold.

**The sink's answer is the definition**, because it is the one a store can produce and the one every
path produces: a rebuild replays the log through `Tables::apply` too, so a follow and a backfill
agree with each other. What they differ from is `Interpreter::fold` and `fold_stored`, which is why
a store fills a column through the sink rather than mixing the two. An author who wants the refold's
answer does not write a range: an arm pinning the whole key means the same thing under both.

`Interpreter::fold_stored`'s bound **is a head and not a position**, which is the one number a store
has: `Log::head` answers
where the next append lands, a command pins its read to one, and a checkpoint records one. It first
took the inclusive last position `Query::upto` carries, and that was wrong twice over. It put a
`head() - 1` at every call site, and an empty log had nothing to pass at all, since `0` is a
position like any other: a store with a head of zero, filling a column before its first event, would
have folded the first event into a row recorded as holding none of it, and nothing downstream could
tell. `fold_stored(.., 0)` is now a column at its seed, `fold_stored(.., 1)` is the first event
alone, and a caller holding a real position asks for `position + 1`.

**A stored sealed column has to carry the field name with its ciphertext**, and that is forced
rather than chosen. `Keys::decrypt` is asked `(subject, id, field, content)` because a host binds
its ciphertext to the name the content was sealed under and sealed content may be moved
(`docs/host.md` section 7), and a `Value::Sealed` carries that name for exactly this reason. **One
column can hold two different ones.** `Builder::seal_fold` takes the *subject* and not the field, so
a single `-> Sealed(String, Customer)` covers an arm binding `email` and an arm binding `contact`:

```hek
fold LatestEmail(customer_id: Customer) -> Sealed(String, Customer) = "" {
  on @a.one(customer_id) { email } => email
  on @a.two(customer_id) { contact } => contact
}
```

One column, `<- @a.one, @a.two`, two field names in it. So the field varies per value and not per
column, and a store that kept the content alone would hold a column it could not read. Storing the
log's own ciphertext verbatim is right, and it is right **with** the name.

**A sealed column is nullable in storage whatever its declared type says.** Rule 9 of
`docs/projectors.md` makes an erase empty the column, and only an optional has an empty value, so
the emptying has to land somewhere for a non-optional one too. Reading it back afterwards raises,
which is `shred`'s loud arm and the honest answer: there is no value that says "this is gone" at a
non-optional position. A store deciding nullability from the declared type alone would therefore
write a null into a `NOT NULL` column the first time a subject was erased.

The two are not in tension. A fold a decision reads is inlined and never executes as itself; a fold
a read model is runs by name and never touches a decision. One declaration, two use sites, and the
splice is what keeps the first from paying for the second.

**A named fold earns a digest entry, and the argument for that is not the one this section first
gave.** It read: `const`, `refusal` and `guard` are absent from the digest form *because the parser
inlines all three, so a declaration of any of them runs nothing and its content is already at every
use site*, and a named fold is spliced identically, so it is the fourth. The premise is false. A
splice deletes a `const`, a `refusal` and a `guard`; it does **not** delete a fold. `splice` leaves
`program.folds` standing and `Interpreter::fold` runs one by name, which is the read a stored column
is filled and served by, and a query-only fold has no use site to have been inlined into at all.

So a fold is a declaration that does something as itself, and it earns an entry whose hash is what a
store keys a column by. `docs/digest.md` rules 7 and 9 carry the rest: the signature is the
parameters and the return type, because those are what `tables::layout` reads for the table, the key
order and the column's type. A moved `signature_hash` is therefore a column that changes table or
type, and a moved `hash` alone is one that refolds where it is. Every named fold earns an entry and
not only the query-only ones, for the reason that document gives.

`hek-digest 6` is the entry arriving, and it moved every hash in every program. Two **bug fixes**
had already moved hashes under `hek-digest 5`: a statement below a guard moved from a stage's
`(pre ..)` to its `(post ..)`, and a fold whose return type already spells its seal stopped being
sealed a second time. The second is the sharper one: a declared `Sealed(T, S)?` was lowered to
`Opt(Sealed(Sealed(T, S), S))`, a type no source can write, disagreeing with the `Update::ty` of its
own arms, and hashed in that shape. It also meant the declared subject was never checked against the
one the arms carry, so `-> Sealed(String, Shop)? ` over an arm folding content sealed under
`Customer` checked clean. Both are now right.

**What the entry buys back is attribution**, which was the real loss while there was none. A fold's
change used to be visible only inside whatever folded it, so renaming one or editing an arm moved
the hash of every command that reads it and nothing identified the fold as the cause. Those hashes
still move, because the content really is at each of those sites, but the fold's own entry moves with
them, so `hek digest --expanded` is no longer the only way to find out which fold did it.

`subject` was the contrast offered for the old line: it earns an entry because its parent appears
nowhere else in the program, "which is not true of anything a splice leaves behind". A fold is
exactly what a splice leaves behind and it earns one anyway, so the contrast was never about the
splice. It is about whether a declaration still does anything once its callers have been served, and
a fold does.

**It also makes a fold testable, and it now is.** `docs/testing.md` §10 refuses to assert state on
the grounds that "a folded value is an intermediate", which stops being true of a declaration with a
signature. A `fold` action asserts one, and section 8 of that document is what it expects. What §9
rules out stays ruled out; a named fold is simply no longer in it.

**Rejected: `read` as the keyword.** It was the first proposal, and it is a second name for a
concept that already has one. A named fold folds, the accumulator and arms are the same syntax
inline or named, and `docs/guards.md` rule 2's argument against a second keyword for one idea
applies here word for word. The construct adds one position, not one word.

## 12. An arm may bind the envelope, and that makes the fold query-only

`docs/commands.md` keeps the envelope out of a fold: an event's `at`, `id` and `position` are not
foldable, so an event a decision has to time carries its own `Timestamp` field. **That rule stays
for decisions**, and the reason that carries it is backfill: an imported or replayed event's append
time is when it was imported, so a decision over it is silently wrong about when the thing happened,
and nothing in the event says so.

The second reason used to be that a test could not pin an append time, and that half has since
stopped being true: a `fold` action asserts a query-only fold's value against the harness's own
clock (`docs/testing.md` section 8). It was always the weaker half, because it was a statement about
the test harness rather than about the language. Backfill is the one that is about the data.

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

Six things are refused, each because it would otherwise mean something the construct cannot do.

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
belongs to each event the fold reads, so the two cannot be one value. A key computed from the
record is what belongs here, and rule 14 is where it went: a clause of its own after the
destructure, because that is the difference it rests on.

**A binding no arm reads.** Rule 3's rule for the accumulator, and sharper here: an unread binding
would decide on its own that the whole fold is query-only, so a word with no meaning would cost a
command its read. Four arms of a converted application wrote `as e` and then used a declared time
instead, which is the right thing to write and the wrong thing to announce.

**A binding that hides a name already in scope.** The accumulator, a parameter, or a field the same
arm binds. Resolution consults the envelope binding before the scopes, so a name that is both wins
as the envelope and whatever it hid becomes unreachable for that arm: an accumulator called `last`
beside `as last` read as the running value in one arm and as the record being folded in the next,
and checked clean. Everywhere else in the language a name cannot hide an enclosing one, and this is
no exception. **The same-arm case is checked separately**, after the binds are allocated rather than
before: `on @a.one as e (shop_id) { e }` is the one spelling the first check structurally cannot
see, because the binding is read before the arm's own scope exists.

**A payload field reached through the binding.** `e.threshold` is not a shorter way to write
`{ threshold }`. The binding carries three fields and is not a second route to the record, which is
what keeps a fold's reads visible in its arms: the binds are the fold's declared surface on the
event, and a projector handler's `e.field` works only because a handler binds on demand and a fold
does not. The message names the field and says where it belongs.

**Two classes of fold is a real cost**, and it is the cheaper side by a wide margin. The alternative
is a declared `Timestamp` to fold instead, and in the application that measured this, the read
models stamp or bucket by append time in 97 places across 20 columns while **13 of the 19 event
types involved declare no timestamp at all**. Taking the alternative means adding one to thirteen
event types no decision reads, which inverts the rule that an event declares a timestamp when a
decision has to time it. Keeping the exception narrow matters more than removing it.

**Only the envelope makes a fold query-only.** Nothing else about a fold does, and nothing else
should: the restriction exists for the two objections above, and neither applies to anything else
a fold can write.

## 13. An arm may match a key part the event does not carry

A filter's left side is a field of the event, written `field: value` or as the bare
shorthand. It may also be a **parameter of the fold**, written `param == value`, and then it is
not a narrowing of the event at all: it says which key this arm writes.

```hek
fold RetryAttempt(shop_id: Shop, kind: String, order_id: Int) -> Int = 0 {
  on @hold.place.failed(shop_id, order_id, kind == "hold_place") { attempt } => attempt
  on @hold.release.failed(shop_id, order_id, kind == "hold_release") { attempt } => attempt
}
```

**The sigil is the whole of the difference.** `:` matches a field, `==` matches a parameter, and
both are legal when one name is both: an event carrying its own `kind` field can still be bucketed
by a `kind` the caller passed. Nothing is overloaded on whether a name happens to be declared.

**What it is for is a queue holding several kinds of work.** The application this was measured on
has two: a failure list of twelve kinds and a retry ladder of nine, each keyed by a synthetic
`Uuid.derive(NS, "{kind}|{ids}")`. Those ids exist only because a projector entity gets one `@key`
column; as folds the ids are already several parameters, and the kind is the one part left with
nowhere to go. Without it the kinds collide: `hold_place`, `hold_release`, `order_tag`,
`order_annotation` and `drift` are all keyed `(shop_id, order_id)`, so one fold per kind per column
is the only alternative, which is 36 folds and 120 where 12 and 30 will do.

**The right side resolves once, in the frame of whatever reads the fold.** A literal, a `const`, a
parameter, a pure `fn` of those: the same things a field filter's value may be, under the same
`folding` rule that bars the clock, a call out, `reveal` and a secret. It may **not** read the event
being folded, and the deferred half of the design is exactly that, a right side evaluated per
record. Rule 12's refusal of `as e` inside a filter is the same door, and says so.

**A conjunct that does not hold drops the slice rather than widening it.** The slice folds nothing,
so it joins no append condition either. That is the opposite of what the design predicted, and it
is available only because the answer is fixed for the invocation: a command reading one kind of a
nine-kind queue would otherwise conflict on all nine. What a command folded is still a subset of
what it conflicts on, and here the subset is tight.

Six things are refused:

**A guard's slice**, and **an inline fold's arm**, for two different reasons that get two different
messages. A guard decides rather than being stored, so there is no row for a key to name; an inline
fold has no signature to declare a parameter in at all.

**A name that is not a parameter.** Resolved against the parameters rather than against everything
in scope, which is the point: `lookup` would answer just as readily for the accumulator, and an
accumulator on the left is a key that depends on what has been folded so far.

**Anything on the right that reads a slot**, a parameter or the accumulator. Whatever writes the
row holds the event and the key it is writing and no frame at all, so a conjunct reading a slot is a
key that depends on the key. `kind == kind` is the degenerate case and it is a tautology, which
would otherwise make `hek check --tables` call the column storable on the strength of nothing;
`kind == order_id` is the same hole with a value that is not a tautology. So the right side is a
literal, a `const`, or a `fn` of those.

**Two conjuncts on one part in one arm.** They cannot both hold unless they are the same value, so
the arm folds nothing while the column still looks storable. An arm that can never fire is a
mistake rather than a narrowing.

**A value the parameter's type cannot take.** The ordinary check, against the declared type of the
parameter rather than of any field.

**Seen from the read side**, the part is `ArmMatch::Fixed` rather than `Field` or `Open`, and the
distinction is not cosmetic: `Open` means a writer ranges over that part, and an arm that fixed it
must not be told to. An arm doing both, narrowing the event's own field *and* fixing the part,
records the fixed value, because that is what the row is written at and the field filter is already
in the predicate that narrows to it.

## 14. An arm may compute a key part from the record

Rule 13 writes a key part the event does not carry, from a value fixed before anything is read.
This writes one the event does not carry **and could not**, because it is a function of the record:

```hek
fn month_of(at: Timestamp) -> String {
  return "{at.year()}-{at.month().pad(2)}"
}

fold UsageCounted(shop_id: Shop, month: String) -> n: Int = 0 {
  on @usage.recorded(shop_id) { at, units } key month = month_of(at) => n + units
}
```

That is a read model keyed by shop and month, from a log that records neither a month nor a bucket.
The other cases are a normalised value (a lowercased email, a trimmed code) and a combination of
fields, which is why two projectors of the ported application stayed projectors: they key on a
concatenated string and nothing could express it.

**It is a clause of its own, and the reason is when it resolves.** Everything in the filter list
resolves **once**, before any event is folded, which is what lets a field conjunct become a
`Predicate` and a rule 13 conjunct a gate. This is evaluated **per record**, so it reads the
destructure's binds and the envelope binding, and the parser reaches the filter list before that
scope exists. Spelling them alike and telling them apart by whether the right side happened to
touch a field would be a distinction nobody could see.

```
on @usage.recorded(shop_id) { at, units } key month = month_of(at) => n + units
                                          ^^^^^^^^^^^^^^^^^^^^^^^^
```

`key` is soft, an ordinary identifier the parser claims only in this position, so nothing else in
the language loses the name. Several parts are a comma list.

**A fold with one is query-only**, exactly as rule 12's envelope binding makes one, and for a
sharper version of the same reason. A command reading a fold turns its slices into a log query and
an append condition, and both are field equality on the event; `month_of(e.at) == m` has no
predicate form at all, so a command could neither narrow the log to those events nor detect a
conflicting write. A test reads one through `fold` or `fill`, and a deployment serves it from the
table. The refusal names the part rather than the clause:

> `UsageCounted` computes the key part `month`, so it can be queried but not folded by a command

**The stored side has no such problem**, which is the whole asymmetry this rule rests on. The sink
walks the log forward one record at a time, so it evaluates the expression against each record and
writes the row at the answer. No predicate is needed because nothing is being narrowed.

**The two directions are one expression read two ways**, which is rule 13's gate one level down. A
read by key has the caller's argument in the parameter's slot, so it computes per record and
**compares**, skipping a record that belongs to another row; the sink has no argument, so what it
computes **is** the key, and it binds the parameter from that. One of the two therefore scans,
which is what a by-key read of a fold already does.

**A computed part and the key are kept apart, and in both directions.** The part reads the record:
the destructure's binds, the envelope binding, `const`s and pure `fn`s of those. Not a parameter,
not even one this same arm pins some other way, and not the accumulator. And no filter of the arm
may read a part the arm computes.

That is not a conservative line, it is the exact one, and each half is a driver that could not
otherwise answer:

- The sink evaluates the expression to decide which row the record belongs to, so it runs *before*
  there is a key to bind the parameters from and *before* there is a row to read the accumulator out
  of. A by-key read has both in its frame and answers happily, which is how the hole stayed open:
  `key bucket = n` and `key bucket = tag.len()` both checked clean and then failed a fill with
  `slot read before it was set`, a message about the interpreter rather than about the program.
- A filter resolves **once** for the whole slice, against the key its caller asked for, and a
  computed part has no such key. `(label: month)` writes the part twice over, since a filter pins a
  part by its *value* rather than by its name; `(label: month.upper())` pins nothing, so the column
  stayed storable while the filter narrowed the log to the records whose field already equalled what
  the part computed, dropping every other one from the read model with nothing to say so.

A third refusal falls out of the second and belongs with it: in an arm that computes a part, **a
filter may not read a parameter the arm leaves open** either. An open parameter is a range over
every key the table holds, so such a conjunct has one value per key the store hands back, and the
key the range is narrowed with is what the computed part is being evaluated to produce.

**That is what lets the sink check the arm's narrowing first**, which it has to: a by-key read skips
a record its predicate rejects and never runs the expression at all, so `key rate = 100 / units`
under `(kind: "paid")` must not fail a replay on a free record with no units. Between them the two
refusals leave every conjunct of such an arm reading only parameters the arm *pins*, which are on
the record or fixed by the arm, so the **whole** predicate resolves before anything is evaluated.
Checking only the conjuncts that read nothing was the first attempt and it was not enough: a second
filter reading an already-pinned parameter is a real narrowing conjunct, and skipping it left the
division running on records the arm never folded.

**A computed part may not be sealed content**, and this is the one position inside an arm where that
is so. Rule 7 lets an arm fold sealed content into a sealed column, which is why the ordinary seal
check stands down inside a fold; a key is different, because erasure empties a column and cannot
reach a primary key, so a row filed under decrypted content would be personal data nothing could
erase. It also could not run: the seal is a `Value::Sealed` at the sink, which will not reduce to a
key, and a by-key read compares the plaintext it was asked for against the ciphertext and folds
nothing. `--tables` called the column stored, `fill` raised `malformed ir`, and `fold` answered the
seed. The fix is rule 7 itself: return the sealed value as a column of its own.

The check is the ordinary seal check with the fold's stand-down lifted for this one clause, so it
runs at **every position inside the expression** rather than over its result. Over the result alone
a `fn` launders the subject away: `key bucket = tag(email)` through `fn tag(s: String) -> String`
checked clean and left the hole exactly where it was.

Binding the parts an arm pins from a field first, and computing the rest against them, is a real
feature and a different one. It would make what a part may read depend on what else the arm pinned
and on the order two computed parts were written in, where this rule is one sentence, and none of
the cases that asked for computed keys reads a key part.

Eight things are refused:

**A part of a subject type.** A subject's ids are minted and filed by the key store, so a computed
one would address a row under a key nothing issued while `KeyPart::subject` reported it as that
subject all the same. Compute a part of a plain type beside it.

**An inline fold's arm.** It is not stored, so there is no key to write a part of, which is rule
13's reason. A guard's slice has no destructure and no `=>`, so the clause cannot be written there
at all.

**A name that is not a parameter.** `key` computes a key part; a value the arm wants and the key
does not is an ordinary expression in the arm.

**A part already written in that arm** by a rule 13 conjunct or by another `key` clause, for the
reason two rule 13 conjuncts on one part are refused. A **field** filter writes one too, and is
refused by the rule below rather than by this one, because a filter writes a part by its *value*:
`(month: 3)` narrows an event field that happens to be spelled like a parameter and pins nothing, so
comparing names refused an ordinary program.

**A read of the key or the accumulator**, which is the rule above:

> `` `tag` is a part of the key this arm is computing, so a computed key part cannot read it ``

**A filter reading a part the arm computes**, and **a filter reading a part the arm leaves open**,
which are the same rule the other way:

> `` `month` is a key part this arm computes, so the filter on `label` cannot read it ``

> the filter on `label` reads `order_id`, which no conjunct of this arm pins

**A part whose value is sealed content**, which is `seal-boundary` rather than
`fold-restriction`, because it is the decrypt boundary and not the key derivation:

> `` `bucket` is a key part, and this is content sealed under `Customer` ``

**Seen from the read side**, the part is `ArmMatch::Computed` and it counts as **pinned**: one
record writes one key, which is not a range, so a host deriving range indexes needs to know nothing
about this state. A fan-out, one record writing every month between two dates, is a different
feature and not this one.

## 15. A key part may be read through another fold

Rule 14 computes a part from the record. This computes one from **another named fold**, as it stood
when the record being folded was appended:

```hek
fold GenreUnits(genre: Genre) -> n: Int = 0 {
  on @game.purchased() { game_id } key genre = GameGenre { game_id } => n + 1
  on @game.refunded() { receipt_id }
    key genre = GameGenre { game_id: ReceiptGame { receipt_id } } => n - 1
}
```

**What it buys is events that stop carrying other entities' attributes.** Before it, a key part had
to be on the event, so events copied values across just so a read model could group by them: in the
ported application `@game.purchased` carried `developer_id`, `genre` and `country` copied from three
folds, and the command folded all three *only* to stamp them on the event, which widened its append
condition with slices the decision never read. `@game.refunded` repeated the whole receipt so every
total could subtract the sale, and its command declared twelve folds mostly to copy their values
out. With this it is `{ receipt_id, reason, playtime_minutes, achievements, refunded_at }`, and a
new "sales by region" total is one new fold filled from the whole log rather than a new event field
with no history behind it.

**The spelling is the one a command reads a fold with**, deliberately: `GameGenre { game_id }` means
the same thing in both places, parens to declare and braces to use, and the bare-name shorthand
works. To the grammar it is a `record_literal` either way, so there is no new rule in
`tree-sitter-hek` and the parser decides, which it may because a lookup is reachable in exactly one
position.

### As of the record, and strictly before it

The value is the looked-up fold over the log `[0, p)` for a record at position `p`, which is what a
command appending that record would have read.

**Not the current value**, so a replay is deterministic and a sale counts towards the genre the game
had when it sold. **And not including the record itself**, which is the half worth proving:

```hek
fold GenreListings(genre: String) -> n: Int = 0 {
  on @game.listed() { game_id } key genre = GameGenre { game_id } => n + 1
}
```

where `@game.listed` is what `GameGenre` folds too, so the record moves the fold it is counted
through. Strictly before, the first listing counts under the seed. The sink has no choice about it:
it works out every key for a record before it writes any of them, so the column it reads holds the
value from before that record, and a by-key read that included it would answer differently. One
expression read two ways has to answer alike in both, so the position is settled by the driver that
cannot bend rather than by taste.

**A lookup can only read what an earlier record established**, which follows and is the first thing
to get wrong. Keying a refund through `ReceiptGame { receipt_id }` works because a receipt is
created by the *purchase*; a `ReceiptGame` folding `@game.refunded` would be reading the refund
itself and would answer its seed.

**And a refund keyed through a fold that still moves decrements where the thing is now.** The
example above subtracts from the genre the game has at the refund, not the genre it was sold under.
That is this rule applied twice rather than a defect, and the fix is modelling: key through a fold
that captures the sale, so what the lookup answers cannot move afterwards.

### Query-only, and what may be looked up

**A fold with a lookup is query-only**, as rule 12's envelope binding and rule 14's computed part
make one, and for the sharpest version of the same reason: a lookup's boundary is *another key's
whole history*, which no append condition can express. There is nothing to narrow the log by and
nothing to conflict on. The refusal names the lookup rather than the part, because it is why the
part cannot be had without reading the log:

> `GenreUnits` keys through `GameGenre`, so it can be queried but not folded by a command

**Only a stored fold may be looked up**, because the sink answers a lookup from the looked-up fold's
own column. A fold whose key no event supplies has no column, and `hek check --tables` lists it:

> `OrdersSince` is not stored, so a key cannot be read through it: no event supplies `at`, so
> nothing keeps a column for it

**A cycle is refused**, including a fold looking itself up, with the message a guard's gets:

> `GenreUnits` keys through `GameGenre` keys through `GenreUnits`: a fold keyed through itself has
> no value to key by

**A sealed result may not be a key part**, which is rule 14's argument one step along: a key is
plaintext and an erase cannot reach one, so a row filed under decrypted content would hold personal
data nothing could erase. Refused on the **declared return type**, which is where a fold publishes
its seal (rule 7), so unlike a computed part there is no `fn` to launder it through.

**And the result's type has to fit the part**, which is the ordinary check: the lookup lowers to a
slot carrying the looked-up fold's declared return type. An optional result in a non-optional part
is refused like any other optional, and the author writes the fallback. A `none` that silently
skipped the record would hide a modelling mistake.

**The arguments are held to rule 14's rule**, and they have to be asked for separately: the lookup
leaves a slot load behind and keeps its arguments off the expression tree, so the walk over the key
expression sees nothing of them. That matters because the shape it lets through is the most natural
one there is, passing the key part the arm already pins:

```hek
// Refused: `shop_id` is a part of the key this arm is computing.
on @game.purchased(shop_id) { game_id } key genre = GameGenre { shop_id, game_id } => n + 1
// Written: the field is destructured, so the lookup reads the record.
on @game.purchased(shop_id) { shop_id, game_id } key genre = GameGenre { shop_id, game_id } => n + 1
```

The filter still narrows and the arm still pins the part from it; what changes is that `shop_id`
inside the clause is the record's field rather than the key's part, which is what the sink has.

**And a lookup may not be written under a binder.** It is hoisted out of the expression onto the arm
and evaluated once per record, which is what lets one chain into another, so a comprehension's name
does not exist yet at that point: `key p = [One { x: i } for i in ids].len()` checked clean and
failed the fill. Refused rather than left in place, because leaving it there would mean a read of
the log per element.

### Seen from the read side

`hek check --tables` prints the dependency, so a reader can see why editing one column refolds
another:

```
  TABLE genre  (genre: Genre)  1 column
    genre_units: Int  <- @game.purchased, @game.refunded  (reads game_genre, receipt_game)
```

Transitive, so a chain shows every link. `Column::reads` is that list, and three things read it:

- **`hek digest`**, because editing a looked-up fold changes the dependent's values. A command's
  entry already covers the folds it reads, since the splice puts them physically inside it; a lookup
  is not spliced, so the dependent's form carries a hash of each fold it reaches instead. A moved
  **signature** of a looked-up fold moves the dependent's `hash` and not its `signature_hash`: the
  dependent's own table and type have not changed, so it is a column to refold in place.
- **`Tables::only`**, because a rebuild of a dependent needs its dependencies' values *as of each
  past record*, which only maintaining them alongside can give. `Layout::closure` adds them for the
  same reason it adds a range's minters, and the two feed each other.
- **a host**, which is told what to replay together rather than working it out.

**What it costs is the by-key read.** `Interpreter::fold` and the `fold` test action answer a lookup
fold by evaluating the lookup at every candidate record's position, which is a fold of the
looked-up fold per record: quadratic, and accepted. A deployment serves these from the table and
never takes that path on a request, and a test is a handful of records.

## 16. An arm's value may read a fold too

Rule 15 reads a fold for a **key part**. This reads one for the **value**, which is the other half
of the same construct and is written the same way:

```hek
fold TestOrder(shop_id: Shop, order_id: Int) -> Bool = false {
  on @order.placed(shop_id, order_id) { test_order } => test_order
}

fold EditsRequested(shop_id: Shop) -> n: Int = 0 {
  on @edit.requested(shop_id) { shop_id, order_id }
    => if (TestOrder { shop_id, order_id }) { n } else { n + 1 }
}
```

**What it buys is a fold that can ask about another key, which rule 1 otherwise forbids.** A fold is
one value out, so a counter that has to leave some records out had no way to know which: the ported
application's statistics kept a `Map(Int, Bool)` **membership set** on the counter's own row, written
by every event that could place a test order, purely so twenty counters could read it in a value
position. That is a sibling column, and rule 1's "one value out" is exactly what stopped a fold
reading one. With this the per-order fact is its own fold and the counters look it up, which is why
that projector was the last one the port could not convert.

**Everything about rule 15 holds unchanged**: as of the record and strictly before it, query-only,
only a stored fold, cycles refused, the result typed by the looked-up fold's declared return type,
transitive in `Column::reads` and in the digest, and replayed alongside by `Tables::only`. The two
rules are one feature with one list of lookups per arm, and the only question this one settles is
*where* the value may be read.

### What differs, and it is two things

**A sealed result is allowed here**, where rule 15 refuses one. A key is plaintext and an erase
cannot reach one; an arm's value lands in a **column**, which `docs/projectors.md` rule 9 makes an
erase empty. So a looked-up seal is ordinary sealed content and rule 7 takes it from there: the
slot carries the declared `Sealed(T, Subject)`, so the arm's value has that type and the fold has to
declare it:

> `ShippedTo` folds content sealed under `Customer` on @order.shipped, and its return type does not
> say so

That is rule 7's existing refusal, naming the type to write. Nothing was added for this case, which
is the argument for it: a looked-up seal is a seal like any other once it is not a key.

**And the positions are decided by where a record is in hand.** An arm's value and a `key` clause
both have one. These do not:

- **a slice filter**, which resolves once for the whole slice against the key its caller asked for,
  and has no record at all. Refused structurally: a filter is parsed before the list that collects a
  lookup exists, so the name is not a value there.
- **a seed**, for the same reason one place further out.
- **an inline fold's arm**, which rule 14 refuses a `key` clause in for the reason that applies here
  too: it is not stored, so it is inside the very command that could not then read it.

**An `if` header still reads `Name {` as a block**, which is `no_record_literal`'s existing rule and
not this one's: `if (TestOrder { .. })` takes parentheses exactly as a record literal there does.

**And a named fold may not share a name with a record or an enum.** A fold is spelled like a type
(rule 1), so it resolves like one: `Name { .. }` reads the fold inside a fold arm and builds the
record everywhere else, and one name meaning both would make one declaration read two ways, with its
seed a record literal and its arm a lookup. Refused where the fold is declared, which is the same
argument a `subject` is already held to for the same two kinds, and refused there rather than at a
use site because a use site is not where the author can fix it.

### Seen from the read side

Nothing new. `--tables` prints `(reads test_order)` against the column, which is why it says *reads*
rather than *keyed through*: a fold may be read for a key part or for a value, and the dependency a
digest records and a rebuild replays is the same either way.

The one place the two differ inside the sink is **where the value is used**. A key lookup is read and
used in the same phase, the one before any write; a value lookup is read there and used in the phase
that writes, so its value is carried across rather than read again. Reading it again would be reading
it *after* the record, since by then the looked-up column may be one of the things this record has
written, and `tests/sink.rs` fails on exactly that if the carrying is removed.

## Seen from the read side

Not a rule, because it constrains nothing about writing a fold: `hek check --tables`
derives a read model from the declarations in which every named fold is a column, its
parameters are the key, and the folds that share a key share a table. A parameter no arm
supplies is not a key anything could be written under, so a fold with one is listed as not
stored. That is a listing and not a diagnostic; the language does not require a fold to be
storable, and a fold a command reads is doing its job either way. `docs/cli.md` has the
listing and what orders a key.
