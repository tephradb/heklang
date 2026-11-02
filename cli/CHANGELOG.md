# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.5.6](https://git.tqwewe.com/tephra/heklang/compare/v0.5.5...v0.5.6) - 2026-11-02

### Added

- @index on a fold's value says the column may be sorted on
- a fill asserts the single-row table, whose name is now reserved
- a subject key part is read through a fold, and else skip says what absent means
- a fold may not invent a subject id, so a key lookup can produce one
- a fold arm may list several event paths, lowered to the arms written out
- a fold arm leaves an empty filter list out, and --boundaries names what reads everything
- a fold arm's value may read another fold, as of the record
- a fold arm may read a key part out of another fold, as of the record
- a fold arm may compute a key part from the record it folds
- a record applied to the derived read model, and fill as a fifth test action
- a fold arm may match a key part the event does not carry
- a fold is a fourth test action, so a named fold's value can be asserted
- a fold arm may bind the envelope, which makes the fold query-only
- a named fold is a column, so hek check --tables derives the read model
- an entity's key may be several columns
- a fold can be declared once and read by name

### Fixed

- hek check reads every path it is given, as one program
- hek fmt no longer writes a blank line in a fold arm
- a fold arm's key clause breaks onto a line of its own, not its value
- a fold arm's key clause places its comments and breaks between its parts
- eight findings from a branch review, and an if expression's optional arms
- four holes in a named fold that the inline form did not have
- a statement below a guard runs after the guard decides

### Other

- Merge branch 'named-folds'

### Added

- The formatter prints `fold_definition`, `fold_body` and `fold_type`, and the `fold x = Name { .. }`
  statement form.
- `hek check` counts folds, and `--boundaries` names what each command reads as well as what it
  guards, transitively through its guards.

## [0.5.5](https://git.tqwewe.com/tephra/heklang/compare/v0.5.4...v0.5.5) - 2026-10-07

### Other

- updated the following local packages: heklang

## [0.5.4](https://git.tqwewe.com/tephra/heklang/compare/v0.5.3...v0.5.4) - 2026-09-21

### Added

- an event is a `fn`'s return type, and a `given` is checked against its declaration

### Fixed

- an empty comprehension takes its element type from its yield, not from Json

## [0.5.3](https://git.tqwewe.com/tephra/heklang/compare/v0.5.2...v0.5.3) - 2026-09-21

### Fixed

- a value-ending declaration no longer swallows the soft one below it

## [0.5.2](https://git.tqwewe.com/tephra/heklang/compare/v0.5.1...v0.5.2) - 2026-09-18

### Added

- a subject is a declared type whose values are its ids

## [0.5.1](https://git.tqwewe.com/tephra/heklang/compare/v0.5.0...v0.5.1) - 2026-09-15

### Fixed

- hek fmt counts only the files it could read

## [0.5.0](https://git.tqwewe.com/tephra/heklang/compare/v0.4.2...v0.5.0) - 2026-09-15

### Added

- [**breaking**] an answer ends the declaration, so it takes no return and no parens

## [0.4.2](https://git.tqwewe.com/tephra/heklang/compare/v0.4.1...v0.4.2) - 2026-09-12

### Other

- updated the following local packages: heklang

## [0.4.1](https://git.tqwewe.com/tephra/heklang/compare/v0.4.0...v0.4.1) - 2026-09-11

### Other

- updated the following local packages: heklang, tree-sitter-hek

## [0.4.0](https://git.tqwewe.com/tephra/heklang/compare/v0.3.1...v0.4.0) - 2026-09-08

### Added

- [**breaking**] a deployment secret is readable where the network is

### Fixed

- *(fmt)* a comment inside a construct no longer deletes source

## [0.3.1](https://git.tqwewe.com/tephra/heklang/compare/v0.3.0...v0.3.1) - 2026-09-06

### Other

- updated the following local packages: heklang

## [0.3.0](https://git.tqwewe.com/tephra/heklang/compare/v0.2.0...v0.3.0) - 2026-09-06

### Added

- [**breaking**] an effect arm names the lane it runs in, and says how much of history it wants

### Fixed

- a comment in an on header no longer swallows the rest of it

## [0.2.0](https://git.tqwewe.com/tephra/heklang/compare/v0.1.2...v0.2.0) - 2026-09-04

### Added

- a program has a digest form, so a meaningful change is one you can hash
- [**breaking**] a fold declaration is one keyword, and state is an ordinary name

### Other

- the project builds and releases on the forge it now lives on

## [0.1.2](https://github.com/tephradb/heklang/compare/v0.1.1...v0.1.2) - 2026-09-02

### Other

- updated the following local packages: heklang

## [0.1.1](https://github.com/tephradb/heklang/compare/v0.1.0...v0.1.1) - 2026-09-02

### Fixed

- a subcommand that rejects stdin may close the pipe before the test writes ([#3](https://github.com/tephradb/heklang/pull/3))
