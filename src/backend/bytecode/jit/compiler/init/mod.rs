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

use cranelift_module::FuncId;

// Re-export all initialization traits

mod arithmetic;

mod bindings;

mod calls;

mod nondet;

mod pattern_matching;

mod rules;

mod space;

mod special_forms;

mod type_ops;

mod sexpr;

mod higher_order;

mod globals;

mod debug;

// Public re-exports

pub use arithmetic::{ArithmeticFuncIds, ArithmeticInit};

pub use bindings::{BindingFuncIds, BindingsInit};

pub use calls::{CallFuncIds, CallsInit};

pub use nondet::{NondetFuncIds, NondetInit};

pub use pattern_matching::{PatternMatchingFuncIds, PatternMatchingInit};

pub use rules::{RulesFuncIds, RulesInit};

pub use space::{SpaceFuncIds, SpaceInit};

pub use special_forms::{SpecialFormsFuncIds, SpecialFormsInit};

pub use type_ops::{TypeOpsFuncIds, TypeOpsInit};

pub use sexpr::{SExprFuncIds, SExprInit};

pub use higher_order::{HigherOrderFuncIds, HigherOrderInit};

pub use globals::{GlobalsFuncIds, GlobalsInit};

pub use debug::{DebugFuncIds, DebugInit};

// =============================================================================
// Aggregated FuncId Groups
// =============================================================================

/// All function ID groups collected together for JitCompiler
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
pub struct MiscFuncIds {
    /// Load constant from constant pool
    pub load_const_func_id: FuncId,
}
