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

**A read closes the stage's prologue, and a `guard` does not.** This is the one place the two
constructs behave differently at a call site, and it is deliberate. A statement written below a read
goes into the stage's second half, so it runs after the fold and sees the value; a statement below a
`guard` goes into the *first* half and runs before the guard decides, which is heklang's
guard-prologue bug and is older than this construct. A fold could not be left that way, because a
statement below a read is normally the one that uses it, and it would have read a slot the splice
assigns later.

The asymmetry has a second reason to stay: making a `guard` close the prologue too would change the
lowering of every existing guard-only command, and therefore its digest hash. Fixing the guard bug
is a change to `guard`, with its own hash bump, and belongs with the rest of that rule rather than
here.

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
`dry_run` in scope and the question cannot be asked at all. The inline form keeps the same wart,
because there the name is load-bearing and cannot be dropped.

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

**Not implemented yet.** The construct landed without it, so nothing currently reports a duplicated
inline fold. Everything else in this rule holds: inline folds are kept, naming is never forced, and
promotion stays a header edit. Until the check exists, the 39% this section is about is found by
reading rather than by `hek check`.

**The trigger has no false positives by construction.** Byte-identical folds cannot differ in
meaning. It fires 167 times on the port, and the near-miss pairs that look alike all survive it:
the FR-11 window snapshot (`@edit.window.opened`, seeded `false`) and the live policy
(`@edit.policy.configured`, seeded `true`) differ in both event and seed.

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
| fold nothing | a value computed from arguments alone is a `fn` | `empty-declaration` |

**An arm is the one place a message is needed**, because an arm is an ordinary expression and
`now()` and `reveal` are ordinary calls in it. Both are already refused there for the inline fold,
by the same `fold-restriction` check and for the same reason, so a named fold inherits them rather
than adding anything.

`empty-declaration` has two spellings and the sharper one wins. A body with no arms is
"fold `F` has no arms, so it is only its seed", which is about the declaration; `folds_nothing` is
the backstop and reports "fold `F` folds nothing".

## 11. Where this diverges from the runtime

Nothing, because a named fold does not reach the runtime. Like a guard it is a parse-time construct
that `src/inline.rs` splices out before `Program` is finished, so a host, the interpreter and
`docs/host.md`'s traits see a command with its folds written out. There is no named fold in the
append condition, in the journal or in a trace.

**The digest gains nothing, and the hash does not move.** `src/digest.rs` gives the rule: `const`,
`refusal` and `guard` are absent from the digest form *because the parser inlines all three, so a
declaration of any of them runs nothing and its content is already at every use site*. A named fold
is spliced identically, so it is the fourth. `docs/digest.md` rule 7 gains one word and nothing
else, `VERSION` stays where it is, and a program with no named folds hashes exactly as it did.

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
