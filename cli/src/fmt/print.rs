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
            "enum_variant" => Doc::join(Doc::text(" "), self.docs(node)),
            "record_field" | "event_field" | "entity_field" => self.field_decl(node),
            "index_clause" => self.index_clause(node),
            "const_declaration" => self.const_decl(node),
            "function_declaration" => self.function(node),
            "command_declaration" => self.callable("command", node),
            "guard_definition" => self.callable("guard", node),
            "parameters" | "arguments" | "annotation_arguments" => {
                self.list("(", ")", false, &self.kids(node))
            }
            "parameter" => self.typed(node),
            "annotation" => Doc::concat(self.docs(node)),
            "event_handler" => self.handler(node),
            "destructure" => self.inline("{", "}", true, &self.kids(node)),

            // ------------------------------------------------------------- tests
            "test_declaration" => self.keyed("test", node),
            "test_body" => self.body(&self.kids(node)),
            "given_clause" => self.keyed("given", node),
            "run_clause" => self.keyed("run", node),
            "project_clause" => self.keyed("project", node),
            "deliver_clause" => self.keyed("deliver", node),
            "erased_clause" => self.keyed("erased", node),
            "respond_clause" => self.respond(node),
            "expect_clause" => self.expect(node),
            "event_expectation" => self.spaced(node),
            "row_expectation" => self.row(node),

            // -------------------------------------------------------- statements
            "block" => self.body(&self.kids(node)),
            "guard_declaration" => self.guard(node),
            "state_declaration" => self.state(node),
            "fold_arm" => self.fold_arm(node),
            "slice_reference" => self.slice(node),
            "filter" | "field_initializer" => self.optional_value(node),
            "let_statement" => self.assignment("let", node),
            "if_statement" => self.if_statement(node),
            "for_statement" => self.keyed("for", node),
            "iter_bindings" => self.iter_bindings(node),
            "return_statement" => self.returned(node),
            "outcome_expression" => self.prefixed(node),
            "emit_statement" => self.keyed("emit", node),
            "put_statement" => self.keyed("put", node),
            "patch_statement" => self.keyed_row(node, true),
            "delete_statement" => self.keyed_row(node, false),
            "expression_statement" => Doc::concat(self.docs(node)),

            // -------------------------------------------------------------- types
            "type" => self.optional_type(node),
            "scaled_type" | "list_type" => self.applied(node),
            "map_type" => self.map_type(node),

            // -------------------------------------------------------- expressions
            "parenthesized_expression" => self.inline("(", ")", false, &self.kids(node)),
            "unary_expression" => self.prefixed(node),
            "binary_expression" => self.binary(node),
            "call_expression" => Doc::concat(self.docs(node)),
            "method_call" => self.method_call(node),
            "field_expression" | "stored_field" => self.dotted(node),
            "named_argument" => Doc::join(Doc::text(" = "), self.docs(node)),
            "record_literal" => self.spaced(node),
            "invoke_expression" => self.keyed("invoke", node),
            "field_initializer_list" | "object_literal" => {
                self.list("{", "}", true, &self.kids(node))
            }
            "object_entry" => Doc::join(Doc::text(": "), self.docs(node)),
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
    fn headed(
        &self,
        keyword: &'a str,
        node: Node<'a>,
        rest: impl Fn(&Self, &[Node<'a>]) -> Doc<'a>,
    ) -> Doc<'a> {
        let kids = self.kids(node);
        let (name, body) = kids.split_at(1);
        Doc::concat([
            Doc::text(keyword),
            Doc::text(" "),
            self.node(name[0]),
            Doc::text(" "),
            rest(self, body),
        ])
    }

    /// `command Name(params) { .. }`, shared with `guard`.
    fn callable(&self, keyword: &'a str, node: Node<'a>) -> Doc<'a> {
        let kids = self.kids(node);
        Doc::concat([
            Doc::text(keyword),
            Doc::text(" "),
            self.node(kids[0]),
            self.node(kids[1]),
            Doc::text(" "),
            self.node(kids[2]),
        ])
    }

    /// `fn name(params) -> Type { .. }`, whose result an effect-local one may omit.
    fn function(&self, node: Node<'a>) -> Doc<'a> {
        let kids = self.kids(node);
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
        Doc::concat(parts)
    }

    fn const_decl(&self, node: Node<'a>) -> Doc<'a> {
        let kids = self.kids(node);
        Doc::concat([
            Doc::text("const "),
            self.node(kids[0]),
            Doc::text(": "),
            self.node(kids[1]),
            Doc::text(" = "),
            self.node(kids[2]),
        ])
    }

    /// `name: Type @annotation` and, for an entity column, `= default`.
    fn field_decl(&self, node: Node<'a>) -> Doc<'a> {
        let kids = self.kids(node);
        let mut parts = vec![self.node(kids[0]), Doc::text(": "), self.node(kids[1])];
        for &extra in &kids[2..] {
            parts.push(Doc::text(if extra.kind() == "annotation" {
                " "
            } else {
                " = "
            }));
            parts.push(self.node(extra));
        }
        Doc::concat(parts)
    }

    /// `index (a, b)`, whose space before the paren is what tells a soft keyword from a
    /// call and is what the corpus writes.
    fn index_clause(&self, node: Node<'a>) -> Doc<'a> {
        let kids = self.kids(node);
        Doc::concat([
            self.node(kids[0]),
            Doc::text(" "),
            self.inline("(", ")", false, &kids[1..]),
        ])
    }

    /// `on @a, @b as e { fields } { .. }`.
    ///
    /// Everything before the block is flat. The corpus writes a 103-column header rather
    /// than break a destructure, and the one header it does write across lines fits in 77
    /// when flattened, so the language's only column alignment costs nothing to drop.
    fn handler(&self, node: Node<'a>) -> Doc<'a> {
        let kids = self.kids(node);
        let mut parts = vec![Doc::text("on ")];
        let mut paths = Vec::new();
        let mut rest = Vec::new();
        for &kid in &kids {
            match kid.kind() {
                "event_path" if rest.is_empty() => paths.push(self.node(kid)),
                _ => rest.push(kid),
            }
        }
        parts.push(Doc::join(Doc::text(", "), paths));
        for kid in rest {
            match kid.kind() {
                "identifier" => {
                    parts.push(Doc::text(" as "));
                    parts.push(self.node(kid));
                }
                _ => {
                    parts.push(Doc::text(" "));
                    parts.push(self.node(kid));
                }
            }
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
        let kids = self.kids(node);
        let mut parts = vec![Doc::text("expect")];
        for word in ["nothing", "skipped"] {
            if self.has_token(node, word) {
                parts.push(Doc::text(" "));
                parts.push(Doc::text(word));
            }
        }
        if !kids.is_empty() {
            parts.push(Doc::text(" "));
            parts.push(Doc::join(Doc::text(" "), self.docs(node)));
        }
        Doc::concat(parts)
    }

    /// `[no] Entity[key] { fields }`.
    fn row(&self, node: Node<'a>) -> Doc<'a> {
        let kids = self.kids(node);
        let mut parts = Vec::new();
        if self.has_token(node, "no") {
            parts.push(Doc::text("no "));
        }
        parts.push(self.node(kids[0]));
        parts.push(Doc::text("["));
        parts.push(self.node(kids[1]));
        parts.push(Doc::text("]"));
        for &kid in &kids[2..] {
            parts.push(Doc::text(" "));
            parts.push(self.node(kid));
        }
        Doc::concat(parts)
    }

    // ---------------------------------------------------------------- statements

    /// Both shapes: raw slices added to the boundary, or a declared guard called.
    fn guard(&self, node: Node<'a>) -> Doc<'a> {
        let kids = self.kids(node);
        if kids
            .first()
            .is_some_and(|kid| kid.kind() == "slice_reference")
        {
            // Flat, and with no trailing comma: `guard_decl` in `parse.rs` is the one
            // comma loop in the language that parses another slice unconditionally after
            // eating a comma, so `guard @a(x),` does not parse.
            return Doc::concat([
                Doc::text("guard "),
                Doc::join(Doc::text(", "), self.docs(node)),
            ]);
        }
        Doc::concat([Doc::text("guard "), self.spaced(node)])
    }

    /// `state x: T = fold seed` with its arms indented under it.
    fn state(&self, node: Node<'a>) -> Doc<'a> {
        let kids = self.kids(node);
        let mut parts = vec![
            Doc::text("state "),
            self.node(kids[0]),
            Doc::text(": "),
            self.node(kids[1]),
            Doc::text(" = fold "),
            self.node(kids[2]),
        ];
        let arms: Vec<Doc<'a>> = kids[3..]
            .iter()
            .flat_map(|&arm| [Doc::Hardline, self.node(arm)])
            .collect();
        if !arms.is_empty() {
            parts.push(Doc::indent(Doc::concat(arms)));
        }
        Doc::concat(parts)
    }

    /// `on @path(filters) { fields } => value`, breaking after the arrow if it must.
    fn fold_arm(&self, node: Node<'a>) -> Doc<'a> {
        let kids = self.kids(node);
        let mut head = vec![Doc::text("on "), self.node(kids[0])];
        for &kid in &kids[1..kids.len() - 1] {
            head.push(Doc::text(" "));
            head.push(self.node(kid));
        }
        head.push(Doc::text(" =>"));
        Doc::group(Doc::concat([
            Doc::concat(head),
            Doc::indent(Doc::concat([Doc::Line, self.node(kids[kids.len() - 1])])),
        ]))
    }

    fn slice(&self, node: Node<'a>) -> Doc<'a> {
        let kids = self.kids(node);
        Doc::concat([self.node(kids[0]), self.list("(", ")", false, &kids[1..])])
    }

    fn if_statement(&self, node: Node<'a>) -> Doc<'a> {
        let kids = self.kids(node);
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
        Doc::concat(parts)
    }

    fn iter_bindings(&self, node: Node<'a>) -> Doc<'a> {
        let kids = self.kids(node);
        let names = Doc::join(Doc::text(", "), self.docs_of(&kids[..kids.len() - 1]));
        Doc::concat([names, Doc::text(" in "), self.node(kids[kids.len() - 1])])
    }

    fn returned(&self, node: Node<'a>) -> Doc<'a> {
        let kids = self.kids(node);
        match kids.first() {
            None => Doc::text("return"),
            Some(&value) => Doc::concat([Doc::text("return "), self.node(value)]),
        }
    }

    /// `patch Entity[key] { fields }`, and `delete Entity[key]` with no fields.
    fn keyed_row(&self, node: Node<'a>, fields: bool) -> Doc<'a> {
        let kids = self.kids(node);
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
            Doc::text("["),
            self.node(kids[1]),
            Doc::text("]"),
        ];
        for &kid in &kids[2..] {
            parts.push(Doc::text(" "));
            parts.push(self.node(kid));
        }
        Doc::concat(parts)
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

    /// `Money(2)`, `List(T)`: the constructor is an anonymous token.
    fn applied(&self, node: Node<'a>) -> Doc<'a> {
        Doc::concat([
            Doc::text(self.leading_token(node)),
            self.inline("(", ")", false, &self.kids(node)),
        ])
    }

    /// Never broken: `map_type` reads its comma with `expect_sym` and has no
    /// trailing-comma escape, so a break that added one would not parse.
    fn map_type(&self, node: Node<'a>) -> Doc<'a> {
        Doc::concat([
            Doc::text("Map"),
            self.inline("(", ")", false, &self.kids(node)),
        ])
    }

    // --------------------------------------------------------------- expressions

    /// A prefix operator or keyword that the grammar leaves anonymous: `!x`, `-x`,
    /// `reject(..)`, `invalid(..)`.
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
        let kids = self.kids(node);
        Doc::concat([
            self.node(kids[0]),
            Doc::text(" "),
            Doc::text(self.leading_token(node)),
            Doc::text(" "),
            self.node(kids[1]),
        ])
    }

    /// A chain never breaks at its dots.
    ///
    /// `receiver` is an `_expression`, so `a.b().c()` left-nests and the obvious grouping
    /// would break the innermost link first, which is backwards. Four lines in 9,531 begin
    /// with a dot, so overflowing is both simpler and closer to what the corpus does than
    /// any breaking rule would be. The arguments still break.
    fn method_call(&self, node: Node<'a>) -> Doc<'a> {
        let kids = self.kids(node);
        Doc::concat([
            self.node(kids[0]),
            Doc::text("."),
            self.node(kids[1]),
            self.node(kids[2]),
        ])
    }

    /// `a.b`, and `.b` with no receiver at all: the stored row, readable only in the value
    /// of a `patch`.
    fn dotted(&self, node: Node<'a>) -> Doc<'a> {
        let kids = self.kids(node);
        match kids.len() {
            1 => Doc::concat([Doc::text("."), self.node(kids[0])]),
            _ => Doc::concat([self.node(kids[0]), Doc::text("."), self.node(kids[1])]),
        }
    }

    fn comprehension(&self, node: Node<'a>) -> Doc<'a> {
        let kids = self.kids(node);
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
        Doc::concat(parts)
    }

    fn if_expression(&self, node: Node<'a>) -> Doc<'a> {
        let kids = self.kids(node);
        Doc::concat([
            Doc::text("if "),
            self.node(kids[0]),
            Doc::text(" { "),
            self.node(kids[1]),
            Doc::text(" } else { "),
            self.node(kids[2]),
            Doc::text(" }"),
        ])
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
                    if index + 1 < children.len() {
                        parts.push(Doc::Hardline);
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
    fn keyed(&self, keyword: &'a str, node: Node<'a>) -> Doc<'a> {
        Doc::concat([Doc::text(keyword), Doc::text(" "), self.spaced(node)])
    }

    /// `keyword name = value`.
    fn assignment(&self, keyword: &'a str, node: Node<'a>) -> Doc<'a> {
        let kids = self.kids(node);
        Doc::concat([
            Doc::text(keyword),
            Doc::text(" "),
            self.node(kids[0]),
            Doc::text(" = "),
            self.node(kids[1]),
        ])
    }

    /// `name: Type`.
    fn typed(&self, node: Node<'a>) -> Doc<'a> {
        Doc::join(Doc::text(": "), self.docs(node))
    }

    /// `name` alone, or `name: value`. The shorthand is the common case in both a filter
    /// and a field initializer, and writing `{ order_id }` is writing every name.
    fn optional_value(&self, node: Node<'a>) -> Doc<'a> {
        Doc::join(Doc::text(": "), self.docs(node))
    }

    /// Children separated by a single space, which is most of the language.
    fn spaced(&self, node: Node<'a>) -> Doc<'a> {
        Doc::join(Doc::text(" "), self.docs(node))
    }

    // -------------------------------------------------------------------- tree

    fn text(&self, node: Node<'a>) -> &'a str {
        &self.source[node.byte_range()]
    }

    /// The named children, which is exactly the content plus the comments: every piece of
    /// punctuation and every keyword in this grammar is an anonymous token.
    fn kids(&self, node: Node<'a>) -> Vec<Node<'a>> {
        let mut cursor = node.walk();
        node.named_children(&mut cursor).collect()
    }

    fn docs(&self, node: Node<'a>) -> Vec<Doc<'a>> {
        self.docs_of(&self.kids(node))
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
