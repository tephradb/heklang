# `hek`

The command-line checker. It reads every `.hk` file under a directory as one program,
reports the first thing wrong with it, and runs the `test` declarations it found.

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
hek [check|test|fmt] [--boundaries] [--check] [path|-]
```

| Command | Does |
| --- | --- |
| `hek check` | parses every `.hk` file under `path` as one program |
| `hek test` | the same, then runs every `test` declaration |
| `hek` | both |
| `hek fmt` | rewrites every `.hk` file under `path` canonically |

`path` is a directory or a single `.hk` file, and defaults to the current directory.

`--boundaries` adds one line per command naming what it guards, transitively
(`docs/guards.md`). It is asked for rather than printed, because `check` is a pass/fail
gate and this is an enumeration rather than a summary: a 26-command program would pay 26
lines on every run to restate what its own `guard` lines already say. What it adds is the
part the page cannot show, which is what a guard reaches *through* another guard.

```
$ hek check --boundaries
checked 9 files
  5 events, 6 commands, 9 guards, 12 refusals, 3 consts, 21 tests

  Subscribe guards CourseIsDefined, StudentIsRegistered, CourseHasSeats
  Unsubscribe guards SubscriptionIsActive
```

**`hek fmt -` formats one module from stdin onto stdout**, which is the shape an editor's
format-on-save wants: helix and the editors that copied it replace the buffer with whatever
the formatter writes. That is also why a module that does not parse **fails** here rather
than printing nothing. An empty stdout and a zero status would tell the editor the file is
now empty, and it would say so on the next save, so the message goes to stderr and stdout is
left alone. `-` belongs to `fmt` alone: `check` and `test` read a whole program, which is a
directory.

`--check` belongs to `fmt` and turns it into a gate: it names the files that would change,
writes nothing, and exits 1 if there are any. `docs/fmt.md` is the contract for what
canonical means. `fmt` is the one command that reads a file at a time rather than the whole
program at once, because layout is a property of one file and a file whose neighbours are
missing still formats.

Exit status is 0 when everything parsed and every test passed, and 1 otherwise. `check`
is the pre-commit form: a failing test does not fail it, because a test that fails is a
program that parsed. Plain `hek fmt` holds to the same cut: it fails only on a file it could
not read as hek at all, not on one it changed.

## Everything under `path` is one program

There is no project file, no manifest and no list of sources. A module is a diagnostic
label rather than a namespace (`docs/modules.md`), declaration order does not matter, and
nothing has to be declared on behalf of anything else, so "every `.hk` file here" is a
complete description of a program and there is nothing left for a manifest to say.

A single file is a whole program for the same reason, which is why pointing at one works.

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
below. `docs/testing.md` rule 9 is why the last two are separate: a mismatch is the test
doing its job, and an error is the program being unable to run at all.

Reading a directory or a file at all is a different kind of problem again, so it goes to
stderr with a `hek:` prefix and prints nothing else.
