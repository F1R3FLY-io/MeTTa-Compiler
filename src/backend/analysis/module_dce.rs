//! Module Dead-Code Elimination via AAM Reachability (Phase 5.6)
//!
//! When ALL rules in a module are dead (unreachable from any top-level
//! expression), the entire module can be removed from the environment,
//! reducing memory footprint and rule matching overhead.

use std::collections::{HashMap, HashSet};

// ============================================================================
// Module Reachability
// ============================================================================

/// Result of module-level dead-code analysis.
#[derive(Debug, Clone)]
pub struct ModuleReachability {
    /// Module name → set of rule indices belonging to that module.
    pub module_rules: HashMap<String, HashSet<u32>>,
    /// Modules where ALL rules are dead.
    pub dead_modules: HashSet<String>,
    /// Modules with some dead rules.
    pub partially_dead_modules: HashMap<String, HashSet<u32>>,
    /// Modules where ALL rules are live.
    pub live_modules: HashSet<String>,
}

impl ModuleReachability {
    /// Compute module reachability from AAM analysis and module-to-rule mapping.
    ///
    /// `module_rules` maps module names to the set of rule indices defined in
    /// that module. `dead_rules` is the set of unreachable rule indices from
    /// the AAM analysis.
    pub fn compute(module_rules: HashMap<String, HashSet<u32>>, dead_rules: &HashSet<u32>) -> Self {
        let mut dead_modules = HashSet::new();
        let mut partially_dead_modules = HashMap::new();
        let mut live_modules = HashSet::new();

        for (module, rules) in &module_rules {
            let dead_in_module: HashSet<u32> = rules.intersection(dead_rules).copied().collect();

            if dead_in_module.len() == rules.len() && !rules.is_empty() {
                // ALL rules dead → dead module
                dead_modules.insert(module.clone());
            } else if !dead_in_module.is_empty() {
                // SOME rules dead → partially dead
                partially_dead_modules.insert(module.clone(), dead_in_module);
            } else {
                // ALL rules live
                live_modules.insert(module.clone());
            }
        }

        Self {
            module_rules,
            dead_modules,
            partially_dead_modules,
            live_modules,
        }
    }

    /// Total number of rules that can be eliminated.
    pub fn total_eliminable_rules(&self) -> usize {
        let dead_module_rules: usize = self
            .dead_modules
            .iter()
            .filter_map(|m| self.module_rules.get(m))
            .map(|rules| rules.len())
            .sum();
        let partial_dead_rules: usize = self
            .partially_dead_modules
            .values()
            .map(|rules| rules.len())
            .sum();
        dead_module_rules + partial_dead_rules
    }

    /// Check if a specific module is dead.
    pub fn is_dead_module(&self, module: &str) -> bool {
        self.dead_modules.contains(module)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_live() {
        let mut module_rules = HashMap::new();
        module_rules.insert("mod_a".to_string(), vec![0, 1, 2].into_iter().collect());
        let dead_rules = HashSet::new();

        let result = ModuleReachability::compute(module_rules, &dead_rules);
        assert!(result.dead_modules.is_empty());
        assert!(result.live_modules.contains("mod_a"));
        assert_eq!(result.total_eliminable_rules(), 0);
    }

    #[test]
    fn test_all_dead() {
        let mut module_rules = HashMap::new();
        module_rules.insert("mod_a".to_string(), vec![0, 1, 2].into_iter().collect());
        let dead_rules: HashSet<u32> = vec![0, 1, 2].into_iter().collect();

        let result = ModuleReachability::compute(module_rules, &dead_rules);
        assert!(result.dead_modules.contains("mod_a"));
        assert!(result.live_modules.is_empty());
        assert!(result.is_dead_module("mod_a"));
        assert_eq!(result.total_eliminable_rules(), 3);
    }

    #[test]
    fn test_partially_dead() {
        let mut module_rules = HashMap::new();
        module_rules.insert("mod_a".to_string(), vec![0, 1, 2].into_iter().collect());
        let dead_rules: HashSet<u32> = vec![1].into_iter().collect();

        let result = ModuleReachability::compute(module_rules, &dead_rules);
        assert!(result.dead_modules.is_empty());
        assert!(result.partially_dead_modules.contains_key("mod_a"));
        assert_eq!(result.total_eliminable_rules(), 1);
    }

    #[test]
    fn test_mixed_modules() {
        let mut module_rules = HashMap::new();
        module_rules.insert("live_mod".to_string(), vec![0, 1].into_iter().collect());
        module_rules.insert("dead_mod".to_string(), vec![2, 3].into_iter().collect());
        module_rules.insert("mixed_mod".to_string(), vec![4, 5, 6].into_iter().collect());
        let dead_rules: HashSet<u32> = vec![2, 3, 5].into_iter().collect();

        let result = ModuleReachability::compute(module_rules, &dead_rules);
        assert!(result.live_modules.contains("live_mod"));
        assert!(result.dead_modules.contains("dead_mod"));
        assert!(result.partially_dead_modules.contains_key("mixed_mod"));
    }
}
