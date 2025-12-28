; Levenshtein Edit Distance WITHOUT Memoization (for comparison)
;
; This version has exponential O(3^(n+m)) time complexity.
; Even small strings can take several seconds to compute.
;
; Compare with levenshtein.metta to see the difference memoization makes.

; Helper: minimum of three values
(= (min3 $a $b $c)
   (if (< $a $b)
       (if (< $a $c) $a $c)
       (if (< $b $c) $b $c)))

; Base cases
(= (lev Nil Nil) 0)
(= (lev Nil (Cons $_ $ys)) (+ 1 (lev Nil $ys)))
(= (lev (Cons $_ $xs) Nil) (+ 1 (lev $xs Nil)))

; Recursive case - three-way branching causes exponential blowup
(= (lev (Cons $x $xs) (Cons $y $ys))
   (if (== $x $y)
       (lev $xs $ys)
       (+ 1 (min3
              (lev $xs (Cons $y $ys))
              (lev (Cons $x $xs) $ys)
              (lev $xs $ys)))))

; Test with small strings only
!(println! "Levenshtein WITHOUT memoization (exponential time):")

!(println! ("lev (a b c) (a b c) ="
   (lev (Cons a (Cons b (Cons c Nil)))
        (Cons a (Cons b (Cons c Nil))))))

!(println! ("lev (a b c) (a b d) ="
   (lev (Cons a (Cons b (Cons c Nil)))
        (Cons a (Cons b (Cons d Nil))))))

; This one already takes several seconds!
!(println! ("lev (k i t t e n) (s i t t i n g) ="
   (lev (Cons k (Cons i (Cons t (Cons t (Cons e (Cons n Nil))))))
        (Cons s (Cons i (Cons t (Cons t (Cons i (Cons n (Cons g Nil))))))))))
