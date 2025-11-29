//! Space Handle - First-class queryable space values
//!
//! SpaceHandle allows spaces to be passed around as values and queried
//! independently of the Environment. This matches HE's design where
//! spaces are first-class values.

use std::sync::{Arc, RwLock};

use super::{MettaValue, Rule};

/// Thread-safe handle to a space's data.
///
/// SpaceHandle wraps the space data in Arc<RwLock<>> for:
/// - Cheap cloning (O(1) - just increments ref count)
/// - Thread-safe read/write access
/// - Shared ownership across MettaValue instances
#[derive(Debug, Clone)]
pub struct SpaceHandle {
    /// Unique identifier for this space
    pub id: u64,
    /// Human-readable name
    pub name: String,
    /// Thread-safe reference to the actual space data
    data: Arc<RwLock<SpaceData>>,
}

/// The actual data stored in a space.
#[derive(Debug, Clone, Default)]
pub struct SpaceData {
    /// Atoms stored in this space
    pub atoms: Vec<MettaValue>,
    /// Rules defined in this space (for matching)
    pub rules: Vec<Rule>,
}

impl SpaceHandle {
    /// Create a new space handle with the given ID and name.
    pub fn new(id: u64, name: String) -> Self {
        Self {
            id,
            name,
            data: Arc::new(RwLock::new(SpaceData::default())),
        }
    }

    /// Create a space handle with existing data.
    pub fn with_data(id: u64, name: String, atoms: Vec<MettaValue>) -> Self {
        Self {
            id,
            name,
            data: Arc::new(RwLock::new(SpaceData {
                atoms,
                rules: Vec::new(),
            })),
        }
    }

    /// Create a space handle that shares data with another handle.
    /// Used when creating references to the same underlying space.
    pub fn share_data(&self, new_id: u64, new_name: String) -> Self {
        Self {
            id: new_id,
            name: new_name,
            data: Arc::clone(&self.data),
        }
    }

    /// Add an atom to this space.
    pub fn add_atom(&self, atom: MettaValue) {
        let mut data = self.data.write().unwrap();
        data.atoms.push(atom);
    }

    /// Remove an atom from this space.
    /// Returns true if the atom was found and removed.
    pub fn remove_atom(&self, atom: &MettaValue) -> bool {
        let mut data = self.data.write().unwrap();
        if let Some(pos) = data.atoms.iter().position(|a| a == atom) {
            data.atoms.remove(pos);
            true
        } else {
            false
        }
    }

    /// Get all atoms in this space (collapse).
    pub fn collapse(&self) -> Vec<MettaValue> {
        let data = self.data.read().unwrap();
        data.atoms.clone()
    }

    /// Get the number of atoms in this space.
    pub fn atom_count(&self) -> usize {
        let data = self.data.read().unwrap();
        data.atoms.len()
    }

    /// Check if the space contains a specific atom.
    pub fn contains(&self, atom: &MettaValue) -> bool {
        let data = self.data.read().unwrap();
        data.atoms.contains(atom)
    }

    /// Add a rule to this space.
    pub fn add_rule(&self, rule: Rule) {
        let mut data = self.data.write().unwrap();
        data.rules.push(rule);
    }

    /// Get all rules in this space.
    pub fn rules(&self) -> Vec<Rule> {
        let data = self.data.read().unwrap();
        data.rules.clone()
    }

    /// Check if two space handles point to the same underlying data.
    pub fn same_space(&self, other: &SpaceHandle) -> bool {
        Arc::ptr_eq(&self.data, &other.data)
    }
}

impl PartialEq for SpaceHandle {
    fn eq(&self, other: &Self) -> bool {
        // Two space handles are equal if they have the same ID
        // (they may or may not share the same underlying data)
        self.id == other.id
    }
}

impl Eq for SpaceHandle {}

impl std::hash::Hash for SpaceHandle {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.id.hash(state);
        self.name.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_space_handle_new() {
        let handle = SpaceHandle::new(1, "test".to_string());
        assert_eq!(handle.id, 1);
        assert_eq!(handle.name, "test");
        assert_eq!(handle.atom_count(), 0);
    }

    #[test]
    fn test_space_handle_add_atom() {
        let handle = SpaceHandle::new(1, "test".to_string());
        handle.add_atom(MettaValue::Long(42));
        assert_eq!(handle.atom_count(), 1);
        assert!(handle.contains(&MettaValue::Long(42)));
    }

    #[test]
    fn test_space_handle_remove_atom() {
        let handle = SpaceHandle::new(1, "test".to_string());
        handle.add_atom(MettaValue::Long(42));
        assert!(handle.remove_atom(&MettaValue::Long(42)));
        assert_eq!(handle.atom_count(), 0);
        assert!(!handle.remove_atom(&MettaValue::Long(42)));
    }

    #[test]
    fn test_space_handle_collapse() {
        let handle = SpaceHandle::new(1, "test".to_string());
        handle.add_atom(MettaValue::Long(1));
        handle.add_atom(MettaValue::Long(2));
        let atoms = handle.collapse();
        assert_eq!(atoms.len(), 2);
    }

    #[test]
    fn test_space_handle_share_data() {
        let handle1 = SpaceHandle::new(1, "test".to_string());
        handle1.add_atom(MettaValue::Long(42));

        let handle2 = handle1.share_data(2, "alias".to_string());

        // Both handles should see the same data
        assert!(handle2.contains(&MettaValue::Long(42)));

        // Adding to one affects the other
        handle2.add_atom(MettaValue::Long(100));
        assert!(handle1.contains(&MettaValue::Long(100)));

        // But they have different IDs
        assert_ne!(handle1.id, handle2.id);
        assert!(handle1.same_space(&handle2));
    }

    #[test]
    fn test_space_handle_equality() {
        let handle1 = SpaceHandle::new(1, "test".to_string());
        let handle2 = SpaceHandle::new(1, "test".to_string());
        let handle3 = SpaceHandle::new(2, "test".to_string());

        assert_eq!(handle1, handle2);
        assert_ne!(handle1, handle3);
    }
}
