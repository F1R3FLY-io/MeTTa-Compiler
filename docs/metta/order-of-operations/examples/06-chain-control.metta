; Example: Chain and Evaluation Control
; Demonstrates using chain to control evaluation order

; Define a function that expects an evaluated number
(= (square $x) (* $x $x))

; Normal order: argument not evaluated
; If square is defined to pattern match on the structure,
; this might not work as expected
!(square (+ 2 3))
; Depends on whether + is eagerly evaluated
; In minimal MeTTa with normal order, this passes (+ 2 3) to square

; With chain: force evaluation first
!(chain (+ 2 3) $x (square $x))
; Expected result: 25
; Evaluation: (+ 2 3) → 5, then (square 5) → 25

; Multiple chains in sequence
!(chain (+ 1 2) $a
     (chain (* $a 3) $b
          (chain (+ $b 10) $c
               $c)))
; Expected result: 19
; Step by step:
;   (+ 1 2) → 3, bind to $a
;   (* 3 3) → 9, bind to $b
;   (+ 9 10) → 19, bind to $c
;   return $c = 19

; Chain with non-deterministic expressions
(= (value) 1)
(= (value) 2)
(= (value) 3)

!(chain (value) $x (* $x 10))
; Expected result: {10, 20, 30}
; Each alternative from (value) flows through the chain:
;   Branch 1: value → 1, (* 1 10) → 10
;   Branch 2: value → 2, (* 2 10) → 20
;   Branch 3: value → 3, (* 3 10) → 30

; Nested chains with non-determinism
(= (first-val) 1)
(= (first-val) 2)
(= (second-val) 10)
(= (second-val) 20)

!(chain (first-val) $x
     (chain (second-val) $y
          (+ $x $y)))
; Expected result: {11, 21, 12, 22}
; Cartesian product of alternatives:
;   (first-val → 1, second-val → 10) → 11
;   (first-val → 1, second-val → 20) → 21
;   (first-val → 2, second-val → 10) → 12
;   (first-val → 2, second-val → 20) → 22

; Using let for binding (similar to chain)
!(let $x (+ 1 2)
     (let $y (* $x 3)
          (+ $y 10)))
; Expected result: 19
; Similar to chained evaluation

; Let with non-deterministic expressions
!(let $x (value)
     (* $x 100))
; Expected result: {100, 200, 300}

; Demonstrating difference between normal and applicative order
; Define a function that checks if argument is a number
(= (is-number $x) true)  ; Simplified - would need actual check

; Normal order: receives unevaluated expression
; !(is-number (+ 1 2))  ; Receives (+ 1 2) as structure

; Applicative order (via chain): receives evaluated result
!(chain (+ 1 2) $x (is-number $x))  ; Receives 3

; Lazy evaluation benefit: avoiding unnecessary computation
(= (const $x $y) $x)

; Second argument never evaluated (normal order)
!(const 42 (very-expensive-computation))
; Expected result: 42
; (very-expensive-computation) is never called!

; With chain, both arguments would be evaluated:
; !(chain (very-expensive-computation) $y (const 42 $y))
; This would compute the expensive operation unnecessarily

; Selective evaluation with conditionals
(= (if-eval $cond $then $else)
   (if $cond
       (chain $then $x $x)    ; Evaluate then branch
       (chain $else $x $x)))  ; Evaluate else branch

; Only the selected branch is evaluated
!(if-eval true (+ 1 2) (/ 1 0))
; Expected result: 3
; else branch (/ 1 0) is NOT evaluated due to short-circuit

!(if-eval false (/ 1 0) (+ 1 2))
; Expected result: 3
; then branch (/ 1 0) is NOT evaluated
