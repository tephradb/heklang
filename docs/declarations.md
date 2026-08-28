# Declarations

What a module may declare, and the name spaces those declarations live in. The rules about *loading*
several modules are in `docs/modules.md`; this file is about what a single declaration means.

## The three handler kinds have separate name spaces

`command`, `projector` and `effect` each have their own space. One program may hold a command, a
projector and an effect all called `Same`, and each kind still rejects its own duplicate.

This is not an accident of the implementation, so it is worth stating as a rule: **a name is looked
up in exactly one kind, or in none.**

| Reference | Resolves to |
| --- | --- |
| `invoke Name { .. }` | a command, and only a command |
| an event path | an event, never a handler |
| a projector's name | nothing in the language; it names a read model to a host |
| an effect's name | nothing in the language; it names a subscription to a host |

Only one of those three is reachable from source at all. A shared space would therefore buy no
disambiguation, because there is nothing to disambiguate: it would only force renames on programs
whose command and effect are two halves of the same feature and want the same name.

A real port hit this twice, renaming an effect to `TeardownShop` and another to `RecordWarrantySales`
purely to avoid a collision it assumed existed. Both renames were unnecessary. That is the cost of
leaving a rule implicit even when the implementation already has it right, which is why there is now
a test that fails if the spaces are ever merged.

**Rejected: one flat space for all three.** The argument for it is that a reader seeing `Same` should
not have to ask which kind it is. But a reader never sees a bare `Same`: they see `invoke Same`, or a
`command Same` declaration, or an `effect Same` declaration. The kind is at every use site already.

## `else if`

An `else` may be followed by another `if` rather than a block, so a multi-way dispatch is a chain:

```
if kind == 1 {
  return
} else if kind == 2 {
  return invalid("two")
} else {
  emit @order.routed { order_id, kind }
}
```

Before this, the statement form required `{` after `else`, so the same dispatch nested one level per
arm and a six-way one was six levels deep at the last case. The expression form (`if c { a } else { b
}`) was already chain-friendly, so this only aligns the statement form with it.

The whole chain is **one** statement in the IR, which matters for the erase-last analysis in
`docs/effects.md` rule 9: an `else if` is a nested `Stmt::If` in the `otherwise` branch, so the join
that analysis performs is exactly the join the reader sees.
