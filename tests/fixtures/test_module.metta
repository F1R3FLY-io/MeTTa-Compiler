; Test module for integration testing
; This module defines some basic functions and rules for testing include/import

; Define a simple function
(= (test-add $x $y)
   (+ $x $y))

; Define a constant
(= (test-value) 42)

; Define a type
(: TestType Type)

; Define a rule with a nested expression
(= (test-nested $x)
   (test-add $x (test-value)))
