; Example: Working with Multiple Atom Spaces
; Demonstrates creating and managing multiple independent spaces

; ========================================
; Creating Spaces
; ========================================

!(println "=== Creating Multiple Spaces ===")

; Create separate spaces
!(bind! &facts (new-space))
!(bind! &rules (new-space))
!(bind! &temp (new-space))

!(println "Created three spaces: &facts, &rules, &temp")

; ========================================
; Space Isolation
; ========================================

!(println "\n=== Space Isolation ===")

; Add facts to fact space
(add-atom &facts (Human Socrates))
(add-atom &facts (Human Plato))
(add-atom &facts (age Socrates 70))

; Add rules to rule space
(add-atom &rules (= (mortal $x) (Human $x)))
(add-atom &rules (= (double $x) (* $x 2)))

; Verify isolation
!(println "Facts in &facts space:")
!(get-atoms &facts)
; Expected: [(Human Socrates), (Human Plato), (age Socrates 70)]

!(println "\nRules in &rules space:")
!(get-atoms &rules)
; Expected: [(= (mortal $x) (Human $x)), (= (double $x) (* $x 2))]

!(println "\n&temp space (should be empty):")
!(get-atoms &temp)
; Expected: []

; ========================================
; Cross-Space Queries
; ========================================

!(println "\n=== Cross-Space Queries ===")

; Query specific spaces
!(println "Humans in &facts:")
!(match &facts (Human $x) $x)
; Expected: [Socrates, Plato]

!(println "\nHumans in &rules (should be none):")
!(match &rules (Human $x) $x)
; Expected: []

; ========================================
; Copying Between Spaces
; ========================================

!(println "\n=== Copying Between Spaces ===")

; Copy all facts to temp space
!(println "Copying facts to temp space...")
!(match &facts $atom
    (add-atom &temp $atom))

!(println "Contents of &temp after copy:")
!(get-atoms &temp)
; Expected: [(Human Socrates), (Human Plato), (age Socrates 70)]

; Filtered copy
!(bind! &humans-only (new-space))
!(match &facts (Human $x)
    (add-atom &humans-only (Human $x)))

!(println "\nHumans-only space:")
!(get-atoms &humans-only)
; Expected: [(Human Socrates), (Human Plato)]

; ========================================
; Merging Spaces
; ========================================

!(println "\n=== Merging Spaces ===")

!(bind! &merged (new-space))

; Merge facts and rules
!(match &facts $atom (add-atom &merged $atom))
!(match &rules $atom (add-atom &merged $atom))

!(println "Merged space contents:")
!(get-atoms &merged)
; Expected: All atoms from both &facts and &rules

; ========================================
; Space Difference
; ========================================

!(println "\n=== Space Difference ===")

; Create two spaces with some overlap
!(bind! &space1 (new-space))
!(bind! &space2 (new-space))

(add-atom &space1 (item A))
(add-atom &space1 (item B))
(add-atom &space1 (item C))

(add-atom &space2 (item B))
(add-atom &space2 (item C))
(add-atom &space2 (item D))

; Find items in space1 but not in space2
!(println "Items in &space1 but not in &space2:")
!(match &space1 $item
    (if (not (match &space2 $item True))
        $item
        ()))
; Expected: [(item A)]

; ========================================
; Space Intersection
; ========================================

!(println "\n=== Space Intersection ===")

; Find items in both spaces
!(println "Items in both &space1 and &space2:")
!(match &space1 $item
    (if (match &space2 $item True)
        $item
        ()))
; Expected: [(item B), (item C)]

; ========================================
; Temporary Workspaces
; ========================================

!(println "\n=== Temporary Workspaces ===")

; Create workspace for computation
!(bind! &workspace (new-space))

; Add intermediate results
(add-atom &workspace (step-1 42))
(add-atom &workspace (step-2 84))
(add-atom &workspace (step-3 126))

!(println "Workspace contents:")
!(get-atoms &workspace)

; Extract final result
!(println "Final result:")
!(match &workspace (step-3 $result) $result)
; Expected: [126]

; Workspace can be discarded (goes out of scope)

; ========================================
; Versioning Pattern
; ========================================

!(println "\n=== Versioning Pattern ===")

; Create version 1
!(bind! &v1 (new-space))
(add-atom &v1 (config timeout 30))
(add-atom &v1 (config retries 3))

!(println "Version 1:")
!(get-atoms &v1)

; Create version 2 (copy v1 + modifications)
!(bind! &v2 (new-space))
!(match &v1 $atom (add-atom &v2 $atom))
(remove-atom &v2 (config timeout 30))
(add-atom &v2 (config timeout 60))  ; Updated value

!(println "\nVersion 2:")
!(get-atoms &v2)
; Expected: [(config retries 3), (config timeout 60)]

!(println "\nVersion 1 unchanged:")
!(get-atoms &v1)
; Expected: [(config timeout 30), (config retries 3)]

; ========================================
; Space Partitioning for Performance
; ========================================

!(println "\n=== Space Partitioning ===")

; Partition by category
!(bind! &animals (new-space))
!(bind! &plants (new-space))
!(bind! &minerals (new-space))

(add-atom &animals (cat Fluffy))
(add-atom &animals (dog Buddy))
(add-atom &plants (tree Oak))
(add-atom &plants (flower Rose))
(add-atom &minerals (rock Granite))

; Queries are faster in partitioned spaces
!(println "All animals:")
!(get-atoms &animals)
; Expected: [(cat Fluffy), (dog Buddy)]

!(println "\nAll plants:")
!(get-atoms &plants)
; Expected: [(tree Oak), (flower Rose)]

; ========================================
; Module-Like Organization
; ========================================

!(println "\n=== Module-Like Organization ===")

; Module A (math functions)
!(bind! &module-math (new-space))
(add-atom &module-math (= (add $x $y) (+ $x $y)))
(add-atom &module-math (= (mul $x $y) (* $x $y)))

; Module B (string functions)
!(bind! &module-string (new-space))
(add-atom &module-string (= (concat $s1 $s2) (str-concat $s1 $s2)))

; Import from module (copy to main space)
!(println "Importing math functions to main space:")
!(match &module-math (= ($fname $args...) $result)
    (add-atom &self (= ($fname $args...) $result)))

!(println "\nMain space now has:")
!(match &self (= ($fname $args...) $result) $fname)
; Expected: [add, mul]

; ========================================
; Testing with Isolated Spaces
; ========================================

!(println "\n=== Testing Pattern ===")

; Create test space
!(bind! &test (new-space))

; Add test data
(add-atom &test (test-data 1))
(add-atom &test (test-data 2))

; Run test
!(println "Test data count:")
!(match &test (test-data $x) $x)
; Expected: [1, 2]

; Test passes, discard space
; (Space goes out of scope when no longer referenced)

; ========================================
; Summary
; ========================================

!(println "\n=== Summary ===")
!(println "Demonstrated:")
!(println "- Creating multiple spaces")
!(println "- Space isolation")
!(println "- Copying and merging")
!(println "- Set operations (difference, intersection)")
!(println "- Temporary workspaces")
!(println "- Versioning")
!(println "- Partitioning for performance")
!(println "- Module-like organization")
