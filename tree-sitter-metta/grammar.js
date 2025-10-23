/// Tree-Sitter grammar for MeTTa language
/// Decomposes atoms into semantic types for precise LSP support
module.exports = grammar({
  name: 'metta',

  extras: $ => [
    /\s/,
    $.line_comment,
    $.block_comment,
  ],

  rules: {
    source_file: $ => repeat($.expression),

    expression: $ => choice(
      $.list,
      $.brace_list,
      $.prefixed_expression,
      $.atom_expression,
    ),

    // Lists: (expr expr ...)
    list: $ => seq(
      '(',
      repeat($.expression),
      ')'
    ),

    // Brace lists: {expr expr ...}
    brace_list: $ => seq(
      '{',
      repeat($.expression),
      '}'
    ),

    // Prefixed expressions: !expr, ?expr, 'expr
    prefixed_expression: $ => seq(
      field('prefix', choice(
        $.exclaim_prefix,
        $.question_prefix,
        $.quote_prefix,
      )),
      field('argument', $.expression)
    ),

    exclaim_prefix: $ => '!',
    question_prefix: $ => '?',
    quote_prefix: $ => '\'',

    // Atomic expressions (decomposed by semantic type)
    atom_expression: $ => choice(
      $.variable,
      $.wildcard,
      $.identifier,
      $.operator,
      $.string_literal,
      $.float_literal,
      $.integer_literal,
      $.boolean_literal,
    ),

    // Variables: $var (for pattern variables)
    // Note: & is an operator (space reference), not a variable prefix
    // Note: 'var is handled by quote_prefix in prefixed_expression
    variable: $ => token(
      seq('$', /[a-zA-Z0-9_'\-+*/&]*/)
    ),

    // Wildcard pattern
    wildcard: $ => '_',

    // Boolean literals
    boolean_literal: $ => choice('True', 'False'),

    // Regular identifiers (no special prefix)
    identifier: $ => token(prec(2, choice(
      // Standard identifiers: letters, digits, allowed special chars
      /[a-zA-Z][a-zA-Z0-9_'\-+*/]*/,
      // Can start with some operators if followed by alphanumeric
      /[+\-*/][a-zA-Z0-9_'\-+*/]+/,
    ))),

    // Operators (decomposed by type)
    operator: $ => choice(
      $.arrow_operator,
      $.comparison_operator,
      $.assignment_operator,
      $.type_annotation_operator,
      $.rule_definition_operator,
      $.punctuation_operator,
      $.arithmetic_operator,
      $.logic_operator,
    ),

    // Arrow operators: ->, <-, <=, <<-
    arrow_operator: $ => token(choice(
      '->',
      '<-',
      '<=',
      '<<-',
    )),

    // Comparison operators: ==, >, <
    comparison_operator: $ => token(choice(
      '==',
      '>',
      '<',
    )),

    // Assignment operator: =
    assignment_operator: $ => '=',

    // Type annotation operator: :
    type_annotation_operator: $ => ':',

    // Rule definition operator: :=
    rule_definition_operator: $ => ':=',

    // Punctuation operators: ;, |, ,, @, &, ., ...
    // Note: : is now separate as type_annotation_operator
    punctuation_operator: $ => token(choice(
      ';',
      '|',
      ',',
      '@',
      '&',
      '...',
      '.',
    )),

    // Arithmetic operators (as standalone symbols): +, -, *, /
    arithmetic_operator: $ => token(prec(1, /[+\-*/]/)),

    // Logic operators: !?, ?!
    logic_operator: $ => token(choice(
      '!?',
      '?!',
    )),

    // String literals with escape sequences
    string_literal: $ => token(seq(
      '"',
      repeat(choice(
        /[^"\\]/,
        seq('\\', choice(
          'n',   // \n
          't',   // \t
          'r',   // \r
          '\\',  // \\
          '"',   // \"
          /./,   // any other escaped char
        ))
      )),
      '"'
    )),

    // Float literals (with optional minus) - highest precedence to match before integer
    // Supports: 3.14, -2.5, 1.0e10, -1.5e-3, 2.0E+5
    float_literal: $ => token(prec(4, seq(
      optional('-'),
      /\d+/,
      '.',
      /\d+/,
      optional(seq(/[eE]/, optional(/[+-]/), /\d+/))
    ))),

    // Integer literals (with optional minus) - high precedence to match before identifier
    integer_literal: $ => token(prec(3, seq(
      optional('-'),
      /\d+/
    ))),

    // Comments - high precedence to match before operators
    line_comment: $ => token(prec(10, choice(
      seq(';', /[^\n]*/),
      seq('//', /[^\n]*/),
    ))),

    block_comment: $ => token(prec(10, seq(
      '/*',
      /([^*]|\*[^/])*/,
      '*/'
    ))),
  }
});
