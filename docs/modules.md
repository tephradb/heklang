# Modules

A program is assembled from several source files. `parse_files` takes them as
`(name, source)` pairs and returns one `Program`:

```rust
parse_files([
    ("events/order.hk", &events),
    ("commands/place_order.hk", &commands),
    ("projectors/orders.hk", &projectors),
])
```

`parse(source)` remains the single-source form, and is the same thing with one unnamed module.

`tests/modules.rs` is this document as executable tests.

## 1. Order does not matter

A command may use an event declared in another file, a projector may reference events it is compiled
alongside, an effect may invoke a command from a third file, and the files may be passed in any
order. This is the two-pass declaration collection that already made order irrelevant inside one
file, applied to every module at once: pass one collects event, entity and enum declarations and
command signatures across the whole program, pass two parses the bodies.

There is no import syntax and no dependency order to get right, because there is nothing to order.

## 2. A module is not a namespace

Event paths, command names, projector names and effect names are global. Two modules cannot both
declare `@order.placed` or a command named `Place`; that is the same "declared twice" error as within
one file, and it names the module of the first declaration.

An effect may invoke a command declared in another module, and a command's signature is collected in
the same pass that collects events, so there is nothing to order there either.

The one scoped namespace is a projector's own: entities and enums belong to the projector that
declares them, so two projectors may each have a `Customer` (see `docs/projectors.md`, rule on
scoping). Nothing else nests.

## 3. There is no header item

A module declares things, and that is all it does. There is nothing a file must open with and nothing
one module has to declare on behalf of the others, so a file of plain declarations is already a whole
program.

heklang briefly had a `currency USD` item here, on the theory that currency was deployment
configuration. It is gone, and `docs/money.md` records why: a multi-tenant deployment serving stores
in different currencies cannot have one configured currency, which is the normal case rather than the
exception. Nothing replaced it, which is the outcome worth noticing: the feature that made module
order awkward turned out to be the wrong feature rather than one needing a better home.

## Positions

Each module is lexed on its own, so **line and column stay module-relative**: a diagnostic in the
third file reports its own line 2, not line 340 of a concatenation.

Errors carry the module they are in and render as `module:line:col: message`.

- **Syntax and lex errors** carry it directly (`SyntaxError::file`).
- **Runtime errors** carry it too (`interp::Error::module`), stamped at the `run` / `project`
  boundary rather than at each raise site. That is the innermost place that knows which declaration
  is executing, and a command or projector is always wholly within one module, so one stamp is
  enough.

An unnamed source (the `parse` path) has no module to name, and its errors render as `line:col:
message` exactly as before.

## Known gaps

Module names are labels for diagnostics, not identities. Nothing keys off them, nothing detects that
a file was renamed or moved, and there is no manifest saying which files make up a program: the
caller decides that by what it passes to `parse_files`. hekla's roadmap notes projector rename
detection as an open question for exactly this reason, and it will want a stable module identity
rather than a display string.
