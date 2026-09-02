# Diagnostics

`hek check` reports **every mistake, not only the first**, and each one carries a code, an extent and
a hint separately:

```
a.hk:2:41 [type-mismatch] expected String, found String?
  |
2 |   emit @order.placed { order_id, name: text }
  |                                        ^^^^
  = `unwrap_or` gives it a fallback, or a branch that proves it present makes it a
    String without one

b.hk:3:9 [declared-twice] command `C` is declared twice
  |
3 | command C(id: Int) { return }
  |         ^
  = a.hk:1:9: first declared here
```

The header is `file:line:col [code] message`. Under it goes the source line with the extent drawn,
then the hint, then every related location, each as a `= ` note. Line and column are 1-based, and the
column counts characters.

**A syntax error abandons its declaration; a semantic one does not.** A token the grammar cannot take
means there is nothing left to read there, so the whole declaration is stepped over and the next one
is parsed as if nothing happened. Everything else parsed, so the rest of the body is still checked.

**A rejected value becomes a poison.** Its type is unknown and an unknown type is never checked, so
`let x = text.trm()` followed by two uses of `x` is one diagnostic, not three. Fix the first one and
re-run rather than reading the rest as independent.

**Reporting stops at the end of the pass that found any**, and the four whole-program checks
(recursion, the self-trigger cycle, a `patch`'s zero values, and the `@max` invariant) report one at
a time. So a clean run after a fix can still surface a new error: keep running `hek check` until it
says nothing.

## The codes

The set is closed: every diagnostic heklang can produce is one of these.

### Lexical and syntactic (these abandon the declaration)

| Code | Means | Fix |
| --- | --- | --- |
| `bad-number` | a numeric literal the scanner could not finish | check for a stray `.` or a missing digit |
| `unterminated-string` | a `"` or `"""` that ran to the end of the file | close it; the position now points at the opening quote |
| `unknown-escape` | `\z`; the set is `\n \t \" \\ \{ \}` | use one of those, or a raw `"""` string |
| `bad-path` | an `@` with no name after it | write the event path, `@thing.happened` |
| `unexpected-character` | a character with no token in the language | delete it |
| `expected-token` | a token the grammar cannot take here | the message names what was wanted |

### Names

| Code | Means | Fix |
| --- | --- | --- |
| `declared-twice` | a name, field, variant, arm or annotation given twice, or a guard named twice on the same arguments | names are global across files; the related note points at the first |
| `not-declared` | spelled fine, declared nowhere (usually an event path) | declare it, or fix the spelling |
| `not-in-scope` | declared somewhere, but not visible here | an effect-local `fn` is visible only in its effect; an entity or projector enum only in its projector |
| `unknown-member` | a field, method, parameter, variant or verb the receiver has not got | check `stdlib.md`; the commonest pair is `is_empty()` asked of a `String?` and `is_none()` asked of a `String` |
| `unknown-type` | a type name that names no type | `Response` and `Outcome` are spellable only in a `fn` signature |

### Types and values

| Code | Means | Fix |
| --- | --- | --- |
| `type-mismatch` | a value that does not fill a declared type | `T?` does not fill `T`: use `unwrap_or`, or a branch that proves it present |
| `bad-operands` | an operator applied to a pair it does not take | scales never meet; `Money` and `Decimal` do not add; there is no `+` on `String` |
| `bad-literal` | a literal that cannot be the type its position declares | usually more decimal places than the target holds, or a malformed uuid or RFC 3339 string |
| `bad-type` | a type spelled wrong | a scale above 18, or a `Map` key that does not order (`Bool`, `Money(n)`, `Decimal(n)`) |
| `needs-target-type` | a value whose type nothing decides | `[]`, `Map.empty`, `Money.parse` and `Decimal.parse` take their type from the target; a `let` is not one |
| `not-a-value` | a statement written where a value was wanted | a call to a void effect-local `fn` is a statement, so it cannot be bound with `let` |
| `arity` | a call with the wrong number of arguments, or braces on a refusal that has no fields | a third positional argument to `http.*` is a timeout, which is configuration; `reject Gone` takes no `{ }` |
| `missing-field` | a field, parameter or argument that has to be given and was not | `emit`, `put`, `given`, `invoke`, a record literal, `reject` with fields and `guard Name { .. }` are all written whole |
| `duplicate-field` | one given twice | |

### Annotations and declaration shape

| Code | Means | Fix |
| --- | --- | --- |
| `unknown-annotation` | `@nope` | events take `@subject`, `@max`, `@no_index`; entities `@key`, `@index`, `@max`; records `@max`; enum variants `@default` |
| `bad-annotation` | a known annotation in a place or shape it does not take | `@max` bounds a `String`; a record field cannot be `@subject`; a `@subject` id may not be optional or itself sealed; an optional column may not default to `none` |
| `empty-declaration` | a declaration whose body would be empty | |
| `entity-shape` | an entity with no `@key`, more than one, an unorderable key, or an index on a field it has not got | |
| `event-shape` | a multi-path arm over event types with nothing in common | a field is shared only when its type and its `@subject` match on every listed path |
| `refusal-shape` | a refusal named or written so its derived code could not survive | start with a capital, use no `_`, and let the message name every field and nothing else |
| `state-shape` | a `state` or `guard` written where or how it does not go | declarations come before the first statement of their stage, never inside an `if` or a `for`; a seed or filter may not read a `state` beside it; a guard is one read |
| `no-zero-value` | a `patch` that would materialise a row it cannot fill | give the column a default, make it optional, or make the write an `update` |

### Context and purity

| Code | Means | Fix |
| --- | --- | --- |
| `wrong-context` | a statement in a declaration kind that does not have it | see the matrix in `stdlib.md`: `emit` is a command's, the four writes are a projector's, `http`/`invoke`/`log`/`fail` are an effect's |
| `impure-fn` | a module `fn` doing something a pure function cannot | move the call to the caller and pass the result in, or make it an effect-local `fn` |
| `fold-restriction` | a `state` fold calling out, invoking, decrypting or reading a clock | a fold has to reproduce without a journal; do it in the body and pass it in |
| `arm-only` | an effect-local `fn` doing what stays in the arm | `reveal`, `erase`, `now()` and `state` stay in the arm; pass the revealed value or the moment in as a parameter |
| `return-shape` | a `return` that does not match the signature it is in | a guard returns only `reject <Name>` or `invalid(...)`; a module `fn` must declare a return type and return on every path; `reject`/`invalid` need `-> Outcome` or `-> Outcome?` |

### Seals

| Code | Means | Fix |
| --- | --- | --- |
| `seal-boundary` | sealed content leaving without `reveal` | move it, ask `.is_some()`/`.is_none()`, or `reveal` it in an effect arm; a `fn` parameter, an interpolation, a comparison, a body and `unwrap_or` all take it out |
| `erase-subject` | an `erase` whose subject or id is not one | the inferring form takes a trigger field; the named form takes a declared subject name and a value of the id's type, with no `reveal` in it |
| `erase-order` | a `reveal` reachable from an `erase` | move the reveal above the erase, or into a branch the erase cannot reach; inside a `for` body, any erase reaches every reveal |

### Whole-program (these report one at a time, after the passes)

| Code | Means | Fix |
| --- | --- | --- |
| `recursive-fn` | a `fn` that calls itself, directly or through a chain | the error names the cycle; rewrite as a `for` over a finite container |
| `recursive-guard` | a guard that names itself | a guard is copied into what names it, so a cycle has no end |
| `max-tightening` | a bounded position tighter than a field written into it | widen the target's `@max`, or narrow the source's; a computed value is not this defect |
| `self-trigger` | an effect that can trigger itself through invoked commands | break the cycle, or have the command refuse a replay so the chain ends |
| `const-cycle` | a `const` that names itself | the error names the chain |

### Tests

| Code | Means | Fix |
| --- | --- | --- |
| `test-shape` | a test body out of order, or an expectation its action cannot produce | order is `given`, then `respond`/`erased`, then exactly one action, then `expect`; `run` expects events and outcomes, `project` expects rows, `deliver` expects a trace |

## Reading a run

```
$ hek check
commands/connect-shop.hk:11:18 [unknown-member] no method `trm` on String
commands/connect-shop.hk:18:8 [not-declared] event @shop.reconneced is not declared
commands/ship-order.hk:8:6 [type-mismatch] expected Bool, found String

3 errors
```

Exit status is 0 when everything parsed and every test passed, and 1 otherwise. `hek check` does not
fail on a failing test, because a test that fails is a program that parsed; `hek test` does.

**What still reaches a test rather than the checker** is arithmetic that cannot be answered exactly:
`Money(n) / Int` and `Money(n) * Decimal(s)` type-check and then fail at run time when the result is
not exact, naming `div` or `mul`. That is data-dependent, so no static check covers it. The
bare-name shorthand is **not** in this category any more: `{ sku }` is checked exactly as
`{ sku: sku }` is, at all seven positions that take one.

A test prints as `pass`, `FAIL` or `ERROR`. `FAIL` is a mismatch, which is the test doing its job.
`ERROR` is the program being unable to run at all, which is a different problem.

Reading a directory or a file at all goes to stderr with a `hek:` prefix.
