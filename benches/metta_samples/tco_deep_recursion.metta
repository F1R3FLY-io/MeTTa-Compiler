; TCO Deep Recursion Stress Test
; Tests tail call optimization with various recursion depths
; Expected: TCO should keep depth constant, no stack overflow

; Tail-recursive countdown - accumulator pattern
(= (countdown 0 $acc) $acc)
(= (countdown $n $acc) (countdown (- $n 1) (+ $acc 1)))

; Chain of rule applications (tests rule RHS tail call)
(= (chain-rule 0) done)
(= (chain-rule $n) (chain-rule (- $n 1)))

; Tail-recursive sum with accumulator
(= (sum-to 0 $acc) $acc)
(= (sum-to $n $acc) (sum-to (- $n 1) (+ $acc $n)))

; Mutual tail recursion
(= (even-check 0) True)
(= (even-check $n) (odd-check (- $n 1)))
(= (odd-check 0) False)
(= (odd-check $n) (even-check (- $n 1)))

; Benchmark different depths
; Shallow (warmup)
!(countdown 50 0)
!(chain-rule 50)

; Medium depth
!(countdown 200 0)
!(chain-rule 200)
!(sum-to 200 0)

; Deep (stress test)
!(countdown 500 0)
!(chain-rule 500)
!(sum-to 500 0)

; Very deep (near MAX_EVAL_DEPTH with TCO)
!(countdown 800 0)
!(chain-rule 800)

; Mutual recursion test
!(even-check 100)
!(odd-check 101)
