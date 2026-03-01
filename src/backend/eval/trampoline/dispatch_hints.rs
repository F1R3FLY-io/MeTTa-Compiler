//! Dispatch Hints — Memoization and Type-Driven Evaluation Shortcuts
//!
//! Optimization helpers extracted from `generic_trampoline.rs` to improve
//! icache locality for the hot trampoline loop. These functions are
//! infrequently called relative to the main evaluation path and benefit
//! from being in a separate compilation unit.
//!
//! ## Contents
//!
//! - **Normal-form memoization**: Bloom filter for fixpoint-detected expressions
//! - **Expected-type derivation**: `derive_arg_expected_type` for applicative evaluation

use std::sync::LazyLock;

use crate::backend::builtin_signatures;
use crate::backend::environment::bloom::AtomicBloomFilter;
use crate::backend::models::{MettaValueFactory, MettaValueTrait};

use super::context::EvalContext;

// ============================================================================
// Phase 9.5: Evaluated-expression normal-form memoization
// ============================================================================

/// Global bloom filter for normal-form expressions (Phase 9.5).
///
/// When an S-expression evaluates to itself (fixpoint), its `inner_ptr`
/// is inserted into this filter. Subsequent evaluations of the same pointer
/// skip the entire eval_step_generic call.
///
/// Invalidated on `add_rule()` since new rules may make previously
/// normal-form expressions reducible. `add_rule()` is O(N) during loading
/// and never during evaluation, so invalidation has zero eval-time cost.
///
/// False positives are benign: they cause us to skip evaluation for an
/// expression that might have been reducible, but since the expression was
/// never inserted (only a hash collision), this is extremely rare and bounded
/// by the bloom filter's ~1% FPR.
static NORMAL_FORM_BLOOM: LazyLock<AtomicBloomFilter> =
    LazyLock::new(|| AtomicBloomFilter::new(100_000));

/// Whether the normal-form bloom filter has any entries since its last clear.
///
/// When `false`, `is_memoized_normal_form` returns `false` immediately without
/// computing a bloom hash or touching atomic memory — eliminating ~1.5% overhead
/// in workloads where `add-atom` / `add_rule` calls keep clearing the filter
/// (e.g., PLN Robot demo with dynamic rule addition).
static NORMAL_FORM_BLOOM_ACTIVE: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Check if a value is memoized as being in normal form (Phase 9.5).
#[inline]
pub fn is_memoized_normal_form<V: MettaValueTrait>(value: &V) -> bool {
    // Fast path: if no entries have been inserted since the last clear,
    // skip bloom hash computation entirely.
    if !NORMAL_FORM_BLOOM_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
        return false;
    }
    let ptr = value.inner_ptr() as usize;
    NORMAL_FORM_BLOOM.may_contain(&ptr.to_le_bytes())
}

/// Record a value as being in normal form (Phase 9.5).
#[inline]
pub fn memoize_normal_form<V: MettaValueTrait>(value: &V) {
    let ptr = value.inner_ptr() as usize;
    NORMAL_FORM_BLOOM.insert(&ptr.to_le_bytes());
    NORMAL_FORM_BLOOM_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// Invalidate the normal-form memoization bloom filter (Phase 9.5).
///
/// Called from `add_rule()` when new rules are added. New rules may make
/// previously normal-form expressions reducible, so the entire filter
/// must be cleared.
pub fn invalidate_normal_form_memo() {
    // Only clear if the LazyLock has been initialized (avoid initializing
    // it just to clear it during early startup)
    if LazyLock::get(&NORMAL_FORM_BLOOM).is_some() {
        NORMAL_FORM_BLOOM.clear();
        NORMAL_FORM_BLOOM_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
    }
}

// ============================================================================
// Phase 9.2 + 9.3: expected_type derivation for applicative evaluation
// ============================================================================

/// Derive `expected_type` for an argument at `arg_idx` (1-based item position)
/// of the parent operator. Uses two tiers:
///
/// 1. **Builtin signatures** (Phase 9.2): Looks up the parent op's builtin
///    signature and extracts the formal type at the arg position.
/// 2. **User-declared arrow types** (Phase 9.3): If no builtin sig, checks
///    `env.get_types_generic(op)` for arrow types and extracts the consistent
///    arg type at the position.
///
/// Returns `None` if:
/// - The parent op has no type info, or
/// - The formal type is a meta-type (Atom, Expression, etc.), or
/// - Multiple arrow types disagree on the arg type at this position.
pub(super) fn derive_arg_expected_type<C: EvalContext>(
    items: &[C::Value],
    arg_idx: usize,
    env: &super::context::ContextEnv<C>,
    factory: &C::Factory,
) -> Option<C::Value>
where
    C::Value: Clone,
{
    let op = items.first().and_then(|v| v.as_atom())?;
    let arg_pos = arg_idx.checked_sub(1)?; // Convert to 0-based arg position

    // Tier 1: Builtin signatures (Phase 9.2)
    if let Some(name) = builtin_signatures::get_signature(op)
        .and_then(|sig| builtin_signatures::get_expected_type_at_position(sig, arg_pos))
        .and_then(builtin_signatures::type_expr_to_expected_type_name)
    {
        return Some(factory.atom(name));
    }

    // Tier 2: User-declared arrow types (Phase 9.3)
    extract_consistent_arg_type_from_env::<C>(op, arg_pos, env, factory)
}

/// Phase 9.3: Extract a consistent expected type at `arg_pos` from all
/// user-declared arrow types for `op`.
///
/// If ALL arrow types agree on a non-meta value type at the position,
/// returns `Some(type_atom)`. If they disagree or are meta-types, returns `None`
/// (conservative — don't prune).
fn extract_consistent_arg_type_from_env<C: EvalContext>(
    op: &str,
    arg_pos: usize,
    env: &super::context::ContextEnv<C>,
    factory: &C::Factory,
) -> Option<C::Value>
where
    C::Value: Clone,
{
    use super::super::step::{extract_arg_types, is_meta_type};

    let types = env.get_types_generic(op);
    if types.is_empty() {
        return None;
    }

    // Collect the type at arg_pos from each arrow type declaration.
    // We map to a &'static str (concrete type name) to avoid lifetime issues
    // with the temporary Vec<V> returned by extract_arg_types.
    let mut consistent_name: Option<&'static str> = None;
    let mut found_arrow = false;

    for t in &types {
        if let Some(arg_types) = extract_arg_types(t) {
            found_arrow = true;
            if let Some(arg_type) = arg_types.get(arg_pos) {
                if is_meta_type(arg_type) {
                    // Meta-type at this position → don't constrain
                    return None;
                }
                if let Some(name) = arg_type.as_atom() {
                    // Map to static str for concrete value types only
                    let static_name: &'static str = match name {
                        "Number" => "Number",
                        "Bool" => "Bool",
                        "String" => "String",
                        _ => return None, // Unknown/user-defined type → conservative
                    };
                    match consistent_name {
                        None => consistent_name = Some(static_name),
                        Some(prev) if prev != static_name => return None, // Disagree
                        _ => {} // Same name — still consistent
                    }
                } else {
                    // Structural type (e.g., (List Number)) — can't map to atom name
                    return None;
                }
            }
            // If arg_pos is out of bounds for this arrow, skip (variadic)
        }
    }

    if !found_arrow {
        return None;
    }

    consistent_name.map(|name| factory.atom(name))
}
