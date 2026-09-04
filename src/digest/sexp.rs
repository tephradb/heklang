//! S-expressions: the packed form, and the two views taken from it.
//!
//! The digest's canonical artifact is text, but it is a **wire format** rather than a
//! rendering: one line per declaration, single spaces, nothing anyone reformats for
//! taste. That is what lets [`super::Digest`] hash it and still leave
//! [`Sexp::expanded`] and [`Sexp::json`] free to change, which is the whole point of the
//! split (`docs/digest.md` rule 2).
//!
//! Generic on purpose. A typed node per heklang construct would have to be kept in step
//! with the IR twice over, and hekla's questions ("did this event lose a field?") are
//! answered by walking a list. `docs/digest.md` has the head table.

use std::error;
use std::fmt;
use std::fmt::Write as _;

use crate::value::Json;

/// Where the expanded view stops putting a list on one line. Not a contract: nothing
/// hashes the expansion, so this may move whenever it reads better.
const WIDTH: usize = 96;

/// One node of the packed form.
///
/// `Atom` and `Str` are both leaves and are kept apart because they are not the same
/// thing: `Int` the type name and `"Int"` the string would otherwise pack to bytes that
/// no longer say which was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sexp {
    Atom(String),
    Str(String),
    List(Vec<Sexp>),
}

impl Sexp {
    pub fn atom(text: impl Into<String>) -> Self {
        Sexp::Atom(text.into())
    }

    pub fn text(value: impl Into<String>) -> Self {
        Sexp::Str(value.into())
    }

    pub fn list(items: impl IntoIterator<Item = Sexp>) -> Self {
        Sexp::List(items.into_iter().collect())
    }

    /// A list built from a head and the rest, which is nearly every list here.
    pub fn of(head: &str, rest: impl IntoIterator<Item = Sexp>) -> Self {
        let mut items = vec![Sexp::atom(head)];
        items.extend(rest);
        Sexp::List(items)
    }

    /// The head of a list, which is what a caller matches on. `None` for a leaf, and for
    /// a list whose first element is not an atom, which nothing here builds.
    pub fn head(&self) -> Option<&str> {
        match self {
            Sexp::List(items) => match items.first() {
                Some(Sexp::Atom(head)) => Some(head),
                _ => None,
            },
            _ => None,
        }
    }

    /// Everything after the head.
    pub fn rest(&self) -> &[Sexp] {
        match self {
            Sexp::List(items) if !items.is_empty() => &items[1..],
            _ => &[],
        }
    }

    /// The children that are lists headed by `head`, which is how a keyword section is
    /// read back: `(command Name (params ..) (stage ..) (stage ..))`.
    pub fn section(&self, head: &str) -> impl Iterator<Item = &Sexp> {
        self.rest()
            .iter()
            .filter(move |item| item.head() == Some(head))
    }

    /// The canonical bytes: one line, one space between tokens, nothing optional.
    pub fn packed(&self) -> String {
        let mut out = String::new();
        self.pack(&mut out);
        out
    }

    fn pack(&self, out: &mut String) {
        match self {
            Sexp::Atom(text) => out.push_str(text),
            Sexp::Str(value) => quote(value, out),
            Sexp::List(items) => {
                out.push('(');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(' ');
                    }
                    item.pack(out);
                }
                out.push(')');
            }
        }
    }

    /// The readable view: the same tree over several lines, so a diff points at the part
    /// that changed rather than at one long line. Nothing hashes this.
    pub fn expanded(&self) -> String {
        let mut lines = Vec::new();
        self.render(0, &mut lines);
        lines.join("\n")
    }

    fn render(&self, depth: usize, lines: &mut Vec<String>) {
        let flat = self.packed();
        let pad = "  ".repeat(depth);
        // A list that fits stays on one line, head and all, which is what keeps a short
        // statement from becoming five lines of punctuation.
        if pad.len() + flat.len() <= WIDTH {
            lines.push(format!("{pad}{flat}"));
            return;
        }
        let Sexp::List(items) = self else {
            lines.push(format!("{pad}{flat}"));
            return;
        };
        // The head keeps the leading leaves for company: a name is what the head is
        // about, and `(command` on a line of its own with `Place` under it reads worse
        // than `(command Place`.
        let mut opened = String::from("(");
        let mut rest = items.as_slice();
        while let Some(item) = rest.first() {
            if opened.len() > 1 && matches!(item, Sexp::List(_)) {
                break;
            }
            if opened.len() > 1 {
                opened.push(' ');
            }
            opened.push_str(&item.packed());
            rest = &rest[1..];
        }
        lines.push(format!("{pad}{opened}"));
        for item in rest {
            item.render(depth + 1, lines);
        }
        // The closing paren rides the last line rather than taking one of its own: a
        // column of them says nothing and makes every nested block cost a line to end.
        if let Some(last) = lines.last_mut() {
            last.push(')');
        }
    }

    /// The structural view, for a consumer that would rather not walk a list.
    ///
    /// A list becomes `{"kind": head, ..}`. A child that is itself a list headed by a
    /// **keyword** becomes a key of its own, so a declaration reads as named fields;
    /// everything else is a value and lands in `args`. That is one small set to keep
    /// rather than a field table per head. Nothing hashes this either.
    pub fn json(&self) -> Json {
        match self {
            // A number stays a number, so a consumer does not have to parse `"5"`.
            Sexp::Atom(text) if is_number(text) => Json::Num(text.clone()),
            Sexp::Atom(text) | Sexp::Str(text) => Json::str(text.clone()),
            Sexp::List(items) => {
                let Some(Sexp::Atom(head)) = items.first() else {
                    return Json::Arr(items.iter().map(Sexp::json).collect());
                };
                let mut fields = std::collections::BTreeMap::new();
                fields.insert("kind".to_string(), Json::str(head.clone()));
                let mut args: Vec<Json> = Vec::new();
                for item in &items[1..] {
                    match item.head() {
                        Some(key) if KEYWORDS.contains(&key) => {
                            let entry = fields
                                .entry(key.to_string())
                                .or_insert_with(|| Json::Arr(Vec::new()));
                            if let Json::Arr(list) = entry {
                                list.push(item.json());
                            }
                        }
                        _ => args.push(item.json()),
                    }
                }
                if !args.is_empty() {
                    fields.insert("args".to_string(), Json::Arr(args));
                }
                Json::Obj(fields)
            }
        }
    }

    /// Reads one packed s-expression back. The other half of the round trip: hekla stores
    /// a line and later expands it with no source tree in reach.
    pub fn parse(text: &str) -> Result<Sexp, SexpError> {
        let mut reader = Reader {
            chars: text.chars().collect(),
            at: 0,
        };
        reader.space();
        let sexp = reader.value()?;
        reader.space();
        if reader.at < reader.chars.len() {
            return Err(reader.err("one s-expression per line, and this line has more"));
        }
        Ok(sexp)
    }
}

impl fmt::Display for Sexp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.packed())
    }
}

/// The heads that name a part of their parent rather than a value in it. Everything not
/// here is a value, which is what keeps `(+ (int 1) (int 2))` from reading its operands
/// as fields.
const KEYWORDS: &[&str] = &[
    "acc", "bind", "body", "col", "default", "do", "else", "entity", "env", "events", "expect",
    "f", "filter", "fold", "given", "index", "in", "item", "key", "load", "max", "now", "of", "on",
    "p", "params", "post", "pre", "rejects", "respond", "returns", "slice", "stage", "status",
    "then", "variants", "when", "yield",
];

fn is_number(text: &str) -> bool {
    let digits = text.strip_prefix('-').unwrap_or(text);
    !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
}

/// One escaping, so a value carrying a quote or a newline cannot make two different
/// declarations pack to the same bytes.
fn quote(value: &str, out: &mut String) {
    out.push('"');
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

struct Reader {
    chars: Vec<char>,
    at: usize,
}

impl Reader {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.at).copied()
    }

    fn space(&mut self) {
        while matches!(self.peek(), Some(c) if c.is_whitespace()) {
            self.at += 1;
        }
    }

    fn err(&self, message: &str) -> SexpError {
        SexpError {
            at: self.at,
            message: message.to_string(),
        }
    }

    fn value(&mut self) -> Result<Sexp, SexpError> {
        match self.peek() {
            None => Err(self.err("a value, and the line ended")),
            Some('(') => self.list(),
            Some(')') => Err(self.err("`)` with no list open")),
            Some('"') => self.string(),
            Some(_) => self.atom(),
        }
    }

    fn list(&mut self) -> Result<Sexp, SexpError> {
        self.at += 1;
        let mut items = Vec::new();
        loop {
            self.space();
            match self.peek() {
                None => return Err(self.err("a list was opened and never closed")),
                Some(')') => {
                    self.at += 1;
                    return Ok(Sexp::List(items));
                }
                Some(_) => items.push(self.value()?),
            }
        }
    }

    fn atom(&mut self) -> Result<Sexp, SexpError> {
        let start = self.at;
        while matches!(self.peek(), Some(c) if !c.is_whitespace() && c != '(' && c != ')' && c != '"')
        {
            self.at += 1;
        }
        if self.at == start {
            return Err(self.err("an empty token"));
        }
        Ok(Sexp::Atom(self.chars[start..self.at].iter().collect()))
    }

    fn string(&mut self) -> Result<Sexp, SexpError> {
        self.at += 1;
        let mut value = String::new();
        loop {
            match self.peek() {
                None => return Err(self.err("a string was opened and never closed")),
                Some('"') => {
                    self.at += 1;
                    return Ok(Sexp::Str(value));
                }
                Some('\\') => {
                    self.at += 1;
                    let escape = self.peek().ok_or_else(|| self.err("a trailing `\\`"))?;
                    self.at += 1;
                    value.push(match escape {
                        '"' => '"',
                        '\\' => '\\',
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        'u' => {
                            let digits: String = self.chars.iter().skip(self.at).take(4).collect();
                            let code = u32::from_str_radix(&digits, 16)
                                .map_err(|_| self.err("`\\u` wants four hex digits"))?;
                            self.at += 4;
                            char::from_u32(code).ok_or_else(|| self.err("not a character"))?
                        }
                        other => return Err(self.err(&format!("`\\{other}` is not an escape"))),
                    });
                }
                Some(c) => {
                    self.at += 1;
                    value.push(c);
                }
            }
        }
    }
}

/// A packed form that could not be read. Loud rather than silent: a stored line that has
/// been truncated or half-migrated must not decode into a plausible wrong answer, because
/// what reads it next is deciding whether a deployment is a breaking change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SexpError {
    /// The character the reader stopped at.
    pub at: usize,
    pub message: String,
}

impl fmt::Display for SexpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: expected {}", self.at, self.message)
    }
}

impl error::Error for SexpError {}
