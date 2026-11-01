# Named folds, and the read model they are

A fold declared at module scope is read by name, which is what `guard` does for a proposition.
Parens declare and braces use.

```hek
fold LifetimeSpend(customer_id: Customer) -> spend: Money(2) = 0 {
  on @order.placed(customer_id) { total } => spend + total
  on @order.cancelled(customer_id) { refund } => spend - refund
}

command GrantTier(customer_id: Customer, tier: String) {
  fold spend = LifetimeSpend { customer_id }
  ...
}
```

Inside the body it is an inline fold: one accumulator, arms only, no `return`, no `reject`. What is
new is everything outside it.

## 1. A named fold is a read model

**One sentence carries this file: a read model is a named fold, kept for every key.** Nothing
declares a table. A fold already says what a value is, which events change it and what it is keyed
by, so:

| | |
| --- | --- |
| the fold's **parameters** | the key |
| the fold itself | **one column** |
| folds sharing one key signature | **one table**, a row per key |

That is the whole of it, and it is why the read side of an application is folds rather than
projectors. A projector declares an entity and copies fields onto it; a fold says what one value is
and the layout follows.

`hek check --tables` prints what a program implies:

```
$ hek check --tables
checked 1 file
  3 events, 5 folds, 2 subjects

  TABLE shop  (shop: Shop)  1 column
    shop_order_ids: List(Int)  <- @order.placed

  TABLE shop__order  (shop: Shop, order: Int)  3 columns
    order_email: String @subject(Customer)?  <- @order.contact.recorded
    order_seen_at: Timestamp?                <- @order.placed
    order_total: Money(2)                    <- @order.placed

  not stored, because no event supplies the key:
    OrdersSince  (at)
```

**Run it whenever you add or rename a fold.** It is the only way to see which table a fold landed
in, and a fold that quietly moved tables is a column a deployment has to rebuild.

## 2. How the names are derived

Nothing here is written by an author, so knowing the derivation is how you predict the schema.

**A column** is the fold's name in `snake_case`, prefix and all: `LifetimeSpend` is
`lifetime_spend`.

**A key part** is named for its **subject** when its type has one, and otherwise for the
**parameter** with one trailing `_id` dropped. So `(shop_id: Shop, order_id: Int)` keys a table on
`(shop, order)`, and `(shop_id: Shop)` and `(shop: Shop)` are the *same* key.

**A table** is its parts' names joined with `__`, and **`global`** when the fold has no parameters,
which is a single-row table holding one value. Subject parts lead, ordered so a parent sits above
the children whose keys wrap under it; plain parts follow alphabetically.

Two refinements you will only meet by hitting them:

- **Where two parts would take one name**, by repeating a subject or spelling it two ways, each is
  named by its **parameter** instead: `(to_id: Region, from_id: Region)` is `(from, to)`.
- **Where two key signatures would take one table name**, because a part is told apart by name
  *and* type, each takes the part types into its name: `(day: String)` and `(day: Int)` are
  `day_string` and `day_int`.

## 3. The naming rules this forces on you

Three refusals, all `fold-restriction`, and all three are the derivation protecting itself.
(Section 5a adds a fourth, for a computed part named after a subject.)

**A named fold is spelled like a type: an initial capital, and no `_`.** `LifetimeSpend`, never
`lifetimeSpend` or `Lifetime_Spend`. The underscores go in the derived column. Without these two
rules `fold Sum` beside `fold sum` would derive one `sum` column twice.

**The column may not be named after a part of its own key.** `fold Shop(shop_id: Shop)` wants a
table whose key column and whose only value column are both `shop`, and nothing can create that:

```
fold `Shop` derives a column called `shop`, and its key already has a part of that name
```

The fix is the style rule below rather than a rename scheme.

**Named for the VALUE, never the entity.** `OrderName`, `ShopAccessToken`, `OpenProposal`,
`QuoteFailureCount`. Not `Order`, not `Shop`, not `Proposal`. One value out, so two facts about an
order are two folds, and a command conflicts on exactly the two slices it read. `Order` with a
`.name` and a `.gid` would be one read whose boundary is the union of everything any caller wanted,
which is an aggregate with the serial numbers filed off.

## 4. Which folds are stored, and which are not

**A fold whose key no event supplies cannot be kept**, and `--tables` lists it at the end rather
than failing. That is a line and not an error: such a fold is perfectly good and a command can read
it, and nothing says every fold has to be storable. `OrdersSince(at: Timestamp)` is the shape, since
no event carries the `at` a caller passes.

Only **some** arm has to supply a given key part, not every arm. An arm that matches part of the key
is a range update over the rest: a fold whose first arm pins `(shop, order)` and whose second pins
`shop` alone is still kept, and the second updates every order of that shop.

**A query-only fold is stored and queryable but cannot be folded by a command.** An arm binding the
envelope with `as` is what makes one:

```hek
fold OrderSeenAt(shop_id: Shop, order_id: Int) -> Timestamp? = none {
  on @order.placed as e (shop_id, order_id) => e.at
}
```

```
`OrderSeenAt` binds the envelope (`as`), so it can be queried but not folded by a command
  = an event's append time is not a decision's to read: a test cannot pin it, and a
    backfilled event's is when it was imported rather than when it happened
```

**An event a fold has to time keeps its own timestamp field.** Reach for a query-only fold when the
value is for a reader rather than a decision; otherwise declare the `Timestamp` on the event.

**A stored column is defined over the keys the table holds.** A fold is a total function of a key
and a log, so a fold with a range arm on `@shop.uninstalled(shop_id)` answers for *any* order id,
including one no event mentioned; the store has no row there and reads the seed. The key space is
unbounded and the alternative is not implementable. What makes it safe is that every key part is
pinned by some arm, so the keys a range arm can need are the keys the pinning arms mint.

## 4a. An arm's filter list is written even when it is empty

`on @game.purchased() { game_id } => ..`, never `on @game.purchased { game_id }`. The list is what a
slice is, so `()` says this arm takes every record of the event, which is a statement about the
caller's boundary. A **projector's** `on` has no filter list at all, so the paren-less spelling is
right there and wrong here: it is the one place the two constructs do not read alike, and it is the
mistake two independent authors made first.

## 5. A fold arm may match a key part the event does not carry

`param == value` in a filter list matches a **fold parameter** rather than an event field, and names
the key part that arm writes:

```hek
fold HoldsOfKind(shop_id: Shop, order_id: Int, kind: String) -> n: Int = 0 {
  on @hold.placed(shop_id, order_id, kind, kind == "fixed") => n + 1
}
```

**Both halves do work, and this is the mistake to avoid.** `kind == "fixed"` says which key part the
arm writes; the bare `kind` beside it is an ordinary field filter narrowing the event. Written
without the second half the arm still writes at `"fixed"` and now counts *every* hold into that row.
The right side resolves once, in the caller's frame, so it is a literal, a `const` or a pure `fn` of
those, and it may not read a slot.

## 5a. A fold arm may compute a key part from the record

The third way an arm writes a part, and the one for a key no event could carry: a month bucket, a
lowercased email, a combination of fields.

```hek
fold UsageCounted(shop_id: Shop, month: String) -> n: Int = 0 {
  on @usage.recorded(shop_id) { at, units } key month = month_of(at) => n + units
}
```

**`key` is a clause of its own, after the destructure, and that placement is the rule.** Everything
in the filter list resolves **once** before any event is folded, which is what lets a field conjunct
become a predicate and a `==` conjunct a gate; this is evaluated **per record**, so it reads the
destructure's binds and the envelope binding, which do not exist yet where the filters are parsed.
`key` is soft, and several parts are a comma list.

**A fold with one is query-only**, as an `as` binding makes one: a computed key has no predicate
form, so a command could neither narrow the log to those events nor conflict on them. A test reads
it with `fold` or `fill`, and a deployment serves it from the table. The refusal names the part.

**A computed part and the key are kept apart, in both directions.** The part reads the record: the
destructure's binds, the envelope binding, `const`s and pure `fn`s of those, and not a parameter or
the accumulator, because the sink evaluates it to decide which row the record belongs to and so runs
before there is a key or a row. And no filter of the arm may read a part the arm computes, because a
filter resolves once for the whole slice against the key its caller asked for, and there is no such
key. Both halves used to check clean and fail only in the sink.

Eight refusals: a subject-typed parameter (ids are minted by the key store, not derived), an inline
fold's arm (not stored, so no key), a name that is not a parameter, a part a rule 13 conjunct or
another `key` clause already wrote, a read of the key or the accumulator, a filter reading a part the
arm computes, a filter reading a part the arm leaves open, and a part whose value is **sealed**
content, which is `seal-boundary`: erasure empties a column and cannot reach a primary key, so a row
filed under decrypted content is personal data nothing could erase. Return the seal as a column of
its own (rule 6). That one is checked at every position inside the expression, not over its result,
because a `fn`'s declared return type launders the subject away.

**No builtin is needed for the usual case.** `Timestamp.year()`/`month()` and `Int.pad(width)` are
all there, so `fn month_of(at: Timestamp) -> String { return "{at.year()}-{at.month().pad(2)}" }`
is the month key; `at.year() * 100 + at.month()` is an `Int` bucket that sorts and pads nothing.

## 5b. A fold arm may read a key part out of another fold

The fourth way an arm writes a part, and the one that stops events carrying other entities'
attributes just so a read model can group by them.

```hek
fold GenreUnits(genre: Genre) -> n: Int = 0 {
  on @game.purchased() { game_id } key genre = GameGenre { game_id } => n + 1
  on @game.refunded() { receipt_id }
    key genre = GameGenre { game_id: ReceiptGame { receipt_id } } => n - 1
}
```

Written exactly as a command reads a fold, and chains. `@game.refunded` carries neither the game nor
the genre.

**As of the record, and strictly before it**: the looked-up fold over the log before the record
being folded, which is what a command appending it would have read. So a sale counts towards the
genre the game had when it sold, and a record that *moves* the fold it is counted through counts
under the old value. Two consequences worth holding:

- **A lookup only reads what an earlier record established.** Key a refund through `ReceiptGame {
  receipt_id }` and it works because the *purchase* created the receipt; a `ReceiptGame` folding
  `@game.refunded` would read the refund itself and answer its seed.
- **A refund keyed through a fold that still moves decrements where the thing is now**, not where it
  was sold. Key through a fold that captures the sale if that matters.

**It makes the fold query-only**, as 5a's computed part does, for the sharpest version of the
reason: a lookup's boundary is another key's whole history, which no append condition can express.
The refusal names the lookup.

Six more refusals on top of 5a's: **only a stored fold** may be looked up (the sink answers from its
column, and `hek check --tables` lists what is kept); a **cycle**, including a fold looking itself
up, is `recursive-fold`; a **sealed** return type is `seal-boundary`, because a key is plaintext and
an erase cannot reach one; the result's type has to fit the part, so an optional result in a
non-optional part is refused and the author writes the fallback; the **arguments** are held to 5a's
rule, so `GameGenre { shop_id, game_id }` passing a key part is refused and destructuring the field
is the fix; and a lookup may not be written **under a binder**, because it is hoisted out of the
expression and a comprehension's name does not exist where it lands.

**`hek check --tables` prints the dependency**, transitively, which is how you see why editing one
column refolds another:

```
    genre_units: Int  <- @game.purchased, @game.refunded  (reads game_genre, receipt_game)
```

That list is also what moves the dependent's digest `hash` when a looked-up fold changes, and what a
rebuild replays alongside. A by-key read of one is quadratic and that is accepted: hekla serves it
from the table.

## 5c. And an arm's value may read a fold too

Rule 16: the same construct in the other half of the arm, which is what lets a fold ask about
another key at all.

```hek
fold TestOrder(shop_id: Shop, order_id: Int) -> Bool = false {
  on @order.placed(shop_id, order_id) { test_order } => test_order
}

fold EditsRequested(shop_id: Shop) -> n: Int = 0 {
  on @edit.requested(shop_id) { shop_id, order_id }
    => if (TestOrder { shop_id, order_id }) { n } else { n + 1 }
}
```

A fold is one value out (section 1), so a counter that has to leave some records out had no way to
know which: the ported application kept a `Map(Int, Bool)` membership set on the counter's own row,
written by every event that could place a test order, purely so twenty counters could read a sibling
column. Now the fact is its own fold and they look it up.

**Everything in 5b holds unchanged.** What differs is two things:

- **A sealed result is allowed**, where a key refuses one: an arm's value lands in a column, which an
  erase empties, rather than in a key an erase cannot reach. It is then ordinary sealed content and
  section 6 applies, so the fold has to **declare** `Sealed(T, Subject)` and the refusal if it does
  not is section 6's own.
- **The positions are where a record is in hand**: an arm's value and a `key` clause. A slice filter,
  a seed and an inline fold's arm are refused, the first two because they resolve with no record and
  the third because it is inside the command that could not then read it.

**An `if` header still reads `Name {` as a block**, so `if (TestOrder { .. })` takes parentheses,
exactly as a record literal there does. And a named fold may not share a name with a record or an
enum, because `Name { .. }` reads the fold in a fold arm and builds the record everywhere else; it
is refused where the fold is declared.

## 6. Sealed return types

`Sealed(T, Subject)` is spellable in a fold's return type **and nowhere else**, so a fold can hand
back content from behind the decrypt boundary for its caller to `reveal`.

```hek
fold OrderEmail(shop_id: Shop, order_id: Int) -> Sealed(String, Customer)? = none {
  on @order.contact.recorded(shop_id, order_id) { email } => email
}
```

**The subject need not be in the key, and usually is not.** That column is keyed on an order and
sealed under a customer, which is the common shape: no column of the row names the subject the
content is filed under. The value carries its own subject, id and field, so a reader needs nothing
from the key.

**The return type has to say so.** A named fold publishes its return type, and that is what its
caller and the stored column are typed from, so an arm folding sealed content under a type that does
not declare the seal is `seal-boundary`:

```
`OrderEmail` folds content sealed under `Customer` on @order.contact.recorded, and its
return type does not say so
  = a named fold's return type is what its caller and the stored column are typed from,
    so declare it: `-> Sealed(String, Customer)?`. The caller `reveal`s it (rule 7)
```

An inline fold needs none of this, because its slot carries the seal where only the body can see it.

## 7. Reading one

Three doors, and which one you want depends on what you are:

| | |
| --- | --- |
| `fold x = Name { .. }` in a command, guard or effect arm | spliced in before run time; the slices join the caller's append condition |
| `Interpreter::fold(name, args)` | the whole log for one key, which is what serves a stored column and what a `fold` test asserts |
| `Interpreter::fold_stored` / `Tables::apply` | an embedder filling and following a column; `docs/folds.md` section 11 |

The first is a parse-time construct: `src/inline.rs` splices it out, so no host, trace or append
condition ever sees a named fold. The rest run it by name.

A deployment also serves a stored column by name over HTTP, with the arguments named as the fold
declares its parameters. That route is hekla's and so is its contract, including which folds it will
and will not answer for; ask the **hekla** skill rather than assuming it from the three rows above.
What is worth carrying across from this file is that the split in section 4 is observable there:
**query-only** and **unstored** are different answers, not two spellings of unavailable. A
query-only fold is a column like any other and a by-name read is the only thing that may read it,
while an unstored fold has no column at all and can only be folded over the log for arguments a
caller supplies.

**A fold is testable two ways**, and `reference/testing.md` sections 8 and 9 have both: `fold Name
{ args }` with `expect <value>` asserts one column at one key, and `fill` with `expect
<table>[key] { column: value }` asserts a row of the whole read model.

## 8. The runtime half

Everything above is the language and `hek`. What a deployment does with it, the `/tables/{table}`
and `/tables/{table}/row` endpoints, the Tables console page, column rebuilds, and the key-part
names a runtime refuses, belongs to **hekla** and its own skill. Do not guess at it from here.

`docs/folds.md` in the heklang repository is the contract for this file, and `docs/cli.md` for
`--tables`.
