; ============================================================================
; Variable Bindings Examples
; ============================================================================
; Demonstrates variable bindings in pattern matching including:
; - Single variable bindings
; - Multiple variable bindings
; - Shared variable constraints
; - Variable equality
; - Binding resolution

; Setup: Create knowledge base
; ============================================================================

(add-atom &self (value A 1))
(add-atom &self (value B 2))
(add-atom &self (value C 3))
(add-atom &self (pair A B))
(add-atom &self (pair B C))
(add-atom &self (same X X))
(add-atom &self (same Y Y))
(add-atom &self (diff X Y))

; Example 1: Single Variable Binding
; ============================================================================
; Variable binds to single value per match

!(match &self (value $x 1) $x)
; Bindings: {$x ← A}
; Expected: [A]

; Example 2: Multiple Variable Bindings
; ============================================================================
; Multiple variables bound in single pattern

!(match &self (value $name $num) (binding $name to $num))
; Bindings: [{$name ← A, $num ← 1},
;           {$name ← B, $num ← 2},
;           {$name ← C, $num ← 3}]
; Expected: [(binding A to 1),
;           (binding B to 2),
;           (binding C to 3)]

; Example 3: Shared Variable Constraint
; ============================================================================
; Same variable appears multiple times

!(match &self (same $x $x) $x)
; Pattern requires both positions equal
; Bindings: [{$x ← X}, {$x ← Y}]
; Expected: [X, Y]
; Note: (diff X Y) does NOT match (X ≠ Y)

; Example 4: Variable Reuse Across Expressions
; ============================================================================
; Variable from pattern used in template

!(match &self (pair $a $b) (first $a second $b))
; Bindings: [{$a ← A, $b ← B},
;           {$a ← B, $b ← C}]
; Expected: [(first A second B),
;           (first B second C)]

; Example 5: Binding with Ground Terms
; ============================================================================
; Mix of variables and constants

!(match &self (value B $n) (b-has-value $n))
; Pattern: (value B $n)
; Ground term "B" must match exactly
; Variable $n binds to second position
; Bindings: {$n ← 2}
; Expected: [(b-has-value 2)]

; Example 6: Complex Binding Pattern
; ============================================================================
; Nested structure with multiple bindings

(add-atom &self (record (id 1) (data (x 10) (y 20))))
(add-atom &self (record (id 2) (data (x 15) (y 25))))

!(match &self
    (record (id $i) (data (x $x_val) (y $y_val)))
    (entry $i coords $x_val $y_val))
; Bindings: [{$i ← 1, $x_val ← 10, $y_val ← 20},
;           {$i ← 2, $x_val ← 15, $y_val ← 25}]
; Expected: [(entry 1 coords 10 20),
;           (entry 2 coords 15 25)]

; Example 7: Binding with Unify
; ============================================================================
; Using unify operation to test bindings

!(unify (value A $x) (value A 1)
    (bound-to $x)     ; Then branch - $x bound
    (no-match))       ; Else branch
; Bindings: {$x ← 1}
; Expected: (bound-to 1)

!(unify (value A $x) (value B 1)
    (bound-to $x)     ; Then branch
    (no-match))       ; Else branch - taken
; Bindings: empty (A ≠ B, no match)
; Expected: (no-match)

; Example 8: Transitive Binding
; ============================================================================
; Variable chains through equality

; Note: This demonstrates concept, actual behavior depends on implementation
!(unify ($a $b) ($x $y)
    (equal $a and $x also $b and $y)
    (no-match))
; Bindings: {$a = $x, $b = $y}
; Variables equal but not bound to concrete values
; Expected: (equal $a and $x also $b and $y)

; Example 9: Binding Consistency
; ============================================================================
; Same variable must bind consistently

(add-atom &self (edge A B))
(add-atom &self (edge B C))
(add-atom &self (edge A A))  ; Self-loop

; Find self-loops
!(match &self (edge $x $x) (self-loop $x))
; Pattern requires $x = first AND $x = second
; Bindings: {$x ← A}
; Expected: [(self-loop A)]

; Example 10: Multiple Bindings in Template
; ============================================================================
; All bound variables available in template

!(match &self (value $v $n) (value is $n symbol is $v together $v-$n))
; Expected: [(value is 1 symbol is A together A-1),
;           (value is 2 symbol is B together B-2),
;           (value is 3 symbol is C together C-3)]

; Example 11: Binding Scope
; ============================================================================
; Variables scoped to their expression

; Different $x in each match
!(match &self (value $x 1) $x)  ; $x#0
!(match &self (pair $x $y) $x)  ; $x#1 (different scope!)
; Each match has its own $x
; Expected from first: [A]
; Expected from second: [A, B]

; Example 12: Partial Binding
; ============================================================================
; Some variables bound, others remain

(add-atom &self (relation person1 knows person2))
(add-atom &self (relation person2 knows person3))

!(match &self (relation $x knows $y) (relationship $x → $y))
; Bindings: [{$x ← person1, $y ← person2},
;           {$x ← person2, $y ← person3}]
; Expected: [(relationship person1 → person2),
;           (relationship person2 → person3)]

; Example 13: Empty Binding
; ============================================================================
; Pattern with no variables

!(match &self (same X X) matched)
; Bindings: {} (empty - no variables to bind)
; Expected: [matched]

; Example 14: Wildcard Bindings
; ============================================================================
; Variables bound but not used

!(match &self (value $v $_) $v)
; $_ bound but ignored in template
; Bindings: [{$v ← A, $_ ← 1},
;           {$v ← B, $_ ← 2},
;           {$v ← C, $_ ← 3}]
; Expected: [A, B, C]

; Example 15: Binding Resolution in Nested Match
; ============================================================================
; Inner match uses bindings from outer match

!(match &self (pair $a $b)
    (match &self (value $b $n)
        (pair $a has-second $b with-value $n)))
; Outer bindings: {$a, $b}
; Inner match uses $b from outer binding
; Expected: [(pair A has-second B with-value 2),
;           (pair B has-second C with-value 3)]

; ============================================================================
; Summary
; ============================================================================
; Variable bindings demonstrated:
; - Single and multiple variable bindings
; - Shared variables create equality constraints
; - Variables can appear multiple times in pattern and template
; - Each match creates new binding set
; - Variables scoped to their expression
; - Binding consistency enforced (same variable = same value)
; - Empty bindings are valid (no variables)
; - Nested matches can use outer bindings
