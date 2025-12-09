==================
Type annotation operator
==================

(: x Number)
(: Socrates Entity)

---

(source_file
  (expression
    (list
      (expression
        (atom_expression
          (operator
            (type_annotation_operator))))
      (expression
        (atom_expression
          (identifier)))
      (expression
        (atom_expression
          (identifier)))))
  (expression
    (list
      (expression
        (atom_expression
          (operator
            (type_annotation_operator))))
      (expression
        (atom_expression
          (identifier)))
      (expression
        (atom_expression
          (identifier))))))

==================
Rule definition operator
==================

(:= (double $x) (* $x 2))
(:= (Add $x Z) $x)

---

(source_file
  (expression
    (list
      (expression
        (atom_expression
          (operator
            (rule_definition_operator))))
      (expression
        (list
          (expression
            (atom_expression
              (identifier)))
          (expression
            (atom_expression
              (variable)))))
      (expression
        (list
          (expression
            (atom_expression
              (operator
                (arithmetic_operator))))
          (expression
            (atom_expression
              (variable)))
          (expression
            (atom_expression
              (integer_literal)))))))
  (expression
    (list
      (expression
        (atom_expression
          (operator
            (rule_definition_operator))))
      (expression
        (list
          (expression
            (atom_expression
              (identifier)))
          (expression
            (atom_expression
              (variable)))
          (expression
            (atom_expression
              (identifier)))))
      (expression
        (atom_expression
          (variable))))))

==================
Arithmetic operators
==================

(+ 1 2)
(- 5 3)
(* 4 6)
(/ 10 2)

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

==================
Comparison operators
==================

(> 5 3)
(< 2 4)
(== 1 1)

---

(source_file
  (expression
    (list
      (expression
        (atom_expression
          (operator
            (comparison_operator))))
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
            (comparison_operator))))
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
            (comparison_operator))))
      (expression
        (atom_expression
          (integer_literal)))
      (expression
        (atom_expression
          (integer_literal))))))

==================
Arrow operators
==================

(-> Type Result)
(<- Input Output)

---

(source_file
  (expression
    (list
      (expression
        (atom_expression
          (operator
            (arrow_operator))))
      (expression
        (atom_expression
          (identifier)))
      (expression
        (atom_expression
          (identifier)))))
  (expression
    (list
      (expression
        (atom_expression
          (operator
            (arrow_operator))))
      (expression
        (atom_expression
          (identifier)))
      (expression
        (atom_expression
          (identifier))))))

==================
Space reference operator
==================

(match &self ($x leaf2) $x)

---

(source_file
  (expression
    (list
      (expression
        (atom_expression
          (identifier)))
      (expression
        (atom_expression
          (space_reference)))
      (expression
        (list
          (expression
            (atom_expression
              (variable)))
          (expression
            (atom_expression
              (identifier)))))
      (expression
        (atom_expression
          (variable))))))
