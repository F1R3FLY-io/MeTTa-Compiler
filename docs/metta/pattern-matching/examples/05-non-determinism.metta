; ============================================================================
; Non-Determinism Examples
; ============================================================================
; Demonstrates non-deterministic evaluation in pattern matching including:
; - Multiple matching rules
; - Multiple space matches
; - Result ordering
; - Controlling non-determinism
; - Combinatorial expansion

; Setup: Create knowledge base
; ============================================================================

(add-atom &self (Human Socrates))
(add-atom &self (Human Plato))
(add-atom &self (Human Aristotle))

; Example 1: Multiple Rule Matches
; ============================================================================
; Multiple rules can match same expression

(= (color) red)
(= (color) green)
(= (color) blue)

!(color)
; Non-deterministic result: may return any or all of:
; [red], [green], [blue], [red, green], [red, green, blue], etc.
; Order is implementation-dependent

; Example 2: Multiple Space Matches
; ============================================================================
; Pattern matches multiple atoms

!(match &self (Human $x) $x)
; Returns all matches (non-deterministic order)
; Expected: [Socrates, Plato, Aristotle] (in any order)

; Example 3: Non-Deterministic Choice
; ============================================================================
; Different rules for same pattern

(= (choose-number) 1)
(= (choose-number) 2)
(= (choose-number) 3)

!(choose-number)
; May return: 1, 2, 3, or [1, 2, 3]
; Depends on evaluation strategy

; Example 4: Cartesian Product Non-Determinism
; ============================================================================
; Conjunction creates combinations

(add-atom &self (first A))
(add-atom &self (first B))
(add-atom &self (second X))
(add-atom &self (second Y))

!(match &self
    (, (first $a)
       (second $b))
    (pair $a $b))
; Result: All combinations
; Expected: [(pair A X), (pair A Y), (pair B X), (pair B Y)]
; Order non-deterministic

; Example 5: Multiple Paths Non-Determinism
; ============================================================================
; Multiple ways to derive same result

(add-atom &self (edge A B))
(add-atom &self (edge B C))
(add-atom &self (edge A C))  ; Direct path

(= (connected $x $y)
    (match &self (edge $x $y) True))

(= (connected $x $z)
    (match &self
        (, (edge $x $y)
           (connected $y $z))
        True))

!(connected A C)
; Two derivations:
; 1. Direct: (edge A C) → True
; 2. Transitive: (edge A B), (connected B C) → True
; May return: [True, True] or deduplicated [True]

; Example 6: Duplicate Results
; ============================================================================
; Same result from different matches

(add-atom &self (type A number))
(add-atom &self (value A 42))
(add-atom &self (type B number))
(add-atom &self (value B 42))

!(match &self (value $_ 42) found)
; Expected: [found, found]
; One per match, even though result is same

; Example 7: Controlling Non-Determinism - First Result
; ============================================================================
; Extract only first result

(= (first-human)
    (let $humans (match &self (Human $x) $x)
        (if (> (length $humans) 0)
            (car $humans)
            none)))

!(first-human)
; Returns single result (whichever comes first)
; Could be: Socrates, Plato, or Aristotle

; Example 8: Filtering Non-Deterministic Results
; ============================================================================
; Filter results with condition

(add-atom &self (age Socrates 70))
(add-atom &self (age Plato 80))
(add-atom &self (age John 25))

!(match &self
    (age $person $years)
    (if (> $years 50)
        $person
        ()))  ; Empty = filter out
; Expected: [Socrates, Plato]
; John filtered out (order non-deterministic)

; Example 9: Aggregating Non-Deterministic Results
; ============================================================================
; Collect and process all results

(= (count-humans)
    (length (match &self (Human $x) $x)))

!(count-humans)
; Expected: 3
; Deterministic count despite non-deterministic match order

(= (all-ages)
    (match &self (age $_ $years) $years))

!(all-ages)
; Expected: [70, 80, 25] (order non-deterministic)

; Example 10: Recursive Non-Determinism
; ============================================================================
; Recursion generates multiple results

(= (countdown 0) (0))
(= (countdown $n)
    (if (> $n 0)
        (cons $n (countdown (- $n 1)))
        ()))

!(countdown 3)
; Expected: [[3, 2, 1, 0]]
; Single result, but recursively generated

; Generate all subsequences (truly non-deterministic)
(= (subseqs ()) [()])
(= (subseqs ($h $t...))
    (let $rest (subseqs $t)
        (union $rest
               (map (λ $s (cons $h $s)) $rest))))

; !(subseqs (A B C))
; Expected: [[], [A], [B], [C], [A B], [A C], [B C], [A B C]]
; Order non-deterministic

; Example 11: Non-Deterministic Branching
; ============================================================================
; Rules with overlapping patterns

(= (classify 0) zero)
(= (classify $x) (if (> $x 0) positive negative))

!(classify 0)
; May match first rule only: [zero]
; Or both rules (though second has condition): depends on implementation

!(classify 5)
; Expected: [positive]

; Example 12: Limiting Non-Determinism
; ============================================================================
; Explicitly limit result count

(= (take-n 0 $_) ())
(= (take-n $n ($h $t...))
    (if (> $n 0)
        (cons $h (take-n (- $n 1) $t))
        ()))

(= (first-n-humans $n)
    (take-n $n (match &self (Human $x) $x)))

!(first-n-humans 2)
; Expected: First 2 humans (which 2 is non-deterministic)
; Could be: [Socrates, Plato] or [Plato, Aristotle], etc.

; Example 13: Non-Determinism in Nested Queries
; ============================================================================
; Inner match produces multiple results per outer match

(add-atom &self (teaches Socrates Philosophy))
(add-atom &self (teaches Socrates Logic))
(add-atom &self (teaches Plato Philosophy))

!(match &self (Human $person)
    ($person teaches
        (match &self (teaches $person $subject) $subject)))
; Expected: [(Socrates teaches [Philosophy, Logic]),
;           (Plato teaches [Philosophy]),
;           (Aristotle teaches [])]

; Example 14: Explosive Non-Determinism
; ============================================================================
; Exponential blowup

(add-atom &self (option 1 A))
(add-atom &self (option 1 B))
(add-atom &self (option 2 X))
(add-atom &self (option 2 Y))
(add-atom &self (option 3 P))
(add-atom &self (option 3 Q))

!(match &self
    (, (option 1 $a)
       (option 2 $b)
       (option 3 $c))
    (choice $a $b $c))
; Result count: 2 × 2 × 2 = 8
; Expected: [(choice A X P), (choice A X Q), (choice A Y P), (choice A Y Q),
;           (choice B X P), (choice B X Q), (choice B Y P), (choice B Y Q)]

; Example 15: Deterministic vs Non-Deterministic
; ============================================================================
; Comparison

; Deterministic: single, predictable result
(= (double $x) (* $x 2))
!(double 5)
; Expected: [10]
; Always returns same result

; Non-Deterministic: multiple or unpredictable results
(= (random-choice) A)
(= (random-choice) B)
!(random-choice)
; Expected: A, B, or [A, B]
; Result not predictable

; Example 16: Handling Empty Non-Deterministic Results
; ============================================================================
; No matches produce empty list

!(match &self (Dog $x) $x)
; Expected: []
; No atoms match, non-deterministic empty result

(= (safe-match)
    (let $result (match &self (NonExistent $x) $x)
        (if (empty? $result)
            [default-value]
            $result)))

!(safe-match)
; Expected: [default-value]

; Example 17: Order Independence
; ============================================================================
; Don't rely on result order

; Bad practice - order dependent
; (= (first-older-than-60)
;     (car (match &self
;         (, (age $p $y)
;            (> $y 60))
;         $p)))

; Good practice - order independent
(= (any-older-than-60)
    (let $results (match &self
                    (, (age $p $y)
                       (> $y 60))
                    $p)
        (if (> (length $results) 0)
            (some-are-older $results)
            (none-older))))

; ============================================================================
; Summary
; ============================================================================
; Non-determinism demonstrated:
; - Multiple rules matching same expression
; - Multiple atoms matching same pattern
; - Result order is non-deterministic
; - Cartesian product in conjunctions
; - Multiple derivation paths
; - Duplicate results from different matches
; - Controlling with filters, limits, aggregations
; - Recursive non-determinism
; - Explosive combinatorial expansion
; - Empty results are valid non-deterministic outcomes
;
; Best practices:
; - Don't rely on result order
; - Handle empty results
; - Filter and aggregate when needed
; - Be aware of combinatorial explosion
; - Use specific patterns to reduce matches
