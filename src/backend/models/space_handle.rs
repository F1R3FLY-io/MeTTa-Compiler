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
//!
//! ## Multiplicity Tracking
//!
//! Space atoms are stored using `AtomMultisetSnapshot` which provides:
//! - O(1) cloning via structural sharing (`im::HashMap`)
//! - Proper multiplicity tracking (count per unique atom)
//! - Memory-efficient storage (one atom + count, not N copies)
//!
//! ## Generic Value Support
//!
//! SpaceHandle stores atoms via `AtomMultisetSnapshot` which uses byte-based
//! storage via `SymbolTable`. This enables generic operations:
//! - `add_atom_generic<V>(&self, atom: &V)` - Serializes V to bytes, interns in SymbolTable
//! - `collapse_generic<V, F>(&self, factory: &F) -> Vec<V>` - Deserializes bytes to V
//!
//! This design eliminates conversions at evaluation boundaries - atoms are stored
//! once as bytes and deserialized only when needed.

use std::sync::Arc;

use parking_lot::RwLock;

use super::metta_value_trait::{MettaValueTrait, MettaValueFactory};
use super::{AtomMultisetSnapshot, MettaValue, Rule, SymbolTable};
use crate::backend::environment::MultiplicityMatch;
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

/// Local modifications overlay for Copy-on-Write semantics.
///
/// When a space is forked, the overlay tracks local changes without modifying
/// the shared base. This enables nondeterministic branch isolation.
///
/// Both `added` and `removed` use `AtomMultisetSnapshot` for:
/// - O(1) cloning via structural sharing
/// - Proper multiplicity tracking
/// - Memory-efficient storage
#[derive(Debug, Clone)]
pub struct SpaceOverlay {
    /// Atoms added in this fork (local additions with multiplicities)
    pub added: AtomMultisetSnapshot,
    /// Atoms removed in this fork (tombstones with counts)
    /// The count represents how many instances were removed
    pub removed: AtomMultisetSnapshot,
    /// Rules added in this fork
    pub added_rules: Vec<Rule>,
}

impl SpaceOverlay {
    /// Create a new empty overlay with the given symbol table.
    pub fn new(symbols: Arc<SymbolTable>) -> Self {
        Self {
            added: AtomMultisetSnapshot::new(Arc::clone(&symbols)),
            removed: AtomMultisetSnapshot::new(symbols),
            added_rules: Vec::new(),
        }
    }

    /// Check if an atom was removed in this overlay
    pub fn is_removed(&self, atom: &MettaValue) -> bool {
        self.removed.contains(atom)
    }

    /// Get the net removal count for an atom (how many more removed than added back)
    pub fn removal_count(&self, atom: &MettaValue) -> usize {
        self.removed.count(atom)
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
    /// Shared symbol table for atom interning (enables O(1) equality via AtomId)
    symbols: Arc<SymbolTable>,
    /// The backing store (owned SpaceData or live ModuleSpace reference)
    backing: SpaceBacking,
}

/// The actual data stored in a space.
///
/// Uses `AtomMultisetSnapshot` for atom storage, providing:
/// - O(1) cloning via structural sharing (`im::HashMap`)
/// - Proper multiplicity tracking (count per unique atom)
/// - Memory-efficient storage (one atom + count, not N copies)
#[derive(Debug, Clone)]
pub struct SpaceData {
    /// Atoms stored in this space with their multiplicities
    pub atoms: AtomMultisetSnapshot,
    /// Rules defined in this space (for matching)
    pub rules: Vec<Rule>,
}

impl SpaceData {
    /// Create new empty SpaceData with the given symbol table.
    pub fn new(symbols: Arc<SymbolTable>) -> Self {
        Self {
            atoms: AtomMultisetSnapshot::new(symbols),
            rules: Vec::new(),
        }
    }

    /// Create SpaceData with initial atoms.
    pub fn with_atoms(
        symbols: Arc<SymbolTable>,
        atoms: impl IntoIterator<Item = MettaValue>,
    ) -> Self {
        let mut multiset = AtomMultisetSnapshot::new(symbols);
        for atom in atoms {
            multiset = multiset.insert(&atom);
        }
        Self {
            atoms: multiset,
            rules: Vec::new(),
        }
    }
}

impl SpaceHandle {
    /// Create a new space handle with the given ID and name.
    /// Uses a fresh symbol table - prefer `with_symbols` for sharing.
    pub fn new(id: u64, name: String) -> Self {
        let symbols = Arc::new(SymbolTable::new());
        Self {
            id,
            name,
            symbols: Arc::clone(&symbols),
            backing: SpaceBacking::Owned {
                base: Arc::new(RwLock::new(SpaceData::new(symbols))),
                overlay: None,
            },
        }
    }

    /// Create a new space handle with a shared symbol table.
    /// This is the preferred constructor when integrating with Environment.
    pub fn with_symbols(id: u64, name: String, symbols: Arc<SymbolTable>) -> Self {
        Self {
            id,
            name,
            symbols: Arc::clone(&symbols),
            backing: SpaceBacking::Owned {
                base: Arc::new(RwLock::new(SpaceData::new(symbols))),
                overlay: None,
            },
        }
    }

    /// Create a space handle with existing data.
    pub fn with_data(id: u64, name: String, atoms: Vec<MettaValue>) -> Self {
        let symbols = Arc::new(SymbolTable::new());
        Self {
            id,
            name,
            symbols: Arc::clone(&symbols),
            backing: SpaceBacking::Owned {
                base: Arc::new(RwLock::new(SpaceData::with_atoms(symbols, atoms))),
                overlay: None,
            },
        }
    }

    /// Create a space handle with existing data and a shared symbol table.
    pub fn with_data_and_symbols(
        id: u64,
        name: String,
        atoms: Vec<MettaValue>,
        symbols: Arc<SymbolTable>,
    ) -> Self {
        Self {
            id,
            name,
            symbols: Arc::clone(&symbols),
            backing: SpaceBacking::Owned {
                base: Arc::new(RwLock::new(SpaceData::with_atoms(symbols, atoms))),
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
        // Module spaces use their own fresh symbol table
        let symbols = Arc::new(SymbolTable::new());
        Self {
            id: mod_id.value(),
            name,
            symbols,
            backing: SpaceBacking::Module { mod_id, space },
        }
    }

    /// Create a space handle backed by a module's space with a shared symbol table.
    pub fn for_module_with_symbols(
        mod_id: ModId,
        name: String,
        space: Arc<RwLock<ModuleSpace>>,
        symbols: Arc<SymbolTable>,
    ) -> Self {
        Self {
            id: mod_id.value(),
            name,
            symbols,
            backing: SpaceBacking::Module { mod_id, space },
        }
    }

    /// Create a space handle from serialized data.
    ///
    /// This is used when deserializing Space values from bytes. The resulting
    /// handle has minimal backing data - it's essentially a reference that must
    /// be resolved against the actual Environment to access atoms.
    ///
    /// # Arguments
    /// - `id` - The space's unique identifier
    /// - `name` - Human-readable name for the space
    /// - `is_module` - Whether this is a module-backed space
    pub fn new_from_serialized(id: u64, name: String, is_module: bool) -> Self {
        let symbols = Arc::new(SymbolTable::new());
        // Create minimal backing - the Environment will need to be consulted for actual data
        if is_module {
            // For module spaces, we create a stub - actual data must come from Environment
            // Create empty owned backing since we don't have the actual module reference
            Self {
                id,
                name,
                symbols: Arc::clone(&symbols),
                backing: SpaceBacking::Owned {
                    base: Arc::new(RwLock::new(SpaceData::new(symbols))),
                    overlay: None,
                },
            }
        } else {
            Self::new(id, name)
        }
    }

    /// Get a reference to the symbol table used by this space.
    #[inline]
    pub fn symbols(&self) -> &Arc<SymbolTable> {
        &self.symbols
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
                    // Materialize current state into a new base using multiset snapshot
                    let current_atoms = self.collapse_to_multiset();
                    let current_rules = self.rules();
                    Self {
                        id: self.id,
                        name: self.name.clone(),
                        symbols: Arc::clone(&self.symbols),
                        backing: SpaceBacking::Owned {
                            base: Arc::new(RwLock::new(SpaceData {
                                atoms: current_atoms,
                                rules: current_rules,
                            })),
                            overlay: Some(Arc::new(RwLock::new(SpaceOverlay::new(Arc::clone(
                                &self.symbols,
                            ))))),
                        },
                    }
                } else {
                    // No overlay yet - forked handle gets a snapshot of current base
                    // to ensure true isolation (original modifications don't affect fork)
                    //
                    // Note: We create a new base from a snapshot of the current atoms.
                    // This is O(1) via AtomMultisetSnapshot's structural sharing.
                    // The forked space has its own independent base and overlay.
                    let base_data = base.read();
                    Self {
                        id: self.id,
                        name: self.name.clone(),
                        symbols: Arc::clone(&self.symbols),
                        backing: SpaceBacking::Owned {
                            base: Arc::new(RwLock::new(SpaceData {
                                // Clone the snapshot - O(1) due to im::HashMap structural sharing
                                atoms: base_data.atoms.clone(),
                                rules: base_data.rules.clone(),
                            })),
                            overlay: Some(Arc::new(RwLock::new(SpaceOverlay::new(Arc::clone(
                                &self.symbols,
                            ))))),
                        },
                    }
                }
            }
            SpaceBacking::Module { mod_id: _, space } => {
                // Module spaces: fork creates a snapshot (not live)
                // This gives each branch its own isolated copy
                let atoms = space.read().get_all_atoms();
                Self {
                    id: self.id,
                    name: self.name.clone(),
                    symbols: Arc::clone(&self.symbols),
                    backing: SpaceBacking::Owned {
                        base: Arc::new(RwLock::new(SpaceData::with_atoms(
                            Arc::clone(&self.symbols),
                            atoms,
                        ))),
                        overlay: Some(Arc::new(RwLock::new(SpaceOverlay::new(Arc::clone(
                            &self.symbols,
                        ))))),
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
            symbols: Arc::clone(&self.symbols),
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
                    let mut overlay_lock = overlay.write();
                    // If this atom was previously removed, decrement removal count
                    if overlay_lock.removed.contains(&atom) {
                        if let Some(new_removed) = overlay_lock.removed.remove(&atom) {
                            overlay_lock.removed = new_removed;
                        }
                    }
                    overlay_lock.added = overlay_lock.added.insert(&atom);
                } else {
                    // Not forked: add directly to base
                    let mut data = base.write();
                    data.atoms = data.atoms.insert(&atom);
                }
            }
            SpaceBacking::Module { space, .. } => {
                let mut space = space.write();
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
                    let mut overlay_lock = overlay.write();

                    // First check if it was added in this overlay
                    if overlay_lock.added.contains(atom) {
                        if let Some(new_added) = overlay_lock.added.remove(atom) {
                            overlay_lock.added = new_added;
                            return true;
                        }
                    }

                    // Check if it exists in base (and not already removed)
                    let base_data = base.read();
                    if base_data.atoms.contains(atom) && !overlay_lock.is_removed(atom) {
                        // Add tombstone
                        overlay_lock.removed = overlay_lock.removed.insert(atom);
                        return true;
                    }

                    false
                } else {
                    // Not forked: remove directly from base
                    let mut data = base.write();
                    if data.atoms.contains(atom) {
                        if let Some(new_atoms) = data.atoms.remove(atom) {
                            data.atoms = new_atoms;
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
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
    /// If forked, returns: (base atoms - removed) + added
    /// Each atom is repeated according to its multiplicity.
    pub fn collapse(&self) -> Vec<MettaValue> {
        match &self.backing {
            SpaceBacking::Owned { base, overlay } => {
                let base_data = base.read();

                if let Some(overlay) = overlay {
                    let overlay_lock = overlay.read();

                    // Start with expanded base atoms, filter out removed ones
                    let mut result: Vec<MettaValue> = Vec::new();
                    for (atom, count) in base_data.atoms.iter() {
                        // Get removal count from overlay
                        let removal_count = overlay_lock.removal_count(&atom);
                        let effective_count = count.saturating_sub(removal_count);
                        for _ in 0..effective_count {
                            result.push(atom.clone());
                        }
                    }

                    // Add atoms from overlay (expanded)
                    result.extend(overlay_lock.added.expand());

                    result
                } else {
                    base_data.atoms.expand()
                }
            }
            SpaceBacking::Module { space, .. } => {
                let space = space.read();
                space.get_all_atoms()
            }
        }
    }

    /// Get atoms as a multiset snapshot (preserves multiplicities without expansion).
    ///
    /// This is more efficient than `collapse()` for cases where multiplicities
    /// need to be preserved or when O(1) cloning is desired.
    pub fn collapse_to_multiset(&self) -> AtomMultisetSnapshot {
        match &self.backing {
            SpaceBacking::Owned { base, overlay } => {
                let base_data = base.read();

                if let Some(overlay) = overlay {
                    let overlay_lock = overlay.read();

                    // Merge: base + added - removed
                    let mut merged = base_data.atoms.clone();

                    // Add from overlay
                    merged = merged.merge(&overlay_lock.added);

                    // Remove tombstones
                    for (atom, removal_count) in overlay_lock.removed.iter() {
                        for _ in 0..removal_count {
                            if let Some(new_merged) = merged.remove(&atom) {
                                merged = new_merged;
                            }
                        }
                    }

                    merged
                } else {
                    base_data.atoms.clone()
                }
            }
            SpaceBacking::Module { space, .. } => {
                // Module spaces don't track multiplicities, so we convert Vec to multiset
                let space = space.read();
                let atoms = space.get_all_atoms();
                SpaceData::with_atoms(Arc::clone(&self.symbols), atoms).atoms
            }
        }
    }

    /// Get all atoms as MultiplicityMatch with their actual counts.
    ///
    /// This provides type consistency with `Environment::match_space()` for
    /// code that needs to handle both owned spaces and module spaces uniformly.
    ///
    /// Unlike the previous implementation that always returned count=1, this
    /// returns the actual multiplicities, enabling efficient handling of
    /// high-multiplicity atoms.
    pub fn collapse_with_multiplicity(&self) -> Vec<MultiplicityMatch<MettaValue>> {
        match &self.backing {
            SpaceBacking::Owned { base, overlay } => {
                let base_data = base.read();

                if let Some(overlay) = overlay {
                    let overlay_lock = overlay.read();

                    // Build result from merged multiset
                    let mut results = Vec::new();

                    // Process base atoms, accounting for removals
                    for (atom, base_count) in base_data.atoms.iter() {
                        let removal_count = overlay_lock.removal_count(&atom);
                        let effective_count = base_count.saturating_sub(removal_count);
                        if effective_count > 0 {
                            results.push(MultiplicityMatch::new(atom, effective_count));
                        }
                    }

                    // Add atoms from overlay
                    for (atom, count) in overlay_lock.added.iter() {
                        results.push(MultiplicityMatch::new(atom, count));
                    }

                    results
                } else {
                    // No overlay: directly convert base atoms
                    base_data
                        .atoms
                        .iter()
                        .map(|(atom, count)| MultiplicityMatch::new(atom, count))
                        .collect()
                }
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
            SpaceBacking::Owned { base, overlay } => {
                if let Some(overlay) = overlay {
                    let base_data = base.read();
                    let overlay_lock = overlay.read();

                    // Count = base total - removed total + added total
                    let base_total = base_data.atoms.total();
                    let removed_total = overlay_lock.removed.total();
                    let added_total = overlay_lock.added.total();

                    base_total.saturating_sub(removed_total) + added_total
                } else {
                    let data = base.read();
                    data.atoms.total()
                }
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
            SpaceBacking::Owned { base, overlay } => {
                if let Some(_overlay) = overlay {
                    // Need to count unique atoms across base + overlay - removed
                    self.collapse_to_multiset().distinct_count()
                } else {
                    let data = base.read();
                    data.atoms.distinct_count()
                }
            }
            SpaceBacking::Module { space, .. } => {
                let space = space.read();
                space.get_all_atoms().len() // Module spaces don't deduplicate
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
                    let overlay_lock = overlay.read();
                    let base_data = base.read();

                    // Check if added in overlay
                    if overlay_lock.added.contains(atom) {
                        return true;
                    }

                    // Check if in base (accounting for removals)
                    let base_count = base_data.atoms.count(atom);
                    let removal_count = overlay_lock.removal_count(atom);

                    base_count > removal_count
                } else {
                    let data = base.read();
                    data.atoms.contains(atom)
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
            SpaceBacking::Owned { base, overlay } => {
                if let Some(overlay) = overlay {
                    let overlay_lock = overlay.read();
                    let base_data = base.read();

                    let base_count = base_data.atoms.count(atom);
                    let removal_count = overlay_lock.removal_count(atom);
                    let added_count = overlay_lock.added.count(atom);

                    base_count.saturating_sub(removal_count) + added_count
                } else {
                    let data = base.read();
                    data.atoms.count(atom)
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

    /// Add a rule to this space.
    /// Note: For module spaces, rules are stored in the Environment, not ModuleSpace.
    pub fn add_rule(&self, rule: Rule) {
        match &self.backing {
            SpaceBacking::Owned { base, overlay } => {
                if let Some(overlay) = overlay {
                    // Forked: add to overlay
                    let mut overlay_lock = overlay.write();
                    overlay_lock.added_rules.push(rule);
                } else {
                    // Not forked: add directly to base
                    let mut data = base.write();
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
                let base_data = base.read();

                if let Some(overlay) = overlay {
                    let overlay_lock = overlay.read();
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

    // ========================================================================
    // Generic Value Operations
    // ========================================================================
    // These methods work with any value type implementing MettaValueTrait.
    // Atoms are stored as bytes via SymbolTable, enabling zero-conversion
    // evaluation at semantic boundaries.
    // ========================================================================

    /// Add an atom of any type implementing MettaValueTrait.
    ///
    /// The value is serialized to bytes and interned in the SymbolTable.
    /// This enables generic evaluation without conversion at boundaries.
    ///
    /// # Example
    /// ```ignore
    /// // Works with both MettaValue and MettaValue
    /// handle.add_atom_generic(&my_value);
    /// ```
    pub fn add_atom_generic<V: MettaValueTrait>(&self, atom: &V) {
        // Serialize to bytes - this is the canonical byte representation
        let bytes = atom.serialize();

        match &self.backing {
            SpaceBacking::Owned { base, overlay } => {
                if let Some(overlay) = overlay {
                    // Forked: add to overlay via deserialization to MettaValue
                    // (AtomMultisetSnapshot currently works with MettaValue)
                    let heap_atom = self.deserialize_to_metta(&bytes);
                    let mut overlay_lock = overlay.write();
                    // If this atom was previously removed, decrement removal count
                    if overlay_lock.removed.contains(&heap_atom) {
                        if let Some(new_removed) = overlay_lock.removed.remove(&heap_atom) {
                            overlay_lock.removed = new_removed;
                        }
                    }
                    overlay_lock.added = overlay_lock.added.insert(&heap_atom);
                } else {
                    // Not forked: add directly to base
                    let heap_atom = self.deserialize_to_metta(&bytes);
                    let mut data = base.write();
                    data.atoms = data.atoms.insert(&heap_atom);
                }
            }
            SpaceBacking::Module { space, .. } => {
                // Module spaces need MettaValue - deserialize from bytes
                let heap_atom = self.deserialize_to_metta(&bytes);
                let mut space = space.write();
                space.add_atom(heap_atom);
            }
        }
    }

    /// Remove an atom of any type implementing MettaValueTrait.
    ///
    /// Returns true if the atom was found and removed.
    pub fn remove_atom_generic<V: MettaValueTrait>(&self, atom: &V) -> bool {
        // Serialize to bytes for lookup
        let bytes = atom.serialize();
        let heap_atom = self.deserialize_to_metta(&bytes);

        // Delegate to existing remove_atom
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
    /// # Type Parameters
    ///
    /// - `V`: The output value type (must implement `MettaValueTrait + Clone`)
    /// - `F`: The factory type (must implement `MettaValueFactory<V>`)
    ///
    /// # Arguments
    ///
    /// - `factory`: Factory for constructing output values
    pub fn collapse_generic<V, F>(&self, factory: &F) -> Vec<V>
    where
        V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
        F: MettaValueFactory<V>,
    {
        // Get heap values and convert each to the target type
        let heap_atoms = self.collapse();
        heap_atoms
            .iter()
            .map(|atom| {
                // Serialize the MettaValue to bytes
                let bytes = atom.serialize();
                // Deserialize to the target type
                match factory.deserialize(&bytes) {
                    Ok((value, _)) => value,
                    Err(_) => {
                        // Fallback: create an error atom
                        factory.atom("?deserialization_error?")
                    }
                }
            })
            .collect()
    }

    /// Get atoms as MultiplicityMatch with their actual counts (generic version).
    ///
    /// This provides type consistency with `Environment::match_space()` and
    /// returns values in the requested generic type.
    pub fn collapse_with_multiplicity_generic<V, F>(&self, factory: &F) -> Vec<GenericMultiplicityMatch<V>>
    where
        V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
        F: MettaValueFactory<V>,
    {
        let heap_matches = self.collapse_with_multiplicity();
        heap_matches
            .into_iter()
            .map(|m| {
                let bytes = m.value.serialize();
                let value = match factory.deserialize(&bytes) {
                    Ok((v, _)) => v,
                    Err(_) => factory.atom("?deserialization_error?"),
                };
                GenericMultiplicityMatch {
                    value,
                    count: m.count,
                }
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
            (SpaceBacking::Module { mod_id: a, .. }, SpaceBacking::Module { mod_id: b, .. }) => {
                a == b
            }
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

        // Add different atoms with different multiplicities
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
        // This test verifies that adding the same atom many times
        // doesn't create N copies in memory
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

    #[test]
    fn test_multiplicity_collapse_to_multiset() {
        let handle = SpaceHandle::new(1, "test".to_string());
        let atom = MettaValue::Long(7);

        for _ in 0..100 {
            handle.add_atom(atom.clone());
        }

        // Get multiset snapshot (O(1) clone)
        let multiset = handle.collapse_to_multiset();

        // Should have 100 total
        assert_eq!(multiset.total(), 100);

        // Should have 1 distinct
        assert_eq!(multiset.distinct_count(), 1);

        // Verify it's a proper snapshot (modifications don't affect original)
        let multiset2 = multiset.insert(&MettaValue::Long(8));
        assert_eq!(multiset2.total(), 101);
        assert_eq!(multiset.total(), 100); // Original unchanged
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
        // Test that add_atom_generic and collapse_generic are semantically equivalent
        // to add_atom and collapse
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
