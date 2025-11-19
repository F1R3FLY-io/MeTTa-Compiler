; Example: Type Operations
; Demonstrates runtime type introspection

; Define some typed values
(: x Number)
(= x 42)

(: greeting String)
(= greeting "Hello")

; Get types
!(get-type x)        ; → Number
!(get-type greeting) ; → String
!(get-type 42)       ; → Number

; Get meta-types (structural)
!(get-metatype x)         ; → Symbol
!(get-metatype 42)        ; → Grounded
!(get-metatype (foo bar)) ; → Expression
!(get-metatype $var)      ; → Variable

; Check types
!(check-type x Number)   ; → True
!(check-type x String)   ; → False

; Type-directed dispatch
(= (process $value)
   (if (check-type $value Number)
       (process-number $value)
       (if (check-type $value String)
           (process-string $value)
           (process-other $value))))

(= (process-number $n) (* $n 2))
(= (process-string $s) (concat $s "!"))
(= (process-other $x) $x)

!(process 42)      ; → 84
!(process "hello") ; → "hello!"
