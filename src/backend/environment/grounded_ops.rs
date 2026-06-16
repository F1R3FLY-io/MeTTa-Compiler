//! Grounded operation access for Environment.
//!
//! Provides methods for accessing grounded (built-in) operations.
//! Active code uses `GroundedRegistry` with static dispatch.

use super::core::GenericEnvironment;
use crate::backend::models::metta_value_trait::{MettaValueFactory, MettaValueTrait};

impl<V, F> GenericEnvironment<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    /// Check if a grounded operation exists by name.
    #[inline]
    pub fn has_grounded_operation(&self, name: &str) -> bool {
        self.shared.grounded_registry.contains(name)
    }
}
