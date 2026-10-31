# The host

A **host** is the world an interpreter runs against: an event log, a clock, a key store, a network
and a set of deployment credentials. heklang ships one, `Harness`, which is entirely in memory, and
every one of its parts is a stand-in for something a real runtime owns.

This document is the contract for the other kind of host, the real one. `src/host.rs` is the seam,
`src/harness.rs` is the reference implementation, and `tests/host.rs` is the same set of rules as
executable tests, driven by a host the crate does not ship.

## Shape

```rust
pub trait Host: Log + Clock + Keys + Http + Secrets {}
impl<T: Log + Clock + Keys + Http + Secrets> Host for T {}

let mut interpreter = Interpreter::with_host(&program, my_runtime);
interpreter.run("PlaceOrder", args)?;
interpreter.deliver("NotifyCustomer", position, &mut my_journal)?;
interpreter.project_into("Orders", &mut my_rows)?;
```

`Interpreter<'a, H = Harness>`, so `Interpreter::new(&program)` still means the harness and nothing
that was written against it has to say so.

**A `Program` and the values it produces are `Send + Sync`**, so a host parses once at load and serves
from as many threads as it likes. That is why the string inside a `Value` is an `Arc` and not an `Rc`,
and `src/lib.rs` asserts it at compile time because nothing else would notice if it stopped being
true. The interpreter itself is single-threaded: one per request, or one per invocation.

## 1. Five traits, and where the line is

The cut is the one `docs/effects.md` rule 11 already makes with its journaled column. **Reading the
log is redone on every attempt and must be**, because that is what makes a retry see the new log. A
side effect, or an unrepeatable observation, is done once and remembered.

| Trait | Methods | Why its own |
| --- | --- | --- |
| `Log` | `head`, `record`, `read`, `append` | the only one that is not journaled |
| `Clock` | `now` | no state and no failure mode; three lines to implement |
| `Keys` | `decrypt`, `erase` | a lifecycle, and in a real host a key management service |
| `Http` | `send` | one attempt, and the only place bytes leave |
| `Secrets` | `secret` | a deployment's, not a program's, and not in the log |

They bundle into `Host` because `Effects` holds one trait object and Rust has no `dyn A + B`. The
bundle is the plumbing; the five are the meaning.

**`subject` is a declared name.** `decrypt` and `erase` take the subject a key is filed under, and
since `docs/effects.md` rule 12 made a subject a declared type, that string is the type's name
(`Customer`) rather than the name of whichever field the annotation pointed at. A host keys its
store by it, so it is one of the names `docs/digest.md` rule 3 keeps verbatim: renaming a subject
re-partitions every key row.

**`Keys` is asked one question and it is a lifecycle one.** `decrypt` answers with the plaintext or
with `None`, and `None` is the key being gone. It is not an `Err`: an erased subject is an outcome
`docs/effects.md` rule 12 names and a program meets, not a host that failed. A host that also wants
to distinguish a tampered ciphertext from a shredded key has nowhere to say so, deliberately: both
are unreadable and terminal, and rule 12's distinction is between absent and unreadable, which
survives because an absent optional never reaches here at all.

**It is called once per `reveal`.** A seal is opaque and heklang keeps it that way, so a fold walks
its whole boundary without asking for a key: only the content a handler actually reveals costs one.
A host that decrypts on the way in instead pays per record for content nothing reads.

**The other question is `is_live`, and it wants no content.** A column whose key is gone has to read
back absent (`docs/projectors.md` rule 9), so the read models heklang keeps itself
(`Interpreter::project`, and the `project` a test asserts on) ask once per sealed column written.
That is a lifecycle question and not a read: a projection **moves** sealed content and never reveals
it, so producing the plaintext to answer a boolean would put personal data on a path built not to
hold any, and would spend a key use per column per row. It is defaulted to `decrypt`, so a host is
correct without implementing it and better off overriding it wherever its key store can answer
without unwrapping. A host that keeps its own read models asks its own store instead, in its own
`Rows`; `project_into` writes through to those rows and asks nothing here.

**The plaintext is text, and for a composite that text is JSON.** `@subject(...)` takes any declared
type, so a field declared a record, a list, a map or a `Json` seals as the document rule 8 writes and
`Value::from_sealed` parses it back against the declaration. A host neither builds that text nor reads
it; what this asks of one is only that `decrypt` return the same bytes that were sealed. The pair is
`value::sealed_text` and `Value::from_sealed`, and a `Json` is the one type sealed whole, quotes and
all, because it is the only one whose value can itself be a string that looks like another.

Two things follow that a host implementor should know. **A composite seal is read as stored history**,
so `@absent` answers inside one and a host need do nothing to make an older seal readable. And **the
reader has a depth limit**, above what a host's own parser will hand over but not unbounded: a `Json`
value nested past it would seal and never come back, so a host that builds a `Json` some other way
than by parsing a body should keep its own limit below this one.

**`Secrets` answers with a value and never with a source.** Where a credential comes from is a
host's business the way a key store is: `docs/effects.md` rule 16 says which declarations may read
one and says nothing about environments, files or vaults. It is asked lazily rather than resolved
when an interpreter is built, so a remote provider is a host change and not a language one, and a
host reading an environment caches trivially. `None` means this deployment did not set it, and a
host is expected to make that unreachable for a required one by refusing to start; heklang's
backstop when a host fails at that is `ErrorKind::MissingSecret`, which **wedges** rather than
skipping, because unlike an erased subject this is recoverable by an operator.

**A request crosses in two renderings, and reading the wrong one is a compile error.** A `secret`
may sit in a url, in a header value or in a body, so `Request` carries `wire` and `shown` rather
than one set of fields: `wire` is what the credential actually is, `shown` names it instead. Send
the first, print the second, key a journal with the second.

The reason it is two fields and not one field plus a convention is that a program holding no secret
makes them identical. A host that read the wrong one would be wrong only for the programs that use
the feature, and would never find out from its own tests. So the flat `url`, `body` and `headers`
fields are gone, and the choice is made once, at the compiler's insistence. Neither rendering ever
contains a `Json::Secret`: that variant lives between an object literal and the request built from
it, and both renderings take it off.

**State is deliberately not among them.** An effect reads state by folding its boundary off the log
(`docs/effects.md` rule 3), and a projector's rows are its output rather than anyone's input
(`docs/projectors.md` rule 4). A host is never asked to answer "what is the state of X".

## 2. Reading is `&self`, appending is `&mut self`

Not a style choice. `Interpreter::project` is a `&self` method and stays one, so `Log::read` has to
be `&self` too. It also matches what a real store can supply: tephra's own read runs on the caller's
thread and never touches the writer.

`Clock::now` is `&self` for the same reason a clock has no state to advance, and so is
`Keys::decrypt`: reading a key does not spend it. `Keys::erase` and `Http::send` are `&mut self`,
because destroying a key and sending a request both change the world.

## 3. A slice is a predicate, resolved

```rust
pub struct Predicate {
    pub event: EventPath,
    pub filters: Vec<(Ident, Value)>,
}
```

A `fold` declares a slice of the log, and the filters are expressions the command evaluated. They
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
    pub from: u64,
    pub upto: Option<u64>,
}
```

Three obligations, each load-bearing:

- **Every record in range matching any predicate is visited**, or a fold silently loses events.
- **Each is visited once**, or `open + 1` counts one event twice.
- **In ascending position order**, because a fold is an order-dependent expression.

**Over-delivering is a cost; under-delivering is a bug.** The fold re-checks each slice itself, so a
store that can only narrow approximately is still correct, only slower. A store that narrows too far
is wrong, and nothing will catch it.

**The range is the exception to that latitude.** A slice is re-checked and a position is not, so a
record handed to a resumed fold that already folded it is counted twice. Both bounds are inclusive.

`upto` is what distinguishes the readers: `None` for a projector, `Some(after - 1)` for a command,
which folds to the head it took `after` from and no further, `Some(head - 1)` for a store filling a
stored column, which is the same conversion over the head its checkpoint records, and
`Some(position)` for an effect arm, which folds to and including its own trigger (`docs/effects.md`
rule 3).

**Two of the four hold a head and subtract, which is why no caller outside this file does.** A head
is what this seam hands out and an inclusive position is what it takes, and the one place that
conversion is interesting is an empty log, where there is no position below the head to name:
`Interpreter::fold_stored` therefore takes the head and reads nothing at zero, rather than asking a
store for a position it does not have (`docs/folds.md` section 11).

`from` is `0` for everything except a command's retry, where it is the position the previous attempt
folded through. **A host answers it by seeking, not by filtering**, or the field buys nothing: the
whole point is that a conflict on a boundary a hundred thousand events deep reads the handful that
beat it. Section 5 has the rest of that argument.

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

The same resolved slices, read up to `after - 1` and conditioned on from `after`. `docs/commands.md`
has the Dynamic Consistency Boundary argument: what you folded is what you conflict on, and the two
bounds meet exactly so that neither statement needs a footnote.

`AppendCondition::conflicts` is the definition, written once, and `Harness` enforces it even though
nothing single-threaded can trip it. There is one answer to what the condition means, and a host
implementing it deserves somewhere to read it rather than somewhere to guess.

**A conflict is an error, not an outcome.** `Outcome` has three variants and they are the command's
own answer: committed, the input was malformed, or the command refused on state grounds. Being beaten
to the log is none of those, so it arrives as `ErrorKind::Conflict { after }`.

### The retry policy is the host's; the carry is not

```rust
interpreter.run(name, args)                      // one attempt; a conflict is the error
interpreter.run_retrying(name, args, &mut again) // `again(attempt) -> bool` decides
```

`again` is asked after each conflict, with the zero-based number of the attempt that just had one.
The budget and the wait between attempts stay where they were, because how many attempts are worth
spending and how long to sleep are decisions only a runtime can make. **What moved in is the carry.**

A fold is a left fold over an append-only log, so folding `[0, a)` and then `[a, b)` is the state
folding `[0, b)` would have given. A retry therefore never has to start over, and on a hot boundary
that is not a marginal saving: it is the difference between a conflict costing the events that beat
you and a conflict costing the boundary. A host cannot do it from outside, because the state lives in
a frame it has no name for, so a host looping over `run` re-reads and re-folds everything every time.

Three things follow, and each is a property `tests/host.rs` asserts:

- A retry asks for `from = ` the position its last attempt folded through. Seeking to it is the
  host's half of the bargain (section 4).
- The delta is folded **onto the state that attempt built**, never onto the seed. Folding one rival
  event onto `0` commits a third order against a cap of two.
- The state is taken **before the body runs**, so a body that assigns into a `fold` changes what it
  decides on and cannot reach what the next attempt folds onto.

What does not carry is what a retry has no reason to redo: `now()` is pinned once for the whole
request (rule 11), and so are the bound arguments.

**The slices are not among them, and that is what staging changed.** A command reads once per
declaration run (`docs/commands.md`), and a later run's filter may name a `fold` an earlier one
folded, so its predicates are not the same on every attempt. Two things follow. The head is pinned
once per attempt, at the first run that reads, and every later run reads to it, so the runs are one
consistent view of the log and the condition stays a single `after` over a flat slice list. And only
the **first** run carries: its fold reads nothing but the arguments, so it cannot move between
attempts, while a later one can, through a `let` between the runs that no comparison of predicates
would notice. A later run re-seeds and folds the whole range, which is what it would have cost with
no carry at all.

**`after` is meaningful only alongside a non-empty `slices`.** A command that read nothing -- one
with no `fold`, or one that returned above its first declaration -- comes back with no slices and
`after` at zero rather than at a head it never asked for. `conflicts` is false for every record
either way, which is correct: a decision that depended on nothing cannot be invalidated. A host that
reads `after` as an expected version rather than as half of the predicate has to check `slices`
first.

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

## 7. Read models are a seam, and a narrow one

```rust
pub trait Rows {
    fn row(&self, entity: &str, key: &[Key]) -> Result<Option<Row>, Error>;
    fn put(&mut self, entity: &Ident, key: Vec<Key>, row: Row) -> Result<(), Error>;
    fn delete(&mut self, entity: &Ident, key: &[Key]) -> Result<(), Error>;
}

let projection = Projection::new(&program, "Orders")?;
let query = projection.query();            // what to subscribe to
projection.apply(&record, &mut my_rows)?;  // one record, handlers in declaration order
```

**Not part of `Host`.** A host is per world and a `Rows` is per projector, the way a `Calls` is per
invocation. `Store` is one implementation and a persistent read model is another, so
`Interpreter::project` is `project_into` against an in-memory `Store` and there are not two write
paths to keep in step.

**A key is a sequence, not a scalar.** One `Key` per `@key` column, in the order the entity declared
them (`docs/projectors.md` rule 11), and a single-column key is a one-element slice rather than a
special case. A host stores them so that the sequence orders **lexicographically**, which is what
makes a prefix of a compound key a range its read API can page on, the way a prefix of a compound
index is. A host that joins them into one string of its own is free to, as long as that ordering
survives the join.

**It reads as well as writes**, which is the only surprising line in it. `docs/projectors.md` rule 3
fills a `patch`'s stored loads from the row before evaluating any of the write's value expressions,
and rule 5 decides materialize-from-zeros on whether the row is there at all. A write-only stream
could carry neither. `row` must also reflect writes this projection has already made, because two
handlers may select one record and the second has to see the first's.

**A `patch` crosses as a whole row**, not as the fields it named. The delta is still in the IR; it is
deliberately not what a host sees, because a host taking a delta while `Store` took a whole row would
be two write paths with nothing comparing them against each other. *Converting at the boundary* below
has the `patch` and `update` rows.

**A sealed column is the host's to empty.** The `Value::Sealed` reaching `put` is what lets a host
store one without opening it, and it is the same fact that makes the erase the host's: a column whose
subject key is gone has to read back absent (`docs/projectors.md` rule 9), and the key store is on
the host's side of this seam. heklang empties the columns of the rows it owns, which are
`Interpreter::project`'s, and never the rows that arrive here.

**`Projection` holds no host.** `docs/projectors.md` rule 4 gives a projector no general read and
`docs/effects.md` rule 11 gives it no clock, so the program and the rows it writes through are the
whole of what one needs. That is what lets a host project from a thread that never touches the log
reader.

**What is still not here**: where a row lands, what a column is called in a store, and what an index
does. Rule 8 of `docs/projectors.md` still holds, and a host reads `Projection::projector()` for the
entities and enums it needs to build a schema from.

## 7a. The derived read model is the same seam, one level narrower

```rust
pub trait Columns {
    fn value(&self, table: &str, key: &[Key], column: &str) -> Result<Option<Value>, Error>;
    fn set(&mut self, table: &str, key: Vec<Key>, column: &str, value: Value) -> Result<(), Error>;
    fn keys(&self, table: &str, pinned: &[(usize, Key)]) -> Result<Vec<Vec<Key>>, Error>;
}

let tables = Tables::new(&program);
let query = tables.query();                 // every event any column folds, each once
tables.apply(&record, &mut my_columns)?;    // one record, pinned arms then ranges
let rebuild = tables.only(&folds);          // the same sink, for these columns
rebuild.folds();                            // what it will write, which is wider
rebuild.apply(&record, &mut my_columns)?;
```

`Columns` is to `Tables` what `Rows` is to `Projection`, and the shapes rhyme on purpose: not part
of `Host`, a key is a sequence that must order lexicographically, it reads as well as writes, and
`Tables` holds no host so a deployment drives it off a thread that never touches the log reader.
`Cells` is the in-memory implementation, as `Store` is for rows.

**Per program rather than per projector.** Nothing declares a derived table: a read model is a named
fold kept for every key, so `tables::layout` derives the whole set from the program's folds and
there is one of them. `table` and `column` are that layout's own names, which is what keeps `hek
check --tables` and the real tables from disagreeing about what an author is looking at.

**It reads for a sharper reason than `Rows` does.** A `patch` reads the row to fill its stored
loads; a fold's accumulator **is** the stored column, so without `value` there is nothing to fold
onto and every record would restart from the seed.

**An absent value and an absent key are one thing, deliberately.** `value` answering `None` means
the column is at its seed, which is what a fold over no matching events folds to, so a store holds
only the keys something has written and a key nothing minted needs no row. `keys` has the same
boundary: a range update covers the keys the table holds. `docs/folds.md` section 11 states what
that costs and why it is safe.

**`keys` is held to `read`'s cut, not to `row`'s.** Over-delivering is a cost and under-delivering is
a bug: a key handed back that the arm does not write is dropped by the sink, while one withheld is a
column that silently stops following its log. Dropped there and not re-checked later, which took a
review to get right: a key part the arm pinned with a `docs/folds.md` rule 13 conjunct is *bound*
from whatever key came back, so the conjunct's gate goes dead, and a dead gate at the fold means the
layout and the arm disagree rather than that there is a key to skip. A store with no index for a
range answering with every row of its table is the implementation this exists to allow.

**A range covers the keys the table holds when the record arrives.** A key minted by a later record
never sees an earlier ranged one, where `Interpreter::fold` over the same key applies it. The sink's
answer is the one a store can produce and the one every path through the sink produces, so a follow
and a backfill agree; `docs/folds.md` section 11 has the argument and what it costs.

**A column may read another column** (`docs/folds.md` rules 15 and 16), for a key part or for a
value, and two things follow for
a host. `apply` reads every lookup for a record **before** it writes anything for that record, across
every table, so the looked-up column is at exactly the record's own position: a host applying records
in order needs to do nothing for that, and a host applying them out of order gets the wrong answer in
a way nothing can detect. And `Column::reads` is the dependency, transitively: it is what
`Layout::closure` adds to a rebuild, and what tells a catalog that editing one fold refolds another.

**`Tables::only` is for a rebuild, and `apply` is wrong for one twice over.** A column is rebuilt by
replaying the log into it, and `apply` runs every arm of every column over that replay: the columns
that are already correct do the work again for nothing, and a body that can `fail` or overflow fails
the rebuild of a column it has nothing to do with. Handing those columns a seed instead of their
held value avoids the overflow and leaves the rest, because the body still runs. The set is named by
the **fold** rather than by the column, because the column's name is `snake_case` of it and a caller
spelling that derivation out to name a column is a second copy of it; a name no column is kept for
is ignored, so an unstored fold and a typo both mean "nothing to apply".

**What it runs is `Layout::closure` of the set, and a range arm is why that is not the set itself.**
An arm leaving a key part open ranges over *the keys the table holds*, and those keys are minted by
the pinned arms of that table's columns, this one's and the others'. A replay restricted to a ranged
column alone mints none of them, so the range covers nothing and the column comes out at its seed
with nothing to say so. The closure adds the columns of that table that **mint**, which is the ones
with a fully pinned arm and not the table: a column no arm of which pins the whole key asks `keys`
and writes at what comes back, so it mints nothing and is no use to anyone else's range. A column
whose every arm pins the whole key, the ordinary case, closes over itself.

**That is why it is a handle and not an argument, and both halves are the same reason.** The closure
costs a walk of the layout and a replay applies one record at a time, so computing it per call would
put a sort and an allocation per record into the one API whose purpose is to make a rebuild cheaper.
And because it is **wider than what was asked for**, `Rebuild::folds` has to be reachable: the sink
carries the accumulator in the store rather than re-deriving it, so replaying the log onto a column
that already holds its value folds every record into it a second time. A store rebuilding
`order_closed` against its live tables gets `order_total` applied beside it and silently doubled, so
`folds` is the set it resets or shadows first, and it is not the set it passed in. The ones it did
not ask for are the ones it throws away afterwards; what it may not do is drop their writes as they
go, because the **keys** are what the additions are for. Recording each key and discarding the value
is enough, since `keys` is all a ranged arm reads, and keeping both costs only an overlay.

So the closure buys correctness and spends the second thing `only` is for: the bodies it adds run
over the whole replay, and one of them that can `fail` can still fail a rebuild of the ranged column
beside it.

**A sealed column is nullable whatever its declared type says.** Rule 9 of `docs/projectors.md`
makes an erase empty the column and only an optional has an empty value, so a store deciding
nullability from the declared type alone writes a null into a `NOT NULL` column the first time a
subject is erased. It also keeps the field name beside the ciphertext, because one column can hold
content sealed under two different field names and `decrypt` is asked for the one it was sealed
with. `docs/folds.md` section 11 has both.

## 8. What stays the language's

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

**The trace is the wire form, and a host must not publish it.** `Effectful::Http` carries the url
and body that actually went out, because a test names the value it supplied and has no other
rendering to assert against (`docs/testing.md` section 7). It is therefore the one surface here that
is **not** redacted: a host that logs or `Debug`-prints a whole trace publishes any credential rule
16 kept out of the journal key, the error text and every `Display`. Read `Effectful::Log` for lines,
and `Request::shown` for a request; take the trace as a test-facing observation rather than an
operational one.

## 9. Converting at the boundary

The traits speak heklang's model. A host whose model differs converts on the way in and out, rather
than the language reshaping itself to match one runtime.

| heklang | a host adapter converts to |
| --- | --- |
| `Timestamp` as `i64` epoch microseconds | whatever its envelope holds, RFC 3339 or otherwise |
| `Outcome`, three variants | its own outcome type, with conflict and unavailable added back |
| `ErrorKind::Conflict` | the retry signal its own loop reads |
| a readable journal key | a content hash of that key |
| `Request`, in two renderings | one request, built from `wire` |
| `now()` pinned once per invocation | one journal entry, however many times the body reads it |
| `update` | the runtime's skipping partial write |
| `patch` | a whole-row write plus the zero values |

### A stored payload is read differently from a body

A host builds a `Record` out of whatever its store holds, and one thing is true of that store and not
of a request body: it is append-only, so a payload in it was written under whatever the declaration was
at the time and may predate a field the declaration carries now. Those are two different readings, and
they are two different calls:

| call | a key the JSON does not hold |
| --- | --- |
| `Value::from_json(json, ty, defs)` | a mismatch reporting `nothing`, because a body has to satisfy the declaration as it stands |
| `value::stored_field(field, stored, defs)` | the field's `@absent` literal when it declares one, `none` when the type is optional, otherwise that same mismatch |

`stored` is an `Option<&Json>`, and that is the whole point of the signature. **A key that is there
holding `null` is a value a producer wrote; a key that is not there is a field the payload predates.** A
host that passed `Some(&Json::Null)` for a missing key would collapse the two, and every `@absent` in
the program would stop being reachable. Read the key; hand over what was there, or `None`.

The split is not a convenience. Applying an absence literal to a body would let a caller omit a field
and be handed a value it never sent, which is a different decision from letting history read.

`Mismatch::is_absence()` is how a host tells the two failures apart afterwards. A key that was never
there is the one fault a declaration can answer for itself, so a host refusing a deployment can name
`@absent` as the fix; a key holding the wrong shape has no such answer and must not be offered one.

Nothing else about the seam moves: the `Record` a host builds is still whole. A missing field in a
`Record` that reached the interpreter is still a broken host rather than a narrower read, because this
is the call that decides it.

## 10. What this is not, yet

- **No cursor and no checkpoint.** Neither an effect's position nor a projector's watermark is
  persisted by anything here.
- **Nothing writes a seal.** `Keys::decrypt` reads one, and a plain value emitted into a
  `@subject(...)` field crosses as plaintext for a host to seal, because that is the direction where
  the content is in hand. A host therefore sees two shapes at a write for one field: fresh plaintext
  to seal, and a `Value::Sealed` that was moved and is already sealed under the name it carries.
- **Nothing is zeroized.** A `Value::Secret` holds two `Arc<str>`, for the reason every other
  string in a `Value` is one, and a shared buffer cannot be wiped. A partial guarantee here would
  be worse than an honest absence, because it would read as one: a host may zeroize the source it
  resolved a credential from, and heklang holds the plaintext for the life of the invocation.
- **`head` and `read` are not atomic together.** A host whose log grows between them hands the fold a
  longer prefix than `after` claims. That is exactly what the append condition catches, which is why
  it is the one thing here that a host must implement rather than approximate.

## Related

- `docs/commands.md`: the append condition, and why a `fold` is a read declaration.
- `docs/effects.md`: rules 3, 5, 10, 11 and 12, which are the reasons this seam is cut where it is,
  and rule 16 for what a `Secrets` is asked and why a request has two renderings.
- `docs/projectors.md`: the other reader of the log, and the one writer of a read model.
- `docs/testing.md`: `given`, `respond` and `erased`, which are how the language scripts a harness.
