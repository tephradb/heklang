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
    total: Money,
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

`on @event.path [as name] { destructure } { body }`. Two adjacent brace blocks: the first
destructures payload fields into slots, the second is the body.

`as name` is optional and binds the envelope. Through it: `.at` (the append timestamp), `.id` (the
event's stable identity) and `.position` (its position in the log), plus payload access, so `e.total`
reads the payload field whether or not it was destructured.

There is no implicit binding in scope. A handler with no `as` clause has no way to reach the
envelope, by design: the envelope is available exactly when the author asked for it by name.

## 2. Statements

| Statement | Meaning |
| --- | --- |
| `put Entity { ... }` | writes all fields |
| `patch Entity[key] { ... }` | writes the listed fields |
| `delete Entity[key]` | removes the row |

All three are unconditional write instructions from the language's point of view. They cannot fail in
a way the program observes, so they return nothing and there is no error an author can catch. What a
runtime does with a patch against a missing key is a runtime concern, except as constrained by rule
5.

`put` requires every declared field to be present. `patch` writes only what is listed and leaves the
rest of the row alone.

A bare `T` written into a `T?` field wraps, so `tracking` destructured as a `String` satisfies a
`String?` column and `e.at` satisfies a `Timestamp?` one. This is not a new rule: it is the coercion
already applied to command arguments, applied at the other end of the same pipe. It only wraps an
exact inner-type match, so a `Uuid` still does not satisfy a `String?`.

## 3. Stored-value reference

Inside a `patch` value expression, a leading dot means the **current stored value** of that field:

```
patch Customer[customer_id] { order_count: .order_count + 1 }
```

A bare name always means a local binding (a destructured field, the `as` binding, or a `let`), never
a stored value, even when no local shadows it. This is the important half of the rule: adding a field
to an event must never silently change the meaning of a projector body. If a bare name could fall
back to the stored value, then declaring a new event field named `order_count` would quietly rebind
every `order_count` in every handler that destructures it.

`.field` is legal only in `patch` value position. Not in `put`, which writes a whole row and has no
prior value to read, and not in filters.

**Rejected: naming the row.** `patch C[k] as c { n: c.n + 1 }` is consistent with `as e` on the
handler, but it is wordier and reads oddly for a row that is being written rather than read. The
leading dot costs one character and carries the same information.

### How it lowers

Each distinct `.field` in one `patch` gets a frame slot. The interpreter fills those slots from the
stored row before evaluating any of the patch's value expressions, so `.order_count` is an ordinary
slot load by the time the expression layer sees it. That is why `.lifetime_spend + total` cross-hints
its literals exactly like any other typed load, and why the expression evaluator needs no new node
for this feature. It is also why "only in `patch` value position" is structural rather than a check:
nowhere else allocates a stored load.

## 4. No general reads

A handler cannot read entity state except through rule 3. It cannot read a different entity, cannot
read a different row, and cannot branch on stored state.

This keeps a projector a pure fold over the event log, so rebuild determinism is structural rather
than a rule authors can violate.

**Rejected: a general `get(entity, key)`.** This is not hypothetical. hekla has exactly that today,
reading any row through the current batch's uncommitted writes. It is more expressive, and it makes
read-modify-write across entities possible. It lost because with it, whether a projector rebuilds to
the same rows depends on the handler bodies being written carefully; without it, that property holds
by construction. Rule 3 covers the read-modify-write case that actually comes up, which is
incrementing a counter on the row being written.

## 5. Zero values

Every entity field has a well-defined initial value, so a `patch` against a missing key can always
materialize the row.

| Type | Zero |
| --- | --- |
| `Bool` | `false` |
| `Int` | `0` |
| `Decimal(n)` | `0` at scale `n` |
| `Money` | zero in the configured currency |
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

### Defaults and zeros agree by construction

A field default is written as a literal, and the parser resolves it against the field's declared type
and the program's currency at the point of parse, exactly as it resolves every other numeric literal.
So `= 0` on a `Money` field becomes zero minor units at the currency's scale, and `= 0` on a
`Decimal(4)` field becomes zero at scale 4. Nothing unresolved reaches the IR.

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

## Where this diverges from the runtime

Four of these rules describe behaviour hekla does not have today. Recorded so that a future change is
an informed one rather than a rediscovery.

| Rule | heklang | hekla today |
| --- | --- | --- |
| 5 | `patch` materializes a missing row from zeros | `patch` is a documented no-op when the row does not exist |
| 5, 6 | entity fields have zeros and defaults | no defaults anywhere; `put` requires every non-optional field, an omitted column binds NULL |
| 1 | `.position` is available through `as` | a handler sees `id`, `timestamp`, `type` and `data` only; the log position never reaches it |
| 4 | no general reads | `get(entity, key)` reads any row, through the current batch's uncommitted writes |

One representation choice also diverges. `Timestamp` here is an `i64` of epoch microseconds; the
runtime's envelope timestamp is an RFC 3339 string and its timestamp column stores text, with no
formatting or arithmetic defined on it. Both orderings agree, so this is safe, but it means a
conversion at the runtime boundary.

## Checker obligations

Two static checks are specified here, recorded in the IR, and not yet enforced. Both are backstopped
at runtime or are outright no-ops, and both belong to the eventual checker rather than the parser:

1. **The `@max` invariant** above. Backstopped by a spanned runtime error.
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
enums. `Bool`, `Money` and `Decimal(n)` are rejected as keys, matching the runtime's requirement that
a key be an orderable scalar, since it doubles as the read API's pagination cursor.

Compound indexes are recorded in full, but the runtime can only filter on an index's leftmost column
today. Nothing here depends on that, since indexes are not used at runtime, but a compound index
declared now is more than the runtime can currently exploit.
