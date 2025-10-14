# Backend API Reference

## Quick Start

```rust
use mettatron::backend::*;

// 1. Compile MeTTa source to s-expressions
let (sexprs, env) = compile("(+ 1 2)")?;

// 2. Evaluate s-expressions
let (results, new_env) = eval(sexprs[0].clone(), env);

println!("{:?}", results); // [Long(3)]
```

## API Overview

### `compile(src: &str) -> Result<(Vec<MettaValue>, Environment), String>`

Parses MeTTa source code into s-expressions.

**Input**: MeTTa source code as a string
**Output**: `(parsed_sexprs, environment)`

**Features**:
- Converts operators to textual names: `+` → `"add"`, `*` → `"mul"`, etc.
- Parses literals: booleans, integers, strings
- Handles nested s-expressions

**Example**:
```rust
let (sexprs, env) = compile("(+ 10 5)").unwrap();
// sexprs = [SExpr([Atom("add"), Long(10), Long(5)])]
```

### `eval(value: MettaValue, env: Environment) -> (Vec<MettaValue>, Environment)`

Evaluates a MettaValue s-expression with lazy evaluation.

**Input**: `MettaValue` to evaluate and an `Environment`
**Output**: `(results, updated_environment)`

**Features**:
- Lazy evaluation (evaluate on demand)
- Pattern matching against rules in environment
- Direct dispatch to built-in operations
- Compositional environment updates

**Example**:
```rust
let expr = MettaValue::SExpr(vec![
    MettaValue::Atom("add".to_string()),
    MettaValue::Long(1),
    MettaValue::Long(2),
]);
let (results, new_env) = eval(expr, Environment::new());
// results = [Long(3)]
```

## Core Types

### `MettaValue`

Represents MeTTa values as s-expressions:

```rust
pub enum MettaValue {
    Atom(String),           // Symbols, variables, operators
    Bool(bool),             // true, false
    Long(i64),              // Integer literals
    String(String),         // "string literals"
    Uri(String),            // URI literals (not yet supported by lexer)
    SExpr(Vec<MettaValue>), // Nested s-expressions
    Nil,                    // Empty/nil
}
```

**Examples**:
```rust
// Atoms
MettaValue::Atom("add".to_string())
MettaValue::Atom("$x".to_string())  // Variable

// Literals
MettaValue::Bool(true)
MettaValue::Long(42)
MettaValue::String("hello".to_string())

// S-expressions
MettaValue::SExpr(vec![
    MettaValue::Atom("add".to_string()),
    MettaValue::Long(1),
    MettaValue::Long(2),
])
```

### `Environment`

Maintains the fact database (pattern matching rules):

```rust
pub struct Environment {
    pub rules: Vec<Rule>,
}

impl Environment {
    pub fn new() -> Self;
    pub fn add_rule(&mut self, rule: Rule);
    pub fn union(&self, other: &Environment) -> Environment;
}
```

**Example**:
```rust
let mut env = Environment::new();
env.add_rule(Rule {
    lhs: MettaValue::SExpr(vec![
        MettaValue::Atom("double".to_string()),
        MettaValue::Atom("$x".to_string()),
    ]),
    rhs: MettaValue::SExpr(vec![
        MettaValue::Atom("mul".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Long(2),
    ]),
});
```

### `Rule`

A pattern matching rule: `(= lhs rhs)`

```rust
pub struct Rule {
    pub lhs: MettaValue,  // Pattern (left-hand side)
    pub rhs: MettaValue,  // Body (right-hand side)
}
```

## Built-in Operations

Operations that are directly dispatched in Rust:

| MeTTa | Textual Name | Type | Example |
|-------|-------------|------|---------|
| `+` | `add` | Arithmetic | `(add 1 2)` → `3` |
| `-` | `sub` | Arithmetic | `(sub 5 3)` → `2` |
| `*` | `mul` | Arithmetic | `(mul 3 4)` → `12` |
| `/` | `div` | Arithmetic | `(div 10 2)` → `5` |
| `<` | `lt` | Comparison | `(lt 1 2)` → `true` |
| `<=` | `lte` | Comparison | `(lte 2 2)` → `true` |
| `>` | `gt` | Comparison | `(gt 3 2)` → `true` |
| `==` | `eq` | Comparison | `(eq 5 5)` → `true` |

**Note**: `>=` and `!=` are not yet supported due to lexer limitations.

## Pattern Matching

Variables and patterns:

**Variables**: Start with `$`, `&`, or `'`
- `$x` - Standard variable
- `&y` - Reference variable
- `'z` - Quote variable

**Wildcard**: `_` - Matches anything without binding

**Example**:
```rust
// Rule: (= (factorial $n) (if (< $n 2) 1 (* $n (factorial (- $n 1)))))
let rule = Rule {
    lhs: MettaValue::SExpr(vec![
        MettaValue::Atom("factorial".to_string()),
        MettaValue::Atom("$n".to_string()),
    ]),
    rhs: /* ... */,
};

env.add_rule(rule);

// Evaluate: (factorial 5)
let expr = MettaValue::SExpr(vec![
    MettaValue::Atom("factorial".to_string()),
    MettaValue::Long(5),
]);

let (result, new_env) = eval(expr, env);
// result = [Long(120)] (if fully implemented)
```

## Usage Patterns

### 1. Simple Evaluation

```rust
use mettatron::backend::*;

let (sexprs, env) = compile("(+ (* 2 3) 4)").unwrap();
let (result, _) = eval(sexprs[0].clone(), env);
println!("{:?}", result); // [Long(10)]
```

### 2. Working with Rules

```rust
let mut env = Environment::new();

// Add rule: (= (square $x) (* $x $x))
env.add_rule(Rule {
    lhs: MettaValue::SExpr(vec![
        MettaValue::Atom("square".to_string()),
        MettaValue::Atom("$x".to_string()),
    ]),
    rhs: MettaValue::SExpr(vec![
        MettaValue::Atom("mul".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Atom("$x".to_string()),
    ]),
});

// Evaluate: (square 5)
let expr = MettaValue::SExpr(vec![
    MettaValue::Atom("square".to_string()),
    MettaValue::Long(5),
]);

let (result, _) = eval(expr, env);
println!("{:?}", result); // [Long(25)]
```

### 3. Compositional Environments

```rust
let (exprs1, env1) = compile("(+ 1 2)").unwrap();
let (result1, env_after1) = eval(exprs1[0].clone(), env1);

let (exprs2, env2) = compile("(* 3 4)").unwrap();
let (result2, env_after2) = eval(exprs2[0].clone(), env2);

// Combine environments
let combined = env_after1.union(&env_after2);
```

### 4. Building Expressions Programmatically

```rust
// Build: (+ (- 10 5) (* 2 3))
let expr = MettaValue::SExpr(vec![
    MettaValue::Atom("add".to_string()),
    MettaValue::SExpr(vec![
        MettaValue::Atom("sub".to_string()),
        MettaValue::Long(10),
        MettaValue::Long(5),
    ]),
    MettaValue::SExpr(vec![
        MettaValue::Atom("mul".to_string()),
        MettaValue::Long(2),
        MettaValue::Long(3),
    ]),
]);

let (result, _) = eval(expr, Environment::new());
println!("{:?}", result); // [Long(11)]
```

## Running Examples

```bash
# Run the usage examples
cargo run --example backend_usage

# Run the interactive REPL
cargo run --example backend_interactive

# Run tests
cargo test backend::
```

## Error Handling

Both `compile` and operations return `Result` types:

```rust
match compile(source) {
    Ok((sexprs, env)) => {
        // Process sexprs
    }
    Err(e) => {
        eprintln!("Compilation error: {}", e);
    }
}
```

## Limitations

1. **URI literals** with backticks are not yet supported
2. **`>=` operator** is tokenized as two separate tokens
3. **PathMap integration** - Current implementation uses `Vec<Rule>` instead of PathMap trie
4. **Rholang AST conversion** - `to_proc_expr` is not yet implemented

## See Also

- [Backend Implementation](BACKEND_IMPLEMENTATION.md) - Architecture details
- [Examples](../examples/) - Code examples
- [Tests](../src/backend/) - Unit tests in each module
