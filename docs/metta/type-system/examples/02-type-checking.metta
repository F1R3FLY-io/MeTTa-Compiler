; Example: Type Checking
; Demonstrates pragma-controlled type checking

; Define typed function
(: add (-> Number Number Number))
(= (add $x $y) (+ $x $y))

; Without type checking (default)
!(add 5 10)  ; → 15
; !(add 5 "hello")  ; May fail at runtime in +, but not caught by type checker

; Enable type checking
!(pragma! type-check auto)

; Now type checking is active
!(add 5 10)  ; → 15 (type-checked)

; This would produce type error:
; !(add 5 "hello")
; → (Error (add 5 "hello") (BadArgType 2 Number String))
