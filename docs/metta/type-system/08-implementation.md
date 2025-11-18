# Implementation Details

## Abstract

This document covers implementation details of MeTTa's type system from hyperon-experimental.

## Core Files

### Main Type System

**lib/src/metta/types.rs** (1320 lines):
- Type inference: `get_atom_types()` (lines 327-410)
- Type checking: `check_type()` (lines 602-639)
- Type unification: `match_reducted_types()` (lines 563-567)
- Subtyping: `query_super_types()`, `add_super_types()` (lines 34-63)

### Runtime Checking

**lib/src/metta/interpreter.rs**:
- Integration point: lines 1126-1159
- Function type checking: lines 1161-1336

### Type Operations

**lib/src/metta/runner/stdlib/atom.rs**:
- `GetTypeOp`: lines 358-380
- `GetTypeSpaceOp`: lines 382-403
- `GetMetaTypeOp`: lines 405-447

## Data Structures

### AtomType

**Location**: `lib/src/metta/types.rs:170-298`

```rust
pub struct AtomType {
    typ: Atom,           // The type
    is_function: bool,   // Cached function type check
    info: TypeInfo,      // Value/Application/Error
}

enum TypeInfo {
    Application,
    ApplicationError { error: Atom },
    Value,
}
```

### Key Methods

- `AtomType::undefined()` - Creates `%Undefined%` type
- `AtomType::value(Atom)` - Type from direct assignment
- `AtomType::application(Atom)` - Type from function application
- `AtomType::error(Atom, Atom)` - Type error

## Algorithms

### Type Inference (Simplified)

```rust
fn get_atom_types(space: &DynSpace, atom: &Atom) -> Vec<AtomType> {
    match atom {
        Atom::Variable(_) => vec![],  // No type constraint
        Atom::Grounded(gnd) => vec![AtomType::value(gnd.type_())],
        Atom::Symbol(sym) => query_types_from_space(space, sym),
        Atom::Expression(expr) if expr.children().is_empty() =>
            vec![AtomType::undefined()],
        Atom::Expression(expr) => {
            let op_types = get_atom_types(space, &expr.children()[0]);
            let arg_types = expr.children()[1..].iter()
                .map(|a| get_atom_types(space, a))
                .collect();
            compute_application_types(op_types, arg_types)
        }
    }
}
```

### Type Checking (Simplified)

```rust
fn check_if_function_type_is_applicable(
    fn_type: &Atom,
    arg_types: &[Vec<AtomType>],
) -> Result<Atom, Atom> {
    let (param_types, ret_type) = extract_function_parts(fn_type)?;
    
    if param_types.len() != arg_types.len() {
        return Err(error_incorrect_arg_count());
    }
    
    for (i, (expected, actual)) in param_types.iter()
        .zip(arg_types.iter()).enumerate() {
        if !can_unify(expected, actual) {
            return Err(error_bad_arg_type(i, expected, actual));
        }
    }
    
    Ok(instantiate_return_type(ret_type, bindings))
}
```

## Performance Characteristics

| Operation | Complexity | Notes |
|-----------|-----------|-------|
| get_atom_types | O(annotations) | Queries space |
| check_type | O(types × unification) | May be expensive |
| get_metatype | O(1) | Simple pattern match |
| Subtype lookup | O(depth) | Transitive closure |

## See Also

- **§02**: Type checking
- **§03**: Type operations
- Source code in hyperon-experimental

---

**Version**: 1.0
**Last Updated**: 2025-11-13
