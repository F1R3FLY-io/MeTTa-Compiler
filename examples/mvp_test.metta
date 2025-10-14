// MVP Test Examples
// Tests core MeTTa reduction engine features

// Test 1: Basic reduction with rules
(= (double $x) (+ $x $x))
(= (triple $x) (+ $x (+ $x $x)))

// Test 2: Pattern matching with variables
(= (fib 0) 1)
(= (fib 1) 1)
(= (fib $n) (+ (fib (- $n 1)) (fib (- $n 2))))

// Test 3: Grounded arithmetic operations
(+ 2 3)
(- 10 4)
(* 3 4)
(/ 12 3)

// Test 4: Symbolic expressions (lazy evaluation)
(+ $x $y)
(double $z)

// Test 5: Nested expressions
(double (+ 2 3))
(triple (fib 3))

// Test 6: Multivalued results (when multiple rules match)
(= (color) red)
(= (color) blue)
(= (color) green)

// Test 7: Error handling (type mismatch)
// (+ 1 "not-a-number")  // Commented out - would produce error
