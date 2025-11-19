;; Simple Pattern Matching Examples
;; These test basic pattern matching functionality

;; Identity function - single variable
(= (id $x) $x)
!(id 42)
!(id "hello")

;; Pair function - multiple variables
(= (pair $a $b) (result $a $b))
!(pair 1 2)
!(pair "foo" "bar")

;; Triple - three variables
(= (triple $x $y $z) (list $x $y $z))
!(triple 1 2 3)

;; Wildcard matching - ignore certain positions
(= (first (_ $x _)) $x)
!(first (ignored 42 also-ignored))

;; Constant matching - must match exactly
(= (is-zero 0) true)
(= (is-zero $n) false)
!(is-zero 0)
!(is-zero 5)

;; Boolean matching
(= (not true) false)
(= (not false) true)
!(not true)
!(not false)

;; String matching
(= (greet "Alice") "Hello, Alice!")
(= (greet "Bob") "Hello, Bob!")
(= (greet $name) (concat "Hello, " $name "!"))
!(greet "Alice")
!(greet "Charlie")
