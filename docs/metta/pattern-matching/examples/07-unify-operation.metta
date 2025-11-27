; ============================================================================
; Unify Operation Examples
; ============================================================================
; Demonstrates the unify operation for conditional pattern matching including:
; - Basic unify syntax
; - Pattern testing
; - Conditional branching
; - Comparison with match operation
; - Complex unify patterns

; Unify Syntax: (unify <atom> <pattern> <then> <else>)
; If atom unifies with pattern (with bindings), evaluates <then>
; Otherwise, evaluates <else>

; Example 1: Simple Unify
; ============================================================================
; Test if atom matches pattern

!(unify 42 $x
    $x              ; Then: return bound value
    "no match")     ; Else: return this
; Bindings: {$x ← 42}
; Expected: 42

; Example 2: Unify with Constant
; ============================================================================
; Test exact match

!(unify 42 42
    "matched"
    "no match")
; Bindings: {} (no variables)
; Expected: "matched"

!(unify 42 99
    "matched"
    "no match")
; No match (42 ≠ 99)
; Expected: "no match"

; Example 3: Unify with Expression
; ============================================================================
; Pattern match on structure

!(unify (Human Socrates) (Human $x)
    (name is $x)
    "not human")
; Bindings: {$x ← Socrates}
; Expected: (name is Socrates)

!(unify (Animal Dog) (Human $x)
    (name is $x)
    "not human")
; No match (Animal ≠ Human)
; Expected: "not human"

; Example 4: Multiple Variables
; ============================================================================
; Bind multiple variables

!(unify (age John 30) (age $person $years)
    ($person is $years years old)
    "invalid format")
; Bindings: {$person ← John, $years ← 30}
; Expected: (John is 30 years old)

; Example 5: Shared Variable Constraint
; ============================================================================
; Same variable must unify consistently

!(unify (same A A) (same $x $x)
    (both-are $x)
    "different")
; Bindings: {$x ← A} (both positions match)
; Expected: (both-are A)

!(unify (same A B) (same $x $x)
    (both-are $x)
    "different")
; No match (A ≠ B, can't bind $x to both)
; Expected: "different"

; Example 6: Nested Structure Unify
; ============================================================================
; Match nested expressions

!(unify
    (person (name Alice) (age 30))
    (person (name $n) (age $a))
    (person-data $n $a)
    "not a person")
; Bindings: {$n ← Alice, $a ← 30}
; Expected: (person-data Alice 30)

; Example 7: Unify vs Match Comparison
; ============================================================================

; Setup for comparison
(add-atom &self (Human Socrates))
(add-atom &self (Human Plato))

; match: queries space, returns all matches
!(match &self (Human $x) $x)
; Expected: [Socrates, Plato]
; Multiple results from space

; unify: tests single atom, returns single result
!(unify (Human Socrates) (Human $x) $x "no match")
; Expected: Socrates
; Single result, no space query

; Example 8: Conditional Logic with Unify
; ============================================================================
; Use unify for branching

(= (process-data $data)
    (unify $data (number $n)
        ; Then: it's a number
        (compute (* $n 2))
        ; Else: not a number
        (unify $data (text $t)
            ; Then: it's text
            (display $t)
            ; Else: unknown type
            (error "unknown type"))))

!(process-data (number 5))
; Expected: (compute 10)

!(process-data (text "hello"))
; Expected: (display "hello")

; !(process-data (unknown foo))
; Expected: (error "unknown type")

; Example 9: Unify with Guards
; ============================================================================
; Add conditions in then branch

!(unify (age $person $years) (age John $y)
    (if (> $y 18)
        (adult John age $y)
        (minor John age $y))
    "no match")
; Depends on $years value

; With specific value:
!(unify (age John 25) (age $p $y)
    (if (> $y 18)
        (adult $p age $y)
        (minor $p age $y))
    "no match")
; Bindings: {$p ← John, $y ← 25}
; Expected: (adult John age 25)

; Example 10: Chained Unify
; ============================================================================
; Multiple unify operations in sequence

!(unify (command "save" "file.txt") (command $cmd $arg)
    (unify $cmd "save"
        (save-file $arg)
        (unify $cmd "load"
            (load-file $arg)
            (unknown-command $cmd)))
    "invalid format")
; Bindings: {$cmd ← "save", $arg ← "file.txt"}
; Inner unify checks command type
; Expected: (save-file "file.txt")

; Example 11: Unify with Computed Then Branch
; ============================================================================
; Then branch can compute values

!(unify (circle (radius 5)) (circle (radius $r))
    (area is (* 3.14159 $r $r))
    "not a circle")
; Bindings: {$r ← 5}
; Computed: π × 5² ≈ 78.54
; Expected: (area is 78.5397...)

; Example 12: Pattern Validation
; ============================================================================
; Use unify to validate structure

(= (validate-input $input)
    (unify $input (request (method $m) (url $u) (body $b))
        (valid-request $m $u $b)
        (error "invalid request format")))

!(validate-input (request (method GET) (url "/api") (body None)))
; Expected: (valid-request GET "/api" None)

; !(validate-input (invalid-structure foo))
; Expected: (error "invalid request format")

; Example 13: Type Checking with Unify
; ============================================================================
; Check if value matches expected type pattern

!(unify (value 42 Number) (value $v Number)
    (valid-number $v)
    "type mismatch")
; Expected: (valid-number 42)

!(unify (value "text" Number) (value $v Number)
    (valid-number $v)
    "type mismatch")
; No match ("text" present but pattern expects Number)
; Expected: "type mismatch"

; Example 14: Extracting from Complex Structure
; ============================================================================
; Extract specific fields from nested structure

!(unify
    (response
        (status 200)
        (headers (content-type "application/json"))
        (body (data (user Alice))))
    (response
        (status $code)
        (headers $h)
        (body (data (user $u))))
    (success user $u status $code)
    (error-response))
; Bindings: {$code ← 200, $h ← (content-type ...), $u ← Alice}
; Expected: (success user Alice status 200)

; Example 15: Empty Structure Unify
; ============================================================================
; Match empty expressions

!(unify () () "empty" "not empty")
; Expected: "empty"

!(unify (non-empty) () "empty" "not empty")
; Expected: "not empty"

; Example 16: Variable-Only Pattern
; ============================================================================
; Pattern is pure variable (matches anything)

!(unify anything $x (matched $x) "no match")
; Bindings: {$x ← anything}
; Expected: (matched anything)

!(unify (complex (nested structure)) $x (matched $x) "no match")
; Bindings: {$x ← (complex (nested structure))}
; Expected: (matched (complex (nested structure)))

; Example 17: Unify with Side Effects
; ============================================================================
; Then/else branches can have effects

; (= (log-and-process $data)
;     (unify $data (valid $value)
;         (do
;             (println "Processing valid data: " $value)
;             (process $value))
;         (do
;             (println "Invalid data: " $data)
;             (error "validation failed"))))

; Example 18: Destructuring with Unify
; ============================================================================
; Extract parts of structure for processing

!(unify (point 10 20) (point $x $y)
    (+ $x $y)  ; Compute sum of coordinates
    0)
; Bindings: {$x ← 10, $y ← 20}
; Expected: 30

; Example 19: Unify with Default Values
; ============================================================================
; Provide defaults in else branch

(= (get-age $person-data)
    (unify $person-data (person (name $n) (age $a))
        $a              ; Return age if present
        unknown-age))   ; Default if structure doesn't match

!(get-age (person (name Alice) (age 30)))
; Expected: 30

!(get-age (person (name Bob)))
; Expected: unknown-age

; Example 20: Practical Example - Config Parser
; ============================================================================
; Parse and validate configuration

(= (parse-config $config)
    (unify $config
        (config (database (host $h) (port $p)) (timeout $t))
        ; Valid config
        (db-settings host $h port $p timeout $t)
        ; Invalid config
        (unify $config
            (config (database $db_data) $rest)
            (error "invalid database config")
            (error "not a config"))))

!(parse-config
    (config (database (host "localhost") (port 5432)) (timeout 30)))
; Expected: (db-settings host "localhost" port 5432 timeout 30)

; !(parse-config
;     (config (database (incomplete)) (timeout 30)))
; Expected: (error "invalid database config")

; ============================================================================
; Summary
; ============================================================================
; Unify operation demonstrated:
; - Syntax: (unify <atom> <pattern> <then> <else>)
; - Tests single atom against pattern
; - Returns single result (not list like match)
; - Bindings applied in then branch
; - Else branch executed on no match
; - Useful for:
;   * Conditional logic based on structure
;   * Input validation
;   * Type checking
;   * Extracting from specific structures
;   * Pattern-based branching
; - Difference from match:
;   * match: queries space, multiple results
;   * unify: tests atom, single result with branching
