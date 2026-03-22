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
pub use state::SeckState;
pub use store::{Store, AllocHint, AllocRegion};
pub use roots::RootSet;
pub use operand_stack::OperandStack;
pub use binding_arena::{BindingArena, ChoicePoint, with_thread_arena, clear_thread_arena};
pub use branch_analysis::{BranchPurity, analyze_branch_purity, classify_branches};
pub use discrimination_tree::{DiscriminationTree, DiscKey};
pub use enhanced_matcher::{EnhancedMatcher, MatchPathDyn};
pub use incremental_gc::{NurseryCollector, NurseryConfig, NurseryState, with_nursery_collector, clear_nursery_collector};
pub use region_alloc::{RegionStack, with_region_stack, clear_region_stack};
pub use reductions::{ReductionCounter, EvalOutcome, SuspendedEval, reduction_budget};
pub use speculative_match::{MatchCandidate, should_speculate, chunk_candidates};
pub use striped_queue::{StripedQueue, StripedTask, current_worker_id, set_worker_id};
pub use thread_local_region::{ThreadLocalRegion, RegionGuard, with_thread_local_region, is_thread_region_active};
pub use tabling::{SubgoalTable, TableLookup, with_subgoal_table, clear_subgoal_table, invalidate_subgoal_table};
pub use thunk::{ThunkTable, ThunkLookup, with_thunk_table, clear_thunk_table};
pub use rete_incremental::with_incremental_index;
pub use adaptive_indexing::with_adaptive_registry;
