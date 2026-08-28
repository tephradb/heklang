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
- Against `Money`, the literal widens from its written scale to the scale of the program's declared
  currency, so `1000.00` is 100000 minor units under USD and 1000000 under BHD, but an error under
  JPY, where the same amount is written `1000`.
- Against any other type, it is an error.

Widening is the only rescale allowed. A literal written with **more** decimal places than the target
can hold is a `TooPrecise` error, never a silent round: `0.0825` cannot become a `Decimal(2)` and
`10.5` cannot become an `Int`. Rounding a literal is always the author's mistake to fix, not the
compiler's to paper over.

## Where the target comes from

In order of priority:

1. **An annotation.** A parameter type, a `state` type, an event field type in a filter or an `emit`,
   or the declared type a `state` fold must produce. `state total: Money = 0` resolves `0` as money.
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

## Table

Assume `currency USD` and a command with `total: Money`, `spend: Money`, `count: Int`,
`rate: Decimal(4)`.

| Source | Resolves to |
| --- | --- |
| `state open: Int = 0` | `Int(0)` |
| `state spent: Money = 0` | `Money(0)` |
| `state rate: Decimal(4) = 0.0825` | `Decimal(825, scale 4)` |
| `count + 1` | `Int(1)` |
| `spend + 1` | `Money(100)` |
| `spend > 1000.00` | `Money(100000)` |
| `1000.00 < spend` | `Money(100000)` |
| `count >= 10` | `Int(10)` |
| `1 + 0.5` | `Decimal(10, scale 1)`, `Decimal(5, scale 1)` |
| `0.5 + 1` | `Decimal(5, scale 1)`, `Decimal(10, scale 1)` |
| `0.9` alone | `Decimal(9, scale 1)` |
| `9` alone | `Int(9)` |
| `total * 0.9` | `Decimal(9, scale 1)`, and the multiplication is an error unless exact |
| `total.mul(0.9, HalfUp)` | `Decimal(9, scale 1)` |
| `total / 3` | `Int(3)` |
| `1000.00` as `Money` under JPY | error: 2 decimal places is too precise for Money |
| `0.0825` as `Decimal(2)` | error: 4 decimal places is too precise for Decimal(2) |
| `10.5` as `Int` | error: 1 decimal place is too precise for Int |

## Known gaps

`type_of` in the parser is a heuristic, not a typechecker. It returns nothing for a method call, so
`x.len() + 1` defaults the literal rather than hinting `Int`, and it reports `Money` for
`Money / Money` where the real type is `Decimal(6)`. A wrong hint can only produce a runtime type
error, never silently wrong arithmetic, so this fails safe. The typechecker should replace it rather
than extend it.
