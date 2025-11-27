; Example: Reduction Order
; Demonstrates how reduction rules are applied

; Simple reduction rule
(= (double $x) (* $x 2))

!(double 5)
; Expected result: 10
; Reduction: (double 5) → (* 5 2) → 10

; Multiple reduction rules for same function
(= (choose) A)
(= (choose) B)
(= (choose) C)

!(choose)
; Expected result: {A, B, C}
; All three rules are applied (non-deterministic)

; Recursive reduction
(= (factorial 0) 1)
(= (factorial $n) (* $n (factorial (- $n 1))))

!(factorial 3)
; Expected result: 6
; Reduction:
;   (factorial 3)
;   → (* 3 (factorial 2))
;   → (* 3 (* 2 (factorial 1)))
;   → (* 3 (* 2 (* 1 (factorial 0))))
;   → (* 3 (* 2 (* 1 1)))
;   → (* 3 (* 2 1))
;   → (* 3 2)
;   → 6

; Overlapping patterns (both match)
(= (classify 0) zero)
(= (classify $n) nonzero)

!(classify 0)
; Expected result: {zero, nonzero}
; Both rules match! First rule: exact match on 0
; Second rule: variable $n matches 0

!(classify 5)
; Expected result: nonzero
; Only second rule matches

; Conditional reduction (pattern guards via rules)
(= (safe-div $x 0) infinity)
(= (safe-div $x $y) (/ $x $y))

!(safe-div 10 2)
; Expected result: 5 or {5, infinity}
; First rule doesn't match (2 ≠ 0)
; Second rule matches: (/ 10 2) → 5

!(safe-div 10 0)
; Expected result: {infinity, error} or just infinity
; First rule matches: infinity
; Second rule may match: (/ 10 0) → error
; Both alternatives are explored!

; Chain forces evaluation in sequence
(= (report $x) (print $x))

; Without chain: (choose) is not evaluated
; (report sees the expression (choose))
; !(report (choose))  ; Would print: (choose)

; With chain: (choose) is evaluated first
; Then each result is passed to report
!(chain (choose) $c (report $c))
; Expected: prints A, B, C (in separate branches)

; Demonstrating that rule definition order doesn't matter
; Define rules in one order
(= (test1) first)
(= (test1) second)

!(test1)
; Expected: {first, second}

; Now flip the order (conceptually - in same space)
(= (test2) second)
(= (test2) first)

!(test2)
; Expected: {first, second} or {second, first}
; Order unspecified - implementation dependent
; But BOTH results are always returned

; Nested reduction with multiple rules
(= (outer-rule) (inner-rule 1))
(= (outer-rule) (inner-rule 2))
(= (inner-rule $x) (* $x 10))
(= (inner-rule $x) (+ $x 100))

!(outer-rule)
; Expected result: {10, 101, 20, 102}
; Reduction paths:
;   outer-rule → inner-rule 1 → {10, 101}
;   outer-rule → inner-rule 2 → {20, 102}
