//! Hybrid JIT/Bytecode Executor
//!
//! This module provides seamless execution that automatically switches
//! between JIT-compiled native code and bytecode VM interpretation based
//! on execution hotness and code characteristics.
//!
//! # Execution Strategy
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────────┐
//! │                     HybridExecutor.run()                            │
//! │                                                                     │
//! │  1. Check JitCache for compiled native code                         │
//! │     └─ If found → Execute JIT code                                  │
//! │                                                                     │
//! │  2. Track execution count via TieredCompiler                        │
//! │     └─ If hot threshold reached → Compile to JIT                    │
//! │                                                                     │
//! │  3. Execute:                                                        │
//! │     └─ JIT available → Native execution                             │
//! │     └─ JIT unavailable/bailed → Bytecode VM fallback                │
//! │                                                                     │
//! │  4. Handle bailout:                                                 │
//! │     └─ Transfer JIT stack → VM stack                                │
//! │     └─ Resume execution from bailout IP                             │
//! └─────────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Example
//!
//! ```ignore
//! use mettatron::backend::bytecode::jit::hybrid::HybridExecutor;
//! use mettatron::backend::bytecode::compile_arc;
//!
//! let chunk = compile_arc("example", &expr)?;
//! let mut executor = HybridExecutor::new();
//! let results = executor.run(&chunk)?;
//! ```

mod constants;
mod config;
mod executor;
mod backtracking;

#[cfg(test)]
mod tests;

// Re-export public API
pub use constants::*;
pub use config::{HybridConfig, HybridStats};
pub use executor::HybridExecutor;
