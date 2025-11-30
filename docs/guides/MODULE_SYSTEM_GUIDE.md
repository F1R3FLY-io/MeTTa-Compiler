# MeTTaTron Module System Guide

This guide covers the MeTTaTron module system, which provides file inclusion, module imports, token binding, export control, and package management.

## Table of Contents

1. [Overview](#overview)
2. [Basic Operations](#basic-operations)
3. [Module Inclusion](#module-inclusion)
4. [Module Imports](#module-imports)
5. [Token Binding](#token-binding)
6. [Export Control](#export-control)
7. [Package Management](#package-management)
8. [Strict Mode](#strict-mode)
9. [Best Practices](#best-practices)

## Overview

The MeTTaTron module system enables:

- **Code organization**: Split code across multiple files
- **Code reuse**: Share functions and types between modules
- **Encapsulation**: Control what symbols are public
- **Dependency management**: Track and control transitive dependencies
- **Package management**: Use TOML manifests for package metadata

## Basic Operations

### include

The `include` operation loads and evaluates a MeTTa file in the current environment.

```metta
; Include a file by absolute path
(include "/path/to/module.metta")

; Include a file by relative path (relative to current module)
(include "utils.metta")
```

**Features:**
- Module caching: Files are only loaded once
- Transitive dependencies: Included files can include other files
- All definitions are added to the current environment

### import!

The `import!` operation provides more control over how modules are loaded.

```metta
; Import all symbols into current space
(import! &self "module.metta")

; Import with namespace alias
(import! mymod "module.metta")

; Selective import (single item)
(import! &self "module.metta" my-function)

; Selective import with alias
(import! &self "module.metta" original-name as new-name)
```

### bind!

The `bind!` operation registers a token for runtime substitution.

```metta
; Bind a simple value
(bind! my-constant 42)

; Bind a computed value
(bind! computed-value (+ 10 20))

; Use the bound token
!(+ my-constant 5)  ; Returns 47
```

**Note:** Bound tokens are resolved during evaluation, making them useful for creating named references to values.

### export!

The `export!` operation marks a symbol as public.

```metta
; Export a function
(= (my-function $x) (+ $x 1))
(export! my-function)

; Export a type
(: MyType Type)
(export! MyType)
```

Exported symbols can be selectively imported by other modules.

### mod-space!

Get a module's space for direct querying.

```metta
; Get module's space
(mod-space! "module.metta")
```

### print-mods!

Print information about all loaded modules.

```metta
(print-mods!)
```

## Module Inclusion

### Creating a Module

A module is simply a `.metta` file containing definitions:

```metta
; math_utils.metta

; Define functions
(= (square $x) (* $x $x))
(= (cube $x) (* $x (square $x)))

; Define types
(: Number Type)
(: square (-> Number Number))

; Export public API
(export! square)
(export! cube)
```

### Using a Module

```metta
; main.metta

; Include the module
(include "math_utils.metta")

; Use the functions
!(square 5)   ; Returns 25
!(cube 3)     ; Returns 27
```

### Transitive Dependencies

Modules can include other modules:

```metta
; module_a.metta
(= (func-a $x) (+ $x 10))

; module_b.metta
(include "module_a.metta")
(= (func-b $x) (+ (func-a $x) 5))

; main.metta
(include "module_b.metta")
!(func-b 1)   ; Returns 16 (1 + 10 + 5)
```

### Module Caching

Modules are cached to prevent multiple loading:

```metta
; Both includes share the same module instance
(include "utils.metta")
(include "utils.metta")  ; No-op, already loaded
```

## Module Imports

### Full Imports

Import all symbols from a module:

```metta
(import! &self "module.metta")
```

### Aliased Imports

Import with a namespace prefix:

```metta
(import! math "math_utils.metta")
; Access via math.square, etc. (if supported)
```

### Selective Imports

Import specific symbols:

```metta
(import! &self "module.metta" square)
```

### Import with Renaming

Rename imported symbols:

```metta
(import! &self "module.metta" square as sq)
; Now use sq instead of square
```

## Token Binding

### Simple Bindings

```metta
(bind! answer 42)
(bind! greeting "Hello")
```

### Computed Bindings

```metta
(bind! pi 3.14159)
(bind! tau (* pi 2))  ; tau = 6.28318
```

### Space Bindings

```metta
; Common pattern: create and bind a space
(bind! &kb (new-space))
(add-atom &kb (fact 1 2 3))
```

## Export Control

### Marking Exports

```metta
; Public function
(= (public-api $x) (internal-helper $x))
(export! public-api)

; Private helper (not exported)
(= (internal-helper $x) (* $x 2))
```

### Export Best Practices

1. Export only the public API
2. Keep implementation details private
3. Document exported symbols
4. Use consistent naming conventions

## Package Management

### Package Manifest

Create a `metta.toml` file in your package root:

```toml
[package]
name = "my-package"
version = "1.0.0"
description = "A useful MeTTa package"
authors = ["Author Name"]

[exports]
public = ["function1", "function2", "MyType"]

[dependencies]
other-package = "^1.0"
```

### Manifest Sections

#### [package]
- `name`: Package name (required)
- `version`: Semantic version (required)
- `description`: Package description
- `authors`: List of authors
- `license`: License identifier
- `repository`: Repository URL

#### [exports]
- `public`: List of exported symbols

#### [dependencies]
- Package dependencies with version constraints

### Version Constraints

```toml
[dependencies]
exact = "1.0.0"        # Exact version
compatible = "^1.0"    # Compatible with 1.x
flexible = "~1.0"      # ~> 1.0.x
range = ">=1.0, <2.0"  # Version range
```

## Strict Mode

Strict mode controls transitive dependency behavior.

### Enabling Strict Mode

From command line:
```bash
mettatron --strict-mode input.metta
```

From code:
```rust
let mut env = Environment::new();
env.set_strict_mode(true);
```

### Effect of Strict Mode

When enabled:
- Transitive imports are disabled
- Each module must explicitly declare its dependencies
- Helps catch missing dependency declarations

When disabled (default):
- Transitive imports are allowed
- Dependencies flow through the module graph
- More permissive, easier to get started

## Best Practices

### 1. Module Organization

```
project/
  metta.toml
  src/
    main.metta
    lib/
      core.metta
      utils.metta
    types/
      basic.metta
```

### 2. Clear Public APIs

```metta
; At the top of your module, document exports
; Public API:
; - process-data: Main data processing function
; - DataType: Type for processed data

(export! process-data)
(export! DataType)

; Implementation follows...
```

### 3. Avoid Circular Dependencies

Structure modules to avoid cycles:
```
common.metta      ; Shared utilities
types.metta       ; Type definitions (uses common)
functions.metta   ; Functions (uses types, common)
main.metta        ; Entry point (uses all)
```

### 4. Use Meaningful Names

```metta
; Good
(= (calculate-total $items) ...)
(export! calculate-total)

; Avoid
(= (calc $x) ...)
(export! calc)
```

### 5. Document Dependencies

In your manifest or module header:
```metta
; Dependencies:
; - math_utils.metta (for arithmetic operations)
; - string_utils.metta (for formatting)

(include "math_utils.metta")
(include "string_utils.metta")
```

## Troubleshooting

### Module Not Found

```
Error: include: failed to read file 'module.metta'
```

Solutions:
- Check file path is correct
- Use absolute path if relative path fails
- Ensure `set_current_module_path` is configured

### Circular Dependency

```
Error: Circular dependency detected
```

Solutions:
- Refactor to break the cycle
- Extract shared code to a common module
- Use forward declarations if supported

### Symbol Not Exported

When a selective import fails:
```
Error: import!: item 'func' not found in module
```

Solutions:
- Ensure the symbol is exported with `(export! func)`
- Check spelling of symbol name
- Verify the module path is correct

---

For more information, see:
- [BUILTIN_FUNCTIONS_REFERENCE.md](../reference/BUILTIN_FUNCTIONS_REFERENCE.md)
- [BACKEND_API_REFERENCE.md](../reference/BACKEND_API_REFERENCE.md)
