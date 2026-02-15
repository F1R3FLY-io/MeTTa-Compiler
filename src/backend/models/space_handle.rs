//! Space Handle - First-class queryable space values with Copy-on-Write semantics
//!
//! SpaceHandle allows spaces to be passed around as values and queried
//! independently of the Environment. This matches HE's design where
//! spaces are first-class values.
//!
//! ## Copy-on-Write (CoW) via PathMap Clone
//!
//! When MeTTa evaluation forks into nondeterministic branches (e.g., from `match`
//! returning multiple results), each branch needs isolated access to mutable state.
//! Without isolation, one branch's `add-atom` affects all other branches.
//!
//! PathMap::clone() provides O(1) CoW via Arc-based structural sharing:
//! - `fork()` creates a logical copy that shares trie data (O(1) operation)
//! - First write to forked space copies only the affected trie nodes
//! - Each branch sees its own modifications without affecting others
//!
//! SpaceHandle supports two backing stores:
//! - PathMap<Multiplicity> — For dynamically created spaces (`new-space`)
//! - ModuleSpace — For module-backed spaces (`mod-space!`) with live references
//!
//! ## Multiplicity Tracking
//!
//! Space atoms are stored as MORK bytes in a PathMap<Multiplicity>, providing:
//! - Proper multiplicity tracking (count per unique atom path)
//! - Memory-efficient storage via trie structural sharing
//! - O(1) fork via PathMap::clone()
//!
//! ## MORK Serialization
//!
//! Each SpaceHandle has a SharedMappingHandle for MORK symbol interning.
//! MettaValue ↔ MORK bytes conversion happens at add/remove/collapse boundaries.

use std::sync::Arc;

use mork_interning::{SharedMapping, SharedMappingHandle};
use parking_lot::RwLock;
use pathmap::PathMap;

use super::metta_value_trait::{MettaValueFactory, MettaValueTrait};
use super::MettaValue;
use crate::backend::environment::multiplicity::{self, Multiplicity};
use crate::backend::environment::mork_encoding;
use crate::backend::environment::MultiplicityMatch;
use crate::backend::mork_convert::with_mork_bytes;
use crate::backend::modules::{ModId, ModuleSpace};

/// Generic version of MultiplicityMatch that works with any value type.
///
/// This enables generic evaluation to work with space multiplicities
/// without requiring conversions at boundaries.
#[derive(Debug, Clone)]
pub struct GenericMultiplicityMatch<V> {
    /// The matched value
    pub value: V,
    /// The number of times this value appears in the space
    pub count: usize,
}

impl<V> GenericMultiplicityMatch<V> {
    /// Create a new generic multiplicity match.
    #[inline]
    pub fn new(value: V, count: usize) -> Self {
        Self { value, count }
    }
}

/// The backing store for a SpaceHandle.
///
/// This enum allows SpaceHandle to work with both:
/// - Standalone spaces (from `new-space`) with PathMap-based CoW
/// - Module spaces (from `mod-space!`) with live references
#[derive(Clone)]
pub enum SpaceBacking {
    /// Owned space data (for new-space) using PathMap for O(1) CoW fork
    Owned {
        /// Atoms stored as MORK bytes with multiplicity tracking.
        /// PathMap::clone() provides O(1) CoW structural sharing.
        atoms: Arc<RwLock<PathMap<Multiplicity>>>,
        /// SharedMappingHandle for MORK symbol interning.
        /// Each SpaceHandle has its own interning table.
        shared_mapping: SharedMappingHandle,
    },
    /// Module-backed space (for mod-space!) with live reference
    Module {
        mod_id: ModId,
        space: Arc<RwLock<ModuleSpace>>,
    },
}

impl std::fmt::Debug for SpaceBacking {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpaceBacking::Owned { atoms, .. } => f
                .debug_struct("Owned")
                .field("atoms", atoms)
                .field("shared_mapping", &"<SharedMappingHandle>")
                .finish(),
            SpaceBacking::Module { mod_id, space } => f
                .debug_struct("Module")
                .field("mod_id", mod_id)
                .field("space", space)
                .finish(),
        }
    }
}

/// Thread-safe handle to a space's data.
///
/// SpaceHandle wraps the space data in Arc<RwLock<>> for:
/// - O(1) fork via PathMap structural sharing (CoW)
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
    /// The backing store (owned PathMap or live ModuleSpace reference)
    backing: SpaceBacking,
}

/// Create a new SharedMappingHandle for a SpaceHandle.
///
/// Each SpaceHandle gets its own SharedMapping to avoid cross-handle
/// lock contention during parallel evaluation. No warmup needed because
/// SpaceHandle's PathMap is behind `Arc<RwLock>` which serializes writes,
/// preventing the `ensure_root()` TOCTOU race.
#[inline]
fn new_space_mapping() -> SharedMappingHandle {
    SharedMappingHandle::from(SharedMapping::new())
}

/// Create a MORK Space for deserialization (only sm is used).
fn deserialization_space(sm: &SharedMappingHandle) -> mork::space::Space<Multiplicity> {
    mork::space::Space {
        sm: sm.clone(),
        btm: PathMap::new(),
        mmaps: std::collections::HashMap::new(),
    }
}

impl SpaceHandle {
    /// Create a new space handle with the given ID and name.
    pub fn new(id: u64, name: String) -> Self {
        Self {
            id,
            name,
            backing: SpaceBacking::Owned {
                atoms: Arc::new(RwLock::new(PathMap::new())),
                shared_mapping: new_space_mapping(),
            },
        }
    }

    /// Create a space handle with existing data.
    pub fn with_data(id: u64, name: String, atoms: Vec<MettaValue>) -> Self {
        let sm = new_space_mapping();
        let mut pm = PathMap::new();
        for atom in &atoms {
            let _ = with_mork_bytes(atom, &sm, |bytes| {
                multiplicity::add_atom(&mut pm, bytes);
            });
        }
        Self {
            id,
            name,
            backing: SpaceBacking::Owned {
                atoms: Arc::new(RwLock::new(pm)),
                shared_mapping: sm,
            },
        }
    }

    /// Create a space handle backed by a module's space (live reference).
    ///
    /// This provides live reference semantics where mutations are immediately
    /// visible to all holders of the space reference.
    pub fn for_module(mod_id: ModId, name: String, space: Arc<RwLock<ModuleSpace>>) -> Self {
        Self {
            id: mod_id.value(),
            name,
            backing: SpaceBacking::Module { mod_id, space },
        }
    }

    /// Create a space handle from serialized data.
    ///
    /// This is used when deserializing Space values from bytes. The resulting
    /// handle has minimal backing data - it's essentially a reference that must
    /// be resolved against the actual Environment to access atoms.
    pub fn new_from_serialized(id: u64, name: String, _is_module: bool) -> Self {
        Self::new(id, name)
    }

    /// Fork this space handle for nondeterministic branch isolation.
    ///
    /// Creates a new handle with an independent PathMap clone. PathMap::clone()
    /// is O(1) via Arc-based structural sharing (CoW) — actual data copying
    /// only happens on first write to divergent trie nodes.
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
            SpaceBacking::Owned {
                atoms,
                shared_mapping,
            } => {
                // PathMap::clone() is O(1) via Arc CoW structural sharing.
                let forked_atoms = atoms.read().clone();
                Self {
                    id: self.id,
                    name: self.name.clone(),
                    backing: SpaceBacking::Owned {
                        atoms: Arc::new(RwLock::new(forked_atoms)),
                        shared_mapping: shared_mapping.clone(),
                    },
                }
            }
            SpaceBacking::Module { space, .. } => {
                // Module spaces: fork creates a snapshot (not live).
                // Serialize module atoms into a new PathMap for isolation.
                let module_atoms = space.read().get_all_atoms();
                let sm = new_space_mapping();
                let mut pm = PathMap::new();
                for atom in &module_atoms {
                    let _ = with_mork_bytes(atom, &sm, |bytes| {
                        multiplicity::add_atom(&mut pm, bytes);
                    });
                }
                Self {
                    id: self.id,
                    name: self.name.clone(),
                    backing: SpaceBacking::Owned {
                        atoms: Arc::new(RwLock::new(pm)),
                        shared_mapping: sm,
                    },
                }
            }
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
            SpaceBacking::Owned { .. } => None,
        }
    }

    /// Create a space handle that shares data with another handle.
    /// Used when creating references to the same underlying space.
    ///
    /// Note: This creates a shallow clone - both handles share the same
    /// underlying PathMap and rules Vec via Arc. For isolated copies,
    /// use `fork()` instead.
    pub fn share_data(&self, new_id: u64, new_name: String) -> Self {
        Self {
            id: new_id,
            name: new_name,
            backing: self.backing.clone(),
        }
    }

    /// Add an atom to this space.
    ///
    /// For owned spaces, serializes to MORK bytes and adds to PathMap.
    /// For module spaces, delegates to ModuleSpace.
    pub fn add_atom(&self, atom: MettaValue) {
        match &self.backing {
            SpaceBacking::Owned {
                atoms,
                shared_mapping,
                ..
            } => {
                let _ = with_mork_bytes(&atom, shared_mapping, |bytes| {
                    let mut pm = atoms.write();
                    multiplicity::add_atom(&mut pm, bytes);
                });
            }
            SpaceBacking::Module { space, .. } => {
                let mut space = space.write();
                space.add_atom(atom);
            }
        }
    }

    /// Remove an atom from this space.
    /// Returns true if the atom was found and removed.
    pub fn remove_atom(&self, atom: &MettaValue) -> bool {
        match &self.backing {
            SpaceBacking::Owned {
                atoms,
                shared_mapping,
                ..
            } => {
                match with_mork_bytes(atom, shared_mapping, |bytes| {
                    let mut pm = atoms.write();
                    let old_count = multiplicity::get_multiplicity(&pm, bytes);
                    if old_count > 0 {
                        multiplicity::remove_atom(&mut pm, bytes);
                        true
                    } else {
                        false
                    }
                }) {
                    Ok(removed) => removed,
                    Err(_) => false,
                }
            }
            SpaceBacking::Module { space, .. } => {
                let mut space = space.write();
                space.remove_atom(atom)
            }
        }
    }

    /// Get all atoms in this space (collapse) - expands multiplicities.
    ///
    /// Iterates the PathMap, deserializes each MORK entry to MettaValue,
    /// and expands by multiplicity count.
    pub fn collapse(&self) -> Vec<MettaValue> {
        match &self.backing {
            SpaceBacking::Owned {
                atoms,
                shared_mapping,
                ..
            } => {
                let pm = atoms.read();
                let space = deserialization_space(shared_mapping);
                let factory = super::GcFactory::default();
                let mut result = Vec::new();

                use pathmap::zipper::{ZipperIteration, ZipperMoving, ZipperValues};
                let mut rz = pm.read_zipper();
                while rz.to_next_val() {
                    let path = rz.path();
                    let count = rz.val().map(|m| m.count()).unwrap_or(0);
                    if count == 0 {
                        continue;
                    }
                    if let Ok(value) =
                        mork_encoding::mork_bytes_to_generic_value(path, &space, &factory)
                    {
                        for _ in 0..count {
                            result.push(value);
                        }
                    }
                }
                result
            }
            SpaceBacking::Module { space, .. } => {
                let space = space.read();
                space.get_all_atoms()
            }
        }
    }

    /// Get all atoms as MultiplicityMatch with their actual counts.
    ///
    /// Returns the actual multiplicities, enabling efficient handling of
    /// high-multiplicity atoms.
    pub fn collapse_with_multiplicity(&self) -> Vec<MultiplicityMatch<MettaValue>> {
        match &self.backing {
            SpaceBacking::Owned {
                atoms,
                shared_mapping,
                ..
            } => {
                let pm = atoms.read();
                let space = deserialization_space(shared_mapping);
                let factory = super::GcFactory::default();
                let mut results = Vec::new();

                use pathmap::zipper::{ZipperIteration, ZipperMoving, ZipperValues};
                let mut rz = pm.read_zipper();
                while rz.to_next_val() {
                    let path = rz.path();
                    let count = rz.val().map(|m| m.count()).unwrap_or(0);
                    if count == 0 {
                        continue;
                    }
                    if let Ok(value) =
                        mork_encoding::mork_bytes_to_generic_value(path, &space, &factory)
                    {
                        results.push(MultiplicityMatch::new(value, count as usize));
                    }
                }
                results
            }
            SpaceBacking::Module { space, .. } => {
                // Module spaces don't track multiplicities, each atom has count=1
                let space = space.read();
                space
                    .get_all_atoms()
                    .into_iter()
                    .map(|v| MultiplicityMatch::new(v, 1))
                    .collect()
            }
        }
    }

    /// Get the total number of atoms in this space (sum of multiplicities).
    pub fn atom_count(&self) -> usize {
        match &self.backing {
            SpaceBacking::Owned { atoms, .. } => {
                let pm = atoms.read();
                let mut total: usize = 0;
                use pathmap::zipper::{ZipperIteration, ZipperValues};
                let mut rz = pm.read_zipper();
                while rz.to_next_val() {
                    total += rz.val().map(|m| m.count() as usize).unwrap_or(0);
                }
                total
            }
            SpaceBacking::Module { space, .. } => {
                let space = space.read();
                space.get_all_atoms().len()
            }
        }
    }

    /// Get the number of unique atoms in this space (distinct count).
    pub fn distinct_atom_count(&self) -> usize {
        match &self.backing {
            SpaceBacking::Owned { atoms, .. } => {
                let pm = atoms.read();
                let mut count: usize = 0;
                use pathmap::zipper::ZipperIteration;
                let mut rz = pm.read_zipper();
                while rz.to_next_val() {
                    count += 1;
                }
                count
            }
            SpaceBacking::Module { space, .. } => {
                let space = space.read();
                space.get_all_atoms().len()
            }
        }
    }

    /// Check if the space contains a specific atom.
    pub fn contains(&self, atom: &MettaValue) -> bool {
        match &self.backing {
            SpaceBacking::Owned {
                atoms,
                shared_mapping,
                ..
            } => {
                match with_mork_bytes(atom, shared_mapping, |bytes| {
                    let pm = atoms.read();
                    multiplicity::get_multiplicity(&pm, bytes) > 0
                }) {
                    Ok(found) => found,
                    Err(_) => false,
                }
            }
            SpaceBacking::Module { space, .. } => {
                let space = space.read();
                space.contains(atom)
            }
        }
    }

    /// Get the multiplicity (count) of a specific atom in this space.
    pub fn atom_multiplicity(&self, atom: &MettaValue) -> usize {
        match &self.backing {
            SpaceBacking::Owned {
                atoms,
                shared_mapping,
                ..
            } => {
                match with_mork_bytes(atom, shared_mapping, |bytes| {
                    let pm = atoms.read();
                    multiplicity::get_multiplicity(&pm, bytes) as usize
                }) {
                    Ok(count) => count,
                    Err(_) => 0,
                }
            }
            SpaceBacking::Module { space, .. } => {
                let space = space.read();
                if space.contains(atom) {
                    1
                } else {
                    0
                }
            }
        }
    }

    // Rules are stored in the Environment's PathMap, not in SpaceHandle.
    // Use env.add_rule(lhs, rhs) instead.

    // ========================================================================
    // Generic Value Operations
    // ========================================================================
    // These methods work with any value type implementing MettaValueTrait.
    // Atoms are stored as MORK bytes in PathMap; generic methods
    // serialize/deserialize when crossing type boundaries.
    // ========================================================================

    /// Add an atom of any type implementing MettaValueTrait.
    ///
    /// The value is serialized to bytes and deserialized to MettaValue
    /// for storage. This enables generic evaluation without conversion
    /// at caller sites.
    pub fn add_atom_generic<V: MettaValueTrait>(&self, atom: &V) {
        let bytes = atom.serialize();
        let heap_atom = self.deserialize_to_metta(&bytes);
        self.add_atom(heap_atom);
    }

    /// Remove an atom of any type implementing MettaValueTrait.
    ///
    /// Returns true if the atom was found and removed.
    pub fn remove_atom_generic<V: MettaValueTrait>(&self, atom: &V) -> bool {
        let bytes = atom.serialize();
        let heap_atom = self.deserialize_to_metta(&bytes);
        self.remove_atom(&heap_atom)
    }

    /// Check if the space contains a specific atom (generic version).
    pub fn contains_generic<V: MettaValueTrait>(&self, atom: &V) -> bool {
        let bytes = atom.serialize();
        let heap_atom = self.deserialize_to_metta(&bytes);
        self.contains(&heap_atom)
    }

    /// Get all atoms in this space as generic values.
    ///
    /// This is the generic version of `collapse()` that returns values
    /// of any type implementing MettaValueTrait.
    ///
    /// When V = MettaValue (production path via GcFactory), `from_metta_value`
    /// is a zero-cost identity — no serialization/deserialization round-trip.
    pub fn collapse_generic<V, F>(&self, factory: &F) -> Vec<V>
    where
        V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
        F: MettaValueFactory<V>,
    {
        let heap_atoms = self.collapse();
        heap_atoms
            .into_iter()
            .map(|atom| factory.from_metta_value(atom))
            .collect()
    }

    /// Get atoms as MultiplicityMatch with their actual counts (generic version).
    ///
    /// When V = MettaValue (production path via GcFactory), `from_metta_value`
    /// is a zero-cost identity — no serialization/deserialization round-trip.
    pub fn collapse_with_multiplicity_generic<V, F>(
        &self,
        factory: &F,
    ) -> Vec<GenericMultiplicityMatch<V>>
    where
        V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
        F: MettaValueFactory<V>,
    {
        let heap_matches = self.collapse_with_multiplicity();
        heap_matches
            .into_iter()
            .map(|m| GenericMultiplicityMatch {
                value: factory.from_metta_value(m.value),
                count: m.count,
            })
            .collect()
    }

    /// Get the multiplicity (count) of a specific atom (generic version).
    pub fn atom_multiplicity_generic<V: MettaValueTrait>(&self, atom: &V) -> usize {
        let bytes = atom.serialize();
        let heap_atom = self.deserialize_to_metta(&bytes);
        self.atom_multiplicity(&heap_atom)
    }

    // ========================================================================
    // Internal Helpers
    // ========================================================================

    /// Deserialize bytes to MettaValue using the built-in deserializer.
    fn deserialize_to_metta(&self, bytes: &[u8]) -> MettaValue {
        use super::GcFactory;
        let factory = GcFactory::default();
        match factory.deserialize(bytes) {
            Ok((value, _)) => value,
            Err(_) => MettaValue::Atom("?deserialization_error?".to_string()),
        }
    }

    // ========================================================================
    // End Generic Value Operations
    // ========================================================================

    /// Collect all slab-allocated MettaValues referenced by this space.
    ///
    /// Used by the GC mark phase to traverse into Space values.
    ///
    /// Owned spaces store atoms as MORK bytes in PathMap (no slab pointers),
    /// so no GC collection is needed. Module spaces have live MettaValue atoms
    /// that reference slab memory.
    pub(crate) fn collect_gc_values(&self, values: &mut Vec<MettaValue>) {
        match &self.backing {
            SpaceBacking::Owned { .. } => {
                // PathMap atoms are MORK bytes — no slab pointers, no GC roots.
            }
            SpaceBacking::Module { space, .. } => {
                // Collect atoms from module space (local atoms only — dependencies have
                // their own environments registered as separate root providers)
                let ms = space.read();
                values.extend(ms.get_atoms_local().into_iter());
            }
        }
    }

    /// Check if two space handles point to the same underlying data.
    pub fn same_space(&self, other: &SpaceHandle) -> bool {
        match (&self.backing, &other.backing) {
            (
                SpaceBacking::Owned { atoms: a, .. },
                SpaceBacking::Owned { atoms: b, .. },
            ) => Arc::ptr_eq(a, b),
            (SpaceBacking::Module { mod_id: a, .. }, SpaceBacking::Module { mod_id: b, .. }) => {
                a == b
            }
            _ => false,
        }
    }
}

impl PartialEq for SpaceHandle {
    fn eq(&self, other: &Self) -> bool {
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

    // ============================================================
    // Copy-on-Write (CoW) Fork Tests
    // ============================================================

    #[test]
    fn test_fork_creates_isolated_copy() {
        let original = SpaceHandle::new(1, "stack".to_string());
        original.add_atom(MettaValue::Long(1));

        // Fork creates isolated copy (O(1) via PathMap CoW)
        let forked = original.fork();

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
        assert!(!original.same_space(&forked));
    }

    #[test]
    fn test_nondeterministic_branch_simulation() {
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

    // ============================================================
    // Multiplicity Tracking Tests
    // ============================================================

    #[test]
    fn test_multiplicity_same_atom_added_multiple_times() {
        let handle = SpaceHandle::new(1, "test".to_string());
        let atom = MettaValue::Atom("foo".to_string());

        // Add the same atom 1000 times
        for _ in 0..1000 {
            handle.add_atom(atom.clone());
        }

        // Total count should be 1000
        assert_eq!(handle.atom_count(), 1000);

        // But distinct count should be 1 (only one unique atom)
        assert_eq!(handle.distinct_atom_count(), 1);

        // Multiplicity of the atom should be 1000
        assert_eq!(handle.atom_multiplicity(&atom), 1000);

        // Contains should return true
        assert!(handle.contains(&atom));
    }

    #[test]
    fn test_multiplicity_collapse_expands_correctly() {
        let handle = SpaceHandle::new(1, "test".to_string());
        let atom = MettaValue::Long(42);

        // Add the same atom 5 times
        for _ in 0..5 {
            handle.add_atom(atom.clone());
        }

        // Collapse should return 5 copies
        let collapsed = handle.collapse();
        assert_eq!(collapsed.len(), 5);
        assert!(collapsed.iter().all(|a| *a == atom));
    }

    #[test]
    fn test_multiplicity_collapse_with_multiplicity_returns_actual_counts() {
        let handle = SpaceHandle::new(1, "test".to_string());
        let atom1 = MettaValue::Long(1);
        let atom2 = MettaValue::Long(2);

        // Add atom1 three times, atom2 once
        for _ in 0..3 {
            handle.add_atom(atom1.clone());
        }
        handle.add_atom(atom2.clone());

        // Get multiplicity matches
        let matches = handle.collapse_with_multiplicity();

        // Should have 2 distinct matches
        assert_eq!(matches.len(), 2);

        // Total when expanded should be 4
        let total_expanded: usize = matches.iter().map(|m| m.count).sum();
        assert_eq!(total_expanded, 4);

        // Find atom1's match and verify count is 3
        let atom1_match = matches
            .iter()
            .find(|m| m.value == atom1)
            .expect("Should find atom1");
        assert_eq!(atom1_match.count, 3);

        // Find atom2's match and verify count is 1
        let atom2_match = matches
            .iter()
            .find(|m| m.value == atom2)
            .expect("Should find atom2");
        assert_eq!(atom2_match.count, 1);
    }

    #[test]
    fn test_multiplicity_remove_decrements_count() {
        let handle = SpaceHandle::new(1, "test".to_string());
        let atom = MettaValue::Atom("multi".to_string());

        // Add the same atom 5 times
        for _ in 0..5 {
            handle.add_atom(atom.clone());
        }
        assert_eq!(handle.atom_multiplicity(&atom), 5);

        // Remove 2 instances
        handle.remove_atom(&atom);
        handle.remove_atom(&atom);

        // Multiplicity should now be 3
        assert_eq!(handle.atom_multiplicity(&atom), 3);
        assert_eq!(handle.atom_count(), 3);

        // Still contains the atom
        assert!(handle.contains(&atom));

        // Remove remaining 3
        handle.remove_atom(&atom);
        handle.remove_atom(&atom);
        handle.remove_atom(&atom);

        // Should no longer contain the atom
        assert_eq!(handle.atom_multiplicity(&atom), 0);
        assert!(!handle.contains(&atom));
    }

    #[test]
    fn test_multiplicity_fork_preserves_counts() {
        let original = SpaceHandle::new(1, "test".to_string());
        let atom = MettaValue::Long(100);

        // Add the same atom 50 times to original
        for _ in 0..50 {
            original.add_atom(atom.clone());
        }

        // Fork the space
        let forked = original.fork();

        // Both should have multiplicity 50
        assert_eq!(original.atom_multiplicity(&atom), 50);
        assert_eq!(forked.atom_multiplicity(&atom), 50);

        // Add more to forked - should not affect original
        for _ in 0..10 {
            forked.add_atom(atom.clone());
        }
        assert_eq!(forked.atom_multiplicity(&atom), 60);
        assert_eq!(original.atom_multiplicity(&atom), 50);

        // Remove some from original - should not affect forked
        original.remove_atom(&atom);
        original.remove_atom(&atom);
        assert_eq!(original.atom_multiplicity(&atom), 48);
        assert_eq!(forked.atom_multiplicity(&atom), 60);
    }

    #[test]
    fn test_multiplicity_mixed_atoms() {
        let handle = SpaceHandle::new(1, "test".to_string());

        let a = MettaValue::Atom("a".to_string());
        let b = MettaValue::Atom("b".to_string());
        let c = MettaValue::Atom("c".to_string());

        for _ in 0..10 {
            handle.add_atom(a.clone());
        }
        for _ in 0..20 {
            handle.add_atom(b.clone());
        }
        for _ in 0..30 {
            handle.add_atom(c.clone());
        }

        // Total should be 60
        assert_eq!(handle.atom_count(), 60);

        // Distinct should be 3
        assert_eq!(handle.distinct_atom_count(), 3);

        // Verify individual multiplicities
        assert_eq!(handle.atom_multiplicity(&a), 10);
        assert_eq!(handle.atom_multiplicity(&b), 20);
        assert_eq!(handle.atom_multiplicity(&c), 30);

        // collapse_with_multiplicity should return 3 entries
        let matches = handle.collapse_with_multiplicity();
        assert_eq!(matches.len(), 3);
    }

    #[test]
    fn test_multiplicity_memory_efficiency() {
        let handle = SpaceHandle::new(1, "test".to_string());
        let atom =
            MettaValue::String("large_string_that_would_waste_memory_if_duplicated".to_string());

        // Add the same atom 10000 times
        for _ in 0..10000 {
            handle.add_atom(atom.clone());
        }

        // Only 1 distinct atom stored
        assert_eq!(handle.distinct_atom_count(), 1);

        // But total count is 10000
        assert_eq!(handle.atom_count(), 10000);

        // collapse_with_multiplicity should return just 1 entry with count=10000
        let matches = handle.collapse_with_multiplicity();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].count, 10000);
    }

    // ============================================================
    // Generic Value Operations Tests
    // ============================================================

    #[test]
    fn test_add_atom_generic() {
        let handle = SpaceHandle::new(1, "test".to_string());
        let atom = MettaValue::Long(42);

        // Add via generic method
        handle.add_atom_generic(&atom);

        assert_eq!(handle.atom_count(), 1);
        assert!(handle.contains(&MettaValue::Long(42)));
    }

    #[test]
    fn test_remove_atom_generic() {
        let handle = SpaceHandle::new(1, "test".to_string());
        let atom = MettaValue::Long(42);

        handle.add_atom(MettaValue::Long(42));
        assert_eq!(handle.atom_count(), 1);

        // Remove via generic method
        assert!(handle.remove_atom_generic(&atom));
        assert_eq!(handle.atom_count(), 0);
    }

    #[test]
    fn test_contains_generic() {
        let handle = SpaceHandle::new(1, "test".to_string());
        let atom = MettaValue::Long(42);

        handle.add_atom(MettaValue::Long(42));

        assert!(handle.contains_generic(&atom));
        assert!(!handle.contains_generic(&MettaValue::Long(99)));
    }

    #[test]
    fn test_collapse_generic() {
        use crate::backend::models::GcFactory;

        let handle = SpaceHandle::new(1, "test".to_string());
        handle.add_atom(MettaValue::Long(1));
        handle.add_atom(MettaValue::Long(2));
        handle.add_atom(MettaValue::Long(3));

        let factory = GcFactory::default();
        let collapsed: Vec<MettaValue> = handle.collapse_generic(&factory);

        assert_eq!(collapsed.len(), 3);

        // Verify all values are present
        let has_1 = collapsed.iter().any(|v| v.as_long() == Some(1));
        let has_2 = collapsed.iter().any(|v| v.as_long() == Some(2));
        let has_3 = collapsed.iter().any(|v| v.as_long() == Some(3));
        assert!(has_1 && has_2 && has_3);
    }

    #[test]
    fn test_collapse_with_multiplicity_generic() {
        use crate::backend::models::GcFactory;

        let handle = SpaceHandle::new(1, "test".to_string());
        let atom = MettaValue::Long(42);

        // Add the same atom 5 times
        for _ in 0..5 {
            handle.add_atom(atom.clone());
        }

        let factory = GcFactory::default();
        let matches: Vec<GenericMultiplicityMatch<MettaValue>> =
            handle.collapse_with_multiplicity_generic(&factory);

        // Should have 1 distinct match
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].count, 5);
        assert_eq!(matches[0].value.as_long(), Some(42));
    }

    #[test]
    fn test_atom_multiplicity_generic() {
        #[allow(unused_imports)]
        use crate::backend::models::GcFactory;

        let handle = SpaceHandle::new(1, "test".to_string());
        let atom = MettaValue::Atom("foo".to_string());

        for _ in 0..7 {
            handle.add_atom(atom.clone());
        }

        assert_eq!(handle.atom_multiplicity_generic(&atom), 7);
    }

    #[test]
    fn test_generic_operations_roundtrip() {
        use crate::backend::models::GcFactory;

        let handle1 = SpaceHandle::new(1, "test1".to_string());
        let handle2 = SpaceHandle::new(2, "test2".to_string());

        let atoms = vec![
            MettaValue::Long(1),
            MettaValue::Atom("foo".to_string()),
            MettaValue::Bool(true),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
        ];

        // Add via normal method
        for atom in &atoms {
            handle1.add_atom(atom.clone());
        }

        // Add via generic method
        for atom in &atoms {
            handle2.add_atom_generic(atom);
        }

        // Both should have same counts
        assert_eq!(handle1.atom_count(), handle2.atom_count());

        // Collapse via normal vs generic should produce same results
        let factory = GcFactory::default();
        let collapsed1 = handle1.collapse();
        let collapsed2: Vec<MettaValue> = handle2.collapse_generic(&factory);

        assert_eq!(collapsed1.len(), collapsed2.len());

        // Each atom should be present in both
        for atom in &atoms {
            assert!(handle1.contains(atom));
            assert!(handle2.contains_generic(atom));
        }
    }

    #[test]
    fn test_generic_operations_with_forked_space() {
        use crate::backend::models::GcFactory;

        let original = SpaceHandle::new(1, "test".to_string());
        original.add_atom(MettaValue::Long(1));

        let forked = original.fork();

        // Add to forked via generic method
        forked.add_atom_generic(&MettaValue::Long(2));

        // Forked should see both
        assert_eq!(forked.atom_count(), 2);
        assert!(forked.contains_generic(&MettaValue::Long(1)));
        assert!(forked.contains_generic(&MettaValue::Long(2)));

        // Original should only see the original atom
        assert_eq!(original.atom_count(), 1);
        assert!(original.contains_generic(&MettaValue::Long(1)));
        assert!(!original.contains_generic(&MettaValue::Long(2)));

        // Collapse forked via generic
        let factory = GcFactory::default();
        let forked_atoms: Vec<MettaValue> = forked.collapse_generic(&factory);
        assert_eq!(forked_atoms.len(), 2);
    }
}
