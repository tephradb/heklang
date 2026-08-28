
// A tree-sitter grammar for hek (`.hk`), the language in ../src.
//
// It mirrors src/lex.rs and src/parse.rs closely enough to highlight, and is a
// deliberate superset in one direction: heklang's parser knows whether it is inside a
// command, a projector, an effect, a `fn` or a test, and refuses statements that do not
// belong there. A tree-sitter grammar has no such context, so every body accepts every
// statement. Nothing valid fails to parse; some invalid programs parse.

const PREC = {
  or: 1,
  and: 2,
  cmp: 3,
  add: 4,
  mul: 5,
  unary: 6,
  postfix: 7,
  call: 8,
};

/** Trailing commas are accepted everywhere the parser accepts one, which is everywhere. */
function commaSep1(rule) {
  return seq(rule, repeat(seq(',', rule)), optional(','));
}

function commaSep(rule) {
  return optional(commaSep1(rule));
}

module.exports = grammar({
  name: 'hek',

  extras: ($) => [$.comment, /\s/],

  word: ($) => $.identifier,

  conflicts: ($) => [
    // `if plan { ... }` against `Item { ... }`: the parser clears a `no_record_literal`
    // flag for headers (parse.rs `header_expr`); here the block wins by dynamic
    // precedence when both parses survive.
    [$._type_name, $._primary],
    // A statement may be a call or an `http.*` call, and the parser cannot know which
    // until it sees whether a `.` follows.
    [$._primary, $.expression_statement],
    // `invoke` is both a statement and a value.
    [$._primary, $._statement],
  ],

  rules: {
    source_file: ($) => repeat($._declaration),

    // Only `//` to end of line. There are no block comments (lex.rs `skip_trivia`).
    comment: (_) => token(seq('//', /.*/)),

    // ---------------------------------------------------------------- declarations

    _declaration: ($) =>
      choice(
        $.enum_declaration,
        $.record_declaration,
        $.const_declaration,
        $.function_declaration,
        $.event_declaration,
        $.command_declaration,
        $.projector_declaration,
        $.effect_declaration,
        $.test_declaration,
      ),

    enum_declaration: ($) =>
      seq(
        'enum',
        field('name', $._type_name),
        '{',
        commaSep1($.enum_variant),
        '}',
      ),

    enum_variant: ($) =>
      seq(optional($.annotation), field('name', $.identifier)),

    record_declaration: ($) =>
      seq(
        'record',
        field('name', $._type_name),
        '{',
        commaSep1($.record_field),
        '}',
      ),

    record_field: ($) =>
      seq(
        field('name', $.identifier),
        ':',
        field('type', $.type),
        repeat($.annotation),
      ),

    // A const has no closing token; it ends where its value does (parse.rs `skip_const`).
    const_declaration: ($) =>
      seq(
        'const',
        field('name', $.identifier),
        ':',
        field('type', $.type),
        '=',
        field('value', $._expression),
      ),

    function_declaration: ($) =>
      seq(
        'fn',
        field('name', $.identifier),
        field('parameters', $.parameters),
        '->',
        field('return_type', $.type),
        field('body', $.block),
      ),

    parameters: ($) => seq('(', commaSep($.parameter), ')'),

    parameter: ($) =>
      seq(field('name', $.identifier), ':', field('type', $.type)),

    event_declaration: ($) =>
      seq(
        'event',
        field('path', $.event_path),
        '{',
        commaSep($.event_field),
        '}',
      ),

    event_field: ($) =>
      seq(
        field('name', $.identifier),
        ':',
        field('type', $.type),
        repeat($.annotation),
      ),

    command_declaration: ($) =>
      seq(
        'command',
        field('name', $._type_name),
        field('parameters', $.parameters),
        field('body', $.block),
      ),

    projector_declaration: ($) =>
      seq(
        'projector',
        field('name', $._type_name),
        '{',
        repeat(
          choice($.enum_declaration, $.entity_declaration, $.event_handler),
        ),
        '}',
      ),

    entity_declaration: ($) =>
      seq(
        'entity',
        field('name', $._type_name),
        '{',
        commaSep(choice($.index_clause, $.entity_field)),
        '}',
      ),

    // `index` stays a soft keyword, so it is read as a name plus a `(` rather than
    // reserved (parse.rs `at_index_clause`).
    index_clause: ($) =>
      seq(
        alias($.identifier, $.index_keyword),
        '(',
        commaSep1($.identifier),
        ')',
      ),

    entity_field: ($) =>
      seq(
        field('name', $.identifier),
        ':',
        field('type', $.type),
        repeat($.annotation),
        optional(seq('=', field('default', $._expression))),
      ),

    effect_declaration: ($) =>
      seq(
        'effect',
        field('name', $._type_name),
        '{',
        repeat($.event_handler),
        '}',
      ),

    // One shape for a projector handler and an effect arm; only an arm lists more than
    // one path, and a projector with two would be rejected by the checker, not here.
    event_handler: ($) =>
      seq(
        'on',
        commaSep1(field('path', $.event_path)),
        optional(seq('as', field('binding', $.identifier))),
        optional(field('destructure', $.destructure)),
        field('body', $.block),
      ),

    destructure: ($) => seq('{', commaSep1($.identifier), '}'),

    // ----------------------------------------------------------------------- tests

    test_declaration: ($) =>
      seq('test', field('name', $.string), field('body', $.test_body)),

    // Every word here but `test` is soft: claimed inside a test body and an ordinary
    // name everywhere else (docs/testing.md rule 1).
    test_body: ($) =>
      seq(
        '{',
        repeat($.given_clause),
        repeat(choice($.respond_clause, $.erased_clause)),
        optional($._action_clause),
        repeat($.expect_clause),
        '}',
      ),

    given_clause: ($) =>
      seq('given', field('path', $.event_path), $.field_initializer_list),

    respond_clause: ($) =>
      seq(
        'respond',
        field('url', $._expression),
        choice(
          'timeout',
          seq(field('status', $.integer_literal), optional($.object_literal)),
        ),
      ),

    erased_clause: ($) =>
      seq('erased', field('subject', $.identifier), field('id', $._expression)),

    _action_clause: ($) =>
      choice($.run_clause, $.project_clause, $.deliver_clause),

    run_clause: ($) =>
      seq('run', field('command', $.identifier), $.field_initializer_list),

    project_clause: ($) => seq('project', field('projector', $.identifier)),

    deliver_clause: ($) => seq('deliver', field('effect', $.identifier)),

    expect_clause: ($) => seq('expect', optional($._expectation)),

    _expectation: ($) =>
      choice(
        'nothing',
        'skipped',
        $.event_expectation,
        $.row_expectation,
        $.outcome_expression,
        $.invoke_expression,
        $.call_expression,
        $.method_call,
      ),

    event_expectation: ($) =>
      seq(field('path', $.event_path), $.field_initializer_list),

    row_expectation: ($) =>
      seq(
        optional('no'),
        field('entity', $.identifier),
        '[',
        field('key', $._expression),
        ']',
        optional($.field_initializer_list),
      ),

    // ------------------------------------------------------------------ statements

    block: ($) => seq('{', repeat($._statement), '}'),

    _statement: ($) =>
      choice(
        $.guard_declaration,
        $.state_declaration,
        $.let_statement,
        $.if_statement,
        $.for_statement,
        $.return_statement,
        $.emit_statement,
        $.put_statement,
        $.patch_statement,
        $.delete_statement,
        $.invoke_expression,
        $.expression_statement,
      ),

    guard_declaration: ($) => seq('guard', commaSep1($.slice_reference)),

    state_declaration: ($) =>
      seq(
        'state',
        field('name', $.identifier),
        ':',
        field('type', $.type),
        '=',
        'fold',
        field('seed', $._expression),
        repeat($.fold_arm),
      ),

    fold_arm: ($) =>
      seq(
        'on',
        $.slice_reference,
        optional(field('destructure', $.destructure)),
        '=>',
        field('value', $._expression),
      ),

    // A slice always carries its parentheses, which is what tells a fold arm from a
    // handler when both begin `on @path` (parse.rs `slice_ref`).
    slice_reference: ($) =>
      seq(field('path', $.event_path), '(', commaSep($.filter), ')'),

    filter: ($) =>
      seq(
        field('field', $.identifier),
        optional(seq(':', field('value', $._expression))),
      ),

    let_statement: ($) =>
      seq('let', field('name', $.identifier), '=', field('value', $._expression)),

    // `else if` is a chain rather than a nesting (parse.rs statement, `Keyword::If`).
    if_statement: ($) =>
      seq(
        'if',
        field('condition', $._expression),
        field('consequence', $.block),
        optional(
          seq(
            'else',
            field('alternative', choice($.if_statement, $.block)),
          ),
        ),
      ),

    for_statement: ($) =>
      seq('for', $.iter_bindings, field('body', $.block)),

    iter_bindings: ($) =>
      seq(
        field('index', $.identifier),
        optional(seq(',', field('item', $.identifier))),
        'in',
        field('container', $._expression),
      ),

    // Bare, an outcome, or a value inside a `fn`.
    return_statement: ($) =>
      prec.right(
        seq('return', optional(choice($.outcome_expression, $._expression))),
      ),

    outcome_expression: ($) =>
      seq(choice('invalid', 'reject'), field('arguments', $.arguments)),

    emit_statement: ($) =>
      seq('emit', field('path', $.event_path), $.field_initializer_list),

    put_statement: ($) =>
      seq('put', field('entity', $.identifier), $.field_initializer_list),

    // `patch` and `update` differ only in what an absent row means, so they are one
    // statement here as they are one IR node (docs/projectors.md rule 5).
    patch_statement: ($) =>
      seq(
        choice('patch', 'update'),
        field('entity', $.identifier),
        '[',
        field('key', $._expression),
        ']',
        $.field_initializer_list,
      ),

    delete_statement: ($) =>
      seq(
        'delete',
        field('entity', $.identifier),
        '[',
        field('key', $._expression),
        ']',
      ),

    // `fail(...)`, `log(...)`, `erase(...)` and a discarded `http.*` call.
    expression_statement: ($) => choice($.call_expression, $.method_call),

    // ----------------------------------------------------------------------- types

    type: ($) =>
      seq(
        choice(
          $.primitive_type,
          $.scaled_type,
          $.list_type,
          $.map_type,
          $._type_name,
        ),
        optional('?'),
      ),

    primitive_type: (_) =>
      choice('Bool', 'Int', 'String', 'Uuid', 'Timestamp', 'Json'),

    scaled_type: ($) =>
      seq(choice('Decimal', 'Money'), '(', $.integer_literal, ')'),

    list_type: ($) => seq('List', '(', $.type, ')'),

    map_type: ($) =>
      seq('Map', '(', field('key', $.type), ',', field('value', $.type), ')'),

    // A single aliased node, so a query on it has no nested `(identifier)` under it
    // competing for the same range.
    _type_name: ($) => alias($.identifier, $.type_identifier),

    // ----------------------------------------------------------------- expressions

    _expression: ($) =>
      choice($._primary, $.unary_expression, $.binary_expression),

    _primary: ($) =>
      choice(
        $.integer_literal,
        $.decimal_literal,
        $.string,
        $.raw_string,
        $.boolean_literal,
        $.none_literal,
        $.identifier,
        $.stored_field,
        $.call_expression,
        $.method_call,
        $.field_expression,
        $.record_literal,
        $.object_literal,
        $.list,
        $.comprehension,
        $.invoke_expression,
        $.if_expression,
        $.parenthesized_expression,
      ),

    parenthesized_expression: ($) => seq('(', $._expression, ')'),

    unary_expression: ($) =>
      prec(PREC.unary, seq(choice('!', '-'), $._expression)),

    // Comparison is non-associative in parse.rs `cmp_expr`; left here, since a chain is
    // a parse error rather than a different tree, and a highlighter should still colour
    // one that was written by mistake.
    binary_expression: ($) =>
      choice(
        ...[
          ['||', PREC.or],
          ['&&', PREC.and],
          ['==', PREC.cmp],
          ['!=', PREC.cmp],
          ['<=', PREC.cmp],
          ['>=', PREC.cmp],
          ['<', PREC.cmp],
          ['>', PREC.cmp],
          ['+', PREC.add],
          ['-', PREC.add],
          ['*', PREC.mul],
          ['/', PREC.mul],
          ['%', PREC.mul],
        ].map(([operator, precedence]) =>
          prec.left(
            precedence,
            seq(
              field('left', $._expression),
              field('operator', operator),
              field('right', $._expression),
            ),
          ),
        ),
      ),

    call_expression: ($) =>
      prec(
        PREC.call,
        seq(field('function', $.identifier), field('arguments', $.arguments)),
      ),

    method_call: ($) =>
      prec(
        PREC.call,
        seq(
          field('receiver', $._expression),
          '.',
          field('method', $.identifier),
          field('arguments', $.arguments),
        ),
      ),

    // Parenless, so a field rather than a method. Only a `Response`, a record and the
    // `as` envelope have any (parse.rs `postfix_expr`).
    field_expression: ($) =>
      prec(
        PREC.postfix,
        seq(field('receiver', $._expression), '.', field('field', $.identifier)),
      ),

    // `headers = ...` is the language's only named argument (parse.rs `http_call`).
    arguments: ($) =>
      seq('(', commaSep(choice($.named_argument, $._expression)), ')'),

    named_argument: ($) =>
      seq(field('name', $.identifier), '=', field('value', $._expression)),

    // The block wins wherever an `if` or `for` header could also read `Name {` as a
    // literal, which is what `no_record_literal` does in the parser.
    record_literal: ($) =>
      prec.dynamic(
        -1,
        seq(field('name', $._type_name), $.field_initializer_list),
      ),

    invoke_expression: ($) =>
      seq('invoke', field('command', $.identifier), $.field_initializer_list),

    // `{ field: value, shorthand }`, shared by emit, put, patch, record literals,
    // invoke, given, run and every expectation that names fields.
    field_initializer_list: ($) =>
      seq('{', commaSep($.field_initializer), '}'),

    field_initializer: ($) =>
      seq(
        field('name', $.identifier),
        optional(seq(':', field('value', $._expression))),
      ),

    // An object key is a quoted string (parse.rs `object_literal`).
    object_literal: ($) => seq('{', commaSep($.object_entry), '}'),

    object_entry: ($) =>
      seq(
        field('key', choice($.string, $.raw_string)),
        ':',
        field('value', $._expression),
      ),

    list: ($) => seq('[', commaSep($._expression), ']'),

    comprehension: ($) =>
      seq(
        '[',
        field('yield', $._expression),
        'for',
        $.iter_bindings,
        optional(seq('if', field('condition', $._expression))),
        ']',
      ),

    // The value-position `if`, where both branches are required.
    if_expression: ($) =>
      seq(
        'if',
        field('condition', $._expression),
        '{',
        field('consequence', $._expression),
        '}',
        'else',
        '{',
        field('alternative', $._expression),
        '}',
      ),

    // `.field` with no receiver: the stored row, readable only in a patch or update
    // value (parse.rs `primary`, `Sym::Dot`).
    stored_field: ($) => seq('.', field('field', $.identifier)),

    // -------------------------------------------------------------------- literals

    boolean_literal: (_) => choice('true', 'false'),

    none_literal: (_) => 'none',

    // ASCII digits, no separators, no exponent, no suffix, and the `.` is taken only
    // when a digit follows (lex.rs `number`).
    decimal_literal: (_) => token(prec(1, /[0-9]+\.[0-9]+/)),

    integer_literal: (_) => token(/[0-9]+/),

    // `"""..."""` is fully raw and must be tried before `"` (lex.rs `raw_text`).
    raw_string: (_) =>
      token(
        prec(
          2,
          seq('"""', repeat(choice(/[^"]/, /"[^"]/, /""[^"]/)), '"""'),
        ),
      ),

    // Interpolation nests because a hole holds an ordinary expression, and a string
    // inside one is just this rule again (lex.rs `interp`).
    string: ($) =>
      seq(
        '"',
        repeat(choice($.string_content, $.escape_sequence, $.interpolation)),
        token.immediate('"'),
      ),

    string_content: (_) => token.immediate(prec(1, /[^"\\{]+/)),

    escape_sequence: (_) => token.immediate(/\\[nt"\\{}]/),

    interpolation: ($) => seq(token.immediate('{'), $._expression, '}'),

    // One token for both an event path and an annotation name (lex.rs `path`); the two
    // rules below split it back apart by position so a theme can colour them apart.
    _at_name: (_) =>
      token(/@[_\p{L}][_\p{L}\p{N}]*(\.[_\p{L}][_\p{L}\p{N}]*)*/),

    event_path: ($) => $._at_name,

    annotation: ($) =>
      seq(
        alias($._at_name, $.annotation_name),
        optional($.annotation_arguments),
      ),

    annotation_arguments: ($) =>
      seq('(', commaSep(choice($.integer_literal, $.identifier)), ')'),

    // Unicode, matching `is_alphabetic`/`is_alphanumeric` in lex.rs.
    identifier: (_) => /[_\p{L}][_\p{L}\p{N}]*/,
  },
});
