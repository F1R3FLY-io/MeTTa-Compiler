// =====================================================================
// MeTTa Compiler Registry for Rholang
// =====================================================================
// This file contains the code that should be added to register the
// MeTTa compiler in the Rholang system process registry.
//
// Add this function to system_processes.rs or create a new module.
// =====================================================================

/// Create MeTTa compiler contract definitions
/// Returns a vector of system process definitions for MeTTa integration
///
/// # Example
/// ```rust
/// let metta_defs = system_processes.metta_contracts();
/// // Register these definitions with the runtime
/// ```
pub fn metta_contracts(&self) -> Vec<Definition> {
    vec![
        Definition {
            // URN for the MeTTa compiler service
            urn: "rho:metta:compile".to_string(),

            // Fixed channel for accessing the compiler
            // Channel 200 - ensure this doesn't conflict with other system processes
            fixed_channel: FixedChannels::byte_name(200),

            // Arity: 2 arguments (source code + return channel)
            arity: 2,

            // Body reference (can be 0 for system processes)
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

        // Future: Add eval service
        // Definition {
        //     urn: "rho:metta:eval".to_string(),
        //     fixed_channel: FixedChannels::byte_name(201),
        //     arity: 2,
        //     body_ref: 0,
        //     handler: {
        //         let sp = self.clone();
        //         Box::new(move |args| {
        //             let sp = sp.clone();
        //             Box::pin(async move { sp.metta_eval(args).await })
        //         })
        //     },
        //     remainder: None,
        // },
    ]
}

// =====================================================================
// Integration Instructions
// =====================================================================
//
// 1. Add metta_compile handler to SystemProcesses (see rholang_handler.rs)
//
// 2. Add metta_contracts() function to SystemProcesses impl
//
// 3. Register MeTTa contracts in the bootstrap or initialization code:
//    ```rust
//    let system_processes = SystemProcesses::new(/* ... */);
//    let metta_defs = system_processes.metta_contracts();
//
//    // Add to registry or contract definitions
//    for def in metta_defs {
//        registry.register(def);
//    }
//    ```
//
// 4. Update Cargo.toml to include metta-compiler dependency:
//    ```toml
//    [dependencies]
//    metta-compiler = { path = "../../../MeTTa-Compiler" }
//    ```
//
// 5. Link the cdylib at build time or runtime
//
// 6. Test the integration:
//    ```rholang
//    new compile, ack in {
//      @"rho:metta:compile"!("(+ 1 2)", *ack) |
//      for (@result <- ack) {
//        stdoutAck!(result, *ack)
//      }
//    }
//    ```
