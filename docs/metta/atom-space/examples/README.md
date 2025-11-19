# MeTTa Atom Space Examples

This directory contains executable examples demonstrating MeTTa's atom space features and operations.

## Examples

1. **[01-basic-operations.metta](01-basic-operations.metta)** - Basic atom space operations
   - Adding atoms with `add-atom`
   - Removing atoms with `remove-atom`
   - Querying with `get-atoms` and `match`
   - Handling duplicates
   - **Difficulty**: Beginner

2. **[02-facts.metta](02-facts.metta)** - Working with facts
   - Simple facts (properties, relations)
   - Entity-Attribute-Value (EAV) pattern
   - Structured and nested facts
   - Conditional queries
   - Bulk operations
   - **Difficulty**: Beginner

3. **[03-rules.metta](03-rules.metta)** - Working with rules
   - Simple and recursive rules
   - Pattern matching rules
   - Conditional rules
   - Logical inference
   - Multiple matching rules
   - Querying and removing rules
   - **Difficulty**: Intermediate

4. **[04-multiple-spaces.metta](04-multiple-spaces.metta)** - Multiple atom spaces
   - Creating independent spaces with `new-space`
   - Space isolation
   - Copying and merging spaces
   - Set operations (difference, intersection)
   - Versioning and partitioning patterns
   - **Difficulty**: Intermediate

5. **[05-edge-cases.metta](05-edge-cases.metta)** - Edge cases and gotchas
   - Empty expressions and spaces
   - Duplicate handling
   - Variable name sensitivity
   - Concurrent modification issues
   - Non-atomic updates
   - **Difficulty**: Advanced

6. **[06-pattern-matching.metta](06-pattern-matching.metta)** - Advanced pattern matching
   - Multi-variable patterns
   - Nested patterns
   - Conditional matching
   - Relationship traversal
   - Existential queries
   - Collection and transformation
   - **Difficulty**: Intermediate

7. **[07-knowledge-base.metta](07-knowledge-base.metta)** - Knowledge base example
   - Building a structured knowledge base
   - Facts, relationships, and attributes
   - Inference rules
   - Multi-hop reasoning
   - Aggregate and temporal queries
   - Knowledge base updates
   - **Difficulty**: Advanced

## Running Examples

### Using MeTTa Executable

```bash
metta 01-basic-operations.metta
```

### Using Python Runner

```bash
python3 -m hyperon.runner 01-basic-operations.metta
```

### Using Rust

```bash
cd /path/to/hyperon-experimental
cargo run --bin metta -- /path/to/example.metta
```

## Prerequisites

- **hyperon-experimental** installed (commit `164c22e9` or later)
- Python 3.8+ (for Python runner)
- Rust 1.70+ (for building from source)

## Learning Path

### Quick Start (30 minutes)
1. Run **01-basic-operations.metta**
2. Run **02-facts.metta**
3. Experiment with modifying examples

### Comprehensive (2 hours)
1. **01-basic-operations.metta** - Learn basics
2. **02-facts.metta** - Understand facts
3. **03-rules.metta** - Learn rules
4. **04-multiple-spaces.metta** - Master spaces
5. **06-pattern-matching.metta** - Advanced queries

### Advanced (Full study)
1. Complete all examples in order
2. Study **05-edge-cases.metta** carefully
3. Build **07-knowledge-base.metta**
4. Create your own knowledge base

## Example Structure

Each example follows this pattern:

```metta
; ========================================
; Section Title
; ========================================

!(println "=== Section Title ===")

; Add atoms
(add-atom &self (data ...))

; Query atoms
!(match &self (pattern $var) result)
; Expected: [...]

!(println "Description of what happened")
```

**Common Patterns:**
- Clear section markers
- Descriptive print statements
- Expected output in comments
- Progressive complexity

## Tips

1. **Read Comments**: Each example is heavily commented
2. **Check Expected Output**: Compare your results with comments
3. **Experiment**: Modify examples to test understanding
4. **Sequential Learning**: Examples build on each other
5. **Error Messages**: Study any errors carefully

## Troubleshooting

**Example doesn't run:**
- Ensure hyperon-experimental is installed
- Check you're in correct directory
- Verify file path is correct

**Unexpected output:**
- Read comments for expected behavior
- Some results are non-deterministic (order may vary)
- Implementation may have changed since examples written

**Errors about missing operations:**
- Update to latest hyperon-experimental
- Check operation names match current implementation
- Refer to main documentation for current syntax

## Integration with Documentation

These examples demonstrate concepts from:
- **[00-overview.md](../00-overview.md)** - Executive summary
- **[01-adding-atoms.md](../01-adding-atoms.md)** - add-atom operation
- **[02-removing-atoms.md](../02-removing-atoms.md)** - remove-atom operation
- **[03-facts.md](../03-facts.md)** - Working with facts
- **[04-rules.md](../04-rules.md)** - Working with rules
- **[05-space-operations.md](../05-space-operations.md)** - All operations
- **[06-space-structure.md](../06-space-structure.md)** - Internal implementation
- **[07-edge-cases.md](../07-edge-cases.md)** - Edge cases

## Contributing

To add new examples:
1. Follow existing naming convention (`NN-description.metta`)
2. Include clear comments and expected output
3. Test with current hyperon-experimental
4. Update this README

## See Also

- **hyperon-experimental tests**: `python/tests/scripts/*.metta`
- **Main documentation**: `../README.md`
- **Atom space overview**: `../00-overview.md`

---

**Version**: 1.0
**Based on**: hyperon-experimental commit `164c22e9`
**Created**: 2025-11-13
