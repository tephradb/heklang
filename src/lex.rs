use std::error;
use std::fmt;

use crate::ir::Number;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keyword {
    As,
    Command,
    Delete,
    Effect,
    Else,
    Emit,
    Entity,
    Enum,
    Event,
    False,
    Fold,
    Guard,
    If,
    Invalid,
    Invoke,
    Let,
    None,
    On,
    Patch,
    Projector,
    Put,
    Reject,
    Return,
    State,
    True,
}

impl Keyword {
    pub fn lookup(word: &str) -> Option<Self> {
        Some(match word {
            "as" => Keyword::As,
            "command" => Keyword::Command,
            "delete" => Keyword::Delete,
            "effect" => Keyword::Effect,
            "else" => Keyword::Else,
            "emit" => Keyword::Emit,
            "entity" => Keyword::Entity,
            "enum" => Keyword::Enum,
            "event" => Keyword::Event,
            "false" => Keyword::False,
            "fold" => Keyword::Fold,
            "guard" => Keyword::Guard,
            "if" => Keyword::If,
            "invalid" => Keyword::Invalid,
            "invoke" => Keyword::Invoke,
            "let" => Keyword::Let,
            "none" => Keyword::None,
            "on" => Keyword::On,
            "patch" => Keyword::Patch,
            "projector" => Keyword::Projector,
            "put" => Keyword::Put,
            "reject" => Keyword::Reject,
            "return" => Keyword::Return,
            "state" => Keyword::State,
            "true" => Keyword::True,
            _ => return None,
        })
    }

    pub fn text(self) -> &'static str {
        match self {
            Keyword::As => "as",
            Keyword::Command => "command",
            Keyword::Delete => "delete",
            Keyword::Effect => "effect",
            Keyword::Else => "else",
            Keyword::Emit => "emit",
            Keyword::Entity => "entity",
            Keyword::Enum => "enum",
            Keyword::Event => "event",
            Keyword::False => "false",
            Keyword::Fold => "fold",
            Keyword::Guard => "guard",
            Keyword::If => "if",
            Keyword::Invalid => "invalid",
            Keyword::Invoke => "invoke",
            Keyword::Let => "let",
            Keyword::None => "none",
            Keyword::On => "on",
            Keyword::Patch => "patch",
            Keyword::Projector => "projector",
            Keyword::Put => "put",
            Keyword::Reject => "reject",
            Keyword::Return => "return",
            Keyword::State => "state",
            Keyword::True => "true",
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

#[derive(Debug, Clone, PartialEq)]
pub struct Spanned {
    pub token: Token,
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyntaxError {
    pub message: String,
    pub line: u32,
    pub col: u32,
    /// The module the error is in. `None` when the source had no name, which is what
    /// `parse` of a single string gives.
    pub file: Option<String>,
}

impl SyntaxError {
    pub fn new(message: impl Into<String>, line: u32, col: u32) -> Self {
        Self {
            message: message.into(),
            line,
            col,
            file: None,
        }
    }

    pub fn in_file(mut self, file: impl Into<String>) -> Self {
        self.file = Some(file.into());
        self
    }
}

impl fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.file {
            Some(file) => write!(f, "{file}:{}:{}: {}", self.line, self.col, self.message),
            None => write!(f, "{}:{}: {}", self.line, self.col, self.message),
        }
    }
}

impl error::Error for SyntaxError {}

pub fn lex(source: &str) -> Result<Vec<Spanned>, SyntaxError> {
    Lexer::new(source).run()
}

struct Lexer {
    chars: Vec<char>,
    pos: usize,
    line: u32,
    col: u32,
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

    fn error<T>(&self, message: impl Into<String>) -> Result<T, SyntaxError> {
        Err(SyntaxError::new(message, self.line, self.col))
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

    fn run(mut self) -> Result<Vec<Spanned>, SyntaxError> {
        let mut tokens = Vec::new();
        loop {
            self.skip_trivia();
            let line = self.line;
            let col = self.col;
            let Some(next) = self.peek() else {
                tokens.push(Spanned {
                    token: Token::End,
                    line,
                    col,
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
            tokens.push(Spanned { token, line, col });
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

    fn number(&mut self) -> Result<Token, SyntaxError> {
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
                    None => return self.error("too many decimal places"),
                };
            }
        }

        match digits.parse::<i128>() {
            Ok(value) => Ok(Token::Number(Number::new(value, scale))),
            Err(_) => self.error("number is too large"),
        }
    }

    fn text(&mut self) -> Result<Token, SyntaxError> {
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
    fn resume_text(&mut self) -> Result<Token, SyntaxError> {
        let (value, open) = self.scan_text()?;
        if open {
            return Ok(Token::TextPart(value));
        }
        self.interp.pop();
        Ok(Token::TextClose(value))
    }

    /// String content up to the closing quote or the next `{`. The flag says which of
    /// the two stopped it.
    fn scan_text(&mut self) -> Result<(String, bool), SyntaxError> {
        let mut value = String::new();
        loop {
            let Some(next) = self.bump() else {
                return self.error("unterminated string");
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
                    Some(other) => return self.error(format!("unknown escape `\\{other}`")),
                    None => return self.error("unterminated string"),
                },
                c => value.push(c),
            }
        }
    }

    /// `"""..."""`: everything between the delimiters, verbatim. No escapes and no
    /// interpolation, because the documents this form exists for are brace-dense.
    fn raw_text(&mut self) -> Result<Token, SyntaxError> {
        for _ in 0..3 {
            self.bump();
        }
        let mut value = String::new();
        loop {
            let Some(next) = self.peek() else {
                return self.error("unterminated multi-line string");
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

    fn path(&mut self) -> Result<Token, SyntaxError> {
        self.bump();
        let mut segments = Vec::new();
        loop {
            let Some(next) = self.peek() else {
                return self.error("expected a name after `@`");
            };
            if !is_ident_start(next) {
                return self.error("expected a name after `@`");
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

    fn symbol(&mut self) -> Result<Token, SyntaxError> {
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
            other => return self.error(format!("unexpected character `{other}`")),
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
