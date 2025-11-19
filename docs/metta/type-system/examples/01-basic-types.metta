; Example: Basic Type Annotations
; Demonstrates fundamental type syntax

; Simple value types
(: pi Number)
(= pi 3.14159)

(: greeting String)
(= greeting "Hello, MeTTa!")

(: flag Bool)
(= flag True)

; Function types
(: square (-> Number Number))
(= (square $x) (* $x $x))

!(square 5)  ; → 25

; Polymorphic function
(: identity (-> $t $t))
(= (identity $x) $x)

!(identity 42)       ; → 42
!(identity "hello")  ; → "hello"
!(identity True)     ; → True
