; Example: Pattern Matching Order
; Demonstrates pattern matching and query ordering

; Create a space with some atoms
!(bind! &space (new-space))
!(add-atom &space (foo A))
!(add-atom &space (foo B))
!(add-atom &space (foo C))
!(add-atom &space (bar X))
!(add-atom &space (bar Y))

; Match all foo atoms
; Order of results is implementation-dependent (not specified)
!(match &space (foo $x) $x)
; Expected result: {A, B, C} (order unspecified)

; Match all bar atoms
!(match &space (bar $x) $x)
; Expected result: {X, Y} (order unspecified)

; Nested pattern matching
!(add-atom &space (outer (inner 1) X))
!(add-atom &space (outer (inner 2) Y))
!(add-atom &space (outer (other 3) Z))

; Match only outer/inner patterns
!(match &space (outer (inner $x) $y) (pair $x $y))
; Expected result: {(pair 1 X), (pair 2 Y)}
; Note: (outer (other 3) Z) does NOT match

; Multiple variables in pattern
!(add-atom &space (edge A B))
!(add-atom &space (edge B C))
!(add-atom &space (edge C D))

!(match &space (edge $x $y) (path $x $y))
; Expected result: {(path A B), (path B C), (path C D)}

; Pattern with same variable twice (must match same value)
!(add-atom &space (same A A))
!(add-atom &space (same B B))
!(add-atom &space (same C D))

; This should only match atoms where both elements are equal
!(match &space (same $x $x) (equal $x))
; Expected result: {(equal A), (equal B)}
; Note: (same C D) does NOT match because C ≠ D

; Using unify directly
; Unify succeeds
!(unify (foo A) (foo $x) (success $x) failure)
; Expected result: (success A)

; Unify fails (structure mismatch)
!(unify (foo A) (bar $x) success failure)
; Expected result: failure

; Unify fails (inconsistent bindings)
!(unify (pair A B) (pair $x $x) success failure)
; Expected result: failure (cannot bind $x to both A and B)
