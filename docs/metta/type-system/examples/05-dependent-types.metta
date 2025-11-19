; Example: Dependent Types
; Demonstrates types that depend on values

; Natural numbers
(: Nat Type)
(: Z Nat)
(: S (-> Nat Nat))

; Length-indexed vectors
(: Vec (-> Type Nat Type))
(: Nil (Vec $t Z))
(: Cons (-> $t (Vec $t $n) (Vec $t (S $n))))

; Vector with known length
(: my-vec (Vec Number (S (S Z))))
(= my-vec (Cons 1 (Cons 2 Nil)))

!(get-type my-vec)
; → (Vec Number (S (S Z)))

; Type-safe head (requires non-empty vector)
(: head (-> (Vec $t (S $n)) $t))
(= (head (Cons $x $xs)) $x)

!(head my-vec)  ; → 1

; Type-safe tail
(: tail (-> (Vec $t (S $n)) (Vec $t $n)))
(= (tail (Cons $x $xs)) $xs)

!(tail my-vec)  ; → (Cons 2 Nil) : Vec Number (S Z)
