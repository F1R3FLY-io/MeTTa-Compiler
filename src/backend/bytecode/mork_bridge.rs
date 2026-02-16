//! MORK Bridge Layer for Bytecode VM
//!
//! This module bridges the bytecode VM with MORK/PathMap for rule lookup.
//! The bridge provides:
//! - Rule dispatch via MORK's O(k) pattern matching
//! - Compiled rule caching (rule RHS → bytecode)
//! - Bindings management for pattern variables
//!
//! # Architecture
//!
//! ```text
//! BytecodeVM ─────► MorkBridge ─────► Environment
//!     │                 │                  │
//!     │                 ▼                  │
//!     │         CompiledRule Cache         │
//!     │                 │                  │
//!     │                 ▼                  │
//!     └──────── Execute Rule Body ◄────────┘
//! ```
//!
//! The bridge maintains a cache of compiled rule bodies. When a rule matches,
//! its RHS is compiled to bytecode (if not already cached) and executed.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use parking_lot::RwLock;
use tracing::warn;

use crate::backend::environment::MettaEnvironment;
// Disabled: pattern_match no longer needed — match_rules_native handles matching at byte level.
// use crate::backend::eval::pattern_match;
use crate::backend::models::{Bindings, MettaValue};
// Disabled: MettaValueTrait import no longer needed — MettaValue has inherent inner() method.
// use crate::backend::models::metta_value_trait::MettaValueTrait;

use super::chunk::BytecodeChunk;
use super::compiler::{compile, CompileError};

/// A compiled rule ready for bytecode execution
#[derive(Debug, Clone)]
pub struct CompiledRule {
    /// Original rule LHS (for debugging/display)
    pub lhs: MettaValue,
    /// Compiled rule RHS
    pub body: Arc<BytecodeChunk>,
    /// Variable bindings from pattern match
    pub bindings: Bindings,
}

/// Cache key for compiled rules
/// Uses the rule RHS hash since that's what we compile
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RuleCacheKey {
    /// Hash of the rule RHS
    rhs_hash: u64,
}

impl RuleCacheKey {
    fn from_rhs(rhs: &MettaValue) -> Self {
        use std::hash::{Hash, Hasher};
        use xxhash_rust::xxh3::Xxh3;
        let mut h = Xxh3::new();
        rhs.hash(&mut h);
        Self {
            rhs_hash: h.finish(),
        }
    }
}

/// Bridge between bytecode VM and MORK/Environment
///
/// Provides rule lookup and caching for efficient bytecode execution.
/// The bridge is typically created once per evaluation context and
/// shared across VM invocations.
pub struct MorkBridge {
    /// Reference to the environment for rule lookup
    env: Arc<RwLock<MettaEnvironment>>,

    /// Cache of compiled rule bodies
    /// Key: hash of rule RHS
    /// Value: compiled bytecode chunk
    rule_cache: RwLock<HashMap<RuleCacheKey, Arc<BytecodeChunk>>>,

    /// Statistics for cache hit/miss tracking (lock-free atomics)
    stats: BridgeStats,
}

impl std::fmt::Debug for MorkBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let cache_size = self.rule_cache.read().len();
        f.debug_struct("MorkBridge")
            .field("cache_size", &cache_size)
            .field("stats", &self.stats.snapshot())
            .finish()
    }
}

/// Statistics for monitoring bridge performance (lock-free atomics).
#[derive(Debug)]
pub struct BridgeStats {
    /// Number of rule lookups performed
    pub lookups: AtomicU64,
    /// Number of rules found across all lookups
    pub rules_found: AtomicU64,
    /// Number of rule cache hits
    pub cache_hits: AtomicU64,
    /// Number of rule cache misses (compilations)
    pub cache_misses: AtomicU64,
}

impl Default for BridgeStats {
    fn default() -> Self {
        Self {
            lookups: AtomicU64::new(0),
            rules_found: AtomicU64::new(0),
            cache_hits: AtomicU64::new(0),
            cache_misses: AtomicU64::new(0),
        }
    }
}

impl Clone for BridgeStats {
    fn clone(&self) -> Self {
        Self {
            lookups: AtomicU64::new(self.lookups.load(Ordering::Relaxed)),
            rules_found: AtomicU64::new(self.rules_found.load(Ordering::Relaxed)),
            cache_hits: AtomicU64::new(self.cache_hits.load(Ordering::Relaxed)),
            cache_misses: AtomicU64::new(self.cache_misses.load(Ordering::Relaxed)),
        }
    }
}

/// Snapshot of bridge statistics for reporting (plain u64 values).
#[derive(Debug, Clone, Default)]
pub struct BridgeStatsSnapshot {
    /// Number of rule lookups performed
    pub lookups: u64,
    /// Number of rules found across all lookups
    pub rules_found: u64,
    /// Number of rule cache hits
    pub cache_hits: u64,
    /// Number of rule cache misses (compilations)
    pub cache_misses: u64,
}

impl BridgeStats {
    /// Create a snapshot of current statistics.
    pub fn snapshot(&self) -> BridgeStatsSnapshot {
        BridgeStatsSnapshot {
            lookups: self.lookups.load(Ordering::Relaxed),
            rules_found: self.rules_found.load(Ordering::Relaxed),
            cache_hits: self.cache_hits.load(Ordering::Relaxed),
            cache_misses: self.cache_misses.load(Ordering::Relaxed),
        }
    }
}

impl MorkBridge {
    /// Create a new bridge with the given environment
    pub fn new(env: Arc<RwLock<MettaEnvironment>>) -> Self {
        Self {
            env,
            rule_cache: RwLock::new(HashMap::new()),
            stats: BridgeStats::default(),
        }
    }

    /// Create a bridge from an owned environment
    pub fn from_env(env: MettaEnvironment) -> Self {
        Self::new(Arc::new(RwLock::new(env)))
    }

    /// Get the underlying environment
    pub fn environment(&self) -> Arc<RwLock<MettaEnvironment>> {
        Arc::clone(&self.env)
    }

    /// Find all matching rules for an expression
    ///
    /// Returns compiled rules ready for bytecode execution.
    /// Rule bodies are compiled on first access and cached.
    ///
    /// # Arguments
    /// * `expr` - The expression to match against rule LHS patterns
    ///
    /// # Returns
    /// Vector of (compiled_rule_body, bindings) pairs for all matching rules
    pub fn dispatch_rules(&self, expr: &MettaValue) -> Vec<CompiledRule> {
        // Update stats (lock-free)
        self.stats.lookups.fetch_add(1, Ordering::Relaxed);

        // Get matching rules from environment
        let env = self.env.read();
        let matches = self.find_matching_rules(expr, &env);

        // Update stats with match count (lock-free)
        self.stats
            .rules_found
            .fetch_add(matches.len() as u64, Ordering::Relaxed);

        // Compile rule bodies (with caching)
        let mut compiled = Vec::with_capacity(matches.len());
        for (lhs, rhs, bindings) in matches {
            match self.get_or_compile_rule(&rhs) {
                Ok(body) => {
                    compiled.push(CompiledRule {
                        lhs,
                        body,
                        bindings,
                    });
                }
                Err(e) => {
                    // Log compilation error but continue with other rules
                    warn!(target: "mettatron::vm::mork", error = %e, "Failed to compile rule body");
                }
            }
        }

        compiled
    }

    /// Find matching rules using native byte-level matching via RuleIndex + extract_data.
    fn find_matching_rules(
        &self,
        expr: &MettaValue,
        env: &MettaEnvironment,
    ) -> Vec<(MettaValue, MettaValue, Bindings)> {
        use crate::backend::models::{GcFactory, GenericBindings};

        // Pass a no-op closure instead of apply_bindings_generic because we only use
        // rhs_template + bindings — the instantiated_rhs field is discarded. This avoids
        // a redundant recursive S-expression traversal + allocation per matching rule.
        let results = env.match_rules_native(
            expr,
            |v: &MettaValue, _: &GenericBindings<MettaValue>, _: &GcFactory| v.clone(),
        );

        results
            .into_iter()
            .map(|r| {
                // Convert GenericBindings<MettaValue> → Bindings (SmartBindings)
                let mut bindings = Bindings::new();
                for (name, value) in r.bindings.iter() {
                    bindings.insert(name.to_string(), value.clone());
                }
                // lhs = rhs_template (for CompiledRule.lhs debugging field)
                // rhs = rhs_template (for get_or_compile_rule caching — original var names → stable hash)
                (r.rhs_template.clone(), r.rhs_template, bindings)
            })
            .collect()
    }

    /// Get a compiled rule body from cache, or compile it
    fn get_or_compile_rule(&self, rhs: &MettaValue) -> Result<Arc<BytecodeChunk>, CompileError> {
        let key = RuleCacheKey::from_rhs(rhs);

        // Check cache first
        {
            let cache = self.rule_cache.read();
            if let Some(chunk) = cache.get(&key) {
                self.stats.cache_hits.fetch_add(1, Ordering::Relaxed);
                return Ok(Arc::clone(chunk));
            }
        }

        // Cache miss - compile the rule body
        let chunk = compile("rule_body", rhs)?;
        let chunk = Arc::new(chunk);

        // Store in cache
        {
            let mut cache = self.rule_cache.write();
            cache.insert(key, Arc::clone(&chunk));
            self.stats.cache_misses.fetch_add(1, Ordering::Relaxed);
        }

        Ok(chunk)
    }

    /// Get bridge statistics as a snapshot (lock-free)
    pub fn stats(&self) -> BridgeStatsSnapshot {
        self.stats.snapshot()
    }

    /// Clear the rule cache
    pub fn clear_cache(&self) {
        self.rule_cache.write().clear();
    }

    /// Get the number of cached rules
    pub fn cache_size(&self) -> usize {
        self.rule_cache.read().len()
    }
}

// Disabled: get_head_symbol is no longer needed here because
// get_matching_rules_for_expr handles head symbol extraction internally.
// fn get_head_symbol(expr: &MettaValue) -> Option<&str> {
//     match expr.inner() {
//         MettaValueInner::SExpr(items) if !items.is_empty() => match items[0].inner() {
//             MettaValueInner::Atom(name) => Some(*name),
//             _ => None,
//         },
//         MettaValueInner::Atom(name) => Some(*name),
//         _ => None,
//     }
// }

#[cfg(test)]
mod tests {
    use super::*;
    // Rule type removed — rules are (lhs, rhs) tuples stored as (= lhs rhs) in PathMap

    #[test]
    fn test_bridge_creation() {
        let env = MettaEnvironment::default();
        let bridge = MorkBridge::from_env(env);
        assert_eq!(bridge.cache_size(), 0);
    }

    #[test]
    fn test_dispatch_no_rules() {
        let env = MettaEnvironment::default();
        let bridge = MorkBridge::from_env(env);

        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("unknown".to_string()),
            MettaValue::Long(42),
        ]);

        let rules = bridge.dispatch_rules(&expr);
        assert!(rules.is_empty());
    }

    #[test]
    fn test_dispatch_with_rule() {
        let mut env = MettaEnvironment::default();

        // Add rule: (= (double $x) (+ $x $x))
        env.add_rule(
            MettaValue::SExpr(vec![
                MettaValue::Atom("double".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
        );

        let bridge = MorkBridge::from_env(env);

        // Dispatch for (double 5)
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("double".to_string()),
            MettaValue::Long(5),
        ]);

        let rules = bridge.dispatch_rules(&expr);
        assert_eq!(rules.len(), 1);

        // Check bindings - pattern_match keeps the $ prefix in variable names
        let compiled = &rules[0];
        assert!(compiled
            .bindings
            .iter()
            .any(|(name, val)| { name == "$x" && *val == MettaValue::Long(5) }));
    }

    #[test]
    fn test_rule_caching() {
        let mut env = MettaEnvironment::default();

        // Add rule
        env.add_rule(
            MettaValue::SExpr(vec![
                MettaValue::Atom("inc".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(1),
            ]),
        );

        let bridge = MorkBridge::from_env(env);

        // First dispatch - cache miss
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("inc".to_string()),
            MettaValue::Long(5),
        ]);
        let _ = bridge.dispatch_rules(&expr);

        let stats1 = bridge.stats();
        assert_eq!(stats1.cache_misses, 1);
        assert_eq!(stats1.cache_hits, 0);

        // Second dispatch - cache hit
        let expr2 = MettaValue::SExpr(vec![
            MettaValue::Atom("inc".to_string()),
            MettaValue::Long(10),
        ]);
        let _ = bridge.dispatch_rules(&expr2);

        let stats2 = bridge.stats();
        assert_eq!(stats2.cache_misses, 1);
        assert_eq!(stats2.cache_hits, 1);
    }

}
