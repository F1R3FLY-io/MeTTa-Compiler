; Example: Working with Facts
; Demonstrates various fact patterns and queries

; ========================================
; Simple Facts
; ========================================

!(println "=== Simple Facts ===")

; Unary facts (properties)
(add-atom &self (Human Socrates))
(add-atom &self (Mortal Socrates))
(add-atom &self (Philosopher Socrates))

!(println "Facts about Socrates:")
!(match &self ($property Socrates) $property)
; Expected: [Human, Mortal, Philosopher]

; ========================================
; Entity-Attribute-Value (EAV) Pattern
; ========================================

!(println "\n=== Entity-Attribute-Value Pattern ===")

; Add EAV facts
(add-atom &self (age John 30))
(add-atom &self (age Alice 25))
(add-atom &self (name John "John Doe"))
(add-atom &self (name Alice "Alice Smith"))
(add-atom &self (city John "Boston"))
(add-atom &self (city Alice "New York"))

; Query all attributes of John
!(println "All facts about John:")
!(match &self ($attr John $value) ($attr = $value))
; Expected: [(age = 30), (name = "John Doe"), (city = "Boston")]

; Query all entities with ages
!(println "\nAll entities with ages:")
!(match &self (age $entity $years) ($entity is $years))
; Expected: [(John is 30), (Alice is 25)]

; ========================================
; Relational Facts
; ========================================

!(println "\n=== Relational Facts ===")

; Binary relations
(add-atom &self (parent Alice Bob))
(add-atom &self (parent Alice Charlie))
(add-atom &self (parent Bob Dave))
(add-atom &self (parent Bob Eve))

; Find all of Alice's children
!(println "Alice's children:")
!(match &self (parent Alice $child) $child)
; Expected: [Bob, Charlie]

; Find all parents
!(println "\nAll parents:")
!(match &self (parent $parent $child) $parent)
; Expected: [Alice, Alice, Bob, Bob]

; Find grandchildren of Alice
!(println "\nAlice's grandchildren:")
!(match &self (parent Alice $child)
    (match &self (parent $child $grandchild)
        $grandchild))
; Expected: [Dave, Eve]

; ========================================
; Structured Facts (Nested)
; ========================================

!(println "\n=== Structured Facts ===")

; Add nested facts
(add-atom &self (person
    (id 1)
    (name "Alice")
    (contact (email "alice@example.com") (phone "555-1234"))))

(add-atom &self (person
    (id 2)
    (name "Bob")
    (contact (email "bob@example.com") (phone "555-5678"))))

; Query structured data
!(println "All person names:")
!(match &self (person (id $id) (name $n) $rest) ($id $n))
; Expected: [(1 "Alice"), (2 "Bob")]

!(println "\nExtract emails:")
!(match &self (person (id $id) $name (contact (email $email) $phone))
    ($id has email $email))
; Expected: [(1 has email "alice@example.com"), (2 has email "bob@example.com")]

; ========================================
; Updating Facts (Remove + Add Pattern)
; ========================================

!(println "\n=== Updating Facts ===")

; Initial fact
(add-atom &self (status project active))

!(println "Initial status:")
!(match &self (status project $s) $s)
; Expected: [active]

; Update: remove old, add new
(remove-atom &self (status project active))
(add-atom &self (status project completed))

!(println "Updated status:")
!(match &self (status project $s) $s)
; Expected: [completed]

; ========================================
; Conditional Queries
; ========================================

!(println "\n=== Conditional Queries ===")

; Add some ages
(add-atom &self (person-age Tom 17))
(add-atom &self (person-age Sarah 25))
(add-atom &self (person-age Mike 40))

; Find adults (age >= 18)
!(println "Adults (age >= 18):")
!(match &self (person-age $name $age)
    (if (>= $age 18)
        $name
        ()))
; Expected: [Sarah, Mike] (Tom filtered out)

; ========================================
; Bulk Operations
; ========================================

!(println "\n=== Bulk Operations ===")

; Add multiple similar facts
(add-atom &self (number 1))
(add-atom &self (number 2))
(add-atom &self (number 3))
(add-atom &self (number 4))
(add-atom &self (number 5))

; Query and transform
!(println "Numbers doubled:")
!(match &self (number $n) (* $n 2))
; Expected: [2, 4, 6, 8, 10]

; Count facts (length of results)
!(println "\nTotal numbers:")
!(match &self (number $n) $n)
; Expected: [1, 2, 3, 4, 5] (count = 5)

; ========================================
; Summary
; ========================================

!(println "\n=== Summary ===")
!(println "Total atoms in space:")
!(get-atoms &self)
