//! Environment module for MeTTa evaluation.
//!
//! The Environment contains the fact database, type assertions, rules, and
//! various registries for MeTTa evaluation. Uses MORK PathMap for efficient
//! trie-based storage with pattern matching support.
//!
//! # Architecture
//!
//! - `Environment` - Type alias for `HeapEnvironment` with Copy-on-Write (CoW) semantics
//! - `EnvironmentShared` - Type alias for `GenericEnvironmentShared<MettaValue>`
//! - `HeadArityBloomFilter` - O(1) rejection for match_space()
//! - `ScopeTracker` - Hierarchical scope tracking for "Did you mean?" suggestions
//!
//! # Thread Safety
//!
//! All shared state uses RwLock/DashMap for concurrent read/exclusive write access.
//! Clone operations are O(1) via Arc sharing until first mutation.
//!
//! # Generic Environment
//!
//! The `GenericEnvironment<V, F>` type is parameterized over value type `V` and factory `F`.
//! This enables zero-conversion evaluation with different allocation strategies:
//!
//! - `HeapEnvironment` = `GenericEnvironment<MettaValue, HeapMettaValueFactory>` (O(1) Arc clone)
//! - `ArenaEnvironment<'a>` = `GenericEnvironment<ArenaValue<'a>, ArenaValueFactory<'a>>` (zero-copy)

mod bloom;
mod fact_storage;
pub(crate) mod generic;
mod grounded_ops;
mod module_ops;
pub(crate) mod mork_encoding;
pub(crate) mod multiplicity;
mod mutable_state;
mod named_spaces;
mod pattern_matching;
mod rule_management;
mod scope;
mod scope_ops;
mod suggestions;
mod symbol_bindings;
#[cfg(test)]
mod tests;
mod type_system;

pub(crate) use bloom::HeadArityBloomFilter;
pub use generic::{GenericEnvironment, GenericEnvironmentShared, HeapEnvironment, MultiplicityMatch as GenericMultiplicityMatch};
pub use named_spaces::NamedSpaceIter;
pub use pattern_matching::MultiplicityMatch;
pub use rule_management::{MatchingRulesIter, RuleHeadsIter, RulesIter};
pub use scope::ScopeTracker;

use std::collections::HashMap;

use mork::space::Space;
use tracing::trace;

use super::models::MettaValueInner;
use super::{MettaValue, Rule};

use multiplicity::Multiplicity;

// ============================================================================
// Type Aliases - Environment is now HeapEnvironment
// ============================================================================

/// The primary environment type for MeTTa evaluation.
///
/// This is a type alias for `HeapEnvironment`, which is `GenericEnvironment<MettaValue, HeapMettaValueFactory>`.
/// MettaValue uses Arc internally, so clone is O(1).
///
/// ## Thread-Safe Copy-on-Write (CoW) Semantics
///
/// - Clones share data until first modification (owns_data = false)
/// - First mutation triggers deep copy via make_owned() (owns_data = true)
/// - parking_lot::RwLock enables concurrent reads
/// - DashMap enables lock-free reads for rule/binding lookups
///
/// ## Performance
///
/// - Clone: O(1) - single Arc increment
/// - First mutation after clone: O(n) deep copy
/// - Subsequent mutations: O(1) in-place
pub type Environment = HeapEnvironment;

/// The shared state type for Environment.
///
/// This is a type alias for `GenericEnvironmentShared<MettaValue>`.
pub type EnvironmentShared = GenericEnvironmentShared<MettaValue>;

// ============================================================================
// Environment-specific Helper Methods
// ============================================================================

impl Environment {
    /// Recursively fork all SpaceHandles in a MettaValue.
    /// Returns a new MettaValue with forked spaces, or the original if no spaces found.
    pub(crate) fn fork_spaces_in_value(value: &MettaValue) -> MettaValue {
        match value.inner() {
            MettaValueInner::Space(handle) => {
                // Fork the space handle for isolation
                MettaValue::Space(handle.fork())
            }
            MettaValueInner::SExpr(items) => {
                let forked_items: Vec<MettaValue> = items
                    .iter()
                    .map(|item| Self::fork_spaces_in_value(item))
                    .collect();
                MettaValue::SExpr(forked_items)
            }
            MettaValueInner::Conjunction(goals) => {
                let forked_goals: Vec<MettaValue> = goals
                    .iter()
                    .map(|goal| Self::fork_spaces_in_value(goal))
                    .collect();
                MettaValue::Conjunction(forked_goals)
            }
            MettaValueInner::Type(inner) => {
                // Recursively fork spaces in type value
                MettaValue::Type(Self::fork_spaces_in_value(inner))
            }
            MettaValueInner::Error(msg, details) => {
                // Recursively fork spaces in error details
                MettaValue::Error(msg.clone(), Self::fork_spaces_in_value(details))
            }
            // Primitives don't contain spaces - return clone (O(1) due to Arc)
            MettaValueInner::Atom(_)
            | MettaValueInner::Bool(_)
            | MettaValueInner::Long(_)
            | MettaValueInner::Float(_)
            | MettaValueInner::String(_)
            | MettaValueInner::Nil
            | MettaValueInner::State(_)
            | MettaValueInner::Unit
            | MettaValueInner::Memo(_)
            | MettaValueInner::Empty => value.clone(),
        }
    }

    /// Create a thread-local Space for operations with explicit HashMap type.
    /// Following the Rholang LSP pattern: cheap clone via structural sharing.
    ///
    /// This variant creates a Space with an explicitly empty HashMap for mmaps,
    /// which is needed for some legacy code paths.
    pub fn create_space_with_mmaps(&self) -> Space<Multiplicity> {
        let btm = self.shared.btm.read().clone();
        Space {
            btm,
            sm: self.shared_mapping.clone(),
            mmaps: HashMap::new(),
        }
    }

    /// Fork this environment for nondeterministic branch isolation.
    ///
    /// This wraps `GenericEnvironment::fork_for_nondeterminism()` and additionally
    /// forks SpaceHandles in bindings for proper branch isolation.
    pub fn fork_for_nondeterminism_with_spaces(&self) -> Environment {
        trace!(target: "mettatron::environment::fork", "Forking environment for nondeterminism with space isolation");

        // First, do the standard fork
        let mut forked = self.fork_for_nondeterminism();

        // Then fork all SpaceHandles in bindings for branch isolation
        // We need to iterate over bindings and update them
        let binding_keys: Vec<String> = forked.shared.bindings.iter()
            .map(|entry| entry.key().clone())
            .collect();

        for key in binding_keys {
            if let Some(value) = forked.shared.bindings.get(&key) {
                let forked_value = Self::fork_spaces_in_value(&value);
                forked.shared.bindings.insert(key, forked_value);
            }
        }

        forked
    }
}
