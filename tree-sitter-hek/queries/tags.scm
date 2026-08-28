; hek has no language server, so this is what the syntax symbol picker has to work with.
; Every top-level declaration, plus the two things declared inside a projector.

(function_declaration
  name: (identifier) @name) @definition.function

(command_declaration
  name: (type_identifier) @name) @definition.function

(projector_declaration
  name: (type_identifier) @name) @definition.class

(effect_declaration
  name: (type_identifier) @name) @definition.class

(record_declaration
  name: (type_identifier) @name) @definition.struct

(enum_declaration
  name: (type_identifier) @name) @definition.enum

(entity_declaration
  name: (type_identifier) @name) @definition.struct

(event_declaration
  path: (event_path) @name) @definition.type

(const_declaration
  name: (identifier) @name) @definition.constant

(test_declaration
  name: (string) @name) @definition.function

(call_expression
  function: (identifier) @name) @reference.call

(method_call
  method: (identifier) @name) @reference.call

(invoke_expression
  command: (identifier) @name) @reference.call
