; MORK Fact Removal Demonstration
; This example shows how to remove facts from MORK space using the (O (- fact)) operation.
;
; Feature: PR #1 - fact-removal
; Implements: remove_from_space() and (O (- fact))

; ============================================================================
; Part 1: Adding Facts to Space
; ============================================================================

; Add some parent relationships
(exec (0 0) (,) (O (+ (parent Alice Bob))))
(exec (0 1) (,) (O (+ (parent Bob Carol))))
(exec (0 2) (,) (O (+ (parent Carol Dave))))

; Add some temporary data
(exec (0 3) (,) (O (+ (temp data1))))
(exec (0 4) (,) (O (+ (temp data2))))
(exec (0 5) (,) (O (+ (temp data3))))

; ============================================================================
; Part 2: Querying Facts (Before Removal)
; ============================================================================

; Query all parent facts
; Expected: (parent Alice Bob), (parent Bob Carol), (parent Carol Dave)
!(match &self (parent $x $y) (parent $x $y))

; Query all temp facts
; Expected: (temp data1), (temp data2), (temp data3)
!(match &self (temp $d) (temp $d))

; ============================================================================
; Part 3: Removing Specific Facts
; ============================================================================

; Remove one parent fact
(exec (1 0) (,) (O (- (parent Bob Carol))))

; Remove one temp fact
(exec (1 1) (,) (O (- (temp data2))))

; ============================================================================
; Part 4: Querying Facts (After Removal)
; ============================================================================

; Query parent facts again
; Expected: (parent Alice Bob), (parent Carol Dave)
; Note: (parent Bob Carol) should be missing
!(match &self (parent $x $y) (parent $x $y))

; Query temp facts again
; Expected: (temp data1), (temp data3)
; Note: (temp data2) should be missing
!(match &self (temp $d) (temp $d))

; ============================================================================
; Part 5: Removing Duplicate/Self-Referential Facts
; ============================================================================

; This pattern is used in ancestor.mm2 line 49 to remove reflexive incest facts
; Add some incest facts (including self-referential ones)
(exec (2 0) (,) (O (+ (incest Alice Bob))))
(exec (2 1) (,) (O (+ (incest Alice Alice))))
(exec (2 2) (,) (O (+ (incest Bob Bob))))

; Query all incest facts
; Expected: (incest Alice Bob), (incest Alice Alice), (incest Bob Bob)
!(match &self (incest $p $q) (incest $p $q))

; Remove self-referential incest facts (incest $p $p pattern)
; Note: In full implementation, this would use pattern matching:
; (exec (2 3) (, (incest $p $p)) (O (- (incest $p $p))))
; For now, we remove explicitly:
(exec (2 3) (,) (O (- (incest Alice Alice))))
(exec (2 4) (,) (O (- (incest Bob Bob))))

; Query incest facts after removing self-referential ones
; Expected: (incest Alice Bob) only
!(match &self (incest $p $q) (incest $p $q))

; ============================================================================
; Part 6: Add and Remove Cycle
; ============================================================================

; Add a fact
(exec (3 0) (,) (O (+ (cycle-test foo))))

; Verify it exists
!(match &self (cycle-test foo) (cycle-test foo))

; Remove it
(exec (3 1) (,) (O (- (cycle-test foo))))

; Verify it's gone
!(match &self (cycle-test foo) (cycle-test foo))

; Re-add it
(exec (3 2) (,) (O (+ (cycle-test foo))))

; Verify it exists again
!(match &self (cycle-test foo) (cycle-test foo))

; ============================================================================
; Part 7: Removing Complex Nested Structures
; ============================================================================

; Add complex nested fact
(exec (4 0) (,) (O (+ (rule (pattern (A (B $x))) (body (C $x))))))

; Query it
!(match &self (rule (pattern (A (B $x))) (body (C $x))) (rule (pattern (A (B $x))) (body (C $x))))

; Remove it
(exec (4 1) (,) (O (- (rule (pattern (A (B $x))) (body (C $x))))))

; Verify it's gone
!(match &self (rule (pattern (A (B $x))) (body (C $x))) (rule (pattern (A (B $x))) (body (C $x))))

; ============================================================================
; Summary
; ============================================================================
; The (O (- fact)) operation successfully removes facts from MORK space.
; This is essential for:
; - Cleaning up temporary data
; - Removing duplicate/invalid facts
; - Implementing retraction in logic programming
; - Supporting the full ancestor.mm2 example (line 49)
