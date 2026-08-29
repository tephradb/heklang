//! What the checker says when something is wrong.
//!
//! `docs/diagnostics.md` is the contract: the closed set of codes, what each covers, and
//! where a span comes from. A diagnostic is data rather than a sentence, so a reader can
//! group by code, show a hint apart from the message, and follow a related location.

use std::error;
use std::fmt;

use crate::ir::Span;

/// How much a diagnostic means. A warning never travels in `Err`: it does not stop a
/// parse, so it is collected beside the errors rather than returned instead of a result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Severity {
    #[default]
    Error,
    Warning,
}

/// The kind of thing that is wrong. A readable slug rather than a number, so it says
/// something on its own and needs no registry to look up.
///
/// Closed on purpose: an enum makes the compiler check that every diagnostic is in the
/// taxonomy, which is what lets `docs/diagnostics.md` publish the whole set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Code {
    // Lexical. The scanner gave up inside a token.
    BadNumber,
    UnterminatedString,
    UnknownEscape,
    BadPath,
    UnexpectedCharacter,

    // Syntactic. A token the grammar cannot take here.
    ExpectedToken,

    // Naming.
    DeclaredTwice,
    NotDeclared,
    NotInScope,
    UnknownMember,
    UnknownType,

    // Types and values.
    TypeMismatch,
    BadOperands,
    BadLiteral,
    BadType,
    NeedsTargetType,
    NotAValue,

    // A set of named things: arguments, fields, parameters.
    Arity,
    MissingField,
    DuplicateField,

    // Annotations.
    UnknownAnnotation,
    BadAnnotation,

    // The shape of a declaration.
    EmptyDeclaration,
    EntityShape,
    EventShape,
    StateShape,
    NoZeroValue,

    // Where a statement may appear.
    WrongContext,
    ImpureFn,
    FoldRestriction,
    ArmOnly,
    ReturnShape,

    // The decrypt boundary.
    SealBoundary,
    EraseSubject,
    EraseOrder,

    // A test body.
    TestShape,

    // Statements about the whole program, checked after every pass.
    RecursiveFn,
    SelfTrigger,
    ConstCycle,
}

impl Code {
    /// Every code, so a test can prove each one is reachable. A decorative variant is
    /// a category the checker claims to have and does not.
    pub const ALL: &[Code] = &[
        Code::BadNumber,
        Code::UnterminatedString,
        Code::UnknownEscape,
        Code::BadPath,
        Code::UnexpectedCharacter,
        Code::ExpectedToken,
        Code::DeclaredTwice,
        Code::NotDeclared,
        Code::NotInScope,
        Code::UnknownMember,
        Code::UnknownType,
        Code::TypeMismatch,
        Code::BadOperands,
        Code::BadLiteral,
        Code::BadType,
        Code::NeedsTargetType,
        Code::NotAValue,
        Code::Arity,
        Code::MissingField,
        Code::DuplicateField,
        Code::UnknownAnnotation,
        Code::BadAnnotation,
        Code::EmptyDeclaration,
        Code::EntityShape,
        Code::EventShape,
        Code::StateShape,
        Code::NoZeroValue,
        Code::WrongContext,
        Code::ImpureFn,
        Code::FoldRestriction,
        Code::ArmOnly,
        Code::ReturnShape,
        Code::SealBoundary,
        Code::EraseSubject,
        Code::EraseOrder,
        Code::TestShape,
        Code::RecursiveFn,
        Code::SelfTrigger,
        Code::ConstCycle,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            Code::BadNumber => "bad-number",
            Code::UnterminatedString => "unterminated-string",
            Code::UnknownEscape => "unknown-escape",
            Code::BadPath => "bad-path",
            Code::UnexpectedCharacter => "unexpected-character",
            Code::ExpectedToken => "expected-token",
            Code::DeclaredTwice => "declared-twice",
            Code::NotDeclared => "not-declared",
            Code::NotInScope => "not-in-scope",
            Code::UnknownMember => "unknown-member",
            Code::UnknownType => "unknown-type",
            Code::TypeMismatch => "type-mismatch",
            Code::BadOperands => "bad-operands",
            Code::BadLiteral => "bad-literal",
            Code::BadType => "bad-type",
            Code::NeedsTargetType => "needs-target-type",
            Code::NotAValue => "not-a-value",
            Code::Arity => "arity",
            Code::MissingField => "missing-field",
            Code::DuplicateField => "duplicate-field",
            Code::UnknownAnnotation => "unknown-annotation",
            Code::BadAnnotation => "bad-annotation",
            Code::EmptyDeclaration => "empty-declaration",
            Code::EntityShape => "entity-shape",
            Code::EventShape => "event-shape",
            Code::StateShape => "state-shape",
            Code::NoZeroValue => "no-zero-value",
            Code::WrongContext => "wrong-context",
            Code::ImpureFn => "impure-fn",
            Code::FoldRestriction => "fold-restriction",
            Code::ArmOnly => "arm-only",
            Code::ReturnShape => "return-shape",
            Code::SealBoundary => "seal-boundary",
            Code::EraseSubject => "erase-subject",
            Code::EraseOrder => "erase-order",
            Code::TestShape => "test-shape",
            Code::RecursiveFn => "recursive-fn",
            Code::SelfTrigger => "self-trigger",
            Code::ConstCycle => "const-cycle",
        }
    }

    /// Whether the token stream stopped making sense. A syntax error abandons the
    /// declaration it was found in, because there is nothing left to read; everything
    /// else parsed, and the parser can carry on past it.
    pub fn is_syntax(self) -> bool {
        matches!(
            self,
            Code::BadNumber
                | Code::UnterminatedString
                | Code::UnknownEscape
                | Code::BadPath
                | Code::UnexpectedCharacter
                | Code::ExpectedToken
        )
    }
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.slug())
    }
}

/// Somewhere else worth looking: the first of two declarations, a link in a cycle, the
/// `erase` a `reveal` can run after.
///
/// `file` is carried rather than assumed, because a second declaration is often in
/// another module and a reader that cannot open it has been told nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Related {
    pub span: Span,
    pub file: Option<String>,
    pub message: String,
}

impl Related {
    pub fn new(message: impl Into<String>, span: Span) -> Self {
        Self {
            span,
            file: None,
            message: message.into(),
        }
    }

    pub fn in_file(mut self, file: impl Into<String>) -> Self {
        self.file = Some(file.into());
        self
    }
}

/// One thing wrong with a program.
///
/// The `message` says what is wrong and the `hint` says what to do about it. They are
/// separate so a reader can show one without the other, and so the hints that describe a
/// concrete edit can become an offer rather than prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: Code,
    pub severity: Severity,
    pub span: Span,
    /// The module the diagnostic is in. `None` when the source had no name, which is
    /// what `parse` of a single string gives.
    pub file: Option<String>,
    pub message: String,
    pub hint: Option<String>,
    pub related: Vec<Related>,
}

impl Diagnostic {
    pub fn new(code: Code, message: impl Into<String>, span: Span) -> Self {
        Self {
            code,
            severity: Severity::Error,
            span,
            file: None,
            message: message.into(),
            hint: None,
            related: Vec::new(),
        }
    }

    pub fn in_file(mut self, file: impl Into<String>) -> Self {
        self.file = Some(file.into());
        self
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    pub fn with_related(mut self, related: Related) -> Self {
        self.related.push(related);
        self
    }

    /// The message and the hint as one sentence, joined the way they read when they were
    /// one. What a reader with a single line to give gets.
    pub fn text(&self) -> String {
        match &self.hint {
            Some(hint) => format!("{}; {hint}", self.message),
            None => self.message.clone(),
        }
    }
}

/// The start alone, through `Span`'s own `Display`. The extent is for a reader that can
/// draw it; `docs/cli.md` has what `hek` does with it.
impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(file) = &self.file {
            write!(f, "{file}:")?;
        }
        match self.severity {
            Severity::Error => write!(f, "{} [{}] ", self.span, self.code)?,
            Severity::Warning => write!(f, "{} [warning: {}] ", self.span, self.code)?,
        }
        f.write_str(&self.text())
    }
}

impl error::Error for Diagnostic {}
