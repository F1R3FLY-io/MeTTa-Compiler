; Fibonacci with Memoization
;
; This example demonstrates how memoization transforms an exponential O(2^n)
; algorithm into a linear O(n) algorithm by caching intermediate results.
;
; Without memoization, fib(30) requires ~1 billion recursive calls.
; With memoization, fib(90) completes in milliseconds.

; Create a memo table for caching fibonacci results
!(bind! &fib-cache (new-memo "fib-cache"))

; Base cases
(= (fib 0) 0)
(= (fib 1) 1)

; Recursive case with memoization
; memo-first caches and returns the first result
(= (fib $n)
   (if (> $n 1)
       (memo-first &fib-cache
         (+ (fib (- $n 1)) (fib (- $n 2))))
       $n))

; Test fibonacci sequence
!(println! "Fibonacci with memoization:")
!(println! ("fib 10 =" (fib 10)))
!(println! ("fib 20 =" (fib 20)))
!(println! ("fib 30 =" (fib 30)))
!(println! ("fib 40 =" (fib 40)))
!(println! ("fib 50 =" (fib 50)))

; These would be impossible without memoization!
!(println! ("fib 60 =" (fib 60)))
!(println! ("fib 70 =" (fib 70)))
!(println! ("fib 80 =" (fib 80)))
!(println! ("fib 90 =" (fib 90)))

; Display cache statistics
; Format: (stats entries hits misses evictions hit_rate_percent)
!(println! ("Cache stats:" (memo-stats &fib-cache)))
