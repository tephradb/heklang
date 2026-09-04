//! The digest form: what a program does, with everything else taken away.
//!
//! Two versions of a program that behave the same produce the same bytes here, so
//! hashing this answers "did it meaningfully change?" and expanding it answers "where?".
//! `hek fmt` cannot be asked either question: it normalises layout and says so
//! (`docs/fmt.md` rule 1), and comparing two `Program`s is a layout test rather than a
//! meaning test, because a span moves when a line does.
//!
//! **The digest form is what runs**, and that one rule decides everything else. It also
//! happens to be the form the IR is already in: `src/parse.rs` lowers straight to IR with
//! no AST and no separate desugaring pass, so a local's name, a written decimal place, a
//! `const`'s name, a `refusal`'s name and a `guard`'s whole body are gone before a
//! `Program` exists.
//!
//! **What is hashed is the packed form, not a rendering.** [`Sexp::expanded`] and
//! [`Sexp::json`] are views taken from it and nothing hashes either, so both are free to
//! read better tomorrow without moving a stored hash. That split is what lets a caller
//! keep the packed line in a database, read it back with no source tree in reach, and
//! still get a diff out of it.
//!
//! `docs/digest.md` is the contract and `tests/digest.rs` is the same rules as executable
//! tests. Change the doc, the tests and the code together.

pub mod sexp;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;

use sha2::{Digest as _, Sha256};

pub use sexp::{Sexp, SexpError};

use crate::ir::{
    Absent, Action, Arm, Bind, Builtin, Command, Effect, EntityDef, EntityField, EnumDef, EnvBind,
    EnvField, EventDef, Expect, Expr, ExprId, Exprs, FieldDef, Function, Handler, Ident, Iter,
    Literal, Program, Projector, RecordDef, RecordField, ReplySpec, Return, Setup, Slice, Slot,
    Stage, Stmt, Test, Type, UnOp,
};
use crate::parse::children;
use crate::value::Json;

/// The first line of every packed form, and part of what is hashed.
///
/// The digest is the meaning of a program *as this version of heklang reads it*, so a
/// change to how the parser desugars, or to the packed form's own spelling, is a change to
/// what a hash means. Bumping this moves every hash at once, which is the point: a global
/// change of hash then has one legible cause instead of looking like every declaration was
/// edited on the same day.
pub const VERSION: &str = "hek-digest 2";

/// A SHA-256 over a packed form. Rendered as sixty-four lowercase hex digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Hash([u8; 32]);

impl Hash {
    fn of(text: &str) -> Self {
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(Sha256::digest(text.as_bytes()).as_slice());
        Hash(bytes)
    }

    /// What a line of the packed form hashes to: the version it was written under, then
    /// the line. Used for one entry and for one signature.
    fn line(sexp: &Sexp) -> Self {
        Hash::of(&format!("{VERSION}\n{}\n", sexp.packed()))
    }

    pub fn bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for Hash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// Which declaration an entry came from, and the head its packed line opens with. The
/// order of the variants is the order entries sort into, so the shape of the whole form is
/// fixed before any name is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Event,
    Enum,
    Record,
    Function,
    Command,
    Projector,
    Effect,
    Test,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Event => "event",
            Kind::Enum => "enum",
            Kind::Record => "record",
            Kind::Function => "function",
            Kind::Command => "command",
            Kind::Projector => "projector",
            Kind::Effect => "effect",
            Kind::Test => "test",
        }
    }

    pub fn lookup(head: &str) -> Option<Self> {
        Some(match head {
            "event" => Kind::Event,
            "enum" => Kind::Enum,
            "record" => Kind::Record,
            "function" => Kind::Function,
            "command" => Kind::Command,
            "projector" => Kind::Projector,
            "effect" => Kind::Effect,
            "test" => Kind::Test,
            _ => return None,
        })
    }
}

/// One declaration's packed form, its signature, and a hash of each.
///
/// The per-entry hash is what makes "which declarations changed?" a comparison of two
/// lists rather than a diff. The signature hash narrows that again: it moves only when
/// something outside the program could notice, so a caller deciding whether a deployment
/// is compatible reads it and never has to decode a body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub kind: Kind,
    /// The declared name, or the path for an event. What a caller outside the program
    /// knows this declaration by, and the join key for two versions of one program.
    pub name: String,
    pub form: Sexp,
    pub hash: Hash,
    /// What is visible outside the program. `None` for a `fn` and a `test`, which nothing
    /// outside can name. See `docs/digest.md` rule 8.
    pub signature: Option<Sexp>,
    pub signature_hash: Option<Hash>,
}

impl Entry {
    fn new(kind: Kind, name: String, form: Sexp, signature: Option<Sexp>) -> Self {
        let signature_hash = signature.as_ref().map(Hash::line);
        Entry {
            kind,
            name,
            hash: Hash::line(&form),
            form,
            signature,
            signature_hash,
        }
    }

    /// Reads one stored row back. The signature is passed rather than re-derived so that
    /// what was stored is what comes back, even across a version where derivation changed.
    pub fn from_packed(form: &str, signature: Option<&str>) -> Result<Self, SexpError> {
        let form = Sexp::parse(form)?;
        let signature = signature.map(Sexp::parse).transpose()?;
        let (kind, name) = head_of(&form)?;
        Ok(Entry {
            kind,
            name,
            hash: Hash::line(&form),
            form,
            signature_hash: signature.as_ref().map(Hash::line),
            signature,
        })
    }
}

fn head_of(form: &Sexp) -> Result<(Kind, String), SexpError> {
    let bad = |message: &str| SexpError {
        at: 0,
        message: message.to_string(),
    };
    let head = form.head().ok_or_else(|| bad("a declaration"))?;
    let kind = Kind::lookup(head).ok_or_else(|| bad("a declaration, not `{head}`"))?;
    let name = match form.rest().first() {
        Some(Sexp::Atom(name)) | Some(Sexp::Str(name)) => name.clone(),
        _ => return Err(bad("a declaration's name")),
    };
    Ok((kind, name))
}

/// A whole program's digest form.
///
/// Tests are kept apart from the program because they are two different questions. A
/// `test` declaration runs nothing in production, so a change confined to one should not
/// move the hash a deploy gate reads; but a change confined to one is still a change, so
/// [`Digest::hash_with_tests`] answers that too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Digest {
    entries: Vec<Entry>,
    tests: Vec<Entry>,
}

impl Digest {
    pub fn of(program: &Program) -> Self {
        let mut forms: Vec<(Kind, String, Sexp)> = Vec::new();

        for def in &program.events {
            forms.push((Kind::Event, def.path.to_string(), event(def)));
        }
        for def in &program.enums {
            forms.push((Kind::Enum, def.name.clone(), enumeration(def)));
        }
        for def in &program.records {
            forms.push((Kind::Record, def.name.clone(), record(def)));
        }
        for def in &program.functions {
            forms.push((Kind::Function, def.name.clone(), function(def)));
        }
        for def in &program.commands {
            forms.push((Kind::Command, def.name.clone(), command(def)));
        }
        for def in &program.projectors {
            forms.push((Kind::Projector, def.name.clone(), projector(def)));
        }
        for def in &program.effects {
            forms.push((Kind::Effect, def.name.clone(), effect(def)));
        }

        let tests: Vec<(Kind, String, Sexp)> = program
            .tests
            .iter()
            .map(|def| (Kind::Test, def.name.clone(), test(def)))
            .collect();

        // `const`, `refusal` and `guard` are absent on purpose. The parser inlines all
        // three (`parse.rs:4853`, `parse.rs:6719`, `inline::splice`), so a declaration of
        // any of them runs nothing and its content is already at every use site.
        Digest::assemble(forms, tests)
    }

    /// Reads a whole packed form back: the version line, then one line per declaration.
    /// Signatures are derived rather than stored here, which is exactly what [`Digest::of`]
    /// does, so a round trip is the same object.
    pub fn from_packed(text: &str) -> Result<Self, SexpError> {
        let mut entries = Vec::new();
        let mut tests = Vec::new();
        for (number, line) in text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            if number == 0 && line == VERSION {
                continue;
            }
            let form = Sexp::parse(line)?;
            let (kind, name) = head_of(&form)?;
            if kind == Kind::Test {
                tests.push((kind, name, form));
            } else {
                entries.push((kind, name, form));
            }
        }
        Ok(Digest::assemble(entries, tests))
    }

    /// Derives every signature, then sorts. One pass rather than two because a command's
    /// signature reaches through the `fn`s it calls, so no signature can be built until
    /// every form exists.
    fn assemble(entries: Vec<(Kind, String, Sexp)>, tests: Vec<(Kind, String, Sexp)>) -> Self {
        let mut helpers: HashMap<String, Sexp> = HashMap::new();
        for (kind, name, form) in &entries {
            match kind {
                Kind::Function => {
                    helpers.insert(name.clone(), form.clone());
                }
                // An effect's helpers are declared inside it and named `Effect.helper` at
                // every call site, which is the key they are filed under here.
                Kind::Effect => {
                    for nested in form.section("function") {
                        if let Some(Sexp::Atom(helper)) = nested.rest().first() {
                            helpers.insert(format!("{name}.{helper}"), nested.clone());
                        }
                    }
                }
                _ => {}
            }
        }

        let build = |rows: Vec<(Kind, String, Sexp)>| -> Vec<Entry> {
            let mut out: Vec<Entry> = rows
                .into_iter()
                .map(|(kind, name, form)| {
                    let signature = signature(kind, &form, &helpers);
                    Entry::new(kind, name, form, signature)
                })
                .collect();
            out.sort_by(|one, two| {
                (one.kind, &one.name, one.form.packed()).cmp(&(
                    two.kind,
                    &two.name,
                    two.form.packed(),
                ))
            });
            out
        };

        Digest {
            entries: build(entries),
            tests: build(tests),
        }
    }

    /// The program's declarations, sorted, tests excluded.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn tests(&self) -> &[Entry] {
        &self.tests
    }

    /// The canonical bytes: the version line, then one line per declaration. This is
    /// exactly what [`Digest::hash`] covers, so piping it through any SHA-256 agrees.
    pub fn packed(&self) -> String {
        packed(VERSION, self.entries.iter())
    }

    pub fn packed_with_tests(&self) -> String {
        packed(VERSION, self.entries.iter().chain(self.tests.iter()))
    }

    /// The readable view. Nothing hashes it, so it may change whenever it reads better.
    pub fn expanded(&self) -> String {
        expanded(self.entries.iter())
    }

    pub fn expanded_with_tests(&self) -> String {
        expanded(self.entries.iter().chain(self.tests.iter()))
    }

    pub fn hash(&self) -> Hash {
        Hash::of(&self.packed())
    }

    pub fn hash_with_tests(&self) -> Hash {
        Hash::of(&self.packed_with_tests())
    }

    /// The structural view, for a caller that would rather not walk a list. `Json::Obj` is
    /// a `BTreeMap`, so the keys come out sorted and one digest serialises byte for byte
    /// the same every time; nothing here needs serde.
    ///
    /// The program without its tests, like [`Digest::packed`] and [`Digest::hash`]: every
    /// hash in the document covers content the document carries.
    pub fn json(&self) -> Json {
        Json::Obj(BTreeMap::from([
            ("version".to_string(), Json::str(VERSION)),
            ("hash".to_string(), Json::str(self.hash().to_string())),
            ("entries".to_string(), listed(&self.entries)),
        ]))
    }

    /// Everything, with both hashes: `hash` is still the program alone and
    /// `hash_with_tests` is what the two lists together come to.
    pub fn json_with_tests(&self) -> Json {
        Json::Obj(BTreeMap::from([
            ("version".to_string(), Json::str(VERSION)),
            ("hash".to_string(), Json::str(self.hash().to_string())),
            (
                "hash_with_tests".to_string(),
                Json::str(self.hash_with_tests().to_string()),
            ),
            ("entries".to_string(), listed(&self.entries)),
            ("tests".to_string(), listed(&self.tests)),
        ]))
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.expanded())
    }
}

fn packed<'a>(version: &str, entries: impl Iterator<Item = &'a Entry>) -> String {
    let mut text = String::from(version);
    for entry in entries {
        text.push('\n');
        text.push_str(&entry.form.packed());
    }
    text.push('\n');
    text
}

fn expanded<'a>(entries: impl Iterator<Item = &'a Entry>) -> String {
    let mut text = String::from(VERSION);
    for entry in entries {
        text.push('\n');
        text.push_str(&entry.form.expanded());
    }
    text.push('\n');
    text
}

fn listed(entries: &[Entry]) -> Json {
    Json::Arr(
        entries
            .iter()
            .map(|entry| {
                let mut fields = BTreeMap::from([
                    ("kind".to_string(), Json::str(entry.kind.name())),
                    ("name".to_string(), Json::str(entry.name.clone())),
                    ("hash".to_string(), Json::str(entry.hash.to_string())),
                    ("packed".to_string(), Json::str(entry.form.packed())),
                    ("form".to_string(), entry.form.json()),
                ]);
                if let (Some(signature), Some(hash)) = (&entry.signature, entry.signature_hash) {
                    fields.insert("signature".to_string(), signature.json());
                    fields.insert(
                        "signature_packed".to_string(),
                        Json::str(signature.packed()),
                    );
                    fields.insert("signature_hash".to_string(), Json::str(hash.to_string()));
                }
                Json::Obj(fields)
            })
            .collect(),
    )
}

fn node(head: &str, rest: impl IntoIterator<Item = Sexp>) -> Sexp {
    Sexp::of(head, rest)
}

fn atom(text: impl Into<String>) -> Sexp {
    Sexp::atom(text)
}

// ---------------------------------------------------------------------------
// Signatures
// ---------------------------------------------------------------------------

/// What is visible outside the program, taken from the packed form rather than from the
/// IR. One implementation then serves both [`Digest::of`] and [`Digest::from_packed`], so
/// a stored row and a freshly parsed program cannot disagree about what a signature is.
fn signature(kind: Kind, form: &Sexp, helpers: &HashMap<String, Sexp>) -> Option<Sexp> {
    let name = form.rest().first()?.clone();
    let mut parts = vec![atom(kind.name()), name];
    match kind {
        // An event, an enum and a record are all shape and no body, so the signature is
        // the declaration. Uniform rather than absent: a caller compares signature to
        // signature without a special case per kind.
        Kind::Event | Kind::Enum | Kind::Record => {
            parts.extend(form.rest().iter().skip(1).cloned());
        }
        Kind::Command => {
            parts.extend(form.section("params").cloned());
            let mut codes: Vec<String> = Vec::new();
            let mut seen = HashSet::new();
            rejects(form, helpers, &mut seen, &mut codes);
            codes.sort();
            codes.dedup();
            parts.push(node("rejects", codes.into_iter().map(atom)));
        }
        // The read model is the API. What a handler does to get there is not.
        Kind::Projector => parts.extend(form.section("entity").cloned()),
        // Which events an effect consumes is what a deployment has to know; the calls it
        // makes are its own business.
        Kind::Effect => {
            let mut events: Vec<String> = form
                .section("on")
                .flat_map(|arm| arm.section("events"))
                .flat_map(|events| events.rest())
                .map(Sexp::packed)
                .collect();
            events.sort();
            events.dedup();
            parts.push(node("events", events.into_iter().map(atom)));
        }
        // Nothing outside the program can name either.
        Kind::Function | Kind::Test => return None,
    }
    Some(node("sig", parts))
}

/// The refusal codes a command can answer with, reaching through the `fn`s it calls.
///
/// A refusal has no entry of its own, because the parser inlines it, so a code is only
/// findable inside a body. It is still the one declared name whose spelling leaves the
/// program and the thing a client switches on, which is why the signature goes and gets
/// it. `seen` guards a cycle that `docs/functions.md` already rejects statically.
fn rejects(
    form: &Sexp,
    helpers: &HashMap<String, Sexp>,
    seen: &mut HashSet<String>,
    out: &mut Vec<String>,
) {
    let Sexp::List(items) = form else {
        return;
    };
    match (form.head(), form.rest().first()) {
        (Some("reject"), Some(Sexp::List(code))) => {
            if let [Sexp::Atom(head), Sexp::Str(code)] = code.as_slice()
                && head == "str"
            {
                out.push(code.clone());
            }
        }
        (Some("fn"), Some(Sexp::Atom(name))) => {
            if seen.insert(name.clone())
                && let Some(body) = helpers.get(name)
            {
                rejects(body, helpers, seen, out);
            }
        }
        _ => {}
    }
    for item in items {
        rejects(item, helpers, seen, out);
    }
}

// ---------------------------------------------------------------------------
// Declarations
// ---------------------------------------------------------------------------

fn event(def: &EventDef) -> Sexp {
    let mut fields: Vec<&FieldDef> = def.fields.iter().collect();
    fields.sort_by(|one, two| one.name.cmp(&two.name));
    let mut parts = vec![atom(def.path.to_string())];
    for field in fields {
        // The type carries `@subject(x)` already: a subject-bound field *is* a `Sealed`,
        // so printing `FieldDef::subject` beside it would say it twice.
        let mut part = vec![atom(field.name.clone()), ty(&field.ty)];
        if let Some(max) = field.max_len {
            part.push(node("max", [atom(max.to_string())]));
        }
        if !field.indexed {
            part.push(atom("no_index"));
        }
        parts.push(node("f", part));
    }
    node("event", parts)
}

fn enumeration(def: &EnumDef) -> Sexp {
    let mut variants: Vec<&Ident> = def.variants.iter().collect();
    variants.sort();
    let mut parts = vec![
        atom(def.name.clone()),
        node("variants", variants.into_iter().map(|v| atom(v.clone()))),
    ];
    // By name, not by index: `EnumDef::default` points into the list this just sorted.
    if let Some(variant) = def.default_variant() {
        parts.push(node("default", [atom(variant.clone())]));
    }
    node("enum", parts)
}

fn record(def: &RecordDef) -> Sexp {
    let mut fields: Vec<&RecordField> = def.fields.iter().collect();
    fields.sort_by(|one, two| one.name.cmp(&two.name));
    let mut parts = vec![atom(def.name.clone())];
    for field in fields {
        let mut part = vec![atom(field.name.clone()), ty(&field.ty)];
        if let Some(max) = field.max_len {
            part.push(node("max", [atom(max.to_string())]));
        }
        parts.push(node("f", part));
    }
    node("record", parts)
}

fn function(def: &Function) -> Sexp {
    let mut frame = Frame::new(&def.exprs);
    // A `fn`'s arguments are positional (`Expr::CallFn` holds a bare `Vec<ExprId>`), so a
    // parameter's name is as local as a `let`'s and only its type is worth keeping.
    let params: Vec<Sexp> = def
        .params
        .iter()
        .map(|param| {
            frame.slot(param.slot);
            ty(&param.ty)
        })
        .collect();
    let mut parts = vec![atom(def.name.clone()), node("params", params)];
    if let Some(ret) = &def.ret {
        parts.push(node("returns", [ty(ret)]));
    }
    parts.push(node("body", frame.body(&def.body)));
    node("function", parts)
}

fn command(def: &Command) -> Sexp {
    let mut frame = Frame::new(&def.exprs);
    // A command's parameter names do leave the program: they are the request body's keys
    // and an `invoke`'s argument names. So the name stays and the slot is implied by
    // position, which is where the numbering starts.
    let params: Vec<Sexp> = def
        .params
        .iter()
        .map(|param| node("p", [atom(param.name.clone()), ty(&param.ty)]))
        .collect();
    for param in &def.params {
        frame.slot(param.slot);
    }
    let mut parts = vec![atom(def.name.clone()), node("params", params)];
    parts.extend(frame.now(def.now));
    parts.extend(def.stages.iter().map(|stage| frame.stage(stage)));
    node("command", parts)
}

fn projector(def: &Projector) -> Sexp {
    let mut parts = vec![atom(def.name.clone())];

    let mut enums: Vec<&EnumDef> = def.enums.iter().collect();
    enums.sort_by(|one, two| one.name.cmp(&two.name));
    parts.extend(enums.into_iter().map(enumeration));

    let mut entities: Vec<&EntityDef> = def.entities.iter().collect();
    entities.sort_by(|one, two| one.name.cmp(&two.name));
    parts.extend(entities.into_iter().map(entity));

    // A handler owns its frame and its arena, so nothing carries between two of them and
    // sorting them cannot move a slot. Which handler runs is decided by the event.
    let mut handlers: Vec<&Handler> = def.handlers.iter().collect();
    handlers.sort_by_key(|handler| handler.event.to_string());
    for handler in handlers {
        let mut frame = Frame::new(&handler.exprs);
        let mut arm = vec![atom(handler.event.to_string())];
        arm.extend(frame.trigger(&handler.binds, &handler.envelope));
        arm.push(node("body", frame.body(&handler.body)));
        parts.push(node("on", arm));
    }

    node("projector", parts)
}

fn entity(def: &EntityDef) -> Sexp {
    let key = def.key_field().name.clone();
    let mut parts = vec![atom(def.name.clone())];

    let mut fields: Vec<&EntityField> = def.fields.iter().collect();
    fields.sort_by(|one, two| one.name.cmp(&two.name));
    for field in fields {
        let mut part = vec![atom(field.name.clone()), ty(&field.ty)];
        if let Some(max) = field.max_len {
            part.push(node("max", [atom(max.to_string())]));
        }
        if let Some(default) = &field.default {
            part.push(node("default", [literal(default)]));
        }
        parts.push(node("col", part));
    }
    // By name, for the reason an enum's default is: `EntityDef::key` is an index into the
    // list that was just sorted.
    parts.push(node("key", [atom(key)]));

    // `@index` on a column and an `index (a, b)` clause build the same `Index` at
    // different positions in this list, so sorting is what makes the two spellings agree.
    // Each index keeps its own field order: a composite index is ordered.
    let mut indexes: Vec<Sexp> = def
        .indexes
        .iter()
        .map(|index| node("index", index.fields.iter().map(|f| atom(f.clone()))))
        .collect();
    indexes.sort_by_key(Sexp::packed);
    parts.extend(indexes);

    node("entity", parts)
}

fn effect(def: &Effect) -> Sexp {
    let mut parts = vec![atom(def.name.clone())];

    let mut functions: Vec<&Function> = def.functions.iter().collect();
    functions.sort_by(|one, two| one.name.cmp(&two.name));
    parts.extend(functions.into_iter().map(function));

    // Rule 1 of `docs/effects.md` makes an event select at most one arm, so the arms are a
    // lookup table rather than a sequence, and each owns its frame and arena.
    let mut arms: Vec<&Arm> = def.arms.iter().collect();
    arms.sort_by_key(|arm| paths(arm).packed());
    for arm in arms {
        let mut frame = Frame::new(&arm.exprs);
        let mut on = vec![paths(arm)];
        on.extend(frame.trigger(&arm.binds, &arm.envelope));
        on.extend(frame.now(arm.now));
        on.extend(arm.stages.iter().map(|stage| frame.stage(stage)));
        parts.push(node("on", on));
    }

    node("effect", parts)
}

/// The events one arm answers, sorted: several paths may share an arm, and which one is
/// written first says nothing.
fn paths(arm: &Arm) -> Sexp {
    let mut paths: Vec<String> = arm.events.iter().map(|path| path.to_string()).collect();
    paths.sort();
    node("events", paths.into_iter().map(atom))
}

fn test(def: &Test) -> Sexp {
    let mut frame = Frame::new(&def.exprs);
    let mut parts = vec![Sexp::text(def.name.clone())];

    // Order is kept through all three sections: a `given` seeds the log in order, and an
    // `expect` list is matched against events in the order they were appended.
    for given in &def.given {
        let mut part = vec![atom(given.event.to_string())];
        part.extend(frame.keyed(&given.fields));
        parts.push(node("given", part));
    }
    for setup in &def.setup {
        parts.push(match setup {
            Setup::Respond { url, reply, .. } => {
                let url = frame.expr(*url);
                let reply = match reply {
                    ReplySpec::Status(status) => node("status", [atom(status.to_string())]),
                    ReplySpec::Body(status, body) => {
                        let body = frame.expr(*body);
                        node("status", [atom(status.to_string()), body])
                    }
                    ReplySpec::Timeout => atom("timeout"),
                };
                node("respond", [url, reply])
            }
            Setup::Erased { subject, id, .. } => {
                let id = frame.expr(*id);
                node("erased", [atom(subject.clone()), id])
            }
        });
    }
    parts.push(match &def.action {
        Action::Run { command, args, .. } => {
            let mut part = vec![atom(command.clone())];
            part.extend(frame.keyed(args));
            node("run", part)
        }
        Action::Project { projector, .. } => node("project", [atom(projector.clone())]),
        Action::Deliver { effect, .. } => node("deliver", [atom(effect.clone())]),
    });
    for expect in &def.expect {
        parts.push(node("expect", [frame.expect(expect)]));
    }

    node("test", parts)
}

// ---------------------------------------------------------------------------
// Bodies
// ---------------------------------------------------------------------------

/// One declaration's arena, and the slot numbers it has handed out.
///
/// A `Slot` in the IR is a position in a frame, and a frame's layout is the parser's
/// business: a spliced guard's slots sit at the end of its caller's, and where `now()`
/// lands has moved before. So a slot is renumbered by **first appearance in the packed
/// form**, which is stable under both. Every slot is introduced by something built before
/// anything can load it, which is what keeps the numbering from depending on a list that
/// gets sorted.
struct Frame<'a> {
    exprs: &'a Exprs,
    slots: HashMap<u32, u32>,
    next: u32,
}

impl<'a> Frame<'a> {
    fn new(exprs: &'a Exprs) -> Self {
        Frame {
            exprs,
            slots: HashMap::new(),
            next: 0,
        }
    }

    fn slot(&mut self, slot: Slot) -> Sexp {
        let next = self.next;
        let number = *self.slots.entry(slot.0).or_insert(next);
        if number == next {
            self.next += 1;
        }
        atom(format!("${number}"))
    }

    /// `now()` lowers to a load of a slot nothing else declares, so without this its
    /// number would be decided by whichever statement read it first.
    fn now(&mut self, slot: Option<Slot>) -> Vec<Sexp> {
        match slot {
            Some(slot) => vec![node("now", [self.slot(slot)])],
            None => Vec::new(),
        }
    }

    /// What an effect arm or a projector handler binds off the event that triggered it.
    fn trigger(&mut self, binds: &[Bind], envelope: &[EnvBind]) -> Vec<Sexp> {
        let mut parts = Vec::new();
        for bind in sorted_binds(binds) {
            let slot = self.slot(bind.slot);
            parts.push(node("bind", [atom(bind.field.clone()), slot]));
        }
        let mut envelope: Vec<&EnvBind> = envelope.iter().collect();
        envelope.sort_by_key(|bind| env(bind.field));
        for bind in envelope {
            let slot = self.slot(bind.slot);
            parts.push(node("env", [atom(env(bind.field)), slot]));
        }
        parts
    }

    fn stage(&mut self, stage: &Stage) -> Sexp {
        let mut parts = Vec::new();
        if !stage.pre.is_empty() {
            let pre = self.body(&stage.pre);
            parts.push(node("pre", pre));
        }
        // Before the slices, because a slice's accumulation writes a fold's slot and this
        // is where that slot is declared.
        for fold in &stage.folds {
            let slot = self.slot(fold.slot);
            let init = self.expr(fold.init);
            parts.push(node("fold", [slot, ty(&fold.ty), init]));
        }
        // Kept in written order. Sorting them would have to be settled against the slot
        // numbering their binds hand out, and reordering fold arms is an edit rather than
        // a way of writing the same thing.
        for slice in &stage.slices {
            parts.push(self.slice(slice));
        }
        if !stage.post.is_empty() {
            let post = self.body(&stage.post);
            parts.push(node("post", post));
        }
        node("stage", parts)
    }

    fn slice(&mut self, slice: &Slice) -> Sexp {
        let mut parts = vec![atom(slice.event.to_string())];
        for (field, value) in self.sorted(&filters(slice)) {
            parts.push(node("filter", [atom(field), value]));
        }
        for bind in sorted_binds(&slice.binds) {
            let slot = self.slot(bind.slot);
            parts.push(node("bind", [atom(bind.field.clone()), slot]));
        }
        for update in &slice.updates {
            let slot = self.slot(update.slot);
            let value = self.expr(update.value);
            parts.push(node("acc", [slot, ty(&update.ty), value]));
        }
        node("slice", parts)
    }

    fn body(&mut self, body: &[Stmt]) -> Vec<Sexp> {
        body.iter().map(|stmt| self.stmt(stmt)).collect()
    }

    fn stmt(&mut self, stmt: &Stmt) -> Sexp {
        match stmt {
            Stmt::Assign { slot, value } => {
                let target = self.slot(*slot);
                let value = self.expr(*value);
                node("set", [target, value])
            }
            Stmt::If {
                cond,
                then,
                otherwise,
            } => {
                let cond = self.expr(*cond);
                let then = self.body(then);
                let mut parts = vec![cond, node("then", then)];
                if !otherwise.is_empty() {
                    let otherwise = self.body(otherwise);
                    parts.push(node("else", otherwise));
                }
                node("if", parts)
            }
            Stmt::Emit { event, fields, .. } => {
                let mut parts = vec![atom(event.to_string())];
                parts.extend(self.keyed(fields));
                node("emit", parts)
            }
            Stmt::Put { entity, fields, .. } => {
                let mut parts = vec![atom(entity.clone())];
                parts.extend(self.keyed(fields));
                node("put", parts)
            }
            Stmt::Patch {
                entity,
                key,
                absent,
                loads,
                fields,
                ..
            } => {
                let head = match absent {
                    Absent::Materialize => "patch",
                    Absent::Skip => "update",
                };
                let key = self.expr(*key);
                let mut parts = vec![atom(entity.clone()), node("key", [key])];
                // Sorted for the same reason the fields below are: which columns rule 3
                // loads follows from which the fields read, so leaving these in written
                // order would leak the field order back into the slot numbering.
                for bind in sorted_binds(loads) {
                    let slot = self.slot(bind.slot);
                    parts.push(node("load", [atom(bind.field.clone()), slot]));
                }
                parts.extend(self.keyed(fields));
                node(head, parts)
            }
            Stmt::Delete { entity, key } => {
                let key = self.expr(*key);
                node("delete", [atom(entity.clone()), node("key", [key])])
            }
            Stmt::Fail { message, .. } => {
                let message = self.expr(*message);
                node("fail", [message])
            }
            Stmt::Log { message } => {
                let message = self.expr(*message);
                node("log", [message])
            }
            Stmt::Erase { subject, value, .. } => {
                let value = self.expr(*value);
                node("erase", [atom(subject.clone()), value])
            }
            Stmt::For { iter, body } => {
                let mut parts = self.iter(iter);
                let body = self.body(body);
                parts.push(node("do", body));
                node("for", parts)
            }
            Stmt::Discard(value) => {
                let value = self.expr(*value);
                node("discard", [value])
            }
            Stmt::Call {
                function,
                scope,
                args,
                ..
            } => {
                let mut parts = vec![atom(qualified(scope.as_deref(), function))];
                parts.extend(args.iter().map(|arg| self.expr(*arg)));
                node("call", parts)
            }
            Stmt::Return(Return::Ok) => node("return", []),
            Stmt::Return(Return::Invalid(message)) => {
                let message = self.expr(*message);
                node("return", [node("invalid", [message])])
            }
            Stmt::Return(Return::Reject { code, message }) => {
                let code = self.expr(*code);
                let message = self.expr(*message);
                node("return", [node("reject", [code, message])])
            }
            Stmt::Return(Return::Value(value)) => {
                let value = self.expr(*value);
                node("return", [node("value", [value])])
            }
            Stmt::Return(Return::Outcome(value)) => {
                let value = self.expr(*value);
                node("return", [node("outcome", [value])])
            }
        }
    }

    fn expect(&mut self, expect: &Expect) -> Sexp {
        match expect {
            Expect::Event { path, fields, .. } => {
                let mut parts = vec![atom(path.to_string())];
                parts.extend(self.keyed(fields));
                node("event", parts)
            }
            Expect::Nothing { .. } => atom("nothing"),
            Expect::Invalid { message, .. } => {
                let message = self.expr(*message);
                node("invalid", [message])
            }
            Expect::Reject { code, message, .. } => {
                let code = self.expr(*code);
                let message = self.expr(*message);
                node("reject", [code, message])
            }
            Expect::Row {
                entity,
                key,
                fields,
                ..
            } => {
                let key = self.expr(*key);
                let mut parts = vec![atom(entity.clone()), node("key", [key])];
                parts.extend(self.keyed(fields));
                node("row", parts)
            }
            Expect::NoRow { entity, key, .. } => {
                let key = self.expr(*key);
                node("norow", [atom(entity.clone()), node("key", [key])])
            }
            Expect::Http {
                verb, url, body, ..
            } => {
                let url = self.expr(*url);
                let mut parts = vec![builtin(*verb), url];
                if let Some(body) = body {
                    let body = self.expr(*body);
                    parts.push(body);
                }
                node("http", parts)
            }
            Expect::Invoke { command, args, .. } => {
                let mut parts = vec![atom(command.clone())];
                parts.extend(self.keyed(args));
                node("invoke", parts)
            }
            Expect::Erase { subject, id, .. } => {
                let id = self.expr(*id);
                node("erase", [atom(subject.clone()), id])
            }
            Expect::Log { message, .. } => {
                let message = self.expr(*message);
                node("log", [message])
            }
            Expect::Failed { message, .. } => {
                let message = self.expr(*message);
                node("failed", [message])
            }
            Expect::Skipped { .. } => atom("skipped"),
        }
    }

    // -----------------------------------------------------------------------
    // Expressions
    // -----------------------------------------------------------------------

    fn expr(&mut self, id: ExprId) -> Sexp {
        let exprs = self.exprs;
        let Some(found) = exprs.get(id) else {
            return node("bad", []);
        };
        match found {
            Expr::Lit(value) => literal(value),
            // A bare `$n` in a value position is a load; in a binding position it is the
            // slot itself. Nothing else spells a slot, so the two never meet.
            Expr::Load(slot) => self.slot(*slot),
            Expr::Unary { op, operand } => {
                let head = match op {
                    UnOp::Not => "not",
                    UnOp::Neg => "neg",
                };
                let operand = self.expr(*operand);
                node(head, [operand])
            }
            Expr::Binary { op, lhs, rhs } => {
                let lhs = self.expr(*lhs);
                let rhs = self.expr(*rhs);
                node(op.symbol(), [lhs, rhs])
            }
            Expr::Method {
                receiver,
                method,
                args,
            } => {
                let mut parts = vec![self.expr(*receiver)];
                parts.extend(args.iter().map(|arg| self.expr(*arg)));
                node(&format!(".{method}"), parts)
            }
            Expr::If {
                cond,
                then,
                otherwise,
            } => {
                let cond = self.expr(*cond);
                let then = self.expr(*then);
                let otherwise = self.expr(*otherwise);
                node("choose", [cond, then, otherwise])
            }
            Expr::Field { receiver, name } => {
                let receiver = self.expr(*receiver);
                node("field", [receiver, atom(name.clone())])
            }
            Expr::Object(fields) => node("obj", self.quoted(fields)),
            Expr::Interp(parts) => node(
                "interp",
                parts
                    .iter()
                    .map(|part| self.expr(*part))
                    .collect::<Vec<_>>(),
            ),
            Expr::List { items, inner } => {
                let mut parts = Vec::new();
                if let Some(inner) = inner {
                    parts.push(node("of", [ty(inner)]));
                }
                parts.extend(items.iter().map(|item| self.expr(*item)));
                node("array", parts)
            }
            Expr::Record { ty: name, fields } => {
                let mut parts = vec![atom(name.clone())];
                parts.extend(self.keyed(fields));
                node("new", parts)
            }
            Expr::CallFn {
                function,
                scope,
                args,
            } => {
                let mut parts = vec![atom(qualified(scope.as_deref(), function))];
                parts.extend(args.iter().map(|arg| self.expr(*arg)));
                node("fn", parts)
            }
            Expr::Comp {
                iter,
                cond,
                yields,
                inner,
            } => {
                let mut parts = Vec::new();
                if let Some(inner) = inner {
                    parts.push(node("of", [ty(inner)]));
                }
                parts.extend(self.iter(iter));
                if let Some(cond) = cond {
                    let cond = self.expr(*cond);
                    parts.push(node("when", [cond]));
                }
                let yields = self.expr(*yields);
                parts.push(node("yield", [yields]));
                node("comp", parts)
            }
            Expr::Call {
                builtin: name,
                args,
            } => {
                let mut parts = vec![builtin(*name)];
                parts.extend(args.iter().map(|arg| self.expr(*arg)));
                node("builtin", parts)
            }
            Expr::Invoke { command, args } => {
                let mut parts = vec![atom(command.clone())];
                parts.extend(self.keyed(args));
                node("invoke", parts)
            }
            Expr::Unwrap(inner) => {
                let inner = self.expr(*inner);
                node("unwrap", [inner])
            }
            Expr::Reveal { value, ty: content } => {
                let value = self.expr(*value);
                node("reveal", [value, ty(content)])
            }
            Expr::Refusal { code, message } => match code {
                Some(code) => {
                    let code = self.expr(*code);
                    let message = self.expr(*message);
                    node("reject", [code, message])
                }
                None => {
                    let message = self.expr(*message);
                    node("invalid", [message])
                }
            },
            Expr::Invalid => node("bad", []),
        }
    }

    /// What a `for` or a comprehension binds. `over` is resolved first because it is
    /// evaluated outside the loop, and the bindings are numbered in the order they are
    /// built, so nothing inside the body can name one before it exists.
    fn iter(&mut self, iter: &Iter) -> Vec<Sexp> {
        let over = self.expr(iter.over);
        let mut parts = vec![node("in", [over])];
        if let Some(index) = iter.index {
            let index = self.slot(index);
            parts.push(node("index", [index]));
        }
        let item = self.slot(iter.item);
        parts.push(node("item", [item]));
        parts
    }

    /// A field list whose keys the target already declares, so which order they were
    /// written in is not observable and sorting them makes the two spellings agree.
    ///
    /// The exception is a value that calls out. An effect's `invoke` and `http.*` land in
    /// the journal in the order they ran, so reordering two of them written as sibling
    /// values *is* a change, and such a list keeps the order it was written in.
    fn keyed(&mut self, fields: &[(Ident, ExprId)]) -> Vec<Sexp> {
        self.sorted(fields)
            .into_iter()
            .map(|(field, value)| node("f", [atom(field), value]))
            .collect()
    }

    /// A JSON object, whose keys are arbitrary text rather than identifiers and so are
    /// quoted. Without that `{"a=1,b": 2}` and `{"a": 1, "b": 2}` could pack the same.
    fn quoted(&mut self, fields: &[(Ident, ExprId)]) -> Vec<Sexp> {
        self.sorted(fields)
            .into_iter()
            .map(|(field, value)| node("f", [Sexp::text(field), value]))
            .collect()
    }

    fn sorted(&mut self, fields: &[(Ident, ExprId)]) -> Vec<(Ident, Sexp)> {
        let exprs = self.exprs;
        let mut order: Vec<&(Ident, ExprId)> = fields.iter().collect();
        if fields.iter().all(|(_, id)| pure(exprs, *id)) {
            order.sort_by(|one, two| one.0.cmp(&two.0));
        }
        // Built after the sort, never before: a slot number is handed out at first
        // appearance, so building in written order and sorting afterwards would let the
        // written order back into the numbering.
        order
            .into_iter()
            .map(|(field, id)| (field.clone(), self.expr(*id)))
            .collect()
    }
}

/// Whether evaluating this subtree can be observed from outside the program. Only three
/// things can be: an HTTP call, an `invoke`, and a call to an effect-local `fn`, which is
/// the only kind of `fn` that may do either.
fn pure(exprs: &Exprs, id: ExprId) -> bool {
    let Some(node) = exprs.get(id) else {
        return true;
    };
    match node {
        Expr::Call { builtin, .. } if builtin.is_http() => false,
        Expr::Invoke { .. } => false,
        Expr::CallFn { scope: Some(_), .. } => false,
        node => children(node).into_iter().all(|child| pure(exprs, child)),
    }
}

fn filters(slice: &Slice) -> Vec<(Ident, ExprId)> {
    slice
        .filters
        .iter()
        .map(|filter| (filter.field.clone(), filter.value))
        .collect()
}

fn sorted_binds(binds: &[Bind]) -> Vec<&Bind> {
    let mut binds: Vec<&Bind> = binds.iter().collect();
    binds.sort_by(|one, two| one.field.cmp(&two.field));
    binds
}

fn env(field: EnvField) -> &'static str {
    match field {
        EnvField::At => "at",
        EnvField::Id => "id",
        EnvField::Position => "position",
    }
}

fn qualified(scope: Option<&str>, name: &str) -> String {
    match scope {
        Some(scope) => format!("{scope}.{name}"),
        None => name.to_string(),
    }
}

/// Types are spelled the way heklang spells them, capitalised, which keeps them apart from
/// the lowercase heads a value uses: `(Money 2)` the type and `(money 2 1050)` the amount
/// are never the same node.
fn ty(value: &Type) -> Sexp {
    match value {
        Type::Bool => atom("Bool"),
        Type::Int => atom("Int"),
        Type::Decimal(scale) => node("Decimal", [atom(scale.to_string())]),
        Type::String => atom("String"),
        Type::Uuid => atom("Uuid"),
        Type::Timestamp => atom("Timestamp"),
        Type::Money(scale) => node("Money", [atom(scale.to_string())]),
        Type::Enum(name) => node("Enum", [atom(name.clone())]),
        Type::Record(name) => node("Record", [atom(name.clone())]),
        Type::Rounding => atom("Rounding"),
        Type::Json => atom("Json"),
        Type::Response => atom("Response"),
        Type::Outcome => atom("Outcome"),
        Type::List(inner) => node("List", [ty(inner)]),
        Type::Map(key, value) => node("Map", [ty(key), ty(value)]),
        Type::Opt(inner) => node("Opt", [ty(inner)]),
        Type::Sealed(inner, subject) => node("Sealed", [ty(inner), atom(subject.clone())]),
    }
}

/// `Money.parse` and `Decimal.parse` carry the scale they were resolved at, which comes
/// from where the result lands rather than from the text, so two calls spelled the same
/// are two different builtins.
fn builtin(name: Builtin) -> Sexp {
    match name {
        Builtin::MoneyParse(scale) | Builtin::DecimalParse(scale) => {
            node(name.name(), [atom(scale.to_string())])
        }
        _ => atom(name.name()),
    }
}

fn literal(value: &Literal) -> Sexp {
    match value {
        Literal::Bool(value) => node("bool", [atom(value.to_string())]),
        Literal::Int(value) => node("int", [atom(value.to_string())]),
        // The units and the scale, never the spelling: `1000` and `1000.00` both reach a
        // `Money(2)` as a hundred thousand units, and the digest says so.
        Literal::Decimal { units, scale } => {
            node("dec", [atom(scale.to_string()), atom(units.to_string())])
        }
        Literal::Money { units, scale } => {
            node("money", [atom(scale.to_string()), atom(units.to_string())])
        }
        Literal::Str(text) => node("str", [Sexp::text(text.to_string())]),
        Literal::Uuid(text) => node("uuid", [Sexp::text(text.to_string())]),
        Literal::Timestamp(micros) => node("ts", [atom(micros.to_string())]),
        Literal::None(inner) => node("none", [ty(inner)]),
        Literal::Some { inner, value } => node("some", [ty(inner), literal(value)]),
        Literal::Enum { ty: name, variant } => {
            node("variant", [atom(name.clone()), atom(variant.clone())])
        }
        Literal::Rounding(mode) => node("rounding", [atom(mode.to_string())]),
        // The same head an `Expr::List` builds, because they are the same value: `[]` is a
        // literal and `[a]` is an expression, and that is the parser's business, not a
        // difference in what runs.
        Literal::List { inner, items } => {
            let mut parts = vec![node("of", [ty(inner)])];
            parts.extend(items.iter().map(literal));
            node("array", parts)
        }
        Literal::EmptyMap(key, value) => node("map-empty", [ty(key), ty(value)]),
        // The same head an empty `Expr::Object` builds: an unwritten `headers` argument is
        // this, and `headers = {}` is that.
        Literal::EmptyJson => node("obj", []),
        Literal::JsonNum(text) => node("json-num", [Sexp::text(text.clone())]),
        Literal::Record { ty: name, fields } => {
            let mut sorted: Vec<&(Ident, Literal)> = fields.iter().collect();
            sorted.sort_by(|one, two| one.0.cmp(&two.0));
            let mut parts = vec![atom(name.clone())];
            parts.extend(
                sorted
                    .into_iter()
                    .map(|(field, value)| node("f", [atom(field.clone()), literal(value)])),
            );
            node("new", parts)
        }
    }
}
