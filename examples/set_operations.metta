;; ============================================================================
;; Set Operations Example - Multiset Semantics
;; ============================================================================
;;
;; This file demonstrates MeTTa's set operations with **multiset semantics**,
;; where element multiplicities (counts) are preserved and respected.
;;
;; All operations use **structural equality** for comparison and preserve
;; the **order from the left input** (where applicable).
;;
;; Reference: Aligned with MeTTa hyperon-experimental stdlib

;; ============================================================================
;; 1. UNIQUE-ATOM - Remove Duplicates
;; ============================================================================
;;
;; Syntax:   (unique-atom $list)
;; Returns:  List with duplicates removed, keeping first occurrence
;; Order:    Preserved (order of first occurrences)
;;
;; Description:
;; Takes a tuple and returns only unique entities. Duplicates are removed
;; while preserving the order of first occurrences.

;; Basic deduplication
!(unique-atom (a b c d d))
;; Expected: (a b c d)
;; Explanation: Second 'd' is removed

!(unique-atom (a b a c b))
;; Expected: (a b c)
;; Explanation: Duplicates 'a' and 'b' removed, first occurrences kept

;; Empty list
!(unique-atom ())
;; Expected: ()

;; Already unique
!(unique-atom (a b c))
;; Expected: (a b c)

;; All duplicates
!(unique-atom (x x x x))
;; Expected: (x)
;; Explanation: Only first 'x' is kept

;; Mixed types
!(unique-atom (1 "hello" 1 True False "hello"))
;; Expected: (1 "hello" True False)
;; Explanation: Works with different value types

;; ============================================================================
;; 2. UNION-ATOM - Multiset Union (Concatenation)
;; ============================================================================
;;
;; Syntax:   (union-atom $list1 $list2)
;; Returns:  Concatenation of both lists (all elements preserved)
;; Order:    Left elements first, then right elements
;;
;; Description:
;; Takes two tuples and returns their union. This is a **multiset union**
;; where ALL elements from both lists are included (no deduplication).
;; Multiplicities are summed: if 'a' appears 2x in left and 3x in right,
;; it appears 5x in the result.

;; Basic union with overlap
!(union-atom (a b b c) (b c c d))
;; Expected: (a b b c b c c d)
;; Explanation: All elements included: left has 2 b's + right has 1 b = 3 b's total

!(union-atom (a b) (c d))
;; Expected: (a b c d)
;; Explanation: Disjoint sets, simple concatenation

;; Empty lists
!(union-atom () (a b))
;; Expected: (a b)

!(union-atom (a b) ())
;; Expected: (a b)

!(union-atom () ())
;; Expected: ()

;; Identical lists
!(union-atom (x y) (x y))
;; Expected: (x y x y)
;; Explanation: Multiset semantics - all elements preserved

;; Mixed types
!(union-atom (1 "a") (2 "b"))
;; Expected: (1 "a" 2 "b")

;; ============================================================================
;; 3. INTERSECTION-ATOM - Multiset Intersection
;; ============================================================================
;;
;; Syntax:   (intersection-atom $list1 $list2)
;; Returns:  Elements common to both lists (min of counts)
;; Order:    Preserved from LEFT input
;;
;; Description:
;; Takes two tuples and returns their intersection. For each element,
;; the result contains **min(left count, right count)** occurrences.
;; This respects multiset semantics where multiplicities matter.

;; Basic intersection with multiplicities
!(intersection-atom (a b c c) (b c c c d))
;; Expected: (b c c)
;; Explanation:
;;   'a': left=1, right=0 → min(1,0)=0 (not in result)
;;   'b': left=1, right=1 → min(1,1)=1 (one 'b' in result)
;;   'c': left=2, right=3 → min(2,3)=2 (two 'c's in result)
;;   'd': left=0, right=1 → min(0,1)=0 (not in result)

;; Order preservation from left
!(intersection-atom (z y x) (x z))
;; Expected: (z x)
;; Explanation: Order from LEFT input (z before x), not from right

!(intersection-atom (x z) (z y x))
;; Expected: (x z)
;; Explanation: Order from LEFT input (x before z)

;; Disjoint sets (no common elements)
!(intersection-atom (a b) (c d))
;; Expected: ()
;; Explanation: No common elements

;; Empty lists
!(intersection-atom () (a b))
;; Expected: ()

!(intersection-atom (a b) ())
;; Expected: ()

;; Identical lists
!(intersection-atom (a b c) (a b c))
;; Expected: (a b c)
;; Explanation: min(n, n) = n for all elements

;; Heavy multiplicity skew
!(intersection-atom (x x x x x) (x))
;; Expected: (x)
;; Explanation: min(5, 1) = 1

!(intersection-atom (x) (x x x x x))
;; Expected: (x)
;; Explanation: min(1, 5) = 1

;; Mixed types
!(intersection-atom (1 "hello" True) (1 "hello" False))
;; Expected: (1 "hello")
;; Explanation: Only common elements

;; Nested S-expressions
!(intersection-atom ((a b) (c d)) ((a b) (e f)))
;; Expected: ((a b))
;; Explanation: Nested structures compared structurally

;; ============================================================================
;; 4. SUBTRACTION-ATOM - Multiset Difference
;; ============================================================================
;;
;; Syntax:   (subtraction-atom $list1 $list2)
;; Returns:  Left list with right elements removed (max(0, left - right))
;; Order:    Preserved from LEFT input
;;
;; Description:
;; Takes two tuples and returns their subtraction (left minus right).
;; For each element, the result contains **max(0, left count - right count)**
;; occurrences. This is **saturating subtraction** - counts never go negative.

;; Basic subtraction with multiplicities
!(subtraction-atom (a b b c) (b c c d))
;; Expected: (a b)
;; Explanation:
;;   'a': left=1, right=0 → 1-0=1 (one 'a' kept)
;;   'b': left=2, right=1 → 2-1=1 (one 'b' kept)
;;   'c': left=1, right=2 → max(0,1-2)=0 (removed, saturating)
;;   'd': left=0, right=1 → not in left (irrelevant)

;; Order preservation from left
!(subtraction-atom (z y x w v u) (w x))
;; Expected: (z y v u)
;; Explanation: 'w' and 'x' removed, order of remaining preserved

;; Subtract everything
!(subtraction-atom (a b) (a b))
;; Expected: ()
;; Explanation: All elements subtracted

;; Disjoint sets (nothing to subtract)
!(subtraction-atom (a b) (c d))
;; Expected: (a b)
;; Explanation: No common elements to subtract

;; Empty lists
!(subtraction-atom () (a b))
;; Expected: ()

!(subtraction-atom (a b) ())
;; Expected: (a b)
;; Explanation: Subtracting nothing leaves everything

;; Subtract subset
!(subtraction-atom (a b c d) (b d))
;; Expected: (a c)
;; Explanation: 'b' and 'd' removed, 'a' and 'c' remain

;; Heavy multiplicity
!(subtraction-atom (x x x x x) (x x))
;; Expected: (x x x)
;; Explanation: 5 - 2 = 3 remaining

;; Over-subtraction (saturating)
!(subtraction-atom (x) (x x x))
;; Expected: ()
;; Explanation: max(0, 1-3) = 0

;; Mixed types
!(subtraction-atom (1 "hello" True) (1 "hello"))
;; Expected: (True)
;; Explanation: Numeric and string removed, boolean remains

;; Nested S-expressions
!(subtraction-atom ((a b) (c d)) ((a b)))
;; Expected: ((c d))
;; Explanation: First nested structure removed

;; ============================================================================
;; COMPARISON: Multiset vs Set Semantics
;; ============================================================================
;;
;; Traditional SET semantics (NOT used here):
;;   - Sets treat {a, a, b} as {a, b} (no duplicates)
;;   - Union: {a, b} ∪ {b, c} = {a, b, c}
;;   - Intersection: {a, b, c} ∩ {b, c, d} = {b, c}
;;
;; MULTISET semantics (used by MeTTa):
;;   - Multisets preserve counts: [a, a, b] keeps both a's
;;   - Union: [a, b] ⊎ [b, c] = [a, b, b, c] (concatenation)
;;   - Intersection: [a, b, c, c] ⊓ [b, c, c, c] = [b, c, c] (min counts)
;;
;; Example demonstrating the difference:
;;
;; Data: list1 = (a b b)  list2 = (b c)
;;
;; Set-based intersection would give:      {b}
;; Multiset-based intersection gives:      (b)     (same here)
;;
;; Data: list1 = (a b b b)  list2 = (b b c)
;;
;; Set-based intersection would give:      {b}     (only one b)
;; Multiset-based intersection gives:      (b b)   (min(3,2)=2 b's)

!(intersection-atom (a b b b) (b b c))
;; Expected: (b b)
;; Explanation: Multiset semantics - min(3 b's, 2 b's) = 2 b's

;; ============================================================================
;; PRACTICAL USE CASES
;; ============================================================================

;; Use Case 1: Remove duplicates from query results
;; !(unique-atom $query-results)

;; Use Case 2: Combine results from multiple sources
;; !(union-atom $source1-results $source2-results)

;; Use Case 3: Find common elements between two knowledge bases
;; !(intersection-atom $kb1-facts $kb2-facts)

;; Use Case 4: Find facts in KB1 but not in KB2
;; !(subtraction-atom $kb1-facts $kb2-facts)

;; Use Case 5: Detect differences between old and new state
;; !(subtraction-atom $new-state $old-state)  ; Added items
;; !(subtraction-atom $old-state $new-state)  ; Removed items

;; ============================================================================
;; COMPLEXITY NOTES
;; ============================================================================
;;
;; All operations use HashMap-based counting for optimal performance:
;;
;; unique-atom:         O(n) time, O(n) space
;; union-atom:          O(n + m) time, O(n + m) space
;; intersection-atom:   O(n + m) time, O(m) space  (m = right list size)
;; subtraction-atom:    O(n + m) time, O(m) space  (m = right list size)
;;
;; where n = left list size, m = right list size

;; ============================================================================
;; END OF EXAMPLES
;; ============================================================================

