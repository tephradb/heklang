# Containers

`List(T)` and `Map(K, V)`, the two ways to iterate them, and the reasons the set stops there.

Until now heklang had no way to hold more than one of anything. A JSON array could arrive in a
response body and be carried around, and nothing could take it apart. A real port needs a line-item
list on an order, a plan catalogue keyed by plan id, a SKU uniqueness map, the customer set a shop
redaction erases, a webhook topic table and an embed field list, so this is not an optional
convenience.

## `List(T)`

```
let ids = [first_id, second_id]
```

| Method | Returns |
| --- | --- |
| `.len()` | `Int` |
| `.is_empty()` | `Bool` |
| `.contains(x)` | `Bool` |
| `.first()` | `T?` |
| `.push(x)` | `List(T)` |
| `.remove(x)` | `List(T)` |

An element written into a `List(T?)` or a `Map(K, V?)` wraps if it is a bare `T`, the same rule that
applies at every other declared position (`docs/optionals.md`). So `xs.push(name)` on a
`List(String?)` stores `some(name)`, and `.first()` on it reads back a `String??` that is `some` twice
over rather than a shape nothing can branch on.

**`push` and `remove` return a new list.** Nothing mutates, so a fold arm still returns new
state and a value that was handed to something else cannot change underneath it. That is the same
property the fold already relied on for scalars, extended rather than excepted.

`remove(x)` drops **every** element equal to `x`, not the first. That makes it idempotent, which
matches `Map`'s `remove` and matches how it is actually used: a fold arm removing an id from an
ordered list on a delete event, where the id occurs once and running the arm twice must not differ
from running it once.

There is no indexing (`xs[0]`). `.first()` covers the one case that came up, returns an optional
rather than trapping, and does not invite the out-of-bounds question at all. Anything else is a
`for` or a comprehension.

## `Map(K, V)`

```
fold skus: Map(Uuid, String) = Map.empty
  on @warranty.plan.created(shop_id) { plan_id, sku } => skus.set(plan_id, sku)
```

| Method | Returns |
| --- | --- |
| `.get(k)` | `V?` |
| `.set(k, v)` | `Map(K, V)` |
| `.remove(k)` | `Map(K, V)` |
| `.contains(k)` | `Bool` |
| `.len()` | `Int` |
| `.is_empty()` | `Bool` |
| `.keys()` | `List(K)` |
| `.values()` | `List(V)` |

`Map.empty` is a constructor, so it is named by its type. That is the rule `Uuid.derive` established
in `docs/effects.md` rule 11: the global namespace holds actions with no natural receiver, and
anything built from nothing belongs to its type.

### A key must be a type that orders

`K` is restricted to `Int`, `String`, `Uuid`, `Timestamp` and enums, exactly the set an entity key is
restricted to in `docs/projectors.md`, for exactly the same reason and with the same wording in the
error. A key that cannot order cannot give a defined iteration order, and a key that cannot hash
cannot be a key at all. `Decimal` and `Money` are the interesting exclusions: they compare across
scales, so two keys can be equal without being identical.

### Iteration is sorted by key, and that is load-bearing

Not insertion order. **Verify mode's determinism is the reason**: `docs/effects.md` rule 14 requires
that the same object built twice serialises identically, and a map whose iteration depends on the
order events happened to arrive cannot promise that. Two replays of the same log would produce two
byte sequences, and the whole point of verify mode is that a difference means a real difference.

An insertion-ordered map would have to justify itself against that, and the cost is not small: the
insertion order is extra state that has to be folded, journaled and compared, all so that a container
can answer a question the author has not said they are asking.

**One real case needed insertion order and worked around it**, which is the honest evidence rather
than a claim that nobody wants it: a port gives a shop's *oldest surviving* plan the default variant
of a master product. A sorted map cannot answer "oldest", so that fold carries a separate
`fold plan_order: List(Uuid)` beside the map. The workaround is fine, and it is better than it
looks: the list says out loud that order matters here, where an insertion-ordered map would have hid
that in the container's choice.

## Where an empty container's type comes from

`[]` and `Map.empty` hold nothing, so nothing in them says what they hold. The type comes from the
target: a `fold` declaration, a command or `fn` parameter, an event or entity field, or the argument
position of a method that already knows (`plans.get(id).unwrap_or(Map.empty)` takes it from `plans`).
Without one, both are a compile error that names those places.

**Inside an object literal there is no target, and `[]` needs none.** A JSON body's values are typed
by what they are rather than by where they land, so `{ "tags": [] }` is an empty array and there is
nothing left to decide: it serialises the same whatever it would have held. Demanding a declaration
there named three places an author cannot reach from inside a body, which read as a bug and was one.
It holds at any depth and wherever an object literal is legal, since a `Json.encode` argument and a
`fn` that returns a `Json` are bodies too:

```
http.post(url, { "tags": [], "meta": { "ids": [] } })
Json.encode({ "plans": [] })
```

`Map.empty` is not on this list. A JSON object is written `{ ... }`, so a map never lands in a body
without a declared type to have come from.

`let` takes no type annotation, so it is not one of them. That is deliberate: adding annotations to
`let` would be a second way to write down a type, in a language where every other binding gets its
type from a declaration that already exists. Across a 3,186-line port, every empty container had a
declared target already, so the annotation would have paid for itself nowhere.

## `for`

```
for id in ids { ... }
for plan_id, plan in plans { ... }
for index, item in items { ... }
```

One name binds the item of a list. Two names bind a key and a value over a map, or an index and an
item over a list. One name over a map is an error that says to write two, because "the element of a
map" is not a thing the language has: there is no pair type, and adding one to serve a loop shape
nobody asked for would be a type earning its keep only inside `for`.

A `for` always terminates: it runs once per element of a finite container, and there is no `while`.
That matters more here than in most languages, because a `fold` has to reproduce without a
journal and a command may be retried.

There is no `break` and no `continue`. Every search in a 3,186-line port is a pure `fn` with an early
`return`, and every accumulation is a comprehension or a fold arm; neither shape needs either
keyword. `return` inside a `for` does propagate out of it, which is what makes the search shape work.

### `erase` inside a loop

`docs/effects.md` rule 9 wrote down what a loop would need before there was one, and this is it: an
`erase` anywhere in a loop body is reachable from every `reveal` in that body, **including a reveal
lexically above it**, because the body runs again. The analysis takes a second pass over the body
seeded with what the first pass found, which is the fixed point, since the lattice has two elements.

So this is rejected, while the same two statements in the same order outside a loop are fine:

```
for x in xs {
  log(reveal(e.email))     // rejected: the erase below runs before the next iteration
  erase(e.customer_id)
}
```

## Comprehensions

```
[plan.title for plan_id, plan in plans if plan_status.get(plan_id).unwrap_or(Draft) == Active]
```

`[ <expr> for <bindings> in <container> (if <condition>)? ]`. The bindings follow `for`'s rules
exactly, so there is one thing to learn rather than two.

The produced expression is written first even though the bindings it uses are introduced later. That
is the order every language with comprehensions uses and the order the reader wants, since the shape
of the result is the point and the loop is the detail. The parser handles it by finding the `for`
first and parsing the loop before going back for the expression, which costs one scan and keeps the
grammar the one people already know.

## No mutable bindings

`let` only. There is no `var`, and this pass deliberately did not add one.

That was an open question rather than a decision: containers are usually what forces a mutable
accumulator into a language. **Across 3,186 lines of ported application code, none was needed.** Every
accumulation is either a comprehension or a fold arm returning new state, and every search is a pure
`fn` with an early `return`. It is recorded here as tested rather than assumed, with a test that
holds the shape: an accumulation over a container is a comprehension, and a search is a `fn` whose
`for` body returns.

Immutability is also what makes the rest of the language's promises cheap. A fold arm that returns
new state cannot alias the old one, a value passed to a helper cannot come back changed, and a replay
cannot diverge because something was written twice in a different order.

## What is deliberately absent

- **No set type.** A `Map(K, Bool)` covers membership, and the one place a port wanted a set it
  wanted an ordered one, which is a list.
- **No tuple type.** `for k, v` binds two names rather than one pair, so nothing needs a pair value.
  A record is the answer when two values genuinely travel together.
- **No nested-container literal shorthand.** `[[1, 2]]` works because a list literal is an ordinary
  expression; there is simply no special form for it.
- **No `sort`, `map`, `filter` or `fold` methods.** A comprehension covers `map` and `filter`,
  iteration order is already defined, and a fold over a container is a `for` in a pure `fn`. Adding
  a second way to spell a comprehension is the kind of surface this language is trying not to grow.
