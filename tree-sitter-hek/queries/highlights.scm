; Helix reads these from runtime/queries/hek/. Order matters: the first pattern that
; matches a node wins, so the specific rules come first and the catch-alls last.

; ------------------------------------------------------------------- declarations

(enum_declaration name: (type_identifier) @type)
(record_declaration name: (type_identifier) @type)
(entity_declaration name: (type_identifier) @type)
(projector_declaration name: (type_identifier) @type)
(effect_declaration name: (type_identifier) @type)
(command_declaration name: (type_identifier) @function)
(function_declaration name: (identifier) @function)
(const_declaration name: (identifier) @constant)

(enum_variant name: (identifier) @type.enum.variant)
(parameter name: (identifier) @variable.parameter)
(event_handler binding: (identifier) @variable.parameter)

; ------------------------------------------------------------- paths, annotations

; `@max`, `@key`, `@subject(...)`, `@no_index`, `@index`, `@default`.
(annotation_name) @attribute

; `@order.placed`. The same lexer token as an annotation, kept a separate colour
; because one names a declaration and the other modifies a field.
(event_path) @label

; ------------------------------------------------------------------------- types

(primitive_type) @type.builtin
(scaled_type ["Decimal" "Money"] @type.builtin)
(list_type "List" @type.builtin)
(map_type "Map" @type.builtin)

; ---------------------------------------------------------- builtins and calls

; The closed global namespace: actions with no natural receiver.
((call_expression function: (identifier) @function.builtin)
 (#any-of? @function.builtin "now" "reveal" "log" "fail" "erase"))

; `Uuid.derive(..)`, `Json.encode(..)`, `Money.parse(..)`, `http.post(..)`.
((method_call
   receiver: (identifier) @type.builtin
   method: (identifier) @function.builtin)
 (#any-of? @type.builtin "Uuid" "Map" "Json" "Timestamp" "Money" "http"))

; `Map.empty` and `Json.empty` take no arguments.
((field_expression
   receiver: (identifier) @type.builtin
   field: (identifier) @function.builtin)
 (#any-of? @type.builtin "Uuid" "Map" "Json" "Timestamp" "Money" "http"))

(outcome_expression ["invalid" "reject"] @function.builtin)

(method_call method: (identifier) @function.method)
(call_expression function: (identifier) @function)

(record_literal name: (type_identifier) @constructor)
(invoke_expression command: (identifier) @function)
(run_clause command: (identifier) @function)
(project_clause projector: (identifier) @type)
(deliver_clause effect: (identifier) @type)

; The entity a write or a row expectation names.
(put_statement entity: (identifier) @type)
(patch_statement entity: (identifier) @type)
(delete_statement entity: (identifier) @type)
(row_expectation entity: (identifier) @type)

; ------------------------------------------------------------------- field names

(record_field name: (identifier) @variable.other.member)
(event_field name: (identifier) @variable.other.member)
(entity_field name: (identifier) @variable.other.member)
(field_initializer name: (identifier) @variable.other.member)
(field_expression field: (identifier) @variable.other.member)
(stored_field field: (identifier) @variable.other.member)
(destructure (identifier) @variable.other.member)
(filter field: (identifier) @variable.other.member)
(named_argument name: (identifier) @variable.other.member)
(erased_clause subject: (identifier) @variable.other.member)
(annotation_arguments (identifier) @variable.other.member)
(index_clause (identifier) @variable.other.member)

; ---------------------------------------------------------------------- keywords

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

; ---------------------------------------------------------------------- literals

(boolean_literal) @constant.builtin.boolean
(none_literal) @constant.builtin

; The rounding modes, the only bare builtin values.
((identifier) @constant.builtin
 (#any-of? @constant.builtin "HalfUp" "HalfEven" "Down"))

(decimal_literal) @constant.numeric.float
(integer_literal) @constant.numeric.integer

[
  (string)
  (raw_string)
] @string

(escape_sequence) @constant.character.escape

(interpolation
  "{" @punctuation.special
  "}" @punctuation.special)

(comment) @comment.line

; --------------------------------------------------------------------- operators

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

["(" ")" "[" "]" "{" "}"] @punctuation.bracket
["," ":" "."] @punctuation.delimiter

; ------------------------------------------------------------------- catch-alls

; SCREAMING_SNAKE is a `const` by convention, and PascalCase in a value position is an
; enum variant: everything else spelled either way is captured above.
((identifier) @constant
 (#match? @constant "^[A-Z][A-Z0-9_]*$"))

((identifier) @type.enum.variant
 (#match? @type.enum.variant "^[A-Z][a-zA-Z0-9_]*$"))

(type_identifier) @type
(identifier) @variable
