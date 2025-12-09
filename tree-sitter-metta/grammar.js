/// Tree-Sitter grammar for MeTTa language
/// Decomposes atoms into semantic types for precise LSP support
module.exports = grammar({
  name: 'metta',

  extras: $ => [
    /\s/,
    $.line_comment,
  ],

  rules: {
    source_file: $ => repeat($.expression),

    expression: $ => choice(
      $.list,
      $.prefixed_expression,
      $.atom_expression,
    ),

    // Lists: (expr expr ...)
    list: $ => seq(
      '(',
      repeat($.expression),
      ')'
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
    // Order matters: more specific patterns first
    atom_expression: $ => choice(
      $.variable,
      $.space_reference,
      $.wildcard,
      $.boolean_literal,  // Must come before identifier
      $.special_type_symbol,  // Must come before operator (contains %)
      $.operator,
      $.string_literal,
      $.float_literal,
      $.integer_literal,
      $.identifier,
    ),

    // Variables: $var (for pattern variables)
    // Supports unicode: letters, numbers, symbols, marks, and most punctuation
    // Excludes: ()[]{}; which have special meaning in MeTTa
    variable: $ => token(
      seq('$', /[^\s()\[\]{};]*/u)
    ),

    // Space references: &name (for referencing spaces)
    // Examples: &self, &kb, &workspace
    // Supports unicode just like variables
    space_reference: $ => token(
      seq('&', /[^\s()\[\]{};]*/u)
    ),

    // Wildcard pattern
    wildcard: $ => '_',

    // Boolean literals (higher precedence than identifier)
    boolean_literal: $ => token(prec(11, choice('True', 'False'))),

    // Special type symbols: %Undefined%, %Irreducible%, etc.
    // Used in official MeTTa stdlib for special type markers
    special_type_symbol: $ => token(prec(11, /%[A-Za-z][A-Za-z0-9_-]*%/)),

    // Regular identifiers (no special prefix)
    // Supports unicode - any non-whitespace, non-special character
    // Must NOT start with special tokens or operators (handled separately)
    // Excludes: ()[]{}; whitespace, and all operator/special chars
    // High precedence (10) ensures identifiers like println! are captured as single
    // tokens before the ! can be claimed by exclaim_prefix for prefixed_expression
    identifier: $ => token(prec(10,
      // Identifiers starting with letters or most unicode (not operators/special)
      // Excluded from start: $ ! ? ' " & digits + - * / _ : = > < | , @ .
      // Continuation allows ! so println!, import!, bind-space! work as single tokens
      /[^\s()\[\]{};$!?'"&\d+\-*/_:=><|,@.][^\s()\[\]{};]*/u,
    )),

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

    // Arrow operators: ->, <-, <=, <<- (higher precedence than single-char operators)
    arrow_operator: $ => token(prec(2, choice(
      '->',
      '<-',
      '<=',
      '<<-',
    ))),

    // Comparison operators: ==, >=, >, < (multi-char needs higher precedence than =)
    comparison_operator: $ => token(prec(2, choice(
      '==',
      '>=',
      '>',
      '<',
    ))),

    // Assignment operator: =
    assignment_operator: $ => '=',

    // Type annotation operator: :
    type_annotation_operator: $ => ':',

    // Rule definition operator: :=
    rule_definition_operator: $ => ':=',

    // Punctuation operators: ;, |, ,, @, &, ., ...
    // Note: : is now separate as type_annotation_operator
    // Note: % removed - now used only in special_type_symbol
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
    // Supports: \n, \t, \r, \\, \", \x##, \u{...}
    string_literal: $ => token(seq(
      '"',
      repeat(choice(
        /[^"\\]/,
        seq('\\', choice(
          'n',   // \n - newline
          't',   // \t - tab
          'r',   // \r - carriage return
          '\\',  // \\ - backslash
          '"',   // \" - quote
          seq('x', /[0-9a-fA-F]{2}/),  // \x## - hex escape (e.g., \x1b)
          seq('u', '{', /[0-9a-fA-F]{1,6}/, '}'),  // \u{...} - unicode escape (e.g., \u{1F4A1})
          /./,   // any other escaped char (fallback)
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
    // Official MeTTa uses only semicolon comments
    line_comment: $ => token(prec(10, seq(';', /[^\n]*/))),
  }
});
