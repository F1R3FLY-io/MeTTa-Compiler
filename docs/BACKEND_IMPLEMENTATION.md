# Backend Implementation Summary

## Overview

This document describes the new Rust-based MeTTa evaluation backend implemented in `src/backend/`. This architecture follows the design where `eval` is implemented in Rust rather than as Rholang contracts, with the compiler exposed via the Rholang registry.

## Architecture

```
MeTTa Source → compile() → PathMap [sexprs, fact_db]
                              ↓
                           run() (in Rholang)
                              ↓
                           eval() (Rust) → Results + Updated Environment
```

### Key Components

1. **compile (Rust)**: `src/backend/compile.rs`
   - Parses MeTTa text into s-expressions
   - Converts operators to textual names (`+` → `"add"`, `*` → `"mul"`, etc.)
   - Returns `(Vec<MettaValue>, Environment)` representing parsed s-expressions and fact database
   - Will be exposed via Rholang registry as a function

2. **eval (Rust)**: `src/backend/eval.rs`
   - Lazy evaluation of s-expressions
   - Pattern matching against fact database (rules defined with `(= lhs rhs)`)
   - Direct dispatch to built-in operations (arithmetic, comparison)
   - Returns `(results, new_environment)` tuple

3. **run (Rholang)**: To be implemented by another team
   - Iterates over parsed s-expressions
   - Lines without `!` → add to fact database
   - Lines with `!` → evaluate with `eval`
   - Compositional: accepts environment from previous invocation

## Implementation Details

### MettaValue Type

S-expressions are represented as `MettaValue` enum:

```rust
pub enum MettaValue {
    Atom(String),      // Symbols, variables, operators
    Bool(bool),        // Boolean literals
    Long(i64),         // Integer literals
    String(String),    // String literals
    Uri(String),       // URI literals (not yet supported by lexer)
    SExpr(Vec<MettaValue>), // Nested s-expressions
    Nil,               // Empty/nil
}
```

### Environment

The environment maintains the fact database (pattern matching rules):

```rust
pub struct Environment {
    pub rules: Vec<Rule>,
}

pub struct Rule {
    pub lhs: MettaValue,  // Pattern (left-hand side)
    pub rhs: MettaValue,  // Body (right-hand side)
}
```

Environments are monotonic - they can be unioned together preserving all rules.

### Operator Mapping

Grounded operators are converted to textual names during compilation:

| MeTTa Operator | Textual Name |
|----------------|--------------|
| `+`            | `add`        |
| `-`            | `sub`        |
| `*`            | `mul`        |
| `/`            | `div`        |
| `<`            | `lt`         |
| `<=`           | `lte`        |
| `>`            | `gt`         |
| `==`           | `eq`         |

### Built-in Evaluation

Built-in operations are dispatched directly in Rust without going through the Rholang interpreter:

```rust
eval((add 1 2), env) → (3, env)
eval((lt 1 2), env) → (true, env)
```

### Pattern Matching

Rules are pattern matched during evaluation:

```rust
// Add rule: (= (double $x) (mul $x 2))
env.add_rule(Rule {
    lhs: (double $x),
    rhs: (mul $x 2)
});

// Evaluate
eval((double 5), env) → (10, env)
```

Variables starting with `$`, `&`, or `'` bind to values. The wildcard `_` matches anything.

### Lazy Evaluation

Evaluation is lazy (as opposed to eager):

```rust
eval(atom, env) = (atom, env)

eval((t1 .. tn), env):
  // Evaluate each term
  r1, env_1 = eval(t1, env)
  ...
  rn, env_n = eval(tn, env)

  // Union environments
  env' = union(env_1, ..., env_n)

  // Try built-in operations
  if (r1 .. rn) matches built-in:
    return built-in result, env'

  // Try pattern matching rules
  for each rule in env':
    if (r1 .. rn) matches rule.lhs with bindings:
      return eval(rule.rhs with bindings), env'

  // No match, return evaluated expression
  return (r1 .. rn), env'
```

## Testing

All core functionality has been tested:

- `test_compile_simple` - Basic compilation
- `test_compile_operators` - Operator name conversion
- `test_compile_literals` - Literal parsing (bool, int, string)
- `test_eval_atom` - Atom evaluation
- `test_eval_builtin_add` - Arithmetic operations
- `test_eval_builtin_comparison` - Comparison operations
- `test_pattern_match_simple` - Variable binding
- `test_pattern_match_sexpr` - Complex pattern matching
- `test_eval_with_rule` - Rule-based evaluation

Run tests:
```bash
cargo test backend::
```

## Known Limitations

1. **URI Literals**: Backtick-quoted URIs (`` `uri` ``) are not yet supported by the lexer in `src/sexpr.rs`

2. **>= Operator**: The `>=` operator is tokenized as two separate tokens (`>` and `=`) by the current lexer. Would need lexer updates to support.

3. **PathMap Integration**: The current implementation uses a simple `Vec<Rule>` for the fact database. Integration with the PathMap trie will be handled by another team. The interface assumes the ability to query PathMap with a Rholang AST and get back a list of matching Rholang ASTs.

4. **Rholang Interpreter Integration**: The `to_proc_expr` function (conversion from MettaValue to Rholang AST) is a placeholder. Full integration requires adding dependencies:
   - `pathmap` from https://github.com/adam-Vandervorst/PathMap/
   - `rholang-rs` from https://github.com/F1R3FLY-io/rholang-rs
   - `f1r3node` interpreter from https://github.com/F1R3FLY-io/f1r3node

## Next Steps

1. **Add Dependencies**: Integrate PathMap, rholang-rs, and f1r3node interpreter as Cargo dependencies

2. **Implement toProcExpr**: Convert `MettaValue` to Rholang `Proc` AST type for PathMap storage

3. **Rholang Registry Integration**: Expose `compile` function via Rholang registry

4. **PathMap Integration**: Replace `Vec<Rule>` with PathMap-based fact database

5. **Implement run Method**: Create the `run` method on PathMap in Rholang that calls the Rust `eval` function

## File Structure

```
src/backend/
├── mod.rs          # Module exports
├── types.rs        # MettaValue, Environment, Rule types
├── compile.rs      # compile() function
└── eval.rs         # eval() function with pattern matching
```

## Usage Example

```rust
use mettatron::backend::*;

// Compile MeTTa source
let src = "(= (fib 0) 1) (= (fib 1) 1) (fib 5)";
let (sexprs, env) = compile(src)?;

// Create fact database from rules
let mut env = Environment::new();
// ... add rules from sexprs that start with =

// Evaluate an expression
let expr = MettaValue::SExpr(vec![
    MettaValue::Atom("fib".to_string()),
    MettaValue::Long(5),
]);
let (results, new_env) = eval(expr, env);
```

## References

- MORK Pattern Matcher: https://github.com/trueagi-io/MORK
- MeTTa Language: https://metta-lang.dev/
- Hyperon Implementation: https://github.com/trueagi-io/hyperon-experimental
- PathMap: https://github.com/adam-Vandervorst/PathMap/
- F1R3 Node: https://github.com/F1R3FLY-io/f1r3node
