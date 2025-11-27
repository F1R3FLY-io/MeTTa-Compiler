# MeTTa Pattern Matching Documentation

Comprehensive documentation of MeTTa's pattern matching system, covering theory, implementation, and practical examples.

## Quick Start

**New to pattern matching?** Start here:
1. [00-overview.md](00-overview.md) - Executive summary and introduction
2. [01-fundamentals.md](01-fundamentals.md) - Basic patterns and syntax
3. [03-match-operation.md](03-match-operation.md) - The match operation
4. [examples/01-basic-patterns.metta](examples/01-basic-patterns.metta) - Executable examples

**Looking for specific topics?** See [INDEX.md](INDEX.md) for comprehensive topic index.

## Documentation Structure

### Core Documents

| File | Description | Lines |
|------|-------------|-------|
| [00-overview.md](00-overview.md) | Executive summary of pattern matching system | ~500 |
| [01-fundamentals.md](01-fundamentals.md) | Pattern syntax, variables, and basic matching | ~600 |
| [02-unification.md](02-unification.md) | Unification algorithm and theory | ~550 |
| [03-match-operation.md](03-match-operation.md) | Match operation implementation and usage | ~550 |
| [04-bindings.md](04-bindings.md) | Variable bindings data structure | ~600 |
| [05-pattern-contexts.md](05-pattern-contexts.md) | Different pattern matching contexts | ~500 |
| [06-advanced-patterns.md](06-advanced-patterns.md) | Advanced techniques and patterns | ~550 |
| [07-implementation.md](07-implementation.md) | Implementation details and algorithms | ~650 |
| [08-non-determinism.md](08-non-determinism.md) | Non-deterministic evaluation | ~500 |
| [09-edge-cases.md](09-edge-cases.md) | Edge cases, gotchas, and debugging | ~500 |

**Total**: ~5,500 lines of comprehensive documentation

### Executable Examples

Located in [examples/](examples/) directory:

| File | Topics Covered |
|------|----------------|
| [01-basic-patterns.metta](examples/01-basic-patterns.metta) | Variables, ground terms, simple queries |
| [02-expression-patterns.metta](examples/02-expression-patterns.metta) | Nested structures, complex expressions |
| [03-bindings.metta](examples/03-bindings.metta) | Variable bindings, constraints |
| [04-conjunction-queries.metta](examples/04-conjunction-queries.metta) | Multi-pattern queries, joins |
| [05-non-determinism.metta](examples/05-non-determinism.metta) | Multiple results, non-deterministic evaluation |
| [06-advanced-matching.metta](examples/06-advanced-matching.metta) | Advanced patterns, recursion, inference |
| [07-unify-operation.metta](examples/07-unify-operation.metta) | Unify operation for conditional matching |
| [08-knowledge-queries.metta](examples/08-knowledge-queries.metta) | Knowledge base queries and reasoning |

See [examples/README.md](examples/README.md) for detailed guide to running examples.

### Support Files

- [INDEX.md](INDEX.md) - Comprehensive topic index
- [STATUS.md](STATUS.md) - Documentation completeness status
- [examples/README.md](examples/README.md) - Guide to executable examples

## Key Concepts

### Pattern Matching Fundamentals

**What is pattern matching?**
- Bidirectional unification: variables on both sides
- Structural matching on atoms and expressions
- Variable binding and substitution
- Query-based knowledge retrieval

**Basic pattern types:**
- **Variables**: `$x` - match any value, bind to variable
- **Ground terms**: `Socrates` - match exact symbol
- **Expressions**: `(Human $x)` - match structure with variables

**Example:**
```metta
!(match &self (Human $x) $x)
; Queries space for (Human ...) patterns
; Returns: [Socrates, Plato, Aristotle]
```

### Core Operations

**match** - Query space with pattern:
```metta
(match <space> <pattern> <template>)
```

**unify** - Test atom against pattern:
```metta
(unify <atom> <pattern> <then> <else>)
```

**Conjunction** - Multi-pattern queries:
```metta
(match &self (, (Human $x) (philosopher $x)) $x)
```

### System Components

1. **Unification Engine** (`hyperon-atom/src/matcher.rs`)
   - Bidirectional pattern matching
   - Variable binding
   - Occurs check for cycles

2. **Bindings** (`matcher.rs:140-765`)
   - Variable-to-atom mappings
   - Two-level structure (HashMap + HoleyVec)
   - Transitive resolution

3. **AtomTrie** (`hyperon-space/src/index/trie.rs`)
   - Efficient pattern-based queries
   - Ground prefix optimization
   - O(log n) to O(n) query time

4. **BindingsSet** (`matcher.rs:886-1044`)
   - Multiple solution representation
   - Union and merge operations
   - Non-deterministic results

## Reading Guide

### By Experience Level

**Beginner** - New to MeTTa pattern matching:
1. [00-overview.md](00-overview.md) - Get oriented
2. [01-fundamentals.md](01-fundamentals.md) - Learn basics
3. [examples/01-basic-patterns.metta](examples/01-basic-patterns.metta) - Try examples
4. [03-match-operation.md](03-match-operation.md) - Understand queries
5. [05-pattern-contexts.md](05-pattern-contexts.md) - Learn contexts

**Intermediate** - Familiar with basics:
1. [02-unification.md](02-unification.md) - Understand algorithm
2. [04-bindings.md](04-bindings.md) - Learn data structures
3. [06-advanced-patterns.md](06-advanced-patterns.md) - Advanced techniques
4. [08-non-determinism.md](08-non-determinism.md) - Handle multiple results
5. [examples/04-conjunction-queries.metta](examples/04-conjunction-queries.metta) - Complex queries

**Advanced** - Ready for implementation details:
1. [07-implementation.md](07-implementation.md) - Deep dive implementation
2. [09-edge-cases.md](09-edge-cases.md) - Handle corner cases
3. [examples/06-advanced-matching.metta](examples/06-advanced-matching.metta) - Advanced patterns
4. [examples/08-knowledge-queries.metta](examples/08-knowledge-queries.metta) - Reasoning

### By Use Case

**Writing Queries**:
- [01-fundamentals.md](01-fundamentals.md) - Pattern syntax
- [03-match-operation.md](03-match-operation.md) - Match operation
- [examples/01-basic-patterns.metta](examples/01-basic-patterns.metta) - Query examples
- [examples/04-conjunction-queries.metta](examples/04-conjunction-queries.metta) - Complex queries

**Building Knowledge Bases**:
- [03-match-operation.md](03-match-operation.md) - Querying
- [05-pattern-contexts.md](05-pattern-contexts.md) - Rules and contexts
- [examples/08-knowledge-queries.metta](examples/08-knowledge-queries.metta) - KB examples
- [08-non-determinism.md](08-non-determinism.md) - Multiple results

**Understanding Implementation**:
- [02-unification.md](02-unification.md) - Algorithm
- [04-bindings.md](04-bindings.md) - Data structures
- [07-implementation.md](07-implementation.md) - Full implementation
- Source code: `hyperon-atom/src/matcher.rs`

**Debugging Issues**:
- [09-edge-cases.md](09-edge-cases.md) - Common problems
- [08-non-determinism.md](08-non-determinism.md) - Non-determinism issues
- [04-bindings.md](04-bindings.md) - Binding problems

## Code References

All documentation includes precise code references to the hyperon-experimental repository:

**Primary Implementation Files**:
- `hyperon-atom/src/matcher.rs` - Core unification and bindings
- `hyperon-space/src/index/trie.rs` - Trie-based indexing
- `lib/src/metta/runner/stdlib/core.rs` - Match operation
- `lib/src/space/grounding/mod.rs` - Grounding space

**Referenced Throughout Documentation**:
- File paths: `path/to/file.rs`
- Line numbers: `file.rs:123-456`
- Functions: `function_name()` - `file.rs:line`

## Examples Usage

### Running Examples

```bash
# Navigate to examples directory
cd docs/metta/pattern-matching/examples

# Run with MeTTa interpreter
metta 01-basic-patterns.metta

# Or run specific examples
metta -c "!(match &self (Human \$x) \$x)"
```

### Example Structure

Each example file includes:
- Setup code (knowledge base)
- Multiple examples with explanations
- Expected outputs
- Summary of concepts

See [examples/README.md](examples/README.md) for details.

## Contributing

When contributing to this documentation:

1. **Maintain Consistency**:
   - Follow existing document structure
   - Use same formatting conventions
   - Include code references

2. **Include Examples**:
   - Add executable examples for new concepts
   - Show expected outputs
   - Explain edge cases

3. **Cross-Reference**:
   - Link to related documentation
   - Reference source code with line numbers
   - Update INDEX.md

4. **Verify Accuracy**:
   - Test all code examples
   - Verify against current implementation
   - Note version in document footer

## Version Information

**Documentation Version**: 1.0
**Based on**: hyperon-experimental commit `164c22e9`
**Created**: 2025-11-17
**Hyperon Version**: experimental (main branch)

## Additional Resources

**Related Documentation**:
- [../atom-space/](../atom-space/) - Atom space documentation
- [../type-system/](../type-system/) - Type system documentation
- [../order-of-operations/](../order-of-operations/) - Evaluation order

**External Resources**:
- [hyperon-experimental repository](https://github.com/trueagi-io/hyperon-experimental)
- [MeTTa specification](https://github.com/trueagi-io/hyperon-experimental/tree/main/docs)
- [Example implementations](https://github.com/trueagi-io/hyperon-experimental/tree/main/lib/examples)

## Quick Reference

### Common Patterns

**Simple query**:
```metta
!(match &self (Human $x) $x)
```

**Conjunction**:
```metta
!(match &self (, (Human $x) (philosopher $x)) $x)
```

**Unify**:
```metta
!(unify (Human Socrates) (Human $x) $x "no match")
```

**Nested pattern**:
```metta
!(match &self (person (name $n) (age $a)) ($n is $a))
```

### Common Gotchas

- **Variable scope**: Each expression has separate scope
- **Result order**: Non-deterministic, don't rely on order
- **Empty results**: `[]` is valid, not an error
- **Occurs check**: Prevents `$x ← (f $x)`
- **Shared variables**: Create equality constraints

See [09-edge-cases.md](09-edge-cases.md) for comprehensive list.

## License

This documentation is part of the MeTTaTron compiler project.

---

**Questions?** Check [INDEX.md](INDEX.md) for comprehensive topic coverage or refer to specific documentation files for detailed information.
