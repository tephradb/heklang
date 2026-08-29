use std::fmt;

use crate::diagnostic::{Code, Diagnostic};
use crate::ir::{Number, Pos, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keyword {
    As,
    Command,
    Const,
    Delete,
    Effect,
    Else,
    Emit,
    Entity,
    Enum,
    Event,
    False,
    Fn,
    Fold,
    For,
    Guard,
    If,
    In,
    Invalid,
    Invoke,
    Let,
    None,
    On,
    Patch,
    Projector,
    Put,
    Record,
    Reject,
    Return,
    State,
    Test,
    True,
    Update,
}

impl Keyword {
    pub fn lookup(word: &str) -> Option<Self> {
        Some(match word {
            "as" => Keyword::As,
            "command" => Keyword::Command,
            "const" => Keyword::Const,
            "delete" => Keyword::Delete,
            "effect" => Keyword::Effect,
            "else" => Keyword::Else,
            "emit" => Keyword::Emit,
            "entity" => Keyword::Entity,
            "enum" => Keyword::Enum,
            "event" => Keyword::Event,
            "false" => Keyword::False,
            "fn" => Keyword::Fn,
            "fold" => Keyword::Fold,
            "for" => Keyword::For,
            "guard" => Keyword::Guard,
            "if" => Keyword::If,
            "in" => Keyword::In,
            "invalid" => Keyword::Invalid,
            "invoke" => Keyword::Invoke,
            "let" => Keyword::Let,
            "none" => Keyword::None,
            "on" => Keyword::On,
            "patch" => Keyword::Patch,
            "projector" => Keyword::Projector,
            "put" => Keyword::Put,
            "record" => Keyword::Record,
            "reject" => Keyword::Reject,
            "return" => Keyword::Return,
            "state" => Keyword::State,
            "test" => Keyword::Test,
            "true" => Keyword::True,
            "update" => Keyword::Update,
            _ => return None,
        })
    }

    pub fn text(self) -> &'static str {
        match self {
            Keyword::As => "as",
            Keyword::Command => "command",
            Keyword::Const => "const",
            Keyword::Delete => "delete",
            Keyword::Effect => "effect",
            Keyword::Else => "else",
            Keyword::Emit => "emit",
            Keyword::Entity => "entity",
            Keyword::Enum => "enum",
            Keyword::Event => "event",
            Keyword::False => "false",
            Keyword::Fn => "fn",
            Keyword::Fold => "fold",
            Keyword::For => "for",
            Keyword::Guard => "guard",
            Keyword::If => "if",
            Keyword::In => "in",
            Keyword::Invalid => "invalid",
            Keyword::Invoke => "invoke",
            Keyword::Let => "let",
            Keyword::None => "none",
            Keyword::On => "on",
            Keyword::Patch => "patch",
            Keyword::Projector => "projector",
            Keyword::Put => "put",
            Keyword::Record => "record",
            Keyword::Reject => "reject",
            Keyword::Return => "return",
            Keyword::State => "state",
            Keyword::Test => "test",
            Keyword::True => "true",
            Keyword::Update => "update",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sym {
    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Colon,
    Question,
    Arrow,
    To,
    Assign,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    AndAnd,
    OrOr,
    Bang,
    Dot,
}

impl Sym {
    pub fn text(self) -> &'static str {
        match self {
            Sym::LParen => "(",
            Sym::RParen => ")",
            Sym::LBrace => "{",
            Sym::RBrace => "}",
            Sym::LBracket => "[",
            Sym::RBracket => "]",
            Sym::Comma => ",",
            Sym::Colon => ":",
            Sym::Question => "?",
            Sym::Arrow => "=>",
            Sym::To => "->",
            Sym::Assign => "=",
            Sym::Eq => "==",
            Sym::Ne => "!=",
            Sym::Lt => "<",
            Sym::Le => "<=",
            Sym::Gt => ">",
            Sym::Ge => ">=",
            Sym::Plus => "+",
            Sym::Minus => "-",
            Sym::Star => "*",
            Sym::Slash => "/",
            Sym::Percent => "%",
            Sym::AndAnd => "&&",
            Sym::OrOr => "||",
            Sym::Bang => "!",
            Sym::Dot => ".",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Ident(String),
    Word(Keyword),
    Path(Vec<String>),
    Number(Number),
    Text(String),
    /// An interpolated string, flattened into the token stream: `TextOpen`, then the
    /// first hole's tokens, then a `TextPart` before each further hole, then
    /// `TextClose`. Flat, so the parser stays a flat recursive-descent one.
    TextOpen(String),
    TextPart(String),
    TextClose(String),
    Sym(Sym),
    End,
}

impl fmt::Display for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Token::Ident(name) => write!(f, "`{name}`"),
            Token::Word(keyword) => write!(f, "`{}`", keyword.text()),
            Token::Path(segments) => write!(f, "`@{}`", segments.join(".")),
            Token::Number(_) => f.write_str("a number"),
            Token::Text(_) => f.write_str("a string"),
            Token::TextOpen(_) | Token::TextPart(_) | Token::TextClose(_) => {
                f.write_str("an interpolated string")
            }
            Token::Sym(sym) => write!(f, "`{}`", sym.text()),
            Token::End => f.write_str("end of file"),
        }
    }
}

/// A token and the extent it covers. The extent is free here: `run` captures the start
/// before the scanner and pushes after it, and the cursor at that moment is already one
/// past the token's last character.
#[derive(Debug, Clone, PartialEq)]
pub struct Spanned {
    pub token: Token,
    pub span: Span,
}

pub fn lex(source: &str) -> Result<Vec<Spanned>, Diagnostic> {
    Lexer::new(source).run()
}

struct Lexer {
    chars: Vec<char>,
    pos: usize,
    line: u32,
    col: u32,
    /// Where the token being scanned began. The sub-scanners never see it otherwise, so
    /// without it an error inside one reports at the cursor, which by then is past the
    /// character it is about.
    start: Pos,
    /// One entry per open interpolation, holding the brace depth inside its hole.
    /// This stack is the whole of the nesting rule: a string literal inside a hole
    /// re-enters the scanner and pushes its own entry, so nothing special-cases it.
    interp: Vec<u32>,
}

impl Lexer {
    fn new(source: &str) -> Self {
        Self {
            chars: source.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
            start: Pos::new(1, 1),
            interp: Vec::new(),
        }
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let next = self.peek()?;
        self.pos += 1;
        if next == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(next)
    }

    /// Where the cursor is now, which after a scanner has run is one past the last
    /// character it consumed: the exclusive end of the token being built.
    fn at(&self) -> Pos {
        Pos::new(self.line, self.col)
    }

    /// From the start of the token to wherever the scanner gave up. `unterminated string`
    /// is about the quote that opened it rather than the end of the file it ran to.
    fn error<T>(&self, code: Code, message: impl Into<String>) -> Result<T, Diagnostic> {
        Err(Diagnostic::new(
            code,
            message,
            Span::new(self.start, self.at()),
        ))
    }

    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => {
                    self.bump();
                }
                Some('/') if self.peek_at(1) == Some('/') => {
                    while let Some(c) = self.peek() {
                        if c == '\n' {
                            break;
                        }
                        self.bump();
                    }
                }
                _ => return,
            }
        }
    }

    fn run(mut self) -> Result<Vec<Spanned>, Diagnostic> {
        let mut tokens = Vec::new();
        loop {
            self.skip_trivia();
            self.start = self.at();
            let start = self.start;
            let Some(next) = self.peek() else {
                tokens.push(Spanned {
                    token: Token::End,
                    span: Span::point(start),
                });
                return Ok(tokens);
            };

            let token = match next {
                c if c.is_ascii_digit() => self.number()?,
                c if is_ident_start(c) => self.word(),
                '"' if self.peek_at(1) == Some('"') && self.peek_at(2) == Some('"') => {
                    self.raw_text()?
                }
                '"' => self.text()?,
                '@' => self.path()?,
                _ => self.symbol()?,
            };
            // The cursor has not moved past the token yet: trivia is skipped at the top of
            // the next turn, so this is the token's own end and nothing else's.
            tokens.push(Spanned {
                token,
                span: Span::new(start, self.at()),
            });
        }
    }

    fn word(&mut self) -> Token {
        let mut name = String::new();
        while let Some(c) = self.peek() {
            if !is_ident_continue(c) {
                break;
            }
            name.push(c);
            self.bump();
        }
        match Keyword::lookup(&name) {
            Some(keyword) => Token::Word(keyword),
            None => Token::Ident(name),
        }
    }

    fn number(&mut self) -> Result<Token, Diagnostic> {
        let mut digits = String::new();
        while let Some(c) = self.peek() {
            if !c.is_ascii_digit() {
                break;
            }
            digits.push(c);
            self.bump();
        }

        let mut scale = 0u8;
        if self.peek() == Some('.') && self.peek_at(1).is_some_and(|c| c.is_ascii_digit()) {
            self.bump();
            while let Some(c) = self.peek() {
                if !c.is_ascii_digit() {
                    break;
                }
                digits.push(c);
                self.bump();
                scale = match scale.checked_add(1) {
                    Some(scale) => scale,
                    None => return self.error(Code::BadNumber, "too many decimal places"),
                };
            }
        }

        match digits.parse::<i128>() {
            Ok(value) => Ok(Token::Number(Number::new(value, scale))),
            Err(_) => self.error(Code::BadNumber, "number is too large"),
        }
    }

    fn text(&mut self) -> Result<Token, Diagnostic> {
        self.bump();
        let (value, open) = self.scan_text()?;
        if open {
            self.interp.push(1);
            return Ok(Token::TextOpen(value));
        }
        Ok(Token::Text(value))
    }

    /// Resumes a string after the `}` that closed a hole. Pops the interpolation when
    /// the string ends, so the stack holds exactly the holes still open.
    fn resume_text(&mut self) -> Result<Token, Diagnostic> {
        let (value, open) = self.scan_text()?;
        if open {
            return Ok(Token::TextPart(value));
        }
        self.interp.pop();
        Ok(Token::TextClose(value))
    }

    /// String content up to the closing quote or the next `{`. The flag says which of
    /// the two stopped it.
    fn scan_text(&mut self) -> Result<(String, bool), Diagnostic> {
        let mut value = String::new();
        loop {
            let Some(next) = self.bump() else {
                return self.error(Code::UnterminatedString, "unterminated string");
            };
            match next {
                '"' => return Ok((value, false)),
                '{' => return Ok((value, true)),
                '\\' => match self.bump() {
                    Some('n') => value.push('\n'),
                    Some('t') => value.push('\t'),
                    Some('"') => value.push('"'),
                    Some('\\') => value.push('\\'),
                    Some('{') => value.push('{'),
                    // Unnecessary, since a close brace in string content is never a
                    // delimiter. Accepted because an author who learns the open one
                    // will reach for its pair.
                    Some('}') => value.push('}'),
                    Some(other) => {
                        return self
                            .error(Code::UnknownEscape, format!("unknown escape `\\{other}`"));
                    }
                    None => return self.error(Code::UnterminatedString, "unterminated string"),
                },
                c => value.push(c),
            }
        }
    }

    /// `"""..."""`: everything between the delimiters, verbatim. No escapes and no
    /// interpolation, because the documents this form exists for are brace-dense.
    fn raw_text(&mut self) -> Result<Token, Diagnostic> {
        for _ in 0..3 {
            self.bump();
        }
        let mut value = String::new();
        loop {
            let Some(next) = self.peek() else {
                return self.error(Code::UnterminatedString, "unterminated multi-line string");
            };
            if next == '"' && self.peek_at(1) == Some('"') && self.peek_at(2) == Some('"') {
                for _ in 0..3 {
                    self.bump();
                }
                return Ok(Token::Text(value));
            }
            value.push(next);
            self.bump();
        }
    }

    fn path(&mut self) -> Result<Token, Diagnostic> {
        self.bump();
        let mut segments = Vec::new();
        loop {
            let Some(next) = self.peek() else {
                return self.error(Code::BadPath, "expected a name after `@`");
            };
            if !is_ident_start(next) {
                return self.error(Code::BadPath, "expected a name after `@`");
            }

            let mut segment = String::new();
            while let Some(c) = self.peek() {
                if !is_ident_continue(c) {
                    break;
                }
                segment.push(c);
                self.bump();
            }
            segments.push(segment);

            if self.peek() == Some('.') && self.peek_at(1).is_some_and(is_ident_start) {
                self.bump();
                continue;
            }
            return Ok(Token::Path(segments));
        }
    }

    fn symbol(&mut self) -> Result<Token, Diagnostic> {
        let first = self.bump().expect("symbol starts with a character");

        // Inside a hole, braces are counted, so a nested block or object literal is
        // not mistaken for the end of the interpolation.
        if let Some(depth) = self.interp.last_mut() {
            match first {
                '{' => *depth += 1,
                '}' if *depth == 1 => return self.resume_text(),
                '}' => *depth -= 1,
                _ => {}
            }
        }

        let second = self.peek();

        let paired = match (first, second) {
            ('=', Some('>')) => Some(Sym::Arrow),
            ('-', Some('>')) => Some(Sym::To),
            ('=', Some('=')) => Some(Sym::Eq),
            ('!', Some('=')) => Some(Sym::Ne),
            ('<', Some('=')) => Some(Sym::Le),
            ('>', Some('=')) => Some(Sym::Ge),
            ('&', Some('&')) => Some(Sym::AndAnd),
            ('|', Some('|')) => Some(Sym::OrOr),
            _ => None,
        };
        if let Some(sym) = paired {
            self.bump();
            return Ok(Token::Sym(sym));
        }

        let single = match first {
            '(' => Sym::LParen,
            ')' => Sym::RParen,
            '{' => Sym::LBrace,
            '}' => Sym::RBrace,
            '[' => Sym::LBracket,
            ']' => Sym::RBracket,
            ',' => Sym::Comma,
            ':' => Sym::Colon,
            '?' => Sym::Question,
            '=' => Sym::Assign,
            '<' => Sym::Lt,
            '>' => Sym::Gt,
            '+' => Sym::Plus,
            '-' => Sym::Minus,
            '*' => Sym::Star,
            '/' => Sym::Slash,
            '%' => Sym::Percent,
            '!' => Sym::Bang,
            '.' => Sym::Dot,
            other => {
                return self.error(
                    Code::UnexpectedCharacter,
                    format!("unexpected character `{other}`"),
                );
            }
        };
        Ok(Token::Sym(single))
    }
}

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident_continue(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}
