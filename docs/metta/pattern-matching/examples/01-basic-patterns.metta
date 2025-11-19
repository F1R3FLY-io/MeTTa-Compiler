; ============================================================================
; Basic Pattern Matching Examples
; ============================================================================
; Demonstrates fundamental pattern matching in MeTTa including:
; - Variable patterns
; - Ground term matching
; - Simple expression patterns
; - Basic queries

; Setup: Create knowledge base
; ============================================================================

; Add some basic facts
(add-atom &self (Human Socrates))
(add-atom &self (Human Plato))
(add-atom &self (Human Aristotle))
(add-atom &self (philosopher Socrates))
(add-atom &self (philosopher Plato))

; Example 1: Variable Pattern
; ============================================================================
; Pattern: (Human $x)
; Matches any atom starting with "Human"
; Variables (like $x) bind to matching values

!(match &self (Human $x) $x)
; Expected: [Socrates, Plato, Aristotle]
; Extracts all humans from knowledge base

; Example 2: Ground Term Pattern
; ============================================================================
; Pattern: (Human Socrates)
; Fully concrete pattern with no variables
; Returns constant template for each match

!(match &self (Human Socrates) found)
; Expected: [found]
; Returns "found" if atom exists

; Example 3: Partial Ground Pattern
; ============================================================================
; Pattern with ground prefix and variable

!(match &self (philosopher $name) (is-philosopher $name))
; Expected: [(is-philosopher Socrates), (is-philosopher Plato)]
; Constructs results using template

; Example 4: Variable in Multiple Positions
; ============================================================================
; Same variable used multiple times creates constraint

(add-atom &self (same red red))
(add-atom &self (same blue blue))
(add-atom &self (same red blue))

!(match &self (same $x $x) $x)
; Expected: [red, blue]
; Only matches where both positions are equal
; (same red blue) does NOT match

; Example 5: Simple Expression Pattern
; ============================================================================
; Pattern matching with expression structure

(add-atom &self (age Socrates 70))
(add-atom &self (age Plato 80))
(add-atom &self (age Aristotle 62))

!(match &self (age $person $years) ($person is $years years old))
; Expected: [(Socrates is 70 years old),
;           (Plato is 80 years old),
;           (Aristotle is 62 years old)]

; Example 6: Wildcard Pattern
; ============================================================================
; Use variables you don't care about (convention: $_)

!(match &self (age $_ $years) $years)
; Expected: [70, 80, 62]
; Extracts just ages, ignoring names

; Example 7: Nested Pattern
; ============================================================================
; Pattern matching in nested structures

(add-atom &self (person (name Alice) (age 30)))
(add-atom &self (person (name Bob) (age 25)))

!(match &self (person (name $n) (age $a)) (person-info $n $a))
; Expected: [(person-info Alice 30), (person-info Bob 25)]

; Example 8: Constant Template
; ============================================================================
; Template can be constant (no variables)

!(match &self (Human $x) found-human)
; Expected: [found-human, found-human, found-human]
; One result per match, all same value

; Example 9: Empty Result
; ============================================================================
; No match returns empty list

!(match &self (Dog $x) $x)
; Expected: []
; No atoms match (Dog ...), so empty result

; Example 10: Multiple Variable Extraction
; ============================================================================
; Extract multiple variables from pattern

(add-atom &self (parent Alice Bob))
(add-atom &self (parent Alice Charlie))
(add-atom &self (parent Bob David))

!(match &self (parent $p $c) (child $c has-parent $p))
; Expected: [(child Bob has-parent Alice),
;           (child Charlie has-parent Alice),
;           (child David has-parent Bob)]

; ============================================================================
; Summary
; ============================================================================
; Basic patterns demonstrated:
; - Variables ($x) bind to values
; - Ground terms match exactly
; - Same variable in multiple positions creates equality constraint
; - Templates construct results from bound variables
; - Empty results indicate no matches
; - Nested patterns work on structured data
