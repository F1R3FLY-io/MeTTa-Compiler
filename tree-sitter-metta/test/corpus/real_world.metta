==================
Factorial function
==================

(: factorial (-> Number Number))
(:= (factorial 0) 1)
(:= (factorial $n)
    (* $n (factorial (- $n 1))))

!(factorial 5)

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
              (identifier)))))))
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
              (integer_literal)))))
      (expression
        (atom_expression
          (integer_literal)))))
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
            (list
              (expression
                (atom_expression
                  (identifier)))
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
                      (integer_literal)))))))))))
  (expression
    (prefixed_expression
      prefix: (exclaim_prefix)
      argument: (expression
        (list
          (expression
            (atom_expression
              (identifier)))
          (expression
            (atom_expression
              (integer_literal))))))))

==================
Pattern matching with match
==================

(leaf1 leaf2)
(leaf0 leaf1)
!(match & self ($x leaf2) $x)

---

(source_file
  (expression
    (list
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
          (identifier)))
      (expression
        (atom_expression
          (identifier)))))
  (expression
    (prefixed_expression
      prefix: (exclaim_prefix)
      argument: (expression
        (list
          (expression
            (atom_expression
              (identifier)))
          (expression
            (atom_expression
              (operator
                (punctuation_operator))))
          (expression
            (atom_expression
              (identifier)))
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
              (variable))))))))

==================
Type annotations and rules
==================

(: Socrates Entity)
(: Mortal (-> Entity Bool))
(:= (Mortal $x) True)

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
              (identifier)))))))
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
        (atom_expression
          (boolean_literal))))))

==================
Complex nested expression
==================

(= (double $x) (* $x 2))
!(double 21)

---

(source_file
  (expression
    (list
      (expression
        (atom_expression
          (operator
            (assignment_operator))))
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
    (prefixed_expression
      prefix: (exclaim_prefix)
      argument: (expression
        (list
          (expression
            (atom_expression
              (identifier)))
          (expression
            (atom_expression
              (integer_literal))))))))
