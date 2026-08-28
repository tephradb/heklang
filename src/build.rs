use std::collections::HashMap;

use crate::currency::Currency;
use crate::ir::{
    Assign, BinOp, Bind, Command, EventPath, Expr, ExprId, Exprs, Filter, Ident, Literal, Number,
    Param, Slice, SliceId, Slot, Span, StateVar, Stmt, Type, UnOp, Update,
};
use crate::scaled::Rounding;

pub struct Builder {
    name: Ident,
    params: Vec<Param>,
    exprs: Exprs,
    prologue: Vec<Assign>,
    slices: Vec<Slice>,
    states: Vec<StateVar>,
    frame: u32,
    span: Span,
    slot_types: Vec<Option<Type>>,
    scopes: Vec<HashMap<Ident, Slot>>,
}

impl Builder {
    pub fn new(name: impl Into<Ident>) -> Self {
        Self {
            name: name.into(),
            params: Vec::new(),
            exprs: Exprs::default(),
            prologue: Vec::new(),
            slices: Vec::new(),
            states: Vec::new(),
            frame: 0,
            span: Span::default(),
            slot_types: Vec::new(),
            scopes: vec![HashMap::new()],
        }
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

    pub fn money(&mut self, units: i64) -> ExprId {
        self.lit(Literal::Money(units))
    }

    pub fn rounding(&mut self, mode: Rounding) -> ExprId {
        self.lit(Literal::Rounding(mode))
    }

    pub fn number(&mut self, digits: i128, scale: u8, ty: &Type, currency: &Currency) -> ExprId {
        let lit = Number::new(digits, scale)
            .resolve(ty, currency)
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

    pub fn finish(self, body: Vec<Stmt>) -> Command {
        Command {
            name: self.name,
            params: self.params,
            frame: self.frame as usize,
            exprs: self.exprs,
            prologue: self.prologue,
            slices: self.slices,
            states: self.states,
            body,
        }
    }
}
