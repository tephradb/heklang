# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
