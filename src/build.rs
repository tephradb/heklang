use std::collections::{HashMap, HashSet};
use std::mem;

use crate::ir::{
    Arm, BinOp, Bind, Command, EnvBind, EnvField, EventPath, Expr, ExprId, Exprs, Filter, Function,
    Guard, GuardCall, Handler, Ident, Literal, Number, Param, Slice, SliceId, Slot, Span, Stage,
    StateVar, Stmt, Type, UnOp, Update,
};
use crate::scaled::Rounding;

pub struct Builder {
    name: Ident,
    module: Option<Ident>,
    params: Vec<Param>,
    exprs: Exprs,
    /// The stages closed so far, and the one still open. A declaration joins the open
    /// stage; a statement written after one closes it, so the next declaration starts a
    /// stage of its own and reads the log again.
    stages: Vec<Stage>,
    open: Stage,
    /// The states the open stage declares. A seed, a filter or a guard argument may read
    /// a state an *earlier* stage folded, and may not read one of these, which have not
    /// folded when this stage resolves.
    pending: HashSet<Slot>,
    frame: u32,
    span: Span,
    slot_types: Vec<Option<Type>>,
    scopes: Vec<HashMap<Ident, Slot>>,
    binds: Vec<Bind>,
    envelope: Vec<EnvBind>,
    now: Option<Slot>,
    /// The slots a branch has proved present. A load of one lowers to an `Unwrap`.
    narrowed: HashSet<Slot>,
    calls: Vec<GuardCall>,
}

impl Builder {
    pub fn new(name: impl Into<Ident>) -> Self {
        Self {
            name: name.into(),
            module: None,
            params: Vec::new(),
            exprs: Exprs::default(),
            stages: Vec::new(),
            open: Stage::default(),
            pending: HashSet::new(),
            frame: 0,
            span: Span::default(),
            slot_types: Vec::new(),
            scopes: vec![HashMap::new()],
            binds: Vec::new(),
            envelope: Vec::new(),
            now: None,
            narrowed: HashSet::new(),
            calls: Vec::new(),
        }
    }

    pub fn in_module(&mut self, module: Option<&str>) {
        self.module = module.map(str::to_string);
    }

    pub fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    pub fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    pub fn alloc(&mut self, name: impl Into<Ident>, ty: Option<Type>) -> Slot {
        let slot = Slot(self.frame);
        self.frame += 1;
        self.slot_types.push(ty);
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.into(), slot);
        }
        slot
    }

    pub fn slot_type(&self, slot: Slot) -> Option<&Type> {
        self.slot_types
            .get(slot.0 as usize)
            .and_then(Option::as_ref)
    }

    pub fn exprs(&self) -> &Exprs {
        &self.exprs
    }

    pub fn patch(&mut self, id: ExprId, expr: Expr) {
        self.exprs.patch(id, expr);
    }

    pub fn lookup(&self, name: &str) -> Option<Slot> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).copied())
    }

    pub fn param(&mut self, name: &str, ty: Type) -> Slot {
        let slot = self.alloc(name, Some(ty.clone()));
        self.params.push(Param {
            name: name.to_string(),
            ty,
            slot,
        });
        slot
    }

    pub fn opt_param(&mut self, name: &str, inner: Type) -> Slot {
        self.param(name, Type::opt(inner))
    }

    pub fn state(&mut self, name: &str, ty: Type, init: ExprId) -> Slot {
        let slot = self.alloc(name, Some(ty.clone()));
        self.pending.insert(slot);
        self.open.states.push(StateVar {
            name: name.to_string(),
            ty,
            slot,
            init,
        });
        slot
    }

    /// Across every stage, open and closed: a slot is a `state` wherever it was folded,
    /// which is what `seal_state` and a diagnostic naming one both want. Whether it has
    /// folded *yet* is `pending`, and a different question.
    pub fn state_of(&self, slot: Slot) -> Option<&StateVar> {
        self.stages
            .iter()
            .chain([&self.open])
            .flat_map(|stage| &stage.states)
            .find(|state| state.slot == slot)
    }

    /// Rule 12: recorded once the whole fold is parsed, because it is a property of
    /// every arm agreeing rather than of any one of them. The seal lands on the
    /// declared type, the same way it lands on an entity column.
    pub fn seal_state(&mut self, slot: Slot, subject: Ident) {
        if let Some(state) = self
            .stages
            .iter_mut()
            .chain([&mut self.open])
            .flat_map(|stage| &mut stage.states)
            .find(|state| state.slot == slot)
        {
            state.ty = seal(state.ty.clone(), subject.clone());
        }
        if let Some(ty) = self.slot_types.get_mut(slot.0 as usize)
            && let Some(ty) = ty.as_mut()
        {
            *ty = seal(ty.clone(), subject);
        }
    }

    pub fn bind(&mut self, field: &str, ty: Option<Type>) -> Bind {
        let slot = self.alloc(field, ty);
        Bind {
            field: field.to_string(),
            slot,
        }
    }

    /// A slice binding the author did not write. Bound from `field` like any other, but
    /// allocated under a name no source token can spell, so folding a subject id the
    /// author did not destructure cannot shadow a trigger binding of the same name.
    pub fn hidden_bind(&mut self, field: &str, ty: Option<Type>) -> Bind {
        let slot = self.alloc(format!("@fold:{field}"), ty);
        Bind {
            field: field.to_string(),
            slot,
        }
    }

    pub fn slice(
        &mut self,
        event: EventPath,
        filters: Vec<Filter>,
        binds: Vec<Bind>,
        updates: Vec<Update>,
    ) -> SliceId {
        let id = SliceId(self.open.slices.len() as u32);
        self.open.slices.push(Slice {
            event,
            filters,
            binds,
            updates,
        });
        id
    }

    pub fn guard(&mut self, event: EventPath, filters: Vec<Filter>) -> SliceId {
        self.slice(event, filters, Vec::new(), Vec::new())
    }

    /// The first filter in the open stage reading a `state` that stage itself declares,
    /// with the span to point at. Filters resolve before the stage folds, so such a
    /// filter can never be satisfied and is a mistake rather than a slow path. A filter
    /// naming a state an *earlier* stage folded is fine, and is the point of staging.
    pub fn filter_past_fold(&self) -> Option<(Ident, Span)> {
        self.open
            .slices
            .iter()
            .flat_map(|slice| &slice.filters)
            .find_map(|filter| {
                self.reads_pending(filter.value)
                    .then(|| (filter.field.clone(), self.exprs.span(filter.value)))
            })
    }

    /// Whether `value` reads a slot the prologue has not filled by the time it runs: a
    /// `state`, which the fold sets at step 6, or a `let` already left in the body
    /// Whether `value` reads a `state` the open stage has not folded yet.
    pub fn reads_pending(&self, value: ExprId) -> bool {
        self.unset_read(value).is_some()
    }

    /// The unfolded `state` an expression reads, with the span of the read. A seed asks
    /// this, because a seed is evaluated before its own stage folds and so sees another
    /// `state`'s seed rather than what it folds to: an answer that is wrong rather than
    /// late. A state an earlier stage folded is not one of these.
    pub fn state_read(&self, value: ExprId) -> Option<(Ident, Span)> {
        let (slot, at) = self.unset_read(value)?;
        let state = self.state_of(slot)?;
        Some((state.name.clone(), self.exprs.span(at)))
    }

    /// The first read of a `state` the open stage declares, as the slot and the load
    /// that reads it. Both callers above want a different half of this, and a second
    /// walk over the same arena is the kind of thing that drifts.
    fn unset_read(&self, value: ExprId) -> Option<(Slot, ExprId)> {
        let mut stack = vec![value];
        while let Some(id) = stack.pop() {
            let Some(expr) = self.exprs.get(id) else {
                continue;
            };
            match expr {
                Expr::Load(slot) => {
                    if self.pending.contains(slot) {
                        return Some((*slot, id));
                    }
                }
                Expr::Lit(_) | Expr::Invalid => {}
                Expr::Unary { operand, .. } => stack.push(*operand),
                Expr::Unwrap(inner) => stack.push(*inner),
                Expr::Reveal { value, .. } => stack.push(*value),
                Expr::Refusal { code, message } => {
                    stack.extend(code);
                    stack.push(*message);
                }
                Expr::Binary { lhs, rhs, .. } => stack.extend([*lhs, *rhs]),
                Expr::Field { receiver, .. } => stack.push(*receiver),
                Expr::Method { receiver, args, .. } => {
                    stack.push(*receiver);
                    stack.extend(args);
                }
                Expr::If {
                    cond,
                    then,
                    otherwise,
                } => stack.extend([*cond, *then, *otherwise]),
                Expr::Object(fields)
                | Expr::Record { fields, .. }
                | Expr::Invoke { args: fields, .. } => {
                    stack.extend(fields.iter().map(|(_, id)| *id));
                }
                Expr::Interp(parts) => stack.extend(parts),
                Expr::List { items, .. } => stack.extend(items),
                Expr::CallFn { args, .. } | Expr::Call { args, .. } => stack.extend(args),
                Expr::Comp {
                    iter, cond, yields, ..
                } => {
                    stack.push(iter.over);
                    stack.extend(cond);
                    stack.push(*yields);
                }
            }
        }
        None
    }

    pub fn at(&mut self, span: Span) {
        self.span = span;
    }

    /// Give a node the extent it was parsed from. The cursor `expr` stamps comes from a
    /// token the parser held before it knew what it was building, so only the production
    /// that finishes can say how far it ran.
    pub fn respan(&mut self, id: ExprId, span: Span) -> ExprId {
        self.exprs.respan(id, span);
        id
    }

    pub fn expr(&mut self, expr: Expr) -> ExprId {
        self.exprs.push(expr, self.span)
    }

    pub fn load(&mut self, name: &str) -> ExprId {
        let slot = self
            .lookup(name)
            .unwrap_or_else(|| panic!("`{name}` is not in scope"));
        let id = self.expr(Expr::Load(slot));
        if self.narrowed.contains(&slot) {
            return self.expr(Expr::Unwrap(id));
        }
        id
    }

    /// Narrows a slot to the type a branch proved it holds, returning what it was so
    /// the caller can put it back. A slot that is not an optional, or one already
    /// narrowed, is left alone: nesting the same test twice is not two narrowings.
    pub fn narrow(&mut self, slot: Slot) -> Option<Option<Type>> {
        let Some(Type::Opt(inner)) = self.slot_type(slot).cloned() else {
            return None;
        };
        let previous = self.slot_types[slot.0 as usize].replace(*inner);
        self.narrowed.insert(slot);
        Some(previous)
    }

    pub fn widen(&mut self, slot: Slot, previous: Option<Type>) {
        self.slot_types[slot.0 as usize] = previous;
        self.narrowed.remove(&slot);
    }

    pub fn read(&mut self, slot: Slot) -> ExprId {
        self.expr(Expr::Load(slot))
    }

    pub fn lit(&mut self, lit: Literal) -> ExprId {
        self.expr(Expr::Lit(lit))
    }

    pub fn bool(&mut self, value: bool) -> ExprId {
        self.lit(Literal::Bool(value))
    }

    pub fn int(&mut self, value: i64) -> ExprId {
        self.lit(Literal::Int(value))
    }

    pub fn str(&mut self, value: &str) -> ExprId {
        self.lit(Literal::Str(value.into()))
    }

    pub fn money(&mut self, units: i64, scale: u8) -> ExprId {
        self.lit(Literal::Money { units, scale })
    }

    pub fn rounding(&mut self, mode: Rounding) -> ExprId {
        self.lit(Literal::Rounding(mode))
    }

    pub fn number(&mut self, digits: i128, scale: u8, ty: &Type) -> ExprId {
        let lit = Number::new(digits, scale)
            .resolve(ty)
            .unwrap_or_else(|err| panic!("{err}"));
        self.lit(lit)
    }

    pub fn decimal(&mut self, units: i64, scale: u8) -> ExprId {
        self.lit(Literal::Decimal { units, scale })
    }

    pub fn unary(&mut self, op: UnOp, operand: ExprId) -> ExprId {
        self.expr(Expr::Unary { op, operand })
    }

    pub fn binary(&mut self, op: BinOp, lhs: ExprId, rhs: ExprId) -> ExprId {
        self.expr(Expr::Binary { op, lhs, rhs })
    }

    pub fn method(&mut self, receiver: ExprId, method: &str, args: Vec<ExprId>) -> ExprId {
        self.expr(Expr::Method {
            receiver,
            method: method.to_string(),
            args,
        })
    }

    pub fn if_expr(&mut self, cond: ExprId, then: ExprId, otherwise: ExprId) -> ExprId {
        self.expr(Expr::If {
            cond,
            then,
            otherwise,
        })
    }

    pub fn finish_fn(self, ret: Option<Type>, body: Vec<Stmt>, span: Span) -> Function {
        Function {
            name: self.name,
            module: self.module,
            params: self.params,
            ret,
            frame: self.frame as usize,
            exprs: self.exprs,
            body,
            span,
        }
    }

    /// A statement, placed by where the open stage is: above its declarations while it
    /// has none, below them once it has. This is the whole of what makes a stage a
    /// stage, so nothing else may push onto `pre` or `post`.
    pub fn stmt(&mut self, stmt: Stmt) {
        if self.open.slices.is_empty() && self.open.states.is_empty() {
            self.open.pre.push(stmt);
        } else {
            self.open.post.push(stmt);
        }
    }

    /// Whether a declaration written now would open a new stage, which it does once a
    /// statement has been written below the open stage's declarations. The caller asks
    /// before declaring so a guard can be refused a second read.
    pub fn would_stage(&self) -> bool {
        !self.open.post.is_empty()
    }

    /// Closes the open stage if a declaration written now would start a new one. The
    /// states it declared stop being pending, because from here they have folded.
    pub fn stage_break(&mut self) {
        if !self.would_stage() {
            return;
        }
        self.stages.push(mem::take(&mut self.open));
        self.pending.clear();
    }

    /// Closes the open stage behind the others. Every finisher starts here, so a
    /// declaration always has at least one stage even when it declared nothing at all.
    fn seal(&mut self) {
        self.stages.push(mem::take(&mut self.open));
    }

    /// A test has values and no statements, so it keeps the arena and the frame width
    /// and nothing else.
    pub fn finish_test(self) -> (usize, Exprs) {
        (self.frame as usize, self.exprs)
    }

    pub fn finish(mut self) -> Command {
        self.seal();
        Command {
            name: self.name,
            module: self.module,
            params: self.params,
            frame: self.frame as usize,
            exprs: self.exprs,
            now: self.now,
            stages: self.stages,
            calls: self.calls,
        }
    }

    /// A command's shape minus `now`, which a guard may not pin because it decides from
    /// the log rather than from the clock, and with one stage rather than many, because
    /// a guard is one read. See `docs/guards.md`.
    pub fn finish_guard(mut self, span: Span) -> Guard {
        self.seal();
        debug_assert_eq!(self.stages.len(), 1, "a guard is one stage");
        Guard {
            name: self.name,
            module: self.module,
            params: self.params,
            frame: self.frame as usize,
            exprs: self.exprs,
            stage: self.stages.pop().unwrap_or_default(),
            calls: self.calls,
            span,
        }
    }

    /// One `guard Name { args }`, recorded where it was written so the splice keeps the
    /// order the author put the refusals in.
    pub fn call(&mut self, guard: &str, args: Vec<(Ident, ExprId)>, span: Span) {
        self.calls.push(GuardCall {
            guard: guard.into(),
            args,
            at_stage: self.stages.len(),
            at_slice: self.open.slices.len(),
            at_state: self.open.states.len(),
            span,
        });
    }

    /// Whether anything has been folded yet, in any stage. A guard that folds nothing
    /// is a `fn`.
    pub fn folds_nothing(&self) -> bool {
        self.stages
            .iter()
            .chain([&self.open])
            .all(|stage| stage.slices.is_empty())
    }
}

impl Builder {
    /// A destructured payload field: allocated under its own name, so the body
    /// reaches it bare.
    pub fn destructure(&mut self, field: &str, ty: Option<Type>) -> Slot {
        let bind = self.bind(field, ty);
        let slot = bind.slot;
        self.binds.push(bind);
        slot
    }

    /// A payload field reached through the `as` binding without being destructured.
    /// Allocated under a name no source token can spell, so `e.total` never puts a
    /// bare `total` in scope.
    pub fn payload(&mut self, field: &str, ty: Option<Type>) -> Slot {
        if let Some(bind) = self.binds.iter().find(|bind| bind.field == field) {
            return bind.slot;
        }
        let slot = self.alloc(format!("@{field}"), ty);
        self.binds.push(Bind {
            field: field.to_string(),
            slot,
        });
        slot
    }

    pub fn envelope(&mut self, field: EnvField) -> Slot {
        if let Some(bind) = self.envelope.iter().find(|bind| bind.field == field) {
            return bind.slot;
        }
        let slot = self.alloc(format!("@@{field:?}"), Some(field.ty()));
        self.envelope.push(EnvBind { field, slot });
        slot
    }

    pub fn none(&mut self, inner: Type) -> ExprId {
        self.lit(Literal::None(inner))
    }

    /// Rule 11: one slot for `now()`, however many times the body calls it, filled
    /// before the body runs. That is what makes "pinned once" structural rather than a
    /// promise: two calls are two reads of the same slot.
    pub fn now(&mut self) -> Slot {
        match self.now {
            Some(slot) => slot,
            None => {
                let slot = self.alloc("@@now", Some(Type::Timestamp));
                self.now = Some(slot);
                slot
            }
        }
    }

    pub fn finish_arm(mut self, events: Vec<EventPath>, span: Span) -> Arm {
        self.seal();
        Arm {
            events,
            binds: self.binds,
            envelope: self.envelope,
            frame: self.frame as usize,
            exprs: self.exprs,
            now: self.now,
            stages: self.stages,
            span,
        }
    }

    pub fn finish_handler(self, event: EventPath, body: Vec<Stmt>) -> Handler {
        Handler {
            event,
            binds: self.binds,
            envelope: self.envelope,
            frame: self.frame as usize,
            exprs: self.exprs,
            body,
        }
    }
}

impl Builder {
    /// The payload field a slot was bound from, if any. Used to propagate `@subject`
    /// from an event field into the entity field a handler writes it to.
    pub fn bound_field(&self, slot: Slot) -> Option<&str> {
        self.binds
            .iter()
            .find(|bind| bind.slot == slot)
            .map(|bind| bind.field.as_str())
    }
}

/// `Opt` stays outermost, so an optional subject-bound value is an optional whose
/// content is sealed rather than a sealed optional. See `docs/effects.md` rule 12.
pub fn seal(ty: Type, subject: Ident) -> Type {
    match ty {
        Type::Opt(inner) => Type::opt(seal(*inner, subject)),
        other => Type::sealed(other, subject),
    }
}
