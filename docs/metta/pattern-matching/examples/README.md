# Pattern Matching Examples

This directory contains executable MeTTa examples demonstrating pattern matching concepts. Each file is self-contained with setup code, examples, and explanations.

## Overview

**Total Examples**: 129 across 8 files
**Format**: Executable `.metta` files
**Organization**: By topic, from basic to advanced

## Quick Start

### Running Examples

```bash
# Navigate to examples directory
cd docs/metta/pattern-matching/examples

# Run with MeTTa interpreter
metta 01-basic-patterns.metta

# Run specific file
metta 06-advanced-matching.metta
```

### Interactive Testing

```bash
# Start MeTTa REPL
metta

# Load example file
!(load "01-basic-patterns.metta")

# Or run individual examples
!(match &self (Human $x) $x)
```

## Example Files

### 01-basic-patterns.metta

**Focus**: Fundamental pattern matching

**Topics Covered**:
- Variable patterns (`$x`)
- Ground term matching
- Simple expression patterns
- Basic queries
- Template construction

**Example Count**: 10

**Key Examples**:
```metta
; Variable extraction
!(match &self (Human $x) $x)

; Shared variable constraint
!(match &self (same $x $x) $x)

; Multiple variable extraction
!(match &self (age $person $years) ($person is $years years old))
```

**Difficulty**: ⭐ Beginner

**Prerequisites**: None

**Related Docs**: [01-fundamentals.md](../01-fundamentals.md), [03-match-operation.md](../03-match-operation.md)

---

### 02-expression-patterns.metta

**Focus**: Matching complex expressions

**Topics Covered**:
- Multi-level nested patterns
- Deep nesting
- Structural decomposition
- Expression transformation
- Head-based matching

**Example Count**: 12

**Key Examples**:
```metta
; Nested structure extraction
!(match &self
    (address (person $name) (city $c) (state $s))
    ($name lives in $c $s))

; Deep nesting (3+ levels)
!(match &self
    (company (info (name $n) (founded $y)) (address $addr))
    ($n was founded in $y at $addr))
```

**Difficulty**: ⭐⭐ Intermediate

**Prerequisites**: 01-basic-patterns.metta

**Related Docs**: [01-fundamentals.md](../01-fundamentals.md#expression-patterns)

---

### 03-bindings.metta

**Focus**: Variable bindings and resolution

**Topics Covered**:
- Single and multiple variable bindings
- Shared variable constraints
- Binding consistency
- Variable scope
- Transitive bindings

**Example Count**: 15

**Key Examples**:
```metta
; Multiple bindings
!(match &self (value $name $num) (binding $name to $num))

; Shared variable constraint
!(match &self (same $x $x) $x)

; Complex nested bindings
!(match &self
    (record (id $i) (data (x $x_val) (y $y_val)))
    (entry $i coords $x_val $y_val))
```

**Difficulty**: ⭐⭐ Intermediate

**Prerequisites**: 01-basic-patterns.metta

**Related Docs**: [04-bindings.md](../04-bindings.md)

---

### 04-conjunction-queries.metta

**Focus**: Multi-pattern queries with comma operator

**Topics Covered**:
- Conjunction syntax
- Shared variables across patterns
- Cartesian products
- Transitive relationships
- Query optimization

**Example Count**: 15

**Key Examples**:
```metta
; Simple conjunction
!(match &self
    (, (Human $x)
       (philosopher $x))
    $x)

; Cartesian product (no shared variables)
!(match &self
    (, (color $c)
       (size $s))
    (item $c $s))

; Grandparent query (transitive)
!(match &self
    (, (parent $gp $p)
       (parent $p $gc))
    (grandparent $gp of $gc))
```

**Difficulty**: ⭐⭐⭐ Intermediate-Advanced

**Prerequisites**: 01-basic-patterns.metta, 03-bindings.metta

**Related Docs**: [03-match-operation.md](../03-match-operation.md#conjunction-queries), [05-pattern-contexts.md](../05-pattern-contexts.md#conjunction-queries)

---

### 05-non-determinism.metta

**Focus**: Non-deterministic evaluation and multiple results

**Topics Covered**:
- Multiple matching rules
- Multiple space matches
- Result ordering
- Controlling non-determinism
- Filtering and aggregation

**Example Count**: 17

**Key Examples**:
```metta
; Multiple rules
(= (color) red)
(= (color) green)
(= (color) blue)
!(color)  ; Non-deterministic

; Cartesian product explosion
!(match &self
    (, (first $a)
       (second $b))
    (pair $a $b))

; Filtering results
!(match &self
    (age $person $years)
    (if (> $years 50)
        $person
        ()))
```

**Difficulty**: ⭐⭐⭐ Advanced

**Prerequisites**: 01-basic-patterns.metta, 04-conjunction-queries.metta

**Related Docs**: [08-non-determinism.md](../08-non-determinism.md)

---

### 06-advanced-matching.metta

**Focus**: Advanced patterns and techniques

**Topics Covered**:
- Deep nested patterns
- Recursive pattern matching
- Pattern guards and conditions
- Transitive closure
- Path finding
- Aggregation patterns
- Join operations

**Example Count**: 20

**Key Examples**:
```metta
; Deep nested extraction
!(match &self
    (organization
        (info (name $n) (type $t))
        (location (city $c) (state $s))
        (size (employees $e) (revenue $r)))
    (company-profile $n $t in $c $s with $e employees))

; Transitive closure
(= (reaches $x $y)
    (match &self (edge $x $y) True))
(= (reaches $x $z)
    (match &self
        (edge $x $y)
        (if (reaches $y $z) True False)))

; Pattern with guards
!(match &self
    (product (name $n) (price $p))
    (if (> $p 15.0)
        (expensive $n costs $p)
        ()))
```

**Difficulty**: ⭐⭐⭐⭐ Advanced

**Prerequisites**: All previous examples

**Related Docs**: [06-advanced-patterns.md](../06-advanced-patterns.md)

---

### 07-unify-operation.metta

**Focus**: Unify operation for conditional matching

**Topics Covered**:
- Unify syntax and semantics
- Pattern testing
- Conditional branching (then/else)
- Comparison with match
- Validation patterns

**Example Count**: 20

**Key Examples**:
```metta
; Basic unify
!(unify 42 $x $x "no match")
; → 42

; Conditional logic
!(unify (Human Socrates) (Human $x)
    (name is $x)
    "not human")
; → (name is Socrates)

; Chained unify
!(unify (command "save" "file.txt") (command $cmd $arg)
    (unify $cmd "save"
        (save-file $arg)
        (unknown-command $cmd))
    "invalid format")
```

**Difficulty**: ⭐⭐⭐ Intermediate-Advanced

**Prerequisites**: 01-basic-patterns.metta

**Related Docs**: [05-pattern-contexts.md](../05-pattern-contexts.md#unify-operation)

---

### 08-knowledge-queries.metta

**Focus**: Knowledge base queries and reasoning

**Topics Covered**:
- Building knowledge bases
- Simple and complex queries
- Derived facts
- Inference rules
- Transitive relationships
- Aggregation
- Existential and universal queries

**Example Count**: 20

**Key Examples**:
```metta
; Simple fact retrieval
!(match &self (Human $x) $x)

; Multi-attribute query
!(match &self
    (, (philosopher $p)
       (age $p $a)
       (Greek $p))
    (profile $p age $a))

; Transitive relationship
(= (studied-under $student $teacher)
    (match &self (teacher $teacher $student) True))
(= (studied-under $student $ancestor)
    (match &self
        (, (teacher $teacher $student)
           (studied-under $teacher $ancestor))
        True))

; Aggregation
(= (average-age)
    (let $ages (match &self (age $_ $a) $a)
        (/ (sum $ages) (length $ages))))
```

**Difficulty**: ⭐⭐⭐⭐ Advanced

**Prerequisites**: 01-basic-patterns.metta, 04-conjunction-queries.metta, 06-advanced-matching.metta

**Related Docs**: [06-advanced-patterns.md](../06-advanced-patterns.md#complex-query-patterns)

---

## Example Structure

Each example file follows this structure:

```metta
; ============================================================================
; File Title and Overview
; ============================================================================
; Brief description of topics covered

; Setup: Create knowledge base
; ============================================================================
; Code to populate knowledge base with test data

; Example N: Topic Name
; ============================================================================
; Explanation of example

!(example-code here)
; Expected output or result
; Additional notes

; ============================================================================
; Summary
; ============================================================================
; Recap of concepts demonstrated
```

## Learning Path

### Beginner Path

1. **Start Here**: [01-basic-patterns.metta](01-basic-patterns.metta)
   - Learn fundamental patterns
   - Understand variable binding
   - Practice simple queries

2. **Next**: [02-expression-patterns.metta](02-expression-patterns.metta)
   - Work with nested structures
   - Practice pattern decomposition

3. **Then**: [03-bindings.metta](03-bindings.metta)
   - Understand binding mechanics
   - Learn about constraints

4. **Finally**: [07-unify-operation.metta](07-unify-operation.metta)
   - Learn conditional matching
   - Understand branching logic

### Intermediate Path

1. **Review**: [01-basic-patterns.metta](01-basic-patterns.metta) (if needed)

2. **Start**: [04-conjunction-queries.metta](04-conjunction-queries.metta)
   - Multi-pattern matching
   - Complex relationships
   - Query optimization

3. **Continue**: [05-non-determinism.metta](05-non-determinism.metta)
   - Handle multiple results
   - Control strategies
   - Performance awareness

4. **Practice**: [08-knowledge-queries.metta](08-knowledge-queries.metta)
   - Build knowledge bases
   - Write inference rules
   - Perform reasoning

### Advanced Path

1. **Master**: [06-advanced-matching.metta](06-advanced-matching.metta)
   - Recursive patterns
   - Complex inference
   - Optimization techniques

2. **Integrate**: [08-knowledge-queries.metta](08-knowledge-queries.metta)
   - Sophisticated reasoning
   - Multi-level inference
   - Graph algorithms

3. **Explore**: Write your own examples
   - Combine techniques
   - Solve real problems
   - Optimize queries

## Tips for Working with Examples

### Running Examples

1. **Sequential Execution**: Run examples in order within each file
2. **Fresh Space**: Each file sets up its own knowledge base
3. **Incremental Learning**: Build understanding progressively

### Modifying Examples

1. **Experiment**: Change patterns and see results
2. **Add Data**: Extend knowledge bases with new facts
3. **Combine Techniques**: Mix patterns from different files

### Debugging Examples

1. **Check Spacing**: Ensure proper spaces (not tabs)
2. **Verify Syntax**: Match parentheses carefully
3. **Inspect Results**: Use `!(get-atoms &self)` to see space contents
4. **Step Through**: Run examples one at a time

## Common Patterns

### Pattern Glossary

```metta
; Variable
$x

; Ground term
Socrates

; Simple expression
(Human $x)

; Nested expression
(person (name $n) (age $a))

; Shared variable (constraint)
(same $x $x)

; Wildcard
$_

; Conjunction
(, (Human $x) (philosopher $x))

; Template with variables
($x is $y years old)

; Unify with branches
(unify <atom> <pattern> <then> <else>)
```

### Query Templates

```metta
; Basic query
!(match &self (<pattern>) <template>)

; Conjunction query
!(match &self (, <pattern1> <pattern2>) <template>)

; Filtered query
!(match &self <pattern>
    (if <condition>
        <result>
        ()))

; Conditional match
!(unify <atom> <pattern> <then> <else>)

; Nested query
!(match &self <pattern1>
    (match &self <pattern2> <template>))
```

## Troubleshooting

### Common Issues

**Issue**: Example doesn't return expected results
- **Check**: Space setup code ran correctly
- **Verify**: Pattern syntax is correct
- **Debug**: Use `!(get-atoms &self)` to inspect space

**Issue**: Variable not binding correctly
- **Check**: Variable used in both pattern and template
- **Verify**: Variable names consistent
- **Debug**: Test simpler version of pattern

**Issue**: Too many/too few results
- **Check**: Pattern specificity (add constraints)
- **Verify**: Conjunction logic (shared variables)
- **Debug**: Test each pattern component separately

**Issue**: Pattern never matches
- **Check**: Space contains matching atoms
- **Verify**: Expression structure matches exactly
- **Debug**: Simplify pattern to find mismatch

### Getting Help

1. **Read Documentation**: See related docs for each example
2. **Check Edge Cases**: [09-edge-cases.md](../09-edge-cases.md)
3. **Review Fundamentals**: [01-fundamentals.md](../01-fundamentals.md)
4. **Consult Index**: [INDEX.md](../INDEX.md) for specific topics

## Additional Resources

**Related Documentation**:
- [Main README](../README.md) - Documentation overview
- [INDEX.md](../INDEX.md) - Comprehensive topic index
- [STATUS.md](../STATUS.md) - Documentation status

**Core Concepts**:
- [Fundamentals](../01-fundamentals.md) - Pattern basics
- [Unification](../02-unification.md) - Matching algorithm
- [Match Operation](../03-match-operation.md) - Query operation
- [Bindings](../04-bindings.md) - Variable bindings

**Advanced Topics**:
- [Advanced Patterns](../06-advanced-patterns.md) - Complex techniques
- [Implementation](../07-implementation.md) - How it works
- [Non-Determinism](../08-non-determinism.md) - Multiple results
- [Edge Cases](../09-edge-cases.md) - Gotchas and debugging

## Contributing Examples

To contribute new examples:

1. **Follow Structure**: Use standard example format
2. **Add Comments**: Explain what each example demonstrates
3. **Show Expected Output**: Include expected results
4. **Test Thoroughly**: Verify all examples work
5. **Update This README**: Document new examples

## Version Information

**Examples Version**: 1.0
**MeTTa Version**: Based on hyperon-experimental commit `164c22e9`
**Last Updated**: 2025-11-17

---

**Happy Pattern Matching!** 🎯

Start with [01-basic-patterns.metta](01-basic-patterns.metta) and work your way through. Each example builds on previous concepts.
