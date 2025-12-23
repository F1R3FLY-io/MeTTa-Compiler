//! JIT Compiler Initialization Module
//!
//! This module organizes runtime function ID declarations into logical groups.
//! Each group has its own struct holding related FuncIds and a trait for
//! initialization (symbol registration + function declaration).
//!
//! # Zero-Overhead Design
//!
//! All traits use static dispatch - they're implemented directly on JitCompiler
//! and the compiler inlines everything at compile time. No `dyn Trait` is used.

#[cfg(feature = "jit")]
use cranelift_module::FuncId;

// Re-export all initialization traits
#[cfg(feature = "jit")]
mod arithmetic;
#[cfg(feature = "jit")]
mod bindings;
#[cfg(feature = "jit")]
mod calls;
#[cfg(feature = "jit")]
mod nondet;
#[cfg(feature = "jit")]
mod pattern_matching;
#[cfg(feature = "jit")]
mod rules;
#[cfg(feature = "jit")]
mod space;
#[cfg(feature = "jit")]
mod special_forms;
#[cfg(feature = "jit")]
mod type_ops;
#[cfg(feature = "jit")]
mod sexpr;
#[cfg(feature = "jit")]
mod higher_order;
#[cfg(feature = "jit")]
mod globals;
#[cfg(feature = "jit")]
mod debug;

// Public re-exports
#[cfg(feature = "jit")]
pub use arithmetic::{ArithmeticFuncIds, ArithmeticInit};
#[cfg(feature = "jit")]
pub use bindings::{BindingFuncIds, BindingsInit};
#[cfg(feature = "jit")]
pub use calls::{CallFuncIds, CallsInit};
#[cfg(feature = "jit")]
pub use nondet::{NondetFuncIds, NondetInit};
#[cfg(feature = "jit")]
pub use pattern_matching::{PatternMatchingFuncIds, PatternMatchingInit};
#[cfg(feature = "jit")]
pub use rules::{RulesFuncIds, RulesInit};
#[cfg(feature = "jit")]
pub use space::{SpaceFuncIds, SpaceInit};
#[cfg(feature = "jit")]
pub use special_forms::{SpecialFormsFuncIds, SpecialFormsInit};
#[cfg(feature = "jit")]
pub use type_ops::{TypeOpsFuncIds, TypeOpsInit};
#[cfg(feature = "jit")]
pub use sexpr::{SExprFuncIds, SExprInit};
#[cfg(feature = "jit")]
pub use higher_order::{HigherOrderFuncIds, HigherOrderInit};
#[cfg(feature = "jit")]
pub use globals::{GlobalsFuncIds, GlobalsInit};
#[cfg(feature = "jit")]
pub use debug::{DebugFuncIds, DebugInit};

// =============================================================================
// Aggregated FuncId Groups
// =============================================================================

/// All function ID groups collected together for JitCompiler
#[cfg(feature = "jit")]
pub struct FuncIdGroups {
    pub arithmetic: ArithmeticFuncIds,
    pub bindings: BindingFuncIds,
    pub calls: CallFuncIds,
    pub nondet: NondetFuncIds,
    pub pattern_matching: PatternMatchingFuncIds,
    pub rules: RulesFuncIds,
    pub space: SpaceFuncIds,
    pub special_forms: SpecialFormsFuncIds,
    pub type_ops: TypeOpsFuncIds,
    pub sexpr: SExprFuncIds,
    pub higher_order: HigherOrderFuncIds,
    pub globals: GlobalsFuncIds,
    pub debug: DebugFuncIds,
}

// =============================================================================
// Legacy FuncId fields (for backwards compatibility during transition)
// =============================================================================

/// Function IDs that don't fit neatly into groups or are used standalone
#[cfg(feature = "jit")]
pub struct MiscFuncIds {
    /// Load constant from constant pool
    pub load_const_func_id: FuncId,
}
