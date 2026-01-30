pub mod atom_id;
pub mod bindings;
pub mod indexed_multiset;
pub mod memo_handle;
pub mod metta_state;
pub mod metta_value;
pub mod multiset;
pub mod space_handle;

pub use atom_id::{AtomId, SymbolTable};
pub use bindings::SmartBindings as Bindings;
pub use indexed_multiset::IndexedMultiset;
pub use memo_handle::MemoHandle;
pub use metta_state::MettaState;
pub use metta_value::{ArcValue, MettaValue, MettaValueInner};
pub use multiset::{AtomMultiset, AtomMultisetSnapshot};
pub use space_handle::SpaceHandle;

use crate::backend::environment::Environment;

/// Result of evaluation: (result, new_environment)
pub type EvalResult = (Vec<MettaValue>, Environment);

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

impl std::hash::Hash for Rule {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.lhs.hash(state);
        self.rhs.hash(state);
        // Note: multiplicity_idx is excluded as it's a cache, not part of rule identity
    }
}

impl PartialEq for Rule {
    fn eq(&self, other: &Self) -> bool {
        self.lhs == other.lhs && self.rhs == other.rhs
        // Note: multiplicity_idx is excluded as it's a cache, not part of rule identity
    }
}

impl Eq for Rule {}

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
    pub(crate) fn with_index(lhs: MettaValue, rhs: MettaValue, idx: u32) -> Self {
        Rule {
            lhs,
            rhs,
            multiplicity_idx: Some(idx),
        }
    }

    /// Create rule from MettaValues with pre-assigned index (alias for API compatibility)
    #[inline]
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
