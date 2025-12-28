; Module A for testing transitive imports
; Defines a function that will be imported by module B

(= (func-a) a-value)

(= (add-from-a $x)
   (+ $x 10))
