; MORK exec Special Form Demonstration
; Syntax: (exec <priority> <antecedent> <consequent>)
;
; exec executes rules when all antecedent conditions match.
; Antecedents and consequents are always conjunctions.

; ============================================================================
; 1. Empty Antecedent - Always Fires
; ============================================================================
; Empty conjunction (,) means "no preconditions" - always succeeds
!(exec P0 (,) (, (always-true)))

; ============================================================================
; 2. Simple Pattern Match with Unary Antecedent
; ============================================================================
; Define some facts
(= (parent Alice Bob))
(= (parent Bob Carol))

; Rule: If someone is a parent, output a result
!(exec P1 (, (parent Alice Bob)) (, (is-parent Alice)))

; ============================================================================
; 3. Binary Conjunction - Two Conditions
; ============================================================================
; Both conditions must match for rule to fire
!(exec P2 (, (parent Alice Bob) (parent Bob Carol))
         (, (grandparent Alice Carol)))

; ============================================================================
; 4. N-ary Conjunction - Multiple Conditions
; ============================================================================
; All three conditions must be satisfied
!(exec P3 (, (parent Alice Bob) (parent Bob Carol) (parent Carol Dave))
         (, (great-grandparent Alice Dave)))

; ============================================================================
; 5. Variable Binding in Antecedent
; ============================================================================
; Use variables to match patterns and bind values
!(exec P4 (, (parent $x Bob)) (, (parent-of-bob $x)))

; ============================================================================
; 6. Shared Variables Across Goals
; ============================================================================
; Variable $x must unify across both antecedent goals
!(exec P5 (, (parent $x $y) (parent $y $z))
         (, (transitivity $x $z)))

; ============================================================================
; 7. Multiple Results in Consequent
; ============================================================================
; Consequent conjunction produces multiple results
!(exec P6 (,) (, (result1 foo) (result2 bar)))

; ============================================================================
; 8. Operation Consequent - Space Modification
; ============================================================================
; Operations modify the MORK space
!(exec P7 (,) (O (+ (new-fact)) (+ (another-fact))))

; ============================================================================
; 9. Complex Rule with Operations
; ============================================================================
; Remove old fact and add new fact
!(exec P8 (, (temp-data $x)) (O (- (temp-data $x)) (+ (permanent-data $x))))
