# Diagnostics

What heklang says when something is wrong, and where it says it is.

This document is the contract. `tests/diagnostics.rs` is the same rules as executable tests. Change
the doc, the tests and the code together.

Until now a position was a point. `Span` was `{ line, col }`, the diagnostic was
`{ message, line, col, file }`, and that was the whole of it. It is enough to jump a cursor to and
not enough to draw anything: an editor underlines a range, a code action needs to know what text it
is replacing, and a hover needs to know what it is hovering. So a span is now an extent.

---

## 1. A position

```rust
pub struct Pos { pub line: u32, pub col: u32 }
```

Both are **1-based**, and the column counts **`char`s**. That is what the lexer counts: it holds the
source as a `Vec<char>` and advances a line and a column as it goes, so a column is a character
offset into a line and not a byte offset into a file.

**Not a byte offset**, deliberately. An editor's position is a line and a character too, so this is
the closer fit and needs no line table to interpret. The one thing it costs is that a client
counting UTF-16 code units has a conversion to do, and that client has the source text in hand,
which is the only place the conversion can be done correctly anyway.

`0:0` is the position of nothing. It is what the end-of-file sentinel carries and what `Pos::default()`
gives, and it is how a diagnostic says it has no place to point (section 5).

## 2. A span

```rust
pub struct Span { pub start: Pos, pub end: Pos }
```

**Half-open**: `end` is one past the last character, so a span holding nothing has `start == end`.
A four-character token at line 7 column 36 is `7:36` to `7:40`.

This is the shape a range has everywhere else, which is the point of it. Half-open makes an empty
span expressible without a special case, and makes the width of a span its column difference on a
single line rather than that plus one.

**`Span`'s `Display` prints the start, and only the start.** Every rendered position goes through it:
a `Diagnostic`, a runtime error, and the messages that name a second declaration. Text has one place
to point at and a reader who can draw has the extent, so the two do not have to agree on a format.

## 3. Where an end comes from

**A token's, from the lexer, for nothing.** `Lexer::run` skips trivia, captures the position, runs a
scanner, and pushes the token. The scanner has advanced the cursor to exactly one past the token's
last character, and trivia is skipped at the top of the *next* turn, so at the push the cursor is the
token's own end and nothing else's. Every `Spanned` in the stream carries a real range.

**Everything larger, from the parser.** The tokens are a slice and the cursor is an index into it, so
the token behind the cursor is always there to be read:

```rust
fn here(&self) -> Pos          // where the next token starts
fn span_here(&self) -> Span    // the next token, whole
fn prev_end(&self) -> Pos      // the end of the token just consumed
fn span_from(&self, start: Pos) -> Span
```

`span_from` is the idiom: capture `here()` before a production, call it after. A recursive-descent
parser needs no other bookkeeping to know the extent of what it just read, which is why this was
worth doing before anything reads the extents rather than after.

**An IR node gets the same treatment.** The builder stamps each node from a cursor the parser set
from a token it held *before* it knew what it was building, so a node closes with `respan` once its
production finishes. Without that, `a + b` would carry the span of the `+`. This is what a runtime
error reads, so a `Money` mismatch at run time now covers the same text the static check would.

## 4. What a diagnostic covers

| Shape | Covers |
| --- | --- |
| a token the parser did not expect | that token |
| a name that is not in scope | that name |
| a declaration declared twice | its name in the second declaration |
| a field given twice, or one the event does not have | that field's name |
| an annotation the declaration does not take | that annotation, `@` included |
| a value in a position that declares a type | the whole value |
| an arithmetic or comparison mistake | both operands and the operator between them |
| a method the receiver does not have | the method's name |

Every one is at least a token wide, and the rule for the rest is that a diagnostic covers what it
is about rather than where the parser noticed. Those are different in three ways that matter:

- **A value is about all of itself.** `expected String, found Int?` on `text.to_int()` reported at
  its first token, which put the underline on `text` while the message described the call.
- **An operator mistake is about the pair.** `cannot apply `>` to Money(2) and Money(3)` covers
  `a > b`. The operator is where the mistake is spelled and the pair is what it is about, and an
  editor can underline only one of them.
- **A name is about the name.** A field the event does not have used to report at the cursor, which
  by then had moved past the name onto the `:` after it. An annotation had the same fault in four
  places, so `@subject(shop_id)` on a column that cannot take one was reported at its `(`.

That last one is worth its own paragraph, because it kept coming back. `emit @shop.reconneced { ... }`
underlined the `{`: `event_def` reported with the cursor, and all six of its callers had just
consumed the path. So did the two places that inlined the same message, the first path of a
multi-path arm, and `unknown type`. Nine sites, one shape, found three times before it was
looked for. A helper that reports where the cursor happens to be is reporting where its caller
left it, so `event_def` takes the span its caller had; the rule is that anything reporting about
a token it did not consume itself is given that token.

The same rule catches a check that runs *after* a whole declaration. Five of the
`declared-twice` checks read the declaration first and then reported at the cursor, which by
then was on the next declaration entirely: a second `enum E` underlined the `command` below
it. Each pass already keeps the index of the token it started at, and the name is the token
after the keyword in every declaration heklang has, so that is what the span comes from.

The lexer's own errors had the same shape of mistake, one level down. A scanner leaves the cursor
*past* the character it gave up on, and the sub-scanners could not see where their token began, so
`unexpected character` pointed one to the right of the character it named and `unterminated string`
pointed at the end of the file rather than at the quote that opened it. The lexer now keeps the start
of the token it is scanning, and an error inside one runs from there to wherever it stopped.

## 5. Where a diagnostic has no extent

Three, and they are all the same thing said differently: the parser is not at a token.

- **End of file.** The token stream ends with a sentinel whose span is `0:0`, so `unclosed `{`` and
  anything else that runs out of input reports there.
- **A check that runs after the passes.** The recursion check and the self-trigger cycle check are
  statements about the program rather than about a place in it, so each reports at a declaration's
  own span, which it has to go and find. The `@max` invariant is the exception among them: it is
  also a statement about two declarations, but the write that brings them together is one
  expression, so it has a place of its own to point at.
- **A runtime error raised outside any expression.** `interp::Error` carries a `Span` rather than an
  `Option<Span>`, and `Span::default()` is the value that means nowhere: such an error renders with
  no position at all rather than with `0:0`. Three sites build one: the `From<ErrorKind>` conversion
  a `?` uses where the caller has no place to give, an event that does not carry a field a handler
  reads, and an `invoke` of a command that is not in the program. The last two are statements about
  the log and the program rather than about a point in a file, so there is no expression to name
  even in principle.

## 6. Two tokens whose span is not what an author would draw

Both come from the lexer flattening a construct that the parser then reassembles, and both are worth
knowing before reading a span in an editor.

- **An interpolated string** is emitted flat: `TextOpen`, the first hole's tokens, a `TextPart`
  before each further hole, then `TextClose`. A `TextPart` starts at the `}` that closed the hole
  before it, not where its own text begins, because the `}` is where the scanner re-entered.
- **A string may span lines.** `"""..."""` obviously does, and a plain `"..."` also may, since the
  scanner has no rule against a literal newline inside one. So `start.line` and `end.line` differ
  more often than the shape of the language suggests, and anything drawing a span has to handle it.

## 7. A diagnostic is a struct

```rust
pub struct Diagnostic {
    pub code: Code,
    pub severity: Severity,
    pub span: Span,
    pub file: Option<String>,
    pub message: String,
    pub hint: Option<String>,
    pub related: Vec<Related>,
}
```

`src/diagnostic.rs`, a module of its own with no parser state in it, so a reader that is
not the parser can use it. It was `SyntaxError { message, span, file }`, one `String`
holding everything a reader might want to treat separately; the name also stops being
right the moment a warning can wear it.

**A code is a readable slug**, `type-mismatch` rather than `E0031`, so it says something
without a registry to look it up in. It is an enum rather than a `&'static str` so the
compiler checks that every diagnostic is in the taxonomy, which is what lets this table
be the whole of it:

| Code | Covers |
| --- | --- |
| `bad-number` | a numeric literal the scanner could not finish |
| `unterminated-string` | a string, raw or plain, that ran to the end of the file |
| `unknown-escape` | `\z` |
| `bad-path` | an `@` with no name after it |
| `unexpected-character` | a character with no token in the language |
| `expected-token` | a token the grammar cannot take here |
| `declared-twice` | a name, field, variant, arm or annotation given twice |
| `not-declared` | a name that is spelled fine and declared nowhere |
| `not-in-scope` | a name declared somewhere, and not here |
| `unknown-member` | a field, method, parameter, variant or verb the receiver has not got |
| `unknown-type` | a type name that names no type |
| `bad-type` | a type that is spelled wrong: a scale that is not a small whole number, a map key that does not order |
| `type-mismatch` | a value that does not fill a declared type |
| `bad-operands` | an operator applied to a pair it does not take |
| `bad-literal` | a literal that cannot be the type its position declares |
| `needs-target-type` | a value whose type nothing in the program decides |
| `not-a-value` | a statement written where a value was wanted |
| `arity` | a call with the wrong number of arguments |
| `missing-field` | a field, parameter or argument that has to be given and was not |
| `duplicate-field` | one given twice |
| `unknown-annotation` | `@nope` |
| `bad-annotation` | a known annotation in a place or shape it does not take |
| `empty-declaration` | a declaration whose body would be empty |
| `entity-shape` | an entity with no `@key`, or an index on a field it has not got |
| `event-shape` | an arm over event types with nothing in common |
| `refusal-shape` | a refusal named or written in a way its derived code could not survive |
| `stage-shape` | a `fold` or `guard` written where or how it does not go |
| `no-zero-value` | a `patch` that would materialise a row it cannot fill |
| `wrong-context` | a statement in a declaration kind that does not have it |
| `impure-fn` | a `fn` doing something a pure function cannot |
| `fold-restriction` | a `fold` calling out or decrypting |
| `arm-only` | an effect-local `fn` doing what stays in the arm |
| `return-shape` | a `return` that does not match the signature it is in |
| `seal-boundary` | rule 12: sealed content leaving without `reveal` |
| `erase-subject` | an `erase` whose subject or id is not one |
| `erase-order` | rule 9: a `reveal` reachable from an `erase` |
| `test-shape` | a test body out of order, or an expectation its action cannot produce |
| `recursive-fn` | a `fn` that calls itself |
| `recursive-guard` | a guard that names itself |
| `max-tightening` | a `@max` position bounding tighter than the field written into it declares |
| `self-trigger` | an effect that can trigger itself |
| `const-cycle` | a `const` that names itself |

**The context codes are what keeps the table this short.** About thirty sites are the
same handful of defects reported differently per declaration kind, because each context
gets a message about that context: `emit` in a projector, in an effect and in a test read
as three sentences and are one defect. The code carries the defect and the prose carries
the context, so those thirty sites are six codes and no message changed.

**`is_syntax` is the cut that matters to the reader.** The lexical codes and
`expected-token` mean the token stream stopped making sense, so there is nothing left to
read in that declaration. Everything else parsed and then failed a check.

**Severity is a channel with no producer.** Nothing is a warning yet. It is a field
rather than a later addition because the lints that need it plug into this shape instead
of changing it again, and because a warning never travels in `Err`: it does not stop a
parse, so it belongs beside the errors rather than instead of a result.

## 8. A message and a hint

**The message says what is wrong. The hint says why, or what to do about it.**

```
expected String, found Int?
  = `unwrap_or` gives it a fallback, or a branch that proves it present makes it a String
    without one
```

They were one string, joined by a `; `, in about eighty messages and in five helpers that
built the tail from the types they were given. As one string a reader could render it or
not, and nothing else; apart, a hover can show the first line and a panel the rest, and
the hints that name a replacement can become an offer rather than prose.

**`Diagnostic::text` joins them back**, with the same `; `, so a reader with one line to
give gets what it always got. That is what `Display` uses, and it is why splitting eighty
messages changed no assertion in the suite beyond the field it reads: the wording is the
same wording.

Two messages did change, and they are the shape to avoid. Their advice began `, and ...`,
which reads as a clause of the sentence before it rather than as a line of its own. A hint
stands alone or it is not a hint, so both were reworded to start after a `; ` like the
rest, which is the only text this work altered.

**Some hints name a replacement**: ``write `let {name} = <seed>` ``, ``did you mean `{name}()`?``,
``move that `let` up``, ``write `for key, value in ...` ``. Those are what a code action
will be built from, and they are the reason `hint` is a field rather than a suffix. It is
not enough on its own: an applicable edit needs a span to replace and the text to put
there, and a hint carries neither. It is the sentence a person reads, and the marker for
where the edits are.

## 9. A second place worth looking

```rust
pub struct Related { pub span: Span, pub file: Option<String>, pub message: String }
```

Some diagnostics are about two places. Until now they said so in prose:

```
a.hk:3:9: command `C` is declared twice
```

which underlines the second `C` and never says where the first is. Three shapes, in
ascending order of how much they were missing:

**A position written into a sentence.** ``the id at 7:12 was learned by revealing`` and
four others interpolated a `line:col`, four of them without even a file name. A position
in prose is not somewhere an editor can go, and it is not something a person reads either.
Those stop naming a position and carry a `Related` instead.

**A chain flattened into a prefix.** ``` `a` calls `b` calls `c` calls `a` ``` and
`@x -> E -> C -> @x` are relations over declarations written as one line of text. Here the
prose stays, because names are what an author reads and a list of spans is not a sentence;
each link also becomes a `Related`, so the loop can be walked. The link that closes the
cycle is the primary span, so it is not repeated.

**`declared-twice`, which had nothing to give.** The duplicate checks walk lists of
declarations that carry no position, so there was nothing to name even had the message
wanted to. What they consult now is one map beside them, `(kind, name) -> Related`, filled
as each declaration is accepted. A map rather than a span on every IR declaration, because
the declaration types are read by the interpreter and this is a question only the parser
asks. The kind is part of the key because `record C` and `command C` are different names,
and a projector's own `enum` and `entity` are tracked inside the projector rather than in
that map, because two projectors may each declare a `Status`.

**A related location carries its own file.** A second declaration is often in another
module, and four of the five messages that named a position never said which.

## 10. What a diagnostic does not stop

A diagnostic used to end the declaration it was found in, whatever it was, because
`Result<_, Diagnostic>` was the only channel the parser had. That is right for a token the
grammar cannot take and wrong for everything else: `text.trm()` is a well-formed method
call and `emit @shop.reconneced { ... }` is a well-formed emit, and the first used to hide
the second eighteen lines down the same command.

**`Code::is_syntax` is the cut.** The lexical codes and `expected-token` abandon the
declaration. Every other code records and carries on, so the rest of the body is checked.

**`Expr::Invalid` is the poison.** A rejected value lowers to it, its type is `None`, and
`docs/types.md` says an unknown type is never checked. So a value the checker refused
reports once and every position it was written into stays quiet: this is the same device
`TyKind::Error` is in rustc and `errorType` is in TypeScript, and heklang had half of it
already in `type_of` returning an `Option`. Without it, carrying on would turn one
mistyped name into twenty diagnostics, which is worse than stopping.

It never reaches the interpreter. `check_files` fails whenever anything was recorded, so a
program holding a poison is a program that did not check; `eval` answers `MalformedIr` if
one ever arrives, which is a defect in the checker rather than a case in the language.

**Where there is nothing to carry on with, the block is stepped over.** An `emit` whose
event is not declared has no field list to check its fields against. Rather than guess, the
braces are skipped and the statements after them are read.

**Recording is not a mutation.** A semantic check deep in the expression ladder holds
`&self` and has nothing to give back but the value it was handed, so the diagnostics live
behind a `RefCell` rather than being threaded back up through every signature. This is what
rustc does with `DiagCtxt` and for the same reason.

**Declaration headers still abandon.** A parameter whose type does not exist leaves nothing
coherent to check the body against, so those keep returning. So does a syntax error, which
is what every compiler does: recovering from one is a parser question rather than a
diagnostic one.

## 11. What this is not, yet

Severity is a channel with no producer: nothing is a warning. The lints that need one are
the next piece of work, and this document is where they land.
