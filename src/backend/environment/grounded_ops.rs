//! Grounded operation access for Environment.
//!
//! Provides methods for accessing grounded (built-in) operations.
//! Active code uses `GenericGroundedRegistry` with static dispatch.

use super::generic::GenericEnvironment;
use crate::backend::grounded::{GenericGroundedState, GenericGroundedWork};
use crate::backend::models::metta_value_trait::{MettaValueFactory, MettaValueTrait};
use crate::backend::MettaValue;

impl<V, F> GenericEnvironment<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    /// Check if a generic grounded operation exists by name.
    #[inline]
    pub fn has_generic_grounded_operation(&self, name: &str) -> bool {
        self.shared.generic_grounded_registry.contains(name)
    }
}

// MettaValue-specific grounded operations
impl super::MettaEnvironment {
    /// Execute a generic grounded operation step.
    ///
    /// Uses the generic grounded registry with static dispatch (no trait objects).
    ///
    /// # Returns
    ///
    /// - `Some(work)` if the operation was found and executed
    /// - `None` if the operation was not found in the registry
    #[inline]
    pub fn execute_generic_grounded_step(
        &self,
        name: &str,
        state: &mut GenericGroundedState<MettaValue>,
    ) -> Option<GenericGroundedWork<MettaValue>> {
        self.shared.generic_grounded_registry.execute_step_heap(name, state)
    }
}
