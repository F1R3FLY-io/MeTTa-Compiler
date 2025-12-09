; Simple MeTTa examples

; Type assertion
(: $x Bool)

; Rule definition
(= (add $a $b) (+ $a $b))

; Note: Nondeterministic evaluation uses superpose, not braces
; See official MeTTa documentation for nondeterministic patterns

; Sequential execution
((+ 1 2) (- 5 3) (* 4 2))

; Pattern matching with &self
!(match &self (= (add $a $b) $body) (found $a $b $body))

; Knowledge base entry
(= $fact 42)

; Query the space
!(match &self (= $fact $val) $val)
