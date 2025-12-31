; Expression Manipulation Operations Demo
; This file demonstrates the expression manipulation built-in functions

; ============================================================================
; 1. cons-atom - Construct expressions from head and tail
; ============================================================================
; (cons-atom head tail) -> (head ...tail)
!(cons-atom a (b c))
; Result: (a b c)

!(cons-atom + (5 3))
; Result: (+ 5 3) - useful for building function calls dynamically

!(cons-atom first (second third))
; Result: (first second third)

; Empty tail creates single-element expression
!(cons-atom x ())
; Result: (x)

; ============================================================================
; 2. decons-atom - Deconstruct expressions into head and tail
; ============================================================================
; (decons-atom (head tail ...)) -> (head (tail ...))
!(decons-atom (a b c))
; Result: (a (b c))

!(decons-atom (sum 5 7))
; Result: (sum (5 7))

; Single element returns head and empty tail
!(decons-atom (single))
; Result: (single ())

; ============================================================================
; 3. car-atom - Extract the first element (head)
; ============================================================================
; (car-atom (head tail ...)) -> head
!(car-atom (a b c))
; Result: a

!(car-atom (sum 5 7))
; Result: sum

!(car-atom ((nested expr) other))
; Result: (nested expr) - preserves nested structures

; ============================================================================
; 4. cdr-atom - Extract the tail (all except first element)
; ============================================================================
; (cdr-atom (head tail ...)) -> (tail ...)
!(cdr-atom (a b c))
; Result: (b c)

!(cdr-atom (sum 5 7))
; Result: (5 7)

; Single element returns empty expression
!(cdr-atom (single))
; Result: ()

; ============================================================================
; 5. size-atom - Get the size (number of elements) of an expression
; ============================================================================
; (size-atom expr) -> number
!(size-atom (a b c))
; Result: 3

!(size-atom ())
; Result: 0

!(size-atom (single))
; Result: 1

!(size-atom (a (nested b) c))
; Result: 3 - nested expressions count as single elements

; ============================================================================
; 6. index-atom - Get element at specific index
; ============================================================================
; (index-atom expr index) -> element
!(index-atom (a b c) 0)
; Result: a

!(index-atom (a b c) 1)
; Result: b

!(index-atom (a b c) 2)
; Result: c

!(index-atom (a (b c) d) 1)
; Result: (b c) - returns nested expressions as-is

; ============================================================================
; 7. min-atom - Find minimum value in numeric expression
; ============================================================================
; (min-atom (num1 num2 ...)) -> minimum
!(min-atom (5 2 8 1))
; Result: 1

!(min-atom (42))
; Result: 42

!(min-atom (10 5 20 3 15))
; Result: 3

; ============================================================================
; 8. max-atom - Find maximum value in numeric expression
; ============================================================================
; (max-atom (num1 num2 ...)) -> maximum
!(max-atom (5 2 8 1))
; Result: 8

!(max-atom (42))
; Result: 42

!(max-atom (10 5 20 3 15))
; Result: 20

; ============================================================================
; 9. Combining operations - Practical examples
; ============================================================================

; Build a function call dynamically
(= (build-call $op $args) (cons-atom $op $args))
!(build-call + (10 20))
; Result: (+ 10 20)

; Extract function name and arguments separately
(= (get-op $expr) (car-atom $expr))
(= (get-args $expr) (cdr-atom $expr))

!(get-op (+ 10 20))
; Result: +

!(get-args (+ 10 20))
; Result: (10 20)

; Check if expression has enough arguments
(= (has-enough-args $expr $min) (>= (size-atom $expr) $min))
!(has-enough-args (sum 5 7) 2)
; Result: True

; Get the last element of an expression
(= (last-element $expr) (index-atom $expr (- (size-atom $expr) 1)))
!(last-element (a b c d))
; Result: d

; Find the range (min and max) of numbers
(= (range $nums) (cons-atom min (cons-atom max (cons-atom (min-atom $nums) (cons-atom (max-atom $nums) ())))))
; Note: This is a simplified example - in practice you'd use decons-atom

