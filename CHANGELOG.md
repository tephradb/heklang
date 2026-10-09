# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Named `fold` declarations. A fold can now be declared once at module scope and read by name,
  which is the value half of what `guard` does for a proposition: `fold spend = LifetimeSpend {
  customer_id }` puts the same slices in the same append condition as writing the fold inline.
  One keyword in two positions, parens to declare and braces to use, as `command`/`invoke` and
  `guard`/`guard` already are. Inline folds are unchanged and still the default; `docs/folds.md`
  rule 9 is the choice between them.
- `Sealed(T, Subject)` is spellable in a fold's return type and nowhere else, so a fold can hand
  back content from behind the decrypt boundary for its caller to `reveal`. The four most
  duplicated folds in the port that measured the construct are sealed.
- `unused-accumulator`: a named fold that binds an accumulator name no arm reads. A fold's value
  is its declaration's name, so the name is written only where an arm reads the running value.
- `hek check --boundaries` names what a command reads as well as what it guards, transitively
  through its guards.

### Changed

- The digest version is `hek-digest 5`. A statement written below a `guard` moves from the stage's
  `(pre ..)` to its `(post ..)`, which is where `docs/guards.md` rule 3 always said it was, so the
  packed form of a command with one changes. Measured on a 25,347-line application: one declaration
  of 84 commands renders differently, and the `hek/` corpus does not change at all.

### Fixed

- **A statement below a `guard` ran before the guard decided.** A bare `return`, an `invalid` or a
  conditional `reject` under `guard G { .. }` answered first and `G` was skipped, with nothing
  reported. A guard declares no slice at parse time, so the stage looked empty and the statement was
  taken for one written *above* the declarations. An `emit` there was survivable, because a refusal
  replaces the outcome; the others were not.
- `hek fmt` deleted a named fold's return type and seed, and a sealed return's subject, when a
  comment appeared anywhere in the header, leaving source that no longer parsed. The three new
  printers indexed positionally over a child list that includes comments; they use `parted` now,
  and the `INTERLEAVED` fixture grew the cases that catch it.
- A named read's destination slot was not marked pending, so `docs/folds.md` rule 2's "an argument
  may not read a fold from its own stage" missed every named fold: a read's argument, a guard's
  argument, a seed and a filter all passed `hek check` and then died at run time with "slot read
  before it was set".
- A guard or a fold spliced into a command's **second** stage landed past the statements already
  there, so the caller could read a slot the splice assigned afterwards. The insert offsets are now
  per stage rather than per caller. Reachable with guards alone, and rare only because
  `docs/guards.md` rule 3 puts guards at the top.
- A guard whose body only reads named folds is no longer reported as folding nothing.

## [0.11.0](https://git.tqwewe.com/tephra/heklang/compare/heklang-v0.10.0...heklang-v0.11.0) - 2026-09-21

### Added

- an event's field block takes the bare-name shorthand every other block takes
- an event is a `fn`'s return type, and a `given` is checked against its declaration
- `&&` narrows where it is true and `||` where it is false, operands included
- `List.sum()` totals where `+` is defined, and `List.concat` joins two lists

### Fixed

- an empty comprehension takes its element type from its yield, not from Json
- a loop body cannot accumulate into an outer `let`, and a branch is a scope

### Other

- update heklang skill around projectors

## [0.10.0](https://git.tqwewe.com/tephra/heklang/compare/heklang-v0.9.0...heklang-v0.10.0) - 2026-09-21

### Fixed

- an erase empties the sealed column too, and a seal cannot be put in a box
- a value-ending declaration no longer swallows the soft one below it

### Other

- a composite seal is opened by reveal, never read part by part

## [0.9.0](https://git.tqwewe.com/tephra/heklang/compare/heklang-v0.8.0...heklang-v0.9.0) - 2026-09-18

### Added

- a subject is a declared type whose values are its ids
- the fixed-length timestamp units, and Int.pad(width)

### Fixed

- a sealed record, list or map reveals as what it was sealed from

### Other

- the demo's address is a record, sealed whole

## [0.8.0](https://git.tqwewe.com/tephra/heklang/compare/heklang-v0.7.0...heklang-v0.8.0) - 2026-09-15

### Added

- [**breaking**] an answer's message is written, not computed

### Fixed

- hek fmt counts only the files it could read

### Other

- the grammar's string message is the language, not a narrowing

## [0.7.0](https://git.tqwewe.com/tephra/heklang/compare/heklang-v0.6.1...heklang-v0.7.0) - 2026-09-15

### Added

- [**breaking**] an answer ends the declaration, so it takes no return and no parens

## [0.6.1](https://git.tqwewe.com/tephra/heklang/compare/heklang-v0.6.0...heklang-v0.6.1) - 2026-09-12

### Added

- a string can be cut to the bound it is written into

## [0.6.0](https://git.tqwewe.com/tephra/heklang/compare/heklang-v0.5.0...heklang-v0.6.0) - 2026-09-11

### Added

- [**breaking**] an event field says what it reads as before it existed

## [0.5.0](https://git.tqwewe.com/tephra/heklang/compare/heklang-v0.4.0...heklang-v0.5.0) - 2026-09-08

### Added

- [**breaking**] a deployment secret is readable where the network is

### Fixed

- *(fmt)* a comment inside a construct no longer deletes source

## [0.4.0](https://git.tqwewe.com/tephra/heklang/compare/heklang-v0.3.0...heklang-v0.4.0) - 2026-09-06

### Added

- [**breaking**] an absent value compares unequal to a present one

## [0.3.0](https://git.tqwewe.com/tephra/heklang/compare/heklang-v0.2.0...heklang-v0.3.0) - 2026-09-06

### Added

- [**breaking**] an effect arm names the lane it runs in, and says how much of history it wants

### Fixed

- a comment in an on header no longer swallows the rest of it

## [0.2.0](https://git.tqwewe.com/tephra/heklang/compare/heklang-v0.1.2...heklang-v0.2.0) - 2026-09-04

### Added

- a program has a digest form, so a meaningful change is one you can hash
- [**breaking**] a fold declaration is one keyword, and state is an ordinary name

### Other

- the forge token secret avoids the reserved FORGEJO_ prefix
- the project builds and releases on the forge it now lives on

## [0.1.2](https://github.com/tephradb/heklang/compare/heklang-v0.1.1...heklang-v0.1.2) - 2026-09-02

### Fixed

- the bare-name shorthand is the same declared position as the long form

### Other

- the language as a skill an agent can write it from
- four ways to install the tool, now that there are four ([#4](https://github.com/tephradb/heklang/pull/4))

## [0.1.1](https://github.com/tephradb/heklang/compare/heklang-v0.1.0...heklang-v0.1.1) - 2026-09-02

### Fixed

- a subcommand that rejects stdin may close the pipe before the test writes ([#3](https://github.com/tephradb/heklang/pull/3))

### Other

- three crates that publish, and the grammar is one of them ([#1](https://github.com/tephradb/heklang/pull/1))
