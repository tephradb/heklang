use std::fs;
use std::process::ExitCode;

use heklang::{Event, EventPath, Interpreter, Outcome, Program, Value, parse};

const SOURCE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/hek/place_order.hk");

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
            ("total", Value::Money(total)),
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
            ("refund", Value::Money(1_000)),
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
        ("total", Value::Money(2_599)),
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
                        println!(
                            "{label:16} ok       {}",
                            event.display(interpreter.currency())
                        );
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
    match interpreter.run("BareDiscount", [("total", Value::Money(units))]) {
        Ok(_) => println!("{label:16} ok       exact, no rounding needed"),
        Err(err) => println!("{label:16} error    {err}"),
    }
}

fn load() -> Result<Program, String> {
    let source = fs::read_to_string(SOURCE).map_err(|err| format!("{SOURCE}: {err}"))?;
    parse(&source).map_err(|err| format!("{SOURCE}:{err}"))
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
        "parsed {} events and {} commands, currency {}, seeded log of {} events\n",
        program.events.len(),
        program.commands.len(),
        program.currency.code,
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

    ExitCode::SUCCESS
}
