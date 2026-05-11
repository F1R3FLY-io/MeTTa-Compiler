//! AAM Static Analysis Framework
//!
//! Implements Van Horn & Might's "Abstracting Abstract Machines" methodology
//! (CACM 2011) over MeTTaTron's concrete SECK machine. The analysis computes
//! a fixed-point over abstract machine states to derive:
//!
//! - **Dead rule detection**: Rules that can never fire from any reachable state
//! - **Determinism detection**: Expressions that always match exactly 1 rule
//! - **Purity analysis**: Expressions with no reachable side effects
//! - **Type specialization**: Expressions that always produce a known type
//! - **Groundness analysis**: Expressions that always produce ground values
//!
//! ## Usage
//!
//! ```text
//! // After loading rules, before evaluation:
//! let result = run_analysis(&top_level_exprs, &env, &AnalysisConfig::default());
//! let derived = derive_analysis(&result);
//! let hints = build_dispatch_hints(&derived);
//! // Install hints in environment for runtime use
//! ```
//!
//! ## Feature Gate
//!
//! Analysis is opt-in via `--analyze` CLI flag or `METTATRON_AAM_ANALYSIS=1` env var.
//! When disabled, no existing code paths are affected.

pub mod abstract_domain;
pub mod abstract_transition;
pub mod derived;
pub mod fixpoint;
pub mod module_dce;
pub mod pushdown;
pub mod race_detection;

// Re-exports
pub use abstract_domain::{
    AbstractAddr, AbstractEnv, AbstractStore, AbstractType, AbstractValue, AbstractValueSet,
    AnalysisConfig,
};
pub use abstract_transition::{AbstractControl, AbstractKont, AbstractState, EnvironmentSnapshot};
pub use derived::{derive_analysis, DerivedAnalysis, DispatchHint};
pub use fixpoint::{run_analysis, AnalysisResult, ExprFact};
