# Money

`Money(n)` is a scaled integer, exactly like `Decimal(n)`, and a **distinct type** from it. The scale
is written the same way and means the same thing:

```
event @order.placed {
  total: Money(2),
  currency: String,
}
```

**Currency is not in the type, not in the value, and not in the config.** An author who needs to know
which currency an amount is in declares an ordinary field beside it, as above.

This document is the contract. `tests/money.rs` is the same rules as executable tests.

## Why `Money` is not just `Decimal`

The operator table, and nothing else. Money is the type whose arithmetic has real-world mistakes in
it, and the table is what refuses them:

| Expression | Result |
| --- | --- |
| `Money(n) + Money(n)` | `Money(n)` |
| `Money(n) - Money(n)` | `Money(n)` |
| `Money(n) * Int`, `Int * Money(n)` | `Money(n)` |
| `Money(n) / Int` | `Money(n)`, and an error unless exact |
| `Money(n) * Decimal(s)`, `Decimal(s) * Money(n)` | `Money(n)`, and an error unless exact |
| `Money(n) / Money(n)` | `Decimal(6)`, a ratio |
| `Money(n).mul(Decimal(s), Rounding)` | `Money(n)` |
| `Money(n).div(Int, Rounding)` | `Money(n)` |
| `Money(n) * Money(n)` | **type error**: two amounts multiplied is not an amount |
| `Money(n) + Decimal(s)` | **type error**: this is adding a tax rate to a total |
| `Money(n) + Money(m)`, `n != m` | **type error**, the same rule `Decimal` has |

An amount times a rate is an amount; an amount divided by an amount is a rate; an amount plus a rate
is a mistake. Collapse `Money` into `Decimal(n)` and every one of those becomes legal.

**The table is checked before the program runs.** A row that is not in it is a compile error naming
both operands, and for the three mistakes with a shape it names the mistake too: two amounts
multiplied is not an amount, two amounts meet at one scale, and `+` between an amount and a rate is
adding a tax rate to a total. It used to be a runtime error, which meant a program could be shipped
with `total + tax_rate` in a branch nobody had taken yet. `docs/types.md` has the rest of the rules
this one belongs to.

The rounding rule is unchanged: where a result is not exactly representable, the bare operator is an
error and the author must say `mul(rate, HalfUp)` or `div(parts, Down)`. Money never rounds silently.

## Literals

A money literal resolves against the declared scale by exactly the rule `Decimal` uses (see
`docs/literal-inference.md`): widening is exact, and more written places than the target holds is a
`TooPrecise` error rather than a silent round.

| Source | Resolves to |
| --- | --- |
| `total: Money(2)`, written `1000.00` | `Money(100000, scale 2)` |
| `total: Money(3)`, written `1000.00` | `Money(1000000, scale 3)` |
| `total: Money(0)`, written `1000` | `Money(1000, scale 0)` |
| `total: Money(0)`, written `1000.00` | error: 2 decimal places is too precise for Money(0) |

A bare literal never defaults to `Money`. Scale 0 defaults to `Int` and any other scale to
`Decimal(n)`, so money is always reached from an annotation.

## Scale is a storage floor, not a currency

`Money(2)` does not mean "dollars" and `Money(0)` does not mean "yen". The scale says how many
decimal places the field stores, and nothing more. A JPY amount in a `Money(2)` field is exact: it
simply never has a non-zero fraction.

So **an application handling several currencies should pick a scale that fits all of them**, usually
`Money(3)` or `Money(4)`. Picking `Money(2)` because most of the currencies have two decimal places
is the mistake this framing is meant to prevent, since the field has to hold the most precise
currency it will ever see, not the most common one.

## The decision, and what was rejected

**Currency in the value**, as `{ units, currency }`, was rejected. It buys exactly one runtime check,
that adding USD to AUD fails, and it costs: an ISO 4217 table in the language, a currency-polymorphic
zero so that `= fold 0` knows what it is zero of, and a currency parameter threaded through parsing
into every literal resolution. One check is not worth a table and a parameter.

**Currency in config** was rejected because a multi-tenant application is the normal case, not the
exception. One deployment serving stores in different currencies cannot have one configured currency,
and that is the shape most of these systems actually have. (heklang had this, briefly, as a
`currency USD` item; it is gone.)

**Collapsing `Money` into `Decimal(n)`** was rejected because it loses the operator table above,
which is the part that catches mistakes. Without it, `total + tax_rate` is a legal expression.

**The cost, stated plainly: the language will not detect adding an AUD amount to a USD one.** That
invariant belongs to the author. In practice it is enforced at the same tenant or store boundary they
are already enforcing, which is where the currency field lives; heklang gives them a place to put it
and does not pretend to check it.

## Known gaps

There is no conversion, no rate type and no way to say that two `Money(2)` fields are in the same
currency. All three would need currency back in the type, which is the decision above.
