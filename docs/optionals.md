# Optionals

`T?` is the only way a value is allowed to be absent. There is no null, no empty-string-means-nothing
and no zero-means-nothing: `docs/projectors.md`'s zero-value table exists to argue against sentinels,
and an optional is what it argues for.

Three methods read one:

| Method | Returns |
| --- | --- |
| `x.is_some()` | `Bool` |
| `x.is_none()` | `Bool` |
| `x.unwrap_or(fallback)` | `T` |

`unwrap_or` is total, and that is the whole set. There is no `unwrap`, no `?` operator and no
default-on-absent coercion, because each of those is a way to read an absent value without having
written down what to do about it.

## Narrowing

A branch that proves an optional present makes it its inner type for as long as the proof holds:

```
let plan = plans.get(plan_id)
if plan.is_none() {
  return
}
sync(plan)                          // plan is Plan here, not Plan?

if found.is_some() {
  use(found)                        // and here
}
```

The rule is three lines:

- `x.is_some()` narrows the **then** branch, `x.is_none()` narrows the **else** branch, and `!` swaps
  which;
- when the then branch never falls through, the test also narrows the **remainder of the enclosing
  block**, which is the early-return shape above;
- a narrowing ends where its block does.

Nothing about this is a new kind of scope. Statically the slot's declared type becomes `T`; at
runtime a load of a narrowed slot lowers to an unwrap node no source token can spell, and reaching it
with an absent value is malformed IR rather than a runtime failure with a story.

**Why it is sound without a data-flow pass.** There are no mutable bindings, so a slot's value is
fixed once bound and a proof about it cannot go stale. "Never falls through" is `always_returns`, the
same analysis that checks a `fn` returns on every path. And the parser lowers a condition before it
parses either of its blocks, so a single-pass parser needs no backtracking to know what it proved.

## What deliberately does not narrow

Each of these is sound in principle. None of them appeared in the port that motivated narrowing, and
leaving them out is what keeps the rule three lines instead of a paragraph with exceptions.

- **Compound conditions.** `if a.is_some() && b.is_some()` narrows neither, and
  `if a.is_none() || b { return }` narrows nothing after it. A conjunction and a disjunction narrow
  in opposite directions, and getting that wrong is silent.
- **The value-position `if`.** `let x = if y.is_some() { y } else { z }` is an expression, and
  narrowing inside it would mean the two branches disagree about a slot's type while both are being
  typed against one target.
- **An `else if`.** A narrowing proved inside one does not escape the chain, because what the chain
  as a whole proves depends on every arm above it.

Where narrowing does not reach, `unwrap_or` still does, and its fallback is written down.

## Rejected: `x.expect("reason")`

The obvious smaller alternative, and a worse language. It turns a fact the parser can already see
into a runtime failure carrying a hand-written message, and it would sit next to `unwrap_or`
permanently as a second way to consume an optional, with no rule for choosing between them.

What made this worth building rather than deferring is what its absence produced. A port carried
constants like `NO_PLAN_SYNC_FACTS`, a whole record of zeros, existing only to satisfy an
`unwrap_or` on a branch three lines below the `is_some()` that had already proved it could not be
taken. Those constants are a bug waiting to be read as data, and no amount of `expect` deletes them
as cleanly as the branch already proving what it proves.
