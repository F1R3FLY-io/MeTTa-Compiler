; Test file for I/O operations (Phase 5: mmverify compatibility)

; Test println! - prints to stdout and returns Unit
!(println! "Hello from MeTTa!")

; Test println! with numbers
!(println! 42)

; Test println! with boolean
!(println! True)

; Test println! with S-expression (will be evaluated first)
!(println! (+ 1 2))

; Test trace! - prints message to stderr, returns value
!(trace! "Debug value:" 42)

; Test trace! returns the value
!(let $result (trace! "Computing:" (+ 10 20))
    $result)

; Test nop - returns Unit, ignores arguments
!(nop)

; Test nop with arguments (should ignore them)
!(nop 1 2 3 "ignored")

; Test combining I/O with other operations
!(let* ((_ (println! "Step 1"))
        (_ (println! "Step 2"))
        ($value (trace! "Final:" 100)))
    $value)
