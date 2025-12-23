//! Lazy Cartesian Product Iterator
//!
//! Generates Cartesian products on-demand using an index-based "multi-digit counter"
//! pattern. Memory usage is O(n) for indices regardless of product size.

use smallvec::SmallVec;
use std::sync::Arc;
use tracing::trace;

use crate::backend::models::MettaValue;

/// Maximum number of results from cartesian product to prevent combinatorial explosion
pub const MAX_CARTESIAN_RESULTS: usize = 10_000;

/// SmallVec type for Cartesian product combinations.
/// Stack-allocated for arity <= 8 (common case), heap-allocated otherwise.
/// This reduces allocation overhead for typical MeTTa expressions.
pub type Combination = SmallVec<[MettaValue; 8]>;

/// Lazy Cartesian product iterator using multi-digit counter approach.
/// Memory: O(n) for indices regardless of total product size.
/// Uses SmallVec for combinations to avoid heap allocation for small expressions.
/// Uses Arc<Vec<MettaValue>> for result lists to enable O(1) cloning of the iterator.
#[derive(Debug, Clone)]
pub struct CartesianProductIter {
    /// Arc-wrapped result lists for O(1) cloning of source data
    results: Vec<Arc<Vec<MettaValue>>>,
    /// Current indices into each result list (the "counter")
    /// SmallVec<[usize; 8]> for stack allocation with typical arity
    indices: SmallVec<[usize; 8]>,
    /// Whether the iterator is exhausted
    exhausted: bool,
}

impl CartesianProductIter {
    /// Create a new lazy Cartesian product iterator.
    /// Returns None if any result list is empty (no combinations possible).
    pub fn new(results: Vec<Vec<MettaValue>>) -> Option<Self> {
        // Check for empty result lists - no combinations possible
        if results.iter().any(|r| r.is_empty()) {
            return None;
        }

        // Use SmallVec for indices - stack allocated for arity <= 8
        let indices = smallvec::smallvec![0; results.len()];

        // Wrap each result list in Arc for O(1) cloning
        let arc_results: Vec<Arc<Vec<MettaValue>>> =
            results.into_iter().map(Arc::new).collect();

        Some(CartesianProductIter {
            indices,
            results: arc_results,
            exhausted: false,
        })
    }

    /// Create from pre-wrapped Arc results (avoids re-wrapping).
    pub fn from_arc(results: Vec<Arc<Vec<MettaValue>>>) -> Option<Self> {
        // Check for empty result lists - no combinations possible
        if results.iter().any(|r| r.is_empty()) {
            return None;
        }

        let indices = smallvec::smallvec![0; results.len()];

        Some(CartesianProductIter {
            indices,
            results,
            exhausted: false,
        })
    }

    /// Advance indices like a multi-digit counter (rightmost varies fastest).
    /// Returns false when counter overflows (all combinations exhausted).
    fn advance_indices(&mut self) {
        for i in (0..self.indices.len()).rev() {
            self.indices[i] += 1;
            if self.indices[i] < self.results[i].len() {
                return; // No carry needed
            }
            self.indices[i] = 0; // Carry to next digit
        }
        // Counter overflowed - all combinations exhausted
        self.exhausted = true;
    }
}

impl Iterator for CartesianProductIter {
    type Item = Combination;

    fn next(&mut self) -> Option<Self::Item> {
        if self.exhausted || self.results.is_empty() {
            return None;
        }

        // Build current combination from indices using SmallVec
        // Stack-allocated for arity <= 8, avoiding heap allocation for common cases
        let mut combo = SmallVec::with_capacity(self.indices.len());
        for (&idx, list) in self.indices.iter().zip(self.results.iter()) {
            combo.push(list[idx].clone());
        }

        self.advance_indices();
        Some(combo)
    }
}

/// Result of creating a lazy Cartesian product
#[derive(Debug)]
pub enum CartesianProductResult {
    /// Fast path: exactly one combination (deterministic evaluation)
    /// This is the common case for arithmetic and most builtin operations
    /// Uses Combination (SmallVec) for stack allocation
    Single(Combination),
    /// Lazy iterator over multiple combinations
    Lazy(CartesianProductIter),
    /// No combinations (empty input or empty result list)
    Empty,
}

/// Create a lazy Cartesian product from evaluation results.
/// Preserves fast path for deterministic evaluation (all single-element results).
pub fn cartesian_product_lazy(results: Vec<Vec<MettaValue>>) -> CartesianProductResult {
    if results.is_empty() {
        // Empty input produces single empty combination (stack-allocated)
        return CartesianProductResult::Single(SmallVec::new());
    }

    // FAST PATH: If all result lists have exactly 1 item (deterministic evaluation),
    // we can just concatenate them directly in O(n) instead of using the iterator
    // This is the common case for arithmetic and most builtin operations
    if results.iter().all(|r| r.len() == 1) {
        // Use SmallVec for stack allocation with typical arity
        let single_combo: Combination = results.into_iter().map(|mut r| r.pop().unwrap()).collect();
        return CartesianProductResult::Single(single_combo);
    }

    // Check for empty result lists
    if results.iter().any(|r| r.is_empty()) {
        return CartesianProductResult::Empty;
    }

    // Create lazy iterator for nondeterministic evaluation
    match CartesianProductIter::new(results) {
        Some(iter) => CartesianProductResult::Lazy(iter),
        None => CartesianProductResult::Empty,
    }
}

/// Compute Cartesian product of multiple result sets (eager version)
///
/// Each element in `results` is a list of possible values.
/// Returns all combinations where one value is selected from each list.
///
/// Example: [[a, b], [1, 2]] -> [[a, 1], [a, 2], [b, 1], [b, 2]]
///
/// This function has a built-in limit (MAX_CARTESIAN_RESULTS) to prevent combinatorial explosion.
/// Returns Err with an error message if the limit is exceeded.
pub fn cartesian_product(results: &[Vec<MettaValue>]) -> Result<Vec<Vec<MettaValue>>, MettaValue> {
    trace!(target: "mettatron::backend::eval::cartesian_product", ?results);
    if results.is_empty() {
        return Ok(vec![vec![]]);
    }

    // FAST PATH: If all result lists have exactly 1 item (deterministic evaluation),
    // we can just concatenate them directly in O(n) instead of O(n²)
    // This is the common case for arithmetic and most builtin operations
    if results.iter().all(|r| r.len() == 1) {
        let single_combo: Vec<MettaValue> = results.iter().map(|r| r[0].clone()).collect();
        trace!(
            target: "mettatron::backend::eval::cartesian_product",
            ?single_combo, "Concatenate all rules to deterministic evaluation"
        );
        return Ok(vec![single_combo]);
    }

    // Calculate the total product size first to check if it would exceed the limit
    let total_size: usize = results
        .iter()
        .map(|r| r.len().max(1))
        .fold(1usize, |acc, len| acc.saturating_mul(len));

    if total_size > MAX_CARTESIAN_RESULTS {
        return Err(MettaValue::Error(
            format!(
                "Combinatorial explosion: evaluation would produce {} results, exceeding limit of {}. \
                 Consider simplifying the expression or adding constraints.",
                total_size, MAX_CARTESIAN_RESULTS
            ),
            Arc::new(MettaValue::Atom("LimitExceeded".to_string())),
        ));
    }

    // Iterative Cartesian product for non-deterministic cases
    // Start with a single empty combination
    let mut product = vec![Vec::with_capacity(results.len())];

    // Process each result list and extend all existing combinations
    for result_list in results {
        if result_list.is_empty() {
            // Empty list contributes nothing to combinations
            continue;
        }

        let new_capacity = product
            .len()
            .checked_mul(result_list.len())
            .ok_or_else(|| {
                MettaValue::Error(
                    "Combinatorial explosion: integer overflow in cartesian product".to_string(),
                    Arc::new(MettaValue::Atom("Overflow".to_string())),
                )
            })?;
        let mut new_product = Vec::with_capacity(new_capacity);

        for combo in &product {
            for item in result_list {
                let mut new_combo = combo.clone();
                new_combo.push(item.clone());
                new_product.push(new_combo);
            }
        }

        product = new_product;
    }

    Ok(product)
}
