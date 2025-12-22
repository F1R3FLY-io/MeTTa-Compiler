; Trampoline Stress Test
; Tests work stack with wide and deep S-expressions
; Expected: Efficient continuation management without stack overflow

; Wide S-expressions: many sibling sub-expressions
(= (wide-5) (+ 1 2 3 4 5))
(= (wide-10) (+ 1 2 3 4 5 6 7 8 9 10))
(= (wide-20) (+ 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20))

; Deep nesting (binary tree shape) - tests continuation depth
(= (deep-5) (+ (+ (+ (+ (+ 1 2) 3) 4) 5) 6))
(= (deep-10) (+ (+ (+ (+ (+ (+ (+ (+ (+ (+ 1 2) 3) 4) 5) 6) 7) 8) 9) 10) 11))
(= (deep-15) (+ (+ (+ (+ (+ (+ (+ (+ (+ (+ (+ (+ (+ (+ (+ 1 2) 3) 4) 5) 6) 7) 8) 9) 10) 11) 12) 13) 14) 15) 16))

; Many continuations: alternating evaluations with let bindings
(= (many-lets-4)
    (let $a (+ 1 2)
      (let $b (+ 3 4)
        (let $c (+ 5 6)
          (let $d (+ 7 8)
            (+ $a $b $c $d))))))

(= (many-lets-8)
    (let $a (+ 1 2)
      (let $b (+ 3 4)
        (let $c (+ 5 6)
          (let $d (+ 7 8)
            (let $e (+ 9 10)
              (let $f (+ 11 12)
                (let $g (+ 13 14)
                  (let $h (+ 15 16)
                    (+ $a $b $c $d $e $f $g $h))))))))))

; Mixed wide and deep
(= (mixed-wide-deep)
    (+ (+ (+ 1 2 3) (+ 4 5 6)) (+ (+ 7 8 9) (+ 10 11 12))))

; Nested function calls (forces CollectSExpr continuations)
(= (add3 $a $b $c) (+ $a $b $c))
(= (nested-calls)
    (add3 (add3 1 2 3) (add3 4 5 6) (add3 7 8 9)))

; List operations that produce many intermediate values
(= (list-stress)
    (cons 1 (cons 2 (cons 3 (cons 4 (cons 5 Nil))))))

; Complex expression tree
(= (expr-tree)
    (* (+ 1 (* 2 3))
       (+ (* 4 5) 6)
       (- (* 7 8) (+ 9 10))))

; Benchmarks
!(wide-5)
!(wide-10)
!(wide-20)
!(deep-5)
!(deep-10)
!(deep-15)
!(many-lets-4)
!(many-lets-8)
!(mixed-wide-deep)
!(nested-calls)
!(list-stress)
!(expr-tree)
