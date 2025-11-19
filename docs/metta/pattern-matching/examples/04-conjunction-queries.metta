; ============================================================================
; Conjunction Query Examples
; ============================================================================
; Demonstrates conjunction (comma operator) in pattern matching including:
; - Multi-pattern queries
; - Shared variables across patterns
; - Cartesian products
; - Complex relational queries
; - Query optimization patterns

; Setup: Create knowledge base
; ============================================================================

; People
(add-atom &self (Human Socrates))
(add-atom &self (Human Plato))
(add-atom &self (Human Aristotle))
(add-atom &self (Human John))

; Philosophers
(add-atom &self (philosopher Socrates))
(add-atom &self (philosopher Plato))
(add-atom &self (philosopher Aristotle))

; Ages
(add-atom &self (age Socrates 70))
(add-atom &self (age Plato 80))
(add-atom &self (age John 25))

; Example 1: Simple Conjunction
; ============================================================================
; Find humans who are philosophers

!(match &self
    (, (Human $x)
       (philosopher $x))
    $x)
; Process:
; 1. Match (Human $x) → {$x ← Socrates}, {$x ← Plato}, {$x ← Aristotle}, {$x ← John}
; 2. For each, match (philosopher $x):
;    - Socrates: matches (philosopher Socrates) ✓
;    - Plato: matches (philosopher Plato) ✓
;    - Aristotle: matches (philosopher Aristotle) ✓
;    - John: no match ✗
; Expected: [Socrates, Plato, Aristotle]

; Example 2: Three-Way Conjunction
; ============================================================================
; Find human philosophers with known age

!(match &self
    (, (Human $x)
       (philosopher $x)
       (age $x $years))
    ($x aged $years))
; Process:
; 1. Match (Human $x)
; 2. Filter by (philosopher $x)
; 3. Filter by (age $x $years)
; Expected: [(Socrates aged 70),
;           (Plato aged 80)]
; Note: Aristotle excluded (no age), John excluded (not philosopher)

; Example 3: Conjunction with Multiple Variables
; ============================================================================
; Extract multiple related values

(add-atom &self (parent Alice Bob))
(add-atom &self (parent Alice Charlie))
(add-atom &self (parent Bob David))
(add-atom &self (age Bob 30))
(add-atom &self (age Charlie 25))
(add-atom &self (age David 5))

!(match &self
    (, (parent $p $c)
       (age $c $a))
    (child $c of $p is $a years old))
; Expected: [(child Bob of Alice is 30 years old),
;           (child Charlie of Alice is 25 years old),
;           (child David of Bob is 5 years old)]

; Example 4: Cartesian Product
; ============================================================================
; No shared variables = all combinations

(add-atom &self (color red))
(add-atom &self (color blue))
(add-atom &self (size small))
(add-atom &self (size large))

!(match &self
    (, (color $c)
       (size $s))
    (item $c $s))
; Process:
; All combinations of colors and sizes
; 2 colors × 2 sizes = 4 results
; Expected: [(item red small),
;           (item red large),
;           (item blue small),
;           (item blue large)]

; Example 5: Grandparent Query
; ============================================================================
; Transitive relationship using conjunction

(add-atom &self (parent Eve Alice))
(add-atom &self (parent Frank Alice))

!(match &self
    (, (parent $gp $p)
       (parent $p $gc))
    (grandparent $gp of $gc))
; Process:
; Find $gp → $p then $p → $gc
; Expected: [(grandparent Alice of David),
;           (grandparent Eve of Bob),
;           (grandparent Eve of Charlie),
;           (grandparent Frank of Bob),
;           (grandparent Frank of Charlie)]

; Example 6: Symmetric Relationships
; ============================================================================
; Find mutual relationships

(add-atom &self (friend Alice Bob))
(add-atom &self (friend Bob Alice))
(add-atom &self (friend Charlie David))

!(match &self
    (, (friend $a $b)
       (friend $b $a))
    (mutual-friends $a $b))
; Expected: [(mutual-friends Alice Bob),
;           (mutual-friends Bob Alice)]
; Note: Charlie-David excluded (only one direction)

; Example 7: Conjunction with Constraints
; ============================================================================
; Use shared variables to enforce constraints

!(match &self
    (, (parent $p $c1)
       (parent $p $c2))
    (siblings $c1 $c2))
; Constraint: Same parent $p
; Note: This includes ($c1, $c1) pairs!
; Expected: [(siblings Bob Bob),
;           (siblings Bob Charlie),
;           (siblings Charlie Bob),
;           (siblings Charlie Charlie),
;           (siblings David David)]

; To exclude self-pairs, add constraint:
!(match &self
    (, (parent $p $c1)
       (parent $p $c2)
       (!= $c1 $c2))
    (siblings $c1 $c2))
; Expected: [(siblings Bob Charlie),
;           (siblings Charlie Bob)]

; Example 8: Multi-Level Nested Conjunction
; ============================================================================
; Complex query with multiple levels

(add-atom &self (teaches Socrates Philosophy))
(add-atom &self (teaches Plato Philosophy))
(add-atom &self (student Bob Philosophy))
(add-atom &self (student Charlie Philosophy))

!(match &self
    (, (Human $teacher)
       (teaches $teacher $subject)
       (student $student $subject))
    (connection $student can-learn $subject from $teacher))
; Expected: [(connection Bob can-learn Philosophy from Socrates),
;           (connection Bob can-learn Philosophy from Plato),
;           (connection Charlie can-learn Philosophy from Socrates),
;           (connection Charlie can-learn Philosophy from Plato)]

; Example 9: Query Optimization - Order Matters
; ============================================================================
; More specific patterns first improves performance

; Less efficient (general pattern first)
; !(match &self
;     (, (Human $x)           ; Many matches
;        (philosopher $x)      ; Filter after
;        (age $x 70))          ; Very specific
;     $x)

; More efficient (specific pattern first)
!(match &self
    (, (age $x 70)             ; Very specific (few matches)
       (philosopher $x)         ; Medium specific
       (Human $x))             ; General
    $x)
; Both return: [Socrates]
; But second is faster (fewer intermediate bindings)

; Example 10: Empty Conjunction Result
; ============================================================================
; Conjunction that matches nothing

!(match &self
    (, (Human $x)
       (Dog $x))               ; Impossible: can't be both
    $x)
; Expected: []
; No atom satisfies both patterns

; Example 11: Conjunction with Ground Terms
; ============================================================================
; Mix shared variables and constants

!(match &self
    (, (parent Alice $c)       ; Alice's children
       (age $c $a))            ; With known age
    (alice-child $c aged $a))
; Expected: [(alice-child Bob aged 30),
;           (alice-child Charlie aged 25)]

; Example 12: Complex Relationship Query
; ============================================================================
; Find triangular relationships

(add-atom &self (knows Alice Bob))
(add-atom &self (knows Bob Charlie))
(add-atom &self (knows Charlie Alice))

!(match &self
    (, (knows $a $b)
       (knows $b $c)
       (knows $c $a))
    (triangle $a $b $c))
; Expected: [(triangle Alice Bob Charlie)]

; Example 13: Conjunction with Computed Values
; ============================================================================
; Use conjunction to compute derived values

!(match &self
    (, (age $person $years)
       (> $years 50))          ; Condition as pattern
    (senior $person))
; Expected: [(senior Socrates),
;           (senior Plato)]

; Example 14: Multiple Source Patterns
; ============================================================================
; Combine data from different patterns

(add-atom &self (hobby Socrates reading))
(add-atom &self (hobby Plato writing))

!(match &self
    (, (philosopher $p)
       (age $p $a)
       (hobby $p $h))
    (profile $p philosopher age $a hobby $h))
; Expected: [(profile Socrates philosopher age 70 hobby reading),
;           (profile Plato philosopher age 80 hobby writing)]

; Example 15: Nested Match in Conjunction
; ============================================================================
; Conjunction with nested match in template

!(match &self
    (, (Human $x)
       (philosopher $x))
    (details $x
        (match &self (age $x $years) $years)))
; Inner match uses $x from outer conjunction
; Expected: [(details Socrates [70]),
;           (details Plato [80]),
;           (details Aristotle [])]

; ============================================================================
; Summary
; ============================================================================
; Conjunction queries demonstrated:
; - Multi-pattern matching with comma operator
; - Shared variables create constraints
; - Independent variables create Cartesian products
; - Transitive relationships (grandparent, etc.)
; - Symmetric relationship queries
; - Query optimization via pattern ordering
; - Empty results from impossible constraints
; - Complex multi-source queries
; - Computed and conditional patterns
