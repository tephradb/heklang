use std::collections::{BTreeSet, HashMap};
use std::mem;

use crate::build::Builder;
use crate::ir::{
    Absent, Action, Arm, BinOp, Bind, Builtin, Command, ConstDef, Effect, EntityDef, EntityField,
    EnumDef, EnvField, EventDef, EventPath, Expect, Expr, ExprId, Exprs, FieldDef, Filter,
    Function, Given, Handler, Ident, Index, Iter, Literal, Number, Param, Program, Projector,
    RecordDef, RecordField, ReplySpec, Return, Setup, Slot, Span, Stmt, Test, Type, UnOp, Update,
};
use crate::lex::{Keyword, Spanned, Sym, SyntaxError, Token, lex};
use crate::scaled::Rounding;
use crate::types::{self, default_type, fills, inner_of, method_sig, response_field, seal, wrap};
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

/// What pass D collects while it walks. A struct rather than five locals so the pass
/// body can be a method, which is what lets one declaration fail without ending the run.
#[derive(Default)]
struct Bodies {
    commands: Vec<Command>,
    effects: Vec<Effect>,
    functions: Vec<Function>,
    /// Where each `test` starts, for pass E to come back to.
    tests: Vec<usize>,
    /// How many projector shells pass D has filled in, since it walks them in order.
    seen: usize,
}

/// Every declaration that failed, rather than only the first. One error per declaration
/// and reporting stops at the end of the pass that found any, because a later pass reads
/// what an earlier one built: a signature that did not parse would make every body that
/// names it wrong in a way its author did not write. `docs/cli.md` has the rest.
pub fn check_files<'a>(
    files: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Result<Program, Vec<SyntaxError>> {
    let files: Vec<(Option<&str>, &str)> = files
        .into_iter()
        .map(|(name, source)| (Some(name), source))
        .collect();
    let mut parser = Parser::open(files).map_err(|err| vec![err])?;
    match parser.program() {
        Ok(program) => Ok(program),
        // A pass that recorded nothing and still failed is one of the whole-program
        // checks, or a `skip_item` that found no next declaration to step to.
        Err(first) if parser.errors.is_empty() => Err(vec![first]),
        Err(_) => Err(parser.errors),
    }
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
    /// Every declaration that failed in the pass now running. `docs/cli.md` has the
    /// granularity and why a pass is where reporting stops.
    errors: Vec<SyntaxError>,
    prologue: bool,
    /// Set while parsing a filter, a `state` seed or a fold arm. Narrower than
    /// `prologue`, which also covers a hoisted `let`: that runs once per request,
    /// before the fold, so it may read the pinned clock while a fold may not.
    folding: bool,
    command_end: usize,
    /// Which declaration kind is being parsed, so a statement in the wrong one can be
    /// rejected with a message about that kind rather than a generic one.
    kind: Kind,
    /// Command signatures, collected before any body so an `invoke` can be checked
    /// against a command declared later or in another module (rule 7).
    commands: Vec<Signature>,
    /// The same, for `fn`, so a helper may call one declared below it.
    functions: Vec<Signature>,
    /// The enclosing effect's own `fn` signatures; empty outside one. Two lists
    /// rather than one for the reason `enums` and `module_enums` are two: a name here
    /// resolves first, and is invisible once the effect closes.
    local_fns: Vec<Signature>,
    /// The effect whose braces the parser is inside, which is the scope stamped onto
    /// a call to one of `local_fns`.
    in_effect: Option<Ident>,
    /// The declared result of the `fn` being parsed, so `return` has a target type.
    returns: Option<Type>,
    /// Set only while parsing an `http.*` body argument, which is what makes an object
    /// literal structurally illegal anywhere else (rule 8).
    in_body: bool,
    /// Set while parsing a value written into an entity column, the one position that
    /// takes sealed content by propagating the seal onto itself rather than reading
    /// what is behind it. A `state` fold is the other, and uses `folding`.
    propagating: bool,
    /// The enclosing projector's declarations; empty outside one.
    enums: Vec<EnumDef>,
    entities: Vec<EntityDef>,
    /// Module scope, collected before anything that could name one. A projector's own
    /// enum shadows one of these, which is why they are two lists rather than one.
    module_enums: Vec<EnumDef>,
    records: Vec<RecordDef>,
    /// Every const, resolved. Filled on demand by `resolve_const` rather than in
    /// declaration order, because a const may name one declared later.
    consts: Vec<ConstDef>,
    /// Every const before its value has been read: pass C0's output.
    shells: Vec<ConstShell>,
    /// The consts whose values are being parsed right now, outermost first, so a
    /// const that names itself is caught with the chain that reached it.
    resolving: Vec<Ident>,
    /// Set while parsing an `if` or `for` header, where the `{` that follows opens a
    /// block. Without it `if plan { ... }` reads as a record literal.
    no_record_literal: bool,
    /// Narrowings a statement introduced for the rest of its block, and what each
    /// slot type was before, so `statements` can put them back where the block ends.
    narrowings: Vec<(Slot, Option<Type>)>,
    /// The handler being parsed: its event, and the name its `as` clause bound. For a
    /// multi-path arm `event` holds the shared fields and `triggers` holds what was
    /// listed, so a missing field can say whether it is absent or merely not shared.
    event: Option<EventDef>,
    triggers: Vec<EventPath>,
    envelope: Option<Ident>,
    /// Set only while parsing a `patch` value, which is what makes `.field`
    /// structurally illegal anywhere else.
    stored: Option<Stored>,
}

/// A const with its value still unread: enough to say what type it is and where to
/// find the tokens, which is all pass C0 can know before any value has been parsed.
#[derive(Debug, Clone)]
struct ConstShell {
    name: Ident,
    module: Option<Ident>,
    ty: Type,
    /// The first token of the value, so resolution can seek back to it.
    at: usize,
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
    Function,
    /// A `fn` declared inside an `effect`. It may call out and `invoke`, so it is not
    /// `Function`; it may not `reveal` or `erase`, so it is not `Effect` either.
    /// See `docs/functions.md`.
    EffectFn,
    /// A test's value position. Pure like a `fn`, but for a different reason: a test
    /// states inputs and expectations, so nothing in one may reach the world.
    Test,
}

#[derive(Debug, Clone)]
struct Signature {
    name: Ident,
    params: Vec<(Ident, Type)>,
    /// A `fn`'s declared result. `None` for a command, which returns an outcome
    /// rather than a value.
    ret: Option<Type>,
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
            errors: Vec::new(),
            prologue: false,
            folding: false,
            command_end: 0,
            kind: Kind::Command,
            commands: Vec::new(),
            functions: Vec::new(),
            local_fns: Vec::new(),
            in_effect: None,
            returns: None,
            in_body: false,
            propagating: false,
            enums: Vec::new(),
            entities: Vec::new(),
            module_enums: Vec::new(),
            records: Vec::new(),
            consts: Vec::new(),
            shells: Vec::new(),
            resolving: Vec::new(),
            no_record_literal: false,
            narrowings: Vec::new(),
            event: None,
            triggers: Vec::new(),
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

    /// A comma with nothing after it but the closing paren.
    fn at_trailing_comma(&self) -> bool {
        self.at_sym(Sym::Comma)
            && matches!(
                self.tokens.get(self.pos + 1).map(|spanned| &spanned.token),
                Some(Token::Sym(Sym::RParen))
            )
    }

    /// `erase(subject, value)` rather than `erase(value)`: a bare name, then a comma
    /// that is not the trailing one. The third token is load-bearing, because
    /// `erase(customer_id,)` is a legal one-argument call and a two-token lookahead
    /// would reparse it as a malformed two-argument one.
    fn at_named_subject(&self) -> bool {
        matches!(self.peek(), Token::Ident(_))
            && matches!(
                self.tokens.get(self.pos + 1).map(|spanned| &spanned.token),
                Some(Token::Sym(Sym::Comma))
            )
            && !matches!(
                self.tokens.get(self.pos + 2).map(|spanned| &spanned.token),
                Some(Token::Sym(Sym::RParen))
            )
    }

    /// Closes a call's argument list. The last argument may carry a comma, the way the
    /// last item of every other comma-separated list in the language already may: a
    /// call written across lines gets one from any formatter, and where a comma is
    /// legal the list should not care which item it follows. Not used for a call that
    /// takes no arguments, where there is no last item for the comma to belong to.
    fn end_args(&mut self) -> Result<(), SyntaxError> {
        if self.at_trailing_comma() {
            self.bump();
        }
        self.expect_sym(Sym::RParen)
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

    /// Six passes over the same token stream, each doing only what the one before it
    /// made possible. A record field may name an enum, a const value may name another
    /// const, an event field may name a record, and a body may name anything, so the
    /// boundaries are not arbitrary. See `docs/declarations.md`.
    fn program(&mut self) -> Result<Program, SyntaxError> {
        // A: enums, and the names of records, so a record field may name a record.
        self.pos = 0;
        let items = self.pos;
        self.sweep(items, |parser| parser.name_item())?;
        self.settled()?;

        // B: record fields, now that every type they might name has a name.
        let mut index = 0usize;
        self.sweep(items, |parser| parser.record_item(&mut index))?;
        self.settled()?;

        // C0: every const's name and type, and where its value starts. Values are not
        // read here, because one may name a const declared below it or in another file.
        self.sweep(items, |parser| parser.const_item())?;
        self.settled()?;
        for index in 0..self.shells.len() {
            let name = self.shells[index].name.clone();
            self.resolve_const(&name)?;
        }
        // Resolution order follows the reference graph, so put them back in the order
        // they were written: nothing depends on it, and a stable order is one less
        // thing for a reader of `Program` to wonder about.
        self.consts.sort_by_key(|def| {
            self.shells
                .iter()
                .position(|shell| shell.name == def.name)
                .unwrap_or(usize::MAX)
        });

        // C: everything whose declaration is a signature rather than a body.
        let mut events: Vec<EventDef> = Vec::new();
        let mut projectors: Vec<Projector> = Vec::new();
        self.sweep(items, |parser| {
            parser.signature_item(&mut events, &mut projectors)
        })?;
        self.settled()?;

        // D: bodies.
        let mut bodies = Bodies::default();
        self.sweep(items, |parser| {
            parser.body_item(&events, &mut projectors, &mut bodies)
        })?;
        self.settled()?;

        let mut program = Program {
            events,
            commands: bodies.commands,
            projectors,
            effects: bodies.effects,
            // Cloned rather than taken: pass E below resolves a test's values against the
            // same tables, so they have to stay in the parser until it has run.
            enums: self.module_enums.clone(),
            records: self.records.clone(),
            consts: self.consts.clone(),
            functions: bodies.functions,
            tests: Vec::new(),
        };
        self.check_recursion(&program)?;
        self.check_cycles(&program)?;
        self.check_zeros(&program)?;

        // E: every test, against the finished program. Order is irrelevant here for
        // the same reason it is everywhere else: nothing a test names is scoped.
        for at in bodies.tests {
            self.pos = at;
            let recovered = self.recovering(at, |parser| {
                let test = parser.test_decl(&program)?;
                if program.tests.iter().any(|other| other.name == test.name) {
                    return Err(parser.err(
                        format!("test {:?} is declared twice", test.name),
                        test.span.line,
                        test.span.col,
                    ));
                }
                Ok(test)
            })?;
            if let Some(test) = recovered {
                program.tests.push(test);
            }
        }
        self.settled()?;
        Ok(program)
    }

    /// One pass over every module's tokens, from `items` to the end, running `step`
    /// once per declaration and stepping over the ones that failed.
    fn sweep(
        &mut self,
        items: usize,
        mut step: impl FnMut(&mut Self) -> Result<(), SyntaxError>,
    ) -> Result<(), SyntaxError> {
        self.pos = items;
        while !matches!(self.peek(), Token::End) {
            let at = self.pos;
            self.recovering(at, &mut step)?;
        }
        Ok(())
    }

    /// Runs one declaration. A failure is recorded rather than returned, and the cursor
    /// goes back to the declaration's first token so `skip_item` can step over the whole
    /// thing: that is what makes a second error, in a second declaration, reachable at
    /// all. `skip_item` failing is different, because it means the braces do not balance
    /// and there is no next declaration to find.
    fn recovering<T>(
        &mut self,
        at: usize,
        step: impl FnOnce(&mut Self) -> Result<T, SyntaxError>,
    ) -> Result<Option<T>, SyntaxError> {
        match step(self) {
            Ok(value) => Ok(Some(value)),
            Err(err) => {
                self.errors.push(err);
                self.pos = at;
                self.skip_item()?;
                Ok(None)
            }
        }
    }

    /// The first error a pass recorded, which ends the run. A later pass reads what an
    /// earlier one built, so a signature that did not parse would make every body that
    /// names it wrong in a way its author did not write: reporting those would be
    /// reporting the checker's confusion rather than the program's.
    fn settled(&mut self) -> Result<(), SyntaxError> {
        match self.errors.first() {
            Some(err) => Err(err.clone()),
            None => Ok(()),
        }
    }

    /// Pass A: enums, and the names of records.
    fn name_item(&mut self) -> Result<(), SyntaxError> {
        match self.peek() {
            Token::Word(Keyword::Enum) => {
                let def = self.enum_decl()?;
                if self.module_enums.iter().any(|other| other.name == def.name) {
                    return self.fail(format!("enum `{}` is declared twice", def.name));
                }
                self.module_enums.push(def);
            }
            Token::Word(Keyword::Record) => {
                let module = self.module_at(self.pos).map(str::to_string);
                self.bump();
                let (line, col) = self.here();
                let name = self.expect_ident()?;
                if self.records.iter().any(|other| other.name == name) {
                    return Err(self.err(format!("record `{name}` is declared twice"), line, col));
                }
                self.records.push(RecordDef {
                    name,
                    module,
                    fields: Vec::new(),
                });
                self.skip_braced()?;
            }
            _ => self.skip_item()?,
        }
        Ok(())
    }

    /// Pass B: a record's fields. `index` walks the shells pass A pushed, so a record
    /// that failed there is not one this can be asked about.
    fn record_item(&mut self, index: &mut usize) -> Result<(), SyntaxError> {
        match self.peek() {
            Token::Word(Keyword::Record) => {
                let fields = self.record_fields()?;
                self.records[*index].fields = fields;
                *index += 1;
            }
            _ => self.skip_item()?,
        }
        Ok(())
    }

    /// Pass C0: a const's name and type, and where its value starts.
    fn const_item(&mut self) -> Result<(), SyntaxError> {
        match self.peek() {
            Token::Word(Keyword::Const) => {
                let shell = self.const_shell()?;
                if self.shell_of(&shell.name).is_some() {
                    return self.fail(format!("const `{}` is declared twice", shell.name));
                }
                self.shells.push(shell);
            }
            _ => self.skip_item()?,
        }
        Ok(())
    }

    /// Pass C: everything whose declaration is a signature rather than a body.
    fn signature_item(
        &mut self,
        events: &mut Vec<EventDef>,
        projectors: &mut Vec<Projector>,
    ) -> Result<(), SyntaxError> {
        match self.peek() {
            Token::Word(Keyword::Enum) | Token::Word(Keyword::Record) => self.skip_item()?,
            Token::Word(Keyword::Const) => self.skip_item()?,
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
                    return self.fail(format!("projector `{}` is declared twice", projector.name));
                }
                projectors.push(projector);
            }
            Token::Word(Keyword::Command) => self.command_signature()?,
            Token::Word(Keyword::Fn) => self.fn_signature()?,
            Token::Word(Keyword::Effect) | Token::Word(Keyword::Test) => self.skip_item()?,
            other => return self.fail(Self::expected_item(other)),
        }
        Ok(())
    }

    /// Pass D: bodies, and the position of every `test` for pass E to come back to.
    fn body_item(
        &mut self,
        events: &[EventDef],
        projectors: &mut [Projector],
        out: &mut Bodies,
    ) -> Result<(), SyntaxError> {
        match self.peek() {
            Token::Word(Keyword::Event)
            | Token::Word(Keyword::Enum)
            | Token::Word(Keyword::Record)
            | Token::Word(Keyword::Const) => self.skip_item()?,
            Token::Word(Keyword::Command) => {
                let command = self.command_decl(events)?;
                out.commands.push(command);
            }
            Token::Word(Keyword::Fn) => out.functions.push(self.fn_decl(events, Kind::Function)?),
            Token::Word(Keyword::Projector) => {
                let (handlers, entities) =
                    self.projector_handlers(&projectors[out.seen], events)?;
                projectors[out.seen].handlers = handlers;
                projectors[out.seen].entities = entities;
                out.seen += 1;
            }
            Token::Word(Keyword::Effect) => {
                let effect = self.effect_decl(events)?;
                if out.effects.iter().any(|other| other.name == effect.name) {
                    return self.fail(format!("effect `{}` is declared twice", effect.name));
                }
                out.effects.push(effect);
            }
            // E: tests, after the program is assembled, because a test names commands,
            // projectors and effects that pass D is still collecting.
            Token::Word(Keyword::Test) => {
                out.tests.push(self.pos);
                self.skip_item()?;
            }
            other => return self.fail(Self::expected_item(other)),
        }
        Ok(())
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

        let params = self.param_list(false)?;
        self.commands.push(Signature {
            name,
            params,
            ret: None,
        });
        self.skip_braced()
    }

    /// Pass C. Parameters and the result type only, so a call can be checked against a
    /// `fn` declared later or in another module.
    /// A `fn` parameter or return type. The one position that admits a `Response`,
    /// because reading one is pure and storing one is not: see `docs/functions.md`.
    /// It sits above `type_ref` rather than inside it, so `List(Response)` stays
    /// rejected along with every other position.
    fn fn_type(&mut self) -> Result<Type, SyntaxError> {
        if !matches!(self.peek(), Token::Ident(name) if name == "Response") {
            return self.type_ref();
        }
        self.bump();
        if self.eat_sym(Sym::Question) {
            return Ok(Type::opt(Type::Response));
        }
        Ok(Type::Response)
    }

    fn fn_signature(&mut self) -> Result<(), SyntaxError> {
        self.expect_word(Keyword::Fn)?;
        let (line, col) = self.here();
        let name = self.expect_ident()?;
        if self.functions.iter().any(|other| other.name == name) {
            return Err(self.err(format!("fn `{name}` is declared twice"), line, col));
        }
        let params = self.param_list(true)?;
        self.expect_sym(Sym::To)?;
        let ret = self.fn_type()?;
        self.functions.push(Signature {
            name,
            params,
            ret: Some(ret),
        });
        self.skip_braced()
    }

    /// Shared with `command`, whose parameters are application input and so may not be
    /// a `Response`. Only a `fn` passes `true`.
    fn param_list(&mut self, response_ok: bool) -> Result<Vec<(Ident, Type)>, SyntaxError> {
        let mut params = Vec::new();
        self.expect_sym(Sym::LParen)?;
        while !self.at_sym(Sym::RParen) {
            let param = self.expect_ident()?;
            self.expect_sym(Sym::Colon)?;
            let ty = if response_ok {
                self.fn_type()?
            } else {
                self.type_ref()?
            };
            params.push((param, ty));
            if !self.eat_sym(Sym::Comma) {
                break;
            }
        }
        self.expect_sym(Sym::RParen)?;
        Ok(params)
    }

    /// Pass D. A `fn` has a command's frame and arena and none of its prologue, since
    /// `state`, `guard` and a hoisted clock are all things a pure helper cannot have.
    fn fn_decl(&mut self, events: &[EventDef], kind: Kind) -> Result<Function, SyntaxError> {
        let module = self.module_at(self.pos).map(str::to_string);
        self.expect_word(Keyword::Fn)?;
        let name = self.expect_ident()?;
        let mut lower = Lower {
            b: Builder::new(&name),
            defaults: HashMap::new(),
        };
        lower.b.in_module(module.as_deref());

        for (param, ty) in self.param_list(true)? {
            lower.b.param(&param, ty);
        }
        let ret = self.fn_result(kind)?;
        self.expect_sym(Sym::LBrace)?;

        self.kind = kind;
        self.returns = ret.clone();
        let body = self.statements(&mut lower, events)?;
        self.returns = None;
        self.kind = Kind::Command;
        self.expect_sym(Sym::RBrace)?;

        // Falls-through, the analysis rule 9 already uses. A `for` body does not count,
        // because the container it walks can be empty. A `fn` returning nothing has
        // nothing to fall through to, so the check is about the declared type.
        if let Some(ret) = &ret
            && !always_returns(&body)
        {
            return self.fail(format!(
                "`{name}` can finish without returning a {ret}; every path out of a `fn` returns one"
            ));
        }
        Ok(lower.b.finish_fn(ret, body))
    }

    /// The `-> Type` after a parameter list. Optional for an effect-local `fn` and
    /// required everywhere else: a pure function that returns nothing does nothing, so
    /// the omission is worth rejecting where the body cannot have an effect.
    fn fn_result(&mut self, kind: Kind) -> Result<Option<Type>, SyntaxError> {
        if kind == Kind::EffectFn && !self.at_sym(Sym::To) {
            return Ok(None);
        }
        self.expect_sym(Sym::To)?;
        Ok(Some(self.fn_type()?))
    }

    /// A `fn` in scope here: the enclosing effect's own before module scope. The two
    /// can never collide, because a local one that shadows a module `fn` is rejected
    /// where it is declared, which is what makes this order the only rule needed.
    fn fn_sig(&self, name: &str) -> Option<&Signature> {
        self.local_fns
            .iter()
            .chain(&self.functions)
            .find(|sig| sig.name == name)
    }

    /// The scope a call to `name` resolves in, which is what the IR node carries.
    fn fn_scope(&self, name: &str) -> Option<Ident> {
        if self.local_fns.iter().any(|sig| sig.name == name) {
            return self.in_effect.clone();
        }
        None
    }

    /// A call, with each argument checked against its declared parameter so literal
    /// inference and enum resolution work through it the way they work through `emit`.
    fn call_fn(
        &mut self,
        lower: &mut Lower,
        name: Ident,
        span: Span,
    ) -> Result<ExprId, SyntaxError> {
        let args = self.call_args(lower, &name, span)?;
        lower.b.at(span);
        let scope = self.fn_scope(&name);
        Ok(lower.b.expr(Expr::CallFn {
            function: name,
            scope,
            args,
        }))
    }

    /// A call to a `fn` that returns nothing, which only an effect-local one can be.
    /// It is a statement and never an expression, so there is nothing to lower it into
    /// an arena slot for.
    fn void_call(&mut self, lower: &mut Lower) -> Result<Stmt, SyntaxError> {
        let span = self.span_here();
        let name = self.expect_ident()?;
        let scope = self.fn_scope(&name);
        let args = self.call_args(lower, &name, span)?;
        Ok(Stmt::Call {
            function: name,
            scope,
            args,
            span,
        })
    }

    /// Whether a bare `name(` here is a call to a `fn` that returns nothing.
    fn starts_void_call(&self, name: &str) -> bool {
        matches!(
            self.tokens.get(self.pos + 1).map(|next| &next.token),
            Some(Token::Sym(Sym::LParen))
        ) && self.fn_sig(name).is_some_and(|sig| sig.ret.is_none())
    }

    /// The argument list, shared by the two call forms. Each argument is checked
    /// against its declared parameter, so literal inference and enum resolution work
    /// through a call the way they work through `emit`.
    fn call_args(
        &mut self,
        lower: &mut Lower,
        name: &str,
        span: Span,
    ) -> Result<Vec<ExprId>, SyntaxError> {
        // Rule 3: a fold has to reproduce without a journal, and an effect-local `fn`
        // is the one helper that may call out. A module `fn` is pure by construction,
        // so a fold may still call one of those.
        if self.local_fns.iter().any(|sig| sig.name == name) {
            self.not_in_fold(&format!("call `{name}`, which may call out"), span)?;
        }
        let params = self.fn_sig(name).expect("checked by caller").params.clone();
        self.expect_sym(Sym::LParen)?;
        let outer = mem::replace(&mut self.no_record_literal, false);
        let mut args = Vec::new();
        while !self.at_sym(Sym::RParen) {
            let expected = params.get(args.len()).map(|(_, ty)| ty.clone());
            let (line, col) = self.here();
            if expected.is_none() {
                return Err(self.err(
                    format!("`{name}` takes {} arguments", params.len()),
                    line,
                    col,
                ));
            }
            args.push(self.expr(lower, expected)?);
            if !self.eat_sym(Sym::Comma) {
                break;
            }
        }
        self.expect_sym(Sym::RParen)?;
        self.no_record_literal = outer;
        if args.len() != params.len() {
            let (name_of, _) = &params[args.len()];
            return Err(self.err(format!("`{name}` needs `{name_of}`"), span.line, span.col));
        }
        Ok(args)
    }

    fn expected_item(found: &Token) -> String {
        format!(
            "expected `enum`, `record`, `const`, `fn`, `event`, `command`, `projector`, `effect` or `test`, found {found}"
        )
    }

    /// The annotations a record field takes, which is `@max` and nothing else so far.
    /// `@subject` gets its own message because it is the obvious next thing to try and
    /// the reason it is absent is not obvious; see `docs/declarations.md`.
    fn length_annotations(&mut self, ty: &Type, field: &str) -> Result<Option<usize>, SyntaxError> {
        let mut max_len = None;
        while let Token::Path(segments) = self.peek().clone() {
            let (line, col) = self.here();
            self.bump();
            let [annotation] = segments.as_slice() else {
                return self.fail("an annotation name cannot contain `.`");
            };
            match annotation.as_str() {
                "max" => max_len = Some(self.max_annotation(ty, field)?),
                "subject" => {
                    return Err(self.err(
                        format!(
                            "a record field cannot be `@subject`, so `{field}` cannot carry personal data; a subject-bound value is recovered from the schema path, and a record reached through a container has no path to recover it from"
                        ),
                        line,
                        col,
                    ));
                }
                other => {
                    return Err(self.err(format!("unknown annotation `@{other}`"), line, col));
                }
            }
        }
        Ok(max_len)
    }

    /// `@max(n)`, and the check that there is something to bound. A length on anything
    /// but a string used to parse and then quietly do nothing.
    fn max_annotation(&mut self, ty: &Type, field: &str) -> Result<usize, SyntaxError> {
        let (line, col) = self.here();
        if !matches!(inner_of(ty), Type::String) {
            return Err(self.err(
                format!("`@max` bounds a length, so it applies to a String; `{field}` is a {ty}"),
                line,
                col,
            ));
        }
        self.expect_sym(Sym::LParen)?;
        let number = self.expect_number()?;
        if number.scale != 0 {
            return self.fail("`@max` takes a whole number");
        }
        let Ok(max) = usize::try_from(number.digits) else {
            return self.fail("`@max` is too large");
        };
        self.expect_sym(Sym::RParen)?;
        Ok(max)
    }
    /// Pass B. The name was taken in pass A, so this reads only the body.
    fn record_fields(&mut self) -> Result<Vec<RecordField>, SyntaxError> {
        self.expect_word(Keyword::Record)?;
        let name = self.expect_ident()?;
        self.expect_sym(Sym::LBrace)?;

        let mut fields: Vec<RecordField> = Vec::new();
        while !self.at_sym(Sym::RBrace) {
            let (line, col) = self.here();
            let field = self.expect_ident()?;
            if fields.iter().any(|other| other.name == field) {
                return Err(self.err(
                    format!("record `{name}` declares `{field}` twice"),
                    line,
                    col,
                ));
            }
            self.expect_sym(Sym::Colon)?;
            let ty = self.type_ref()?;
            let max_len = self.length_annotations(&ty, &field)?;
            fields.push(RecordField {
                name: field,
                ty,
                max_len,
            });
            if !self.eat_sym(Sym::Comma) {
                break;
            }
        }
        self.expect_sym(Sym::RBrace)?;
        if fields.is_empty() {
            return self.fail(format!("record `{name}` declares no fields"));
        }
        Ok(fields)
    }

    /// `const NAME: Type =`, and where the value begins. The value is left unread,
    /// because it may name a const declared below this one or in another file, and
    /// pass C0 has not seen those yet.
    fn const_shell(&mut self) -> Result<ConstShell, SyntaxError> {
        let module = self.module_at(self.pos).map(str::to_string);
        self.expect_word(Keyword::Const)?;
        let name = self.expect_ident()?;
        self.expect_sym(Sym::Colon)?;
        let ty = self.type_ref()?;
        self.expect_sym(Sym::Assign)?;
        let at = self.pos;
        self.skip_value();
        Ok(ConstShell {
            name,
            module,
            ty,
            at,
        })
    }

    /// One const's value, parsed on demand and memoised, which is what lets a value
    /// name a const the reader has not reached yet. Literals and literal aggregates
    /// only, for the reason an entity default is: no expression arena hangs off a
    /// declaration.
    fn resolve_const(&mut self, name: &str) -> Result<Literal, SyntaxError> {
        if let Some(def) = self.const_def(name) {
            return Ok(def.value.clone());
        }
        let Some(shell) = self.shell_of(name).cloned() else {
            return self.fail(format!("no const `{name}`"));
        };
        if let Some(from) = self.resolving.iter().position(|held| held == name) {
            let mut chain: Vec<String> = self.resolving[from..]
                .iter()
                .map(|held| format!("`{held}`"))
                .collect();
            chain.push(format!("`{name}`"));
            return self.fail(format!(
                "{}: a `const` cannot name itself, directly or through another, so that every const has a value",
                chain.join(" names ")
            ));
        }

        let saved = self.pos;
        self.pos = shell.at;
        self.resolving.push(shell.name.clone());
        let value = self.default_literal("const", &shell.ty)?;
        // A literal ends the declaration, so the next token belongs to the next item.
        // Checked here rather than by the caller, because the caller has seeked away
        // and would never see a trailing `+ 1`.
        if !matches!(self.peek(), Token::End)
            && !matches!(self.peek(), Token::Word(word) if starts_item(*word))
        {
            return self.fail(Self::expected_item(self.peek()));
        }
        self.resolving.pop();
        self.pos = saved;

        self.consts.push(ConstDef {
            name: shell.name,
            module: shell.module,
            ty: shell.ty,
            value: value.clone(),
        });
        Ok(value)
    }

    fn record_def(&self, name: &str) -> Option<&RecordDef> {
        self.records.iter().find(|def| def.name == name)
    }

    fn const_def(&self, name: &str) -> Option<&ConstDef> {
        self.consts.iter().find(|def| def.name == name)
    }

    fn shell_of(&self, name: &str) -> Option<&ConstShell> {
        self.shells.iter().find(|shell| shell.name == name)
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
        // Every item but `const` has a braced body, and skipping a `const` by looking
        // for one runs past it into whatever declaration comes next.
        if self.at_word(Keyword::Const) {
            return self.skip_const();
        }
        self.bump();
        self.skip_braced()
    }

    /// A `const` ends where its literal does, and a literal has no closing token of
    /// its own.
    fn skip_const(&mut self) -> Result<(), SyntaxError> {
        self.expect_word(Keyword::Const)?;
        self.skip_value();
        Ok(())
    }

    /// Runs to the next thing that can start an item, tracking depth so a list or a
    /// record value does not end it early. Pass C0 skips a value it comes back for
    /// later; `skip_item` skips one it never reads.
    fn skip_value(&mut self) {
        let mut depth = 0i32;
        loop {
            match self.peek() {
                Token::End => return,
                Token::Sym(Sym::LBrace | Sym::LBracket | Sym::LParen) => depth += 1,
                Token::Sym(Sym::RBrace | Sym::RBracket | Sym::RParen) => depth -= 1,
                Token::Word(word) if depth == 0 && starts_item(*word) => return,
                _ => {}
            }
            self.bump();
        }
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
                        field.max_len = Some(self.max_annotation(&field.ty.clone(), &field_name)?);
                    }
                    other => return self.fail(format!("unknown annotation `@{other}`")),
                }
            }

            if self.eat_sym(Sym::Assign) {
                // Only `none` is refused, and only here: an optional column already
                // starts absent, so writing it is a second spelling of the zero. A
                // present default is the ordinary rule, the one every other declared
                // position follows.
                if self.at_word(Keyword::None) {
                    return self.fail(format!(
                        "`{field_name}` is optional, so it is already `none` by default"
                    ));
                }
                field.default = Some(self.default_literal("default", &ty)?);
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
    fn default_literal(&mut self, what: &str, ty: &Type) -> Result<Literal, SyntaxError> {
        let negated = self.eat_sym(Sym::Minus);
        let spanned = self.bump();
        let (line, col) = (spanned.line, spanned.col);
        let bad = |found: &str| format!("a {ty} {what} cannot be {found}");
        // Every shape below resolves against the inner type, so a bare literal in an
        // optional position reads exactly as it does anywhere else. The wrap happens
        // once, at the end.
        let target = inner_of(ty);

        let lit = match spanned.token {
            Token::Number(number) => {
                let digits = if negated {
                    -number.digits
                } else {
                    number.digits
                };
                Number::new(digits, number.scale)
                    .resolve(target)
                    .map_err(|err| self.err(err.to_string(), line, col))?
            }
            _ if negated => return Err(self.err(bad("a negated value"), line, col)),
            // The only way to write a `Uuid` down. There is no Uuid literal token,
            // because a bare hex-and-dashes word is not one, so the target type is
            // what decides that this string is one.
            Token::Text(text) if matches!(target, Type::Uuid) => {
                if uuid::Uuid::parse_str(&text).is_err() {
                    return Err(self.err(format!("`{text}` is not a Uuid"), line, col));
                }
                Literal::Uuid(text)
            }
            // A `Timestamp` is written the way a `Uuid` is, and for the same reason:
            // there is no token for one, so the target type is what makes this string
            // a moment. Without it a `Timestamp` column had no writable default, and
            // the advice `check_zeros` gives about one could not be followed.
            Token::Text(text) if matches!(target, Type::Timestamp) => {
                let Some(micros) = value::timestamp(&text) else {
                    return Err(self.err(not_a_timestamp(&text), line, col));
                };
                Literal::Timestamp(micros)
            }
            Token::Text(text) => Literal::Str(text),
            Token::Sym(Sym::LBracket) => {
                let Type::List(inner) = target else {
                    return Err(self.err(bad("a list"), line, col));
                };
                let mut items = Vec::new();
                while !self.at_sym(Sym::RBracket) {
                    items.push(self.default_literal(what, inner)?);
                    if !self.eat_sym(Sym::Comma) {
                        break;
                    }
                }
                self.expect_sym(Sym::RBracket)?;
                Literal::List {
                    inner: inner.as_ref().clone(),
                    items,
                }
            }
            Token::Ident(name) if name == "Json" && self.at_sym(Sym::Dot) => {
                if !matches!(target, Type::Json) {
                    return Err(self.err(bad("a Json value"), line, col));
                }
                self.expect_sym(Sym::Dot)?;
                let member = self.expect_ident()?;
                if member != "empty" {
                    return Err(self.err(bad(&format!("`Json.{member}`")), line, col));
                }
                Literal::EmptyJson
            }
            Token::Ident(name) if name == "Map" && self.at_sym(Sym::Dot) => {
                let Type::Map(key, value) = target else {
                    return Err(self.err(bad("a map"), line, col));
                };
                self.expect_sym(Sym::Dot)?;
                let member = self.expect_ident()?;
                if member != "empty" {
                    return Err(self.err(bad(&format!("`Map.{member}`")), line, col));
                }
                Literal::EmptyMap(key.as_ref().clone(), value.as_ref().clone())
            }
            Token::Ident(name) if self.record_def(&name).is_some() => {
                let def = self.record_def(&name).cloned().expect("checked just above");
                self.expect_sym(Sym::LBrace)?;
                let mut fields: Vec<(Ident, Literal)> = Vec::new();
                while !self.at_sym(Sym::RBrace) {
                    let (line, col) = self.here();
                    let field = self.expect_ident()?;
                    let Some(declared) = def.field(&field) else {
                        return Err(self.err(
                            format!("record `{name}` has no field `{field}`"),
                            line,
                            col,
                        ));
                    };
                    let declared = declared.ty.clone();
                    self.expect_sym(Sym::Colon)?;
                    fields.push((field, self.default_literal(what, &declared)?));
                    if !self.eat_sym(Sym::Comma) {
                        break;
                    }
                }
                self.expect_sym(Sym::RBrace)?;
                for declared in &def.fields {
                    if !fields.iter().any(|(given, _)| given == &declared.name) {
                        return self.fail(format!("record `{name}` needs `{}`", declared.name));
                    }
                }
                Literal::Record { ty: name, fields }
            }
            // Against `ty` rather than `target`: absence needs an optional to be
            // absent from, and the wrap at the end has nothing to wrap.
            Token::Word(Keyword::None) => {
                let Type::Opt(inner) = ty else {
                    return Err(self.err(bad("`none`"), line, col));
                };
                Literal::None(inner.as_ref().clone())
            }
            Token::Word(Keyword::True) => Literal::Bool(true),
            Token::Word(Keyword::False) => Literal::Bool(false),
            // Ahead of the enum-variant arm below, which is the precedence `primary`
            // already uses for a bare name: record, then const, then variant.
            Token::Ident(name) if self.shell_of(&name).is_some() => {
                let declared = self.shell_of(&name).expect("checked just above").ty.clone();
                // Checked from the shell rather than from the resolved value, so a
                // mismatch names the const, and reports even inside a reference cycle.
                if !fills(&declared, ty) {
                    return Err(self.err(
                        format!("a {ty} {what} cannot be `{name}`, which is a {declared}"),
                        line,
                        col,
                    ));
                }
                self.resolve_const(&name)?
            }
            Token::Ident(variant) => {
                let Type::Enum(enum_name) = target else {
                    return Err(self.err(bad(&format!("`{variant}`")), line, col));
                };
                let Some(def) = self.enum_def(enum_name) else {
                    return Err(self.err(bad(&format!("`{variant}`")), line, col));
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
            other => return Err(self.err(bad(&other.to_string()), line, col)),
        };

        let found = value::literal(&lit).ty();
        if !fills(&found, ty) {
            return Err(self.err(format!("a {ty} {what} cannot be a {found}"), line, col));
        }
        Ok(wrap(lit, &found, ty))
    }

    /// Every enum a name could resolve against here: the projector's own first, then
    /// the module's.
    fn visible_enums(&self) -> impl Iterator<Item = &EnumDef> {
        self.enums.iter().chain(&self.module_enums)
    }

    /// A projector's own enum shadows a module-scope one of the same name, which is
    /// the same precedence a local binding has over a builtin.
    fn enum_def(&self, name: &str) -> Option<&EnumDef> {
        self.visible_enums().find(|def| def.name == name)
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
        if def.field(&field).is_none() && self.triggers.len() > 1 {
            let listed: Vec<String> = self.triggers.iter().map(EventPath::to_string).collect();
            return Err(self.err(
                format!(
                    "`{field}` is not shared by {}, so an arm listing them cannot name it; a binding names only what every listed type has, with the same type and the same `@subject`",
                    listed.join(", ")
                ),
                line,
                col,
            ));
        }
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
                        let max = self.max_annotation(&field.ty.clone(), &name)?;
                        field = field.max_len(max);
                    }
                    "no_index" => field = field.no_index(),
                    other => return self.fail(format!("unknown annotation `@{other}`")),
                }
            }

            // Rule 12: the annotation is the authored form and the type is what
            // propagates from it. Sealed after the annotation loop, so `@max` still
            // measures the value rather than its wrapper.
            if let Some(subject) = field.subject.clone() {
                field.ty = seal(field.ty, subject);
            }
            fields.push(field);
            if !self.eat_sym(Sym::Comma) {
                break;
            }
        }

        self.expect_sym(Sym::RBrace)?;

        let def = EventDef::new(path, fields);
        self.check_subjects(&def)?;
        Ok(def)
    }

    /// Rule 12: a subject id is the name a key is filed under, so it has to be a value
    /// that is always there and that does not itself need a key. Checked where the
    /// annotation is written rather than where a `reveal` finds it.
    fn check_subjects(&self, def: &EventDef) -> Result<(), SyntaxError> {
        for field in &def.fields {
            let Some(subject) = &field.subject else {
                continue;
            };
            let name = &field.name;
            let Some(id) = def.field(subject) else {
                return self.fail(format!(
                    "`@subject({subject})` on `{name}` names no field of {}",
                    def.path
                ));
            };
            if id.subject.is_some() {
                return self.fail(format!(
                    "`@subject({subject})` on `{name}` names a subject-encrypted field; a subject id is the name a key is filed under, so it cannot need a key itself"
                ));
            }
            if matches!(id.ty, Type::Opt(_)) {
                return self.fail(format!(
                    "`@subject({subject})` on `{name}` names an optional field; a subject id is the name a key is filed under, so a missing one is not `no key`, it is no question at all"
                ));
            }
        }
        Ok(())
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
            "Json" => Type::Json,
            "List" => {
                self.expect_sym(Sym::LParen)?;
                let inner = self.type_ref()?;
                self.expect_sym(Sym::RParen)?;
                Type::list(inner)
            }
            "Map" => {
                self.expect_sym(Sym::LParen)?;
                let (line, col) = self.here();
                let key = self.type_ref()?;
                // The same set an entity key is restricted to, for the same reason:
                // a key that cannot order cannot give a defined iteration order.
                if !value::can_key(&key) {
                    return Err(self.err(
                        format!("a {key} cannot be a map key, for the reason it cannot be an entity key: it does not order"),
                        line,
                        col,
                    ));
                }
                self.expect_sym(Sym::Comma)?;
                let value = self.type_ref()?;
                self.expect_sym(Sym::RParen)?;
                Type::map(key, value)
            }
            other => match self.enum_def(other) {
                Some(def) => Type::Enum(def.name.clone()),
                None => match self.record_def(other) {
                    Some(def) => Type::Record(def.name.clone()),
                    None => return self.fail(format!("unknown type `{other}`")),
                },
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
        let _decl = self.span_here();
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

        // Rule 12: whether the variable is subject-bound is a property of every arm
        // agreeing, so it is settled across the declaration rather than at any one arm.
        // The seed is never subject-bound, which is allowed and is why `plain` records
        // only arms.
        let mut bound: Option<(Ident, EventPath)> = None;
        let mut plain: Option<(EventPath, Span)> = None;

        while self.at_word(Keyword::On) {
            let arm = self.span_here();
            self.bump();
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

            // Rule 12: whether this arm folds sealed content is a property of the
            // value's type, so a transformed arm drops the seal by construction rather
            // than by a rule of its own. The id no longer has to be folded alongside:
            // it rides on the value.
            let carried = self
                .type_of(lower, value)
                .and_then(|ty| ty.subject().cloned());
            lower.b.pop_scope();

            let updates = vec![Update {
                slot,
                value,
                ty: ty.clone(),
            }];

            match carried {
                Some(field) => {
                    if let Some((have, first)) = &bound
                        && have != &field
                    {
                        return Err(self.err(
                            format!(
                                "`{name}` folds under two subjects, `{have}` from {first} and `{field}` from {path}; one variable holds one subject, because `reveal` names the key by it"
                            ),
                            arm.line,
                            arm.col,
                        ));
                    }
                    if let Some((first, at)) = &plain {
                        return Err(self.err(
                            self.mixed_fold(&name, &field, first),
                            at.line,
                            at.col,
                        ));
                    }
                    bound.get_or_insert((field, path.clone()));
                }
                None => {
                    if let Some((have, _)) = &bound {
                        let message = self.mixed_fold(&name, have, &path);
                        return Err(self.err(message, arm.line, arm.col));
                    }
                    plain.get_or_insert((path.clone(), arm));
                }
            }

            lower.b.slice(path, filters, binds, updates);
        }

        // The declared type is what the author wrote; the seal propagates onto it, the
        // same way it propagates onto an entity column. That is what `reveal` reads.
        if let Some((field, _)) = bound {
            lower.b.seal_state(slot, field);
        }

        Ok(())
    }

    /// Rule 12: a seed may be plain, an arm may not. Said the same way whichever order
    /// the two arms are written in, because the defect is the pair rather than either.
    fn mixed_fold(&self, name: &str, subject: &str, offender: &EventPath) -> String {
        format!(
            "`{name}` folds a subject-bound value under `{subject}`, so the arm on {offender} cannot fold a plain one into it; plaintext and a value that needs a key would share one slot, with nothing static to say which is in it. A seed may be plain, an arm may not."
        )
    }

    fn hoisted_let(&mut self, lower: &mut Lower) -> Result<(), SyntaxError> {
        self.expect_word(Keyword::Let)?;
        let name = self.expect_ident()?;
        self.expect_sym(Sym::Assign)?;
        let value = self.expr(lower, None)?;
        let ty = self.type_of(lower, value);
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
        // A narrowing ends where its block does, so this is the one place that has to
        // put back what the statements in it proved.
        let depth = self.narrowings.len();
        let mut stmts = Vec::new();
        while !self.at_sym(Sym::RBrace) && !matches!(self.peek(), Token::End) {
            stmts.push(self.statement(lower, events)?);
        }
        self.unnarrow(lower, depth);
        Ok(stmts)
    }

    /// Narrows for one branch of an `if`, when the proof is about that branch.
    fn hold(
        &self,
        lower: &mut Lower,
        proof: Option<(Slot, bool)>,
        branch: bool,
    ) -> Option<(Slot, Option<Type>)> {
        let (slot, present) = proof?;
        if present != branch {
            return None;
        }
        lower.b.narrow(slot).map(|previous| (slot, previous))
    }

    fn release(&self, lower: &mut Lower, held: Option<(Slot, Option<Type>)>) {
        if let Some((slot, previous)) = held {
            lower.b.widen(slot, previous);
        }
    }

    fn unnarrow(&mut self, lower: &mut Lower, depth: usize) {
        while self.narrowings.len() > depth {
            let (slot, previous) = self.narrowings.pop().expect("the loop checked this");
            lower.b.widen(slot, previous);
        }
    }

    fn block(&mut self, lower: &mut Lower, events: &[EventDef]) -> Result<Vec<Stmt>, SyntaxError> {
        self.expect_sym(Sym::LBrace)?;
        let stmts = self.statements(lower, events)?;
        self.expect_sym(Sym::RBrace)?;
        Ok(stmts)
    }

    /// Rule 9: the seal propagates from the value into the entity column written from
    /// it, so a column is never authored `@subject`. Read off the value's **type**
    /// rather than the slot it loaded from, which is what makes it survive a `let` and
    /// what makes the conflict below checkable.
    fn propagate_subject(
        &mut self,
        lower: &Lower,
        entity: &str,
        field: &str,
        value: ExprId,
        line: u32,
        col: u32,
    ) -> Result<(), SyntaxError> {
        let Some(subject) = self
            .type_of(lower, value)
            .as_ref()
            .and_then(Type::subject)
            .cloned()
        else {
            return Ok(());
        };
        let Some(target) = self
            .entities
            .iter_mut()
            .find(|def| def.name == entity)
            .and_then(|def| def.fields.iter_mut().find(|def| def.name == field))
        else {
            return Ok(());
        };
        // Rule 9's second check, which was a no-op until the seal was a type: one
        // column holds one subject, because a key is filed under exactly one and a
        // column with two would have nothing static to say which it needs. The same
        // sentence rule 12 says about a `state` fold.
        if let Some(seen) = &target.subject
            && seen != &subject
        {
            let message = format!(
                "`{entity}.{field}` already holds content sealed under `{seen}`, so it cannot also hold content sealed under `{subject}`; one column holds one subject, because `erase` files a key under exactly one"
            );
            return Err(self.err(message, line, col));
        }
        if target.subject.is_none() {
            target.ty = seal(target.ty.clone(), subject.clone());
            target.subject = Some(subject);
        }
        Ok(())
    }
    fn entity_ref(&mut self) -> Result<(Ident, EntityDef), SyntaxError> {
        let (line, col) = self.here();
        let name = self.expect_ident()?;
        match self.entity_def(&name) {
            Some(def) => Ok((name, def.clone())),
            None => Err(self.err(format!("entity `{name}` is not declared"), line, col)),
        }
    }

    /// Whether the next token is a bare word. Every word a test body uses but `test`
    /// itself is soft: claimed only inside one, so a construct only tests use costs no
    /// name anywhere else.
    fn at_soft(&self, word: &str) -> bool {
        matches!(self.peek(), Token::Ident(name) if name == word)
    }

    fn eat_soft(&mut self, word: &str) -> bool {
        if self.at_soft(word) {
            self.bump();
            return true;
        }
        false
    }

    fn expect_text(&mut self) -> Result<String, SyntaxError> {
        match self.peek().clone() {
            Token::Text(text) => {
                self.bump();
                Ok(text)
            }
            other => self.fail(format!("expected a string, found {other}")),
        }
    }

    /// Rule 1: a log, one action, and what should come out. Parsed in a pass of its
    /// own after the program is assembled, because a test names commands, projectors
    /// and effects rather than declaring them.
    fn test_decl(&mut self, program: &Program) -> Result<Test, SyntaxError> {
        let module = self.module_at(self.pos).map(str::to_string);
        let span = self.span_here();
        self.expect_word(Keyword::Test)?;
        let name = self.expect_text()?;
        let outer = mem::replace(&mut self.kind, Kind::Test);

        let mut lower = Lower {
            b: Builder::new(name.clone()),
            defaults: HashMap::new(),
        };
        lower.b.in_module(module.as_deref());
        self.expect_sym(Sym::LBrace)?;

        let mut given = Vec::new();
        while self.at_soft("given") {
            given.push(self.given_decl(&mut lower, program)?);
        }

        let mut setup = Vec::new();
        while self.at_soft("respond") || self.at_soft("erased") {
            setup.push(if self.at_soft("respond") {
                self.respond_decl(&mut lower)?
            } else {
                self.erased_decl(&mut lower)?
            });
        }

        let action = self.action_decl(&mut lower, program)?;

        let mut expect = Vec::new();
        while !self.at_sym(Sym::RBrace) && !matches!(self.peek(), Token::End) {
            expect.push(self.expect_decl(&mut lower, program, &action)?);
        }
        self.expect_sym(Sym::RBrace)?;
        self.kind = outer;

        let (frame, exprs) = lower.b.finish_test();
        Ok(Test {
            name,
            module,
            frame,
            exprs,
            given,
            setup,
            action,
            expect,
            span,
        })
    }

    /// Rule 2: one appended event, every field written out, the same rule `emit` and
    /// `put` follow.
    fn given_decl(&mut self, lower: &mut Lower, program: &Program) -> Result<Given, SyntaxError> {
        let span = self.span_here();
        self.bump();
        let path = self.expect_path()?;
        let Some(def) = program.event(&path) else {
            return self.fail(format!("event {path} is not declared"));
        };
        self.expect_sym(Sym::LBrace)?;
        let fields = self.event_fields(lower, def, "given")?;
        Ok(Given {
            event: path,
            fields,
            span,
        })
    }

    /// The `{ field: value }` block shared by `given` and `expect @path`. Every field
    /// is required, because an event with a hole is not one the log could hold.
    fn event_fields(
        &mut self,
        lower: &mut Lower,
        def: &EventDef,
        what: &str,
    ) -> Result<Vec<(Ident, ExprId)>, SyntaxError> {
        let mut fields: Vec<(Ident, ExprId)> = Vec::new();
        while !self.at_sym(Sym::RBrace) {
            let (line, col) = self.here();
            let name = self.expect_ident()?;
            let Some(declared) = def.field(&name) else {
                return Err(self.err(format!("{} has no field `{name}`", def.path), line, col));
            };
            if fields.iter().any(|(seen, _)| seen == &name) {
                return Err(self.err(format!("`{name}` is given twice"), line, col));
            }
            self.expect_sym(Sym::Colon)?;
            let value = self.expr(lower, Some(declared.ty.clone()))?;
            fields.push((name, value));
            if !self.eat_sym(Sym::Comma) {
                break;
            }
        }
        self.expect_sym(Sym::RBrace)?;
        for declared in &def.fields {
            if !fields.iter().any(|(name, _)| name == &declared.name) {
                return self.fail(format!(
                    "`{what} {}` needs `{}`; an event is written whole",
                    def.path, declared.name
                ));
            }
        }
        Ok(fields)
    }

    /// Rule 3: a queued reply for one URL. A queue rather than one value, so scripting
    /// a 503 then a 200 is how a test says the first attempt was absorbed.
    fn respond_decl(&mut self, lower: &mut Lower) -> Result<Setup, SyntaxError> {
        let span = self.span_here();
        self.bump();
        let url = self.expr(lower, Some(Type::String))?;
        if self.eat_soft("timeout") {
            return Ok(Setup::Respond {
                url,
                reply: ReplySpec::Timeout,
                span,
            });
        }
        let (line, col) = self.here();
        let number = self.expect_number()?;
        let status = u16::try_from(number.digits)
            .ok()
            .filter(|status| number.scale == 0 && (100..=599).contains(status));
        let Some(status) = status else {
            return Err(self.err("a status is a whole number from 100 to 599", line, col));
        };
        let reply = if self.at_sym(Sym::LBrace) {
            ReplySpec::Body(status, self.json_body(lower)?)
        } else {
            ReplySpec::Status(status)
        };
        Ok(Setup::Respond { url, reply, span })
    }

    /// Rule 3: the only way to write a shredded-key test, since a test cannot call
    /// `erase` itself.
    fn erased_decl(&mut self, lower: &mut Lower) -> Result<Setup, SyntaxError> {
        let span = self.span_here();
        self.bump();
        let subject = self.expect_ident()?;
        let id = self.expr(lower, Some(Type::String))?;
        Ok(Setup::Erased { subject, id, span })
    }

    /// A `{ ... }` in body position, which is where an object literal is claimed.
    fn json_body(&mut self, lower: &mut Lower) -> Result<ExprId, SyntaxError> {
        let outer = self.in_body;
        self.in_body = true;
        let value = self.expr(lower, Some(Type::Json));
        self.in_body = outer;
        value
    }

    /// Rule 4: exactly one, and it decides which expectations are legal.
    fn action_decl(&mut self, lower: &mut Lower, program: &Program) -> Result<Action, SyntaxError> {
        let span = self.span_here();
        if self.eat_soft("run") {
            let (line, col) = self.here();
            let command = self.expect_ident()?;
            let Some(def) = program.command(&command) else {
                return Err(self.err(format!("command `{command}` is not declared"), line, col));
            };
            let params = def.params.clone();
            let args = self.named_args(lower, &params, &command)?;
            return Ok(Action::Run {
                command,
                args,
                span,
            });
        }
        if self.eat_soft("project") {
            let (line, col) = self.here();
            let projector = self.expect_ident()?;
            if program.projector(&projector).is_none() {
                return Err(self.err(
                    format!("projector `{projector}` is not declared"),
                    line,
                    col,
                ));
            }
            return Ok(Action::Project { projector, span });
        }
        if self.eat_soft("deliver") {
            let (line, col) = self.here();
            let effect = self.expect_ident()?;
            if program.effect(&effect).is_none() {
                return Err(self.err(format!("effect `{effect}` is not declared"), line, col));
            }
            return Ok(Action::Deliver { effect, span });
        }
        if self.at_word(Keyword::Emit) {
            return self
                .fail("a test writes its log with `given`, which appends the event directly");
        }
        self.fail(format!(
            "a test does one thing: `run`, `project` or `deliver`, found {}",
            self.peek()
        ))
    }

    /// `{ name: value, ... }` against a command's declared parameters, the same braces
    /// `invoke` uses: named fields are always a brace block in heklang. Every parameter
    /// is required but an optional one, which is the rule `bind_params` binds by.
    fn named_args(
        &mut self,
        lower: &mut Lower,
        params: &[Param],
        what: &str,
    ) -> Result<Vec<(Ident, ExprId)>, SyntaxError> {
        self.expect_sym(Sym::LBrace)?;
        let mut args: Vec<(Ident, ExprId)> = Vec::new();
        while !self.at_sym(Sym::RBrace) {
            let (line, col) = self.here();
            let name = self.expect_ident()?;
            let Some(param) = params.iter().find(|param| param.name == name) else {
                return Err(self.err(format!("`{what}` has no parameter `{name}`"), line, col));
            };
            if args.iter().any(|(seen, _)| seen == &name) {
                return Err(self.err(format!("`{name}` is given twice"), line, col));
            }
            self.expect_sym(Sym::Colon)?;
            let value = self.expr(lower, Some(param.ty.clone()))?;
            args.push((name, value));
            if !self.eat_sym(Sym::Comma) {
                break;
            }
        }
        self.expect_sym(Sym::RBrace)?;
        for param in params {
            if !matches!(param.ty, Type::Opt(_))
                && !args.iter().any(|(name, _)| name == &param.name)
            {
                return self.fail(format!("`{what}` needs `{}`", param.name));
            }
        }
        Ok(args)
    }

    /// Rules 5 to 7. Which expectations are legal is decided by the action, so the
    /// error for a projector assertion after `run` names both rather than saying that
    /// `Order` is not a statement.
    fn expect_decl(
        &mut self,
        lower: &mut Lower,
        program: &Program,
        action: &Action,
    ) -> Result<Expect, SyntaxError> {
        let span = self.span_here();
        if !self.eat_soft("expect") {
            if self.at_soft("run") || self.at_soft("project") || self.at_soft("deliver") {
                return self
                    .fail("a test does one thing; a second action is a second test, or a `given`");
            }
            return self.fail(format!(
                "a test body is `given`, then setup, then one action, then `expect`, found {}",
                self.peek()
            ));
        }

        if self.eat_soft("nothing") {
            return match action {
                Action::Project { .. } => self.wrong_expectation(action, "`expect nothing`"),
                _ => Ok(Expect::Nothing { span }),
            };
        }

        match action {
            Action::Run { .. } => self.run_expectation(lower, program, span),
            Action::Project { projector, .. } => {
                let def = program
                    .projector(projector)
                    .expect("resolved by the action");
                self.row_expectation(lower, def, span)
            }
            Action::Deliver { .. } => self.effect_expectation(lower, program, span),
        }
    }

    fn wrong_expectation<T>(&self, action: &Action, what: &str) -> Result<T, SyntaxError> {
        let (word, name) = match action {
            Action::Run { command, .. } => ("run", command.as_str()),
            Action::Project { projector, .. } => ("project", projector.as_str()),
            Action::Deliver { effect, .. } => ("deliver", effect.as_str()),
        };
        self.fail(format!(
            "{what} is not something `{word} {name}` produces; the action decides which expectations a test can write"
        ))
    }

    /// Rule 5: the appended events in order, or the outcome that stopped them.
    fn run_expectation(
        &mut self,
        lower: &mut Lower,
        program: &Program,
        span: Span,
    ) -> Result<Expect, SyntaxError> {
        if self.at_word(Keyword::Invalid) {
            self.bump();
            self.expect_sym(Sym::LParen)?;
            let message = self.expr(lower, Some(Type::String))?;
            self.end_args()?;
            return Ok(Expect::Invalid { message, span });
        }
        if self.at_word(Keyword::Reject) {
            self.bump();
            self.expect_sym(Sym::LParen)?;
            let code = self.expr(lower, Some(Type::String))?;
            self.expect_sym(Sym::Comma)?;
            let message = self.expr(lower, Some(Type::String))?;
            self.end_args()?;
            return Ok(Expect::Reject {
                code,
                message,
                span,
            });
        }
        if matches!(self.peek(), Token::Path(_)) {
            let path = self.expect_path()?;
            let Some(def) = program.event(&path) else {
                return self.fail(format!("event {path} is not declared"));
            };
            self.expect_sym(Sym::LBrace)?;
            let fields = self.event_fields(lower, def, "expect")?;
            return Ok(Expect::Event { path, fields, span });
        }
        self.fail(format!(
            "a `run` produces events, `invalid` or `reject`, found {}",
            self.peek()
        ))
    }

    /// Rule 6: a row and its listed columns, or the absence of one. The projector's own
    /// enums come into scope for the duration, so a column's variant resolves here the
    /// way it does in the handler that wrote it.
    fn row_expectation(
        &mut self,
        lower: &mut Lower,
        projector: &Projector,
        span: Span,
    ) -> Result<Expect, SyntaxError> {
        self.enums = projector.enums.clone();
        let expect = self.row_columns(lower, projector, span);
        self.enums.clear();
        expect
    }

    fn row_columns(
        &mut self,
        lower: &mut Lower,
        projector: &Projector,
        span: Span,
    ) -> Result<Expect, SyntaxError> {
        let absent = self.eat_soft("no");
        let (line, col) = self.here();
        let entity = self.expect_ident()?;
        let Some(def) = projector.entity(&entity).cloned() else {
            return Err(self.err(
                format!("projector `{}` has no entity `{entity}`", projector.name),
                line,
                col,
            ));
        };
        self.expect_sym(Sym::LBracket)?;
        let key = self.expr(lower, Some(def.key_field().ty.clone()))?;
        self.expect_sym(Sym::RBracket)?;

        if absent {
            return Ok(Expect::NoRow { entity, key, span });
        }
        self.expect_sym(Sym::LBrace)?;
        let mut fields: Vec<(Ident, ExprId)> = Vec::new();
        while !self.at_sym(Sym::RBrace) {
            let (line, col) = self.here();
            let name = self.expect_ident()?;
            let Some(declared) = def.field(&name) else {
                return Err(self.err(
                    format!("entity `{entity}` has no field `{name}`"),
                    line,
                    col,
                ));
            };
            if fields.iter().any(|(seen, _)| seen == &name) {
                return Err(self.err(format!("`{name}` is given twice"), line, col));
            }
            self.expect_sym(Sym::Colon)?;
            let value = self.expr(lower, Some(declared.ty.clone()))?;
            fields.push((name, value));
            if !self.eat_sym(Sym::Comma) {
                break;
            }
        }
        self.expect_sym(Sym::RBrace)?;
        Ok(Expect::Row {
            entity,
            key,
            fields,
            span,
        })
    }

    /// Rule 7: one entry of the effect's trace.
    fn effect_expectation(
        &mut self,
        lower: &mut Lower,
        program: &Program,
        span: Span,
    ) -> Result<Expect, SyntaxError> {
        if self.at_soft("http") {
            self.bump();
            self.expect_sym(Sym::Dot)?;
            let (line, col) = self.here();
            let name = self.expect_verb()?;
            let Some(verb) = Builtin::verb(&name) else {
                return Err(self.err(
                    format!("`http` has no verb `{name}`; it has get, post, put, patch and delete"),
                    line,
                    col,
                ));
            };
            self.expect_sym(Sym::LParen)?;
            let url = self.expr(lower, Some(Type::String))?;
            let body = if !self.at_trailing_comma() && self.eat_sym(Sym::Comma) {
                Some(self.json_body(lower)?)
            } else {
                None
            };
            self.end_args()?;
            return Ok(Expect::Http {
                verb,
                url,
                body,
                span,
            });
        }
        if self.at_word(Keyword::Invoke) {
            self.bump();
            let (line, col) = self.here();
            let command = self.expect_ident()?;
            let Some(def) = program.command(&command) else {
                return Err(self.err(format!("command `{command}` is not declared"), line, col));
            };
            let params = def.params.clone();
            let args = self.named_args(lower, &params, &command)?;
            return Ok(Expect::Invoke {
                command,
                args,
                span,
            });
        }
        if self.eat_soft("erase") {
            self.expect_sym(Sym::LParen)?;
            let subject = self.expect_ident()?;
            self.expect_sym(Sym::Comma)?;
            let id = self.expr(lower, Some(Type::String))?;
            self.end_args()?;
            return Ok(Expect::Erase { subject, id, span });
        }
        if self.eat_soft("log") {
            self.expect_sym(Sym::LParen)?;
            let message = self.expr(lower, Some(Type::String))?;
            self.end_args()?;
            return Ok(Expect::Log { message, span });
        }
        if self.eat_soft("fail") {
            self.expect_sym(Sym::LParen)?;
            let message = self.expr(lower, Some(Type::String))?;
            self.end_args()?;
            return Ok(Expect::Failed { message, span });
        }
        if self.eat_soft("skipped") {
            return Ok(Expect::Skipped { span });
        }
        self.fail(format!(
            "a `deliver` produces `http.*`, `invoke`, `erase`, `log`, `fail` or `skipped`, found {}",
            self.peek()
        ))
    }

    /// An HTTP verb after the dot. Three of the five are keywords, so this reads a word
    /// as well as a name rather than making `http.put` unwritable.
    fn expect_verb(&mut self) -> Result<String, SyntaxError> {
        match self.peek().clone() {
            Token::Ident(name) => {
                self.bump();
                Ok(name)
            }
            Token::Word(word) => {
                self.bump();
                Ok(word.text().to_string())
            }
            other => self.fail(format!("expected a verb, found {other}")),
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
            let outer = mem::replace(&mut self.propagating, true);
            let value = if self.eat_sym(Sym::Colon) {
                self.expr(lower, Some(expected))?
            } else {
                if lower.b.lookup(&name).is_none() {
                    return Err(self.not_in_scope(&name, line, col));
                }
                lower.b.at(Span::new(line, col));
                lower.b.load(&name)
            };
            self.propagating = outer;
            self.propagate_subject(lower, &def.name, &name, value, line, col)?;
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
                let cond = self.header_expr(lower, Some(Type::Bool))?;
                // What this condition proves, and on which side. See
                // `docs/optionals.md`; the condition is lowered before either block is
                // parsed, which is what lets a single-pass parser narrow at all.
                let proof = narrowing(lower, cond);

                let held = self.hold(lower, proof, true);
                let then = self.block(lower, events)?;
                self.release(lower, held);

                let held = self.hold(lower, proof, false);
                let otherwise = if self.eat_word(Keyword::Else) {
                    // `else if` is one statement rather than a block, so a chain of
                    // conditions reads as a chain instead of nesting one level per arm.
                    if self.at_word(Keyword::If) {
                        // What a chain proves as a whole depends on every arm above,
                        // so a narrowing proved inside one does not escape it.
                        let depth = self.narrowings.len();
                        let stmt = self.statement(lower, events)?;
                        self.unnarrow(lower, depth);
                        vec![stmt]
                    } else {
                        self.block(lower, events)?
                    }
                } else {
                    Vec::new()
                };
                self.release(lower, held);

                // The early-return shape: reaching past this `if` means the condition
                // was false, because the branch it guards never falls through.
                if let Some((slot, false)) = proof
                    && always_returns(&then)
                    && let Some(previous) = lower.b.narrow(slot)
                {
                    self.narrowings.push((slot, previous));
                }

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
                        Kind::Effect | Kind::EffectFn => format!(
                            "`{outcome}` is a command's outcome; an effect's terminal outcome is `fail(...)`"
                        ),
                        Kind::Function => format!(
                            "`{outcome}` is a command's outcome; a `fn` returns a value, so a caller decides what a bad one means"
                        ),
                        _ => format!(
                            "`{outcome}` is a command's outcome; a projector write cannot fail in a way the program observes"
                        ),
                    });
                }
                if let Some(want) = self.returns.clone() {
                    let (line, col) = self.here();
                    if self.ends_return() {
                        return Err(self.err(
                            format!("this `fn` returns {want}, so `return` needs a value"),
                            line,
                            col,
                        ));
                    }
                    let value = self.expr(lower, Some(want))?;
                    return Ok(Stmt::Return(Return::Value(value)));
                }
                // The symmetric half of the check above, for the one signature that
                // declares no result. Without it a `return 5` here would silently be a
                // bare `return` and the `5` would fail as the next statement.
                if self.kind == Kind::EffectFn {
                    let (line, col) = self.here();
                    if !self.ends_return() {
                        return Err(self.err(
                            "this `fn` returns nothing, so `return` takes no value".to_string(),
                            line,
                            col,
                        ));
                    }
                    return Ok(Stmt::Return(Return::Ok));
                }
                let ret = if self.eat_word(Keyword::Invalid) {
                    self.expect_sym(Sym::LParen)?;
                    let message = self.expr(lower, Some(Type::String))?;
                    self.end_args()?;
                    Return::Invalid(message)
                } else if self.eat_word(Keyword::Reject) {
                    self.expect_sym(Sym::LParen)?;
                    let code = self.expr(lower, Some(Type::String))?;
                    self.expect_sym(Sym::Comma)?;
                    let message = self.expr(lower, Some(Type::String))?;
                    self.end_args()?;
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
                    Kind::Effect | Kind::EffectFn => {
                        return self.fail(
                            "an effect never appends events; call a command with `invoke`, which appends under its own guard",
                        );
                    }
                    Kind::Function => {
                        return Err(self.purity_error("append events", self.span_here()));
                    }
                    Kind::Test => {
                        return self.fail(
                            "a test writes its log with `given`, which appends the event directly",
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
                self.not_in_fn("write a read model", self.span_here())?;
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
            // Rule 5: one parse for both, because the two statements differ only in
            // what they do with a row that is not there.
            Token::Word(Keyword::Patch) | Token::Word(Keyword::Update) => {
                let word = match self.peek() {
                    Token::Word(Keyword::Update) => Keyword::Update,
                    _ => Keyword::Patch,
                };
                let absent = match word {
                    Keyword::Update => Absent::Skip,
                    _ => Absent::Materialize,
                };
                let text = word.text();
                self.not_in_fn("write a read model", self.span_here())?;
                if self.kind != Kind::Projector {
                    return self.fail(format!(
                        "`{text}` writes an entity, so it can only appear in a projector"
                    ));
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
                    absent,
                    loads: stored.loads,
                    fields,
                    span,
                })
            }
            Token::Word(Keyword::Delete) => {
                self.not_in_fn("write a read model", self.span_here())?;
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
                let ty = self.type_of(lower, value);
                let slot = lower.b.alloc(&name, ty);
                Ok(Stmt::Assign { slot, value })
            }
            Token::Word(Keyword::Invoke) => {
                let span = self.span_here();
                self.bump();
                let value = self.invoke_expr(lower, span)?;
                Ok(Stmt::Discard(value))
            }
            Token::Word(Keyword::For) => {
                self.bump();
                lower.b.push_scope();
                let iter = self.iter_bindings(lower)?;
                let body = self.block(lower, events)?;
                lower.b.pop_scope();
                Ok(Stmt::For { iter, body })
            }
            Token::Word(Keyword::State) | Token::Word(Keyword::Guard) => {
                if self.kind == Kind::Function {
                    return self
                        .fail("a `fn` has no `state`; it is a pure function of its arguments");
                }
                if self.kind == Kind::EffectFn {
                    return self.fail(
                        "an effect-local `fn` has no `state`; the fold belongs to the arm, so pass what it decided in as a parameter",
                    );
                }
                self.fail("`state` and `guard` must come before the first statement")
            }
            Token::Ident(name) if self.starts_effect_statement(name) => {
                self.effect_statement(lower, events)
            }
            Token::Ident(name) if self.starts_void_call(name) => self.void_call(lower),
            other => self.fail(format!("expected a statement, found {other}")),
        }
    }

    fn expr(&mut self, lower: &mut Lower, expect: Option<Type>) -> Result<ExprId, SyntaxError> {
        let (line, col) = self.here();
        let value = self.or_expr(lower, expect.clone())?;
        // Rule 12, and the type, at the one place every declared position funnels
        // through. The seal check runs first because its message is the specific one:
        // sealed content in a plain position is a `reveal` that is missing, not a type
        // that is wrong.
        if let Some(want) = &expect {
            self.check_seal(lower, value, want, line, col)?;
            self.check_type(lower, value, want, line, col)?;
        }
        Ok(value)
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
        // A `Bool` target describes the comparison rather than its operands, and this
        // cannot know yet whether one follows. Nothing resolves against `Bool`, so
        // dropping it costs no inference and stops `if 5 > count` from reporting "a
        // number cannot be a Bool" about a literal the other operand would have typed.
        let lhs = self.add_expr(lower, expect.filter(|ty| ty != &Type::Bool))?;
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
        let (line, col) = self.here();
        let rhs = self.add_expr(lower, hint)?;
        // Rule 12: an equality on sealed content leaks whether two ciphertexts hold the
        // same thing, and under a real cipher it would not even answer. `reveal` both
        // sides, or compare something plaintext.
        self.no_seal(lower, lhs, "be compared", span.line, span.col)?;
        self.no_seal(lower, rhs, "be compared", line, col)?;
        self.settle(lower, lhs, rhs);
        self.check_compare(lower, op, lhs, rhs, span)?;
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
            self.check_arith(lower, op, lhs, rhs, span)?;
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
            self.check_arith(lower, op, lhs, rhs, span)?;
            lower.b.at(span);
            lhs = lower.b.binary(op, lhs, rhs);
        }
        Ok(lhs)
    }

    /// An operator applied to a pair the table has no row for. Runs after `settle`, so
    /// a defaulted literal has already taken the type its neighbour gave it, and only
    /// where both sides are known, so an unknown type is never an accusation.
    fn check_arith(
        &self,
        lower: &Lower,
        op: BinOp,
        lhs: ExprId,
        rhs: ExprId,
        span: Span,
    ) -> Result<(), SyntaxError> {
        // Rule 12: arithmetic reads its operands, and a sum of sealed content is
        // plaintext derived from it. The seal message is the useful one here, so it
        // gets to run first.
        self.no_seal(lower, lhs, "be used in arithmetic", span.line, span.col)?;
        self.no_seal(lower, rhs, "be used in arithmetic", span.line, span.col)?;
        let (Some(left), Some(right)) = (self.type_of(lower, lhs), self.type_of(lower, rhs)) else {
            return Ok(());
        };
        if types::arithmetic(op, &left, &right).is_some() {
            return Ok(());
        }
        Err(self.err(bad_operands(op, &left, &right), span.line, span.col))
    }

    /// The same rule for a comparison, which the runtime holds to the same table: two
    /// scales do not meet under `>` any more than under `+`.
    fn check_compare(
        &self,
        lower: &Lower,
        op: BinOp,
        lhs: ExprId,
        rhs: ExprId,
        span: Span,
    ) -> Result<(), SyntaxError> {
        let (Some(left), Some(right)) = (self.type_of(lower, lhs), self.type_of(lower, rhs)) else {
            return Ok(());
        };
        if types::comparable(op, &left, &right) {
            return Ok(());
        }
        Err(self.err(bad_operands(op, &left, &right), span.line, span.col))
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
                let receiver = self.type_of(lower, value);
                // Rule 12: a method reads content, and content behind the seal needs
                // `reveal` first. The presence checks are the exception, because
                // asking whether an optional holds anything does not read what it
                // holds. See `docs/effects.md`.
                if let Some(subject) = receiver.as_ref().and_then(Type::subject)
                    && !matches!(name.as_str(), "is_some" | "is_none")
                {
                    let why = if name == "unwrap_or" {
                        format!(
                            "`unwrap_or` would put a plaintext default and content sealed under `{subject}` in one slot, with nothing static to say which is in it"
                        )
                    } else {
                        format!("`{name}` reads content sealed under `{subject}`")
                    };
                    return Err(self.err(
                        format!(
                            "{why}; `reveal` it first. `.is_some()` and `.is_none()` are the exception, because presence is not content"
                        ),
                        line,
                        col,
                    ));
                }
                // Narrowing changes what the optional methods mean, so reading one off
                // a narrowed value is a mistake worth catching here rather than at run
                // time, where it would arrive as "Int has no method `unwrap_or`".
                if matches!(name.as_str(), "is_some" | "is_none" | "unwrap_or")
                    && matches!(lower.b.exprs().get(value), Some(Expr::Unwrap(_)))
                    && let Some(ty) = &receiver
                {
                    return Err(self.err(
                        format!(
                            "`{name}` reads an optional, and the branch above already proved this one present, so it is a {ty} here; use it directly"
                        ),
                        line,
                        col,
                    ));
                }
                // The same table that types the result knows whether there is one to
                // type. A receiver whose type is unknown is not accused, as everywhere
                // else, so this fires exactly where an answer exists.
                let sig = match &receiver {
                    Some(ty) => match method_sig(ty, &name) {
                        Some(sig) => Some(sig),
                        None => {
                            return Err(self.err(no_method(ty, &name), line, col));
                        }
                    },
                    None => None,
                };
                let outer = mem::replace(&mut self.no_record_literal, false);
                let mut args = Vec::new();
                while !self.at_sym(Sym::RParen) {
                    let hint = sig
                        .as_ref()
                        .and_then(|sig| sig.params.get(args.len()).cloned().flatten());
                    args.push(self.expr(lower, hint)?);
                    if !self.eat_sym(Sym::Comma) {
                        break;
                    }
                }
                self.expect_sym(Sym::RParen)?;
                self.no_record_literal = outer;
                // The argument types checked themselves on the way in, through the
                // hint; the count is what is left, and it is the table's again.
                if let Some(sig) = &sig
                    && sig.params.len() != args.len()
                {
                    let plural = if sig.params.len() == 1 { "" } else { "s" };
                    return Err(self.err(
                        format!(
                            "`{name}` takes {} argument{plural}, and this gives {}",
                            sig.params.len(),
                            args.len()
                        ),
                        line,
                        col,
                    ));
                }
                lower.b.at(span);
                value = lower.b.method(value, &name, args);
                continue;
            }

            // Parenless, so a field rather than a method. Only a `Response` has any,
            // and where the receiver's type is known the mistake is caught here rather
            // than at run time, which is where `total.trim` used to be caught.
            match self.type_of(lower, value) {
                Some(Type::Response) if response_field(&name).is_some() => {}
                Some(Type::Response) => {
                    return Err(self.err(
                        format!("a Response carries `status` and `body`, not `{name}`"),
                        line,
                        col,
                    ));
                }
                Some(Type::Record(record))
                    if self
                        .record_def(&record)
                        .is_some_and(|def| def.field(&name).is_some()) => {}
                Some(Type::Record(record)) => {
                    return Err(self.err(
                        format!("record `{record}` has no field `{name}`"),
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
            // There is no Uuid literal token, so the target type is what makes a string
            // one (`docs/declarations.md`). The same rule a `const` and an entity default
            // already followed, which is where it used to stop.
            Token::Text(text) if matches!(expect.as_ref().map(inner_of), Some(Type::Uuid)) => {
                if uuid::Uuid::parse_str(&text).is_err() {
                    return Err(self.err(format!("`{text}` is not a Uuid"), span.line, span.col));
                }
                Ok(lower.b.lit(Literal::Uuid(text)))
            }
            // The same rule for a `Timestamp`, in the expression half of it.
            Token::Text(text) if matches!(expect.as_ref().map(inner_of), Some(Type::Timestamp)) => {
                let Some(micros) = value::timestamp(&text) else {
                    return Err(self.err(not_a_timestamp(&text), span.line, span.col));
                };
                Ok(lower.b.lit(Literal::Timestamp(micros)))
            }
            Token::Text(text) => Ok(lower.b.lit(Literal::Str(text))),
            Token::Word(Keyword::True) => Ok(lower.b.bool(true)),
            Token::Word(Keyword::False) => Ok(lower.b.bool(false)),
            Token::Word(Keyword::If) => {
                let cond = self.header_expr(lower, Some(Type::Bool))?;
                self.expect_sym(Sym::LBrace)?;
                let then = self.expr(lower, expect.clone())?;
                self.expect_sym(Sym::RBrace)?;
                self.expect_word(Keyword::Else)?;
                self.expect_sym(Sym::LBrace)?;
                let otherwise = self.expr(lower, expect)?;
                self.expect_sym(Sym::RBrace)?;
                // Both arms are the value, so two known types that disagree is a value
                // with no type at all. Where a target declared one this has already
                // been said twice, once per arm; where nothing did, this is the only
                // place it can be said.
                if let (Some(left), Some(right)) =
                    (self.type_of(lower, then), self.type_of(lower, otherwise))
                    && left != right
                {
                    return Err(self.err(
                        format!(
                            "these branches give a {left} and a {right}, so this `if` has no one type; both arms are the value"
                        ),
                        span.line,
                        span.col,
                    ));
                }
                lower.b.at(span);
                Ok(lower.b.if_expr(cond, then, otherwise))
            }
            Token::Sym(Sym::LParen) => {
                let outer = mem::replace(&mut self.no_record_literal, false);
                let value = self.expr(lower, expect)?;
                self.no_record_literal = outer;
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
            Token::Sym(Sym::LBrace) => self.object_literal(lower, expect.as_ref(), span),
            Token::Sym(Sym::LBracket) => self.bracketed(lower, expect, span),
            Token::TextOpen(head) => self.interpolation(lower, head, span),
            Token::Sym(Sym::Dot) => {
                let (line, col) = self.here();
                let field = self.expect_ident()?;

                let ty = match self.stored.as_ref() {
                    None => {
                        return Err(self.err(
                            format!(
                                "`.{field}` reads the stored value, which only a `patch` or `update` value can do"
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
                if let Some(value) = self.builtin(lower, &name, expect.as_ref(), span)? {
                    return Ok(value);
                }
                if self.at_sym(Sym::LParen) && self.fn_sig(&name).is_some() {
                    if self.fn_sig(&name).is_some_and(|sig| sig.ret.is_none()) {
                        return Err(self.err(
                            format!(
                                "`{name}` returns nothing, so a call to it is a statement rather than a value"
                            ),
                            span.line,
                            span.col,
                        ));
                    }
                    return self.call_fn(lower, name, span);
                }
                if !self.no_record_literal
                    && self.at_sym(Sym::LBrace)
                    && self.record_def(&name).is_some()
                {
                    return self.record_literal(lower, name, span);
                }
                if let Some(def) = self.const_def(&name) {
                    let value = def.value.clone();
                    return Ok(lower.b.lit(value));
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
                match self.visible_enums().filter(|def| def.has(&name)).count() {
                    1 => {
                        let ty = self
                            .visible_enums()
                            .find(|def| def.has(&name))
                            .expect("counted one")
                            .name
                            .clone();
                        Ok(lower.b.lit(Literal::Enum { ty, variant: name }))
                    }
                    0 => Err(self.not_in_scope(&name, spanned.line, spanned.col)),
                    _ => {
                        let candidates: Vec<&str> = self
                            .visible_enums()
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
        // A target that is not numeric is not a target. It reaches here from the other
        // operand of a comparison, so `owner_email > 0` used to hint `String` onto the
        // `0` and report that a number cannot be a String. That is true and it is not
        // the mistake: the mistake is comparing a String to a number, and the operator
        // table says so once the literal has been allowed a type of its own. `settle`
        // already ignores a hint that will not resolve, so this only moves the moment.
        let target = expect
            .filter(|ty| matches!(inner_of(ty), Type::Int | Type::Decimal(_) | Type::Money(_)));
        let defaulted = target.is_none();
        // A literal in a `T?` position is still a `T`; the write site wraps it.
        let ty = match target {
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
        self.type_of(lower, id)
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
                if let Some(ty) = self.type_of(lower, rhs) {
                    self.retype(lower, lhs, &ty);
                }
            }
            (None, Some(_)) => {
                if let Some(ty) = self.type_of(lower, lhs) {
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

fn rounding_mode(name: &str) -> Option<Rounding> {
    Some(match name {
        "HalfUp" => Rounding::HalfUp,
        "HalfEven" => Rounding::HalfEven,
        "Down" => Rounding::Down,
        _ => return None,
    })
}

impl Parser {
    /// A static type where one is knowable. `None` is not an error: it means the
    /// expression's type is only decided at run time, which most of them are.
    fn type_of(&self, lower: &Lower, id: ExprId) -> Option<Type> {
        match lower.b.exprs().get(id)? {
            Expr::Lit(lit) => Some(value::literal(lit).ty()),
            Expr::Load(slot) => lower.b.slot_type(*slot).cloned(),
            Expr::Unary { operand, .. } => self.type_of(lower, *operand),
            // The narrowing already checked the slot held an optional; a load of
            // anything else is left alone rather than made into a second rule.
            Expr::Unwrap(inner) => match self.type_of(lower, *inner)? {
                Type::Opt(inner) => Some(*inner),
                other => Some(other),
            },
            Expr::Binary { op, lhs, rhs } => match op {
                BinOp::Eq
                | BinOp::Ne
                | BinOp::Lt
                | BinOp::Le
                | BinOp::Gt
                | BinOp::Ge
                | BinOp::And
                | BinOp::Or => Some(Type::Bool),
                // The operator table, not the left operand. Taking the left one gave
                // `Money(n)` for `Money(n) / Money(n)`, whose value is a `Decimal(6)`,
                // and a synthesised type that is wrong is worse than none: it rejects
                // a program that is right.
                _ => types::arithmetic(
                    *op,
                    &self.type_of(lower, *lhs)?,
                    &self.type_of(lower, *rhs)?,
                ),
            },
            Expr::Method {
                receiver, method, ..
            } => method_sig(&self.type_of(lower, *receiver)?, method).map(|sig| sig.ret),
            // Both arms, because both are the value. Reading only the `then` arm
            // reported a type the `else` arm may not produce.
            Expr::If {
                then, otherwise, ..
            } => {
                let then = self.type_of(lower, *then)?;
                (self.type_of(lower, *otherwise)? == then).then_some(then)
            }
            Expr::Field { receiver, name } => match self.type_of(lower, *receiver)? {
                Type::Record(ty) => self.record_def(&ty)?.field(name).map(|f| f.ty.clone()),
                // Only a `Response` has parenless fields, so only a `Response` may
                // answer for one. Anything else reaching `status` or `body` here was
                // reading the table for a type that does not have them.
                Type::Response => response_field(name),
                _ => None,
            },
            Expr::Object(_) => Some(Type::Json),
            Expr::Interp(_) => Some(Type::String),
            Expr::List { items, inner } => Some(Type::list(match inner {
                Some(declared) => declared.clone(),
                None => items.first().and_then(|id| self.type_of(lower, *id))?,
            })),
            // The declared element type when the target gave one, exactly as a list
            // literal does, so an empty result keeps the type it was written for.
            Expr::Comp { yields, inner, .. } => Some(Type::list(match inner {
                Some(declared) => declared.clone(),
                None => self.type_of(lower, *yields)?,
            })),
            Expr::Call { builtin, .. } => Some(match builtin {
                Builtin::UuidDerive => Type::Uuid,
                Builtin::JsonEncode => Type::String,
                Builtin::TimestampParse => Type::opt(Type::Timestamp),
                Builtin::MoneyParse(scale) => Type::opt(Type::Money(*scale)),
                Builtin::HttpGet
                | Builtin::HttpPost
                | Builtin::HttpPut
                | Builtin::HttpPatch
                | Builtin::HttpDelete => Type::Response,
            }),
            Expr::Invoke { .. } => Some(Type::Outcome),
            Expr::Record { ty, .. } => Some(Type::Record(ty.clone())),
            Expr::CallFn { function, .. } => self.fn_sig(function)?.ret.clone(),
            // Rule 12: an optional in, an optional out, so a `reveal` of a field that may
            // not be there still reads as one.
            // Rule 12: an optional in, an optional out, and the seal comes off.
            Expr::Reveal(value) => Some(self.type_of(lower, *value)?.unsealed()),
        }
    }
}

/// A value that does not fill the position it was written into. The wording is the
/// runtime's, so the static report and the dynamic one read the same, and two shapes
/// carry the way out because in both the author knows what they meant and only needs
/// the spelling.
fn mismatch(found: &Type, want: &Type) -> String {
    let fix = match (found, want) {
        (Type::Opt(inner), _) if fills(inner, want) => format!(
            "; `unwrap_or` gives it a fallback, or a branch that proves it present makes it a {inner} without one"
        ),
        (Type::Int | Type::String, Type::Timestamp) => {
            ", and a Timestamp is written as a string, like \"2026-01-01T00:00:00Z\"".to_string()
        }
        (Type::String, Type::Uuid) => {
            ", and a Uuid is written as a string, so this one is not one".to_string()
        }
        _ => String::new(),
    };
    format!("expected {want}, found {found}{fix}")
}

/// A method the receiver does not have. The pair this sees most is an optional asked an
/// emptiness question and a plain value asked a presence one, which is one confusion
/// from either side, so each is named rather than only refused: an author who wrote
/// `is_empty()` on a `String?` knows exactly what they meant.
fn no_method(receiver: &Type, name: &str) -> String {
    let instead = match (receiver, name) {
        (Type::Opt(inner), "is_empty") => format!(
            "; an optional is asked `is_none()`. `is_empty()` is a question for a {inner}, and this may not be holding one"
        ),
        (Type::String, "is_none" | "is_some") => {
            "; a String is always there, so `is_empty()` is the question. Absence is what a String? is for".to_string()
        }
        (Type::Opt(_), _) => format!(
            "; an optional has `is_some()`, `is_none()` and `unwrap_or(fallback)`, and `{name}` is a question for what it holds"
        ),
        // Named because its absence is a decision rather than an oversight, and the
        // author who reached for it is the one the decision is addressed to.
        (Type::Timestamp, _) => {
            "; calendar arithmetic is not in the language, because month-end clamping is one opinion among several and a language that picks one cannot be argued with. Write it as a `fn`".to_string()
        }
        (_, "unwrap_or") => {
            format!("; a {receiver} is already there, so there is nothing to fall back to")
        }
        _ => String::new(),
    };
    format!("no method `{name}` on {receiver}{instead}")
}

/// An operator with no row in the table. The three `docs/money.md` names get the reason
/// as well as the fact, because each of them is a mistake with a shape: the operator is
/// not missing, the expression means something the author did not intend.
fn bad_operands(op: BinOp, lhs: &Type, rhs: &Type) -> String {
    let why = match (lhs, op, rhs) {
        (Type::Money(_), BinOp::Mul, Type::Money(_)) => {
            "; two amounts multiplied is not an amount. An amount scales by an `Int` or a `Decimal`"
        }
        (Type::Money(a), _, Type::Money(b)) if a != b => {
            "; two amounts meet at one scale, the rule `Decimal` has for the same reason: a silent rescale is how a total loses a cent"
        }
        (Type::Money(_), BinOp::Add | BinOp::Sub, Type::Decimal(_))
        | (Type::Decimal(_), BinOp::Add | BinOp::Sub, Type::Money(_)) => {
            "; this adds a rate to an amount. To scale one, write `*`, or `mul(rate, HalfUp)` where the result has to round"
        }
        (Type::Decimal(a), _, Type::Decimal(b)) if a != b => {
            "; two decimals meet at one scale, and widening one is the author's call to make"
        }
        _ => "",
    };
    format!("cannot apply `{op}` to {lhs} and {rhs}{why}")
}

/// A written `Timestamp` that did not read as one. The offset is the part authors
/// leave off, and `value::timestamp` refuses a local time on purpose, so the message
/// names the shape rather than only reporting that the text was wrong.
fn not_a_timestamp(text: &str) -> String {
    format!(
        "`{text}` is not a Timestamp; it is RFC 3339 and carries an offset, like \"2026-01-01T00:00:00Z\" or \"2026-01-01T09:30:00+10:00\""
    )
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

/// What a condition proves about one optional, and on which branch: `true` means the
/// value is present in the `then` branch. Recognised on a single test, because a
/// conjunction and a disjunction narrow in opposite directions and getting that wrong
/// is silent. See `docs/optionals.md`.
fn narrowing(lower: &Lower, cond: ExprId) -> Option<(Slot, bool)> {
    let exprs = lower.b.exprs();
    let (test, sense) = match exprs.get(cond)? {
        Expr::Unary {
            op: UnOp::Not,
            operand,
        } => (*operand, false),
        _ => (cond, true),
    };
    let Expr::Method {
        receiver,
        method,
        args,
    } = exprs.get(test)?
    else {
        return None;
    };
    if !args.is_empty() {
        return None;
    }
    let present = match method.as_str() {
        "is_some" => sense,
        "is_none" => !sense,
        _ => return None,
    };
    let Expr::Load(slot) = exprs.get(*receiver)? else {
        return None;
    };
    matches!(lower.b.slot_type(*slot), Some(Type::Opt(_))).then_some((*slot, present))
}
impl Parser {
    fn effect_decl(&mut self, events: &[EventDef]) -> Result<Effect, SyntaxError> {
        let module = self.module_at(self.pos).map(str::to_string);
        self.expect_word(Keyword::Effect)?;
        let name = self.expect_ident()?;
        self.expect_sym(Sym::LBrace)?;
        let body = self.pos;

        // Sweep one: the helpers' signatures, so an arm may call one declared below it
        // and a helper may call a sibling. The same two-sweep shape `projector_shell`
        // uses for a projector's own enums, and for the same reason: declaration order
        // is irrelevant everywhere else in heklang.
        while !self.at_sym(Sym::RBrace) && !matches!(self.peek(), Token::End) {
            match self.peek() {
                Token::Word(Keyword::Fn) => self.local_fn_signature(&name)?,
                Token::Word(Keyword::On) => self.skip_handler()?,
                other => {
                    return self.fail(format!("expected `on` or `fn`, found {other}"));
                }
            }
        }

        self.pos = body;
        self.in_effect = Some(name.clone());
        let mut functions: Vec<Function> = Vec::new();
        let mut arms: Vec<Arm> = Vec::new();
        let mut at: Vec<usize> = Vec::new();
        while !self.at_sym(Sym::RBrace) && !matches!(self.peek(), Token::End) {
            if self.at_word(Keyword::Fn) {
                functions.push(self.fn_decl(events, Kind::EffectFn)?);
                continue;
            }
            let start = self.pos;
            let arm = self.arm(&name, events)?;
            // Rule 1: one event selects exactly one arm, so two arms naming a type
            // would make declaration order decide what a replay does. Unchanged by an
            // arm listing several paths; it is checked over the whole set instead.
            for path in &arm.events {
                let clash = arms
                    .iter()
                    .position(|other| other.events.iter().any(|seen| seen == path));
                if let Some(index) = clash {
                    let first = self.location(at[index]);
                    return Err(self.err(
                        format!(
                            "`{name}` already has an arm on {path} at {first}; one event selects exactly one arm"
                        ),
                        arm.span.line,
                        arm.span.col,
                    ));
                }
            }
            arms.push(arm);
            at.push(start);
        }
        self.expect_sym(Sym::RBrace)?;
        self.in_effect = None;
        self.local_fns.clear();

        if arms.is_empty() {
            return self.fail(format!("effect `{name}` declares no arms"));
        }
        Ok(Effect {
            name,
            module,
            functions,
            arms,
        })
    }

    /// Sweep one's half of an effect-local `fn`: the signature, and nothing else.
    fn local_fn_signature(&mut self, effect: &Ident) -> Result<(), SyntaxError> {
        self.expect_word(Keyword::Fn)?;
        let (line, col) = self.here();
        let name = self.expect_ident()?;
        if self.local_fns.iter().any(|other| other.name == name) {
            return Err(self.err(
                format!("`{effect}` already declares a `fn` named `{name}`"),
                line,
                col,
            ));
        }
        // A module `fn` is in scope inside every effect, so a local one of the same
        // name would silently change which code runs at a call site that reads the
        // same in two files. Two different effects may each declare the name.
        if self.functions.iter().any(|other| other.name == name) {
            return Err(self.err(
                format!(
                    "`{name}` is already a `fn` at module scope, and one is in scope inside every effect; an effect-local `fn` cannot shadow it"
                ),
                line,
                col,
            ));
        }
        let params = self.param_list(true)?;
        let ret = self.fn_result(Kind::EffectFn)?;
        self.local_fns.push(Signature { name, params, ret });
        self.skip_braced()
    }

    fn arm(&mut self, effect: &Ident, events: &[EventDef]) -> Result<Arm, SyntaxError> {
        let span = self.span_here();
        self.expect_word(Keyword::On)?;

        let mut paths = vec![self.expect_path()?];
        let mut spans = vec![self.span_here()];
        while self.eat_sym(Sym::Comma) {
            spans.push(self.span_here());
            let path = self.expect_path()?;
            if paths.contains(&path) {
                return self.fail(format!("this arm already lists {path}"));
            }
            paths.push(path);
        }
        let def = self.common_fields(&paths, &spans, events)?;
        self.triggers = paths.clone();

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

        self.triggers.clear();
        let arm = lower.b.finish_arm(paths, span, body);
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

    /// Whether a `return` in a `fn` ends without a value. `http.*` is deliberately not
    /// counted even though it can begin a statement: its statement form is a value
    /// being discarded, so after `return` it is the value being returned.
    fn ends_return(&self) -> bool {
        if matches!(self.peek(), Token::Ident(name) if name == "http") {
            return false;
        }
        self.at_sym(Sym::RBrace) || self.starts_statement()
    }

    /// Whether the next token could begin a statement. Used by `return` to tell a bare
    /// one from a `return <expr>`.
    fn starts_statement(&self) -> bool {
        match self.peek() {
            Token::Word(
                Keyword::If
                | Keyword::Return
                | Keyword::Emit
                | Keyword::Put
                | Keyword::Patch
                | Keyword::Update
                | Keyword::Delete
                | Keyword::Let
                | Keyword::Invoke
                | Keyword::For
                | Keyword::State
                | Keyword::Guard,
            ) => true,
            Token::Ident(name) => self.starts_effect_statement(name),
            _ => false,
        }
    }

    /// The fields every listed path shares, as one synthesised definition. A field
    /// counts only when its type **and** its `@subject` match everywhere, so a
    /// `reveal` through a multi-path binding stays sound.
    fn common_fields(
        &self,
        paths: &[EventPath],
        spans: &[Span],
        events: &[EventDef],
    ) -> Result<EventDef, SyntaxError> {
        let first = self.event_def(events, &paths[0])?.clone();
        if paths.len() == 1 {
            return Ok(first);
        }

        let rest: Vec<&EventDef> = paths[1..]
            .iter()
            .map(|path| self.event_def(events, path))
            .collect::<Result<_, _>>()?;

        let mut fields = Vec::new();
        for field in &first.fields {
            let mut shared = true;
            for other in &rest {
                let matches = other
                    .field(&field.name)
                    .is_some_and(|found| found.ty == field.ty && found.subject == field.subject);
                if !matches {
                    shared = false;
                    break;
                }
            }
            if shared {
                fields.push(field.clone());
            }
        }
        if fields.is_empty() {
            let at = spans[0];
            return Err(self.err(
                format!(
                    "these event types share no field, so an arm listing them could name nothing; {} is the first that differs",
                    paths[1]
                ),
                at.line,
                at.col,
            ));
        }
        Ok(EventDef::new(first.path.clone(), fields))
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
                    "fail",
                    span,
                )?;
                self.bump();
                self.expect_sym(Sym::LParen)?;
                let message = self.expr(lower, Some(Type::String))?;
                self.end_args()?;
                Ok(Stmt::Fail { message, span })
            }
            "log" => {
                self.gate(
                    "`log` is an effect builtin; a command's decision is already visible in what it emits",
                    "`log` is an effect builtin; a projector runs once per rebuild, so its lines are not a trace",
                    "log",
                    span,
                )?;
                self.bump();
                self.expect_sym(Sym::LParen)?;
                let message = self.expr(lower, Some(Type::String))?;
                self.end_args()?;
                Ok(Stmt::Log { message })
            }
            "erase" => {
                self.gate(
                    "only an effect crosses the decrypt boundary; a command decides from state without reaching personal data",
                    "only an effect crosses the decrypt boundary; a projector stores what the event carries",
                    "erase a subject key",
                    span,
                )?;
                self.arm_only(
                    "erase a subject key",
                    "write the `erase` in the arm that calls it",
                    span,
                )?;
                self.bump();
                self.expect_sym(Sym::LParen)?;

                // Naming the subject is what a value the parser cannot trace back to a
                // field needs, and it is the spelling `docs/testing.md` already uses
                // for the matching expectation.
                let named = if self.at_named_subject() {
                    let name = self.expect_ident()?;
                    self.expect_sym(Sym::Comma)?;
                    Some(name)
                } else {
                    None
                };

                let (line, col) = self.here();
                let value = self.expr(lower, None)?;
                self.end_args()?;

                let subject = match named {
                    Some(name) => {
                        self.check_named_subject(lower, &name, value, line, col)?;
                        name
                    }
                    // Without a name the subject has to be recovered, and only a
                    // trigger field carries one. Rule 12's fold path does not apply:
                    // it tracks the subject of a *value*, and this is the id itself.
                    None => {
                        let Some(subject) = self.trigger_field(lower, value) else {
                            return Err(self.err(
                                "`erase` takes a field of the triggering event, like `e.customer_id`, or names its subject: `erase(customer_id, id)`"
                                    .to_string(),
                                line,
                                col,
                            ));
                        };
                        subject
                    }
                };

                let Some(field) = subject_field(events, &subject) else {
                    return Err(self.err(
                        format!(
                            "nothing is scoped to `{subject}`, so there is no key to erase; `erase` takes the subject id that a field is declared `@subject(...)` of"
                        ),
                        line,
                        col,
                    ));
                };
                // The declared type of the field the key is filed under. Skipped when
                // the value's type is unknown, the way every other optional check here
                // is skipped rather than guessed.
                if let Some(found) = self.type_of(lower, value)
                    && &found != field
                {
                    return Err(self.err(
                        format!(
                            "`{subject}` files its keys under a {field}, so `erase` cannot take a {found}"
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

    /// Rule 9's second rule, which only the named form can reach: an id learned by
    /// revealing must not be erased, because a repeat request for an already-erased
    /// subject then cannot be read at all. The inferring form cannot reach it, since a
    /// `reveal` is not a trigger field load.
    fn check_named_subject(
        &self,
        lower: &Lower,
        subject: &str,
        value: ExprId,
        line: u32,
        col: u32,
    ) -> Result<(), SyntaxError> {
        if let Some(at) = reveal_in(lower.b.exprs(), value) {
            return Err(self.err(
                format!(
                    "the id at {at} was learned by revealing, so `erase({subject}, ...)` would make a repeat request for an erased subject unreadable; take a subject id from a plaintext field"
                ),
                line,
                col,
            ));
        }
        Ok(())
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

    /// A `fn` is pure, and the message says which rule that keeps rather than calling
    /// it a style. Rule 9's erase-last check runs over one arm's statement tree; the
    /// moment a helper can hold a `reveal` or an `erase` it has to follow calls.
    fn purity_error(&self, what: &str, span: Span) -> SyntaxError {
        self.err(
            format!(
                "a `fn` is pure, so it cannot {what}; that is what keeps the erase-last check inside one arm instead of following calls"
            ),
            span.line,
            span.col,
        )
    }

    /// Rule 12: sealed content may be written only where the same seal is declared.
    /// Everywhere else it would leave the boundary without `reveal`, which is the one
    /// thing the seal exists to stop. A `state` fold and an entity column are the
    /// exceptions: both take it by propagating the seal onto themselves.
    fn check_seal(
        &self,
        lower: &Lower,
        value: ExprId,
        want: &Type,
        line: u32,
        col: u32,
    ) -> Result<(), SyntaxError> {
        if self.folding || self.propagating {
            return Ok(());
        }
        let Some(found) = self.type_of(lower, value) else {
            return Ok(());
        };
        let Some(subject) = found.subject() else {
            return Ok(());
        };
        if want.subject() == Some(subject) {
            return Ok(());
        }
        Err(self.err(
            format!(
                "this is content sealed under `{subject}` and a {want} is not; `reveal` it first, because writing it here takes it out from behind the decrypt boundary"
            ),
            line,
            col,
        ))
    }

    /// A value written where a type is declared has to fill it. `docs/types.md` is the
    /// rule; `fills` is the whole of it, and it is the same relation `interp::coerce`
    /// applies at the write, so what passes here is what the runtime would have taken.
    ///
    /// Silent where synthesis is unknown, because an unknown type is not an accusation.
    /// This check shrinks as synthesis grows and never guesses, which is what makes it
    /// safe to run at every declared position at once.
    ///
    /// The seal is transparent here, as it is to the runtime: writing plaintext into a
    /// sealed position is the encrypting direction and needs no ceremony, and the other
    /// direction is `check_seal`'s, which has already run.
    fn check_type(
        &self,
        lower: &Lower,
        value: ExprId,
        want: &Type,
        line: u32,
        col: u32,
    ) -> Result<(), SyntaxError> {
        let Some(found) = self.type_of(lower, value) else {
            return Ok(());
        };
        if fills(&found.unsealed(), &want.unsealed()) {
            return Ok(());
        }
        Err(self.err(mismatch(&found, want), line, col))
    }

    /// The same rule where nothing declares a type: an interpolation hole, a
    /// comparison, an HTTP body. Reading content into any of them is reading it.
    fn no_seal(
        &self,
        lower: &Lower,
        value: ExprId,
        what: &str,
        line: u32,
        col: u32,
    ) -> Result<(), SyntaxError> {
        let Some(subject) = self
            .type_of(lower, value)
            .as_ref()
            .and_then(Type::subject)
            .cloned()
        else {
            return Ok(());
        };
        Err(self.err(
            format!(
                "this is content sealed under `{subject}`, so it cannot {what} without `reveal`"
            ),
            line,
            col,
        ))
    }

    /// `reveal` and `erase` stay in the arm. Not for purity, since an effect-local
    /// `fn` may call out, but because rule 9 checks that no reveal is reachable from
    /// an erase over one arm's statement tree. See `docs/functions.md`.
    fn arm_only(&self, what: &str, fix: &str, span: Span) -> Result<(), SyntaxError> {
        if self.kind != Kind::EffectFn {
            return Ok(());
        }
        Err(self.err(
            format!(
                "an effect-local `fn` cannot {what}; it stays in the arm, which is what keeps rule 9's erase-last check inside one statement tree, so {fix}"
            ),
            span.line,
            span.col,
        ))
    }

    fn not_in_fn(&self, what: &str, span: Span) -> Result<(), SyntaxError> {
        if self.kind == Kind::Function {
            return Err(self.purity_error(what, span));
        }
        Ok(())
    }

    fn gate(
        &self,
        command: &str,
        projector: &str,
        what: &str,
        span: Span,
    ) -> Result<(), SyntaxError> {
        match self.kind {
            Kind::Effect | Kind::EffectFn => Ok(()),
            Kind::Command => Err(self.err(command, span.line, span.col)),
            Kind::Projector => Err(self.err(projector, span.line, span.col)),
            Kind::Function => Err(self.purity_error(what, span)),
            Kind::Test => Err(self.err(
                format!("a test states inputs and expectations, so it cannot {what}"),
                span.line,
                span.col,
            )),
        }
    }

    /// The soft-named builtins, resolved after the scope lookup so a local shadows one.
    /// That is what lets `log` and the rest stay usable as ordinary names (rule 10).
    fn builtin(
        &mut self,
        lower: &mut Lower,
        name: &str,
        expect: Option<&Type>,
        span: Span,
    ) -> Result<Option<ExprId>, SyntaxError> {
        let called = self.at_sym(Sym::LParen);
        match name {
            "http" if self.at_sym(Sym::Dot) => self.http_call(lower, span).map(Some),
            "Uuid" if self.at_sym(Sym::Dot) => self.uuid_call(lower, span).map(Some),
            "Map" if self.at_sym(Sym::Dot) => self.map_empty(lower, expect, span).map(Some),
            "Json" if self.at_sym(Sym::Dot) => self.json_member(lower, span).map(Some),
            "Timestamp" | "Money" if self.at_sym(Sym::Dot) => {
                self.parse_member(lower, name, expect, span).map(Some)
            }
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
                if self.kind == Kind::Test {
                    return Err(self.err(
                        "a test states inputs and expectations, so it cannot read a clock",
                        span.line,
                        span.col,
                    ));
                }
                // Rule 11 pins the clock once per invocation, into a slot the arm
                // fills before its body runs. A helper has no such slot, and giving it
                // one would make `now()` mean something different inside a call.
                if self.kind == Kind::EffectFn {
                    return Err(self.err(
                        "an effect-local `fn` cannot read a clock; `now()` is pinned once per invocation, so read it in the arm and pass it in",
                        span.line,
                        span.col,
                    ));
                }
                self.not_in_fn("read a clock", span)?;
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
        self.end_args()?;
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
            "call out",
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

        // Named, because the existing positional-third-argument error teaches rule 13
        // and should keep firing for one.
        let mut headers = None;
        if self.at_sym(Sym::Comma)
            && matches!(self.tokens.get(self.pos + 1).map(|t| &t.token), Some(Token::Ident(name)) if name == "headers")
        {
            self.bump();
            self.bump();
            self.expect_sym(Sym::Assign)?;
            let outer = self.in_body;
            self.in_body = true;
            headers = Some(self.expr(lower, Some(Type::Json))?);
            self.in_body = outer;
            self.eat_sym(Sym::Comma);
        }
        // A trailing comma closes the list; only a real third argument reaches rule 13.
        if self.at_trailing_comma() {
            self.bump();
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
        // Always present in the IR, empty when unwritten, so the interpreter reads one
        // shape rather than two.
        lower.b.at(span);
        args.push(match headers {
            Some(headers) => headers,
            None => lower.b.lit(Literal::EmptyJson),
        });
        lower.b.at(span);
        Ok(lower.b.expr(Expr::Call { builtin, args }))
    }

    fn reveal_call(&mut self, lower: &mut Lower, span: Span) -> Result<ExprId, SyntaxError> {
        self.gate(
            "only an effect crosses the decrypt boundary; a command decides from state without reaching personal data",
            "only an effect crosses the decrypt boundary; a projector stores what the event carries",
            "decrypt",
            span,
        )?;
        self.arm_only("decrypt", "pass the revealed value in as a parameter", span)?;
        self.not_in_fold("decrypt", span)?;
        self.expect_sym(Sym::LParen)?;
        let (line, col) = self.here();
        let value = self.expr(lower, None)?;
        self.end_args()?;

        // Rule 12: the seal is in the type, so this is the whole check. The subject,
        // its id and the field name ride on the value at run time, which is what lets
        // a `let` keep the seal instead of laundering it.
        let ty = self.type_of(lower, value);
        if ty.as_ref().and_then(Type::subject).is_none() {
            let found = match &ty {
                Some(ty) => format!("this is a plain {ty}"),
                None => "this is not one".to_string(),
            };
            return Err(self.err(
                format!(
                    "`reveal` takes subject-bound content and {found}; it decrypts a field declared `@subject(...)`, or a `state` folded from one. An arm that transforms what it folds drops the seal, because the key belongs to the field's content rather than to whatever is computed from it"
                ),
                line,
                col,
            ));
        }

        lower.b.at(span);
        Ok(lower.b.expr(Expr::Reveal(value)))
    }

    /// The triggering event's field this expression loads, if it is one. The only
    /// caller left is `erase`'s inferring form: a subject **id** is plaintext, so no
    /// type says which key namespace it names and the field it was bound from is the
    /// only place to recover that. See `docs/effects.md` rule 9.
    fn trigger_field(&self, lower: &Lower, value: ExprId) -> Option<Ident> {
        // A narrowed load is still that load: proving a value present says nothing
        // about where it came from.
        let value = match lower.b.exprs().get(value) {
            Some(Expr::Unwrap(inner)) => *inner,
            _ => value,
        };
        let Some(Expr::Load(slot)) = lower.b.exprs().get(value) else {
            return None;
        };
        lower.b.bound_field(*slot).map(str::to_string)
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
            let (line, col) = self.here();
            let hole = self.expr(lower, None)?;
            self.no_seal(lower, hole, "be interpolated into a string", line, col)?;
            parts.push(hole);
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

    /// `[a, b]`, `[]` or a comprehension. Which one is decided by a scan for a `for`
    /// at bracket depth zero, before anything is parsed, so neither form needs to be
    /// speculatively parsed and undone.
    fn bracketed(
        &mut self,
        lower: &mut Lower,
        expect: Option<Type>,
        span: Span,
    ) -> Result<ExprId, SyntaxError> {
        let inner = match expect.as_ref().map(inner_of) {
            Some(Type::List(item)) => Some(item.as_ref().clone()),
            _ => None,
        };

        let outer = mem::replace(&mut self.no_record_literal, false);
        let value = self.bracketed_inner(lower, inner, span);
        self.no_record_literal = outer;
        value
    }

    fn bracketed_inner(
        &mut self,
        lower: &mut Lower,
        inner: Option<Type>,
        span: Span,
    ) -> Result<ExprId, SyntaxError> {
        if let Some(at) = self.comprehension_for() {
            return self.comprehension(lower, inner, at, span);
        }

        if self.eat_sym(Sym::RBracket) {
            // A request body needs no declared element type. An empty array serialises
            // the same whatever it would have held, so there is nothing here for a
            // target to decide, and no declaration to take one from: a body's values
            // are typed by what they are rather than by where they land.
            let inner = inner.or_else(|| self.in_body.then_some(Type::Json));
            let Some(inner) = inner else {
                return Err(self.err(
                    "an empty list needs a target type to know what it holds; that comes from the `state`, parameter or field it fills",
                    span.line,
                    span.col,
                ));
            };
            lower.b.at(span);
            return Ok(lower.b.lit(Literal::List {
                inner,
                items: Vec::new(),
            }));
        }

        let mut items = Vec::new();
        loop {
            items.push(self.expr(lower, inner.clone())?);
            if !self.eat_sym(Sym::Comma) || self.at_sym(Sym::RBracket) {
                break;
            }
        }
        self.expect_sym(Sym::RBracket)?;
        lower.b.at(span);
        Ok(lower.b.expr(Expr::List { items, inner }))
    }

    /// Where the comprehension's `for` is, or `None` when this is a list literal.
    fn comprehension_for(&self) -> Option<usize> {
        let mut depth = 0i32;
        for (index, spanned) in self.tokens.iter().enumerate().skip(self.pos) {
            match &spanned.token {
                Token::Sym(Sym::LBracket | Sym::LParen | Sym::LBrace) => depth += 1,
                Token::Sym(Sym::RBracket) if depth == 0 => return None,
                Token::Sym(Sym::RBracket | Sym::RParen | Sym::RBrace) => depth -= 1,
                Token::Word(Keyword::For) if depth == 0 => return Some(index),
                Token::End => return None,
                _ => {}
            }
        }
        None
    }

    /// The produced expression is written first but uses bindings introduced later, so
    /// the loop is parsed first and the position rewound. One scan buys the reading
    /// order every language with comprehensions already uses.
    fn comprehension(
        &mut self,
        lower: &mut Lower,
        inner: Option<Type>,
        at: usize,
        span: Span,
    ) -> Result<ExprId, SyntaxError> {
        let start = self.pos;
        self.pos = at;
        self.expect_word(Keyword::For)?;
        lower.b.push_scope();
        let iter = self.iter_bindings(lower)?;
        let cond = if self.eat_word(Keyword::If) {
            Some(self.expr(lower, Some(Type::Bool))?)
        } else {
            None
        };
        self.expect_sym(Sym::RBracket)?;
        let end = self.pos;

        self.pos = start;
        let yields = self.expr(lower, inner.clone())?;
        if self.pos != at {
            return self.fail("expected `for` after a comprehension's expression");
        }
        self.pos = end;
        lower.b.pop_scope();
        lower.b.at(span);
        Ok(lower.b.expr(Expr::Comp {
            iter,
            cond,
            yields,
            inner,
        }))
    }

    /// `name [, name] in <container>`, shared by `for` and comprehensions so there is
    /// one rule rather than two. The caller has already pushed the scope.
    fn iter_bindings(&mut self, lower: &mut Lower) -> Result<Iter, SyntaxError> {
        let (line, col) = self.here();
        let first = self.expect_ident()?;
        let second = if self.eat_sym(Sym::Comma) {
            Some(self.expect_ident()?)
        } else {
            None
        };
        self.expect_word(Keyword::In)?;
        let (over_line, over_col) = self.here();
        let over = self.header_expr(lower, None)?;

        let (index_ty, item_ty) = match self.type_of(lower, over) {
            Some(Type::List(item)) => (second.as_ref().map(|_| Type::Int), item.as_ref().clone()),
            Some(Type::Map(key, value)) => {
                if second.is_none() {
                    return Err(self.err(
                        "a map yields a key beside its value, so `for` over one binds two names; write `for key, value in ...`",
                        line,
                        col,
                    ));
                }
                (Some(key.as_ref().clone()), value.as_ref().clone())
            }
            Some(other) => {
                return Err(self.err(
                    format!("`for` walks a List or a Map, and this is a {other}"),
                    over_line,
                    over_col,
                ));
            }
            None => {
                return Err(self.err(
                    "cannot tell what this holds; `for` needs the container's element type, which comes from a declaration",
                    over_line,
                    over_col,
                ));
            }
        };

        let (index_name, item_name) = match second {
            Some(second) => (Some(first), second),
            None => (None, first),
        };
        let index = index_name.map(|name| lower.b.alloc(name, index_ty));
        let item = lower.b.alloc(item_name, Some(item_ty));
        Ok(Iter { index, item, over })
    }

    /// `Timestamp.parse(text)` and `Money.parse(text)`, the two values a webhook or a
    /// GraphQL response delivers as a string. Both return an optional, because the text
    /// comes from outside and may be anything.
    fn parse_member(
        &mut self,
        lower: &mut Lower,
        ty: &str,
        expect: Option<&Type>,
        span: Span,
    ) -> Result<ExprId, SyntaxError> {
        self.expect_sym(Sym::Dot)?;
        let (line, col) = self.here();
        let member = self.expect_ident()?;
        if member != "parse" {
            return Err(self.err(
                format!("`{ty}` has no `{member}`; it has `parse(text)`"),
                line,
                col,
            ));
        }
        let builtin = if ty == "Timestamp" {
            Builtin::TimestampParse
        } else {
            // The scale is a property of where the amount lands, not of the text, so
            // it comes from the target the way `Money.empty` would.
            let Some(Type::Money(scale)) = expect.map(inner_of) else {
                return Err(self.err(
                    "`Money.parse` needs a target scale to know what it is parsing into; that comes from the field, parameter or `state` it fills",
                    span.line,
                    span.col,
                ));
            };
            Builtin::MoneyParse(*scale)
        };
        self.expect_sym(Sym::LParen)?;
        let outer = mem::replace(&mut self.no_record_literal, false);
        let text = self.expr(lower, Some(Type::String))?;
        self.no_record_literal = outer;
        self.end_args()?;
        lower.b.at(span);
        Ok(lower.b.expr(Expr::Call {
            builtin,
            args: vec![text],
        }))
    }

    /// `Json.empty` and `Json.encode(value)`. The encoder is rule 8's table pointed at
    /// a string instead of a socket, which is why it is not a second serialisation.
    fn json_member(&mut self, lower: &mut Lower, span: Span) -> Result<ExprId, SyntaxError> {
        self.expect_sym(Sym::Dot)?;
        let (line, col) = self.here();
        let member = self.expect_ident()?;
        match member.as_str() {
            "empty" => {
                lower.b.at(span);
                Ok(lower.b.lit(Literal::EmptyJson))
            }
            "encode" => {
                self.expect_sym(Sym::LParen)?;
                let outer = mem::replace(&mut self.no_record_literal, false);
                // Any value encodes, so there is no hint to give, except that a `{`
                // here is an object rather than something that needs a target. What
                // is inside it is a body like any other, so it is read as one: without
                // that only the outermost brace had a target, and a nested object or
                // an empty array failed on a rule about declarations it has none of.
                let body = mem::replace(&mut self.in_body, true);
                let hint = self.at_sym(Sym::LBrace).then_some(Type::Json);
                let value = self.expr(lower, hint)?;
                self.in_body = body;
                self.no_record_literal = outer;
                self.end_args()?;
                lower.b.at(span);
                Ok(lower.b.expr(Expr::Call {
                    builtin: Builtin::JsonEncode,
                    args: vec![value],
                }))
            }
            other => Err(self.err(
                format!("`Json` has no `{other}`; it has `empty` and `encode(value)`"),
                line,
                col,
            )),
        }
    }

    /// `Map.empty`, on the type for the reason `Uuid.derive` is: the global namespace
    /// is closed to constructors.
    fn map_empty(
        &mut self,
        lower: &mut Lower,
        expect: Option<&Type>,
        span: Span,
    ) -> Result<ExprId, SyntaxError> {
        self.expect_sym(Sym::Dot)?;
        let (line, col) = self.here();
        let member = self.expect_ident()?;
        if member != "empty" {
            return Err(self.err(
                format!("`Map` has no `{member}`; it has `empty`, and everything else is a method on a map value"),
                line,
                col,
            ));
        }
        let Some(Type::Map(key, value)) = expect.map(inner_of) else {
            return Err(self.err(
                "`Map.empty` needs a target type to know what it holds; that comes from the `state`, parameter or field it fills",
                span.line,
                span.col,
            ));
        };
        let lit = Literal::EmptyMap(key.as_ref().clone(), value.as_ref().clone());
        lower.b.at(span);
        Ok(lower.b.lit(lit))
    }

    /// `Name { field: value }`, with the same bare-name shorthand `emit` and `put`
    /// already use. Only reached when `Name` is a declared record and no `if` or `for`
    /// header is waiting for its block, so the `{` cannot be misread.
    fn record_literal(
        &mut self,
        lower: &mut Lower,
        name: Ident,
        span: Span,
    ) -> Result<ExprId, SyntaxError> {
        let def = self.record_def(&name).cloned().expect("checked by caller");
        self.expect_sym(Sym::LBrace)?;

        let outer = mem::replace(&mut self.no_record_literal, false);
        let mut fields: Vec<(Ident, ExprId)> = Vec::new();
        while !self.at_sym(Sym::RBrace) {
            let (line, col) = self.here();
            let field = self.expect_ident()?;
            let Some(declared) = def.field(&field) else {
                return Err(self.err(format!("record `{name}` has no field `{field}`"), line, col));
            };
            if fields.iter().any(|(seen, _)| seen == &field) {
                return Err(self.err(format!("`{field}` is given twice"), line, col));
            }
            let expected = declared.ty.clone();
            let value = if self.eat_sym(Sym::Colon) {
                self.expr(lower, Some(expected))?
            } else {
                if lower.b.lookup(&field).is_none() {
                    return Err(self.not_in_scope(&field, line, col));
                }
                lower.b.at(Span::new(line, col));
                lower.b.load(&field)
            };
            fields.push((field, value));
            if !self.eat_sym(Sym::Comma) {
                break;
            }
        }
        self.expect_sym(Sym::RBrace)?;
        self.no_record_literal = outer;

        // Every field, always. A record with a hole would need a zero for the missing
        // one, and a partial record is what record update exists for, which is out.
        for declared in &def.fields {
            if !fields.iter().any(|(given, _)| given == &declared.name) {
                return self.fail(format!("record `{name}` needs `{}`", declared.name));
            }
        }
        lower.b.at(span);
        Ok(lower.b.expr(Expr::Record { ty: name, fields }))
    }

    /// An `if` or `for` header, where the `{` that follows opens a block rather than a
    /// record literal. Nested delimiters clear the restriction again, so a record
    /// literal inside a call in a header still works.
    fn header_expr(
        &mut self,
        lower: &mut Lower,
        expect: Option<Type>,
    ) -> Result<ExprId, SyntaxError> {
        let outer = mem::replace(&mut self.no_record_literal, true);
        let value = self.expr(lower, expect);
        self.no_record_literal = outer;
        value
    }

    fn object_literal(
        &mut self,
        lower: &mut Lower,
        expect: Option<&Type>,
        span: Span,
    ) -> Result<ExprId, SyntaxError> {
        // Legal where a `Json` is expected, which since `Json` became a declarable type
        // includes a `fn` return and a command parameter, not only an HTTP body. Rule
        // 7 still holds: `invoke` checks its fields against declared parameter types,
        // so an object only reaches one whose parameter is a `Json`.
        if !self.in_body && expect.map(inner_of) != Some(&Type::Json) {
            return Err(self.err(
                "an object literal is an HTTP request body; `invoke` takes a typed struct, checked against the command's parameters",
                span.line,
                span.col,
            ));
        }

        let outer = mem::replace(&mut self.no_record_literal, false);
        // Everything inside an object literal is JSON, whatever led here: an `http`
        // body, a `Json.encode`, a `fn` that returns one. So the members are read as a
        // body rather than only the outermost brace, which is what lets a nested object
        // and an empty array be written at any depth.
        let body = mem::replace(&mut self.in_body, true);
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
            let (at_line, at_col) = self.here();
            let value = self.expr(lower, None)?;
            self.no_seal(lower, value, "be sent in a request body", at_line, at_col)?;
            fields.push((key, value));
            if !self.eat_sym(Sym::Comma) {
                break;
            }
        }
        self.expect_sym(Sym::RBrace)?;
        self.in_body = body;
        self.no_record_literal = outer;
        lower.b.at(span);
        Ok(lower.b.expr(Expr::Object(fields)))
    }

    fn invoke_expr(&mut self, lower: &mut Lower, span: Span) -> Result<ExprId, SyntaxError> {
        self.gate(
            "`invoke` calls a command, so it can only appear in an effect; a command that needs another command's work emits, and an effect reacts",
            "`invoke` calls a command, so it can only appear in an effect; a projector is a pure fold",
            "call a command",
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
    /// A `fn` may not call itself, directly or through another. Termination by
    /// construction rather than by a cap, which is the argument the self-trigger check
    /// already makes: a fold arm re-runs on every attempt and must not be able to hang.
    /// Rule 5: the zero table is read by `materialize` and by nothing else, and a
    /// materializing `patch` on an absent key is the only thing that reaches it. So an
    /// entity written only by `put`, `update` and `delete` needs no zeros, and asking
    /// it for one asks for a sentinel. Runs after pass D, because the answer is in the
    /// handlers rather than in the declaration.
    fn check_zeros(&self, program: &Program) -> Result<(), SyntaxError> {
        for projector in &program.projectors {
            let defs = value::Defs {
                local: &projector.enums,
                enums: &self.module_enums,
                records: &self.records,
            };
            let mut patched: Vec<(&Ident, Span)> = Vec::new();
            for handler in &projector.handlers {
                walk_stmts(&handler.body, &mut |stmt| {
                    if let Stmt::Patch {
                        entity,
                        absent: Absent::Materialize,
                        span,
                        ..
                    } = stmt
                    {
                        patched.push((entity, *span));
                    }
                });
            }

            for (entity, span) in patched {
                let Some(def) = projector.entity(entity) else {
                    continue;
                };
                for (position, field) in def.fields.iter().enumerate() {
                    // The subscript supplies the key, and a default is the zero the
                    // author chose.
                    if position == def.key || field.default.is_some() {
                        continue;
                    }
                    if value::zero(&field.ty, defs).is_some() {
                        continue;
                    }
                    let ty = &field.ty;
                    // An enum with no `@default` has no zero for a different reason
                    // than a `Uuid` does, and it has one more fix.
                    let complaint = match ty {
                        Type::Enum(name) => format!(
                            "`{}` is a `{name}` with no `@default` variant; give the enum one, give the field a default, or make this an `update`",
                            field.name
                        ),
                        _ => format!(
                            "`{}` is a {ty} with no zero value; give it a default, make it `{ty}?`, or make this an `update`",
                            field.name
                        ),
                    };
                    let error = SyntaxError::new(
                        format!("this `patch` materializes a `{entity}`, and {complaint}"),
                        span.line,
                        span.col,
                    );
                    return Err(match &projector.module {
                        Some(module) => error.in_file(module),
                        None => error,
                    });
                }
            }
        }
        Ok(())
    }

    fn check_recursion(&self, program: &Program) -> Result<(), SyntaxError> {
        let Some(path) = fn_cycle(program) else {
            return Ok(());
        };
        let names: Vec<String> = path.iter().map(|name| format!("`{name}`")).collect();
        self.fail(format!(
            "{}: a `fn` cannot call itself, directly or through another, so that every call ends",
            names.join(" calls ")
        ))
    }

    fn check_cycles(&self, program: &Program) -> Result<(), SyntaxError> {
        let mut edges: Vec<Edge> = Vec::new();
        for effect in &program.effects {
            for arm in &effect.arms {
                for command in invoked(&arm.exprs, &arm.body) {
                    let Some(target) = program.command(&command) else {
                        continue;
                    };
                    for (trigger, event) in arm.events.iter().flat_map(|trigger| {
                        emitted(&target.body).into_iter().map(move |e| (trigger, e))
                    }) {
                        edges.push(Edge {
                            from: trigger.clone(),
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
        match stmt {
            Stmt::If {
                then, otherwise, ..
            } => {
                walk_stmts(then, visit);
                walk_stmts(otherwise, visit);
            }
            Stmt::For { body, .. } => walk_stmts(body, visit),
            _ => {}
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
        Stmt::For { iter, .. } => vec![iter.over],
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
        Stmt::Call { args, .. } => args.clone(),
        Stmt::Return(Return::Ok) => Vec::new(),
        Stmt::Return(Return::Invalid(message) | Return::Value(message)) => vec![*message],
        Stmt::Return(Return::Reject { code, message }) => vec![*code, *message],
    }
}

fn children(expr: &Expr) -> Vec<ExprId> {
    match expr {
        Expr::Lit(_) | Expr::Load(_) => Vec::new(),
        Expr::Unary { operand, .. } => vec![*operand],
        Expr::Unwrap(inner) => vec![*inner],
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
        Expr::List { items, .. } => items.clone(),
        Expr::Record { fields, .. } => fields.iter().map(|(_, id)| *id).collect(),
        Expr::CallFn { args, .. } => args.clone(),
        Expr::Comp {
            iter, cond, yields, ..
        } => {
            let mut ids = vec![iter.over, *yields];
            ids.extend(cond.iter().copied());
            ids
        }
        Expr::Call { args, .. } => args.clone(),
        Expr::Invoke { args, .. } => args.iter().map(|(_, id)| *id).collect(),
        Expr::Reveal(value) => vec![*value],
    }
}

/// Whether every path out of a body returns. An `if` counts only when it has an
/// `else` and both branches return; a `for` body never counts, because the container
/// it walks can be empty and the loop can run zero times.
/// The keywords a top-level declaration begins with.
fn starts_item(word: Keyword) -> bool {
    matches!(
        word,
        Keyword::Enum
            | Keyword::Record
            | Keyword::Const
            | Keyword::Fn
            | Keyword::Event
            | Keyword::Command
            | Keyword::Projector
            | Keyword::Effect
            | Keyword::Test
    )
}

fn always_returns(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|stmt| match stmt {
        Stmt::Return(_) | Stmt::Fail { .. } => true,
        Stmt::If {
            then, otherwise, ..
        } => !otherwise.is_empty() && always_returns(then) && always_returns(otherwise),
        _ => false,
    })
}

/// A `fn` in the call graph: the scope it resolves in, then its name. An effect-local
/// one is reachable only from its own effect, so a cycle never spans two scopes and
/// the path can still be printed as bare names.
type Callee = (Option<Ident>, Ident);

/// Every `fn` a body calls, at any depth in its expressions.
fn calls(exprs: &Exprs, body: &[Stmt]) -> Vec<Callee> {
    let mut found = Vec::new();
    walk_stmts(body, &mut |stmt| {
        // A void call is a statement, so it has no `Expr::CallFn` for the walk below
        // to find.
        if let Stmt::Call {
            function, scope, ..
        } = stmt
        {
            found.push((scope.clone(), function.clone()));
        }
        for root in roots(stmt) {
            collect(exprs, root, &mut found, &|_, expr, out| {
                if let Expr::CallFn {
                    function, scope, ..
                } = expr
                {
                    out.push((scope.clone(), function.clone()));
                }
            });
        }
    });
    found
}

/// A cycle in the call graph, as the path that closes it.
fn fn_cycle(program: &Program) -> Option<Vec<Ident>> {
    let mut done: BTreeSet<Callee> = BTreeSet::new();
    let module = program.functions.iter().map(|def| (None, def.name.clone()));
    let local = program.effects.iter().flat_map(|effect| {
        effect
            .functions
            .iter()
            .map(|def| (Some(effect.name.clone()), def.name.clone()))
    });
    for callee in module.chain(local) {
        let mut stack = Vec::new();
        if reaches_itself(program, &callee, &mut stack, &mut done) {
            return Some(stack.into_iter().map(|(_, name)| name).collect());
        }
    }
    None
}

fn reaches_itself(
    program: &Program,
    callee: &Callee,
    stack: &mut Vec<Callee>,
    done: &mut BTreeSet<Callee>,
) -> bool {
    if let Some(at) = stack.iter().position(|seen| seen == callee) {
        stack.drain(..at);
        stack.push(callee.clone());
        return true;
    }
    if done.contains(callee) {
        return false;
    }
    let Some(def) = program.function_in(callee.0.as_deref(), &callee.1) else {
        return false;
    };
    stack.push(callee.clone());
    for next in calls(&def.exprs, &def.body) {
        if reaches_itself(program, &next, stack, done) {
            return true;
        }
    }
    stack.pop();
    done.insert(callee.clone());
    false
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
            // A loop body may run again, so an `erase` anywhere in it is reachable
            // from every reveal in it, including one lexically above. Two passes reach
            // the fixed point, because the lattice has two elements.
            Stmt::For { body, .. } => {
                let once = scan(exprs, body, erased)?;
                let twice = scan(exprs, body, once.erased)?;
                erased = twice.erased;
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

/// The declared type of the field a subject files its keys under. `@subject(x)` must
/// name a field of the same event, so the event carrying the annotation carries `x`
/// too; absent when nothing is scoped to the name at all.
fn subject_field<'a>(events: &'a [EventDef], subject: &str) -> Option<&'a Type> {
    events
        .iter()
        .filter(|def| {
            def.fields
                .iter()
                .any(|field| field.subject.as_deref() == Some(subject))
        })
        .find_map(|def| {
            def.fields
                .iter()
                .find(|field| field.name == subject)
                .map(|field| &field.ty)
        })
}

/// The first `reveal` anywhere in one expression, by span. The statement-level
/// `first_reveal` below is rule 9's; this is the same walk over a single root.
fn reveal_in(exprs: &Exprs, root: ExprId) -> Option<Span> {
    let mut found: Vec<ExprId> = Vec::new();
    collect(exprs, root, &mut found, &|id, expr, out| {
        if matches!(expr, Expr::Reveal { .. }) {
            out.push(id);
        }
    });
    found
        .into_iter()
        .min_by_key(|id| id.0)
        .map(|id| exprs.span(id))
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
