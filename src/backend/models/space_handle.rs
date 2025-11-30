//! Space Handle - First-class queryable space values
//!
//! SpaceHandle allows spaces to be passed around as values and queried
//! independently of the Environment. This matches HE's design where
//! spaces are first-class values.
//!
//! SpaceHandle supports two backing stores:
//! - `SpaceData` - For dynamically created spaces (`new-space`)
//! - `ModuleSpace` - For module-backed spaces (`mod-space!`) with live references

use std::sync::{Arc, RwLock};

use super::{MettaValue, Rule};
use crate::backend::modules::{ModId, ModuleSpace};

/// The backing store for a SpaceHandle.
///
/// This enum allows SpaceHandle to work with both:
/// - Standalone spaces (from `new-space`)
/// - Module spaces (from `mod-space!`) with live references
#[derive(Debug, Clone)]
pub enum SpaceBacking {
    /// Owned space data (for new-space)
    Owned(Arc<RwLock<SpaceData>>),
    /// Module-backed space (for mod-space!) with live reference
    Module {
        mod_id: ModId,
        space: Arc<RwLock<ModuleSpace>>,
    },
}

/// Thread-safe handle to a space's data.
///
/// SpaceHandle wraps the space data in Arc<RwLock<>> for:
/// - Cheap cloning (O(1) - just increments ref count)
/// - Thread-safe read/write access
/// - Shared ownership across MettaValue instances
///
/// For module-backed spaces, mutations are immediately visible to all
/// holders of the space reference (live reference semantics).
#[derive(Debug, Clone)]
pub struct SpaceHandle {
    /// Unique identifier for this space
    pub id: u64,
    /// Human-readable name
    pub name: String,
    /// The backing store (owned SpaceData or live ModuleSpace reference)
    backing: SpaceBacking,
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
            backing: SpaceBacking::Owned(Arc::new(RwLock::new(SpaceData::default()))),
        }
    }

    /// Create a space handle with existing data.
    pub fn with_data(id: u64, name: String, atoms: Vec<MettaValue>) -> Self {
        Self {
            id,
            name,
            backing: SpaceBacking::Owned(Arc::new(RwLock::new(SpaceData {
                atoms,
                rules: Vec::new(),
            }))),
        }
    }

    /// Create a space handle backed by a module's space (live reference).
    ///
    /// This provides live reference semantics where mutations are immediately
    /// visible to all holders of the space reference.
    ///
    /// # Arguments
    /// - `mod_id` - The module's unique identifier
    /// - `name` - Human-readable name for the space
    /// - `space` - Arc reference to the module's ModuleSpace
    pub fn for_module(mod_id: ModId, name: String, space: Arc<RwLock<ModuleSpace>>) -> Self {
        Self {
            id: mod_id.value(),
            name,
            backing: SpaceBacking::Module { mod_id, space },
        }
    }

    /// Check if this space is backed by a module.
    pub fn is_module_space(&self) -> bool {
        matches!(self.backing, SpaceBacking::Module { .. })
    }

    /// Get the ModId if this is a module-backed space.
    pub fn module_id(&self) -> Option<ModId> {
        match &self.backing {
            SpaceBacking::Module { mod_id, .. } => Some(*mod_id),
            SpaceBacking::Owned(_) => None,
        }
    }

    /// Create a space handle that shares data with another handle.
    /// Used when creating references to the same underlying space.
    pub fn share_data(&self, new_id: u64, new_name: String) -> Self {
        Self {
            id: new_id,
            name: new_name,
            backing: self.backing.clone(),
        }
    }

    /// Add an atom to this space.
    pub fn add_atom(&self, atom: MettaValue) {
        match &self.backing {
            SpaceBacking::Owned(data) => {
                let mut data = data.write().unwrap();
                data.atoms.push(atom);
            }
            SpaceBacking::Module { space, .. } => {
                let mut space = space.write().unwrap();
                space.add_atom(atom);
            }
        }
    }

    /// Remove an atom from this space.
    /// Returns true if the atom was found and removed.
    pub fn remove_atom(&self, atom: &MettaValue) -> bool {
        match &self.backing {
            SpaceBacking::Owned(data) => {
                let mut data = data.write().unwrap();
                if let Some(pos) = data.atoms.iter().position(|a| a == atom) {
                    data.atoms.remove(pos);
                    true
                } else {
                    false
                }
            }
            SpaceBacking::Module { space, .. } => {
                let mut space = space.write().unwrap();
                space.remove_atom(atom)
            }
        }
    }

    /// Get all atoms in this space (collapse).
    pub fn collapse(&self) -> Vec<MettaValue> {
        match &self.backing {
            SpaceBacking::Owned(data) => {
                let data = data.read().unwrap();
                data.atoms.clone()
            }
            SpaceBacking::Module { space, .. } => {
                let space = space.read().unwrap();
                space.get_all_atoms()
            }
        }
    }

    /// Get the number of atoms in this space.
    pub fn atom_count(&self) -> usize {
        match &self.backing {
            SpaceBacking::Owned(data) => {
                let data = data.read().unwrap();
                data.atoms.len()
            }
            SpaceBacking::Module { space, .. } => {
                let space = space.read().unwrap();
                space.get_all_atoms().len()
            }
        }
    }

    /// Check if the space contains a specific atom.
    pub fn contains(&self, atom: &MettaValue) -> bool {
        match &self.backing {
            SpaceBacking::Owned(data) => {
                let data = data.read().unwrap();
                data.atoms.contains(atom)
            }
            SpaceBacking::Module { space, .. } => {
                let space = space.read().unwrap();
                space.contains(atom)
            }
        }
    }

    /// Add a rule to this space.
    /// Note: For module spaces, rules are stored in the Environment, not ModuleSpace.
    pub fn add_rule(&self, rule: Rule) {
        match &self.backing {
            SpaceBacking::Owned(data) => {
                let mut data = data.write().unwrap();
                data.rules.push(rule);
            }
            SpaceBacking::Module { .. } => {
                // Module spaces store rules in Environment, not here
                // This is a no-op for module spaces (rules added via eval)
            }
        }
    }

    /// Get all rules in this space.
    /// Note: For module spaces, returns empty (rules are in Environment).
    pub fn rules(&self) -> Vec<Rule> {
        match &self.backing {
            SpaceBacking::Owned(data) => {
                let data = data.read().unwrap();
                data.rules.clone()
            }
            SpaceBacking::Module { .. } => {
                // Module rules are stored in Environment, not ModuleSpace
                Vec::new()
            }
        }
    }

    /// Check if two space handles point to the same underlying data.
    pub fn same_space(&self, other: &SpaceHandle) -> bool {
        match (&self.backing, &other.backing) {
            (SpaceBacking::Owned(a), SpaceBacking::Owned(b)) => Arc::ptr_eq(a, b),
            (
                SpaceBacking::Module { mod_id: a, .. },
                SpaceBacking::Module { mod_id: b, .. },
            ) => a == b,
            _ => false,
        }
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
