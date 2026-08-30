//! `docs/guards.md` as executable rules. A guard is a named proposition about the log,
//! spliced into the command that names it, so the two things worth proving are that its
//! refusal arrives in the right order and that its slices reach the append condition.

use heklang::{Event, EventPath, Interpreter, Outcome, Program, Value, parse};

/// `@a.b { field: value }`, so a `given` in prose and a `given` in a test read alike.
fn given<'a>(
    interpreter: &mut Interpreter<'_>,
    path: &str,
    fields: impl IntoIterator<Item = (&'a str, Value)>,
) {
    interpreter.append(Event::new(
        EventPath::new(path.split('.')),
        fields
            .into_iter()
            .map(|(name, value)| (name.to_string(), value)),
    ));
}

fn program(source: &str) -> Program {
    match parse(source) {
        Ok(program) => program,
        Err(err) => panic!("{err}"),
    }
}

fn error(source: &str) -> String {
    parse(source)
        .map(|_| String::new())
        .expect_err("this source should not have parsed")
        .message
}

const EVENTS: &str = "\
event @course.defined { course: String, capacity: Int }
event @student.registered { student: String }
event @student.subscribed { course: String, student: String }
";

const GUARDS: &str = "\
guard CourseIsDefined(course: String) {
  state defined: Bool = fold false
    on @course.defined(course) => true

  if !defined {
    return reject(\"undefined_course\", \"no such course\")
  }
}

guard StudentIsRegistered(student: String) {
  state registered: Bool = fold false
    on @student.registered(student) => true

  if !registered {
    return reject(\"unregistered_student\", \"no such student\")
  }
}

guard CourseHasSeats(course: String) {
  state seats: Int = fold 0
    on @course.defined(course) { capacity } => capacity

  state enrolled: Int = fold 0
    on @student.subscribed(course) => enrolled + 1

  if enrolled >= seats {
    return reject(\"course_full\", \"that course is full\")
  }
}

command Subscribe(student: String, course: String) {
  guard CourseIsDefined { course }
  guard StudentIsRegistered { student }
  guard CourseHasSeats { course }

  emit @student.subscribed { course, student }
}
";

fn subscription() -> Program {
    program(&format!("{EVENTS}{GUARDS}"))
}

fn subscribe(interpreter: &mut Interpreter<'_>) -> Outcome {
    interpreter
        .run(
            "Subscribe",
            [("student", Value::str("s1")), ("course", Value::str("c1"))],
        )
        .expect("ran")
        .outcome
}

// ---------------------------------------------------------------------------------
// Rule 3: the guards decide in the order they are written.

#[test]
fn a_guard_refuses_before_the_body_runs() {
    let program = subscription();
    let mut interpreter = Interpreter::new(&program);
    assert!(matches!(
        subscribe(&mut interpreter),
        Outcome::Reject { code, .. } if code == "undefined_course"
    ));
    assert!(
        interpreter.log().is_empty(),
        "a refused command appends nothing"
    );
}

#[test]
fn the_order_on_the_page_is_the_precedence() {
    let program = subscription();
    let mut interpreter = Interpreter::new(&program);
    // Both the course guard and the student guard would refuse. The one written first
    // is the one that answers, which is the whole reason the ladder moved to the call
    // site: its order is visible there.
    given(
        &mut interpreter,
        "student.subscribed",
        [("course", Value::str("c1")), ("student", Value::str("s0"))],
    );
    assert!(matches!(
        subscribe(&mut interpreter),
        Outcome::Reject { code, .. } if code == "undefined_course"
    ));
}

#[test]
fn a_later_guard_answers_once_the_earlier_ones_hold() {
    let program = subscription();
    let mut interpreter = Interpreter::new(&program);
    given(
        &mut interpreter,
        "course.defined",
        [("course", Value::str("c1")), ("capacity", Value::Int(1))],
    );
    given(
        &mut interpreter,
        "student.registered",
        [("student", Value::str("s1"))],
    );
    given(
        &mut interpreter,
        "student.subscribed",
        [("course", Value::str("c1")), ("student", Value::str("s0"))],
    );
    assert!(matches!(
        subscribe(&mut interpreter),
        Outcome::Reject { code, .. } if code == "course_full"
    ));
}

#[test]
fn every_guard_holding_runs_the_body() {
    let program = subscription();
    let mut interpreter = Interpreter::new(&program);
    given(
        &mut interpreter,
        "course.defined",
        [("course", Value::str("c1")), ("capacity", Value::Int(2))],
    );
    given(
        &mut interpreter,
        "student.registered",
        [("student", Value::str("s1"))],
    );
    assert!(matches!(subscribe(&mut interpreter), Outcome::Ok(events) if events.len() == 1));
}

// ---------------------------------------------------------------------------------
// Rule 4: a guard's slices are the command's boundary.

/// The reason a guard is a declaration rather than a `fn`. What a command folded is what
/// it conflicts on, and a guard folds on the command's behalf, so its slices have to
/// arrive in the condition or the command would decide on a boundary it does not hold.
#[test]
fn a_guards_slices_reach_the_append_condition() {
    let program = subscription();
    let mut interpreter = Interpreter::new(&program);
    given(
        &mut interpreter,
        "course.defined",
        [("course", Value::str("c1")), ("capacity", Value::Int(2))],
    );
    given(
        &mut interpreter,
        "student.registered",
        [("student", Value::str("s1"))],
    );
    let execution = interpreter
        .run(
            "Subscribe",
            [("student", Value::str("s1")), ("course", Value::str("c1"))],
        )
        .expect("ran");
    // Four: `@course.defined` twice (once for `defined`, once for `seats`, which are
    // separate folds in separate guards), `@student.registered`, `@student.subscribed`.
    // The command itself declares none of them.
    assert_eq!(execution.condition.slices.len(), 4);
    let events: Vec<String> = execution
        .condition
        .slices
        .iter()
        .map(|predicate| predicate.event.to_string())
        .collect();
    assert_eq!(
        events,
        [
            "@course.defined",
            "@student.registered",
            "@course.defined",
            "@student.subscribed",
        ]
    );
}

/// A predicate carries the values its filters resolved to, and a guard's filters resolve
/// against the arguments the command passed. So the condition names the course the
/// command was called for, not the parameter the guard declared.
#[test]
fn a_guards_filters_resolve_against_the_arguments() {
    let program = subscription();
    let mut interpreter = Interpreter::new(&program);
    given(
        &mut interpreter,
        "course.defined",
        [("course", Value::str("c1")), ("capacity", Value::Int(2))],
    );
    given(
        &mut interpreter,
        "student.registered",
        [("student", Value::str("s1"))],
    );
    let execution = interpreter
        .run(
            "Subscribe",
            [("student", Value::str("s1")), ("course", Value::str("c1"))],
        )
        .expect("ran");
    let narrowed: Vec<(String, Value)> = execution
        .condition
        .slices
        .iter()
        .flat_map(|predicate| predicate.filters.clone())
        .collect();
    assert!(narrowed.contains(&("course".to_string(), Value::str("c1"))));
    assert!(narrowed.contains(&("student".to_string(), Value::str("s1"))));
}

// ---------------------------------------------------------------------------------
// Rule 5: guards compose.

const NESTED: &str = "\
event @shop.connected { shop_id: Int }
event @plan.created { plan_id: Int, shop_id: Int }
event @plan.archived { plan_id: Int, shop_id: Int }

guard ShopIsConnected(shop_id: Int) {
  state connected: Bool = fold false
    on @shop.connected(shop_id) => true

  if !connected {
    return reject(\"shop_not_found\", \"shop does not exist\")
  }
}

guard PlanExists(plan_id: Int, shop_id: Int) {
  guard ShopIsConnected { shop_id }

  state exists: Bool = fold false
    on @plan.created(plan_id, shop_id) => true

  if !exists {
    return reject(\"plan_not_found\", \"no such plan\")
  }
}

command ArchivePlan(plan_id: Int, shop_id: Int) {
  guard PlanExists { plan_id, shop_id }

  emit @plan.archived { plan_id, shop_id }
}
";

fn archive(interpreter: &mut Interpreter<'_>) -> Outcome {
    interpreter
        .run(
            "ArchivePlan",
            [("plan_id", Value::Int(1)), ("shop_id", Value::Int(7))],
        )
        .expect("ran")
        .outcome
}

#[test]
fn a_guard_a_guard_names_decides_first() {
    let program = program(NESTED);
    let mut interpreter = Interpreter::new(&program);
    assert!(matches!(
        archive(&mut interpreter),
        Outcome::Reject { code, .. } if code == "shop_not_found"
    ));
}

#[test]
fn a_nested_guards_slices_reach_the_command() {
    let program = program(NESTED);
    let mut interpreter = Interpreter::new(&program);
    given(
        &mut interpreter,
        "shop.connected",
        [("shop_id", Value::Int(7))],
    );
    given(
        &mut interpreter,
        "plan.created",
        [("plan_id", Value::Int(1)), ("shop_id", Value::Int(7))],
    );
    let execution = interpreter
        .run(
            "ArchivePlan",
            [("plan_id", Value::Int(1)), ("shop_id", Value::Int(7))],
        )
        .expect("ran");
    // `@shop.connected` is two levels down and still in the boundary. That is the cost
    // composition buys: the condition is the transitive closure, not what is on the page.
    let events: Vec<String> = execution
        .condition
        .slices
        .iter()
        .map(|predicate| predicate.event.to_string())
        .collect();
    assert_eq!(events, ["@shop.connected", "@plan.created"]);
}

// ---------------------------------------------------------------------------------
// Rule 6: what a guard may not do.

#[test]
fn a_guard_appends_nothing() {
    let message = error(&format!(
        "{EVENTS}guard G(course: String) {{
  state d: Bool = fold false
    on @course.defined(course) => true
  emit @student.subscribed {{ course, student: \"s\" }}
}}
"
    ));
    assert_eq!(
        message,
        "a guard decides whether a command may run, so it appends nothing"
    );
}

/// A guard holds by reaching its end, so a bare `return` says nothing. It is rejected
/// rather than ignored because a guard is spliced into a command, where the same
/// statement would mean the command succeeded and appended nothing.
#[test]
fn a_guard_returns_only_a_refusal() {
    let message = error(&format!(
        "{EVENTS}guard G(course: String) {{
  state d: Bool = fold false
    on @course.defined(course) => true
  if d {{ return }}
}}
"
    ));
    assert_eq!(
        message,
        "a guard holds by reaching its end, so this `return` says nothing"
    );
}

#[test]
fn a_guard_that_folds_nothing_is_a_fn() {
    let message = error(&format!(
        "{EVENTS}guard G(course: String) {{
  if course == \"\" {{ return reject(\"no\", \"no\") }}
}}
"
    ));
    assert_eq!(message, "guard `G` folds nothing");
}

#[test]
fn a_guard_has_no_clock() {
    let message = error(&format!(
        "{EVENTS}guard G(course: String) {{
  state d: Bool = fold false
    on @course.defined(course) => true
  if !d {{ return reject(\"{{now()}}\", \"no\") }}
}}
"
    ));
    assert_eq!(message, "a guard has no clock");
}

// ---------------------------------------------------------------------------------
// Rule 7: naming a guard.

#[test]
fn a_guard_must_be_declared() {
    let message = error(&format!(
        "{EVENTS}command C(course: String) {{
  guard Nope {{ course }}
  emit @student.subscribed {{ course, student: \"s\" }}
}}
"
    ));
    assert_eq!(message, "guard `Nope` is not declared");
}

#[test]
fn a_guard_takes_every_parameter() {
    let message = error(&format!(
        "{EVENTS}{GUARDS}command C(course: String) {{
  guard CourseIsDefined {{ }}
  emit @student.subscribed {{ course, student: \"s\" }}
}}
"
    ));
    assert_eq!(message, "guard `CourseIsDefined` needs `course`");
}

/// The same guard on the same arguments decides what it already decided. A cycle is a
/// different mistake; this one type-checks and runs and means nothing, which is what
/// makes it worth a diagnostic rather than a silent deduplication.
#[test]
fn the_same_guard_on_the_same_arguments_is_refused() {
    let message = error(&format!(
        "{EVENTS}{GUARDS}command C(course: String) {{
  guard CourseIsDefined {{ course }}
  guard CourseIsDefined {{ course }}
  emit @student.subscribed {{ course, student: \"s\" }}
}}
"
    ));
    assert_eq!(
        message,
        "guard `CourseIsDefined` is named twice on the same arguments"
    );
}

/// Different arguments are a different question, so the same guard may be named twice.
#[test]
fn the_same_guard_on_different_arguments_is_two_questions() {
    let source = format!(
        "{EVENTS}{GUARDS}command C(one: String, two: String) {{
  guard CourseIsDefined {{ course: one }}
  guard CourseIsDefined {{ course: two }}
  emit @student.subscribed {{ course: one, student: \"s\" }}
}}
"
    );
    let program = program(&source);
    let command = program.command("C").expect("declared");
    assert_eq!(command.calls.len(), 2);
}

/// A guard is copied into what names it, so the arguments never enter the question:
/// `guard A { n: n + 1 }` inside `A` does not terminate either.
#[test]
fn a_guard_cannot_name_itself() {
    let message = error(&format!(
        "{EVENTS}guard G(course: String) {{
  guard G {{ course }}
  state d: Bool = fold false
    on @course.defined(course) => true
  if !d {{ return reject(\"no\", \"no\") }}
}}
"
    ));
    assert_eq!(
        message,
        "`G` guards `G`: a guard is copied into what names it, so one that names itself has no end"
    );
}

#[test]
fn a_cycle_through_another_guard_is_refused() {
    let message = error(&format!(
        "{EVENTS}guard A(course: String) {{
  guard B {{ course }}
  state d: Bool = fold false
    on @course.defined(course) => true
  if !d {{ return reject(\"no\", \"no\") }}
}}
guard B(course: String) {{
  guard A {{ course }}
  state e: Bool = fold false
    on @course.defined(course) => true
  if !e {{ return reject(\"no\", \"no\") }}
}}
"
    ));
    assert_eq!(
        message,
        "`A` guards `B` guards `A`: a guard is copied into what names it, so one that names itself has no end"
    );
}

/// An argument is evaluated before any fold, so a `state` is not available to one. The
/// same rule a seed follows, for the same reason: the answer would be wrong rather than
/// late.
#[test]
fn a_guard_argument_cannot_read_a_state() {
    let message = error(&format!(
        "{EVENTS}{GUARDS}command C(course: String) {{
  state seen: String = fold \"\"
    on @course.defined(course) => course
  guard CourseIsDefined {{ course: seen }}
  emit @student.subscribed {{ course, student: \"s\" }}
}}
"
    ));
    assert_eq!(
        message,
        "`course` is taken from `seen`, which has not folded yet"
    );
}

/// A guard is its own name space, the second one reachable from source. `docs/declarations.md`
/// claims a command and a guard may share a name, so here is the claim.
#[test]
fn a_command_and_a_guard_may_share_a_name() {
    let source = format!(
        "{EVENTS}command Same(course: String) {{
  guard Same {{ course }}
  emit @student.subscribed {{ course, student: \"s\" }}
}}
guard Same(course: String) {{
  state d: Bool = fold false
    on @course.defined(course) => true
  if !d {{ return reject(\"undefined_course\", \"no such course\") }}
}}
"
    );
    let program = program(&source);
    assert!(program.command("Same").is_some());
    assert!(program.guard("Same").is_some());
}
