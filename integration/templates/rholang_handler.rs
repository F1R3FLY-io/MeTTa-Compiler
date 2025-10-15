// =====================================================================
// MeTTa Compiler Handlers for Rholang (v3 - Direct Rust Linking)
// =====================================================================
// This file contains handlers for BOTH MeTTa compiler services using
// DIRECT RUST LINKING (no FFI, no unsafe code).
//
// Services:
// 1. rho:metta:compile       - Traditional with explicit return channel (arity: 2)
// 2. rho:metta:compile:sync  - Synchronous pattern optimized for !? (arity: 1)
//
// Add these handlers to SystemProcesses impl in system_processes.rs
// =====================================================================

use mettatron::rholang_integration::compile_safe;

// =====================================================================
// Handler 1: Traditional Pattern (arity: 2)
// =====================================================================
/// MeTTa compiler handler - Traditional pattern with explicit return channel
///
/// # Service
/// URN: `rho:metta:compile`
/// Channel: 200
/// Arity: 2 (source code + return channel)
///
/// # Usage
/// ```rholang
/// new result in {
///   @"rho:metta:compile"!("(+ 1 2)", *result) |
///   for (@json <- result) {
///     stdoutAck!(json, *ack)
///   }
/// }
/// ```
///
/// # Returns
/// JSON string with compilation result:
/// - Success: `{"success":true,"exprs":[...]}`
/// - Error: `{"success":false,"error":"message"}`
pub async fn metta_compile(
    &self,
    contract_args: (Vec<ListParWithRandom>, bool, Vec<Par>),
) -> Result<Vec<Par>, InterpreterError> {
    // Unpack contract arguments
    let Some((_, _, _, args)) = self.is_contract_call().unapply(contract_args) else {
        return Err(illegal_argument_error("metta_compile"));
    };

    // Expect exactly 1 argument (source code string)
    // The return channel is handled by Rholang runtime
    let [arg] = args.as_slice() else {
        return Err(InterpreterError::IllegalArgumentException {
            message: "metta_compile: expected 1 argument (source code string)".to_string(),
        });
    };

    // Extract source code from Rholang Par
    let src = self.pretty_printer.build_string_from_message(arg);

    // Direct Rust call - no FFI, no unsafe code!
    let result_json = compile_safe(&src);

    // Convert JSON string to Rholang Par
    Ok(vec![RhoString::create_par(result_json)])
}

// =====================================================================
// Handler 2: Synchronous Pattern (arity: 1)
// =====================================================================
/// MeTTa compiler handler - Synchronous pattern with implicit return
///
/// # Service
/// URN: `rho:metta:compile:sync`
/// Channel: 201
/// Arity: 1 (source code only)
///
/// # Usage
/// ```rholang
/// @"rho:metta:compile:sync" !? ("(+ 1 2)") ; {
///   stdoutAck!("Compilation complete", *ack)
/// }
/// ```
///
/// Or with registry binding:
/// ```rholang
/// new compile in {
///   registryLookup!("rho:metta:compile:sync", *compile) |
///   for (@service <- compile) {
///     for (@pm <- service!?(text)) {
///       stdoutAck!(pm, *ack)
///     }
///   }
/// }
/// ```
///
/// # Returns
/// JSON string via produce mechanism (implicit return for !? operator):
/// - Success: `{"success":true,"exprs":[...]}`
/// - Error: `{"success":false,"error":"message"}`
pub async fn metta_compile_sync(
    &self,
    contract_args: (Vec<ListParWithRandom>, bool, Vec<Par>),
) -> Result<Vec<Par>, InterpreterError> {
    // Unpack contract arguments including produce mechanism
    let Some((produce, is_replay, previous_output, args)) =
        self.is_contract_call().unapply(contract_args) else {
        return Err(illegal_argument_error("metta_compile_sync"));
    };

    // Handle replay: return cached result
    if is_replay {
        return Ok(previous_output);
    }

    // Expect exactly 1 argument (source code string)
    let [arg] = args.as_slice() else {
        return Err(InterpreterError::IllegalArgumentException {
            message: "metta_compile_sync: expected 1 argument (source code string)".to_string(),
        });
    };

    // Extract source code from Rholang Par
    let src = self.pretty_printer.build_string_from_message(arg);

    // Direct Rust call - no FFI, no unsafe code!
    let result_json = compile_safe(&src);

    // Convert JSON string to Rholang Par
    let result = vec![RhoString::create_par(result_json)];

    // Produce result to make it available to continuation (!? operator)
    let ack = self.get_ack();
    produce(&result, ack).await?;

    Ok(result)
}

// =====================================================================
// Integration Instructions
// =====================================================================
//
// STEP 1: Add mettatron dependency to Cargo.toml
// ------------------------------------------------
// [dependencies]
// mettatron = { path = "../../../MeTTa-Compiler" }
//
// STEP 2: Import required types
// ------------------------------
// Add at the top of system_processes.rs:
//
// use mettatron::rholang_integration::compile_safe;
//
// STEP 3: Add both handlers to SystemProcesses impl
// --------------------------------------------------
// Copy both handler functions (metta_compile and metta_compile_sync)
// into the SystemProcesses impl block
//
// STEP 4: Add registry function
// ------------------------------
// Copy the metta_contracts() function from rholang_registry_v3_direct.rs
//
// STEP 5: Register at bootstrap
// ------------------------------
// let mut all_defs = system_processes.test_framework_contracts();
// all_defs.extend(system_processes.metta_contracts());
//
// for def in all_defs {
//     registry.register(def)?;
// }
//
// =====================================================================
// Advantages of Direct Rust Linking
// =====================================================================
//
// ✅ Type Safety: Compile-time error checking
// ✅ Memory Safety: No manual memory management
// ✅ No Unsafe Code: No raw pointers or FFI boundary
// ✅ Better Performance: No C ABI overhead
// ✅ Simpler Code: ~20 lines vs ~50 lines with FFI
// ✅ Better Errors: Rust error messages instead of null pointer checks
// ✅ Zero Conversion Overhead: Direct String operations
//
// =====================================================================
// Usage Examples
// =====================================================================

/*
EXAMPLE 1: Traditional Async Pattern
-------------------------------------
new result in {
  @"rho:metta:compile"!("(+ 1 2)", *result) |
  for (@json <- result) {
    stdoutAck!(json, *ack)
  }
}

EXAMPLE 2: Synchronous with !?
-------------------------------
@"rho:metta:compile:sync" !? ("(+ 1 2)") ; {
  stdoutAck!("Compilation complete", *ack)
}

EXAMPLE 3: Sequential Pipeline
-------------------------------
@"rho:metta:compile:sync" !? ("(= (double $x) (* $x 2))") ; {
  @"rho:metta:compile:sync" !? ("!(double 21)") ; {
    stdoutAck!("Pipeline complete", *ack)
  }
}

EXAMPLE 4: Registry Binding with !?
------------------------------------
new compile in {
  registryLookup!("rho:metta:compile:sync", *compile) |
  for (@service <- compile) {
    for (@pm <- service!?("(+ 1 2)")) {
      stdoutAck!("Result: " ++ pm, *ack)
    }
  }
}

EXAMPLE 5: Error Handling
--------------------------
new result in {
  @"rho:metta:compile"!("(+ 1 2", *result) |  // Missing closing paren
  for (@json <- result) {
    match json.contains("\"success\":false") {
      true => stdoutAck!("Compilation failed: " ++ json, *ack)
      false => stdoutAck!("Compilation succeeded: " ++ json, *ack)
    }
  }
}

EXAMPLE 6: Batch Compilation
-----------------------------
contract @"compileBatch"(sources, return) = {
  new results in {
    for (@source <- sources) {
      @"rho:metta:compile:sync" !? (source) ; {
        // Continue to next
      }
    } |
    return!("All compilations complete")
  }
}
*/

// =====================================================================
// Performance Comparison: FFI vs Direct Rust
// =====================================================================

/*
┌────────────────────────────────────────────────────────────────┐
│                    FFI vs Direct Rust                          │
├────────────────────────────────────────────────────────────────┤
│ Aspect              │ FFI (v2)          │ Direct Rust (v3)   │
│─────────────────────┼───────────────────┼────────────────────│
│ String Conversion   │ String→CString    │ Direct &str        │
│                     │ →C→CStr→String    │                    │
│ Memory Safety       │ Manual (unsafe)   │ Automatic (safe)   │
│ Call Overhead       │ C ABI crossing    │ Native Rust call   │
│ Type Safety         │ Runtime checks    │ Compile-time       │
│ Code Complexity     │ ~50 lines         │ ~20 lines          │
│ Null Pointer Checks │ Required          │ Not needed         │
│ Performance         │ Slower            │ Faster             │
│ Error Handling      │ Manual            │ Idiomatic Rust     │
│ Maintenance         │ Higher            │ Lower              │
└────────────────────────────────────────────────────────────────┘

RECOMMENDATION: Use Direct Rust (v3) for Rholang integration
             Use FFI (v2) only for external (non-Rust) languages
*/

// =====================================================================
// Troubleshooting
// =====================================================================

/*
ISSUE: Compilation errors about missing types
SOLUTION: Ensure mettatron is in Cargo.toml dependencies

ISSUE: "compile_safe not found"
SOLUTION: Add use mettatron::rholang_integration::compile_safe;

ISSUE: Service doesn't respond
SOLUTION: Check that registry.register() was called for both services

ISSUE: JSON parsing errors in Rholang
SOLUTION: The JSON format is correct - check your Rholang JSON parser
*/
