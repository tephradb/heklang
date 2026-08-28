; Conservative: only the bindings a reader would call locals. Field names, entity names
; and declaration names are left to highlights.scm.

[
  (function_declaration)
  (command_declaration)
  (event_handler)
  (fold_arm)
  (for_statement)
  (comprehension)
  (block)
] @local.scope

(parameter name: (identifier) @local.definition)
(let_statement name: (identifier) @local.definition)
(state_declaration name: (identifier) @local.definition)
(event_handler binding: (identifier) @local.definition)
(destructure (identifier) @local.definition)
(iter_bindings index: (identifier) @local.definition)
(iter_bindings item: (identifier) @local.definition)

(identifier) @local.reference
