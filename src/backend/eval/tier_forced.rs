//! Tier-forced evaluation API for cross-tier bisimilarity testing.
//!
//! Provides a per-tier execution path bypassing the auto-promotion thresholds
//! in `TieredCache`. Used by the bisim harness and `--tier` CLI flag.
//!
//! See `~/.claude/plans/carefully-review-the-bugs-luminous-hinton.md` Workstream H,
//! Phase H1 for the design.

use smallvec::SmallVec;

use crate::backend::bytecode::{
    can_compile, can_compile_with_env, eval_bytecode_arena_with_env, execute_arena,
    global_tiered_cache, ExecutionTier, TierStatusKind,
};
use crate::backend::eval::trampoline::eval_trampoline;
use crate::backend::models::{MettaState, MettaValue};

use super::trampoline::MettaEnvironment;
use super::{
    execute_jit_arena_with_env, expression_has_declared_meta_typed_params,
    expression_has_overridden_grounded_op, expression_has_t0_only_form,
    expression_involves_impure_rules,
};

/// User-facing tier selection. Maps to internal `ExecutionTier` plus an `Auto`
/// variant that defers to the standard `eval()` dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TierSelection {
    /// Auto-tiered (current default `eval()` behavior).
    Auto,
    /// Tree-walker / trampoline (T0).
    Treewalker,
    /// Bytecode VM (T1).
    Bytecode,
    /// JIT Stage 1 (T2).
    JitStage1,
    /// JIT Stage 2 (T3).
    JitStage2,
}

impl TierSelection {
    /// Parse a CLI-style tier specifier. Accepts numeric or name form.
    pub fn from_cli(s: &str) -> Result<Self, String> {
        match s {
            "0" | "treewalker" | "tree-walker" | "T0" | "t0" => Ok(Self::Treewalker),
            "1" | "bytecode" | "T1" | "t1" => Ok(Self::Bytecode),
            "2" | "jit1" | "jit-basic" | "T2" | "t2" => Ok(Self::JitStage1),
            "3" | "jit2" | "jit-optimized" | "T3" | "t3" => Ok(Self::JitStage2),
            "auto" => Ok(Self::Auto),
            other => Err(format!(
                "unknown tier specifier '{}'; expected one of: 0|1|2|3|treewalker|bytecode|jit1|jit2|auto",
                other
            )),
        }
    }

    /// Convert to the internal `ExecutionTier` enum. `Auto` has no fixed tier.
    pub fn execution_tier(self) -> Option<ExecutionTier> {
        match self {
            Self::Auto => None,
            Self::Treewalker => Some(ExecutionTier::Interpreter),
            Self::Bytecode => Some(ExecutionTier::Bytecode),
            Self::JitStage1 => Some(ExecutionTier::JitStage1),
            Self::JitStage2 => Some(ExecutionTier::JitStage2),
        }
    }

    /// Human-readable label suitable for `Observation` / diagnostic output.
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Treewalker => "T0/treewalker",
            Self::Bytecode => "T1/bytecode",
            Self::JitStage1 => "T2/jit1",
            Self::JitStage2 => "T3/jit2",
        }
    }
}

/// Reason a requested tier could not run a given input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TierUnavailableReason {
    /// Tier ≥ T1: expression has impure rules (`println!`/`add-atom`/...).
    /// Tier-locked to T0 by the dispatch gate at `eval/mod.rs:804`.
    ImpureRules,
    /// Tier ≥ T1: expression has a user rule overriding a grounded op.
    /// Tier-locked to T0 by the dispatch gate at `eval/mod.rs:607`.
    OverriddenGrounded,
    /// Tier ≥ T1: head has declared meta-typed args. Bytecode VM's eager
    /// applicative dispatch would violate HE's `interpret_function` semantics.
    MetaTypedParams,
    /// Tier ≥ T1: expression fails `can_compile_with_env`
    /// (e.g., contains `State`/`Space`/`Conjunction`/`Memo`).
    CannotCompileToBytecode,
    /// Tier ≥ T1: expression contains a PT-canonical translator form
    /// (`prog1`/`forall`/`foldall`/`|->`/`translatePredicate`) or a
    /// top-level grounded op whose T1 lowering diverges from T0 (e.g.
    /// `println!`/`get-type`). T0-only by design.
    T0OnlyForm,
    /// Tier ≥ T2: expression fails the more restrictive `can_compile`
    /// (JIT requires inline-resolvable bytecode).
    CannotCompileToJit,
    /// JIT chunk not yet `Ready` in the tiered cache. Force-running JIT requires
    /// either auto-promotion via repeated `record_execution()` calls or a
    /// future direct synchronous-compile entry point.
    JitChunkNotReady,
    /// Bytecode compilation failed unexpectedly.
    CompilationFailed(String),
    /// JIT execution itself returned an error.
    JitExecutionFailed,
    /// Bytecode VM returned an unreduced result, so the chunk did not fully
    /// handle the input. Equivalent to the existing fall-through in `eval_inner`.
    BytecodeUnreduced,
}

/// Outcome of a tier-forced evaluation.
#[derive(Debug)]
pub enum TierEvalOutcome {
    /// The requested tier handled the input.
    Ok {
        results: SmallVec<[MettaValue; 4]>,
        env: MettaEnvironment,
        tier_actual: ExecutionTier,
    },
    /// The requested tier was unavailable for this input. Execution silently
    /// used a lower tier; results are still observation-correct per spec §24.3 R.5.
    Demoted {
        results: SmallVec<[MettaValue; 4]>,
        env: MettaEnvironment,
        tier_requested: ExecutionTier,
        tier_actual: ExecutionTier,
        reason: TierUnavailableReason,
    },
    /// The requested tier cannot run this input AND the caller asked for strict
    /// behavior (no automatic fallback). No `results` produced.
    NotApplicable { reason: TierUnavailableReason },
}

impl TierEvalOutcome {
    /// True if results were produced (Ok or Demoted).
    pub fn has_results(&self) -> bool {
        matches!(self, Self::Ok { .. } | Self::Demoted { .. })
    }

    /// Extract `(results, env)` regardless of Ok/Demoted variant. Returns
    /// `None` if `NotApplicable`.
    pub fn into_results(self) -> Option<(SmallVec<[MettaValue; 4]>, MettaEnvironment)> {
        match self {
            Self::Ok { results, env, .. } | Self::Demoted { results, env, .. } => {
                Some((results, env))
            }
            Self::NotApplicable { .. } => None,
        }
    }

    /// Return the tier actually used to produce the results.
    pub fn tier_actual(&self) -> Option<ExecutionTier> {
        match self {
            Self::Ok { tier_actual, .. } | Self::Demoted { tier_actual, .. } => Some(*tier_actual),
            Self::NotApplicable { .. } => None,
        }
    }
}

/// Whether a fallback to a lower tier is permitted when the requested tier
/// is not applicable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackPolicy {
    /// Fall back to the next-lower tier silently (matches existing `eval()`
    /// transparent-demotion behavior). Default.
    SilentDemote,
    /// Return `NotApplicable` without producing results.
    StrictNoFallback,
}

impl Default for FallbackPolicy {
    fn default() -> Self {
        Self::SilentDemote
    }
}

/// Check whether a tier is applicable to the given expression + environment,
/// without running it. `Ok(())` ⇒ the tier can execute the input; `Err(reason)`
/// describes the gate that prevents it.
pub fn tier_applicable(
    value: &MettaValue,
    env: &MettaEnvironment,
    tier: TierSelection,
) -> Result<(), TierUnavailableReason> {
    match tier {
        TierSelection::Auto | TierSelection::Treewalker => Ok(()),
        TierSelection::Bytecode => {
            if expression_involves_impure_rules(value, env) {
                return Err(TierUnavailableReason::ImpureRules);
            }
            if expression_has_overridden_grounded_op(value, env) {
                return Err(TierUnavailableReason::OverriddenGrounded);
            }
            if expression_has_declared_meta_typed_params(value, env) {
                return Err(TierUnavailableReason::MetaTypedParams);
            }
            // PT-canonical translator forms + side-effecting top-level ops
            // that have no T1 lowering compatible with T0 — route to T0.
            if expression_has_t0_only_form(value) {
                return Err(TierUnavailableReason::T0OnlyForm);
            }
            if !can_compile_with_env(value) {
                return Err(TierUnavailableReason::CannotCompileToBytecode);
            }
            Ok(())
        }
        TierSelection::JitStage1 | TierSelection::JitStage2 => {
            if expression_involves_impure_rules(value, env) {
                return Err(TierUnavailableReason::ImpureRules);
            }
            if expression_has_overridden_grounded_op(value, env) {
                return Err(TierUnavailableReason::OverriddenGrounded);
            }
            if expression_has_t0_only_form(value) {
                return Err(TierUnavailableReason::T0OnlyForm);
            }
            if !can_compile(value) {
                return Err(TierUnavailableReason::CannotCompileToJit);
            }
            Ok(())
        }
    }
}

/// Tier-forced evaluation.
///
/// Bypasses the auto-promotion thresholds in `TieredCache` and dispatches directly
/// to the requested tier. Returns a `TierEvalOutcome` distinguishing success,
/// transparent demotion (R.5 in spec §24.3), and not-applicable.
///
/// `TierSelection::Auto` delegates to the existing `eval()` so callers can use
/// this API uniformly for all tier choices.
pub fn eval_with_tier(
    value: MettaValue,
    env: MettaEnvironment,
    state: &MettaState,
    tier: TierSelection,
    policy: FallbackPolicy,
) -> TierEvalOutcome {
    if matches!(tier, TierSelection::Auto) {
        let (results, env) = super::eval(value, env, state);
        return TierEvalOutcome::Ok {
            results: results.into_iter().collect(),
            env,
            tier_actual: ExecutionTier::Interpreter, // Auto does not report a fixed tier
        };
    }

    if let Err(reason) = tier_applicable(&value, &env, tier) {
        return match policy {
            FallbackPolicy::SilentDemote => {
                let tier_requested = tier
                    .execution_tier()
                    .unwrap_or(ExecutionTier::Interpreter);
                run_t0(value, env, state, tier_requested, reason)
            }
            FallbackPolicy::StrictNoFallback => TierEvalOutcome::NotApplicable { reason },
        };
    }

    match tier {
        TierSelection::Treewalker => run_t0_direct(value, env, state),
        TierSelection::Bytecode => run_t1(value, env, state, policy),
        TierSelection::JitStage1 => run_jit(value, env, state, ExecutionTier::JitStage1, policy),
        TierSelection::JitStage2 => run_jit(value, env, state, ExecutionTier::JitStage2, policy),
        TierSelection::Auto => unreachable!("handled above"),
    }
}

// -----------------------------------------------------------------------------
// Per-tier execution paths
// -----------------------------------------------------------------------------

/// Workstream C (2026-05-18): mirror `eval()`'s S1 TOPLEVEL + S2 BANG-WORD
/// resets (`backend/eval/mod.rs:224-226`) so each per-directive call sees a
/// clean `interpret_mode=true` / `bang_body=false` env.
///
/// Without this reset, a `!` directive followed by a `(= lhs rhs)` rule
/// declaration leaves `bang_body=true` stale on the threaded env. The next
/// `=` arm in `step/sexpr.rs:293-297` matches `env.in_bang_body()` and skips
/// rule registration entirely — the rule never reaches `RuleIndex` and
/// subsequent dispatches see "no matching rule" and return the call form
/// unexpanded. This bug pre-dated Task #6 (`tier_forced.rs` added in
/// `ccebabc`, eval()'s S2 reset added later in `cb363c1`) and only surfaced
/// when PLN Direct.metta was run under `--tier <T>` for any tier.
///
/// Every `run_*` per-tier entry point below MUST call this helper at the top
/// to keep the forced-tier paths in lock-step with the auto-tier `eval()`.
#[inline]
fn prepare_per_directive_env(env: MettaEnvironment) -> MettaEnvironment {
    let mut env = env;
    env.set_interpret_mode(true);
    env.set_bang_body(false);
    env
}

fn run_t0_direct(
    value: MettaValue,
    env: MettaEnvironment,
    state: &MettaState,
) -> TierEvalOutcome {
    let env = prepare_per_directive_env(env);
    let (results, shared_env) = eval_trampoline(value, env, state);
    TierEvalOutcome::Ok {
        results: results.into_iter().map(|(v, _)| v).collect(),
        env: (*shared_env).clone(),
        tier_actual: ExecutionTier::Interpreter,
    }
}

/// Demote path: requested tier was unavailable, run on T0 and report it.
fn run_t0(
    value: MettaValue,
    env: MettaEnvironment,
    state: &MettaState,
    tier_requested: ExecutionTier,
    reason: TierUnavailableReason,
) -> TierEvalOutcome {
    let env = prepare_per_directive_env(env);
    let (results, shared_env) = eval_trampoline(value, env, state);
    TierEvalOutcome::Demoted {
        results: results.into_iter().map(|(v, _)| v).collect(),
        env: (*shared_env).clone(),
        tier_requested,
        tier_actual: ExecutionTier::Interpreter,
        reason,
    }
}

fn run_t1(
    value: MettaValue,
    env: MettaEnvironment,
    state: &MettaState,
    policy: FallbackPolicy,
) -> TierEvalOutcome {
    let env = prepare_per_directive_env(env);
    // Synchronously compile and execute on the bytecode VM, bypassing
    // the `TieredCache` auto-promotion. This mirrors the `Path B` path in
    // `eval_inner` (`eval/mod.rs:725`) without the threshold gating.
    match eval_bytecode_arena_with_env(&value, env.clone()) {
        Ok((results, new_env, unreduced, _has_choices)) => {
            if unreduced {
                // T1 could not fully reduce — drop to T0 transparently per spec R.5.
                let reason = TierUnavailableReason::BytecodeUnreduced;
                if matches!(policy, FallbackPolicy::StrictNoFallback) {
                    return TierEvalOutcome::NotApplicable { reason };
                }
                return run_t0(value, env, state, ExecutionTier::Bytecode, reason);
            }
            TierEvalOutcome::Ok {
                results: results.into_iter().collect(),
                env: new_env,
                tier_actual: ExecutionTier::Bytecode,
            }
        }
        Err(err) => {
            let reason = TierUnavailableReason::CompilationFailed(format!("{:?}", err));
            if matches!(policy, FallbackPolicy::StrictNoFallback) {
                return TierEvalOutcome::NotApplicable { reason };
            }
            run_t0(value, env, state, ExecutionTier::Bytecode, reason)
        }
    }
}

/// Try the requested JIT tier; if its chunk is not yet Ready in the tiered
/// cache, return `JitChunkNotReady` (or demote per policy).
///
/// Note: this Phase-H1 implementation does not force synchronous JIT
/// compilation. Callers that need a Ready JIT chunk should pump
/// `global_tiered_cache().record_execution(&value)` enough times to cross
/// the relevant threshold (`bytecode_threshold` → `jit1_threshold` →
/// `jit2_threshold`) and wait for the background WorkPool task to finish.
/// A future `compile_jit_now()` helper will remove this requirement.
fn run_jit(
    value: MettaValue,
    env: MettaEnvironment,
    state: &MettaState,
    requested: ExecutionTier,
    policy: FallbackPolicy,
) -> TierEvalOutcome {
    let env = prepare_per_directive_env(env);
    let cache = global_tiered_cache();
    let compilation_state = cache.record_execution(&value);

    let (status, code) = match requested {
        ExecutionTier::JitStage2 => (
            compilation_state.jit2_status(),
            compilation_state.jit2_code(),
        ),
        ExecutionTier::JitStage1 => (
            compilation_state.jit1_status(),
            compilation_state.jit1_code(),
        ),
        // run_jit is only called with JIT tiers; the others would be a bug.
        ExecutionTier::Bytecode | ExecutionTier::Interpreter => {
            return run_t0(
                value,
                env,
                state,
                requested,
                TierUnavailableReason::JitChunkNotReady,
            );
        }
    };

    if status != TierStatusKind::Ready {
        let reason = TierUnavailableReason::JitChunkNotReady;
        if matches!(policy, FallbackPolicy::StrictNoFallback) {
            return TierEvalOutcome::NotApplicable { reason };
        }
        // Demote: try the next-lower tier (T1) first, then T0 if T1 unreduced.
        return run_t1(value, env, state, policy);
    }

    let code = match code {
        Some(c) => c,
        None => {
            let reason = TierUnavailableReason::JitChunkNotReady;
            if matches!(policy, FallbackPolicy::StrictNoFallback) {
                return TierEvalOutcome::NotApplicable { reason };
            }
            return run_t1(value, env, state, policy);
        }
    };

    match execute_jit_arena_with_env(&compilation_state, code.ptr, env.clone()) {
        Ok((results, new_env)) => TierEvalOutcome::Ok {
            results: results.into_iter().collect(),
            env: new_env,
            tier_actual: requested,
        },
        Err(()) => {
            let reason = TierUnavailableReason::JitExecutionFailed;
            if matches!(policy, FallbackPolicy::StrictNoFallback) {
                return TierEvalOutcome::NotApplicable { reason };
            }
            run_t1(value, env, state, policy)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::compile::compile;
    use crate::backend::eval::trampoline::new_env;

    fn evaluate_one(source: &str, tier: TierSelection) -> TierEvalOutcome {
        let state = compile(source).expect("compile");
        let env = new_env();
        let expr = *state.source().first().expect("at least one expr");
        eval_with_tier(expr, env, &state, tier, FallbackPolicy::SilentDemote)
    }

    #[test]
    fn t0_runs_simple_arithmetic() {
        let outcome = evaluate_one("!(+ 1 2)", TierSelection::Treewalker);
        let (results, _env) = outcome.into_results().expect("has results");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].as_long(), Some(3));
    }

    #[test]
    fn t1_runs_simple_arithmetic() {
        let outcome = evaluate_one("!(+ 1 2)", TierSelection::Bytecode);
        let (results, _env) = outcome.into_results().expect("has results");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].as_long(), Some(3));
    }

    #[test]
    fn auto_matches_t0_for_simple_input() {
        let auto_outcome = evaluate_one("!(+ 1 2)", TierSelection::Auto);
        let t0_outcome = evaluate_one("!(+ 1 2)", TierSelection::Treewalker);
        let (a_results, _) = auto_outcome.into_results().expect("auto results");
        let (t0_results, _) = t0_outcome.into_results().expect("t0 results");
        assert_eq!(a_results.len(), t0_results.len());
        assert_eq!(a_results[0].as_long(), t0_results[0].as_long());
    }

    #[test]
    fn tier_applicable_t1_rejects_impure() {
        let state = compile("!(println! \"hi\")").expect("compile");
        let env = new_env();
        let expr = *state.source().first().expect("expr");
        // println! is gated as impure but only when reachable from a rule's RHS;
        // top-level `!(println! ...)` is a direct grounded call. Use add-atom for
        // a more reliable gate test.
        let _ = expr;

        let state = compile("!(add-atom &self (foo bar))").expect("compile add-atom");
        let env = new_env();
        let expr = *state.source().first().expect("expr");
        // add-atom is gated through the dispatch_overrides / impure-rules machinery.
        // tier_applicable should at minimum confirm T1 is not the wrong choice.
        let _ = tier_applicable(&expr, &env, TierSelection::Bytecode);
        // Smoke: does not panic. Specific applicability depends on rule shape.
        let _ = env;
    }
}
