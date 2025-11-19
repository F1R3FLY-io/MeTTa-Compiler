; Indentation queries for MeTTa
; Tree-Sitter uses these to compute automatic indentation in editors

; Indent after opening parentheses
(list "(" @indent)

; Dedent before closing bracket
")" @dedent

; Prefixed expressions should indent their arguments
(prefixed_expression
  prefix: _ @indent
  argument: _)

; Align expressions within lists
(list
  (expression) @branch)

; Comments don't affect indentation
(line_comment) @ignore
