# Rholang Integration Guide

## Overview

This document describes how to expose the MeTTa `compile` function to Rholang through its system process registry. The integration allows Rholang contracts to compile MeTTa source code and receive the compiled result.

## Architecture

### Integration Points

1. **MeTTa Compiler** (`metta-compiler` crate)
   - Provides the `compile(src: &str)` function
   - Returns `(Vec<MettaValue>, Environment)`

2. **Rholang System Processes** (`f1r3node/rholang/src/rust/interpreter/system_processes.rs`)
   - Hosts the handler function that calls MeTTa compiler
   - Converts between Rholang `Par` types and MeTTa types

3. **Rholang Registry** (`f1r3node/rholang/src/rust/interpreter/registry/`)
   - Registers the MeTTa compile handler
   - Provides the fixed channel for accessing the compiler

## Implementation Plan

### Step 1: Add Rholang Integration Module to MeTTa Compiler

Create `src/rholang_integration.rs` in the MeTTa compiler:

```rust
/// Rholang integration module
/// Provides conversion between MeTTa types and Rholang Par types

use crate::backend::types::MettaValue;
use std::collections::HashMap;

/// Convert MettaValue to a JSON-like string representation
/// This can be parsed by Rholang to reconstruct the value
pub fn metta_value_to_rholang_string(value: &MettaValue) -> String {
    match value {
        MettaValue::Atom(s) => format!(r#"{{"type":"atom","value":"{}"}}"#, escape_json(s)),
        MettaValue::Bool(b) => format!(r#"{{"type":"bool","value":{}}}"#, b),
        MettaValue::Long(n) => format!(r#"{{"type":"number","value":{}}}"#, n),
        MettaValue::String(s) => format!(r#"{{"type":"string","value":"{}"}}"#, escape_json(s)),
        MettaValue::Uri(s) => format!(r#"{{"type":"uri","value":"{}"}}"#, escape_json(s)),
        MettaValue::Nil => r#"{"type":"nil"}"#.to_string(),
        MettaValue::SExpr(items) => {
            let items_json: Vec<String> = items.iter()
                .map(|v| metta_value_to_rholang_string(v))
                .collect();
            format!(r#"{{"type":"sexpr","items":[{}]}}"#, items_json.join(","))
        }
        MettaValue::Error(msg, details) => {
            format!(
                r#"{{"type":"error","message":"{}","details":{}}}"#,
                escape_json(msg),
                metta_value_to_rholang_string(details)
            )
        }
        MettaValue::Type(t) => {
            format!(r#"{{"type":"metatype","value":{}}}"#, metta_value_to_rholang_string(t))
        }
    }
}

/// Escape JSON special characters
fn escape_json(s: &str) -> String {
    s.replace('\\', r"\\")
        .replace('"', r#"\""#)
        .replace('\n', r"\n")
        .replace('\r', r"\r")
        .replace('\t', r"\t")
}

/// Compile MeTTa source and return JSON string representation
pub fn compile_to_json(src: &str) -> Result<String, String> {
    let (exprs, _env) = crate::backend::compile::compile(src)?;

    let exprs_json: Vec<String> = exprs.iter()
        .map(|expr| metta_value_to_rholang_string(expr))
        .collect();

    Ok(format!(r#"{{"success":true,"exprs":[{}]}}"#, exprs_json.join(",")))
}

/// Compile MeTTa source and return error JSON on failure
pub fn compile_safe(src: &str) -> String {
    match compile_to_json(src) {
        Ok(json) => json,
        Err(e) => format!(r#"{{"success":false,"error":"{}"}}"#, escape_json(&e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compile_simple() {
        let src = "(+ 1 2)";
        let result = compile_safe(src);
        assert!(result.contains(r#""success":true"#));
        assert!(result.contains(r#""type":"sexpr""#));
    }

    #[test]
    fn test_compile_error() {
        let src = "(unclosed";
        let result = compile_safe(src);
        assert!(result.contains(r#""success":false"#));
        assert!(result.contains(r#""error""#));
    }

    #[test]
    fn test_metta_value_conversion() {
        let value = MettaValue::Atom("test".to_string());
        let json = metta_value_to_rholang_string(&value);
        assert_eq!(json, r#"{"type":"atom","value":"test"}"#);

        let value = MettaValue::Long(42);
        let json = metta_value_to_rholang_string(&value);
        assert_eq!(json, r#"{"type":"number","value":42}"#);
    }
}
```

### Step 2: Add C-Compatible FFI Layer

For easier integration with Rholang's Rust code, add `src/ffi.rs`:

```rust
/// FFI layer for Rholang integration
/// Provides C-compatible functions for calling from Rholang

use std::ffi::{CStr, CString};
use std::os::raw::c_char;

/// Compile MeTTa source code and return JSON result
///
/// # Safety
/// - src_ptr must be a valid null-terminated C string
/// - The returned pointer must be freed using metta_free_string
#[no_mangle]
pub unsafe extern "C" fn metta_compile(src_ptr: *const c_char) -> *mut c_char {
    if src_ptr.is_null() {
        let error = r#"{"success":false,"error":"null pointer provided"}"#;
        return CString::new(error).unwrap().into_raw();
    }

    let src = match CStr::from_ptr(src_ptr).to_str() {
        Ok(s) => s,
        Err(_) => {
            let error = r#"{"success":false,"error":"invalid UTF-8"}"#;
            return CString::new(error).unwrap().into_raw();
        }
    };

    let result = crate::rholang_integration::compile_safe(src);
    CString::new(result).unwrap().into_raw()
}

/// Free a string allocated by metta_compile
///
/// # Safety
/// - ptr must be a pointer returned by metta_compile
/// - ptr must not be used after calling this function
#[no_mangle]
pub unsafe extern "C" fn metta_free_string(ptr: *mut c_char) {
    if !ptr.is_null() {
        drop(CString::from_raw(ptr));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_ffi_compile() {
        let src = CString::new("(+ 1 2)").unwrap();
        unsafe {
            let result_ptr = metta_compile(src.as_ptr());
            assert!(!result_ptr.is_null());

            let result = CStr::from_ptr(result_ptr).to_str().unwrap();
            assert!(result.contains(r#""success":true"#));

            metta_free_string(result_ptr);
        }
    }
}
```

### Step 3: Update lib.rs

Add the new modules to `src/lib.rs`:

```rust
pub mod rholang_integration;
pub mod ffi;
```

### Step 4: Add Handler to Rholang system_processes.rs

In `f1r3node/rholang/src/rust/interpreter/system_processes.rs`, add:

```rust
/// MeTTa compiler handler
/// Compiles MeTTa source code and returns the result as a JSON string
pub async fn metta_compile(
    &self,
    contract_args: (Vec<ListParWithRandom>, bool, Vec<Par>),
) -> Result<Vec<Par>, InterpreterError> {
    // Unpack contract arguments
    let Some((_, _, _, args)) = self.is_contract_call().unapply(contract_args) else {
        return Err(illegal_argument_error("metta_compile"));
    };

    // Expect exactly one argument: the MeTTa source code string
    let [arg] = args.as_slice() else {
        return Err(illegal_argument_error("metta_compile: expected 1 argument"));
    };

    // Extract the source code string
    let src = self.pretty_printer.build_string_from_message(arg);

    // Call the MeTTa compiler via FFI
    use std::ffi::{CStr, CString};

    let src_cstr = CString::new(src.as_str())
        .map_err(|_| InterpreterError::IllegalArgumentException {
            message: "Invalid MeTTa source string".to_string(),
        })?;

    let result_json = unsafe {
        let result_ptr = metta_compile_ffi(src_cstr.as_ptr());
        if result_ptr.is_null() {
            return Err(InterpreterError::IllegalArgumentException {
                message: "MeTTa compiler returned null".to_string(),
            });
        }
        let json_str = CStr::from_ptr(result_ptr).to_str()
            .map_err(|_| InterpreterError::IllegalArgumentException {
                message: "Invalid UTF-8 from MeTTa compiler".to_string(),
            })?
            .to_string();
        metta_free_string_ffi(result_ptr);
        json_str
    };

    // Return the JSON result as a Rholang string
    Ok(vec![RhoString::create_par(result_json)])
}

// FFI declarations
extern "C" {
    fn metta_compile_ffi(src: *const std::os::raw::c_char) -> *mut std::os::raw::c_char;
    fn metta_free_string_ffi(ptr: *mut std::os::raw::c_char);
}
```

### Step 5: Register in Rholang Registry

Add to the contracts function (e.g., `test_framework_contracts()` or create a new `metta_contracts()`):

```rust
pub fn metta_contracts(&self) -> Vec<Definition> {
    vec![
        Definition {
            urn: "rho:metta:compile".to_string(),
            fixed_channel: FixedChannels::byte_name(200), // Use an unused channel number
            arity: 2, // source + return channel
            body_ref: 0,
            handler: {
                let sp = self.clone();
                Box::new(move |args| {
                    let sp = sp.clone();
                    Box::pin(async move { sp.metta_compile(args).await })
                })
            },
            remainder: None,
        },
    ]
}
```

## Usage from Rholang

Once registered, you can use the MeTTa compiler from Rholang:

```rholang
new mettaCompile in {
  // Get the MeTTa compile service
  @"rho:metta:compile"!(mettaCompile) |

  // Compile some MeTTa code
  contract @"compileMetta"(source, return) = {
    mettaCompile!(source, *return)
  } |

  // Example usage
  @"compileMetta"("(+ 1 2)", "resultChan") |
  for (@result <- @"resultChan") {
    // result is a JSON string with the compiled MeTTa AST
    stdoutAck!(result, *ack)
  }
}
```

## Build Configuration

### Update Cargo.toml

Add FFI exports:

```toml
[lib]
name = "metta_compiler"
crate-type = ["rlib", "cdylib"]

[features]
ffi = []
```

### Link MeTTa Compiler to Rholang

In Rholang's `Cargo.toml` or build configuration:

```toml
[dependencies]
metta-compiler = { path = "../../../MeTTa-Compiler" }
```

Or use a git dependency:

```toml
[dependencies]
metta-compiler = { git = "https://github.com/F1R3FLY-io/MeTTa-Compiler", branch = "main" }
```

## Testing

### Unit Tests

Test the FFI layer:

```bash
cd /path/to/MeTTa-Compiler
cargo test rholang_integration
cargo test ffi
```

### Integration Test

Create a test in Rholang that exercises the MeTTa compiler:

```rust
#[tokio::test]
async fn test_metta_compile_integration() {
    let mut system = SystemProcesses::new(/* ... */);

    let source = "(+ 1 2)";
    let result = system.metta_compile(/* construct args */).await;

    assert!(result.is_ok());
    let json = extract_string_from_par(&result.unwrap()[0]);
    assert!(json.contains(r#""success":true"#));
}
```

## Error Handling

The MeTTa compiler returns JSON with either success or error:

**Success**:
```json
{
  "success": true,
  "exprs": [
    {"type":"sexpr","items":[...]}
  ]
}
```

**Error**:
```json
{
  "success": false,
  "error": "Parse error at line 1: unexpected token"
}
```

## Security Considerations

1. **Input Validation**: MeTTa compiler validates input and returns errors safely
2. **Memory Safety**: FFI layer uses proper CString handling
3. **Resource Limits**: Consider adding timeouts and size limits for compilation
4. **Sandboxing**: MeTTa compiler is pure and doesn't access filesystem or network

## Future Enhancements

1. **Eval Support**: Add handler for MeTTa `eval` function
2. **Streaming**: Support large MeTTa programs with streaming compilation
3. **Caching**: Cache compiled MeTTa code for reuse
4. **Direct AST**: Return structured AST instead of JSON for better performance
5. **Type Checking**: Expose MeTTa type checker through registry

## References

- [MeTTa Compiler README](../README.md)
- [Rholang System Processes](https://github.com/F1R3FLY-io/f1r3node/tree/new_parser_path_map_support_full/rholang/src/rust/interpreter)
- [PathMap Documentation](../docs/PATHMAP_SUMMARY.md)
