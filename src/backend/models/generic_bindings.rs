//! Generic Variable Bindings for Pattern Matching
//!
//! This module provides `GenericBindings<V>`, a generic bindings type that stores
//! values of any type implementing `MettaValueTrait`. This eliminates boundary
//! conversions during pattern matching by allowing bindings to store values in
//! their native representation (either heap-allocated `MettaValue` or arena-allocated
//! `ArenaValue`).
//!
//! ## Design
//!
//! The type mirrors `SmartBindings` but is parameterized over the value type `V`:
//! - Empty: Zero-cost for no bindings
//! - Single: Inline for 1 binding (eliminates iterator/closure overhead)
//! - Small: SmallVec for 2-8 bindings (stack-allocated, cache-friendly)
//! - Large: SmallVec spills to heap for >8 bindings
//!
//! ## Performance
//!
//! By using `GenericBindings<V>`, we avoid O(n) deep allocations during pattern
//! matching and binding application:
//! - `MettaValue.clone()` = O(1) Arc increment
//! - `ArenaValue.clone()` = O(1) pointer copy
//! - No conversions between types during evaluation

use smallvec::SmallVec;
use std::fmt::Debug;

use super::MettaValueTrait;

/// Generic bindings structure optimized for common cases.
///
/// Parameterized over the value type `V`, enabling zero-conversion pattern
/// matching with both heap and arena allocation strategies.
//
// Clippy warns about the large size difference between variants (Empty: 0 bytes,
// Single: varies, Small: varies), recommending we Box the SmallVec to reduce
// the enum size.
//
// However, benchmarking shows that the unboxed version provides significant performance
// improvements in the pattern matching hot path (see SmartBindings benchmarks).
// The same rationale applies here:
// - Passed by reference (no copy overhead from large size)
// - Short-lived (created during pattern matching, quickly dropped)
// - Used in performance-critical code paths
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum GenericBindings<V: MettaValueTrait + Clone> {
    /// No bindings (zero-cost)
    Empty,
    /// Single binding (inline, no allocation)
    Single((String, V)),
    /// 2-8 bindings (stack-allocated via SmallVec)
    /// >8 bindings (SmallVec spills to heap automatically)
    Small(SmallVec<[(String, V); 8]>),
}

impl<V: MettaValueTrait + Clone> GenericBindings<V> {
    /// Create empty bindings
    #[inline]
    pub fn new() -> Self {
        GenericBindings::Empty
    }

    /// Get a binding by name
    #[inline]
    pub fn get(&self, name: &str) -> Option<&V> {
        match self {
            GenericBindings::Empty => None,
            GenericBindings::Single((n, v)) => {
                if n == name {
                    Some(v)
                } else {
                    None
                }
            }
            GenericBindings::Small(vec) => vec.iter().find(|(n, _)| n == name).map(|(_, v)| v),
        }
    }

    /// Insert a binding
    ///
    /// Transitions:
    /// - Empty → Single
    /// - Single → Small (with 2 elements)
    /// - Small → Small (push)
    #[inline]
    pub fn insert(&mut self, name: String, value: V) {
        match self {
            GenericBindings::Empty => {
                *self = GenericBindings::Single((name, value));
            }
            GenericBindings::Single(existing) => {
                // Transition to Small with 2 elements
                let mut vec = SmallVec::new();
                vec.push(existing.clone());
                vec.push((name, value));
                *self = GenericBindings::Small(vec);
            }
            GenericBindings::Small(vec) => {
                vec.push((name, value));
            }
        }
    }

    /// Iterate over all bindings
    pub fn iter(&self) -> GenericBindingsIter<'_, V> {
        GenericBindingsIter {
            bindings: self,
            index: 0,
        }
    }

    /// Get the number of bindings
    #[inline]
    pub fn len(&self) -> usize {
        match self {
            GenericBindings::Empty => 0,
            GenericBindings::Single(_) => 1,
            GenericBindings::Small(vec) => vec.len(),
        }
    }

    /// Check if there are no bindings
    #[inline]
    pub fn is_empty(&self) -> bool {
        matches!(self, GenericBindings::Empty)
    }

    /// Merge bindings from another GenericBindings, checking for conflicts.
    ///
    /// Returns `true` if merge was successful, `false` if there was a conflict
    /// (same variable bound to different values).
    pub fn merge(&mut self, other: &GenericBindings<V>) -> bool
    where
        V: PartialEq,
    {
        for (name, value) in other.iter() {
            if let Some(existing) = self.get(name) {
                // Check for conflict
                if existing != value {
                    return false;
                }
                // Same value, skip insertion
            } else {
                self.insert(name.clone(), value.clone());
            }
        }
        true
    }

    /// Extend bindings from an iterator of (name, value) pairs.
    pub fn extend<I>(&mut self, iter: I)
    where
        I: IntoIterator<Item = (String, V)>,
    {
        for (name, value) in iter {
            self.insert(name, value);
        }
    }
}

impl<V: MettaValueTrait + Clone> Default for GenericBindings<V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<V: MettaValueTrait + Clone + PartialEq> PartialEq for GenericBindings<V> {
    fn eq(&self, other: &Self) -> bool {
        if self.len() != other.len() {
            return false;
        }
        for (name, value) in self.iter() {
            match other.get(name) {
                Some(other_value) if other_value == value => continue,
                _ => return false,
            }
        }
        true
    }
}

/// Iterator over generic bindings
pub struct GenericBindingsIter<'a, V: MettaValueTrait + Clone> {
    bindings: &'a GenericBindings<V>,
    index: usize,
}

impl<'a, V: MettaValueTrait + Clone> Iterator for GenericBindingsIter<'a, V> {
    type Item = (&'a String, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        match self.bindings {
            GenericBindings::Empty => None,
            GenericBindings::Single((n, v)) => {
                if self.index == 0 {
                    self.index += 1;
                    Some((n, v))
                } else {
                    None
                }
            }
            GenericBindings::Small(vec) => {
                if self.index < vec.len() {
                    let result = &vec[self.index];
                    self.index += 1;
                    Some((&result.0, &result.1))
                } else {
                    None
                }
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = match self.bindings {
            GenericBindings::Empty => 0,
            GenericBindings::Single(_) => {
                if self.index == 0 {
                    1
                } else {
                    0
                }
            }
            GenericBindings::Small(vec) => vec.len().saturating_sub(self.index),
        };
        (remaining, Some(remaining))
    }
}

impl<'a, V: MettaValueTrait + Clone> ExactSizeIterator for GenericBindingsIter<'a, V> {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::MettaValue;

    #[test]
    fn test_empty_bindings() {
        let bindings: GenericBindings<MettaValue> = GenericBindings::new();
        assert!(bindings.is_empty());
        assert_eq!(bindings.len(), 0);
        assert_eq!(bindings.get("$x"), None);
    }

    #[test]
    fn test_single_binding() {
        let mut bindings: GenericBindings<MettaValue> = GenericBindings::new();
        bindings.insert("$x".to_string(), MettaValue::Long(42));

        assert!(!bindings.is_empty());
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings.get("$x"), Some(&MettaValue::Long(42)));
        assert_eq!(bindings.get("$y"), None);

        // Check variant
        assert!(matches!(bindings, GenericBindings::Single(_)));
    }

    #[test]
    fn test_transition_to_small() {
        let mut bindings: GenericBindings<MettaValue> = GenericBindings::new();
        bindings.insert("$x".to_string(), MettaValue::Long(42));
        bindings.insert("$y".to_string(), MettaValue::Long(43));

        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings.get("$x"), Some(&MettaValue::Long(42)));
        assert_eq!(bindings.get("$y"), Some(&MettaValue::Long(43)));

        // Check variant transitioned to Small
        assert!(matches!(bindings, GenericBindings::Small(_)));
    }

    #[test]
    fn test_small_bindings() {
        let mut bindings: GenericBindings<MettaValue> = GenericBindings::new();
        for i in 0..5 {
            bindings.insert(format!("$v{}", i), MettaValue::Long(i as i64));
        }

        assert_eq!(bindings.len(), 5);
        for i in 0..5 {
            assert_eq!(
                bindings.get(&format!("$v{}", i)),
                Some(&MettaValue::Long(i as i64))
            );
        }
    }

    #[test]
    fn test_iterator() {
        let mut bindings: GenericBindings<MettaValue> = GenericBindings::new();
        bindings.insert("$x".to_string(), MettaValue::Long(1));
        bindings.insert("$y".to_string(), MettaValue::Long(2));
        bindings.insert("$z".to_string(), MettaValue::Long(3));

        let collected: Vec<_> = bindings.iter().collect();
        assert_eq!(collected.len(), 3);

        // Check all bindings are present
        let has_x = collected
            .iter()
            .any(|(n, v)| n == &"$x" && **v == MettaValue::Long(1));
        let has_y = collected
            .iter()
            .any(|(n, v)| n == &"$y" && **v == MettaValue::Long(2));
        let has_z = collected
            .iter()
            .any(|(n, v)| n == &"$z" && **v == MettaValue::Long(3));
        assert!(has_x && has_y && has_z);
    }

    #[test]
    fn test_empty_iterator() {
        let bindings: GenericBindings<MettaValue> = GenericBindings::new();
        let collected: Vec<_> = bindings.iter().collect();
        assert_eq!(collected.len(), 0);
    }

    #[test]
    fn test_single_iterator() {
        let mut bindings: GenericBindings<MettaValue> = GenericBindings::new();
        bindings.insert("$x".to_string(), MettaValue::Long(42));

        let collected: Vec<_> = bindings.iter().collect();
        assert_eq!(collected.len(), 1);
        assert_eq!(collected[0].0, "$x");
        assert_eq!(*collected[0].1, MettaValue::Long(42));
    }

    #[test]
    fn test_merge_success() {
        let mut bindings1: GenericBindings<MettaValue> = GenericBindings::new();
        bindings1.insert("$x".to_string(), MettaValue::Long(1));

        let mut bindings2: GenericBindings<MettaValue> = GenericBindings::new();
        bindings2.insert("$y".to_string(), MettaValue::Long(2));

        assert!(bindings1.merge(&bindings2));
        assert_eq!(bindings1.len(), 2);
        assert_eq!(bindings1.get("$x"), Some(&MettaValue::Long(1)));
        assert_eq!(bindings1.get("$y"), Some(&MettaValue::Long(2)));
    }

    #[test]
    fn test_merge_conflict() {
        let mut bindings1: GenericBindings<MettaValue> = GenericBindings::new();
        bindings1.insert("$x".to_string(), MettaValue::Long(1));

        let mut bindings2: GenericBindings<MettaValue> = GenericBindings::new();
        bindings2.insert("$x".to_string(), MettaValue::Long(2)); // Different value!

        assert!(!bindings1.merge(&bindings2)); // Conflict!
    }

    #[test]
    fn test_merge_same_value() {
        let mut bindings1: GenericBindings<MettaValue> = GenericBindings::new();
        bindings1.insert("$x".to_string(), MettaValue::Long(1));

        let mut bindings2: GenericBindings<MettaValue> = GenericBindings::new();
        bindings2.insert("$x".to_string(), MettaValue::Long(1)); // Same value

        assert!(bindings1.merge(&bindings2)); // No conflict
        assert_eq!(bindings1.len(), 1); // Still just one binding
    }

    #[test]
    fn test_equality() {
        let mut bindings1: GenericBindings<MettaValue> = GenericBindings::new();
        bindings1.insert("$x".to_string(), MettaValue::Long(1));
        bindings1.insert("$y".to_string(), MettaValue::Long(2));

        let mut bindings2: GenericBindings<MettaValue> = GenericBindings::new();
        bindings2.insert("$y".to_string(), MettaValue::Long(2));
        bindings2.insert("$x".to_string(), MettaValue::Long(1));

        assert_eq!(bindings1, bindings2);
    }
}
