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
hek [check|test] [path]
```

| Command | Does |
| --- | --- |
| `hek check` | parses every `.hk` file under `path` as one program |
| `hek test` | the same, then runs every `test` declaration |
| `hek` | both |

`path` is a directory or a single `.hk` file, and defaults to the current directory.

Exit status is 0 when everything parsed and every test passed, and 1 otherwise. `check`
is the pre-commit form: a failing test does not fail it, because a test that fails is a
program that parsed.

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
checks and its decrypt boundary, `@subject` validation, and the recursion check.
`docs/projectors.md` and `docs/effects.md` both record which checks are still deferred to
a checker that does not exist yet, and `hek check` does not run those, because nothing
does.

The type check is the one with a reason to stay rather than only a history. It has to
run while the program is lowered, because a numeric literal needs its scale to be built
at all and a narrowed optional lowers to a different node than a plain one. What is
separable is separated: `src/types.rs` holds the tables and the relations with no parser
state in them, so a checker outside the parser gets them for free.

When that checker splits out of the parser, this is where it gets called, and `check` is
the command that grows rather than a new one.

## Reporting

A syntax error prints as `file:line:col: message`, with the file relative to the path the
run was pointed at, so an editor jumps to it. The position is the start of the extent the
diagnostic covers, and `docs/diagnostics.md` is the contract for that extent.

Under it goes the source line, with the extent drawn:

```
a.hk:2:41: expected Money(2), found String
  |
2 |   emit @order.placed { order_id, total: text }
  |                                         ^^^^
```

**The header is what an editor reads and the drawing is what a person reads**, so the
header is exactly what it was before there were extents. The gutter is as wide as the line
number. Columns count `char`s, so the carets line up under text that is not ASCII.

Two cases draw less or nothing. A span with nothing in it (the end of the file, rule 5 of
`docs/diagnostics.md`) prints the header alone, because there is no line 0 to draw under.
A span ending on a later line is drawn to the end of the first and stops, since a raw
string would otherwise take the screen.

Drawing it is also what keeps it honest: an extent nobody renders is one nobody checks,
and a wrong one reads as a plausible position right up until something underlines it.

## Every declaration that failed, not only the first

```
$ hek check
commands/place-order.hk:14:36: expected String, found Int?
commands/ship-order.hk:8:6: expected Bool, found String
effects/notify.hk:31:8: cannot apply `>` to Money(2) and Money(3); two amounts meet at one scale

3 errors
```

**One per declaration.** A declaration that fails is stepped over whole, and the next one
is parsed as if nothing happened. Reporting several *inside* one declaration would need
the expression ladder to carry a poison value for every result it returns, which is a
rewrite of the error path rather than a place to recover; and the second error inside one
body is usually the first one again, seen from further along.

**Reporting stops at the end of the pass that found any.** The six passes
(`docs/declarations.md`) each read what the one before it built, so an event whose fields
did not parse makes every body that names it wrong in a way its author did not write.
Reporting those would be reporting the checker's confusion. In practice the type errors
are all in pass D, so one run names every command, projector and effect with something
wrong in it.

The three whole-program checks that run after the passes (recursion, the self-trigger
cycle, and a `patch`'s zero values) still report one at a time, because each of them is a
statement about the program rather than about a declaration.

`parse_files` keeps returning the first error alone, for an embedder that wants one;
`check_files` is what `hek` calls.

A test prints as `pass`, `FAIL` or `ERROR`, and the last two carry a reason on the line
below. `docs/testing.md` rule 9 is why the last two are separate: a mismatch is the test
doing its job, and an error is the program being unable to run at all.

Reading a directory or a file at all is a different kind of problem again, so it goes to
stderr with a `hek:` prefix and prints nothing else.
