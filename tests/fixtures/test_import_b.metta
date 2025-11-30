; Module B for testing transitive imports
; Imports module A, and defines its own functions

; Import module A
!(include "test_import_a.metta")

(= (func-b) b-value)

(= (add-from-b $x)
   (+ (add-from-a $x) 5))
