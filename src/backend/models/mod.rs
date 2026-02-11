pub mod atom_id;
pub mod bindings;
pub mod gc_allocator;
pub mod gc_cron;
pub mod gc_thread;
pub mod generic_bindings;
pub mod generic_rule;
pub mod indexed_multiset;
pub mod memo_handle;
pub mod metta_state;
pub mod metta_value;
pub mod metta_value_trait;
pub mod multiset;
pub mod space_handle;

pub use metta_value::{MettaValue, MettaValueInner};
pub use atom_id::{AtomId, SymbolTable};
pub use gc_allocator::{
    collect_all_roots, global_allocator, global_factory, global_gc_thread,
    init_global_allocator, maybe_trigger_gc, register_root_provider,
    request_gc, trigger_gc_cycle, try_register_env_roots,
    GcFactory, RootProvider, SlabAllocator,
};
pub use bindings::SmartBindings as Bindings;
pub use generic_bindings::{GenericBindings, GenericBindingsIter};
pub use generic_rule::{GenericRule, RuleBytes};
pub use indexed_multiset::IndexedMultiset;
pub use memo_handle::MemoHandle;
pub use metta_state::MettaState;
pub use metta_value::{escape_json, serialize_tags};
pub use metta_value_trait::{MettaValueTrait, MettaValueFactory};
pub use multiset::{AtomMultiset, AtomMultisetSnapshot};
pub use space_handle::{GenericMultiplicityMatch, SpaceHandle};

use crate::backend::environment::MettaEnvironment;

/// Result of evaluation: (result, new_environment)
pub type EvalResult = (Vec<MettaValue>, MettaEnvironment);

/// Represents a pattern matching rule: (= lhs rhs)
/// MettaValue is O(1) to clone (internally Arc-wrapped), so rules
/// are efficient to clone and match.
#[derive(Debug, Clone)]
pub struct Rule {
    pub lhs: MettaValue,
    pub rhs: MettaValue,
    /// Cached index into multiplicity counts array.
    /// Set during add_rule(), used for O(1) count lookup.
    /// None for rules created before being added to an environment.
    pub(crate) multiplicity_idx: Option<u32>,
}

impl Rule {
    /// Create a new rule from MettaValues
    pub fn new(lhs: MettaValue, rhs: MettaValue) -> Self {
        Rule {
            lhs,
            rhs,
            multiplicity_idx: None, // Set during add_rule()
        }
    }

    /// Create a new rule from MettaValues (alias for API compatibility)
    /// Since MettaValue is now O(1) to clone, this is equivalent to new()
    #[inline]
    pub fn from_arc(lhs: MettaValue, rhs: MettaValue) -> Self {
        Rule::new(lhs, rhs)
    }

    /// Create rule with pre-assigned index (for bulk operations)
    #[allow(dead_code)]
    pub(crate) fn with_index(lhs: MettaValue, rhs: MettaValue, idx: u32) -> Self {
        Rule {
            lhs,
            rhs,
            multiplicity_idx: Some(idx),
        }
    }

    /// Create rule from MettaValues with pre-assigned index (alias for API compatibility)
    #[inline]
    #[allow(dead_code)]
    pub(crate) fn from_arc_with_index(lhs: MettaValue, rhs: MettaValue, idx: u32) -> Self {
        Rule::with_index(lhs, rhs, idx)
    }

    /// Get a reference to the LHS pattern
    #[inline]
    pub fn lhs_ref(&self) -> &MettaValue {
        &self.lhs
    }

    /// Get a reference to the RHS template
    #[inline]
    pub fn rhs_ref(&self) -> &MettaValue {
        &self.rhs
    }

    /// Clone the RHS (O(1) operation since MettaValue uses Arc internally)
    #[inline]
    pub fn rhs_arc(&self) -> MettaValue {
        self.rhs.clone()
    }

    /// Clone the LHS (O(1) operation since MettaValue uses Arc internally)
    #[inline]
    pub fn lhs_arc(&self) -> MettaValue {
        self.lhs.clone()
    }
}
