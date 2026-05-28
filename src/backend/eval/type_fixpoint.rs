//! Iterative fixpoint type inference for mutually recursive functions (Phase 10.5).
//!
//! When rule return types depend on other user-defined functions (whose types
//! were also just inferred), this module iterates until types stabilize.
//!
//! ## Algorithm
//!
//! 1. Build dependency graph: for each function f, find all user-defined functions
//!    referenced in f's RHS.
//! 2. Topological sort with SCC detection via Tarjan's algorithm.
//! 3. Process SCCs in reverse topological order (leaves first):
//!    - Singletons: re-infer once using current inferred_fn_types.
//!    - Multi-node SCCs (mutual recursion): iterate until fixpoint or state cycle detected.
//! 4. Update inferred_fn_types with converged types.
//!
//! ## Design Principles
//!
//! - All inference is conservative: unknown types → `%Undefined%` (no false rejections).
//! - State-based cycle detection prevents divergence in non-monotonic type lattices.
//! - Only re-infers functions without explicit `(: f (-> ...))` type declarations.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use crate::backend::environment::GenericEnvironment;
use crate::backend::eval::types::{infer_arrow_type_from_rule, infer_type_generic};
use crate::backend::models::{MettaValueFactory, MettaValueTrait};

/// Run iterative fixpoint type inference over all rules in the environment.
///
/// This is the main entry point for Phase 10.5. It should be called after
/// all rules have been loaded (e.g., at the end of `compile()`).
///
/// The algorithm:
/// 1. Collects all user-defined function heads from the rule index.
/// 2. Builds a dependency graph: f → {functions referenced in f's RHS}.
/// 3. Runs Tarjan's SCC algorithm to find strongly connected components.
/// 4. Processes SCCs in topological order, re-inferring types until fixpoint.
pub fn run_type_fixpoint<V, F>(env: &GenericEnvironment<V, F>)
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    let factory = env.factory().clone();

    // 1. Collect all function heads that have rules (from rule_index)
    let rule_index = env.shared.rule_index.read();
    let mut fn_heads: HashSet<String> = HashSet::new();
    let mut fn_rules: HashMap<String, Vec<(V, V)>> = HashMap::new(); // head → [(lhs, rhs)]

    for entry in rule_index.get_all_rules() {
        if let Some(head) = entry.lhs.get_head_symbol() {
            let head_str = head.to_string();
            fn_heads.insert(head_str.clone());
            fn_rules
                .entry(head_str)
                .or_default()
                .push((entry.lhs.clone(), entry.rhs.clone()));
        }
    }
    drop(rule_index);

    if fn_heads.is_empty() {
        return;
    }

    // 2. Build dependency graph: for each function, find user-defined functions in RHS
    let mut deps: HashMap<String, Vec<String>> = HashMap::new();
    for (head, rules) in &fn_rules {
        let mut head_deps = Vec::new();
        for (_lhs, rhs) in rules {
            collect_fn_references(rhs, &fn_heads, &mut head_deps);
        }
        head_deps.sort_unstable();
        head_deps.dedup();
        deps.insert(head.clone(), head_deps);
    }

    // 3. Tarjan's SCC algorithm
    let fn_list: Vec<String> = fn_heads.into_iter().collect();
    let sccs = tarjan_scc(&fn_list, &deps);

    // 4. Process SCCs in topological order (Tarjan returns reverse topological)
    for scc in sccs.iter().rev() {
        // Skip functions with explicit arrow type declarations
        let scc_filtered: Vec<&String> = scc
            .iter()
            .filter(|head| {
                !env.get_types_generic(head).iter().any(|t| {
                    t.as_sexpr()
                        .and_then(|items| items.first().and_then(|v| v.as_atom()))
                        == Some("->")
                })
            })
            .collect();

        if scc_filtered.is_empty() {
            continue;
        }

        if scc_filtered.len() == 1 {
            // Singleton: re-infer once
            let head = scc_filtered[0];
            if let Some(rules) = fn_rules.get(head) {
                re_infer_function(head, rules, &factory, env);
            }
        } else {
            // Multi-node SCC: iterate until fixpoint or state cycle detected.
            //
            // State-based cycle detection: after each iteration, snapshot the
            // inferred type state for all functions in this SCC. If the snapshot
            // matches a previously seen state, we've hit a cycle (oscillation)
            // and further iteration cannot produce new types.
            //
            // Under the current monotonic lattice (types only added, never
            // removed), the `changed` flag alone guarantees termination. The
            // cycle detection serves as a safety net for future non-monotonic
            // extensions and provides diagnostic value.
            let mut seen_states: HashSet<u64> = HashSet::new();
            // Record initial state before first iteration
            seen_states.insert(snapshot_scc_state(&scc_filtered, env));

            let mut changed = true;
            while changed {
                changed = false;

                for head in &scc_filtered {
                    if let Some(rules) = fn_rules.get(*head) {
                        if re_infer_function(head, rules, &factory, env) {
                            changed = true;
                        }
                    }
                }

                if changed {
                    // Snapshot post-iteration state
                    let state_hash = snapshot_scc_state(&scc_filtered, env);
                    if !seen_states.insert(state_hash) {
                        // State cycle detected — this exact type configuration
                        // was seen before. Further iteration will repeat the
                        // same cycle. Break with current types.
                        break;
                    }
                }
            }
        }
    }
}

/// Re-infer the return type and arrow type for a single function.
///
/// Returns `true` if the inferred type changed (for fixpoint detection).
fn re_infer_function<V, F>(
    head: &str,
    rules: &[(V, V)],
    factory: &F,
    env: &GenericEnvironment<V, F>,
) -> bool
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    let mut changed = false;

    for (lhs, rhs) in rules {
        // Re-infer RHS type using current inferred_fn_types state
        let new_rhs_type = infer_type_generic(rhs, factory, env);
        let rhs_type_opt = if new_rhs_type.as_atom() != Some("%Undefined%") {
            Some(new_rhs_type)
        } else {
            None
        };

        // Register updated rhs_type.
        // PLN-fix 2026-04: gate against freshened-var leakage (same as
        // add_rule's phase-10.1 registration). Rule RHSes here are already
        // freshened at load time; naive inference over them may produce a
        // type containing `$__fr_*` atoms, which would poison the global
        // type registry and break type-directed dispatch downstream.
        if let Some(ref rt) = rhs_type_opt {
            let old_types = env.get_inferred_fn_types(head);
            if !old_types.contains(rt)
                && !crate::backend::environment::rule_management::type_contains_freshened_var(rt)
            {
                env.register_inferred_type(head, rt);
                changed = true;
            }
        }

        // Re-synthesize arrow type (PLN-fix: same gate).
        if let Some(arrow) =
            infer_arrow_type_from_rule(lhs, rhs, rhs_type_opt.as_ref(), factory, env)
        {
            let old_types = env.get_inferred_fn_types(head);
            if !old_types.contains(&arrow)
                && !crate::backend::environment::rule_management::type_contains_freshened_var(
                    &arrow,
                )
            {
                env.register_inferred_type(head, &arrow);
                changed = true;
            }
        }
    }

    changed
}

/// Collect references to user-defined functions in an expression.
///
/// DFS over the expression tree, collecting atom names that are known
/// function heads (i.e., have rules defined).
fn collect_fn_references<V: MettaValueTrait>(
    expr: &V,
    known_fns: &HashSet<String>,
    result: &mut Vec<String>,
) {
    if let Some(items) = expr.as_sexpr() {
        if let Some(head) = items.first().and_then(|v| v.as_atom()) {
            if known_fns.contains(head) {
                result.push(head.to_string());
            }
        }
        // Recurse into all children
        for item in items {
            collect_fn_references(item, known_fns, result);
        }
    }
}

/// Snapshot the inferred type state for all functions in an SCC.
///
/// Produces a compact `u64` fingerprint by hashing each function's name and
/// its current inferred type set (in deterministic sorted-by-name order).
/// Used for state-based cycle detection: if two iterations produce the same
/// fingerprint, the type lattice has entered a cycle and further iteration
/// is fruitless.
fn snapshot_scc_state<V, F>(scc: &[&String], env: &GenericEnvironment<V, F>) -> u64
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    use std::collections::hash_map::DefaultHasher;

    let mut hasher = DefaultHasher::new();

    // Process functions in deterministic order (sorted by name)
    let mut sorted_heads: Vec<&String> = scc.to_vec();
    sorted_heads.sort();

    for head in &sorted_heads {
        head.hash(&mut hasher);
        let types = env.get_inferred_fn_types(head);
        // Hash type count + each type's debug representation
        types.len().hash(&mut hasher);
        for t in &types {
            format!("{:?}", t).hash(&mut hasher);
        }
    }

    hasher.finish()
}

/// Tarjan's strongly connected components algorithm.
///
/// Returns SCCs in reverse topological order (leaves last — the natural
/// output order of Tarjan's). The caller reverses for processing order.
fn tarjan_scc(nodes: &[String], edges: &HashMap<String, Vec<String>>) -> Vec<Vec<String>> {
    let mut state = TarjanState {
        index_counter: 0,
        stack: Vec::new(),
        on_stack: HashSet::new(),
        indices: HashMap::new(),
        lowlinks: HashMap::new(),
        sccs: Vec::new(),
    };

    for node in nodes {
        if !state.indices.contains_key(node) {
            strongconnect(node, edges, &mut state);
        }
    }

    state.sccs
}

struct TarjanState {
    index_counter: usize,
    stack: Vec<String>,
    on_stack: HashSet<String>,
    indices: HashMap<String, usize>,
    lowlinks: HashMap<String, usize>,
    sccs: Vec<Vec<String>>,
}

fn strongconnect(v: &str, edges: &HashMap<String, Vec<String>>, state: &mut TarjanState) {
    let v_index = state.index_counter;
    state.index_counter += 1;
    state.indices.insert(v.to_string(), v_index);
    state.lowlinks.insert(v.to_string(), v_index);
    state.stack.push(v.to_string());
    state.on_stack.insert(v.to_string());

    // Consider successors of v
    if let Some(successors) = edges.get(v) {
        for w in successors {
            if !state.indices.contains_key(w) {
                // Successor w not yet visited
                strongconnect(w, edges, state);
                let w_lowlink = state.lowlinks[w];
                let v_lowlink = state.lowlinks.get_mut(v).expect("v should have lowlink");
                *v_lowlink = (*v_lowlink).min(w_lowlink);
            } else if state.on_stack.contains(w.as_str()) {
                // Successor w is on stack and hence in the current SCC
                let w_index = state.indices[w];
                let v_lowlink = state.lowlinks.get_mut(v).expect("v should have lowlink");
                *v_lowlink = (*v_lowlink).min(w_index);
            }
        }
    }

    // If v is a root node, pop the SCC
    if state.lowlinks[v] == state.indices[v] {
        let mut scc = Vec::new();
        loop {
            let w = state.stack.pop().expect("stack should not be empty");
            state.on_stack.remove(&w);
            scc.push(w.clone());
            if w == v {
                break;
            }
        }
        state.sccs.push(scc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tarjan_scc_simple_chain() {
        // A → B → C (no cycles)
        let nodes = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let mut edges = HashMap::new();
        edges.insert("A".to_string(), vec!["B".to_string()]);
        edges.insert("B".to_string(), vec!["C".to_string()]);

        let sccs = tarjan_scc(&nodes, &edges);
        // Each node is its own SCC (no cycles)
        assert_eq!(sccs.len(), 3);
        for scc in &sccs {
            assert_eq!(scc.len(), 1);
        }
    }

    #[test]
    fn test_tarjan_scc_mutual_recursion() {
        // A → B → A (mutual recursion)
        let nodes = vec!["A".to_string(), "B".to_string()];
        let mut edges = HashMap::new();
        edges.insert("A".to_string(), vec!["B".to_string()]);
        edges.insert("B".to_string(), vec!["A".to_string()]);

        let sccs = tarjan_scc(&nodes, &edges);
        // One SCC containing both A and B
        assert_eq!(sccs.len(), 1);
        assert_eq!(sccs[0].len(), 2);
        assert!(sccs[0].contains(&"A".to_string()));
        assert!(sccs[0].contains(&"B".to_string()));
    }

    #[test]
    fn test_tarjan_scc_mixed() {
        // A → B → C, B → A (A,B cycle; C is leaf)
        let nodes = vec!["A".to_string(), "B".to_string(), "C".to_string()];
        let mut edges = HashMap::new();
        edges.insert("A".to_string(), vec!["B".to_string()]);
        edges.insert("B".to_string(), vec!["A".to_string(), "C".to_string()]);

        let sccs = tarjan_scc(&nodes, &edges);
        // Two SCCs: {A, B} and {C}
        assert_eq!(sccs.len(), 2);
        let scc_sizes: Vec<usize> = sccs.iter().map(|s| s.len()).collect();
        assert!(scc_sizes.contains(&2)); // A, B cycle
        assert!(scc_sizes.contains(&1)); // C leaf
    }

    #[test]
    fn test_snapshot_scc_state_deterministic() {
        use crate::backend::environment::MettaEnvironment;
        // (convert) Active factory so this runs under both the slab `GcFactory`
        // and the index `IndexFactory` (GC A/B differential).
        use crate::backend::models::{active_factory, MettaValueFactory};

        let factory = active_factory();
        let env = MettaEnvironment::new(factory.clone());

        let a = "alpha".to_string();
        let b = "beta".to_string();
        let scc: Vec<&String> = vec![&a, &b];

        // Empty state: same inputs → same hash
        let h1 = snapshot_scc_state(&scc, &env);
        let h2 = snapshot_scc_state(&scc, &env);
        assert_eq!(h1, h2, "identical state must produce identical hashes");

        // Order independence: [&a, &b] vs [&b, &a] → same hash (sorted internally)
        let scc_rev: Vec<&String> = vec![&b, &a];
        let h3 = snapshot_scc_state(&scc_rev, &env);
        assert_eq!(h1, h3, "SCC order must not affect hash (sorted internally)");

        // After registering a type, the hash should change
        env.register_inferred_type("alpha", &factory.atom("Number"));
        let h4 = snapshot_scc_state(&scc, &env);
        assert_ne!(h1, h4, "different type state must produce different hash");

        // Same state again → same hash
        let h5 = snapshot_scc_state(&scc, &env);
        assert_eq!(
            h4, h5,
            "same state must produce same hash after type registration"
        );

        // Registering a type for the other function changes hash again
        env.register_inferred_type("beta", &factory.atom("Bool"));
        let h6 = snapshot_scc_state(&scc, &env);
        assert_ne!(h4, h6, "adding type to different function must change hash");
    }
}
