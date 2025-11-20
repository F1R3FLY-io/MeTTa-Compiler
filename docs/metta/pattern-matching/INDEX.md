# Pattern Matching Documentation Index

Comprehensive index of all topics covered in the pattern matching documentation.

## Core Concepts

### Pattern Matching Basics
- **What is Pattern Matching?** - [00-overview.md](00-overview.md#what-is-pattern-matching)
- **Patterns vs Atoms** - [01-fundamentals.md](01-fundamentals.md#patterns-vs-atoms)
- **Pattern Syntax** - [01-fundamentals.md](01-fundamentals.md#pattern-syntax)
- **Variables** - [01-fundamentals.md](01-fundamentals.md#variables)
  - Variable naming - [01-fundamentals.md](01-fundamentals.md#variable-naming)
  - Variable scope - [06-advanced-patterns.md](06-advanced-patterns.md#variable-scope)
  - Variable identity - [06-advanced-patterns.md](06-advanced-patterns.md#variable-identity)
- **Ground Terms** - [01-fundamentals.md](01-fundamentals.md#ground-terms)
- **Expression Patterns** - [01-fundamentals.md](01-fundamentals.md#expression-patterns)

### Unification
- **Unification Algorithm** - [02-unification.md](02-unification.md#unification-algorithm)
- **Bidirectional Matching** - [02-unification.md](02-unification.md#bidirectional-nature)
- **Occurs Check** - [02-unification.md](02-unification.md#occurs-check)
  - Implementation - [07-implementation.md](07-implementation.md#occurs-check)
- **Unification Rules** - [02-unification.md](02-unification.md#unification-rules)
- **Symmetric Unification** - [02-unification.md](02-unification.md#symmetry)

### Bindings
- **What are Bindings?** - [04-bindings.md](04-bindings.md#what-are-bindings)
- **Variable Assignments** - [04-bindings.md](04-bindings.md#two-types-of-bindings)
- **Variable Equalities** - [04-bindings.md](04-bindings.md#two-types-of-bindings)
- **Bindings Data Structure** - [04-bindings.md](04-bindings.md#bindings-implementation)
  - Implementation - [07-implementation.md](07-implementation.md#bindings-implementation)
- **Creating Bindings** - [04-bindings.md](04-bindings.md#creating-bindings)
- **Resolving Bindings** - [04-bindings.md](04-bindings.md#resolving-bindings)
  - Resolution algorithm - [07-implementation.md](07-implementation.md#resolve-implementation)
- **Merging Bindings** - [04-bindings.md](04-bindings.md#merging-bindings)
- **Binding Consistency** - [04-bindings.md](04-bindings.md#binding-consistency)
- **BindingsSet** - [04-bindings.md](04-bindings.md#bindingsset)
  - Implementation - [07-implementation.md](07-implementation.md#bindingsset-implementation)

## Operations

### Match Operation
- **Match Specification** - [03-match-operation.md](03-match-operation.md#match-operation-specification)
- **Match Syntax** - [03-match-operation.md](03-match-operation.md#syntax)
- **Match Implementation** - [03-match-operation.md](03-match-operation.md#implementation)
  - Execution flow - [07-implementation.md](07-implementation.md#match-execution-flow)
- **Space Queries** - [03-match-operation.md](03-match-operation.md#space-query)
- **Template Evaluation** - [03-match-operation.md](03-match-operation.md#template-evaluation)
- **Match Examples** - [examples/01-basic-patterns.metta](examples/01-basic-patterns.metta)

### Unify Operation
- **Unify Specification** - [05-pattern-contexts.md](05-pattern-contexts.md#unify-operation)
- **Unify Syntax** - [05-pattern-contexts.md](05-pattern-contexts.md#unify-operation-details)
- **Unify vs Match** - [05-pattern-contexts.md](05-pattern-contexts.md#unify-vs-match)
- **Unify Examples** - [examples/07-unify-operation.metta](examples/07-unify-operation.metta)

### Conjunction Queries
- **Comma Operator** - [03-match-operation.md](03-match-operation.md#conjunction-queries)
- **Multi-Pattern Matching** - [05-pattern-contexts.md](05-pattern-contexts.md#conjunction-query-details)
- **Conjunction Semantics** - [05-pattern-contexts.md](05-pattern-contexts.md#multi-pattern-matching)
- **Conjunction Optimization** - [05-pattern-contexts.md](05-pattern-contexts.md#conjunction-optimization)
- **Conjunction Examples** - [examples/04-conjunction-queries.metta](examples/04-conjunction-queries.metta)

## Pattern Contexts

### Different Contexts
- **Overview of Contexts** - [05-pattern-contexts.md](05-pattern-contexts.md#pattern-matching-contexts)
- **Match Context** - [05-pattern-contexts.md](05-pattern-contexts.md#match-operation-queries)
- **Rule Application** - [05-pattern-contexts.md](05-pattern-contexts.md#rule-application)
- **Unify Context** - [05-pattern-contexts.md](05-pattern-contexts.md#unify-operation)
- **Conjunction Context** - [05-pattern-contexts.md](05-pattern-contexts.md#conjunction-queries)
- **Destructuring** - [05-pattern-contexts.md](05-pattern-contexts.md#destructuring-implicit)
- **Context Comparison** - [05-pattern-contexts.md](05-pattern-contexts.md#context-comparison)

### Rule Evaluation
- **Rule Matching Process** - [05-pattern-contexts.md](05-pattern-contexts.md#pattern-matching-in-rule-evaluation)
- **Multiple Matching Rules** - [05-pattern-contexts.md](05-pattern-contexts.md#multiple-matching-rules)
- **Rule Precedence** - [05-pattern-contexts.md](05-pattern-contexts.md#rule-precedence)

## Advanced Topics

### Advanced Patterns
- **Nested Patterns** - [06-advanced-patterns.md](06-advanced-patterns.md#nested-patterns)
  - Deep nesting - [06-advanced-patterns.md](06-advanced-patterns.md#deep-nesting)
  - Multi-level extraction - [06-advanced-patterns.md](06-advanced-patterns.md#multi-level-extraction)
- **Custom Matching** - [06-advanced-patterns.md](06-advanced-patterns.md#custom-matching)
  - CustomMatch trait - [06-advanced-patterns.md](06-advanced-patterns.md#custommatch-trait)
  - Implementation - [06-advanced-patterns.md](06-advanced-patterns.md#custom-match-example)
- **Variable Scope** - [06-advanced-patterns.md](06-advanced-patterns.md#variable-scope)
- **Variable Equivalence** - [06-advanced-patterns.md](06-advanced-patterns.md#variable-equivalence)
  - Equivalence checking - [06-advanced-patterns.md](06-advanced-patterns.md#equivalence-checking)
- **Pattern Guards** - [06-advanced-patterns.md](06-advanced-patterns.md#pattern-guards-constraints)
- **Recursive Patterns** - [06-advanced-patterns.md](06-advanced-patterns.md#recursive-patterns)
- **Pattern Optimization** - [06-advanced-patterns.md](06-advanced-patterns.md#pattern-optimization-strategies)
- **Complex Queries** - [06-advanced-patterns.md](06-advanced-patterns.md#complex-query-patterns)
  - Transitive closure - [06-advanced-patterns.md](06-advanced-patterns.md#transitive-closure)
  - Path finding - [06-advanced-patterns.md](06-advanced-patterns.md#path-finding)
  - Aggregation - [06-advanced-patterns.md](06-advanced-patterns.md#aggregation-patterns)
  - Negation - [06-advanced-patterns.md](06-advanced-patterns.md#negation-patterns)
- **Pattern Metaprogramming** - [06-advanced-patterns.md](06-advanced-patterns.md#pattern-metaprogramming)
- **Examples** - [examples/06-advanced-matching.metta](examples/06-advanced-matching.metta)

### Non-Determinism
- **What is Non-Determinism?** - [08-non-determinism.md](08-non-determinism.md#what-is-non-determinism)
- **Sources of Non-Determinism** - [08-non-determinism.md](08-non-determinism.md#sources-of-non-determinism)
  - Multiple rules - [08-non-determinism.md](08-non-determinism.md#multiple-matching-rules)
  - Multiple matches - [08-non-determinism.md](08-non-determinism.md#multiple-space-matches)
  - Conjunction - [08-non-determinism.md](08-non-determinism.md#conjunction-with-multiple-solutions)
  - Recursion - [08-non-determinism.md](08-non-determinism.md#recursive-rules)
- **Non-Deterministic Evaluation** - [08-non-determinism.md](08-non-determinism.md#non-deterministic-evaluation)
- **Controlling Non-Determinism** - [08-non-determinism.md](08-non-determinism.md#controlling-non-determinism)
- **Result Ordering** - [08-non-determinism.md](08-non-determinism.md#ordering)
- **Performance Implications** - [08-non-determinism.md](08-non-determinism.md#performance-implications)
- **Examples** - [examples/05-non-determinism.metta](examples/05-non-determinism.metta)

## Implementation

### Core Implementation
- **Architecture Overview** - [07-implementation.md](07-implementation.md#architecture-overview)
- **matcher.rs Implementation** - [07-implementation.md](07-implementation.md#matcherrs-implementation)
  - match_atoms function - [07-implementation.md](07-implementation.md#match_atoms-function)
  - Occurs check - [07-implementation.md](07-implementation.md#occurs-check)
  - Variable handling - [07-implementation.md](07-implementation.md#variable-handling)
- **Bindings Implementation** - [07-implementation.md](07-implementation.md#bindings-implementation)
  - Data structure - [07-implementation.md](07-implementation.md#data-structure)
  - HoleyVec - [07-implementation.md](07-implementation.md#holeyvec)
  - Binding groups - [07-implementation.md](07-implementation.md#binding-groups)
  - add_var_binding - [07-implementation.md](07-implementation.md#add_var_binding-implementation)
  - resolve - [07-implementation.md](07-implementation.md#resolve-implementation)
  - merge - [07-implementation.md](07-implementation.md#merge-implementation)
- **BindingsSet Implementation** - [07-implementation.md](07-implementation.md#bindingsset-implementation)
- **Integration Points** - [07-implementation.md](07-implementation.md#integration-points)

### Query Optimization
- **AtomTrie Structure** - [07-implementation.md](07-implementation.md#atomtrie-structure)
- **Query Algorithm** - [07-implementation.md](07-implementation.md#query-algorithm)
- **Trie-Based Pruning** - [03-match-operation.md](03-match-operation.md#trie-based-pruning)
- **Query Complexity** - [03-match-operation.md](03-match-operation.md#query-complexity)
- **Optimization Strategies** - [03-match-operation.md](03-match-operation.md#optimization-strategies)
- **Ground Prefix Optimization** - [06-advanced-patterns.md](06-advanced-patterns.md#ground-prefix-optimization)

### Performance
- **Time Complexity** - [07-implementation.md](07-implementation.md#time-complexity)
- **Space Complexity** - [07-implementation.md](07-implementation.md#space-complexity)
- **Memory Layout** - [07-implementation.md](07-implementation.md#memory-layout)
- **Optimization Techniques** - [07-implementation.md](07-implementation.md#optimization-strategies)
- **Benchmarking** - [07-implementation.md](07-implementation.md#benchmarking-considerations)

## Edge Cases and Debugging

### Edge Cases
- **Empty Patterns** - [09-edge-cases.md](09-edge-cases.md#empty-patterns-and-atoms)
- **Variable Patterns** - [09-edge-cases.md](09-edge-cases.md#variable-patterns)
- **Cyclic Structures** - [09-edge-cases.md](09-edge-cases.md#cyclic-structures)
- **Type Mismatches** - [09-edge-cases.md](09-edge-cases.md#type-mismatches)
- **Unbound Variables** - [09-edge-cases.md](09-edge-cases.md#unbound-variables)
- **Infinite Patterns** - [09-edge-cases.md](09-edge-cases.md#infinite-patterns)
- **Large Expressions** - [09-edge-cases.md](09-edge-cases.md#large-expressions)
- **Grounded Atom Edge Cases** - [09-edge-cases.md](09-edge-cases.md#grounded-atom-edge-cases)
- **Performance Edge Cases** - [09-edge-cases.md](09-edge-cases.md#performance-edge-cases)

### Debugging
- **Debugging Edge Cases** - [09-edge-cases.md](09-edge-cases.md#debugging-edge-cases)
- **Error Handling** - [09-edge-cases.md](09-edge-cases.md#error-handling-best-practices)
- **Testing Edge Cases** - [09-edge-cases.md](09-edge-cases.md#testing-edge-cases)
- **Common Gotchas** - [09-edge-cases.md](09-edge-cases.md#common-gotchas)

## Examples

### By Topic
- **Basic Patterns** - [examples/01-basic-patterns.metta](examples/01-basic-patterns.metta)
  - Variable patterns
  - Ground terms
  - Simple expressions
- **Expression Patterns** - [examples/02-expression-patterns.metta](examples/02-expression-patterns.metta)
  - Nested structures
  - Complex expressions
  - Pattern transformation
- **Bindings** - [examples/03-bindings.metta](examples/03-bindings.metta)
  - Variable bindings
  - Shared variables
  - Binding resolution
- **Conjunction** - [examples/04-conjunction-queries.metta](examples/04-conjunction-queries.metta)
  - Multi-pattern queries
  - Cartesian products
  - Transitive relationships
- **Non-Determinism** - [examples/05-non-determinism.metta](examples/05-non-determinism.metta)
  - Multiple results
  - Result filtering
  - Aggregation
- **Advanced Matching** - [examples/06-advanced-matching.metta](examples/06-advanced-matching.metta)
  - Recursive patterns
  - Pattern guards
  - Complex queries
- **Unify Operation** - [examples/07-unify-operation.metta](examples/07-unify-operation.metta)
  - Conditional matching
  - Pattern testing
  - Branching logic
- **Knowledge Queries** - [examples/08-knowledge-queries.metta](examples/08-knowledge-queries.metta)
  - Knowledge base queries
  - Inference
  - Reasoning

## Quick References

### Operations Quick Reference
- **match** syntax - [03-match-operation.md](03-match-operation.md#syntax)
- **unify** syntax - [05-pattern-contexts.md](05-pattern-contexts.md#specification)
- **Conjunction** syntax - [03-match-operation.md](03-match-operation.md#comma-operator)

### Pattern Types Quick Reference
- Variables: `$x`
- Ground terms: `Socrates`
- Expressions: `(Human $x)`
- Nested: `(person (name $n) (age $a))`
- Shared variable: `(same $x $x)`
- Wildcard: `$_`

### Common Patterns
- **Simple query** - [README.md](README.md#quick-reference)
- **Conjunction** - [README.md](README.md#quick-reference)
- **Unify** - [README.md](README.md#quick-reference)
- **Nested pattern** - [README.md](README.md#quick-reference)

### Code Locations
- **Unification**: `hyperon-atom/src/matcher.rs:1089-1129`
- **Bindings**: `hyperon-atom/src/matcher.rs:140-765`
- **BindingsSet**: `hyperon-atom/src/matcher.rs:886-1044`
- **Match Operation**: `lib/src/metta/runner/stdlib/core.rs:141-167`
- **Space Query**: `hyperon-space/src/lib.rs:156-175`
- **AtomTrie**: `hyperon-space/src/index/trie.rs`
- **Custom Matching**: `lib/examples/custom_match.rs:1-100`

## Best Practices

### Pattern Writing
- **Descriptive Names** - [06-advanced-patterns.md](06-advanced-patterns.md#use-descriptive-variable-names)
- **Structural Constraints** - [06-advanced-patterns.md](06-advanced-patterns.md#leverage-structural-constraints)
- **Documentation** - [06-advanced-patterns.md](06-advanced-patterns.md#document-complex-patterns)
- **Testing** - [06-advanced-patterns.md](06-advanced-patterns.md#test-patterns-incrementally)
- **Custom Matching** - [06-advanced-patterns.md](06-advanced-patterns.md#use-custom-matching-judiciously)

### Query Optimization
- **Ground Prefixes** - [03-match-operation.md](03-match-operation.md#ground-prefixes)
- **Specific Patterns** - [03-match-operation.md](03-match-operation.md#specific-patterns)
- **Conjunction Order** - [03-match-operation.md](03-match-operation.md#order-in-conjunctions)
- **Early Filtering** - [05-pattern-contexts.md](05-pattern-contexts.md#optimize-conjunction-order)

### Error Handling
- **Validate Inputs** - [09-edge-cases.md](09-edge-cases.md#validate-inputs)
- **Handle Empty Results** - [09-edge-cases.md](09-edge-cases.md#handle-empty-results)
- **Limit Recursion** - [09-edge-cases.md](09-edge-cases.md#limit-recursion-depth)
- **Catch Type Errors** - [09-edge-cases.md](09-edge-cases.md#catch-type-errors)
- **Log Issues** - [09-edge-cases.md](09-edge-cases.md#log-unexpected-cases)

## Version Information

**Documentation Version**: 1.0
**Based on**: hyperon-experimental commit `164c22e9`
**Created**: 2025-11-17

---

**Navigation**:
- [README.md](README.md) - Main documentation index
- [STATUS.md](STATUS.md) - Documentation completeness
- [examples/README.md](examples/README.md) - Examples guide
