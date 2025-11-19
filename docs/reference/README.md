# API and Language Reference

Comprehensive reference documentation for MeTTaTron's APIs and the MeTTa language.

## Backend API

**[BACKEND_API_REFERENCE.md](BACKEND_API_REFERENCE.md)** - Rust Backend API Reference
- Core types: `MettaValue`, `Environment`, `Rule`
- Compilation functions: `compile_metta()`, `parse_and_compile()`
- Evaluation functions: `eval()`, `eval_with_bindings()`, `run_state()`
- Async evaluation: `run_state_async()`
- Environment management
- Complete API examples
- Integration patterns

## Built-in Functions

**[BUILTIN_FUNCTIONS_REFERENCE.md](BUILTIN_FUNCTIONS_REFERENCE.md)** - MeTTa Built-in Functions
- Comprehensive catalog of 147 MeTTa built-in functions
- Implementation status tracker (26 implemented / 121 not implemented)
- Categorized by functionality:
  - Arithmetic operations
  - List operations
  - Space operations
  - Type system operations
  - Control flow
  - Pattern matching
  - Error handling
- Links to official MeTTa reference implementation
- Priority targets for implementation

## Type System

**[METTA_TYPE_SYSTEM_REFERENCE.md](METTA_TYPE_SYSTEM_REFERENCE.md)** - MeTTa Type System Specification
- Type assertions with `:`
- Type inference with `get-type`
- Type checking with `check-type`
- Ground types: `Bool`, `Long`, `Float`, `String`, `URI`, `Nil`
- Expression types and signatures
- Polymorphic type system
- Type compatibility rules
- Comprehensive examples
- Implementation notes
