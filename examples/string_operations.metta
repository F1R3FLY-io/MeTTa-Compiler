; Test file for string operations (Phase 6: mmverify compatibility)

; Test repr - convert atom to string representation
!(repr 42)
!(repr True)
!(repr "hello")
!(repr (list 1 2 3))

; Test format-args - string formatting with {} placeholders
!(format-args "Hello, {}!" ("World"))
!(format-args "The answer is {}." (42))
!(format-args "{} + {} = {}" (1 2 3))

; Test format-args with evaluated expressions
!(format-args "Result: {}" ((+ 10 20)))

; Test escaped braces
!(format-args "Use {{}} for placeholders" ())

; Test string comparison with < (lexicographic)
!(< "apple" "banana")
!(< "banana" "apple")
!(< "aaa" "aab")

; Test string comparison with other operators
!(<= "apple" "apple")
!(> "zebra" "apple")
!(>= "apple" "apple")
!(== "hello" "hello")
!(!= "hello" "world")

; Test integer comparison still works
!(< 1 2)
!(> 10 5)
!(== 42 42)

; Test mixed type comparison (should error)
; Note: This would produce an error about type mismatch
; !(< "hello" 42)
