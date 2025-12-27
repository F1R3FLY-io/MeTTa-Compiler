# MeTTaTron Module System Guide

This guide covers the MeTTaTron module system, which provides file inclusion, module imports, token binding, and package management.

## Table of Contents

1. [Overview](#overview)
2. [Basic Operations](#basic-operations)
3. [Module Inclusion](#module-inclusion)
4. [Module Imports](#module-imports)
5. [Token Binding](#token-binding)
6. [Package Management](#package-management)
7. [Strict Mode](#strict-mode)
8. [Best Practices](#best-practices)

## Overview

The MeTTaTron module system enables:

- **Code organization**: Split code across multiple files
- **Code reuse**: Share functions and types between modules
- **Encapsulation**: Control what symbols are public via package manifests
- **Dependency management**: Track and control transitive dependencies
- **Package management**: Supports both HE-compatible `_pkg-info.metta` and TOML `metta.toml` manifest formats

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

## Package Management

MeTTaTron supports two package manifest formats:

1. **`_pkg-info.metta`** - HE-compatible Atom-Serde format (preferred)
2. **`metta.toml`** - TOML-based format (MeTTaTron native)

When both files exist, `_pkg-info.metta` takes precedence.

### MeTTa Manifest Format (`_pkg-info.metta`)

The HE-compatible format uses `#`-prefixed symbols as keys in S-expressions:

```metta
; Package metadata (required section)
(#package
    (#name "my-package")
    (#version "1.0.0")
    (#description "A useful MeTTa package")
    (#authors ("Author Name" "Contributor"))
    (#license "MIT")
    (#repository "https://github.com/user/repo")
    (#keywords ("metta" "utility"))
)

; Dependencies (optional section)
(#dependencies
    ; Simple version constraint
    (#other-package "^1.0")

    ; Path dependency
    (#local-lib (#path "../local-lib"))

    ; Git dependency
    (#external (#git "https://github.com/user/external" #tag "v2.0"))
)

; Exports (optional section)
(#exports
    ; List of public symbols
    (#public (function1 function2 MyType))

    ; Or export all symbols
    ; (#all True)
)
```

### TOML Manifest Format (`metta.toml`)

```toml
[package]
name = "my-package"
version = "1.0.0"
description = "A useful MeTTa package"
authors = ["Author Name"]
license = "MIT"
repository = "https://github.com/user/repo"

[exports]
public = ["function1", "function2", "MyType"]

[dependencies]
other-package = "^1.0"
local-lib = { path = "../local-lib" }
external = { git = "https://github.com/user/external", tag = "v2.0" }
```

### Manifest Sections

#### Package Section
- `name`: Package name (required)
- `version`: Semantic version (required)
- `description`: Package description
- `authors`: List of authors
- `license`: License identifier (e.g., "MIT", "Apache-2.0")
- `repository`: Repository URL
- `documentation`: Documentation URL
- `homepage`: Homepage URL
- `keywords`: List of keywords for discovery
- `categories`: List of categories

#### Exports Section
- `public`: List of publicly exported symbols
- `all`: Boolean to export all symbols (default: `false`)

When no exports section is present, symbols are not automatically exported (closed by default).

#### Dependencies Section
- Package dependencies with version constraints
- Supports simple version strings, path references, and git references

### Version Constraints

| Constraint | Meaning | Example |
|------------|---------|---------|
| `"1.0.0"` | Exact version | Only 1.0.0 |
| `"^1.0"` | Compatible with | 1.0.x, 1.1.x, etc. |
| `"~1.0"` | Approximately | ~> 1.0.x only |
| `">=1.0"` | Greater or equal | 1.0.0 and above |
| `"=1.0.0"` | Exactly | Only 1.0.0 |

### Format Precedence

When loading a package manifest, MeTTaTron checks in this order:

1. `_pkg-info.metta` - HE-compatible format (if exists and valid)
2. `metta.toml` - TOML format (fallback)

If `_pkg-info.metta` exists but has parse errors, the system logs a warning and falls back to `metta.toml`.

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

Declare public symbols in your manifest (`_pkg-info.metta` preferred):

```metta
; In _pkg-info.metta
(#exports
    (#public (process-data DataType))
)
```

Or in `metta.toml`:

```toml
[exports]
public = ["process-data", "DataType"]
```

Then document them in your module:

```metta
; Public API:
; - process-data: Main data processing function
; - DataType: Type for processed data

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

; Avoid
(= (calc $x) ...)
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

### Symbol Not Found

When a selective import fails:
```
Error: import!: item 'func' not found in module
```

Solutions:
- Check spelling of symbol name
- Verify the module path is correct
- Ensure the symbol is declared in the module's exports section:
  - In `_pkg-info.metta`: `(#exports (#public (func)))`
  - In `metta.toml`: `public = ["func"]`

### Manifest Parse Error

When `_pkg-info.metta` has syntax errors:
```
Warning: Failed to parse _pkg-info.metta: ...
```

Solutions:
- Check for missing `#package` section
- Ensure `#name` and `#version` fields are present
- Verify S-expression syntax is correct
- The system will fall back to `metta.toml` if available

---

For more information, see:
- [BUILTIN_FUNCTIONS_REFERENCE.md](../reference/BUILTIN_FUNCTIONS_REFERENCE.md)
- [BACKEND_API_REFERENCE.md](../reference/BACKEND_API_REFERENCE.md)
- [MANIFEST_FORMATS.md](MANIFEST_FORMATS.md) - Detailed manifest format specification
