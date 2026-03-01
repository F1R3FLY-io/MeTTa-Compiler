;; Type-heavy MeTTa program for end-to-end type system benchmarking

;; Subtype hierarchy
(:< Dog Animal)
(:< Cat Animal)
(:< Animal LivingThing)

;; Typed atoms
(: fido Dog)
(: whiskers Cat)

;; Polymorphic function
(: identity (-> $t $t))
(= (identity $x) $x)

;; Arithmetic with type inference
(: add1 (-> Number Number))
(= (add1 $n) (+ $n 1))

;; Type system queries
!(get-type fido)
!(get-type (identity fido))
!(check-type fido Animal)
!(check-type fido LivingThing)
!(type-cast fido Animal &self)
!(type-cast whiskers Dog &self)
!(is-function (-> Number Number))
!(get-type (add1 5))
