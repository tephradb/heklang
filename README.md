# heklang

A small, **total** language for event-sourced application logic. It is the module language for
[hekla], a single-app event-sourcing runtime over the Dynamic Consistency Boundary.

Five kinds of declaration do the work. A **command** replays the history its decision depends on
and appends events. A **projector** consumes events into a read model. An **effect** reacts to
appended events with durable side effects, and is the only one that reaches the network. A
**guard** is a named proposition about the log that several commands can share, and a **refusal**
is a named reason one said no.

**The restrictions are the point.** A command cannot call out, read a clock or decrypt; a
projector has no failure channel; a fold cannot observe anything but the log. Each is true because
of what kind of declaration it is, not because something checks at run time, which is what lets a
projector rebuild and an effect replay reproduce exactly what they did the first time.

heklang is also *total*: there is no `while`, recursion is rejected statically, a `for` runs once
per element of a finite container, and every path must return. Every program terminates. A smart
contract language buys that guarantee at run time with gas metering; here it is not expressible in
the first place.

[hekla]: https://git.tqwewe.com/tephra/hekla

## What it looks like

```hek
event @order.placed {
  order_id: Uuid,
  customer_id: Int,
  email: String @subject(customer_id) @max(200),
  total: Money(2),
}

event @order.cancelled {
  order_id: Uuid,
  customer_id: Int,
}

refusal TooManyOpen "this customer has too many open orders"

command PlaceOrder(order_id: Uuid, customer_id: Int, email: String, total: Money(2)) {
  // What this folds is what it conflicts on: if another writer lands in the same
  // slice first, the append is rejected and the whole command retries.
  fold open_orders: Int = 0
    on @order.placed(customer_id) => open_orders + 1
    on @order.cancelled(customer_id) => open_orders - 1

  if open_orders >= 10 {
    return reject TooManyOpen
  }

  emit @order.placed { order_id, customer_id, email, total }
}
```

## Four ideas worth knowing

- **A fold is a read declaration, not a variable.** `fold` names a slice of the log, and the
  slices a command folded *are* the condition its append is checked against. What you read is what
  you conflict on, so optimistic concurrency falls out of the code instead of being configured
  beside it. That is why the keyword is not `let`.

- **Crypto-shredding is a type.** `@subject(customer_id)` makes a field a `Sealed(String,
  customer_id)`, which propagates through folds and read models untouched. Only an effect may
  `reveal` one, and erasing a subject's key makes every seal bearing it permanently unreadable.
  Moving sealed content is not reading it, and the difference is checked rather than trusted.

- **Money is its own type and never rounds silently.** `Money(2)` is a scaled integer, distinct
  from `Decimal(2)`, with an operator table that refuses what it cannot answer exactly: `price *
  rate` is an error naming `mul` and an explicit rounding mode.

- **A test is a declaration.** `given` seeds the log, `run` or `deliver` acts, `expect` asserts on
  the events, rows and calls that resulted. Tests live beside the code they exercise and run with
  the same binary that checks it, so there is no framework to adopt.

## Install

`hek` is the checker, test runner, formatter and digest tool, and one binary is all of it. Prebuilt Linux
binaries (`x86_64` and `aarch64`, gnu and musl) are attached to every release, and
[cargo-binstall] fetches the right one rather than compiling it:

```sh
cargo binstall hek
```

Also `cargo install hek` to build it from source, `nix run git+https://git.tqwewe.com/tephra/heklang`
to run it without installing anything, or take a tarball straight from [Releases] (each is
published beside its `.sha256`, and a `SHA256SUMS` covers the set).

To embed the language instead of running the tool, `cargo add heklang`: that crate is the lexer,
parser, checker and interpreter, and [hekla] is what drives it in production. The editor grammar
is a third crate, [tree-sitter-hek], so an editor can load it without either of the others.

[cargo-binstall]: https://github.com/cargo-bins/cargo-binstall
[Releases]: https://git.tqwewe.com/tephra/heklang/releases
[tree-sitter-hek]: https://crates.io/crates/tree-sitter-hek

## The tool

```sh
hek check  hek/   # parse every `.hk` file under a path as one program
hek test   hek/   # the same, then run every `test` declaration in it
hek fmt    hek/   # rewrite canonically; `--check` makes it a gate
hek digest hek/   # what the program does, with everything else taken away
```

Every static check lives in the parser, so "parses" and "checks" are the same pass, and a
diagnostic carries a code, an extent and a hint separately rather than as one sentence.

`digest` is the odd one out: it prints a condensed, deterministic rendering of what the program
*does*, with local names, layout, comments, file boundaries and declaration order taken out. Two
versions that behave the same hash the same. The canonical form is s-expressions, one line per
declaration (`--packed`), it reads back into the same object with no source tree in reach, and
every declaration carries a hash of its own plus a hash of just its externally-visible signature.
[docs/digest.md] has the rules.

From a checkout the same four are `cargo run -p hek -- check hek/` and so on. The repository is
also a flake: `nix build .#hek` for the binary, `nix build .#tree-sitter-hek` for the grammar,
`nix flake check` for the suite.

## Editor support

[tree-sitter-hek] is a tree-sitter grammar and queries for `.hk`, published as its own crate and
living in `tree-sitter-hek/` here. `hek fmt -` formats a module from stdin, which is what an
editor's format-on-save wants, and the two stay in step because `hek fmt` links that same grammar.
`tree-sitter-hek/README.md` has the Helix wiring for both.

## Agent support

[.claude/skills/heklang/] is a Claude skill: the whole language distilled into rules an agent can
follow, one reference file per declaration kind, a table from every diagnostic code to its fix, and
a seven-file example program that passes `hek check`, `hek test` and `hek fmt --check`. Copy the
directory into `.claude/skills/` in an application that writes `.hk` files, and the agent working
there picks it up. It is the specification in [docs/] with the reasoning taken out and the rules
left in, so the two should change together.

[.claude/skills/heklang/]: .claude/skills/heklang/

## Learn more

- **[docs/]** is the specification, one document per idea, each paired with a test file of the same
  name that is the same rules made executable.
- [docs/commands.md] is the best place to start, then [docs/effects.md] for the rules a handler
  that reaches outside has to keep, and [docs/refusals.md] for the shape of a refusal.
- [hekla] is what runs it, and does not repeat the language.

[docs/]: docs/
[docs/commands.md]: docs/commands.md
[docs/effects.md]: docs/effects.md
[docs/refusals.md]: docs/refusals.md
[docs/digest.md]: docs/digest.md

## License

Licensed under either of [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT) at your option.

heklang was built with AI use and careful review.
