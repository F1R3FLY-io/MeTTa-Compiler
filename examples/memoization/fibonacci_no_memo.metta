; Fibonacci WITHOUT Memoization (for comparison)
;
; This version has exponential O(2^n) time complexity.
; fib(25) takes several seconds, fib(30) would take minutes,
; and fib(40) would take hours.
;
; Compare with fibonacci.metta to see the dramatic difference
; that memoization makes.

; Base cases
(= (fib 0) 0)
(= (fib 1) 1)

; Recursive case - each call spawns two more calls
(= (fib $n)
   (if (> $n 1)
       (+ (fib (- $n 1)) (fib (- $n 2)))
       $n))

; Test fibonacci - limited to small values due to exponential growth
!(println! "Fibonacci WITHOUT memoization (exponential time):")
!(println! ("fib 10 =" (fib 10)))
!(println! ("fib 15 =" (fib 15)))
!(println! ("fib 20 =" (fib 20)))
!(println! ("fib 25 =" (fib 25)))
; WARNING: Values above 25 will be very slow!
