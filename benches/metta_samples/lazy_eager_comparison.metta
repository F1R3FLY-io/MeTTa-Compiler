; Lazy vs Eager Evaluation Comparison
; Tests short-circuit evaluation and branch skipping
; Expected: Lazy evaluation should skip unevaluated branches

; Expensive computation (many operations)
(= (expensive)
    (+ (* 100 200) (* 300 400) (* 500 600) (* 700 800)))

; Very expensive computation (nested)
(= (very-expensive)
    (+ (expensive) (expensive) (expensive) (expensive)))

; Short-circuit AND: should not evaluate second arg if first is False
(= (short-circuit-and-false)
    (and False (expensive)))

(= (short-circuit-and-true)
    (and True (expensive)))

; Short-circuit OR: should not evaluate second arg if first is True
(= (short-circuit-or-true)
    (or True (expensive)))

(= (short-circuit-or-false)
    (or False (expensive)))

; Lazy if with expensive branches
(= (lazy-if-true)
    (if True 1 (expensive)))

(= (lazy-if-false)
    (if False (expensive) 1))

; Nested lazy if
(= (nested-lazy-if)
    (if True
        (if True 1 (expensive))
        (very-expensive)))

; Case with expensive branches
(= (lazy-case $x)
    (case $x
      ((0) zero)
      ((1) one)
      (($other) (expensive))))

; Eager patterns for comparison (evaluate both branches)
(= (eager-if-both)
    (let $then 1
      (let $else (expensive)
        (if True $then $else))))

; Chain of short-circuits
(= (short-circuit-chain)
    (and (or False True)
         (and True
              (or True (expensive)))))

; Conditional with side effects simulation
(= (conditional-chain $x)
    (if (< $x 0)
        (error negative $x)
        (if (== $x 0)
            zero
            (if (< $x 10)
                small
                large))))

; Benchmarks - lazy should be faster than eager
!(short-circuit-and-false)
!(short-circuit-and-true)
!(short-circuit-or-true)
!(short-circuit-or-false)
!(lazy-if-true)
!(lazy-if-false)
!(nested-lazy-if)
!(lazy-case 0)
!(lazy-case 1)
!(eager-if-both)
!(short-circuit-chain)
!(conditional-chain 5)
!(conditional-chain 15)
