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
//! A named fold is spliced the same way, and differs in exactly one statement: a guard
//! discards what it folded, and a fold assigns it into the caller's local. A fold's own
//! body is arms only, so it names nothing and is always a leaf.
//!
//! `docs/guards.md` and `docs/folds.md` are the contract.

use std::collections::{HashMap, HashSet};
use std::mem;

use crate::ir::{
    Arm, Bind, Command, Expr, ExprId, Exprs, Filter, Fold, FoldCall, FoldVar, Guard, GuardCall,
    Ident, Iter, Param, Program, Return, Slice, Slot, Span, Stage, Stmt, Update,
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
    // A fold is always a leaf: its body is arms only, so there is nowhere in one to name
    // a guard or another fold, and nothing to splice into it first. That is also why
    // there is no fold cycle to check for.
    //
    // Taken out of the program and put back below, rather than cloned. The borrow checker
    // is the only reason a map is needed at all, and cloning a `Fold` copies its whole
    // expression arena: a program with sixty named folds and no reads at all was copying
    // sixty arenas on every `hek check`.
    let declared = mem::take(&mut program.folds);
    let folds: HashMap<&str, &Fold> = declared
        .iter()
        .map(|fold| (fold.name.as_str(), fold))
        .collect();

    let mut ready: HashMap<Ident, Guard> = HashMap::new();
    for name in order(program) {
        let Some(position) = program.guards.iter().position(|one| one.name == name) else {
            continue;
        };
        let mut guard = program.guards[position].clone();
        let mut grown = Grown::default();
        for named in source_order(&guard.calls.clone(), &guard.reads.clone()) {
            match named {
                Named::Guard(call) => {
                    let Some(callee) = ready.get(&call.guard) else {
                        continue;
                    };
                    at(
                        &mut guard_site(&mut guard, call.at_stage),
                        &borrowed_guard(callee),
                        &placed(call),
                        &mut grown,
                    );
                }
                Named::Fold(call) => {
                    let Some(callee) = folds.get(call.fold.as_str()) else {
                        continue;
                    };
                    at(
                        &mut guard_site(&mut guard, call.at_stage),
                        &borrowed_fold(callee),
                        &fold_placed(callee, call),
                        &mut grown,
                    );
                }
            }
        }
        program.guards[position] = guard.clone();
        ready.insert(guard.name.clone(), guard);
    }

    for command in &mut program.commands {
        let mut grown: HashMap<usize, Grown> = HashMap::new();
        for named in source_order(&command.calls.clone(), &command.reads.clone()) {
            match named {
                Named::Guard(call) => {
                    let Some(callee) = ready.get(&call.guard) else {
                        continue;
                    };
                    let grown = grown.entry(call.at_stage).or_default();
                    let Some(mut site) = command_site(command, call.at_stage) else {
                        continue;
                    };
                    at(&mut site, &borrowed_guard(callee), &placed(call), grown);
                }
                Named::Fold(call) => {
                    let Some(callee) = folds.get(call.fold.as_str()) else {
                        continue;
                    };
                    let grown = grown.entry(call.at_stage).or_default();
                    let Some(mut site) = command_site(command, call.at_stage) else {
                        continue;
                    };
                    at(
                        &mut site,
                        &borrowed_fold(callee),
                        &fold_placed(callee, call),
                        grown,
                    );
                }
            }
        }
    }

    // Effect arms. Nothing spliced into one before this, because an arm has no `guard`,
    // and 178 of the 425 folds in the port that motivated the construct are in one.
    for effect in &mut program.effects {
        for arm in &mut effect.arms {
            let mut grown: HashMap<usize, Grown> = HashMap::new();
            for call in arm.reads.clone() {
                let Some(callee) = folds.get(call.fold.as_str()) else {
                    continue;
                };
                let grown = grown.entry(call.at_stage).or_default();
                let Some(mut site) = arm_site(arm, call.at_stage) else {
                    continue;
                };
                at(
                    &mut site,
                    &borrowed_fold(callee),
                    &fold_placed(callee, &call),
                    grown,
                );
            }
        }
    }

    program.folds = declared;
}

/// One thing a declaration names: a guard, or a fold.
enum Named<'a> {
    Guard(&'a GuardCall),
    Fold(&'a FoldCall),
}

/// The two call lists merged back into the order the author wrote them in.
///
/// Load-bearing rather than tidy. [`insert`] places each splice at its recorded position
/// plus how far earlier splices pushed it, which is the author's position only if the
/// calls arrive in the author's order. Walking the guards and then the folds would put
/// every fold's slices ahead of a guard written above it, and the append condition would
/// then name them in an order nothing on the page explains.
///
/// Keyed on the span's start, because a declaration's body is one module and line and
/// column are module-relative (`docs/modules.md`). Comparing `at_slice` would not do: a
/// guard and a fold written next to each other record the same one.
fn source_order<'a>(calls: &'a [GuardCall], reads: &'a [FoldCall]) -> Vec<Named<'a>> {
    let mut out: Vec<Named<'a>> = calls
        .iter()
        .map(Named::Guard)
        .chain(reads.iter().map(Named::Fold))
        .collect();
    out.sort_by_key(|named| match named {
        Named::Guard(call) => call.span.start,
        Named::Fold(call) => call.span.start,
    });
    out
}

/// How far the splices already done have pushed the caller's own declarations along, so
/// the next guard still lands where its author wrote it.
///
/// **Per stage, not per caller.** Each stage is its own slice list, fold list and pair of
/// statement runs, so growth in one says nothing about where the next lands in another.
/// Sharing one counter across a command with declarations in two stages placed the second
/// stage's splices past the statements that were already there, and the caller then read a
/// slot the splice assigned after it. A guard can reach this too, and rarely does, because
/// `docs/guards.md` rule 3 puts guards at the top where there is only one stage.
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

/// A command's destination: the stage the author wrote the call in. The clamp is the same
/// defence [`insert`] makes: a caller whose declarations were rejected has fewer stages
/// than the count recorded at parse time, and a panic here would report a `guard` for
/// someone else's mistake.
fn command_site(command: &mut Command, at_stage: usize) -> Option<Site<'_>> {
    let stage = command.stages.get_mut(at_stage)?;
    Some(Site {
        exprs: &mut command.exprs,
        frame: &mut command.frame,
        stage,
    })
}

/// An effect arm's. An arm has no `guard` (`docs/effects.md` rule 1), so only a fold ever
/// lands here, but it stages exactly as a command does and the clamp is needed for the
/// same reason.
fn arm_site(arm: &mut Arm, at_stage: usize) -> Option<Site<'_>> {
    let stage = arm.stages.get_mut(at_stage)?;
    Some(Site {
        exprs: &mut arm.exprs,
        frame: &mut arm.frame,
        stage,
    })
}

/// A guard's. It is one read, so it has exactly one stage and every call in it names that
/// one; `docs/guards.md` rule 6 has why.
fn guard_site(guard: &mut Guard, at_stage: usize) -> Site<'_> {
    debug_assert_eq!(at_stage, 0, "a guard is one stage");
    Site {
        exprs: &mut guard.exprs,
        frame: &mut guard.frame,
        stage: &mut guard.stage,
    }
}

/// What a splice copies, borrowed from either kind of callee. As far as this module is
/// concerned the two are the same body; [`Placed::bind`] is the whole difference.
struct Copied<'a> {
    params: &'a [Param],
    frame: usize,
    exprs: &'a Exprs,
    stage: &'a Stage,
}

fn borrowed_guard(guard: &Guard) -> Copied<'_> {
    Copied {
        params: &guard.params,
        frame: guard.frame,
        exprs: &guard.exprs,
        stage: &guard.stage,
    }
}

fn borrowed_fold(fold: &Fold) -> Copied<'_> {
    Copied {
        params: &fold.params,
        frame: fold.frame,
        exprs: &fold.exprs,
        stage: &fold.stage,
    }
}

/// Where one splice lands and what it is handed.
struct Placed<'a> {
    args: &'a [(Ident, ExprId)],
    at_slice: usize,
    at_fold: usize,
    /// For a fold: the callee's accumulator, the caller's local, and the span to blame.
    /// A guard folds and discards, so it has none.
    bind: Option<(Slot, Slot, Span)>,
}

fn placed(call: &GuardCall) -> Placed<'_> {
    Placed {
        args: &call.args,
        at_slice: call.at_slice,
        at_fold: call.at_fold,
        bind: None,
    }
}

fn fold_placed<'a>(callee: &Fold, call: &'a FoldCall) -> Placed<'a> {
    Placed {
        args: &call.args,
        at_slice: call.at_slice,
        at_fold: call.at_fold,
        bind: Some((callee.slot, call.dest, call.span)),
    }
}

/// Splices `callee` in where the author wrote the `guard`, and records how far that
/// pushed everything after it. Slices, states and statements all land in source order,
/// so a reader of the append condition sees the guards in the order they decide.
fn at(site: &mut Site<'_>, callee: &Copied<'_>, call: &Placed<'_>, grown: &mut Grown) {
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
    for param in callee.params {
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
    // The one statement a fold adds, and the whole of what makes it a fold rather than a
    // guard. It lands in `post`, which runs at step 2.6 *after* the stage folds at 2.5, so
    // the local is assigned what the arms left rather than the seed. The caller's slot is
    // the caller's and does not shift; the accumulator's is the callee's and does.
    if let Some((acc, dest, span)) = call.bind {
        let value = site.exprs.push(Expr::Load(shift_slot(acc, slot_off)), span);
        moved.push(Stmt::Assign { slot: dest, value });
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
        // A secret read holds neither an `ExprId` nor a `Slot`: it is a name, and a
        // name does not move when a guard's frame does.
        Expr::Lit(_) | Expr::Invalid | Expr::Secret { .. } => {}
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
        Expr::Record { ty: _, fields } | Expr::Event { path: _, fields } => {
            fields.iter_mut().for_each(|(_, value)| one(value));
        }
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
        Expr::Wrap { value, inner: _ } => one(value),
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
