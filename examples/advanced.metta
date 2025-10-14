// Advanced MeTTa features

// Type assertion with arrow type
(: $func (-> Long Long Bool))

// Nested rules
(= (factorial $n) (* $n (factorial (- $n 1))))

// Add atom to space
(add-atom $space (= $x 10))

// Remove atom from space
(rem-atom $space $x)

// Transform operation
(transform $space (+ 1 2))

// Bind operation
(bind! $result (+ 1 2))

// Concurrent with sequential
{
  (+ 1 2)
  ((* 3 4) (/ 8 2))
}

// Builtin operations
(+ 10 20)
(- 100 50)
(* 5 6)
(/ 100 4)
(< 5 10)
(<= 5 5)
(> 10 5)
(>= 10 10)
(== 42 42)
