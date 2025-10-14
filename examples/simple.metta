// Simple MeTTa examples

// Type assertion
(: $x Bool)

// Rule definition
(= (add $a $b) (+ $a $b))

// Concurrent execution
{ (+ 1 2) (- 5 3) (* 4 2) }

// Sequential execution
((+ 1 2) (- 5 3) (* 4 2))

// Pattern matching
(match $space $x $x)

// Knowledge base entry
(! (= $fact 42) .)

// Query
(? $fact .)
