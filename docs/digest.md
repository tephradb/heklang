# Digest

A deterministic, condensed rendering of what a program *does*, and a SHA-256 over it. Hash it to
find out whether a program meaningfully changed; expand it to find out where; store it and read it
back with no source tree in reach.

`hek fmt` cannot answer any of that and says so: it changes whitespace and nothing else
(`docs/fmt.md` rule 1), so two files that differ only in layout differ. Comparing two `Program`
values cannot either, because a span moves when a line does, which makes structural equality a
layout test rather than a meaning test.

`tests/digest.rs` is this document as executable tests.

## 1. The digest form is what runs

The rule everything else follows from, and also the form the IR is already in: `src/parse.rs`
lowers straight to IR while it checks, with no AST and no separate desugaring pass, so by the time
a `Program` exists the sugar is not merely normalised, it is gone.

| Written | In the digest form |
| --- | --- |
| `let total = ...` | a numbered slot; the name never existed |
| `{ sku }` and `{ sku: sku }` | the same field and the same load |
| `1000` and `1000.00` into a `Money(2)` | `(money 2 100000)` |
| `const FREE_LIMIT` at a use site | the literal it stands for |
| `reject SkuTaken { sku }` | the code and the message text, at the use site |
| `guard NotArchived { id }` | the guard's own slices, folds and statements, in the caller |
| `http.get(u)` | the headers argument, empty, because the IR always carries one |
| `[]` and `[a]` | the same `array`, though the parser builds two different nodes |
| `else if` | one `if` nested in the `else` of another |
| comments, blank lines, trailing commas, module names | nothing |

So a rename, a reformat and a move between files all leave the hash where it was, because none of
them changed what runs.

## 2. What is hashed is the packed form, not a rendering

There is one canonical artifact and two views taken from it:

```
Program (IR)  ──►  packed form  ──┬──►  expansion   (readable, for a person and for a diff)
                   (hashed)       └──►  JSON        (structural, for another tool)
```

The packed form is a **wire format**, not a rendering: one line per declaration, single spaces,
nothing anyone reformats for taste. That is what lets the expansion and the JSON change whenever
they read better without moving a stored hash, and it is the difference between hashing a decision
and hashing a punctuation choice.

The first version of this got it backwards and hashed the readable text, so every improvement to
the layout would have invalidated every stored hash. That is the whole reason `VERSION` is `2`.

## 3. Declared names stay, and are repeated rather than indexed

A name that leaves the program is kept verbatim and written out at every use: event paths, event,
record and entity field names, command and `fn` names, command parameter names, refusal codes,
entity and enum names, subject names, method names and JSON object keys. Renaming one of those *is*
a change, because something outside the program is holding the old spelling.

A name that does not leave is a slot number: a `let`, a `fold`'s state, a `for`'s bindings, a
`fn`'s parameters. A `fn`'s arguments are positional, so its parameter names are as local as a
`let`'s; a command's are not, because they are the request body's keys.

**Rejected: a string table with references into it.** It would shrink the whole form by about a
quarter, and the objection that used to apply (it makes every diff global) no longer does, because
nothing diffs the packed form. What it would cost is the readability that chose s-expressions over
an opcode stream in the first place: a stored row you can read in a database browser is worth more
than the bytes, and the median declaration is 231 of them.

## 4. A slot is numbered by first appearance, not by its frame position

The `Slot` in the IR is a position in a frame, and the layout of a frame is the parser's business: a
spliced guard's slots sit at the end of its caller's, and where `now()` lands has moved before.
Printing raw slots would make every hash move the next time either changed.

So the numbering is by first appearance in the packed form, and every slot is introduced by
something built before anything can load it: a declaration's header carries its parameters, then
`now()`, then what it binds off its trigger, and a stage carries its folds before the slices that
accumulate into them.

A bare `$n` in a value position is a load; in a binding position it is the slot itself. Nothing
else spells a slot, so the two never meet.

## 5. What the language treats as a set is sorted; what is a sequence is not

Declaration order and file boundaries carry no meaning (`docs/modules.md` rules 1 and 2), so sorting
them is not a normalisation that needs defending.

**Sorted:** the entries themselves, by kind and then by name; an event's, a record's and an entity's
fields; an enum's variants; an entity's indexes; a projector's entities, enums and handlers; an
effect's helpers and arms, and the event paths one arm lists.

An entity's `@key` column and an enum's `@default` variant are carried **by name**, because both are
stored as an index into a list this rule just sorted.

An entity's indexes are sorted for a second reason: `@index` on a column and an `index (a, b)`
clause build the same index at different positions in the list, so without sorting the two spellings
of one index would not agree. Each index keeps its own field order, because a composite index is
ordered.

**Not sorted:** parameters, statements, stages, slices, folds, array items, interpolation parts, and
a test's `given`, `respond` and `expect` lines. Each is a sequence whose order is part of what it
means. Slices are the one that could go either way, and they stay written: sorting them would have
to be settled against the slot numbering their binds hand out, and reordering fold arms is an edit
rather than another way of writing the same thing.

## 6. A field list the target declares is sorted, unless a value calls out

`emit`, `put`, `patch`, `update`, a record literal, an `invoke`'s arguments and a JSON body all name
fields the target already declares, so which order they were written in is not observable. A JSON
body is a sorted map by the time it goes out (`docs/effects.md` rule 14). These are sorted by field
name, so reordering them changes nothing.

**The exception is a value that calls out.** An `http.*`, an `invoke` and a call to an effect-local
`fn` land in the journal in the order they ran, so two of them written as sibling values in one list
*are* ordered, and a list holding one keeps the order it was written in. The test is one predicate
over the whole subtree, applied to every such list rather than to the effect ones only, because a
rule with two cases is easier to keep than a rule with two cases and a list of where they apply.

A JSON object's keys are quoted, unlike every other field name, because they are arbitrary text
rather than identifiers: without the quotes `{"a=1,b": 2}` and `{"a": 1, "b": 2}` could pack to the
same bytes.

## 7. `const`, `refusal` and `guard` are not in it

All three are inlined by the parser, so a declaration of any of them runs nothing and its content is
already at every site that uses it. Printing the declaration as well would count it twice and would
give up the property rule 1 is for.

What that buys, and what it costs:

- Renaming a `const` or moving it to another file changes nothing. Changing its **value** changes
  every declaration that names it, which is where the change actually is.
- Changing a `refusal`'s message changes every `reject` that carries it. The message is what leaves
  the program, so that is the right blast radius.
- A `guard` has no entry, and its body is carried inside every command that names it. Editing one
  therefore changes every command that uses it, which is right: they all now decide differently.
  Extracting a `guard` is **not** free, though, because the splice materialises its parameters as
  assignments in the caller. What the rule buys here is that a guard is never counted twice.
- An unused `const`, `refusal` or `guard` is invisible, so deleting one is not a change.

## 8. Every entry carries its own hash

`Digest::entries` is one `Entry` per declaration, each with the kind, the name it is known by, its
packed line and a hash of just that. So "which declarations changed?" is a comparison of two lists
rather than a diff, and a caller that only cares about one command does not have to read the rest.

An entry's hash covers the version line and that entry alone; the program's hash covers the whole
packed form.

This is the shape a caller storing digests wants:

```sql
create table declaration (
  deploy_id, kind, name, hash, signature_hash, form, signature
);
```

Two deploys then compare with a join on `(kind, name)`: rows only on the left were removed, only on
the right were added, a differing `signature_hash` is a compatibility question and a differing
`hash` alone is a change of behaviour. Expand only the rows that differ.

## 9. The signature is what is visible outside the program

A second, smaller form per entry holding only what something outside could notice, with its own
hash. Comparing signature hashes finds the candidates; walking the signature says what changed. A
body never has to be decoded to run a compatibility check.

| Kind | Signature |
| --- | --- |
| `event` | the declaration: path, fields, types, `@subject`, `@max`, `@no_index` |
| `enum` | the declaration: variants and the default, by name |
| `record` | the declaration: fields, types, `@max` |
| `command` | name, parameters in order with their types, and the refusal codes it can answer with |
| `projector` | name, and per entity: columns with types, `@max` and defaults, the key, the indexes |
| `effect` | name, and the events its arms subscribe to |
| `fn`, `test` | none; nothing outside the program can name either |

An event, an enum and a record are all shape and no body, so the signature repeats the declaration
rather than being absent. Uniform on purpose: a caller compares signature to signature without a
special case per kind.

**The refusal codes in a command's signature are the part worth arguing for.** A refusal has no
entry of its own (rule 7), so a code is otherwise only findable inside a body. Yet it is the one
declared name whose spelling leaves the program, and a client switches on it. So the signature goes
and collects the codes a command can answer with, **reaching through the `fn`s it calls**, because
`docs/functions.md` lets a helper decide a refusal on a command's behalf.

**The digest reports the shape; the caller decides what is breaking.** Whether a removed refusal
code, a widened `@max` or a new optional field counts as a break is policy, and policy does not
belong in here.

A signature is derived from the packed form rather than from the IR, so one implementation serves
both a freshly parsed program and a stored row, and the two cannot disagree about what a signature
is.

## 10. Tests are a section of their own

A `test` declaration runs nothing in production, so a change confined to one must not move the hash
a deploy gate reads. But it is still a change, so both questions are answerable:
`Digest::hash` is the program without its tests and `Digest::hash_with_tests` is everything.

**`--tests` says what the output holds, in every form it can be read in.** The packed and expanded
forms gain the section, `--hash` prints the other hash, and the JSON gains a `tests` array and the
`hash_with_tests` beside it. So every hash in a document is one taken over content that document
carries, and a reader never has to go looking for what one covered.

## 11. The version line is part of the hash

The first line is `hek-digest 2`, and it is hashed with the rest. The digest is the meaning of a
program *as this version of heklang reads it*, so a change to how the parser desugars, or to the
packed form's own spelling, is a change to what a hash means. Bumping the version moves every hash
at once, which is the point: a global change then has one legible cause instead of looking like
every declaration was edited on the same day.

## 12. A program that does not check has no digest form

`Digest::of` takes a `Program`, and a `Program` that failed to check holds `Expr::Invalid` where a
value should be. `hek digest` reports the diagnostics on stderr and exits non-zero rather than
hashing one.

## 13. The packed form reads back

`Digest::from_packed` and `Entry::from_packed` parse a stored form into the same object it came
from, signatures and all, so a caller can keep the line, come back later with no sources, and get
an expansion or a JSON tree out of it.

A form that was truncated or half-migrated **fails loudly** rather than decoding into a plausible
wrong answer, because what reads it next is deciding whether a deployment is safe.

heklang does not ship a differ. The expansion is the interface: expand both sides and hand them to
whatever diff you like.

## The form

Atoms are bare tokens (identifiers, numbers, `$0`, `@order.placed`) or double-quoted strings.
Everything else is a list whose head names the node. Types are spelled the way heklang spells them,
capitalised, which keeps them apart from the lowercase heads a value uses: `(Money 2)` the type and
`(money 2 1050)` the amount are never the same node.

| | Heads |
| --- | --- |
| declaration | `event` `enum` `record` `function` `command` `projector` `effect` `test` |
| structure | `params` `p` `f` `col` `key` `index` `entity` `on` `events` `bind` `env` `now` `stage` `pre` `post` `fold` `slice` `filter` `acc` `variants` `default` `max` `no_index` `returns` `body` `sig` `rejects` |
| statement | `set` `if` `then` `else` `emit` `put` `patch` `update` `delete` `fail` `log` `erase` `for` `in` `index` `item` `do` `discard` `call` `return` `value` `outcome` |
| type | `Bool` `Int` `String` `Uuid` `Timestamp` `Rounding` `Json` `Response` `Outcome` `(Decimal n)` `(Money n)` `(Enum N)` `(Record N)` `(List t)` `(Map k v)` `(Opt t)` `(Sealed t subject)` |
| value | `$n` `bool` `int` `dec` `money` `str` `uuid` `ts` `none` `some` `variant` `rounding` `array` `of` `map-empty` `obj` `json-num` `new` |
| expression | `neg` `not` `+ - * / % == != < <= > >= && \|\|` `.method` `field` `choose` `interp` `fn` `builtin` `invoke` `unwrap` `reveal` `reject` `invalid` `comp` `when` `yield` `bad` |
| test | `given` `respond` `status` `timeout` `erased` `run` `project` `deliver` `expect` `event` `nothing` `row` `norow` `http` `failed` `skipped` |

The JSON view turns a list into `{"kind": head, ..}`. A child that is itself a list headed by one of
the **structure** heads becomes a key of its own; everything else is a value and lands in `args`.
That is one small set to keep rather than a field table per head.

## What it looks like

Packed, which is what is hashed:

```
hek-digest 2
(event @order.placed (f customer_id Int) (f order_id Uuid) (f total (Money 2)))
(command Place (params (p order_id Uuid) (p customer_id Int) (p total (Money 2))) (stage (fold $3 Int (int 0)) (slice @order.placed (filter customer_id $1) (acc $3 Int (+ $3 (int 1)))) (post (if (>= $3 (int 10)) (then (return (reject (str "too_many_open") (str "too many open orders"))))) (emit @order.placed (f customer_id $1) (f order_id $0) (f total $2)))))
```

Expanded, which is what a person and a diff read:

```
(command Place
  (params (p order_id Uuid) (p customer_id Int) (p total (Money 2)))
  (stage
    (fold $3 Int (int 0))
    (slice @order.placed (filter customer_id $1) (acc $3 Int (+ $3 (int 1))))
    (post
      (if (>= $3 (int 10))
        (then (return (reject (str "too_many_open") (str "too many open orders")))))
      (emit @order.placed (f customer_id $1) (f order_id $0) (f total $2)))))
```

And its signature:

```
(sig command Place (params (p order_id Uuid) (p customer_id Int) (p total (Money 2))) (rejects too_many_open))
```

One child per line when a list has to break, rather than filling the width, because a filled line
reflows when anything is inserted and a diff should point at what changed.

## The tool

```sh
hek digest hek/            # the expansion
hek digest --packed hek/   # the canonical form the hash covers
hek digest --hash hek/     # only the hash
hek digest --json hek/     # the form as JSON, structurally, with every hash
hek digest --tests hek/    # include the `test` declarations, in whichever form
hek digest -               # one module from stdin
```

Nothing else is printed on the way, so `hek digest --packed hek/ | sha256sum` agrees with
`hek digest --hash hek/`.

`--packed`, `--hash` and `--json` together are an error: they are three ways of reading one answer.

## Known gaps

- **The form is not `.hk`.** It is a rendering, not a serialisation: a digest cannot be turned back
  into a module that `hek check` accepts. Two things stand in the way rather than one, and both are
  real work: `reject("code", "message")` was removed from the language, so refusal declarations
  would have to be re-synthesized from the inlined pairs; and `Expr::Unwrap` is created by narrowing
  with no token that spells it.
- **Nothing detects a rename.** Renaming a command produces a removed entry and an added one, the
  same as deleting one and writing another, because a name is the only identity a declaration has
  (`docs/modules.md` says the same about modules).
- **There is no per-module identity**, so a digest cannot say which file something came from. That
  is deliberate upstream: a module is a label for a diagnostic and nothing keys off it.
- **The form shows the order the IR runs in, which is not always the order the source reads in.** A
  command whose only declaration is a named `guard` keeps its own statements in the prologue, so
  they appear above the guard's fold. That is what the IR says and the digest reports it faithfully.
