# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Named `fold` declarations. A fold can now be declared once at module scope and read by name,
  which is the value half of what `guard` does for a proposition: `fold spend = LifetimeSpend {
  customer_id }` puts the same slices in the same append condition as writing the fold inline.
  One keyword in two positions, parens to declare and braces to use, as `command`/`invoke` and
  `guard`/`guard` already are. Inline folds are unchanged and still the default; `docs/folds.md`
  rule 9 is the choice between them.
- `Sealed(T, Subject)` is spellable in a fold's return type and nowhere else, so a fold can hand
  back content from behind the decrypt boundary for its caller to `reveal`. The four most
  duplicated folds in the port that measured the construct are sealed.
- `duplicate-fold`: one inline fold written the same way in two bodies, which is `docs/folds.md`
  rule 9. An error rather than a warning, because a fold declares the slice its caller conflicts on
  and `docs/testing.md` §9 keeps the append condition out of a test, so nothing else reports two
  copies drifting apart. An effect's arm counts as a body. Compared as written, including the
  accumulator's name, so the check under-counts and never misfires. It fires 167 times on the
  pre-migration port, and found one duplicate the migration's own tooling had missed. The first
  copy's location is a related line and not part of the message, as it is for every other
  two-place diagnostic in the crate.
- `unused-accumulator`: a named fold that binds an accumulator name no arm reads. A fold's value
  is its declaration's name, so the name is written only where an arm reads the running value.
- A named fold's name is spelled like a type: an initial capital, and no `_`. The second name in
  heklang whose spelling leaves the program, after a `refusal`'s, and held to the same two rules for
  the same reason: a named fold is a stored column and the column's name is `snake_case` of this
  one, so the rules keep that derivation reversible and `fold Sum` beside `fold sum` can no longer
  derive one column twice. An inline fold's accumulator is a local and is unaffected.

  **A third rule joins them: the derived column may not be named after a part of its own key.**
  `fold Shop(shop_id: Shop)` asked for a table whose key column and whose only value column were
  both `shop`, which nothing can create, and `hek check --tables` printed it without complaint. Both
  names now come out of one function, `tables::part_names`, so the refusal reads the same derivation
  the layout does, and it names the parameter the colliding part came from. It is a refusal rather
  than a disambiguation because the author has somewhere to go, which is where `docs/folds.md`
  rule 1 already pointed: a fold is named for the value and not for the entity, so `ShopInstalled`
  is the name and `shop_installed` the column. A fold with no parameters has no part to collide
  with.
- `hek check --boundaries` names what a command reads as well as what it guards, transitively
  through its guards.
- **Compound `@key`.** An entity's key may be several columns: `@key` goes on each one, written
  order is the key's order, and every statement that names a row names all of it
  (`patch Period[shop_id, period] { .. }`, `delete`, `update`, and `expect` in a test). No new
  syntax: it is the annotation an effect arm's partition key already spells the same way, and a
  one-column key is the same rule with one column, so no existing entity changes shape. The arity
  is checked against the declaration and each position is typed from the column it addresses.
  `docs/projectors.md` rule 11 is the contract.

  What it replaces is a derived key column. Measured on the same 25,347-line application: five of
  eighteen entities carried one, which cost five fabricated key columns, four key-joining `fn`s,
  four `@max` bounds nobody chose, six indexes the key now serves, and **three subject columns
  declared optional** only so a materializing `patch` could find a zero for them. Migrating the
  five deleted 98 lines across four files and left 265/265 tests passing.
- `hek check --tables`: the read model a program's named folds imply. Nothing declares it, because
  a fold already says what a value is, which events change it and what it is keyed by, so a fold is
  a column, its parameters are the key, and a table is the folds that share one. The rules live in
  `heklang::tables` rather than in the CLI, because a runtime creating the real tables has to use
  the same ones. A parameter no arm supplies is listed as not stored rather than reported: the
  language does not require a fold to be storable, and there is no warning channel to say it in
  anyway. Checked against the 219-fold application the design was measured on, where it derives the
  same 26 tables the measurement's own script does.

- A fold arm may bind the envelope with `as`, and a fold that does is **query-only**: stored and
  queried, never folded by a command. The envelope stays out of every decision for the two reasons
  it always did, that a test cannot pin an append time and a backfilled event's is when it was
  imported, and neither is about a read model. Refused five ways: a command or guard folding one,
  an inline fold binding one, the binding inside a filter (which resolves once, where the envelope
  belongs to each event), a binding no arm reads, and a binding that would hide the accumulator or
  a parameter. Converting the twelve projectors of the ported application found
  this to be the largest single gap: they stamp or bucket by append time in 97 places across 20
  columns, and 13 of the 19 event types involved declare no timestamp at all. `docs/folds.md`
  rule 12.
- `hek check --tables` gives two key signatures that derive one table name a name each, by taking
  the part types into both. `(day: String)` and `(day: Int)` were two tables both called `day`,
  which a runtime keying on the name would have served as one. No program has done it.
- **`fold` is a fourth test action**, so a named fold's value can be asserted:
  `fold LifetimeSpend { customer_id: 7 }` and then `expect 250.00`. Written as a command reads a
  fold, minus the binding. Exactly one expectation, unlike every other action, because a fold has
  one accumulator and a second `expect` would be a second answer to one question; a test with none
  is refused too. `expect <value>` stays illegal under `run`, `project` and `deliver`, which is
  what keeps a free-standing `assert <expr>` out of the language. `docs/testing.md` section 8, and
  section 9 is rewritten: the prohibition on asserting state was always about an intermediate
  inside a decision, and a declaration with a signature and a key is not one. **A query-only fold
  is readable here and nowhere else**, which is the first execution of one anywhere.
- **A fold arm may match a key part the event does not carry**, written `param == value`:
  `on @hold.place.failed(shop_id, order_id, kind == "hold_place")`. The left side is a parameter
  rather than a field, so the conjunct says which key the arm writes rather than narrowing the
  event, and the sigil is the whole of the difference: `:` matches a field, `==` matches a
  parameter, and both are legal when one name is both. The right side resolves once in the frame of
  whatever reads the fold, under the same rule a field filter's value is, and may not read the event
  being folded. It may not read a **slot** either, a parameter or the accumulator: whatever writes
  the row holds the event and the key and no frame, so a conjunct reading one is a key that depends
  on the key, and `kind == kind` would otherwise make the column look storable on the strength of a
  tautology. So the right side is a literal, a `const`, or a `fn` of those. Also refused in a
  guard's slice and in an inline fold's arm (two reasons, two messages), for a name that is not a
  parameter, for two conjuncts on one part in one arm, and for a value the parameter's type cannot
  take. `docs/folds.md` rule 13.

  **A conjunct that does not hold drops the slice rather than widening it**, so it folds nothing and
  joins no append condition. That is the opposite of what the design predicted and is available only
  because the answer is fixed for the invocation: a command reading one kind of a nine-kind queue
  conflicts on that kind's events and no others. `Predicate`, `Query`, `Log` and `AppendCondition`
  are byte-identical, because the conjunct is not a predicate on the event and a store asked for a
  field the events do not carry would answer `false` for every record.

  What it is for is a queue holding several kinds of work, which the ported application has two of:
  a failure list of twelve kinds and a retry ladder of nine, each keyed by a synthetic
  `Uuid.derive(NS, "{kind}|{ids}")` that exists only because a projector entity gets one `@key`
  column. Five of the retry kinds are keyed `(shop_id, order_id)` and so collide without it; the
  alternative is one fold per kind per column, 36 and 120 where 12 and 30 will do.
- `Interpreter::fold(name, args)` folds one named fold over the whole log for one key, beside
  `project` and `project_into`. **The first thing that executes a named fold**: everywhere else one
  runs by having been spliced into its caller, so `docs/folds.md` rule 11 is rewritten too. It is
  the read a stored column is served by, and the test action goes through it rather than through a
  test-only path. Sealed content whose key has been erased reads back absent, exactly as the same
  column does through `project`.

  Restoring the 27 read-model tests that retiring a ported application's projectors deleted found
  four bugs in the first 78 cases: two in the converted folds (a seven-day overage clock started at
  the wrong threshold, and an order withheld before its contact event had no customer), and the two
  under Fixed below.
- `Interpreter::fold_stored(name, args, head)` is the read a store **fills** a column from, where
  `Interpreter::fold` is the one it is served by. Two differences, and a store needs both: the fold
  stops at the log length the store gives it, so every column of a row is at one and a query reads
  one, and a `Value::Sealed` is handed back rather than shredded, so the store holds the ciphertext
  and shreds on its own read. The second is what `project_into` does and `project` does not, and
  without it filling a non-optional sealed column whose subject was erased before the fill would
  fail, where the same column through a projector stores its ciphertext and raises at the query.

  **The bound is a head and not a position**, which is a correction to the first form of this call
  rather than a choice between two spellings. `Log::head` answers where the *next* append lands, and
  that is the only number a store has: it is what the seam hands out, what a command pins its read
  to, and what a checkpoint records. Taking the inclusive position `Query::upto` carries put a
  `head() - 1` at every call site and left an empty log with nothing to pass, since `0` is a
  position like any other, so a store filling a column before its first event would have folded that
  event into a row recorded as holding none of it. `fold_stored(.., 0)` is a column at its seed and
  `fold_stored(.., 1)` is the first event alone.
- **A key part an arm computes from the record**, written `key <param> = <expr>` after the
  destructure (`docs/folds.md` rule 14). A read model can now key by a month bucket, a lowercased
  email or a combination of fields, none of which any event carries. Rule 13 writes a part the
  event does not carry from a value fixed before anything is read; this writes one that is a
  function of the record, so it may read the arm's binds and its envelope binding.

  **It is a clause rather than a filter because of when it resolves.** Everything in the filter
  list resolves once, before any event is folded, which is what lets a field conjunct become a
  `Predicate` and a rule 13 conjunct a gate; this is evaluated per record. Spelling them alike and
  telling them apart by whether the right side happened to touch a field is not a distinction to
  rest a key on. `key` is soft, so no name is lost anywhere else.

  **A fold with one is query-only**, as rule 12's envelope binding makes one, and the refusal names
  the part. `month_of(e.at) == m` has no predicate form, so a command could narrow the log to
  neither those events nor conflict on them; the sink has no such problem, because it walks the log
  forward and evaluates the expression per record. The two directions are one expression read two
  ways: a by-key read compares and skips, while the sink writes the row at whatever it computes.

  Seen from the layout it is `ArmMatch::Computed`, and it counts as **pinned**, so a host deriving
  range indexes needs to know nothing about the new state. A subject-typed parameter is refused,
  because a subject's ids are minted and filed by the key store rather than derived. No new builtin
  was needed for the motivating case: `Timestamp.year()`/`month()` and `Int.pad(width)` compose
  into `month_of` in a pure `fn`.

  **A computed part and the key are kept apart, in both directions.** The part reads the record: its
  binds, the envelope binding, consts and pure `fn`s of those, and not a parameter or the
  accumulator. And no filter of the arm may read a part the arm computes. Each half is a driver that
  could not otherwise answer, and all three shapes below checked clean before this:

  - The sink evaluates the expression to decide which row the record belongs to, so it runs before
    there is a key to bind the parameters from and before there is a row holding the accumulator. A
    by-key read has both and answers, so `key bucket = n` and `key bucket = tag.len()` failed a
    `fill` with `slot read before it was set`.
  - A filter resolves once for the whole slice, against the key its caller asked for, and a computed
    part has no such key. `(label: month)` writes the part twice over, since a filter pins a part by
    its value rather than by its name, and `(label: month.upper())` pins nothing at all: the column
    stayed storable while the filter narrowed the log to the records whose field already equalled
    what the part computed, dropping every other one from the read model.

  **And the sink now checks an arm's narrowing before it evaluates a computed part**, which the
  second half above is what makes possible: with no conjunct able to read a part the sink has not
  computed, the frame-free ones can be resolved first. A by-key read skips a record its predicate
  rejects and never runs the expression, so `key rate = 100 / units` under `(kind: "paid")` failed
  the whole replay on a free record with no units where the same fold read by key answered.

  **The digest sorts a `key` clause list**, as it sorts a slice's field filters, key conjuncts and
  binds, so reordering the comma list is no longer a breaking change: nothing means an order there,
  since the parts are found by name, all of them must agree, and none may read another.
  `hek-digest 6` stands and needs no bump, because the clause itself is unreleased.

  **A computed part may not be sealed content**, which is the one position inside an arm where a
  seal is wrong. Rule 7 lets an arm fold sealed content into a sealed column, which is why the
  ordinary seal check stands down inside a fold; erasure empties a column and cannot reach a primary
  key, so a row filed under decrypted content would be personal data nothing could erase. It could
  not run either: the seal is a `Value::Sealed` at the sink, which will not reduce to a key, and a
  by-key read compares the plaintext it was asked for against the ciphertext and folds nothing, so
  `--tables` called the column stored, `fill` raised `malformed ir` and `fold` answered the seed.
  Return the sealed value as a column of its own instead, which is rule 7. The check is the ordinary
  seal check with the fold's stand-down lifted for this clause, so it runs at every position inside
  the expression: over the result alone a `fn` launders the subject away, and
  `key bucket = tag(email)` through `fn tag(s: String) -> String` checked clean.

  **A `key` clause's own comments trail the arm**, so `hek fmt` is a fixed point over one. The
  clause is mid-line and undelimited, so there is no group for the comment-aware list printer to
  break and no line of its own for a suffix to land on: emitted inside the clause, the suffix landed
  after the `=>` once the arm's value broke, a reparse read it as the value's, and `hek fmt --check`
  failed on a file `hek fmt` had just written. The hoist stops at the clause's direct children: a
  comment inside a part's *value* is placed by the value's own printer, and taking it as well
  printed it twice. That one is `docs/fmt.md`'s known gap reached through a fourth shape, since the
  clause is what first put an arbitrary expression in an arm's head.

  **And the clause breaks between its parts**, which it had no way to do: it sits in the head and
  the head has no break either, so two interpolated parts ran to 103 columns against a width of 90.
- **A per-record sink for the derived read model**: `Tables` and the `Columns` seam, which are to
  `fold_stored` what `Projection` and `Rows` are to a projector rebuild. `Tables::apply(record,
  columns)` applies one record to whatever columns its arms name, carrying the accumulator in the
  store, and `Tables::query` is the subscription a host follows the log with. `Cells` is the
  in-memory implementation, as `Store` is for rows, and `Tables` holds no host so a deployment
  drives it from a thread that never touches the log reader.

  **This is what a column should cost.** Both existing reads fold the whole log for one key, which
  is right for a backfill and wrong for following a log: on the ported application's order table the
  median write is two columns of 87 across 46 event types, so a store refolding per record paid two
  whole-log scans forever.

  It is **more correct** as well as cheaper. A host choosing which columns a record could have
  changed has only `ColumnArm`, which carries the key parts an arm pins and not the field filters it
  also wrote, so it has to over-approximate and refold columns the record never touched; `apply` has
  the filters and re-checks them per record, exactly as a projection re-checks a handler's path.

  Two obligations are the sink's own. **Fully pinned arms run before ranged ones**, within one
  record, because a range over a key this same record minted has to see it and nothing afterwards
  corrects a column already written; one record is the whole obligation, since a key an earlier
  record minted is already stored. And **the subscription is derived here rather than by each host**:
  one predicate per event any column folds, with no filters, because two columns on one event filter
  it differently. A store that built its own from `EventPath`'s `Display` kept the `@` that an event
  type does not have, matched nothing, and advanced its checkpoint over every batch while its status
  endpoint reported nothing behind.
- **`Tables::only(folds)`**: the same sink, restricted to the columns a rebuild is replaying into.
  `apply` runs every arm of every column, so a store replaying the log to rebuild one column pays
  for every other column over the whole of history, and a body that can `fail` or overflow fails the
  rebuild of a column it has nothing to do with. Handing the others a seed instead of their held
  value avoids the overflow case and not the rest, because the body still runs. The set is named by
  the **fold**, since a column's name is `snake_case` of it and a caller spelling that derivation out
  is a second copy of it; a fold no column is kept for is nothing to apply rather than an error.
  `Layout::column(fold)` names the table and column one fold is kept in, which is the pair anything
  reading a stored fold needs.

  **What it runs is `Layout::closure` of the set**, because a column is not a closed system. An arm
  leaving a key part open ranges over the keys its table holds, and those keys are minted by the
  pinned arms of that table's columns, so a replay of a ranged column alone mints none of them: the
  range covers nothing and the column comes out at its seed with nothing to say so. What the closure
  adds is the columns of that table that **mint**, the ones with a fully pinned arm, since a column
  no arm of which pins the whole key writes only at keys that already exist. A column whose every arm
  pins the whole key, which is the ordinary case, closes over itself.

  **It is a handle rather than an argument, and both halves are the same reason.** The closure costs
  a walk of the layout and a replay applies one record at a time, so computing it per call would put
  a sort and an allocation per record into the one API whose purpose is to make a rebuild cheaper.
  And because the closure is wider than what was asked for, `Rebuild::folds` has to be readable: the
  sink carries the accumulator in the store, so replaying onto a column that already holds its value
  folds every record into it twice. A store rebuilding `order_closed` against its live tables gets
  `order_total` applied beside it and silently doubled, so `folds` is the set it resets or shadows
  first and the extra columns are the ones it throws away afterwards. What it may not do is drop
  their writes as they go, because the **keys** are what the additions are for: recording each key
  and discarding the value is enough, since `Columns::keys` is all a ranged arm reads.
- **`fill` is a fifth test action**, and `expect <table>[key] { column: value }` a row expectation
  over the derived read model. It takes no name, because nothing declares what it fills: a program
  has one read model and `layout` derives all of it from the folds. This is `docs/testing.md`
  section 6 over a derived table, which is what those assertions were before the read side became
  folds: retiring a ported application's projectors turned 27 row tests into 78 `fold` tests, and
  this turns them back. The table and column names are checked when the test is parsed, so a typo
  names the tables that do exist. One thing reads differently from a projector's row and it is the
  fold's own rule: a column no arm wrote is at **its seed**, so `expect plan[7] { plan_revenue: 0 }`
  is an assertion rather than a missing field. A fold with no parameters has no row to name and says
  so, pointing at the `fold` action.
- A test compares a sealed column **through the world's key store** rather than against what is
  stored, for both a derived column and a projector's row. The runner opens the value first, with
  the same `Keys::decrypt` call `reveal` makes; an erased subject reads as the absent optional and a
  column with no absent value says so loudly. `fold_stored` still hands the seal back untouched,
  because a store depends on that.

  `docs/testing.md` section 6 used to argue the other way, that *a test has no key* so the stored
  form was all there was to compare and a real host answering false was correct. The premise stopped
  holding when the runner started taking a `World`: a runner driving a real world has that world's
  key store. Left alone it made every sealed-column expectation fail against a real deployment,
  which is what hekla's run of the ported corpus found, 313 of 314 with `shop__order[42,
  9001].order_ship_to` comparing ciphertext against the plaintext the test wrote. The harness hid it
  for the reason it hides nothing else: its key store is the identity function, so `tests/testing.rs`
  now drives these four cases through a world whose is not.
- `tables::Column` carries `args`, the parameter each key part came from in the table's key order,
  which is what `fold_stored` and `Tables::apply` take. A part is named for its subject or for its
  parameter with one trailing `_id` off, so the part's name is usually not callable, and a reader
  deriving it was reimplementing `part_names` to do it.
- `tables::part_names(params)` is public: the name each key part takes, which the parser now reads
  too (see the third naming rule above).
- `tables::fixed(program, fold, part, at)` reads the value an arm pins a key part to, which is
  rule 13's `param == value` seen from the store. The `ExprId` in `ArmMatch::Fixed` indexes the
  fold's own arena, so it cannot be evaluated without the fold; the conjunct resolves against an
  empty frame, which is the whole of the rule's guarantee, and the value arrives at the part's
  declared type, so a part of `Money(2)` pinned to `0` is an amount rather than an `Int`.

### Changed

- A `patch` or an `update` may no longer write a key column. See Fixed: this closed a hole rather
  than adding a restriction. `put` is unaffected, because it has no subscript and fills a key
  column like any other.
- The `Rows` host seam takes a row's key as a sequence (`&[Key]` reading, `Vec<Key>` writing), and
  `Store::get` and `Store::rows` with it. A single-column key is a one-element slice rather than a
  special case, and the sequence orders lexicographically, which is what keeps a prefix of a
  compound key a range the read API can page on.
- The digest version is `hek-digest 6`, and **every** hash moves with it, because the version line
  is hashed with the form. The bump is a named fold earning an entry of its own. A fold is not
  inlined away the way a `guard` is: `splice` leaves `program.folds` standing and
  `Interpreter::fold` runs one by name, so a fold is a declaration that does something as itself,
  and its entry hash is what a store keys a derived column by. The signature is the parameters and
  the return type, which is what decides the table, the key order and the column's type, so a moved
  `signature_hash` is a column that changes table or type and a moved `hash` alone is one that
  refolds in place.

  Three fixes on this branch move hashes beyond the bump itself, and none of the features does:
  compound keys move no hash at all, because a one-column key writes exactly the `(key x)` node it
  wrote before, and the `fold` test action moves the form only for a program that writes one, and
  only under `--tests`.

  The third of those fixes is the optional lift on a spliced call's argument, which adds a `Wrap`
  node to the caller's arena. It moves the form only for a declaration that passes a bare value to
  an optional `guard` or named `fold` parameter, and no `hek/` declaration does.

  A statement written below a `guard` moves from the stage's `(pre ..)` to its `(post ..)`, which is
  where `docs/guards.md` rule 3 always said it was, so the packed form of a command with one
  changes. Measured on a 25,347-line application: one declaration of 84 commands renders
  differently, and the `hek/` corpus does not change for it.

  A named fold whose return type already spells `Sealed(T, S)` is no longer sealed a second time, so
  its accumulator's type changes from `Opt(Sealed(Sealed(T, S), S))` to what the author wrote. One
  entry of the `hek/` corpus moves (`effect AnnounceTier`, the one declaration that reads a sealed
  named fold); in the port it is 5 sealed folds across 37 call sites in 13 declarations.

### Fixed

- **One column's arms now apply in the order they were written** when a record reaches more than one
  of them. `Tables::apply` runs every fully pinned arm before every ranged one, because a range over
  a key this same record mints has to see it, and it used to fold as it went: a column with one arm
  of each kind on one event therefore applied them in *pass* order, so the pinned arm counted and
  the ranged arm then overwrote it, where a refold resets and then counts. A fold is its arms
  applied in source order to one accumulator and the sink has to be the same fold, which is the
  claim `tests/sink.rs` is built on; no fixture had a column whose pinned and ranged arms shared an
  event path, so nothing caught it.

  The two passes now work out only *what* the record writes, the keys they mint accumulate in a set
  that a range unions in rather than being read back out of the store, and the work is grouped by
  column and key and folded in written order. A row is therefore one `Columns::value` and one
  `Columns::set` rather than one of each per arm.
- **An over-delivered key no longer aborts the record.** `Columns::keys` is held to `Log::read`'s
  cut, so a store with no index for a range may answer with every row of its table, and the sink now
  drops the keys the arm does not write instead of leaving them to the predicate re-check. It could
  not survive that: a part the arm pinned with a rule 13 conjunct is *bound* from whatever key came
  back, so the conjunct's gate went dead, and a dead gate there is read as a disagreement between
  `layout` and the arm rather than as a key to skip, which raised `malformed ir`.
- **`docs/folds.md` section 11's safety argument for a range arm was wrong**, and the boundary is now
  stated and tested rather than discovered. It said the keys a range can need "are exactly the keys
  the pinning arms mint", which is about which keys exist and not about *when*: a refold applies
  every ranged record to every key, while a sink walks forward and can only apply one to the rows
  that are there, so **a key minted after a ranged record never sees it**. An order placed after a
  `@shop.uninstalled` is `false` in the store and `true` on a refold. The sink's answer is the
  definition, because it is the one a store can produce and every path through the sink produces it,
  so a follow and a backfill agree with each other; an author who wants the other does not write a
  range.
- **A named fold laundered a `@subject` seal out through a plain declared return type.** A fold
  declared `-> String?` folding a `String @subject(shop_id)` field handed back the **plaintext**,
  and it reached a plain event field with no `reveal` on a clean `hek check`. An arm that folds
  sealed content sealed the *local slot*, which is right for an inline fold, whose value lives in
  that slot in the declaration that reads it; a named fold publishes its declared `ret` instead, so
  the caller's local, the IR and the column `hek check --tables` derives were all typed plaintext
  while the value was ciphertext, and nothing would ever have shredded that column for an erased
  subject. It is now `seal-boundary`, with the spelling rule 7 licenses in the hint. The
  byte-identical **inline** fold was refused the whole time, which is what made the asymmetry
  invisible: the shape looked covered. It caught a live instance in the converted application on
  the first run, a `Customer`-sealed address typed as a plaintext column.
- **Two key parts of one name were told apart by position, and a stored column could be written at
  the wrong address.** `_1`/`_2` counted declaration order, so `fold A(to_id: Region, from_id:
  Region)` and `fold B(from_id: Region, to_id: Region)` derived the same two names over the same
  two types, which is one table by `hek check --tables`' own grouping, with `region_1` meaning a
  different parameter in each. A writer following it would store one row's two columns at each
  other's addresses. They are told apart by the **parameter** now, which is the one thing unique
  within a fold and does not depend on order, so that pair is `(from, to)` either way round. It
  also closes a second hole: a suffix is spellable, so a parameter actually called `region_1`
  collided with another part's suffix and produced two key columns of one name.
- **A clash-broken table name was not an identifier.** The name that tells two key signatures apart
  by part type put `Type`'s own rendering through `snake_case`, which only lowercases, leaving
  `shop_shop__amount_money(2)` and `shop_shop__tag_string?`; a sealed part added a comma and a
  space. The module promises a runtime creates the real tables from these names.
- **`snake_case` broke on ASCII uppercase while lowercasing with Unicode**, so `FÄb` and `Fäb`
  derived one column and one refusal code. It breaks on any uppercase now, which is the relation it
  already used to lowercase, and is what the naming rules on a fold and a refusal are written to
  rely on.
- **`Sealed(T?, S)` built the nesting inside out.** A trailing `?` was handled and an inner one was
  not, so `Sealed(String?, Shop)` became `Sealed(Opt(String), Shop)`: a type no other position can
  write, whose own diagnostics then contradicted each other. All three spellings go through
  `types::seal` now and mean one type.
- `Stmt::Delete` read its span by indexing the key list without a guard, which is the one unguarded
  index the compound-key change added. Unreachable from a valid program, so an empty list is
  `MalformedIr` rather than a panic.
- **An `if` expression's arms could not differ by an optional, even at a position declaring one.**
  `if c { e.at } else { held }` into a `Timestamp?` was refused both ways round with "these
  branches give a Timestamp and a Timestamp?, so this `if` has no one type". Each arm is its own
  declared position and each already accepted the bare value; what refused the program was the
  comparison *between* the arms, which used equality where everything around it uses the coercion
  rule. Each arm is now lifted on its own, and `type_of` reads the pair the same way so a nested
  `if` still reports a type. Where nothing declares one the arms are still compared and still have
  to agree, because then the `if` is the only place the disagreement can be said. An `if`
  *statement* was never affected: it produces no value. `docs/optionals.md`'s table gains the
  position, and the list of times that table has been short goes from five to six.

  Found twice in two days converting a ported application, and both times the author split the
  expression across two `return`s in a `fn` purely to get the two declared positions the `if` would
  not give.
- **A spliced call's argument was not coerced into an optional parameter.** `fold s = Sum { bias: 5 }`
  where `bias` is `Int?` put a bare `Int` in the slot, and the callee's `bias.is_none()` died at run
  time on a clean `hek check`; the same hole was on the `guard` path. Every other declared position
  coerces at run time, where the type is in hand, but the splice turns an argument into a
  `Stmt::Assign`, which carries no declared type, so the lift is written in the parser at the call
  site. It also kept one declaration answering one way: `Interpreter::fold` reads the same fold
  through `bind_params`, which does coerce, so a `fold` test could otherwise pass on a fold no
  command could run. `docs/optionals.md`'s table gains the position, and `Expr::Wrap` gains a second
  synthesiser.
- **`hek check` aborted on a fold arm reading a payload field through its envelope binding.**
  `on @a.one as e (shop_id) => e.n` reached the handler path that binds a field on demand, and a
  module-scope fold has no event declaration there to unwrap, so the compiler panicked with
  `only called inside a handler`. It is now a diagnostic naming the field and where it belongs. The
  sixth refusal in `docs/folds.md` rule 12.
- **An envelope binding could be rebound as a field of its own arm.** `on @a.one as e (shop_id) { e }`
  checked clean: the shadow check runs before the arm's binds are allocated, so this was the one
  spelling it could not see, and resolution puts the envelope ahead of the scopes, which left the
  bind unreachable for that arm. Now `declared-twice` at the bind.
- **A slice filter on an optional event field matched nothing.** `on @hold.release.failed(shop_id,
  fulfillment_order_id)` where the field is declared `Int?` compared a bare `Int` against the
  optional the event holds, so the slice folded no events at all. The parser accepts the bare value,
  because that is the coercion rule at a declared position, and the predicate the fold read with
  carried the unwrapped one. Silent: the arm reads as written, `hek check` passes, and the only
  symptom is a value that is always the seed. **Not a read-model bug**: the same filter in a
  command's fold decides on nothing. The fit is applied where the predicate is built rather than
  where a record is matched, because the predicate is also what a host narrows with.
- **A named fold holding sealed content handed back the plaintext after the key was erased.** The
  shredding that empties a sealed column lives in `project`'s wrapper over the `Rows` seam, and the
  fold read had no equivalent, so a fold declared `Sealed(Address, Customer)?` read as the address
  in the clear for an erased customer. `Interpreter::fold` now shreds the value it returns, and a
  fold whose type is not optional raises `ShreddedColumn` the way a required column does.
  `project_into` is deliberately still unchanged: those rows belong to a store that shreds on its
  own read.
- **A named fold's arm filters were never checked against its own stage.** A command's body closes
  each stage through `decl_body`, which runs the check; a fold's body is arms only, so nothing was
  calling it. `fold F(..) -> acc: Int = 0 { on @a.b(id: acc) => acc + 1 }` reported `checked 1 file`
  and then died at run time with "slot read before it was set", which is the one shape
  `Builder::fold_call`'s own comment calls the thing a checker must not allow. The inline form was
  always checked; only the named one escaped.
- **A fold arm could silently discard the running value.** Where one arm reads the accumulator and
  another binds an event field of the accumulator's name, the bind wins for that arm, so
  `on @order.adjusted(..) { spend } => spend + 1.00` folded the event's `spend` and reset the
  lifetime total. `unused-accumulator` could not see it, because the *other* arm satisfies that
  check. Now `declared-twice` at the bind, for the inline form as well, and only when some arm does
  read the accumulator: a fold whose every arm replaces the value has nothing to shadow, and
  `fold token: String? = none / on @shop.connected { token } => token` stays the ordinary spelling
  of that.
- **A fold's declared seal was applied twice, and its declared subject was never checked.**
  `-> Sealed(String, Customer)?` lowered to `Opt(Sealed(Sealed(String, Customer), Customer))`:
  unwritable in source, disagreeing with the `Update::ty` of its own arms, and hashed in that shape.
  The double seal is also why nothing noticed that `-> Sealed(String, Shop)?` over an arm folding
  content sealed under `Customer` checked clean; the two have to name the same subject, because
  `reveal` names the key by it.
- `reveal` and a `secret` read inside a fold's arm reported `wrong-context` with a message about
  what a command may do, rather than the `fold-restriction` `docs/folds.md` rule 10 promises. The
  context gate answers for a fold as though it were the command it is spliced into, so it has to run
  after the fold check, not before. The old message was wrong on its face for a fold read from an
  effect arm.
- The hint for a seed or an argument that reads a fold too early said it reads "its own seed", which
  is true of an accumulator and false of a named read: a read's destination is assigned by the
  splice after the stage, so nothing seeds it and the author was sent looking for one that does not
  exist.
- **A row's key column could disagree with the row's own key.** `put` reads a key off the row's
  columns and a `patch` takes it from its subscript, so `patch Order[order_id] { order_id: order_id
  + 100 }` checked clean, ran, and filed a row under id 7 holding `order_id: 107`. The same row
  disagreed with itself about which row it was, a later `put` of it would have been filed somewhere
  else again, and nothing static or dynamic reported any of it. A key column is now written by the
  subscript and by `put`, and by nothing else.
- The `@max` invariant did not read a materializing `patch`'s **subscript**, only its body, so a
  bounded key column fed from an unbounded event field was the same two declarations disagreeing
  with nothing to catch it. An `update` is still left out, because it never materializes and so its
  subscript stores nothing.
- `hek fmt` lifted a comment written inside a row subscript out to above the whole statement. The
  subscript is its own node now, so a comment stays with the key it was written against.
- **A statement below a `guard` ran before the guard decided.** A bare `return`, an `invalid` or a
  conditional `reject` under `guard G { .. }` answered first and `G` was skipped, with nothing
  reported. A guard declares no slice at parse time, so the stage looked empty and the statement was
  taken for one written *above* the declarations. An `emit` there was survivable, because a refusal
  replaces the outcome; the others were not.
- `hek fmt` deleted a named fold's return type and seed, and a sealed return's subject, when a
  comment appeared anywhere in the header, leaving source that no longer parsed. The three new
  printers indexed positionally over a child list that includes comments; they use `parted` now,
  and the `INTERLEAVED` fixture grew the cases that catch it.
- A named read's destination slot was not marked pending, so `docs/folds.md` rule 2's "an argument
  may not read a fold from its own stage" missed every named fold: a read's argument, a guard's
  argument, a seed and a filter all passed `hek check` and then died at run time with "slot read
  before it was set".
- A guard or a fold spliced into a command's **second** stage landed past the statements already
  there, so the caller could read a slot the splice assigned afterwards. The insert offsets are now
  per stage rather than per caller. Reachable with guards alone, and rare only because
  `docs/guards.md` rule 3 puts guards at the top.
- A guard whose body only reads named folds is no longer reported as folding nothing.

## [0.11.1](https://git.tqwewe.com/tephra/heklang/compare/heklang-v0.11.0...heklang-v0.11.1) - 2026-10-07

### Added

- a fold arm reads the record it folds, never the triggering event

### Other

- the envelope rule, and the field an event a fold must time keeps

## [0.11.0](https://git.tqwewe.com/tephra/heklang/compare/heklang-v0.10.0...heklang-v0.11.0) - 2026-09-21

### Added

- an event's field block takes the bare-name shorthand every other block takes
- an event is a `fn`'s return type, and a `given` is checked against its declaration
- `&&` narrows where it is true and `||` where it is false, operands included
- `List.sum()` totals where `+` is defined, and `List.concat` joins two lists

### Fixed

- an empty comprehension takes its element type from its yield, not from Json
- a loop body cannot accumulate into an outer `let`, and a branch is a scope

### Other

- update heklang skill around projectors

## [0.10.0](https://git.tqwewe.com/tephra/heklang/compare/heklang-v0.9.0...heklang-v0.10.0) - 2026-09-21

### Fixed

- an erase empties the sealed column too, and a seal cannot be put in a box
- a value-ending declaration no longer swallows the soft one below it

### Other

- a composite seal is opened by reveal, never read part by part

## [0.9.0](https://git.tqwewe.com/tephra/heklang/compare/heklang-v0.8.0...heklang-v0.9.0) - 2026-09-18

### Added

- a subject is a declared type whose values are its ids
- the fixed-length timestamp units, and Int.pad(width)

### Fixed

- a sealed record, list or map reveals as what it was sealed from

### Other

- the demo's address is a record, sealed whole

## [0.8.0](https://git.tqwewe.com/tephra/heklang/compare/heklang-v0.7.0...heklang-v0.8.0) - 2026-09-15

### Added

- [**breaking**] an answer's message is written, not computed

### Fixed

- hek fmt counts only the files it could read

### Other

- the grammar's string message is the language, not a narrowing

## [0.7.0](https://git.tqwewe.com/tephra/heklang/compare/heklang-v0.6.1...heklang-v0.7.0) - 2026-09-15

### Added

- [**breaking**] an answer ends the declaration, so it takes no return and no parens

## [0.6.1](https://git.tqwewe.com/tephra/heklang/compare/heklang-v0.6.0...heklang-v0.6.1) - 2026-09-12

### Added

- a string can be cut to the bound it is written into

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
