; Example: Knowledge Base with Atom Space
; Demonstrates building and querying a knowledge base

; ========================================
; Building a Knowledge Base
; ========================================

!(println "=== Building Knowledge Base ===")

; Facts about entities
(add-atom &self (Human Socrates))
(add-atom &self (Human Plato))
(add-atom &self (Human Aristotle))
(add-atom &self (Human Pythagoras))

; Properties
(add-atom &self (philosopher Socrates))
(add-atom &self (philosopher Plato))
(add-atom &self (philosopher Aristotle))
(add-atom &self (mathematician Pythagoras))
(add-atom &self (mathematician Plato))

; Relationships
(add-atom &self (teacher Socrates Plato))
(add-atom &self (teacher Plato Aristotle))
(add-atom &self (student Plato Socrates))
(add-atom &self (student Aristotle Plato))

; Attributes
(add-atom &self (born Socrates -470))
(add-atom &self (born Plato -428))
(add-atom &self (born Aristotle -384))
(add-atom &self (died Socrates -399))

; Contributions
(add-atom &self (known-for Socrates "Socratic Method"))
(add-atom &self (known-for Plato "Theory of Forms"))
(add-atom &self (known-for Aristotle "Logic"))
(add-atom &self (known-for Pythagoras "Pythagorean Theorem"))

!(println "Knowledge base populated with facts about ancient philosophers")

; ========================================
; Inference Rules
; ========================================

!(println "\n=== Adding Inference Rules ===")

; Mortality rule
(add-atom &self (= (Mortal $x) (Human $x)))

; Wisdom rule
(add-atom &self (= (wise $x) (philosopher $x)))

; Transitivity of teaching
(add-atom &self (= (influenced $x $z)
    (match &self (teacher $x $y)
        (match &self (teacher $y $z)
            True))))

!(println "Inference rules added")

; ========================================
; Simple Queries
; ========================================

!(println "\n=== Simple Queries ===")

!(println "Who are the humans?")
!(match &self (Human $x) $x)
; Expected: [Socrates, Plato, Aristotle, Pythagoras]

!(println "\nWho are the philosophers?")
!(match &self (philosopher $x) $x)
; Expected: [Socrates, Plato, Aristotle]

!(println "\nWho are the mathematicians?")
!(match &self (mathematician $x) $x)
; Expected: [Pythagoras, Plato]

; ========================================
; Relational Queries
; ========================================

!(println "\n=== Relational Queries ===")

!(println "Who taught whom?")
!(match &self (teacher $t $s) ($t taught $s))
; Expected: [(Socrates taught Plato), (Plato taught Aristotle)]

!(println "\nWho were Plato's teachers?")
!(match &self (teacher $t Plato) $t)
; Expected: [Socrates]

!(println "\nWho were Plato's students?")
!(match &self (teacher Plato $s) $s)
; Expected: [Aristotle]

; ========================================
; Inference Queries
; ========================================

!(println "\n=== Inference Queries ===")

!(println "Who is mortal? (inferred from being human)")
!(match &self (Human $x) (Mortal $x))
; Expected: [(Mortal Socrates), (Mortal Plato), (Mortal Aristotle), (Mortal Pythagoras)]

!(println "\nWho is wise? (inferred from being philosopher)")
!(match &self (philosopher $x) (wise $x))
; Expected: [(wise Socrates), (wise Plato), (wise Aristotle)]

; ========================================
; Multi-Hop Queries
; ========================================

!(println "\n=== Multi-Hop Queries ===")

!(println "Who did Socrates teach (directly)?")
!(match &self (teacher Socrates $student) $student)
; Expected: [Plato]

!(println "\nWho did Socrates influence (indirectly - students of students)?")
!(match &self (teacher Socrates $direct)
    (match &self (teacher $direct $indirect)
        $indirect))
; Expected: [Aristotle]

!(println "\nTeaching chain: Socrates -> ? -> ?")
!(match &self (teacher Socrates $x)
    (match &self (teacher $x $y)
        (chain Socrates $x $y)))
; Expected: [(chain Socrates Plato Aristotle)]

; ========================================
; Aggregate Queries
; ========================================

!(println "\n=== Aggregate Queries ===")

!(println "What are all the contributions?")
!(match &self (known-for $person $contribution)
    ($person contributed $contribution))
; Expected: [(Socrates contributed "Socratic Method"), ...]

!(println "\nWho lived in 5th century BCE?")
!(match &self (born $person $year)
    (if (and (>= $year -500) (< $year -400))
        ($person lived in 5th century BCE)
        ()))
; Expected: [(Socrates lived in 5th century BCE), (Plato lived in 5th century BCE)]

; ========================================
; Existential Queries
; ========================================

!(println "\n=== Existential Queries ===")

!(println "Did Plato have a teacher?")
!(if (match &self (teacher $t Plato) True)
    (println "Yes, Plato had a teacher")
    (println "No records of Plato's teacher"))
; Expected: "Yes, Plato had a teacher"

!(println "\nDid Pythagoras teach anyone?")
!(if (match &self (teacher Pythagoras $s) True)
    (println "Yes, Pythagoras taught someone")
    (println "No records of Pythagoras teaching"))
; Expected: "No records of Pythagoras teaching"

; ========================================
; Conditional Reasoning
; ========================================

!(println "\n=== Conditional Reasoning ===")

; Add conditional rule
(add-atom &self (= (ancient $person)
    (match &self (born $person $year)
        (< $year -300))))

!(println "Who is ancient (born before 300 BCE)?")
!(match &self (Human $x) (ancient $x))
; Would evaluate ancient rule for each human

; ========================================
; Knowledge Base Statistics
; ========================================

!(println "\n=== Knowledge Base Statistics ===")

!(println "Total number of atoms in KB:")
!(get-atoms &self)
; Lists all atoms

!(println "\nCount of humans:")
!(match &self (Human $x) $x)
; Count results to get number

!(println "\nCount of teacher relationships:")
!(match &self (teacher $t $s) found)
; Count occurrences

; ========================================
; Updating Knowledge Base
; ========================================

!(println "\n=== Updating Knowledge Base ===")

; Add new fact
!(println "Adding new philosopher:")
(add-atom &self (Human Epicurus))
(add-atom &self (philosopher Epicurus))
(add-atom &self (born Epicurus -341))

!(println "Updated list of philosophers:")
!(match &self (philosopher $x) $x)
; Expected: [Socrates, Plato, Aristotle, Epicurus]

; Correct a fact
!(println "\nCorrecting a fact:")
(remove-atom &self (born Aristotle -384))
(add-atom &self (born Aristotle -384))  ; Re-add (would be different in real correction)

; ========================================
; Complex Queries
; ========================================

!(println "\n=== Complex Queries ===")

; Find philosopher-mathematicians
!(println "Who is both philosopher and mathematician?")
!(match &self (philosopher $x)
    (if (match &self (mathematician $x) True)
        $x
        ()))
; Expected: [Plato]

; Find people with known birth year who were philosophers
!(println "\nPhilosophers with known birth years:")
!(match &self (philosopher $x)
    (match &self (born $x $year)
        ($x born $year)))
; Expected: [(Socrates born -470), (Plato born -428), (Aristotle born -384)]

; ========================================
; Semantic Queries
; ========================================

!(println "\n=== Semantic Queries ===")

; Who influenced modern logic?
!(println "Find person known for Logic:")
!(match &self (known-for $person "Logic") $person)
; Expected: [Aristotle]

; Who developed methods?
!(println "\nPeople known for methods:")
!(match &self (known-for $person $contribution)
    (if (contains? $contribution "Method")
        ($person developed $contribution)
        ()))
; Expected: [(Socrates developed "Socratic Method")]

; ========================================
; Temporal Queries
; ========================================

!(println "\n=== Temporal Queries ===")

; Who was born earliest?
!(println "Birth years:")
!(match &self (born $person $year) ($person $year))

; Find contemporary philosophers (overlapping lifespans)
!(println "\nWho could have met? (overlapping lifespans)")
; This would require birth/death date comparisons

; ========================================
; Data Export
; ========================================

!(println "\n=== Exporting Knowledge ===")

; Export all facts about Plato
!(println "All facts about Plato:")
!(match &self ($relation Plato $value) (fact $relation $value))
!(match &self ($relation $value Plato) (fact $relation $value))
; Expected: All facts mentioning Plato

; Export all relationships
!(println "\nAll relationships:")
!(match &self (teacher $x $y) (teacher $x $y))
!(match &self (student $x $y) (student $x $y))

; ========================================
; Knowledge Base Validation
; ========================================

!(println "\n=== Validation ===")

; Check consistency: every student should have corresponding teacher
!(println "Validating student-teacher consistency:")
!(match &self (student $s $t)
    (if (match &self (teacher $t $s) True)
        (valid relationship $s $t)
        (invalid no teacher record for $s learning from $t)))
; Expected: (valid ...) for each

; ========================================
; Summary
; ========================================

!(println "\n=== Knowledge Base Summary ===")
!(println "Demonstrated:")
!(println "- Building structured knowledge base")
!(println "- Adding facts and inference rules")
!(println "- Simple and complex queries")
!(println "- Multi-hop reasoning")
!(println "- Existential queries")
!(println "- Conditional reasoning")
!(println "- Knowledge base updates")
!(println "- Temporal and semantic queries")
!(println "- Data export and validation")

!(println "\nFinal KB state:")
!(get-atoms &self)
