# MeTTaTron

[![Integration Tests](https://github.com/F1R3FLY-io/MeTTa-Compiler/actions/workflows/integration-tests.yml/badge.svg)](https://github.com/F1R3FLY-io/MeTTa-Compiler/actions/workflows/integration-tests.yml)
[![Nightly Tests](https://github.com/F1R3FLY-io/MeTTa-Compiler/actions/workflows/nightly-tests.yml/badge.svg)](https://github.com/F1R3FLY-io/MeTTa-Compiler/actions/workflows/nightly-tests.yml)
[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

A MeTTa language evaluator with lazy evaluation, pattern matching, and special forms.

## Overview

MeTTaTron is a direct evaluator for the MeTTa language featuring lazy evaluation semantics, pattern matching with variables, rule definitions, and control flow. It features a clean, pure Rust implementation with direct S-expression parsing.

## Features

- **Direct S-expression parsing** - No external parser generators required
- **Pure Rust implementation** - Fast, safe, and portable
- **Lazy evaluation** - Expressions evaluated only when needed
- **Async parallel evaluation** - True parallelization with configurable threading via Tokio
- **Pattern matching** - Automatic variable binding with `$x`, `&y`, `'z` variables
- **Rule definitions** - Define rewrite rules with `=`
- **Special forms** - Control flow (`if`), evaluation (`!`), quote, error handling
- **Type system** - Type assertions, type inference, and type checking with arrow types
- **Grounded functions** - Direct arithmetic and comparison operations
- **MORK/PathMap integration** - Efficient pattern matching with MORK zipper optimization
- **REPL mode** - Interactive evaluation environment
- **CLI and library** - Use as a command-line tool or integrate into your Rust projects
- **Comprehensive tests** - 474 tests covering all language features
- **Nondeterministic evaluation** - Multiply-defined patterns with Cartesian product semantics

## Prerequisites

- Rust nightly toolchain (required by dependencies)
- Cargo (comes with Rust)

**Installing Rust Nightly:**
```bash
rustup install nightly
rustup default nightly
```

## Installation

### From Source

```bash
git clone https://github.com/F1R3FLY-io/MeTTa-Compiler.git
cd MeTTa-Compiler
cargo build --release
```

**Note:** The project requires Rust nightly due to dependencies (MORK, PathMap, gxhash) that use unstable features.

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
metta[2]> !(double 21)
[42]
metta[3]> (= (factorial 0) 1)
metta[4]> (= (factorial $n) (* $n (factorial (- $n 1))))
metta[5]> !(factorial 5)
[120]
metta[6]> (= (coin) heads)
metta[7]> (= (coin) tails)
metta[8]> !(coin)
[heads, tails]
metta[9]> exit
Goodbye!
```

See `docs/guides/REPL_GUIDE.md` for complete REPL documentation.

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

let state = compile(input).unwrap();
let mut env = state.environment;

for sexpr in state.source {
    let (results, new_env) = eval(sexpr, env);
    env = new_env;

    for result in results {
        println!("{:?}", result);  // Prints: Long(42)
    }
}
```

For async parallel evaluation:

```rust
use mettatron::{compile, run_state_async, MettaState, config};

// Configure threading (optional, call once at startup)
config::configure_eval(config::EvalConfig::cpu_optimized());

#[tokio::main]
async fn main() {
    let state = MettaState::new_empty();
    let compiled = compile("!(+ 1 2)").unwrap();

    let result = run_state_async(state, compiled).await.unwrap();
    println!("{:?}", result.output);  // Prints evaluation results
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

- **Ground Types**: `Bool`, `String`, `Long`, `Float`, `URI`
- **Literals**: `true`, `false`, `42`, `3.14`, `"hello"`
- **Variables**: `$x`, `&y`, `'z`
- **Wildcards**: `_`
- **S-expressions**: `(expr ...)`
- **Nil**: Returned by rule definitions
- **Error**: `(error msg details)`

### Special Forms

- **`=`** - Define rule: `(= lhs rhs)` adds pattern matching rule
- **`!`** - Force evaluation: `!(expr)` applies rules
- **`if`** - Conditional: `(if cond then else)` with lazy branch evaluation
- **`match`** - Pattern matching: `(match space pattern template)` queries atom space
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
!(get-type 3.14)       ; Returns: Number
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
- **Ground type inference**: Automatic types for Bool, Long, Float, String, URI, Nil
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

### Match Operation

The `match` special form queries the atom space for patterns and instantiates templates:

**Basic Syntax**: `(match & self pattern template)`
- `&` - Space reference operator
- `self` - Name of the space (currently only `self` is supported)
- `pattern` - Pattern to match (with variables like `$x`, `$y`)
- `template` - Template to instantiate for each match

**Example - Basic Pattern**:
```lisp
(Sam is a frog)
(Tom is a cat)
!(match & self ($who is a $what) ($who the $what))
; Returns: [(Sam the frog), (Tom the cat)]
```

**Example - Extracting Values**:
```lisp
(number 42)
(number 100)
!(match & self (number $n) (value $n))
; Returns: [(value 42), (value 100)]
```

**Example - Nested Structures**:
```lisp
((nested value) result)
!(match & self (($x $y) result) (found $x and $y))
; Returns: [(found nested and value)]
```

**Key Features**:
- Returns ALL matching results (nondeterministic)
- Works directly with MORK/PathMap for efficient querying
- Variables in pattern bind to matched values
- Template is instantiated for each match
- Empty list returned if no matches found

### Nondeterministic Evaluation

MeTTa supports nondeterministic evaluation where multiply-defined patterns return all matching results:

**Simple Nondeterminism**:
```lisp
(= (coin) heads)
(= (coin) tails)
!(coin)  ; Returns [heads, tails]
```

**Nested Application** - Functions apply to ALL results:
```lisp
(= (f) 1)
(= (f) 2)
(= (f) 3)
(= (g $x) (* $x $x))
!(g (f))  ; Returns [1, 4, 9]
```

**Cartesian Product** - Multiple nondeterministic operands:
```lisp
(= (a) 1)
(= (a) 2)
(= (b) 10)
(= (b) 20)
!(+ (a) (b))  ; Returns [11, 21, 12, 22]
```

**Pattern Specificity**: When patterns overlap, only the most specific matches are used:
```lisp
(= (factorial 0) 1)
(= (factorial $n) (* $n (factorial (- $n 1))))
!(factorial 5)  ; Returns [120] (not multiple results)
```

The `(factorial 0)` pattern is more specific than `(factorial $n)`, so only the best match is evaluated.

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

See the `examples/` directory for sample programs and detailed usage guide in `examples/README.md`.

### MeTTa Language Examples

- `examples/simple.metta` - Basic language features
- `examples/advanced.metta` - Advanced patterns
- `examples/mvp_test.metta` - MVP feature demonstrations
- `examples/type_system_demo.metta` - Type system demonstrations
- `examples/pathmap_demo.metta` - PathMap operations

### Rust Backend Examples

```bash
cargo run --example backend_usage         # Basic backend API usage
cargo run --example backend_interactive   # Interactive REPL implementation
cargo run --example mvp_complete          # Complete MVP demonstration
cargo run --example test_zipper_optimization  # MORK zipper optimization
cargo run --example threading_config      # Threading configuration examples
```

### Run a MeTTa File

```bash
# Build first
cargo build --release

# Run an example
./target/release/mettatron examples/simple.metta
```

### Rholang Integration Examples

- `examples/metta_rholang_example.rho` - Basic MeTTa usage from Rholang
- `examples/robot_planning.rho` - Robot planning domain example

## Architecture

The evaluator consists of two main stages:

### 1. Tree-Sitter Parsing (`src/tree_sitter_parser.rs` + `src/ir.rs`)
- Uses Tree-Sitter grammar for robust parsing
- Converts parse trees to S-expression IR (`SExpr`)
- Tracks source positions for error reporting
- Handles comments: `//`, `/* */`, `;`
- Supports special operators: `!`, `?`, `<-`, `<=`, `<<-`, etc.

### 2. Backend Evaluation (`src/backend/`)

#### Models (`src/backend/models/`)
- Core type definitions: `MettaValue`, `Environment`, `Rule`
- Pattern matching support with variables and wildcards
- Type system representations

#### Compilation (`src/backend/compile.rs`)
- Parses MeTTa source to `MettaValue` expressions
- Preserves operator symbols as-is (`+` stays `+`, not normalized)
- Returns `MettaState` with source expressions and empty environment

#### Evaluation (`src/backend/eval/`)
Modular evaluation engine split by functionality:
- **`mod.rs`** - Core evaluation logic and pattern matching
- **`evaluation.rs`** - Main eval loop and rule application
- **`bindings.rs`** - Variable binding and unification
- **`control_flow.rs`** - `if`, `switch`, `case` special forms
- **`errors.rs`** - Error handling and propagation
- **`list_ops.rs`** - List operations (cons, car, cdr, etc.)
- **`quoting.rs`** - Quote and eval special forms
- **`space.rs`** - Space operations and match
- **`types.rs`** - Type inference and checking
- **`macros.rs`** - Helper macros for evaluation
- Async parallel evaluation support via `run_state_async`

### Evaluation Flow

```
MeTTa Source → Tree-Sitter → SExpr IR → MettaValue → Evaluation Results
                    ↑            ↑          ↑              ↑
           tree_sitter_parser  ir.rs  compile.rs    eval/mod.rs
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
├── src/                           # Source code
│   ├── main.rs                    # CLI and REPL implementation
│   ├── lib.rs                     # Library exports
│   ├── config.rs                  # Threading configuration
│   ├── ir.rs                      # Intermediate representation (SExpr, Position, Span)
│   ├── tree_sitter_parser.rs      # Tree-Sitter based parser
│   ├── backend/                   # Evaluation engine
│   │   ├── mod.rs                 # Module exports
│   │   ├── compile.rs             # MeTTa source → MettaValue compilation
│   │   ├── models/                # Type definitions
│   │   │   ├── mod.rs
│   │   │   └── metta_value.rs     # MettaValue enum and methods
│   │   ├── eval/                  # Modular evaluation engine
│   │   │   ├── mod.rs             # Core evaluation and pattern matching
│   │   │   ├── evaluation.rs      # Main eval loop and rule application
│   │   │   ├── bindings.rs        # Variable binding and unification
│   │   │   ├── control_flow.rs    # if, switch, case special forms
│   │   │   ├── errors.rs          # Error handling and propagation
│   │   │   ├── list_ops.rs        # List operations (cons, car, cdr, etc.)
│   │   │   ├── quoting.rs         # Quote and eval special forms
│   │   │   ├── space.rs           # Space operations and match
│   │   │   ├── types.rs           # Type inference and checking
│   │   │   └── macros.rs          # Helper macros for evaluation
│   │   └── mork_convert.rs        # MORK/PathMap conversion
│   ├── rholang_integration.rs     # Rholang integration API (sync & async)
│   ├── pathmap_par_integration.rs # PathMap Par conversion
│   └── environment.rs             # Environment and rule management
├── examples/                      # Code examples
│   ├── README.md                  # Examples guide
│   ├── *.metta                    # MeTTa language examples
│   ├── *.rs                       # Rust backend examples
│   └── *.rho                      # Rholang integration examples
├── docs/                          # User documentation
│   ├── README.md                  # Documentation index
│   ├── ISSUE_3_SATISFACTION.md    # MVP requirements analysis
│   ├── MVP_BACKEND_COMPLETE.md    # MVP milestone documentation
│   ├── THREADING_MODEL.md         # Threading documentation
│   ├── guides/                    # User guides (REPL, reduction prevention)
│   ├── reference/                 # API and language reference
│   ├── design/                    # Design documents
│   ├── testing/                   # Testing documentation
│   └── archive/                   # Historical documents
├── integration/                   # Integration guides
│   ├── README.md                  # Integration overview
│   ├── QUICK_START.md             # Quick start guide
│   ├── RHOLANG_INTEGRATION.md     # Rholang integration details
│   ├── DEPLOYMENT_*.md            # Deployment guides
│   ├── TESTING_GUIDE.md           # Testing documentation
│   ├── test_*.rho                 # Integration test files
│   └── *.md                       # Other integration docs
├── .claude/                       # Claude AI documentation (28 files)
│   ├── README.md                  # Guide to AI docs
│   ├── CLAUDE.md                  # Claude Code instructions
│   └── *.md                       # Planning guides, status docs
├── target/                        # Build artifacts (gitignored)
├── Cargo.toml                     # Rust project configuration
├── Cargo.lock                     # Dependency lock file
├── LICENSE                        # Apache 2.0 license
└── README.md                      # This file
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

See **`integration/RHOLANG_SYNC_GUIDE.md`** for complete usage patterns with the `!?` operator

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
- **`integration/RHOLANG_INTEGRATION.md`** - Technical architecture details

## Threading and Performance

MeTTaTron supports **async parallel evaluation** for independent MeTTa expressions using Tokio's async runtime. This enables true parallelization and efficient resource utilization.

### Performance Highlights

Recent optimizations in the `dylon/rholang-language-server` branch deliver **significant performance improvements**:

- **2.05× overall speedup** (51.2% faster on average)
- **Async evaluation**: 2.94× faster (66.1% improvement)
- **Sync evaluation**: 1.44× faster (28.4% improvement)

**Key improvements:**
- Async concurrent operations: **4.14× faster** (75.9% improvement)
- Async reasoning tasks: **3.42× faster** (70.7% improvement)
- Pattern matching: **1.60× faster** (37.5% improvement)

These results come from comprehensive benchmarking using real-world MeTTa programs. See **`docs/benchmarks/METTA_BENCHMARK_SUITE.md`** for complete benchmark documentation.

### Configuration

```rust
use mettatron::config::{EvalConfig, configure_eval};

// Configure once at startup
configure_eval(EvalConfig::cpu_optimized());
```

### Preset Configurations

- **`default()`** - Tokio default (512 max blocking threads)
- **`cpu_optimized()`** - Best for CPU-bound workloads (num_cpus × 2)
- **`memory_optimized()`** - Best for memory-constrained systems (num_cpus)
- **`throughput_optimized()`** - Best for high-throughput batch processing (1024 threads)

### Custom Configuration

```rust
configure_eval(EvalConfig {
    max_blocking_threads: 256,
    batch_size_hint: 64,
});
```

### Benchmarking

Run the comprehensive MeTTa benchmark suite:

```bash
# Run all benchmarks
cargo bench --bench metta

# Run specific benchmark categories
cargo bench --bench metta async_    # Async benchmarks only
cargo bench --bench rule_matching   # Rule matching performance
cargo bench --bench pattern_match   # Pattern matching performance
```

See **`docs/THREADING_MODEL.md`**, **`docs/guides/CONFIGURATION.md`**, and **`docs/benchmarks/METTA_BENCHMARK_SUITE.md`** for detailed information.

## Documentation

### Getting Started
- **`examples/README.md`** - Examples usage guide
- **`integration/QUICK_START.md`** - Quick start for Rholang integration

### User Guides
- **`docs/guides/REPL_GUIDE.md`** - Comprehensive interactive REPL guide
- **`docs/guides/REDUCTION_PREVENTION.md`** - Comprehensive reduction prevention guide
- **`docs/guides/CONFIGURATION.md`** - Configuration guide
- **`docs/THREADING_MODEL.md`** - Threading and parallelization documentation

### API Reference
- **`docs/reference/BACKEND_API_REFERENCE.md`** - Complete backend API reference
- **`docs/reference/METTA_TYPE_SYSTEM_REFERENCE.md`** - Official MeTTa type system reference
- **`docs/reference/BUILTIN_FUNCTIONS_REFERENCE.md`** - Built-in functions catalog and status

### Design Documents
- **`docs/design/BACKEND_IMPLEMENTATION.md`** - Backend implementation details
- **`docs/design/TYPE_SYSTEM_IMPLEMENTATION.md`** - Type system design
- **`docs/design/MORK_PATHMAP_QUERY_DESIGN.md`** - MORK PathMap query design
- **`docs/design/RULE_INDEX_OPTIMIZATION.md`** - Rule indexing optimization
- **`docs/design/SEXPR_FACTS_DESIGN.md`** - S-expression facts design

### Integration
- **`integration/README.md`** - Integration overview and guides
- **`integration/DIRECT_RUST_INTEGRATION.md`** - Direct Rust integration (recommended)
- **`integration/RHOLANG_INTEGRATION.md`** - Technical architecture details
- **`integration/DEPLOYMENT_GUIDE.md`** - Deployment guide
- **`integration/TESTING_GUIDE.md`** - Testing documentation
- **`docs/guides/RHOLANG_PARSER_NAMED_COMMENTS.md`** - Rholang parser configuration and named comments feature
- **`docs/guides/RHOLANG_BUILD_AUTOMATION.md`** - Build automation strategies for Rholang parser

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
3. Code is formatted: `cargo fmt -- --check` (do NOT use `--all`)
4. Code is linted: `cargo clippy --all-targets --all-features -- -D warnings`

**Note:** Use `cargo fmt` (without `--all`) to avoid formatting external path dependencies (MORK, PathMap, f1r3node).

## Support

For issues and questions:
- GitHub Issues: https://github.com/F1R3FLY-io/MeTTa-Compiler/issues
- Repository: https://github.com/F1R3FLY-io/MeTTa-Compiler
