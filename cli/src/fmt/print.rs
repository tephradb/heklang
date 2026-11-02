//! A tree-sitter tree, lowered into a [`Doc`].
//!
//! Every function here iterates a node's children and switches on `kind()`. It never asks
//! the grammar for a field in order to *find* a child, only to tell one it already has from
//! its siblings. That is not a style preference: a comment is an `extra`, so it can land
//! between any two children of any node, and a printer that fetched `left`, `operator` and
//! `right` by name would drop every comment written between them. Iterating is also why the
//! grammar's missing fields (`unary_expression` has none) cost nothing.
//!
//! The one node kind never descended into is a string. `interpolation` parses its hole in
//! ordinary token context, so `extras` are grammatically live in there; walking a string
//! while also emitting it as a verbatim slice would print a comment twice and the second
//! copy would eat the closing quote.

use tree_sitter::Node;

use super::doc::Doc;

/// Where a line wraps.
///
/// Measured rather than chosen: code sits at p95 = 87 columns across the 83 `.hk` files in
/// existence, hand-wrapped comments have a sharp cliff at 87-88, and command signatures
/// break in an 88-97 band. One number covers all three.
///
/// Not to be confused with the narrower width `main.rs` wraps a diagnostic note at. A note
/// is prose sitting beside the source it is about, and it reads better narrower than the
/// code; this is the code.
pub const WIDTH: usize = 90;

pub struct Printer<'a> {
    source: &'a str,
}

impl<'a> Printer<'a> {
    pub fn new(source: &'a str) -> Self {
        Self { source }
    }

    /// The whole module.
    pub fn file(&self, root: Node<'a>) -> Doc<'a> {
        self.sequence(&self.kids(root))
    }

    // ------------------------------------------------------------------ dispatch

    fn node(&self, node: Node<'a>) -> Doc<'a> {
        match node.kind() {
            // Reproduced exactly. A literal, a name, and a path are what they are.
            "identifier" | "type_identifier" | "index_keyword" | "event_path"
            | "annotation_name" | "primitive_type" | "boolean_literal" | "none_literal"
            | "integer_literal" | "decimal_literal" | "comment" => Doc::text(self.text(node)),
            // A `"""` body holds another language at its own indentation, and a plain
            // string's content is byte-exact by construction (`token.immediate`).
            "string" | "raw_string" => Doc::verbatim(self.text(node)),

            // ------------------------------------------------------- declarations
            "enum_declaration" => {
                self.headed("enum", node, |this, kids| this.list("{", "}", true, kids))
            }
            "record_declaration" => self.headed("record", node, Self::fields),
            "event_declaration" => self.headed("event", node, Self::fields),
            "entity_declaration" => self.headed("entity", node, Self::fields),
            "projector_declaration" => self.headed("projector", node, Self::body),
            "effect_declaration" => self.headed("effect", node, Self::body),
            "enum_variant" => self.joined(node, " "),
            "record_field" | "event_field" | "entity_field" => self.field_decl(node),
            "index_clause" => self.index_clause(node),
            "const_declaration" => self.const_decl(node),
            "secret_declaration" => self.secret_decl(node),
            "subject_declaration" => self.subject_decl(node),
            "secret_clause" => self.secret_clause(node),
            "refusal_declaration" => self.refusal(node),
            "function_declaration" => self.function(node),
            "command_declaration" => self.callable("command", node),
            "guard_definition" => self.callable("guard", node),
            "fold_definition" => self.fold_definition(node),
            "fold_body" => self.fold_body(node),
            "fold_type" => self.fold_type(node),
            "parameters" | "arguments" | "annotation_arguments" => {
                self.list("(", ")", false, &self.kids(node))
            }
            "parameter" => self.typed(node),
            "annotation" => self.joined(node, ""),
            "event_handler" => self.handler(node),
            "destructure" => self.list("{", "}", true, &self.kids(node)),
            // `@key shop_id`, one space and never a break between them: the marker means
            // nothing apart from the field it is on. The same shape `enum_variant` prints
            // `@default Free` as.
            "keyed_field" => self.joined(node, " "),

            // ------------------------------------------------------------- tests
            "test_declaration" => self.keyed("test", node),
            "test_body" => self.body(&self.kids(node)),
            "given_clause" => self.keyed("given", node),
            "run_clause" => self.keyed("run", node),
            "project_clause" => self.keyed("project", node),
            "deliver_clause" => self.keyed("deliver", node),
            // `key month = ..`. A comma list with no delimiter of its own, so it takes no
            // trailing comma for the reason an `on` header's path list does not, and each
            // part is a node so that the `=` survives: `kids` is the named children and
            // every piece of punctuation in this grammar is anonymous.
            "computed_keys" => self.computed_keys(node),
            "computed_key" => self.computed_key(node),
            "fold_clause" => self.keyed("fold", node),
            // One word and no children: the action with nothing to name.
            "fill_clause" => Doc::text("fill"),
            "erased_clause" => self.keyed("erased", node),
            "respond_clause" => self.respond(node),
            "expect_clause" => self.expect(node),
            "event_expectation" | "fixture_expectation" => self.spaced(node),
            "row_expectation" => self.row(node),
            // `[a, b]`, which breaks like every other comma list and keeps a comment
            // among the keys where it was written.
            "subscript" => self.list("[", "]", false, &self.kids(node)),

            // -------------------------------------------------------- statements
            "block" => self.body(&self.kids(node)),
            "guard_declaration" => self.guard(node),
            "fold_declaration" => self.fold(node),
            "fold_arm" => self.fold_arm(node),
            "slice_reference" | "guard_slice" => self.slice(node),
            "filter" => self.filter(node),
            "field_initializer" => self.optional_value(node),
            "let_statement" => self.assignment("let", node),
            "if_statement" => self.if_statement(node),
            "for_statement" => self.keyed("for", node),
            "iter_bindings" => self.iter_bindings(node),
            "return_statement" => self.returned(node),
            "outcome_expression" => self.keyed_value("invalid", node),
            "refusal_expression" => self.keyed_value("reject", node),
            "fail_statement" => self.keyed_value("fail", node),
            "emit_statement" => self.keyed("emit", node),
            // The same shape an `emit` writes, one keyword shorter.
            "event_literal" => self.spaced(node),
            "put_statement" => self.keyed("put", node),
            "patch_statement" => self.keyed_row(node, true),
            "delete_statement" => self.keyed_row(node, false),
            "expression_statement" => self.joined(node, ""),

            // -------------------------------------------------------------- types
            "type" => self.optional_type(node),
            "scaled_type" | "list_type" => self.applied(node),
            "map_type" => self.map_type(node),

            // -------------------------------------------------------- expressions
            "parenthesized_expression" => self.parens(node),
            "unary_expression" => self.prefixed(node),
            "binary_expression" => self.binary(node),
            "call_expression" => self.joined(node, ""),
            "method_call" => self.method_call(node),
            "field_expression" | "stored_field" => self.dotted(node),
            "named_argument" => self.joined(node, " = "),
            "record_literal" => self.spaced(node),
            "invoke_expression" => self.keyed_value("invoke", node),
            "field_initializer_list" | "object_literal" => {
                self.list("{", "}", true, &self.kids(node))
            }
            "object_entry" => self.joined(node, ": "),
            "list" => self.list("[", "]", false, &self.kids(node)),
            "comprehension" => self.comprehension(node),
            "if_expression" => self.if_expression(node),

            // A kind the grammar grew and this did not. Reproducing it is wrong in
            // layout and right in content, which is the only pair of those two worth
            // having; `hek fmt` is checked against every fixture, so this is reachable
            // only from a grammar change that landed without one.
            _ => Doc::verbatim(self.text(node)),
        }
    }

    // -------------------------------------------------------------- declarations

    /// `keyword Name <rest>`, where the rest is the braced part.
    /// Splits a node whose head is read by position but whose tail is not.
    ///
    /// Lifts only the comments written before the first `head` structural children;
    /// everything from there on is handed back untouched, because it goes to a printer
    /// that places comments itself (`list`, `fields`, `body`, or a sequence of arms).
    /// Taking those out too would move a comment written against one filter or one fold
    /// arm up to the construct's header, which is a placement the author did not choose.
    fn headed_by(&self, node: Node<'a>, head: usize) -> (Vec<Doc<'a>>, Vec<Node<'a>>) {
        let all = self.all_kids(node);
        let mut seen = 0;
        let mut at = 0;
        for (index, kid) in all.iter().enumerate() {
            if kid.kind() != "comment" {
                seen += 1;
                if seen == head {
                    at = index;
                    break;
                }
            }
        }
        let (before, rest): (Vec<_>, Vec<_>) =
            all[..at].iter().partition(|kid| kid.kind() == "comment");
        let mut kids = rest;
        kids.extend_from_slice(&all[at..]);
        (self.docs_of(&before), kids)
    }

    fn headed(
        &self,
        keyword: &'a str,
        node: Node<'a>,
        rest: impl Fn(&Self, &[Node<'a>]) -> Doc<'a>,
    ) -> Doc<'a> {
        // Only the comments *before* the name are lifted: everything after it goes to a
        // printer that places comments itself, and taking those out would move a
        // comment written inside a body up to the declaration's header.
        let (leading, kids) = self.headed_by(node, 1);
        let (name, body) = kids.split_at(1);
        Self::led(
            leading,
            Doc::concat([
                Doc::text(keyword),
                Doc::text(" "),
                self.node(name[0]),
                Doc::text(" "),
                rest(self, body),
            ]),
        )
    }

    /// `command Name(params) { .. }`, shared with `guard`.
    fn callable(&self, keyword: &'a str, node: Node<'a>) -> Doc<'a> {
        let (leading, kids) = self.parted(node);
        Self::led(
            leading,
            Doc::concat([
                Doc::text(keyword),
                Doc::text(" "),
                self.node(kids[0]),
                self.node(kids[1]),
                Doc::text(" "),
                self.node(kids[2]),
            ]),
        )
    }

    /// `fn name(params) -> Type { .. }`, whose result an effect-local one may omit.
    fn function(&self, node: Node<'a>) -> Doc<'a> {
        let (leading, kids) = self.parted(node);
        let mut parts = vec![
            Doc::text("fn"),
            Doc::text(" "),
            self.node(kids[0]),
            self.node(kids[1]),
        ];
        if kids.len() == 4 {
            parts.push(Doc::text(" -> "));
            parts.push(self.node(kids[2]));
        }
        parts.push(Doc::text(" "));
        parts.push(self.node(kids[kids.len() - 1]));
        Self::led(leading, Doc::concat(parts))
    }

    fn const_decl(&self, node: Node<'a>) -> Doc<'a> {
        let (leading, kids) = self.parted(node);
        Self::led(
            leading,
            Doc::concat([
                Doc::text("const "),
                self.node(kids[0]),
                Doc::text(": "),
                self.node(kids[1]),
                Doc::text(" = "),
                self.node(kids[2]),
            ]),
        )
    }

    /// A node's structural children, with any comment lifted out ahead of them.
    ///
    /// A comment is an `extra`, so it can land between any two children of any node, and
    /// `kids` keeps it: a printer that then indexes positionally reads the comment as a
    /// structural child and emits a real token in its place, which for `hek fmt` means
    /// deleting source. The one-line declarations below take this instead, and a comment
    /// written inside one leads it on its own line, which is rule 6's shape.
    fn parted(&self, node: Node<'a>) -> (Vec<Doc<'a>>, Vec<Node<'a>>) {
        let mut leading = Vec::new();
        let mut rest = Vec::new();
        for kid in self.kids(node) {
            match kid.kind() {
                "comment" => leading.push(self.node(kid)),
                _ => rest.push(kid),
            }
        }
        (leading, rest)
    }

    /// Puts any comment the node carried above the one line it prints as.
    ///
    /// For a node that **starts** a line: a declaration, a statement. `docs/fmt.md`
    /// section 6's rule is that a comment leads whatever follows it, and here it can.
    fn led(leading: Vec<Doc<'a>>, line: Doc<'a>) -> Doc<'a> {
        if leading.is_empty() {
            return line;
        }
        let mut parts = Vec::new();
        for comment in leading {
            parts.push(comment);
            parts.push(Doc::Hardline);
        }
        parts.push(line);
        Doc::concat(parts)
    }

    /// The same for a node that sits **mid-line**: an expression, a type, a binding.
    ///
    /// Leading is wrong there and not merely ugly. A comment emitted before the operands
    /// of `let x = a + 1` lands after the `=`, and re-parsing finds it in a different
    /// place in the tree, so a second run moves it again and `fmt` has no fixed point.
    /// A `LineSuffix` defers to the end of whatever line the expression ends on, which
    /// is where a re-parse finds it, so the second run is the first.
    fn trailed(&self, trailing: &[Node<'a>], doc: Doc<'a>) -> Doc<'a> {
        if trailing.is_empty() {
            return doc;
        }
        let mut parts = vec![doc];
        for &comment in trailing {
            parts.extend(self.suffix(comment));
        }
        Doc::concat(parts)
    }

    /// A node's structural children, with its comments kept as nodes so a caller can
    /// choose between [`Self::led`] and [`Self::trailed`].
    fn split(&self, node: Node<'a>) -> (Vec<Node<'a>>, Vec<Node<'a>>) {
        let mut comments = Vec::new();
        let mut rest = Vec::new();
        for kid in self.all_kids(node) {
            match kid.kind() {
                "comment" => comments.push(kid),
                _ => rest.push(kid),
            }
        }
        (comments, rest)
    }

    /// `secret NAME`, or `secret NAME?`. A one-line shape beside `const`: it holds no
    /// list, so there is nothing that could break.
    ///
    /// The `?` is read from the source rather than from a child, because the grammar
    /// carries it as an anonymous token and `kids` keeps only the named ones.
    fn secret_decl(&self, node: Node<'a>) -> Doc<'a> {
        let (leading, kids) = self.parted(node);
        let optional = self.text(node).trim_end().ends_with('?');
        Self::led(
            leading,
            Doc::concat([
                Doc::text("secret "),
                self.node(kids[0]),
                Doc::text(if optional { "?" } else { "" }),
            ]),
        )
    }

    /// `subject Name(Id)` and `subject Name(Id) under Parent`. A one-line declaration
    /// like `const` and `secret`, so it never wraps: the parent is one word.
    fn subject_decl(&self, node: Node<'a>) -> Doc<'a> {
        let (leading, kids) = self.parted(node);
        let mut parts = vec![
            Doc::text("subject "),
            self.node(kids[0]),
            Doc::text("("),
            self.node(kids[1]),
            Doc::text(")"),
        ];
        if let Some(parent) = kids.get(2) {
            parts.push(Doc::text(" under "));
            parts.push(self.node(*parent));
        }
        Self::led(leading, Doc::concat(parts))
    }

    /// `secret NAME = <value>` in a test's setup section: the same word, and the same
    /// one-line shape, with a value after it.
    fn secret_clause(&self, node: Node<'a>) -> Doc<'a> {
        let (leading, kids) = self.parted(node);
        Self::led(
            leading,
            Doc::concat([
                Doc::text("secret "),
                self.node(kids[0]),
                Doc::text(" = "),
                self.node(kids[1]),
            ]),
        )
    }

    /// `refusal Name "message"`, or `refusal Name(field: Type) "message"`. Parens
    /// declare, so the list hugs the name the way a command's does, and the message
    /// takes a space after whichever came before it. `callable` cannot serve: it
    /// indexes three children unconditionally and a refusal has two or three, with no
    /// block among them.
    fn refusal(&self, node: Node<'a>) -> Doc<'a> {
        let (leading, kids) = self.parted(node);
        let mut parts = vec![Doc::text("refusal "), self.node(kids[0])];
        for &rest in &kids[1..] {
            if rest.kind() != "parameters" {
                parts.push(Doc::text(" "));
            }
            parts.push(self.node(rest));
        }
        Self::led(leading, Doc::concat(parts))
    }

    /// `name: Type @annotation` and, for an entity column, `= default`.
    fn field_decl(&self, node: Node<'a>) -> Doc<'a> {
        let (leading, kids) = self.parted(node);
        let mut parts = vec![self.node(kids[0]), Doc::text(": "), self.node(kids[1])];
        for &extra in &kids[2..] {
            parts.push(Doc::text(if extra.kind() == "annotation" {
                " "
            } else {
                " = "
            }));
            parts.push(self.node(extra));
        }
        Self::led(leading, Doc::concat(parts))
    }

    /// `index (a, b)`, whose space before the paren is what tells a soft keyword from a
    /// call and is what the corpus writes.
    fn index_clause(&self, node: Node<'a>) -> Doc<'a> {
        let (leading, kids) = self.parted(node);
        Self::led(
            leading,
            Doc::concat([
                self.node(kids[0]),
                Doc::text(" "),
                self.inline("(", ")", false, &kids[1..]),
            ]),
        )
    }

    /// `@a.b, @c.d`, an `on` header's path list, and whether it left the line already
    /// indented.
    ///
    /// **Shared by an effect arm and a fold arm's slice**, which list paths
    /// the same way. The second copy drifted the moment it existed: printing a fold arm's
    /// second path as its `as` binding wrote `on @a.b as @c.d (..)`, which does not parse.
    ///
    /// The path list breaks like any other list except that it takes **no trailing comma**:
    /// it has no closing delimiter, so a comma there would be followed by `as` and would
    /// not parse. Its continuations are indented rather than aligned under the first path,
    /// which is what the corpus does by hand; one site writes twelve paths that way, and
    /// flat it would be a 328-column line. The comma goes on the path rather than in the
    /// separator, so that a comment landing between two paths still gets one and a comment
    /// after the last path does not.
    ///
    /// Missing that closing delimiter is also why a comment in here needs its own answer
    /// rather than `list`'s. A list ends its line with the delimiter, so `docs/fmt.md`
    /// section 6 gets what it needs by breaking the group; a header has nothing to end the
    /// line, so a `//` written anywhere in it swallowed the paths, the binding and the
    /// block's opening brace, and the file no longer parsed. Every comment here therefore
    /// ends its own line with a hard break, and the piece it leads starts the next one.
    fn path_list(&self, paths: &[Node<'a>]) -> (Vec<Doc<'a>>, bool) {
        let last = paths.iter().rposition(|kid| kid.kind() == "event_path");
        let mut lead = Vec::new();
        let mut previous: Option<Node<'a>> = None;
        let mut hanging = false;
        for (index, &kid) in paths.iter().enumerate() {
            if kid.kind() == "comment" {
                let more = paths[index + 1..]
                    .iter()
                    .any(|next| next.kind() != "comment");
                if !hanging && previous.is_some_and(|before| self.trails(kid, before)) && more {
                    lead.extend(self.suffix(kid));
                    lead.push(Doc::BreakParent);
                    continue;
                }
                if hanging || previous.is_some() {
                    lead.push(Doc::Hardline);
                }
                lead.push(Doc::text(self.text(kid)));
                hanging = true;
                continue;
            }
            if hanging {
                lead.push(Doc::Hardline);
                hanging = false;
            } else if previous.is_some() {
                lead.push(Doc::Line);
            }
            lead.push(match last {
                Some(last) if index < last => Doc::concat([self.node(kid), Doc::text(",")]),
                _ => self.node(kid),
            });
            previous = Some(kid);
        }
        if hanging {
            lead.push(Doc::Hardline);
        }
        (lead, hanging)
    }

    /// `on latest @a, @b as e { @key k, field } { .. }`.
    ///
    /// The delivery modifier stays on the header line with `on`. It is taken out before the
    /// path loop rather than matched inside it, because it is the one child that comes
    /// *before* the first path: left in, it would make `rest` non-empty and send every path
    /// down the fallback arm, commas and all.
    fn handler(&self, node: Node<'a>) -> Doc<'a> {
        let kids = self.kids(node);
        let mut delivery = None;
        let mut paths = Vec::new();
        let mut rest = Vec::new();
        for &kid in &kids {
            match kid.kind() {
                "delivery_keyword" => delivery = Some(self.text(kid)),
                "event_path" | "comment" if rest.is_empty() => paths.push(kid),
                _ => rest.push(kid),
            }
        }
        let (lead, hanging) = self.path_list(&paths);
        let mut parts = vec![
            match delivery {
                Some(word) => Doc::concat([Doc::text("on "), Doc::text(word), Doc::text(" ")]),
                None => Doc::text("on "),
            },
            Doc::group(Doc::indent(Doc::concat(lead))),
        ];
        // A hard break inside the group left the line already indented, so the binding it
        // leads takes no space in front of it.
        let mut fresh = hanging;
        for kid in rest {
            if kid.kind() == "comment" {
                if !fresh {
                    parts.push(Doc::Hardline);
                }
                parts.push(Doc::text(self.text(kid)));
                parts.push(Doc::Hardline);
                fresh = true;
                continue;
            }
            if !fresh {
                parts.push(Doc::text(" "));
            }
            fresh = false;
            parts.push(match kid.kind() {
                "identifier" => Doc::concat([Doc::text("as "), self.node(kid)]),
                _ => self.node(kid),
            });
        }
        Doc::concat(parts)
    }

    // --------------------------------------------------------------------- tests

    fn respond(&self, node: Node<'a>) -> Doc<'a> {
        let mut parts = vec![Doc::text("respond "), self.spaced(node)];
        if self.has_token(node, "timeout") {
            parts.push(Doc::text(" timeout"));
        }
        Doc::concat(parts)
    }

    fn expect(&self, node: Node<'a>) -> Doc<'a> {
        let (trailing, kids) = self.split(node);
        let mut parts = vec![Doc::text("expect")];
        for word in ["nothing", "skipped"] {
            if self.has_token(node, word) {
                parts.push(Doc::text(" "));
                parts.push(Doc::text(word));
            }
        }
        if !kids.is_empty() {
            parts.push(Doc::text(" "));
            parts.push(Doc::join(Doc::text(" "), self.docs_of(&kids)));
        }
        self.trailed(&trailing, Doc::concat(parts))
    }

    /// `[no] Entity[key] { fields }`.
    fn row(&self, node: Node<'a>) -> Doc<'a> {
        let (trailing, kids) = self.split(node);
        let mut parts = Vec::new();
        if self.has_token(node, "no") {
            parts.push(Doc::text("no "));
        }
        // **The single-row table is the word and nothing else**, and the word is an
        // anonymous token, so it is not among the kids at all and `kids[0]` is already
        // whatever follows it. Written as a branch rather than an index, because the
        // indexes below are what would have panicked on it.
        let rest = match self.has_token(node, "global") {
            true => {
                parts.push(Doc::text("global"));
                &kids[..]
            }
            false => {
                parts.push(self.node(kids[0]));
                // The subscript prints its own brackets and sits against the name with no
                // space, so it goes through `node` like any other child rather than being
                // assembled here. That is also what keeps a comment written among the
                // keys inside them.
                parts.push(self.node(kids[1]));
                &kids[2..]
            }
        };
        for &kid in rest {
            parts.push(Doc::text(" "));
            parts.push(self.node(kid));
        }
        self.trailed(&trailing, Doc::concat(parts))
    }

    // ---------------------------------------------------------------- statements

    /// Both shapes: raw slices added to the boundary, or a declared guard called.
    /// The comment handling here is load-bearing twice over. A comment among the slices
    /// joined by `", "` swallowed every slice after it and `hek check` still passed, so a
    /// command silently lost half its append condition; and a comment *before* the first
    /// slice made `kids.first()` fail the `guard_slice` test, which took the flat
    /// branch below and **deleted the commas**. Renaming the node reproduced it exactly,
    /// so the test above is the one to update when the grammar moves.
    fn guard(&self, node: Node<'a>) -> Doc<'a> {
        let (trailing, kids) = self.split(node);
        if kids.first().is_some_and(|kid| kid.kind() == "guard_slice") {
            // Flat, and with no trailing comma: `guard_decl` in `parse.rs` is the one
            // comma loop in the language that parses another slice unconditionally after
            // eating a comma, so `guard @a(x),` does not parse.
            return self.trailed(
                &trailing,
                Doc::concat([
                    Doc::text("guard "),
                    Doc::join(Doc::text(", "), self.docs_of(&kids)),
                ]),
            );
        }
        self.trailed(
            &trailing,
            Doc::concat([
                Doc::text("guard "),
                Doc::join(Doc::text(" "), self.docs_of(&kids)),
            ]),
        )
    }

    /// `fold x: T = seed` with its arms indented under it.
    /// A named fold declaration: `fold N(p) -> [acc:] T = seed { arms }`.
    ///
    /// `callable` cannot serve, for the reason `refusal` gives: it indexes exactly three
    /// children and the accumulator name here is optional, so the count varies. The arms
    /// go through `body`, which makes them a sequence that always breaks and keeps the
    /// author's blank lines, as a `projector`'s handlers are.
    fn fold_definition(&self, node: Node<'a>) -> Doc<'a> {
        // `parted` and not `headed_by`, because every child of this node is positional:
        // the accumulator is optional, so the type and the seed are found by counting.
        // `headed_by` leaves comments past its head *in* the list, and a comment anywhere
        // in the header then shifts that count and the printer emits a different child in
        // each slot. That deleted the return type and the seed outright and produced
        // source that no longer parses, which `hek fmt` then wrote over the original.
        //
        // An arm's comment is unaffected either way: the arms are children of `fold_body`,
        // so a comment against one is that node's and never reaches here.
        let (leading, kids) = self.parted(node);
        let body = kids[kids.len() - 1];
        let middle = &kids[2..kids.len() - 1];
        // `-> acc: T = seed` against `-> T = seed`: three children or two.
        let (acc, ty, seed) = match middle.len() {
            3 => (Some(middle[0]), middle[1], middle[2]),
            _ => (None, middle[0], middle[1]),
        };
        let mut parts = vec![
            Doc::text("fold "),
            self.node(kids[0]),
            self.node(kids[1]),
            Doc::text(" -> "),
        ];
        if let Some(acc) = acc {
            parts.push(self.node(acc));
            parts.push(Doc::text(": "));
        }
        parts.push(self.node(ty));
        parts.push(Doc::text(" = "));
        parts.push(self.node(seed));
        parts.push(Doc::text(" "));
        parts.push(self.node(body));
        Self::led(leading, Doc::concat(parts))
    }

    /// The braced arm run. A sequence, so it always breaks: a fold with one arm on one
    /// line would read as the inline form, which is a different construct.
    fn fold_body(&self, node: Node<'a>) -> Doc<'a> {
        self.body(&self.kids(node))
    }

    /// `Sealed(T, Subject)`, or an ordinary type.
    ///
    /// The children are positional here too, so the comments come out first for the same
    /// reason they do in `fold_definition`: one inside the parens shifted the count and
    /// the subject was printed as the content type and then dropped. A type sits mid-line,
    /// so they trail rather than lead.
    fn fold_type(&self, node: Node<'a>) -> Doc<'a> {
        let (trailing, kids) = self.split(node);
        if kids.len() == 1 {
            return self.trailed(&trailing, self.node(kids[0]));
        }
        // The `?` is an anonymous token, so it is not among the named children, and
        // `has_token` is the question to ask: it looks at this node's own anonymous
        // children only, so a `?` written *inside* the content type belongs to that
        // `type` node and is not counted. Reading it off the last child was equally
        // correct about that and wrong the moment a comment is the final child, which
        // would silently drop a declared `?`.
        let optional = self.has_token(node, "?");
        self.trailed(
            &trailing,
            Doc::concat([
                Doc::text("Sealed("),
                self.node(kids[0]),
                Doc::text(", "),
                self.node(kids[1]),
                Doc::text(")"),
                match optional {
                    true => Doc::text("?"),
                    false => Doc::nil(),
                },
            ]),
        )
    }

    fn fold(&self, node: Node<'a>) -> Doc<'a> {
        // The use form binds a local to a declared fold and has no type, seed or arms.
        // `parted` again: three positional children, so a comment between any two of them
        // shifts the count. `headed_by(node, 1)` dropped one written after the local
        // outright, because nothing below printed the children it left in the list.
        if node.child_by_field_name("fold").is_some() {
            let (leading, kids) = self.parted(node);
            return Self::led(
                leading,
                Doc::concat([
                    Doc::text("fold "),
                    self.node(kids[0]),
                    Doc::text(" = "),
                    self.node(kids[1]),
                    Doc::text(" "),
                    // The initializer list is a node of its own and prints its own braces,
                    // so wrapping it in `list` again gives `Name{ { a, b } }`.
                    self.node(kids[2]),
                ]),
            );
        }
        let (leading, kids) = self.headed_by(node, 3);
        let mut parts = vec![
            Doc::text("fold "),
            self.node(kids[0]),
            Doc::text(": "),
            self.node(kids[1]),
            Doc::text(" = "),
            self.node(kids[2]),
        ];
        let arms: Vec<Doc<'a>> = kids[3..]
            .iter()
            .flat_map(|&arm| [Doc::Hardline, self.node(arm)])
            .collect();
        if !arms.is_empty() {
            parts.push(Doc::indent(Doc::concat(arms)));
        }
        Self::led(leading, Doc::concat(parts))
    }

    /// `on @path(filters) { fields } => value`, breaking after the arrow if it must.
    fn fold_arm(&self, node: Node<'a>) -> Doc<'a> {
        let (mut trailing, kids) = self.split(node);
        // **A `key` clause's comments are trailed on the arm**, which is the one place the
        // formatter reaches into a child for them, and `slice` above says why it does not
        // do that for a filter: a filter list is comment-aware, so taking its comments out
        // would move one written against a filter up to the slice. A `key` clause is
        // mid-line and undelimited, so there is no group for `list` to break and no line
        // of its own for a suffix to land on. Emitted inside the clause, the suffix landed
        // after the `=>` once the value broke, a reparse read it as the value's, and
        // `hek fmt` moved it twice before settling: `--check` failed on a file `hek fmt`
        // had just written.
        for &kid in &kids {
            if kid.kind() == "computed_keys" {
                trailing.extend(self.clause_comments(kid));
            }
        }
        trailing.sort_by_key(Node::start_byte);
        // **A `key` clause breaks onto a line of its own, and the header is its own
        // group.** With one group for the whole arm the header was only ever measured as
        // part of the value's line, so an over-long header stayed put and the innermost
        // call inside the clause broke instead: `key period = period_key(` was left past
        // the margin with `e.at,` below it.
        //
        // Only the clause takes a `Line`. A destructure keeps its space and breaks its own
        // braces open, which is what every other list in the language does and what the
        // arms without a clause already settled on.
        let mut head = vec![Doc::text("on "), self.node(kids[0])];
        for &kid in &kids[1..kids.len() - 1] {
            match kid.kind() {
                "computed_keys" => head.push(Doc::indent(Doc::concat([Doc::Line, self.node(kid)]))),
                _ => {
                    head.push(Doc::text(" "));
                    head.push(self.node(kid));
                }
            }
        }
        head.push(Doc::text(" =>"));
        self.trailed(
            &trailing,
            Doc::group(Doc::concat([
                Doc::group(Doc::concat(head)),
                Doc::indent(Doc::concat([Doc::Line, self.node(kids[kids.len() - 1])])),
            ])),
        )
    }

    /// The filter list is comment-aware (`list` breaks the group and keeps one in place),
    /// so only the comments before the path are lifted: taking the rest out would move a
    /// comment written against one filter up to the slice.
    fn slice(&self, node: Node<'a>) -> Doc<'a> {
        let (leading, kids) = self.headed_by(node, 1);
        // `on @path as e (..)`: the binding is a named child exactly as the filters are,
        // so what separates the two is where the `(` falls and not how many children came
        // first. Counting them put the binding inside the parentheses as a filter of its
        // own the moment a comment sat between the path and the `as`, and dropped the
        // comment: `hek fmt` wrote out a slice that no longer parsed.
        let mut cursor = node.walk();
        let paren = node
            .children(&mut cursor)
            .find(|kid| !kid.is_named() && kid.kind() == "(")
            .map_or(usize::MAX, |kid| kid.start_byte());
        let (before, filters): (Vec<Node<'a>>, Vec<Node<'a>>) =
            kids[1..].iter().partition(|kid| kid.start_byte() < paren);

        // **A fold arm's slice lists paths too**, so the ones after the first and any
        // comment among them go to `path_list` and only what follows them is the binding.
        // Taking `kids[0]` as the path and everything before the `(` as the binding wrote
        // `on @a.b as @c.d (..)`, which does not parse.
        let mut listed = vec![kids[0]];
        let mut bound = Vec::new();
        for &kid in &before {
            match kid.kind() {
                "event_path" | "comment" if bound.is_empty() => listed.push(kid),
                _ => bound.push(kid),
            }
        }
        let (paths, hanging) = self.path_list(&listed);

        // **An empty list is dropped, and `guard_slice` keeps it**, which is the grammar's
        // own split: a fold arm may leave it out and a guard may not. Dropping it is the
        // canonical form, so `on @x() { .. }` is rewritten to `on @x { .. }`.
        //
        // A comment inside the parentheses holds them open, because `list` is where a
        // comment in an empty list survives and there is nowhere else for one to go: the
        // slice is mid-line, so a suffix emitted after a dropped `()` would land past the
        // `=>` and reparse against the arm's value. Such a comment is already in
        // `filters`, since the partition above goes by whether it falls after the `(`, so
        // asking whether `filters` is empty asks about it too. Asking the whole node
        // instead kept `()` for a comment written *before* the paren, which `bound` prints
        // and which leaves the list genuinely empty.
        //
        // Decided before the binding is printed, because the binding's trailing space is
        // there to separate `as e` from the `(` and is a double space without one.
        let paren = node.kind() == "guard_slice" || !filters.is_empty();

        // A line of its own for a comment and a space for the binding, which is what
        // `event_handler` does with the same two things in the same position. A hard break
        // in the path list above has already left the line indented, so nothing is pushed
        // in front of the first thing here when it has: `fresh` starts at `hanging` and
        // the push is guarded, because doing it unguarded put a blank line mid-construct.
        //
        // **The comment arm is the whole of this.** Without it a comment after the binding
        // is printed as `as // why`, the rest of the line is commented out, and `hek fmt`
        // writes a file that no longer parses. `bound` holds everything after the binding,
        // comments included, since only *leading* comments go to the path list.
        let mut tail = Vec::new();
        let mut fresh = hanging;
        // Whether this loop printed anything, which is what the trailing break below is
        // guarded on. `fresh` cannot answer it: with no binding and no comment here it is
        // still whatever `hanging` was, and the path list has already ended its own line.
        let mut printed = false;
        for kid in bound {
            printed = true;
            if kid.kind() == "comment" {
                if !fresh {
                    tail.push(Doc::Hardline);
                }
                tail.push(Doc::text(self.text(kid)));
                fresh = true;
                continue;
            }
            if !fresh {
                tail.push(Doc::text(" "));
            }
            // The binding carries its own trailing separator, as it always did: one space
            // to hold the `(` off `as e`, and none when the list was dropped.
            tail.push(Doc::concat([
                Doc::text("as "),
                self.node(kid),
                match paren {
                    true => Doc::text(" "),
                    false => Doc::text(""),
                },
            ]));
            fresh = false;
        }
        // **A line of its own after a comment this loop printed**, and only then. Pushed on
        // `fresh` alone it fired for a path list that ended on a comment with no binding
        // after it, where `path_list` had already broken the line, and the second break was
        // a blank line in the middle of an arm: `on @a.x` / `// c` / blank / `(game_id) =>`.
        if fresh && printed {
            tail.push(Doc::Hardline);
        }
        // **A separator only where nothing else supplies one.** A binding brings its own,
        // above; with none, several paths take a space so `@a, @b (x)` does not read as
        // `@a` and then `@b(x)`, and one path keeps it tight, which is how every arm in
        // every corpus is written. `if_break` is what keeps the space off a continuation
        // line, since a list that broke has already separated them.
        let several = listed
            .iter()
            .filter(|kid| kid.kind() == "event_path")
            .count()
            > 1;
        let apart = match tail.is_empty() && several {
            true => Doc::if_break(Doc::text(""), Doc::text(" ")),
            false => Doc::text(""),
        };
        Self::led(
            leading,
            Doc::concat([
                Doc::group(Doc::concat([
                    Doc::indent(Doc::concat(paths)),
                    match paren {
                        true => apart,
                        false => Doc::text(""),
                    },
                ])),
                Doc::indent(Doc::concat(tail)),
                match paren {
                    true => self.list("(", ")", false, &filters),
                    false => Doc::text(""),
                },
            ]),
        )
    }

    fn if_statement(&self, node: Node<'a>) -> Doc<'a> {
        let (leading, kids) = self.parted(node);
        let mut parts = vec![
            Doc::text("if "),
            self.node(kids[0]),
            Doc::text(" "),
            self.node(kids[1]),
        ];
        if let Some(&alternative) = kids.get(2) {
            parts.push(Doc::text(" else "));
            parts.push(self.node(alternative));
        }
        Self::led(leading, Doc::concat(parts))
    }

    fn iter_bindings(&self, node: Node<'a>) -> Doc<'a> {
        let (trailing, kids) = self.split(node);
        let names = Doc::join(Doc::text(", "), self.docs_of(&kids[..kids.len() - 1]));
        self.trailed(
            &trailing,
            Doc::concat([names, Doc::text(" in "), self.node(kids[kids.len() - 1])]),
        )
    }

    /// A comment here used to take `kids.first()`'s place, so `return // why` above the
    /// value printed the comment and **deleted** the value outright.
    fn returned(&self, node: Node<'a>) -> Doc<'a> {
        let (trailing, kids) = self.split(node);
        self.trailed(
            &trailing,
            match kids.first() {
                None => Doc::text("return"),
                Some(&value) => Doc::concat([Doc::text("return "), self.node(value)]),
            },
        )
    }

    /// `patch Entity[key] { fields }`, and `delete Entity[key]` with no fields.
    fn keyed_row(&self, node: Node<'a>, fields: bool) -> Doc<'a> {
        let (leading, kids) = self.parted(node);
        let keyword = if !fields {
            "delete"
        } else if self.has_token(node, "update") {
            "update"
        } else {
            "patch"
        };
        let mut parts = vec![
            Doc::text(keyword),
            Doc::text(" "),
            self.node(kids[0]),
            self.node(kids[1]),
        ];
        for &kid in &kids[2..] {
            parts.push(Doc::text(" "));
            parts.push(self.node(kid));
        }
        Self::led(leading, Doc::concat(parts))
    }

    // --------------------------------------------------------------------- types

    /// The trailing `?` is an anonymous child, so it is asked about rather than walked to.
    fn optional_type(&self, node: Node<'a>) -> Doc<'a> {
        let inner = self.spaced(node);
        if self.has_token(node, "?") {
            return Doc::concat([inner, Doc::text("?")]);
        }
        inner
    }

    /// `(a + 1)`, and the three below it: `inline` joins with `", "`, so a comment among
    /// the children became an item and put a separator after a `//`.
    fn parens(&self, node: Node<'a>) -> Doc<'a> {
        let (trailing, kids) = self.split(node);
        self.trailed(&trailing, self.inline("(", ")", false, &kids))
    }

    /// `Money(2)`, `List(T)`: the constructor is an anonymous token.
    fn applied(&self, node: Node<'a>) -> Doc<'a> {
        let (trailing, kids) = self.split(node);
        self.trailed(
            &trailing,
            Doc::concat([
                Doc::text(self.leading_token(node)),
                self.inline("(", ")", false, &kids),
            ]),
        )
    }

    /// Never broken: `map_type` reads its comma with `expect_sym` and has no
    /// trailing-comma escape, so a break that added one would not parse.
    fn map_type(&self, node: Node<'a>) -> Doc<'a> {
        let (trailing, kids) = self.split(node);
        self.trailed(
            &trailing,
            Doc::concat([Doc::text("Map"), self.inline("(", ")", false, &kids)]),
        )
    }

    // --------------------------------------------------------------- expressions

    /// A prefix operator that the grammar leaves anonymous: `!x`, `-x`. The three
    /// outcome verbs are no longer among them: each takes an operand a space away rather
    /// than an argument list, so they print through `keyed_value` beside `invoke`.
    fn prefixed(&self, node: Node<'a>) -> Doc<'a> {
        Doc::concat([Doc::text(self.leading_token(node)), self.spaced(node)])
    }

    /// `a + b`, and never broken across lines.
    ///
    /// The operator is an anonymous child, so it has to be asked for: iterating the named
    /// children alone gives `a b`, which is a plausible-looking way to silently change
    /// arithmetic. Not breaking is the corpus's own rule, and an emphatic one: no line in
    /// 9,531 ends in `&&` or `||`, because where a condition would be too long its author
    /// extracted a `fn` or a `let` instead of wrapping it.
    fn binary(&self, node: Node<'a>) -> Doc<'a> {
        let (trailing, kids) = self.split(node);
        self.trailed(
            &trailing,
            Doc::concat([
                self.node(kids[0]),
                Doc::text(" "),
                Doc::text(self.leading_token(node)),
                Doc::text(" "),
                self.node(kids[1]),
            ]),
        )
    }

    /// A chain never breaks at its dots.
    ///
    /// `receiver` is an `_expression`, so `a.b().c()` left-nests and the obvious grouping
    /// would break the innermost link first, which is backwards. Four lines in 9,531 begin
    /// with a dot, so overflowing is both simpler and closer to what the corpus does than
    /// any breaking rule would be. The arguments still break.
    fn method_call(&self, node: Node<'a>) -> Doc<'a> {
        let (trailing, kids) = self.split(node);
        self.trailed(
            &trailing,
            Doc::concat([
                self.node(kids[0]),
                Doc::text("."),
                self.node(kids[1]),
                self.node(kids[2]),
            ]),
        )
    }

    /// `a.b`, and `.b` with no receiver at all: the stored row, readable only in the value
    /// of a `patch`.
    fn dotted(&self, node: Node<'a>) -> Doc<'a> {
        let (trailing, kids) = self.split(node);
        self.trailed(
            &trailing,
            match kids.len() {
                1 => Doc::concat([Doc::text("."), self.node(kids[0])]),
                _ => Doc::concat([self.node(kids[0]), Doc::text("."), self.node(kids[1])]),
            },
        )
    }

    fn comprehension(&self, node: Node<'a>) -> Doc<'a> {
        let (trailing, kids) = self.split(node);
        let mut parts = vec![
            Doc::text("["),
            self.node(kids[0]),
            Doc::text(" for "),
            self.node(kids[1]),
        ];
        if let Some(&condition) = kids.get(2) {
            parts.push(Doc::text(" if "));
            parts.push(self.node(condition));
        }
        parts.push(Doc::text("]"));
        self.trailed(&trailing, Doc::concat(parts))
    }

    fn if_expression(&self, node: Node<'a>) -> Doc<'a> {
        let (trailing, kids) = self.split(node);
        self.trailed(
            &trailing,
            Doc::concat([
                Doc::text("if "),
                self.node(kids[0]),
                Doc::text(" { "),
                self.node(kids[1]),
                Doc::text(" } else { "),
                self.node(kids[2]),
                Doc::text(" }"),
            ]),
        )
    }

    // ------------------------------------------------------------------- shapes

    /// Statements and declarations: one per line, with an authored blank line kept and a
    /// run of them collapsed to one.
    ///
    /// A blank line is the only part of the output the author still controls, and it is
    /// the only one carrying meaning the tree does not: `const` runs are grouped by what
    /// they are for, a `test` body separates arrange from act from assert, and three folds
    /// that answer one question are written packed.
    fn sequence(&self, children: &[Node<'a>]) -> Doc<'a> {
        let mut parts = Vec::new();
        let mut previous: Option<Node<'a>> = None;
        for &child in children {
            if let Some(before) = previous {
                if self.trails(child, before) {
                    parts.extend(self.suffix(child));
                    previous = Some(child);
                    continue;
                }
                parts.push(Doc::Hardline);
                if self.blank_between(before, child) {
                    parts.push(Doc::Hardline);
                }
            }
            parts.push(self.node(child));
            previous = Some(child);
        }
        Doc::concat(parts)
    }

    /// A braced sequence.
    fn body(&self, children: &[Node<'a>]) -> Doc<'a> {
        if children.is_empty() {
            return Doc::text("{}");
        }
        Doc::concat([
            Doc::text("{"),
            Doc::indent(Doc::concat([Doc::Hardline, self.sequence(children)])),
            Doc::Hardline,
            Doc::text("}"),
        ])
    }

    /// A braced sequence of comma-separated fields, one per line however short.
    ///
    /// No `record`, `event` or `entity` in the corpus is written on one line, and every
    /// `enum` is. That is the corpus telling a type declaration apart from a value
    /// enumeration, which is the same line rustfmt draws at a named-field struct and
    /// Prettier at a TypeScript interface.
    fn fields(&self, children: &[Node<'a>]) -> Doc<'a> {
        if children.is_empty() {
            return Doc::text("{}");
        }
        let mut parts = Vec::new();
        let mut previous: Option<Node<'a>> = None;
        for &child in children {
            if let Some(before) = previous {
                if self.trails(child, before) {
                    parts.extend(self.suffix(child));
                    previous = Some(child);
                    continue;
                }
                parts.push(Doc::Hardline);
                if self.blank_between(before, child) {
                    parts.push(Doc::Hardline);
                }
            }
            parts.push(self.node(child));
            if child.kind() != "comment" {
                parts.push(Doc::text(","));
            }
            previous = Some(child);
        }
        Doc::concat([
            Doc::text("{"),
            Doc::indent(Doc::concat([Doc::Hardline, Doc::concat(parts)])),
            Doc::Hardline,
            Doc::text("}"),
        ])
    }

    /// A comma-separated list that fits on one line or goes one item per line.
    ///
    /// `pad` is whether the delimiters take a space when flat: braces do (`{ a, b }`),
    /// parentheses and brackets do not (`f(a, b)`).
    fn list(&self, open: &'a str, close: &'a str, pad: bool, children: &[Node<'a>]) -> Doc<'a> {
        if children.is_empty() {
            return Doc::concat([Doc::text(open), Doc::text(close)]);
        }
        let edge = if pad { Doc::Line } else { Doc::Softline };
        let mut parts = Vec::new();
        let mut separate = false;
        let mut previous: Option<Node<'a>> = None;
        for (index, &child) in children.iter().enumerate() {
            if child.kind() == "comment" {
                if let Some(before) = previous.filter(|&before| self.trails(child, before)) {
                    let _ = before;
                    parts.extend(self.suffix(child));
                    parts.push(Doc::BreakParent);
                } else {
                    if separate {
                        parts.push(Doc::text(","));
                        parts.push(Doc::Line);
                        separate = false;
                    }
                    parts.push(Doc::text(self.text(child)));
                    match index + 1 < children.len() {
                        true => parts.push(Doc::Hardline),
                        // **The closer has to leave the line.** A `//` comment runs to the
                        // end of it, so a `)` printed after one is commented out and
                        // `hek fmt` writes a file that no longer parses. Reachable only
                        // when a comment is the last child, which before the filter list
                        // became optional meant `on @x(/* why */)` and nothing else, so no
                        // fixture had it. `BreakParent` breaks the group without a line of
                        // its own, and the group's edge then puts the closer below.
                        false => parts.push(Doc::BreakParent),
                    }
                }
                previous = Some(child);
                continue;
            }
            if separate {
                parts.push(Doc::text(","));
                parts.push(Doc::Line);
            }
            parts.push(self.node(child));
            separate = true;
            previous = Some(child);
        }
        if separate {
            parts.push(Doc::if_break(Doc::text(","), Doc::nil()));
        }
        Doc::group(Doc::concat([
            Doc::text(open),
            Doc::indent(Doc::concat([edge.clone(), Doc::concat(parts)])),
            edge,
            Doc::text(close),
        ]))
    }

    /// A comma-separated list that never breaks and never takes a trailing comma.
    ///
    /// A destructure, an `iter_bindings`, a `Map(K, V)` and the raw guard slice list. The
    /// last three have no trailing-comma escape in `parse.rs`, so for them this is
    /// correctness rather than layout.
    fn inline(&self, open: &'a str, close: &'a str, pad: bool, children: &[Node<'a>]) -> Doc<'a> {
        if children.is_empty() {
            return Doc::concat([Doc::text(open), Doc::text(close)]);
        }
        let edge = Doc::text(if pad { " " } else { "" });
        Doc::concat([
            Doc::text(open),
            edge.clone(),
            Doc::join(Doc::text(", "), self.docs_of(children)),
            edge,
            Doc::text(close),
        ])
    }

    // ------------------------------------------------------------------ fragments

    /// `keyword <children separated by spaces>`.
    /// `keyword rest`, for a construct that **starts** its line.
    ///
    /// It cannot go through `spaced`, whose trailing comment is flushed at the end of the
    /// line the construct *ends* on: for a `test` or a `for`, that is the line after its
    /// whole body, so the comment came out describing whatever followed the closing
    /// brace.
    fn keyed(&self, keyword: &'a str, node: Node<'a>) -> Doc<'a> {
        let (comments, kids) = self.split(node);
        Self::led(
            self.docs_of(&comments),
            Doc::concat([
                Doc::text(keyword),
                Doc::text(" "),
                Doc::join(Doc::text(" "), self.docs_of(&kids)),
            ]),
        )
    }

    /// `key a = x, b = y` (`docs/folds.md` rule 14), and `param = value` inside it.
    ///
    /// **Neither emits its own comments**, which [`fold_arm`](Self::fold_arm) trails on the
    /// whole arm instead, and that method says why. Every comment descendant goes, which is
    /// what makes it a fixed point rather than one shape of it: a comment can sit after
    /// `key`, after the parameter and after the `=`, and each of the three is mid-line in
    /// a construct that has no line of its own.
    fn computed_keys(&self, node: Node<'a>) -> Doc<'a> {
        let (_, kids) = self.split(node);
        // Grouped with a break between the parts, because the clause sits in the arm's
        // head and the head has no break of its own: two interpolated parts ran to 103
        // columns against a `WIDTH` of 90, which is the one measure the formatter holds
        // to everywhere else. Indented, so a part that moves down reads as a continuation
        // of the clause rather than as the next thing in the arm.
        Doc::group(Doc::concat([
            Doc::text("key "),
            Doc::indent(Doc::join(
                Doc::concat([Doc::text(","), Doc::Line]),
                self.docs_of(&kids),
            )),
        ]))
    }

    /// `param = value`, and `else skip` after it where one was written.
    ///
    /// The separator cannot be `" = "` joined across every child: `skip_keyword` is a child
    /// too, so joining wrote `param = value = skip` and the file no longer parsed.
    fn computed_key(&self, node: Node<'a>) -> Doc<'a> {
        let (_, kids) = self.split(node);
        let (skip, rest): (Vec<Node<'a>>, Vec<Node<'a>>) =
            kids.iter().partition(|kid| kid.kind() == "skip_keyword");
        let mut parts = vec![Doc::join(Doc::text(" = "), self.docs_of(&rest))];
        if !skip.is_empty() {
            parts.push(Doc::text(" else skip"));
        }
        Doc::concat(parts)
    }

    /// The comments of a `key` clause itself: its own and its `computed_key`s', in source
    /// order, and **not** the ones inside a part's value.
    ///
    /// The clause is what has nowhere to put a comment. A value does: it is an ordinary
    /// expression, with whatever parens, blocks and comment-aware lists it is made of, and
    /// its own printer already places one. Taking those too wrote the comment twice, once
    /// hoisted and once by the printer that owned it, and in an `if` value the two copies
    /// landed on different lines and collapsed differently on the next pass, which is the
    /// non-fixed-point this hoist exists to remove.
    fn clause_comments(&self, node: Node<'a>) -> Vec<Node<'a>> {
        let mut found = Vec::new();
        for kid in self.all_kids(node) {
            match kid.kind() {
                "comment" => found.push(kid),
                "computed_key" => found.extend(
                    self.all_kids(kid)
                        .into_iter()
                        .filter(|part| part.kind() == "comment"),
                ),
                _ => {}
            }
        }
        found.sort_by_key(Node::start_byte);
        found
    }

    /// The same for a construct that may sit **mid-line**: `let outcome = invoke C { .. }`,
    /// `expect reject X`. Leading there puts a hardline after the `=`. The outcome verbs
    /// take this too, because `expect` puts each of them mid-line as well.
    fn keyed_value(&self, keyword: &'a str, node: Node<'a>) -> Doc<'a> {
        let (trailing, kids) = self.split(node);
        self.trailed(
            &trailing,
            Doc::concat([
                Doc::text(keyword),
                Doc::text(" "),
                Doc::join(Doc::text(" "), self.docs_of(&kids)),
            ]),
        )
    }

    /// `keyword name = value`.
    fn assignment(&self, keyword: &'a str, node: Node<'a>) -> Doc<'a> {
        let (leading, kids) = self.parted(node);
        Self::led(
            leading,
            Doc::concat([
                Doc::text(keyword),
                Doc::text(" "),
                self.node(kids[0]),
                Doc::text(" = "),
                self.node(kids[1]),
            ]),
        )
    }

    /// `name: Type`.
    fn typed(&self, node: Node<'a>) -> Doc<'a> {
        self.joined(node, ": ")
    }

    /// Children with a separator between them, and any comment sent to the end of the
    /// line.
    ///
    /// The separator is what makes this the shape that must not see a comment: joining
    /// one as though it were a child puts `: ` or `, ` after a `//`, which comments out
    /// the rest of the line and takes the real children with it.
    fn joined(&self, node: Node<'a>, separator: &'a str) -> Doc<'a> {
        let (trailing, kids) = self.split(node);
        self.trailed(
            &trailing,
            Doc::join(Doc::text(separator), self.docs_of(&kids)),
        )
    }

    /// `name` alone, or `name: value`. The shorthand is the common case in both a filter
    /// and a field initializer, and writing `{ order_id }` is writing every name.
    fn optional_value(&self, node: Node<'a>) -> Doc<'a> {
        self.joined(node, ": ")
    }

    /// A filter's two forms, which differ only in the separator: `field: value` or the
    /// bare shorthand, and `param == value` for the key part an arm writes
    /// (`docs/folds.md` rule 13). Asked of the node rather than counted, because the
    /// separator is an anonymous child and the named ones are the same either way.
    fn filter(&self, node: Node<'a>) -> Doc<'a> {
        match self.has_token(node, "==") {
            true => self.joined(node, " == "),
            false => self.joined(node, ": "),
        }
    }

    /// Children separated by a single space, which is most of the language.
    fn spaced(&self, node: Node<'a>) -> Doc<'a> {
        self.joined(node, " ")
    }

    // -------------------------------------------------------------------- tree

    fn text(&self, node: Node<'a>) -> &'a str {
        &self.source[node.byte_range()]
    }

    /// The named children, which is exactly the content plus the comments: every piece of
    /// punctuation and every keyword in this grammar is an anonymous token.
    fn kids(&self, node: Node<'a>) -> Vec<Node<'a>> {
        self.all_kids(node)
    }

    /// Every named child, comments included. What a printer that **places** comments
    /// wants: `sequence`, `list`, `fields` and `handler` each decide where one goes.
    fn all_kids(&self, node: Node<'a>) -> Vec<Node<'a>> {
        let mut cursor = node.walk();
        node.named_children(&mut cursor).collect()
    }

    fn docs_of(&self, nodes: &[Node<'a>]) -> Vec<Doc<'a>> {
        nodes.iter().map(|&node| self.node(node)).collect()
    }

    fn has_token(&self, node: Node<'a>, token: &str) -> bool {
        let mut cursor = node.walk();
        node.children(&mut cursor)
            .any(|child| !child.is_named() && child.kind() == token)
    }

    /// The first anonymous child, which is where this grammar keeps a choice of keyword or
    /// operator that it did not give a field to.
    fn leading_token(&self, node: Node<'a>) -> &'a str {
        let mut cursor = node.walk();
        node.children(&mut cursor)
            .find(|child| !child.is_named())
            .map_or("", |child| self.text(child))
    }

    /// Whether `comment` was written after code on the same line.
    fn trails(&self, comment: Node<'a>, before: Node<'a>) -> bool {
        comment.kind() == "comment" && before.end_position().row == comment.start_position().row
    }

    /// A trailing comment, held back so it lands after the rest of its line rather than
    /// above the next one. Nothing in the corpus writes one; the grammar allows it, so it
    /// gets an answer rather than a surprise.
    fn suffix(&self, comment: Node<'a>) -> [Doc<'a>; 2] {
        [Doc::LineSuffix(" "), Doc::LineSuffix(self.text(comment))]
    }

    fn blank_between(&self, before: Node<'a>, after: Node<'a>) -> bool {
        after.start_position().row > before.end_position().row + 1
    }
}
