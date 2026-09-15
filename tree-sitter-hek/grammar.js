
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
        $.secret_declaration,
        $.function_declaration,
        $.event_declaration,
        $.refusal_declaration,
        $.command_declaration,
        $.guard_definition,
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

    // Like a `const`, no closing token: it ends at its name, or at the `?` that makes it
    // optional. `secret` stays soft the way `latest` and `live` do (`delivery_keyword`
    // below): `word` is `identifier`, so keyword extraction claims it only where a rule
    // expects it, and a field or parameter of that name still lexes as an identifier.
    secret_declaration: ($) =>
      seq('secret', field('name', $.identifier), optional('?')),

    // The result is optional because an effect-local `fn` may omit it: `fail` is its
    // only other way out, so there is nothing for a caller to decide from. A module
    // `fn` must declare one, and that is a rule the parser keeps rather than the
    // grammar, which has no idea which body it is inside.
    function_declaration: ($) =>
      seq(
        'fn',
        field('name', $.identifier),
        field('parameters', $.parameters),
        optional(seq('->', field('return_type', $.type))),
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

    // A named proposition about the log. Parens declare and braces use, which is what
    // tells this apart from the `guard Name { .. }` inside a body.
    guard_definition: ($) =>
      seq(
        'guard',
        field('name', $._type_name),
        field('parameters', $.parameters),
        field('body', $.block),
      ),

    // A named refusal and the one message it carries. Parens declare and braces use,
    // the same split `guard` has; a refusal with no fields declares no parens and is
    // named with no braces, which is what lets `reject Name` end a block.
    refusal_declaration: ($) =>
      seq(
        'refusal',
        field('name', $._type_name),
        optional(field('parameters', $.parameters)),
        field('message', choice($.string, $.raw_string)),
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

    // An effect body takes helpers beside its arms; a projector body does not
    // (parse.rs `effect_decl` against `projector_shell`).
    effect_declaration: ($) =>
      seq(
        'effect',
        field('name', $._type_name),
        '{',
        repeat(choice($.event_handler, $.function_declaration)),
        '}',
      ),

    // One shape for a projector handler and an effect arm; only an arm carries a
    // delivery modifier or lists more than one path, and a projector doing either would
    // be rejected by the checker, not here.
    event_handler: ($) =>
      seq(
        'on',
        optional(field('delivery', $.delivery_keyword)),
        commaSep1(field('path', $.event_path)),
        optional(seq('as', field('binding', $.identifier))),
        optional(field('destructure', $.destructure)),
        field('body', $.block),
      ),

    // `latest` and `live` stay soft, so they are still writable as names everywhere else
    // (parse.rs `delivery`): `word` is `identifier`, so keyword extraction claims them
    // only in this position. A node of its own rather than a bare anonymous token, so a
    // theme can colour it and so `hek fmt`, which walks named children only, can see it.
    delivery_keyword: (_) => choice('latest', 'live'),

    // An entry may carry `@key`, which marks an effect arm's partition key (rule 15).
    // Only the annotated form is wrapped, so a plain field stays the bare identifier
    // every query and every corpus tree already names.
    destructure: ($) => seq('{', commaSep1(choice($.identifier, $.keyed_field)), '}'),

    keyed_field: ($) => seq($.annotation, field('name', $.identifier)),

    // ----------------------------------------------------------------------- tests

    test_declaration: ($) =>
      seq('test', field('name', $.string), field('body', $.test_body)),

    // Every word here but `test` is soft: claimed inside a test body and an ordinary
    // name everywhere else (docs/testing.md rule 1).
    test_body: ($) =>
      seq(
        '{',
        repeat($.given_clause),
        repeat(choice($.respond_clause, $.erased_clause, $.secret_clause)),
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

    // The fourth setup lever (docs/testing.md section 3). It shares its word with the
    // declaration above, which is the one word this construct does not reserve for
    // itself; both are soft.
    secret_clause: ($) =>
      seq('secret', field('name', $.identifier), '=', field('value', $._expression)),

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
        $.refusal_expression,
        $.fail_statement,
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
        $.fold_declaration,
        $.let_statement,
        $.if_statement,
        $.for_statement,
        $.return_statement,
        $.outcome_expression,
        $.refusal_expression,
        $.fail_statement,
        $.emit_statement,
        $.put_statement,
        $.patch_statement,
        $.delete_statement,
        $.invoke_expression,
        $.expression_statement,
      ),

    // Two shapes, told apart by the token after `guard`: a path is raw slices added to
    // the boundary, a name is a declared guard. See `docs/guards.md`.
    guard_declaration: ($) =>
      choice(
        seq('guard', commaSep1($.slice_reference)),
        seq('guard', field('guard', $._type_name), $.field_initializer_list),
      ),

    fold_declaration: ($) =>
      seq(
        'fold',
        field('name', $.identifier),
        ':',
        field('type', $.type),
        '=',
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

    // Bare, or a value inside a `fn`. An outcome is no longer one of them: saying the
    // answer ends the declaration, so `reject` and `invalid` are statements of their own.
    //
    // `fail` is named here because it is a soft name (rule 10) that this grammar also
    // spells as a keyword. After `return` it can only be a value: a `fail` *statement*
    // there would be unreachable. That is the same call `Parser::ends_return` makes, and
    // without it a local named `fail` is a file `hek fmt` cannot read.
    //
    // The two answers are named here for the opposite reason: `return reject X` is the
    // removed spelling, and this grammar is a superset on purpose. Without them the words
    // still parse, as a bare `return` followed by a statement, and `hek fmt` relays the
    // one line out into two before `hek check` ever gets to explain it. Keeping the old
    // shape whole is what lets the formatter hand a pre-migration file back unchanged.
    return_statement: ($) =>
      prec.right(
        seq(
          'return',
          optional(
            choice(
              $._expression,
              $.outcome_expression,
              $.refusal_expression,
              alias('fail', $.identifier),
            ),
          ),
        ),
      ),

    // `invalid <message>`, `fail <message>`. No parens, because parens are a call and a
    // call comes back; these are two of the three answers.
    //
    // The message is a string, and that is the language rather than a narrowing here:
    // `docs/refusals.md` has it as "a message and nothing else", and a value reaches one
    // through a hole. `src/parse.rs` took any expression with a `String` hint until
    // `Parser::answer_message` was written, which made this the one place the two
    // disagreed about what the language is, and the disagreement was silent in the
    // direction that matters: `invalid err` passed `hek check` while `hek fmt` declined
    // the whole file.
    //
    // It has to stay a string for the formatter's sake as well. Were the message
    // `$._expression`, the parens of a removed `invalid("x")` would read as a grouping
    // and `hek fmt` would rewrite the file to `invalid ("x")`: output of its own that
    // `hek check` rejects, which is the one thing a formatter must never produce. As a
    // string it matches no rule at all, so `fmt` leaves the file for `check` to explain.
    outcome_expression: ($) =>
      prec.right(seq('invalid', field('message', choice($.string, $.raw_string)))),

    fail_statement: ($) =>
      prec.right(seq('fail', field('message', choice($.string, $.raw_string)))),

    // `reject Name`, or `reject Name { field: value }`.
    // `prec.right` because a `{` after the name is always the fields: unlike a record
    // literal, which loses to a block in a header (`no_record_literal` in parse.rs),
    // a refusal is never a header's condition, so there is no block for it to lose to.
    refusal_expression: ($) =>
      prec.right(
        seq(
          'reject',
          field('name', $._type_name),
          optional(field('fields', $.field_initializer_list)),
        ),
      ),

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

    // `log(...)`, `erase(...)` and a discarded `http.*` call. `fail` is no longer one of
    // them: it takes no parens, so it is not a call.
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
      choice('Bool', 'Int', 'String', 'Uuid', 'Timestamp', 'Json', 'Response'),

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

    // An expression rather than the two shapes `@max` and `@subject` take, because
    // `@absent` takes a literal and a literal is as wide as a record holding a list.
    // Widening to `_expression` rather than enumerating the literal rules is the same
    // choice an entity column's `= default` already makes, and it keeps this rule from
    // needing an edit every time a literal gains a spelling. The parser is what decides
    // which of these an annotation actually accepts.
    //
    // `@subject(x)` and `@max(255)` still yield `(identifier)` and `(integer_literal)`:
    // `_expression` is hidden, so it adds no node of its own.
    annotation_arguments: ($) => seq('(', commaSep($._expression), ')'),

    // Unicode, matching `is_alphabetic`/`is_alphanumeric` in lex.rs.
    identifier: (_) => /[_\p{L}][_\p{L}\p{N}]*/,
  },
});
