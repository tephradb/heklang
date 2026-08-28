use std::collections::HashMap;

use crate::ir::{
    Arm, Assign, BinOp, Bind, Command, EnvBind, EnvField, EventPath, Expr, ExprId, Exprs, Filter,
    Function, Handler, Ident, Literal, Number, Param, Slice, SliceId, Slot, Span, StateVar, Stmt,
    Type, UnOp, Update,
};
use crate::scaled::Rounding;

pub struct Builder {
    name: Ident,
    module: Option<Ident>,
    params: Vec<Param>,
    exprs: Exprs,
    prologue: Vec<Assign>,
    slices: Vec<Slice>,
    states: Vec<StateVar>,
    frame: u32,
    span: Span,
    slot_types: Vec<Option<Type>>,
    scopes: Vec<HashMap<Ident, Slot>>,
    binds: Vec<Bind>,
    envelope: Vec<EnvBind>,
    now: Option<Slot>,
}

impl Builder {
    pub fn new(name: impl Into<Ident>) -> Self {
        Self {
            name: name.into(),
            module: None,
            params: Vec::new(),
            exprs: Exprs::default(),
            prologue: Vec::new(),
            slices: Vec::new(),
            states: Vec::new(),
            frame: 0,
            span: Span::default(),
            slot_types: Vec::new(),
            scopes: vec![HashMap::new()],
            binds: Vec::new(),
            envelope: Vec::new(),
            now: None,
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
        self.states.push(StateVar {
            name: name.to_string(),
            ty,
            slot,
            init,
        });
        slot
    }

    pub fn bind(&mut self, field: &str, ty: Option<Type>) -> Bind {
        let slot = self.alloc(field, ty);
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
        let id = SliceId(self.slices.len() as u32);
        self.slices.push(Slice {
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

    pub fn hoist(&mut self, name: &str, value: ExprId, ty: Option<Type>) -> Slot {
        let slot = self.alloc(name, ty);
        self.prologue.push(Assign { slot, value });
        slot
    }

    pub fn at(&mut self, span: Span) {
        self.span = span;
    }

    pub fn expr(&mut self, expr: Expr) -> ExprId {
        self.exprs.push(expr, self.span)
    }

    pub fn load(&mut self, name: &str) -> ExprId {
        let slot = self
            .lookup(name)
            .unwrap_or_else(|| panic!("`{name}` is not in scope"));
        self.expr(Expr::Load(slot))
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
        self.lit(Literal::Str(value.to_string()))
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

    pub fn finish_fn(self, ret: Type, body: Vec<Stmt>) -> Function {
        Function {
            name: self.name,
            module: self.module,
            params: self.params,
            ret,
            frame: self.frame as usize,
            exprs: self.exprs,
            body,
        }
    }

    pub fn finish(self, body: Vec<Stmt>) -> Command {
        Command {
            name: self.name,
            module: self.module,
            params: self.params,
            frame: self.frame as usize,
            exprs: self.exprs,
            now: self.now,
            prologue: self.prologue,
            slices: self.slices,
            states: self.states,
            body,
        }
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

    pub fn finish_arm(self, events: Vec<EventPath>, span: Span, body: Vec<Stmt>) -> Arm {
        Arm {
            events,
            binds: self.binds,
            envelope: self.envelope,
            frame: self.frame as usize,
            exprs: self.exprs,
            prologue: self.prologue,
            slices: self.slices,
            states: self.states,
            now: self.now,
            body,
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
