==================
Line comment with semicolon
==================

; This is a comment
(+ 1 2)

---

(source_file
  (line_comment)
  (expression
    (list
      (expression
        (atom_expression
          (operator
            (arithmetic_operator))))
      (expression
        (atom_expression
          (integer_literal)))
      (expression
        (atom_expression
          (integer_literal))))))

==================
Multiple line comments
==================

; First comment
; Second comment
; Third comment
(+ 1 2)

---

(source_file
  (line_comment)
  (line_comment)
  (line_comment)
  (expression
    (list
      (expression
        (atom_expression
          (operator
            (arithmetic_operator))))
      (expression
        (atom_expression
          (integer_literal)))
      (expression
        (atom_expression
          (integer_literal))))))

==================
Inline comments
==================

(+ 1 ; comment
   2)

---

(source_file
  (expression
    (list
      (expression
        (atom_expression
          (operator
            (arithmetic_operator))))
      (expression
        (atom_expression
          (integer_literal)))
      (line_comment)
      (expression
        (atom_expression
          (integer_literal))))))
