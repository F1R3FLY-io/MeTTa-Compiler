# Package Manifest Formats Specification

This document specifies the two package manifest formats supported by MeTTaTron.

## Overview

MeTTaTron supports two package manifest formats:

| Format | File | Description | Priority |
|--------|------|-------------|----------|
| MeTTa Atom-Serde | `_pkg-info.metta` | HE-compatible S-expression format | Primary |
| TOML | `metta.toml` | Standard TOML format | Fallback |

When both files exist in a directory, `_pkg-info.metta` takes precedence.

## MeTTa Atom-Serde Format (`_pkg-info.metta`)

### Format Design

The Atom-Serde format uses `#`-prefixed symbols as keys, following MeTTa Hyperon Experimental (HE) conventions. This format enables:

- **Native MeTTa parsing**: Uses the same parser as MeTTa code
- **HE compatibility**: Aligns with emerging MeTTa ecosystem standards
- **Self-describing**: Manifest is valid MeTTa code

### Syntax

```metta
; Required: Package metadata
(#package
    (#name "package-name")
    (#version "1.0.0")
    ; Optional fields follow
)

; Optional: Dependencies
(#dependencies
    (#dep-name "version-constraint")
)

; Optional: Exports
(#exports
    (#public (symbol1 symbol2))
)
```

### Package Section (`#package`)

The `#package` section is **required** and must contain at minimum:

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `#name` | String | Yes | Package name |
| `#version` | String | Yes | Semantic version (e.g., "1.0.0") |
| `#description` | String | No | Package description |
| `#authors` | List | No | Author names/emails |
| `#license` | String | No | License identifier |
| `#repository` | String | No | Repository URL |
| `#documentation` | String | No | Documentation URL |
| `#homepage` | String | No | Homepage URL |
| `#keywords` | List | No | Discovery keywords |
| `#categories` | List | No | Classification categories |

**Example:**

```metta
(#package
    (#name "my-awesome-lib")
    (#version "2.1.0")
    (#description "A comprehensive utility library for MeTTa")
    (#authors ("Alice Developer <alice@example.com>" "Bob Contributor"))
    (#license "Apache-2.0")
    (#repository "https://github.com/alice/my-awesome-lib")
    (#keywords ("utilities" "metta" "helpers"))
    (#categories ("library" "utilities"))
)
```

### Dependencies Section (`#dependencies`)

The `#dependencies` section is optional and specifies package dependencies.

#### Simple Version Constraint

```metta
(#dependencies
    (#std "^1.0")
    (#core "~2.0")
    (#exact-dep "=1.0.0")
    (#minimum ">=1.5.0")
)
```

#### Path Dependency

```metta
(#dependencies
    (#local-lib (#path "../local-lib"))
    (#shared (#path "/opt/shared/metta-lib"))
)
```

#### Git Dependency

```metta
(#dependencies
    ; Tag reference
    (#external (#git "https://github.com/user/repo" #tag "v1.0.0"))

    ; Branch reference
    (#dev-lib (#git "https://github.com/user/dev" #branch "develop"))

    ; Commit reference
    (#pinned (#git "https://github.com/user/stable" #rev "abc123def"))
)
```

#### Complex Dependency

```metta
(#dependencies
    ; Version with features
    (#featured-lib (#version "1.0" #features ("feature1" "feature2")))

    ; Optional dependency
    (#optional-lib (#version "1.0" #optional True))
)
```

### Exports Section (`#exports`)

The `#exports` section controls symbol visibility.

#### Public Symbol List

```metta
(#exports
    (#public (function1 function2 TypeA TypeB CONSTANT))
)
```

#### Export All

```metta
(#exports
    (#all True)
)
```

#### Default Behavior

When no `#exports` section is present, the package uses **closed-by-default** semantics:
- `#all` defaults to `False`
- `#public` defaults to empty list
- No symbols are automatically exported

### Complete Example

```metta
; _pkg-info.metta - Full featured example

(#package
    (#name "metta-utilities")
    (#version "1.2.3")
    (#description "Common utilities for MeTTa development")
    (#authors ("Core Team <team@example.com>"))
    (#license "MIT")
    (#repository "https://github.com/example/metta-utilities")
    (#documentation "https://docs.example.com/metta-utilities")
    (#homepage "https://example.com/metta-utilities")
    (#keywords ("metta" "utilities" "helpers" "common"))
    (#categories ("library" "utilities"))
)

(#dependencies
    ; Standard library
    (#std "^1.0")

    ; Local development library
    (#dev-helpers (#path "../dev-helpers"))

    ; External git dependency
    (#metta-core (#git "https://github.com/metta/core" #tag "v2.0.0"))
)

(#exports
    (#public (
        ; Functions
        process-data
        transform-value
        validate-input

        ; Types
        DataType
        ResultType

        ; Constants
        DEFAULT_TIMEOUT
        MAX_RETRIES
    ))
)
```

## TOML Format (`metta.toml`)

### Syntax

Standard TOML format with three main sections:

```toml
[package]
name = "package-name"
version = "1.0.0"

[dependencies]
dep-name = "version-constraint"

[exports]
public = ["symbol1", "symbol2"]
```

### Package Section

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `name` | string | Yes | Package name |
| `version` | string | Yes | Semantic version |
| `description` | string | No | Package description |
| `authors` | array | No | Author list |
| `license` | string | No | License identifier |
| `repository` | string | No | Repository URL |
| `documentation` | string | No | Documentation URL |
| `homepage` | string | No | Homepage URL |
| `keywords` | array | No | Discovery keywords |
| `categories` | array | No | Classification categories |

### Dependencies Section

#### Simple Version

```toml
[dependencies]
std = "^1.0"
core = "~2.0"
```

#### Detailed Specification

```toml
[dependencies]
local-lib = { path = "../local-lib" }
external = { git = "https://github.com/user/repo", tag = "v1.0" }
featured = { version = "1.0", features = ["feature1", "feature2"] }
optional = { version = "1.0", optional = true }
```

### Exports Section

```toml
[exports]
public = ["function1", "function2", "TypeA"]
all = false  # default
```

### Complete Example

```toml
# metta.toml - Full featured example

[package]
name = "metta-utilities"
version = "1.2.3"
description = "Common utilities for MeTTa development"
authors = ["Core Team <team@example.com>"]
license = "MIT"
repository = "https://github.com/example/metta-utilities"
documentation = "https://docs.example.com/metta-utilities"
homepage = "https://example.com/metta-utilities"
keywords = ["metta", "utilities", "helpers", "common"]
categories = ["library", "utilities"]

[dependencies]
std = "^1.0"
dev-helpers = { path = "../dev-helpers" }
metta-core = { git = "https://github.com/metta/core", tag = "v2.0.0" }

[exports]
public = [
    "process-data",
    "transform-value",
    "validate-input",
    "DataType",
    "ResultType",
    "DEFAULT_TIMEOUT",
    "MAX_RETRIES"
]
```

## Version Constraints

Both formats support the same version constraint syntax:

| Constraint | Meaning | Matches |
|------------|---------|---------|
| `"1.0.0"` | Exact match | Only 1.0.0 |
| `"=1.0.0"` | Exact match | Only 1.0.0 |
| `"^1.0"` | Compatible | 1.0.0, 1.1.0, 1.9.9 (not 2.0.0) |
| `"~1.0"` | Approximately | 1.0.0, 1.0.9 (not 1.1.0) |
| `">=1.0"` | Greater or equal | 1.0.0 and above |
| `">1.0"` | Greater than | Above 1.0.0 |
| `"<2.0"` | Less than | Below 2.0.0 |
| `"<=2.0"` | Less or equal | 2.0.0 and below |

## Format Comparison

| Feature | `_pkg-info.metta` | `metta.toml` |
|---------|-------------------|--------------|
| Native MeTTa syntax | Yes | No |
| HE compatibility | Yes | No |
| Tooling support | MeTTa parsers | Standard TOML |
| Human readability | Good | Excellent |
| Error messages | MeTTa parser errors | TOML parser errors |
| Precedence | Primary | Fallback |

## Migration Guide

### From `metta.toml` to `_pkg-info.metta`

1. Convert TOML sections to S-expressions with `#` prefixes
2. Replace `[]` arrays with `()` lists
3. Replace `{ key = value }` with `(#key value)`
4. Convert quoted strings as-is

**Before (`metta.toml`):**
```toml
[package]
name = "my-pkg"
version = "1.0.0"

[exports]
public = ["func1", "func2"]
```

**After (`_pkg-info.metta`):**
```metta
(#package
    (#name "my-pkg")
    (#version "1.0.0")
)

(#exports
    (#public (func1 func2))
)
```

## Error Handling

### Parse Errors in `_pkg-info.metta`

When `_pkg-info.metta` exists but contains errors:

1. A warning is logged with the error details
2. The system falls back to `metta.toml`
3. If `metta.toml` also fails or doesn't exist, no manifest is loaded

### Required Field Validation

Both formats require:
- `name` in package section
- `version` in package section

Missing required fields result in parse errors.

## API Usage

### Rust API

```rust
use mettatron::backend::modules::{PackageInfo, load_pkg_info_metta, parse_pkg_info_metta};

// Load from directory (auto-detects format with precedence)
let pkg = PackageInfo::load(&module_dir);

// Load _pkg-info.metta specifically
let result = load_pkg_info_metta(&module_dir);

// Parse _pkg-info.metta content directly
let pkg = parse_pkg_info_metta(content)?;

// Load metta.toml specifically
let pkg = PackageInfo::load_from_toml_path(&path);
```
