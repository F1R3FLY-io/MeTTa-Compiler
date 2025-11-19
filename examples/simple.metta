; Simple MeTTa examples

; Type assertion
(: $x Bool)

; Rule definition
(= (add $a $b) (+ $a $b))

; Note: Nondeterministic evaluation uses superpose, not braces
; See official MeTTa documentation for nondeterministic patterns

; Sequential execution
((+ 1 2) (- 5 3) (* 4 2))

; Pattern matching
(match $space $x $x)

; Knowledge base entry
(! (= $fact 42) .)

; Query
(? $fact .)
