;; Structural dependent types
(: Nat Type)
(: Z Nat)
(: S (-> Nat Nat))

(: Vec (-> Type Nat Type))
(: VNil (-> (Vec $t Z)))
(: VCons (-> $t (Vec $t $n) (Vec $t (S $n))))

;; Arithmetic dependent types
(: VecN (-> Type Number Type))
(: NilN (VecN $t 0))
(: ConsN (-> $t (VecN $t $x) (VecN $t (+ $x 1))))

!(get-type NilN)
!(get-type (ConsN "a" NilN))
!(get-type (ConsN "a" (ConsN "b" NilN)))
