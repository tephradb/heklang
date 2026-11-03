# `hek`

The command-line checker. It reads every `.hk` file under the paths it is given as one
program, reports the first thing wrong with it, and runs the `test` declarations it found.

```
$ hek
checked 7 files
  16 events, 4 commands, 4 projectors, 3 effects, 4 fns, 1 record, 1 enum, 5 consts, 12 tests

pass   a first order for a fresh customer is appended as written
pass   an email that already placed an order is refused
...

12 passed, 0 failed
```

`tests/cli.rs` is this document as executable tests.

## Usage

```
hek [check|test|fmt|digest] [--boundaries] [--tables] [--ids] [--check]
    [--packed|--hash|--json] [--tests] [path..|-]
```

| Command | Does |
| --- | --- |
| `hek check` | parses every `.hk` file under the paths as one program |
| `hek test` | the same, then runs every `test` declaration |
| `hek` | both |
| `hek fmt` | rewrites every `.hk` file under the paths canonically |
| `hek digest` | prints what that program does, with everything else taken away |

Each `path` is a directory or a single `.hk` file, and with none given the current directory
is used.

**Several paths are one program**, which is what a module being a diagnostic label rather
than a namespace already means (`docs/modules.md`): `hek check lib folds` is the same program
as `hek check .` over a tree holding only those two, and declaration order across the files
does not matter. A file named twice, or named inside a directory already given, is read
**once**, because naming a file twice is not declaring it twice. A path that is neither a
directory nor a `.hk` file is named wherever it sits in the list.

That is a fix rather than a feature. A single slot held the path, so each argument silently
overwrote the one before it: `hek check lib folds` checked `folds` alone and reported the
types declared under `lib` as unknown, which is how a port found it, and `hek check folds lib`
checked `lib` alone and **exited zero** having looked at none of the fold declarations. A
`hek check` in CI naming three directories checked one of them and passed.

A diagnostic is labelled by the module's path inside the project, which with several paths is
read against the deepest directory all of them share **as they were written**: `hek check
lib/folds lib/events` labels a module `folds/order.hk`, which is what naming `lib` would have
given. Written and not canonicalised, because the label is stripped off the path the caller's
own argument was joined onto, and comparing the two in different spaces means the strip
silently never happens. Mixing a relative path with an absolute one therefore shares nothing,
and a label is then the path as given, which is a long answer rather than a wrong one.

A directory that holds no `.hk` file is named wherever it sits, rather than passing because
another path contributed some: `hek check lib folds commands` with `folds` renamed away is a
run that would otherwise check two directories and exit zero.

`--boundaries` adds one line per command naming what it guards and what it reads, transitively
(`docs/guards.md`). It is asked for rather than printed, because `check` is a pass/fail
gate and this is an enumeration rather than a summary: a 26-command program would pay 26
lines on every run to restate what its own `guard` lines already say. What it adds is the
part the page cannot show, which is what a guard reaches *through* another guard.

```
$ hek check --boundaries
checked 9 files
  5 events, 6 commands, 9 guards, 12 refusals, 3 consts, 21 tests

  Subscribe guards CourseIsDefined, StudentIsRegistered, CourseHasSeats
             reads CourseExists, StudentExists, CourseSubscriptionCount
             every @course.opened via CourseHasSeats
  Unsubscribe guards SubscriptionIsActive
```

The third line is the one claim the declarations no longer show. A slice with no filters
conflicts on every record of its event, which is the widest boundary a command can have, and an
arm leaves an empty filter list out (`docs/folds.md` section 5), so nothing in the fold says so
either. `every` names each such event and the guard or fold that explains it; an event with no
`via` came from one of the command's own inline folds, which has no name to give.

Informational, not a warning. A command that reads a global counter is doing nothing wrong, and a
diagnostic an author learns to ignore is worse than a line they can look at when they want it.

`--tables` prints the read model the program's named folds imply. Nothing declares it: a
fold already says what a value is, which events change it and what it is keyed by, so a
fold is a column, its parameters are the key, and the folds that share a key share a row.
Asked for rather than printed, for the reason `--boundaries` is.

```
$ hek check --tables
checked 9 files
  5 events, 2 commands, 4 folds, 1 subject, 7 tests

  (shop: Shop)
    ShopInstalled: Bool       <- @shop.installed, @shop.uninstalled

  (shop: Shop, order: Int)
    OrderCancelled: Bool      <- @order.cancelled
    OrderTotal: Money(2)      <- @order.placed
```

**A group has no name, and that is the point.** Every stored fold is in exactly one group, so
the fold's own name is a sufficient address: a test, a route and this listing all name folds.
A name derived from the key was the one thing adding an unrelated fold could change, since
two keys that joined to one name were both renamed apart, and it put a word in front of an
author that nothing they wrote spelled. `--ids` adds each group's storage id beside its key (and implies `--tables`),
for someone looking at the store itself: `t_` and sixteen hex digits of a hash over the key's
part names and types (`Table::id`), which depends on that key alone.

A key part is named by its subject when it has one and by its parameter otherwise, with
one trailing `_id` taken off. Subject parts lead, ordered so a parent sits above the
children whose keys wrap under it; plain parts follow alphabetically. A group sorts below
every group it narrows, which is what puts a one-fold group next to the forty-fold one
it should probably have been part of.

**Where two parts would take one name**, by repeating a subject or by spelling it two ways,
each is named by its **parameter** instead: `(to_id: Region, from_id: Region)` is `(from,
to)`. Never by position. A counter over declaration order made two folds declaring one
pair in opposite orders into one table whose key columns meant different parameters in
each, and a suffix is spellable, so `_1` collided with a parameter already called that.
A parameter is the one name unique within a fold, so it cannot do either.

**A part's name is not callable, so a column also records the parameters it came from.**
`Column::args` is the parameter per key position in the table's key order, which is what
`Interpreter::fold_stored` and `Tables::apply` have to be called with: `(shop_id: Shop,
request_id: Int)` keys a table on `(shop, request)` and neither of those is a parameter.
A reader matching part names back onto parameters reimplements `tables::part_names` to do
it, and gets `missing argument` for every fold the first time it is slightly wrong. Per
column and not per table, because one key signature is reached by parameters of different
names: `(shop_id: Shop)` and `(shop: Shop)` are one key.

**A fold that reads another says so**, which is `docs/folds.md` rules 15 and 16:

```
    GenreUnits: Int  <- @game.purchased, @game.refunded  (reads GameGenre, ReceiptGame)
```

Transitive, so a chain shows every link rather than the one the arm wrote. "reads" rather than
"keyed through", because a fold may be read for a key part (rule 15) or for a value (rule 16) and
the dependency is the same either way. It is the answer to "why does editing `GameGenre` refold
`GenreUnits`", and the same list is `Column::reads`: what the digest records so the dependent's
hash moves, and what `Layout::closure` adds to a rebuild.

**A fold that may be sorted on says so too**, which is `docs/folds.md` rule 20:

```
    ReviewHelpful: Int  <- @review.voted  (sorts by (game, ReviewHelpful))
```

Printed as the index a store would build rather than as the annotation the author wrote: the
parameters it names become key parts, **in the key's order whatever order they were written in**,
and the fold's own value is the last position. That is also exactly what `Column::indexes` holds,
as sorted positions into the key, so a host reading this listing and a host reading the layout see
one answer.

**A key part and a fold's column may not take one name**, and that one is refused rather than
disambiguated. `fold Shop(shop_id: Shop)` wants a row whose key column and whose only value
column are both `shop`, which nothing can create, and unlike the case above the author has
somewhere to go: a fold is named for the value and not for the entity (`docs/folds.md` rule 1),
so `ShopInstalled` is the name. The refusal is the parser's, and it reads the same derivation
this listing does.

A fold whose key no event supplies cannot be kept, and is listed at the end instead:

```
  not stored, because no event supplies the key:
    OrdersSince  (at)
```

**That is a line and not an error.** Such a fold is perfectly good and a command can read
it; nothing says every fold has to be storable. heklang has no warning channel to say it
in either (`docs/diagnostics.md` §11), so the listing says it and the run still passes.
Only *some* arm has to supply a given part, not every arm: an arm matching part of the key
is a range update over the rest.

**`hek fmt -` formats one module from stdin onto stdout**, which is the shape an editor's
format-on-save wants: helix and the editors that copied it replace the buffer with whatever
the formatter writes. That is also why a module that does not parse **fails** here rather
than printing nothing. An empty stdout and a zero status would tell the editor the file is
now empty, and it would say so on the next save, so the message goes to stderr and stdout is
left alone. `-` belongs to `fmt` and `digest`: both read one module, and a module is a whole program
(`docs/modules.md`). `check` and `test` read the tests beside the code, which is a directory.

`--check` belongs to `fmt` and turns it into a gate: it names the files that would change,
writes nothing, and exits 1 if there are any. `docs/fmt.md` is the contract for what
canonical means. `fmt` is the one command that reads a file at a time rather than the whole
program at once, because layout is a property of one file and a file whose neighbours are
missing still formats.

**`hek digest` prints the digest form**, which is what the program does with names of local
bindings, layout, comments, file boundaries and declaration order taken out of it. Hash it to find
out whether a program meaningfully changed, expand it to find out where. `docs/digest.md` is the
contract.

There is one canonical artifact and two views of it. `--packed` is the canonical form, one line per
declaration, and the only thing the hash covers; the default output is the readable expansion and
`--json` is the structural view, and nothing hashes either, so both may read better later without
moving a stored hash. `--hash` prints the hash alone, which is what a gate wants, and `--tests` adds
the `test` declarations, which are otherwise a section of their own. `--tests` says what the output
holds in every one of the four forms, and `--packed`, `--hash` and `--json` together are an error:
they are three ways of reading one answer.

Nothing is printed on the way, so `hek digest --packed hek/ | sha256sum` agrees with
`hek digest --hash hek/`. A program that does not check has no digest form, so the diagnostics go to
stderr and the status is 1. `hek digest -` reads one module from stdin, like `fmt -`.

**`-` and a path cannot both be given.** A run reads stdin or the paths, so there is nothing for a
path to mean beside `-`, and naming both used to read stdin and discard the paths without a word.
That is the same silent discard as the last-path-wins arg loop above, one argument over, and it was
found by looking for siblings of it rather than stopping at the one that was reported.

Exit status is 0 when everything parsed and every test passed, and 1 otherwise. `check`
is the pre-commit form: a failing test does not fail it, because a test that fails is a
program that parsed. Plain `hek fmt` holds to the same cut: it fails only on a file it could
not read as hek at all, not on one it changed.

## Everything under the paths is one program

There is no project file, no manifest and no list of sources. A module is a diagnostic
label rather than a namespace (`docs/modules.md`), declaration order does not matter, and
nothing has to be declared on behalf of anything else, so "every `.hk` file here" is a
complete description of a program and there is nothing left for a manifest to say.

A single file is a whole program for the same reason, which is why pointing at one works,
and so is a handful of directories: the paths are a way of saying which files, not a set of
separate programs to check one after another.

Two directories are skipped: anything beginning with `.`, and `target`. A build directory
holding a copied `.hk` file would otherwise join the program and collide with the source
it was copied from, and that error names two files that are the same file.

Files are read in sorted order, so two runs report the same way. Nothing depends on the
order semantically.

## Checking and parsing are the same pass

Every static check heklang has lives in the parser: the type check (`docs/types.md`),
rule 9's erase-last reachability, the self-trigger cycle check, rule 12's fold subject
checks and its decrypt boundary, `@subject` validation, the recursion check, and the
`@max` invariant (`docs/projectors.md`). No specified check is deferred any more, so
`hek check` runs the whole set; `docs/effects.md` records which of them have a reason to
live in the parser and which are there only because nothing else exists yet.

The type check is the one with a reason to stay rather than only a history. It has to
run while the program is lowered, because a numeric literal needs its scale to be built
at all and a narrowed optional lowers to a different node than a plain one. What is
separable is separated: `src/types.rs` holds the tables and the relations with no parser
state in them, so a checker outside the parser gets them for free.

When that checker splits out of the parser, this is where it gets called, and `check` is
the command that grows rather than a new one.

## Reporting

A diagnostic prints as `file:line:col [code] message`, with the file relative to the path
the run was pointed at, so an editor jumps to it. The position is the start of the extent
the diagnostic covers, and `docs/diagnostics.md` is the contract for both the extent and
the closed set of codes.

Under it goes the source line with the extent drawn, then the hint, then every related
location:

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

**The header is what an editor reads and the drawing is what a person reads.** The code
sits in the header rather than under the drawing, because grouping and filtering are
things a reader does to a list of headers. The message alone is in the header, for the
same reason: it is the one line, and the hint is the paragraph. The gutter is as wide as
the line number. Columns count `char`s, so the carets line up under text that is not
ASCII.

**A `= ` line is a note.** The hint is one, and so is each related location, which reads
`file:line:col: message` because it is somewhere to go too, exactly like the header. A
note wraps at 84 columns and its continuation lines up under the first word rather than
under the `=`, so one note reads as one thing. `hek` prints related locations as notes
rather than drawing each one under its own source line: a second extent is a second block,
and a list of them stops being a list.

Two cases draw less or nothing. A span with nothing in it (the end of the file, rule 5 of
`docs/diagnostics.md`) prints the header alone, because there is no line 0 to draw under.
A span ending on a later line is drawn to the end of the first and stops, since a raw
string would otherwise take the screen.

Drawing it is also what keeps it honest: an extent nobody renders is one nobody checks,
and a wrong one reads as a plausible position right up until something underlines it.

## Every mistake, not only the first

```
$ hek check
commands/connect-shop.hk:11:18 [unknown-member] no method `trm` on String
commands/connect-shop.hk:18:8 [not-declared] event @shop.reconneced is not declared
commands/ship-order.hk:8:6 [type-mismatch] expected Bool, found String

3 errors
```

**A syntax error abandons its declaration. A semantic one does not.** A token the grammar
cannot take means there is nothing left to read here, so the declaration is stepped over
whole and the next one is parsed as if nothing happened. Everything else parsed, and the
rest of the body is still worth checking: `text.trm()` is a well-formed method call and
`emit @shop.reconneced { ... }` is a well-formed emit, and until this split the first hid
the second eighteen lines away. `Code::is_syntax` is the cut, which is why the codes came
first.

**A rejected value becomes a poison.** `Expr::Invalid` has no type, and `docs/types.md`
says an unknown type is never checked, so nothing downstream of a rejected value reports a
second time. `let x = text.trm()` followed by two uses of `x` is one diagnostic, not three.
That is what makes carrying on worth doing: without it the second error inside one body
really would be the first one again, seen from further along.

**Where there is nothing to carry on with, the block is stepped over.** An `emit` whose
event is not declared has no field list to check its fields against, so the braces are
skipped and the statements after them are read. One mistake, one diagnostic, and the rest
of the body still checked.

**Reporting stops at the end of the pass that found any.** The six passes
(`docs/declarations.md`) each read what the one before it built, so an event whose fields
did not parse makes every body that names it wrong in a way its author did not write.
Reporting those would be reporting the checker's confusion. In practice the type errors
are all in pass D, so one run names every command, projector and effect with something
wrong in it.

The four whole-program checks that run after the passes (recursion, the self-trigger
cycle, a `patch`'s zero values, and the `@max` invariant) still report one at a time,
because each of them is a statement about the program rather than about a declaration.

`parse_files` keeps returning the first error alone, for an embedder that wants one;
`check_files` is what `hek` calls.

A test prints as `pass`, `FAIL` or `ERROR`, and the last two carry a reason on the line
below. `docs/testing.md` rule 10 is why the last two are separate: a mismatch is the test
doing its job, and an error is the program being unable to run at all.

Reading a directory or a file at all is a different kind of problem again, so it goes to
stderr with a `hek:` prefix and prints nothing else.
