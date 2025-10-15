// =====================================================================
// MeTTa Compiler Registry for Rholang (v2 - Dual Pattern Support)
// =====================================================================
// This file contains the registry code for BOTH MeTTa compiler services:
// 1. rho:metta:compile       - Traditional with explicit return channel
// 2. rho:metta:compile:sync  - Synchronous pattern optimized for !?
//
// Add this function to system_processes.rs
// =====================================================================

/// Create MeTTa compiler contract definitions
/// Returns a vector of system process definitions for MeTTa integration
///
/// This registers TWO services to support different calling patterns:
/// - `rho:metta:compile` - Traditional pattern with explicit return channel
/// - `rho:metta:compile:sync` - Synchronous pattern for use with !?
///
/// # Example
/// ```rust
/// let metta_defs = system_processes.metta_contracts();
/// for def in metta_defs {
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
// Integration Instructions
// =====================================================================
//
// STEP 1: Add BOTH handlers to SystemProcesses
// ----------------------------------------------
// Copy the code from rholang_handler_v2.rs into system_processes.rs:
// - call_metta_compiler_ffi (helper function)
// - metta_compile (arity-2 handler)
// - metta_compile_sync (arity-1 handler)
//
// STEP 2: Add metta_contracts() function to SystemProcesses impl
// ---------------------------------------------------------------
// Copy this function into the SystemProcesses impl block
//
// STEP 3: Register MeTTa contracts at bootstrap
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
// STEP 4: Update Cargo.toml
// --------------------------
// ```toml
// [dependencies]
// mettatron = { path = "../../../MeTTa-Compiler" }
// ```
//
// STEP 5: Verify channel numbers don't conflict
// ----------------------------------------------
// Check that channels 200 and 201 are not used by other system processes:
// ```bash
// grep "FixedChannels::byte_name(20[01])" system_processes.rs
// ```
//
// If conflicts exist, choose different channel numbers
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

EXAMPLE 2: Synchronous with !? (Explicit Channel)
-------------------------------------------------
new result in {
  @"rho:metta:compile" !? ("(+ 1 2)", *result) ; {
    for (@json <- result) {
      stdoutAck!(json, *ack)
    }
  }
}

EXAMPLE 3: Synchronous with !? (Implicit Return)
------------------------------------------------
@"rho:metta:compile:sync" !? ("(+ 1 2)") ; {
  stdoutAck!("Compilation complete", *ack)
}

EXAMPLE 4: Sequential Compilation Pipeline
-------------------------------------------
// Compile rule, then compile usage
@"rho:metta:compile:sync" !? ("(= (double $x) (* $x 2))") ; {
  @"rho:metta:compile:sync" !? ("!(double 21)") ; {
    stdoutAck!("Pipeline complete", *ack)
  }
}

EXAMPLE 5: Conditional Compilation
-----------------------------------
contract @"conditionalCompile"(source, condition, return) = {
  match condition {
    true => {
      new result in {
        @"rho:metta:compile" !? (source, *result) ; {
          for (@json <- result) {
            return!(json)
          }
        }
      }
    }
    false => {
      return!({"success": false, "error": "Compilation skipped"})
    }
  }
}

EXAMPLE 6: Error Handling with Both Patterns
--------------------------------------------
contract @"safeCompile"(source, @onSuccess, @onError) = {
  new result in {
    // Use traditional pattern for more control
    @"rho:metta:compile"!(source, *result) |
    for (@json <- result) {
      match json.contains("\"success\":true") {
        true => onSuccess!(json)
        false => onError!(json)
      }
    }
  }
}

// Or use synchronous pattern
contract @"safeCompileSync"(source) = {
  @"rho:metta:compile:sync" !? (source) ; {
    // Result automatically produced
    stdoutAck!("Done", *ack)
  }
}

EXAMPLE 7: Batch Processing
----------------------------
contract @"compileBatch"(sources, return) = {
  new resultChan in {
    // Compile each source sequentially
    for (source <- sources) {
      @"rho:metta:compile:sync" !? (source) ; {
        // Continue to next
      }
    } |
    // All done
    return!("All compilations complete")
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
│ Backward Compatible │ Yes               │ N/A (new)              │
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
  stdoutAck!("Test passed", *ack)
}

TEST 3: Error Handling (Both)
------------------------------
// Test with invalid MeTTa
@"rho:metta:compile"!("(+ 1 2", *result) |
for (@json <- result) {
  // Expect: {"success":false,"error":"..."}
}

@"rho:metta:compile:sync" !? ("(+ 1 2") ; {
  // Expect: Error produced, continuation executes
}

TEST 4: Sequential Pipeline
----------------------------
@"rho:metta:compile:sync" !? ("(+ 1 2)") ; {
  @"rho:metta:compile:sync" !? ("(* 3 4)") ; {
    stdoutAck!("Both compiled sequentially", *ack)
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
