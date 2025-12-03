; Grounded TCO Operations Stress Test
; Tests TCO state machine with arithmetic and logic chains
; Expected: TCO operations should not overflow stack

; Chain of arithmetic operations (many TCO state machine steps)
(= (arithmetic-chain)
    (+ (* (- (/ 100 2) 10) 3) (* 4 5)))

; Deep nesting of grounded operations
(= (deep-add-5)
    (+ (+ (+ (+ (+ 1 2) 3) 4) 5) 6))

(= (deep-add-10)
    (+ (+ (+ (+ (+ (+ (+ (+ (+ (+ 1 2) 3) 4) 5) 6) 7) 8) 9) 10) 11))

(= (deep-mul-5)
    (* (* (* (* (* 1 2) 3) 4) 5) 6))

; Comparison chains (tests comparison TCO ops)
(= (comparison-chain-short)
    (and (< 1 2) (< 2 3)))

(= (comparison-chain-medium)
    (and (< 1 2) (and (< 2 3) (and (< 3 4) (< 4 5)))))

(= (comparison-chain-long)
    (and (< 1 2)
         (and (< 2 3)
              (and (< 3 4)
                   (and (< 4 5)
                        (and (< 5 6)
                             (and (< 6 7)
                                  (< 7 8))))))))

; Mixed arithmetic and comparison
(= (mixed-ops)
    (if (< (+ 1 2) (* 2 3))
        (- 100 (+ 10 20))
        0))

; Logical operations chain (tests AndOpTCO/OrOpTCO)
(= (logic-chain-and)
    (and True (and True (and True (and True True)))))

(= (logic-chain-or)
    (or False (or False (or False (or False True)))))

(= (logic-chain-mixed)
    (or (and True True)
        (and False (or True False))))

; Division chain (tests DivOpTCO)
(= (division-chain)
    (/ (/ (/ 1000 2) 5) 10))

; Equality chain (tests EqOpTCO)
(= (equality-chain)
    (and (== 1 1) (and (== 2 2) (== 3 3))))

; Complex grounded expression tree
(= (complex-grounded)
    (* (+ (- 10 5) (/ 20 4))
       (+ (* 3 3) (- 15 6))))

; Recursive arithmetic with grounded ops
(= (recursive-arithmetic 0) 0)
(= (recursive-arithmetic $n)
    (+ $n (recursive-arithmetic (- $n 1))))

; Benchmarks
!(arithmetic-chain)
!(deep-add-5)
!(deep-add-10)
!(deep-mul-5)
!(comparison-chain-short)
!(comparison-chain-medium)
!(comparison-chain-long)
!(mixed-ops)
!(logic-chain-and)
!(logic-chain-or)
!(logic-chain-mixed)
!(division-chain)
!(equality-chain)
!(complex-grounded)
!(recursive-arithmetic 10)
!(recursive-arithmetic 50)
