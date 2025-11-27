; ============================================================================
; Expression Pattern Matching Examples
; ============================================================================
; Demonstrates pattern matching on expression structures including:
; - Multi-level nesting
; - Complex expression patterns
; - Head and tail matching
; - Structural decomposition

; Setup: Create knowledge base
; ============================================================================

; Nested data structures
(add-atom &self (address (person Alice) (city Boston) (state MA)))
(add-atom &self (address (person Bob) (city Austin) (state TX)))
(add-atom &self (contact (name Charlie) (email charlie@example.com) (phone 555-1234)))

; Example 1: Nested Expression Pattern
; ============================================================================
; Extract from nested structure

!(match &self
    (address (person $name) (city $c) (state $s))
    ($name lives in $c $s))
; Expected: [(Alice lives in Boston MA),
;           (Bob lives in Austin TX)]

; Example 2: Deep Nesting
; ============================================================================
; Pattern matches multiple levels deep

(add-atom &self (company
                    (info (name TechCorp) (founded 1995))
                    (address (city Seattle) (state WA))))

!(match &self
    (company (info (name $n) (founded $y)) (address $addr))
    ($n was founded in $y at $addr))
; Expected: [(TechCorp was founded in 1995 at (city Seattle) (state WA))]

; Example 3: Partial Structure Matching
; ============================================================================
; Match part of structure, capture rest

!(match &self
    (contact (name $n) $rest)
    (has-contact $n with-details $rest))
; Expected: [(has-contact Charlie with-details (email charlie@example.com) (phone 555-1234))]

; Example 4: Expression Head Matching
; ============================================================================
; Match based on expression head (first element)

(add-atom &self (calculate + 5 3))
(add-atom &self (calculate - 10 4))
(add-atom &self (calculate * 6 7))

!(match &self
    (calculate $op $a $b)
    (operation $op with-args $a $b))
; Expected: [(operation + with-args 5 3),
;           (operation - with-args 10 4),
;           (operation * with-args 6 7)]

; Example 5: Same Structure, Different Data
; ============================================================================
; Pattern matches structure regardless of data

(add-atom &self (point (x 10) (y 20)))
(add-atom &self (point (x 5) (y 15)))
(add-atom &self (point (x -3) (y 7)))

!(match &self
    (point (x $x_coord) (y $y_coord))
    (coordinates $x_coord $y_coord))
; Expected: [(coordinates 10 20),
;           (coordinates 5 15),
;           (coordinates -3 7)]

; Example 6: Expression Transformation
; ============================================================================
; Match and transform structure

(add-atom &self (old-format (first-name John) (last-name Doe)))
(add-atom &self (old-format (first-name Jane) (last-name Smith)))

!(match &self
    (old-format (first-name $f) (last-name $l))
    (new-format (full-name $f $l)))
; Expected: [(new-format (full-name John Doe)),
;           (new-format (full-name Jane Smith))]

; Example 7: Heterogeneous Expressions
; ============================================================================
; Different expression types in space

(add-atom &self (vehicle (car (make Toyota) (model Camry))))
(add-atom &self (vehicle (bike (type Mountain) (gears 21))))
(add-atom &self (vehicle (car (make Honda) (model Civic))))

; Match only cars
!(match &self
    (vehicle (car (make $m) (model $mo)))
    (automobile $m $mo))
; Expected: [(automobile Toyota Camry),
;           (automobile Honda Civic)]

; Match only bikes
!(match &self
    (vehicle (bike (type $t) (gears $g)))
    (bicycle $t with $g gears))
; Expected: [(bicycle Mountain with 21 gears)]

; Example 8: Mixed Ground and Variable Terms
; ============================================================================
; Some positions fixed, others variable

(add-atom &self (located Boston MA USA))
(add-atom &self (located Austin TX USA))
(add-atom &self (located Toronto ON Canada))

; Find USA cities only
!(match &self
    (located $city $state USA)
    (us-city $city $state))
; Expected: [(us-city Boston MA),
;           (us-city Austin TX)]

; Example 9: Expression Length Sensitivity
; ============================================================================
; Pattern must match expression length

(add-atom &self (pair A B))
(add-atom &self (triple X Y Z))
(add-atom &self (pair C D))

!(match &self (pair $a $b) ($a paired-with $b))
; Expected: [(A paired-with B),
;           (C paired-with D)]
; Note: (triple X Y Z) does NOT match (different length)

; Example 10: Complex Nested Query
; ============================================================================
; Multi-level pattern with multiple variables

(add-atom &self (organization
                    (name AcmeCorp)
                    (division
                        (name Engineering)
                        (head (name Alice) (title VP)))
                    (location Seattle)))

!(match &self
    (organization
        (name $org)
        (division (name $div) (head (name $head) (title $title)))
        (location $loc))
    (structure $org has $div led-by $head who-is $title in $loc))
; Expected: [(structure AcmeCorp has Engineering led-by Alice who-is VP in Seattle)]

; Example 11: Empty Expression
; ============================================================================
; Match empty expressions

(add-atom &self ())
(add-atom &self (non-empty value))

!(match &self () found-empty)
; Expected: [found-empty]
; Only matches the empty expression ()

; Example 12: Single Element Expression
; ============================================================================
; Expression with one element

(add-atom &self (singleton))
(add-atom &self (another-single))

!(match &self ($x) (single-element $x))
; Expected: [(single-element singleton),
;           (single-element another-single)]

; ============================================================================
; Summary
; ============================================================================
; Expression patterns demonstrated:
; - Nested structure matching (multi-level)
; - Partial structure matching
; - Head-based matching
; - Length-sensitive matching
; - Structure transformation
; - Mixed ground/variable patterns
; - Complex nested queries
