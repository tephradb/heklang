//! The vocabulary an interpreter and a host share.
//!
//! Nothing here holds interpreter state, so a host implementor reads one file.

use crate::ir::{EventPath, Ident};
use crate::value::Value;

/// One slice of the log with its filters resolved to values.
///
/// A `state` declares a slice, and what a command folded is what it conflicts on, so
/// the query a host reads with and the condition it appends against are one shape.
/// `docs/commands.md` has the argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Predicate {
    pub event: EventPath,
    /// Sorted by field name, so one slice is one predicate however it was written:
    /// `(shop_id, topic)` and `(topic, shop_id)` narrow the same set of events and
    /// have no business comparing unequal.
    pub filters: Vec<(Ident, Value)>,
}

impl Predicate {
    pub fn new(event: EventPath, mut filters: Vec<(Ident, Value)>) -> Self {
        filters.sort_by(|(one, _), (other, _)| one.cmp(other));
        Self { event, filters }
    }
}

/// What a run read, and therefore what it can be beaten to.
///
/// Returned with every outcome rather than only a commit: a refusal still read the log,
/// and a host that wants to cache or trace the decision needs to know what it depended
/// on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppendCondition {
    /// The head the run folded against. Another writer landing in one of `slices` after
    /// this position is what makes the append stale.
    pub after: u64,
    pub slices: Vec<Predicate>,
}
