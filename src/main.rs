use std::fs;
use std::process::ExitCode;

use heklang::{
    Event, EventPath, Interpreter, Invocation, Journal, Key, Outcome, Program, Reply, Store, Value,
    parse_files,
};

/// The demo's money scale. A storage precision floor, not a claim about a currency: a
/// program that handles several picks one that fits them all.
const SCALE: u8 = 2;

const COMMANDS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/hek/place_order.hk");
const PROJECTORS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/hek/orders.hk");
const EFFECTS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/hek/notify.hk");
const CATALOG: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/hek/catalog.hk");
const SHOP: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/hek/shop.hk");
const AUDIT: &str = "https://audit.example/catalog";
const SYNC: &str = "https://one.example/admin/api/sync";
const CONFIRM: &str = "https://mail.example/confirm";

fn order_placed() -> EventPath {
    EventPath::new(["order", "placed"])
}

fn order_cancelled() -> EventPath {
    EventPath::new(["order", "cancelled"])
}

fn customer_blocked() -> EventPath {
    EventPath::new(["customer", "blocked"])
}

fn uuid(seq: u32) -> String {
    format!("0190d1a1-0000-7000-8000-{seq:012}")
}

fn placed(seq: u32, customer_id: i64, email: &str, total: i64) -> Event {
    Event::new(
        order_placed(),
        [
            ("order_id", Value::uuid(uuid(seq))),
            ("customer_id", Value::Int(customer_id)),
            ("email", Value::str(email)),
            ("address", Value::str("1 Seed St")),
            ("total", Value::money(total, SCALE)),
            ("tax_rate", Value::decimal(825, 4)),
            ("notes", Value::str("")),
        ],
    )
}

fn seed() -> Vec<Event> {
    let mut log = vec![
        placed(1, 7, "big1@example.com", 60_000),
        placed(2, 7, "big2@example.com", 60_000),
        Event::new(
            customer_blocked(),
            [
                ("customer_id", Value::Int(99)),
                ("reason", Value::str("chargebacks")),
            ],
        ),
    ];

    for seq in 0..11 {
        log.push(placed(
            100 + seq,
            8,
            &format!("open{seq}@example.com"),
            1_000,
        ));
    }
    log.push(Event::new(
        order_cancelled(),
        [
            ("order_id", Value::uuid(uuid(100))),
            ("customer_id", Value::Int(8)),
            ("refund", Value::money(1_000, SCALE)),
        ],
    ));

    log
}

fn place(
    interpreter: &mut Interpreter<'_>,
    label: &str,
    seq: u32,
    customer_id: i64,
    email: &str,
    address: &str,
    notes: Option<&str>,
) {
    let mut args = vec![
        ("order_id", Value::uuid(uuid(seq))),
        ("customer_id", Value::Int(customer_id)),
        ("email", Value::str(email)),
        ("address", Value::str(address)),
        ("total", Value::money(2_599, SCALE)),
        ("tax_rate", Value::decimal(825, 4)),
    ];
    if let Some(notes) = notes {
        args.push(("notes", Value::str(notes)));
    }

    match interpreter.run("PlaceOrder", args) {
        Ok(execution) => {
            match &execution.outcome {
                Outcome::Ok(events) => {
                    for event in events {
                        println!("{label:16} ok       {event}");
                    }
                }
                Outcome::Invalid(message) => println!("{label:16} invalid  {message}"),
                Outcome::Reject { code, message } => {
                    println!("{label:16} reject   {code}: {message}")
                }
            }
            println!(
                "{:16}          read {} slices after position {}",
                "",
                execution.condition.slices.len(),
                execution.condition.after
            );
        }
        Err(err) => println!("{label:16} error    {err}"),
    }
}

fn discount(interpreter: &mut Interpreter<'_>, label: &str, units: i64) {
    match interpreter.run("BareDiscount", [("total", Value::money(units, SCALE))]) {
        Ok(_) => println!("{label:16} ok       exact, no rounding needed"),
        Err(err) => println!("{label:16} error    {err}"),
    }
}

fn read(path: &str) -> Result<String, String> {
    fs::read_to_string(path).map_err(|err| format!("{path}: {err}"))
}

fn load() -> Result<Program, String> {
    let commands = read(COMMANDS)?;
    let projectors = read(PROJECTORS)?;
    let effects = read(EFFECTS)?;
    parse_files([
        ("commands/place_order.hk", commands.as_str()),
        ("projectors/orders.hk", projectors.as_str()),
        ("effects/notify.hk", effects.as_str()),
    ])
    .map_err(|err| err.to_string())
}

/// The catalog module is its own program, so the orders demo above stays exactly what
/// it was and this section is purely additive.
fn load_catalog() -> Result<Program, String> {
    let catalog = read(CATALOG)?;
    parse_files([("catalog/catalog.hk", catalog.as_str())]).map_err(|err| err.to_string())
}

/// The shop module, likewise its own program. Rule 12's fold path needs a credential
/// appended long before the event being handled, which is a log of its own.
fn load_shop() -> Result<Program, String> {
    let shop = read(SHOP)?;
    parse_files([("shop/shop.hk", shop.as_str())]).map_err(|err| err.to_string())
}

fn main() -> ExitCode {
    let program = match load() {
        Ok(program) => program,
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::FAILURE;
        }
    };

    let mut interpreter = Interpreter::with_log(&program, seed());
    println!(
        "parsed {} events, {} commands, {} projectors and {} effects, seeded log of {} events\n",
        program.events.len(),
        program.commands.len(),
        program.projectors.len(),
        program.effects.len(),
        interpreter.log().len()
    );

    place(
        &mut interpreter,
        "fresh order",
        10,
        1,
        "ada@example.com",
        "12 Reykjavik St",
        Some("leave at the door"),
    );
    place(
        &mut interpreter,
        "blank address",
        11,
        1,
        "grace@example.com",
        "   ",
        None,
    );
    place(
        &mut interpreter,
        "email reuse",
        12,
        1,
        "ada@example.com",
        "12 Reykjavik St",
        None,
    );
    place(
        &mut interpreter,
        "blocked customer",
        13,
        99,
        "mallory@example.com",
        "9 Hekla Rd",
        None,
    );
    place(
        &mut interpreter,
        "loyal customer",
        14,
        7,
        "big3@example.com",
        "7 Hekla Rd",
        None,
    );
    place(
        &mut interpreter,
        "too many open",
        15,
        8,
        "over@example.com",
        "3 Hekla Rd",
        None,
    );
    place(
        &mut interpreter,
        "no notes",
        16,
        2,
        "grace@example.com",
        "4 Hekla Rd",
        None,
    );
    place(
        &mut interpreter,
        "address too long",
        17,
        3,
        "linus@example.com",
        &"x".repeat(201),
        None,
    );
    println!();
    discount(&mut interpreter, "bare * on 25.99", 2_599);
    discount(&mut interpreter, "bare * on 20.00", 2_000);

    println!("\nprojector Orders");
    // A fresh `put`, and a `Customer` row materialized from zeros by the patch.
    println!("  after the command scenarios");
    if let Some(store) = project(&interpreter, "after commands", "Orders") {
        rows(
            &store,
            "Customer",
            &["1"],
            &["order_count", "lifetime_spend", "name"],
        );
    }

    // The same customer again: the patch reads the row it wrote last time.
    interpreter.append(placed(20, 1, "ada+2@example.com", 5_000));
    println!("\n  after one more @order.placed for customer 1");
    if let Some(store) = project(&interpreter, "second order", "Orders") {
        rows(
            &store,
            "Customer",
            &["1"],
            &["order_count", "lifetime_spend", "name"],
        );
    }

    // `.total` is the stored price, `total` is the one the event carries.
    interpreter.append(repriced(10, 1_999));
    interpreter.append(shipped(10, "TRK-0001"));
    println!("\n  after @order.repriced and @order.shipped on order ...010");
    if let Some(store) = project(&interpreter, "repriced", "Orders") {
        rows(
            &store,
            "Order",
            &["000000000010"],
            &["total", "previous_total", "status", "tracking"],
        );
    }

    // Delete, then a patch that re-materializes the row from zeros.
    interpreter.append(purged(20));
    interpreter.append(shipped(20, "TRK-0002"));
    println!("\n  after @order.purged then @order.shipped on order ...020");
    if let Some(store) = project(&interpreter, "purged", "Orders") {
        rows(
            &store,
            "Order",
            &["000000000020"],
            &["total", "status", "tracking", "placed_at"],
        );
    }

    println!("\nprojector Notes");
    project(&interpreter, "note too long", "Notes");

    println!("\neffect NotifyCustomer");
    // The runtime absorbs the 503 and re-sends, so the handler only ever sees the 200.
    interpreter.script(CONFIRM, [Reply::Status(503), Reply::Status(200)]);
    let mut journal = Journal::default();
    notify(&mut interpreter, "notify order 1", 0, &mut journal);
    for (call, _) in journal.calls() {
        println!("{:16}          journaled {call}", "");
    }

    // The same journal makes this a replay: journaled calls return their recorded
    // result and are not performed again, while `reveal` and `log` run every time.
    println!();
    notify(&mut interpreter, "replay order 1", 0, &mut journal);

    println!();
    counters(&program);

    println!();
    rejected("erase last", ERASE_LAST);
    rejected("bad invoke", BAD_INVOKE);
    rejected("triggers itself", SELF_TRIGGER);

    match load_catalog() {
        Ok(catalog) => catalog_demo(&catalog),
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::FAILURE;
        }
    }

    match load_shop() {
        Ok(shop) => shop_demo(&shop),
        Err(err) => {
            eprintln!("{err}");
            return ExitCode::FAILURE;
        }
    }

    ExitCode::SUCCESS
}

fn connected(shop_id: i64, domain: &str, token: &str) -> Event {
    Event::new(
        EventPath::new(["shop", "connected"]),
        [
            ("shop_id", Value::Int(shop_id)),
            ("shop_domain", Value::str(domain)),
            ("access_token", Value::str(token)),
        ],
    )
}

fn sync_requested(shop_id: i64) -> Event {
    Event::new(
        EventPath::new(["shop", "sync", "requested"]),
        [("shop_id", Value::Int(shop_id))],
    )
}

/// The two absences that must not collapse, side by side: a shop that never connected
/// and a shop whose key is gone. One is an ordinary branch, the other is terminal.
fn shop_demo(program: &Program) {
    println!("\nmodule shop.hk");

    let log = vec![
        connected(1, "one.example", "shpat-one"),
        connected(3, "three.example", "shpat-three"),
        sync_requested(1),
        sync_requested(2),
        sync_requested(3),
    ];

    let mut interpreter = Interpreter::with_log(program, log);
    interpreter.script(SYNC, [Reply::Status(200)]);
    // Shop 3 connected and was then redacted. Nothing static catches that, which is
    // exactly what rule 12's message says.
    interpreter.erase_subject("shop_id", "3");

    let labels = ["connected", "never connected", "erased subject"];
    let mut seen = 0usize;
    for (label, position) in labels.iter().zip([2u64, 3, 4]) {
        let mut journal = Journal::default();
        match interpreter.deliver("SyncShop", position, &mut journal) {
            Ok(Invocation::Done) => println!("{label:16} done"),
            Ok(Invocation::Skipped(message)) => println!("{label:16} skipped  {message}"),
            Ok(Invocation::Failed(message)) => println!("{label:16} failed   {message}"),
            Ok(Invocation::Ignored) => println!("{label:16} ignored"),
            Err(err) => println!("{label:16} error    {err}"),
        }
        for line in &interpreter.lines()[seen..] {
            println!("{:16} {line}", "");
        }
        seen = interpreter.lines().len();
        for (call, _) in journal.calls() {
            println!("{:16} journaled {call}", "");
        }
    }
    // The one request that went out, carrying a credential nothing else in the log
    // could have produced.
    if let Some(sent) = interpreter.requests().first() {
        println!("\n  {} sent {}", sent.url, sent.headers);
    }
}

fn tier(name: &str) -> Value {
    Value::Enum {
        ty: "Tier".to_string(),
        variant: name.to_string(),
    }
}

fn tags(names: &[&str]) -> Value {
    Value::list(
        heklang::Type::String,
        names.iter().map(|name| Value::str(*name)),
    )
}

fn list_item(
    interpreter: &mut Interpreter<'_>,
    label: &str,
    seq: u32,
    sku: Option<&str>,
    price: i64,
    names: &[&str],
    which: &str,
) {
    let sku = match sku {
        Some(text) => Value::some(Value::str(text)),
        None => Value::none(heklang::Type::String),
    };
    let args = vec![
        ("item_id", Value::uuid(uuid(seq))),
        ("seller_id", Value::Int(1)),
        ("sku", sku),
        ("price", Value::money(price, SCALE)),
        ("tags", tags(names)),
        ("tier", tier(which)),
    ];
    match interpreter.run("ListItem", args) {
        Ok(execution) => match execution.outcome {
            Outcome::Ok(events) => println!("{label:16} {:8} {}", "ok", events[0]),
            Outcome::Invalid(message) => println!("{label:16} {:8} {message}", "invalid"),
            Outcome::Reject { code, message } => {
                println!("{label:16} {:8} {code}: {message}", "reject");
            }
        },
        Err(err) => println!("{label:16} {:8} {err}", "error"),
    }
}

/// Everything the port's first tranche added, in one module: a module-scope enum and
/// record, constants, two pure helpers, both containers, a `for`, a comprehension,
/// interpolation, and one arm on three event types.
fn catalog_demo(program: &Program) {
    println!("\nmodule catalog.hk");
    println!(
        "  {} events, {} commands, {} projectors, {} effects, {} fns, {} records, {} consts, {} enums",
        program.events.len(),
        program.commands.len(),
        program.projectors.len(),
        program.effects.len(),
        program.functions.len(),
        program.records.len(),
        program.consts.len(),
        program.enums.len(),
    );

    let mut interpreter = Interpreter::new(program);
    list_item(
        &mut interpreter,
        "derived sku",
        1,
        None,
        1_999,
        &["house", "new"],
        "Paid",
    );
    list_item(
        &mut interpreter,
        "given sku",
        2,
        Some("  MINE-1  "),
        2_500,
        &["new"],
        "Paid",
    );
    list_item(
        &mut interpreter,
        "sku taken",
        3,
        Some("MINE-1"),
        3_000,
        &[],
        "Paid",
    );
    list_item(
        &mut interpreter,
        "free tier full",
        4,
        Some("MINE-2"),
        1_000,
        &[],
        "Free",
    );

    interpreter.append(Event::new(
        EventPath::new(["item", "flagged"]),
        [
            ("item_id", Value::uuid(uuid(1))),
            ("seller_id", Value::Int(1)),
            ("reason", Value::str("counterfeit")),
        ],
    ));

    if let Some(store) = project(&interpreter, "catalog", "Catalog") {
        println!("\n  read model");
        rows(
            &store,
            "Listing",
            &[],
            &["sku", "price", "tier", "tags", "flags"],
        );
    }

    // One arm on three event types, so the flag above lands in the same body the two
    // listings did.
    println!("\n  effect AuditCatalog");
    interpreter.script(
        AUDIT,
        [Reply::Status(200), Reply::Status(200), Reply::Status(200)],
    );
    // One journal per invocation, which is what `drive` does: an invocation replays its
    // own calls and never another's.
    let mut seen = 0usize;
    for position in [0u64, 1, 2] {
        let mut journal = Journal::default();
        if let Err(err) = interpreter.deliver("AuditCatalog", position, &mut journal) {
            println!("  position {position}: {err}");
            continue;
        }
        for line in &interpreter.lines()[seen..] {
            println!("  {line}");
        }
        seen = interpreter.lines().len();
        for (call, _) in journal.calls() {
            println!("  journaled {call}");
        }
    }
    if let Some(sent) = interpreter.requests().first() {
        println!("  headers {}", sent.headers);
    }
}

/// Rule 4's counters, on their own small log so the three stay legible. An effect
/// quietly failing every event looks exactly like one quietly succeeding unless
/// `failed` is separate from `wedged`.
fn counters(program: &Program) {
    let log = vec![
        placed(30, 20, "ok@example.com", 1_000),
        placed(31, 21, "rejected@example.com", 1_000),
    ];
    let mut interpreter = Interpreter::with_log(program, log);
    interpreter.script(CONFIRM, [Reply::Status(200), Reply::Status(422)]);

    match interpreter.drive("NotifyCustomer") {
        Ok(counts) => {
            println!(
                "{:16} {:8} {} done, {} failed, {} skipped, {} wedged",
                "two orders",
                "counts",
                counts.done,
                counts.failed(),
                counts.skipped(),
                usize::from(counts.wedged.is_some())
            );
            for message in &counts.failures {
                println!("{:16}          failed: {message}", "");
            }
        }
        Err(err) => println!("{:16} error    {err}", "two orders"),
    }
}

fn notify(interpreter: &mut Interpreter<'_>, label: &str, position: u64, journal: &mut Journal) {
    let calls = interpreter.http_calls();
    let absorbed = interpreter.absorbed();
    let events = interpreter.log().len();
    let lines = interpreter.lines().len();

    match interpreter.deliver("NotifyCustomer", position, journal) {
        Ok(outcome) => {
            let (kind, detail) = match &outcome {
                Invocation::Done => ("ok", String::new()),
                Invocation::Ignored => ("ignored", String::new()),
                Invocation::Failed(message) => ("failed", message.clone()),
                Invocation::Skipped(message) => ("skipped", message.clone()),
            };
            println!("{}", format!("{label:16} {kind:8} {detail}").trim_end());
            println!(
                "{:16}          {} http call(s), {} absorbed, events {events} -> {}, {} log line(s)",
                "",
                interpreter.http_calls() - calls,
                interpreter.absorbed() - absorbed,
                interpreter.log().len(),
                interpreter.lines().len() - lines
            );
        }
        // A wedge does not advance, and the script cannot observe it.
        Err(err) => println!("{label:16} wedged   {err}"),
    }
}

/// Each of these is a program the checker refuses, printed with its location so the
/// rejection is something you can see rather than something the spec claims.
fn rejected(label: &str, arm: &str) {
    let source = format!("{REJECTED_PRELUDE}{arm}");
    match parse_files([("effects/notify.hk", source.as_str())]) {
        Ok(_) => println!("{label:16} ok       unexpectedly parsed"),
        Err(err) => println!("{label:16} error    {err}"),
    }
}

const REJECTED_PRELUDE: &str = "event @order.placed {
  order_id: Uuid,
  customer_id: Int,
  email: String @subject(customer_id),
}
event @order.notified { order_id: Uuid }

command RecordNotified(order_id: Uuid) {
  emit @order.notified { order_id }
}
";

/// Rule 9: `erase` is journaled and `reveal` is not, so the replay re-runs the reveal
/// against a key that is gone.
const ERASE_LAST: &str = "effect NotifyCustomer {
  on @order.placed as e {
    erase(e.customer_id)
    log(reveal(e.email))
  }
}
";

/// Rule 7: checked against the command's declared parameters, at compile time.
const BAD_INVOKE: &str = "effect NotifyCustomer {
  on @order.placed as e {
    invoke RecordNotified { order: e.order_id }
  }
}
";

/// An effect that reacts to what it causes is an unbounded event stream.
const SELF_TRIGGER: &str = "effect Loop {
  on @order.notified as e {
    invoke RecordNotified { order_id: e.order_id }
  }
}
";

fn short(key: &Key) -> String {
    match key {
        Key::Uuid(value) => value.rsplit('-').next().unwrap_or(value).to_string(),
        Key::Int(value) => value.to_string(),
        Key::Str(value) => value.clone(),
        Key::Timestamp(micros) => micros.to_string(),
        Key::Enum { variant, .. } => variant.clone(),
    }
}

fn rows(store: &Store, entity: &str, keys: &[&str], only: &[&str]) {
    for (key, row) in store.rows(entity) {
        let label = short(key);
        if !keys.is_empty() && !keys.contains(&label.as_str()) {
            continue;
        }
        let fields: Vec<String> = row
            .0
            .iter()
            .filter(|(name, _)| only.is_empty() || only.contains(&name.as_str()))
            .map(|(name, value)| format!("{name}: {value}"))
            .collect();
        println!("  {entity}[{label}] {}", fields.join(", "));
    }
}

fn project(interpreter: &Interpreter<'_>, label: &str, name: &str) -> Option<Store> {
    match interpreter.project(name) {
        Ok(store) => Some(store),
        Err(err) => {
            println!("{label:16} error    {err}");
            None
        }
    }
}

fn shipped(seq: u32, tracking: &str) -> Event {
    Event::new(
        EventPath::new(["order", "shipped"]),
        [
            ("order_id", Value::uuid(uuid(seq))),
            ("tracking", Value::str(tracking)),
        ],
    )
}

fn repriced(seq: u32, total: i64) -> Event {
    Event::new(
        EventPath::new(["order", "repriced"]),
        [
            ("order_id", Value::uuid(uuid(seq))),
            ("total", Value::money(total, SCALE)),
        ],
    )
}

fn purged(seq: u32) -> Event {
    Event::new(
        EventPath::new(["order", "purged"]),
        [("order_id", Value::uuid(uuid(seq)))],
    )
}
