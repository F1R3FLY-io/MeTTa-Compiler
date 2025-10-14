# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

This is a compiler from MeTTa to Rholang. MeTTa is a language with LISP-like S-expression syntax supporting type assertions, rules, pattern matching, and concurrent/sequential expressions. The compiler uses direct S-expression parsing (no BNFC) and is implemented in pure Rust.

## Build System

### Main Build Command

```bash
cargo build --release
```

The compiled binary will be at `./target/release/mettatron`

### Running the Compiler

```bash
# Compile MeTTa to Rholang
./target/release/mettatron input.metta

# Write to file
./target/release/mettatron input.metta -o output.rho

# Show AST
./target/release/mettatron --ast input.metta

# Show S-expressions
./target/release/mettatron --sexpr input.metta
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

### Compilation Pipeline

```
MeTTa Source → Tokens → S-expressions → MeTTa AST → Rholang Code
```

The compiler consists of three main stages:

1. **Lexical Analysis & S-expression Parsing** (`src/sexpr.rs` ~450 lines)
   - Tokenizes input text into structured tokens
   - Parses tokens into S-expressions
   - Handles comments: `//`, `/* */`, `;`
   - Supports special operators: `<-`, `<=`, `<<-`, `?!`, `!?`, `...`

2. **MeTTa AST Construction** (`src/parser.rs` ~300 lines)
   - Converts S-expressions to typed MeTTa AST
   - Validates MeTTa language constructs
   - Builds type assertions, rules, expressions

3. **Rholang Code Generation** (`src/compiler.rs` ~1000 lines)
   - Translates MeTTa AST to Rholang
   - Manages variable scoping and fresh name generation
   - Implements compilation strategies for each construct

### Key File Locations

- **Library exports**: `src/lib.rs`
- **CLI implementation**: `src/main.rs`
- **Lexer/S-expr parser**: `src/sexpr.rs`
- **MeTTa parser**: `src/parser.rs`
- **Rholang compiler**: `src/compiler.rs`
- **Project config**: `Cargo.toml`
- **Examples**: `examples/*.metta`

## MeTTa Language Features

The MeTTa grammar supports:

- **Programs**: Colonies containing knowledge base entries (`!`), queries (`?`), or expressions
- **Types**: Arrow types `(-> Type ...)`, ground types (Bool, String, Long, URI), type assertions `(: expr type)`
- **Expressions**:
  - Rules: `(= pattern body)`
  - Atom operations: `add-atom`, `rem-atom`, `transform`
  - Pattern matching: `(match space pattern template)`
  - Binding: `(bind! var expr)`
  - Sequential: `(expr1 expr2 ...)`
  - Concurrent: `{expr1 expr2 ...}`
- **Atoms**: Grounded values, builtins, variables, wildcard `_`
- **Ground types**: Booleans (`true`, `false`), integers, strings, URIs
- **Builtins**: Arithmetic (`+`, `-`, `*`, `/`) and comparison (`<`, `<=`, `>`, `>=`, `==`)

### Lexical Tokens

- **Variables**: `$x`, `&y`, `'z` (start with `$`, `&`, or `'`)
- **Strings**: `"hello"` (double-quoted with escapes)
- **URIs**: `` `uri` `` (backtick-quoted)
- **Integers**: `42`, `-10`
- **Comments**: `//`, `/* */`, `;`

## Rholang Compilation Strategy

The compiler translates MeTTa constructs to Rholang as follows:

| MeTTa Construct | Rholang Translation |
|-----------------|---------------------|
| Rules | Contracts with pattern params and return channels |
| Sequential | Chained `for` comprehensions with ack channels |
| Concurrent | Parallel composition with `\|` operator |
| Variables | Unforgeable names, persistent sends `!!` |
| Pattern matching | `for(@pattern <- channel)` bindings |
| Type assertions | Comments (type information preserved) |
| Knowledge base | Contracts deployed with `contract name(_)` |
| Queries | Execute with result channel |

### Key Compilation Details

- **Variable Sanitization**: `$x` → `x`, `$foo-bar` → `foo_bar`
- **Fresh Names**: Generated as `prefix_N` where N increments
- **Sequential Ordering**: Uses ack channels to ensure execution order
- **Concurrent Execution**: Uses `|` operator for parallel composition
- **Contract Parameters**: Use `@` binding syntax for pattern matching
- **Return Channels**: Rules compile to contracts with explicit return channels

## Working with the Codebase

### Adding New Features

1. **New MeTTa syntax**: Update `src/sexpr.rs` (tokens) and `src/parser.rs` (AST)
2. **New Rholang output**: Update `src/compiler.rs` (code generation)
3. **New CLI options**: Update `src/main.rs`

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

### Debugging Compilation

Use the `--ast` and `--sexpr` flags to inspect intermediate stages:

```bash
# Show S-expressions
./target/release/mettatron --sexpr input.metta

# Show MeTTa AST
./target/release/mettatron --ast input.metta

# Show Rholang output
./target/release/mettatron input.metta
```

### Common Tasks

**Add a new builtin operator:**
1. Add token in `src/sexpr.rs` if needed
2. Add to `Builtin` enum in `src/parser.rs`
3. Add case in `compile_builtin()` in `src/compiler.rs`

**Add a new MeTTa expression form:**
1. Add variant to `MettaExpr` enum in `src/parser.rs`
2. Add parser case in `parse_expr()` in `src/parser.rs`
3. Add compilation case in `compile_expr()` in `src/compiler.rs`

**Modify Rholang output:**
- All code generation is in `src/compiler.rs`
- Use `CompileContext` for variable management
- Use `fresh_var()` for generating unique names
- Use `sanitize_var_name()` for cleaning variable names

## Examples

Example files in `examples/`:

- `examples/simple.metta` - Basic language features
- `examples/advanced.metta` - Advanced patterns

Test with:
```bash
cargo run -- examples/simple.metta
```

## Legacy BNFC Parser

The `parser/` directory contains the original BNFC-based implementation using C2Rust transpilation. This is kept for reference only and is NOT used by the current compiler.

To build the legacy parser (requires BNFC, C2Rust, flex, bison, gcc, bear):
```bash
./build_rust_parser
```

## Code Organization

```
src/
├── main.rs        # CLI: argument parsing, file I/O
├── lib.rs         # Public API and convenience functions
├── sexpr.rs       # Lexer (Lexer) + S-expr parser (Parser)
├── parser.rs      # MeTTa AST types + parser (MettaParser)
└── compiler.rs    # Rholang compiler (RholangCompiler)
```

## Style Guidelines

- Use `cargo fmt` for formatting
- Fix `cargo clippy` warnings
- Avoid warnings in release builds
- Add tests for new features
- Document public API with doc comments
