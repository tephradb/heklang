# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- `fold_definition`, the module-scope named fold, with `fold_body` for its arm run and
  `fold_type` for the `Sealed(T, Subject)` its return position may spell. `fold_declaration` is
  now a choice of two shapes, told apart by the token after the name: `:` declares a fold here and
  `=` reads one declared at module scope, the same one-token decision `guard_declaration` makes.
- Queries for all of it: `highlights`, `locals`, `tags`, `textobjects`, `rainbows`, and
  `indents`, where a named fold joins the delimited list because it owns its braces.
- `subscript`, the `[a, b]` a `patch_statement`, a `delete_statement` and a `row_expectation`
  address a row with. It takes one `key` field per `@key` column, since an entity's key may now be
  several (`docs/projectors.md` rule 11), and it is a node of its own rather than brackets inlined
  into those three so that it **owns its delimiters**: that is what `indents.scm` keys on, and it
  is what keeps a comment written among the keys with the key it was written against. The three
  statements therefore hold a `(subscript ..)` where each held a bare `key` child, which is a tree
  change for any query matching those positionally; the `entity:` field is unmoved.

## [0.6.3](https://git.tqwewe.com/tephra/heklang/compare/tree-sitter-hek-v0.6.2...tree-sitter-hek-v0.6.3) - 2026-09-21

### Added

- an event's field block takes the bare-name shorthand every other block takes
- an event is a `fn`'s return type, and a `given` is checked against its declaration

## [0.6.2](https://git.tqwewe.com/tephra/heklang/compare/tree-sitter-hek-v0.6.1...tree-sitter-hek-v0.6.2) - 2026-09-18

### Added

- a subject is a declared type whose values are its ids

## [0.6.1](https://git.tqwewe.com/tephra/heklang/compare/tree-sitter-hek-v0.6.0...tree-sitter-hek-v0.6.1) - 2026-09-15

### Other

- the grammar's string message is the language, not a narrowing

## [0.6.0](https://git.tqwewe.com/tephra/heklang/compare/tree-sitter-hek-v0.5.0...tree-sitter-hek-v0.6.0) - 2026-09-15

### Added

- [**breaking**] an answer ends the declaration, so it takes no return and no parens

## [0.5.0](https://git.tqwewe.com/tephra/heklang/compare/tree-sitter-hek-v0.4.0...tree-sitter-hek-v0.5.0) - 2026-09-11

### Added

- [**breaking**] an event field says what it reads as before it existed

## [0.4.0](https://git.tqwewe.com/tephra/heklang/compare/tree-sitter-hek-v0.3.0...tree-sitter-hek-v0.4.0) - 2026-09-08

### Added

- [**breaking**] a deployment secret is readable where the network is

## [0.3.0](https://git.tqwewe.com/tephra/heklang/compare/tree-sitter-hek-v0.2.0...tree-sitter-hek-v0.3.0) - 2026-09-06

### Added

- [**breaking**] an effect arm names the lane it runs in, and says how much of history it wants

## [0.2.0](https://git.tqwewe.com/tephra/heklang/compare/tree-sitter-hek-v0.1.0...tree-sitter-hek-v0.2.0) - 2026-09-04

### Added

- [**breaking**] a fold declaration is one keyword, and state is an ordinary name
