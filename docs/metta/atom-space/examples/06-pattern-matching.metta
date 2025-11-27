; Example: Advanced Pattern Matching
; Demonstrates various pattern matching techniques

; ========================================
; Basic Pattern Matching
; ========================================

!(println "=== Basic Pattern Matching ===")

; Add facts
(add-atom &self (color apple red))
(add-atom &self (color banana yellow))
(add-atom &self (color grape purple))

; Simple variable binding
!(println "All items with colors:")
!(match &self (color $item $c) ($item is $c))
; Expected: [(apple is red), (banana is yellow), (grape is purple)]

; Specific queries
!(println "\nWhat color is banana?")
!(match &self (color banana $c) $c)
; Expected: [yellow]

!(println "\nWhat is red?")
!(match &self (color $item red) $item)
; Expected: [apple]

; ========================================
; Multi-Variable Patterns
; ========================================

!(println "\n=== Multi-Variable Patterns ===")

(add-atom &self (triangle 3 4 5))
(add-atom &self (triangle 5 12 13))
(add-atom &self (rectangle 4 6))

!(println "All triangles:")
!(match &self (triangle $a $b $c) (sides $a $b $c))
; Expected: [(sides 3 4 5), (sides 5 12 13)]

!(println "\nAll shapes with 2 parameters:")
!(match &self (rectangle $w $h) (w= $w h= $h))
; Expected: [(w= 4 h= 6)]

; ========================================
; Nested Pattern Matching
; ========================================

!(println "\n=== Nested Patterns ===")

(add-atom &self (person (name Alice) (age 30)))
(add-atom &self (person (name Bob) (age 25)))
(add-atom &self (person (name Charlie) (age 35)))

!(println "Extract names:")
!(match &self (person (name $n) (age $a)) $n)
; Expected: [Alice, Bob, Charlie]

!(println "\nExtract name-age pairs:")
!(match &self (person (name $n) (age $a)) ($n $a))
; Expected: [(Alice 30), (Bob 25), (Charlie 35)]

; Deeply nested
(add-atom &self (company
    (name TechCorp)
    (location (city Boston) (state MA))
    (employees 100)))

!(println "\nExtract city from company:")
!(match &self (company $name (location (city $c) $state) $emp) $c)
; Expected: [Boston]

; ========================================
; Partial Patterns
; ========================================

!(println "\n=== Partial Patterns ===")

(add-atom &self (data x 1 y 2 z 3))
(add-atom &self (data a 10 b 20 c 30))

; Match first two fields
!(println "Matching first two fields:")
!(match &self (data $f1 $v1 $f2 $v2 $rest...) ($f1= $v1 $f2= $v2))
; Expected: [(x= 1 y= 2), (a= 10 b= 20)]

; ========================================
; Conditional Pattern Matching
; ========================================

!(println "\n=== Conditional Matching ===")

(add-atom &self (product laptop 1200))
(add-atom &self (product mouse 25))
(add-atom &self (product keyboard 75))
(add-atom &self (product monitor 300))

; Find expensive products (> 100)
!(println "Expensive products (price > 100):")
!(match &self (product $name $price)
    (if (> $price 100)
        ($name costs $price)
        ()))
; Expected: [(laptop costs 1200), (monitor costs 300)]

; Find cheap products (<= 100)
!(println "\nCheap products (price <= 100):")
!(match &self (product $name $price)
    (if (<= $price 100)
        $name
        ()))
; Expected: [mouse, keyboard]

; ========================================
; Pattern Matching with Computation
; ========================================

!(println "\n=== Pattern Matching with Computation ===")

(add-atom &self (inventory item1 10))
(add-atom &self (inventory item2 5))
(add-atom &self (inventory item3 0))

; Compute total value
!(println "Double all inventory counts:")
!(match &self (inventory $item $count) ($item (* $count 2)))
; Expected: [(item1 20), (item2 10), (item3 0)]

; Filter and compute
!(println "\nItems with non-zero inventory:")
!(match &self (inventory $item $count)
    (if (> $count 0)
        ($item has $count)
        ()))
; Expected: [(item1 has 10), (item2 has 5)]

; ========================================
; Complex Filtering
; ========================================

!(println "\n=== Complex Filtering ===")

(add-atom &self (student Alice 85 90 92))
(add-atom &self (student Bob 78 82 88))
(add-atom &self (student Charlie 92 95 98))

; Find students with all grades > 80
!(println "Students with all grades > 80:")
!(match &self (student $name $g1 $g2 $g3)
    (if (and (> $g1 80) (and (> $g2 80) (> $g3 80)))
        $name
        ()))
; Expected: [Alice, Charlie]

; Calculate average grade
!(println "\nStudent averages:")
!(match &self (student $name $g1 $g2 $g3)
    ($name avg (/ (+ (+ $g1 $g2) $g3) 3)))
; Expected: [(Alice avg 89), (Bob avg 82.66), (Charlie avg 95)]

; ========================================
; Pattern Matching for Relationships
; ========================================

!(println "\n=== Relationship Patterns ===")

(add-atom &self (parent Alice Bob))
(add-atom &self (parent Alice Charlie))
(add-atom &self (parent Bob Dave))
(add-atom &self (parent Bob Eve))
(add-atom &self (parent Charlie Frank))

; Find all children of Alice
!(println "Alice's children:")
!(match &self (parent Alice $child) $child)
; Expected: [Bob, Charlie]

; Find all grandchildren of Alice (two-level match)
!(println "\nAlice's grandchildren:")
!(match &self (parent Alice $child)
    (match &self (parent $child $grandchild)
        $grandchild))
; Expected: [Dave, Eve, Frank]

; Find all parents
!(println "\nAll unique parents:")
!(match &self (parent $p $c) $p)
; Expected: [Alice, Alice, Bob, Bob, Charlie] (with duplicates)

; ========================================
; Existential Patterns
; ========================================

!(println "\n=== Existential Queries ===")

; Check if atom exists
!(println "Does Bob have children?")
!(if (match &self (parent Bob $x) True)
    (println "Yes, Bob has children")
    (println "No, Bob has no children"))
; Expected: "Yes, Bob has children"

!(println "\nDoes Grace have children?")
!(if (match &self (parent Grace $x) True)
    (println "Yes, Grace has children")
    (println "No, Grace has no children"))
; Expected: "No, Grace has no children"

; ========================================
; Pattern Matching with Rules
; ========================================

!(println "\n=== Patterns with Rules ===")

; Add facts
(add-atom &self (Human Socrates))
(add-atom &self (Human Plato))

; Add rules
(add-atom &self (= (Mortal $x) (Human $x)))

; Query derived facts
!(println "Who is mortal?")
!(match &self (Human $x) (Mortal $x))
; Expected: [(Mortal Socrates), (Mortal Plato)]

; ========================================
; Wildcard Patterns
; ========================================

!(println "\n=== Wildcard Patterns ===")

(add-atom &self (record 1 data1 extra1))
(add-atom &self (record 2 data2 extra2))

; Use $_ for values we don't care about
!(println "Match records, ignore extras:")
!(match &self (record $id $data $_) ($id $data))
; Expected: [(1 data1), (2 data2)]

; ========================================
; Matching Ground Terms
; ========================================

!(println "\n=== Ground Term Matching ===")

(add-atom &self (type apple fruit))
(add-atom &self (type carrot vegetable))
(add-atom &self (type banana fruit))

; Match specific type
!(println "All fruits:")
!(match &self (type $item fruit) $item)
; Expected: [apple, banana]

; Match specific item
!(println "What type is carrot?")
!(match &self (type carrot $t) $t)
; Expected: [vegetable]

; ========================================
; Collecting and Transforming
; ========================================

!(println "\n=== Collect and Transform ===")

(add-atom &self (point 1 2))
(add-atom &self (point 3 4))
(add-atom &self (point 5 6))

; Collect all x coordinates
!(println "All x coordinates:")
!(match &self (point $x $y) $x)
; Expected: [1, 3, 5]

; Transform points (scale by 2)
!(println "\nScaled points:")
!(match &self (point $x $y) (point (* $x 2) (* $y 2)))
; Expected: [(point 2 4), (point 6 8), (point 10 12)]

; Calculate distances from origin
!(println "\nDistances from origin (Manhattan):")
!(match &self (point $x $y) (+ $x $y))
; Expected: [3, 7, 11]

; ========================================
; Summary
; ========================================

!(println "\n=== Pattern Matching Summary ===")
!(println "Demonstrated:")
!(println "- Basic variable binding")
!(println "- Multi-variable patterns")
!(println "- Nested patterns")
!(println "- Partial patterns")
!(println "- Conditional matching")
!(println "- Pattern-based computation")
!(println "- Complex filtering")
!(println "- Relationship traversal")
!(println "- Existential queries")
!(println "- Wildcard patterns")
!(println "- Collection and transformation")
