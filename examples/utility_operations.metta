; Test file for utility operations (Phase 7: mmverify compatibility)

; Test empty - returns no values
!(empty)

; Test get-metatype for different atom types
!(get-metatype foo)           ; Symbol
!(get-metatype $x)            ; Variable
!(get-metatype 42)            ; Grounded
!(get-metatype 3.14)          ; Grounded
!(get-metatype "hello")       ; Grounded
!(get-metatype True)          ; Grounded
!(get-metatype (list 1 2 3))  ; Expression

; Test empty in conditional
!(if (== 1 2)
    "yes"
    (empty))

; Test get-metatype with evaluated expressions
!(get-metatype (+ 1 2))       ; Result is Grounded (Long)
