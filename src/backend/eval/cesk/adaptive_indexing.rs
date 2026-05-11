//! Adaptive Multi-Argument Indexing (Phase 5.5, SICStus/YAP Prolog-inspired)
//!
//! Dynamically selects which argument position(s) to use for rule candidate
//! indexing based on runtime query statistics. Argument positions with higher
//! discrimination power (more distinct head symbols) are preferred.

use std::collections::HashMap;

use smallvec::SmallVec;

use crate::backend::models::MettaValueTrait;

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for adaptive indexing.
#[derive(Debug, Clone)]
pub struct AdaptiveConfig {
    /// Queries before first rebalance consideration.
    pub warmup_queries: u64,
    /// Minimum selectivity improvement to justify switching.
    pub rebalance_threshold: f32,
    /// Check for rebalance every N queries.
    pub rebalance_interval: u64,
    /// Minimum number of rules to enable adaptive indexing (below this, overhead exceeds benefit).
    pub min_rules: usize,
}

impl Default for AdaptiveConfig {
    fn default() -> Self {
        Self {
            warmup_queries: 100,
            rebalance_threshold: 0.1,
            rebalance_interval: 1000,
            min_rules: 8,
        }
    }
}

// ============================================================================
// Group Statistics
// ============================================================================

/// Runtime statistics for a (head, arity) rule group.
#[derive(Debug, Default, Clone)]
pub struct GroupStats {
    /// Total queries against this group.
    pub query_count: u64,
    /// Per-argument-position discrimination power:
    /// number of distinct head symbols observed at each position.
    pub distinct_heads_per_arg: SmallVec<[u16; 8]>,
    /// Currently selected primary index argument position.
    pub primary_index_arg: u8,
    /// Last query count at which rebalance was checked.
    pub last_rebalance_at: u64,
}

impl GroupStats {
    /// Create stats for a group with given arity.
    pub fn new(arity: usize) -> Self {
        Self {
            query_count: 0,
            distinct_heads_per_arg: SmallVec::from_elem(0, arity.saturating_sub(1)), // exclude head position
            primary_index_arg: 0, // Default: first argument
            last_rebalance_at: 0,
        }
    }

    /// Record a query, updating per-argument head counts.
    ///
    /// `arg_heads` is a slice of optional head symbols for each argument
    /// position (None if the argument is not an atom or S-expression with head).
    pub fn record_query(&mut self, arg_heads: &[Option<&str>]) {
        self.query_count += 1;
        // Note: distinct head counting requires a more complex approach
        // (e.g., HyperLogLog or small HashSet per position). For now,
        // we track whether arguments have concrete heads at all.
        for (i, head) in arg_heads.iter().enumerate() {
            if i < self.distinct_heads_per_arg.len() && head.is_some() {
                // Approximate: increment if head is concrete
                self.distinct_heads_per_arg[i] = self.distinct_heads_per_arg[i].saturating_add(1);
            }
        }
    }

    /// Check if rebalance should be considered.
    pub fn should_rebalance(&self, config: &AdaptiveConfig) -> bool {
        self.query_count >= config.warmup_queries
            && self.query_count - self.last_rebalance_at >= config.rebalance_interval
    }

    /// Select the best argument position for indexing.
    ///
    /// Returns the position with the highest discrimination power
    /// (most distinct heads observed). Returns None if no improvement
    /// exceeds the threshold.
    pub fn best_index_arg(&self, config: &AdaptiveConfig) -> Option<u8> {
        if self.distinct_heads_per_arg.is_empty() {
            return None;
        }

        let current_score = self
            .distinct_heads_per_arg
            .get(self.primary_index_arg as usize)
            .copied()
            .unwrap_or(0);

        let mut best_pos = self.primary_index_arg;
        let mut best_score = current_score;

        for (i, &score) in self.distinct_heads_per_arg.iter().enumerate() {
            if score > best_score {
                best_score = score;
                best_pos = i as u8;
            }
        }

        // Only switch if improvement exceeds threshold
        if best_pos != self.primary_index_arg
            && current_score > 0
            && (best_score as f32 - current_score as f32) / current_score as f32
                >= config.rebalance_threshold
        {
            Some(best_pos)
        } else if best_pos != self.primary_index_arg && current_score == 0 && best_score > 0 {
            // Current position has no discrimination; any improvement is good
            Some(best_pos)
        } else {
            None
        }
    }
}

// ============================================================================
// Adaptive Index Registry
// ============================================================================

/// Registry of per-group adaptive statistics.
///
/// Thread-local. Updated during rule matching, consulted during rebalance.
#[derive(Debug, Default)]
pub struct AdaptiveRegistry {
    /// (head symbol hash, arity) -> GroupStats.
    /// Uses u64 hash of head symbol to avoid &'static str lifetime issues.
    groups: HashMap<(u64, usize), GroupStats>,
    /// Configuration.
    config: AdaptiveConfig,
}

impl AdaptiveRegistry {
    pub fn new() -> Self {
        Self {
            groups: HashMap::new(),
            config: AdaptiveConfig::default(),
        }
    }

    pub fn with_config(config: AdaptiveConfig) -> Self {
        Self {
            groups: HashMap::new(),
            config,
        }
    }

    /// Record a query for a (head, arity) group.
    pub fn record_query(&mut self, head_hash: u64, arity: usize, arg_heads: &[Option<&str>]) {
        let stats = self
            .groups
            .entry((head_hash, arity))
            .or_insert_with(|| GroupStats::new(arity));
        stats.record_query(arg_heads);
    }

    /// Check if any group needs rebalancing.
    pub fn check_rebalances(&mut self) -> Vec<(u64, usize, u8)> {
        let mut rebalances = Vec::new();
        for (&(head_hash, arity), stats) in &mut self.groups {
            if stats.should_rebalance(&self.config) {
                stats.last_rebalance_at = stats.query_count;
                if let Some(new_arg) = stats.best_index_arg(&self.config) {
                    stats.primary_index_arg = new_arg;
                    rebalances.push((head_hash, arity, new_arg));
                }
            }
        }
        rebalances
    }

    /// Get the currently selected index argument for a group.
    pub fn index_arg_for(&self, head_hash: u64, arity: usize) -> u8 {
        self.groups
            .get(&(head_hash, arity))
            .map(|s| s.primary_index_arg)
            .unwrap_or(0) // Default: first argument
    }

    /// Clear all statistics.
    pub fn clear(&mut self) {
        self.groups.clear();
    }

    /// Number of tracked groups.
    pub fn group_count(&self) -> usize {
        self.groups.len()
    }
}

// ============================================================================
// Thread-Local Access
// ============================================================================

use std::cell::RefCell;

thread_local! {
    static ADAPTIVE_REGISTRY: RefCell<AdaptiveRegistry> = RefCell::new(AdaptiveRegistry::new());
}

/// Access the thread-local adaptive registry.
#[inline]
pub fn with_adaptive_registry<R>(f: impl FnOnce(&mut AdaptiveRegistry) -> R) -> R {
    ADAPTIVE_REGISTRY.with(|cell| f(&mut cell.borrow_mut()))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_group_stats_new() {
        let stats = GroupStats::new(3);
        assert_eq!(stats.query_count, 0);
        assert_eq!(stats.primary_index_arg, 0);
        assert_eq!(stats.distinct_heads_per_arg.len(), 2); // 3 arity - 1 head = 2 args
    }

    #[test]
    fn test_record_query() {
        let mut stats = GroupStats::new(3);
        stats.record_query(&[Some("a"), Some("b")]);
        assert_eq!(stats.query_count, 1);
        assert_eq!(stats.distinct_heads_per_arg[0], 1);
        assert_eq!(stats.distinct_heads_per_arg[1], 1);
    }

    #[test]
    fn test_should_rebalance() {
        let config = AdaptiveConfig {
            warmup_queries: 10,
            rebalance_interval: 5,
            ..Default::default()
        };
        let mut stats = GroupStats::new(3);

        for _ in 0..10 {
            stats.record_query(&[Some("a"), Some("b")]);
        }
        assert!(stats.should_rebalance(&config));
    }

    #[test]
    fn test_best_index_arg() {
        let config = AdaptiveConfig::default();
        let mut stats = GroupStats::new(4); // 3 argument positions

        // Position 0: 5 distinct heads, position 1: 10, position 2: 3
        stats.distinct_heads_per_arg = SmallVec::from_slice(&[5, 10, 3]);
        stats.primary_index_arg = 0;

        let best = stats.best_index_arg(&config);
        assert_eq!(best, Some(1)); // Position 1 is most discriminative
    }

    #[test]
    fn test_registry() {
        let mut registry = AdaptiveRegistry::new();
        registry.record_query(42, 3, &[Some("a"), Some("b")]);
        assert_eq!(registry.group_count(), 1);
        assert_eq!(registry.index_arg_for(42, 3), 0); // Default
    }

    #[test]
    fn test_registry_clear() {
        let mut registry = AdaptiveRegistry::new();
        registry.record_query(42, 3, &[Some("a")]);
        registry.clear();
        assert_eq!(registry.group_count(), 0);
    }
}
