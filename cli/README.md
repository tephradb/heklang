# `hek`

The command-line tool for [heklang]: a checker, a test runner and a formatter. It reads
every `.hk` file under a directory as one program, reports the first thing wrong with it,
and runs the `test` declarations it found.

```sh
cargo install hek
```

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
`hek fmt -` formats one module from stdin onto stdout, which is the shape an editor's
format-on-save wants, and `hek fmt --check` turns the formatter into a gate.

`check` and `fmt` read the language through two different front ends: `check` uses
heklang's own parser, which lowers straight to IR, and `fmt` uses the [`tree-sitter-hek`]
grammar, which keeps every byte including the comments. The grammar is a deliberate
superset, so `check` remains the gate on whether a program means anything.

**[`docs/cli.md`] is the contract**, and `tests/cli.rs` is that document as executable
tests.

[heklang]: https://github.com/tephradb/heklang
[`tree-sitter-hek`]: https://crates.io/crates/tree-sitter-hek
[`docs/cli.md`]: https://github.com/tephradb/heklang/blob/main/docs/cli.md

## License

Licensed under either of [Apache-2.0](https://github.com/tephradb/heklang/blob/main/LICENSE-APACHE)
or [MIT](https://github.com/tephradb/heklang/blob/main/LICENSE-MIT) at your option.
