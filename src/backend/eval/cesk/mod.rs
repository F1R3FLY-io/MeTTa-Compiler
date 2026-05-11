//! SECK Abstract Machine Formalization
//!
//! This module formalizes MeTTaTron's implicit CEK machine (trampoline evaluator)
//! into an explicit **SECK machine** (Store, Environment, Control, Kontinuation),
//! inspired by the SECD machine with an explicit operand stack.
//!
//! ## Machine Components
//!
//! ```text
//! ⟨S, E, C, K, Store⟩
//!
//! S:     Pre-allocated operand stack (intermediate results)
//! E:     Environment (variable bindings, rules — Arc CoW, unchanged)
//! C:     Control expression (current expression being evaluated)
//! K:     Continuation stack (saved machine states — unchanged semantics)
//! Store: Parameterized allocation/GC subsystem
//! ```
//!
//! ## Design Rationale
//!
//! The Store is formalized with a parameterized `alloc(value, hint)` function
//! (Van Horn & Might, "Abstracting Abstract Machines", CACM 2011, Section 2.4),
//! enabling:
//!
//! - Context-sensitive allocation (short-lived vs long-lived hints)
//! - Region-based allocation (bulk-free `let*` scopes)
//! - Abstract interpretation via the AAM methodology
//! - Algebraic root set computation for precise GC
//!
//! ## Integration
//!
//! This is a **formalization layer** — the existing trampoline loop, continuation
//! types, and environment remain unchanged. The SECK types wrap the existing
//! scattered state into a coherent machine description, enabling future phases
//! (GC improvements, parallelism, AAM analysis) to reason about machine state
//! algebraically.

pub mod adaptive_indexing;
pub mod binding_arena;
pub mod branch_analysis;
pub mod continuation_compression;
pub mod coroutine;
pub mod discrimination_tree;
pub mod enhanced_matcher;
pub mod incremental_gc;
pub mod operand_stack;
pub mod reductions;
pub mod region_alloc;
pub mod rete_incremental;
pub mod roots;
pub mod speculative_match;
pub mod state;
pub mod store;
pub mod striped_queue;
pub mod tabling;
pub mod thread_local_region;
pub mod thunk;

// Re-export primary types
pub use adaptive_indexing::with_adaptive_registry;
pub use binding_arena::{clear_thread_arena, with_thread_arena, BindingArena, ChoicePoint};
pub use branch_analysis::{analyze_branch_purity, classify_branches, BranchPurity};
pub use discrimination_tree::{DiscKey, DiscriminationTree};
pub use enhanced_matcher::{EnhancedMatcher, MatchPathDyn};
pub use incremental_gc::{
    clear_nursery_collector, with_nursery_collector, NurseryCollector, NurseryConfig, NurseryState,
};
pub use operand_stack::OperandStack;
pub use reductions::{reduction_budget, EvalOutcome, ReductionCounter, SuspendedEval};
pub use region_alloc::{clear_region_stack, with_region_stack, RegionStack};
pub use rete_incremental::with_incremental_index;
pub use roots::RootSet;
pub use speculative_match::{chunk_candidates, should_speculate, MatchCandidate};
pub use state::SeckState;
pub use store::{AllocHint, AllocRegion, Store};
pub use striped_queue::{current_worker_id, set_worker_id, StripedQueue, StripedTask};
pub use tabling::{
    clear_active_eval_set, clear_subgoal_table, invalidate_subgoal_table, is_actively_evaluating,
    mark_eval_active, unmark_eval_active, with_subgoal_table, SubgoalTable, TableLookup,
};
pub use thread_local_region::{
    is_thread_region_active, with_thread_local_region, RegionGuard, ThreadLocalRegion,
};
pub use thunk::{clear_thunk_table, with_thunk_table, ThunkLookup, ThunkTable};
