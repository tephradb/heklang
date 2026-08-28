use std::collections::HashMap;
use std::mem;

use crate::build::Builder;
use crate::ir::{
    Arm, BinOp, Bind, Builtin, Command, Effect, EntityDef, EntityField, EnumDef, EnvField,
    EventDef, EventPath, Expr, ExprId, Exprs, FieldDef, Filter, Handler, Ident, Index, Literal,
    Number, Program, Projector, Return, Slot, Span, Stmt, Type, UnOp, Update,
};
use crate::lex::{Keyword, Spanned, Sym, SyntaxError, Token, lex};
use crate::scaled::Rounding;
use crate::value;

pub fn parse(source: &str) -> Result<Program, SyntaxError> {
    Parser::open([(None, source)])?.program()
}

/// Parses several modules as one program. Declaration order across modules does not
/// matter, and an error names the module it is in.
pub fn parse_files<'a>(
    files: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<Program, SyntaxError> {
    let files: Vec<(Option<&str>, &str)> = files
        .into_iter()
        .map(|(name, source)| (Some(name), source))
        .collect();
    Parser::open(files)?.program()
}

/// Per-declaration lowering state: the arena and scopes being built, plus the
/// numeric literals that were typed by default and may still be retyped.
struct Lower {
    b: Builder,
    defaults: HashMap<ExprId, Number>,
}

/// Parsing state that the expression ladder needs but cannot be threaded through it,
/// since those functions take only the unit being lowered and a type hint.
struct Parser {
    tokens: Vec<Spanned>,
    /// First token index of each module, with its name. Sorted, so a position maps
    /// back to the module it came from.
    modules: Vec<(usize, Option<String>)>,
    pos: usize,
    prologue: bool,
    /// Set while parsing a filter, a `state` seed or a fold arm. Narrower than
    /// `prologue`, which also covers a hoisted `let`: that runs once per request,
    /// before the fold, so it may read the pinned clock while a fold may not.
    folding: bool,
    command_end: usize,
    /// Which declaration kind is being parsed, so a statement in the wrong one can be
    /// rejected with a message about that kind rather than a generic one.
    kind: Kind,
    /// Command signatures, collected in pass 1 so an `invoke` can be checked against a
    /// command declared later or in another module (rule 7).
    commands: Vec<Signature>,
    /// Set only while parsing an `http.*` body argument, which is what makes an object
    /// literal structurally illegal anywhere else (rule 8).
    in_body: bool,
    /// The enclosing projector's declarations; empty outside one.
    enums: Vec<EnumDef>,
    entities: Vec<EntityDef>,
    /// The handler being parsed: its event, and the name its `as` clause bound.
    event: Option<EventDef>,
    envelope: Option<Ident>,
    /// Set only while parsing a `patch` value, which is what makes `.field`
    /// structurally illegal anywhere else.
    stored: Option<Stored>,
}

struct Stored {
    entity: EntityDef,
    loads: Vec<Bind>,
    slots: HashMap<Ident, Slot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Command,
    Projector,
    Effect,
}

#[derive(Debug, Clone)]
struct Signature {
    name: Ident,
    params: Vec<(Ident, Type)>,
}

impl Parser {
    /// Lexes every module into one token stream, remembering where each begins so a
    /// position can be mapped back to the module it came from. Line and column stay
    /// module-relative, because each module is lexed on its own.
    fn open<'a>(
        files: impl IntoIterator<Item = (Option<&'a str>, &'a str)>,
    ) -> Result<Self, SyntaxError> {
        let mut tokens = Vec::new();
        let mut modules = Vec::new();
        for (name, source) in files {
            let mut lexed = lex(source).map_err(|err| match name {
                Some(name) => err.in_file(name),
                None => err,
            })?;
            lexed.pop();
            modules.push((tokens.len(), name.map(str::to_string)));
            tokens.append(&mut lexed);
        }
        tokens.push(Spanned {
            token: Token::End,
            line: 0,
            col: 0,
        });

        Ok(Parser {
            tokens,
            modules,
            pos: 0,
            prologue: false,
            folding: false,
            command_end: 0,
            kind: Kind::Command,
            commands: Vec::new(),
            in_body: false,
            enums: Vec::new(),
            entities: Vec::new(),
            event: None,
            envelope: None,
            stored: None,
        })
    }

    fn module_at(&self, pos: usize) -> Option<&str> {
        self.modules
            .iter()
            .rev()
            .find(|(start, _)| *start <= pos)
            .and_then(|(_, name)| name.as_deref())
    }

    fn err(&self, message: impl Into<String>, line: u32, col: u32) -> SyntaxError {
        let error = SyntaxError::new(message, line, col);
        match self.module_at(self.pos) {
            Some(module) => error.in_file(module),
            None => error,
        }
    }

    /// A `file:line:col` for a token elsewhere in the stream, for messages that point
    /// at a first declaration from the site of a second.
    fn location(&self, pos: usize) -> String {
        let (line, col) = self
            .tokens
            .get(pos)
            .map(|spanned| (spanned.line, spanned.col))
            .unwrap_or((0, 0));
        match self.module_at(pos) {
            Some(module) => format!("{module}:{line}:{col}"),
            None => format!("{line}:{col}"),
        }
    }

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
                 bound above them; move that `let` up"
            ),
            (Some(span), false) => {
                format!("`{name}` is not in scope yet; it is defined below at {span}")
            }
            (None, _) => format!("`{name}` is not in scope"),
        };
        self.err(message, line, col)
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
        Err(self.err(message, line, col))
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
        self.pos = 0;
        let items = self.pos;
        let mut events: Vec<EventDef> = Vec::new();
        let mut projectors: Vec<Projector> = Vec::new();
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
                Token::Word(Keyword::Projector) => {
                    let projector = self.projector_shell()?;
                    if projectors.iter().any(|other| other.name == projector.name) {
                        return self
                            .fail(format!("projector `{}` is declared twice", projector.name));
                    }
                    projectors.push(projector);
                }
                Token::Word(Keyword::Command) => self.command_signature()?,
                Token::Word(Keyword::Effect) => self.skip_item()?,
                other => return self.fail(Self::expected_item(other)),
            }
        }

        self.pos = items;
        let mut commands = Vec::new();
        let mut effects: Vec<Effect> = Vec::new();
        let mut seen = 0usize;
        loop {
            match self.peek() {
                Token::End => break,
                Token::Word(Keyword::Event) => self.skip_item()?,
                Token::Word(Keyword::Command) => {
                    let command = self.command_decl(&events)?;
                    commands.push(command);
                }
                Token::Word(Keyword::Projector) => {
                    let (handlers, entities) =
                        self.projector_handlers(&projectors[seen], &events)?;
                    projectors[seen].handlers = handlers;
                    projectors[seen].entities = entities;
                    seen += 1;
                }
                Token::Word(Keyword::Effect) => {
                    let effect = self.effect_decl(&events)?;
                    if effects.iter().any(|other| other.name == effect.name) {
                        return self.fail(format!("effect `{}` is declared twice", effect.name));
                    }
                    effects.push(effect);
                }
                other => return self.fail(Self::expected_item(other)),
            }
        }

        let program = Program {
            events,
            commands,
            projectors,
            effects,
        };
        self.check_cycles(&program)?;
        Ok(program)
    }

    /// Pass 1: a command's name and parameter types, so rule 7 can check an `invoke`
    /// against a command declared later or in another module. A parameter list is
    /// `name: Type` and nothing else, with no defaults and no literals, so it never
    /// consults the event table. That is what keeps it a pass-1 job.
    fn command_signature(&mut self) -> Result<(), SyntaxError> {
        self.expect_word(Keyword::Command)?;
        let (line, col) = self.here();
        let name = self.expect_ident()?;
        if self.commands.iter().any(|other| other.name == name) {
            return Err(self.err(format!("command `{name}` is declared twice"), line, col));
        }

        let mut params = Vec::new();
        self.expect_sym(Sym::LParen)?;
        while !self.at_sym(Sym::RParen) {
            let param = self.expect_ident()?;
            self.expect_sym(Sym::Colon)?;
            params.push((param, self.type_ref()?));
            if !self.eat_sym(Sym::Comma) {
                break;
            }
        }
        self.expect_sym(Sym::RParen)?;
        self.commands.push(Signature { name, params });
        self.skip_braced()
    }

    fn expected_item(found: &Token) -> String {
        format!("expected `event`, `command`, `projector` or `effect`, found {found}")
    }

    /// Collects a projector's `enum` and `entity` declarations, leaving its handlers
    /// for the second pass, so a handler may reference an event declared later or in
    /// another file. Two sub-passes, because an entity field may name an enum
    /// declared below it.
    fn projector_shell(&mut self) -> Result<Projector, SyntaxError> {
        let module = self.module_at(self.pos).map(str::to_string);
        self.expect_word(Keyword::Projector)?;
        let name = self.expect_ident()?;
        self.expect_sym(Sym::LBrace)?;
        let body = self.pos;

        let mut enums: Vec<EnumDef> = Vec::new();
        while !self.at_sym(Sym::RBrace) && !matches!(self.peek(), Token::End) {
            match self.peek() {
                Token::Word(Keyword::Enum) => {
                    let def = self.enum_decl()?;
                    if enums.iter().any(|other| other.name == def.name) {
                        return self.fail(format!("enum `{}` is declared twice", def.name));
                    }
                    enums.push(def);
                }
                Token::Word(Keyword::Entity) => self.skip_braced()?,
                Token::Word(Keyword::On) => self.skip_handler()?,
                other => return self.fail(Self::expected_member(other)),
            }
        }

        self.pos = body;
        self.enums = enums;
        let mut entities: Vec<EntityDef> = Vec::new();
        while !self.at_sym(Sym::RBrace) && !matches!(self.peek(), Token::End) {
            match self.peek() {
                Token::Word(Keyword::Enum) => self.skip_braced()?,
                Token::Word(Keyword::Entity) => {
                    let def = self.entity_decl()?;
                    if entities.iter().any(|other| other.name == def.name) {
                        return self.fail(format!("entity `{}` is declared twice", def.name));
                    }
                    entities.push(def);
                }
                Token::Word(Keyword::On) => self.skip_handler()?,
                other => return self.fail(Self::expected_member(other)),
            }
        }
        self.expect_sym(Sym::RBrace)?;

        let enums = mem::take(&mut self.enums);
        if entities.is_empty() {
            return self.fail(format!("projector `{name}` declares no entities"));
        }

        Ok(Projector {
            name,
            module,
            enums,
            entities,
            handlers: Vec::new(),
        })
    }

    fn expected_member(found: &Token) -> String {
        format!("expected `enum`, `entity` or `on`, found {found}")
    }

    /// Returns the handlers and the entities, which the pass may have annotated with
    /// propagated subjects (rule 9).
    fn projector_handlers(
        &mut self,
        projector: &Projector,
        events: &[EventDef],
    ) -> Result<(Vec<Handler>, Vec<EntityDef>), SyntaxError> {
        self.expect_word(Keyword::Projector)?;
        self.expect_ident()?;
        self.expect_sym(Sym::LBrace)?;

        self.enums = projector.enums.clone();
        self.entities = projector.entities.clone();

        let mut handlers = Vec::new();
        while !self.at_sym(Sym::RBrace) && !matches!(self.peek(), Token::End) {
            match self.peek() {
                Token::Word(Keyword::Enum) | Token::Word(Keyword::Entity) => self.skip_braced()?,
                Token::Word(Keyword::On) => handlers.push(self.handler(&projector.name, events)?),
                other => return self.fail(Self::expected_member(other)),
            }
        }
        self.expect_sym(Sym::RBrace)?;

        self.enums.clear();
        Ok((handlers, mem::take(&mut self.entities)))
    }

    /// Scans to the next `{` and skips the balanced block.
    fn skip_braced(&mut self) -> Result<(), SyntaxError> {
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

    /// A handler is one or two adjacent blocks: an optional destructure, then the
    /// body. Scans to the first block before asking which form this is.
    fn skip_handler(&mut self) -> Result<(), SyntaxError> {
        while !self.at_sym(Sym::LBrace) {
            if matches!(self.peek(), Token::End) {
                return self.fail("expected `{`, found end of file");
            }
            self.bump();
        }
        if self.has_destructure() {
            self.skip_braced()?;
        }
        self.skip_braced()
    }

    fn skip_item(&mut self) -> Result<(), SyntaxError> {
        self.bump();
        self.skip_braced()
    }

    fn enum_decl(&mut self) -> Result<EnumDef, SyntaxError> {
        self.expect_word(Keyword::Enum)?;
        let name = self.expect_ident()?;
        self.expect_sym(Sym::LBrace)?;

        let mut variants: Vec<Ident> = Vec::new();
        let mut default = None;
        while !self.at_sym(Sym::RBrace) {
            let mut marked = false;
            if let Token::Path(segments) = self.peek().clone() {
                let [annotation] = segments.as_slice() else {
                    return self.fail("an annotation name cannot contain `.`");
                };
                if annotation != "default" {
                    return self.fail(format!("unknown annotation `@{annotation}`"));
                }
                self.bump();
                marked = true;
            }

            let (line, col) = self.here();
            let variant = self.expect_ident()?;
            if variants.contains(&variant) {
                return Err(self.err(format!("`{name}` declares `{variant}` twice"), line, col));
            }
            if marked {
                if default.is_some() {
                    return Err(self.err(
                        format!("`{name}` has more than one `@default` variant"),
                        line,
                        col,
                    ));
                }
                default = Some(variants.len());
            }
            variants.push(variant);
            if !self.eat_sym(Sym::Comma) {
                break;
            }
        }
        self.expect_sym(Sym::RBrace)?;

        if variants.is_empty() {
            return self.fail(format!("enum `{name}` declares no variants"));
        }
        Ok(EnumDef {
            name,
            variants,
            default,
        })
    }

    fn entity_decl(&mut self) -> Result<EntityDef, SyntaxError> {
        self.expect_word(Keyword::Entity)?;
        let name = self.expect_ident()?;
        self.expect_sym(Sym::LBrace)?;

        let mut fields: Vec<EntityField> = Vec::new();
        let mut at: Vec<(u32, u32)> = Vec::new();
        let mut indexes: Vec<Index> = Vec::new();
        let mut key: Option<usize> = None;

        while !self.at_sym(Sym::RBrace) {
            if self.at_index_clause() {
                indexes.push(self.index_clause()?);
                if !self.eat_sym(Sym::Comma) {
                    break;
                }
                continue;
            }

            let (line, col) = self.here();
            let field_name = self.expect_ident()?;
            if fields.iter().any(|field| field.name == field_name) {
                return Err(self.err(
                    format!("entity `{name}` declares `{field_name}` twice"),
                    line,
                    col,
                ));
            }
            self.expect_sym(Sym::Colon)?;
            let ty = self.type_ref()?;
            let mut field = EntityField::new(&field_name, ty.clone());
            let mut is_key = false;

            while let Token::Path(segments) = self.peek().clone() {
                self.bump();
                let [annotation] = segments.as_slice() else {
                    return self.fail("an annotation name cannot contain `.`");
                };
                match annotation.as_str() {
                    "key" => is_key = true,
                    "index" => indexes.push(Index {
                        fields: vec![field_name.clone()],
                    }),
                    "max" => {
                        self.expect_sym(Sym::LParen)?;
                        let number = self.expect_number()?;
                        if number.scale != 0 {
                            return self.fail("`@max` takes a whole number");
                        }
                        let Ok(max) = usize::try_from(number.digits) else {
                            return self.fail("`@max` is too large");
                        };
                        field.max_len = Some(max);
                        self.expect_sym(Sym::RParen)?;
                    }
                    other => return self.fail(format!("unknown annotation `@{other}`")),
                }
            }

            if self.eat_sym(Sym::Assign) {
                if matches!(ty, Type::Opt(_)) {
                    return self.fail(format!(
                        "`{field_name}` is optional, so it is already `none` by default"
                    ));
                }
                field.default = Some(self.default_literal(&ty)?);
            }

            if is_key {
                if key.is_some() {
                    return Err(self.err(
                        format!("entity `{name}` has more than one `@key`"),
                        line,
                        col,
                    ));
                }
                if !value::can_key(&ty) {
                    return Err(self.err(
                        format!("`{field_name}` is a {ty}, which cannot be an entity key"),
                        line,
                        col,
                    ));
                }
                key = Some(fields.len());
            }

            fields.push(field);
            at.push((line, col));
            if !self.eat_sym(Sym::Comma) {
                break;
            }
        }
        self.expect_sym(Sym::RBrace)?;

        let Some(key) = key else {
            return self.fail(format!("entity `{name}` has no `@key` field"));
        };

        for index in &indexes {
            for column in &index.fields {
                if !fields.iter().any(|field| &field.name == column) {
                    return self.fail(format!("entity `{name}` has no field `{column}` to index"));
                }
            }
        }

        for (position, field) in fields.iter().enumerate() {
            if position == key || field.default.is_some() {
                continue;
            }
            let (line, col) = at[position];
            if let Type::Enum(enum_name) = &field.ty
                && self
                    .enum_def(enum_name)
                    .is_some_and(|def| def.default.is_none())
            {
                return Err(self.err(
                    format!(
                        "`{enum_name}` needs a `@default` variant to be a field of `{name}`, or `{}` must be optional",
                        field.name
                    ),
                    line,
                    col,
                ));
            }
            if value::zero(&field.ty, &self.enums).is_none() {
                let ty = &field.ty;
                return Err(self.err(
                    format!(
                        "`{}` is a {ty} with no zero value; give it a default or make it `{ty}?`",
                        field.name
                    ),
                    line,
                    col,
                ));
            }
        }

        Ok(EntityDef {
            name,
            fields,
            key,
            indexes,
        })
    }

    /// `index` stays a soft keyword, so it is still usable as a field name.
    fn at_index_clause(&self) -> bool {
        matches!(self.peek(), Token::Ident(word) if word == "index")
            && matches!(
                self.tokens.get(self.pos + 1).map(|next| &next.token),
                Some(Token::Sym(Sym::LParen))
            )
    }

    fn index_clause(&mut self) -> Result<Index, SyntaxError> {
        self.bump();
        self.expect_sym(Sym::LParen)?;
        let mut columns = Vec::new();
        while !self.at_sym(Sym::RParen) {
            columns.push(self.expect_ident()?);
            if !self.eat_sym(Sym::Comma) {
                break;
            }
        }
        self.expect_sym(Sym::RParen)?;
        if columns.is_empty() {
            return self.fail("an index needs at least one field");
        }
        Ok(Index { fields: columns })
    }

    /// A field default is a literal, resolved here against the declared type, so
    /// nothing unresolved reaches the IR and a default always agrees in type with the
    /// zero it replaces.
    fn default_literal(&mut self, ty: &Type) -> Result<Literal, SyntaxError> {
        let negated = self.eat_sym(Sym::Minus);
        let spanned = self.bump();
        let (line, col) = (spanned.line, spanned.col);
        let bad =
            |found: &str| self.err(format!("a {ty} field cannot default to {found}"), line, col);

        let lit = match spanned.token {
            Token::Number(number) => {
                let digits = if negated {
                    -number.digits
                } else {
                    number.digits
                };
                Number::new(digits, number.scale)
                    .resolve(ty)
                    .map_err(|err| self.err(err.to_string(), line, col))?
            }
            _ if negated => return Err(bad("a negated value")),
            Token::Text(text) => Literal::Str(text),
            Token::Word(Keyword::True) => Literal::Bool(true),
            Token::Word(Keyword::False) => Literal::Bool(false),
            Token::Ident(variant) => {
                let Type::Enum(enum_name) = ty else {
                    return Err(bad(&format!("`{variant}`")));
                };
                let Some(def) = self.enum_def(enum_name) else {
                    return Err(bad(&format!("`{variant}`")));
                };
                if !def.has(&variant) {
                    return Err(self.err(
                        format!("`{enum_name}` has no variant `{variant}`"),
                        line,
                        col,
                    ));
                }
                Literal::Enum {
                    ty: enum_name.clone(),
                    variant,
                }
            }
            other => return Err(bad(&other.to_string())),
        };

        let found = value::literal(&lit).ty();
        if &found != ty {
            return Err(self.err(
                format!("a {ty} field cannot default to a {found}"),
                line,
                col,
            ));
        }
        Ok(lit)
    }

    fn enum_def(&self, name: &str) -> Option<&EnumDef> {
        self.enums.iter().find(|def| def.name == name)
    }

    fn entity_def(&self, name: &str) -> Option<&EntityDef> {
        self.entities.iter().find(|def| def.name == name)
    }

    fn handler(&mut self, projector: &Ident, events: &[EventDef]) -> Result<Handler, SyntaxError> {
        self.expect_word(Keyword::On)?;
        let path = self.expect_path()?;
        let def = self.event_def(events, &path)?.clone();

        let mut lower = Lower {
            b: Builder::new(projector),
            defaults: HashMap::new(),
        };

        let envelope = if self.eat_word(Keyword::As) {
            Some(self.expect_ident()?)
        } else {
            None
        };

        self.destructure_block(&mut lower, &def)?;

        self.expect_sym(Sym::LBrace)?;
        self.command_end = self.command_end();
        self.event = Some(def);
        self.envelope = envelope;
        self.kind = Kind::Projector;
        let body = self.statements(&mut lower, events)?;
        self.expect_sym(Sym::RBrace)?;
        self.event = None;
        self.envelope = None;
        self.kind = Kind::Command;

        Ok(lower.b.finish_handler(path, body))
    }

    /// The optional `{ field, field }` block, shared by projector handlers and effect
    /// arms so the two kinds cannot drift apart on the same construct.
    fn destructure_block(&mut self, lower: &mut Lower, def: &EventDef) -> Result<(), SyntaxError> {
        if !self.has_destructure() {
            if self.looks_like_destructure() {
                return self.fail(
                    "this looks like a destructure block; a handler with one needs a body block after it",
                );
            }
            return Ok(());
        }

        let path = &def.path;
        self.expect_sym(Sym::LBrace)?;
        while !self.at_sym(Sym::RBrace) {
            let field = self.expect_ident()?;
            let Some(declared) = def.field(&field) else {
                return self.fail(format!("{path} has no field `{field}`"));
            };
            lower.b.destructure(&field, Some(declared.ty.clone()));
            if !self.eat_sym(Sym::Comma) {
                break;
            }
        }
        self.expect_sym(Sym::RBrace)
    }

    /// Whether the block starting here is a destructure rather than a body: it is when
    /// another block follows it. No statement can begin with `{`, so this decides.
    fn has_destructure(&self) -> bool {
        if !self.at_sym(Sym::LBrace) {
            return false;
        }
        let mut depth = 0u32;
        for (index, spanned) in self.tokens.iter().enumerate().skip(self.pos) {
            match &spanned.token {
                Token::Sym(Sym::LBrace) => depth += 1,
                Token::Sym(Sym::RBrace) => {
                    depth -= 1;
                    if depth == 0 {
                        return matches!(
                            self.tokens.get(index + 1).map(|next| &next.token),
                            Some(Token::Sym(Sym::LBrace))
                        );
                    }
                }
                Token::End => return false,
                _ => {}
            }
        }
        false
    }

    /// A lone block holding only names and commas is a destructure whose body was
    /// forgotten, which is worth saying rather than reporting the first name as a bad
    /// statement.
    fn looks_like_destructure(&self) -> bool {
        if !self.at_sym(Sym::LBrace) {
            return false;
        }
        let mut names = 0usize;
        for spanned in self.tokens.iter().skip(self.pos + 1) {
            match &spanned.token {
                Token::Ident(_) => names += 1,
                Token::Sym(Sym::Comma) => {}
                Token::Sym(Sym::RBrace) => return names > 0,
                _ => return false,
            }
        }
        false
    }

    /// `e.at` / `e.id` / `e.position` become envelope slots; anything else is a
    /// payload field, bound on demand.
    fn envelope_access(
        &mut self,
        lower: &mut Lower,
        name: &str,
        span: Span,
    ) -> Result<ExprId, SyntaxError> {
        if !self.eat_sym(Sym::Dot) {
            return Err(self.err(
                format!("`{name}` is the event envelope; read a field from it, like `{name}.at`"),
                span.line,
                span.col,
            ));
        }

        let (line, col) = self.here();
        let field = self.expect_ident()?;
        lower.b.at(span);

        if let Some(env) = EnvField::lookup(&field) {
            let slot = lower.b.envelope(env);
            return Ok(lower.b.read(slot));
        }

        let def = self.event.as_ref().expect("only called inside a handler");
        let Some(declared) = def.field(&field) else {
            return Err(self.err(
                format!(
                    "{} has no field `{field}`, and the envelope carries only `at`, `id` and `position`",
                    def.path
                ),
                line,
                col,
            ));
        };
        let slot = lower.b.payload(&field, Some(declared.ty.clone()));
        Ok(lower.b.read(slot))
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

    /// The `(n)` a `Decimal` or a `Money` carries. Both are scaled integers and both
    /// spell their scale the same way.
    fn scale_arg(&mut self, what: &str) -> Result<u8, SyntaxError> {
        self.expect_sym(Sym::LParen)?;
        let number = self.expect_number()?;
        self.expect_sym(Sym::RParen)?;
        match (number.scale, u8::try_from(number.digits)) {
            (0, Ok(scale)) => Ok(scale),
            _ => self.fail(format!("a {what} scale must be a small whole number")),
        }
    }

    fn type_ref(&mut self) -> Result<Type, SyntaxError> {
        let name = self.expect_ident()?;
        let ty = match name.as_str() {
            "Bool" => Type::Bool,
            "Int" => Type::Int,
            "String" => Type::String,
            "Uuid" => Type::Uuid,
            "Timestamp" => Type::Timestamp,
            "Decimal" => Type::Decimal(self.scale_arg("Decimal")?),
            "Money" => Type::Money(self.scale_arg("Money")?),
            other => match self.enum_def(other) {
                Some(def) => Type::Enum(def.name.clone()),
                None => return self.fail(format!("unknown type `{other}`")),
            },
        };

        if self.eat_sym(Sym::Question) {
            return Ok(Type::opt(ty));
        }
        Ok(ty)
    }

    fn command_decl(&mut self, events: &[EventDef]) -> Result<Command, SyntaxError> {
        let module = self.module_at(self.pos).map(str::to_string);
        self.expect_word(Keyword::Command)?;
        let name = self.expect_ident()?;
        let mut lower = Lower {
            b: Builder::new(&name),
            defaults: HashMap::new(),
        };
        lower.b.in_module(module.as_deref());

        self.expect_sym(Sym::LParen)?;
        while !self.at_sym(Sym::RParen) {
            let param = self.expect_ident()?;
            self.expect_sym(Sym::Colon)?;
            let ty = self.type_ref()?;
            lower.b.param(&param, ty);
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
                Token::Word(Keyword::Guard) => self.guard_decl(&mut lower, events)?,
                Token::Word(Keyword::State) => self.state_decl(&mut lower, events)?,
                Token::Word(Keyword::Let) => self.hoisted_let(&mut lower)?,
                _ => break,
            }
        }
        self.prologue = false;

        let body = self.statements(&mut lower, events)?;
        self.expect_sym(Sym::RBrace)?;
        Ok(lower.b.finish(body))
    }

    fn guard_decl(&mut self, lower: &mut Lower, events: &[EventDef]) -> Result<(), SyntaxError> {
        self.expect_word(Keyword::Guard)?;
        loop {
            let (path, filters) = self.slice_ref(lower, events)?;
            lower.b.guard(path, filters);
            if !self.eat_sym(Sym::Comma) {
                return Ok(());
            }
        }
    }

    fn state_decl(&mut self, lower: &mut Lower, events: &[EventDef]) -> Result<(), SyntaxError> {
        self.expect_word(Keyword::State)?;
        let name = self.expect_ident()?;
        self.expect_sym(Sym::Colon)?;
        let ty = self.type_ref()?;
        self.expect_sym(Sym::Assign)?;
        if !self.eat_word(Keyword::Fold) {
            return self.fail(format!(
                "`{name}` is a fold over the log, so `=` introduces a seed rather than a value; write `= fold <seed>`"
            ));
        }
        self.folding = true;
        let init = self.expr(lower, Some(ty.clone()))?;
        self.folding = false;
        let slot = lower.b.state(&name, ty.clone(), init);

        while self.eat_word(Keyword::On) {
            let (path, filters) = self.slice_ref(lower, events)?;
            let def = self.event_def(events, &path)?;

            lower.b.push_scope();
            let mut binds = Vec::new();
            if self.eat_sym(Sym::LBrace) {
                while !self.at_sym(Sym::RBrace) {
                    let field = self.expect_ident()?;
                    let Some(declared) = def.field(&field) else {
                        return self.fail(format!("{path} has no field `{field}`"));
                    };
                    binds.push(lower.b.bind(&field, Some(declared.ty.clone())));
                    if !self.eat_sym(Sym::Comma) {
                        break;
                    }
                }
                self.expect_sym(Sym::RBrace)?;
            }

            self.expect_sym(Sym::Arrow)?;
            self.folding = true;
            let value = self.expr(lower, Some(ty.clone()))?;
            self.folding = false;
            lower.b.pop_scope();
            lower
                .b
                .slice(path, filters, binds, vec![Update { slot, value }]);
        }

        Ok(())
    }

    fn hoisted_let(&mut self, lower: &mut Lower) -> Result<(), SyntaxError> {
        self.expect_word(Keyword::Let)?;
        let name = self.expect_ident()?;
        self.expect_sym(Sym::Assign)?;
        let value = self.expr(lower, None)?;
        let ty = type_of(lower, value);
        lower.b.hoist(&name, value, ty);
        Ok(())
    }

    fn slice_ref(
        &mut self,
        lower: &mut Lower,
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
            // A filter is lowered once per invocation, before the fold, so it is held
            // to the same rule the fold is: no clock, no call out.
            self.folding = true;
            let value = if self.eat_sym(Sym::Colon) {
                self.expr(lower, Some(expected))?
            } else {
                if lower.b.lookup(&field).is_none() {
                    self.folding = false;
                    return Err(self.not_in_scope(&field, line, col));
                }
                lower.b.at(Span::new(line, col));
                lower.b.load(&field)
            };
            self.folding = false;
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

    fn statements(
        &mut self,
        lower: &mut Lower,
        events: &[EventDef],
    ) -> Result<Vec<Stmt>, SyntaxError> {
        let mut stmts = Vec::new();
        while !self.at_sym(Sym::RBrace) && !matches!(self.peek(), Token::End) {
            stmts.push(self.statement(lower, events)?);
        }
        Ok(stmts)
    }

    fn block(&mut self, lower: &mut Lower, events: &[EventDef]) -> Result<Vec<Stmt>, SyntaxError> {
        self.expect_sym(Sym::LBrace)?;
        let stmts = self.statements(lower, events)?;
        self.expect_sym(Sym::RBrace)?;
        Ok(stmts)
    }

    /// Rule 9: `@subject` is a property of the value, so it propagates from the
    /// event field, through the destructured slot, into the entity field written
    /// from it. Recorded here rather than authored on the entity.
    fn propagate_subject(&mut self, lower: &Lower, entity: &str, field: &str, value: ExprId) {
        let Some(Expr::Load(slot)) = lower.b.exprs().get(value) else {
            return;
        };
        let Some(source) = lower.b.bound_field(*slot) else {
            return;
        };
        let Some(subject) = self
            .event
            .as_ref()
            .and_then(|event| event.field(source))
            .and_then(|declared| declared.subject.clone())
        else {
            return;
        };

        let Some(target) = self
            .entities
            .iter_mut()
            .find(|def| def.name == entity)
            .and_then(|def| def.fields.iter_mut().find(|def| def.name == field))
        else {
            return;
        };
        check_subject(target, &subject);
        target.subject.get_or_insert(subject);
    }
    fn entity_ref(&mut self) -> Result<(Ident, EntityDef), SyntaxError> {
        let (line, col) = self.here();
        let name = self.expect_ident()?;
        match self.entity_def(&name) {
            Some(def) => Ok((name, def.clone())),
            None => Err(self.err(format!("entity `{name}` is not declared"), line, col)),
        }
    }

    /// The `{ field: value, shorthand }` block shared by `put` and `patch`. Closes
    /// the block itself, since both callers do the same thing after.
    fn write_fields(
        &mut self,
        lower: &mut Lower,
        def: &EntityDef,
    ) -> Result<Vec<(Ident, ExprId)>, SyntaxError> {
        let mut fields = Vec::new();
        while !self.at_sym(Sym::RBrace) {
            let (line, col) = self.here();
            let name = self.expect_ident()?;
            let Some(declared) = def.field(&name) else {
                return Err(self.err(
                    format!("entity `{}` has no field `{name}`", def.name),
                    line,
                    col,
                ));
            };
            let expected = declared.ty.clone();
            let value = if self.eat_sym(Sym::Colon) {
                self.expr(lower, Some(expected))?
            } else {
                if lower.b.lookup(&name).is_none() {
                    return Err(self.not_in_scope(&name, line, col));
                }
                lower.b.at(Span::new(line, col));
                lower.b.load(&name)
            };
            self.propagate_subject(lower, &def.name, &name, value);
            fields.push((name, value));
            if !self.eat_sym(Sym::Comma) {
                break;
            }
        }
        self.expect_sym(Sym::RBrace)?;
        Ok(fields)
    }
    fn statement(&mut self, lower: &mut Lower, events: &[EventDef]) -> Result<Stmt, SyntaxError> {
        match self.peek() {
            Token::Word(Keyword::If) => {
                self.bump();
                let cond = self.expr(lower, Some(Type::Bool))?;
                let then = self.block(lower, events)?;
                let otherwise = if self.eat_word(Keyword::Else) {
                    // `else if` is one statement rather than a block, so a chain of
                    // conditions reads as a chain instead of nesting one level per arm.
                    if self.at_word(Keyword::If) {
                        vec![self.statement(lower, events)?]
                    } else {
                        self.block(lower, events)?
                    }
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
                if self.kind != Kind::Command
                    && (self.at_word(Keyword::Invalid) || self.at_word(Keyword::Reject))
                {
                    let outcome = if self.at_word(Keyword::Invalid) {
                        "invalid"
                    } else {
                        "reject"
                    };
                    return self.fail(match self.kind {
                        Kind::Effect => format!(
                            "`{outcome}` is a command's outcome; an effect's terminal outcome is `fail(...)`"
                        ),
                        _ => format!(
                            "`{outcome}` is a command's outcome; a projector write cannot fail in a way the program observes"
                        ),
                    });
                }
                let ret = if self.eat_word(Keyword::Invalid) {
                    self.expect_sym(Sym::LParen)?;
                    let message = self.expr(lower, Some(Type::String))?;
                    self.expect_sym(Sym::RParen)?;
                    Return::Invalid(message)
                } else if self.eat_word(Keyword::Reject) {
                    self.expect_sym(Sym::LParen)?;
                    let code = self.expr(lower, Some(Type::String))?;
                    self.expect_sym(Sym::Comma)?;
                    let message = self.expr(lower, Some(Type::String))?;
                    self.expect_sym(Sym::RParen)?;
                    Return::Reject { code, message }
                } else {
                    Return::Ok
                };
                Ok(Stmt::Return(ret))
            }
            Token::Word(Keyword::Emit) => {
                match self.kind {
                    Kind::Command => {}
                    Kind::Projector => {
                        return self
                            .fail("`emit` appends an event, so it can only appear in a command");
                    }
                    Kind::Effect => {
                        return self.fail(
                            "an effect never appends events; call a command with `invoke`, which appends under its own guard",
                        );
                    }
                }
                let span = self.span_here();
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
                        self.expr(lower, Some(expected))?
                    } else {
                        if lower.b.lookup(&name).is_none() {
                            return Err(self.not_in_scope(&name, line, col));
                        }
                        lower.b.at(Span::new(line, col));
                        lower.b.load(&name)
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
                    span,
                })
            }
            Token::Word(Keyword::Put) => {
                if self.kind != Kind::Projector {
                    return self
                        .fail("`put` writes an entity, so it can only appear in a projector");
                }
                let span = self.span_here();
                self.bump();
                let (name, def) = self.entity_ref()?;
                self.expect_sym(Sym::LBrace)?;
                let fields = self.write_fields(lower, &def)?;
                Ok(Stmt::Put {
                    entity: name,
                    fields,
                    span,
                })
            }
            Token::Word(Keyword::Patch) => {
                if self.kind != Kind::Projector {
                    return self
                        .fail("`patch` writes an entity, so it can only appear in a projector");
                }
                let span = self.span_here();
                self.bump();
                let (name, def) = self.entity_ref()?;

                self.expect_sym(Sym::LBracket)?;
                let key = self.expr(lower, Some(def.key_field().ty.clone()))?;
                self.expect_sym(Sym::RBracket)?;

                self.expect_sym(Sym::LBrace)?;
                self.stored = Some(Stored {
                    entity: def.clone(),
                    loads: Vec::new(),
                    slots: HashMap::new(),
                });
                let fields = self.write_fields(lower, &def)?;
                let stored = self.stored.take().expect("set just above");

                Ok(Stmt::Patch {
                    entity: name,
                    key,
                    loads: stored.loads,
                    fields,
                    span,
                })
            }
            Token::Word(Keyword::Delete) => {
                if self.kind != Kind::Projector {
                    return self
                        .fail("`delete` writes an entity, so it can only appear in a projector");
                }
                self.bump();
                let (name, def) = self.entity_ref()?;
                self.expect_sym(Sym::LBracket)?;
                let key = self.expr(lower, Some(def.key_field().ty.clone()))?;
                self.expect_sym(Sym::RBracket)?;
                Ok(Stmt::Delete { entity: name, key })
            }
            Token::Word(Keyword::Let) => {
                self.bump();
                let name = self.expect_ident()?;
                self.expect_sym(Sym::Assign)?;
                let value = self.expr(lower, None)?;
                let ty = type_of(lower, value);
                let slot = lower.b.alloc(&name, ty);
                Ok(Stmt::Assign { slot, value })
            }
            Token::Word(Keyword::Invoke) => {
                let span = self.span_here();
                self.bump();
                let value = self.invoke_expr(lower, span)?;
                Ok(Stmt::Discard(value))
            }
            Token::Word(Keyword::State) | Token::Word(Keyword::Guard) => {
                self.fail("`state` and `guard` must come before the first statement")
            }
            Token::Ident(name) if self.starts_effect_statement(name) => {
                self.effect_statement(lower, events)
            }
            other => self.fail(format!("expected a statement, found {other}")),
        }
    }

    fn expr(&mut self, lower: &mut Lower, expect: Option<Type>) -> Result<ExprId, SyntaxError> {
        self.or_expr(lower, expect)
    }

    fn or_expr(&mut self, lower: &mut Lower, expect: Option<Type>) -> Result<ExprId, SyntaxError> {
        let mut lhs = self.and_expr(lower, expect)?;
        while let Some(span) = self.eat_at(Sym::OrOr) {
            let rhs = self.and_expr(lower, Some(Type::Bool))?;
            lower.b.at(span);
            lhs = lower.b.binary(BinOp::Or, lhs, rhs);
        }
        Ok(lhs)
    }

    fn and_expr(&mut self, lower: &mut Lower, expect: Option<Type>) -> Result<ExprId, SyntaxError> {
        let mut lhs = self.cmp_expr(lower, expect)?;
        while let Some(span) = self.eat_at(Sym::AndAnd) {
            let rhs = self.cmp_expr(lower, Some(Type::Bool))?;
            lower.b.at(span);
            lhs = lower.b.binary(BinOp::And, lhs, rhs);
        }
        Ok(lhs)
    }

    fn cmp_expr(&mut self, lower: &mut Lower, expect: Option<Type>) -> Result<ExprId, SyntaxError> {
        let lhs = self.add_expr(lower, expect)?;
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

        let hint = self.hint_from(lower, lhs);
        let rhs = self.add_expr(lower, hint)?;
        self.settle(lower, lhs, rhs);
        lower.b.at(span);
        Ok(lower.b.binary(op, lhs, rhs))
    }

    fn add_expr(&mut self, lower: &mut Lower, expect: Option<Type>) -> Result<ExprId, SyntaxError> {
        let mut lhs = self.mul_expr(lower, expect.clone())?;
        while let Some((op, span)) =
            self.eat_op(&[(Sym::Plus, BinOp::Add), (Sym::Minus, BinOp::Sub)])
        {
            let hint = self.hint_from(lower, lhs).or_else(|| expect.clone());
            let rhs = self.mul_expr(lower, hint)?;
            self.settle(lower, lhs, rhs);
            lower.b.at(span);
            lhs = lower.b.binary(op, lhs, rhs);
        }
        Ok(lhs)
    }

    fn mul_expr(&mut self, lower: &mut Lower, expect: Option<Type>) -> Result<ExprId, SyntaxError> {
        let mut lhs = self.unary_expr(lower, expect)?;
        while let Some((op, span)) = self.eat_op(&[
            (Sym::Star, BinOp::Mul),
            (Sym::Slash, BinOp::Div),
            (Sym::Percent, BinOp::Rem),
        ]) {
            let rhs = self.unary_expr(lower, None)?;
            lower.b.at(span);
            lhs = lower.b.binary(op, lhs, rhs);
        }
        Ok(lhs)
    }

    fn unary_expr(
        &mut self,
        lower: &mut Lower,
        expect: Option<Type>,
    ) -> Result<ExprId, SyntaxError> {
        if let Some(span) = self.eat_at(Sym::Bang) {
            let operand = self.unary_expr(lower, Some(Type::Bool))?;
            lower.b.at(span);
            return Ok(lower.b.unary(UnOp::Not, operand));
        }
        if let Some(span) = self.eat_at(Sym::Minus) {
            let operand = self.unary_expr(lower, expect)?;
            lower.b.at(span);
            return Ok(lower.b.unary(UnOp::Neg, operand));
        }
        self.postfix_expr(lower, expect)
    }

    fn postfix_expr(
        &mut self,
        lower: &mut Lower,
        expect: Option<Type>,
    ) -> Result<ExprId, SyntaxError> {
        let mut value = self.primary(lower, expect)?;
        while self.eat_sym(Sym::Dot) {
            let span = self.span_here();
            let (line, col) = self.here();
            let name = self.expect_ident()?;

            if self.eat_sym(Sym::LParen) {
                let mut args = Vec::new();
                while !self.at_sym(Sym::RParen) {
                    args.push(self.expr(lower, None)?);
                    if !self.eat_sym(Sym::Comma) {
                        break;
                    }
                }
                self.expect_sym(Sym::RParen)?;
                lower.b.at(span);
                value = lower.b.method(value, &name, args);
                continue;
            }

            // Parenless, so a field rather than a method. Only a `Response` has any,
            // and where the receiver's type is known the mistake is caught here rather
            // than at run time, which is where `total.trim` used to be caught.
            match type_of(lower, value) {
                Some(Type::Response) if response_field(&name).is_some() => {}
                Some(Type::Response) => {
                    return Err(self.err(
                        format!("a Response carries `status` and `body`, not `{name}`"),
                        line,
                        col,
                    ));
                }
                Some(ty) => {
                    return Err(self.err(
                        format!("no field `{name}` on {ty}; did you mean `{name}()`?"),
                        line,
                        col,
                    ));
                }
                None => {}
            }
            lower.b.at(span);
            value = lower.b.expr(Expr::Field {
                receiver: value,
                name,
            });
        }
        Ok(value)
    }

    fn primary(&mut self, lower: &mut Lower, expect: Option<Type>) -> Result<ExprId, SyntaxError> {
        let spanned = self.bump();
        let span = Span::new(spanned.line, spanned.col);
        lower.b.at(span);
        match spanned.token {
            Token::Number(number) => self.number(lower, number, expect, &spanned),
            Token::Text(text) => Ok(lower.b.lit(Literal::Str(text))),
            Token::Word(Keyword::True) => Ok(lower.b.bool(true)),
            Token::Word(Keyword::False) => Ok(lower.b.bool(false)),
            Token::Word(Keyword::If) => {
                let cond = self.expr(lower, Some(Type::Bool))?;
                self.expect_sym(Sym::LBrace)?;
                let then = self.expr(lower, expect.clone())?;
                self.expect_sym(Sym::RBrace)?;
                self.expect_word(Keyword::Else)?;
                self.expect_sym(Sym::LBrace)?;
                let otherwise = self.expr(lower, expect)?;
                self.expect_sym(Sym::RBrace)?;
                lower.b.at(span);
                Ok(lower.b.if_expr(cond, then, otherwise))
            }
            Token::Sym(Sym::LParen) => {
                let value = self.expr(lower, expect)?;
                self.expect_sym(Sym::RParen)?;
                Ok(value)
            }
            Token::Word(Keyword::None) => match expect {
                Some(Type::Opt(inner)) => Ok(lower.b.none(inner.as_ref().clone())),
                Some(found) => Err(self.err(
                    format!("`none` needs an optional target, but this position is {found}"),
                    spanned.line,
                    spanned.col,
                )),
                None => Err(self.err(
                    "`none` needs an optional target to know what it is none of",
                    spanned.line,
                    spanned.col,
                )),
            },
            Token::Word(Keyword::Invoke) => self.invoke_expr(lower, span),
            Token::Sym(Sym::LBrace) => self.object_literal(lower, span),
            Token::TextOpen(head) => self.interpolation(lower, head, span),
            Token::Sym(Sym::Dot) => {
                let (line, col) = self.here();
                let field = self.expect_ident()?;

                let ty = match self.stored.as_ref() {
                    None => {
                        return Err(self.err(
                            format!(
                                "`.{field}` reads the stored value, which only a `patch` value can do"
                            ),
                            span.line,
                            span.col,
                        ));
                    }
                    Some(stored) => match stored.entity.field(&field) {
                        Some(declared) => declared.ty.clone(),
                        None => {
                            return Err(self.err(
                                format!("entity `{}` has no field `{field}`", stored.entity.name),
                                line,
                                col,
                            ));
                        }
                    },
                };

                // One slot per distinct `.field` in this patch; the interpreter fills
                // them from the stored row before any value expression runs, so this
                // is an ordinary load by the time `eval` sees it.
                let stored = self.stored.as_mut().expect("checked just above");
                let slot = match stored.slots.get(&field) {
                    Some(slot) => *slot,
                    None => {
                        let slot = lower.b.alloc(format!(".{field}"), Some(ty));
                        stored.slots.insert(field.clone(), slot);
                        stored.loads.push(Bind { field, slot });
                        slot
                    }
                };
                Ok(lower.b.read(slot))
            }
            Token::Ident(name) => {
                if self.envelope.as_deref() == Some(name.as_str()) {
                    return self.envelope_access(lower, &name, span);
                }
                if lower.b.lookup(&name).is_some() {
                    return Ok(lower.b.load(&name));
                }
                if let Some(value) = self.builtin(lower, &name, span)? {
                    return Ok(value);
                }
                if let Some(Type::Enum(enum_name)) = expect.as_ref().map(inner_of)
                    && let Some(def) = self.enum_def(enum_name)
                {
                    if !def.has(&name) {
                        return Err(self.err(
                            format!("`{enum_name}` has no variant `{name}`"),
                            spanned.line,
                            spanned.col,
                        ));
                    }
                    let ty = enum_name.clone();
                    return Ok(lower.b.lit(Literal::Enum { ty, variant: name }));
                }
                if let Some(mode) = rounding_mode(&name) {
                    return Ok(lower.b.rounding(mode));
                }
                match self.enums.iter().filter(|def| def.has(&name)).count() {
                    1 => {
                        let ty = self
                            .enums
                            .iter()
                            .find(|def| def.has(&name))
                            .expect("counted one")
                            .name
                            .clone();
                        Ok(lower.b.lit(Literal::Enum { ty, variant: name }))
                    }
                    0 => Err(self.not_in_scope(&name, spanned.line, spanned.col)),
                    _ => {
                        let candidates: Vec<&str> = self
                            .enums
                            .iter()
                            .filter(|def| def.has(&name))
                            .map(|def| def.name.as_str())
                            .collect();
                        Err(self.err(
                            format!(
                                "`{name}` is a variant of {}, so it is ambiguous here; the target type would decide it",
                                candidates.join(" and ")
                            ),
                            spanned.line,
                            spanned.col,
                        ))
                    }
                }
            }

            other => Err(self.err(
                format!("expected a value, found {other}"),
                spanned.line,
                spanned.col,
            )),
        }
    }

    fn number(
        &mut self,
        lower: &mut Lower,
        number: Number,
        expect: Option<Type>,
        at: &Spanned,
    ) -> Result<ExprId, SyntaxError> {
        let defaulted = expect.is_none();
        // A literal in a `T?` position is still a `T`; the write site wraps it.
        let ty = match expect {
            Some(ty) => inner_of(&ty).clone(),
            None => default_type(number),
        };
        let lit = number
            .resolve(&ty)
            .map_err(|err| self.err(err.to_string(), at.line, at.col))?;
        let id = lower.b.lit(lit);
        if defaulted {
            lower.defaults.insert(id, number);
        }
        Ok(id)
    }

    fn hint_from(&self, lower: &Lower, id: ExprId) -> Option<Type> {
        if lower.defaults.contains_key(&id) {
            return None;
        }
        type_of(lower, id)
    }

    fn settle(&self, lower: &mut Lower, lhs: ExprId, rhs: ExprId) {
        match (
            lower.defaults.get(&lhs).copied(),
            lower.defaults.get(&rhs).copied(),
        ) {
            (Some(left), Some(right)) => {
                if left.scale > right.scale {
                    self.retype(lower, rhs, &default_type(left));
                } else if right.scale > left.scale {
                    self.retype(lower, lhs, &default_type(right));
                }
            }
            (Some(_), None) => {
                if let Some(ty) = type_of(lower, rhs) {
                    self.retype(lower, lhs, &ty);
                }
            }
            (None, Some(_)) => {
                if let Some(ty) = type_of(lower, lhs) {
                    self.retype(lower, rhs, &ty);
                }
            }
            (None, None) => {}
        }
    }

    fn retype(&self, lower: &mut Lower, id: ExprId, ty: &Type) {
        let Some(number) = lower.defaults.get(&id).copied() else {
            return;
        };
        if let Ok(lit) = number.resolve(ty) {
            lower.b.patch(id, Expr::Lit(lit));
            lower.defaults.remove(&id);
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

/// Looks through `T?` to the `T` a literal in that position is really making.
fn inner_of(ty: &Type) -> &Type {
    match ty {
        Type::Opt(inner) => inner,
        _ => ty,
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

fn type_of(lower: &Lower, id: ExprId) -> Option<Type> {
    match lower.b.exprs().get(id)? {
        Expr::Lit(lit) => Some(value::literal(lit).ty()),
        Expr::Load(slot) => lower.b.slot_type(*slot).cloned(),
        Expr::Unary { operand, .. } => type_of(lower, *operand),
        Expr::Binary { op, lhs, rhs } => match op {
            BinOp::Eq
            | BinOp::Ne
            | BinOp::Lt
            | BinOp::Le
            | BinOp::Gt
            | BinOp::Ge
            | BinOp::And
            | BinOp::Or => Some(Type::Bool),
            _ => type_of(lower, *lhs).or_else(|| type_of(lower, *rhs)),
        },
        Expr::Method { .. } => None,
        Expr::If { then, .. } => type_of(lower, *then),
        Expr::Field { name, .. } => response_field(name),
        Expr::Object(_) => Some(Type::Json),
        Expr::Interp(_) => Some(Type::String),
        Expr::Call { builtin, .. } => Some(match builtin {
            Builtin::UuidDerive => Type::Uuid,
            _ => Type::Response,
        }),
        Expr::Invoke { .. } => Some(Type::Outcome),
        Expr::Reveal { .. } => Some(Type::String),
    }
}

/// Rule 11 stated where an author looks for it. `Uuid.new` sits next to `derive` in
/// the same namespace, so its absence is visible rather than something to notice.
fn uuid_member(member: &str) -> String {
    match member {
        "new" | "random" | "generate" | "v4" => format!(
            "`Uuid` has no `{member}`: an id has to be derived from its inputs, so that a command retry and an effect replay produce the id they produced the first time; write `Uuid.derive(seed, name)`"
        ),
        _ => format!("`Uuid` has no `{member}`; it has `derive(seed, name)`"),
    }
}

/// The two fields a `Response` carries. Parenless field access exists for these and
/// nothing else, so this doubles as the check.
fn response_field(name: &str) -> Option<Type> {
    Some(match name {
        "status" => Type::Int,
        "body" => Type::Json,
        _ => return None,
    })
}

/// Rule 9: the two subject checks are not implemented yet. When they are, this is
/// where the conflict one belongs, asserting what hekla's `enforce_subject_columns`
/// does: a field written under two different subjects is an error, and a
/// subject-bound value may not land in a field that discards the binding. The
/// propagation that feeds them is live, and every handler stays in the IR, so the
/// checker can recover both spans; only the checking is deferred.
fn check_subject(_target: &EntityField, _incoming: &Ident) {}

impl Parser {
    fn effect_decl(&mut self, events: &[EventDef]) -> Result<Effect, SyntaxError> {
        let module = self.module_at(self.pos).map(str::to_string);
        self.expect_word(Keyword::Effect)?;
        let name = self.expect_ident()?;
        self.expect_sym(Sym::LBrace)?;

        let mut arms: Vec<Arm> = Vec::new();
        let mut at: Vec<usize> = Vec::new();
        while !self.at_sym(Sym::RBrace) && !matches!(self.peek(), Token::End) {
            if !self.at_word(Keyword::On) {
                return self.fail(format!("expected `on`, found {}", self.peek()));
            }
            let start = self.pos;
            let arm = self.arm(&name, events)?;
            // Rule 1: one event selects exactly one arm, so two arms on a type would
            // make declaration order decide what a replay does.
            if let Some(index) = arms.iter().position(|other| other.event == arm.event) {
                let first = self.location(at[index]);
                return Err(self.err(
                    format!(
                        "`{name}` already has an arm on {} at {first}; one event selects exactly one arm",
                        arm.event
                    ),
                    arm.span.line,
                    arm.span.col,
                ));
            }
            arms.push(arm);
            at.push(start);
        }
        self.expect_sym(Sym::RBrace)?;

        if arms.is_empty() {
            return self.fail(format!("effect `{name}` declares no arms"));
        }
        Ok(Effect { name, module, arms })
    }

    fn arm(&mut self, effect: &Ident, events: &[EventDef]) -> Result<Arm, SyntaxError> {
        let span = self.span_here();
        self.expect_word(Keyword::On)?;
        let path = self.expect_path()?;
        let def = self.event_def(events, &path)?.clone();

        let mut lower = Lower {
            b: Builder::new(effect),
            defaults: HashMap::new(),
        };

        let envelope = if self.eat_word(Keyword::As) {
            Some(self.expect_ident()?)
        } else {
            None
        };
        // Rule 2: the trigger binding is in scope for the arm's `state` filters as well
        // as its body, so it is registered before the prologue runs.
        self.event = Some(def.clone());
        self.envelope = envelope;
        self.kind = Kind::Effect;

        self.destructure_block(&mut lower, &def)?;

        self.expect_sym(Sym::LBrace)?;
        self.command_end = self.command_end();

        // An arm's prologue is `state` alone. A command hoists a leading `let` so a
        // filter can name it; rule 2 gives an arm's filters the trigger binding
        // instead, so a `let` here is an ordinary body statement and can call out.
        self.prologue = true;
        loop {
            match self.peek() {
                Token::Word(Keyword::State) => self.state_decl(&mut lower, events)?,
                Token::Word(Keyword::Guard) => {
                    return self.fail(
                        "an effect has no `guard`; it appends nothing, so there is no append condition to build",
                    );
                }
                _ => break,
            }
        }
        self.prologue = false;

        let body = self.statements(&mut lower, events)?;
        self.expect_sym(Sym::RBrace)?;
        self.event = None;
        self.envelope = None;
        self.kind = Kind::Command;

        let arm = lower.b.finish_arm(path, span, body);
        if let Err((erase, reveal)) = erase_last(&arm.exprs, &arm.body) {
            return Err(self.err(
                format!(
                    "`reveal` at {reveal} can run after the `erase` at {erase}; `erase` is journaled and `reveal` is not, so a replay skips the erase and re-runs the reveal against a key that is gone"
                ),
                reveal.line,
                reveal.col,
            ));
        }
        Ok(arm)
    }

    /// Whether a statement begins with one of the soft-named effect builtins. They are
    /// not keywords (rule 10), so this is recognised by shape rather than by token.
    fn starts_effect_statement(&self, name: &str) -> bool {
        let next = self.tokens.get(self.pos + 1).map(|next| &next.token);
        match name {
            "fail" | "log" | "erase" => matches!(next, Some(Token::Sym(Sym::LParen))),
            "http" => matches!(next, Some(Token::Sym(Sym::Dot))),
            _ => false,
        }
    }

    fn effect_statement(
        &mut self,
        lower: &mut Lower,
        events: &[EventDef],
    ) -> Result<Stmt, SyntaxError> {
        let span = self.span_here();
        let Token::Ident(name) = self.peek().clone() else {
            return self.fail("expected a statement");
        };

        match name.as_str() {
            "fail" => {
                self.gate(
                    "`fail` is an effect's terminal outcome; a command returns `invalid(...)` or `reject(...)`",
                    "`fail` is an effect's terminal outcome; a projector write cannot fail in a way the program observes",
                    span,
                )?;
                self.bump();
                self.expect_sym(Sym::LParen)?;
                let message = self.expr(lower, Some(Type::String))?;
                self.expect_sym(Sym::RParen)?;
                Ok(Stmt::Fail { message, span })
            }
            "log" => {
                self.gate(
                    "`log` is an effect builtin; a command's decision is already visible in what it emits",
                    "`log` is an effect builtin; a projector runs once per rebuild, so its lines are not a trace",
                    span,
                )?;
                self.bump();
                self.expect_sym(Sym::LParen)?;
                let message = self.expr(lower, Some(Type::String))?;
                self.expect_sym(Sym::RParen)?;
                Ok(Stmt::Log { message })
            }
            "erase" => {
                self.gate(
                    "only an effect crosses the decrypt boundary; a command decides from state without reaching personal data",
                    "only an effect crosses the decrypt boundary; a projector stores what the event carries",
                    span,
                )?;
                self.bump();
                self.expect_sym(Sym::LParen)?;
                let (line, col) = self.here();
                let value = self.expr(lower, None)?;
                self.expect_sym(Sym::RParen)?;

                let subject = self.subject_source(lower, value, line, col, "erase")?;
                let scoped = events.iter().any(|def| {
                    def.fields
                        .iter()
                        .any(|field| field.subject.as_deref() == Some(subject.as_str()))
                });
                if !scoped {
                    return Err(self.err(
                        format!(
                            "nothing is scoped to `{subject}`, so there is no key to erase; `erase` takes the subject id that a field is declared `@subject(...)` of"
                        ),
                        line,
                        col,
                    ));
                }
                Ok(Stmt::Erase {
                    subject,
                    value,
                    span,
                })
            }
            // `http.*`, whose result this statement discards.
            _ => {
                let value = self.expr(lower, None)?;
                Ok(Stmt::Discard(value))
            }
        }
    }

    /// Rule 5's gating. Each wrong context gets a message about that context, so the
    /// error teaches the rule at the point of violation rather than naming a category.
    /// Rule 3: a `state` fold is not journaled, because every attempt re-folds and
    /// gets the same answer. That only holds if the fold cannot call out or decrypt.
    fn not_in_fold(&self, what: &str, span: Span) -> Result<(), SyntaxError> {
        if self.folding {
            return Err(self.err(
                format!("`state` folds the log, so it cannot {what}"),
                span.line,
                span.col,
            ));
        }
        Ok(())
    }

    fn gate(&self, command: &str, projector: &str, span: Span) -> Result<(), SyntaxError> {
        match self.kind {
            Kind::Effect => Ok(()),
            Kind::Command => Err(self.err(command, span.line, span.col)),
            Kind::Projector => Err(self.err(projector, span.line, span.col)),
        }
    }

    /// The soft-named builtins, resolved after the scope lookup so a local shadows one.
    /// That is what lets `log` and the rest stay usable as ordinary names (rule 10).
    fn builtin(
        &mut self,
        lower: &mut Lower,
        name: &str,
        span: Span,
    ) -> Result<Option<ExprId>, SyntaxError> {
        let called = self.at_sym(Sym::LParen);
        match name {
            "http" if self.at_sym(Sym::Dot) => self.http_call(lower, span).map(Some),
            "Uuid" if self.at_sym(Sym::Dot) => self.uuid_call(lower, span).map(Some),
            "reveal" if called => self.reveal_call(lower, span).map(Some),
            "now" if called => {
                // Rule 11's clock rule: a clock exists where its result is pinned or
                // journaled, and is absent where replay demands determinism.
                if self.kind == Kind::Projector {
                    return Err(self.err(
                        "a projector has no clock, because a rebuild must reproduce every value it writes",
                        span.line,
                        span.col,
                    ));
                }
                self.not_in_fold("read a clock", span)?;
                self.bump();
                self.expect_sym(Sym::RParen)?;
                let slot = lower.b.now();
                lower.b.at(span);
                Ok(Some(lower.b.read(slot)))
            }
            "erase" if called => Err(self.err(
                "`erase` is a statement rather than a value; it returns nothing, because there is nothing an author could do differently on either answer",
                span.line,
                span.col,
            )),
            "fail" | "log" if called => Err(self.err(
                format!("`{name}` is a statement rather than a value"),
                span.line,
                span.col,
            )),
            "uuid4" | "random" | "uuid5" if called => Err(self.err(
                format!(
                    "there is no `{name}` in heklang: an id has to be derived from its inputs, so that a command retry and an effect replay produce the id they produced the first time; write `Uuid.derive(seed, name)`"
                ),
                span.line,
                span.col,
            )),
            _ => Ok(None),
        }
    }

    /// The language's first type-qualified call. The global namespace holds actions
    /// with no natural receiver, so a value constructed from nothing is named by its
    /// type instead; `Json.parse` and `Timestamp.from_micros` will take this shape too.
    fn uuid_call(&mut self, lower: &mut Lower, span: Span) -> Result<ExprId, SyntaxError> {
        self.expect_sym(Sym::Dot)?;
        let (line, col) = self.here();
        let member = self.expect_ident()?;
        if member != "derive" {
            return Err(self.err(uuid_member(&member), line, col));
        }
        self.expect_sym(Sym::LParen)?;
        let seed = self.expr(lower, Some(Type::Uuid))?;
        self.expect_sym(Sym::Comma)?;
        let name = self.expr(lower, Some(Type::String))?;
        self.expect_sym(Sym::RParen)?;
        lower.b.at(span);
        Ok(lower.b.expr(Expr::Call {
            builtin: Builtin::UuidDerive,
            args: vec![seed, name],
        }))
    }

    fn http_call(&mut self, lower: &mut Lower, span: Span) -> Result<ExprId, SyntaxError> {
        self.expect_sym(Sym::Dot)?;
        let (line, col) = self.here();
        let verb = self.expect_ident()?;
        let Some(builtin) = Builtin::verb(&verb) else {
            return Err(self.err(
                format!("`http` has no verb `{verb}`; it has get, post, put, patch and delete"),
                line,
                col,
            ));
        };
        self.gate(
            "a command decides from state and appends; only an effect can call out, because only an effect journals the call",
            "a projector is a pure fold over the log, so it cannot make an HTTP call",
            span,
        )?;
        self.not_in_fold("call out", span)?;

        self.expect_sym(Sym::LParen)?;
        let mut args = vec![self.expr(lower, Some(Type::String))?];
        if builtin.has_body() {
            self.expect_sym(Sym::Comma)?;
            let outer = self.in_body;
            self.in_body = true;
            args.push(self.expr(lower, Some(Type::Json))?);
            self.in_body = outer;
        }
        if self.at_sym(Sym::Comma) {
            // Rule 13: a timeout belongs to configuration, not to the call site.
            let (line, col) = self.here();
            let plural = if args.len() == 1 { "" } else { "s" };
            return Err(self.err(
                format!(
                    "`{}` takes {} argument{plural}; a timeout is configuration rather than a call argument",
                    builtin.name(),
                    args.len()
                ),
                line,
                col,
            ));
        }
        self.expect_sym(Sym::RParen)?;
        lower.b.at(span);
        Ok(lower.b.expr(Expr::Call { builtin, args }))
    }

    fn reveal_call(&mut self, lower: &mut Lower, span: Span) -> Result<ExprId, SyntaxError> {
        self.gate(
            "only an effect crosses the decrypt boundary; a command decides from state without reaching personal data",
            "only an effect crosses the decrypt boundary; a projector stores what the event carries",
            span,
        )?;
        self.not_in_fold("decrypt", span)?;
        self.expect_sym(Sym::LParen)?;
        let (line, col) = self.here();
        let value = self.expr(lower, None)?;
        self.expect_sym(Sym::RParen)?;

        let field = self.subject_source(lower, value, line, col, "reveal")?;
        let def = self.event.clone().expect("only inside an arm");
        let Some(declared) = def.field(&field) else {
            return Err(self.err(format!("{} has no field `{field}`", def.path), line, col));
        };
        let Some(subject) = declared.subject.clone() else {
            return Err(self.err(
                format!(
                    "`{field}` is not subject-encrypted, so there is nothing to reveal; only a field declared `@subject(...)` needs it"
                ),
                line,
                col,
            ));
        };
        let Some(id) = def.field(&subject) else {
            return Err(self.err(
                format!(
                    "`@subject({subject})` on `{field}` names no field of {}",
                    def.path
                ),
                line,
                col,
            ));
        };

        let slot = lower.b.payload(&subject, Some(id.ty.clone()));
        let subject_value = lower.b.read(slot);
        lower.b.at(span);
        Ok(lower.b.expr(Expr::Reveal {
            value,
            field,
            subject,
            subject_value,
        }))
    }

    /// The event field an expression loads. `reveal` and `erase` both name a value and
    /// recover the schema fact from it, because subject-ness is a property of the path
    /// rather than of the value.
    fn subject_source(
        &self,
        lower: &Lower,
        value: ExprId,
        line: u32,
        col: u32,
        what: &str,
    ) -> Result<Ident, SyntaxError> {
        let bound = match lower.b.exprs().get(value) {
            Some(Expr::Load(slot)) => lower.b.bound_field(*slot),
            _ => None,
        };
        match bound {
            Some(field) => Ok(field.to_string()),
            None => Err(self.err(
                format!("`{what}` takes a field of the triggering event, like `e.email`"),
                line,
                col,
            )),
        }
    }

    /// `TextOpen`, then a hole, then a `TextPart` before each further hole, then
    /// `TextClose`. The lexer flattened the nesting into that sequence, so this reads
    /// it straight through without a stack of its own.
    fn interpolation(
        &mut self,
        lower: &mut Lower,
        head: String,
        span: Span,
    ) -> Result<ExprId, SyntaxError> {
        lower.b.at(span);
        let mut parts = vec![lower.b.str(&head)];
        loop {
            parts.push(self.expr(lower, None)?);
            let spanned = self.bump();
            match spanned.token {
                Token::TextPart(text) => {
                    lower.b.at(Span::new(spanned.line, spanned.col));
                    parts.push(lower.b.str(&text));
                }
                Token::TextClose(text) => {
                    lower.b.at(Span::new(spanned.line, spanned.col));
                    parts.push(lower.b.str(&text));
                    lower.b.at(span);
                    return Ok(lower.b.expr(Expr::Interp(parts)));
                }
                other => {
                    return Err(self.err(
                        format!("expected the rest of the string, found {other}"),
                        spanned.line,
                        spanned.col,
                    ));
                }
            }
        }
    }

    fn object_literal(&mut self, lower: &mut Lower, span: Span) -> Result<ExprId, SyntaxError> {
        if !self.in_body {
            return Err(self.err(
                "an object literal is an HTTP request body; `invoke` takes a typed struct, checked against the command's parameters",
                span.line,
                span.col,
            ));
        }

        let mut fields: Vec<(Ident, ExprId)> = Vec::new();
        while !self.at_sym(Sym::RBrace) {
            let (line, col) = self.here();
            let key = match self.bump().token {
                Token::Text(text) => text,
                other => {
                    return Err(self.err(
                        format!("an object key is a quoted string, found {other}"),
                        line,
                        col,
                    ));
                }
            };
            if fields.iter().any(|(seen, _)| seen == &key) {
                return Err(self.err(format!("`{key}` is given twice"), line, col));
            }
            self.expect_sym(Sym::Colon)?;
            let value = self.expr(lower, None)?;
            fields.push((key, value));
            if !self.eat_sym(Sym::Comma) {
                break;
            }
        }
        self.expect_sym(Sym::RBrace)?;
        lower.b.at(span);
        Ok(lower.b.expr(Expr::Object(fields)))
    }

    fn invoke_expr(&mut self, lower: &mut Lower, span: Span) -> Result<ExprId, SyntaxError> {
        self.gate(
            "`invoke` calls a command, so it can only appear in an effect; a command that needs another command's work emits, and an effect reacts",
            "`invoke` calls a command, so it can only appear in an effect; a projector is a pure fold",
            span,
        )?;
        self.not_in_fold("call a command", span)?;

        let (line, col) = self.here();
        let name = self.expect_ident()?;
        let Some(signature) = self
            .commands
            .iter()
            .find(|other| other.name == name)
            .cloned()
        else {
            return Err(self.err(format!("command `{name}` is not declared"), line, col));
        };

        // Rule 7: the input is a typed struct, checked against the declared parameters.
        self.expect_sym(Sym::LBrace)?;
        let mut args: Vec<(Ident, ExprId)> = Vec::new();
        while !self.at_sym(Sym::RBrace) {
            let (line, col) = self.here();
            let field = self.expect_ident()?;
            let Some((_, declared)) = signature.params.iter().find(|(param, _)| param == &field)
            else {
                return Err(self.err(
                    format!("command `{name}` has no parameter `{field}`"),
                    line,
                    col,
                ));
            };
            if args.iter().any(|(seen, _)| seen == &field) {
                return Err(self.err(format!("`{field}` is given twice"), line, col));
            }

            let expected = declared.clone();
            let value = if self.eat_sym(Sym::Colon) {
                self.expr(lower, Some(expected))?
            } else {
                if lower.b.lookup(&field).is_none() {
                    return Err(self.not_in_scope(&field, line, col));
                }
                lower.b.at(Span::new(line, col));
                lower.b.load(&field)
            };
            args.push((field, value));
            if !self.eat_sym(Sym::Comma) {
                break;
            }
        }
        self.expect_sym(Sym::RBrace)?;

        for (param, declared) in &signature.params {
            // An omitted optional parameter binds `none`, exactly as it does for a
            // command called from outside.
            if matches!(declared, Type::Opt(_)) {
                continue;
            }
            if !args.iter().any(|(given, _)| given == param) {
                return Err(self.err(
                    format!("command `{name}` needs `{param}`"),
                    span.line,
                    span.col,
                ));
            }
        }

        lower.b.at(span);
        Ok(lower.b.expr(Expr::Invoke {
            command: name,
            args,
        }))
    }

    /// No effect may trigger itself, directly or through a chain of invokes. An edge
    /// `trigger -> emitted` for every (arm, command it invokes, event that command
    /// emits); a cycle in that graph is an unbounded event stream.
    fn check_cycles(&self, program: &Program) -> Result<(), SyntaxError> {
        let mut edges: Vec<Edge> = Vec::new();
        for effect in &program.effects {
            for arm in &effect.arms {
                for command in invoked(&arm.exprs, &arm.body) {
                    let Some(target) = program.command(&command) else {
                        continue;
                    };
                    for event in emitted(&target.body) {
                        edges.push(Edge {
                            from: arm.event.clone(),
                            to: event,
                            effect: effect.name.clone(),
                            command: command.clone(),
                            module: effect.module.clone(),
                            span: arm.span,
                        });
                    }
                }
            }
        }

        let Some(cycle) = find_cycle(&edges) else {
            return Ok(());
        };

        let mut path = cycle[0].from.to_string();
        for edge in &cycle {
            path.push_str(" -> ");
            path.push_str(&edge.effect);
            path.push_str(" -> ");
            path.push_str(&edge.command);
            path.push_str(" -> ");
            path.push_str(&edge.to.to_string());
        }
        let error = SyntaxError::new(
            format!("{path}: this effect can trigger itself, so the log would grow without end"),
            cycle[0].span.line,
            cycle[0].span.col,
        );
        Err(match &cycle[0].module {
            Some(module) => error.in_file(module),
            None => error,
        })
    }
}

struct Edge {
    from: EventPath,
    to: EventPath,
    effect: Ident,
    command: Ident,
    module: Option<Ident>,
    span: Span,
}

fn find_cycle(edges: &[Edge]) -> Option<Vec<&Edge>> {
    for start in edges {
        let mut nodes = vec![start.from.clone()];
        let mut path: Vec<&Edge> = Vec::new();
        if let Some(found) = descend(edges, &mut nodes, &mut path) {
            return Some(found);
        }
    }
    None
}

fn descend<'a>(
    edges: &'a [Edge],
    nodes: &mut Vec<EventPath>,
    path: &mut Vec<&'a Edge>,
) -> Option<Vec<&'a Edge>> {
    let node = nodes.last().expect("never empty").clone();
    for edge in edges.iter().filter(|edge| edge.from == node) {
        if let Some(start) = nodes.iter().position(|seen| seen == &edge.to) {
            let mut found: Vec<&Edge> = path[start..].to_vec();
            found.push(edge);
            return Some(found);
        }
        nodes.push(edge.to.clone());
        path.push(edge);
        if let Some(found) = descend(edges, nodes, path) {
            return Some(found);
        }
        path.pop();
        nodes.pop();
    }
    None
}

fn emitted(body: &[Stmt]) -> Vec<EventPath> {
    let mut found = Vec::new();
    walk_stmts(body, &mut |stmt| {
        if let Stmt::Emit { event, .. } = stmt {
            found.push(event.clone());
        }
    });
    found
}

fn invoked(exprs: &Exprs, body: &[Stmt]) -> Vec<Ident> {
    let mut found = Vec::new();
    walk_stmts(body, &mut |stmt| {
        for root in roots(stmt) {
            collect(exprs, root, &mut found, &|_, expr, out| {
                if let Expr::Invoke { command, .. } = expr {
                    out.push(command.clone());
                }
            });
        }
    });
    found
}

fn walk_stmts<'a>(stmts: &'a [Stmt], visit: &mut impl FnMut(&'a Stmt)) {
    for stmt in stmts {
        visit(stmt);
        if let Stmt::If {
            then, otherwise, ..
        } = stmt
        {
            walk_stmts(then, visit);
            walk_stmts(otherwise, visit);
        }
    }
}

fn collect<T>(
    exprs: &Exprs,
    id: ExprId,
    out: &mut Vec<T>,
    take: &impl Fn(ExprId, &Expr, &mut Vec<T>),
) {
    let Some(expr) = exprs.get(id) else {
        return;
    };
    take(id, expr, out);
    for child in children(expr) {
        collect(exprs, child, out, take);
    }
}

/// The expressions one statement owns, not counting the bodies of a nested `if`, whose
/// statements the caller walks separately.
fn roots(stmt: &Stmt) -> Vec<ExprId> {
    match stmt {
        Stmt::Assign { value, .. } => vec![*value],
        Stmt::If { cond, .. } => vec![*cond],
        Stmt::Emit { fields, .. } | Stmt::Put { fields, .. } => {
            fields.iter().map(|(_, id)| *id).collect()
        }
        Stmt::Patch { key, fields, .. } => {
            let mut ids = vec![*key];
            ids.extend(fields.iter().map(|(_, id)| *id));
            ids
        }
        Stmt::Delete { key, .. } => vec![*key],
        Stmt::Fail { message, .. } | Stmt::Log { message } => vec![*message],
        Stmt::Erase { value, .. } | Stmt::Discard(value) => vec![*value],
        Stmt::Return(Return::Ok) => Vec::new(),
        Stmt::Return(Return::Invalid(message)) => vec![*message],
        Stmt::Return(Return::Reject { code, message }) => vec![*code, *message],
    }
}

fn children(expr: &Expr) -> Vec<ExprId> {
    match expr {
        Expr::Lit(_) | Expr::Load(_) => Vec::new(),
        Expr::Unary { operand, .. } => vec![*operand],
        Expr::Binary { lhs, rhs, .. } => vec![*lhs, *rhs],
        Expr::Method { receiver, args, .. } => {
            let mut ids = vec![*receiver];
            ids.extend(args.iter().copied());
            ids
        }
        Expr::If {
            cond,
            then,
            otherwise,
        } => vec![*cond, *then, *otherwise],
        Expr::Field { receiver, .. } => vec![*receiver],
        Expr::Object(fields) => fields.iter().map(|(_, id)| *id).collect(),
        Expr::Interp(parts) => parts.clone(),
        Expr::Call { args, .. } => args.clone(),
        Expr::Invoke { args, .. } => args.iter().map(|(_, id)| *id).collect(),
        Expr::Reveal {
            value,
            subject_value,
            ..
        } => vec![*value, *subject_value],
    }
}

/// Rule 9. `Err((erase, reveal))` names the erase that may already have run and the
/// reveal that is still reachable from it. Reachability rather than lexical order, so
/// an erase on a path that ends in `fail` does not poison what follows the `if`.
fn erase_last(exprs: &Exprs, body: &[Stmt]) -> Result<(), (Span, Span)> {
    scan(exprs, body, None).map(|_| ())
}

struct Reach {
    erased: Option<Span>,
    falls_through: bool,
}

fn scan(exprs: &Exprs, stmts: &[Stmt], incoming: Option<Span>) -> Result<Reach, (Span, Span)> {
    let mut erased = incoming;
    for stmt in stmts {
        // Every reveal in one statement is checked against the same incoming state,
        // which is what keeping `erase` a statement buys: their order cannot matter.
        if let Some(at) = erased
            && let Some(reveal) = first_reveal(exprs, stmt)
        {
            return Err((at, reveal));
        }

        match stmt {
            Stmt::Erase { span, .. } => erased = erased.or(Some(*span)),
            Stmt::Fail { .. } | Stmt::Return(_) => {
                return Ok(Reach {
                    erased,
                    falls_through: false,
                });
            }
            Stmt::If {
                then, otherwise, ..
            } => {
                let taken = scan(exprs, then, erased)?;
                let skipped = scan(exprs, otherwise, erased)?;
                erased = match (taken.falls_through, skipped.falls_through) {
                    (true, true) => taken.erased.or(skipped.erased),
                    (true, false) => taken.erased,
                    (false, true) => skipped.erased,
                    // Neither branch falls through, so nothing after the `if` runs.
                    (false, false) => {
                        return Ok(Reach {
                            erased: None,
                            falls_through: false,
                        });
                    }
                };
            }
            _ => {}
        }
    }

    Ok(Reach {
        erased,
        falls_through: true,
    })
}

/// The earliest `reveal` among a statement's own expressions. The arena is built in
/// parse order, so the smallest id is the first one in the source.
fn first_reveal(exprs: &Exprs, stmt: &Stmt) -> Option<Span> {
    let mut found: Vec<ExprId> = Vec::new();
    for root in roots(stmt) {
        collect(exprs, root, &mut found, &|id, expr, out| {
            if matches!(expr, Expr::Reveal { .. }) {
                out.push(id);
            }
        });
    }
    found
        .into_iter()
        .min_by_key(|id| id.0)
        .map(|id| exprs.span(id))
}
