// MeTTa Type System Demonstration
// Basic Type Assertions and Type Checking

// Type assertions for values
(: x Number)
(: name String)
(: flag Bool)

// Check types of ground values
!(get-type 42)
!(get-type true)
!(get-type "hello")

// Check types of named values
!(get-type x)
!(get-type name)
!(get-type flag)

// Type assertions for functions with arrow types
(: double (-> Number Number))
(: add3 (-> Number Number Number Number))

// Define the actual functions
(= (double $x) (mul $x 2))
(= (add3 $a $b $c) (add $a (add $b $c)))

// Get types of functions
!(get-type double)

// Check type matching
!(check-type x Number)
!(check-type x String)

// Type variables match anything
!(check-type 42 $t)
!(check-type "test" $t)

// Evaluate with types
!(double 21)
!(add3 1 2 3)

// Get types of built-in operations
!(get-type (add 1 2))
!(get-type (lt 5 10))
