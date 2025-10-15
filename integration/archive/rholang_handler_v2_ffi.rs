// =====================================================================
// MeTTa Compiler Handlers for Rholang (v2 - Dual Pattern Support)
// =====================================================================
// This file contains TWO handlers to support both calling patterns:
// 1. Explicit return channel (arity: 2) - backward compatible
// 2. Synchronous with implicit return (arity: 1) - for use with !?
//
// Add this code to: f1r3node/rholang/src/rust/interpreter/system_processes.rs
// =====================================================================

// FFI declarations (add once at module level)
extern "C" {
    fn metta_compile(src: *const std::os::raw::c_char) -> *mut std::os::raw::c_char;
    fn metta_free_string(ptr: *mut std::os::raw::c_char);
}

/// Helper function to call MeTTa compiler FFI
/// Shared by both handlers to avoid code duplication
async fn call_metta_compiler_ffi(src: &str) -> Result<String, InterpreterError> {
    use std::ffi::{CStr, CString};

    let src_cstr = CString::new(src)
        .map_err(|_| InterpreterError::IllegalArgumentException {
            message: "Invalid MeTTa source string (contains null byte)".to_string(),
        })?;

    let result_json = unsafe {
        let result_ptr = metta_compile(src_cstr.as_ptr());
        if result_ptr.is_null() {
            return Err(InterpreterError::IllegalArgumentException {
                message: "MeTTa compiler returned null".to_string(),
            });
        }
        let json_str = CStr::from_ptr(result_ptr)
            .to_str()
            .map_err(|_| InterpreterError::IllegalArgumentException {
                message: "Invalid UTF-8 from MeTTa compiler".to_string(),
            })?
            .to_string();
        metta_free_string(result_ptr);
        json_str
    };

    Ok(result_json)
}

// =====================================================================
// Handler 1: Explicit Return Channel (Arity: 2)
// =====================================================================

/// MeTTa compiler handler with explicit return channel
///
/// # Arity: 2 (source code + return channel)
///
/// # Usage from Rholang (Traditional Pattern)
/// ```rholang
/// new result in {
///   @"rho:metta:compile"!(source, *result) |
///   for (@json <- result) {
///     stdoutAck!(json, *ack)
///   }
/// }
/// ```
///
/// # Usage with !? (Synchronous Send)
/// ```rholang
/// new result in {
///   @"rho:metta:compile" !? (source, *result) ; {
///     // Continuation executes after compile completes
///     for (@json <- result) {
///       stdoutAck!(json, *ack)
///     }
///   }
/// }
/// ```
///
/// # Return Format
/// Success: `{"success":true,"exprs":[...]}`
/// Error: `{"success":false,"error":"message"}`
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
        return Err(InterpreterError::IllegalArgumentException {
            message: "metta_compile: expected 1 argument (source code string)".to_string(),
        });
    };

    // Extract the source code string
    let src = self.pretty_printer.build_string_from_message(arg);

    // Call the MeTTa compiler via FFI
    let result_json = call_metta_compiler_ffi(&src).await?;

    // Return the JSON result as a Rholang string
    Ok(vec![RhoString::create_par(result_json)])
}

// =====================================================================
// Handler 2: Synchronous Pattern (Arity: 1)
// =====================================================================

/// MeTTa compiler handler for synchronous pattern (optimized for !?)
///
/// # Arity: 1 (source code only)
///
/// This variant is designed for use with the !? operator where you want
/// the result to be returned immediately without managing channels.
///
/// # Usage from Rholang (Synchronous Pattern with !?)
/// ```rholang
/// // Direct synchronous call - result returned via produce mechanism
/// @"rho:metta:compile:sync" !? (source) ; {
///   // Continuation executes after compile completes
///   // Result is available via the produce mechanism
/// }
/// ```
///
/// # Usage in Contracts
/// ```rholang
/// contract @"myCompiler"(source, return) = {
///   @"rho:metta:compile:sync" !? (source) ; {
///     // Result automatically produced
///     return!(*result)
///   }
/// }
/// ```
///
/// # Return Format
/// Success: `{"success":true,"exprs":[...]}`
/// Error: `{"success":false,"error":"message"}`
pub async fn metta_compile_sync(
    &self,
    contract_args: (Vec<ListParWithRandom>, bool, Vec<Par>),
) -> Result<Vec<Par>, InterpreterError> {
    // Unpack contract arguments with produce mechanism
    let Some((produce, is_replay, previous_output, args)) =
        self.is_contract_call().unapply(contract_args) else {
        return Err(illegal_argument_error("metta_compile_sync"));
    };

    // Handle replay: return cached result
    if is_replay {
        return Ok(previous_output);
    }

    // Expect exactly one argument: the MeTTa source code string
    let [arg] = args.as_slice() else {
        return Err(InterpreterError::IllegalArgumentException {
            message: "metta_compile_sync: expected 1 argument (source code string)".to_string(),
        });
    };

    // Extract the source code string
    let src = self.pretty_printer.build_string_from_message(arg);

    // Call the MeTTa compiler via FFI
    let result_json = call_metta_compiler_ffi(&src).await?;

    // Create result as Par
    let result = vec![RhoString::create_par(result_json)];

    // Produce result (makes it available to continuation)
    // This is used by the !? operator's continuation mechanism
    let ack = self.get_ack();
    produce(&result, ack).await?;

    Ok(result)
}

// =====================================================================
// Usage Examples
// =====================================================================

/*
PATTERN 1: Traditional with Explicit Return Channel (rho:metta:compile)
------------------------------------------------------------------------
new result in {
  @"rho:metta:compile"!("(+ 1 2)", *result) |
  for (@json <- result) {
    stdoutAck!(json, *ack)
  }
}

PATTERN 2: Synchronous with !? and Explicit Channel (rho:metta:compile)
------------------------------------------------------------------------
new result in {
  @"rho:metta:compile" !? ("(+ 1 2)", *result) ; {
    // Continuation executes after compile completes
    for (@json <- result) {
      stdoutAck!(json, *ack)
    }
  }
}

PATTERN 3: Synchronous with !? Implicit Return (rho:metta:compile:sync)
------------------------------------------------------------------------
@"rho:metta:compile:sync" !? ("(+ 1 2)") ; {
  // Continuation executes after compile completes
  // Result is implicitly available via produce mechanism
  stdoutAck!("Compilation complete", *ack)
}

PATTERN 4: Using in Contracts
------------------------------------------------------------------------
contract @"compileAndProcess"(source, return) = {
  new result in {
    @"rho:metta:compile" !? (source, *result) ; {
      for (@json <- result) {
        // Process JSON
        match json.contains("success\":true") {
          true => return!({"compiled": json})
          false => return!({"error": json})
        }
      }
    }
  }
}

PATTERN 5: Wrapper Contract for Ergonomic Usage
------------------------------------------------------------------------
contract @"compile"(source, return) = {
  new result in {
    @"rho:metta:compile"!(source, *result) |
    for (@json <- result) {
      return!(json)
    }
  }
} |

// Now you can use it more concisely:
new ack in {
  @"compile"!("(+ 1 2)", *ack) |
  for (@result <- ack) {
    stdoutAck!(result, *ack)
  }
}

PATTERN 6: Sequential Compilation with !?
------------------------------------------------------------------------
// Compile multiple expressions in sequence
@"rho:metta:compile:sync" !? ("(= (double $x) (* $x 2))") ; {
  @"rho:metta:compile:sync" !? ("!(double 21)") ; {
    stdoutAck!("Both compilations complete", *ack)
  }
}
*/

// =====================================================================
// Integration Notes
// =====================================================================

/*
1. Add both handlers to SystemProcesses impl block

2. Register both services in registry (see rholang_registry_v2.rs)

3. The synchronous variant (metta_compile_sync) uses the produce mechanism
   which is standard for Rholang system processes

4. The !? operator guarantees sequential execution of continuations

5. Both handlers share the same FFI call (call_metta_compiler_ffi)
   to avoid code duplication

6. The explicit return channel pattern (Handler 1) is backward compatible
   with existing code

7. The synchronous pattern (Handler 2) provides better ergonomics for
   use with !? operator

8. Choose the pattern based on your use case:
   - Use Handler 1 for traditional async patterns
   - Use Handler 2 for synchronous sequential workflows
*/
