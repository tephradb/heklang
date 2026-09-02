# Literal inference

The lexer emits one untyped token for every numeric literal: `Number { digits, scale }`, where the
value is `digits x 10^-scale` and `scale` is the count of digits written after the decimal point:
`1000` is `{1000, scale 0}`, `1000.00` is `{100000, scale 2}`, and `0.9` is `{9, scale 1}`. There is
no suffix and no constructor call; whether a literal becomes `Int`, `Decimal(s)` or `Money` is
decided by the type it flows into.

This document is the contract. The parser implements it today; the typechecker must reproduce it
exactly. `tests/literal_inference.rs` is the same table as executable tests.

## Resolution

A literal resolves against a **target type**, which is either known from context or defaulted:

- Against `Int`, the literal must have scale 0.
- Against `Decimal(s)`, the literal widens from its written scale to `s`. Widening is exact.
- Against `Money(n)`, exactly as for `Decimal(n)`: the literal widens from its written scale to `n`.
  Currency is not involved, because it is not in the type (see `docs/money.md`).
- Against any other type, it is an error.

Widening is the only rescale allowed. A literal written with **more** decimal places than the target
can hold is a `TooPrecise` error, never a silent round: `0.0825` cannot become a `Decimal(2)` and
`10.5` cannot become an `Int`. Rounding a literal is always the author's mistake to fix, not the
compiler's to paper over.

## Where the target comes from

In order of priority:

1. **An annotation.** A parameter type, a `fold` type, an event field type in a filter or an `emit`,
   or the declared type a `fold` must produce. `fold total: Money(2) = 0` resolves `0`
   as money at scale 2.
2. **The other operand of `+`, `-` or a comparison.** `lifetime_spend > 1000.00` resolves the literal
   as `Money` because the left side is money. This works in both directions: for `1000.00 < spend`
   the literal is resolved once the right side's type is known, and the already-emitted IR node is
   patched.
3. **The default**: scale 0 becomes `Int`, any other scale becomes `Decimal(n)` at the scale written.

Three rules constrain step 2, and each exists because dropping it produces a wrong program rather
than a worse error message.

**`*`, `/` and `%` never cross-hint.** Their operands are deliberately different types: `Money`
times `Decimal` is a rate applied to an amount, `Money` divided by `Int` is a split. Hinting `Money`
onto the literal in `total * 0.9` would read it as $0.90 and produce a nonsensical `Money * Money`
instead of the intended error about rounding. Only `+`, `-` and comparisons, whose operands must
agree, cross-hint.

**A hint is never taken from a literal that was itself defaulted.** In `1 + 0.5` the leading `1`
defaults to `Int`; letting that default become the hint would reject `0.5` as too precise for `Int`,
and the author never wrote a type at all.

**Two defaulted literals settle toward the one with more decimal places.** `1 + 0.5` and `0.5 + 1`
both give `Decimal(1) + Decimal(1)`. Since widening is exact, the more precise side is always the
safe target.

**A target that cannot hold a number is not a target.** Only `Int`, `Decimal(n)` and `Money(n)` are,
and a literal offered anything else takes its default and lets the position it is in say what it
actually wanted. This is not leniency: it moves the report from the literal to the mistake.

```
if owner_email > 0            → cannot apply `>` to String and Int
total.mul(rate, 1)            → expected Rounding, found Int
effective_sku(sku, 1)         → expected Uuid, found Int
```

Each of those used to read "a number cannot be a `X`", which is true of the literal and is not what
is wrong with the line. The `>` case is the one that shows why: the target came from the *other
operand*, so the parser was reporting a type it had inferred, about a token the author wrote for a
different reason entirely.

**A `Bool` annotation is not a target for either operand.** The same rule, one level up. A `Bool`
describes the comparison, not its operands, and the parser reads the left operand before it knows a
comparison follows. Without this, `if 5 > count` resolves `5` against `Bool`. Nothing resolves
against `Bool`, so dropping it costs no inference, and it is what makes the `1000.00 < spend` row
above writable inside an `if` and not only in a `let`.

A declaration is different, and keeps the older message: `const LAUNCH: Timestamp = 5` says a number
cannot be a Timestamp and names the string form, because there the target is not inferred from
anything. It is what the author wrote three tokens ago.

## Table

Assume a command with `total: Money(2)`, `spend: Money(2)`, `count: Int`, `rate: Decimal(4)`.

| Source | Resolves to |
| --- | --- |
| `fold open: Int = 0` | `Int(0)` |
| `fold spent: Money(2) = 0` | `Money(0, scale 2)` |
| `fold rate: Decimal(4) = 0.0825` | `Decimal(825, scale 4)` |
| `count + 1` | `Int(1)` |
| `spend + 1` | `Money(100, scale 2)` |
| `spend > 1000.00` | `Money(100000, scale 2)` |
| `1000.00 < spend` | `Money(100000, scale 2)` |
| `count >= 10` | `Int(10)` |
| `1 + 0.5` | `Decimal(10, scale 1)`, `Decimal(5, scale 1)` |
| `0.5 + 1` | `Decimal(5, scale 1)`, `Decimal(10, scale 1)` |
| `0.9` alone | `Decimal(9, scale 1)` |
| `9` alone | `Int(9)` |
| `total * 0.9` | `Decimal(9, scale 1)`, and the multiplication is an error unless exact |
| `total.mul(0.9, HalfUp)` | `Decimal(9, scale 1)` |
| `total / 3` | `Int(3)` |
| `1000.00` as `Money(0)` | error: 2 decimal places is too precise for Money(0) |
| `0.0825` as `Decimal(2)` | error: 4 decimal places is too precise for Decimal(2) |
| `10.5` as `Int` | error: 1 decimal place is too precise for Int |

## Where a hint comes from is where a type comes from

The target in step 1 is a declaration, so it is exact by construction. The one in step 2 is
**synthesised** from the other operand, and for a long time that was a heuristic: it returned nothing
for a method call, and it reported `Money(n)` for `Money(n) / Money(n)` where the value is a
`Decimal(6)`. Both are fixed, and both had to be before anything could check a type rather than only
hint one, because a hint that is wrong costs a worse error message while a **check** that is wrong
rejects a correct program.

`docs/types.md` is where synthesis is written down now. Two of its rules are visible from here:

| Source | Resolves to | Because |
| --- | --- | --- |
| `spend / total + 1` | `Decimal(1000000, scale 6)` | an amount over an amount is a ratio (`docs/money.md`) |
| `total.mul(rate, HalfUp) + 1` | `Money(100, scale 2)` | a rate applied to an amount is an amount |

## Known gaps

Synthesis still answers "unknown" in places, and a `let` carries that forward: its binding takes the
type of its value, so an unknown one makes every later use of that name unknown too. Nothing here is
wrong, it is only absent, and absence costs a defaulted literal rather than a bad one.

Enum literals inherit the same limit, which `docs/projectors.md` records: a variant in a position
whose type comes back as `None` falls to the unique-across-enums rule rather than being resolved from
context.
