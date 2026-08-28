# tree-sitter-hek

A [tree-sitter](https://tree-sitter.github.io) grammar for **hek** (`.hk`), the language
in this repository. It exists so an editor can highlight `.hk` sources; the authority on
the language is still `../src/lex.rs` and `../src/parse.rs`, and `grammar.js` cites them
where a rule is not obvious.

It lives beside the language rather than in its own repository so that one commit can
change the lexer and the grammar together, and so the grammar can be checked against the
real acceptance sources in `../hek` instead of a copy that drifts.

## What it does and does not do

The grammar mirrors the parser's shapes, with one deliberate widening: heklang's parser
knows whether it is inside a command, a projector, an effect, a `fn` or a test, and
refuses statements that do not belong there (`put` outside a projector, `invoke` outside
an effect, `emit` in a `fn`). A tree-sitter grammar has no such context, so every body
here accepts every statement. Nothing valid fails to parse; some invalid programs do.

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

`--abi 14` is deliberate: it is the ABI Helix 25.07 loads, and the version the other
grammars in this setup are generated at.

`src/` is **committed**, because `pkgs.tree-sitter.buildGrammar` compiles `src/parser.c`
and does not run `tree-sitter generate`. Regenerate and commit it with any grammar
change.

There are no `.hk` files in this directory on purpose: `hek check` reads every `.hk`
under a path, skipping only dot-directories and `target`, so a sample here would join the
program and produce a "declared twice" error naming two paths. The corpus is
`test/corpus/*.txt`, and real-file checking points at `../hek`.

## Helix

Helix loads `hek.so` from `runtime/grammars/` and the queries from `runtime/queries/hek/`.
For a home-manager setup, the pieces are:

`flake.nix`:

```nix
heklang = {
  url = "git+file:///home/ari/dev/tephradb/heklang";
  flake = false;
};
```

`home/modules/helix.nix`, in the `let` block:

```nix
hek-grammar = pkgs.tree-sitter.buildGrammar {
  language = "hek";
  version = "unstable";
  src = "${inputs.heklang}/tree-sitter-hek";
};
```

in `programs.helix.settings.language`:

```nix
{
  name = "hek";
  scope = "source.hek";
  injection-regex = "hek";
  file-types = [ "hk" ];
  roots = [ ];
  auto-format = false;
  comment-token = "//";
  indent = {
    tab-width = 2;
    unit = "  ";
  };
  grammar = "hek";
}
```

and beside the other `xdg.configFile` entries:

```nix
xdg.configFile."helix/runtime/grammars/hek.so" = {
  source = "${hek-grammar}/parser";
  force = true;
};
xdg.configFile."helix/runtime/queries/hek" = {
  source = "${inputs.heklang}/tree-sitter-hek/queries";
  force = true;
};
```

hek has no block comment form, so there is no `block-comment-tokens` entry.

## Queries

- `highlights.scm` — the first pattern that matches a node wins, so specific rules come
  first and the catch-alls last. Two conventions are read as rules at the bottom:
  `SCREAMING_SNAKE` is a `const` and a bare PascalCase name in a value position is an
  enum variant, neither of which a grammar can know.
- `indents.scm`, `textobjects.scm`, `locals.scm`.

Annotations (`@max`, `@key`) and event paths (`@order.placed`) are one token in the lexer
and two nodes here, so a theme can colour them apart: `@attribute` and `@label`.
