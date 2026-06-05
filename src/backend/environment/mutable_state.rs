//! Mutable state operations for Environment.
//!
//! Provides methods for creating and managing mutable state cells (new-state, get-state, change-state!).
//!
//! ## Storage Model
//!
//! States are stored directly as V values in the RwLock<HashMap<u64, V>>.
//! This enables zero-conversion evaluation - values are never serialized/deserialized.
//!
//! NOTE: States are truly mutable - they are created in the shared store and visible to all
//! environments sharing the same Arc<EnvironmentShared>. This matches MeTTa HE semantics
//! where change-state! is immediately observable.

use std::sync::atomic::Ordering;

use super::core::GenericEnvironment;
use crate::backend::models::metta_value_trait::{MettaValueFactory, MettaValueTrait};

impl<V, F> GenericEnvironment<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    /// Create a new mutable state cell with an initial value.
    ///
    /// States are stored directly as V (no serialization).
    /// States are truly mutable and globally visible (not CoW).
    pub fn create_state(&mut self, initial_value: &V) -> u64 {
        // No make_owned() - states are shared, not copy-on-write
        // AtomicU64 - use fetch_add for atomic increment
        let id = self.shared.next_state_id.fetch_add(1, Ordering::AcqRel);

        // Store value directly (no serialization)
        self.shared.states.write().insert(id, initial_value.clone());

        self.modified.store(true, Ordering::Release);
        id
    }

    /// Get the current value of a state cell.
    ///
    /// Returns the stored value directly (no deserialization needed).
    pub fn get_state(&self, state_id: u64) -> Option<V> {
        self.shared.states.read().get(&state_id).cloned()
    }

    /// Change the value of a state cell.
    ///
    /// Returns true if successful, false if state doesn't exist.
    /// States are truly mutable and changes are globally visible.
    pub fn change_state(&mut self, state_id: u64, new_value: &V) -> bool {
        // No make_owned() - states are shared, not copy-on-write
        #[cfg(feature = "index-gc")]
        let changed = super::core::with_env_satb_deletion_barrier(|satb_active| {
            let mut guard = self.shared.states.write();
            if let Some(entry) = guard.get_mut(&state_id) {
                let old = std::mem::replace(entry, new_value.clone());
                drop(guard);
                if satb_active {
                    super::core::shade_generic_values_for_satb(std::iter::once(old));
                }
                true
            } else {
                false
            }
        });
        #[cfg(not(feature = "index-gc"))]
        let changed = {
            let mut guard = self.shared.states.write();
            if let Some(entry) = guard.get_mut(&state_id) {
                *entry = new_value.clone();
                true
            } else {
                false
            }
        };
        if changed {
            self.modified.store(true, Ordering::Release);
        }
        changed
    }

    /// Check if a state cell exists.
    pub fn has_state(&self, state_id: u64) -> bool {
        self.shared.states.read().contains_key(&state_id)
    }
}
