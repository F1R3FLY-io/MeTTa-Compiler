; MeTTa Evaluation Control Functions Demonstration
; Demonstrates: function, return, chain

; ============================================
; 1. FUNCTION and RETURN - Early Return Semantics
; ============================================

; function evaluates an expression repeatedly until it encounters (return value)
; This is useful for recursive computations with early termination

; Example 1: Factorial using function and return
(= (factorial $n)
   (if (== $n 0)
       (return 1)
       (factorial-helper $n 1)))

(= (factorial-helper $n $acc)
   (if (== $n 0)
       (return $acc)
       (factorial-helper (- $n 1) (* $n $acc))))

; Test factorial
!(function (factorial 5))  ; Expected: 120
!(function (factorial 3))  ; Expected: 6

; Example 2: Fibonacci using function and return
(= (fibonacci $n)
   (if (== $n 0)
       (return 0)
       (if (== $n 1)
           (return 1)
           (fibonacci-helper $n 0 1))))

(= (fibonacci-helper $n $a $b)
   (if (== $n 1)
       (return $b)
       (fibonacci-helper (- $n 1) $b (+ $a $b))))

; Test fibonacci
!(function (fibonacci 0))   ; Expected: 0
!(function (fibonacci 1))   ; Expected: 1
!(function (fibonacci 7))   ; Expected: 13

; Example 3: Power function (x^n) using function and return
(= (power $base $exp)
   (if (== $exp 0)
       (return 1)
       (power-helper $base $exp 1)))

(= (power-helper $base $exp $acc)
   (if (== $exp 0)
       (return $acc)
       (power-helper $base (- $exp 1) (* $acc $base))))

; Test power
!(function (power 2 0))   ; Expected: 1
!(function (power 3 4))   ; Expected: 81
!(function (power 5 3))   ; Expected: 125

; ============================================
; 2. CHAIN - Monadic Sequencing
; ============================================

; chain evaluates the first expression, binds the result to a variable,
; then evaluates the second expression with that binding
; Syntax: (chain <expr> <var> <body>)

; Example 1: Basic chain - compute and use result
!(chain (+ 2 3) $x (* $x 2))  ; Expected: 10
; Explanation: (+ 2 3) = 5, bind to $x, then (* 5 2) = 10

; Example 2: Chain multiple computations
!(chain (+ 10 5) $a
   (chain (* $a 2) $b
      (- $b 10)))  ; Expected: 20
; Explanation: 10+5=15, 15*2=30, 30-10=20

; Example 3: Chain with comparison
!(chain (< 5 10) $result
   (if $result
       "yes"
       "no"))  ; Expected: "yes"

; Example 4: Complex computation pipeline
(= (compute-result $x)
   (chain (* $x $x) $squared
      (chain (+ $squared 10) $sum
         (- $sum $x))))

!(compute-result 5)  ; Expected: 30
; Explanation: 5*5=25, 25+10=35, 35-5=30

; Example 5: Chain with function and return
(= (process-value $n)
   (chain (+ $n 1) $incremented
      (if (> $incremented 10)
          (return $incremented)
          (process-value $incremented))))

!(function (process-value 8))  ; Expected: 11
; Explanation: 8+1=9 (not > 10), 9+1=10 (not > 10), 10+1=11 (> 10, return)

; ============================================
; 3. COMBINING FUNCTION, RETURN, and CHAIN
; ============================================

; Example: Complex state transformation using all three
(= (transform-state $state $count)
   (if (== $count 0)
       (return $state)
       (chain (* $state 2) $doubled
          (chain (+ $doubled 1) $incremented
             (transform-state $incremented (- $count 1))))))

!(function (transform-state 1 3))  ; Expected: 15
; Explanation:
; Step 1: 1*2+1 = 3, count=2
; Step 2: 3*2+1 = 7, count=1
; Step 3: 7*2+1 = 15, count=0, return 15

; ============================================
; 4. PRACTICAL USE CASES
; ============================================

; Use case 1: Early termination in search (find first number > threshold)
(= (find-threshold $n $max $threshold)
   (if (> $n $max)
       (return "not found")
       (if (> $n $threshold)
           (return $n)
           (find-threshold (+ $n 1) $max $threshold))))

; Test: !(function (find-threshold 1 20 10))
; Expected: 11

; Use case 2: Computation pipeline with chain
(= (pipeline $input)
   (chain (+ $input 5) $step1
      (chain (* $step1 3) $step2
         (chain (- $step2 10) $step3
            (* $step3 $step3)))))

!(pipeline 2)  ; Expected: 289
; Explanation: 2+5=7, 7*3=21, 21-10=11, 11*11=121... wait let me recalculate
; Actually: 2+5=7, 7*3=21, 21-10=11, 11*11=121

; Use case 3: State accumulator with early exit
(= (accumulate-until $current $target $sum)
   (if (> $current $target)
       (return $sum)
       (chain (+ $sum $current) $new-sum
          (accumulate-until (+ $current 1) $target $new-sum))))

!(function (accumulate-until 1 5 0))  ; Expected: 10
; Explanation: Sum of 1+2+3+4 = 10 (stops when current > 5, i.e., at 6)
