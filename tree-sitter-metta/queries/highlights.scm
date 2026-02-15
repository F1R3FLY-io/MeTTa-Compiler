; Syntax highlighting queries for MeTTa

; Comments
(line_comment) @comment

; Literals
(string_literal) @string
(integer_literal) @number
(float_literal) @number.float
(boolean_literal) @boolean

; Special type symbols (%Undefined%, %Irreducible%, etc.)
(special_type_symbol) @type.builtin

; Variables (pattern variables)
(variable) @variable

; Wildcard pattern
(wildcard) @variable.special

; Identifiers (function names, symbols)
(identifier) @function

; Operators
(arrow_operator) @operator
(comparison_operator) @operator
(assignment_operator) @operator
(type_annotation_operator) @operator.type
(rule_definition_operator) @keyword.operator
(arithmetic_operator) @operator
(logic_operator) @operator
(punctuation_operator) @punctuation.delimiter

; Prefixes (special forms)
(exclaim_prefix) @keyword
(question_prefix) @keyword

; Brackets
"(" @punctuation.bracket
")" @punctuation.bracket

; Special keywords in identifier position
((identifier) @keyword
 (#any-of? @keyword "match" "if" "error" "quote" "let" "case"))
