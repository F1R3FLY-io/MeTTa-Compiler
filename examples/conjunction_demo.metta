; MeTTa Conjunction Pattern Demonstration
; This file demonstrates the MORK-style conjunction operator (,)
;
; Conjunctions are logical AND operations that evaluate goals left-to-right
; with variable binding threading through each goal.

; ============================================================================
; 1. Empty Conjunction - Always succeeds with Nil
; ============================================================================
; (,) → Nil
!(,)

; ============================================================================
; 2. Unary Conjunction - Direct evaluation (passthrough)
; ============================================================================
; (, expr) → evaluates expr directly
!(, (+ 5 3))
!(, 42)

; ============================================================================
; 3. Binary Conjunction - Sequential evaluation
; ============================================================================
; (, expr1 expr2) → evaluate expr1, then expr2
!(, (+ 2 3) (* 4 5))

; ============================================================================
; 4. N-ary Conjunction - Left-to-right evaluation
; ============================================================================
; (, e1 e2 e3 ... en) → evaluate each goal in sequence
!(, (+ 1 1) (+ 2 2) (+ 3 3) (+ 4 4))

; ============================================================================
; 5. Conjunction with Rules (when pattern matching is supported)
; ============================================================================
; Define some simple rules
(= (double $x) (* $x 2))
(= (triple $x) (* $x 3))

; Use conjunction to chain rule applications
!(, (double 5) (triple 4))

; ============================================================================
; 6. Nested Conjunctions
; ============================================================================
; Conjunctions can be nested
!(, (+ 1 2) (, (+ 3 4) (+ 5 6)))

; ============================================================================
; 7. Empty Goals in Conjunction
; ============================================================================
; Empty conjunction as a goal
!(, (+ 1 2) (,) (+ 3 4))
