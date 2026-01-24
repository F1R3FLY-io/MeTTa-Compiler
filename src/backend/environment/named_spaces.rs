//! Named space operations for Environment.
//!
//! Provides methods for creating and managing named spaces (new-space, add-atom, remove-atom, collapse).
//!
//! For retrieving atoms from named spaces, use `collapse_named_space_iter()` which provides
//! lazy iteration with O(1) memory overhead.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::RwLockReadGuard;

use either::Either;
use owning_ref::OwningHandle;

use super::{Environment, MettaValue};

/// Lazy iterator over named space atoms.
///
/// Uses `OwningHandle` to keep the RwLock guard alive while iterating.
/// Atoms are cloned lazily as the iterator is consumed, avoiding the need
/// to clone the entire Vec upfront.
///
/// # Performance
/// - Memory overhead: O(1) for the iterator structure
/// - Atoms cloned: O(k) where k = number of items actually consumed
///
/// # Example
/// ```ignore
/// // Lazy iteration - clones atoms on demand
/// for atom in env.collapse_named_space_iter(space_id).take(5) {
///     // Only 5 atoms are cloned, regardless of space size
///     println!("{:?}", atom);
/// }
/// ```
pub struct NamedSpaceIter<'a> {
    inner: Either<
        OwningHandle<
            RwLockReadGuard<'a, HashMap<u64, (String, Vec<MettaValue>)>>,
            Box<dyn Iterator<Item = MettaValue> + 'a>,
        >,
        std::iter::Empty<MettaValue>,
    >,
}

impl<'a> Iterator for NamedSpaceIter<'a> {
    type Item = MettaValue;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        match &mut self.inner {
            Either::Left(handle) => handle.next(),
            Either::Right(empty) => empty.next(),
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        match &self.inner {
            Either::Left(handle) => handle.size_hint(),
            Either::Right(_) => (0, Some(0)),
        }
    }
}

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

    /// Lazy iterator over named space atoms.
    ///
    /// Uses `OwningHandle` to keep the RwLock guard alive while iterating.
    /// Atoms are cloned lazily as the iterator is consumed, avoiding the need
    /// to clone the entire Vec upfront.
    ///
    /// # Performance
    /// - Memory overhead: O(1) for the iterator structure
    /// - Atoms cloned: O(k) where k = number of items actually consumed
    /// - Useful when you only need a subset of atoms or want to short-circuit
    ///
    /// # Example
    /// ```ignore
    /// // Only clones atoms as they're consumed
    /// for atom in env.collapse_named_space_iter(space_id).take(5) {
    ///     println!("{:?}", atom);
    /// }
    ///
    /// // Check if any atom matches a condition (short-circuits on first match)
    /// let has_target = env.collapse_named_space_iter(space_id)
    ///     .any(|atom| atom == target);
    /// ```
    pub fn collapse_named_space_iter(&self, space_id: u64) -> NamedSpaceIter<'_> {
        let guard = self
            .shared
            .named_spaces
            .read()
            .expect("named_spaces lock poisoned");

        // Check if space exists before creating OwningHandle
        if !guard.contains_key(&space_id) {
            return NamedSpaceIter {
                inner: Either::Right(std::iter::empty()),
            };
        }

        // Create OwningHandle that keeps guard alive while we iterate
        // Safety: The OwningHandle ensures the guard lives as long as the iterator
        let handle = OwningHandle::new_with_fn(guard, |spaces_ptr| {
            let spaces = unsafe { &*spaces_ptr };
            match spaces.get(&space_id) {
                Some((_, atoms)) => {
                    Box::new(atoms.iter().cloned()) as Box<dyn Iterator<Item = MettaValue> + '_>
                }
                None => Box::new(std::iter::empty()) as Box<dyn Iterator<Item = MettaValue> + '_>,
            }
        });

        NamedSpaceIter {
            inner: Either::Left(handle),
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
