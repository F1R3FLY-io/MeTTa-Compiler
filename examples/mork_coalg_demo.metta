; MORK coalg Special Form Demonstration
; Syntax: (coalg <pattern> <templates>)
;
; coalg performs tree transformations with explicit result cardinality.
; Templates are always conjunctions indicating number of results.

; ============================================================================
; 1. Single Template - One Result
; ============================================================================
; Lift a tree into a context
!(coalg (tree $t) (, (ctx $t nil)))

; Input: (tree (branch (leaf 1) (leaf 2)))
; Output: (ctx (branch (leaf 1) (leaf 2)) nil)

; ============================================================================
; 2. Binary Template - Two Results (Unfold)
; ============================================================================
; Explode a branch into two contexts
!(coalg (ctx (branch $left $right) $path)
        (, (ctx $left (cons $path L))
           (ctx $right (cons $path R))))

; Input: (ctx (branch (leaf 1) (leaf 2)) nil)
; Output:
;   (ctx (leaf 1) (cons nil L))
;   (ctx (leaf 2) (cons nil R))

; ============================================================================
; 3. Empty Template - Termination
; ============================================================================
; Empty conjunction means "no results" - stops unfolding
!(coalg (terminal-state) (,))

; Input: (terminal-state)
; Output: (nothing - termination)

; ============================================================================
; 4. Complete Tree-to-Space Transformation
; ============================================================================

; Step 1: Lift - Wrap tree in initial context
!(coalg (tree $t) (, (ctx $t nil)))

; Step 2: Explode - Unfold branches
!(coalg (ctx (branch $l $r) $p)
        (, (ctx $l (cons $p L))
           (ctx $r (cons $p R))))

; Step 3: Drop - Extract leaf values
!(coalg (ctx (leaf $v) $p) (, (value $p $v)))

; Example execution trace:
; (tree (branch (leaf 11) (leaf 12)))
;   → (ctx (branch (leaf 11) (leaf 12)) nil)
;   → (ctx (leaf 11) (cons nil L)), (ctx (leaf 12) (cons nil R))
;   → (value (cons nil L) 11), (value (cons nil R) 12)

; ============================================================================
; 5. N-ary Template - Multiple Results
; ============================================================================
; Produce three results from one input
!(coalg (triple $x)
        (, (first $x) (second $x) (third $x)))

; Input: (triple data)
; Output: (first data), (second data), (third data)

; ============================================================================
; 6. Nested Structure Unfolding
; ============================================================================
; Unfold nested structures
!(coalg (nested (outer $o) (inner $i))
        (, (outer-data $o) (inner-data $i)))

; ============================================================================
; 7. List Unfolding Pattern
; ============================================================================
; Unfold a list into head and tail
!(coalg (list (cons $head $tail))
        (, (head $head) (tail $tail)))

; Input: (list (cons 1 (cons 2 (cons 3 nil))))
; Output: (head 1), (tail (cons 2 (cons 3 nil)))
