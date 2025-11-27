; Example: Type Errors
; Demonstrates type error detection and messages

!(pragma! type-check auto)

; Define typed function
(: add (-> Number Number Number))
(= (add $x $y) (+ $x $y))

; Correct usage
!(add 5 10)  ; → 15

; Type error: wrong argument type
; Uncomment to see error:
; !(add 5 "hello")
; → (Error (add 5 "hello") (BadArgType 2 Number String))

; Type error: wrong number of arguments
; Uncomment to see error:
; !(add 5)
; → (Error (add 5) IncorrectNumberOfArguments)

; Type error: completely wrong type
; Uncomment to see error:
; (: x Number)
; (= x "not a number")
; !(get-type x)
; → (Error x (BadType Number String))
