//! The read-model layout a program implies: which named folds can be kept, and what
//! tables they group into.
//!
//! One sentence carries it: **a read model is a named fold, kept for every key.** A fold
//! already says what a value is, which events change it and what it is keyed by, so the
//! tables an application wants are derivable rather than declared, and this derives them.
//!
//! Nothing here stores anything or reports a diagnostic. `hek check --tables` prints what
//! [`layout`] returns, and the runtime creates the real tables from the same rules, which
//! is why the rules live in the library rather than in the CLI.

use crate::interp::{Error, ErrorKind, eval_pure, fitted_at};
use crate::ir::{EventPath, Expr, ExprId, Exprs, Filter, Fold, Ident, Program, Slot, Type};
use crate::parse::snake_case;
use crate::value::Value;

/// Every table a program implies, and every fold that cannot be kept.
#[derive(Debug, Clone, Default)]
pub struct Layout {
    /// In tree order: a key sorts before any key that extends it, so a child table sits
    /// just below the parent it narrows.
    pub tables: Vec<Table>,
    /// In declaration order.
    pub unstored: Vec<Unstored>,
}

impl Layout {
    /// The table and column one named fold is kept in, or `None` for a fold that is not
    /// kept at all.
    ///
    /// A fold is one column, so this is a lookup rather than a search with a choice in
    /// it. Here rather than at each caller because the pair is what anything reading a
    /// stored fold needs: the table names the row and the column names the cell, and a
    /// caller holding only one of the two cannot ask [`crate::Columns`] for anything.
    pub fn column(&self, fold: &str) -> Option<(&Table, &Column)> {
        self.tables.iter().find_map(|table| {
            table
                .columns
                .iter()
                .find(|column| column.fold == fold)
                .map(|column| (table, column))
        })
    }

    /// Every column that has to be applied for `folds` to come out right, named by fold
    /// and sorted: what [`crate::Tables::only`] actually applies.
    ///
    /// **A column is not a closed system, and a range arm is why.** An arm leaving a key
    /// part open is a range update over *the keys the table holds*, and the keys are
    /// minted by the fully pinned arms of that table's columns, this one's and the
    /// others'. So a replay restricted to a ranged column alone never mints the keys its
    /// range covers: the range finds nothing, the column stays at its seed, and the
    /// answer is wrong with nothing to say so. Restricting to a column whose every arm
    /// pins the whole key, which is the ordinary case, restricts to that column.
    ///
    /// **What the range needs added is the minting**, so what is added is the columns of
    /// that table with a fully pinned arm, and not the table. A column *no* arm of which
    /// pins the whole key writes only at keys that already exist, since
    /// [`Columns::keys`] is the only thing it asks, so it mints nothing and is no use to
    /// anyone else's range. One pinned arm is enough to be added, which is why the test
    /// is not the negation of the one above: a column can have a pinning arm and a
    /// ranging one, and `OrderClosed` is that shape.
    ///
    /// [`Columns::keys`]: crate::Columns::keys
    ///
    /// A name no column is kept for contributes nothing, so an unstored fold and a typo
    /// both drop out here rather than at the apply.
    ///
    /// **What the additions are for is the keys, so the keys are what a caller has to
    /// keep.** A rebuild that drops their writes outright drops the keys with them and is
    /// back where it started; one that records the key and discards the value has
    /// everything a range asked for, since [`crate::Columns::keys`] is all a ranged arm
    /// reads. Keeping the values too is the simpler thing and costs only an overlay.
    pub fn closure(&self, folds: &[Ident]) -> Vec<Ident> {
        let mints = |column: &&Column| {
            column
                .arms
                .iter()
                .any(|arm| arm.fields.iter().all(ArmMatch::pinned))
        };
        // A worklist, because the two rules feed each other: a column added for a range
        // may itself key through something, and a dependency may range over a table whose
        // minters are not in yet. Each name enters `out` once, so it closes.
        let mut out: Vec<Ident> = Vec::new();
        let mut queue: Vec<Ident> = folds.to_vec();
        while let Some(name) = queue.pop() {
            // A name no column is kept for contributes nothing, so an unstored fold and a
            // typo both drop out here rather than at the apply.
            let Some((table, column)) = self.column(&name) else {
                continue;
            };
            if out.contains(&name) {
                continue;
            }
            out.push(name);
            // **Rule 15.** A column keyed through another needs that one's value as of
            // each past record, which only maintaining it alongside can give.
            queue.extend(column.reads.iter().cloned());
            // **The range rule.** An arm leaving a part open covers the keys the table
            // holds, and its table's minters are what put them there.
            let ranges = column
                .arms
                .iter()
                .any(|arm| arm.fields.iter().any(|field| !field.pinned()));
            if ranges {
                queue.extend(table.columns.iter().filter(mints).map(|at| at.fold.clone()));
            }
        }
        out.sort();
        out
    }
}

/// One key signature and every fold kept for it.
#[derive(Debug, Clone)]
pub struct Table {
    /// The key parts joined with `__`, or `global` for the single-row table a fold with
    /// no parameters is kept in.
    pub name: String,
    pub key: Vec<KeyPart>,
    /// Alphabetical by column name.
    pub columns: Vec<Column>,
}

/// One column of a table's key.
#[derive(Debug, Clone)]
pub struct KeyPart {
    pub name: String,
    /// The parameter's declared type, kept whole rather than reduced to a name, because
    /// a runtime building the real column needs it.
    pub ty: Type,
    /// The subject this part keys, when it keys one.
    pub subject: Option<Ident>,
}

/// One fold, kept for every key the table has.
#[derive(Debug, Clone)]
pub struct Column {
    /// The fold's name in snake_case, prefix and all. Dropping a shared prefix is a
    /// presentation decision and belongs to whatever is presenting.
    pub name: String,
    pub fold: Ident,
    pub ty: Type,
    /// The **parameter** each part of the table's key came from, in the table's key
    /// order: what [`crate::Interpreter::fold_stored`] has to be called with.
    ///
    /// Not the part names, and that is the whole reason this is recorded rather than
    /// re-derived. A part is named for its subject, or for its parameter with one
    /// trailing `_id` taken off, so `(shop_id: Shop, request_id: Int)` keys a table on
    /// `(shop, request)` and neither name is callable. A reader matching part names back
    /// onto parameters reimplements [`part_names`] to do it, and gets `missing argument`
    /// for every fold the first time it is slightly wrong.
    ///
    /// Per column and not per table, because one key signature is reached by parameters
    /// of different names: `(shop_id: Shop)` and `(shop: Shop)` are one key.
    pub args: Vec<Ident>,
    /// The folds this column **keys through** (`docs/folds.md` rule 15), directly or
    /// through a chain, sorted and each once. Empty for almost every column.
    ///
    /// Transitive rather than direct, because that is the set anything acting on it wants:
    /// a change two links away changes this column's values just as a change one link away
    /// does, so it is what the digest records, what a rebuild has to replay alongside, and
    /// what `hek check --tables` prints.
    pub reads: Vec<Ident>,
    /// The events this column folds, first written first, each once.
    pub events: Vec<EventPath>,
    /// One per `on`, in source order.
    pub arms: Vec<ColumnArm>,
}

/// One arm, as whatever writes the row sees it.
#[derive(Debug, Clone)]
pub struct ColumnArm {
    pub event: EventPath,
    /// For each part of the table's key, in the table's order, what this arm pins it to.
    pub fields: Vec<ArmMatch>,
}

/// How one arm fills one part of the key.
///
/// Three states and not two, because an arm can pin a part to something that is not a
/// field of the event at all (`docs/folds.md` rule 13). Collapsing `Fixed` into `Open`
/// would make the fold storable while telling the writer to range over the very part the
/// arm fixed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArmMatch {
    /// The **event field** whose value fills this part.
    ///
    /// The field, not the parameter, because the two are often two different names:
    /// `on @address.change.escalated(shop_id, order_id, request_id: item_id)` fills a key
    /// part called `item` from a field called `request_id`. Working that out is the same
    /// analysis storability already does, so it is recorded here rather than left for a
    /// second implementation of the rule to get wrong.
    Field(Ident),
    /// A value the arm fixes, from `param == value`: not on the event, and resolved once
    /// in the frame of whatever reads the fold. The id indexes that fold's own arena.
    Fixed(ExprId),
    /// A value the arm **computes** from the record, from `key param = expr`: evaluated
    /// per record against that record's binds and envelope, so unlike [`Self::Fixed`] it
    /// cannot be resolved without one. The id indexes the fold's own arena.
    ///
    /// Still [`pinned`](Self::pinned): one record writes one key, which is not a range.
    /// That is what keeps a host's index derivation right without it knowing this state
    /// exists.
    Computed(ExprId),
    /// Not pinned by this arm, which is not a defect: an arm matching a subset of the key
    /// is a range update over the parts it leaves open.
    Open,
}

impl ArmMatch {
    /// Whether this arm pins the part at all, either way.
    pub fn pinned(&self) -> bool {
        !matches!(self, ArmMatch::Open)
    }
}

/// A fold that cannot be kept, because part of its key is not something an event
/// supplies.
#[derive(Debug, Clone)]
pub struct Unstored {
    pub fold: Ident,
    /// Every parameter no arm matches, as the key would have ordered them. Named by the
    /// parameter rather than by the part it would have been, because the author has to
    /// find it in a signature.
    pub unmatched: Vec<Ident>,
}

/// The tables `program`'s named folds imply.
pub fn layout(program: &Program) -> Layout {
    let mut tables: Vec<Table> = Vec::new();
    let mut unstored = Vec::new();

    for fold in &program.folds {
        // The key in table order, each part paired with the parameter it came from, so
        // an arm's fields can be listed in the same order the key is.
        let key = key_of(program, fold);
        let arms: Vec<ColumnArm> = fold
            .stage
            .slices
            .iter()
            .map(|slice| ColumnArm {
                event: slice.event.clone(),
                fields: key
                    .iter()
                    .map(|(at, _)| {
                        let param = &fold.params[*at];
                        // Rule 13 first, because an arm may do both and the key conjunct
                        // is the stronger statement: `(kind, kind == "x")` writes the row
                        // at `"x"` and narrows the event to the rows whose field agrees,
                        // and a writer told only the field would write at whatever the
                        // event carried.
                        let fixed = slice.filters.iter().find_map(|filter| match filter {
                            Filter::Key {
                                param: named,
                                value,
                                ..
                            } => (named == &param.name).then_some(ArmMatch::Fixed(*value)),
                            Filter::Field { .. } => None,
                        });
                        // Then a computed part, which is a key this arm writes just as a
                        // fixed one is, and before the field search for the same reason:
                        // it is the stronger statement about where the row goes.
                        let computed = || {
                            slice
                                .keys
                                .iter()
                                .find(|key| key.param == param.name)
                                .map(|key| ArmMatch::Computed(key.value))
                        };
                        let found = fixed.or_else(computed).or_else(|| {
                            slice.filters.iter().find_map(|filter| match filter {
                                Filter::Field { field, value } => {
                                    pins(&fold.exprs, *value, param.slot)
                                        .then(|| ArmMatch::Field(field.clone()))
                                }
                                Filter::Key { .. } => None,
                            })
                        });
                        found.unwrap_or(ArmMatch::Open)
                    })
                    .collect(),
            })
            .collect();

        // Some arm, not every arm. An arm matching a subset of the key is a range update
        // over that subset, so a fold whose first arm pins `(code, shop_id)` and whose
        // rest pin `shop_id` alone is still kept.
        let unmatched: Vec<Ident> = key
            .iter()
            .enumerate()
            .filter(|(at, _)| arms.iter().all(|arm| !arm.fields[*at].pinned()))
            .map(|(_, (at, _))| fold.params[*at].name.clone())
            .collect();
        if !unmatched.is_empty() {
            unstored.push(Unstored {
                fold: fold.name.clone(),
                unmatched,
            });
            continue;
        }

        // The parameter per key position, taken here while the pairing still exists:
        // `key` is about to lose the indexes that say which parameter each part was.
        let args: Vec<Ident> = key
            .iter()
            .map(|(at, _)| fold.params[*at].name.clone())
            .collect();
        let key: Vec<KeyPart> = key.into_iter().map(|(_, part)| part).collect();
        let column = Column {
            name: snake_case(&fold.name),
            fold: fold.name.clone(),
            ty: fold.ret.clone(),
            args,
            reads: program.reads(&fold.name),
            events: events_of(&arms),
            arms,
        };
        match tables.iter_mut().find(|table| same(&table.key, &key)) {
            Some(table) => table.columns.push(column),
            None => tables.push(Table {
                name: table_name(&key),
                key,
                columns: vec![column],
            }),
        }
    }

    // Two key signatures can derive one name, because a part is told apart by name and
    // type and a name is only the names joined: `(day: String)` and `(day: Int)` are two
    // tables both called `day`. Rare enough that no program has done it, and a runtime
    // keying on the name would silently serve one table for both, so the type goes into
    // the name of each rather than into a diagnostic nothing could act on.
    let clashes: Vec<String> = tables
        .iter()
        .filter(|table| {
            tables
                .iter()
                .filter(|other| other.name == table.name)
                .count()
                > 1
        })
        .map(|table| table.name.clone())
        .collect();
    for table in &mut tables {
        if clashes.contains(&table.name) {
            table.name = table
                .key
                .iter()
                .map(|part| format!("{}_{}", part.name, type_tag(&part.ty)))
                .collect::<Vec<_>>()
                .join("__");
        }
    }

    // The name breaks a tie, so that two keys alike in their part names but not their
    // types still have one order. Without it the two fall back on declaration order,
    // which is the one thing a layout is not allowed to depend on.
    tables.sort_by(|a, b| parts(a).cmp(&parts(b)).then_with(|| a.name.cmp(&b.name)));
    for table in &mut tables {
        table.columns.sort_by(|a, b| a.name.cmp(&b.name));
    }
    Layout { tables, unstored }
}

/// Whether a filter's value pins its field to `param`, which is what makes the parameter
/// a key a row can be written under and read back out of.
///
/// The load has to be the whole value. `(day: normalise(day))` reads the parameter but
/// pins the field to something the key cannot be recovered from, and a walk looking for
/// the slot anywhere underneath would wrongly accept it.
///
/// `Expr::Wrap` is deliberately not peeled, and neither of its two synthesisers can put
/// one here: `Parser::lift` makes it an operand of a comparison, and `Parser::lifted` the
/// value of a spliced call's parameter assignment, which lives in the **caller's** arena
/// rather than in `fold.exprs`.
///
/// Over an [`Exprs`] rather than a [`Fold`], because the parser asks the same question of
/// an arm it has not lowered into one yet: rule 14 refuses a filter reading a parameter
/// this arm leaves *open*, and open means no filter pins it. One derivation and two
/// readers, as `part_names` already is.
pub fn pins(exprs: &Exprs, value: ExprId, slot: Slot) -> bool {
    let mut id = value;
    // A fold body has no statements and so nothing that narrows, but peeling is free and
    // the house style everywhere else.
    if let Some(Expr::Unwrap(inner)) = exprs.get(id) {
        id = *inner;
    }
    matches!(exprs.get(id), Some(Expr::Load(found)) if *found == slot)
}

/// The name each key part takes, in **parameter** order rather than key order: what
/// [`KeyPart::name`] will hold.
///
/// Public and separate from `key_of` because the parser asks the same question of a
/// signature it has not lowered yet, to refuse a fold whose own column name is one of
/// these (`docs/folds.md` rule 1). One derivation and two readers, so the refusal cannot
/// drift from the names it is about.
pub fn part_names<'a>(params: impl IntoIterator<Item = (&'a Ident, &'a Type)>) -> Vec<String> {
    let params: Vec<(&Ident, &Type)> = params.into_iter().collect();

    // The name each part would take if nothing shared it: a subject part is named for the
    // subject, which is what makes `(shop_id: Shop)` and `(shop: Shop)` one key.
    let wanted: Vec<String> = params
        .iter()
        .map(|(name, ty)| match subject_of(ty) {
            Some(subject) => snake_case(subject),
            None => plain_name(name),
        })
        .collect();

    // **Two parts of one key can want one name**, by repeating a subject or by spelling it
    // two ways, and neither is a mistake. They are told apart by the **parameter**, never
    // by position.
    //
    // Position was the first answer and it was wrong twice. `_1`/`_2` counted declaration
    // order, so `(to_id: Region, from_id: Region)` and `(from_id: Region, to_id: Region)`
    // produced the same two names over the same two types, which `same` reads as one
    // table, with `region_1` meaning a different parameter in each: a writer would store
    // one row's two columns at each other's addresses. And a suffix is spellable, so a
    // parameter actually called `region_1` collided with the suffix of another part.
    //
    // The parameter is the one thing that is unique within a fold, so falling back to it
    // cannot collide and does not depend on order. `plain_name` first, because `from` and
    // `to` read better than `from_id` and `to_id`; the raw name after it, for the pair
    // that `_id`-stripping maps together.
    params
        .iter()
        .enumerate()
        .map(|(at, (name, _))| {
            if wanted.iter().filter(|other| *other == &wanted[at]).count() == 1 {
                return wanted[at].clone();
            }
            let plain = plain_name(name);
            match params
                .iter()
                .filter(|(other, _)| plain_name(other) == plain)
                .count()
            {
                1 => plain,
                _ => name.to_string(),
            }
        })
        .collect()
}

/// A fold's key: its parameters, named and ordered for a table rather than for a call,
/// each paired with the position it came from in `fold.params`.
fn key_of(program: &Program, fold: &Fold) -> Vec<(usize, KeyPart)> {
    let names = part_names(fold.params.iter().map(|param| (&param.name, &param.ty)));

    let mut sorted: Vec<(u8, Vec<String>, usize, KeyPart)> = fold
        .params
        .iter()
        .enumerate()
        .map(|(at, param)| {
            let name = names[at].clone();
            match subject_of(&param.ty) {
                // The sort path is the subject's line from the root, with this part's own
                // name as the leaf: containment orders the ancestors, and the name is what
                // separates two parts of one subject.
                Some(subject) => {
                    let mut line = path(program, subject);
                    line.pop();
                    line.push(name.clone());
                    (
                        0,
                        line,
                        at,
                        KeyPart {
                            name,
                            ty: param.ty.clone(),
                            subject: Some(subject.clone()),
                        },
                    )
                }
                None => (
                    1,
                    vec![name.clone()],
                    at,
                    KeyPart {
                        name,
                        ty: param.ty.clone(),
                        subject: None,
                    },
                ),
            }
        })
        .collect();
    // Every name is distinct now, so this is a total order and the `sort_by` needs no
    // stability to lean on.
    sorted.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));

    sorted
        .into_iter()
        .map(|(_, _, at, part)| (at, part))
        .collect()
}

/// A subject's line from the root down, which is what orders subject parts.
///
/// Containment, so a parent sits above its children, and alphabetical among the subjects
/// no containment relates. Comparing whole paths rather than asking "is one an ancestor
/// of the other" is what makes that a total order: a bare ancestor test with an
/// alphabetical fallback is not transitive, and sorting on it has no defined answer.
fn path(program: &Program, subject: &str) -> Vec<String> {
    let mut path: Vec<String> = program
        .ancestors(subject)
        .iter()
        .rev()
        .map(|def| snake_case(&def.name))
        .collect();
    path.push(snake_case(subject));
    path
}

/// The subject a parameter keys, through an optional, or `None` for a plain parameter.
///
/// Neither `Type::as_scalar` nor `Type::subject` answers this: the first reads a subject
/// through to the scalar its ids are and throws the name away, and the second reports the
/// subject a *seal* files its content under.
pub(crate) fn subject_of(ty: &Type) -> Option<&Ident> {
    match ty {
        Type::Subject(sub) => Some(&sub.name),
        Type::Opt(inner) => subject_of(inner),
        _ => None,
    }
}

/// A plain parameter's part name: the parameter, with one trailing `_id` taken off. The
/// column holds an order, and `order_id` in a key named `order` says the same thing twice.
fn plain_name(param: &str) -> String {
    param
        .strip_suffix("_id")
        .filter(|rest| !rest.is_empty())
        .unwrap_or(param)
        .to_string()
}

/// A type as an **identifier**, for the clash-breaking table name above.
///
/// `Type`'s own rendering is for a reader: `Money(2)`, `String?`, `String @subject(Shop)`.
/// Putting that through `snake_case` left `(`, `)`, `?` and `, ` in a name the module doc
/// promises a runtime will create a table from, so the shape is spelled out here instead.
/// Injective where `Display` is, because the disambiguation it feeds exists precisely to
/// tell two types apart.
fn type_tag(ty: &Type) -> String {
    match ty {
        Type::Opt(inner) => format!("{}_opt", type_tag(inner)),
        Type::Money(scale) => format!("money_{scale}"),
        Type::Decimal(scale) => format!("decimal_{scale}"),
        Type::List(inner) => format!("list_{}", type_tag(inner)),
        Type::Map(key, value) => format!("map_{}_{}", type_tag(key), type_tag(value)),
        Type::Sealed(inner, subject) => {
            format!("sealed_{}_{}", type_tag(inner), snake_case(subject))
        }
        Type::Subject(sub) => snake_case(&sub.name),
        Type::Enum(name) | Type::Record(name) => snake_case(name),
        Type::Event(path) => snake_case(&path.to_string().replace(['@', '.'], "_")),
        // Bool, Int, String, Uuid, Timestamp, Rounding, Json, Response, Outcome: each
        // renders as one capitalised word, which is already an identifier.
        plain => snake_case(&plain.to_string()),
    }
}

fn table_name(key: &[KeyPart]) -> String {
    match key.is_empty() {
        true => "global".to_string(),
        false => key
            .iter()
            .map(|part| part.name.as_str())
            .collect::<Vec<_>>()
            .join("__"),
    }
}

/// The events a column folds, first written first, each once. Two arms on one event is
/// not rare, and the listing says what feeds a column rather than how often it was
/// written.
fn events_of(arms: &[ColumnArm]) -> Vec<EventPath> {
    let mut events: Vec<EventPath> = Vec::new();
    for arm in arms {
        if !events.contains(&arm.event) {
            events.push(arm.event.clone());
        }
    }
    events
}

/// Whether two keys are the same table. Name and type both, because two parts spelled the
/// same over different types are two keys, and a runtime has to build a column for each.
fn same(a: &[KeyPart], b: &[KeyPart]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(a, b)| a.name == b.name && a.ty == b.ty)
}

fn parts(table: &Table) -> Vec<&str> {
    table.key.iter().map(|part| part.name.as_str()).collect()
}

/// The value an arm pins a key part to, for [`ArmMatch::Fixed`].
///
/// **Evaluated against an empty frame, which is the whole of rule 13's guarantee.** The
/// right side of a `param == value` conjunct may not read a slot, so it is a literal, a
/// `const` or a pure `fn` of those: whatever writes the row holds the event and the key it
/// is writing and no frame at all, and this is that caller's door onto the value. An
/// `ExprId` indexes the fold's own arena, so it cannot be evaluated without the fold, and
/// that is why this is here rather than left for a store to work out.
///
/// `part` is the key part the arm pins, which is the one at the same index in
/// [`Table::key`] as the [`ArmMatch`] is in [`ColumnArm::fields`]. Its declared type is
/// what the value is fitted to, so a part of `Money(2)` pinned to `0` is an amount rather
/// than an `Int`, the same fit `Interpreter::fold` makes when it resolves the conjunct.
pub fn fixed(program: &Program, fold: &Ident, part: &KeyPart, at: ExprId) -> Result<Value, Error> {
    let def = program
        .fold(fold)
        .ok_or_else(|| Error::new(ErrorKind::UnknownFold(fold.to_string())))?;
    let value = eval_pure(program, &def.exprs, def.frame, at)?;
    fitted_at(value, &part.ty, def.exprs.span(at))
}
