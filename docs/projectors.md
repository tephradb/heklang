# Projectors

A projector is the read side: a pure fold over the event log into **entities**, which are read-model
rows. Where a command reads the log to decide whether to append, a projector reads the log to
maintain a shape that is cheap to query. A read model is a rebuildable cache, never a source of
truth, so everything here is arranged so that replaying the log from position 0 reproduces the same
rows.

This document is the contract. `tests/projectors.rs` is the same set of rules as executable tests,
one test per numbered rule. Change the doc, the tests and the code together.

## Shape

```
projector Orders {
  enum Status { @default Placed, Shipped, Cancelled }

  entity Order {
    order_id: Uuid @key,
    customer_id: Int @index,
    email: String @max(200),
    total: Money(2),
    status: Status,
    tracking: String?,
    placed_at: Timestamp?,

    index (customer_id, status)
  }

  on @order.placed as e { order_id, customer_id, email, total } {
    put Order {
      order_id, customer_id, email, total,
      status: Placed,
      tracking: none,
      placed_at: e.at,
    }
    patch Customer[customer_id] {
      order_count: .order_count + 1,
      lifetime_spend: .lifetime_spend + total,
    }
  }
}
```

Entities and enums are **projector-scoped**. Two projectors may each declare a `Customer` and the two
are unrelated. This matches the runtime, where each projector owns its own database and collects its
entities from its own module scope, so a shared entity is not expressible there either.

---

## 1. Handler form

`on @event.path [as name] [{ destructure }] { body }`. One or two adjacent brace blocks: with two,
the first destructures payload fields into slots and the second is the body; with one, there is
nothing to destructure and the block is the body. Which form a handler is in is decided by whether a
block follows the first one, and no statement can begin with `{`, so this is unambiguous.

An effect arm has exactly the same shape (`docs/effects.md`, rule 1). The two kinds share one
construct rather than each having a slightly different one.

`as name` is optional and binds the envelope. Through it: `.at` (the append timestamp), `.id` (the
event's stable identity) and `.position` (its position in the log), plus payload access, so `e.total`
reads the payload field whether or not it was destructured.

There is no implicit binding in scope. A handler with no `as` clause has no way to reach the
envelope, by design: the envelope is available exactly when the author asked for it by name.

## 2. Statements

| Statement | Meaning |
| --- | --- |
| `put Entity { ... }` | writes all fields |
| `patch Entity[key] { ... }` | writes the listed fields, materializing the row if it is absent |
| `update Entity[key] { ... }` | writes the listed fields, dropping the write if the row is absent |
| `delete Entity[key]` | removes the row |

All four are unconditional write instructions from the language's point of view. They cannot fail in
a way the program observes, so they return nothing and there is no error an author can catch.

`put` requires every declared field to be present. `patch` and `update` write only what is listed and
leave the rest of the row alone; they differ only in what an absent row means, which is rule 5.
`delete` removes the row without recording that it did, which is not the same as erasing anything;
see "`delete` is not a tombstone" under rule 5.

A bare `T` written into a `T?` field wraps, so `tracking` destructured as a `String` satisfies a
`String?` column and `e.at` satisfies a `Timestamp?` one. This is not a new rule: it is the coercion
every declared position applies, and `docs/optionals.md` lists them all. It only wraps an exact
inner-type match, so a `Uuid` still does not satisfy a `String?`.

## 3. Stored-value reference

Inside a `patch` or `update` value expression, a leading dot means the **current stored value** of
that field:

```
patch Customer[customer_id] { order_count: .order_count + 1 }
```

A bare name always means a local binding (a destructured field, the `as` binding, or a `let`), never
a stored value, even when no local shadows it. This is the important half of the rule: adding a field
to an event must never silently change the meaning of a projector body. If a bare name could fall
back to the stored value, then declaring a new event field named `order_count` would quietly rebind
every `order_count` in every handler that destructures it.

`.field` is legal only in a `patch` or `update` value position. Not in `put`, which writes a whole
row and has no prior value to read, and not in filters.

Inside an `update` the story is cleaner than inside a `patch`: the statement only proceeds when the
row exists, so a stored load is always filled from a real row and the zero table of rule 5 is not
consulted at all on this path.

**Rejected: naming the row.** `patch C[k] as c { n: c.n + 1 }` is consistent with `as e` on the
handler, but it is wordier and reads oddly for a row that is being written rather than read. The
leading dot costs one character and carries the same information.

### How it lowers

Each distinct `.field` in one `patch` or `update` gets a frame slot. The interpreter fills those
slots from the stored row before evaluating any of the write's value expressions, so `.order_count`
is an ordinary slot load by the time the expression layer sees it. That is why
`.lifetime_spend + total` cross-hints its literals exactly like any other typed load, and why the
expression evaluator needs no new node for this feature. It is also why "only in a value position of
one of those two" is structural rather than a check: nowhere else allocates a stored load.

## 4. No general reads

A handler cannot read entity state except through rule 3. It cannot read a different entity, cannot
read a different row, and cannot branch on stored state, with the one narrow exception `update`
carves out below.

This keeps a projector a pure fold over the event log, so rebuild determinism is structural rather
than a rule authors can violate.

**Rejected: a general `get(entity, key)`.** This is not hypothetical. hekla has exactly that today,
reading any row through the current batch's uncommitted writes. It is more expressive, and it makes
read-modify-write across entities possible. It lost because with it, whether a projector rebuilds to
the same rows depends on the handler bodies being written carefully; without it, that property holds
by construction. Rule 3 covers the read-modify-write case that actually comes up, which is
incrementing a counter on the row being written.

### What `update` reads, and why it is not a read

`update` does look at stored state: at whether the row is there. The carve-out is that **no value
derived from that look reaches the program.** The statement returns nothing (rule 2), so the one bit
decides whether a write lands and nothing else, and a row's presence at position N is a pure function
of events 0 to N, so a rebuild from position 0 takes the same branch and produces the same rows.

What rule 4 exists to prevent is a *value* out of the store shaping a decision the author wrote,
because that is what makes rebuild determinism depend on how carefully a body is written. One
unobservable bit does not. It is the same argument that already licenses rule 3, which reads a stored
value outright.

## 5. Zero values

Every entity field has a well-defined initial value, so a `patch` against a missing key can always
materialize the row.

| Type | Zero |
| --- | --- |
| `Bool` | `false` |
| `Int` | `0` |
| `Decimal(n)` | `0` at scale `n` |
| `Money(n)` | `0` at scale `n` |
| `String` | `""` |
| `T?` | `none` |
| an enum | its `@default` variant (rule 6) |
| `Uuid` | none: no zero exists |
| `Timestamp` | none: no zero exists |

`= <literal>` on a field overrides the zero. An **optional field takes no default**, because it
already has one: `none`. Anything else would need a literal for `some(x)`, which the language has no
way to write, so the restriction costs nothing and is additive to lift.

`Uuid` and `Timestamp` have no zero because the nil UUID and epoch zero are real values that get
mistaken for data. A row that materialized with `00000000-0000-0000-0000-000000000000` looks like a
row that was written, and a `placed_at` of 1970-01-01 sorts to the top of every query. So a non-key
field of either type must be optional or carry an explicit default, and anything else is a compile
error naming the field and both fixes.

`@key` fields need no default, because the subscript supplies them.

**Rejected: the nil UUID and epoch zero.** They make the zero table total and remove a compile error,
at the cost of making "absent" and "present but unset" indistinguishable in the data. The compile
error is the cheaper of the two.

### Choosing between `patch` and `update`

The zero table is what makes `patch` total, and `update` is the statement that declines it. Which one
a write wants follows from the entity it writes rather than from the handler or the event, and the
question is one sentence:

> What does an absent row mean for this entity?

- Absent means **"zero of this thing"**: counters, totals, running sums, first-time aggregates.
  `patch` is right, and materializing from zeros is the feature rather than a fallback. `Customer`
  with an `order_count` is the case rule 5 was written for: a customer with no row yet has placed
  zero orders, which is exactly what the zero says.
- Absent means **"this thing does not exist"**: a plan, an order, a shop, anything the read model
  treats as an identity. `update` is right, and a `patch` there **manufactures a record**.

The worked example is the one that produced this rule. A merchant-facing plan catalogue:

```
on @plan.deleted { plan_id } { delete Plan[plan_id] }

on @plan.sold { plan_id, price } {
  patch Plan[plan_id] {
    total_sold: .total_sold + 1,
    revenue: .revenue + price,
  }
}
```

A sale arrives for a plan that was deleted. The `patch` materializes a `Plan` from zeros and fills in
the sale's two columns, so the catalogue now holds a row with a real id, a real revenue figure, an
empty title, a zero price and a `created_at` of `none`. It looks like a plan. Nothing failed, nothing
logged, and a rebuild reproduces it faithfully, because the row is what the handlers say to write.
Written as `update Plan[plan_id]`, the sale for a deleted plan is dropped and the catalogue stays
correct.

The original this was ported from read the row and returned early when it was missing. Rule 4 forbids
that, so `update` is how the same intent is spelled here: not "read, then decide", but "write, and
say what absent means".

**Rejected: `patch? Entity[key]`.** It makes the relationship to `patch` explicit and reserves no
word. It loses because `?` already means "optional type" everywhere else in the language, and this
would give the character a second, unrelated job: "optional write". `update` is SQL's word for
exactly this, where `UPDATE ... WHERE key = k` affects zero rows when the key is absent, so a reader
arrives with the right meaning already. It is `patch` that is the unusual one here: HTTP's PATCH on a
missing resource is a 404, not a create.

**Rejected: a property of the entity**, declared once and applied to every write. The rule above is
stated per entity, and a real port is the strongest case for it: all seventeen `patch` statements
across its three projectors write identity entities and all seventeen want `update`, so the
declaration would carry the whole decision and the call sites would carry none of it. It loses
anyway, because it makes `patch Plan[k]` unreadable without scrolling to the declaration, and the
difference between manufacturing a record and dropping a write is precisely what should be legible
where the write is. It is also not composable: an identity entity with one genuine counter column
would have no way out.

**Rejected: changing `patch`'s default** so that materializing is the opt-in. The same port evidence
argues for it, seventeen to zero. It loses because it silently changes what every projector already
written does, including this repo's own `Customer` counter, and because it trades a decision made at
the write site for one inherited from a keyword's history.

### `delete` is not a tombstone

`delete` removes the row. It does not record that the row was removed, so rule 5 then applies
unchanged to whatever arrives next: a `patch` naming that key materializes a fresh row from zeros.
A late `@order.shipped` after an `@order.purged` leaves an `Order` whose `total` is zero and whose
`placed_at` is `none`, a row in the read model for an order that no longer exists.

That is the price of rule 5, and it is paid deliberately, because the same rule is what lets a
counter increment without a read. But it has a consequence worth stating outright: **`delete` is not
an erasure mechanism.** Reaching for it to remove someone's data leaves a hollow row behind on the
next event that touches the key, which is worse than not deleting at all, because the row looks like
data rather than like a gap.

**`update` is how an author declines that price.** A handler that writes `update` rather than `patch`
leaves a deleted row deleted, because there is nothing to update. That is the half of this gap a
statement can close, and it closes it at the write site where the author already knows which they
meant.

Erasure is `hekla erase`, which drops the per-subject key and shreds the value across the log and
every read model at once, in constant time and without a rewrite. A projector `delete` is a
read-model edit and nothing more.

**Still open: what a shred should cascade to.** heklang has no way to say that a row keyed by, or
derived from, a shredded subject should itself go. `update` does not touch this: it lets a handler
decline to resurrect a row, and says nothing about which rows a shredded subject should take with it.
That still wants a tombstone the fold can see, which is a real addition to rule 4's no-reads model
rather than a small one, and it should be designed against the shred cascade as a whole rather than
bolted onto `delete`.

### Defaults and zeros agree by construction

A field default is written as a literal, and the parser resolves it against the field's declared type
at the point of parse, exactly as it resolves every other numeric literal. So `= 0` on a `Money(2)`
field becomes zero at scale 2, and `= 0` on a `Decimal(4)` field becomes zero at scale 4. Nothing unresolved reaches the IR.

That is what makes rule 5 a guarantee rather than a hope: for every field, the default (if any) and
the zero (if any) are both values of exactly the declared type, so materializing a row always
succeeds. `tests/projectors.rs` asserts it directly.

**Rejected: arbitrary constant expressions as defaults.** It would need the entity declaration to
carry its own expression arena for a feature nothing yet uses. Literals cover the cases that come up,
and relaxing this later is purely additive.

## 6. Enum defaults

An enum used as a non-optional entity field needs a `@default` variant marked in its declaration.

Silently defaulting to the first variant would make reordering variants a semantic change. Variant
order is otherwise cosmetic, and it should stay that way.

## 7. Enum literals

Variants are written bare (`status: Placed`) and resolved from the target type by the same
bidirectional inference used for numeric literals (see `docs/literal-inference.md`).

| Source | Resolves to |
| --- | --- |
| `status: Placed` where the field's type is `Status` | `Status.Placed` |
| `let s = Placed` where exactly one in-scope enum has `Placed` | that enum's variant |
| `let s = Placed` where two in-scope enums have `Placed` | error naming both candidates |
| `let s = Nope` matching no in-scope enum | `Nope` is not in scope |
| `status: Shipped` where `Status` has no `Shipped` variant | error naming `Status` |

An unqualified variant that matches no in-scope enum, or is ambiguous across two, is a compile error
naming the candidates.

Because heklang's enums are types rather than a set of allowed strings, a variant that is not in the
set cannot be written at all. The runtime's `one_of` validates membership on command input but not on
a projector write, so a bad variant there lands in the column and only fails later at bind time. That
hole cannot exist here.

**Enums are now declared at module scope as well** (`docs/declarations.md`), which closes the other
half of the same hole. While they were projector-scoped, a bad variant could not reach the read model
but could still reach the **event**, because an event field could not have an enum type at all and had
to be a `String`. The same enum on the event, on the command parameter and on the column means there
is no boundary left for a wrong value to cross. A projector's own enum still shadows a module one.

## 8. Indexes

Field-level `@index` with no name declares a single-column index. Entity-level `index (a, b)`
declares a compound one, and the order is significant.

Index naming is a storage concern and is not authored. Every index column must name a declared field.

Indexes are recorded in the IR and ignored by the interpreter, which is a test harness rather than
the real store.

## 9. Subject propagation, not declaration

Entity fields do not restate `@subject`. Subject binding is a property of the value: it propagates
from the event field, through destructuring, into the entity field the value is written to.

The propagation is live: parsing a `put` or `patch` field whose value came from a subject-bound event
field records that subject on the entity field, so `EntityField::subject` is populated without any
author writing it.

The two checks over it are not (see "Checker obligations"):

- a field written with subject-bound values from two handlers with different subjects is a conflict
  error;
- assigning a subject-bound value into a context that discards the binding is an error.

The first of those **is** implemented for the other place subjects propagate, a `state` fold
(`docs/effects.md` rule 12), where the same sentence appears about a fold arm rather than a handler.
The projector side is still a no-op, so this is a gap in the checker rather than a disagreement about
the rule.

## 10. Scoping

The same prologue-free model as command bodies: destructured names, the `as` binding, and `let`
bindings are in scope in the body. Handlers do not share state with each other, which is structural
rather than a rule: each handler has its own frame and its own expression arena.

---

## The `@max` invariant

`@max` has two failure modes, and they differ by declaration kind:

- in a **command**, an over-length value at `emit` is `Outcome::Invalid`, the runtime's validation
  channel;
- in a **projector**, it is a hard error with a line and column, because rule 2 gives a projector no
  observable failure channel to route it through.

That asymmetry is only defensible if the projector error is unreachable in a well-formed program.
The invariant that makes it so:

> An entity field's `@max` must be no tighter than the `@max` of every event field written into it.
> A field with no `@max` on the event side may not be written into an entity field that has one.

A command already rejects an over-length value at `emit`, so the only way a projector can observe one
is if the entity's constraint is tighter than the event's, which is a schema bug rather than a data
problem. That is a static check and it belongs in the checker. Until the checker exists, the runtime
error is the backstop, and a projector that trips it is reporting a mismatch between two declarations
rather than a bad event.

**`update` makes the backstop patchier, which is an argument for the checker rather than against
`update`.** A write that is dropped never evaluates its field values, so a schema mismatch inside an
`update` is reported only on the runs where the row happens to exist. The check was always meant to
be static; this makes the interim version depend on the data, which the real one will not.

## Where this diverges from the runtime

Four of these rules describe behaviour hekla does not have today. Recorded so that a future change is
an informed one rather than a rediscovery.

| Rule | heklang | hekla today |
| --- | --- | --- |
| 2, 5 | `update` drops the write when the row is absent | this is hekla's only form: its `patch` is a documented no-op on a missing row |
| 5 | `patch` materializes a missing row from zeros | no such statement; a materializing write has to be spelled as a `put` |
| 5, 6 | entity fields have zeros and defaults | no defaults anywhere; `put` requires every non-optional field, an omitted column binds NULL |
| 1 | `.position` is available through `as` | a handler sees `id`, `timestamp`, `type` and `data` only; the log position never reaches it |
| 4 | no general reads | `get(entity, key)` reads any row, through the current batch's uncommitted writes |

The first two rows are one divergence read from both ends, and the direction is worth being precise
about: **hekla's `patch` is heklang's `update`.** So the runtime does not need to learn the skipping
form, it needs to learn the materializing one, and the name it already uses means the other thing.
A port of these rules to hekla is a rename plus an addition, not a behaviour change to an existing
statement.

One representation choice also diverges. `Timestamp` here is an `i64` of epoch microseconds; the
runtime's envelope timestamp is an RFC 3339 string and its timestamp column stores text, with no
formatting or arithmetic defined on it. Both orderings agree, so this is safe, but it means a
conversion at the runtime boundary.

## What the port wrote before `update`

The FlowWarranty port recorded `patch` on an absent row as an open problem, and the workaround was
that there was none: every write went through, so a sale for a deleted plan materialized a hollow
`Plan` and the port's README carried a note saying so.

```
// before: the sale for a deleted plan materializes a hollow row
patch Plan[plan_id] { total_sold: .total_sold + 1, revenue: .revenue + price }

// after
update Plan[plan_id] { total_sold: .total_sold + 1, revenue: .revenue + price }
```

The count is the interesting part. All seventeen `patch` statements in the port become `update`,
eight in its plans projector, four in its shops projector and five in its warranties projector. Not
one of them wanted rule 5's zeros, because all three entities are identities rather than counters.
That is the evidence behind the guidance under rule 5, and behind not making it a per-entity
declaration: a rule that would have been right seventeen times out of seventeen there is still one an
author should state where the write is.

**Rejected at the time, and still rejected: making plan deletion a soft delete.** It works, and it
pushes the problem into every projector: every read of every entity has to remember to filter the
tombstone, and the one that forgets is a bug that looks exactly like this one. A rule the read model
enforces once beats a discipline every handler has to keep.

## Checker obligations

Two static checks are specified here, recorded in the IR, and not yet enforced. Both are backstopped
at runtime or are outright no-ops, and both belong to the eventual checker rather than the parser:

1. **The `@max` invariant** above. Backstopped by a spanned runtime error, and only on the writes
   that actually land: an `update` against an absent row evaluates nothing, so the backstop is now
   data-dependent as well as late.
2. **Rule 9's subject checks.** Both are static, and both are an explicit no-op today rather than a
   panic, so ordinary parsing runs through them. hekla's `enforce_subject_columns` is the reference
   for what they assert: a subject-bound value may only be written into a subject-bound field, the
   field's declared subject must agree, and the subject value must equal the row's own subject-id
   column. Nothing is lost by deferring them, because every handler stays in the IR alongside the
   event definitions, so the checker can recompute both subjects and report both spans.

## Known gaps

`type_of` in the parser is still the heuristic described in `docs/literal-inference.md`, and enum
literals inherit its limits: a variant in a position whose type comes back as `None` falls to the
unique-across-enums rule rather than being resolved from context.

Key types are restricted to those that can order and hash: `Int`, `String`, `Uuid`, `Timestamp` and
enums. `Bool`, `Money(n)` and `Decimal(n)` are rejected as keys, matching the runtime's requirement that
a key be an orderable scalar, since it doubles as the read API's pagination cursor.

Compound indexes are recorded in full, but the runtime can only filter on an index's leftmost column
today. Nothing here depends on that, since indexes are not used at runtime, but a compound index
declared now is more than the runtime can currently exploit.
