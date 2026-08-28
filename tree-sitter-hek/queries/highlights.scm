; Helix reads these from runtime/queries/hek/.
;
; ORDER: the LAST matching pattern wins, and the innermost node wins
; (helix book, guides/adding_languages.md). So this file runs general to specific: the
; catch-alls are at the top and the most specific rules at the bottom, which is the same
; shape helix's own runtime/queries/rust/highlights.scm has.

; ---------------------------------------------------------------------- catch-alls

(identifier) @variable
(type_identifier) @type

; Two conventions a grammar cannot know. Anything spelled either way that is *not* one of
; these is captured more specifically below, so it wins over both.
((identifier) @type.enum.variant
 (#match? @type.enum.variant "^[A-Z][a-zA-Z0-9_]*$"))

; Second, because SCREAMING_SNAKE matches the PascalCase pattern too and has to win.
((identifier) @constant
 (#match? @constant "^[A-Z][A-Z0-9_]*$"))

; ------------------------------------------------------------------------ literals

(comment) @comment.line

[
  (string)
  (raw_string)
] @string

(escape_sequence) @constant.character.escape

(decimal_literal) @constant.numeric.float
(integer_literal) @constant.numeric.integer

(boolean_literal) @constant.builtin.boolean
(none_literal) @constant.builtin

; After the PascalCase rule above, which would otherwise read these as enum variants.
((identifier) @constant.builtin
 (#any-of? @constant.builtin "HalfUp" "HalfEven" "Down"))

; ----------------------------------------------------------- punctuation, operators

["(" ")" "[" "]" "{" "}"] @punctuation.bracket
["," ":" "."] @punctuation.delimiter

[
  "!"
  "!="
  "%"
  "&&"
  "*"
  "+"
  "-"
  "->"
  "/"
  "<"
  "<="
  "="
  "=="
  "=>"
  ">"
  ">="
  "?"
  "||"
] @operator

; The same `{` and `}` tokens the bracket rule above matched, so this has to follow it.
(interpolation
  "{" @punctuation.special
  "}" @punctuation.special)

; ------------------------------------------------------------------------ keywords

[
  "enum"
  "record"
  "event"
  "entity"
  "const"
  "test"
] @keyword.storage.type

[
  "fn"
  "command"
  "projector"
  "effect"
] @keyword.function

[
  "if"
  "else"
] @keyword.control.conditional

[
  "for"
  "in"
] @keyword.control.repeat

"return" @keyword.control.return

[
  "as"
  "delete"
  "emit"
  "fold"
  "guard"
  "invoke"
  "let"
  "on"
  "patch"
  "put"
  "state"
  "update"
] @keyword

; `index` is soft: a name plus a `(`, so it is claimed here rather than reserved.
(index_keyword) @keyword

; Soft inside a test body and an ordinary name everywhere else.
[
  "given"
  "respond"
  "erased"
  "timeout"
  "run"
  "project"
  "deliver"
  "expect"
  "no"
  "nothing"
  "skipped"
] @keyword

; ------------------------------------------------------------- paths, annotations

; `@max`, `@key`, `@subject(...)`, `@no_index`, `@index`, `@default`.
(annotation_name) @attribute

; `@order.placed`. The same lexer token as an annotation, kept a separate colour because
; one names a declaration and the other modifies a field.
(event_path) @label

; --------------------------------------------------------------------------- types

(primitive_type) @type.builtin
(scaled_type ["Decimal" "Money"] @type.builtin)
(list_type "List" @type.builtin)
(map_type "Map" @type.builtin)

; --------------------------------------------------------------------- field names

(record_field name: (identifier) @variable.other.member)
(event_field name: (identifier) @variable.other.member)
(entity_field name: (identifier) @variable.other.member)
(field_initializer name: (identifier) @variable.other.member)
(field_expression field: (identifier) @variable.other.member)
(stored_field field: (identifier) @variable.other.member)
(filter field: (identifier) @variable.other.member)
(named_argument name: (identifier) @variable.other.member)
(erased_clause subject: (identifier) @variable.other.member)
(annotation_arguments (identifier) @variable.other.member)
(index_clause (identifier) @variable.other.member)

; ------------------------------------------------------------------------ bindings

(parameter name: (identifier) @variable.parameter)

; A destructure and an `as` binding are how a handler names its inputs, which is what a
; parameter list is; locals.scm carries the same class so their uses match.
(event_handler binding: (identifier) @variable.parameter)
(destructure (identifier) @variable.parameter)

(enum_variant name: (identifier) @type.enum.variant)

; -------------------------------------------------------------------- declarations

(enum_declaration name: (type_identifier) @type)
(record_declaration name: (type_identifier) @type)
(entity_declaration name: (type_identifier) @type)
(projector_declaration name: (type_identifier) @type)
(effect_declaration name: (type_identifier) @type)
(command_declaration name: (type_identifier) @function)
(function_declaration name: (identifier) @function)
(const_declaration name: (identifier) @constant)

; The entity, command, projector or effect a statement or a test clause names.
(put_statement entity: (identifier) @type)
(patch_statement entity: (identifier) @type)
(delete_statement entity: (identifier) @type)
(row_expectation entity: (identifier) @type)
(project_clause projector: (identifier) @type)
(deliver_clause effect: (identifier) @type)

; ----------------------------------------------------------------- calls, builtins

(call_expression function: (identifier) @function)
(method_call method: (identifier) @function.method)
(record_literal name: (type_identifier) @constructor)
(invoke_expression command: (identifier) @function)
(run_clause command: (identifier) @function)

(outcome_expression ["invalid" "reject"] @function.builtin)

; The closed global namespace: actions with no natural receiver. After the generic call
; rules above, so these override them.
((call_expression function: (identifier) @function.builtin)
 (#any-of? @function.builtin "now" "reveal" "log" "fail" "erase"))

; `Uuid.derive(..)`, `Json.encode(..)`, `Money.parse(..)`, `http.post(..)`.
((method_call
   receiver: (identifier) @type.builtin
   method: (identifier) @function.builtin)
 (#any-of? @type.builtin "Uuid" "Map" "Json" "Timestamp" "Money" "http"))

; `Map.empty` and `Json.empty` take no arguments, so they are a field access.
((field_expression
   receiver: (identifier) @type.builtin
   field: (identifier) @function.builtin)
 (#any-of? @type.builtin "Uuid" "Map" "Json" "Timestamp" "Money" "http"))
