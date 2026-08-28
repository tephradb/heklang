; Scopes

[
  (function_declaration)
  (command_declaration)
  (event_handler)
  (fold_arm)
  (for_statement)
  (comprehension)
  (block)
] @local.scope

; Definitions.
;
; The class after `local.definition.` is the highlight a *resolved reference* gets, so
; these repeat the scope names highlights.scm uses rather than being bare
; `@local.definition`. With a bare one a parameter's uses fall through to the
; `(identifier) @variable` catch-all instead of staying `variable.parameter`.
;
; A handler's destructure and its `as` binding are how it names its inputs, which is what
; a parameter list is, so all three share `variable.parameter`.

(parameter
  name: (identifier) @local.definition.variable.parameter)

(event_handler
  binding: (identifier) @local.definition.variable.parameter)

(destructure
  (identifier) @local.definition.variable.parameter)

(let_statement
  name: (identifier) @local.definition.variable)

(state_declaration
  name: (identifier) @local.definition.variable)

(iter_bindings
  index: (identifier) @local.definition.variable)

(iter_bindings
  item: (identifier) @local.definition.variable)

; References

(identifier) @local.reference

; Names that are not variable references. Without these, a field or method that happens
; to share a name with an in-scope local takes that local's colour: `fn effective_sku`
; binds `sku`, and `item.sku` three lines down would follow it.

(call_expression
  function: (identifier) @_)
(method_call
  method: (identifier) @_)
(field_expression
  field: (identifier) @_)
(stored_field
  field: (identifier) @_)
(field_initializer
  name: (identifier) @_)
(named_argument
  name: (identifier) @_)
(filter
  field: (identifier) @_)
(record_field
  name: (identifier) @_)
(event_field
  name: (identifier) @_)
(entity_field
  name: (identifier) @_)
(enum_variant
  name: (identifier) @_)
(annotation_arguments
  (identifier) @_)
(index_clause
  (identifier) @_)
(function_declaration
  name: (identifier) @_)
(const_declaration
  name: (identifier) @_)
(put_statement
  entity: (identifier) @_)
(patch_statement
  entity: (identifier) @_)
(delete_statement
  entity: (identifier) @_)
(row_expectation
  entity: (identifier) @_)
(invoke_expression
  command: (identifier) @_)
(run_clause
  command: (identifier) @_)
(project_clause
  projector: (identifier) @_)
(deliver_clause
  effect: (identifier) @_)
(erased_clause
  subject: (identifier) @_)
