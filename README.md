# MeTTaTron

A MeTTa language evaluator with lazy evaluation, pattern matching, and special forms.

## Overview

MeTTaTron is a direct evaluator for the MeTTa language featuring lazy evaluation semantics, pattern matching with variables, rule definitions, and control flow. It features a clean, pure Rust implementation with direct S-expression parsing.

## Features

- **Direct S-expression parsing** - No external parser generators required
- **Pure Rust implementation** - Fast, safe, and portable
- **Lazy evaluation** - Expressions evaluated only when needed
- **Pattern matching** - Automatic variable binding with `$x`, `&y`, `'z` variables
- **Rule definitions** - Define rewrite rules with `=`
- **Special forms** - Control flow (`if`), evaluation (`!`), quote, error handling
- **Type system** - Type assertions, type inference, and type checking with arrow types
- **Grounded functions** - Direct arithmetic and comparison operations
- **REPL mode** - Interactive evaluation environment
- **CLI and library** - Use as a command-line tool or integrate into your Rust projects

## Prerequisites

- Rust toolchain (1.70 or later)
- Cargo (comes with Rust)

## Installation

### From Source

```bash
git clone https://github.com/F1R3FLY-io/MeTTa-Compiler.git
cd MeTTa-Compiler
cargo build --release
```

The compiled binary will be available at `./target/release/mettatron`

### Install System-Wide

```bash
cargo install --path .
```

## Usage

### Command Line

```bash
# Evaluate MeTTa file
mettatron input.metta

# Write output to file
mettatron input.metta -o output.txt

# Show S-expressions (parse only)
mettatron --sexpr input.metta

# Start interactive REPL
mettatron --repl

# Read from stdin
cat input.metta | mettatron -
```

### Interactive REPL

Start the REPL for interactive MeTTa evaluation:

```bash
mettatron --repl
```

Example REPL session:

```
MeTTaTron REPL v0.1.0
Enter MeTTa expressions. Type 'exit' or 'quit' to exit.

metta[1]> (= (double $x) (* $x 2))
Nil

metta[2]> !(double 21)
42

metta[3]> (= (factorial 0) 1)
Nil

metta[4]> (= (factorial $n) (* $n (factorial (- $n 1))))
Nil

metta[5]> !(factorial 5)
120

metta[6]> exit
Goodbye!
```

See `docs/REPL_USAGE.md` for complete REPL documentation.

### Library Usage

Add to your `Cargo.toml`:

```toml
[dependencies]
mettatron = { path = "../MeTTa-Compiler" }
```

Use in your Rust code:

```rust
use mettatron::backend::*;

// Define a rule and evaluate it
let input = r#"
    (= (double $x) (* $x 2))
    !(double 21)
"#;

let (sexprs, mut env) = compile(input).unwrap();
for sexpr in sexprs {
    let (results, new_env) = eval(sexpr, env);
    env = new_env;

    for result in results {
        println!("{:?}", result);  // Prints: Long(42)
    }
}
```

## MeTTa Language

### Syntax Overview

MeTTa uses S-expression syntax similar to Lisp:

```lisp
// Rule definitions
(= (f) 42)
(= (double $x) (* $x 2))

// Evaluation (force rule application)
!(f)
!(double 21)

// Conditionals with lazy branches
(if (< 5 10) "less" "greater")

// Quote (prevent evaluation)
(quote (+ 1 2))

// Error handling
(error "message" details)

// Arithmetic
(+ 1 2)
(* (- 10 5) 2)

// Comparison
(< 5 10)
(== 42 42)
```

### Data Types

- **Ground Types**: `Bool`, `String`, `Long`, `URI`
- **Literals**: `true`, `false`, `42`, `"hello"`
- **Variables**: `$x`, `&y`, `'z`
- **Wildcards**: `_`
- **S-expressions**: `(expr ...)`
- **Nil**: Returned by rule definitions
- **Error**: `(error msg details)`

### Special Forms

- **`=`** - Define rule: `(= lhs rhs)` adds pattern matching rule
- **`!`** - Force evaluation: `!(expr)` applies rules
- **`if`** - Conditional: `(if cond then else)` with lazy branch evaluation
- **`quote`** - Prevent evaluation: `(quote expr)` returns expr unevaluated
- **`eval`** - Force evaluation: `(eval expr)` evaluates quoted expressions
- **`error`** - Create error: `(error msg details)`
- **`catch`** - Error recovery: `(catch expr default)` returns default if expr errors
- **`is-error`** - Error check: `(is-error expr)` returns true if expr is an error

### Type System

The evaluator includes basic type system support with type assertions and type checking:

**Type Assertions**: `(: expr type)`
```lisp
(: x Number)           ; Assert x has type Number
(: name String)        ; Assert name has type String
(: add (-> Number Number Number))  ; Function type with arrow notation
```

**Type Inference**: `(get-type expr)`
```lisp
!(get-type 42)         ; Returns: Number
!(get-type true)       ; Returns: Bool
!(get-type "hello")    ; Returns: String
!(get-type x)          ; Returns type of x from assertions
!(get-type (add 1 2))  ; Returns: Number (inferred from operation)
```

**Type Checking**: `(check-type expr expected-type)`
```lisp
!(check-type 42 Number)    ; Returns: true
!(check-type 42 String)    ; Returns: false
!(check-type x $t)         ; Returns: true (type variable matches anything)
```

**Features**:
- **Ground type inference**: Automatic types for Bool, Long, String, URI, Nil
- **Type assertions**: Explicit type declarations for atoms and functions
- **Arrow types**: Function types with `(-> ArgType... ReturnType)` syntax
- **Type variables**: Polymorphic types with `$t`, `$a`, etc.
- **Built-in operation types**: Arithmetic returns Number, comparisons return Bool

See `examples/type_system_demo.metta` for a complete demonstration.

### Grounded Functions

Built-in operations evaluated directly:

- **Arithmetic**: `+` (add), `-` (sub), `*` (mul), `/` (div)
- **Comparison**: `<` (lt), `<=` (lte), `>` (gt), `==` (eq)

### Pattern Matching

Variables automatically bind during pattern matching:

```lisp
metta[1]> (= (add $a $b) (+ $a $b))
Nil

metta[2]> !(add 10 20)
30
```

### Evaluation Strategy

- **Lazy Evaluation**: Expressions evaluated only when needed
- **Rule Application**: `!` operator applies matching rules recursively
- **Direct Evaluation**: Arithmetic and comparisons evaluated directly
- **Error Propagation**: First error stops evaluation immediately
- **Reduction Prevention**: Control evaluation with `quote`, `eval`, `catch`, and error handling

### Reduction Prevention

The evaluator provides comprehensive reduction prevention mechanisms:

```lisp
; Quote prevents evaluation
(quote (+ 1 2))  ; Returns (+ 1 2) unevaluated

; Eval forces evaluation of quoted expressions
(eval (quote (+ 1 2)))  ; Returns 3

; Catch recovers from errors
(catch (error "fail" 0) 42)  ; Returns 42 instead of error

; Is-error checks for errors
(is-error (error "test" 0))  ; Returns true
(is-error 42)                 ; Returns false

; Complex error handling
(catch (/ 10 0) (error "recovered" 0))  ; Catches division error
```

## Examples

See the `examples/` directory for sample programs:

- `examples/mvp_test.metta` - MVP feature demonstrations
- `examples/simple.metta` - Basic language features
- `examples/advanced.metta` - Advanced patterns
- `examples/type_system_demo.metta` - Type system demonstrations

Example files using the backend:

```bash
cargo run --example backend_usage      # Basic backend usage
cargo run --example backend_interactive # Interactive REPL
cargo run --example mvp_complete       # Complete MVP demonstration
```

Run an example:

```bash
./target/release/mettatron examples/mvp_test.metta
```

## Architecture

The evaluator consists of two main stages:

### 1. Lexical Analysis & S-expression Parsing (`src/sexpr.rs`)
- Tokenizes input text
- Parses tokens into S-expressions
- Handles comments: `//`, `/* */`, `;`
- Supports special operators: `!`, `?`, `<-`, `<=`, `<<-`, etc.
- Prefix operator handling: `!(expr)` → `(! expr)`

### 2. Backend Evaluation (`src/backend/`)

#### Types (`src/backend/types.rs`)
- Core type definitions: `MettaValue`, `Environment`, `Rule`
- Pattern matching support with variables and wildcards

#### Compilation (`src/backend/compile.rs`)
- Parses MeTTa source to `MettaValue` expressions
- Converts operators to function names (`+` → `"add"`)
- Returns expressions and initial environment

#### Evaluation (`src/backend/eval.rs`)
- Lazy evaluation with special forms
- Pattern matching with variable binding
- Rule application with `!` operator
- Grounded function dispatch
- Error propagation

### Evaluation Flow

```
MeTTa Source → Tokens → S-expressions → MettaValue → Evaluation Results
                ↑                          ↑              ↑
           sexpr.rs                  compile.rs      eval.rs
```

## Development

### Building

```bash
# Debug build
cargo build

# Release build
cargo build --release

# Run tests
cargo test

# Run with example
cargo run -- examples/mvp_test.metta
```

### Testing

```bash
# Run all tests
cargo test

# Run specific test
cargo test test_eval_grounded_add

# Show test output
cargo test -- --nocapture
```

### Project Structure

```
MeTTa-Compiler/
├── src/                        # Source code
│   ├── main.rs                 # CLI and REPL implementation
│   ├── lib.rs                  # Library exports
│   ├── sexpr.rs                # Lexer and S-expression parser
│   ├── backend/                # Evaluation engine
│   │   ├── mod.rs              # Module exports
│   │   ├── types.rs            # Core types (MettaValue, Environment, Rule)
│   │   ├── compile.rs          # MeTTa source → MettaValue compilation
│   │   └── eval.rs             # Lazy evaluation with pattern matching
│   ├── rholang_integration.rs  # Rholang integration API
│   └── ffi.rs                  # C FFI layer
├── examples/                   # Example files
│   ├── *.metta                 # MeTTa language examples
│   ├── *.rs                    # Rust backend examples
│   └── *.rho                   # Rholang integration examples
├── docs/                       # Documentation
│   ├── guides/                 # User guides
│   ├── reference/              # API and language reference
│   ├── design/                 # Design documents
│   ├── ISSUE_3_SATISFACTION.md # MVP satisfaction analysis
│   └── MVP_BACKEND_COMPLETE.md # MVP status report
├── integration/                # Rholang integration
│   ├── templates/              # Integration code templates
│   ├── archive/                # Legacy FFI approaches
│   ├── README.md               # Integration guide
│   └── *.md                    # Integration documentation
├── target/                     # Build artifacts (gitignored)
├── Cargo.toml                  # Rust project configuration
├── Cargo.lock                  # Dependency lock file
├── CLAUDE.md                   # Claude Code guidance
├── LICENSE                     # Apache 2.0 license
└── README.md                   # This file
```

## Rholang Integration

MeTTaTron can be integrated with Rholang to provide MeTTa compilation as a system process service. Since both projects are written in Rust, **direct Rust linking** is recommended for better performance, safety, and simplicity.

### Integration Approach

**⭐ Recommended: Direct Rust Linking (v3)**
- Simple, safe, and fast
- No FFI overhead
- Pure Rust integration
- 60% less code than FFI

**Alternative: FFI (v2)**
- For non-Rust languages (Python, Node.js, C++)
- C-compatible interface
- Cross-language ABI

### Quick Start

**Direct Rust Integration (~15 minutes)**:
1. Add to Rholang's `Cargo.toml`: `mettatron = { path = "../../../MeTTa-Compiler" }`
2. Import: `use mettatron::rholang_integration::compile_safe;`
3. Call directly: `let result = compile_safe(&src);`

**For complete instructions**, see:
- **`integration/DIRECT_RUST_INTEGRATION.md`** ⭐ - Direct Rust integration guide (recommended)
- **`integration/FFI_VS_DIRECT_COMPARISON.md`** - Detailed comparison of approaches
- **`integration/DEPLOYMENT_CHECKLIST.md`** - Quick reference checklist
- **`integration/DEPLOYMENT_GUIDE.md`** - Comprehensive step-by-step guide (FFI approach)

### What's Provided

✅ **Direct Rust Integration (v3) - Recommended**:
- Handler code (`integration/templates/rholang_handler.rs`) - Direct Rust handlers (no FFI)
- Registry code (`integration/templates/rholang_registry.rs`) - Service registration
- JSON serialization (`src/rholang_integration.rs`) - Native Rust API
- Complete documentation (`integration/DIRECT_RUST_INTEGRATION.md`)

✅ **FFI Integration (v2) - For non-Rust languages**:
- C FFI layer (`src/ffi.rs`) - Memory-safe C-compatible interface
- Handler code (`integration/archive/rholang_handler_v2_ffi.rs`) - FFI handlers
- Registry code (`integration/archive/rholang_registry_v2_ffi.rs`) - FFI service registration
- Deployment guide (`integration/DEPLOYMENT_GUIDE.md`)

### Usage from Rholang

Once deployed, compile MeTTa code from Rholang using **two patterns**:

**Traditional Pattern** (explicit return channel):
```rholang
new result in {
  @"rho:metta:compile"!("(+ 1 2)", *result) |
  for (@json <- result) {
    stdoutAck!(json, *ack)
  }
}
```

**Synchronous Pattern** (optimized for `!?`):
```rholang
@"rho:metta:compile:sync" !? ("(+ 1 2)") ; {
  // Continuation executes after compile completes
  stdoutAck!("Compilation done", *ack)
}
```

See **`RHOLANG_SYNC_GUIDE.md`** for complete usage patterns with the `!?` operator

### Integration Features

**Direct Rust (v3) Benefits:**
- ✅ **Type Safe**: Compile-time error checking
- ✅ **Memory Safe**: Automatic memory management (no manual allocation)
- ✅ **Performance**: 5-10x faster than FFI (no ABI overhead)
- ✅ **Simple**: 60% less code than FFI approach
- ✅ **No Unsafe Code**: Pure safe Rust
- ✅ **Better Debugging**: Full Rust stack traces

**Both Approaches:**
- **Thread Safe**: No shared mutable state
- **Error Handling**: JSON responses for success and failure
- **Zero I/O**: Pure compilation (no filesystem/network access)
- **Compilation Time**: ~1-5ms per expression

### Documentation

**Integration Guides:**
- **`integration/DIRECT_RUST_INTEGRATION.md`** ⭐ - Direct Rust integration (recommended for Rholang)
- **`integration/FFI_VS_DIRECT_COMPARISON.md`** - Complete comparison of FFI vs Direct Rust
- **`integration/DEPLOYMENT_CHECKLIST.md`** - Quick deployment checklist
- **`integration/DEPLOYMENT_GUIDE.md`** - Complete deployment guide (FFI approach)

**Usage Guides:**
- **`integration/RHOLANG_SYNC_GUIDE.md`** - Using MeTTa with Rholang's `!?` operator (two patterns)
- **`integration/RHOLANG_REGISTRY_PATTERN.md`** - Registry binding with `!?` operator
- **`integration/SYNC_OPERATOR_SUMMARY.md`** - `!?` operator implementation summary

**Technical Details:**
- **`integration/RHOLANG_INTEGRATION_SUMMARY.md`** - Integration status and overview
- **`integration/RHOLANG_INTEGRATION.md`** - Technical architecture details

## Documentation

### User Guides
- **`docs/guides/REPL_USAGE.md`** - Interactive REPL usage guide
- **`docs/guides/REDUCTION_PREVENTION.md`** - Comprehensive reduction prevention guide

### API Reference
- **`docs/reference/BACKEND_API_REFERENCE.md`** - Complete backend API reference
- **`docs/reference/METTA_TYPE_SYSTEM_REFERENCE.md`** - Official MeTTa type system reference
- **`docs/reference/TYPE_SYSTEM_ANALYSIS.md`** - Type system implementation analysis

### Design Documents
- **`docs/design/BACKEND_IMPLEMENTATION.md`** - Backend implementation details
- **`docs/design/TYPE_SYSTEM_IMPLEMENTATION.md`** - Type system design
- **`docs/design/TYPE_SYSTEM_RHOLANG_INTEGRATION.md`** - Type system Rholang integration
- **`docs/design/MORK_PATHMAP_QUERY_DESIGN.md`** - MORK PathMap query design
- **`docs/design/RULE_INDEX_OPTIMIZATION.md`** - Rule indexing optimization
- **`docs/design/SEXPR_FACTS_DESIGN.md`** - S-expression facts design
- **`docs/design/TODO_ANALYSIS.md`** - TODO analysis and planning

### Status Reports
- **`docs/ISSUE_3_SATISFACTION.md`** - GitHub Issue #3 MVP requirements satisfaction
- **`docs/MVP_BACKEND_COMPLETE.md`** - MVP implementation status and test results

### Integration
- **`integration/`** - Complete Rholang integration documentation (see `integration/README.md`)

## MVP Status

The implementation satisfies all 7 MVP requirements:

1. ✅ Variable binding in rules
2. ✅ Multivalued results
3. ✅ Control flow (if with lazy branches)
4. ✅ Grounded functions (arithmetic & comparisons)
5. ✅ Evaluation order (lazy evaluation)
6. ✅ Equality operator (==)
7. ✅ Error termination (Error variant with propagation)

See `docs/MVP_BACKEND_COMPLETE.md` for details.

## License

This project is licensed under the Apache License 2.0 - see the [LICENSE](LICENSE) file for details.

Copyright 2025 F1R3FLY.io

## Contributing

Contributions are welcome! Please ensure:

1. Code compiles without warnings: `cargo build --release`
2. All tests pass: `cargo test`
3. Code is formatted: `cargo fmt`
4. Code is linted: `cargo clippy`

## Support

For issues and questions:
- GitHub Issues: https://github.com/F1R3FLY-io/MeTTa-Compiler/issues
- Repository: https://github.com/F1R3FLY-io/MeTTa-Compiler
