; Example: Working with Rules
; Demonstrates adding, querying, and evaluating rules

; ========================================
; Simple Rules
; ========================================

!(println "=== Simple Rules ===")

; Add basic rules
(add-atom &self (= (double $x) (* $x 2)))
(add-atom &self (= (triple $x) (* $x 3)))
(add-atom &self (= (square $x) (* $x $x)))

; Evaluate rules
!(println "Doubling 5:")
!(double 5)
; Expected: 10

!(println "Tripling 4:")
!(triple 4)
; Expected: 12

!(println "Squaring 6:")
!(square 6)
; Expected: 36

; ========================================
; Recursive Rules
; ========================================

!(println "\n=== Recursive Rules ===")

; Factorial
(add-atom &self (= (factorial 0) 1))
(add-atom &self (= (factorial $n)
    (if (> $n 0)
        (* $n (factorial (- $n 1)))
        1)))

!(println "Factorial of 5:")
!(factorial 5)
; Expected: 120

; Fibonacci
(add-atom &self (= (fib 0) 0))
(add-atom &self (= (fib 1) 1))
(add-atom &self (= (fib $n)
    (if (> $n 1)
        (+ (fib (- $n 1)) (fib (- $n 2)))
        $n)))

!(println "Fibonacci of 7:")
!(fib 7)
; Expected: 13

; ========================================
; Pattern Matching Rules
; ========================================

!(println "\n=== Pattern Matching Rules ===")

; Boolean operations
(add-atom &self (= (and True True) True))
(add-atom &self (= (and True False) False))
(add-atom &self (= (and False True) False))
(add-atom &self (= (and False False) False))

!(println "True AND False:")
!(and True False)
; Expected: False

; List operations
(add-atom &self (= (head ($h $t...)) $h))
(add-atom &self (= (tail ($h $t...)) $t))

!(println "\nHead of list (1 2 3):")
!(head (1 2 3))
; Expected: 1

!(println "Tail of list (1 2 3):")
!(tail (1 2 3))
; Expected: (2 3)

; ========================================
; Conditional Rules
; ========================================

!(println "\n=== Conditional Rules ===")

; Absolute value
(add-atom &self (= (abs $x)
    (if (< $x 0)
        (- $x)
        $x)))

!(println "Absolute value of -5:")
!(abs -5)
; Expected: 5

!(println "Absolute value of 7:")
!(abs 7)
; Expected: 7

; Max of two numbers
(add-atom &self (= (max $x $y)
    (if (> $x $y) $x $y)))

!(println "Max of 10 and 3:")
!(max 10 3)
; Expected: 10

; ========================================
; Logical Inference Rules
; ========================================

!(println "\n=== Logical Inference ===")

; Add facts
(add-atom &self (Human Socrates))
(add-atom &self (Human Plato))

; Add inference rules
(add-atom &self (= (Mortal $x) (Human $x)))
(add-atom &self (= (can-think $x) (Human $x)))

!(println "Is Socrates mortal?")
!(Mortal Socrates)
; Expected: True (derived from Human Socrates + rule)

!(println "Who can think?")
!(match &self (Human $x) (can-think $x))
; Expected: [(can-think Socrates), (can-think Plato)]

; ========================================
; Multiple Matching Rules
; ========================================

!(println "\n=== Multiple Matching Rules ===")

; Add multiple rules for same pattern
(add-atom &self (= (choice $x) (option-A $x)))
(add-atom &self (= (choice $x) (option-B $x)))

!(println "Calling choice with 42:")
!(choice 42)
; Expected: May return (option-A 42) or (option-B 42) or both
; (non-deterministic)

; ========================================
; Querying Rules
; ========================================

!(println "\n=== Querying Rules ===")

; Find all rules defining 'double'
!(println "Rules defining 'double':")
!(match &self (= (double $x) $result) (double $x becomes $result))
; Expected: [(double $x becomes (* $x 2))]

; Find all function names with rules
!(println "\nAll functions with rules:")
!(match &self (= ($fname $args...) $result) $fname)
; Expected: [double, triple, square, factorial, fib, ...]

; Extract recursive rules (rules mentioning themselves in result)
!(println "\nFinding factorial rule:")
!(match &self (= (factorial $n) $result) $result)
; Expected: [1, (if (> $n 0) (* $n (factorial (- $n 1))) 1)]

; ========================================
; Removing Rules
; ========================================

!(println "\n=== Removing Rules ===")

; Add a rule
(add-atom &self (= (temp-func $x) (* $x 100)))

!(println "Before removal:")
!(temp-func 5)
; Expected: 500

; Remove the rule (must match exactly)
(remove-atom &self (= (temp-func $x) (* $x 100)))

!(println "After removal (will likely error or return unevaluated):")
; !(temp-func 5)  ; Would error: no matching rule

; Verify removal
!(println "Checking if rule exists:")
!(match &self (= (temp-func $x) $result) found)
; Expected: [] (empty - rule removed)

; ========================================
; Transformation Rules
; ========================================

!(println "\n=== Transformation Rules ===")

; Type conversion
(add-atom &self (= (to-bool 0) False))
(add-atom &self (= (to-bool $x)
    (if (!= $x 0) True False)))

!(println "to-bool 0:")
!(to-bool 0)
; Expected: False

!(println "to-bool 5:")
!(to-bool 5)
; Expected: True

; ========================================
; Summary
; ========================================

!(println "\n=== Summary ===")
!(println "All rules in space:")
!(match &self (= $pattern $result) (rule $pattern to $result))
