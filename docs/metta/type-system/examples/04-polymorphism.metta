; Example: Polymorphic Types
; Demonstrates type variables and generic functions

; Generic identity
(: identity (-> $t $t))
(= (identity $x) $x)

; Works for any type
!(identity 42)       ; $t = Number
!(identity "hello")  ; $t = String
!(identity True)     ; $t = Bool

; Generic pair function
(: pair (-> $a $b (Pair $a $b)))
(= (pair $x $y) (MkPair $x $y))

!(pair 1 "one")      ; (Pair Number String)
!(pair True False)   ; (Pair Bool Bool)

; Generic list operations
(: map (-> (-> $a $b) (List $a) (List $b)))
(= (map $f Nil) Nil)
(= (map $f (Cons $x $xs)) (Cons ($f $x) (map $f $xs)))

(: inc (-> Number Number))
(= (inc $x) (+ $x 1))

!(map inc (Cons 1 (Cons 2 (Cons 3 Nil))))
; → (Cons 2 (Cons 3 (Cons 4 Nil)))
