// =====================================================================
// MeTTa Compiler Registry for Rholang (v3 - Direct Rust Linking)
// =====================================================================
// This file contains the registry code for BOTH MeTTa compiler services
// using DIRECT RUST LINKING (no FFI, no unsafe code).
//
// Services:
// 1. rho:metta:compile       - Traditional with explicit return channel
// 2. rho:metta:compile:sync  - Synchronous pattern optimized for !?
//
// Add this function to SystemProcesses impl in system_processes.rs
// =====================================================================

/// Create MeTTa compiler contract definitions
/// Returns a vector of system process definitions for MeTTa integration
///
/// This registers TWO services using direct Rust linking:
/// - `rho:metta:compile` - Traditional pattern with explicit return channel
/// - `rho:metta:compile:sync` - Synchronous pattern for use with !?
///
/// # Example
/// ```rust
/// // In your bootstrap code
/// let mut all_defs = system_processes.test_framework_contracts();
/// all_defs.extend(system_processes.metta_contracts());
///
/// for def in all_defs {
///     registry.register(def)?;
/// }
/// ```
pub fn metta_contracts(&self) -> Vec<Definition> {
    vec![
        // ================================================================
        // Service 1: Traditional Pattern (rho:metta:compile)
        // ================================================================
        Definition {
            // URN for the MeTTa compiler service
            urn: "rho:metta:compile".to_string(),

            // Fixed channel for accessing the compiler
            // Channel 200 - ensure this doesn't conflict with other system processes
            fixed_channel: FixedChannels::byte_name(200),

            // Arity: 2 arguments (source code + return channel)
            arity: 2,

            // Body reference (0 for system processes)
            body_ref: 0,

            // Handler function that calls metta_compile
            // Direct Rust function call - no FFI!
            handler: {
                let sp = self.clone();
                Box::new(move |args| {
                    let sp = sp.clone();
                    Box::pin(async move { sp.metta_compile(args).await })
                })
            },

            // No remainder
            remainder: None,
        },

        // ================================================================
        // Service 2: Synchronous Pattern (rho:metta:compile:sync)
        // ================================================================
        Definition {
            // URN for the synchronous MeTTa compiler service
            urn: "rho:metta:compile:sync".to_string(),

            // Fixed channel for accessing the synchronous compiler
            // Channel 201 - next sequential channel after rho:metta:compile
            fixed_channel: FixedChannels::byte_name(201),

            // Arity: 1 argument (source code only)
            // Return is handled via produce mechanism
            arity: 1,

            // Body reference (0 for system processes)
            body_ref: 0,

            // Handler function that calls metta_compile_sync
            // Direct Rust function call - no FFI!
            handler: {
                let sp = self.clone();
                Box::new(move |args| {
                    let sp = sp.clone();
                    Box::pin(async move { sp.metta_compile_sync(args).await })
                })
            },

            // No remainder
            remainder: None,
        },

        // ================================================================
        // Future: Additional MeTTa Services
        // ================================================================
        // Future services can be added here:
        // - rho:metta:eval (compile and evaluate)
        // - rho:metta:typecheck (type checking)
        // - rho:metta:compile:batch (batch compilation)
    ]
}

// =====================================================================
// Integration Instructions - Direct Rust Approach
// =====================================================================
//
// STEP 1: Add mettatron dependency to Cargo.toml
// ------------------------------------------------
// In your Rholang interpreter's Cargo.toml:
//
// [dependencies]
// mettatron = { path = "../../../MeTTa-Compiler" }
//
// Or if using git:
//
// [dependencies]
// mettatron = { git = "https://github.com/F1R3FLY-io/MeTTa-Compiler.git" }
//
// STEP 2: Add imports to system_processes.rs
// -------------------------------------------
// Add at the top of system_processes.rs:
//
// use mettatron::rholang_integration::compile_safe;
//
// STEP 3: Add BOTH handlers to SystemProcesses impl
// --------------------------------------------------
// Copy the handlers from rholang_handler_v3_direct.rs:
// - metta_compile (arity-2 handler)
// - metta_compile_sync (arity-1 handler)
//
// STEP 4: Add metta_contracts() function to SystemProcesses impl
// ---------------------------------------------------------------
// Copy this function into the SystemProcesses impl block
//
// STEP 5: Register MeTTa contracts at bootstrap
// ----------------------------------------------
// ```rust
// let system_processes = SystemProcesses::new(/* ... */);
// let mut all_defs = system_processes.test_framework_contracts();
// all_defs.extend(system_processes.metta_contracts());
//
// // Register all definitions
// for def in all_defs {
//     registry.register(def)?;
// }
// ```
//
// STEP 6: Build and verify
// -------------------------
// cargo build --release
//
// No FFI compilation needed! Pure Rust.
//
// STEP 7: Verify channel numbers don't conflict
// ----------------------------------------------
// Check that channels 200 and 201 are not used by other system processes:
// ```bash
// grep "FixedChannels::byte_name(20[01])" system_processes.rs
// ```
//
// If conflicts exist, choose different channel numbers
//
// =====================================================================
// Advantages of Direct Rust Integration
// =====================================================================
//
// ✅ NO FFI OVERHEAD: Direct function calls
// ✅ NO UNSAFE CODE: All safe Rust
// ✅ TYPE SAFETY: Compile-time checking
// ✅ MEMORY SAFETY: Automatic memory management
// ✅ BETTER PERFORMANCE: No C ABI crossing
// ✅ SIMPLER CODE: ~60% less code than FFI
// ✅ EASIER DEBUGGING: Rust stack traces work correctly
// ✅ BETTER ERRORS: Rust error messages instead of null checks
// ✅ ZERO CONVERSION OVERHEAD: No CString conversions
// ✅ IDIOMATIC RUST: Natural Rust patterns
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

EXAMPLE 2: Synchronous with !? (Simple)
----------------------------------------
@"rho:metta:compile:sync" !? ("(+ 1 2)") ; {
  stdoutAck!("Compilation complete", *ack)
}

EXAMPLE 3: Sequential Pipeline (⭐ Best Use Case)
------------------------------------------------
// Compile multiple expressions in guaranteed sequence
@"rho:metta:compile:sync" !? ("(= (double $x) (* $x 2))") ; {
  @"rho:metta:compile:sync" !? ("!(double 21)") ; {
    @"rho:metta:compile:sync" !? ("(+ 10 20)") ; {
      stdoutAck!("All three compiled in sequence!", *ack)
    }
  }
}

EXAMPLE 4: Registry Binding with !? (⭐ Idiomatic Pattern)
----------------------------------------------------------
new compile in {
  registryLookup!("rho:metta:compile:sync", *compile) |
  for (@service <- compile) {
    // Use with !? in for comprehension
    for (@pm <- service!?("(+ 1 2)")) {
      stdoutAck!("Result: " ++ pm, *ack)
    }
  }
}

EXAMPLE 5: Error Handling
--------------------------
contract @"safeCompile"(source, @onSuccess, @onError) = {
  new result in {
    @"rho:metta:compile" !? (source, *result) ; {
      for (@json <- result) {
        match json.contains("\"success\":true") {
          true => onSuccess!(json)
          false => onError!(json)
        }
      }
    }
  }
}

EXAMPLE 6: Batch Processing
----------------------------
contract @"compileBatch"(sources, return) = {
  new compile_next in {
    contract compile_next(@index, @results) = {
      match index < sources.length() {
        true => {
          @"rho:metta:compile:sync" !? (sources[index]) ; {
            compile_next!(index + 1, results ++ [result])
          }
        }
        false => {
          return!(results)
        }
      }
    } |
    compile_next!(0, [])
  }
}

EXAMPLE 7: Conditional Compilation
-----------------------------------
contract @"conditionalCompile"(source, condition, return) = {
  match condition {
    true => {
      @"rho:metta:compile:sync" !? (source) ; {
        return!("Compiled")
      }
    }
    false => {
      return!("Skipped")
    }
  }
}
*/

// =====================================================================
// Service Comparison
// =====================================================================

/*
┌────────────────────────────────────────────────────────────────────┐
│                   Service Feature Comparison                       │
├────────────────────────────────────────────────────────────────────┤
│ Feature             │ rho:metta:compile │ rho:metta:compile:sync │
│─────────────────────┼───────────────────┼────────────────────────│
│ URN                 │ rho:metta:compile │ rho:metta:compile:sync │
│ Channel             │ 200               │ 201                    │
│ Arity               │ 2                 │ 1                      │
│ Arguments           │ source + channel  │ source only            │
│ Return Pattern      │ Explicit channel  │ Implicit (produce)     │
│ Best For            │ Async patterns    │ Sequential workflows   │
│ !? Compatible       │ Yes               │ Yes (optimized)        │
│ Registry Binding    │ Yes               │ Yes (with !? pattern)  │
│ Implementation      │ Direct Rust       │ Direct Rust            │
│ Use Case            │ General purpose   │ Synchronous pipelines  │
└────────────────────────────────────────────────────────────────────┘

WHEN TO USE EACH:

Use rho:metta:compile when:
  ✓ You need explicit control over return channels
  ✓ You're building async/concurrent workflows
  ✓ You want backward compatibility
  ✓ You're integrating with existing code

Use rho:metta:compile:sync when:
  ✓ You're building sequential compilation pipelines
  ✓ You want simpler, more concise code with !?
  ✓ You don't need explicit channel management
  ✓ You're building synchronous workflows
  ✓ You want the idiomatic for (@pm <- service!?(...)) pattern
*/

// =====================================================================
// Direct Rust vs FFI Comparison
// =====================================================================

/*
┌────────────────────────────────────────────────────────────────────┐
│           Implementation Approach Comparison                       │
├────────────────────────────────────────────────────────────────────┤
│ Aspect              │ FFI (v2)          │ Direct Rust (v3) ⭐    │
│─────────────────────┼───────────────────┼────────────────────────│
│ String Conversion   │ String→CString    │ Direct &str            │
│                     │ →C→CStr→String    │ (zero overhead)        │
│ Memory Safety       │ Manual (unsafe)   │ Automatic (safe)       │
│ Call Overhead       │ C ABI crossing    │ Native Rust call       │
│ Type Safety         │ Runtime checks    │ Compile-time checks    │
│ Code Complexity     │ ~50 lines         │ ~20 lines (60% less)   │
│ Null Pointer Checks │ Required          │ Not needed             │
│ Performance         │ Slower            │ Faster                 │
│ Error Handling      │ Manual checking   │ Idiomatic Rust Result  │
│ Debugging           │ Harder            │ Natural Rust traces    │
│ Maintenance         │ Higher            │ Lower                  │
│ Dependencies        │ C toolchain       │ Rust only              │
│ Cross-compilation   │ Complex           │ Simple                 │
└────────────────────────────────────────────────────────────────────┘

RECOMMENDATION:
  - For Rholang integration: Use Direct Rust (v3) ⭐
  - For external languages: Use FFI (v2)
*/

// =====================================================================
// Testing
// =====================================================================

/*
After deployment, test both services:

TEST 1: Traditional Pattern
----------------------------
new result in {
  @"rho:metta:compile"!("(+ 1 2)", *result) |
  for (@json <- result) {
    // Expect: {"success":true,"exprs":[...]}
    stdoutAck!(json, *ack)
  }
}

TEST 2: Synchronous Pattern
----------------------------
@"rho:metta:compile:sync" !? ("(+ 1 2)") ; {
  // Expect: Compilation completes, continuation executes
  stdoutAck!("✓ Synchronous pattern works", *ack)
}

TEST 3: Error Handling
----------------------
new result in {
  @"rho:metta:compile"!("(+ 1 2", *result) |  // Missing closing paren
  for (@json <- result) {
    // Expect: {"success":false,"error":"..."}
    match json.contains("\"success\":false") {
      true => stdoutAck!("✓ Error handling works", *ack)
      false => stdoutAck!("✗ Should have been an error", *ack)
    }
  }
}

TEST 4: Sequential Pipeline
----------------------------
@"rho:metta:compile:sync" !? ("(+ 1 2)") ; {
  @"rho:metta:compile:sync" !? ("(* 3 4)") ; {
    stdoutAck!("✓ Sequential pipeline works", *ack)
  }
}

TEST 5: Registry Binding Pattern
---------------------------------
new compile in {
  registryLookup!("rho:metta:compile:sync", *compile) |
  for (@service <- compile) {
    for (@pm <- service!?("(+ 1 2)")) {
      stdoutAck!("✓ Registry binding with !? works: " ++ pm, *ack)
    }
  }
}
*/

// =====================================================================
// Channel Allocation Reference
// =====================================================================

/*
MeTTa Services Channel Allocation:
  - 200: rho:metta:compile (traditional pattern)
  - 201: rho:metta:compile:sync (synchronous pattern)
  - 202: (reserved for future: rho:metta:eval)
  - 203: (reserved for future: rho:metta:typecheck)
  - 204: (reserved for future: rho:metta:compile:batch)

Ensure these channels don't conflict with other system processes.
Check and update if necessary.
*/

// =====================================================================
// Troubleshooting
// =====================================================================

/*
ISSUE: "mettatron not found" compilation error
SOLUTION: Add mettatron to [dependencies] in Cargo.toml

ISSUE: "compile_safe not found"
SOLUTION: Add use mettatron::rholang_integration::compile_safe;

ISSUE: Service doesn't respond
SOLUTION:
  - Check registry.register() was called for both services
  - Verify channel numbers don't conflict
  - Check bootstrap code includes metta_contracts()

ISSUE: Type errors during compilation
SOLUTION: Ensure Rholang types (Par, InterpreterError, etc.) are imported

ISSUE: JSON parsing errors in Rholang
SOLUTION:
  - Check JSON format: {"success":true/false,...}
  - Verify Rholang JSON parser configuration
  - Test with simple expression first: "(+ 1 2)"
*/

// =====================================================================
// Migration from FFI (v2) to Direct Rust (v3)
// =====================================================================

/*
If you've already deployed v2 (FFI approach), migrating to v3 is simple:

1. Update Cargo.toml
   - Remove: extern "C" declarations
   - Add: mettatron dependency

2. Update imports
   - Add: use mettatron::rholang_integration::compile_safe;
   - Remove: FFI declarations

3. Update handlers
   - Replace: call_metta_compiler_ffi(&src)
   - With: compile_safe(&src)
   - Remove: All unsafe blocks and CString conversions

4. Update registry
   - No changes needed (same structure)

5. Test
   - All existing Rholang code continues to work
   - No API changes, just implementation improvement

Benefits of migration:
  ✅ Eliminate all unsafe code
  ✅ Improve performance
  ✅ Simplify maintenance
  ✅ Better error messages
  ✅ Remove C toolchain dependency
*/
