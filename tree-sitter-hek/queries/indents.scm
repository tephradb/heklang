; Only the node that owns the delimiters is captured, so a `command` indents through its
; `block` and its `parameters` rather than twice over.

[
  (enum_declaration)
  (record_declaration)
  (event_declaration)
  (entity_declaration)
  (projector_declaration)
  (effect_declaration)
  (block)
  (test_body)
  (destructure)
  (field_initializer_list)
  (object_literal)
  (list)
  (comprehension)
  (parameters)
  (arguments)
  (annotation_arguments)
  (slice_reference)
  (parenthesized_expression)
] @indent

[
  "}"
  "]"
  ")"
] @outdent
