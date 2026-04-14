//! Environment module for MeTTa evaluation.
//!
//! The Environment contains the fact database, type assertions, rules, and
//! various registries for MeTTa evaluation. Uses MORK/PathMap for efficient
//! trie-based storage with pattern matching support.
//!
//! # Architecture
//!
//! - `Environment` - Type alias for `MettaEnvironment` with Copy-on-Write (CoW) semantics
//! - `EnvironmentShared` - Type alias for `GenericEnvironmentShared<MettaValue>`
//! - `HeadArityBloomFilter` - O(1) rejection for match_space()
//! - `ScopeTracker` - Hierarchical scope tracking for "Did you mean?" suggestions
//!
//! # Thread Safety
//!
//! All shared state uses RwLock for concurrent read/exclusive write access.
//! Clone operations are O(1) via Arc sharing until first mutation.
//!
//! # Generic Environment
//!
//! The `GenericEnvironment<V, F>` type is parameterized over value type `V` and factory `F`.
//! This enables zero-conversion evaluation with different allocation strategies:
//!
//! - `MettaEnvironment` = `GenericEnvironment<MettaValue, GcFactory>` (O(1) Arc clone)
//! - `MettaEnvironment` = `GenericEnvironment<MettaValue, GcFactory>` (slab-allocated)

pub(crate) mod atom_space;
pub(crate) mod bloom;
pub(crate) mod dispatch_overrides;
mod fact_storage;
pub(crate) mod core;
mod grounded_ops;
mod module_ops;
pub(crate) mod mork_encoding;
pub(crate) mod multiplicity;
mod mutable_state;
mod named_spaces;
mod pattern_matching;
pub(crate) mod rule_management;
mod scope;
mod scope_ops;
mod suggestions;
mod symbol_bindings;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod proptests;
mod type_system;

pub use core::{GenericEnvironment, GenericEnvironmentShared, MettaEnvironment, MultiplicityMatch as GenericMultiplicityMatch};
pub use dispatch_overrides::{
    overridable_op_id, DispatchOverrides, OverridableOpId, NUM_OVERRIDABLE_OPS,
};
pub use named_spaces::NamedSpaceIter;
pub use pattern_matching::MultiplicityMatch;
pub use rule_management::RuleHeadsIter;
pub use scope::ScopeTracker;

use super::MettaValue;
