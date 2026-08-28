use std::collections::HashMap;

use crate::build::Builder;
use crate::currency::Currency;
use crate::ir::{
    BinOp, Command, EventDef, EventPath, Expr, ExprId, FieldDef, Filter, Literal, Number, Program,
    Return, Span, Stmt, Type, UnOp, Update,
};
use crate::lex::{Keyword, Spanned, Sym, SyntaxError, Token, lex};
use crate::scaled::Rounding;

pub fn parse(source: &str) -> Result<Program, SyntaxError> {
    Parser {
        tokens: lex(source)?,
        pos: 0,
        prologue: false,
        command_end: 0,
    }
    .program()
}

struct Cmd {
    b: Builder,
    defaults: HashMap<ExprId, Number>,
    currency: Currency,
}

struct Parser {
    tokens: Vec<Spanned>,
    pos: usize,
    prologue: bool,
    command_end: usize,
}

impl Parser {
    fn peek(&self) -> &Token {
        self.tokens
            .get(self.pos)
            .map(|spanned| &spanned.token)
            .unwrap_or(&Token::End)
    }

    fn here(&self) -> (u32, u32) {
        self.tokens
            .get(self.pos)
            .map(|spanned| (spanned.line, spanned.col))
            .unwrap_or((0, 0))
    }

    fn span_here(&self) -> Span {
        let (line, col) = self.here();
        Span::new(line, col)
    }

    fn command_end(&self) -> usize {
        let mut depth = 0u32;
        for (index, spanned) in self.tokens.iter().enumerate().skip(self.pos) {
            match &spanned.token {
                Token::Sym(Sym::LBrace) => depth += 1,
                Token::Sym(Sym::RBrace) => {
                    if depth == 0 {
                        return index;
                    }
                    depth -= 1;
                }
                Token::End => return index,
                _ => {}
            }
        }
        self.tokens.len()
    }

    fn later_let(&self, name: &str) -> Option<Span> {
        for (index, spanned) in self
            .tokens
            .iter()
            .enumerate()
            .take(self.command_end)
            .skip(self.pos)
        {
            if let Token::Word(Keyword::Let) = &spanned.token
                && let Some(Spanned {
                    token: Token::Ident(found),
                    line,
                    col,
                }) = self.tokens.get(index + 1)
                && found == name
            {
                return Some(Span::new(*line, *col));
            }
        }
        None
    }

    fn not_in_scope(&self, name: &str, line: u32, col: u32) -> SyntaxError {
        let message = match (self.later_let(name), self.prologue) {
            (Some(span), true) => format!(
                "`{name}` is defined at {span}, below the declarations; \
                 `guard` and `state` run before the body, so they can only use names \
                 bound above them — move that `let` up"
            ),
            (Some(span), false) => {
                format!("`{name}` is not in scope yet; it is defined below at {span}")
            }
            (None, _) => format!("`{name}` is not in scope"),
        };
        SyntaxError::new(message, line, col)
    }

    fn bump(&mut self) -> Spanned {
        let spanned = self.tokens.get(self.pos).cloned().unwrap_or(Spanned {
            token: Token::End,
            line: 0,
            col: 0,
        });
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        spanned
    }

    fn fail<T>(&self, message: impl Into<String>) -> Result<T, SyntaxError> {
        let (line, col) = self.here();
        Err(SyntaxError::new(message, line, col))
    }

    fn at_sym(&self, sym: Sym) -> bool {
        matches!(self.peek(), Token::Sym(found) if *found == sym)
    }

    fn at_word(&self, keyword: Keyword) -> bool {
        matches!(self.peek(), Token::Word(found) if *found == keyword)
    }

    fn eat_sym(&mut self, sym: Sym) -> bool {
        if self.at_sym(sym) {
            self.bump();
            return true;
        }
        false
    }

    fn eat_word(&mut self, keyword: Keyword) -> bool {
        if self.at_word(keyword) {
            self.bump();
            return true;
        }
        false
    }

    fn expect_sym(&mut self, sym: Sym) -> Result<(), SyntaxError> {
        if self.eat_sym(sym) {
            return Ok(());
        }
        self.fail(format!("expected `{}`, found {}", sym.text(), self.peek()))
    }

    fn expect_word(&mut self, keyword: Keyword) -> Result<(), SyntaxError> {
        if self.eat_word(keyword) {
            return Ok(());
        }
        self.fail(format!(
            "expected `{}`, found {}",
            keyword.text(),
            self.peek()
        ))
    }

    fn expect_ident(&mut self) -> Result<String, SyntaxError> {
        match self.peek().clone() {
            Token::Ident(name) => {
                self.bump();
                Ok(name)
            }
            other => self.fail(format!("expected a name, found {other}")),
        }
    }

    fn expect_path(&mut self) -> Result<EventPath, SyntaxError> {
        match self.peek().clone() {
            Token::Path(segments) => {
                self.bump();
                Ok(EventPath::new(segments))
            }
            other => self.fail(format!("expected an event path, found {other}")),
        }
    }

    fn expect_number(&mut self) -> Result<Number, SyntaxError> {
        match self.peek().clone() {
            Token::Number(number) => {
                self.bump();
                Ok(number)
            }
            other => self.fail(format!("expected a number, found {other}")),
        }
    }

    fn program(&mut self) -> Result<Program, SyntaxError> {
        self.expect_word(Keyword::Currency)?;
        let code = self.expect_ident()?;
        let currency = Currency::from_code(&code);

        let items = self.pos;
        let mut events: Vec<EventDef> = Vec::new();
        loop {
            match self.peek() {
                Token::End => break,
                Token::Word(Keyword::Event) => {
                    let event = self.event_decl()?;
                    if events.iter().any(|def| def.path == event.path) {
                        return self.fail(format!("event {} is declared twice", event.path));
                    }
                    events.push(event);
                }
                Token::Word(Keyword::Command) => self.skip_item()?,
                other => return self.fail(format!("expected `event` or `command`, found {other}")),
            }
        }

        self.pos = items;
        let mut commands = Vec::new();
        loop {
            match self.peek() {
                Token::End => break,
                Token::Word(Keyword::Event) => self.skip_item()?,
                Token::Word(Keyword::Command) => {
                    let command = self.command_decl(&events, &currency)?;
                    if commands
                        .iter()
                        .any(|other: &Command| other.name == command.name)
                    {
                        return self.fail(format!("command `{}` is declared twice", command.name));
                    }
                    commands.push(command);
                }
                other => return self.fail(format!("expected `event` or `command`, found {other}")),
            }
        }

        Ok(Program {
            currency,
            events,
            commands,
        })
    }

    fn skip_item(&mut self) -> Result<(), SyntaxError> {
        self.bump();
        while !self.at_sym(Sym::LBrace) {
            if matches!(self.peek(), Token::End) {
                return self.fail("expected `{`, found end of file");
            }
            self.bump();
        }

        let mut depth = 0u32;
        loop {
            match self.peek() {
                Token::Sym(Sym::LBrace) => depth += 1,
                Token::Sym(Sym::RBrace) => depth -= 1,
                Token::End => return self.fail("unclosed `{`"),
                _ => {}
            }
            self.bump();
            if depth == 0 {
                return Ok(());
            }
        }
    }

    fn event_decl(&mut self) -> Result<EventDef, SyntaxError> {
        self.expect_word(Keyword::Event)?;
        let path = self.expect_path()?;
        self.expect_sym(Sym::LBrace)?;

        let mut fields = Vec::new();
        while !self.at_sym(Sym::RBrace) {
            let name = self.expect_ident()?;
            self.expect_sym(Sym::Colon)?;
            let ty = self.type_ref()?;
            let mut field = FieldDef::new(&name, ty);

            while let Token::Path(segments) = self.peek().clone() {
                self.bump();
                let [annotation] = segments.as_slice() else {
                    return self.fail("an annotation name cannot contain `.`");
                };
                match annotation.as_str() {
                    "subject" => {
                        self.expect_sym(Sym::LParen)?;
                        field = field.subject(self.expect_ident()?);
                        self.expect_sym(Sym::RParen)?;
                    }
                    "max" => {
                        self.expect_sym(Sym::LParen)?;
                        let number = self.expect_number()?;
                        if number.scale != 0 {
                            return self.fail("`@max` takes a whole number");
                        }
                        let Ok(max) = usize::try_from(number.digits) else {
                            return self.fail("`@max` is too large");
                        };
                        field = field.max_len(max);
                        self.expect_sym(Sym::RParen)?;
                    }
                    "no_index" => field = field.no_index(),
                    other => return self.fail(format!("unknown annotation `@{other}`")),
                }
            }

            fields.push(field);
            if !self.eat_sym(Sym::Comma) {
                break;
            }
        }

        self.expect_sym(Sym::RBrace)?;
        Ok(EventDef::new(path, fields))
    }

    fn type_ref(&mut self) -> Result<Type, SyntaxError> {
        let name = self.expect_ident()?;
        let ty = match name.as_str() {
            "Bool" => Type::Bool,
            "Int" => Type::Int,
            "String" => Type::String,
            "Uuid" => Type::Uuid,
            "Money" => Type::Money,
            "Decimal" => {
                self.expect_sym(Sym::LParen)?;
                let number = self.expect_number()?;
                self.expect_sym(Sym::RParen)?;
                match (number.scale, u8::try_from(number.digits)) {
                    (0, Ok(scale)) => Type::Decimal(scale),
                    _ => return self.fail("a Decimal scale must be a small whole number"),
                }
            }
            other => return self.fail(format!("unknown type `{other}`")),
        };

        if self.eat_sym(Sym::Question) {
            return Ok(Type::opt(ty));
        }
        Ok(ty)
    }

    fn command_decl(
        &mut self,
        events: &[EventDef],
        currency: &Currency,
    ) -> Result<Command, SyntaxError> {
        self.expect_word(Keyword::Command)?;
        let name = self.expect_ident()?;
        let mut cmd = Cmd {
            b: Builder::new(&name),
            defaults: HashMap::new(),
            currency: currency.clone(),
        };

        self.expect_sym(Sym::LParen)?;
        while !self.at_sym(Sym::RParen) {
            let param = self.expect_ident()?;
            self.expect_sym(Sym::Colon)?;
            let ty = self.type_ref()?;
            cmd.b.param(&param, ty);
            if !self.eat_sym(Sym::Comma) {
                break;
            }
        }
        self.expect_sym(Sym::RParen)?;
        self.expect_sym(Sym::LBrace)?;
        self.command_end = self.command_end();

        self.prologue = true;
        loop {
            match self.peek() {
                Token::Word(Keyword::Guard) => self.guard_decl(&mut cmd, events)?,
                Token::Word(Keyword::State) => self.state_decl(&mut cmd, events)?,
                Token::Word(Keyword::Let) => self.hoisted_let(&mut cmd)?,
                _ => break,
            }
        }
        self.prologue = false;

        let body = self.statements(&mut cmd, events)?;
        self.expect_sym(Sym::RBrace)?;
        Ok(cmd.b.finish(body))
    }

    fn guard_decl(&mut self, cmd: &mut Cmd, events: &[EventDef]) -> Result<(), SyntaxError> {
        self.expect_word(Keyword::Guard)?;
        loop {
            let (path, filters) = self.slice_ref(cmd, events)?;
            cmd.b.guard(path, filters);
            if !self.eat_sym(Sym::Comma) {
                return Ok(());
            }
        }
    }

    fn state_decl(&mut self, cmd: &mut Cmd, events: &[EventDef]) -> Result<(), SyntaxError> {
        self.expect_word(Keyword::State)?;
        let name = self.expect_ident()?;
        self.expect_sym(Sym::Colon)?;
        let ty = self.type_ref()?;
        self.expect_sym(Sym::Assign)?;
        let init = self.expr(cmd, Some(ty.clone()))?;
        let slot = cmd.b.state(&name, ty.clone(), init);

        while self.eat_word(Keyword::On) {
            let (path, filters) = self.slice_ref(cmd, events)?;
            let def = self.event_def(events, &path)?;

            cmd.b.push_scope();
            let mut binds = Vec::new();
            if self.eat_sym(Sym::LBrace) {
                while !self.at_sym(Sym::RBrace) {
                    let field = self.expect_ident()?;
                    let Some(declared) = def.field(&field) else {
                        return self.fail(format!("{path} has no field `{field}`"));
                    };
                    binds.push(cmd.b.bind(&field, Some(declared.ty.clone())));
                    if !self.eat_sym(Sym::Comma) {
                        break;
                    }
                }
                self.expect_sym(Sym::RBrace)?;
            }

            self.expect_sym(Sym::Arrow)?;
            let value = self.expr(cmd, Some(ty.clone()))?;
            cmd.b.pop_scope();
            cmd.b
                .slice(path, filters, binds, vec![Update { slot, value }]);
        }

        Ok(())
    }

    fn hoisted_let(&mut self, cmd: &mut Cmd) -> Result<(), SyntaxError> {
        self.expect_word(Keyword::Let)?;
        let name = self.expect_ident()?;
        self.expect_sym(Sym::Assign)?;
        let value = self.expr(cmd, None)?;
        let ty = type_of(cmd, value);
        cmd.b.hoist(&name, value, ty);
        Ok(())
    }

    fn slice_ref(
        &mut self,
        cmd: &mut Cmd,
        events: &[EventDef],
    ) -> Result<(EventPath, Vec<Filter>), SyntaxError> {
        let path = self.expect_path()?;
        let def = self.event_def(events, &path)?;
        self.expect_sym(Sym::LParen)?;

        let mut filters = Vec::new();
        while !self.at_sym(Sym::RParen) {
            let (line, col) = self.here();
            let field = self.expect_ident()?;
            let Some(declared) = def.field(&field) else {
                return self.fail(format!("{path} has no field `{field}`"));
            };
            let expected = declared.ty.clone();
            let value = if self.eat_sym(Sym::Colon) {
                self.expr(cmd, Some(expected))?
            } else {
                if cmd.b.lookup(&field).is_none() {
                    return Err(self.not_in_scope(&field, line, col));
                }
                cmd.b.at(Span::new(line, col));
                cmd.b.load(&field)
            };
            filters.push(Filter::new(field, value));
            if !self.eat_sym(Sym::Comma) {
                break;
            }
        }

        self.expect_sym(Sym::RParen)?;
        Ok((path, filters))
    }

    fn event_def<'a>(
        &self,
        events: &'a [EventDef],
        path: &EventPath,
    ) -> Result<&'a EventDef, SyntaxError> {
        match events.iter().find(|def| &def.path == path) {
            Some(def) => Ok(def),
            None => self.fail(format!("event {path} is not declared")),
        }
    }

    fn statements(&mut self, cmd: &mut Cmd, events: &[EventDef]) -> Result<Vec<Stmt>, SyntaxError> {
        let mut stmts = Vec::new();
        while !self.at_sym(Sym::RBrace) && !matches!(self.peek(), Token::End) {
            stmts.push(self.statement(cmd, events)?);
        }
        Ok(stmts)
    }

    fn block(&mut self, cmd: &mut Cmd, events: &[EventDef]) -> Result<Vec<Stmt>, SyntaxError> {
        self.expect_sym(Sym::LBrace)?;
        let stmts = self.statements(cmd, events)?;
        self.expect_sym(Sym::RBrace)?;
        Ok(stmts)
    }

    fn statement(&mut self, cmd: &mut Cmd, events: &[EventDef]) -> Result<Stmt, SyntaxError> {
        match self.peek() {
            Token::Word(Keyword::If) => {
                self.bump();
                let cond = self.expr(cmd, Some(Type::Bool))?;
                let then = self.block(cmd, events)?;
                let otherwise = if self.eat_word(Keyword::Else) {
                    self.block(cmd, events)?
                } else {
                    Vec::new()
                };
                Ok(Stmt::If {
                    cond,
                    then,
                    otherwise,
                })
            }
            Token::Word(Keyword::Return) => {
                self.bump();
                let ret = if self.eat_word(Keyword::Invalid) {
                    self.expect_sym(Sym::LParen)?;
                    let message = self.expr(cmd, Some(Type::String))?;
                    self.expect_sym(Sym::RParen)?;
                    Return::Invalid(message)
                } else if self.eat_word(Keyword::Reject) {
                    self.expect_sym(Sym::LParen)?;
                    let code = self.expr(cmd, Some(Type::String))?;
                    self.expect_sym(Sym::Comma)?;
                    let message = self.expr(cmd, Some(Type::String))?;
                    self.expect_sym(Sym::RParen)?;
                    Return::Reject { code, message }
                } else {
                    Return::Ok
                };
                Ok(Stmt::Return(ret))
            }
            Token::Word(Keyword::Emit) => {
                self.bump();
                let path = self.expect_path()?;
                let def = self.event_def(events, &path)?;
                self.expect_sym(Sym::LBrace)?;

                let mut fields = Vec::new();
                while !self.at_sym(Sym::RBrace) {
                    let (line, col) = self.here();
                    let name = self.expect_ident()?;
                    let Some(declared) = def.field(&name) else {
                        return self.fail(format!("{path} has no field `{name}`"));
                    };
                    let expected = declared.ty.clone();
                    let value = if self.eat_sym(Sym::Colon) {
                        self.expr(cmd, Some(expected))?
                    } else {
                        if cmd.b.lookup(&name).is_none() {
                            return Err(self.not_in_scope(&name, line, col));
                        }
                        cmd.b.at(Span::new(line, col));
                        cmd.b.load(&name)
                    };
                    fields.push((name, value));
                    if !self.eat_sym(Sym::Comma) {
                        break;
                    }
                }

                self.expect_sym(Sym::RBrace)?;
                Ok(Stmt::Emit {
                    event: path,
                    fields,
                })
            }
            Token::Word(Keyword::Let) => {
                self.bump();
                let name = self.expect_ident()?;
                self.expect_sym(Sym::Assign)?;
                let value = self.expr(cmd, None)?;
                let ty = type_of(cmd, value);
                let slot = cmd.b.alloc(&name, ty);
                Ok(Stmt::Assign { slot, value })
            }
            Token::Word(Keyword::State) | Token::Word(Keyword::Guard) => {
                self.fail("`state` and `guard` must come before the first statement")
            }
            other => self.fail(format!("expected a statement, found {other}")),
        }
    }

    fn expr(&mut self, cmd: &mut Cmd, expect: Option<Type>) -> Result<ExprId, SyntaxError> {
        self.or_expr(cmd, expect)
    }

    fn or_expr(&mut self, cmd: &mut Cmd, expect: Option<Type>) -> Result<ExprId, SyntaxError> {
        let mut lhs = self.and_expr(cmd, expect)?;
        while let Some(span) = self.eat_at(Sym::OrOr) {
            let rhs = self.and_expr(cmd, Some(Type::Bool))?;
            cmd.b.at(span);
            lhs = cmd.b.binary(BinOp::Or, lhs, rhs);
        }
        Ok(lhs)
    }

    fn and_expr(&mut self, cmd: &mut Cmd, expect: Option<Type>) -> Result<ExprId, SyntaxError> {
        let mut lhs = self.cmp_expr(cmd, expect)?;
        while let Some(span) = self.eat_at(Sym::AndAnd) {
            let rhs = self.cmp_expr(cmd, Some(Type::Bool))?;
            cmd.b.at(span);
            lhs = cmd.b.binary(BinOp::And, lhs, rhs);
        }
        Ok(lhs)
    }

    fn cmp_expr(&mut self, cmd: &mut Cmd, expect: Option<Type>) -> Result<ExprId, SyntaxError> {
        let lhs = self.add_expr(cmd, expect)?;
        let Some((op, span)) = self.eat_op(&[
            (Sym::Eq, BinOp::Eq),
            (Sym::Ne, BinOp::Ne),
            (Sym::Le, BinOp::Le),
            (Sym::Ge, BinOp::Ge),
            (Sym::Lt, BinOp::Lt),
            (Sym::Gt, BinOp::Gt),
        ]) else {
            return Ok(lhs);
        };

        let hint = self.hint_from(cmd, lhs);
        let rhs = self.add_expr(cmd, hint)?;
        self.settle(cmd, lhs, rhs);
        cmd.b.at(span);
        Ok(cmd.b.binary(op, lhs, rhs))
    }

    fn add_expr(&mut self, cmd: &mut Cmd, expect: Option<Type>) -> Result<ExprId, SyntaxError> {
        let mut lhs = self.mul_expr(cmd, expect.clone())?;
        while let Some((op, span)) =
            self.eat_op(&[(Sym::Plus, BinOp::Add), (Sym::Minus, BinOp::Sub)])
        {
            let hint = self.hint_from(cmd, lhs).or_else(|| expect.clone());
            let rhs = self.mul_expr(cmd, hint)?;
            self.settle(cmd, lhs, rhs);
            cmd.b.at(span);
            lhs = cmd.b.binary(op, lhs, rhs);
        }
        Ok(lhs)
    }

    fn mul_expr(&mut self, cmd: &mut Cmd, expect: Option<Type>) -> Result<ExprId, SyntaxError> {
        let mut lhs = self.unary_expr(cmd, expect)?;
        while let Some((op, span)) = self.eat_op(&[
            (Sym::Star, BinOp::Mul),
            (Sym::Slash, BinOp::Div),
            (Sym::Percent, BinOp::Rem),
        ]) {
            let rhs = self.unary_expr(cmd, None)?;
            cmd.b.at(span);
            lhs = cmd.b.binary(op, lhs, rhs);
        }
        Ok(lhs)
    }

    fn unary_expr(&mut self, cmd: &mut Cmd, expect: Option<Type>) -> Result<ExprId, SyntaxError> {
        if let Some(span) = self.eat_at(Sym::Bang) {
            let operand = self.unary_expr(cmd, Some(Type::Bool))?;
            cmd.b.at(span);
            return Ok(cmd.b.unary(UnOp::Not, operand));
        }
        if let Some(span) = self.eat_at(Sym::Minus) {
            let operand = self.unary_expr(cmd, expect)?;
            cmd.b.at(span);
            return Ok(cmd.b.unary(UnOp::Neg, operand));
        }
        self.postfix_expr(cmd, expect)
    }

    fn postfix_expr(&mut self, cmd: &mut Cmd, expect: Option<Type>) -> Result<ExprId, SyntaxError> {
        let mut value = self.primary(cmd, expect)?;
        while self.eat_sym(Sym::Dot) {
            let span = self.span_here();
            let method = self.expect_ident()?;
            self.expect_sym(Sym::LParen)?;
            let mut args = Vec::new();
            while !self.at_sym(Sym::RParen) {
                args.push(self.expr(cmd, None)?);
                if !self.eat_sym(Sym::Comma) {
                    break;
                }
            }
            self.expect_sym(Sym::RParen)?;
            cmd.b.at(span);
            value = cmd.b.method(value, &method, args);
        }
        Ok(value)
    }

    fn primary(&mut self, cmd: &mut Cmd, expect: Option<Type>) -> Result<ExprId, SyntaxError> {
        let spanned = self.bump();
        let span = Span::new(spanned.line, spanned.col);
        cmd.b.at(span);
        match spanned.token {
            Token::Number(number) => self.number(cmd, number, expect, &spanned),
            Token::Text(text) => Ok(cmd.b.lit(Literal::Str(text))),
            Token::Word(Keyword::True) => Ok(cmd.b.bool(true)),
            Token::Word(Keyword::False) => Ok(cmd.b.bool(false)),
            Token::Word(Keyword::If) => {
                let cond = self.expr(cmd, Some(Type::Bool))?;
                self.expect_sym(Sym::LBrace)?;
                let then = self.expr(cmd, expect.clone())?;
                self.expect_sym(Sym::RBrace)?;
                self.expect_word(Keyword::Else)?;
                self.expect_sym(Sym::LBrace)?;
                let otherwise = self.expr(cmd, expect)?;
                self.expect_sym(Sym::RBrace)?;
                cmd.b.at(span);
                Ok(cmd.b.if_expr(cond, then, otherwise))
            }
            Token::Sym(Sym::LParen) => {
                let value = self.expr(cmd, expect)?;
                self.expect_sym(Sym::RParen)?;
                Ok(value)
            }
            Token::Ident(name) => {
                if cmd.b.lookup(&name).is_some() {
                    return Ok(cmd.b.load(&name));
                }
                match rounding_mode(&name) {
                    Some(mode) => Ok(cmd.b.rounding(mode)),
                    None => Err(self.not_in_scope(&name, spanned.line, spanned.col)),
                }
            }
            other => Err(SyntaxError::new(
                format!("expected a value, found {other}"),
                spanned.line,
                spanned.col,
            )),
        }
    }

    fn number(
        &mut self,
        cmd: &mut Cmd,
        number: Number,
        expect: Option<Type>,
        at: &Spanned,
    ) -> Result<ExprId, SyntaxError> {
        let defaulted = expect.is_none();
        let ty = expect.unwrap_or_else(|| default_type(number));
        let lit = number
            .resolve(&ty, &cmd.currency)
            .map_err(|err| SyntaxError::new(err.to_string(), at.line, at.col))?;
        let id = cmd.b.lit(lit);
        if defaulted {
            cmd.defaults.insert(id, number);
        }
        Ok(id)
    }

    fn hint_from(&self, cmd: &Cmd, id: ExprId) -> Option<Type> {
        if cmd.defaults.contains_key(&id) {
            return None;
        }
        type_of(cmd, id)
    }

    fn settle(&self, cmd: &mut Cmd, lhs: ExprId, rhs: ExprId) {
        match (
            cmd.defaults.get(&lhs).copied(),
            cmd.defaults.get(&rhs).copied(),
        ) {
            (Some(left), Some(right)) => {
                if left.scale > right.scale {
                    self.retype(cmd, rhs, &default_type(left));
                } else if right.scale > left.scale {
                    self.retype(cmd, lhs, &default_type(right));
                }
            }
            (Some(_), None) => {
                if let Some(ty) = type_of(cmd, rhs) {
                    self.retype(cmd, lhs, &ty);
                }
            }
            (None, Some(_)) => {
                if let Some(ty) = type_of(cmd, lhs) {
                    self.retype(cmd, rhs, &ty);
                }
            }
            (None, None) => {}
        }
    }

    fn retype(&self, cmd: &mut Cmd, id: ExprId, ty: &Type) {
        let Some(number) = cmd.defaults.get(&id).copied() else {
            return;
        };
        if let Ok(lit) = number.resolve(ty, &cmd.currency) {
            cmd.b.patch(id, Expr::Lit(lit));
            cmd.defaults.remove(&id);
        }
    }

    fn eat_op(&mut self, table: &[(Sym, BinOp)]) -> Option<(BinOp, Span)> {
        for (sym, op) in table {
            if let Some(span) = self.eat_at(*sym) {
                return Some((*op, span));
            }
        }
        None
    }

    fn eat_at(&mut self, sym: Sym) -> Option<Span> {
        if !self.at_sym(sym) {
            return None;
        }
        let span = self.span_here();
        self.bump();
        Some(span)
    }
}

fn default_type(number: Number) -> Type {
    if number.scale == 0 {
        Type::Int
    } else {
        Type::Decimal(number.scale)
    }
}

fn rounding_mode(name: &str) -> Option<Rounding> {
    Some(match name {
        "HalfUp" => Rounding::HalfUp,
        "HalfEven" => Rounding::HalfEven,
        "Down" => Rounding::Down,
        _ => return None,
    })
}

fn type_of(cmd: &Cmd, id: ExprId) -> Option<Type> {
    match cmd.b.exprs().get(id)? {
        Expr::Lit(lit) => Some(match lit {
            Literal::Bool(_) => Type::Bool,
            Literal::Int(_) => Type::Int,
            Literal::Decimal { scale, .. } => Type::Decimal(*scale),
            Literal::Str(_) => Type::String,
            Literal::Uuid(_) => Type::Uuid,
            Literal::Money(_) => Type::Money,
            Literal::Rounding(_) => Type::Rounding,
        }),
        Expr::Load(slot) => cmd.b.slot_type(*slot).cloned(),
        Expr::Unary { operand, .. } => type_of(cmd, *operand),
        Expr::Binary { op, lhs, rhs } => match op {
            BinOp::Eq
            | BinOp::Ne
            | BinOp::Lt
            | BinOp::Le
            | BinOp::Gt
            | BinOp::Ge
            | BinOp::And
            | BinOp::Or => Some(Type::Bool),
            _ => type_of(cmd, *lhs).or_else(|| type_of(cmd, *rhs)),
        },
        Expr::Method { .. } => None,
        Expr::If { then, .. } => type_of(cmd, *then),
    }
}
