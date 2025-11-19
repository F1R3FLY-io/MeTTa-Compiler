; ============================================================================
; Knowledge Base Query Examples
; ============================================================================
; Demonstrates querying knowledge bases using pattern matching including:
; - Building knowledge bases
; - Simple and complex queries
; - Derived facts
; - Inference rules
; - Knowledge base reasoning

; Setup: Build Knowledge Base
; ============================================================================

; Basic facts about people
(add-atom &self (Human Socrates))
(add-atom &self (Human Plato))
(add-atom &self (Human Aristotle))
(add-atom &self (Human John))
(add-atom &self (Human Mary))

; Philosophical affiliations
(add-atom &self (philosopher Socrates))
(add-atom &self (philosopher Plato))
(add-atom &self (philosopher Aristotle))
(add-atom &self (Greek Socrates))
(add-atom &self (Greek Plato))
(add-atom &self (Greek Aristotle))

; Personal data
(add-atom &self (age Socrates 70))
(add-atom &self (age Plato 80))
(add-atom &self (age Aristotle 62))
(add-atom &self (age John 30))
(add-atom &self (age Mary 28))

; Relationships
(add-atom &self (teacher Socrates Plato))
(add-atom &self (teacher Plato Aristotle))
(add-atom &self (friend Socrates Plato))
(add-atom &self (friend Plato Socrates))

; Example 1: Simple Fact Retrieval
; ============================================================================
; Query basic facts

!(match &self (Human $x) $x)
; Expected: [Socrates, Plato, Aristotle, John, Mary]
; All humans in knowledge base

; Example 2: Filtered Query
; ============================================================================
; Query with conditions

!(match &self
    (, (Human $x)
       (philosopher $x))
    $x)
; Expected: [Socrates, Plato, Aristotle]
; Only humans who are philosophers

; Example 3: Multi-Attribute Query
; ============================================================================
; Retrieve related attributes

!(match &self
    (, (philosopher $p)
       (age $p $a)
       (Greek $p))
    (profile $p age $a))
; Expected: [(profile Socrates age 70),
;           (profile Plato age 80),
;           (profile Aristotle age 62)]

; Example 4: Relationship Query
; ============================================================================
; Find relationships between entities

!(match &self
    (teacher $teacher $student)
    ($teacher taught $student))
; Expected: [(Socrates taught Plato),
;           (Plato taught Aristotle)]

; Example 5: Transitive Relationship
; ============================================================================
; Derive transitive relationships

(= (studied-under $student $teacher)
    (match &self (teacher $teacher $student) True))

(= (studied-under $student $ancestor-teacher)
    (match &self
        (, (teacher $teacher $student)
           (studied-under $teacher $ancestor-teacher))
        True))

!(studied-under Aristotle Socrates)
; Expected: True
; Aristotle → Plato → Socrates (transitive)

; Example 6: Symmetric Relationship
; ============================================================================
; Find mutual relationships

!(match &self
    (, (friend $a $b)
       (friend $b $a))
    (mutual-friends $a $b))
; Expected: [(mutual-friends Socrates Plato),
;           (mutual-friends Plato Socrates)]

; Example 7: Negation Query
; ============================================================================
; Find entities NOT matching criterion

!(match &self
    (Human $x)
    (if (not (match &self (philosopher $x) True))
        (non-philosopher $x)
        ()))
; Expected: [(non-philosopher John),
;           (non-philosopher Mary)]

; Example 8: Aggregation Query
; ============================================================================
; Compute aggregate values

(= (average-age)
    (let $ages (match &self (age $_ $a) $a)
        (/ (sum $ages) (length $ages))))

; !(average-age)
; Expected: 54 (average of 70, 80, 62, 30, 28)

(= (oldest-person)
    (let $people-ages (match &self (age $p $a) (pair $p $a))
        (find-max $people-ages)))

; Helper to find max
(= (find-max ($h $t...))
    (if (empty? $t)
        $h
        (let $max-rest (find-max $t)
            (if (> (second $h) (second $max-rest))
                $h
                $max-rest))))

; !(oldest-person)
; Expected: (pair Plato 80)

; Example 9: Range Query
; ============================================================================
; Query within range

!(match &self
    (age $person $years)
    (if (and (>= $years 60) (<= $years 75))
        (senior $person $years)
        ()))
; Expected: [(senior Socrates 70), (senior Aristotle 62)]

; Example 10: Join Query
; ============================================================================
; SQL-like join between facts

(add-atom &self (city Socrates Athens))
(add-atom &self (city Plato Athens))
(add-atom &self (city John Boston))

!(match &self
    (, (philosopher $p)
       (city $p $c))
    (philosopher-in $p lives-in $c))
; Expected: [(philosopher-in Socrates lives-in Athens),
;           (philosopher-in Plato lives-in Athens)]

; Example 11: Derived Facts
; ============================================================================
; Create new facts from existing ones

; Rule: All Greek philosophers are ancient philosophers
(= (ancient-philosopher $x)
    (match &self
        (, (philosopher $x)
           (Greek $x))
        True))

!(ancient-philosopher Socrates)
; Expected: True

!(ancient-philosopher John)
; Expected: [] (not a Greek philosopher)

; Example 12: Inference Chain
; ============================================================================
; Multi-step inference

; If X is human and philosopher, X is intellectual
(= (intellectual $x)
    (match &self
        (, (Human $x)
           (philosopher $x))
        True))

; If X is intellectual and Greek, X is classical-scholar
(= (classical-scholar $x)
    (if (intellectual $x)
        (match &self (Greek $x) True)
        False))

!(classical-scholar Aristotle)
; Expected: True (Human + philosopher + Greek)

; Example 13: Count Query
; ============================================================================
; Count entities matching criterion

(= (count-philosophers)
    (length (match &self (philosopher $x) $x)))

; !(count-philosophers)
; Expected: 3

(= (count-young-people)
    (length
        (match &self
            (age $p $a)
            (if (< $a 35) $p ()))))

; !(count-young-people)
; Expected: 2 (John and Mary)

; Example 14: Existential Query
; ============================================================================
; Check if any entity satisfies condition

(= (has-ancient-philosopher)
    (if (> (length (match &self
                     (, (philosopher $x)
                        (Greek $x))
                     $x))
           0)
        True
        False))

; !(has-ancient-philosopher)
; Expected: True

; Example 15: Universal Query
; ============================================================================
; Check if all entities satisfy condition

(= (all-philosophers-are-human)
    (let $philosophers (match &self (philosopher $x) $x)
        (let $non-humans
            (filter (λ $p (not (match &self (Human $p) True)))
                   $philosophers)
            (empty? $non-humans))))

; !(all-philosophers-are-human)
; Expected: True (all philosophers are humans)

; Example 16: Graph Query - Connected Components
; ============================================================================
; Find connected groups

(add-atom &self (knows Alice Bob))
(add-atom &self (knows Bob Charlie))
(add-atom &self (knows David Eve))

(= (connected-to $a $b)
    (or (match &self (knows $a $b) True)
        (match &self (knows $b $a) True)))

(= (indirectly-connected $a $b)
    (if (connected-to $a $b)
        True
        (match &self
            (knows $a $mid)
            (indirectly-connected $mid $b))))

; !(indirectly-connected Alice Charlie)
; Expected: True (Alice → Bob → Charlie)

; Example 17: Knowledge Base Completion
; ============================================================================
; Generate missing facts

; If A is teacher of B, and B is teacher of C,
; then A is academic-ancestor of C
!(match &self
    (, (teacher $a $b)
       (teacher $b $c))
    (add-atom &self (academic-ancestor $a $c)))

; Now query academic ancestors
!(match &self (academic-ancestor $a $d) ($a ancestor-of $d))
; Expected: [(Socrates ancestor-of Aristotle)]

; Example 18: Consistency Check
; ============================================================================
; Detect inconsistencies

(= (check-age-consistency)
    (match &self
        (, (age $p $a1)
           (age $p $a2)
           (!= $a1 $a2))
        (error "inconsistent ages for " $p)))

; !(check-age-consistency)
; Expected: [] (no inconsistencies in our KB)

; Example 19: Recursive Knowledge Query
; ============================================================================
; Query with recursion

(add-atom &self (part-of Athens Greece))
(add-atom &self (part-of Greece Europe))
(add-atom &self (part-of Europe Earth))

(= (located-in $entity $location)
    (match &self (city $entity $location) True))

(= (located-in $entity $container)
    (match &self
        (, (city $entity $loc)
           (part-of $loc $container))
        True))

(= (located-in $entity $container)
    (match &self
        (, (city $entity $loc)
           (part-of $loc $mid)
           (located-in $mid $container))
        True))

; !(located-in Socrates Earth)
; Expected: True (Socrates in Athens, Athens in Greece, Greece in Europe, Europe on Earth)

; Example 20: Complex Inference Example
; ============================================================================
; Sophisticated reasoning

; Define mortality rule
(= (mortal $x)
    (match &self (Human $x) True))

; Define wisdom rule
(= (wise $x)
    (match &self
        (, (philosopher $x)
           (age $x $a)
           (> $a 65))
        True))

; Combine rules
(= (wise-mortal $x)
    (if (and (mortal $x) (wise $x))
        True
        False))

!(match &self (Human $x) (if (wise-mortal $x) (wise-mortal $x) ()))
; Expected: [(wise-mortal Socrates), (wise-mortal Plato)]

; ============================================================================
; Summary
; ============================================================================
; Knowledge base queries demonstrated:
; - Simple fact retrieval
; - Filtered queries with conditions
; - Multi-attribute queries
; - Relationship queries (teacher, friend, etc.)
; - Transitive relationships (studied-under)
; - Symmetric relationships (mutual friends)
; - Negation queries (non-philosophers)
; - Aggregation (average age, oldest person)
; - Range queries (age ranges)
; - Join queries (philosopher + city)
; - Derived facts (ancient-philosopher)
; - Inference chains (classical-scholar)
; - Count queries
; - Existential queries (has any...)
; - Universal queries (all...)
; - Graph queries (connected components)
; - Knowledge base completion
; - Consistency checking
; - Recursive queries (part-of hierarchies)
; - Complex inference (wise-mortal)
;
; These patterns enable:
; - Building rich knowledge representations
; - Querying structured information
; - Deriving new facts through inference
; - Reasoning about relationships
; - Validating knowledge consistency
