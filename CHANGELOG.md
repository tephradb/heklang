# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
