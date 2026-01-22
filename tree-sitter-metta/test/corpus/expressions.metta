==================
Simple list
==================

(+ 1 2)

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
      (expression
        (atom_expression
          (integer_literal))))))

==================
Nested lists
==================

(+ (* 2 3) 4)

---

(source_file
  (expression
    (list
      (expression
        (atom_expression
          (operator
            (arithmetic_operator))))
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
              (integer_literal)))))
      (expression
        (atom_expression
          (integer_literal))))))

==================
Empty list
==================

()

---

(source_file
  (expression
    (list)))

==================
Prefixed expression with exclaim
==================

!(+ 1 2)

---

(source_file
  (expression
    (prefixed_expression
      prefix: (exclaim_prefix)
      argument: (expression
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
              (integer_literal))))))))

==================
Prefixed expression with question
==================

?query

---

(source_file
  (expression
    (prefixed_expression
      prefix: (question_prefix)
      argument: (expression
        (atom_expression
          (identifier))))))

==================
Prefixed expression with quote (using quote function)
==================

(quote x)

---

(source_file
  (expression
    (list
      (expression
        (atom_expression
          (identifier)))
      (expression
        (atom_expression
          (identifier))))))

==================
Multiple expressions
==================

(+ 1 2)
(* 3 4)

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
      (expression
        (atom_expression
          (integer_literal)))))
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
