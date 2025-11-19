; Example: Gradual Typing
; Demonstrates %Undefined% and mixed typed/untyped code

; Untyped value (has %Undefined% type)
(= mystery 42)

!(get-type mystery)  ; → %Undefined%

; Typed function
(: double (-> Number Number))
(= (double $x) (* $x 2))

; Can pass %Undefined% to typed function
!(double mystery)  ; Works: %Undefined% unifies with Number

; Gradual type introduction
; Phase 1: Untyped
(= (factorial $n)
   (if (== $n 0)
       1
       (* $n (factorial (- $n 1)))))

; Phase 2: Add type
(: factorial (-> Nat Nat))

; Phase 3: Enable checking when ready
; !(pragma! type-check auto)
