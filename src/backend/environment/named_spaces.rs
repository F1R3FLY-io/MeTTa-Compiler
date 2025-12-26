//! Named space operations for Environment.
//!
//! Provides methods for creating and managing named spaces (new-space, add-atom, remove-atom, collapse).

use std::sync::atomic::Ordering;

use super::{Environment, MettaValue};

impl Environment {
    /// Create a new named space and return its ID
    /// Used by new-space operation
    pub fn create_named_space(&mut self, name: &str) -> u64 {
        self.make_owned();

        let id = {
            let mut next_id = self
                .shared
                .next_space_id
                .write()
                .expect("next_space_id lock poisoned");
            let id = *next_id;
            *next_id += 1;
            id
        };

        self.shared
            .named_spaces
            .write()
            .expect("named_spaces lock poisoned")
            .insert(id, (name.to_string(), Vec::new()));

        self.modified.store(true, Ordering::Release);
        id
    }

    /// Add an atom to a named space by ID
    /// Used by add-atom operation
    pub fn add_to_named_space(&mut self, space_id: u64, value: &MettaValue) -> bool {
        self.make_owned();

        let mut spaces = self
            .shared
            .named_spaces
            .write()
            .expect("named_spaces lock poisoned");
        if let Some((_, atoms)) = spaces.get_mut(&space_id) {
            atoms.push(value.clone());
            self.modified.store(true, Ordering::Release);
            true
        } else {
            false
        }
    }

    /// Remove an atom from a named space by ID
    /// Used by remove-atom operation
    pub fn remove_from_named_space(&mut self, space_id: u64, value: &MettaValue) -> bool {
        self.make_owned();

        let mut spaces = self
            .shared
            .named_spaces
            .write()
            .expect("named_spaces lock poisoned");
        if let Some((_, atoms)) = spaces.get_mut(&space_id) {
            // Remove first matching atom
            if let Some(pos) = atoms.iter().position(|x| x == value) {
                atoms.remove(pos);
                self.modified.store(true, Ordering::Release);
                return true;
            }
        }
        false
    }

    /// Get all atoms from a named space as a list
    /// Used by collapse operation
    pub fn collapse_named_space(&self, space_id: u64) -> Vec<MettaValue> {
        let spaces = self
            .shared
            .named_spaces
            .read()
            .expect("named_spaces lock poisoned");
        if let Some((_, atoms)) = spaces.get(&space_id) {
            atoms.clone()
        } else {
            vec![]
        }
    }

    /// Check if a named space exists
    pub fn has_named_space(&self, space_id: u64) -> bool {
        self.shared
            .named_spaces
            .read()
            .expect("named_spaces lock poisoned")
            .contains_key(&space_id)
    }
}
