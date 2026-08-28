; The three handler kinds and a `fn` are all "functions" for movement; a declaration that
; only holds fields is a "class".

(function_declaration
  body: (block) @function.inside) @function.around

(command_declaration
  body: (block) @function.inside) @function.around

(event_handler
  body: (block) @function.inside) @function.around

(fold_arm) @function.around

[
  (record_declaration)
  (enum_declaration)
  (entity_declaration)
  (event_declaration)
  (projector_declaration)
  (effect_declaration)
] @class.around

(test_declaration
  body: (test_body) @test.inside) @test.around

(parameters
  (parameter) @parameter.inside)

(arguments
  (_) @parameter.inside)

[
  (field_initializer)
  (object_entry)
  (record_field)
  (event_field)
  (entity_field)
  (enum_variant)
] @entry.inside

(comment) @comment.inside

(comment)+ @comment.around
