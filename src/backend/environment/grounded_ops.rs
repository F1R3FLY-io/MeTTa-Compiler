//! Grounded operation access for Environment.
//!
//! Provides methods for accessing grounded (built-in) operations.

use std::sync::Arc;

use super::generic::GenericEnvironment;
use crate::backend::grounded::{GenericGroundedState, GenericGroundedWork, GroundedOperation, GroundedOperationTCO};
use crate::backend::models::metta_value_trait::{MettaValueFactory, MettaValue as MettaValueTrait};
use crate::backend::MettaValue;

impl<V, F> GenericEnvironment<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    /// Get a grounded operation by name (e.g., "+", "-", "and")
    /// Used for lazy evaluation of built-in operations
    #[inline]
    pub fn get_grounded_operation(
        &self,
        name: &str,
    ) -> Option<Arc<dyn GroundedOperation>> {
        // parking_lot::RwLock - no .expect()
        self.shared.grounded_registry.read().get(name)
    }

    /// Get a TCO-compatible grounded operation by name (e.g., "+", "-", "and")
    /// TCO operations return work items instead of calling eval internally,
    /// enabling deep recursion without stack overflow
    #[inline]
    pub fn get_grounded_operation_tco(
        &self,
        name: &str,
    ) -> Option<Arc<dyn GroundedOperationTCO>> {
        // parking_lot::RwLock - no .expect()
        self.shared.grounded_registry_tco.read().get(name)
    }

    /// Check if a generic grounded operation exists by name.
    #[inline]
    pub fn has_generic_grounded_operation(&self, name: &str) -> bool {
        self.shared.generic_grounded_registry.contains(name)
    }
}

// MettaValue-specific grounded operations
impl super::Environment {
    /// Execute a generic grounded operation step.
    ///
    /// This uses the generic grounded registry which works with any value type
    /// implementing `MettaValueTrait`. For heap-allocated `MettaValue`, this
    /// eliminates conversions at grounded op boundaries.
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
