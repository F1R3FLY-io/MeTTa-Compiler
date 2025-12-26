//! Mutable state operations for Environment.
//!
//! Provides methods for creating and managing mutable state cells (new-state, get-state, change-state!).
//!
//! NOTE: States are truly mutable - they are created in the shared store and visible to all
//! environments sharing the same Arc<EnvironmentShared>. This matches MeTTa HE semantics
//! where change-state! is immediately observable.

use std::sync::atomic::Ordering;

use super::{Environment, MettaValue};

impl Environment {
    /// Create a new mutable state cell with an initial value
    /// Used by new-state operation
    ///
    /// NOTE: States are truly mutable - they are created in the shared store
    /// and visible to all environments sharing the same Arc<EnvironmentShared>.
    /// We intentionally do NOT call make_owned() here because new states should
    /// be globally visible, matching MeTTa HE semantics.
    pub fn create_state(&mut self, initial_value: MettaValue) -> u64 {
        // No make_owned() - states are shared, not copy-on-write
        let id = {
            let mut next_id = self
                .shared
                .next_state_id
                .write()
                .expect("next_state_id lock poisoned");
            let id = *next_id;
            *next_id += 1;
            id
        };

        self.shared
            .states
            .write()
            .expect("states lock poisoned")
            .insert(id, initial_value);

        self.modified.store(true, Ordering::Release);
        id
    }

    /// Get the current value of a state cell
    /// Used by get-state operation
    pub fn get_state(&self, state_id: u64) -> Option<MettaValue> {
        self.shared
            .states
            .read()
            .expect("states lock poisoned")
            .get(&state_id)
            .cloned()
    }

    /// Change the value of a state cell
    /// Used by change-state! operation
    /// Returns true if successful, false if state doesn't exist
    ///
    /// NOTE: States are truly mutable - changes are visible to all environments
    /// sharing the same Arc<EnvironmentShared>. We intentionally do NOT call
    /// make_owned() here because state mutations should be globally visible,
    /// matching MeTTa HE semantics where change-state! is immediately observable.
    pub fn change_state(&mut self, state_id: u64, new_value: MettaValue) -> bool {
        // No make_owned() - states are shared, not copy-on-write
        let mut states = self.shared.states.write().expect("states lock poisoned");
        if let std::collections::hash_map::Entry::Occupied(mut e) = states.entry(state_id) {
            e.insert(new_value);
            self.modified.store(true, Ordering::Release);
            true
        } else {
            false
        }
    }

    /// Check if a state cell exists
    pub fn has_state(&self, state_id: u64) -> bool {
        self.shared
            .states
            .read()
            .expect("states lock poisoned")
            .contains_key(&state_id)
    }
}
