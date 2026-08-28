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

Every static check heklang has lives in the parser: rule 9's erase-last reachability, the
self-trigger cycle check, rule 12's fold subject checks, `@subject` validation, and the
recursion check. `docs/projectors.md` and `docs/effects.md` both record which checks are
still deferred to a checker that does not exist yet, and `hek check` does not run those,
because nothing does.

When that checker splits out of the parser, this is where it gets called, and `check` is
the command that grows rather than a new one.

## Reporting

A syntax error prints as `file:line:col: message`, with the file relative to the path the
run was pointed at, so an editor jumps to it. Parsing stops at the first one.

A test prints as `pass`, `FAIL` or `ERROR`, and the last two carry a reason on the line
below. `docs/testing.md` rule 9 is why the last two are separate: a mismatch is the test
doing its job, and an error is the program being unable to run at all.

Reading a directory or a file at all is a different kind of problem again, so it goes to
stderr with a `hek:` prefix and prints nothing else.
