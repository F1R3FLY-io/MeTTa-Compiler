; Levenshtein Edit Distance with Memoization
;
; Computes the minimum number of single-character edits (insertions,
; deletions, or substitutions) needed to change one string into another.
;
; Without memoization: O(3^(n+m)) - exponential
; With memoization: O(n*m) - polynomial
;
; This is a classic dynamic programming problem where memoization
; transforms an intractable algorithm into an efficient one.

; Create memo table for edit distance calculations
!(bind! &lev-cache (new-memo "lev-cache"))

; Helper: minimum of three values
(= (min3 $a $b $c)
   (if (< $a $b)
       (if (< $a $c) $a $c)
       (if (< $b $c) $b $c)))

; Base cases: one string is empty
(= (lev Nil Nil) 0)
(= (lev Nil (Cons $_ $ys)) (+ 1 (lev Nil $ys)))
(= (lev (Cons $_ $xs) Nil) (+ 1 (lev $xs Nil)))

; Recursive case with memoization
(= (lev (Cons $x $xs) (Cons $y $ys))
   (memo-first &lev-cache
     (if (== $x $y)
         ; Characters match - no edit needed
         (lev $xs $ys)
         ; Characters differ - try all three operations
         (+ 1 (min3
                (lev $xs (Cons $y $ys))      ; deletion
                (lev (Cons $x $xs) $ys)      ; insertion
                (lev $xs $ys))))))           ; substitution

; Test cases
!(println! "Levenshtein Edit Distance with memoization:")

; Same strings - distance 0
!(println! ("lev (a b c) (a b c) ="
   (lev (Cons a (Cons b (Cons c Nil)))
        (Cons a (Cons b (Cons c Nil))))))

; One character different - distance 1
!(println! ("lev (a b c) (a b d) ="
   (lev (Cons a (Cons b (Cons c Nil)))
        (Cons a (Cons b (Cons d Nil))))))

; Classic example: kitten -> sitting (distance 3)
; k->s (substitution), e->i (substitution), +g (insertion)
!(println! ("lev (k i t t e n) (s i t t i n g) ="
   (lev (Cons k (Cons i (Cons t (Cons t (Cons e (Cons n Nil))))))
        (Cons s (Cons i (Cons t (Cons t (Cons i (Cons n (Cons g Nil))))))))))

; Longer example: saturday -> sunday (distance 3)
!(println! ("lev (s a t u r d a y) (s u n d a y) ="
   (lev (Cons s (Cons a (Cons t (Cons u (Cons r (Cons d (Cons a (Cons y Nil))))))))
        (Cons s (Cons u (Cons n (Cons d (Cons a (Cons y Nil)))))))))

; Display cache statistics
!(println! ("Cache stats:" (memo-stats &lev-cache)))
