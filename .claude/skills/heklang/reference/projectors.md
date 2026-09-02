# Projectors

A projector is the read side: a pure fold over the event log into **entities**, which are read-model
rows. A read model is a rebuildable cache, never a source of truth, so everything here exists so that
replaying the log from position 0 reproduces the same rows.

```hek
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

    index (customer_id, status),
  }

  entity Customer {
    customer_id: Int @key,
    order_count: Int,
    lifetime_spend: Money(2),
  }

  on @order.placed as e { order_id, customer_id, email, total } {
    put Order {
      order_id,
      customer_id,
      email,
      total,
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
are unrelated.

## 1. Handler form

`on @event.path [as name] [{ destructure }] { body }`. With two adjacent brace blocks the first
destructures payload fields and the second is the body; with one, there is nothing to destructure and
the block is the body.

`as name` binds the envelope and is optional: through it, `.at` (the append timestamp), `.id` (the
event's stable identity) and `.position` (its position in the log), plus payload access, so `e.total`
reads a field whether or not it was destructured. There is no implicit binding: a handler with no
`as` clause cannot reach the envelope.

**Several handlers may name the same event type**, in one projector or across projectors. Fanning one
event out to several read models is the point, and a rebuild replays every handler in the same order.
(An effect is the opposite: one arm per event type.)

Handlers do not share state with each other. Destructured names, the `as` binding and `let` bindings
are in scope in the body.

## 2. The four write statements

| Statement | Meaning |
| --- | --- |
| `put Entity { ... }` | writes all fields; takes **no** key, because the key is a column it fills |
| `patch Entity[key] { ... }` | writes the listed fields, **materializing** the row if it is absent |
| `update Entity[key] { ... }` | writes the listed fields, **dropping** the write if the row is absent |
| `delete Entity[key]` | removes the row |

All four are unconditional write instructions. They return nothing and cannot fail in a way the
program observes. `put Entity[key]` does not parse, and neither does `patch Entity { .. }`.

`put` requires every declared field to be present, including a field that has a default, because a
`put` writes the whole row and never reads a default. `patch` and `update` write only what is listed.

A bare `T` written into a `T?` column wraps, so a destructured `String` satisfies a `String?` column
and `e.at` satisfies a `Timestamp?` one.

## 3. The stored value: a leading dot

```hek
patch Customer[customer_id] { order_count: .order_count + 1 }
```

Inside a `patch` or `update` **value expression**, a leading dot means the current stored value of
that field. A bare name always means a local binding (a destructured field, the `as` binding, or a
`let`), never a stored value, even when no local shadows it: otherwise adding an event field named
`order_count` would silently rebind every `order_count` in every handler.

`.field` is legal only in a `patch` or `update` value position. Not in a `put`, which has no prior
value, and not in a key or a filter.

## 4. No general reads

A handler cannot read entity state except through rule 3. It cannot read a different entity, cannot
read a different row, and cannot branch on stored state. There is no `get(entity, key)`. That is what
makes rebuild determinism structural rather than a discipline.

`update` is the one carve-out: it looks at whether the row is there, but no value derived from that
look reaches the program, so the one bit decides whether a write lands and nothing else.

## 5. Zero values, and choosing `patch` or `update`

A `patch` against a missing key materializes the row, so every column it might fill needs a value:

| Type | Zero |
| --- | --- |
| `Bool` | `false` |
| `Int` | `0` |
| `Decimal(n)` | `0` at scale `n` |
| `Money(n)` | `0` at scale `n` |
| `String` | `""` |
| `T?` | `none` |
| an enum | its `@default` variant |
| `Uuid` | **none: no zero exists** |
| `Timestamp` | **none: no zero exists** |

`Uuid` and `Timestamp` have no zero because the nil UUID and epoch zero are real values that get
mistaken for data. Both are writable as a `= <literal>` default (a string in each case).

`= <literal>` on a field overrides the zero. An **optional field takes no default of `none`**,
because it already has one, though a present default (`note: String? = "x"`) is ordinary. `@key`
fields need no default, because the subscript supplies them.

**The demand is on the write, not the declaration.** An entity that some `patch` can materialize
needs a zero or a default for every column; an entity written only by `put`, `update` and `delete`
needs neither. The error names the `patch` that creates the requirement and lists all three fixes:
give the column a default, make it optional, or make the write an `update`.

### The question to ask at every write

> What does an absent row mean for this entity?

- Absent means **"zero of this thing"**: counters, totals, running sums. `patch` is right, and
  materializing from zeros is the feature.
- Absent means **"this thing does not exist"**: a plan, an order, a shop, anything the read model
  treats as an identity. `update` is right, and a `patch` there **manufactures a record**.

A sale arriving for a deleted plan, written as `patch Plan[plan_id] { revenue: .revenue + price }`,
leaves a row with a real id, a real revenue figure, an empty title and a `created_at` of `none`. It
looks like a plan, nothing failed, nothing logged, and a rebuild reproduces it faithfully. Written as
`update`, the sale is dropped and the catalogue stays correct. In one real port, all seventeen
`patch` statements wanted `update`.

**`delete` is not a tombstone.** It removes the row without recording that it did, so a later `patch`
naming that key materializes a fresh one from zeros. Reaching for `delete` to remove someone's data
leaves a hollow row behind on the next event that touches the key. Erasure is `erase(subject)` in an
effect, which drops the per-subject key and shreds the value across the log and every read model at
once.

## 6. Enums

An enum used as a non-optional entity field needs a `@default` variant, so that reordering variants
is not a semantic change. Variants are written bare (`status: Placed`) and resolved from the target
type; a variant the target's enum does not declare is an error naming the enum.

## 7. Indexes

Field-level `@index` with no name declares a single-column index. Entity-level `index (a, b)`
declares a compound one, and the order is significant. Every index column must name a declared field.
Index names are a storage concern and are not authored.

## 8. Sealed columns

Entity fields do **not** restate `@subject`. The seal is a property of the value's type and
propagates from the event field, through destructuring and through a `let`, onto the column written
from it. Writing sealed content into a column seals the column.

That is why a projector can store a credential it may never read: only an effect crosses the decrypt
boundary, and a projector never `reveal`s, yet storing personal data into a read model is most of
what real projectors do. Moving sealed content is not reading it.

Two checks hold:

- **One column, one subject.** Two handlers writing content sealed under different subjects into one
  column is an error naming both.
- **Sealed content may not be written where the seal is discarded**, which is the same boundary rule
  every other position keeps.

## 9. The `@max` invariant

> A bounded position's `@max` must be no tighter than the `@max` of every field written into it, and
> a field with no `@max` may not be written into a position that has one.

A command already rejects an over-length value at `emit`, so the only way a projector can observe one
is a schema bug. That is checked statically and reported as `max-tightening`:

```
this write narrows `notes`: @order.placed declares no bound and `Note.note` is `@max(8)`
```

It holds at an `emit` too, between two event fields. A **computed** value has no second declaration
to compare against, so `note: "note: {notes}"` is not this defect and the run-time bound is what
covers it. A fold whose arms transform what they fold makes the whole state unknown rather than
partly known.

`@max` names a length, so it applies to `String` and `String?` and nothing else. `@max` on an `Int`,
a container or a record is an error naming the type.

## 10. What a projector may not do

No `now()`, no `http.*`, no `invoke`, no `log`, no `fail`, no `reveal`, no `erase`, no `emit`, no
`fold`, no `guard`. A projector has no clock and no network because a rebuild has to reproduce every
value it wrote, and no failure channel at all because a write returns nothing. It may call a module
`fn`, which is pure.
