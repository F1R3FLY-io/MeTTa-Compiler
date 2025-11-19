; Example: Normal Order Evaluation
; Demonstrates that arguments are NOT evaluated before function application

; Define a function that ignores its second argument
(= (first $x $y) $x)

; This should return 42 without evaluating the division by zero
; In normal order, $y is never evaluated
!(first 42 (/ 1 0))
; Expected result: 42

; Compare with explicit evaluation using chain
; This WILL cause an error because we force evaluation
; !(chain (/ 1 0) $result (first 42 $result))
; Expected: Error (division by zero)

; Example: Conditional evaluation
(= (if-then-else true $then $else) $then)
(= (if-then-else false $then $else) $else)

; Only the 'then' branch should be evaluated, not 'else'
!(if-then-else true (+ 1 2) (/ 1 0))
; Expected result: 3 (else branch not evaluated)

!(if-then-else false (/ 1 0) (+ 1 2))
; Expected result: 3 (then branch not evaluated)
