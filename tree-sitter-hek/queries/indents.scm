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

; A fold is the one construct here that indents with no delimiter to do it: its arms sit two
; spaces under the `state` line and nothing closes them.
;
; Deliberately `@indent` without `@extend`. `@extend` is how helix's own python queries carry
; an indent past a node's end, and it is wrong here: a fold ends where its last arm does, and
; the statement after it belongs back at the block's level, with nothing to hang an
; `@extend.prevent-once` on. The cost is that the *first* arm is still indented by hand,
; because until it exists the declaration ends at its seed; every arm after it follows from
; this, which is 98 of the 158 folds in the sources.
(state_declaration) @indent

[
  "}"
  "]"
  ")"
] @outdent
