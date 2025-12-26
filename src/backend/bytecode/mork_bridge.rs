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
use std::sync::{Arc, RwLock};
use tracing::warn;

use crate::backend::environment::Environment;
use crate::backend::models::{Bindings, MettaValue};
use crate::backend::eval::pattern_match;

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
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        rhs.hash(&mut hasher);
        Self {
            rhs_hash: hasher.finish(),
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
    env: Arc<RwLock<Environment>>,

    /// Cache of compiled rule bodies
    /// Key: hash of rule RHS
    /// Value: compiled bytecode chunk
    rule_cache: RwLock<HashMap<RuleCacheKey, Arc<BytecodeChunk>>>,

    /// Statistics for cache hit/miss tracking
    stats: RwLock<BridgeStats>,
}

impl std::fmt::Debug for MorkBridge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let stats = self.stats.read().ok();
        let cache_size = self.rule_cache.read().map(|c| c.len()).unwrap_or(0);
        f.debug_struct("MorkBridge")
            .field("cache_size", &cache_size)
            .field("stats", &stats)
            .finish()
    }
}

/// Statistics for monitoring bridge performance
#[derive(Debug, Default, Clone)]
pub struct BridgeStats {
    /// Number of rule lookups performed
    pub lookups: u64,
    /// Number of rules found across all lookups
    pub rules_found: u64,
    /// Number of rule cache hits
    pub cache_hits: u64,
    /// Number of rule cache misses (compilations)
    pub cache_misses: u64,
}

impl MorkBridge {
    /// Create a new bridge with the given environment
    pub fn new(env: Arc<RwLock<Environment>>) -> Self {
        Self {
            env,
            rule_cache: RwLock::new(HashMap::new()),
            stats: RwLock::new(BridgeStats::default()),
        }
    }

    /// Create a bridge from an owned environment
    pub fn from_env(env: Environment) -> Self {
        Self::new(Arc::new(RwLock::new(env)))
    }

    /// Get the underlying environment
    pub fn environment(&self) -> Arc<RwLock<Environment>> {
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
        // Update stats
        {
            let mut stats = self.stats.write().expect("stats lock");
            stats.lookups += 1;
        }

        // Get matching rules from environment
        let env = self.env.read().expect("env lock");
        let matches = self.find_matching_rules(expr, &env);

        // Update stats with match count
        {
            let mut stats = self.stats.write().expect("stats lock");
            stats.rules_found += matches.len() as u64;
        }

        // Compile rule bodies (with caching)
        let mut compiled = Vec::with_capacity(matches.len());
        for (lhs, rhs, bindings) in matches {
            match self.get_or_compile_rule(&rhs) {
                Ok(body) => {
                    compiled.push(CompiledRule {
                        lhs: (*lhs).clone(),
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

    /// Find matching rules using the same logic as the tree-walker
    fn find_matching_rules(
        &self,
        expr: &MettaValue,
        env: &Environment,
    ) -> Vec<(Arc<MettaValue>, Arc<MettaValue>, Bindings)> {
        // Extract head symbol and arity for indexed lookup
        let matching_rules = if let Some(head) = get_head_symbol(expr) {
            let arity = expr.get_arity();
            env.get_matching_rules(head, arity)
        } else {
            // For expressions without head symbol, check wildcard rules
            env.get_matching_rules("", 0)
        };

        // Collect matching rules with bindings
        let mut matches: Vec<(Arc<MettaValue>, Arc<MettaValue>, Bindings, usize)> = Vec::new();
        for rule in matching_rules {
            if let Some(bindings) = pattern_match(&rule.lhs, expr) {
                let specificity = pattern_specificity(&rule.lhs);
                matches.push((Arc::new(rule.lhs.clone()), Arc::new(rule.rhs.clone()), bindings, specificity));
            }
        }

        // Find best specificity and filter
        if let Some(best_spec) = matches.iter().map(|(_, _, _, spec)| *spec).min() {
            matches
                .into_iter()
                .filter(|(_, _, _, spec)| *spec == best_spec)
                .map(|(lhs, rhs, bindings, _)| (lhs, rhs, bindings))
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Get a compiled rule body from cache, or compile it
    fn get_or_compile_rule(&self, rhs: &MettaValue) -> Result<Arc<BytecodeChunk>, CompileError> {
        let key = RuleCacheKey::from_rhs(rhs);

        // Check cache first
        {
            let cache = self.rule_cache.read().expect("cache lock");
            if let Some(chunk) = cache.get(&key) {
                let mut stats = self.stats.write().expect("stats lock");
                stats.cache_hits += 1;
                return Ok(Arc::clone(chunk));
            }
        }

        // Cache miss - compile the rule body
        let chunk = compile("rule_body", rhs)?;
        let chunk = Arc::new(chunk);

        // Store in cache
        {
            let mut cache = self.rule_cache.write().expect("cache lock");
            cache.insert(key, Arc::clone(&chunk));
            let mut stats = self.stats.write().expect("stats lock");
            stats.cache_misses += 1;
        }

        Ok(chunk)
    }

    /// Get bridge statistics
    pub fn stats(&self) -> BridgeStats {
        self.stats.read().expect("stats lock").clone()
    }

    /// Clear the rule cache
    pub fn clear_cache(&self) {
        self.rule_cache.write().expect("cache lock").clear();
    }

    /// Get the number of cached rules
    pub fn cache_size(&self) -> usize {
        self.rule_cache.read().expect("cache lock").len()
    }
}

/// Extract head symbol from an expression
fn get_head_symbol(expr: &MettaValue) -> Option<&str> {
    match expr {
        MettaValue::SExpr(items) if !items.is_empty() => {
            match &items[0] {
                MettaValue::Atom(name) => Some(name.as_str()),
                _ => None,
            }
        }
        MettaValue::Atom(name) => Some(name.as_str()),
        _ => None,
    }
}

/// Calculate pattern specificity (lower = more specific)
///
/// Specificity is determined by:
/// - Number of variables (more variables = less specific)
/// - Wildcard presence (wildcards are least specific)
fn pattern_specificity(pattern: &MettaValue) -> usize {
    match pattern {
        MettaValue::Atom(name) if name == "_" => 1000, // Wildcard - least specific
        MettaValue::Atom(name) if name.starts_with('$') => 100, // Variable
        MettaValue::Atom(_) => 0, // Concrete symbol
        MettaValue::SExpr(items) => {
            items.iter().map(pattern_specificity).sum()
        }
        MettaValue::Long(_) | MettaValue::Float(_) | MettaValue::Bool(_) | MettaValue::String(_) => 0,
        _ => 50, // Other types
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::Rule;

    #[test]
    fn test_bridge_creation() {
        let env = Environment::new();
        let bridge = MorkBridge::from_env(env);
        assert_eq!(bridge.cache_size(), 0);
    }

    #[test]
    fn test_dispatch_no_rules() {
        let env = Environment::new();
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
        let mut env = Environment::new();

        // Add rule: (= (double $x) (+ $x $x))
        let rule = Rule::new(
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
        env.add_rule(rule);

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
        assert!(compiled.bindings.iter().any(|(name, val)| {
            name == "$x" && *val == MettaValue::Long(5)
        }));
    }

    #[test]
    fn test_rule_caching() {
        let mut env = Environment::new();

        // Add rule
        let rule = Rule::new(
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
        env.add_rule(rule);

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

    #[test]
    fn test_pattern_specificity() {
        // Concrete atom - most specific
        assert_eq!(pattern_specificity(&MettaValue::Atom("foo".to_string())), 0);

        // Variable - less specific
        assert_eq!(pattern_specificity(&MettaValue::Atom("$x".to_string())), 100);

        // Wildcard - least specific
        assert_eq!(pattern_specificity(&MettaValue::Atom("_".to_string())), 1000);

        // S-expression adds up
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),  // 0
            MettaValue::Atom("$x".to_string()),   // 100
        ]);
        assert_eq!(pattern_specificity(&sexpr), 100);
    }
}
