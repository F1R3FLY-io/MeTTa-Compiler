; ============================================================================
; Advanced Pattern Matching Examples
; ============================================================================
; Demonstrates advanced pattern matching techniques including:
; - Nested patterns
; - Recursive patterns
; - Pattern guards
; - Meta-patterns
; - Complex query strategies

; Setup: Create knowledge base
; ============================================================================

(add-atom &self (person (name Alice) (age 30) (city Boston)))
(add-atom &self (person (name Bob) (age 25) (city Austin)))
(add-atom &self (company (name TechCorp) (employees 1000) (founded 1995)))

; Example 1: Deep Nested Pattern
; ============================================================================
; Pattern with multiple nesting levels

(add-atom &self (organization
                    (info (name AcmeCorp) (type Private))
                    (location (city Seattle) (state WA))
                    (size (employees 500) (revenue 50M))))

!(match &self
    (organization
        (info (name $n) (type $t))
        (location (city $c) (state $s))
        (size (employees $e) (revenue $r)))
    (company-profile $n $t in $c $s with $e employees and $r revenue))
; Expected: [(company-profile AcmeCorp Private in Seattle WA
;             with 500 employees and 50M revenue)]

; Example 2: Pattern with Shared Variable Across Levels
; ============================================================================
; Same variable at different nesting depths

(add-atom &self (recursive (id X) (contains (ref X))))
(add-atom &self (recursive (id Y) (contains (ref Z))))

!(match &self
    (recursive (id $x) (contains (ref $x)))
    (self-referential $x))
; Constraint: $x must match at both levels
; Expected: [(self-referential X)]
; Note: Y doesn't match (ref Z ≠ id Y)

; Example 3: Pattern Guards with Conditions
; ============================================================================
; Add conditions to constrain matches

(add-atom &self (product (name Widget) (price 19.99)))
(add-atom &self (product (name Gadget) (price 49.99)))
(add-atom &self (product (name Doohickey) (price 9.99)))

!(match &self
    (product (name $n) (price $p))
    (if (> $p 15.0)
        (expensive $n costs $p)
        ()))  ; Filter out
; Expected: [(expensive Widget costs 19.99),
;           (expensive Gadget costs 49.99)]

; Example 4: Recursive Pattern Matching
; ============================================================================
; Pattern that references itself recursively

(= (flatten ()) ())
(= (flatten ($h $t...))
    (if (list? $h)
        (append (flatten $h) (flatten $t))
        (cons $h (flatten $t))))

; !(flatten ((1 2) 3 (4 (5 6))))
; Expected: [1, 2, 3, 4, 5, 6]
; Recursively flattens nested structure

; Example 5: Transitive Closure Pattern
; ============================================================================
; Find all reachable nodes

(add-atom &self (edge A B))
(add-atom &self (edge B C))
(add-atom &self (edge C D))
(add-atom &self (edge A E))

(= (reaches $x $y)
    (match &self (edge $x $y) True))

(= (reaches $x $z)
    (match &self
        (edge $x $y)
        (if (reaches $y $z)
            True
            False)))

!(reaches A D)
; Expected: True (path A → B → C → D)

; Example 6: Path Finding Pattern
; ============================================================================
; Find path between nodes

(= (path $start $end)
    (unify $start $end
        (list $start)  ; Base case: start = end
        ; Recursive case: start → next → ... → end
        (match &self
            (edge $start $next)
            (cons $start (path $next $end)))))

!(path A D)
; Expected: [[A, B, C, D]]

; Example 7: Pattern with Type Guards
; ============================================================================
; Use type checking as constraint

; (: process-number (-> Number %Undefined%))
(= (process-number $x)
    (if (number? $x)
        (* $x 2)
        (error "not a number")))

!(process-number 5)
; Expected: 10

; !(process-number "text")
; Expected: error

; Example 8: Negation Pattern
; ============================================================================
; Find atoms NOT matching pattern

!(match &self
    (person $data)
    (if (not (match &self (company (name $n)) (== $data (name $n))))
        (person-not-company $data)
        ()))
; Expected: All person entries (since none are companies)

; Example 9: Aggregation Pattern
; ============================================================================
; Collect and aggregate results

(add-atom &self (score Alice 95))
(add-atom &self (score Bob 87))
(add-atom &self (score Charlie 92))

(= (average-score)
    (let $scores (match &self (score $_ $s) $s)
        (/ (sum $scores) (length $scores))))

; !(average-score)
; Expected: 91.33... (average of 95, 87, 92)

(= (count-high-scorers)
    (length
        (match &self
            (score $p $s)
            (if (> $s 90) $p ()))))

; !(count-high-scorers)
; Expected: 2 (Alice and Charlie)

; Example 10: Pattern Metaprogramming
; ============================================================================
; Patterns as data

; Store pattern
(add-atom &pattern-space (my-pattern (Human $x)))

; Use stored pattern
!(match &self
    (eval (match &pattern-space (my-pattern $p) $p))
    $x)
; Evaluates stored pattern dynamically

; Example 11: Higher-Order Pattern Functions
; ============================================================================
; Functions that manipulate patterns

(= (filter-by-pattern $pattern)
    (match &self $pattern $pattern))

!(filter-by-pattern (person $data))
; Expected: All person atoms with their data

; Example 12: Complex Structural Constraint
; ============================================================================
; Multiple constraints on structure

(add-atom &self (transaction (id 1) (amount 100) (from Alice) (to Bob)))
(add-atom &self (transaction (id 2) (amount 50) (from Bob) (to Charlie)))
(add-atom &self (transaction (id 3) (amount 200) (from Alice) (to Charlie)))

; Find large transactions from Alice
!(match &self
    (transaction (id $id) (amount $amt) (from Alice) (to $recipient))
    (if (> $amt 100)
        (large-transaction $id to $recipient amount $amt)
        ()))
; Expected: [(large-transaction 3 to Charlie amount 200)]

; Example 13: Pattern with Computed Template
; ============================================================================
; Template contains computations

!(match &self
    (product (name $n) (price $p))
    (with-tax $n costs (* $p 1.1)))  ; Add 10% tax
; Expected: [(with-tax Widget costs 21.989),
;           (with-tax Gadget costs 54.989),
;           (with-tax Doohickey costs 10.989)]

; Example 14: Multi-Level Filtering
; ============================================================================
; Nested filtering with multiple criteria

!(match &self
    (, (person (name $n) (age $a) (city $c))
       (> $a 25))
    (if (== $c Boston)
        (boston-adult $n age $a)
        ()))
; Expected: [(boston-adult Alice age 30)]
; Filters: age > 25 AND city = Boston

; Example 15: Pattern Optimization Example
; ============================================================================
; Demonstrate good vs bad pattern design

; Bad: Variable prefix (can't optimize)
; !(match &self
;     ($relation-type Alice $value)
;     ...)

; Good: Ground prefix (trie can optimize)
!(match &self
    (person (name Alice) $rest)
    (alice-data $rest))
; Expected: [(alice-data (age 30) (city Boston))]

; Example 16: Symmetric Pattern Matching
; ============================================================================
; Find symmetric relationships

(add-atom &self (connected X Y))
(add-atom &self (connected Y X))
(add-atom &self (connected A B))
; (connected B A) not present

!(match &self
    (, (connected $a $b)
       (connected $b $a))
    (bidirectional $a $b))
; Expected: [(bidirectional X Y), (bidirectional Y X)]
; Note: A-B not included (only one direction)

; Example 17: Pattern with Multiple Guards
; ============================================================================
; Chain multiple conditions

!(match &self
    (score $person $s)
    (if (> $s 85)
        (if (< $s 95)
            (grade-B $person $s)
            (grade-A $person $s))
        ()))
; Expected: [(grade-A Alice 95),
;           (grade-B Bob 87),
;           (grade-B Charlie 92)]

; Example 18: Finding Cycles
; ============================================================================
; Detect circular references

(add-atom &self (points-to A B))
(add-atom &self (points-to B C))
(add-atom &self (points-to C A))  ; Cycle!

(= (has-cycle $start $visited)
    (if (member? $start $visited)
        True  ; Cycle detected
        (match &self
            (points-to $start $next)
            (has-cycle $next (cons $start $visited)))))

!(has-cycle A ())
; Expected: True (cycle A → B → C → A)

; Example 19: Join Operation Pattern
; ============================================================================
; SQL-like join using patterns

(add-atom &self (employee (id 1) (name Alice) (dept-id 10)))
(add-atom &self (employee (id 2) (name Bob) (dept-id 20)))
(add-atom &self (department (id 10) (name Engineering)))
(add-atom &self (department (id 20) (name Sales)))

!(match &self
    (, (employee (id $_) (name $emp_name) (dept-id $dept_id))
       (department (id $dept_id) (name $dept_name)))
    (works-in $emp_name department $dept_name))
; Expected: [(works-in Alice department Engineering),
;           (works-in Bob department Sales)]

; Example 20: Pattern with Backtracking
; ============================================================================
; Pattern that explores multiple alternatives

(= (solve-puzzle $state)
    (if (goal? $state)
        $state
        (match &self
            (next-move $state $new_state)
            (solve-puzzle $new_state))))

; Tries all possible next moves until goal found
; Backtracks on failure

; ============================================================================
; Summary
; ============================================================================
; Advanced patterns demonstrated:
; - Deep nested patterns (multi-level matching)
; - Shared variables across nesting levels
; - Pattern guards with conditions
; - Recursive pattern matching
; - Transitive closure and path finding
; - Type-based guards
; - Negation patterns
; - Aggregation patterns
; - Pattern metaprogramming
; - Higher-order pattern functions
; - Complex structural constraints
; - Computed templates
; - Multi-level filtering
; - Pattern optimization strategies
; - Symmetric relationship detection
; - Cycle detection
; - Join operations
; - Backtracking patterns
