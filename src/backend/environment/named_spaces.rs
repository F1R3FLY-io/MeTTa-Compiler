//! Named space operations for Environment.
//!
//! Provides methods for creating and managing named spaces (new-space, add-atom, remove-atom, collapse).
//!
//! For retrieving atoms from named spaces, use `collapse_named_space_iter()` which provides
//! lazy iteration with O(1) memory overhead.

use std::sync::atomic::Ordering;

use super::generic::GenericEnvironment;
use crate::backend::models::metta_value_trait::{MettaValueFactory, MettaValue as MettaValueTrait};
use crate::backend::MettaValue;

/// Lazy iterator over named space atoms.
///
/// With DashMap, we collect the atoms into a Vec for iteration since DashMap
/// doesn't support OwningHandle pattern. The atoms are cloned during collection.
///
/// # Performance
/// - Memory overhead: O(n) where n = number of atoms in the space
/// - Atoms cloned: O(n) during collection
///
/// # Example
/// ```ignore
/// // Iteration - atoms are collected from DashMap
/// for atom in env.collapse_named_space_iter(space_id).take(5) {
///     println!("{:?}", atom);
/// }
/// ```
pub struct NamedSpaceIter {
    inner: std::vec::IntoIter<MettaValue>,
}

impl Iterator for NamedSpaceIter {
    type Item = MettaValue;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<V, F> GenericEnvironment<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    /// Create a new named space and return its ID
    /// Used by new-space operation
    pub fn create_named_space(&mut self, name: &str) -> u64 {
        self.make_owned();

        // AtomicU64 - use fetch_add for atomic increment
        let id = self.shared.next_space_id.fetch_add(1, Ordering::AcqRel);

        // DashMap - use .insert() directly
        self.shared
            .named_spaces
            .insert(id, (name.to_string(), Vec::new()));

        self.modified.store(true, Ordering::Release);
        id
    }

    /// Add an atom to a named space by ID
    /// Used by add-atom operation
    pub fn add_to_named_space(&mut self, space_id: u64, value: V) -> bool {
        self.make_owned();

        // DashMap - use .get_mut() for mutable access
        if let Some(mut entry) = self.shared.named_spaces.get_mut(&space_id) {
            entry.value_mut().1.push(value);
            self.modified.store(true, Ordering::Release);
            true
        } else {
            false
        }
    }

    /// Remove an atom from a named space by ID
    /// Used by remove-atom operation
    pub fn remove_from_named_space(&mut self, space_id: u64, value: &V) -> bool
    where
        V: PartialEq,
    {
        self.make_owned();

        // DashMap - use .get_mut() for mutable access
        if let Some(mut entry) = self.shared.named_spaces.get_mut(&space_id) {
            let (_, atoms) = entry.value_mut();
            // Remove first matching atom
            if let Some(pos) = atoms.iter().position(|x| x == value) {
                atoms.remove(pos);
                self.modified.store(true, Ordering::Release);
                return true;
            }
        }
        false
    }

    /// Get atoms from a named space (collects into Vec).
    pub fn collapse_named_space(&self, space_id: u64) -> Vec<V> {
        // DashMap - use .get() directly
        self.shared
            .named_spaces
            .get(&space_id)
            .map(|entry| entry.value().1.clone())
            .unwrap_or_default()
    }

    /// Check if a named space exists
    pub fn has_named_space(&self, space_id: u64) -> bool {
        // DashMap - use .contains_key() directly
        self.shared.named_spaces.contains_key(&space_id)
    }
}

// MettaValue-specific iterator for collapse_named_space_iter
impl super::Environment {
    /// Iterator over named space atoms.
    ///
    /// With DashMap, atoms are collected into a Vec for iteration.
    ///
    /// # Performance
    /// - Memory overhead: O(n) for the collected Vec
    /// - Atoms cloned: O(n) during collection
    ///
    /// # Example
    /// ```ignore
    /// for atom in env.collapse_named_space_iter(space_id).take(5) {
    ///     println!("{:?}", atom);
    /// }
    ///
    /// // Check if any atom matches a condition
    /// let has_target = env.collapse_named_space_iter(space_id)
    ///     .any(|atom| atom == target);
    /// ```
    pub fn collapse_named_space_iter(&self, space_id: u64) -> NamedSpaceIter {
        // DashMap - use .get() directly
        let atoms = self
            .shared
            .named_spaces
            .get(&space_id)
            .map(|entry| entry.value().1.clone())
            .unwrap_or_default();

        NamedSpaceIter {
            inner: atoms.into_iter(),
        }
    }
}
