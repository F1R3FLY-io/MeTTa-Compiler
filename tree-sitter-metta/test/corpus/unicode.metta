==================
Unicode identifiers - angle brackets
==================

⟨0⟩
⟮test⟯

---

(source_file
  (expression
    (atom_expression
      (identifier)))
  (expression
    (atom_expression
      (identifier))))

==================
Unicode identifiers - Cyrillic
==================

ж
привет

---

(source_file
  (expression
    (atom_expression
      (identifier)))
  (expression
    (atom_expression
      (identifier))))

==================
Unicode identifiers - Greek letters
==================

λ
∀x
∃y

---

(source_file
  (expression
    (atom_expression
      (identifier)))
  (expression
    (atom_expression
      (identifier)))
  (expression
    (atom_expression
      (identifier))))

==================
Unicode identifiers - mathematical symbols
==================

→
∧
∨
¬

---

(source_file
  (expression
    (atom_expression
      (identifier)))
  (expression
    (atom_expression
      (identifier)))
  (expression
    (atom_expression
      (identifier)))
  (expression
    (atom_expression
      (identifier))))

==================
Unicode variables
==================

$λ
$∀

---

(source_file
  (expression
    (atom_expression
      (variable)))
  (expression
    (atom_expression
      (variable))))

==================
Unicode in expressions (issue #30 example)
==================

(: ⟨0⟩ Const)

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
          (identifier))))))

==================
Special type symbols with percent
==================

%Undefined%
%Irreducible%

---

(source_file
  (expression
    (atom_expression
      (special_type_symbol)))
  (expression
    (atom_expression
      (special_type_symbol))))
