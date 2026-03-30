//! Grounded operation access for Environment.
//!
//! Provides methods for accessing grounded (built-in) operations.
//! Active code uses `GroundedRegistry` with static dispatch.

use super::core::GenericEnvironment;
use crate::backend::grounded::{GroundedState, GroundedWork};
use crate::backend::models::metta_value_trait::{MettaValueFactory, MettaValueTrait};
use crate::backend::MettaValue;

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

// MettaValue-specific grounded operations
impl super::MettaEnvironment {
    /// Execute a grounded operation step.
    ///
    /// Uses the grounded registry with static dispatch (no trait objects).
    ///
    /// # Returns
    ///
    /// - `Some(work)` if the operation was found and executed
    /// - `None` if the operation was not found in the registry
    #[inline]
    pub fn execute_grounded_step(
        &self,
        name: &str,
        state: &mut GroundedState<MettaValue>,
    ) -> Option<GroundedWork<MettaValue>> {
        self.shared.grounded_registry.execute_step_heap(name, state)
    }
}
