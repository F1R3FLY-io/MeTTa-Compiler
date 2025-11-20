;; Complex Pattern Matching Examples
;; These test nested patterns and destructuring

;; Nested destructuring - 2 levels
(= (nested ($a ($b $c))) (result $a $b $c))
!(nested (1 (2 3)))

;; Tree depth calculation
(= (depth ($left $val $right))
   (+ 1 (max (depth $left) (depth $right))))
(= (depth nil) 0)
!(depth (((nil 1 nil) 2 nil) 3 (nil 4 nil)))

;; List operations - head/tail pattern
(= (sum ()) 0)
(= (sum ($head . $tail)) (+ $head (sum $tail)))
!(sum (1 2 3 4 5))

;; Multiple argument destructuring
(= (multi $a $b $c $d) (+ (+ $a $b) (+ $c $d)))
!(multi 1 2 3 4)

;; Nested tuples
(= (process (($cmd $arg) ($nested $data)))
   (execute $cmd $arg $nested $data))
!(process ((run test) (config settings)))

;; Variable sharing - same variable multiple times
(= (same $x $x) (equal $x $x))
!(same 42 42)   ;; Should match
;; !(same 42 43)   ;; Should not match

;; Mixed depth patterns
(= (eval-pair (pair $x $y))
   (+ $x $y))
!(eval-pair (pair 5 7))

;; Deeply nested s-expression (5 levels)
(= (deep5 ($a ($b ($c ($d $e)))))
   (result $a $b $c $d $e))
!(deep5 (1 (2 (3 (4 5)))))

;; Wide pattern - many variables at same level
(= (wide $a $b $c $d $e $f)
   (sum6 $a $b $c $d $e $f))
!(wide 1 2 3 4 5 6)

;; Combination - wide and deep
(= (combo ($a $b) ($c ($d $e)) ($f $g $h))
   (sum8 $a $b $c $d $e $f $g $h))
!(combo (1 2) (3 (4 5)) (6 7 8))
