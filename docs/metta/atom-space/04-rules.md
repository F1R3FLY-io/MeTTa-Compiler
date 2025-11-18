# Rules in MeTTa Atom Space

## Overview

In MeTTa, **rules** are atoms that define rewrites, computations, or transformations. Rules use the `=` operator to specify pattern-result mappings that the interpreter uses during evaluation. This document provides comprehensive details about rules: their syntax, storage, and how they interact with the atom space.

## What is a Rule?

### Specification

**Definition**: A rule is an atom using the `=` operator that defines a rewrite from a pattern to a result.

**Syntax:**
```metta
(= <pattern> <result>)
```

**Formal Definition:**
```
Rule := (= Pattern Result)
Pattern := Atom with possible variables
Result := Atom (may reference pattern variables)
```

**Semantics**: During evaluation, if an expression matches `Pattern`, it can be rewritten to `Result` with variable bindings applied.

### Examples

**Simple Rules:**
```metta
; Constant function
(= (always-true) True)

; Identity function
(= (id $x) $x)

; Negation
(= (not True) False)
(= (not False) True)
```

**Recursive Rules:**
```metta
; Factorial
(= (fac 0) 1)
(= (fac $n) (* $n (fac (- $n 1))))

; Fibonacci
(= (fib 0) 0)
(= (fib 1) 1)
(= (fib $n) (+ (fib (- $n 1)) (fib (- $n 2))))
```

**Pattern Matching Rules:**
```metta
; List operations
(= (head ($h $t...)) $h)
(= (tail ($h $t...)) $t)

; Conditional
(= (if True $then $else) $then)
(= (if False $then $else) $else)
```

**Complex Rules:**
```metta
; Logical inference
(= (mortal $x) (Human $x))
(= (can-die $x) (mortal $x))

; Type conversion
(= (to-string $n) (str $n))
```

## Rules vs Facts

### Conceptual Distinction

**Rules:**
- Define computations or transformations
- Use `=` operator (required)
- Evaluated during reduction
- Pattern on left, result on right
- Examples: `(= (f $x) (* $x 2))`

**Facts:**
- Represent static data
- No `=` operator
- Stored as-is, used in matching
- No left/right distinction
- Examples: `(Human Socrates)`, `42`

### Storage: No Distinction

**Critical**: At the storage level, rules are atoms just like facts.

```rust
// Both stored identically in the trie
space.add(expr![sym!("Human"), sym!("Socrates")]);        // Fact
space.add(expr![sym!("="), pattern, result]);             // Rule
```

**Storage Characteristics:**
- Rules tokenized like expressions: `[OpenParen, Symbol("="), ..., CloseParen]`
- Indexed in trie with `=` as first token after open paren
- No special storage mechanism for rules
- Retrievable via `get-atoms` or `match` like any atom

### Distinction Emerges During Evaluation

**Interpreter Behavior:**

1. **Expression Evaluation:**
   ```metta
   (f 10)  ; Evaluates to find matching rules
   ```

2. **Rule Search:**
   - Interpreter searches space for `(= (f $x) ...)`
   - Pattern matches against `(f 10)`
   - Binds `$x` to `10`

3. **Result Substitution:**
   - Takes result part of rule
   - Applies variable bindings
   - Evaluates result

**Example:**
```metta
; Rule in space
(add-atom &self (= (double $x) (* $x 2)))

; Evaluation
!(double 5)
; 1. Searches for (= (double $x) ...)
; 2. Finds rule, binds $x=5
; 3. Substitutes: (* 5 2)
; 4. Evaluates to 10
; → 10
```

**Location**: `lib/src/metta/interpreter.rs:250-400` (rule matching and application)

## Rule Syntax

### Basic Structure

**Form:**
```metta
(= <pattern> <result>)
```

**Components:**

1. **Operator**: `=` (equals sign)
2. **Pattern**: Left side, may contain variables
3. **Result**: Right side, may reference pattern variables

**Type:**
```
= : Pattern → Result → Rule
```

### Pattern Syntax

**Ground Terms (no variables):**
```metta
(= (pi) 3.14159)
(= (true-const) True)
```

**Variable Patterns:**
```metta
(= (id $x) $x)              ; Single variable
(= (add $x $y) (+ $x $y))   ; Multiple variables
```

**Nested Patterns:**
```metta
(= (process (data $x)) (transform $x))
(= (extract (wrapper $inner)) $inner)
```

**Variadic Patterns (with $...):**
```metta
(= (first ($h $t...)) $h)
(= (rest ($h $t...)) $t)
```

**Constraint Patterns (conditional rules):**
```metta
; Implemented via guards in result
(= (pos $x) (if (> $x 0) $x 0))
```

### Result Syntax

**Literal Results:**
```metta
(= (always-42) 42)
(= (get-true) True)
```

**Variable Results:**
```metta
(= (id $x) $x)
(= (swap $x $y) ($y $x))
```

**Computed Results:**
```metta
(= (double $x) (* $x 2))
(= (area $w $h) (* $w $h))
```

**Recursive Results:**
```metta
(= (fac 0) 1)
(= (fac $n) (* $n (fac (- $n 1))))
```

**Conditional Results:**
```metta
(= (abs $x) (if (< $x 0) (- $x) $x))
```

## Adding Rules

### Using add-atom

**Syntax:**
```metta
(add-atom <space> (= <pattern> <result>))
```

**Examples:**
```metta
; Add simple rule
(add-atom &self (= (double $x) (* $x 2)))

; Add recursive rule
(add-atom &self (= (fac 0) 1))
(add-atom &self (= (fac $n) (* $n (fac (- $n 1)))))

; Add pattern matching rule
(add-atom &self (= (head ($h $t...)) $h))
```

### Bulk Rule Loading

**Pattern:**
```metta
; Define multiple rules at once
(add-atom &self (= (double $x) (* $x 2)))
(add-atom &self (= (triple $x) (* $x 3)))
(add-atom &self (= (quad $x) (* $x 4)))
```

**From List:**
```metta
!(bind! &rules
    ((= (f1 $x) (* $x 2))
     (= (f2 $x) (* $x 3))
     (= (f3 $x) (* $x 4))))

!(match &rules $rule
    (add-atom &self $rule))
```

## Rule Evaluation

### Matching Process

**Steps:**

1. **Expression to Reduce:**
   ```metta
   (f arg1 arg2)
   ```

2. **Search for Matching Rules:**
   ```metta
   !(match &self (= (f $x1 $x2) $result) ...)
   ```

3. **Pattern Matching:**
   - Compare expression `(f arg1 arg2)` with pattern `(f $x1 $x2)`
   - Bind variables: `$x1 ← arg1`, `$x2 ← arg2`

4. **Substitute Bindings:**
   - Take `$result` from rule
   - Replace `$x1` with `arg1`, `$x2` with `arg2`

5. **Evaluate Result:**
   - Recursively evaluate substituted result
   - Return final value

**Implementation**: `lib/src/metta/interpreter.rs:300-350`

### Multiple Matching Rules

**Non-Determinism:**
If multiple rules match, MeTTa may try all of them (non-deterministic semantics).

**Example:**
```metta
; Two rules for same pattern
(add-atom &self (= (choice $x) (option1 $x)))
(add-atom &self (= (choice $x) (option2 $x)))

!(choice 42)
; May return: (option1 42) or (option2 42)
; Or both in different branches
```

**Rule Selection:**
- No guaranteed order
- May depend on trie traversal order
- All matches may be explored

### Rule Precedence

**No Built-in Precedence:**
- MeTTa doesn't enforce rule priority
- More specific rules don't automatically override general ones
- Programmer must manage rule conflicts

**Pattern:**
```metta
; General rule
(= (process $x) (generic-handler $x))

; Specific rule (no automatic precedence!)
(= (process special-case) (special-handler))

; Both may match (process special-case)
```

**Workaround: Use Guards:**
```metta
(= (process special-case) (special-handler))
(= (process $x)
    (if (== $x special-case)
        (special-handler)
        (generic-handler $x)))
```

## Removing Rules

### Using remove-atom

**Syntax:**
```metta
(remove-atom <space> (= <exact-pattern> <exact-result>))
```

**Important**: Must match the rule exactly (not just the pattern part).

**Examples:**
```metta
; Add rule
(add-atom &self (= (double $x) (* $x 2)))

; Remove rule (must match exactly)
(remove-atom &self (= (double $x) (* $x 2)))  ; → True

; This won't match (different variable name)
(remove-atom &self (= (double $y) (* $y 2)))  ; → False

; This won't match (different result)
(remove-atom &self (= (double $x) (+ $x $x))) ; → False
```

### Pattern-Based Rule Removal

**Find and Remove:**
```metta
; Remove all rules defining 'double'
!(match &self (= (double $x) $result)
    (remove-atom &self (= (double $x) $result)))
```

**Careful**: May not work as expected if rules modified during iteration.

## Querying Rules

### Using match

**Find Rules by Pattern:**
```metta
; Find all rules defining 'double'
!(match &self (= (double $x) $result) $result)
; → [(* $x 2)]  (list of result parts)

; Find all rules (any pattern/result)
!(match &self (= $pattern $result) ($pattern $result))
```

**Extract Rule Components:**
```metta
; Get all function names defined by rules
!(match &self (= ($fname $args...) $result) $fname)

; Get all recursive rules (rules that reference themselves in result)
!(match &self (= ($fname $args...) $result)
    (if (contains? $result $fname)
        $fname
        ()))
```

### Using get-atoms

**Retrieve All Atoms (including rules):**
```metta
!(get-atoms &self)
; → [all atoms, including (= ...) rules]
```

**Filter Rules:**
```metta
; Manual filtering (pseudo-code)
!(match (get-atoms &self) $atom
    (if (is-rule? $atom)
        $atom
        ()))
```

## Rule Patterns

### Base Case + Recursive Case

**Pattern:**
```metta
(= (func base-pattern) base-result)
(= (func recursive-pattern) recursive-result-with-recursive-call)
```

**Examples:**
```metta
; Factorial
(= (fac 0) 1)
(= (fac $n) (* $n (fac (- $n 1))))

; List length
(= (len ()) 0)
(= (len ($h $t...)) (+ 1 (len $t)))
```

### Multiple Clauses

**Pattern:**
```metta
(= (func pattern1) result1)
(= (func pattern2) result2)
(= (func pattern3) result3)
```

**Examples:**
```metta
; Boolean operations
(= (and True True) True)
(= (and True False) False)
(= (and False True) False)
(= (and False False) False)

; Pattern-specific behavior
(= (handle empty-input) default-value)
(= (handle (single $x)) (process-one $x))
(= (handle (multiple $xs...)) (process-many $xs))
```

### Conditional Rules

**Pattern:**
```metta
(= (func $args...)
    (if <condition>
        <then-result>
        <else-result>))
```

**Examples:**
```metta
; Absolute value
(= (abs $x) (if (< $x 0) (- $x) $x))

; Max of two numbers
(= (max $x $y) (if (> $x $y) $x $y))

; Guarded recursion
(= (countdown $n)
    (if (> $n 0)
        (countdown (- $n 1))
        done))
```

### Transformation Rules

**Pattern:**
```metta
(= (transform input-pattern) output-pattern)
```

**Examples:**
```metta
; List transformations
(= (reverse ()) ())
(= (reverse ($h $t...)) (append (reverse $t) ($h)))

; Data structure conversions
(= (to-list (set $elems...)) $elems)
(= (to-set ($elems...)) (set $elems))
```

### Logical Inference Rules

**Pattern:**
```metta
(= (conclusion $x) (premise $x))
```

**Examples:**
```metta
; Syllogism
(= (mortal $x) (Human $x))
(= (can-die $x) (mortal $x))

; Transitive relations
(= (ancestor $x $y)
    (match &self (parent $x $y) True))
(= (ancestor $x $z)
    (match &self (parent $x $y)
        (ancestor $y $z)))
```

## Rule Scope

### Space-Specific Rules

**Rules are local to their space:**
```metta
; Rules in &self
(add-atom &self (= (f $x) (* $x 2)))

; Create new space
!(bind! &other (new-space))

; f not defined in &other
!(match &other (= (f $x) $result) $result)
; → []  (no matches)
```

**Cross-Space References:**
```metta
; Rule in &self can reference other spaces
(add-atom &self (= (lookup $key)
                    (match &database ($key $value) $value)))
```

### Module-Level Rules

**Default Space:**
- `&self` refers to current module's space
- Rules added to `&self` available within module

**Example:**
```metta
; In module file1.metta
(add-atom &self (= (module1-func $x) (* $x 2)))

; In module file2.metta
; Cannot directly access file1's rules unless explicitly imported
```

## Implementation Details

### Storage as Atoms

**Rules Stored as Expressions:**
```rust
// Rule: (= (double $x) (* $x 2))
// Stored as:
Atom::Expression(vec![
    Atom::Symbol(Symbol::new("=")),
    Atom::Expression(vec![
        Atom::Symbol(Symbol::new("double")),
        Atom::Variable(Variable::new("x")),
    ]),
    Atom::Expression(vec![
        Atom::Symbol(Symbol::new("*")),
        Atom::Variable(Variable::new("x")),
        Atom::Number(Number::Integer(2)),
    ]),
])
```

**Trie Indexing:**
```
Trie path for (= (double $x) (* $x 2)):
  root
    → OpenParen
      → Symbol("=")
        → OpenParen
          → Symbol("double")
            → Variable("x")
              → CloseParen
                → ... (result part)
                  → Leaf: [rule atom]
```

### Rule Application (Interpreter)

**Location**: `lib/src/metta/interpreter.rs:250-450`

**Simplified Algorithm:**
```rust
fn evaluate(space: &Space, expr: &Atom) -> Vec<Atom> {
    // 1. Find matching rules
    let pattern = expr!["=", expr.clone(), var!("result")];
    let matches = space.query(&pattern);

    // 2. For each matching rule
    let mut results = vec![];
    for rule in matches {
        // 3. Extract pattern and result from rule
        let (pattern, result) = extract_rule_parts(rule);

        // 4. Match expression against pattern
        if let Some(bindings) = match_pattern(expr, &pattern) {
            // 5. Substitute bindings into result
            let substituted = apply_bindings(&result, &bindings);

            // 6. Recursively evaluate result
            let evaluated = evaluate(space, &substituted);
            results.extend(evaluated);
        }
    }

    // 7. Return all results (may be multiple due to non-determinism)
    results
}
```

**Key Functions:**
- `match_pattern()` - Binds variables from pattern
- `apply_bindings()` - Substitutes variables in result
- Non-deterministic: may return multiple results

## Performance Considerations

### Rule Query Overhead

**Pattern Matching Cost:**
- Every function call requires rule search
- Trie indexing helps but still O(k + m)
  - k = pattern tokens
  - m = matching rules

**Optimization:**
- Ground patterns (like `(f ...)`) narrow search
- Variables in pattern require broader search

### Rule Count Impact

**Many Rules:**
- More rules in space = more candidates to check
- Each match attempt has cost

**Mitigation:**
- Use specific patterns (avoid overly general rules)
- Organize rules in separate spaces if possible

### Recursive Rules

**Stack Depth:**
- Deep recursion may hit stack limits
- MeTTa uses plan-based evaluation (LIFO queue)
- Very deep recursion may be slow or fail

**Example:**
```metta
; Deep recursion:
(= (countdown 10000) ...)  ; May hit limits
```

## Common Pitfalls

### 1. Variable Name Mismatch

**Problem:**
```metta
(add-atom &self (= (f $x) (* $x 2)))

; Trying to remove with different variable name
(remove-atom &self (= (f $y) (* $y 2)))  ; → False (doesn't match!)
```

**Solution**: Use exact same variable names.

### 2. Non-Deterministic Rule Conflicts

**Problem:**
```metta
(add-atom &self (= (f $x) (result1 $x)))
(add-atom &self (= (f $x) (result2 $x)))

!(f 10)  ; Which result?
```

**Solution**: Design rules to be non-overlapping or use guards.

### 3. Infinite Recursion

**Problem:**
```metta
(add-atom &self (= (loop $x) (loop $x)))

!(loop 1)  ; Infinite loop!
```

**Solution**: Ensure base cases and decreasing measures.

### 4. Shadowing Built-ins

**Problem:**
```metta
; Redefining built-in functions
(add-atom &self (= (+ $x $y) (my-plus $x $y)))

!(+ 1 2)  ; May use your rule instead of built-in!
```

**Solution**: Avoid redefining built-in operators.

### 5. Order Dependency

**Problem:**
```metta
; Assuming rule order matters
(add-atom &self (= (f special) special-result))
(add-atom &self (= (f $x) general-result))

!(f special)  ; No guaranteed preference!
```

**Solution**: Use guards or ensure non-overlapping patterns.

## Best Practices

### 1. Name Rules Clearly

```metta
; Good
(= (factorial $n) ...)
(= (is-even $n) ...)

; Avoid
(= (f $x) ...)
(= (g $x $y) ...)
```

### 2. Document Rules

```metta
; Factorial function: computes n!
; Base case: 0! = 1
(= (factorial 0) 1)
; Recursive case: n! = n * (n-1)!
(= (factorial $n) (* $n (factorial (- $n 1))))
```

### 3. Ensure Termination

```metta
; Good: has base case
(= (countdown 0) done)
(= (countdown $n) (countdown (- $n 1)))

; Bad: no base case
; (= (infinite-loop $n) (infinite-loop $n))
```

### 4. Use Guards for Constraints

```metta
; Factorial with guard
(= (factorial $n)
    (if (== $n 0)
        1
        (* $n (factorial (- $n 1)))))
```

### 5. Test Rules Incrementally

```metta
; Add rules one at a time
(add-atom &self (= (fac 0) 1))
!(fac 0)  ; Test base case → 1

(add-atom &self (= (fac $n) (* $n (fac (- $n 1)))))
!(fac 3)  ; Test recursive case → 6
```

### 6. Avoid Redundant Rules

```metta
; Redundant
(= (is-zero 0) True)
(= (is-zero $x) (if (== $x 0) True False))

; Better
(= (is-zero $x) (== $x 0))
```

## Integration with Type System

### Type-Annotated Rules

**Rules can have type annotations:**
```metta
; Type annotation
(: double (-> Number Number))

; Rule definition
(= (double $x) (* $x 2))
```

**Type Checking:**
- With `pragma! type-check auto`, rules are type-checked
- Pattern variables must have consistent types
- Result must match declared return type

**Example:**
```metta
!(pragma! type-check auto)

(: safe-div (-> Number Number Number))
(= (safe-div $x 0) error)
(= (safe-div $x $y) (/ $x $y))

!(safe-div 10 2)  ; → 5 (type-safe)
```

### Type as Constraints

**Use types to constrain rules:**
```metta
(: process-nat (-> Nat String))
(= (process-nat $n) (to-string $n))

; Type system ensures $n is Nat
```

## Related Documentation

- **[Facts](03-facts.md)** - How facts differ from rules
- **[Adding Atoms](01-adding-atoms.md)** - Detailed add-atom behavior
- **[Removing Atoms](02-removing-atoms.md)** - Detailed remove-atom behavior
- **[Space Operations](05-space-operations.md)** - All space operations
- **Evaluation** - `../order-of-operations/01-evaluation-order.md`
- **Type System** - `../type-system/02-type-checking.md`

## Examples

See **[examples/02-rules.metta](examples/02-rules.metta)** for executable examples of:
- Simple and recursive rules
- Pattern matching rules
- Conditional rules
- Multiple matching rules
- Rule queries and removal

## Summary

**Rules in MeTTa:**
✅ Define rewrites/computations using `=` operator
✅ Stored as atoms (no special storage)
✅ Pattern on left, result on right
✅ Evaluated by interpreter during reduction
✅ Support recursion, patterns, conditionals

**Key Points:**
- Rules are conceptually distinct but stored like facts
- Interpreter searches for `(= pattern result)` during evaluation
- Multiple matching rules → non-deterministic evaluation
- No built-in precedence or rule ordering
- Must use exact equality for removal (not pattern matching)

**Rule Structure:**
```metta
(= <pattern> <result>)
   ↓           ↓
   Matches     Produces
   input       output
```

**Best Practices:**
- Clear naming and documentation
- Ensure termination (base cases)
- Use guards for constraints
- Test incrementally
- Avoid rule conflicts
- Leverage type annotations

---

**Version**: 1.0
**Based on**: hyperon-experimental commit `164c22e9`
**Created**: 2025-11-13
