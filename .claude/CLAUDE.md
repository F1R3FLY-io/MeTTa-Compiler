# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is a MeTTa language evaluator (MeTTaTron) with lazy evaluation, pattern matching, and special forms. MeTTa is a language with LISP-like S-expression syntax supporting rules, control flow, type assertions, and grounded functions. The evaluator uses direct S-expression parsing (no BNFC) and is implemented in pure Rust. It can also integrate with Rholang via direct Rust linking.

## Build System

### Main Build Command

```bash
cargo build --release
```

The compiled binary will be at `./target/release/mettatron`

### Running the Evaluator

```bash
# Evaluate MeTTa file
./target/release/mettatron input.metta

# Write output to file
./target/release/mettatron input.metta -o output.txt

# Show S-expressions (parse only)
./target/release/mettatron --sexpr input.metta

# Start interactive REPL
./target/release/mettatron --repl

# Read from stdin
cat input.metta | ./target/release/mettatron -
```

### Development Commands

```bash
# Debug build
cargo build

# Run tests
cargo test

# Format code
cargo fmt

# Lint
cargo clippy

# Run with example
cargo run -- examples/simple.metta
```

## Prerequisites

- Rust toolchain (1.70+)
- Cargo (comes with Rust)

No external parser generators or C toolchain required.

## Architecture

### Evaluation Pipeline

```
MeTTa Source → Tokens → S-expressions → MettaValue → Evaluation Results
```

The evaluator consists of two main stages:

1. **Lexical Analysis & S-expression Parsing** (`src/sexpr.rs`)
   - Tokenizes input text into structured tokens
   - Parses tokens into S-expressions
   - Handles comments: `//`, `/* */`, `;`
   - Supports special operators: `!`, `?`, `<-`, `<=`, etc.
   - Prefix operator handling: `!(expr)` → `(! expr)`

2. **Backend Evaluation** (`src/backend/`)
   - **Compilation** (`compile.rs`) - Parses MeTTa source to `MettaValue` expressions
   - **Types** (`types.rs`) - Core types: `MettaValue`, `Environment`, `Rule`
   - **Evaluation** (`eval.rs`) - Lazy evaluation with pattern matching and special forms

### Key File Locations

- **Library exports**: `src/lib.rs`
- **CLI and REPL**: `src/main.rs`
- **Lexer/S-expr parser**: `src/sexpr.rs`
- **Backend evaluator**: `src/backend/` (compile.rs, eval.rs, types.rs, mod.rs)
- **Rholang integration**: `src/rholang_integration.rs`
- **FFI layer**: `src/ffi.rs`
- **Project config**: `Cargo.toml`
- **Examples**: `examples/` (*.metta files and Rust examples)

## MeTTa Language Features

### Core Features

- **Rule Definition**: `(= pattern body)` - Define pattern matching rules
- **Evaluation**: `!(expr)` - Force evaluation with rule application
- **Pattern Matching**: Variables (`$x`, `&y`, `'z`) and wildcard (`_`)
- **Control Flow**: `(if cond then else)` - Conditional with lazy branches
- **Quote**: `(quote expr)` - Prevent evaluation
- **Eval**: `(eval expr)` - Force evaluation of quoted expressions
- **Error Handling**: `(error msg details)`, `(catch expr default)`, `(is-error expr)`
- **Type System**: Type assertions `(: expr type)`, `(get-type expr)`, `(check-type expr type)`
- **Grounded Functions**: Arithmetic (`+`, `-`, `*`, `/`) and comparisons (`<`, `<=`, `>`, `==`)

### Data Types

- **Ground Types**: `Bool`, `String`, `Long`, `URI`, `Nil`
- **Literals**: `true`, `false`, `42`, `"hello"`, `` `uri` ``
- **Variables**: `$x`, `&y`, `'z` (start with `$`, `&`, or `'`)
- **Wildcards**: `_` (matches anything)
- **S-expressions**: `(expr ...)` and quoted forms
- **Errors**: `(error msg details)`

### Lexical Tokens

- **Variables**: `$x`, `&y`, `'z`
- **Strings**: `"hello"` (double-quoted with escapes)
- **URIs**: `` `uri` `` (backtick-quoted)
- **Integers**: `42`, `-10`
- **Booleans**: `true`, `false`
- **Comments**: `//` (line), `/* */` (block), `;` (line)

## Working with the Codebase

### Adding New Features

1. **New MeTTa syntax**: Update `src/sexpr.rs` (tokens) and `src/backend/compile.rs` (parsing)
2. **New evaluation semantics**: Update `src/backend/eval.rs` (evaluation logic)
3. **New special forms**: Update `src/backend/eval.rs` (add to eval_special_form)
4. **New grounded functions**: Update `src/backend/eval.rs` (add to eval_grounded)
5. **New CLI options**: Update `src/main.rs`

### Testing

The codebase includes tests in each module:

```bash
# Run all tests
cargo test

# Run tests with output
cargo test -- --nocapture

# Run specific test
cargo test test_compile_simple
```

### Debugging Evaluation

Use the `--sexpr` flag to inspect parsing:

```bash
# Show S-expressions (parse only)
./target/release/mettatron --sexpr input.metta

# Run with debug output
RUST_LOG=debug ./target/release/mettatron input.metta

# Interactive debugging with REPL
./target/release/mettatron --repl
```

### Common Tasks

**Add a new grounded function:**
1. Update `eval_grounded()` in `src/backend/eval.rs`
2. Add pattern match for the function name
3. Implement the operation logic
4. Add tests in `src/backend/eval.rs`

**Add a new special form:**
1. Update `eval_special_form()` in `src/backend/eval.rs`
2. Add pattern match for the form name
3. Implement the evaluation semantics
4. Add tests

**Add a new operator token:**
1. Update `Lexer` in `src/sexpr.rs` to recognize the token
2. Update `Parser` to handle prefix/infix positioning
3. Map to appropriate function name in `compile()` if needed

**Modify type system:**
- Type assertions: `src/backend/types.rs` (`Environment` struct)
- Type inference: `src/backend/eval.rs` (`infer_type()` function)
- Type checking: `src/backend/eval.rs` (`check_type_match()` function)

## Examples

### MeTTa Language Examples

- `examples/simple.metta` - Basic language features
- `examples/advanced.metta` - Advanced patterns
- `examples/mvp_test.metta` - MVP feature tests
- `examples/type_system_demo.metta` - Type system demonstrations
- `examples/pathmap_demo.metta` - PathMap operations

### Rust Backend Examples

- `examples/backend_usage.rs` - Direct backend API usage
- `examples/backend_interactive.rs` - Interactive REPL implementation
- `examples/mvp_complete.rs` - Complete MVP demonstration

### Rholang Integration

- `examples/metta_rholang_example.rho` - Using MeTTa from Rholang

Run examples:
```bash
# MeTTa examples
./target/release/mettatron examples/simple.metta

# Rust examples
cargo run --example backend_usage
cargo run --example backend_interactive
```

## Threading and Parallelization

MeTTaTron supports parallel evaluation of independent MeTTa expressions using Tokio's async runtime.

### Configuration

```rust
use mettatron::config::{EvalConfig, configure_eval};

// Configure before any async operations (call once at startup)
configure_eval(EvalConfig::cpu_optimized());
```

### Preset Configurations

- **`EvalConfig::default()`** - Default Tokio settings (512 max blocking threads)
- **`EvalConfig::cpu_optimized()`** - Best for CPU-bound workloads (num_cpus × 2)
- **`EvalConfig::memory_optimized()`** - Best for memory-constrained systems (num_cpus)
- **`EvalConfig::throughput_optimized()`** - Best for high-throughput batch processing (1024 threads)

### Custom Configuration

```rust
configure_eval(EvalConfig {
    max_blocking_threads: 256,    // Max parallel evaluations
    batch_size_hint: 64,          // Batch size for consecutive evals
});
```

### Threading Model

MeTTaTron uses **two thread pools** managed by Rholang's Tokio runtime:

1. **Async Executor Threads** (Tokio default)
   - Handles async coordination and I/O
   - Fixed by Tokio (~num_cpus threads)

2. **Blocking Thread Pool** (Configurable)
   - Handles CPU-intensive MeTTa evaluation
   - Configurable via `max_blocking_threads`
   - Prevents starving async executor

**Key Point**: Both pools are coordinated by the **same Tokio runtime**, ensuring optimal resource management without contention.

For detailed information, see `docs/THREADING_MODEL.md`.

## Code Organization

```
src/
├── main.rs                  # CLI and REPL implementation
├── lib.rs                   # Public API exports
├── config.rs                # Threading configuration
├── sexpr.rs                 # Lexer and S-expression parser
├── backend/                 # Evaluation engine
│   ├── mod.rs              # Module exports
│   ├── types.rs            # MettaValue, Environment, Rule
│   ├── compile.rs          # MeTTa source → MettaValue
│   └── eval.rs             # Lazy evaluation engine
├── rholang_integration.rs   # Rholang integration API (sync & async)
├── pathmap_par_integration.rs  # PathMap Par conversion
└── ffi.rs                   # C FFI layer
```

## Style Guidelines

- Use `cargo fmt` for formatting
- Fix `cargo clippy` warnings
- Avoid warnings in release builds
- Add tests for new features
- Document public API with doc comments
