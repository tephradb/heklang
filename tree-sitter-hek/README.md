# tree-sitter-hek

A [tree-sitter](https://tree-sitter.github.io) grammar for **hek** (`.hk`), the language
in this repository. It exists so an editor can highlight `.hk` sources **and so `hek fmt`
has a tree to print back**; the authority on what the language *means* is still
`../src/lex.rs` and `../src/parse.rs`, and `grammar.js` cites them where a rule is not
obvious.

The formatter changed what a mistake here costs. `../cli/build.rs` compiles `src/parser.c`
into the `hek` binary, so this is a build input rather than an editor convenience, and the
"regenerate and commit `src/`" rule below is now load-bearing: skip it and `hek fmt` parses
a language nobody is writing. `../cli/tests/grammar.rs` is the standing check that the
grammar and the acceptance sources in `../hek` still agree, which until now was a command
someone had to remember to run.

It lives beside the language rather than in its own repository so that one commit can
change the lexer and the grammar together, and so the grammar can be checked against the
real acceptance sources in `../hek` instead of a copy that drifts.

## What it does and does not do

The grammar mirrors the parser's shapes, with one deliberate widening.

heklang's parser knows whether it is inside a command, a projector, an effect, a `fn` or
a test, and refuses statements that do not belong there (`put` outside a projector,
`invoke` outside an effect, `emit` in a `fn`). A tree-sitter grammar has no such context,
so every body here accepts every statement. Nothing valid fails to parse; some invalid
programs do.

An `effect` body takes `fn` helpers beside its `on` arms; a `projector` body does not,
and a `fn` is not a statement. That asymmetry is real rather than an oversight, so the
grammar keeps it: `effect_decl` sweeps for `fn` and `on`, `projector_shell` for `enum`,
`entity` and `on`.

Two things are decided by lookahead the way the parser decides them with a flag:

- `on @p { a, b } { ... }` against `on @p { ... }` — a destructure is a block with
  another block after it (`parse.rs` `has_destructure`).
- `if plan { ... }` against `Item { ... }` — a record literal loses to a block wherever
  both would parse, which is what `no_record_literal` does in `parse.rs` `header_expr`.

## Building

Neither `tree-sitter` nor `node` is on `PATH` by default; `devenv.nix` now carries both,
or use an ad-hoc shell:

```sh
nix shell nixpkgs#tree-sitter nixpkgs#nodejs
```

Then, from this directory:

```sh
tree-sitter generate --abi 14   # regenerate src/ after editing grammar.js
tree-sitter test                # the corpus in test/corpus
tree-sitter parse ../hek/*.hk --quiet --stat   # must be 100%, and it is
tree-sitter build -o hek.so     # the shared object an editor loads
```

`--abi 14` is deliberate: it is the ABI Helix 25.07 loads. Bare `generate` uses whatever the
installed CLI defaults to, which is newer, and the mismatch is only found when the editor
declines to load the parser.

`src/` is **committed**, because `pkgs.tree-sitter.buildGrammar` compiles `src/parser.c`
and does not run `tree-sitter generate`. Regenerate and commit it with any grammar
change.

There are no `.hk` files in this directory on purpose: `hek check` reads every `.hk`
under a path, skipping only dot-directories and `target`, so a sample here would join the
program and produce a "declared twice" error naming two paths. The corpus is
`test/corpus/*.txt`, and real-file checking points at `../hek`.

## Helix

Two halves: the compiled grammar and the queries beside it. `hek fmt` is worth wiring at the
same time, since it reads the same grammar.

`languages.toml`:

```toml
[[language]]
name = "hek"
scope = "source.hek"
injection-regex = "hek"
file-types = ["hk"]
roots = []
comment-token = "//"
indent = { tab-width = 2, unit = "  " }
formatter = { command = "hek", args = ["fmt", "-"] }
auto-format = true

[[grammar]]
name = "hek"
source = { path = "/path/to/heklang/tree-sitter-hek" }
```

hek has no block comment form, so there is no `block-comment-tokens` entry.

Then `hx --grammar build`, and put this directory's `queries/` at
`runtime/queries/hek/` under Helix's configuration directory.

**`auto-format` is safe because of how `hek fmt -` fails.** It reads one module on stdin and
writes it back on stdout, and on a module that does not parse it exits non-zero with **stdout
empty**. Helix replaces the buffer with what a formatter writes, so a formatter that printed
nothing and succeeded would empty a file the moment it stopped parsing mid-edit. Instead the
error is reported and the buffer is left alone.

### With nix

The repository is a flake, so the binary and the grammar come from one commit and cannot
drift apart:

```nix
inputs.heklang.url = "github:owner/heklang";
```

```nix
hek = inputs.heklang.packages.${pkgs.system}.hek;
hek-grammar = inputs.heklang.packages.${pkgs.system}.tree-sitter-hek;
```

The grammar derivation carries the queries with it, so both linked into Helix's runtime is:

```nix
"helix/runtime/grammars/hek.so".source = "${hek-grammar}/parser";
"helix/runtime/queries/hek".source = "${hek-grammar}/queries";
```

and the formatter is `"${hek}/bin/hek"` with `[ "fmt" "-" ]`.

**A `path:` input rather than `github:` or `git+file:` is worth it while the language is
moving**: `path:` reads the working tree, so a grammar change reaches Helix after a
`nix flake update` with no commit in between. Point it at this directory rather than the
repository root, which carries a multi-gigabyte `target/` that a `path:` input would copy.
A flake input excludes it for free, which is the trade the two shapes are between.

### After changing the grammar

```sh
tree-sitter generate --abi 14 && tree-sitter test
```

then rebuild whatever installed the grammar; a content-locked input needs its lock refreshed
before the change is visible. `hx --health hek` should show six ticks. An `hek.so` built in
this directory is for `tree-sitter` command-line use only, and goes stale silently: it is not
what Helix loads.

## Queries

- `highlights.scm` — **the LAST matching pattern wins** (helix book,
  `guides/adding_languages.md`), so the file runs general to specific: catch-alls at the
  top, the most specific rules at the bottom. That is the shape helix's own
  `runtime/queries/rust/highlights.scm` has, with `(identifier) @variable` on line 9.
  Getting this backwards is silent: every rule still matches, the catch-all just wins
  them all, and every identifier reads `variable`.
  Two conventions are encoded as `#match?` rules near the top, since a grammar cannot
  know them: a bare PascalCase name in a value position is an enum variant, and
  `SCREAMING_SNAKE` is a `const`. The const rule comes second because a
  `SCREAMING_SNAKE` name matches the PascalCase pattern too and has to win.
- `indents.scm`, `textobjects.scm`.
- `locals.scm` — the capture must be `@local.definition.<scope>`, not a bare
  `@local.definition`: the class after the prefix **is** the highlight a resolved
  reference gets. With a bare one a parameter renders `variable.parameter` at its
  declaration and plain `variable` at every use, because the use falls through to the
  `(identifier) @variable` catch-all. `:tree-sitter-highlight-name` in Helix is how to
  check. The `@_` captures at the bottom keep a field or method that shares a name with
  an in-scope local from taking that local's colour.
- `tags.scm` — hek has no language server, so this is all the syntax symbol picker has
  to work with: every top-level declaration, plus a projector's `enum` and `entity`.
- `rainbows.scm`.

Annotations (`@max`, `@key`) and event paths (`@order.placed`) are one token in the lexer
and two nodes here, so a theme can colour them apart: `@attribute` and `@label`.
