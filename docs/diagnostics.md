# Diagnostics

What heklang says when something is wrong, and where it says it is.

This document is the contract. `tests/diagnostics.rs` is the same rules as executable tests. Change
the doc, the tests and the code together.

Until now a position was a point. `Span` was `{ line, col }`, `SyntaxError` was
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
a `SyntaxError`, a runtime error, and the messages that name a second declaration. Text has one place
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
  by then had moved past the name onto the `:` after it.

## 5. Where a diagnostic has no extent

Three, and they are all the same thing said differently: the parser is not at a token.

- **End of file.** The token stream ends with a sentinel whose span is `0:0`, so `unclosed `{`` and
  anything else that runs out of input reports there.
- **A check that runs after the passes.** The recursion check and the self-trigger cycle check are
  statements about the program rather than about a place in it, so each reports at a declaration's
  own span, which it has to go and find.
- **A runtime error raised outside any expression.** `interp::Error` carries a `Span` rather than an
  `Option<Span>`, and `Span::default()` is the value that means nowhere: such an error renders with
  no position at all rather than with `0:0`. There is one, and it is an internal-invariant failure.

## 6. Two tokens whose span is not what an author would draw

Both come from the lexer flattening a construct that the parser then reassembles, and both are worth
knowing before reading a span in an editor.

- **An interpolated string** is emitted flat: `TextOpen`, the first hole's tokens, a `TextPart`
  before each further hole, then `TextClose`. A `TextPart` starts at the `}` that closed the hole
  before it, not where its own text begins, because the `}` is where the scanner re-entered.
- **A string may span lines.** `"""..."""` obviously does, and a plain `"..."` also may, since the
  scanner has no rule against a literal newline inside one. So `start.line` and `end.line` differ
  more often than the shape of the language suggests, and anything drawing a span has to handle it.

## 7. What this is not, yet

A diagnostic is still a formatted `String`. There is no code, no severity, no separate hint and no
secondary span, so a reader can render the text and nothing else, and the fix that several messages
plainly describe cannot be offered as one. That is the next piece of work, and this document is where
it lands. A warning severity, and the lints that need one, come after it.
