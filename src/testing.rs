//! Running `test` declarations. `docs/testing.md` is the contract.
//!
//! Everything here goes through the same public API an embedder has, which is what
//! keeps rule 8 honest: a test cannot observe anything a program could not.

use std::collections::BTreeMap;
use std::fmt;

use crate::harness::{Reply, Sandbox};
use crate::host::{Host, Rows};
use crate::interp::{Effectful, Error, ErrorKind, Interpreter, Outcome, Row, coerce, subject_id};
use crate::ir::{
    Action, Expect, ExprId, Exprs, Ident, Program, ReplySpec, Setup, Span, Test, Type,
};
use crate::value::{Event, Json, Key, Value};

/// One test's verdict, in declaration order.
#[derive(Debug, Clone)]
pub struct TestResult {
    pub name: String,
    pub module: Option<Ident>,
    pub outcome: TestOutcome,
}

impl TestResult {
    pub fn passed(&self) -> bool {
        matches!(self.outcome, TestOutcome::Passed)
    }
}

/// Rule 9: a mismatch and an error are different facts. One is the test doing its job;
/// the other is the program being unable to run at all.
#[derive(Debug, Clone)]
pub enum TestOutcome {
    Passed,
    Failed(String),
    Errored(String),
}

impl fmt::Display for TestResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.outcome {
            TestOutcome::Passed => write!(f, "pass  {:?}", self.name),
            TestOutcome::Failed(why) => write!(f, "FAIL  {:?}: {why}", self.name),
            TestOutcome::Errored(why) => write!(f, "ERROR {:?}: {why}", self.name),
        }
    }
}

/// The world one test runs in: a log to seed, a network to script, keys to shred, and
/// the read models a `project` writes into and a row expectation reads back from.
///
/// [`Sandbox`] is the in-memory one and a real runtime brings another, so a suite can be
/// run against the store it will actually deploy on without a second definition of what
/// `expect` means.
///
/// Rule 8 survives the generalisation. Everything the runner asserts still comes from
/// the public API: an outcome, the effect trace, and one row through [`Rows::row`],
/// which is the read a `patch` already makes. A world is asked for nothing a program
/// could not reach.
pub trait World: Sized {
    type Host: Host;
    type Rows: Rows;

    /// One `given` event, appended with the world's own envelope.
    fn given(&mut self, event: Event) -> Result<(), Error>;
    /// One queued reply for a URL.
    fn respond(&mut self, url: &str, reply: Reply) -> Result<(), Error>;
    /// A subject whose key is already destroyed.
    fn erased(&mut self, subject: &str, id: &str) -> Result<(), Error>;

    /// One key row the `given` log minted, and the keys it is wrapped inside, nearest
    /// first. Called once per minted key, after the log and before any `erased`.
    ///
    /// A child's key is wrapped in its parent's, so erasing the parent takes it
    /// (`docs/effects.md` rule 12). A world with a real key store recorded that when it
    /// appended and needs nothing here; one that models the lifecycle alone cannot know
    /// it, because nothing in it ever mints a key. Defaulted to nothing, so a world that
    /// does not implement it is unaffected, and skipped entirely for a program whose
    /// subjects declare no parent.
    fn wrapping(&mut self, _key: (&str, &str), _under: &[(Ident, String)]) -> Result<(), Error> {
        Ok(())
    }

    /// One deployment credential, or `None` for a deployment that did not set it.
    ///
    /// Defaulted so that an out-of-repo world is not broken by rule 16 arriving: a world
    /// that does not implement it simply answers whatever its own [`Secrets`] does, and
    /// a test that writes the directive against one gets told so rather than silently
    /// getting the default.
    ///
    /// [`Secrets`]: crate::host::Secrets
    fn secret(&mut self, name: &str, _value: Option<&str>) -> Result<(), Error> {
        Err(Error::new(ErrorKind::Host(format!(
            "this world cannot set `{name}`: it does not implement `World::secret`"
        ))))
    }

    /// Setup is finished; hand over what the action runs against.
    fn open(self) -> Result<(Self::Host, Self::Rows), Error>;
}

/// Every test in the program, each against a fresh in-memory world holding only its own
/// `given` log. Declaration order decides the report's order and nothing else.
pub fn run_tests(program: &Program) -> Vec<TestResult> {
    run_tests_in(program, &mut || Ok(Sandbox::default()))
}

/// The same run, against a world the crate does not own.
///
/// A fresh world per test rather than one reused, because rule 9's "only its own `given`
/// log" is a property of the world and not of the interpreter.
pub fn run_tests_in<W: World>(
    program: &Program,
    fresh: &mut dyn FnMut() -> Result<W, Error>,
) -> Vec<TestResult> {
    program
        .tests
        .iter()
        .map(|test| TestResult {
            name: test.name.clone(),
            module: test.module.clone(),
            outcome: match fresh() {
                Ok(world) => run_test(program, test, world),
                // A world that cannot be built is not the program's fault, so it reads
                // as an error rather than as every test failing.
                Err(err) => TestOutcome::Errored(err.to_string()),
            },
        })
        .collect()
}

/// The text a key is filed under, which is the interpreter's answer rather than a
/// second one. `None` is a field holding nothing to file a key under, which the ancestry
/// rule makes impossible for an ancestor and possible for nothing else here.
fn id_text(value: &Value) -> Option<String> {
    subject_id(value, Span::default()).ok().flatten()
}

/// The key rows one `given` log mints, and what each is wrapped under.
///
/// A real key store keeps this: minting a customer's key wraps it inside its shop's, so
/// deleting the shop's row makes every key beneath it unwrappable in one delete
/// (`docs/effects.md` rule 12). Nothing here mints anything, so the same fact is
/// recovered from the log, using the rule that exists so a runtime could mint the key
/// at all: **every event that seals under a subject carries every ancestor's id**, in
/// plaintext, on the event itself.
///
/// **First mint wins**, which is the one place this could disagree with a store and
/// does not. Two events putting customer 88 in different shops are both well formed and
/// the key is minted once, under whichever arrived first; rule 12 says so in as many
/// words, and says that nothing can check it.
#[derive(Debug, Default)]
struct Minted {
    /// Each key, and the keys its own is wrapped inside. Ordered so a run reports the
    /// same way twice.
    under: BTreeMap<(Ident, String), Vec<(Ident, String)>>,
}

impl Minted {
    fn of(program: &Program, log: &[Event]) -> Self {
        let mut under: BTreeMap<(Ident, String), Vec<(Ident, String)>> = BTreeMap::new();
        for event in log {
            let Some(def) = program.event(&event.path) else {
                continue;
            };
            for field in &def.fields {
                // A **sealed** field is what mints a key. A subject-typed field that
                // seals nothing mints none, which is the trigger the ancestry rule has
                // where it is checked.
                let Some(id_field) = &field.subject else {
                    continue;
                };
                let Some(Type::Subject(sub)) = def.field(id_field).map(|def| &def.ty) else {
                    continue;
                };
                let Some(id) = event.fields.get(id_field).and_then(id_text) else {
                    continue;
                };
                let wrap = under.entry((sub.name.clone(), id)).or_default();
                if !wrap.is_empty() {
                    continue;
                }
                for ancestor in program.ancestors(&sub.name) {
                    // **Exactly one** field of the ancestor's type, or the chain stops
                    // here. Two would mean picking one, and nothing an author wrote says
                    // which: `@subject(...)` disambiguates its own subject by naming a
                    // sibling and an ancestor gets no such syntax, so choosing the first
                    // declared would make which shop a customer's key hangs from depend
                    // on field order. A runtime stops short for the same reason.
                    let mut carriers = def.fields.iter().filter(|field| {
                        field.subject.is_none()
                            && matches!(&field.ty, Type::Subject(sub) if sub.name == ancestor.name)
                    });
                    let (Some(carried), None) = (carriers.next(), carriers.next()) else {
                        break;
                    };
                    let Some(id) = event.fields.get(&carried.name).and_then(id_text) else {
                        break;
                    };
                    wrap.push((ancestor.name.clone(), id));
                }
            }
        }
        Self { under }
    }
}

fn run_test<W: World>(program: &Program, test: &Test, world: W) -> TestOutcome {
    match check(program, test, world) {
        Ok(None) => TestOutcome::Passed,
        Ok(Some(why)) => TestOutcome::Failed(why),
        Err(err) => TestOutcome::Errored(err),
    }
}

/// `Ok(None)` passed, `Ok(Some(why))` a mismatch, `Err` the run itself could not go on.
fn check<W: World>(program: &Program, test: &Test, mut world: W) -> Result<Option<String>, String> {
    let mut values = Values::new(program, test);

    // Kept as well as given, so the world can be told what the log minted. Only for a
    // program that has a subject with a parent, since that is the whole of what the
    // table decides and most programs have none.
    let wraps = program
        .subjects
        .iter()
        .any(|subject| subject.parent.is_some());
    let mut log: Vec<Event> = Vec::new();
    for given in &test.given {
        let def = program
            .event(&given.event)
            .ok_or_else(|| format!("event {} is not declared", given.event))?;
        let mut fields = BTreeMap::new();
        for (name, value) in &given.fields {
            // A declared field coerces here as it does at every other declared
            // position, so a bare `T` fills a `T?` event field. See `docs/optionals.md`.
            let ty = def.field(name).map(|field| field.ty.clone());
            fields.insert(name.clone(), values.at(*value, ty.as_ref())?);
        }
        let event = Event {
            path: given.event.clone(),
            fields,
        };
        if wraps {
            log.push(event.clone());
        }
        world.given(event).map_err(|err: Error| err.to_string())?;
    }
    if wraps {
        for ((subject, id), under) in &Minted::of(program, &log).under {
            world
                .wrapping((subject, id), under)
                .map_err(|err: Error| err.to_string())?;
        }
    }

    for setup in &test.setup {
        match setup {
            Setup::Respond { url, reply, .. } => {
                let url = values.text(*url)?;
                let reply = match reply {
                    ReplySpec::Status(status) => Reply::Status(*status),
                    ReplySpec::Body(status, body) => Reply::Body(*status, values.json(*body)?),
                    ReplySpec::Timeout => Reply::Transport("timeout".to_string()),
                };
                world
                    .respond(&url, reply)
                    .map_err(|err: Error| err.to_string())?;
            }
            Setup::Erased { subject, id, .. } => {
                let id = values.text(*id)?;
                // One key, the one the test named. What a parent's key row going takes
                // with it is the key store's answer rather than the runner's, which is
                // what keeps an effect's own `erase` answering the same way this does.
                world
                    .erased(subject, &id)
                    .map_err(|err: Error| err.to_string())?;
            }
            Setup::Secret { name, value, .. } => {
                // `none` is a deployment that did not set it, which is the only way to
                // reach the absent branch of an optional and the missing-credential
                // wedge: every other name answers with the harness's stand-in.
                let value = values.optional_text(*value)?;
                world
                    .secret(name, value.as_deref())
                    .map_err(|err: Error| err.to_string())?;
            }
        }
    }

    let (host, mut rows) = world.open().map_err(|err: Error| err.to_string())?;
    let mut interpreter = Interpreter::with_host(program, host);

    match &test.action {
        Action::Run { command, args, .. } => {
            let mut bound = Vec::new();
            for (name, value) in args {
                bound.push((name.clone(), values.eval(*value)?));
            }
            let execution = interpreter
                .run(command, bound)
                .map_err(|err: Error| err.to_string())?;
            check_run(&mut values, &test.expect, execution.outcome)
        }
        Action::Project { projector, .. } => {
            interpreter
                .project_into(projector, &mut rows)
                .map_err(|err: Error| err.to_string())?;
            check_project(&mut values, projector, &test.expect, &rows)
        }
        Action::Deliver { effect, .. } => {
            let counts = interpreter
                .drive(effect)
                .map_err(|err: Error| err.to_string())?;
            if let Some((position, err)) = counts.wedged {
                return Err(format!("{effect} wedged at position {position}: {err}"));
            }
            check_deliver(&mut values, &test.expect, interpreter.trace())
        }
    }
}

/// Rule 5: the appended events one for one and in order, or the outcome that replaced
/// them.
fn check_run(
    values: &mut Values<'_>,
    expect: &[Expect],
    outcome: Outcome,
) -> Result<Option<String>, String> {
    let events = match (&outcome, expect.first()) {
        (Outcome::Invalid(actual), Some(Expect::Invalid { message, .. })) => {
            let wanted = values.text(*message)?;
            return Ok((&wanted != actual)
                .then(|| format!("expected invalid {wanted:?}, got invalid {actual:?}")));
        }
        (
            Outcome::Reject { code, message },
            Some(Expect::Reject {
                code: wanted_code,
                message: wanted_message,
                ..
            }),
        ) => {
            let wanted_code = values.text(*wanted_code)?;
            let wanted_message = values.text(*wanted_message)?;
            return Ok((&wanted_code != code || &wanted_message != message).then(|| {
                format!(
                    "expected reject {wanted_code:?}, {wanted_message:?}, got reject {code:?}, {message:?}"
                )
            }));
        }
        (Outcome::Ok(events), _) => events,
        (Outcome::Invalid(message), _) => {
            return Ok(Some(format!("the command was invalid: {message}")));
        }
        (Outcome::Reject { code, message }, _) => {
            return Ok(Some(format!("the command rejected: {code}: {message}")));
        }
    };

    if matches!(expect.first(), Some(Expect::Nothing { .. })) {
        return Ok((!events.is_empty()).then(|| {
            format!(
                "expected nothing, got {}",
                listed(events.iter().map(|event| event.path.to_string()))
            )
        }));
    }

    if expect.len() != events.len() {
        return Ok(Some(format!(
            "expected {} event(s), got {}: {}",
            expect.len(),
            events.len(),
            listed(events.iter().map(|event| event.path.to_string()))
        )));
    }
    for (wanted, actual) in expect.iter().zip(events) {
        let Expect::Event { path, fields, .. } = wanted else {
            return Ok(Some(format!(
                "expected {}, got the event {}",
                describe(wanted),
                actual.path
            )));
        };
        if path != &actual.path {
            return Ok(Some(format!("expected {path}, got {}", actual.path)));
        }
        let def = values.program.event(path).map(|def| def.fields.clone());
        for (name, value) in fields {
            let ty = def.as_ref().and_then(|fields| {
                fields
                    .iter()
                    .find(|field| &field.name == name)
                    .map(|field| field.ty.clone())
            });
            let wanted = values.at(*value, ty.as_ref())?;
            match actual.fields.get(name) {
                Some(found) if found.same(&wanted) => {}
                Some(found) => {
                    // The stored content, not `<sealed under ...>`: the comparison above
                    // read through the seal, so a report that puts it back describes a
                    // question nobody asked. A test names what it put in and is owed
                    // what came out.
                    let found = shown(found);
                    return Ok(Some(format!(
                        "{path}.{name}: expected {wanted}, got {found}"
                    )));
                }
                None => return Ok(Some(format!("{path} has no `{name}`"))),
            }
        }
    }
    Ok(None)
}

/// Rule 6: the listed columns of each named row, or its absence.
fn check_project(
    values: &mut Values<'_>,
    projector: &str,
    expect: &[Expect],
    rows: &dyn Rows,
) -> Result<Option<String>, String> {
    for wanted in expect {
        match wanted {
            Expect::Row {
                entity,
                key,
                fields,
                ..
            } => {
                let key = key_of(values.eval(*key)?)?;
                let found = rows
                    .row(entity, &key)
                    .map_err(|err: Error| err.to_string())?;
                let Some(row) = found else {
                    return Ok(Some(format!("{entity}[{}] is absent", show(&key))));
                };
                if let Some(why) = mismatch(values, projector, entity, &key, &row, fields)? {
                    return Ok(Some(why));
                }
            }
            Expect::NoRow { entity, key, .. } => {
                let key = key_of(values.eval(*key)?)?;
                let found = rows
                    .row(entity, &key)
                    .map_err(|err: Error| err.to_string())?;
                if found.is_some() {
                    return Ok(Some(format!("{entity}[{}] is present", show(&key))));
                }
            }
            other => {
                return Ok(Some(format!(
                    "a projector produces rows, not {}",
                    describe(other)
                )));
            }
        }
    }
    Ok(None)
}

fn mismatch(
    values: &mut Values<'_>,
    projector: &str,
    entity: &str,
    key: &Key,
    row: &Row,
    fields: &[(Ident, ExprId)],
) -> Result<Option<String>, String> {
    let def = values
        .program
        .projector(projector)
        .and_then(|projector| projector.entity(entity))
        .map(|entity| entity.fields.clone());
    for (name, value) in fields {
        let ty = def.as_ref().and_then(|fields| {
            fields
                .iter()
                .find(|field| &field.name == name)
                .map(|field| field.ty.clone())
        });
        let wanted = values.at(*value, ty.as_ref())?;
        match row.field(name) {
            Some(found) if found.same(&wanted) => {}
            Some(found) => {
                let found = shown(found);
                return Ok(Some(format!(
                    "{entity}[{}].{name}: expected {wanted}, got {found}",
                    show(key)
                )));
            }
            None => return Ok(Some(format!("{entity} has no `{name}`"))),
        }
    }
    Ok(None)
}

/// Rule 7: the trace, one for one and in order.
fn check_deliver(
    values: &mut Values<'_>,
    expect: &[Expect],
    trace: &[Effectful],
) -> Result<Option<String>, String> {
    if matches!(expect.first(), Some(Expect::Nothing { .. })) {
        return Ok((!trace.is_empty())
            .then(|| format!("expected nothing, got {}", listed(trace.iter().map(render)))));
    }
    if expect.len() != trace.len() {
        return Ok(Some(format!(
            "expected {} effect(s), got {}: {}",
            expect.len(),
            trace.len(),
            listed(trace.iter().map(render))
        )));
    }
    for (wanted, actual) in expect.iter().zip(trace) {
        if let Some(why) = one_effect(values, wanted, actual)? {
            return Ok(Some(why));
        }
    }
    Ok(None)
}

fn one_effect(
    values: &mut Values<'_>,
    wanted: &Expect,
    actual: &Effectful,
) -> Result<Option<String>, String> {
    let no = |why: String| Ok(Some(why));
    match (wanted, actual) {
        (
            Expect::Http {
                verb, url, body, ..
            },
            Effectful::Http {
                verb: sent,
                url: to,
                body: carried,
            },
        ) => {
            if &verb.name() != sent {
                return no(format!("expected {}, got {sent}", verb.name()));
            }
            let url = values.text(*url)?;
            if &url != to {
                return no(format!("expected {sent} {url}, got {sent} {to}"));
            }
            // Rule 7: the keys the test wrote, because a body is often a large
            // generated document and the assertion is what it carried.
            if let Some(body) = body {
                let wanted = values.json(*body)?;
                let Some(carried) = carried else {
                    return no(format!("{sent} {to} carried no body"));
                };
                if let Some(why) = json_covers(&wanted, carried, "body") {
                    return no(why);
                }
            }
            Ok(None)
        }
        (
            Expect::Invoke { command, args, .. },
            Effectful::Invoke {
                command: called,
                args: passed,
            },
        ) => {
            if command != called {
                return no(format!("expected invoke {command}, got invoke {called}"));
            }
            // Both sides against the declared parameters: the trace holds what the arm
            // evaluated, which `bind_params` would coerce on the way in, so comparing
            // them raw would make an optional parameter a false mismatch.
            let params = values
                .program
                .command(command)
                .map(|def| def.params.clone())
                .unwrap_or_default();
            let ty = |name: &Ident| {
                params
                    .iter()
                    .find(|param| &param.name == name)
                    .map(|param| param.ty.clone())
            };
            let mut wanted = BTreeMap::new();
            for (name, value) in args {
                wanted.insert(name.clone(), values.at(*value, ty(name).as_ref())?);
            }
            let passed: BTreeMap<Ident, Value> = passed
                .iter()
                .map(|(name, value)| match ty(name) {
                    Some(ty) => (name.clone(), coerce(value.clone(), &ty)),
                    None => (name.clone(), value.clone()),
                })
                .collect();
            if wanted != passed {
                return no(format!(
                    "invoke {command}: expected {}, got {}",
                    args_of(&wanted),
                    args_of(&passed)
                ));
            }
            Ok(None)
        }
        (
            Expect::Erase { subject, id, .. },
            Effectful::Erase {
                subject: erased,
                id: which,
            },
        ) => {
            let id = values.text(*id)?;
            if subject != erased || &id != which {
                return no(format!(
                    "expected erase {subject} {id:?}, got erase {erased} {which:?}"
                ));
            }
            Ok(None)
        }
        (Expect::Log { message, .. }, Effectful::Log(line)) => {
            let wanted = values.text(*message)?;
            if &wanted != line {
                return no(format!("expected log {wanted:?}, got log {line:?}"));
            }
            Ok(None)
        }
        (Expect::Failed { message, .. }, Effectful::Failed(why)) => {
            let wanted = values.text(*message)?;
            if &wanted != why {
                return no(format!("expected failed {wanted:?}, got failed {why:?}"));
            }
            Ok(None)
        }
        (Expect::Skipped { .. }, Effectful::Skipped(_)) => Ok(None),
        (wanted, actual) => no(format!(
            "expected {}, got {}",
            describe(wanted),
            render(actual)
        )),
    }
}

/// Every key the test wrote is present and equal. Rule 7's partial body match, applied
/// recursively so a nested object is as partial as the top-level one.
fn json_covers(wanted: &Json, actual: &Json, at: &str) -> Option<String> {
    match (wanted, actual) {
        (Json::Obj(wanted), Json::Obj(actual)) => {
            for (key, value) in wanted {
                let found = actual.get(key)?;
                if let Some(why) = json_covers(value, found, &format!("{at}.{key}")) {
                    return Some(why);
                }
            }
            None
        }
        (wanted, actual) if wanted == actual => None,
        (wanted, actual) => Some(format!("{at}: expected {wanted}, got {actual}")),
    }
}

/// A test's expression arena, evaluated against an empty frame. Values only, so this
/// never needs a sink and cannot reach the world.
struct Values<'a> {
    program: &'a Program,
    exprs: &'a Exprs,
    frame: usize,
}

impl<'a> Values<'a> {
    fn new(program: &'a Program, test: &'a Test) -> Self {
        Self {
            program,
            exprs: &test.exprs,
            frame: test.frame,
        }
    }

    fn eval(&mut self, id: ExprId) -> Result<Value, String> {
        crate::interp::eval_pure(self.program, self.exprs, self.frame, id)
            .map_err(|err| err.to_string())
    }

    /// A value against the declared type it will be compared to, so a test states
    /// `tracking: "TRK-1"` for a `String?` column the way every other declared position
    /// lets it. Without this the report reads `expected "TRK-1", got "TRK-1"`.
    fn at(&mut self, id: ExprId, ty: Option<&Type>) -> Result<Value, String> {
        let value = self.eval(id)?;
        Ok(match ty {
            Some(ty) => coerce(value, ty),
            None => value,
        })
    }

    fn text(&mut self, id: ExprId) -> Result<String, String> {
        Ok(crate::value::text(&self.eval(id)?))
    }

    fn json(&mut self, id: ExprId) -> Result<Json, String> {
        Ok(Json::from_value(&self.eval(id)?))
    }

    /// The same as [`Self::text`] where the position takes a `String?`, so `none` stays
    /// absent instead of becoming the text `"null"`.
    fn optional_text(&mut self, id: ExprId) -> Result<Option<String>, String> {
        Ok(match self.eval(id)? {
            Value::Opt { value: None, .. } => None,
            held => Some(crate::value::text(&held)),
        })
    }
}

fn key_of(value: Value) -> Result<Key, String> {
    Key::from_value(&value).ok_or_else(|| format!("`{value}` cannot be an entity key"))
}

fn show(key: &Key) -> String {
    crate::value::text(&crate::interp::key_as_value(key))
}

/// What a mismatch shows for a value that may be sealed.
///
/// `Display` prints `<sealed under ...>` and never the content, which is right
/// everywhere a program can reach and useless here: a test names what it put in and is
/// owed what came out. The stored form is the only thing heklang has without a key, and
/// under the harness that is exactly what the test wrote (`docs/host.md`).
fn shown(value: &Value) -> String {
    match value {
        Value::Sealed { content, .. } => format!("{content:?}"),
        // Rule 16 keeps a secret out of every event field and every column, which is
        // all this renders, so nothing reaches here. Left as a panic rather than given
        // a plausible rendering: a wrong answer would be invisible, and the two
        // candidates (the value a test supplied, or its redaction) differ.
        Value::Secret { .. } => unreachable!("rule 16 keeps a secret out of a row and an event"),
        Value::Opt {
            value: Some(held), ..
        } => shown(held),
        other => other.to_string(),
    }
}

fn args_of(args: &BTreeMap<Ident, Value>) -> String {
    listed(args.iter().map(|(name, value)| format!("{name}: {value}")))
}

fn listed(items: impl Iterator<Item = String>) -> String {
    let items: Vec<String> = items.collect();
    if items.is_empty() {
        return "nothing".to_string();
    }
    items.join(", ")
}

fn render(effect: &Effectful) -> String {
    match effect {
        Effectful::Http { verb, url, .. } => format!("{verb} {url}"),
        Effectful::Invoke { command, .. } => format!("invoke {command}"),
        Effectful::Erase { subject, id } => format!("erase {subject} {id:?}"),
        Effectful::Log(line) => format!("log {line:?}"),
        Effectful::Failed(why) => format!("failed {why:?}"),
        Effectful::Skipped(_) => "skipped".to_string(),
    }
}

fn describe(expect: &Expect) -> String {
    match expect {
        Expect::Event { path, .. } => path.to_string(),
        Expect::Nothing { .. } => "nothing".to_string(),
        Expect::Invalid { .. } => "invalid".to_string(),
        Expect::Reject { .. } => "reject".to_string(),
        Expect::Row { entity, .. } => format!("a `{entity}` row"),
        Expect::NoRow { entity, .. } => format!("no `{entity}` row"),
        Expect::Http { verb, .. } => verb.name().to_string(),
        Expect::Invoke { command, .. } => format!("invoke {command}"),
        Expect::Erase { subject, .. } => format!("erase {subject}"),
        Expect::Log { .. } => "log".to_string(),
        Expect::Failed { .. } => "failed".to_string(),
        Expect::Skipped { .. } => "skipped".to_string(),
    }
}
