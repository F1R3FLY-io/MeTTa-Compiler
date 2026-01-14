; Example usage of assertion functions

; ===== assertEqual and assertEqualMsg =====
; These evaluate BOTH arguments and compare results

; Basic assertions with literals
!(assertEqual 5 5)
; Returns: ()

!(assertEqual (+ 1 2) 3)
; Returns: ()

; This will fail and return an error
; !(assertEqual 5 10)
; Returns: Error "Assertion failed: results are not equal..."

; Using assertEqualMsg with custom error messages
!(assertEqualMsg (+ 2 3) 5 "Addition test passed")
; Returns: ()

!(assertEqualMsg (* 2 3) 6 "Multiplication test passed")
; Returns: ()

; This will fail with a custom message
; !(assertEqualMsg (+ 1 1) 3 "Math is broken!")
; Returns: Error "Math is broken!"

; Assertions with complex expressions
!(assertEqual (if (< 1 2) "yes" "no") "yes")
; Returns: ()

; Comparing s-expressions
!(assertEqual (a b c) (a b c))
; Returns: ()

; String comparisons
!(assertEqualMsg "hello" "hello" "String equality test")
; Returns: ()

; ===== assertEqualToResult and assertEqualToResultMsg =====
; These evaluate ONLY the first argument, treating the second as literal expected results

; Compare evaluated expression with literal result
!(assertEqualToResult (+ 1 2) 3)
; Returns: () - evaluates (+ 1 2) to 3, compares with literal 3

!(assertEqualToResult (* 2 5) 10)
; Returns: ()

; This demonstrates the difference - assertEqual evaluates both sides
!(assertEqual (+ 1 2) (+ 1 2))
; Returns: () - both sides evaluate to 3

; But assertEqualToResult does NOT evaluate the second argument
; !(assertEqualToResult (+ 1 2) (+ 1 2))
; Returns: Error - evaluates (+ 1 2) to 3, compares with literal expression (+ 1 2), which fails!

; With custom error messages
!(assertEqualToResultMsg (+ 10 20) 30 "Addition works correctly")
; Returns: ()

; !(assertEqualToResultMsg (+ 10 20) 50 "Addition is broken!")
; Returns: Error "Addition is broken!"

; Useful for testing that expressions evaluate to specific literals
!(assertEqualToResult (if True "yes" "no") "yes")
; Returns: ()

!(assertEqualToResultMsg (< 5 10) True "Comparison should return True")
; Returns: ()
