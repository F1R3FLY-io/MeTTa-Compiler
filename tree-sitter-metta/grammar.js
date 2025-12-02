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
      $.wildcard,
      $.boolean_literal,  // Must come before identifier
      $.special_type_symbol,  // Must come before operator (contains %)
      $.space_reference,  // Must come before identifier (starts with &)
      $.operator,
      $.string_literal,
      $.float_literal,
      $.integer_literal,
      $.identifier,
    ),

    // Variables: $var (for pattern variables)
    // Uses blacklist approach: $ followed by any non-delimiter chars
    // Delimiters: whitespace, (), ;, ", #
    // Note: # is reserved for internal use per official MeTTa spec
    variable: $ => token(
      seq('$', /[^\s()";#]*/)
    ),

    // Wildcard pattern
    wildcard: $ => '_',

    // Space references: &name (HE-compatible single-token approach)
    // Used for referencing spaces like &self, &kb, etc.
    // High precedence to match before identifier
    // Uses same delimiter rules as variable: whitespace, (), ;, ", #
    space_reference: $ => token(prec(5, seq('&', /[^\s()";#]+/))),

    // Boolean literals (high precedence to match before identifier)
    boolean_literal: $ => token(prec(5, choice('True', 'False'))),

    // Special type symbols: %Undefined%, %Irreducible%, etc.
    // Used in official MeTTa stdlib for special type markers
    // High precedence to match before identifier
    special_type_symbol: $ => token(prec(5, /%[A-Za-z][A-Za-z0-9_-]*%/)),

    // Regular identifiers (no special prefix)
    // Uses blacklist approach: any sequence of non-delimiter characters
    // Delimiters: whitespace, (), ;, ", $
    // Also excludes: !, ?, ' (prefix operators), & (space reference), _ (wildcard), [], {} (reserved)
    // Lower precedence (1) so specific tokens match first
    identifier: $ => token(prec(1, /[^\s()\[\]{}"$;!?'_&][^\s()\[\]{}"$;]*/)),

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

    // Arrow operators: ->, <-, <<- (higher precedence than comparison operators)
    // Note: <= moved to comparison_operator for consistent handling
    // prec(4) to ensure <- and <<- match before < is matched as comparison
    arrow_operator: $ => token(prec(4, choice(
      '->',
      '<-',
      '<<-',
    ))),

    // Comparison operators: ==, !=, <=, >=, >, <
    // Use prec(3) to ensure multi-char operators match before single-char ones
    comparison_operator: $ => token(prec(3, choice(
      '==',
      '!=',
      '<=',
      '>=',
      '>',
      '<',
    ))),

    // Assignment operator: =
    assignment_operator: $ => token(prec(2, '=')),

    // Type annotation operator: :
    type_annotation_operator: $ => token(prec(2, ':')),

    // Rule definition operator: :=
    rule_definition_operator: $ => token(prec(3, ':=')),

    // Punctuation operators: ;, |, ,, @, ., ...
    // Note: : is now separate as type_annotation_operator
    // Note: % removed - now used only in special_type_symbol
    // Note: & removed - now used only in space_reference token
    punctuation_operator: $ => token(prec(2, choice(
      ';',
      '|',
      ',',
      '@',
      '...',
      '.',
    ))),

    // Arithmetic operators (as standalone symbols): +, -, *, /
    arithmetic_operator: $ => token(prec(2, /[+\-*/]/)),

    // Logic operators: !?, ?!
    logic_operator: $ => token(prec(2, choice(
      '!?',
      '?!',
    ))),

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
