//! WFST/WPDS Scheduler Automaton for MeTTaTron.
//!
//! Replaces the reactive min-heap PriorityQueue scoring with a structurally-informed
//! automata-based scheduler that uses expression shape, analysis hints, and evaluation
//! context to make scheduling decisions.
//!
//! ## Three-Layer Architecture
//!
//! ```text
//! Layer 1: Weighted Tree Automaton (WTA)
//!    MeTTa expression tree → CostClass (bottom-up, O(1) per node)
//!    "What kind of expression is this?"
//!
//! Layer 2: Weighted Finite State Transducer (WFST)
//!    CostClass → SchedulingAction (table lookup, O(1))
//!    "What scheduling decision should we make for this class?"
//!
//! Layer 3: Weighted Pushdown System (WPDS)
//!    (CostClass, ContinuationStack) → RefinedWeight (poststar, amortized O(1))
//!    "Given the evaluation context, how should we refine the cost?"
//! ```
//!
//! ## Why Three Layers?
//!
//! - **WTA alone** classifies expressions but ignores evaluation context
//! - **WFST alone** transduces flat classifications but can't model pushdown structure
//! - **WPDS adds context**: the continuation stack (K in SECK) determines cost refinement
//!
//! ## Modules
//!
//! - `semiring` — Weight algebra (Semiring trait, TropicalWeight, CountingWeight, ProductWeight)
//! - `tree_automaton` — Weighted tree automaton for expression classification
//! - `cost_class` — CostClass enum, TaskDescriptor, SchedulingAction
//! - `classification` — Expression → CostClass two-level table + heuristic fallback
//! - `transducer` — CostClass → SchedulingAction transduction table
//! - `wpds` — Weighted Pushdown System for context-aware weight refinement
//! - `context_weights` — Continuation-to-StackSymbol mapping, context hash
//! - `wavefront` — Dependency DAG and topological wavefront grouping
//! - `online_refinement` — EMA weight updates, P²-to-EMA transfer
//! - `aam_builder` — Build scheduler automaton from DerivedAnalysis

pub mod semiring;
pub mod tree_automaton;
pub mod cost_class;
pub mod classification;
pub mod transducer;
pub mod wpds;
pub mod context_weights;
pub mod wavefront;
pub mod online_refinement;
pub mod aam_builder;

// Re-export key types for convenient access
pub use cost_class::{AffinityHint, CostClass, SchedulingAction, TaskDescriptor};
pub use classification::{SchedulerAutomaton, global_scheduler, install_scheduler};
pub use semiring::{CountingWeight, Semiring, TropicalWeight, SchedulerWeight};
