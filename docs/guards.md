# Guards

A `guard` is a named proposition about the log, and one refusal for when it does not hold:

```
refusal UndefinedCourse "no such course"

guard CourseIsDefined(course: String) {
  state defined: Bool = fold false
    on @course.defined(course) => true

  if !defined {
    return reject UndefinedCourse
  }
}

command Subscribe(student: String, course: String) {
  guard CourseIsDefined { course }
  guard StudentIsRegistered { student }
  guard NotSubscribed { student, course }
  guard CourseHasSeats { course }
  guard UnderCourseLimit { student }

  emit @student.subscribed { course, student }
}
```

This document is the contract. `tests/guards.rs` is the same set of rules as executable tests.
Change the doc, the tests and the code together.

The gap it closes is the one duplication heklang forced and no test could catch. A `state` fold
declares the read boundary and the conflict boundary together (`docs/commands.md`), so it cannot be
factored into a `fn` the way a decision can. Two independent applications wrote the same folds over
and over: one repeats `state shop: Bool = fold false / on @shop.connected(shop_id) => true`
**thirteen times byte-identical**, each followed by its own copy of the rejection; the other gives
two commands six identical folds and a shared six-argument `fn` whose three `Bool`s and three `Int`s
transpose without a type error. What drifts when those fall out of step is the append condition, and
`docs/testing.md` §8 keeps that out of a test on purpose.

---

## 1. Declaring

```
guard <Name>(<name>: <Type>, ...) {
  <state>*
  <guard>*
  <let>*
  <statement>*
}
```

A command body with no `emit`. Parameters in, folds, and a decision; **falling off the end means it
holds**. There is no return type, because a guard either refuses or it does not.

**A guard names a proposition, not an entity.** `CourseIsDefined`, `UserHasRegistered`,
`ShopIsConnected`. Not `Course`, `User`, `Shop`.

This is the rule that keeps heklang from growing aggregates, which is the thing a Dynamic
Consistency Boundary exists to avoid, and the first instinct with DDD in hand is to break it. A
named, parameterized fold group called `Warranty` is an aggregate in everything but name. What keeps
it from being one is that a command stacks several and emits across all of them, with the condition
as the union: **a command's boundary is the union of the boundaries it guards.** An aggregate is one
per command and the unit of writing. Naming guards as propositions is what makes stacking the
obvious thing to write.

It has a second effect, and it is why the rule is here rather than in a style note. A proposition
holds one or two folds and one refusal, so the ladder cannot live inside the declaration. It moves
to the call site, where its order is on the page.

## 2. Using

```
guard <Name> { <name>: <value>, ... }
```

Braces with the bare-name shorthand, the same block `emit`, `put`, `invoke` and a record literal
take, so `guard CourseIsDefined { course }` is `{ course: course }`.

**One word for the declaration and the use.** Parens declare and braces use, which is how heklang
already separates `command Foo(...)` from `invoke Foo { ... }`. The alternative was a second
keyword for a second name for one concept.

`guard` keeps its older shape too, for the case no proposition can express: **raw slices**.

```
guard @order.placed(order_id), @order.cancelled(order_id)
```

That adds slices to the boundary and binds nothing, which is right when the decision needs a slice
in the condition that nothing folds. The token after `guard` decides which shape it is: a path is
raw slices, a name is a guard.

## 3. The order on the page is the precedence

The guards run in the order they are written, each before the body, and the first that refuses is
the command's outcome. That is the whole reason the ladder moved out of the declaration: five
`guard` lines in precedence order say what a five-rung `if` ladder said, in the place a reader looks
for it.

`docs/commands.md`'s execution order gains one step, between the fold and the body:

> 7. **the guards' decisions** run, in the order the guards are written.

A guard never straddles two phases. Its folds are `state` folds and run at step 6 with every other;
its decision runs at step 7; the body it guards runs at step 8. That is why a `guard` is a
declaration rather than a statement, and why it cannot be written among statements.

## 4. A guard's slices are the command's boundary

This is the reason a guard is a declaration rather than a `fn`. What a command folded is what it
conflicts on, and a guard folds on the command's behalf, so its slices arrive in the
`AppendCondition` exactly as if they had been written inline:

```
command Subscribe(student: String, course: String) {
  guard CourseIsDefined { course }      // @course.defined narrowed to this course
  guard StudentIsRegistered { student } // @student.registered narrowed to this student
  ...
```

The filters resolve against the arguments the caller passed, so the predicate names the course the
command was called for and not the parameter the guard declared.

**Nothing is deduplicated.** Guarding the same proposition through two paths costs one extra
accumulator over slices that were read anyway, and its decision is the same decision twice. Two
identical predicates in a condition are two identical OR-terms. What is *not* allowed is naming the
same guard twice on the same arguments in one declaration, which is rule 7.

## 5. Guards compose

A guard may guard another guard, to any depth:

```
guard PlanExists(plan_id: Uuid, shop_id: Int) {
  guard ShopIsConnected { shop_id }

  state exists: Bool = fold false
    on @plan.created(plan_id, shop_id) => true

  if !exists {
    return reject PlanNotFound
  }
}
```

Without this, `ShopIsConnected`'s two lines get rebuilt inside every guard that needs a shop, which
is the duplication this construct exists to remove.

**The cost is that the boundary stops being readable off the page.** A command's append condition is
now the transitive closure of what it guards, so a fold added three levels down widens the boundary
of every command above it and gives them more contention, invisibly. `hek check --boundaries` prints
the closure, which is what pays for it:

```
  ArchivePlan guards PlanExists, ShopIsConnected
```

`ShopIsConnected` is two levels down and in the condition, and that line is the only place it shows.
The listing is also the only way to compare two commands' boundaries, since a test cannot ask.

**Asked for rather than printed**, because it is an enumeration rather than a summary, and for a
program whose guards do not nest it restates the `guard` lines a reader just read.

**A guard is copied, not called.** `src/inline.rs` splices it into whatever names it, before the
interpreter sees either, so a command reaches the fold with one arena, one frame, one slice list and
**one read of the log** however many guards it names. Composition costs nothing at runtime, and
`src/interp.rs` does not know the word.

## 6. What a guard may not do

| | why |
| --- | --- |
| `emit` | a guard decides whether a command may run; the command appends |
| `put`, `patch`, `update`, `delete` | a guard reads |
| `invoke`, `http.*`, `reveal`, `erase`, `fail`, `log` | an effect's, and a guard runs inside a command |
| `now()` | a guard decides from the log; take the moment as a parameter |
| a bare `return`, or `return <value>` | see below |
| fold nothing | a decision made from arguments alone is a `fn` |

**A guard returns only a refusal.** `return reject <Name>` and `return invalid(...)`, and nothing
else. A guard is spliced into the command that names it, where a bare `return` would read as *the
command succeeded and appended nothing*, which is the opposite of what the author wrote:

> a guard holds by reaching its end, so this `return` says nothing; write `return reject <Name>` or
> `return invalid(...)`, or delete it

So an early exit meaning "this holds" is spelled by not writing one: `if !defined { return reject
(...) }` rather than `if defined { return }`.

**A guard refuses; an idempotent no-op is not a guard.** That is the line between what belongs in
one and what stays inline. `CancelWarranty` guards the shop and the sale, and keeps its own fold
for the case that returns `ok`:

```
command CancelWarranty(warranty_id: Uuid, shop_id: Int) {
  guard ShopIsConnected { shop_id }
  guard WarrantyIsSold { warranty_id, shop_id }

  state cancelled: Bool = fold false
    on @warranty.cancelled(warranty_id, shop_id) => true
  if cancelled {
    return
  }

  emit @warranty.cancelled { warranty_id, shop_id }
}
```

**A guard binds nothing into its caller.** Its states are its own, so a `guard CourseHasSeats`
gives the command no `seats`. A caller that wants a value folds it inline; the slice is already in
the boundary, so the second fold costs an accumulator and nothing else. Handing values back would
make a guard an object with fields, which is the aggregate rule 1 is about.

## 7. Naming a guard

**Every parameter, once.** The same rule `emit`, `given` and a record literal hold to, checked at
the call against the declaration.

**A guard may not name itself, directly or through another.** A guard is copied into what names it,
so a cycle is a copy with no end. The arguments are not consulted and could not be: the splice
happens before anything is evaluated, so `guard A { n: n + 1 }` inside `A` does not terminate
either.

> `A` guards `B` guards `A`: a guard is copied into what names it, so one that names itself has no
> end

**The same guard on the same arguments is refused.** It decides what it already decided, so the
second one does nothing:

> guard `G` is named twice on the same arguments; the second one decides what the first already
> decided; delete it, or give it the arguments you meant

Compared **as written** rather than as IR, because two spellings of one value are two questions to a
reader and only an identical one is certainly a slip. So `guard G { course: a }` beside
`guard G { course: b }` is two questions and stays legal.

This is a *direct* duplicate in one declaration. Reaching the same guard twice through composition
is allowed and silent: a caller often cannot know what the guards it names reach, and rule 5 is the
point of the construct. `hek check` shows each one once.

**An argument may not read a `state`.** An argument becomes a prologue assignment, which runs before
any fold, so it would read the other state's seed rather than what it folds to. Exactly the mistake
a seed makes (`docs/commands.md`), rejected the same way and for the same reason: the answer would
be wrong rather than late.

> `course` is taken from `seen`, which has not folded yet

## 8. Where this diverges from the runtime

Nothing, because a guard does not reach the runtime. It is a parse-time construct: `src/inline.rs`
splices every guard into every command that names one before `Program` is finished, so a host, the
interpreter and `docs/host.md`'s traits see a command with the folds written out. There is no
`Guard` in the append condition, in the journal, or in a trace.

One thing that follows, and is a known limit: a **runtime** error raised inside a spliced guard body
carries the guard's span but is reported against the command's module, because a `Span` is a
position pair with no file. Parse-time diagnostics are unaffected, since a guard's body is checked
once, in its own module. Fixing it means putting a module on every span, which is a change to every
diagnostic in the language for a case that only arises when a program is already broken.
