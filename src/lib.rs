pub mod build;
pub mod diagnostic;
pub mod harness;
pub mod host;
pub mod interp;
pub mod ir;
pub mod lex;
pub mod parse;
pub mod scaled;
pub mod testing;
pub mod types;
pub mod value;

pub use build::Builder;
pub use diagnostic::{Code, Diagnostic, Related, Severity};
pub use harness::{Harness, Journal, Reply};
pub use host::{
    AppendCondition, Attempt, Calls, Clock, Host, Http, Keys, Log, Predicate, Query, Recorded,
    Request, Rows,
};
pub use interp::{
    Counts, Effectful, Error, ErrorKind, Execution, Interpreter, Invocation, Outcome, Projection,
    Row, Store,
};
pub use ir::{
    Absent, Action, Arm, Assign, BinOp, Bind, Builtin, Command, ConstDef, Effect, EntityDef,
    EntityField, EnumDef, EnvBind, EnvField, EventDef, EventPath, Expr, ExprId, Exprs, FieldDef,
    Filter, Function, Handler, Ident, Index, Iter, Literal, Number, NumberError, Param, Pos,
    Program, Projector, RecordDef, RecordField, Return, Slice, SliceId, Slot, Span, StateVar, Stmt,
    Type, UnOp, Update,
};
pub use parse::{check_files, parse, parse_files};
pub use testing::{TestOutcome, TestResult, run_tests};
pub use value::{Event, Invoked, Json, Key, Record, Value};

/// A host parses a program once and serves from several threads, so the string a
/// `Literal` and a `Value` share is an `Arc` rather than an `Rc`. Nothing else would
/// fail if that stopped being true, so it is asserted here instead.
const _: () = {
    const fn shareable<T: Send + Sync>() {}
    shareable::<Program>();
    shareable::<Value>();
    shareable::<Event>();
    shareable::<Record>();
    shareable::<Harness>();
    shareable::<Diagnostic>();
};
