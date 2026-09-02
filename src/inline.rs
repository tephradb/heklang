//! Splicing a guard into what names it.
//!
//! A guard is not called at runtime. It is copied into the command that guards it,
//! before the interpreter ever sees either, so a command reaches the fold with one
//! arena, one frame, one slice list and one append condition, exactly as if the folds
//! had been written inline. That is what keeps a command's boundary one read of the log
//! rather than one per guard, and it is why `src/interp.rs` knows nothing about guards.
//!
//! The whole transform is one offset. A guard's expressions move to the end of the
//! caller's arena and its slots to the end of the caller's frame, so every `ExprId` and
//! every `Slot` inside the copy shifts by a constant. Arguments become assignments above
//! the stage the guard was written in, onto the shifted parameter slots, which the caller
//! evaluates before it resolves that stage's filters (`docs/commands.md`).
//!
//! `docs/guards.md` is the contract.

use std::collections::{HashMap, HashSet};

use crate::ir::{
    Bind, Command, Expr, ExprId, Exprs, Filter, FoldVar, Guard, GuardCall, Ident, Iter, Program,
    Return, Slice, Slot, Stage, Stmt, Update,
};

/// The guards a name reaches through, first to last, when they form a cycle. `None`
/// when they do not. Depth-first with a path stack, so the answer names the loop rather
/// than reporting that one exists.
pub fn cycle(program: &Program) -> Option<Vec<Ident>> {
    let mut done: HashSet<&str> = HashSet::new();
    for guard in &program.guards {
        let mut path: Vec<Ident> = Vec::new();
        if let Some(found) = walk(program, &guard.name, &mut path, &mut done) {
            return Some(found);
        }
    }
    None
}

fn walk<'a>(
    program: &'a Program,
    name: &'a str,
    path: &mut Vec<Ident>,
    done: &mut HashSet<&'a str>,
) -> Option<Vec<Ident>> {
    if let Some(at) = path.iter().position(|seen| seen == name) {
        let mut found = path[at..].to_vec();
        found.push(name.to_string());
        return Some(found);
    }
    if done.contains(name) {
        return None;
    }
    let guard = program.guard(name)?;
    path.push(name.to_string());
    for call in &guard.calls {
        if let Some(found) = walk(program, &call.guard, path, done) {
            return Some(found);
        }
    }
    path.pop();
    done.insert(&guard.name);
    None
}

/// Splices every guard into every guard and command that names one. Guards are done
/// first and in dependency order, so each is self-contained by the time a command takes
/// a copy of it, and a chain three deep costs the same walk as a chain one deep.
///
/// The caller has already rejected a cycle, so the order below terminates.
pub fn splice(program: &mut Program) {
    let mut ready: HashMap<Ident, Guard> = HashMap::new();
    for name in order(program) {
        let Some(position) = program.guards.iter().position(|one| one.name == name) else {
            continue;
        };
        let mut guard = program.guards[position].clone();
        let mut grown = Grown::default();
        for call in guard.calls.clone() {
            let Some(callee) = ready.get(&call.guard) else {
                continue;
            };
            into_guard(&mut guard, callee, &call, &mut grown);
        }
        program.guards[position] = guard.clone();
        ready.insert(guard.name.clone(), guard);
    }

    for command in &mut program.commands {
        let mut grown = Grown::default();
        for call in command.calls.clone() {
            let Some(callee) = ready.get(&call.guard) else {
                continue;
            };
            into_command(command, callee, &call, &mut grown);
        }
    }
}

/// How far the splices already done have pushed the caller's own declarations along, so
/// the next guard still lands where its author wrote it.
#[derive(Default)]
struct Grown {
    slices: usize,
    folds: usize,
    body: usize,
}

/// Guards deepest first. A guard whose own guards are already spliced is a guard that
/// can be spliced whole, which is what makes composition cost one copy rather than one
/// per level.
fn order(program: &Program) -> Vec<Ident> {
    let mut out: Vec<Ident> = Vec::new();
    let mut seen: HashSet<Ident> = HashSet::new();
    for guard in &program.guards {
        visit(program, &guard.name, &mut seen, &mut out);
    }
    out
}

fn visit(program: &Program, name: &str, seen: &mut HashSet<Ident>, out: &mut Vec<Ident>) {
    if seen.contains(name) {
        return;
    }
    seen.insert(name.to_string());
    if let Some(guard) = program.guard(name) {
        for call in &guard.calls {
            visit(program, &call.guard, seen, out);
        }
    }
    out.push(name.to_string());
}

/// What one splice needs from its destination, so a command and a guard share the code
/// that does it: the arena and frame the callee's expressions move into, and the one
/// stage its declarations join.
struct Site<'a> {
    exprs: &'a mut Exprs,
    frame: &'a mut usize,
    stage: &'a mut Stage,
}

fn into_command(command: &mut Command, callee: &Guard, call: &GuardCall, grown: &mut Grown) {
    // A guard joins the stage the author wrote it in. The clamp is the same defence
    // `insert` makes below: a caller whose declarations were rejected has fewer stages
    // than the count recorded at parse time, and a panic here would report a `guard`
    // for someone else's mistake.
    let Some(stage) = command.stages.get_mut(call.at_stage) else {
        return;
    };
    let mut site = Site {
        exprs: &mut command.exprs,
        frame: &mut command.frame,
        stage,
    };
    at(&mut site, callee, call, grown);
}

fn into_guard(guard: &mut Guard, callee: &Guard, call: &GuardCall, grown: &mut Grown) {
    // A guard is one read, so it has exactly one stage and every call in it names that
    // one. `docs/guards.md` rule 6 has why.
    debug_assert_eq!(call.at_stage, 0, "a guard is one stage");
    let mut site = Site {
        exprs: &mut guard.exprs,
        frame: &mut guard.frame,
        stage: &mut guard.stage,
    };
    at(&mut site, callee, call, grown);
}

/// Splices `callee` in where the author wrote the `guard`, and records how far that
/// pushed everything after it. Slices, states and statements all land in source order,
/// so a reader of the append condition sees the guards in the order they decide.
fn at(site: &mut Site<'_>, callee: &Guard, call: &GuardCall, grown: &mut Grown) {
    let expr_off = site.exprs.len();
    let slot_off = *site.frame as u32;

    for (expr, span) in callee.exprs.entries() {
        let mut moved = expr.clone();
        shift_expr(&mut moved, expr_off, slot_off);
        site.exprs.push(moved, span);
    }
    *site.frame += callee.frame;

    // An argument first, because the statements above the guard's declarations and its
    // filters both read the parameter it fills. The argument is already an expression in
    // this arena, so it is the one thing here that does not shift. Both land at the end
    // of `pre`, which is where the author wrote the `guard`: a declaration run closes
    // `pre` before it opens, so nothing of the caller's can follow them.
    for param in &callee.params {
        let Some((_, value)) = call.args.iter().find(|(name, _)| name == &param.name) else {
            continue;
        };
        site.stage.pre.push(Stmt::Assign {
            slot: shift_slot(param.slot, slot_off),
            value: *value,
        });
    }
    for stmt in &callee.stage.pre {
        let mut moved = stmt.clone();
        shift_stmt(&mut moved, expr_off, slot_off);
        site.stage.pre.push(moved);
    }

    let slices: Vec<Slice> = callee
        .stage
        .slices
        .iter()
        .map(|slice| Slice {
            event: slice.event.clone(),
            filters: slice
                .filters
                .iter()
                .map(|filter| Filter {
                    field: filter.field.clone(),
                    value: shift_expr_id(filter.value, expr_off),
                })
                .collect(),
            binds: slice
                .binds
                .iter()
                .map(|b| shift_bind(b, slot_off))
                .collect(),
            updates: slice
                .updates
                .iter()
                .map(|update| Update {
                    slot: shift_slot(update.slot, slot_off),
                    value: shift_expr_id(update.value, expr_off),
                    ty: update.ty.clone(),
                })
                .collect(),
        })
        .collect();
    grown.slices += insert(&mut site.stage.slices, call.at_slice + grown.slices, slices);

    let folds: Vec<FoldVar> = callee
        .stage
        .folds
        .iter()
        .map(|fold| FoldVar {
            name: fold.name.clone(),
            ty: fold.ty.clone(),
            slot: shift_slot(fold.slot, slot_off),
            init: shift_expr_id(fold.init, expr_off),
        })
        .collect();
    grown.folds += insert(&mut site.stage.folds, call.at_fold + grown.folds, folds);

    // Ahead of the body the author wrote, and after the guards written above this one:
    // a refusal is decided in the order the guards appear, and all of them before the
    // first statement of the body they guard.
    let mut moved: Vec<Stmt> = callee.stage.post.to_vec();
    for stmt in &mut moved {
        shift_stmt(stmt, expr_off, slot_off);
    }
    grown.body += insert(&mut site.stage.post, grown.body, moved);
}

/// Puts `what` into `into` at `at`, and answers how many that was. Clamped rather than
/// trusted: a caller whose declarations were rejected has fewer than the count recorded
/// at parse time, and a panic there would report a `guard` for someone else's mistake.
fn insert<T>(into: &mut Vec<T>, at: usize, what: Vec<T>) -> usize {
    let count = what.len();
    let tail = into.split_off(at.min(into.len()));
    into.extend(what);
    into.extend(tail);
    count
}

fn shift_slot(slot: Slot, by: u32) -> Slot {
    Slot(slot.0 + by)
}

fn shift_expr_id(id: ExprId, by: u32) -> ExprId {
    ExprId(id.0 + by)
}

fn shift_bind(bind: &Bind, by: u32) -> Bind {
    Bind {
        field: bind.field.clone(),
        slot: shift_slot(bind.slot, by),
    }
}

fn shift_iter(iter: &mut Iter, expr_off: u32, slot_off: u32) {
    if let Some(index) = &mut iter.index {
        *index = shift_slot(*index, slot_off);
    }
    iter.item = shift_slot(iter.item, slot_off);
    iter.over = shift_expr_id(iter.over, expr_off);
}

/// Every variant, with no wildcard: a node that gains an `ExprId` or a `Slot` later has
/// to be handled here, and the compiler is what says so.
fn shift_expr(expr: &mut Expr, expr_off: u32, slot_off: u32) {
    let one = |id: &mut ExprId| *id = shift_expr_id(*id, expr_off);
    match expr {
        Expr::Lit(_) | Expr::Invalid => {}
        Expr::Load(slot) => *slot = shift_slot(*slot, slot_off),
        Expr::Unary { op: _, operand } => one(operand),
        Expr::Binary { op: _, lhs, rhs } => {
            one(lhs);
            one(rhs);
        }
        Expr::Method {
            receiver,
            method: _,
            args,
        } => {
            one(receiver);
            args.iter_mut().for_each(one);
        }
        Expr::If {
            cond,
            then,
            otherwise,
        } => {
            one(cond);
            one(then);
            one(otherwise);
        }
        Expr::Field { receiver, name: _ } => one(receiver),
        Expr::Object(fields) => fields.iter_mut().for_each(|(_, value)| one(value)),
        Expr::Interp(parts) => parts.iter_mut().for_each(one),
        Expr::List { items, inner: _ } => items.iter_mut().for_each(one),
        Expr::Record { ty: _, fields } => fields.iter_mut().for_each(|(_, value)| one(value)),
        Expr::CallFn {
            function: _,
            scope: _,
            args,
        } => args.iter_mut().for_each(one),
        Expr::Comp {
            iter,
            cond,
            yields,
            inner: _,
        } => {
            shift_iter(iter, expr_off, slot_off);
            if let Some(cond) = cond {
                one(cond);
            }
            one(yields);
        }
        Expr::Call { builtin: _, args } => args.iter_mut().for_each(one),
        Expr::Invoke { command: _, args } => args.iter_mut().for_each(|(_, value)| one(value)),
        Expr::Unwrap(inner) => one(inner),
        Expr::Reveal { value, .. } => one(value),
        Expr::Refusal { code, message } => {
            if let Some(code) = code {
                one(code);
            }
            one(message);
        }
    }
}

fn shift_stmt(stmt: &mut Stmt, expr_off: u32, slot_off: u32) {
    let one = |id: &mut ExprId| *id = shift_expr_id(*id, expr_off);
    match stmt {
        Stmt::Assign { slot, value } => {
            *slot = shift_slot(*slot, slot_off);
            one(value);
        }
        Stmt::If {
            cond,
            then,
            otherwise,
        } => {
            one(cond);
            for stmt in then.iter_mut().chain(otherwise.iter_mut()) {
                shift_stmt(stmt, expr_off, slot_off);
            }
        }
        Stmt::Emit {
            event: _,
            fields,
            span: _,
        }
        | Stmt::Put {
            entity: _,
            fields,
            span: _,
        } => fields.iter_mut().for_each(|(_, value)| one(value)),
        Stmt::Patch {
            entity: _,
            key,
            absent: _,
            loads,
            fields,
            span: _,
        } => {
            one(key);
            for load in loads.iter_mut() {
                *load = shift_bind(load, slot_off);
            }
            fields.iter_mut().for_each(|(_, value)| one(value));
        }
        Stmt::Delete { entity: _, key } => one(key),
        Stmt::Fail { message, span: _ } | Stmt::Log { message } => one(message),
        Stmt::Erase {
            subject: _,
            value,
            span: _,
        } => one(value),
        Stmt::For { iter, body } => {
            shift_iter(iter, expr_off, slot_off);
            for stmt in body.iter_mut() {
                shift_stmt(stmt, expr_off, slot_off);
            }
        }
        Stmt::Discard(value) => one(value),
        Stmt::Call {
            function: _,
            scope: _,
            args,
            span: _,
        } => args.iter_mut().for_each(one),
        Stmt::Return(ret) => match ret {
            Return::Ok => {}
            Return::Invalid(value) | Return::Value(value) | Return::Outcome(value) => one(value),
            Return::Reject { code, message } => {
                one(code);
                one(message);
            }
        },
    }
}
