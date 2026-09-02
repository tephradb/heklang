# `hek fmt`

```
$ hek fmt --check hek/
orders.hk would be reformatted

1 file would change
```

One canonical layout per program. `hek fmt` rewrites, `hek fmt --check` names what would
change and writes nothing, which is the pre-commit form. `cli/tests/fmt.rs` is this document
as executable tests.

## 1. Formatting changes whitespace and nothing else

The property everything else rests on, and the one you have to be able to take on faith
before running this over a tree you care about.

It is checked rather than asserted. `formatting_changes_only_whitespace` lexes both sides
with heklang's own lexer and compares the token streams, and the only difference it forgives
is a comma before a closing delimiter, which is the one token `fmt` adds and removes. Beside
it, `every_comment_survives` counts the comments and `formatting_twice_is_formatting_once`
demands a fixed point.

**A `Program` comparison would have been the obvious check and it is the wrong one.** The IR
carries a `Span` in dozens of places, and every one of them legitimately moves when text
moves, so two programs that differ only in layout are not equal and nothing useful could be
concluded from that. The token stream is the thing that is supposed to be identical.

## 2. It reads the grammar, not the parser

`hek check` goes through `src/parse.rs`, which lowers straight to IR and throws comments away
with the rest of the trivia. There is no tree to print back and no comment to print, so `fmt`
goes through `tree-sitter-hek/` instead, which keeps every byte.

That grammar is a deliberate superset: it does not know whether it is inside a command or a
projector, so it accepts `put` in a command. **So `fmt` is not a validity gate.** It will
format a program `check` rejects, and the only thing that stops it is a file that does not
parse at all, which it names and leaves alone rather than failing the run.

## 3. A group is one line or one line each

A delimited list is laid out flat if it fits in **90 columns** and one item per line if it
does not, with a trailing comma when and only when it broke.

The width is measured rather than chosen. Across the 83 `.hk` files in existence, code sits
at p95 = 87 columns, hand-wrapped comments have a sharp cliff at 87-88, and command
signatures break in an 88-97 band. One number covers all three. 80 would reflow 6.4% of code
and about half of all comments, which is restyling rather than codifying; 100 would change
almost nothing.

The broken form is the one the corpus already writes everywhere: `(` hangs at the end of the
header, items sit one per line at +2, and the closing delimiter is alone and flush.

## 4. Three shapes, and the difference is not the braces

The same `{ }` is a sequence in one place and a list in another, so the partition is by node
and not by delimiter.

| | behaviour | where |
| --- | --- | --- |
| **sequence** | always breaks; an authored blank line between children is kept | a module, a `block`, a `test` body, a `projector`, an `effect`, a `record`, an `event`, an `entity`, and a fold's arms |
| **one line** | never breaks and holds no list of its own | a `const`, and a `refusal` (whose parameter list is still a list) |
| **list** | fits or breaks; a trailing comma only when broken | `enum` variants, parameters, arguments, annotations, field initializers, object literals, list literals, slice filters, a destructure |
| **flat** | never breaks, and overflows instead | `for` bindings, `Map(K, V)`, `Money(2)`, and the raw `guard` slice list |

An `on` header's path list is a list with one difference: **it takes no trailing comma**,
because it has no closing delimiter and a comma there would be followed by `as`. Its
continuations are indented rather than aligned under the first path.

**`record`, `event` and `entity` always break; `enum` does not.** Measured: no `record`,
`event` or `entity` in the corpus is written on one line, and every `enum` is, seven of seven
with no multi-line counterexample. That is the corpus telling a type declaration apart from a
value enumeration, which is the line rustfmt draws at a named-field struct and Prettier at a
TypeScript interface.

**Three of the flat entries are correctness, not taste.** `Map(K, V)` and `for a, b in c`
read their comma with `expect_sym` and have no trailing-comma escape, and `guard_decl` in
`src/parse.rs` is the one comma loop in the language that unconditionally parses another
slice after eating a comma. So `guard @order.placed(id),` does not parse, and a list that
broke and added a comma there would emit a file that no longer compiles.

## 5. Blank lines are the author's

Layout is canonical; **a blank line between two statements is not**, because it carries
meaning nothing in the tree records. A run of two or more collapses to one, and none is ever
added or removed otherwise.

The corpus is emphatic about this. `const` runs are packed by what they are for and
blank-separated between groups. A `test` body separates arrange from act from assert:
`given` is preceded by a blank 0 times out of 459 and `deliver` 53 out of 53. Three folds
that answer one question are written packed, while 75 other `fold` declarations are
separated. No rule derivable from the tree gets all of that right, and the author already
got it right.

Inside a list a blank line is deleted, which costs nothing: there are zero blank lines
inside a comma-separated list in the whole corpus.

## 6. A comment is a line

Comments are `extras` in the grammar, mentioned by no rule and listed in `node-types.json` as
nobody's child, so a printer written against that schema drops every one. This one iterates
all children of every node instead, which is also why the grammar's missing fields cost
nothing.

Every comment in 9,531 lines is on its own line, and 501 of the 713 sit at column 0
documenting a declaration. So the rule is short: a comment leads whatever follows it,
consecutive comments are one block, a comment with nothing after it before the closing brace
is emitted anyway rather than lost, and **a comment anywhere inside a list forces that list
to break** because a line comment would otherwise swallow the rest of the line.

A comment written after code on the same line keeps its place. Nothing in the corpus writes
one, but the grammar allows it, and a comment that migrated below the statement it describes
would make section 1's claim conditional.

## 7. Some things are copied, not printed

A **string** is reproduced byte for byte and never descended into. `token.immediate` anchors
the whole body, so the slice is exact, and `\{` against `{` is a distinction no reprinting is
allowed to lose. A `"""` body is the sharper case: fifteen of them hold GraphQL starting at
column 0 regardless of the nesting around them, so re-indenting one would corrupt another
language.

The same goes for a number, whose token precedence is the only thing keeping `1.5` from being
three tokens.

## What this deliberately does not do

- **No parentheses are removed.** `parenthesized_expression` is a real node, so reprinting it
  needs no precedence reasoning at all. Elision has a trap waiting: the grammar declares
  comparison `prec.left` while `src/parse.rs` `cmp_expr` makes it non-associative, so
  `(a < b) < c` parses in the grammar and is an error in the language. A naive "drop the
  parens when the child binds tighter" rule would emit a file that stopped compiling.
- **A chain never breaks at its dots.** `receiver` is an ordinary expression, so `a.b().c()`
  left-nests and the obvious grouping would break the innermost link first, which is
  backwards. Four lines in 9,531 begin with a dot, so overflowing is closer to the corpus
  than any breaking rule, and it costs a hand-broken chain in `lib/webhook.hk` that paired
  each accessor with its `unwrap_or`.
- **A boolean condition never breaks.** No line in the corpus ends in `&&` or `||`: where one
  would be too long, its author extracted a `fn` or a `let`. Inventing a wrap style the
  language has never used would be worse than letting it overhang.
- **The last argument does not hug.** Prettier would keep `http.post(url, {` on one line and
  break only the object. A plain group breaks every argument instead, which is the form 17 of
  the corpus's 81 `http.*` calls already use.
- **Comments are not re-wrapped.** They are hand-wrapped at 87-88 columns and a formatter
  that reflowed prose would fight its author over every edit.
- **The one column alignment in the language is not reproduced.** A multi-path `on` header
  was written with its continuations aligned under the first path, five columns in. They are
  indented two instead. An alignment mode would exist for that one construct and would have
  to thread a second kind of indentation through the whole renderer, and the header still
  breaks in the same places without it.

  This was very nearly got wrong. The first attempt made the whole header flat on the
  evidence that the corpus writes a 103-column header rather than break a destructure, and
  that the one aligned site flattens to 77 columns. Both facts are true and the conclusion
  was not: run over a real application it produced a **328-column line** for a handler
  listing twelve paths, and a 267-column one for a destructure of eighteen fields, which the
  author had written packed across four lines. Widths measured on `hek/` alone would not
  have caught either.
- **There are no options.** No width flag, no indent flag. A formatter with settings is a
  formatter with arguments about settings.

## Known gaps

- **An expression inside a string interpolation is not formatted.** The hole holds an
  ordinary expression and the grammar parses it as one, but the string is emitted as a slice,
  so `"{ a+b }"` keeps its spacing. Descending would mean splicing around the hole while
  never walking the string itself, since `extras` are live inside one and a comment would
  otherwise be printed twice.
- **`fmt` reads a file at a time and `check` reads them all at once.** That is right, since
  layout is a property of one file, but it does mean `fmt` cannot use anything the checker
  knows.

## Related

- `docs/cli.md`: the binary, its exit statuses, and how `path` is swept.
- `tree-sitter-hek/README.md`: the grammar, why its generated `src/` is committed, and the
  obligation that creates now that a formatter compiles it.
- `docs/diagnostics.md`: what `check` reports, which is the other half of a pre-commit gate.
