# The host

A **host** is the world an interpreter runs against: an event log, a clock, a key store and a
network. heklang ships one, `Harness`, which is entirely in memory, and every one of its parts is a
stand-in for something a real runtime owns.

This document is the contract for the other kind of host, the real one. `src/host.rs` is the seam,
`src/harness.rs` is the reference implementation, and `tests/host.rs` is the same set of rules as
executable tests, driven by a host the crate does not ship.

## Shape

```rust
pub trait Host: Log + Clock + Keys + Http {}
impl<T: Log + Clock + Keys + Http> Host for T {}

let mut interpreter = Interpreter::with_host(&program, my_runtime);
interpreter.run("PlaceOrder", args)?;
interpreter.deliver("NotifyCustomer", position, &mut my_journal)?;
```

`Interpreter<'a, H = Harness>`, so `Interpreter::new(&program)` still means the harness and nothing
that was written against it has to say so.

## 1. Four traits, and where the line is

The cut is the one `docs/effects.md` rule 11 already makes with its journaled column. **Reading the
log is redone on every attempt and must be**, because that is what makes a retry see the new log. A
side effect, or an unrepeatable observation, is done once and remembered.

| Trait | Methods | Why its own |
| --- | --- | --- |
| `Log` | `head`, `record`, `read`, `append` | the only one that is not journaled |
| `Clock` | `now` | no state and no failure mode; three lines to implement |
| `Keys` | `erased`, `erase` | a lifecycle, and in a real host a key management service |
| `Http` | `send` | one attempt, and the only place bytes leave |

They bundle into `Host` because `Effects` holds one trait object and Rust has no `dyn A + B`. The
bundle is the plumbing; the four are the meaning.

**State is deliberately not among them.** An effect reads state by folding its boundary off the log
(`docs/effects.md` rule 3), and a projector's rows are its output rather than anyone's input
(`docs/projectors.md` rule 4). A host is never asked to answer "what is the state of X".

## 2. Reading is `&self`, appending is `&mut self`

Not a style choice. `Interpreter::project` is a `&self` method and stays one, so `Log::read` has to
be `&self` too. It also matches what a real store can supply: tephra's own read runs on the caller's
thread and never touches the writer.

`Clock::now` is `&self` for the same reason a clock has no state to advance. `Keys::erase` and
`Http::send` are `&mut self`, because destroying a key and sending a request both change the world.

## 3. A slice is a predicate, resolved

```rust
pub struct Predicate {
    pub event: EventPath,
    pub filters: Vec<(Ident, Value)>,
}
```

A `state` declares a slice of the log, and the filters are expressions the command evaluated. They
arrive at the host as **values**, because "which slice" means nothing to something that did not
compile the program, and because a value is what an index can be looked up by.

The filters are sorted by field name, so one slice is one predicate however it was written. Two
filters in either order narrow the same events and have no business comparing unequal.

**This maps onto a tag query one for one.** A `Predicate` is one query item: an event type plus a set
of equalities that all have to hold. A `Vec<Predicate>` is a disjunction of those. An adapter for a
store with that shape is a `map`, not a translation.

## 4. What `read` owes the fold

```rust
pub struct Query {
    pub slices: Vec<Predicate>,
    pub upto: Option<u64>,
}
```

Three obligations, each load-bearing:

- **Every record matching any predicate is visited**, or a fold silently loses events.
- **Each is visited once**, or `open + 1` counts one event twice.
- **In ascending position order**, because a fold is an order-dependent expression.

**Over-delivering is a cost; under-delivering is a bug.** The fold re-checks each slice itself, so a
store that can only narrow approximately is still correct, only slower. A store that narrows too far
is wrong, and nothing will catch it.

`upto` is inclusive and is what distinguishes the readers: `None` for a command and a projector,
`Some(position)` for an effect arm, which folds to and including its own trigger (`docs/effects.md`
rule 3).

`read` takes a visitor rather than returning a collection, so a host can stream. A fold's live heap
should not have to be linear in the size of its boundary, and a returned `Vec` would make that
impossible before the first line of the adapter was written.

## 5. The append condition

```rust
pub struct AppendCondition {
    pub after: u64,
    pub slices: Vec<Predicate>,
}
```

The same resolved slices, bounded above when you read and below when you write. `Query` bounds them
with `upto`; `AppendCondition` bounds them with `after`. `docs/commands.md` has the Dynamic
Consistency Boundary argument: what you folded is what you conflict on.

`AppendCondition::conflicts` is the definition, written once, and `Harness` enforces it even though
nothing single-threaded can trip it. There is one answer to what the condition means, and a host
implementing it deserves somewhere to read it rather than somewhere to guess.

**A conflict is an error, not an outcome.** `Outcome` has three variants and they are the command's
own answer: committed, the input was malformed, or the command refused on state grounds. Being beaten
to the log is none of those, so it arrives as `ErrorKind::Conflict { after }`, and the loop that
re-folds and re-runs belongs to the host. That is where every runtime already puts it.

## 6. The journal is not part of the host

`deliver` takes `&mut dyn Calls` beside the effect and the position, because a journal is **per
invocation** and a host is per world. Nothing carries between positions.

```rust
pub trait Calls {
    fn recorded(&self, call: &str, ordinal: u32) -> Result<Option<Recorded>, Error>;
    fn record(&mut self, call: &str, ordinal: u32, recorded: Recorded) -> Result<(), Error>;
}
```

The key is a readable description of the call plus an ordinal for repeated identical ones. A host
that would rather store a hash hashes exactly that string, which keeps the **key** the language's and
the **storage** the host's. What is in the key is a language decision and is not negotiable: the verb,
the URL and the body, and deliberately not the headers, because a rotated idempotency key must land
on the same entry or a replay re-sends the request it was written to suppress.

`Journal` is the in-memory implementation and it is in `src/harness.rs`, where the other stand-ins
live.

## 7. What stays the language's

A host is not asked for these, and offering to supply them would break a rule the language is
holding.

**The retry policy.** `docs/effects.md` rule 5 says only a decidable result reaches the handler.
`Http::send` performs one attempt and heklang runs the loop, because the moment the host chose the
policy the same program would present different handler-visible outcomes on two hosts, and rule 5
would stop being a language rule.

**Identity.** There is no `Uuid.new`, no random, nothing minted from nothing, anywhere in the
language, and rule 11 spends four paragraphs on why. `Uuid.derive(seed, name)` is a pure function of
its arguments. A `Host::derive` would hand that guarantee back and make rule 11 a convention.

**The cascade backstop**, and `drive`. `drive` walks from position 0 with an in-memory counter, which
is a harness's idea of a subscription. A real host keeps its own cursor and calls `deliver` per
position; `deliver` is the host-facing primitive.

**`log` output.** Rule 10 says it is not journaled. A host that wants the lines reads
`Effectful::Log` out of the trace, which is ordered and complete.

## 8. Converting at the boundary

The traits speak heklang's model. A host whose model differs converts on the way in and out, rather
than the language reshaping itself to match one runtime.

| heklang | a host adapter converts to |
| --- | --- |
| `Timestamp` as `i64` epoch microseconds | whatever its envelope holds, RFC 3339 or otherwise |
| `Outcome`, three variants | its own outcome type, with conflict and unavailable added back |
| `ErrorKind::Conflict` | the retry signal its own loop reads |
| a readable journal key | a content hash of that key |
| `now()` pinned once per invocation | one journal entry, however many times the body reads it |
| `update` | the runtime's skipping partial write |
| `patch` | a whole-row write plus the zero values |

## 9. What this is not, yet

- **Read models are not a seam.** `project` still returns an in-memory `Store` and re-folds the whole
  log. Nothing inside the language can observe where a row lands, so this is separable, and it is a
  larger piece of work than this one.
- **No cursor and no checkpoint.** Neither an effect's position nor a projector's watermark is
  persisted by anything here.
- **Ciphertext is still not modelled.** `Keys` answers whether a subject is erased, which is what
  rules 9 and 12 turn on. `docs/effects.md` records the rest as a known gap.
- **`head` and `read` are not atomic together.** A host whose log grows between them hands the fold a
  longer prefix than `after` claims. That is exactly what the append condition catches, which is why
  it is the one thing here that a host must implement rather than approximate.

## Related

- `docs/commands.md`: the append condition, and why a `state` is a read declaration.
- `docs/effects.md`: rules 3, 5, 10, 11 and 12, which are the reasons this seam is cut where it is.
- `docs/projectors.md`: the other reader of the log, and the one writer of a read model.
- `docs/testing.md`: `given`, `respond` and `erased`, which are how the language scripts a harness.
