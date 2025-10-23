; Indentation queries for MeTTa
; Tree-Sitter uses these to compute automatic indentation in editors

; Indent after opening parentheses and braces
(list "(" @indent)
(brace_list "{" @indent)

; Dedent before closing brackets
")" @dedent
"}" @dedent

; Prefixed expressions should indent their arguments
(prefixed_expression
  prefix: _ @indent
  argument: _)

; Align expressions within lists
(list
  (expression) @branch)

(brace_list
  (expression) @branch)

; Comments don't affect indentation
(line_comment) @ignore
(block_comment) @ignore
