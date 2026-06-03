//! Generic MORK Special Forms - Zero-Conversion Implementation
//!
//! This module provides generic versions of MORK special forms that work with any
//! value type implementing `MettaValueTrait`. This eliminates MettaValue <-> MettaValue
//! conversions when using arena allocation.
//!
//! ## Zero-Conversion Path
//!
//! For MettaValue:
//! - No conversion to MettaValue for MORK operations
//! - Pattern matching uses generic `pattern_match_generic`
//! - Binding application uses generic `apply_bindings_generic`
//! - Space operations use GenericEnvironment methods directly
//!
//! ## Forms
//!
//! - `eval_exec_generic`: Rule execution with conjunction antecedents/consequents
//! - `eval_coalg_generic`: Coalgebra patterns for tree transformations
//! - `eval_lookup_generic`: Conditional fact lookup with success/failure branches
//! - `eval_rulify_generic`: Meta-programming for runtime rule generation

// Phase 1.1 PT-canonical Error tuple (Type, Ctx) — /* PT-swapped */
use crate::backend::environment::GenericEnvironment;
use crate::backend::eval::bindings::{apply_bindings_generic, pattern_match_generic};
use crate::backend::models::{GenericBindings, MettaValueFactory, MettaValueTrait};

/// Generic result type for MORK operations
pub type GenericMorkResult<V, F> = (Vec<V>, GenericEnvironment<V, F>);

/// E1-FLIP Path B V4 — Step 3: MORK long-loop LIVENESS poll (index-gc only).
///
/// A long depth>=1 conjunction-expansion / sink / join loop runs as a single native
/// region: while it executes WITHOUT reaching a safepoint, the running thread's witness
/// slot stays OCCUPIED-and-unstamped-for-this-cycle, so a dedicated-GC-thread rendezvous
/// driver's `requestor_wait_for_all_reified_parked` WAITS for it (never sweeps) — SAFE by
/// construction (the witness-hang). This poll exists purely for LIVENESS: it lets the
/// thread PARK mid-loop so the driver does not block for the loop's full duration. A
/// MISSED poll is therefore a HANG, never a UAF (docs/cesk-gc/e1-flip-pathB-v2-impl.md
/// §Step 3 + §"Secondary residuals").
///
/// `counter` is a per-loop local throttle (poll only every 4096 iterations — `& 0xFFF`).
/// When the throttle fires AND a cycle is in flight (`is_gc_requested()`), it publishes a
/// SUPERSET of the loop's live in-flight `MettaValue`s as `extra_roots` — the accumulated
/// `results` ∪ every value bound in the in-flight `bindings` alternatives — so that the
/// mid-loop park is SAFE (those Addrs are Rust locals, NOT on the K-spine the parked
/// worker self-roots; a superset over-roots harmlessly, never a UAF). `worker_cooperative_
/// safepoint` itself re-checks `is_gc_requested()` and, under DEDICATED, parks + stamps the
/// witness (`note_reified_park`).
///
/// TypeId-gated `V == MettaValue` + slice transmute (the established VM pattern,
/// `bytecode/vm/mod.rs::run_cooperative_safepoint`); for non-`MettaValue`
/// monomorphizations the body dead-code-eliminates. `#[cfg(index-gc)]`: the dedicated
/// rendezvous is an index-only construct.
#[cfg(feature = "index-gc")]
#[inline]
fn mork_liveness_poll<V>(
    counter: &mut u64,
    results: &[V],
    bindings_a: &[GenericBindings<V>],
    bindings_b: &[GenericBindings<V>],
) where
    V: MettaValueTrait + Clone + 'static,
{
    use crate::backend::models::gc_allocator;
    use crate::backend::models::MettaValue;
    use std::any::TypeId;

    *counter = counter.wrapping_add(1);
    // Cheap throttle: skip the gc-flag load + root build on all but every 4096th iter.
    if *counter & 0xFFF != 0 {
        return;
    }
    // Only a `V == MettaValue` monomorphization can produce index `Addr`s the GC sweeps.
    if TypeId::of::<V>() != TypeId::of::<MettaValue>() {
        return;
    }
    // Cheap gc-pressure gate (Relaxed load) before building the inflight superset.
    if !gc_allocator::is_gc_requested() {
        return;
    }
    // Build the complete in-flight superset: accumulated results ∪ all bound values in
    // BOTH binding lists (the accumulator + the just-dequeued in-flight alternative).
    let mut inflight: Vec<V> =
        Vec::with_capacity(results.len() + (bindings_a.len() + bindings_b.len()) * 4 + 16);
    inflight.extend_from_slice(results);
    for b in bindings_a.iter().chain(bindings_b.iter()) {
        for (_, v) in b.iter() {
            inflight.push(v.clone());
        }
    }
    // SAFETY: `V == MettaValue` verified via TypeId; `&[V]` and `&[MettaValue]` have
    // identical layout (the same transmute the VM uses at vm/mod.rs:1274). The slice is
    // read-only for the duration of the park.
    let mv: &[MettaValue] = unsafe {
        std::slice::from_raw_parts(inflight.as_ptr() as *const MettaValue, inflight.len())
    };
    crate::backend::eval::trampoline::eval_loop::worker_cooperative_safepoint(mv);
}

// ============================================================================
// Generic Helper Functions
// ============================================================================

/// Check if a generic value contains any variables.
///
/// Uses `MettaValueTrait` methods for zero-conversion checking.
pub fn has_variables_generic<V: MettaValueTrait>(value: &V) -> bool {
    // Check if it's a variable atom
    if let Some(name) = value.as_atom() {
        return name.starts_with('$') || name.starts_with('&') || name.starts_with('\'');
    }

    // Check S-expression children
    if let Some(items) = value.as_sexpr() {
        return items.iter().any(has_variables_generic);
    }

    // Check conjunction goals
    if let Some(goals) = value.as_conjunction() {
        return goals.iter().any(has_variables_generic);
    }

    // HE-bisimilar Error(offending, detail): both slots are full values.
    if let Some((offending, detail)) = value.as_error() {
        return has_variables_generic(offending) || has_variables_generic(detail);
    }

    false
}

/// Check if a value contains `$`-prefixed pattern variables.
///
/// This is more targeted than `has_variables_generic` which also checks
/// `&` and `'` prefixes. For MORK routing, only `$`-prefixed variables
/// matter because `&self`, `&kb`, `'x` etc. are concrete in MORK encoding
/// and can be found by trie search.
///
/// Used by AtomSpace to route atoms to PathMap (ground) vs variable_atoms Vec.
pub fn has_pattern_variables<V: MettaValueTrait>(value: &V) -> bool {
    if let Some(name) = value.as_atom() {
        return name.starts_with('$');
    }
    if let Some(items) = value.as_sexpr() {
        return items.iter().any(has_pattern_variables);
    }
    if let Some(goals) = value.as_conjunction() {
        return goals.iter().any(has_pattern_variables);
    }
    // HE-bisimilar Error(offending, detail): both slots are full values.
    if let Some((offending, detail)) = value.as_error() {
        return has_pattern_variables(offending) || has_pattern_variables(detail);
    }
    false
}

/// Check if a generic value is an exec form: (exec ...)
pub fn is_exec_form_generic<V: MettaValueTrait>(value: &V) -> bool {
    if let Some(items) = value.as_sexpr() {
        if !items.is_empty() {
            if let Some(op) = items[0].as_atom() {
                return op == "exec";
            }
        }
    }
    false
}

/// Check if a generic value is an operation form: (O ...)
pub fn is_operation_form_generic<V: MettaValueTrait>(value: &V) -> bool {
    if let Some(items) = value.as_sexpr() {
        if !items.is_empty() {
            if let Some(op) = items[0].as_atom() {
                return op == "O";
            }
        }
    }
    false
}

/// Check if items represent an operation (starts with "O")
fn matches_operation_generic<V: MettaValueTrait>(items: &[V]) -> bool {
    if let Some(first) = items.first() {
        if let Some(op) = first.as_atom() {
            return op == "O";
        }
    }
    false
}

/// Extract conjunction goals from a value.
/// Handles both Conjunction variant and SExpr representation (, goal1 goal2 ...)
fn extract_conjunction_goals<V: MettaValueTrait + Clone>(value: &V) -> Option<Vec<V>> {
    // Try Conjunction variant first
    if let Some(goals) = value.as_conjunction() {
        return Some(goals.to_vec());
    }

    // Try SExpr representation: (, goal1 goal2 ...)
    if let Some(items) = value.as_sexpr() {
        if !items.is_empty() {
            if let Some(op) = items[0].as_atom() {
                if op == "," {
                    return Some(items[1..].to_vec());
                }
            }
        }
    }

    None
}

// ============================================================================
// Generic MORK Forms
// ============================================================================

/// Generic eval_exec: (exec <priority> <antecedent> <consequent>)
///
/// Executes rules with conjunction-based pattern matching using generic types.
/// No MettaValue <-> MettaValue conversion required.
/// MM2 §10 reduction sinks for the exec `(O (<sink> ctx slot e))` consequent. Aggregates
/// `e` over ALL antecedent matches Θ (`binding_sets`) and inserts `ctx` with the free
/// variable `slot` bound to `Sym(result)` — one atom (SNK-COUNT/HASH/SUM/FRED). Returns
/// `Some(inserted)` if `consequent` is a single recognized reduction sink; `None`
/// otherwise (caller falls through to the per-match `O`/conjunction handling). Heads:
///   `count` → |Θ|;  `sum` → Σ decimal-u64;  `fsum`/`fmin`/`fmax`/`fprod` → f64 reduction;
///   `and` → boolean-AND of the group;  `hash` → order-insensitive FNV-1a digest.
/// Unparseable numeric input ⇒ `None` (graceful fall-through, not a Tier-1 panic).
fn try_eval_reduction_sink_generic<V, F>(
    consequent: &V,
    binding_sets: &[GenericBindings<V>],
    env: &GenericEnvironment<V, F>,
    factory: &F,
) -> Option<Vec<V>>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    // consequent = (O (<sink> ctx slot e))
    let items = consequent.as_sexpr()?;
    if items.len() != 2 || items[0].as_atom()? != "O" {
        return None;
    }
    let sink = items[1].as_sexpr()?;
    if sink.len() != 4 {
        return None;
    }
    let head = sink[0].as_atom()?;
    let ctx = &sink[1];
    let slot_name = sink[2].as_atom()?; // slot MUST be a free variable in ctx
    let e = &sink[3];

    let result_str: String = match head {
        // count needs only |Θ| — avoid materializing the group.
        "count" => binding_sets.len().to_string(),
        _ => {
            // The matched group { θ·e : θ ∈ Θ } as textual payloads.
            // E1-FLIP Path B V4 — Step 3: this materialization is the long SINK op over
            // the join's binding alternatives; poll for liveness (in-flight = the whole
            // `binding_sets`, which carries live MettaValues the caller holds across this
            // reduction). Explicit counted loop (was `.map().collect()`) to host the poll.
            #[cfg(feature = "index-gc")]
            let mut gc_poll_counter: u64 = 0;
            let mut group: Vec<String> = Vec::with_capacity(binding_sets.len());
            for theta in binding_sets.iter() {
                #[cfg(feature = "index-gc")]
                mork_liveness_poll(&mut gc_poll_counter, &[], binding_sets, &[]);
                group.push(apply_bindings_generic(e, theta, factory).friendly_repr());
            }
            match head {
                "sum" => {
                    let mut acc: u64 = 0;
                    for g in &group {
                        acc = acc.checked_add(g.trim().parse::<u64>().ok()?)?;
                    }
                    acc.to_string()
                }
                "fsum" | "fmin" | "fmax" | "fprod" => {
                    let mut xs: Vec<f64> = Vec::with_capacity(group.len());
                    for g in &group {
                        xs.push(g.trim().parse::<f64>().ok()?);
                    }
                    let r = match head {
                        "fsum" => xs.iter().sum::<f64>(),
                        "fprod" => xs.iter().product::<f64>(),
                        "fmin" => xs.iter().copied().fold(f64::INFINITY, f64::min),
                        "fmax" => xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                        _ => unreachable!(),
                    };
                    format!("{}", r)
                }
                "and" => {
                    if group.iter().all(|g| g == "True") {
                        "True".to_string()
                    } else {
                        "False".to_string()
                    }
                }
                "hash" => {
                    // Deterministic, order-insensitive FNV-1a digest of the group.
                    let mut sorted = group;
                    sorted.sort();
                    let mut h: u64 = 0xcbf29ce484222325;
                    for g in &sorted {
                        for b in g.as_bytes() {
                            h ^= *b as u64;
                            h = h.wrapping_mul(0x100000001b3);
                        }
                        h ^= 0xff; // element boundary
                        h = h.wrapping_mul(0x100000001b3);
                    }
                    h.to_string()
                }
                _ => return None, // not a reduction sink — fall through
            }
        }
    };

    // Σ ▷ Insert(ctx[slot ↦ Sym(result)]).
    let mut subst = GenericBindings::new();
    subst.insert(slot_name, factory.atom(&result_str));
    let inserted = apply_bindings_generic(ctx, &subst, factory);
    env.add_to_space_shared(&inserted);
    Some(vec![inserted])
}

pub fn eval_exec_generic<V, F>(
    items: Vec<V>,
    env: GenericEnvironment<V, F>,
    factory: &F,
) -> GenericMorkResult<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Copy + Clone,
{
    let args = &items[1..]; // Skip "exec" operator

    if args.len() < 3 {
        let err = factory.error(
            factory.atom("IncorrectNumberOfArguments"),
            factory.sexpr(args.to_vec()),
        );
        return (vec![err], env);
    }

    let _priority = &args[0]; // Priority for future use
    let antecedent = &args[1];
    let consequent = &args[2];

    // X-followup (2026-05-11): the previous implementation added the exec
    // S-expression itself to env.space via `env.add_to_space(&exec_fact)`,
    // making (get-atoms &self) report the exec call alongside the genuine
    // facts. HE's `exec` does not self-store; remove the add. Dynamic
    // re-firing of exec rules is tracked separately via the rule registry.

    // Extract antecedent goals
    let antecedent_goals = match extract_conjunction_goals(antecedent) {
        Some(goals) => goals,
        None => {
            let err = factory.error(
                factory.string("exec antecedent must be a conjunction (,)"),
                antecedent.clone(),
            );
            return (vec![err], env);
        }
    };

    // Evaluate antecedent conjunction to get bindings
    let binding_sets = match_conjunction_goals_generic(&antecedent_goals, &env, factory);

    // If antecedent failed, rule doesn't fire
    if binding_sets.is_empty() {
        return (vec![], env);
    }

    // Stage 5b (MM2 §10 reduction sinks): a consequent that is a single reduction-sink
    // O-template `(O (<sink> ctx slot e))` (count/sum/fsum/fmin/fmax/fprod/and/hash)
    // aggregates `e` over ALL antecedent matches Θ and inserts `ctx[slot ↦ Sym(result)]`
    // ONCE (SNK-COUNT/SUM/FRED/…) — distinct from the per-match `O`-dispatch below.
    if let Some(sink_results) =
        try_eval_reduction_sink_generic(consequent, &binding_sets, &env, factory)
    {
        return (sink_results, env);
    }

    // For each binding set, evaluate consequent
    let mut all_results = Vec::new();
    let mut final_env = env;

    // E1-FLIP Path B V4 — Step 3: per-loop liveness throttle (index-gc only).
    #[cfg(feature = "index-gc")]
    let mut gc_poll_counter: u64 = 0;
    for bindings in binding_sets {
        // E1-FLIP Path B V4 — Step 3: liveness poll (results so far ∪ this binding set).
        #[cfg(feature = "index-gc")]
        mork_liveness_poll(
            &mut gc_poll_counter,
            &all_results,
            std::slice::from_ref(&bindings),
            &[],
        );
        // Apply bindings to consequent
        let instantiated_consequent = apply_bindings_generic(consequent, &bindings, factory);

        // Check if consequent is a conjunction
        if let Some(goals) = extract_conjunction_goals(&instantiated_consequent) {
            let (conseq_results, conseq_env) = eval_consequent_conjunction_generic(
                goals,
                bindings.clone(),
                final_env.clone(),
                factory,
            );
            all_results.extend(conseq_results);
            final_env = conseq_env;
        } else if let Some(items) = instantiated_consequent.as_sexpr() {
            if matches_operation_generic(items) {
                // Handle operation: (O (+ fact) (- fact) ...)
                let (op_results, op_env) =
                    eval_operation_generic(items, final_env.clone(), factory);
                all_results.extend(op_results);
                final_env = op_env;
            } else {
                let err = factory.error(
                    factory.string("exec consequent must be a conjunction or operation (O ...)"),
                    instantiated_consequent.clone(),
                );
                all_results.push(err);
            }
        } else {
            let err = factory.error(
                factory.string("exec consequent must be a conjunction or operation (O ...)"),
                instantiated_consequent.clone(),
            );
            all_results.push(err);
        }
    }

    (all_results, final_env)
}

/// Match conjunction goals with binding threading (generic version).
fn match_conjunction_goals_generic<V, F>(
    goals: &[V],
    env: &GenericEnvironment<V, F>,
    factory: &F,
) -> Vec<GenericBindings<V>>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Copy + Clone,
{
    if goals.is_empty() {
        return vec![GenericBindings::new()];
    }

    // Stage 1c (MM2 R-TPL-CC exec conformance + full ProductZipper integration): match
    // the `(exec …)` antecedent conjunction with MORK's `query_multi`/`ProductZipper`
    // n-way trie join. This is the genuine consumer of the validated conjunction join
    // (Stage 1): an antecedent `(, π₁ … πₙ)` is matched as a single (n−1)-factor
    // left-deep trie join (byte-prefix-pruned) instead of the `O(|space|^n)` fetch-all +
    // Rust Cartesian merge in `thread_bindings_through_goals_generic`. When the join
    // produces bindings, the directive FIRES (`eval_exec_generic` Pass 2 inserts the
    // instantiated templates) — making `(exec)` conformant with mm2-spec §6 R-TPL-CC
    // (`Insert(θ · τⱼ)`), which the iterative path's empty-binding behavior silently
    // suppressed. `match_conjunction_query_multi` returns `None` (→ iterative fallback)
    // whenever completeness is not guaranteed (variable facts present, `=`-headed goal,
    // arity overflow), so non-ground antecedents keep the bidirectional iterative join.
    if let Some(binding_sets) = env.match_conjunction_query_multi(goals) {
        return binding_sets;
    }

    let initial_bindings = vec![GenericBindings::new()];
    thread_bindings_through_goals_generic(goals, initial_bindings, env, factory)
}

/// Thread bindings through conjunction goals (generic version).
fn thread_bindings_through_goals_generic<V, F>(
    goals: &[V],
    current_bindings: Vec<GenericBindings<V>>,
    env: &GenericEnvironment<V, F>,
    factory: &F,
) -> Vec<GenericBindings<V>>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Copy + Clone,
{
    if goals.is_empty() {
        return current_bindings;
    }

    let goal = &goals[0];
    let remaining_goals = &goals[1..];

    let mut next_bindings = Vec::new();

    // E1-FLIP Path B V4 — Step 3: per-loop liveness throttle (index-gc only).
    #[cfg(feature = "index-gc")]
    let mut gc_poll_counter: u64 = 0;
    for bindings in current_bindings {
        // E1-FLIP Path B V4 — Step 3: liveness poll. In-flight = the accumulated
        // `next_bindings` alternatives ∪ the just-dequeued `bindings` being expanded.
        #[cfg(feature = "index-gc")]
        mork_liveness_poll(
            &mut gc_poll_counter,
            &[],
            &next_bindings,
            std::slice::from_ref(&bindings),
        );
        // Apply current bindings to goal
        let instantiated_goal = apply_bindings_generic(goal, &bindings, factory);

        // Get all facts from space using generic method
        let wildcard = factory.atom("$_");
        let all_facts = env.match_space(&wildcard, &wildcard);

        // Try to match against each fact
        for match_result in all_facts.iter() {
            if let Some(new_bindings) =
                pattern_match_generic(&instantiated_goal, &match_result.value)
            {
                // Merge bindings
                let mut merged = bindings.clone();
                let mut conflict = false;

                for (name, value) in new_bindings.iter() {
                    if let Some(existing) = merged.get(name) {
                        // Check for conflict using friendly_repr comparison
                        if existing.friendly_repr() != value.friendly_repr() {
                            conflict = true;
                            break;
                        }
                    } else {
                        merged.insert(name, value.clone());
                    }
                }

                if !conflict {
                    next_bindings.push(merged);
                }
            }
        }
    }

    if next_bindings.is_empty() {
        return vec![];
    }

    thread_bindings_through_goals_generic(remaining_goals, next_bindings, env, factory)
}

/// Evaluate consequent conjunction with binding threading (generic version).
///
/// ## CoW-Safe Implementation
///
/// Uses `add_to_space_shared()` for interior mutability on the shared state,
/// avoiding CoW deep copies that would cause state loss when the environment
/// is cloned in loops.
fn eval_consequent_conjunction_generic<V, F>(
    goals: Vec<V>,
    initial_bindings: GenericBindings<V>,
    env: GenericEnvironment<V, F>,
    factory: &F,
) -> GenericMorkResult<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Copy + Clone,
{
    if goals.is_empty() {
        return (vec![factory.unit()], env);
    }

    // BUG-T0-013 (spec §23.2): Pass 1 must Cartesian-fan-out across ALL
    // matches for each goal, not just the first. Maintain a Vec<bindings>
    // that grows as we iterate; for each goal, expand by all matches.
    // Stack-safe (iterative loop, no recursion).
    let mut binding_alternatives: Vec<GenericBindings<V>> = vec![initial_bindings.clone()];

    // E1-FLIP Path B V4 — Step 3: per-loop liveness throttle (index-gc only).
    #[cfg(feature = "index-gc")]
    let mut gc_poll_counter: u64 = 0;
    for goal in goals.iter() {
        // Skip exec forms in pass 1
        if is_exec_form_generic(goal) {
            continue;
        }

        let mut next_alternatives: Vec<GenericBindings<V>> =
            Vec::with_capacity(binding_alternatives.len());

        for current_bindings in binding_alternatives.iter() {
            // E1-FLIP Path B V4 — Step 3: liveness poll. In-flight = the source
            // `binding_alternatives` ∪ the accumulating `next_alternatives`.
            #[cfg(feature = "index-gc")]
            mork_liveness_poll(
                &mut gc_poll_counter,
                &[],
                &binding_alternatives,
                &next_alternatives,
            );
            let instantiated_goal = apply_bindings_generic(goal, current_bindings, factory);

            // If goal has variables, fan out across all matches in space.
            if has_variables_generic(&instantiated_goal) {
                let matches = env.match_space(&instantiated_goal, &instantiated_goal);
                if matches.is_empty() {
                    // No match — preserve current bindings unchanged (so
                    // subsequent goals can still add facts via Pass 2).
                    next_alternatives.push(current_bindings.clone());
                } else {
                    // Each match produces a new alternative.
                    for m in matches.iter() {
                        if let Some(new_bindings) =
                            pattern_match_generic(&instantiated_goal, &m.value)
                        {
                            let mut merged = current_bindings.clone();
                            for (name, value) in new_bindings.iter() {
                                merged.insert(name, value.clone());
                            }
                            next_alternatives.push(merged);
                        }
                    }
                }
            } else {
                // Ground goal — preserve current bindings.
                next_alternatives.push(current_bindings.clone());
            }
        }

        binding_alternatives = next_alternatives;
        if binding_alternatives.is_empty() {
            return (vec![], env);
        }
    }

    // Pass 2: For each binding alternative, add all goals to space and emit
    // the instantiated results. The Cartesian product is preserved.
    let mut all_results: Vec<V> = Vec::with_capacity(binding_alternatives.len() * goals.len());

    for current_bindings in binding_alternatives.iter() {
        // E1-FLIP Path B V4 — Step 3: liveness poll. In-flight = the emitted
        // `all_results` so far ∪ the remaining `binding_alternatives` to instantiate.
        #[cfg(feature = "index-gc")]
        mork_liveness_poll(
            &mut gc_poll_counter,
            &all_results,
            &binding_alternatives,
            &[],
        );
        for goal in goals.iter() {
            let fully_instantiated = apply_bindings_generic(goal, current_bindings, factory);

            if is_exec_form_generic(&fully_instantiated) {
                env.add_to_space_shared(&fully_instantiated);
                all_results.push(factory.unit());
            } else if is_operation_form_generic(&fully_instantiated) {
                if let Some(items) = fully_instantiated.as_sexpr() {
                    let (op_results, _) = eval_operation_generic_shared(items, &env, factory);
                    all_results.extend(op_results);
                }
            } else {
                env.add_to_space_shared(&fully_instantiated);
                all_results.push(fully_instantiated.clone());
            }
        }
    }

    (all_results, env)
}

/// Evaluate operation (O (+ fact) (- fact) ...) (generic version).
fn eval_operation_generic<V, F>(
    items: &[V],
    mut env: GenericEnvironment<V, F>,
    factory: &F,
) -> GenericMorkResult<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    let operations = &items[1..]; // Skip "O" operator

    for op in operations {
        if let Some(op_items) = op.as_sexpr() {
            if op_items.len() == 2 {
                if let Some(op_type) = op_items[0].as_atom() {
                    let fact = &op_items[1];
                    match op_type {
                        "+" => {
                            env.add_to_space(fact);
                        }
                        "-" => {
                            env.remove_from_space(fact);
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    (vec![factory.unit()], env)
}

/// Evaluate operation using interior mutability (CoW-safe version).
///
/// Uses `add_to_space_shared()` and `remove_from_space_shared()` to avoid
/// triggering CoW deep copies when the environment is cloned.
fn eval_operation_generic_shared<V, F>(
    items: &[V],
    env: &GenericEnvironment<V, F>,
    factory: &F,
) -> (Vec<V>, ())
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    let operations = &items[1..]; // Skip "O" operator

    for op in operations {
        if let Some(op_items) = op.as_sexpr() {
            if op_items.len() == 2 {
                if let Some(op_type) = op_items[0].as_atom() {
                    let fact = &op_items[1];
                    match op_type {
                        "+" => {
                            // Use interior mutability - no CoW copy
                            env.add_to_space_shared(fact);
                        }
                        "-" => {
                            // Use interior mutability - no CoW copy
                            env.remove_from_space_shared(fact);
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    (vec![factory.unit()], ())
}

/// Generic eval_coalg: (coalg <pattern> <templates>)
///
/// Coalgebra patterns for tree transformations using generic types.
pub fn eval_coalg_generic<V, F>(
    items: Vec<V>,
    env: GenericEnvironment<V, F>,
    factory: &F,
) -> GenericMorkResult<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    let args = &items[1..]; // Skip "coalg" operator

    if args.len() < 2 {
        let err = factory.error(
            factory.atom("IncorrectNumberOfArguments"),
            factory.sexpr(args.to_vec()),
        );
        return (vec![err], env);
    }

    let pattern = &args[0];
    let templates = &args[1];

    // Templates must be a conjunction
    let template_list: Vec<V> = match templates.as_conjunction() {
        Some(temps) => temps.to_vec(),
        None => {
            let err = factory.error(
                factory.string("coalg templates must be a conjunction (,)"),
                templates.clone(),
            );
            return (vec![err], env);
        }
    };

    // BUG-T0-011 (spec §23.4): real coalg implementation. Pattern-match
    // against each fact in the space, substitute into every template, return
    // all instantiated templates as a flat Vec (template-major, match-major
    // order). Uses iterator-fusion to avoid materializing intermediate
    // per-match Vecs. Stack-safe (no recursion).
    let matches = env.match_space(pattern, pattern);
    if matches.is_empty() {
        // No fact in space matches — coalg yields no results.
        return (vec![], env);
    }

    let mut results: Vec<V> = Vec::with_capacity(matches.len() * template_list.len());
    for m in matches.iter() {
        // Re-bind pattern against this match's atom value to obtain bindings.
        if let Some(bindings) = pattern_match_generic(pattern, &m.value) {
            for tmpl in &template_list {
                results.push(apply_bindings_generic(tmpl, &bindings, factory));
            }
        }
    }

    (results, env)
}

/// Generic eval_lookup: (lookup <pattern> <success-goals> <failure-goals>)
///
/// Conditional execution based on space queries using generic types.
pub fn eval_lookup_generic<V, F>(
    items: Vec<V>,
    env: GenericEnvironment<V, F>,
    factory: &F,
) -> GenericMorkResult<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Copy + Clone,
{
    let args = &items[1..]; // Skip "lookup" operator

    if args.len() < 3 {
        let err = factory.error(
            factory.atom("IncorrectNumberOfArguments"),
            factory.sexpr(args.to_vec()),
        );
        return (vec![err], env);
    }

    let pattern = &args[0];
    let success_goals = &args[1];
    let failure_goals = &args[2];

    // Validate branches are conjunctions
    if success_goals.as_conjunction().is_none() {
        let err = factory.error(
            factory.string("lookup success branch must be a conjunction (,)"),
            success_goals.clone(),
        );
        return (vec![err], env);
    }

    if failure_goals.as_conjunction().is_none() {
        let err = factory.error(
            factory.string("lookup failure branch must be a conjunction (,)"),
            failure_goals.clone(),
        );
        return (vec![err], env);
    }

    // BUG-T0-010 (spec §23.5): Use actual space query, not syntactic
    // "starts-with-$" heuristic. The pattern is "found" when env.match_space
    // returns at least one matching atom; otherwise we evaluate the failure
    // branch. This matches the spec's intent for `(lookup pat yes no)`:
    // evaluate yes-conjunction if any fact matches `pat`, else evaluate no.
    let matches = env.match_space(pattern, pattern);
    let pattern_found = !matches.is_empty();

    if pattern_found {
        // Evaluate success branch
        if let Some(goals) = success_goals.as_conjunction() {
            eval_conjunction_goals_generic(goals.to_vec(), env, factory)
        } else {
            (vec![], env)
        }
    } else {
        // Evaluate failure branch
        if let Some(goals) = failure_goals.as_conjunction() {
            eval_conjunction_goals_generic(goals.to_vec(), env, factory)
        } else {
            (vec![], env)
        }
    }
}

/// Evaluate conjunction goals sequentially (generic version).
///
/// FULLY GENERIC - NO CONVERSIONS REQUIRED.
///
/// For MORK semantics, conjunction goals don't need full MeTTa evaluation.
/// They need:
/// 1. Pattern matching against space
/// 2. Adding/removing facts from space
/// 3. Executing operations (O ...)
///
/// This enables zero-conversion evaluation for both heap and arena modes.
fn eval_conjunction_goals_generic<V, F>(
    goals: Vec<V>,
    mut env: GenericEnvironment<V, F>,
    factory: &F,
) -> GenericMorkResult<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Copy + Clone,
{
    let mut all_results = Vec::new();

    for goal in goals {
        // Handle different goal types using MORK semantics (no full eval needed)

        // Check if goal is an exec form - add to space
        if is_exec_form_generic(&goal) {
            env.add_to_space(&goal);
            all_results.push(factory.unit());
            continue;
        }

        // Check if goal is an operation form - execute it
        if is_operation_form_generic(&goal) {
            if let Some(items) = goal.as_sexpr() {
                let (op_results, op_env) = eval_operation_generic(items, env, factory);
                all_results.extend(op_results);
                env = op_env;
            }
            continue;
        }

        // Check if goal is a lookup - evaluate it recursively
        if let Some(items) = goal.as_sexpr() {
            if !items.is_empty() {
                if let Some(op) = items[0].as_atom() {
                    if op == "lookup" {
                        let (lookup_results, lookup_env) =
                            eval_lookup_generic(items.to_vec(), env, factory);
                        all_results.extend(lookup_results);
                        env = lookup_env;
                        continue;
                    }
                    // Handle nested exec in goals
                    if op == "exec" {
                        let (exec_results, exec_env) =
                            eval_exec_generic(items.to_vec(), env, factory);
                        all_results.extend(exec_results);
                        env = exec_env;
                        continue;
                    }
                    // Handle nested coalg in goals
                    if op == "coalg" {
                        let (coalg_results, coalg_env) =
                            eval_coalg_generic(items.to_vec(), env, factory);
                        all_results.extend(coalg_results);
                        env = coalg_env;
                        continue;
                    }
                }
            }
        }

        // For other goals: If it has variables, try to match against space
        // Otherwise, add it to space as a fact
        if has_variables_generic(&goal) {
            // Try to match against space and return the matches
            let matches = env.match_space(&goal, &goal);
            if !matches.is_empty() {
                // Return first match
                all_results.push(matches[0].value.clone());
            } else {
                // No match found - return the goal itself
                all_results.push(goal.clone());
            }
        } else {
            // Ground fact - add to space and return it
            env.add_to_space(&goal);
            all_results.push(goal.clone());
        }
    }

    (all_results, env)
}

/// Generic eval_rulify: (rulify $name (, $p0) (, $t0 ...) <antecedent> <consequent>)
///
/// Generates exec rules from coalgebra definitions using generic types.
pub fn eval_rulify_generic<V, F>(
    items: Vec<V>,
    env: GenericEnvironment<V, F>,
    factory: &F,
) -> GenericMorkResult<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V> + Clone,
{
    let args = &items[1..]; // Skip "rulify" operator

    if args.len() < 5 {
        let err = factory.error(
            factory.atom("IncorrectNumberOfArguments"),
            factory.sexpr(args.to_vec()),
        );
        return (vec![err], env);
    }

    let name = &args[0];
    let pattern_conj = &args[1];
    let templates_conj = &args[2];
    let rule_antecedent = &args[3];
    let rule_consequent = &args[4];

    // Extract pattern from unary conjunction
    let pattern = match pattern_conj.as_conjunction() {
        Some(ps) if ps.len() == 1 => ps[0].clone(),
        _ => {
            let err = factory.error(
                factory.string("rulify pattern must be a unary conjunction (, $p0)"),
                pattern_conj.clone(),
            );
            return (vec![err], env);
        }
    };

    // Extract templates from conjunction
    let templates = match templates_conj.as_conjunction() {
        Some(ts) => ts.to_vec(),
        None => {
            let err = factory.error(
                factory.string("rulify templates must be a conjunction (, $t0 ...)"),
                templates_conj.clone(),
            );
            return (vec![err], env);
        }
    };

    // Create meta-rule structure
    let meta_rule = factory.sexpr(vec![
        factory.atom("meta-rule"),
        name.clone(),
        pattern,
        factory.conjunction(templates),
        rule_antecedent.clone(),
        rule_consequent.clone(),
    ]);

    (vec![meta_rule], env)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::environment::MettaEnvironment;
    // (convert) Active factory so MORK special-form evaluation tests run under
    // both the slab `GcFactory` and the index `IndexFactory` (GC A/B differential).
    use crate::backend::models::{active_factory, MettaValue};

    #[test]
    fn test_has_variables_generic() {
        let var = MettaValue::Atom("$x".to_string());
        assert!(has_variables_generic(&var));

        let atom = MettaValue::Atom("foo".to_string());
        assert!(!has_variables_generic(&atom));

        let sexpr_with_var = MettaValue::SExpr(vec![
            MettaValue::Atom("f".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);
        assert!(has_variables_generic(&sexpr_with_var));
    }

    #[test]
    fn test_is_exec_form_generic() {
        let exec = MettaValue::SExpr(vec![
            MettaValue::Atom("exec".to_string()),
            MettaValue::Atom("P0".to_string()),
        ]);
        assert!(is_exec_form_generic(&exec));

        let not_exec = MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Atom("bar".to_string()),
        ]);
        assert!(!is_exec_form_generic(&not_exec));
    }

    #[test]
    fn test_eval_exec_generic_empty_antecedent() {
        let env = MettaEnvironment::new(active_factory());
        let factory = active_factory();

        let items = vec![
            MettaValue::Atom("exec".to_string()),
            MettaValue::Atom("P0".to_string()),
            MettaValue::Conjunction(vec![]), // Empty antecedent
            MettaValue::Conjunction(vec![MettaValue::Long(42)]),
        ];

        let (results, _) = eval_exec_generic(items, env, &factory);
        assert!(!results.is_empty());
    }

    /// BUG-T0-011 (post-fix): coalg now performs an actual space query.
    /// With an empty space, no matches are produced and no templates are
    /// instantiated — coalg yields zero results. Pre-populating the space
    /// with a matching fact restores results.
    #[test]
    fn test_eval_coalg_generic() {
        let factory = active_factory();
        let env = MettaEnvironment::new(factory.clone());

        let items = vec![
            factory.atom("coalg"),
            factory.sexpr(vec![factory.atom("tree"), factory.atom("$t")]),
            factory.conjunction(vec![factory.sexpr(vec![
                factory.atom("ctx"),
                factory.atom("$t"),
                factory.atom("nil"),
            ])]),
        ];

        // Empty space: coalg yields no results (no facts to coalgebraically
        // unfold from).
        let (results, env) = eval_coalg_generic(items.clone(), env, &factory);
        assert!(
            results.is_empty(),
            "coalg with empty space should yield zero results, got: {:?}",
            results
        );

        // Pre-populate space with a matching fact and re-run.
        let fact = factory.sexpr(vec![factory.atom("tree"), factory.atom("leaf1")]);
        env.add_to_space_shared(&fact);
        let (results, _) = eval_coalg_generic(items, env, &factory);
        assert_eq!(
            results.len(),
            1,
            "coalg with one matching fact should yield one template instantiation"
        );
    }

    #[test]
    fn test_eval_lookup_generic_success() {
        let env = MettaEnvironment::new(active_factory());
        let factory = active_factory();

        let items = vec![
            MettaValue::Atom("lookup".to_string()),
            MettaValue::Atom("foo".to_string()), // Not a variable
            MettaValue::Conjunction(vec![MettaValue::Atom("T".to_string())]),
            MettaValue::Conjunction(vec![MettaValue::Atom("F".to_string())]),
        ];

        let (results, _) = eval_lookup_generic(items, env, &factory);
        assert!(!results.is_empty());
    }

    #[test]
    fn test_eval_rulify_generic() {
        let env = MettaEnvironment::new(active_factory());
        let factory = active_factory();

        let items = vec![
            MettaValue::Atom("rulify".to_string()),
            MettaValue::Atom("test_rule".to_string()),
            MettaValue::Conjunction(vec![MettaValue::Atom("$p0".to_string())]),
            MettaValue::Conjunction(vec![MettaValue::Atom("$t0".to_string())]),
            MettaValue::Conjunction(vec![]),
            MettaValue::Atom("consequent".to_string()),
        ];

        let (results, _) = eval_rulify_generic(items, env, &factory);
        assert!(!results.is_empty());
    }

    // ========================================================================
    // Error Path Tests (Phase 6: Branch Coverage)
    // ========================================================================

    #[test]
    fn test_exec_wrong_arity() {
        // exec requires 3 arguments: priority, antecedent, consequent
        let env = MettaEnvironment::new(active_factory());
        let factory = active_factory();

        // Only 2 arguments (missing consequent)
        let items = vec![
            MettaValue::Atom("exec".to_string()),
            MettaValue::Atom("P0".to_string()),
            MettaValue::Conjunction(vec![]),
        ];

        let (results, _) = eval_exec_generic(items, env.clone(), &factory);
        assert_eq!(results.len(), 1);
        assert!(results[0].is_error(), "Should return error for wrong arity");

        // Only 1 argument
        let items2 = vec![
            MettaValue::Atom("exec".to_string()),
            MettaValue::Atom("P0".to_string()),
        ];
        let (results2, _) = eval_exec_generic(items2, env, &factory);
        assert!(results2[0].is_error());
    }

    #[test]
    fn test_exec_antecedent_not_conjunction() {
        // exec antecedent must be a conjunction
        let env = MettaEnvironment::new(active_factory());
        let factory = active_factory();

        let items = vec![
            MettaValue::Atom("exec".to_string()),
            MettaValue::Atom("P0".to_string()),
            MettaValue::Atom("not_a_conjunction".to_string()), // Not a conjunction
            MettaValue::Conjunction(vec![MettaValue::Long(42)]),
        ];

        let (results, _) = eval_exec_generic(items, env, &factory);
        assert_eq!(results.len(), 1);
        assert!(
            results[0].is_error(),
            "Should return error for non-conjunction antecedent"
        );
    }

    #[test]
    fn test_coalg_wrong_arity() {
        // coalg requires 2 arguments: pattern and templates
        let env = MettaEnvironment::new(active_factory());
        let factory = active_factory();

        // Only 1 argument (missing templates)
        let items = vec![
            MettaValue::Atom("coalg".to_string()),
            MettaValue::Atom("pattern".to_string()),
        ];

        let (results, _) = eval_coalg_generic(items, env.clone(), &factory);
        assert_eq!(results.len(), 1);
        assert!(results[0].is_error(), "Should return error for wrong arity");

        // No arguments
        let items2 = vec![MettaValue::Atom("coalg".to_string())];
        let (results2, _) = eval_coalg_generic(items2, env, &factory);
        assert!(results2[0].is_error());
    }

    #[test]
    fn test_coalg_templates_not_conjunction() {
        // coalg templates must be a conjunction
        let env = MettaEnvironment::new(active_factory());
        let factory = active_factory();

        let items = vec![
            MettaValue::Atom("coalg".to_string()),
            MettaValue::Atom("pattern".to_string()),
            MettaValue::Atom("not_a_conjunction".to_string()), // Not a conjunction
        ];

        let (results, _) = eval_coalg_generic(items, env, &factory);
        assert_eq!(results.len(), 1);
        assert!(
            results[0].is_error(),
            "Should return error for non-conjunction templates"
        );
    }

    #[test]
    fn test_lookup_wrong_arity() {
        // lookup requires 3 arguments: pattern, success-goals, failure-goals
        let env = MettaEnvironment::new(active_factory());
        let factory = active_factory();

        // Only 2 arguments
        let items = vec![
            MettaValue::Atom("lookup".to_string()),
            MettaValue::Atom("pattern".to_string()),
            MettaValue::Conjunction(vec![]),
        ];

        let (results, _) = eval_lookup_generic(items, env.clone(), &factory);
        assert_eq!(results.len(), 1);
        assert!(results[0].is_error(), "Should return error for wrong arity");

        // Only 1 argument
        let items2 = vec![
            MettaValue::Atom("lookup".to_string()),
            MettaValue::Atom("pattern".to_string()),
        ];
        let (results2, _) = eval_lookup_generic(items2, env, &factory);
        assert!(results2[0].is_error());
    }

    #[test]
    fn test_lookup_success_not_conjunction() {
        // lookup success branch must be a conjunction
        let env = MettaEnvironment::new(active_factory());
        let factory = active_factory();

        let items = vec![
            MettaValue::Atom("lookup".to_string()),
            MettaValue::Atom("pattern".to_string()),
            MettaValue::Atom("not_a_conjunction".to_string()), // Not a conjunction
            MettaValue::Conjunction(vec![]),
        ];

        let (results, _) = eval_lookup_generic(items, env, &factory);
        assert_eq!(results.len(), 1);
        assert!(
            results[0].is_error(),
            "Should return error for non-conjunction success branch"
        );
    }

    #[test]
    fn test_lookup_failure_not_conjunction() {
        // lookup failure branch must be a conjunction
        let env = MettaEnvironment::new(active_factory());
        let factory = active_factory();

        let items = vec![
            MettaValue::Atom("lookup".to_string()),
            MettaValue::Atom("pattern".to_string()),
            MettaValue::Conjunction(vec![]),
            MettaValue::Atom("not_a_conjunction".to_string()), // Not a conjunction
        ];

        let (results, _) = eval_lookup_generic(items, env, &factory);
        assert_eq!(results.len(), 1);
        assert!(
            results[0].is_error(),
            "Should return error for non-conjunction failure branch"
        );
    }

    #[test]
    fn test_lookup_variable_pattern() {
        // lookup with variable pattern takes failure branch
        let env = MettaEnvironment::new(active_factory());
        let factory = active_factory();

        let items = vec![
            MettaValue::Atom("lookup".to_string()),
            MettaValue::Atom("$x".to_string()), // Variable pattern
            MettaValue::Conjunction(vec![MettaValue::Atom("success".to_string())]),
            MettaValue::Conjunction(vec![MettaValue::Atom("failure".to_string())]),
        ];

        let (results, _) = eval_lookup_generic(items, env, &factory);
        // Variable pattern means "not found", so failure branch should be taken
        assert!(!results.is_empty());
    }

    #[test]
    fn test_rulify_wrong_arity() {
        // rulify requires 5 arguments
        let env = MettaEnvironment::new(active_factory());
        let factory = active_factory();

        // Only 4 arguments
        let items = vec![
            MettaValue::Atom("rulify".to_string()),
            MettaValue::Atom("name".to_string()),
            MettaValue::Conjunction(vec![MettaValue::Atom("$p0".to_string())]),
            MettaValue::Conjunction(vec![MettaValue::Atom("$t0".to_string())]),
            MettaValue::Conjunction(vec![]),
        ];

        let (results, _) = eval_rulify_generic(items, env.clone(), &factory);
        assert_eq!(results.len(), 1);
        assert!(results[0].is_error(), "Should return error for wrong arity");

        // Only 3 arguments
        let items2 = vec![
            MettaValue::Atom("rulify".to_string()),
            MettaValue::Atom("name".to_string()),
            MettaValue::Conjunction(vec![MettaValue::Atom("$p0".to_string())]),
            MettaValue::Conjunction(vec![MettaValue::Atom("$t0".to_string())]),
        ];
        let (results2, _) = eval_rulify_generic(items2, env, &factory);
        assert!(results2[0].is_error());
    }

    #[test]
    fn test_rulify_pattern_not_unary_conjunction() {
        // rulify pattern must be a unary conjunction
        let env = MettaEnvironment::new(active_factory());
        let factory = active_factory();

        // Empty conjunction
        let items = vec![
            MettaValue::Atom("rulify".to_string()),
            MettaValue::Atom("name".to_string()),
            MettaValue::Conjunction(vec![]), // Empty, not unary
            MettaValue::Conjunction(vec![MettaValue::Atom("$t0".to_string())]),
            MettaValue::Conjunction(vec![]),
            MettaValue::Atom("consequent".to_string()),
        ];

        let (results, _) = eval_rulify_generic(items, env.clone(), &factory);
        assert_eq!(results.len(), 1);
        assert!(
            results[0].is_error(),
            "Should return error for non-unary conjunction"
        );

        // Binary conjunction
        let items2 = vec![
            MettaValue::Atom("rulify".to_string()),
            MettaValue::Atom("name".to_string()),
            MettaValue::Conjunction(vec![
                MettaValue::Atom("$p0".to_string()),
                MettaValue::Atom("$p1".to_string()),
            ]), // Binary, not unary
            MettaValue::Conjunction(vec![MettaValue::Atom("$t0".to_string())]),
            MettaValue::Conjunction(vec![]),
            MettaValue::Atom("consequent".to_string()),
        ];
        let (results2, _) = eval_rulify_generic(items2, env.clone(), &factory);
        assert!(results2[0].is_error());

        // Not a conjunction at all
        let items3 = vec![
            MettaValue::Atom("rulify".to_string()),
            MettaValue::Atom("name".to_string()),
            MettaValue::Atom("not_a_conjunction".to_string()),
            MettaValue::Conjunction(vec![MettaValue::Atom("$t0".to_string())]),
            MettaValue::Conjunction(vec![]),
            MettaValue::Atom("consequent".to_string()),
        ];
        let (results3, _) = eval_rulify_generic(items3, env, &factory);
        assert!(results3[0].is_error());
    }

    #[test]
    fn test_rulify_templates_not_conjunction() {
        // rulify templates must be a conjunction
        let env = MettaEnvironment::new(active_factory());
        let factory = active_factory();

        let items = vec![
            MettaValue::Atom("rulify".to_string()),
            MettaValue::Atom("name".to_string()),
            MettaValue::Conjunction(vec![MettaValue::Atom("$p0".to_string())]),
            MettaValue::Atom("not_a_conjunction".to_string()), // Not a conjunction
            MettaValue::Conjunction(vec![]),
            MettaValue::Atom("consequent".to_string()),
        ];

        let (results, _) = eval_rulify_generic(items, env, &factory);
        assert_eq!(results.len(), 1);
        assert!(
            results[0].is_error(),
            "Should return error for non-conjunction templates"
        );
    }

    #[test]
    fn test_is_operation_form_generic() {
        // Test the O operation form checker
        let op_form = MettaValue::SExpr(vec![
            MettaValue::Atom("O".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Atom("fact".to_string()),
            ]),
        ]);
        assert!(is_operation_form_generic(&op_form));

        let not_op_form = MettaValue::SExpr(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Atom("bar".to_string()),
        ]);
        assert!(!is_operation_form_generic(&not_op_form));

        // Empty s-expr
        let empty = MettaValue::SExpr(vec![]);
        assert!(!is_operation_form_generic(&empty));
    }

    #[test]
    fn test_has_variables_conjunction() {
        // Test has_variables_generic with Conjunction variant
        let conj_with_var = MettaValue::Conjunction(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);
        assert!(has_variables_generic(&conj_with_var));

        let conj_no_var = MettaValue::Conjunction(vec![
            MettaValue::Atom("foo".to_string()),
            MettaValue::Atom("bar".to_string()),
        ]);
        assert!(!has_variables_generic(&conj_no_var));
    }

    #[test]
    fn test_has_variables_error() {
        // HE-bisimilar Error(offending, detail). A variable in either slot
        // must make the whole Error report having variables.
        let err_with_var = MettaValue::Error(MettaValue::Atom("$x"), MettaValue::String("test"));
        assert!(has_variables_generic(&err_with_var));

        let err_no_var = MettaValue::Error(MettaValue::Atom("foo"), MettaValue::String("test"));
        assert!(!has_variables_generic(&err_no_var));
    }

    #[test]
    fn test_has_variables_ampersand_and_quote() {
        // Test other variable prefixes
        let amp_var = MettaValue::Atom("&x".to_string());
        assert!(has_variables_generic(&amp_var));

        let quote_var = MettaValue::Atom("'x".to_string());
        assert!(has_variables_generic(&quote_var));
    }
}
