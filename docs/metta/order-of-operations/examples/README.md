# MeTTa Order of Operations Examples

This directory contains executable MeTTa example files demonstrating various aspects of MeTTa's order of operations and evaluation semantics.

## Example Files

### 01-normal-order.metta
**Topic**: Normal Order Evaluation

Demonstrates that MeTTa uses normal order evaluation (call-by-name) where arguments are NOT evaluated before being passed to functions.

**Key Concepts**:
- Arguments passed unevaluated
- Lazy evaluation avoids unnecessary computation
- Comparison with eager evaluation

**Run**:
```bash
metta 01-normal-order.metta
```

### 02-non-determinism.metta
**Topic**: Non-Deterministic Evaluation

Demonstrates MeTTa's non-deterministic evaluation where multiple matching rules create multiple result alternatives.

**Key Concepts**:
- Multiple reduction rules
- All alternatives explored
- Cartesian products of alternatives
- Superpose and collapse operations

**Run**:
```bash
metta 02-non-determinism.metta
```

### 03-pattern-matching.metta
**Topic**: Pattern Matching Order

Demonstrates pattern matching, queries, and unification in MeTTa.

**Key Concepts**:
- Pattern matching against atom space
- Query ordering (implementation-dependent)
- Unification with variables
- Nested patterns

**Run**:
```bash
metta 03-pattern-matching.metta
```

**Note**: Requires space operations to be available.

### 04-mutations.metta
**Topic**: Mutation Order and Side Effects

Demonstrates atom space mutations and their ordering semantics.

**Key Concepts**:
- Sequential mutations within a branch
- Non-deterministic mutations across branches
- add-atom, remove-atom, replace-atom operations
- Non-confluent behavior with conditionals
- RefCell borrow conflicts

**Run**:
```bash
metta 04-mutations.metta
```

**Warning**: Some examples demonstrate error conditions (RefCell panics).

### 05-reduction-order.metta
**Topic**: Reduction Order

Demonstrates how reduction rules are applied and the order in which they execute.

**Key Concepts**:
- Single reduction path
- Multiple matching rules
- Recursive reductions
- Overlapping patterns
- Rule definition order irrelevance

**Run**:
```bash
metta 05-reduction-order.metta
```

### 06-chain-control.metta
**Topic**: Chain and Evaluation Control

Demonstrates using the `chain` operation to control evaluation order and force evaluation of arguments.

**Key Concepts**:
- Forcing evaluation with chain
- Sequential evaluation
- Chain with non-determinism
- Lazy vs eager evaluation
- Avoiding unnecessary computation

**Run**:
```bash
metta 06-chain-control.metta
```

## Running the Examples

### Prerequisites

- MeTTa interpreter (from hyperon-experimental)
- Python 3.8+ (if using Python bindings)

### Using the Command Line

```bash
# Run with metta executable
metta examples/01-normal-order.metta

# Or with Python
python3 -m hyperon.runner examples/01-normal-order.metta
```

### Using Python REPL

```python
from hyperon import MeTTa

metta = MeTTa()
metta.run(open('examples/01-normal-order.metta').read())
```

## Understanding the Output

### Non-Deterministic Results

When an example produces multiple results, they are shown as a set:
```metta
!(color)
; Output: [red, green, blue]
```

The order of results is **not specified** and may vary between runs or implementations.

### Side Effects

Examples involving `add-atom`, `remove-atom`, etc. have side effects:
```metta
!(add-atom &space A)
; Output: ()
; Side effect: A is added to &space
```

### Error Conditions

Some examples demonstrate error conditions:
```metta
; This would cause a RefCell panic:
; !(match &space (foo $x) (add-atom &space (new $x)))
```

These are commented out to prevent crashes but illustrate important limitations.

## Expected Behavior

### Confluence

**Pure Examples** (no side effects):
- Results should be the same regardless of evaluation order
- Only the **set** of results matters, not the order

**Impure Examples** (with side effects):
- Results may differ based on evaluation order
- Final atom space state may vary
- Demonstrates non-confluent behavior

### Implementation Variations

Different MeTTa implementations may:
- Process alternatives in different orders (LIFO, FIFO, etc.)
- Return results in different orders
- Have different performance characteristics
- Handle errors differently

However, all implementations should:
- Return the **same set** of results (for pure programs)
- Explore **all alternatives** (not just first match)
- Follow **normal order** evaluation (for minimal MeTTa)

## Modifying the Examples

Feel free to modify these examples to experiment with different behaviors:

1. **Change Rules**: Add or remove reduction rules to see how alternatives change
2. **Add Logging**: Insert print statements to trace evaluation
3. **Measure Performance**: Time different evaluation strategies
4. **Test Limits**: Try deeply nested or highly branching computations

## Troubleshooting

### Import Errors

If you get import errors, ensure hyperon-experimental is installed:
```bash
cd hyperon-experimental
pip install -e ./python
```

### RefCell Panics

If you see "already borrowed" errors:
- You're attempting concurrent mutation during query
- See `04-mutations.metta` for safe patterns

### Stack Overflow

If you see stack overflow errors:
- Increase stack depth limit
- Or rewrite recursive functions iteratively

## Further Reading

See the main documentation files:
- `../00-overview.md` - Executive summary
- `../01-evaluation-order.md` - Detailed evaluation semantics
- `../02-mutation-order.md` - Mutation ordering
- `../03-pattern-matching.md` - Pattern matching
- `../04-reduction-order.md` - Reduction rules
- `../05-non-determinism.md` - Non-deterministic semantics

## Contributing

To add new examples:
1. Create a new `.metta` file in this directory
2. Add clear comments explaining the behavior
3. Include expected results
4. Update this README with a description
5. Test with hyperon-experimental

---

**Last Updated**: 2025-11-13
**Compatible with**: hyperon-experimental commit `164c22e9`
