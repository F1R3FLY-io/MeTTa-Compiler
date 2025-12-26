//! Grounded operation access for Environment.
//!
//! Provides methods for accessing grounded (built-in) operations.

use std::sync::Arc;

use super::Environment;

impl Environment {
    /// Get a grounded operation by name (e.g., "+", "-", "and")
    /// Used for lazy evaluation of built-in operations
    pub fn get_grounded_operation(
        &self,
        name: &str,
    ) -> Option<Arc<dyn crate::backend::grounded::GroundedOperation>> {
        self.shared.grounded_registry.read().expect("grounded_registry lock poisoned").get(name)
    }

    /// Get a TCO-compatible grounded operation by name (e.g., "+", "-", "and")
    /// TCO operations return work items instead of calling eval internally,
    /// enabling deep recursion without stack overflow
    pub fn get_grounded_operation_tco(
        &self,
        name: &str,
    ) -> Option<Arc<dyn crate::backend::grounded::GroundedOperationTCO>> {
        self.shared.grounded_registry_tco.read().expect("grounded_registry_tco lock poisoned").get(name)
    }
}
