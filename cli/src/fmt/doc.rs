//! The document algebra: everything the formatter decides before it knows any hek.
//!
//! This is Wadler's pretty printer in the shape Prettier gave it. A document is built
//! without committing to a layout, and one rule resolves it: **a group renders flat if it
//! fits on the rest of the line, and broken if it does not, and breaking a group breaks
//! every line directly inside it.** Nesting falls out, so `put Order { a, b }` on one line
//! and the same statement one field per line are the same document rather than two branches
//! of a printer.
//!
//! `Text` borrows. Every string a formatter emits is either a slice of the source or a
//! literal in this crate, so the lifetime is not a tax: it is the rule that a formatter may
//! not invent text, made unwritable rather than merely intended.

use std::fmt::Write as _;

/// Two spaces, which is what all 83 `.hk` files in existence use and what
/// `tree-sitter-hek/queries/indents.scm` tells an editor.
const INDENT: usize = 2;

/// Whether a group settled on one line or several. A `Line` reads its own mode, which is
/// how one document renders two ways.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Flat,
    Break,
}

/// An unresolved layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Doc<'a> {
    /// Verbatim. Never contains a newline: a construct that must break says so with
    /// `Hardline` instead, so that the width algorithm can see it.
    Text(&'a str),
    /// Source reproduced exactly, newlines and all, and never re-indented. A `"""` body
    /// holds someone else's language at whatever column they wrote it (fifteen of them
    /// hold GraphQL starting at column 0), so the one safe thing to do with it is nothing.
    /// A multi-line one breaks every group around it, because it has already left the line.
    Verbatim(&'a str),
    /// Nothing at all, except that every group containing one is broken. What a trailing
    /// comment needs: the comment itself is held back to the end of the line, so it cannot
    /// use a `Hardline` to say that the line is now spoken for.
    BreakParent,
    /// A space when flat, a newline when broken. The separator between list items.
    Line,
    /// Nothing when flat, a newline when broken. What sits just inside a delimiter, so
    /// `{ a }` has its spaces from `Line` and `(a)` has none from `Softline`.
    Softline,
    /// Always a newline, and every group containing one is broken before it is measured.
    /// A statement separator, and what follows a comment.
    Hardline,
    /// Emitted only in the mode named. `if_break(",", "")` is the trailing comma: present
    /// when the list broke, absent when it did not.
    IfBreak {
        broken: Box<Doc<'a>>,
        flat: Box<Doc<'a>>,
    },
    /// Held back until just before the next newline. A comment written after code on the
    /// same line is the only thing that needs it: without it the comment would land on the
    /// line below and quietly describe the wrong statement.
    LineSuffix(&'a str),
    Concat(Vec<Doc<'a>>),
    /// Two more spaces on every line inside.
    Indent(Box<Doc<'a>>),
    Group {
        doc: Box<Doc<'a>>,
        /// Set when the contents hold a `Hardline` anywhere, including inside a nested
        /// group. Such a group can never be flat, so it is not measured.
        must_break: bool,
    },
}

impl<'a> Doc<'a> {
    pub fn text(text: &'a str) -> Self {
        debug_assert!(
            !text.contains('\n'),
            "a newline in `Text` is invisible to `fits`"
        );
        Doc::Text(text)
    }

    /// Nothing at all. The identity for `concat`, and what the flat side of an `if_break`
    /// usually is.
    pub fn nil() -> Self {
        Doc::Concat(Vec::new())
    }

    pub fn concat(parts: impl IntoIterator<Item = Doc<'a>>) -> Self {
        Doc::Concat(parts.into_iter().collect())
    }

    /// `parts` separated by `sep`, which is the shape of every list in the language.
    pub fn join(sep: Doc<'a>, parts: impl IntoIterator<Item = Doc<'a>>) -> Self {
        let mut out = Vec::new();
        for part in parts {
            if !out.is_empty() {
                out.push(sep.clone());
            }
            out.push(part);
        }
        Doc::Concat(out)
    }

    pub fn indent(doc: Doc<'a>) -> Self {
        Doc::Indent(Box::new(doc))
    }

    pub fn if_break(broken: Doc<'a>, flat: Doc<'a>) -> Self {
        Doc::IfBreak {
            broken: Box::new(broken),
            flat: Box::new(flat),
        }
    }

    /// A layout decision. Whether it can be flat at all is settled here rather than at
    /// render time, because a `Hardline` deep inside has to reach every enclosing group and
    /// walking back up is not something the renderer's stack can do.
    pub fn group(doc: Doc<'a>) -> Self {
        let must_break = doc.contains_hardline();
        Doc::Group {
            doc: Box::new(doc),
            must_break,
        }
    }

    /// Source reproduced exactly. Use it for anything that may span lines; `text` is for
    /// everything else, and asserts that it does not.
    pub fn verbatim(text: &'a str) -> Self {
        Doc::Verbatim(text)
    }

    fn contains_hardline(&self) -> bool {
        match self {
            Doc::Hardline | Doc::BreakParent => true,
            Doc::Verbatim(text) => text.contains('\n'),
            Doc::Text(_) | Doc::Line | Doc::Softline | Doc::LineSuffix(_) => false,
            Doc::IfBreak { broken, flat } => broken.contains_hardline() || flat.contains_hardline(),
            Doc::Concat(parts) => parts.iter().any(Doc::contains_hardline),
            Doc::Indent(inner) => inner.contains_hardline(),
            // Through a nested group too: a hard break propagates all the way out, which is
            // what stops an outer group claiming to fit on a line its contents will leave.
            Doc::Group { doc, .. } => doc.contains_hardline(),
        }
    }
}

/// Lay a document out at `width` columns.
///
/// The output never has trailing whitespace and never has a line wider than `width` except
/// where nothing could have been broken: a long string literal, or a construct the printer
/// declared flat on purpose.
pub fn render(doc: &Doc<'_>, width: usize) -> String {
    let mut out = String::new();
    let mut column = 0;
    let mut suffixes: Vec<&str> = Vec::new();
    let mut stack = vec![(0usize, Mode::Break, doc)];

    while let Some((indent, mode, doc)) = stack.pop() {
        match doc {
            Doc::Text(text) => {
                out.push_str(text);
                column += columns(text);
            }
            Doc::Verbatim(text) => {
                out.push_str(text);
                // A line the source ended is a line this is not still on, so the column
                // restarts from whatever followed the last newline rather than accumulating.
                column = match text.rsplit_once('\n') {
                    Some((_, tail)) => columns(tail),
                    None => column + columns(text),
                };
            }
            Doc::BreakParent => {}
            Doc::Concat(parts) => {
                stack.extend(parts.iter().rev().map(|part| (indent, mode, part)));
            }
            Doc::Indent(inner) => stack.push((indent + INDENT, mode, inner.as_ref())),
            Doc::Group { doc, must_break } => {
                let flat = !must_break && fits(width.saturating_sub(column), doc, &stack);
                let mode = if flat { Mode::Flat } else { Mode::Break };
                stack.push((indent, mode, doc.as_ref()));
            }
            Doc::IfBreak { broken, flat } => {
                let taken = if mode == Mode::Break { broken } else { flat };
                stack.push((indent, mode, taken.as_ref()));
            }
            Doc::LineSuffix(text) => suffixes.push(text),
            Doc::Line if mode == Mode::Flat => {
                out.push(' ');
                column += 1;
            }
            Doc::Softline if mode == Mode::Flat => {}
            // Every remaining case ends the line: a `Line` or `Softline` in break mode, and
            // a `Hardline` in either, since a group holding one is never flat.
            Doc::Line | Doc::Softline | Doc::Hardline => {
                for suffix in suffixes.drain(..) {
                    out.push_str(suffix);
                }
                truncate_trailing_spaces(&mut out);
                out.push('\n');
                let _ = write!(out, "{:indent$}", "");
                column = indent;
            }
        }
    }

    for suffix in suffixes {
        out.push_str(suffix);
    }
    truncate_trailing_spaces(&mut out);
    out
}

/// Whether `next` laid out flat, followed by whatever is already queued, reaches a newline
/// before it runs out of room.
///
/// **This measures the document and never the source it came from.** Idempotence depends on
/// it: a list written across lines carries a trailing comma in its source, and measuring
/// those bytes would make a 91-column group break on the first pass and fit at 90 on the
/// second. The flat rendering has no trailing comma in it, so both passes decide alike.
fn fits(mut remaining: usize, next: &Doc<'_>, queued: &[(usize, Mode, &Doc<'_>)]) -> bool {
    let mut work = vec![(Mode::Flat, next)];
    // `queued` is the render stack, so its last entry is the next one to be processed.
    let mut ahead = queued.len();

    loop {
        let (mode, doc) = match work.pop() {
            Some(item) => item,
            None => match ahead.checked_sub(1) {
                // Nothing left to lay out, so what there was fitted.
                None => return true,
                Some(next) => {
                    ahead = next;
                    let (_, mode, doc) = queued[next];
                    (mode, doc)
                }
            },
        };

        match doc {
            Doc::Text(text) => {
                let taken = columns(text);
                if taken > remaining {
                    return false;
                }
                remaining -= taken;
            }
            Doc::Verbatim(text) => match text.split_once('\n') {
                // It ends the line itself, so whatever came before it fitted.
                Some(_) => return true,
                None => {
                    let taken = columns(text);
                    if taken > remaining {
                        return false;
                    }
                    remaining -= taken;
                }
            },
            Doc::BreakParent => {}
            Doc::Concat(parts) => work.extend(parts.iter().rev().map(|part| (mode, part))),
            Doc::Indent(inner) => work.push((mode, inner.as_ref())),
            Doc::Group { doc, must_break } => {
                let mode = if *must_break { Mode::Break } else { mode };
                work.push((mode, doc.as_ref()));
            }
            Doc::IfBreak { broken, flat } => {
                let taken = if mode == Mode::Break { broken } else { flat };
                work.push((mode, taken.as_ref()));
            }
            // A suffix is emitted before the newline that ends the line, so it costs
            // nothing to whether the line reaches one.
            Doc::LineSuffix(_) => {}
            Doc::Line if mode == Mode::Flat => {
                if remaining == 0 {
                    return false;
                }
                remaining -= 1;
            }
            Doc::Softline if mode == Mode::Flat => {}
            // A newline: the line ended without running out, so it fits.
            Doc::Line | Doc::Softline | Doc::Hardline => return true,
        }
    }
}

/// How wide `text` prints. Characters rather than bytes, because a string literal may hold
/// any of them and a byte count would make an accented word look long.
fn columns(text: &str) -> usize {
    text.chars().count()
}

/// Indentation written for a line that turned out to hold nothing, and the space a flat
/// `Line` emitted before a group above it decided to break.
fn truncate_trailing_spaces(out: &mut String) {
    out.truncate(out.trim_end_matches(' ').len());
}
