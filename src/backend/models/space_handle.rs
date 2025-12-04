//! Space Handle - First-class queryable space values with Copy-on-Write semantics
//!
//! SpaceHandle allows spaces to be passed around as values and queried
//! independently of the Environment. This matches HE's design where
//! spaces are first-class values.
//!
//! ## Copy-on-Write (CoW) for Nondeterministic Branch Isolation
//!
//! When MeTTa evaluation forks into nondeterministic branches (e.g., from `match`
//! returning multiple results), each branch needs isolated access to mutable state.
//! Without isolation, one branch's `add-atom` affects all other branches.
//!
//! CoW semantics solve this:
//! - `fork()` creates a logical copy that shares base data (O(1) operation)
//! - First write to forked space copies data to local overlay
//! - Each branch sees its own modifications without affecting others
//!
//! SpaceHandle supports two backing stores:
//! - `SpaceData` - For dynamically created spaces (`new-space`)
//! - `ModuleSpace` - For module-backed spaces (`mod-space!`) with live references

use std::sync::{Arc, RwLock};

use super::{MettaValue, Rule};
use crate::backend::modules::{ModId, ModuleSpace};

/// Local modifications overlay for Copy-on-Write semantics.
///
/// When a space is forked, the overlay tracks local changes without modifying
/// the shared base. This enables nondeterministic branch isolation.
#[derive(Debug, Clone, Default)]
pub struct SpaceOverlay {
    /// Atoms added in this fork (local additions)
    pub added: Vec<MettaValue>,
    /// Atoms removed in this fork (tombstones)
    /// Stored as Vec since MettaValue doesn't implement Hash
    pub removed: Vec<MettaValue>,
    /// Rules added in this fork
    pub added_rules: Vec<Rule>,
}

impl SpaceOverlay {
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if an atom was removed in this overlay
    pub fn is_removed(&self, atom: &MettaValue) -> bool {
        self.removed.iter().any(|r| r == atom)
    }
}

/// The backing store for a SpaceHandle.
///
/// This enum allows SpaceHandle to work with both:
/// - Standalone spaces (from `new-space`) with optional CoW overlay
/// - Module spaces (from `mod-space!`) with live references
#[derive(Debug, Clone)]
pub enum SpaceBacking {
    /// Owned space data (for new-space) with optional CoW overlay
    Owned {
        /// Base space data (shared, read-only after fork)
        base: Arc<RwLock<SpaceData>>,
        /// Overlay for local modifications (None = no local changes yet)
        /// When Some, this branch has been forked and modifications go here
        overlay: Option<Arc<RwLock<SpaceOverlay>>>,
    },
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
            backing: SpaceBacking::Owned {
                base: Arc::new(RwLock::new(SpaceData::default())),
                overlay: None,
            },
        }
    }

    /// Create a space handle with existing data.
    pub fn with_data(id: u64, name: String, atoms: Vec<MettaValue>) -> Self {
        Self {
            id,
            name,
            backing: SpaceBacking::Owned {
                base: Arc::new(RwLock::new(SpaceData {
                    atoms,
                    rules: Vec::new(),
                })),
                overlay: None,
            },
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

    /// Fork this space handle for nondeterministic branch isolation.
    ///
    /// Creates a new handle that shares the base data but has its own overlay
    /// for local modifications. This is an O(1) operation - actual data copying
    /// only happens on first write (Copy-on-Write semantics).
    ///
    /// # Example
    /// ```ignore
    /// let original = SpaceHandle::new(1, "stack".to_string());
    /// original.add_atom(MettaValue::Long(1));
    ///
    /// let forked = original.fork();
    /// forked.add_atom(MettaValue::Long(2));
    ///
    /// // original sees: [1]
    /// // forked sees: [1, 2]
    /// ```
    pub fn fork(&self) -> Self {
        match &self.backing {
            SpaceBacking::Owned { base, overlay } => {
                // If we already have an overlay, we need to create a new base that
                // represents the current state (base + overlay) for the forked child.
                // This ensures proper isolation.
                if overlay.is_some() {
                    // Materialize current state into a new base
                    let current_atoms = self.collapse();
                    let current_rules = self.rules();
                    Self {
                        id: self.id,
                        name: self.name.clone(),
                        backing: SpaceBacking::Owned {
                            base: Arc::new(RwLock::new(SpaceData {
                                atoms: current_atoms,
                                rules: current_rules,
                            })),
                            overlay: Some(Arc::new(RwLock::new(SpaceOverlay::new()))),
                        },
                    }
                } else {
                    // No overlay yet - create forked handle with fresh overlay
                    Self {
                        id: self.id,
                        name: self.name.clone(),
                        backing: SpaceBacking::Owned {
                            base: Arc::clone(base),
                            overlay: Some(Arc::new(RwLock::new(SpaceOverlay::new()))),
                        },
                    }
                }
            }
            SpaceBacking::Module { mod_id, space } => {
                // Module spaces: for now, fork creates a snapshot (not live)
                // This gives each branch its own isolated copy
                let atoms = space.read().unwrap().get_all_atoms();
                Self {
                    id: self.id,
                    name: self.name.clone(),
                    backing: SpaceBacking::Owned {
                        base: Arc::new(RwLock::new(SpaceData {
                            atoms,
                            rules: Vec::new(),
                        })),
                        overlay: Some(Arc::new(RwLock::new(SpaceOverlay::new()))),
                    },
                }
            }
        }
    }

    /// Check if this space has been forked (has an overlay).
    pub fn is_forked(&self) -> bool {
        matches!(
            &self.backing,
            SpaceBacking::Owned {
                overlay: Some(_),
                ..
            }
        )
    }

    /// Check if this space is backed by a module.
    pub fn is_module_space(&self) -> bool {
        matches!(self.backing, SpaceBacking::Module { .. })
    }

    /// Get the ModId if this is a module-backed space.
    pub fn module_id(&self) -> Option<ModId> {
        match &self.backing {
            SpaceBacking::Module { mod_id, .. } => Some(*mod_id),
            SpaceBacking::Owned { .. } => None,
        }
    }

    /// Create a space handle that shares data with another handle.
    /// Used when creating references to the same underlying space.
    ///
    /// Note: This creates a shallow clone - both handles share the same base AND overlay.
    /// For isolated copies, use `fork()` instead.
    pub fn share_data(&self, new_id: u64, new_name: String) -> Self {
        Self {
            id: new_id,
            name: new_name,
            backing: self.backing.clone(),
        }
    }

    /// Add an atom to this space.
    ///
    /// If forked (has overlay), adds to overlay.
    /// Otherwise, adds directly to base.
    pub fn add_atom(&self, atom: MettaValue) {
        match &self.backing {
            SpaceBacking::Owned { base, overlay } => {
                if let Some(overlay) = overlay {
                    // Forked: add to overlay
                    let mut overlay = overlay.write().unwrap();
                    // If this atom was previously removed, un-remove it
                    if let Some(pos) = overlay.removed.iter().position(|r| r == &atom) {
                        overlay.removed.remove(pos);
                    }
                    overlay.added.push(atom);
                } else {
                    // Not forked: add directly to base
                    let mut data = base.write().unwrap();
                    data.atoms.push(atom);
                }
            }
            SpaceBacking::Module { space, .. } => {
                let mut space = space.write().unwrap();
                space.add_atom(atom);
            }
        }
    }

    /// Remove an atom from this space.
    /// Returns true if the atom was found and removed.
    ///
    /// If forked (has overlay), adds tombstone to overlay.
    /// Otherwise, removes directly from base.
    pub fn remove_atom(&self, atom: &MettaValue) -> bool {
        match &self.backing {
            SpaceBacking::Owned { base, overlay } => {
                if let Some(overlay) = overlay {
                    // Forked: check if atom exists (in base or overlay.added)
                    let mut overlay_lock = overlay.write().unwrap();

                    // First check if it was added in this overlay
                    if let Some(pos) = overlay_lock.added.iter().position(|a| a == atom) {
                        overlay_lock.added.remove(pos);
                        return true;
                    }

                    // Check if it exists in base (and not already removed)
                    let base_data = base.read().unwrap();
                    if base_data.atoms.contains(atom) && !overlay_lock.is_removed(atom) {
                        // Add tombstone
                        overlay_lock.removed.push(atom.clone());
                        return true;
                    }

                    false
                } else {
                    // Not forked: remove directly from base
                    let mut data = base.write().unwrap();
                    if let Some(pos) = data.atoms.iter().position(|a| a == atom) {
                        data.atoms.remove(pos);
                        true
                    } else {
                        false
                    }
                }
            }
            SpaceBacking::Module { space, .. } => {
                let mut space = space.write().unwrap();
                space.remove_atom(atom)
            }
        }
    }

    /// Get all atoms in this space (collapse).
    ///
    /// If forked, returns: (base atoms - removed) + added
    pub fn collapse(&self) -> Vec<MettaValue> {
        match &self.backing {
            SpaceBacking::Owned { base, overlay } => {
                let base_data = base.read().unwrap();

                if let Some(overlay) = overlay {
                    let overlay_lock = overlay.read().unwrap();

                    // Start with base atoms, filter out removed ones
                    let mut result: Vec<MettaValue> = base_data
                        .atoms
                        .iter()
                        .filter(|atom| !overlay_lock.is_removed(atom))
                        .cloned()
                        .collect();

                    // Add atoms from overlay
                    result.extend(overlay_lock.added.iter().cloned());

                    result
                } else {
                    base_data.atoms.clone()
                }
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
            SpaceBacking::Owned { base, overlay } => {
                if let Some(overlay) = overlay {
                    let base_data = base.read().unwrap();
                    let overlay_lock = overlay.read().unwrap();

                    // Count = base - removed + added
                    let base_count = base_data.atoms.len();
                    let removed_count = overlay_lock.removed.len();
                    let added_count = overlay_lock.added.len();

                    base_count.saturating_sub(removed_count) + added_count
                } else {
                    let data = base.read().unwrap();
                    data.atoms.len()
                }
            }
            SpaceBacking::Module { space, .. } => {
                let space = space.read().unwrap();
                space.get_all_atoms().len()
            }
        }
    }

    /// Check if the space contains a specific atom.
    ///
    /// If forked, checks overlay first (added/removed), then base.
    pub fn contains(&self, atom: &MettaValue) -> bool {
        match &self.backing {
            SpaceBacking::Owned { base, overlay } => {
                if let Some(overlay) = overlay {
                    let overlay_lock = overlay.read().unwrap();

                    // Check if removed
                    if overlay_lock.is_removed(atom) {
                        return false;
                    }

                    // Check if added
                    if overlay_lock.added.contains(atom) {
                        return true;
                    }

                    // Check base
                    let base_data = base.read().unwrap();
                    base_data.atoms.contains(atom)
                } else {
                    let data = base.read().unwrap();
                    data.atoms.contains(atom)
                }
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
            SpaceBacking::Owned { base, overlay } => {
                if let Some(overlay) = overlay {
                    // Forked: add to overlay
                    let mut overlay_lock = overlay.write().unwrap();
                    overlay_lock.added_rules.push(rule);
                } else {
                    // Not forked: add directly to base
                    let mut data = base.write().unwrap();
                    data.rules.push(rule);
                }
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
            SpaceBacking::Owned { base, overlay } => {
                let base_data = base.read().unwrap();

                if let Some(overlay) = overlay {
                    let overlay_lock = overlay.read().unwrap();
                    let mut rules = base_data.rules.clone();
                    rules.extend(overlay_lock.added_rules.iter().cloned());
                    rules
                } else {
                    base_data.rules.clone()
                }
            }
            SpaceBacking::Module { .. } => {
                // Module rules are stored in Environment, not ModuleSpace
                Vec::new()
            }
        }
    }

    /// Check if two space handles point to the same underlying data.
    ///
    /// Note: Two forked handles from the same base are NOT the same space
    /// (they have different overlays).
    pub fn same_space(&self, other: &SpaceHandle) -> bool {
        match (&self.backing, &other.backing) {
            (
                SpaceBacking::Owned {
                    base: a,
                    overlay: ao,
                },
                SpaceBacking::Owned {
                    base: b,
                    overlay: bo,
                },
            ) => {
                // Same base AND same overlay (or both None)
                Arc::ptr_eq(a, b)
                    && match (ao, bo) {
                        (None, None) => true,
                        (Some(ao), Some(bo)) => Arc::ptr_eq(ao, bo),
                        _ => false,
                    }
            }
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
        assert!(!handle.is_forked());
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

    // ============================================================
    // Copy-on-Write (CoW) Fork Tests
    // ============================================================

    #[test]
    fn test_fork_creates_isolated_copy() {
        let original = SpaceHandle::new(1, "stack".to_string());
        original.add_atom(MettaValue::Long(1));

        // Fork creates isolated copy
        let forked = original.fork();
        assert!(forked.is_forked());

        // Forked sees original data
        assert!(forked.contains(&MettaValue::Long(1)));
        assert_eq!(forked.atom_count(), 1);

        // Add to forked - should NOT affect original
        forked.add_atom(MettaValue::Long(2));

        assert_eq!(forked.atom_count(), 2);
        assert!(forked.contains(&MettaValue::Long(2)));

        // Original should NOT see the new atom
        assert_eq!(original.atom_count(), 1);
        assert!(!original.contains(&MettaValue::Long(2)));
    }

    #[test]
    fn test_fork_remove_isolation() {
        let original = SpaceHandle::new(1, "stack".to_string());
        original.add_atom(MettaValue::Long(1));
        original.add_atom(MettaValue::Long(2));

        // Fork
        let forked = original.fork();

        // Remove from forked - should NOT affect original
        assert!(forked.remove_atom(&MettaValue::Long(1)));

        // Forked should no longer contain the removed atom
        assert!(!forked.contains(&MettaValue::Long(1)));
        assert_eq!(forked.atom_count(), 1);

        // Original should still have it
        assert!(original.contains(&MettaValue::Long(1)));
        assert_eq!(original.atom_count(), 2);
    }

    #[test]
    fn test_fork_collapse_merges_correctly() {
        let original = SpaceHandle::new(1, "stack".to_string());
        original.add_atom(MettaValue::Long(1));
        original.add_atom(MettaValue::Long(2));

        let forked = original.fork();
        forked.add_atom(MettaValue::Long(3));
        forked.remove_atom(&MettaValue::Long(1));

        // Collapse should return: [2, 3] (original minus removed plus added)
        let atoms = forked.collapse();
        assert_eq!(atoms.len(), 2);
        assert!(!atoms.contains(&MettaValue::Long(1)));
        assert!(atoms.contains(&MettaValue::Long(2)));
        assert!(atoms.contains(&MettaValue::Long(3)));

        // Original collapse should still return [1, 2]
        let orig_atoms = original.collapse();
        assert_eq!(orig_atoms.len(), 2);
        assert!(orig_atoms.contains(&MettaValue::Long(1)));
        assert!(orig_atoms.contains(&MettaValue::Long(2)));
    }

    #[test]
    fn test_fork_from_fork() {
        // Test nested forking
        let original = SpaceHandle::new(1, "stack".to_string());
        original.add_atom(MettaValue::Long(1));

        let fork1 = original.fork();
        fork1.add_atom(MettaValue::Long(2));

        let fork2 = fork1.fork();
        fork2.add_atom(MettaValue::Long(3));

        // fork2 should see all: [1, 2, 3]
        assert_eq!(fork2.atom_count(), 3);
        assert!(fork2.contains(&MettaValue::Long(1)));
        assert!(fork2.contains(&MettaValue::Long(2)));
        assert!(fork2.contains(&MettaValue::Long(3)));

        // fork1 should see: [1, 2]
        assert_eq!(fork1.atom_count(), 2);
        assert!(!fork1.contains(&MettaValue::Long(3)));

        // original should see: [1]
        assert_eq!(original.atom_count(), 1);
    }

    #[test]
    fn test_fork_re_add_removed_atom() {
        let original = SpaceHandle::new(1, "stack".to_string());
        original.add_atom(MettaValue::Long(1));

        let forked = original.fork();

        // Remove and re-add
        forked.remove_atom(&MettaValue::Long(1));
        assert!(!forked.contains(&MettaValue::Long(1)));

        forked.add_atom(MettaValue::Long(1));
        assert!(forked.contains(&MettaValue::Long(1)));
    }

    #[test]
    fn test_fork_same_space_returns_false() {
        let original = SpaceHandle::new(1, "stack".to_string());
        let forked = original.fork();

        // Forked should NOT be the same space as original
        // (they have different overlays)
        assert!(!original.same_space(&forked));
    }

    #[test]
    fn test_nondeterministic_branch_simulation() {
        // Simulate what happens in nondeterministic evaluation:
        // - Original space has some data
        // - Multiple branches fork and modify independently
        // - Each branch should see only its own modifications

        let original = SpaceHandle::new(1, "kb".to_string());
        original.add_atom(MettaValue::Atom("fact1".to_string()));

        // Branch 1: adds fact2
        let branch1 = original.fork();
        branch1.add_atom(MettaValue::Atom("fact2".to_string()));

        // Branch 2: adds fact3
        let branch2 = original.fork();
        branch2.add_atom(MettaValue::Atom("fact3".to_string()));

        // Branch 3: removes fact1, adds fact4
        let branch3 = original.fork();
        branch3.remove_atom(&MettaValue::Atom("fact1".to_string()));
        branch3.add_atom(MettaValue::Atom("fact4".to_string()));

        // Verify isolation:
        // Branch 1: [fact1, fact2]
        assert_eq!(branch1.atom_count(), 2);
        assert!(branch1.contains(&MettaValue::Atom("fact1".to_string())));
        assert!(branch1.contains(&MettaValue::Atom("fact2".to_string())));

        // Branch 2: [fact1, fact3]
        assert_eq!(branch2.atom_count(), 2);
        assert!(branch2.contains(&MettaValue::Atom("fact1".to_string())));
        assert!(branch2.contains(&MettaValue::Atom("fact3".to_string())));

        // Branch 3: [fact4] (fact1 removed)
        assert_eq!(branch3.atom_count(), 1);
        assert!(!branch3.contains(&MettaValue::Atom("fact1".to_string())));
        assert!(branch3.contains(&MettaValue::Atom("fact4".to_string())));

        // Original unchanged: [fact1]
        assert_eq!(original.atom_count(), 1);
        assert!(original.contains(&MettaValue::Atom("fact1".to_string())));
    }
}
