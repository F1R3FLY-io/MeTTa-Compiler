# Facts in MeTTa Atom Space

## Overview

In MeTTa, **facts** are atoms stored in an atom space that represent data, assertions, or ground truth. Facts form the knowledge base that MeTTa programs query and reason over. This document provides comprehensive details about facts: what they are, how they're stored, and how to work with them.

## What is a Fact?

### Specification

**Definition**: A fact is any atom in an atom space that represents static data or an assertion about the world.

**Formal Definition:**
```
Fact := Atom ∈ Space
```

**Key Characteristic**: There is no syntactic or structural distinction between facts and other atoms at the storage level. The term "fact" is conceptual, not technical.

### Examples

**Simple Facts:**
```metta
Socrates                    ; Fact: the symbol Socrates
42                          ; Fact: the number 42
"Hello, World!"             ; Fact: the string
```

**Relational Facts:**
```metta
(Human Socrates)            ; Fact: Socrates is human
(age John 30)               ; Fact: John's age is 30
(parent Alice Bob)          ; Fact: Alice is parent of Bob
(knows Alice Bob)           ; Fact: Alice knows Bob
```

**Complex Facts:**
```metta
(person (name "Alice") (age 30) (city "Boston"))
(edge (node 1) (node 2) (weight 5.5))
((position 10 20) (velocity 1.5 0.0))
```

**Typed Facts (with type annotations):**
```metta
(: socrates Person)         ; Fact: socrates has type Person
(: (age John) (: 30 Nat))   ; Fact: John's age is a natural number
```

## Facts vs Rules

### Conceptual Distinction

**Facts:**
- Represent static data
- No computational behavior
- Direct assertions
- Examples: `(Human Socrates)`, `42`, `"data"`

**Rules:**
- Define computations or rewrites
- Use `=` operator
- Define transformations
- Examples: `(= (mortal $x) (Human $x))`

### Storage Reality

**Important**: At the storage level, facts and rules are indistinguishable:

```rust
// Both stored as Atom in the same trie
space.add(expr![sym!("Human"), sym!("Socrates")]);  // Fact
space.add(expr![sym!("="), pattern, result]);        // Rule
```

**Distinction emerges during:**
- **Evaluation**: Rules (with `=`) are used for rewriting
- **Querying**: Facts are retrieved as-is
- **Semantics**: Programmer's interpretation of atoms

### Example Showing No Storage Distinction

```metta
; Both added identically
(add-atom &self (Human Socrates))           ; Conceptually a "fact"
(add-atom &self (= (mortal $x) (Human $x))) ; Conceptually a "rule"

; Both retrieved identically
!(get-atoms &self)
; → [(Human Socrates), (= (mortal $x) (Human $x))]

; Both can be removed identically
(remove-atom &self (Human Socrates))
(remove-atom &self (= (mortal $x) (Human $x)))
```

## Representing Facts

### Atomic Facts

**Symbols:**
```metta
(add-atom &self Socrates)
(add-atom &self red)
(add-atom &self active)
```

**Usage**: Represent named entities, states, or flags.

**Numbers:**
```metta
(add-atom &self 42)
(add-atom &self 3.14159)
(add-atom &self -100)
```

**Usage**: Represent numeric data.

**Strings:**
```metta
(add-atom &self "Alice")
(add-atom &self "Hello, World!")
(add-atom &self "")
```

**Usage**: Represent textual data.

### Relational Facts (N-ary Relations)

**Unary Relations (Properties):**
```metta
(Human Socrates)
(Even 42)
(Prime 17)
```

**Binary Relations:**
```metta
(parent Alice Bob)
(likes John Pizza)
(knows Alice Bob)
(age John 30)
```

**Ternary Relations:**
```metta
(edge node1 node2 5)
(works Alice CompanyX Manager)
(grade Student Course 95)
```

**N-ary Relations:**
```metta
(transaction Date Customer Product Quantity Price)
(record Field1 Field2 Field3 Field4 Field5)
```

### Nested Facts (Structured Data)

**Records/Structs:**
```metta
(person
    (name "Alice")
    (age 30)
    (address (street "Main St") (city "Boston")))
```

**Lists:**
```metta
(list 1 2 3 4 5)
(names Alice Bob Charlie)
```

**Trees:**
```metta
(tree
    (node 5
        (node 3 (leaf 1) (leaf 4))
        (node 7 (leaf 6) (leaf 9))))
```

**Graphs:**
```metta
(graph
    (nodes (1 2 3 4))
    (edges ((1 2) (2 3) (3 4) (4 1))))
```

## Adding Facts

### Using add-atom

**Syntax:**
```metta
(add-atom <space> <fact>)
```

**Examples:**
```metta
; Simple facts
(add-atom &self (Human Socrates))
(add-atom &self (age John 30))

; Complex facts
(add-atom &self (person (name "Alice") (age 30)))

; Multiple facts
(add-atom &self (Human Socrates))
(add-atom &self (Human Plato))
(add-atom &self (Human Aristotle))
```

### Bulk Loading Facts

**Pattern:**
```metta
; Define facts in a list, then add them
!(bind! &facts
    ((Human Socrates)
     (Human Plato)
     (Human Aristotle)))

!(match &facts $fact
    (add-atom &self $fact))
```

**From Computation:**
```metta
; Generate and add facts
!(match &nums (1 2 3 4 5) $n
    (add-atom &self (number $n)))
```

### Conditional Fact Addition

**Example:**
```metta
; Add fact only if condition holds
!(if (> $age 18)
    (add-atom &self (adult $person))
    ())
```

## Querying Facts

### Using get-atoms

**Retrieve All Facts:**
```metta
!(get-atoms &self)
; → [all atoms in space]
```

**Limitation**: Returns all atoms (facts + rules + everything), no filtering.

### Using match (Pattern Matching)

**Syntax:**
```metta
!(match <space> <pattern> <result-expression>)
```

**Examples:**

**Exact Matching:**
```metta
(add-atom &self (Human Socrates))
!(match &self (Human Socrates) found)
; → [found]
```

**Variable Binding:**
```metta
(add-atom &self (Human Socrates))
(add-atom &self (Human Plato))

!(match &self (Human $x) $x)
; → [Socrates, Plato]
```

**Complex Patterns:**
```metta
(add-atom &self (age John 30))
(add-atom &self (age Alice 25))

!(match &self (age $person $age) ($person $age))
; → [(John 30), (Alice 25)]
```

**Nested Patterns:**
```metta
(add-atom &self (person (name "Alice") (age 30)))

!(match &self (person (name $n) (age $a)) ($n $a))
; → [("Alice" 30)]
```

### Filtering Facts

**Pattern-Based Filtering:**
```metta
; Find all humans
!(match &self (Human $x) $x)

; Find specific age
!(match &self (age $person 30) $person)
```

**Computed Filtering:**
```metta
; Find adults (age > 18)
!(match &self (age $person $age)
    (if (> $age 18) $person ()))
```

## Updating Facts

### Replace Pattern (Remove + Add)

**MeTTa has no atomic update operation.** To update, remove old and add new:

```metta
; Update John's age from 30 to 31
(remove-atom &self (age John 30))
(add-atom &self (age John 31))
```

**Limitation**: Not atomic - if failure occurs between remove and add, fact is lost.

### Safer Update Pattern

**Check before update:**
```metta
; (= (update-age $person $new-age)
;     (if (match &self (age $person $old-age) True)
;         (seq (remove-atom &self (age $person $old-age))
;              (add-atom &self (age $person $new-age)))
;         (add-atom &self (age $person $new-age))))
```

## Deleting Facts

### Using remove-atom

**Syntax:**
```metta
(remove-atom <space> <exact-fact>)
```

**Examples:**
```metta
; Remove specific fact
(remove-atom &self (Human Socrates))  ; → True if found

; Remove with exact structure
(remove-atom &self (age John 30))  ; → True if found
```

### Pattern-Based Deletion

**Manual Approach:**
```metta
; Remove all humans
!(match &self (Human $x)
    (remove-atom &self (Human $x)))
```

**Careful with Evaluation Order:**
```metta
; This may not work as expected due to concurrent modification:
!(match &self $fact  ; Iterates over facts
    (remove-atom &self $fact))  ; Modifies space during iteration

; Better: collect first, then remove
!(bind! &to-remove (match &self $fact $fact))
!(match &to-remove $fact
    (remove-atom &self $fact))
```

## Fact Semantics

### Open World vs Closed World

**Open World Assumption (OWA):**
- Absence of fact ≠ fact is false
- Lack of `(Human Alien)` doesn't mean Alien is not human
- Default in MeTTa

**Closed World Assumption (CWA):**
- Absence of fact = fact is false
- Requires explicit negation
- Must be implemented at application level

**Example:**
```metta
; Only Socrates is explicitly human
(add-atom &self (Human Socrates))

; Query for Plato
!(match &self (Human Plato) found)
; → []  (no matches)

; Under OWA: Unknown if Plato is human
; Under CWA: Plato is not human (must be enforced by application)
```

### Fact Duplicates

**With AllowDuplication (default):**
```metta
(add-atom &self (Human Socrates))
(add-atom &self (Human Socrates))
(add-atom &self (Human Socrates))

!(match &self (Human Socrates) found)
; → [found, found, found]  (3 results)
```

**Interpretation:**
- Multiple instances ≠ more true
- May represent frequency, events, etc.
- Application must interpret meaning

**With NoDuplication:**
```metta
; Assuming NoDuplication strategy
(add-atom &self (Human Socrates))
(add-atom &self (Human Socrates))  ; Ignored

!(match &self (Human Socrates) found)
; → [found]  (1 result)
```

**Interpretation:**
- Set semantics (at most one instance)
- Standard logical interpretation

## Implementation Details

### Storage

**Facts Stored as Atoms:**
```rust
pub enum Atom {
    Symbol(Symbol),
    Variable(Variable),
    Expression(ExpressionAtom),
    Grounded(Grounded),
}
```

**In Trie:**
- Facts decomposed into tokens
- Stored at leaf nodes
- Indexed for efficient pattern matching

**Example:**
```metta
(Human Socrates)
```

**Stored as:**
```rust
// Tokens: [OpenParen, Symbol("Human"), Symbol("Socrates"), CloseParen]
// Trie path: root → OpenParen → Human → Socrates → CloseParen → Leaf
// Leaf contains: vec![Atom::Expression([Symbol("Human"), Symbol("Socrates")])]
```

### Retrieval

**Pattern Matching Process:**

1. **Parse Pattern:**
   ```metta
   (Human $x)
   ```
   Tokens: `[OpenParen, Symbol("Human"), Variable("x"), CloseParen]`

2. **Traverse Trie:**
   - Follow exact tokens: `OpenParen`, `Symbol("Human")`
   - At `Variable("x")`, explore all branches
   - Collect atoms at matching leaves

3. **Bind Variables:**
   - For each match, bind `$x` to corresponding value
   - Return results with bindings applied

**Implementation**: `hyperon-space/src/index/trie.rs:450-650`

### Query Optimization

**Trie Benefits for Facts:**
- Ground prefixes narrow search space
- Shared structure reduces memory
- Efficient for large fact databases

**Example:**
```metta
; Database with 10,000 facts about humans
(add-atom &self (Human Socrates))
(add-atom &self (Human Plato))
; ... 9,998 more ...

; Query efficiently uses trie index:
!(match &self (Human $x) $x)
; Only searches (Human ...) branch, not other facts
```

## Common Fact Patterns

### Entity-Attribute-Value (EAV)

**Pattern:**
```metta
(attribute Entity Value)
```

**Examples:**
```metta
(age John 30)
(name Alice "Alice Smith")
(salary Bob 50000)
```

**Queries:**
```metta
; Get all attributes of John
!(match &self ($attr John $val) ($attr $val))

; Get all entities with age
!(match &self (age $entity $age) $entity)
```

### Subject-Predicate-Object (RDF-style)

**Pattern:**
```metta
(triple Subject Predicate Object)
```

**Examples:**
```metta
(triple Socrates type Human)
(triple Socrates age 70)
(triple Socrates teacher Plato)
```

**Queries:**
```metta
; Get all facts about Socrates
!(match &self (triple Socrates $pred $obj) ($pred $obj))
```

### Datalog-style Facts

**Pattern:**
```metta
(relation Arg1 Arg2 ... ArgN)
```

**Examples:**
```metta
(parent Alice Bob)
(parent Bob Charlie)
(sibling Alice Dave)
```

**Queries:**
```metta
; Who are Alice's children?
!(match &self (parent Alice $child) $child)

; Who are Charlie's grandparents?
!(match &self (parent $parent Charlie)
    (match &self (parent $grandparent $parent)
        $grandparent))
```

### Property Lists

**Pattern:**
```metta
(entity (property1 value1) (property2 value2) ...)
```

**Examples:**
```metta
(person
    (name "Alice")
    (age 30)
    (city "Boston"))
```

**Queries:**
```metta
; Get person named Alice
!(match &self (person (name "Alice") $other-props) $other-props)
```

## Best Practices

### 1. Consistent Naming

**Use clear, consistent naming for relations:**
```metta
; Good
(parent Alice Bob)
(age John 30)
(knows Alice Bob)

; Avoid mixing conventions
(Parent Alice Bob)  ; Inconsistent capitalization
(John-age 30)       ; Different structure
```

### 2. Normalize Data

**Avoid redundancy:**
```metta
; Good
(person (id 1) (name "Alice"))
(age 1 30)

; Redundant
(person (id 1) (name "Alice") (age 30))
(age "Alice" 30)
```

### 3. Use Appropriate Granularity

**Fine-grained:**
```metta
(firstName John)
(lastName Doe)
(age John 30)
```

**Coarse-grained:**
```metta
(person John (name "John" "Doe") (age 30))
```

**Choose based on query patterns and update frequency.**

### 4. Document Schema

**Maintain documentation of fact structure:**
```metta
; Schema documentation:
; (Human <entity>) - entity is human
; (age <entity> <number>) - entity's age in years
; (parent <parent> <child>) - parent-child relationship

(add-atom &self (Human Socrates))
(add-atom &self (age Socrates 70))
```

### 5. Validate Facts

**Implement validation at application level:**
```metta
; (= (add-validated-age $person $age)
;     (if (and (> $age 0) (< $age 150))
;         (add-atom &self (age $person $age))
;         (print "Invalid age")))
```

### 6. Use Typed Facts When Appropriate

**Leverage MeTTa's type system:**
```metta
; Type-annotated facts
(: john Person)
(: (age john) (-> Person Nat))
(add-atom &self (age john 30))
```

## Performance Considerations

### Query Performance

**Efficient Queries (use ground prefixes):**
```metta
; Good: (Human ...) narrows search
!(match &self (Human $x) $x)

; Less efficient: searches all atoms
!(match &self $x $x)
```

**Avoid:**
```metta
; Very inefficient: full scan
!(match &self $anything $anything)
```

### Fact Organization

**Group Related Facts:**
```metta
; Good: shared prefix (age ...)
(add-atom &self (age John 30))
(add-atom &self (age Alice 25))
(add-atom &self (age Bob 35))

; Trie benefits from shared structure
```

### Duplication Strategy

**Choose appropriate strategy:**

**AllowDuplication:**
- Use when duplicates meaningful (e.g., events)
- Allows counting occurrences

**NoDuplication:**
- Use for unique constraints (e.g., properties)
- Reduces space usage
- Faster queries (fewer duplicates)

## Integration with Types

### Type Annotations as Facts

**Type assertions are facts:**
```metta
(: socrates Human)
(: plato Human)
```

**Stored like any other fact:**
```rust
// (:  socrates Human) stored as expression with `:` operator
```

**Query types:**
```metta
!(match &self (: $entity Human) $entity)
; → [socrates, plato]
```

### Type-Checked Fact Addition

**Use type system to validate facts:**
```metta
; Define typed addition function
(: add-person-age (-> Person Nat ()))
(= (add-person-age $person $age)
    (add-atom &self (age $person $age)))

; Type checking ensures valid arguments
```

## Related Documentation

- **[Rules](04-rules.md)** - How rules differ from facts
- **[Adding Atoms](01-adding-atoms.md)** - Detailed add-atom behavior
- **[Removing Atoms](02-removing-atoms.md)** - Detailed remove-atom behavior
- **[Space Operations](05-space-operations.md)** - All space query operations
- **Type System** - `../type-system/01-fundamentals.md`

## Examples

See **[examples/01-facts.metta](examples/01-facts.metta)** for executable examples of:
- Adding various types of facts
- Querying facts with patterns
- Updating and deleting facts
- Common fact patterns (EAV, RDF-style, Datalog-style)

## Summary

**Facts in MeTTa:**
✅ Any atom in an atom space representing data
✅ No syntactic distinction from other atoms at storage level
✅ Stored efficiently in trie structure
✅ Queried via pattern matching
✅ Flexible representation (atomic, relational, nested)

**Key Points:**
- Facts are conceptual, not technically distinct from other atoms
- Use `add-atom` to add facts
- Use `match` to query facts with patterns
- Use `remove-atom` to delete specific facts
- No atomic updates (must remove + add)
- Storage strategy (AllowDuplication vs NoDuplication) affects duplicates

**Best Practices:**
- Consistent naming conventions
- Normalize data appropriately
- Document fact schemas
- Validate at application level
- Choose appropriate granularity
- Use type annotations when beneficial

---

**Version**: 1.0
**Based on**: hyperon-experimental commit `164c22e9`
**Created**: 2025-11-13
