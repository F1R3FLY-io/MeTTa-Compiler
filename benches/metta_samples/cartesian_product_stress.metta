; Cartesian Product Stress Test
; Tests lazy iterator with various nondeterminism levels
; Expected: Memory-efficient generation of combinations

; Binary choice (2 values)
(= (binary) 0)
(= (binary) 1)

; Ternary choice (3 values)
(= (ternary) 0)
(= (ternary) 1)
(= (ternary) 2)

; Quinary choice (5 values)
(= (quinary) 0)
(= (quinary) 1)
(= (quinary) 2)
(= (quinary) 3)
(= (quinary) 4)

; Small Cartesian product: 2^3 = 8 combinations
(= (binary-triple)
    (let $a (binary)
      (let $b (binary)
        (let $c (binary)
          (list $a $b $c)))))

; Medium Cartesian product: 3^3 = 27 combinations
(= (ternary-triple)
    (let $a (ternary)
      (let $b (ternary)
        (let $c (ternary)
          (list $a $b $c)))))

; Larger Cartesian product: 5^3 = 125 combinations
(= (quinary-triple)
    (let $a (quinary)
      (let $b (quinary)
        (let $c (quinary)
          (list $a $b $c)))))

; Arithmetic over nondeterministic values (forces Cartesian in grounded ops)
(= (nondet-add-binary)
    (+ (binary) (binary)))

(= (nondet-add-ternary)
    (+ (ternary) (ternary)))

(= (nondet-mul-binary)
    (* (binary) (binary)))

; Nested nondeterminism: 2^2 * 2^2 = 16 combinations
(= (nested-nondet)
    (+ (+ (binary) (binary)) (+ (binary) (binary))))

; Deep nondeterminism: 2^5 = 32 combinations
(= (deep-nondet)
    (let $a (binary)
      (let $b (binary)
        (let $c (binary)
          (let $d (binary)
            (let $e (binary)
              (list $a $b $c $d $e)))))))

; Benchmarks - progressively more combinations
!(binary-triple)           ; 8 results
!(ternary-triple)          ; 27 results
!(quinary-triple)          ; 125 results
!(nondet-add-binary)       ; 4 results
!(nondet-add-ternary)      ; 9 results
!(nondet-mul-binary)       ; 4 results
!(nested-nondet)           ; 16 results
!(deep-nondet)             ; 32 results
