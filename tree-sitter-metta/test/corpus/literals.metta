==================
Integer literals
==================

42
-17
0

---

(source_file
  (expression
    (atom_expression
      (integer_literal)))
  (expression
    (atom_expression
      (integer_literal)))
  (expression
    (atom_expression
      (integer_literal))))

==================
Float literals
==================

3.14
-2.5
1.0e10
-1.5e-3
2.0E+5

---

(source_file
  (expression
    (atom_expression
      (float_literal)))
  (expression
    (atom_expression
      (float_literal)))
  (expression
    (atom_expression
      (float_literal)))
  (expression
    (atom_expression
      (float_literal)))
  (expression
    (atom_expression
      (float_literal))))

==================
String literals
==================

"hello"
"world\nwith\tescapes"
"path\\to\\file"

---

(source_file
  (expression
    (atom_expression
      (string_literal)))
  (expression
    (atom_expression
      (string_literal)))
  (expression
    (atom_expression
      (string_literal))))

==================
Boolean literals
==================

True
False

---

(source_file
  (expression
    (atom_expression
      (boolean_literal)))
  (expression
    (atom_expression
      (boolean_literal))))

==================
Variables
==================

$x
$variable_name
$x123

---

(source_file
  (expression
    (atom_expression
      (variable)))
  (expression
    (atom_expression
      (variable)))
  (expression
    (atom_expression
      (variable))))

==================
Wildcard
==================

_

---

(source_file
  (expression
    (atom_expression
      (wildcard))))
