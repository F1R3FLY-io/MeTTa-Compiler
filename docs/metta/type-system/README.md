# MeTTa Type System Documentation

## Overview

This directory contains comprehensive documentation of MeTTa's type system based on the hyperon-experimental reference implementation (commit `164c22e9`).

## Documentation Status

### Completed

- ✅ **00-overview.md** - Comprehensive executive summary covering all type system aspects
- ✅ **01-fundamentals.md** - Detailed coverage of type syntax, built-in types, constructors, and polymorphism

### Planned

The following documents are planned to provide additional depth:

- **02-type-checking.md** - Type checking mechanisms, inference, and pragmas
- **03-type-operations.md** - Runtime type operations (get-type, check-type, etc.)
- **04-gradual-typing.md** - Gradual typing with %Undefined%
- **05-dependent-types.md** - Dependent types and type-level computation
- **06-advanced-features.md** - Higher-kinded types, meta-types, subtyping
- **07-evaluation-interaction.md** - How types affect evaluation order
- **08-implementation.md** - Implementation details
- **09-type-errors.md** - Error handling and debugging
- **10-formal-semantics.md** - Formal type rules and soundness
- **11-comparisons.md** - Comparisons with other type systems
- **examples/*.metta** - Executable examples

## Current Documentation Coverage

The **completed documents provide comprehensive coverage**:

### 00-overview.md (Executive Summary)
- Type system characteristics and philosophy
- All built-in types
- Type syntax and notation
- Quick reference guide
- Error handling overview
- Performance considerations
- Comparison with other languages
- Complete FAQ

### 01-fundamentals.md (Detailed Foundation)
- Complete type syntax reference
- All built-in types with detailed explanations
- Type constructors and parameterized types
- Polymorphism and type variables
- Type unification
- Extensive examples for each concept
- Best practices and naming conventions

## Quick Start

1. **New to MeTTa Types?** Start with **00-overview.md**
2. **Need details on syntax?** See **01-fundamentals.md**
3. **Want to understand gradual typing?** See overview §"Gradual/Optional Typing"
4. **Looking for examples?** Both documents include extensive examples

## Key Resources

### From hyperon-experimental

**Source Code**:
- `lib/src/metta/types.rs` - Main type system implementation
- `lib/src/metta/interpreter.rs:1126-1336` - Runtime type checking
- `lib/src/metta/runner/stdlib/atom.rs:354-447` - Type operations
- `lib/src/metta/mod.rs:13-32` - Type constants and symbols

**Test Files** (extensive examples):
- `python/tests/scripts/b5_types_prelim.metta` - Type basics (251 lines)
- `python/tests/scripts/d1_gadt.metta` - GADTs
- `python/tests/scripts/d2_higherfunc.metta` - Higher-order functions
- `python/tests/scripts/d3_deptypes.metta` - Dependent types
- `python/tests/scripts/d4_type_prop.metta` - Types as propositions
- `python/tests/scripts/d5_auto_types.metta` - Auto type checking

## Type System Quick Reference

### Basic Syntax

```metta
; Type assignment
(: atom Type)

; Subtyping
(:< SubType SuperType)

; Function types
(-> ArgType... ReturnType)

; Polymorphic types
(: identity (-> $t $t))
```

### Built-in Types

**Core**: %Undefined%, Type, Number, String, Bool

**Structural**: Atom, Symbol, Variable, Expression, Grounded

**Special**: SpaceType, ErrorType

### Enable Type Checking

```metta
!(pragma! type-check auto)
```

### Common Operations

```metta
!(get-type atom)              ; Get type of atom
!(check-type atom Type)       ; Check if atom has type
!(type-cast atom Type &space) ; Cast to type
```

## Documentation Philosophy

This documentation is:
- **Rigorous**: Formal definitions with mathematical notation where appropriate
- **Accessible**: Clear prose explanations alongside formal content
- **Practical**: Extensive examples and usage guidelines
- **Comprehensive**: Covers specification and implementation details
- **Reference-based**: Links to exact source code locations

## For Compiler Implementers

The completed documentation provides:
- Complete specification of type syntax
- All built-in types and their semantics
- Type constructor mechanisms
- Polymorphism and unification
- Gradual typing semantics with %Undefined%

For implementation details, see:
- Overview §"Implementation Summary"
- Source code references throughout documents
- Test files in hyperon-experimental

## For MeTTa Users

### Learning Path

1. **Overview** (00-overview.md) - Understand what MeTTa's type system offers
2. **Fundamentals** (01-fundamentals.md) - Learn syntax and basic concepts
3. **Practice** - Study test files in hyperon-experimental
4. **Advanced** - Explore dependent types and meta-types

### When to Use Types

✅ **Use types for**:
- Library functions and public APIs
- Complex data structures
- Critical code that needs safety
- Documentation

❌ **Skip types for**:
- Quick prototypes
- Simple scripts
- Genuinely dynamic code
- Meta-programming

## Contributing

To extend this documentation:
1. Follow the established format (formal + prose + examples)
2. Reference source code with file:line notation
3. Include executable examples
4. Maintain scientific rigor per project requirements

## Version Information

- **Documentation Version**: 1.0
- **Based on**: hyperon-experimental commit `164c22e9`
- **Date**: 2025-11-13
- **Status**: Core documentation complete, additional depth documents planned

## See Also

- **Order of Operations Documentation**: `../order-of-operations/` - How evaluation order interacts with types
- **hyperon-experimental**: Official MeTTa implementation
- **MeTTa-Compiler**: Compiler project using this documentation

---

**Note**: The two completed documents (overview + fundamentals) provide substantial coverage of MeTTa's type system. They cover all essential concepts needed for understanding and using the type system effectively. Additional planned documents would provide further depth on specific advanced topics.
