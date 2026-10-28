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

use crate::ir::{EventPath, Expr, ExprId, Fold, Ident, Param, Program, Type};
use crate::parse::snake_case;

/// Every table a program implies, and every fold that cannot be kept.
#[derive(Debug, Clone, Default)]
pub struct Layout {
    /// In tree order: a key sorts before any key that extends it, so a child table sits
    /// just below the parent it narrows.
    pub tables: Vec<Table>,
    /// In declaration order.
    pub unstored: Vec<Unstored>,
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
    /// The events this column folds, first written first, each once.
    pub events: Vec<EventPath>,
    /// One per `on`, in source order.
    pub arms: Vec<ColumnArm>,
}

/// One arm, as whatever writes the row sees it.
#[derive(Debug, Clone)]
pub struct ColumnArm {
    pub event: EventPath,
    /// For each part of the table's key, in the table's order, the **event field** whose
    /// value fills it, or `None` when this arm does not pin that part.
    ///
    /// The field, not the parameter, because the two are often two different names:
    /// `on @address.change.escalated(shop_id, order_id, request_id: item_id)` fills a key
    /// part called `item` from a field called `request_id`. Working that out is the same
    /// analysis storability already does, so it is recorded here rather than left for a
    /// second implementation of the rule to get wrong.
    ///
    /// A `None` is not a defect: an arm matching a subset of the key is a range update
    /// over the parts it leaves open.
    pub fields: Vec<Option<Ident>>,
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
                        slice
                            .filters
                            .iter()
                            .find(|filter| pins(fold, filter.value, param))
                            .map(|filter| filter.field.clone())
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
            .filter(|(at, _)| arms.iter().all(|arm| arm.fields[*at].is_none()))
            .map(|(_, (at, _))| fold.params[*at].name.clone())
            .collect();
        if !unmatched.is_empty() {
            unstored.push(Unstored {
                fold: fold.name.clone(),
                unmatched,
            });
            continue;
        }

        let key: Vec<KeyPart> = key.into_iter().map(|(_, part)| part).collect();
        let column = Column {
            name: snake_case(&fold.name),
            fold: fold.name.clone(),
            ty: fold.ret.clone(),
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

    tables.sort_by(|a, b| parts(a).cmp(&parts(b)));
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
/// `Expr::Wrap` is deliberately not peeled: it is only ever an operand of the comparison
/// that made it (`Builder::wrap`), so it cannot be a filter's value.
fn pins(fold: &Fold, value: ExprId, param: &Param) -> bool {
    let mut id = value;
    // A fold body has no statements and so nothing that narrows, but peeling is free and
    // the house style everywhere else.
    if let Some(Expr::Unwrap(inner)) = fold.exprs.get(id) {
        id = *inner;
    }
    matches!(fold.exprs.get(id), Some(Expr::Load(slot)) if *slot == param.slot)
}

/// A fold's key: its parameters, named and ordered for a table rather than for a call,
/// each paired with the position it came from in `fold.params`.
fn key_of(program: &Program, fold: &Fold) -> Vec<(usize, KeyPart)> {
    let mut sorted: Vec<(u8, Vec<String>, usize, KeyPart)> = fold
        .params
        .iter()
        .enumerate()
        .map(|(at, param)| match subject_of(&param.ty) {
            Some(subject) => (
                0,
                path(program, subject),
                at,
                KeyPart {
                    name: snake_case(subject),
                    ty: param.ty.clone(),
                    subject: Some(subject.clone()),
                },
            ),
            None => {
                let name = plain_name(&param.name);
                (
                    1,
                    vec![name.clone()],
                    at,
                    KeyPart {
                        name,
                        ty: param.ty.clone(),
                        subject: None,
                    },
                )
            }
        })
        .collect();
    // Stable, so two parts that sort the same keep the order the fold declared them in,
    // which is the order the `_1`/`_2` suffixes below count.
    sorted.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));

    let mut key: Vec<(usize, KeyPart)> = sorted
        .into_iter()
        .map(|(_, _, at, part)| (at, part))
        .collect();
    // Two parts of one key can land on one name, either by repeating a subject or by
    // spelling it two ways. Neither is a mistake, so they are told apart by position.
    let bases: Vec<String> = key.iter().map(|(_, part)| part.name.clone()).collect();
    for (at, (_, part)) in key.iter_mut().enumerate() {
        if bases.iter().filter(|base| *base == &bases[at]).count() > 1 {
            let nth = bases[..at]
                .iter()
                .filter(|base| *base == &bases[at])
                .count()
                + 1;
            part.name = format!("{}_{nth}", part.name);
        }
    }
    key
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
fn subject_of(ty: &Type) -> Option<&Ident> {
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
