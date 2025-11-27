; Example: Atom Space Edge Cases
; Demonstrates edge cases and special behaviors

; ========================================
; Empty Expressions
; ========================================

!(println "=== Empty Expressions ===")

; Empty expression is valid
(add-atom &self ())

!(println "Matching empty expression:")
!(match &self () found)
; Expected: [found]

; Remove empty expression
!(println "Removing empty expression:")
!(remove-atom &self ())
; Expected: True

; ========================================
; Duplicates with AllowDuplication
; ========================================

!(println "\n=== Handling Duplicates ===")

; Add same atom multiple times
(add-atom &self (dup-test 1))
(add-atom &self (dup-test 1))
(add-atom &self (dup-test 1))

!(println "Query returns duplicates:")
!(match &self (dup-test 1) found)
; Expected: [found, found, found]

; Remove only removes one instance
!(println "\nAfter removing once:")
!(remove-atom &self (dup-test 1))
!(match &self (dup-test 1) found)
; Expected: [found, found]

; Remove all instances pattern
!(println "Removing all remaining:")
(= (remove-all $space $atom)
    (if (remove-atom $space $atom)
        (remove-all $space $atom)
        ()))

!(remove-all &self (dup-test 1))

!(println "After remove-all:")
!(match &self (dup-test 1) found)
; Expected: []

; ========================================
; Variable Name Sensitivity
; ========================================

!(println "\n=== Variable Name Sensitivity ===")

; Add rule with variable $x
(add-atom &self (= (func $x) (* $x 2)))

; Try to remove with different variable name
!(println "Removing with wrong variable name ($y):")
!(remove-atom &self (= (func $y) (* $y 2)))
; Expected: False (doesn't match)

; Remove with correct variable name
!(println "Removing with correct variable name ($x):")
!(remove-atom &self (= (func $x) (* $x 2)))
; Expected: True

; ========================================
; Pattern Matching vs Exact Matching
; ========================================

!(println "\n=== Pattern vs Exact Matching ===")

(add-atom &self (fact Socrates))

; Pattern matching: variables match anything
!(println "Pattern match with variable:")
!(match &self (fact $x) $x)
; Expected: [Socrates]

; Exact matching: variables are literal
!(println "Exact remove with variable (won't work):")
!(remove-atom &self (fact $x))
; Expected: False (tries to remove literal $x)

; Must use exact atom
!(println "Exact remove with ground term:")
!(remove-atom &self (fact Socrates))
; Expected: True

; ========================================
; Concurrent Modification Issues
; ========================================

!(println "\n=== Concurrent Modification ===")

; Add test atoms
(add-atom &self (test 1))
(add-atom &self (test 2))
(add-atom &self (test 3))

; DANGEROUS: modifying while iterating
!(println "Dangerous: removing while matching")
; !(match &self (test $x)
;     (remove-atom &self (test $x)))
; This may skip atoms or behave unexpectedly!

; SAFE: collect first, then modify
!(println "Safe pattern: collect then remove")
!(bind! &to-remove (match &self (test $x) (test $x)))
!(match &to-remove $atom
    (remove-atom &self $atom))

!(println "After safe removal:")
!(match &self (test $x) $x)
; Expected: [] (all removed)

; ========================================
; Empty Space Operations
; ========================================

!(println "\n=== Empty Space Operations ===")

!(bind! &empty (new-space))

!(println "get-atoms on empty space:")
!(get-atoms &empty)
; Expected: []

!(println "match on empty space:")
!(match &empty $x $x)
; Expected: []

!(println "remove-atom on empty space:")
!(remove-atom &empty (anything))
; Expected: False

; ========================================
; Self-Referential Spaces
; ========================================

!(println "\n=== Self-Referential Space ===")

; Add space to itself
(add-atom &self &self)

!(println "Space contains itself:")
!(match &self &self found)
; Expected: [found]

; Remove self-reference
!(println "Removing self-reference:")
!(remove-atom &self &self)
; Expected: True

; ========================================
; Deeply Nested Structures
; ========================================

!(println "\n=== Deeply Nested Atoms ===")

; Add deeply nested atom
(add-atom &self (level1 (level2 (level3 (level4 (level5 deep-value))))))

!(println "Query deeply nested:")
!(match &self (level1 (level2 (level3 (level4 (level5 $val))))) $val)
; Expected: [deep-value]

; Remove deeply nested
!(println "Remove deeply nested:")
!(remove-atom &self (level1 (level2 (level3 (level4 (level5 deep-value))))))
; Expected: True

; ========================================
; Query Result Order
; ========================================

!(println "\n=== Non-Deterministic Ordering ===")

; Add atoms in order
(add-atom &self (ordered A))
(add-atom &self (ordered B))
(add-atom &self (ordered C))

!(println "Query results (order not guaranteed):")
!(match &self (ordered $x) $x)
; Expected: [A, B, C] but order may vary

; ========================================
; Removing Non-Existent Atoms
; ========================================

!(println "\n=== Removing Non-Existent Atoms ===")

!(println "Remove non-existent atom:")
!(remove-atom &self (never-added))
; Expected: False (not an error)

; ========================================
; Large Number of Atoms
; ========================================

!(println "\n=== Performance with Many Atoms ===")

; Add many atoms
(= (add-many 0) ())
(= (add-many $n)
    (if (> $n 0)
        (seq (add-atom &self (num $n))
             (add-many (- $n 1)))
        ()))

!(println "Adding 100 atoms...")
!(add-many 100)

!(println "Query specific atom:")
!(match &self (num 50) found)
; Expected: [found]

; Clean up
!(bind! &nums (match &self (num $x) (num $x)))
!(match &nums $atom (remove-atom &self $atom))

; ========================================
; Type Mismatches
; ========================================

!(println "\n=== Type Errors ===")

; These would cause errors:
; !(get-atoms not-a-space)    ; Error: Expected Space type
; !(add-atom 42 (fact 1))     ; Error: Expected Space type
; (add-atom &self)            ; Error: Missing argument

!(println "Type errors demonstrated in comments (would crash if run)")

; ========================================
; Update Pattern (No Atomicity)
; ========================================

!(println "\n=== Non-Atomic Updates ===")

(add-atom &self (value original))

!(println "Original value:")
!(match &self (value $v) $v)
; Expected: [original]

; Update: remove then add (not atomic!)
(remove-atom &self (value original))
(add-atom &self (value updated))
; If crash occurs between these, value is lost!

!(println "Updated value:")
!(match &self (value $v) $v)
; Expected: [updated]

; Safer: add first, then remove
(add-atom &self (value safer))
(remove-atom &self (value updated))

!(println "Safer update result:")
!(match &self (value $v) $v)
; Expected: [safer]

; ========================================
; Summary
; ========================================

!(println "\n=== Edge Cases Summary ===")
!(println "Covered:")
!(println "- Empty expressions and spaces")
!(println "- Duplicate handling")
!(println "- Variable name sensitivity")
!(println "- Pattern vs exact matching")
!(println "- Concurrent modification dangers")
!(println "- Self-referential structures")
!(println "- Deep nesting")
!(println "- Non-deterministic ordering")
!(println "- Non-atomic updates")
