; Example: Mutation Order and Side Effects
; Demonstrates atom space mutations and their ordering

; Create a new space
!(bind! &myspace (new-space))

; Sequential mutations (within a single branch)
; These should execute in order: A added first, then B, then C
!(let () (add-atom &myspace A)
     (let () (add-atom &myspace B)
          (add-atom &myspace C)))

; Query the space to see all atoms
!(match &myspace $x $x)
; Expected result: {A, B, C}

; Remove an atom
!(remove-atom &myspace B)

; Query again
!(match &myspace $x $x)
; Expected result: {A, C}

; Replace an atom
!(replace-atom &myspace A Z)

; Query again
!(match &myspace $x $x)
; Expected result: {Z, C}

; Non-deterministic mutations (order-dependent behavior)
!(bind! &space2 (new-space))

; Define multiple rules that mutate the space
(= (mutate1) (add-atom &space2 X))
(= (mutate1) (add-atom &space2 Y))

; Both rules fire, but order is unspecified
; Both X and Y should be added, but we can't predict the order
!(mutate1)

!(match &space2 $x $x)
; Expected result: {X, Y} (both added, order unspecified)

; Example of non-confluent behavior with conditionals
!(bind! &space3 (new-space))

; This function adds an atom conditionally based on space state
; WARNING: This demonstrates non-confluent behavior!
; (= (conditional-add)
;    (if (empty? &space3)
;        (add-atom &space3 A)
;        (add-atom &space3 B)))

; If evaluated in different orders with multiple calls,
; could produce different results

; Example: Mutation during pattern matching
; NOTE: This will likely cause a RefCell panic!
; Pattern matching borrows the space immutably,
; while add-atom requires mutable borrow

; (= (mutate-on-match $x) (add-atom &space4 (new $x)))
; !(bind! &space4 (new-space))
; !(add-atom &space4 (foo 1))
; This would panic:
; !(match &space4 (foo $x) (mutate-on-match $x))

; Safe pattern: collect matches first, then mutate
!(bind! &space5 (new-space))
!(add-atom &space5 (item 1))
!(add-atom &space5 (item 2))
!(add-atom &space5 (item 3))

; Collect items
!(let $items (collapse (match &space5 (item $x) $x))
     ; Now mutate based on collected items
     (add-atom &space5 (processed $items)))

!(match &space5 $x $x)
; Expected: {(item 1), (item 2), (item 3), (processed (1 2 3))}
