; Peano Number Demonstration
; This example shows Peano number support in MeTTaTron.
;
; Feature: PR #2 - peano-numbers
; Peano numbers represent natural numbers as:
; - Z (zero)
; - (S Z) (one - successor of zero)
; - (S (S Z)) (two - successor of successor of zero)
; - etc.

; ============================================================================
; Part 1: Basic Peano Literals
; ============================================================================

; Define some facts with Peano numbers
(number Z)
(number (S Z))
(number (S (S Z)))
(number (S (S (S Z))))

; Query all numbers
; Expected: Z, (S Z), (S (S Z)), (S (S (S Z)))
!(match &self (number $n) $n)

; ============================================================================
; Part 2: Pattern Matching with Peano Numbers
; ============================================================================

; Define generation facts (as in ancestor.mm2)
(generation Z Alice Bob)
(generation (S Z) Bob Carol)
(generation (S (S Z)) Carol Dave)

; Query for generation zero (direct ancestors)
; Expected: (generation Z Alice Bob)
!(match &self (generation Z $c $p) (generation Z $c $p))

; Query for all generations
; Expected: All three generation facts
!(match &self (generation $n $c $p) (generation $n $c $p))

; Query generations with pattern matching on Peano structure
; Match any successor: (S $x)
; Expected: (generation (S Z) Bob Carol), (generation (S (S Z)) Carol Dave)
!(match &self (generation (S $x) $c $p) (generation (S $x) $c $p))

; ============================================================================
; Part 3: Peano Numbers in Rules
; ============================================================================

; Define a rule to get the next Peano number
(= (next Z) (S Z))
(= (next (S $n)) (S (S $n)))

; Test the next rule
!(next Z)                  ; Expected: (S Z)
!(next (S Z))             ; Expected: (S (S Z))
!(next (S (S Z)))         ; Expected: (S (S (S Z)))

; ============================================================================
; Part 4: Peano Pattern Destructuring
; ============================================================================

; Define counted items
(count apples Z)
(count oranges (S Z))
(count bananas (S (S Z)))
(count grapes (S (S (S Z))))

; Get the count (Peano number) for each item
!(match &self (count $item $n) (item $item count $n))

; Get items with non-zero count (destructure successor pattern)
!(match &self (count $item (S $pred)) (item $item has (S $pred)))

; ============================================================================
; Part 5: Considered Incest Example (from ancestor.mm2 line 46)
; ============================================================================

; Define which generation distances are considered incest
(considered_incest (S Z))
(considered_incest (S (S Z)))
(considered_incest (S (S (S Z))))

; Query all considered incest distances
!(match &self (considered_incest $x) $x)

; Check if a specific distance is considered incest
!(match &self (considered_incest (S (S Z))) True)  ; Expected: True (2nd generation)

; ============================================================================
; Part 6: Peano Arithmetic Rules
; ============================================================================

; Define addition for Peano numbers
(= (add Z $n) $n)
(= (add (S $m) $n) (S (add $m $n)))

; Test addition
!(add Z (S Z))                    ; 0 + 1 = 1
!(add (S Z) (S Z))               ; 1 + 1 = 2
!(add (S Z) (S (S Z)))           ; 1 + 2 = 3
!(add (S (S Z)) (S (S Z)))       ; 2 + 2 = 4

; ============================================================================
; Part 7: Converting Between Peano and Integers
; ============================================================================

; Rules to convert Peano to integer representation
(= (to-int Z) 0)
(= (to-int (S $n)) (+ 1 (to-int $n)))

; Rules to convert integer to Peano (limited to small numbers)
(= (to-peano 0) Z)
(= (to-peano 1) (S Z))
(= (to-peano 2) (S (S Z)))
(= (to-peano 3) (S (S (S Z))))

; Test conversions
!(to-int Z)                      ; Expected: 0
!(to-int (S Z))                  ; Expected: 1
!(to-int (S (S Z)))             ; Expected: 2
!(to-int (S (S (S Z))))         ; Expected: 3

!(to-peano 0)                    ; Expected: Z
!(to-peano 1)                    ; Expected: (S Z)
!(to-peano 2)                    ; Expected: (S (S Z))

; ============================================================================
; Summary
; ============================================================================
; Peano numbers are fully supported in MeTTaTron:
; - Pattern matching: Exact matches and variable binding
; - Destructuring: (S $x) patterns extract predecessors
; - Rules: Define recursive operations on Peano numbers
; - Space operations: Add, remove, and query Peano facts
; - Integration: Works with MORK special forms (exec, match, etc.)
;
; This enables the full ancestor.mm2 example which uses Peano numbers
; for tracking generation distances in family trees.
