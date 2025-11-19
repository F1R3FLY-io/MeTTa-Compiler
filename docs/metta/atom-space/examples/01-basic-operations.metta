; Example: Basic Atom Space Operations
; Demonstrates add-atom, remove-atom, get-atoms, and match

; ========================================
; Adding Atoms
; ========================================

; Add simple facts
(add-atom &self (Human Socrates))
(add-atom &self (Human Plato))
(add-atom &self (Human Aristotle))

; Add relational facts
(add-atom &self (teacher Socrates Plato))
(add-atom &self (teacher Plato Aristotle))

; Add numeric facts
(add-atom &self (age Socrates 70))
(add-atom &self (age Plato 80))

!(println "=== Added atoms ===")

; ========================================
; Querying with get-atoms
; ========================================

!(println "\nAll atoms in space:")
!(get-atoms &self)

; ========================================
; Pattern Matching with match
; ========================================

!(println "\nAll humans:")
!(match &self (Human $x) $x)
; Expected: [Socrates, Plato, Aristotle]

!(println "\nAll teacher relationships:")
!(match &self (teacher $t $s) ($t taught $s))
; Expected: [(Socrates taught Plato), (Plato taught Aristotle)]

!(println "\nFinding ages:")
!(match &self (age $person $years) ($person is $years years old))
; Expected: [(Socrates is 70 years old), (Plato is 80 years old)]

; ========================================
; Complex Patterns
; ========================================

!(println "\nFind Socrates' students:")
!(match &self (teacher Socrates $student) $student)
; Expected: [Plato]

!(println "\nFind who taught Aristotle:")
!(match &self (teacher $teacher Aristotle) $teacher)
; Expected: [Plato]

; ========================================
; Removing Atoms
; ========================================

!(println "\n=== Removing atoms ===")

; Remove a specific fact
!(println "\nRemoving (age Socrates 70):")
!(remove-atom &self (age Socrates 70))
; Expected: True

; Verify removal
!(println "Ages after removal:")
!(match &self (age $person $years) ($person $years))
; Expected: [(Plato 80)]

; Try to remove non-existent atom
!(println "\nTrying to remove non-existent atom:")
!(remove-atom &self (age Socrates 70))
; Expected: False

; ========================================
; Duplicates (if AllowDuplication strategy)
; ========================================

!(println "\n=== Testing duplicates ===")

; Add duplicate
(add-atom &self (fact duplicate-test))
(add-atom &self (fact duplicate-test))
(add-atom &self (fact duplicate-test))

!(println "Matching duplicate fact:")
!(match &self (fact duplicate-test) found)
; Expected: [found, found, found] (with AllowDuplication)

; Remove one instance
!(println "\nRemoving one duplicate:")
!(remove-atom &self (fact duplicate-test))

!(println "After removing one:")
!(match &self (fact duplicate-test) found)
; Expected: [found, found] (two remain)

; ========================================
; Summary
; ========================================

!(println "\n=== Final state ===")
!(println "All atoms remaining:")
!(get-atoms &self)
