//! Where the time and the allocations go in a fold over a large log.
//!
//! `cargo bench`, or `cargo bench -- <events> <rounds>` to change the shape.
//!
//! **Allocations are the signal here, not milliseconds.** A count is exact and a
//! wall-clock reading is not, and every finding this benchmark has produced so far was
//! visible in the count first: an `Int?` state allocating four times per event where an
//! `Int` state allocated none is what turned up `has_type` comparing two types by
//! building them.
//!
//! Each case differs from its neighbour in one thing, so a difference has one cause.
//!
//! **Fewer allocations is not the same as faster, and this benchmark has the receipt.**
//! Making `Ident` an `Arc<str>` so a name is shared rather than copied halves what
//! `bind a sealed String` allocates, from four per event to two. It also makes that case
//! *slower*, 17.9 ms to 18.7 ms, and costs about 38% on every case that allocates
//! nothing at all: `accumulate Money(2)` goes 13.2 ms to 18.2 ms. The reason is that a
//! fold's hot path is not allocation, it is **name comparison**: `seal` alone scans the
//! event list and then the field list of the event it found, comparing names, once per
//! record. An `Arc<str>` puts the text one dependent load further away, and the
//! interpreter does that lookup often enough for the extra load to cost more than the
//! copies it saves.
//!
//! So the way to make names cheap is to stop looking them up, not to make them cheaper
//! to clone: resolve a field to an index at parse time, the way a variable already
//! resolves to a `Slot`. Measure before assuming otherwise; that experiment has been run.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use heklang::{Event, EventPath, Interpreter, Json, Record, Type, Value, parse};

static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);

struct Counting;

/// Counts what the fold asks for. Only `alloc` is counted: what is wanted is how much
/// the interpreter demanded, not how much was live at once.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const PRELUDE: &str = "
event @order.placed {
  order_id: Uuid,
  customer_id: Int,
  email: String @subject(customer_id),
  sku: String,
  total: Money(2),
}
";

const ORDER: &str = "0190d1a1-0000-7000-8000-000000000001";

/// The tail of every case, so the command has somewhere to end. One append against a
/// log of `n` is noise beside the fold.
const EMIT: &str = "  emit @order.placed { order_id, customer_id, email: \"x@e.com\", sku: \"S\", total: 1.00 }\n}";

/// `n` events, every one of them matching the filter, so the fold visits and folds all
/// of them. A benchmark whose predicate rejects most records measures the predicate.
fn log(n: usize) -> Vec<Event> {
    (0..n)
        .map(|i| {
            Event::new(
                EventPath::new(["order", "placed"]),
                [
                    ("order_id".to_string(), Value::uuid(ORDER)),
                    ("customer_id".to_string(), Value::Int(7)),
                    ("email".to_string(), Value::str(format!("c{i}@example.com"))),
                    ("sku".to_string(), Value::str(format!("SKU-{i}"))),
                    ("total".to_string(), Value::money(2_599, 2)),
                ],
            )
        })
        .collect()
}

struct Run {
    nanos: u128,
    allocs: usize,
    bytes: usize,
}

/// The best of `rounds`, because the fastest run is the one least disturbed by
/// everything else on the machine. The counts are the same every round.
fn measure(label: &str, fold: &str, events: &[Event], rounds: usize) {
    let source =
        format!("{PRELUDE}command Probe(order_id: Uuid, customer_id: Int) {{\n{fold}\n{EMIT}\n");
    let program = parse(&source).unwrap_or_else(|err| panic!("{label}: {err}"));

    let mut best: Option<Run> = None;
    for _ in 0..rounds {
        // Built outside the measured region: what is counted is the fold, not the log.
        let mut interpreter = Interpreter::with_log(&program, events.to_vec());

        let allocs = ALLOCS.load(Ordering::Relaxed);
        let bytes = BYTES.load(Ordering::Relaxed);
        let start = Instant::now();
        let execution = interpreter
            .run(
                "Probe",
                [
                    ("order_id", Value::uuid(ORDER)),
                    ("customer_id", Value::Int(7)),
                ],
            )
            .unwrap_or_else(|err| panic!("{label}: {err}"));
        let nanos = start.elapsed().as_nanos();
        let run = Run {
            nanos,
            allocs: ALLOCS.load(Ordering::Relaxed) - allocs,
            bytes: BYTES.load(Ordering::Relaxed) - bytes,
        };
        std::hint::black_box(&execution);
        if best.as_ref().is_none_or(|seen| run.nanos < seen.nanos) {
            best = Some(run);
        }
    }

    let best = best.expect("rounds is at least one");
    let n = events.len();
    println!(
        "{label:<34} {:>8.2} ms {:>7.1} allocs/event {:>8.0} B/event",
        best.nanos as f64 / 1_000_000.0,
        best.allocs as f64 / n as f64,
        best.bytes as f64 / n as f64,
    );
}

fn main() {
    let mut args = std::env::args().skip(1).filter(|arg| !arg.starts_with('-'));
    let n: usize = args
        .next()
        .and_then(|arg| arg.parse().ok())
        .unwrap_or(100_000);
    let rounds: usize = args.next().and_then(|arg| arg.parse().ok()).unwrap_or(5);

    // `Value` is what every `eval` returns by value, so its width is a cost on every
    // expression. The widest variant is `Map`, which holds two `Type`s inline.
    println!(
        "size_of  Value {}  Type {}  Json {}  Record {}  Event {}\n",
        size_of::<Value>(),
        size_of::<Type>(),
        size_of::<Json>(),
        size_of::<Record>(),
        size_of::<Event>(),
    );
    println!("fold over {n} events, best of {rounds}\n");

    let events = log(n);

    // The floor: visit every record, check the filter, add one. Nothing bound.
    measure(
        "count, no binds",
        "  state seen: Int = fold 0\n    on @order.placed(customer_id) => seen + 1",
        &events,
        rounds,
    );

    // Against the floor, this is the cost of reading a state variable at all.
    measure(
        "count, no state read",
        "  state seen: Int = fold 0\n    on @order.placed(customer_id) => 1",
        &events,
        rounds,
    );

    // Two slices over the same events, which is the shape a real command has.
    measure(
        "two slices",
        "  state seen: Int = fold 0\n    on @order.placed(customer_id) => seen + 1\n  \
         state sum: Money(2) = fold 0.00\n    on @order.placed(customer_id) { total } => sum + total",
        &events,
        rounds,
    );

    // A scalar payload: bound, read and accumulated without allocating once.
    measure(
        "accumulate Money(2)",
        "  state sum: Money(2) = fold 0.00\n    on @order.placed(customer_id) { total } => sum + total",
        &events,
        rounds,
    );

    // Every `Expr::Load` of a String clones it, so reading `sku` twice costs twice.
    measure(
        "bind a String, read it twice",
        "  state seen: Int = fold 0\n    on @order.placed(customer_id) { sku } => seen + sku.len() - sku.len() + 1",
        &events,
        rounds,
    );

    // A sealed field, which is what the port's credential folds bind. `seal` wraps the
    // stored text with the field, the subject and the id, so this is the widest bind
    // there is. It was four allocations and 123 B while a seal held a `Box<Value>` of
    // the plaintext; carrying the stored text instead makes the common bind a refcount
    // bump, and the content type moved to `Expr::Reveal` rather than sitting on every
    // value (which cost `Value` a third of its size, and every move with it).
    measure(
        "bind a sealed String",
        "  state seen: Int = fold 0\n    on @order.placed(customer_id) { email } => seen + 1",
        &events,
        rounds,
    );

    // The other side of sharing a string rather than owning it: building one costs an
    // allocation for the text and another to share it. Reads outnumber constructions in
    // a fold, but this is where the trade is paid rather than collected.
    measure(
        "build a String per event",
        "  state s: String = fold \"\"\n    on @order.placed(customer_id) { sku } => \"{sku}-x\"",
        &events,
        rounds,
    );

    // The optional pair. These differ in one character and nothing else, which is what
    // makes the gap between them attributable.
    measure(
        "Int state",
        "  state n: Int = fold 0\n    on @order.placed(customer_id) => 1",
        &events,
        rounds,
    );
    measure(
        "Int? state",
        "  state n: Int? = fold none\n    on @order.placed(customer_id) => 1",
        &events,
        rounds,
    );
    measure(
        "String state",
        "  state s: String = fold \"\"\n    on @order.placed(customer_id) => \"x\"",
        &events,
        rounds,
    );
    measure(
        "String? state",
        "  state s: String? = fold none\n    on @order.placed(customer_id) => \"x\"",
        &events,
        rounds,
    );

    // A String bound from the record and folded into an optional state, which is what
    // the port's credential folds actually look like.
    measure(
        "bind a String into a String?",
        "  state last: String? = fold none\n    on @order.placed(customer_id) { sku } => sku",
        &events,
        rounds,
    );
}
