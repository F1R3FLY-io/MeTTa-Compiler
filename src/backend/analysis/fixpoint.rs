//! Fixed-Point Reachable State Computation
//!
//! Implements the worklist algorithm for computing the fixed-point of the
//! abstract SECK transition function. Iterates until no new states or store
//! changes are discovered (convergence) or a configurable bound is reached.

use std::collections::{HashMap, HashSet, VecDeque};

use smallvec::SmallVec;

use crate::backend::models::{MettaValue, MettaValueTrait};

use super::abstract_domain::*;
use super::abstract_transition::*;

// ============================================================================
// Per-Expression Analysis Facts
// ============================================================================

/// Facts collected about a specific expression during analysis.
#[derive(Debug, Clone, Default)]
pub struct ExprFact {
    /// Rule indices reachable for this expression.
    pub reachable_rules: SmallVec<[u32; 4]>,
    /// Abstract result types observed.
    pub result_types: HashSet<AbstractType>,
    /// Whether the expression is provably pure.
    pub is_pure: Option<bool>,
    /// Whether evaluation is deterministic (exactly 1 rule).
    pub is_deterministic: Option<bool>,
    /// Whether the expression is always ground.
    pub is_ground: Option<bool>,
    /// Number of times this expression was visited.
    pub visit_count: u32,
}

// ============================================================================
// Analysis Result
// ============================================================================

/// Complete results of the fixed-point analysis.
#[derive(Debug)]
pub struct AnalysisResult {
    /// All reachable abstract states discovered.
    pub reachable_states: HashSet<AbstractState>,
    /// The final abstract store after convergence.
    pub store: AbstractStore,
    /// Per-expression facts keyed by expression content hash.
    pub expr_facts: HashMap<u64, ExprFact>,
    /// Number of fixed-point iterations performed.
    pub iterations: u32,
    /// Whether the analysis converged (vs hit max iterations).
    pub converged: bool,
    /// Wall-clock time of the analysis in milliseconds.
    pub analysis_time_ms: u64,
}

impl AnalysisResult {
    /// Get the ExprFact for a specific expression, if analyzed.
    pub fn get_fact(&self, expr_hash: u64) -> Option<&ExprFact> {
        self.expr_facts.get(&expr_hash)
    }

    /// Check if a specific rule is reachable from any expression.
    pub fn is_rule_reachable(&self, rule_index: u32) -> bool {
        self.expr_facts
            .values()
            .any(|fact| fact.reachable_rules.contains(&rule_index))
    }

    /// Get all expression hashes that were analyzed.
    pub fn analyzed_expressions(&self) -> Vec<u64> {
        self.expr_facts.keys().copied().collect()
    }
}

// ============================================================================
// Worklist
// ============================================================================

struct Worklist {
    queue: VecDeque<AbstractState>,
    seen: HashSet<AbstractState>,
}

impl Worklist {
    fn new() -> Self {
        Self {
            queue: VecDeque::with_capacity(256),
            seen: HashSet::with_capacity(256),
        }
    }

    fn push(&mut self, state: AbstractState) -> bool {
        if self.seen.insert(state.clone()) {
            self.queue.push_back(state);
            true
        } else {
            false
        }
    }

    fn pop(&mut self) -> Option<AbstractState> {
        self.queue.pop_front()
    }

    fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    fn seen_count(&self) -> usize {
        self.seen.len()
    }
}

// ============================================================================
// Main Entry Point
// ============================================================================

/// Build an EnvironmentSnapshot from the concrete environment.
///
/// I-11: Reads the rule index (read-only) and constructs AbstractRules with
/// purity analysis for each rule's RHS.
pub fn snapshot_environment(
    env: &crate::backend::environment::MettaEnvironment,
) -> EnvironmentSnapshot {
    use crate::backend::eval::cesk::branch_analysis::analyze_branch_purity;

    let rule_index = env.shared.rule_index.read();
    let mut rules: HashMap<(&'static str, usize), Vec<AbstractRule>> = HashMap::new();
    let mut total_rules: u32 = 0;

    for entry in rule_index.get_all_rules() {
        // Determine head and arity from LHS
        let (head, arity) = if let Some(items) = entry.lhs.as_sexpr() {
            let head = items.first().and_then(|h| h.as_atom()).unwrap_or("");
            (head, items.len())
        } else if let Some(atom) = entry.lhs.as_atom() {
            (atom, 0)
        } else {
            continue; // Skip non-indexable rules
        };

        let purity = analyze_branch_purity(&entry.rhs);
        let rhs_type = entry
            .rhs
            .as_sexpr()
            .and_then(|items| items.first())
            .and_then(|h| h.as_atom())
            .and_then(|name| match name {
                "+" | "-" | "*" | "/" | "%" | "abs" | "pow" => Some(AbstractType::Long),
                "<" | "<=" | ">" | ">=" | "==" | "!=" | "and" | "or" | "not" => {
                    Some(AbstractType::Bool)
                }
                _ => None,
            });

        let abstract_rule = AbstractRule {
            lhs_hash: entry.lhs.hash_value(),
            rhs_hash: entry.rhs.hash_value(),
            lhs: entry.lhs.clone(),
            rhs: entry.rhs.clone(),
            rhs_type,
            rhs_has_variables: entry.rhs_has_variables,
            purity,
            rule_index: total_rules,
        };

        rules.entry((head, arity)).or_default().push(abstract_rule);
        total_rules += 1;
    }

    EnvironmentSnapshot { rules, total_rules }
}

/// Run the fixed-point analysis from initial expressions.
///
/// This is the main AAM entry point. It:
/// 1. Creates initial abstract states from top-level expressions
/// 2. Iterates the worklist until convergence or max iterations
/// 3. Collects per-expression facts
pub fn run_analysis(
    initial_exprs: &[MettaValue],
    env: &crate::backend::environment::MettaEnvironment,
    config: &AnalysisConfig,
) -> AnalysisResult {
    let start = std::time::Instant::now();
    let env_snapshot = snapshot_environment(env);

    let mut worklist = Worklist::new();
    let mut store = AbstractStore::new();
    let mut expr_facts: HashMap<u64, ExprFact> = HashMap::new();
    let mut iterations: u32 = 0;

    // Initialize: create abstract state for each top-level expression
    for expr in initial_exprs {
        let hash = expr.hash_value();

        // Inject concrete value into abstract store
        let abstract_val = alpha(expr);
        store.alloc(AbstractAddr::mono(hash), abstract_val);

        // Create initial eval state
        let state = AbstractState {
            control: AbstractControl::Eval {
                expr_hash: hash,
                env: AbstractEnv::new(),
                depth: 0,
            },
            kont: AbstractKont::Done,
        };
        worklist.push(state);

        // If it's an S-expression with a known head, also create Apply states
        if let Some(items) = expr.as_sexpr() {
            if let Some(head) = items.first().and_then(|h| h.as_atom()) {
                let candidates = env_snapshot.get_candidates(head, items.len());
                if !candidates.is_empty() {
                    let indices: SmallVec<[u32; 8]> =
                        candidates.iter().map(|r| r.rule_index).collect();

                    // Record reachable rules for this expression
                    let fact = expr_facts.entry(hash).or_default();
                    fact.reachable_rules = indices.iter().copied().collect();
                    fact.is_deterministic = Some(indices.len() == 1);
                    fact.is_pure = Some(candidates.iter().all(|r| {
                        r.purity == crate::backend::eval::cesk::branch_analysis::BranchPurity::Pure
                    }));
                    fact.visit_count += 1;

                    // Schedule RHS bodies
                    for rule in candidates {
                        let rhs_state = AbstractState {
                            control: AbstractControl::Eval {
                                expr_hash: rule.rhs_hash,
                                env: AbstractEnv::new(),
                                depth: 1,
                            },
                            kont: AbstractKont::Done,
                        };
                        worklist.push(rhs_state);

                        if let Some(t) = rule.rhs_type {
                            let fact = expr_facts.entry(hash).or_default();
                            fact.result_types.insert(t);
                        }
                    }
                }
            }
        }
    }

    // Fixed-point iteration
    while let Some(state) = worklist.pop() {
        if iterations >= config.max_iterations || worklist.seen_count() >= config.max_states {
            break;
        }
        iterations += 1;

        let (successors, _store_changed) = abstract_step(&state, &mut store, &env_snapshot, config);

        for successor in successors {
            // Update expression facts
            if let AbstractControl::Eval { expr_hash, .. } = &successor.control {
                let fact = expr_facts.entry(*expr_hash).or_default();
                fact.visit_count += 1;
            }

            worklist.push(successor);
        }
    }

    let elapsed = start.elapsed();

    let converged = worklist.is_empty();
    AnalysisResult {
        reachable_states: worklist.seen,
        store,
        expr_facts,
        iterations,
        converged,
        analysis_time_ms: elapsed.as_millis() as u64,
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::eval::trampoline::StaticEvalContext;
    use crate::backend::models::{global_factory, MettaValueFactory};

    fn f() -> crate::backend::models::GcFactory {
        global_factory()
    }

    #[test]
    fn test_analysis_empty_program() {
        let env = StaticEvalContext::new_env();
        let config = AnalysisConfig::default();
        let result = run_analysis(&[], &env, &config);

        assert!(result.converged);
        assert_eq!(result.iterations, 0);
        assert!(result.expr_facts.is_empty());
    }

    #[test]
    fn test_analysis_ground_value() {
        let env = StaticEvalContext::new_env();
        let config = AnalysisConfig::default();
        let exprs = vec![f().long(42)];
        let result = run_analysis(&exprs, &env, &config);

        assert!(result.converged);
        // The ground value was injected into the abstract store
        let hash = exprs[0].hash_value();
        let addr = AbstractAddr::mono(hash);
        let val_set = result.store.lookup(&addr).expect("should be in store");
        assert!(val_set.is_singleton());
    }

    #[test]
    fn test_analysis_converges() {
        let env = StaticEvalContext::new_env();
        let config = AnalysisConfig {
            max_iterations: 100,
            ..Default::default()
        };
        let exprs = vec![f().sexpr(vec![f().atom("+"), f().long(1), f().long(2)])];
        let result = run_analysis(&exprs, &env, &config);
        assert!(result.converged);
    }

    #[test]
    fn test_analysis_with_rules() {
        let mut env = StaticEvalContext::new_env();
        let factory = global_factory();

        // Add rule: (= (double $x) (+ $x $x))
        let lhs = factory.sexpr(vec![factory.atom("double"), factory.atom("$x")]);
        let rhs = factory.sexpr(vec![
            factory.atom("+"),
            factory.atom("$x"),
            factory.atom("$x"),
        ]);
        env.add_rule(lhs, rhs);

        let config = AnalysisConfig::default();
        let exprs = vec![factory.sexpr(vec![factory.atom("double"), factory.long(5)])];
        let result = run_analysis(&exprs, &env, &config);

        // Should find the rule for "double" with arity 2
        let hash = exprs[0].hash_value();
        if let Some(fact) = result.get_fact(hash) {
            assert!(
                !fact.reachable_rules.is_empty(),
                "Should find reachable rules"
            );
            assert_eq!(
                fact.is_deterministic,
                Some(true),
                "Single rule = deterministic"
            );
        }
    }

    #[test]
    fn test_analysis_result_helpers() {
        let result = AnalysisResult {
            reachable_states: HashSet::new(),
            store: AbstractStore::new(),
            expr_facts: {
                let mut m = HashMap::new();
                let mut fact = ExprFact::default();
                fact.reachable_rules = SmallVec::from_slice(&[0, 1]);
                m.insert(42, fact);
                m
            },
            iterations: 5,
            converged: true,
            analysis_time_ms: 10,
        };

        assert!(result.is_rule_reachable(0));
        assert!(result.is_rule_reachable(1));
        assert!(!result.is_rule_reachable(2));
        assert_eq!(result.analyzed_expressions(), vec![42]);
    }
}
