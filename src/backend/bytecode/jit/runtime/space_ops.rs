//! Space operations runtime functions for JIT compilation
//!
//! This module provides FFI-callable space operations:
//! - space_add - Add an atom to a space
//! - space_remove - Remove an atom from a space
//! - space_get_atoms - Get all atoms from a space
//! - space_match - Pattern match against space atoms
//! - space_match_nondet - Nondeterministic space match with choice points
//! - resume_space_match - Resume backtracking for space match
//! - free_space_match_alternatives - Cleanup space match alternatives

use super::bindings::{
    jit_runtime_fork_bindings, jit_runtime_free_saved_bindings, jit_runtime_restore_bindings,
    JitSavedBindings,
};
use super::helpers::metta_to_jit;
use super::pattern_matching::pattern_matches_impl;
use super::MAX_ALTERNATIVES_INLINE;
use crate::backend::bytecode::jit::types::{
    JitAlternative, JitAlternativeTag, JitBailoutReason, JitBindingEntry, JitChoicePoint,
    JitContext, JitValue, TAG_UNIT,
};
use crate::backend::models::{MettaValue, ValueView};

// =============================================================================
// Phase D: Space Operations
// =============================================================================

/// Add an atom to a space.
///
/// Stack: [space, atom] -> [Unit]
///
/// Mirrors the trampoline reference in `eval/trampoline/eval_loop.rs` and the
/// bytecode VM `op_space_add`. Three resolution paths (Plan A Phase 4):
///   1. `Space(handle)` → if module-space or `&self`, route through env's
///      PathMap (`env.add_to_space`) so rule definitions populate RuleIndex;
///      otherwise write to the SpaceHandle directly.
///   2. `Atom("&self")` (un-evaluated form) → same env path.
///   3. `Atom(name)` → resolve token; recurse on the resolved Space.
///   4. Otherwise → return a MeTTa Error value (caller can branch on `is-error`).
///
/// After mutation, calls `increment_mutation_epoch()` so EVAL_MEMO and
/// MATCH_RESULT_CACHE are invalidated — without this, subsequent matches
/// would return stale data (Plan A Bug 4).
///
/// # Arguments
/// * `ctx` - JIT context (must carry a valid `env_ptr` for module/`&self` writes)
/// * `space` - NaN-boxed space handle, atom token, or other (Type error)
/// * `atom` - NaN-boxed atom to add (already kept unevaluated by compile path)
/// * `_ip` - Instruction pointer (for debugging)
///
/// # Returns
/// NaN-boxed Unit on success, NaN-boxed Error on type mismatch.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_space_add(
    ctx: *mut JitContext,
    space: u64,
    atom: u64,
    _ip: u64,
) -> u64 {
    let space_val = JitValue::from_raw(space);
    let atom_val = JitValue::from_raw(atom);

    let space_metta = space_val.to_metta();
    let atom_metta = atom_val.to_metta();

    // Path 1: evaluated Space handle
    if let ValueView::Space(handle) = space_metta.view() {
        if handle.is_module_space() || handle.name == "self" {
            if let Some(ctx_ref) = ctx.as_ref() {
                if !ctx_ref.env_ptr.is_null() {
                    let env = &mut *(ctx_ref.env_ptr
                        as *mut crate::backend::environment::MettaEnvironment);
                    env.add_to_space(&atom_metta);
                    crate::backend::eval::trampoline::dispatch_hints::increment_mutation_epoch();
                    return JitValue::unit().to_bits();
                }
            }
            return super::helpers::make_jit_error(
                "add-atom: no environment for &self/module space",
            );
        }
        handle.add_atom(atom_metta);
        crate::backend::eval::trampoline::dispatch_hints::increment_mutation_epoch();
        return JitValue::unit().to_bits();
    }

    // Path 2 + 3: un-evaluated atom-named space
    if let ValueView::Atom(name) = space_metta.view() {
        if let Some(ctx_ref) = ctx.as_ref() {
            if !ctx_ref.env_ptr.is_null() {
                let env_mut =
                    &mut *(ctx_ref.env_ptr as *mut crate::backend::environment::MettaEnvironment);
                if name == "&self" {
                    env_mut.add_to_space(&atom_metta);
                    crate::backend::eval::trampoline::dispatch_hints::increment_mutation_epoch();
                    return JitValue::unit().to_bits();
                }
                let factory = crate::backend::models::GcFactory::default();
                if let Some(resolved) = env_mut.lookup_token_generic(name, &factory) {
                    if let Some(handle) = resolved.as_space() {
                        if handle.is_module_space() || handle.name == "self" {
                            env_mut.add_to_space(&atom_metta);
                        } else {
                            handle.add_atom(atom_metta);
                        }
                        crate::backend::eval::trampoline::dispatch_hints::increment_mutation_epoch(
                        );
                        return JitValue::unit().to_bits();
                    }
                }
            }
        }
    }

    // Path 4: type error
    super::helpers::make_jit_error("add-atom: expected Space, got non-Space value")
}

/// Remove an atom from a space.
///
/// Stack: [space, atom] -> [Unit]
///
/// Returns Unit per HE / spec §9.2 (the boolean removed-flag is discarded).
/// Mirrors `op_space_remove` in the bytecode VM with the same three-path
/// resolution and `increment_mutation_epoch()` call (Plan A Phase 4 + Bug 4).
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_space_remove(
    ctx: *mut JitContext,
    space: u64,
    atom: u64,
    _ip: u64,
) -> u64 {
    let space_val = JitValue::from_raw(space);
    let atom_val = JitValue::from_raw(atom);

    let space_metta = space_val.to_metta();
    let atom_metta = atom_val.to_metta();

    if let ValueView::Space(handle) = space_metta.view() {
        if handle.is_module_space() || handle.name == "self" {
            if let Some(ctx_ref) = ctx.as_ref() {
                if !ctx_ref.env_ptr.is_null() {
                    let env = &mut *(ctx_ref.env_ptr
                        as *mut crate::backend::environment::MettaEnvironment);
                    env.remove_from_space(&atom_metta);
                    crate::backend::eval::trampoline::dispatch_hints::increment_mutation_epoch();
                    return JitValue::unit().to_bits();
                }
            }
            return super::helpers::make_jit_error(
                "remove-atom: no environment for &self/module space",
            );
        }
        let _ = handle.remove_atom(&atom_metta);
        crate::backend::eval::trampoline::dispatch_hints::increment_mutation_epoch();
        return JitValue::unit().to_bits();
    }

    if let ValueView::Atom(name) = space_metta.view() {
        if let Some(ctx_ref) = ctx.as_ref() {
            if !ctx_ref.env_ptr.is_null() {
                let env_mut =
                    &mut *(ctx_ref.env_ptr as *mut crate::backend::environment::MettaEnvironment);
                if name == "&self" {
                    env_mut.remove_from_space(&atom_metta);
                    crate::backend::eval::trampoline::dispatch_hints::increment_mutation_epoch();
                    return JitValue::unit().to_bits();
                }
                let factory = crate::backend::models::GcFactory::default();
                if let Some(resolved) = env_mut.lookup_token_generic(name, &factory) {
                    if let Some(handle) = resolved.as_space() {
                        if handle.is_module_space() || handle.name == "self" {
                            env_mut.remove_from_space(&atom_metta);
                        } else {
                            let _ = handle.remove_atom(&atom_metta);
                        }
                        crate::backend::eval::trampoline::dispatch_hints::increment_mutation_epoch(
                        );
                        return JitValue::unit().to_bits();
                    }
                }
            }
        }
    }

    super::helpers::make_jit_error("remove-atom: expected Space, got non-Space value")
}

/// Get all atoms from a space
///
/// Stack: [space] -> [SExpr]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `space` - NaN-boxed space handle
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed SExpr containing all atoms in the space
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_space_get_atoms(
    ctx: *mut JitContext,
    space: u64,
    ip: u64,
) -> u64 {
    let space_val = JitValue::from_raw(space);
    let space_metta = space_val.to_metta();

    let atoms: Vec<MettaValue> = match space_metta.view() {
        ValueView::Space(handle) => handle.collapse(),
        ValueView::Atom(name) => {
            // Named space (e.g., &kb) → resolve through environment tokenizer
            if name == "&self" {
                if let Some(ctx_ref) = ctx.as_ref() {
                    if !ctx_ref.env_ptr.is_null() {
                        let env = &*(ctx_ref.env_ptr
                            as *const crate::backend::environment::MettaEnvironment);
                        env.get_all_atoms()
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                }
            } else if let Some(ctx_ref) = ctx.as_ref() {
                if !ctx_ref.env_ptr.is_null() {
                    let env =
                        &*(ctx_ref.env_ptr as *const crate::backend::environment::MettaEnvironment);
                    let factory = crate::backend::models::GcFactory::default();
                    if let Some(resolved) = env.lookup_token_generic(name, &factory) {
                        if let Some(handle) = resolved.as_space() {
                            handle.collapse()
                        } else {
                            Vec::new()
                        }
                    } else {
                        Vec::new()
                    }
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    };

    // Nondeterministic return via JIT choice points
    // (same pattern as jit_runtime_superpose in special_forms.rs)
    if atoms.is_empty() {
        return TAG_UNIT;
    }

    let first_jit = metta_to_jit(&atoms[0]);

    if atoms.len() == 1 {
        return first_jit.to_bits();
    }

    // 2+ atoms: return first, create Value choice points for the rest
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return first_jit.to_bits(),
    };

    let alt_count = atoms.len() - 1;

    // Check choice point capacity
    if ctx_ref.choice_points.is_null() || ctx_ref.choice_point_count >= ctx_ref.choice_point_cap {
        ctx_ref.bailout = true;
        ctx_ref.bailout_reason = JitBailoutReason::NonDeterminism;
        ctx_ref.bailout_ip = ip as usize;
        return first_jit.to_bits();
    }

    if alt_count > MAX_ALTERNATIVES_INLINE {
        // BUG-T0-T2-014 (plan T2/T3.E): when alt_count exceeds the inline
        // choice-point array size (MAX_ALTERNATIVES_INLINE = 16), we bail
        // to the VM tier. The executor at `hybrid/executor.rs:561-578`
        // discards `jit_result` on bailout and transfers `ctx.value_stack`
        // (which does NOT contain `first_jit`) — so the VM resumes from
        // the same IP and re-enumerates all alternatives via T0/T1's
        // native dispatch. No double-yield because the JIT return value
        // is never pushed when `ctx.bailout = true`. Verified by reading
        // the executor flow; first_jit.to_bits() here is effectively a
        // sentinel that is ignored downstream.
        ctx_ref.bailout = true;
        ctx_ref.bailout_reason = JitBailoutReason::NonDeterminism;
        ctx_ref.bailout_ip = ip as usize;
        return first_jit.to_bits();
    }

    // Create choice point with Value alternatives
    let cp = &mut *ctx_ref.choice_points.add(ctx_ref.choice_point_count);
    cp.saved_sp = ctx_ref.sp as u64;
    cp.saved_ip = ip;
    cp.saved_chunk = ctx_ref.current_chunk;
    cp.saved_stack_pool_idx = -1;
    cp.saved_stack_count = 0;
    cp.alt_count = alt_count as u64;
    cp.current_index = 0;
    cp.fork_depth = ctx_ref.fork_depth;
    cp.saved_binding_frames_count = ctx_ref.binding_frames_count;
    cp.is_collect_boundary = false;

    for (i, atom) in atoms[1..].iter().enumerate() {
        cp.alternatives_inline[i] = JitAlternative::value(metta_to_jit(atom));
    }

    ctx_ref.choice_point_count += 1;
    ctx_ref.in_nondet_mode = true;

    first_jit.to_bits()
}

/// Match a pattern against all atoms in a space
///
/// Stack: [space, pattern, template] -> [SExpr]
///
/// # Arguments
/// * `ctx` - JIT context
/// * `space` - NaN-boxed space handle
/// * `pattern` - NaN-boxed pattern to match
/// * `_template` - NaN-boxed template (currently ignored, simplified impl)
/// * `_ip` - Instruction pointer
///
/// # Returns
/// NaN-boxed SExpr containing matching atoms
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_space_match(
    _ctx: *mut JitContext,
    space: u64,
    pattern: u64,
    _template: u64,
    _ip: u64,
) -> u64 {
    let space_val = JitValue::from_raw(space);
    let pattern_val = JitValue::from_raw(pattern);

    let space_metta = space_val.to_metta();
    let pattern_metta = pattern_val.to_metta();

    match space_metta.view() {
        ValueView::Space(handle) => {
            let atoms = handle.collapse();
            let mut results = Vec::new();

            // Simple pattern matching against atoms
            for atom in &atoms {
                if pattern_matches_impl(&pattern_metta, atom) {
                    results.push(atom.clone());
                }
            }

            metta_to_jit(&MettaValue::SExpr(results)).to_bits()
        }
        ValueView::Float(_)
        | ValueView::Bool(_)
        | ValueView::Long(_)
        | ValueView::Unit
        | ValueView::Empty
        | ValueView::NotReducible
        | ValueView::Atom(_)
        | ValueView::String(_)
        | ValueView::SExpr(_)
        | ValueView::Error(_, _)
        | ValueView::Type(_)
        | ValueView::Conjunction(_)
        | ValueView::State(_)
        | ValueView::Memo(_)
        | ValueView::Quoted(_) => {
            // Type error - return empty S-expression
            metta_to_jit(&MettaValue::SExpr(vec![])).to_bits()
        }
    }
}

// =============================================================================
// Space Ops Phase 5: Nondeterministic Space Match
// =============================================================================

/// Nondeterministic space match with choice point creation.
///
/// This function performs pattern matching against all atoms in a space,
/// creating choice points for alternatives when multiple matches exist.
/// It implements the nondeterministic semantics required for MeTTa's `match` form.
///
/// # Arguments
/// * `ctx` - JIT context pointer (mutable for choice point creation)
/// * `space` - NaN-boxed Space value
/// * `pattern` - NaN-boxed pattern expression
/// * `template` - NaN-boxed template expression for result instantiation
/// * `ip` - Instruction pointer for resumption
///
/// # Returns
/// - On single match: NaN-boxed result (template instantiated with bindings)
/// - On multiple matches: First result, with choice points created for rest
/// - On no match: TAG_UNIT
/// - On error: TAG_UNIT with bailout flag set
///
/// # Semantics
/// ```text
/// 0 matches: return nil (empty)
/// 1 match:   return instantiate(template, bindings[0])
/// N matches: return instantiate(template, bindings[0])
///            + create N-1 choice points for alternatives
///            + signal YIELD if in nondet mode
/// ```
///
/// # Safety
/// The context pointer must be valid. The space value must be a valid SpaceHandle.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_space_match_nondet(
    ctx: *mut JitContext,
    space: u64,
    pattern: u64,
    template: u64,
    ip: u64,
) -> u64 {
    let ctx_ref = match ctx.as_mut() {
        Some(c) => c,
        None => return TAG_UNIT,
    };

    let space_val = JitValue::from_raw(space);
    let pattern_val = JitValue::from_raw(pattern);
    let template_val = JitValue::from_raw(template);

    let space_metta = space_val.to_metta();
    let pattern_metta = pattern_val.to_metta();
    let template_metta = template_val.to_metta();

    // Validate we have a space
    let handle = match space_metta.view() {
        ValueView::Space(h) => h,
        ValueView::Float(_)
        | ValueView::Bool(_)
        | ValueView::Long(_)
        | ValueView::Unit
        | ValueView::Empty
        | ValueView::NotReducible
        | ValueView::Atom(_)
        | ValueView::String(_)
        | ValueView::SExpr(_)
        | ValueView::Error(_, _)
        | ValueView::Type(_)
        | ValueView::Conjunction(_)
        | ValueView::State(_)
        | ValueView::Memo(_)
        | ValueView::Quoted(_) => {
            // Type error - not a space
            ctx_ref.bailout = true;
            ctx_ref.bailout_reason = JitBailoutReason::TypeError;
            ctx_ref.bailout_ip = ip as usize;
            return TAG_UNIT;
        }
    };

    // Collapse space to get all atoms
    let atoms = handle.collapse();
    if atoms.is_empty() {
        // No atoms in space - return nil (empty result)
        return TAG_UNIT;
    }

    // Collect matching atoms with their bindings
    let mut matches: Vec<(MettaValue, Vec<(String, MettaValue)>)> = Vec::new();

    for atom in &atoms {
        let mut bindings = Vec::new();
        if pattern_matches_with_bindings_impl(&pattern_metta, atom, &mut bindings) {
            matches.push((atom.clone(), bindings));
        }
    }

    let match_count = matches.len();

    if match_count == 0 {
        // No matches - return nil
        return TAG_UNIT;
    }

    // Take first match
    let (_first_atom, first_bindings) = matches.remove(0);

    // Instantiate template with first bindings
    let first_result = instantiate_template_impl(&template_metta, &first_bindings);
    let first_jit = metta_to_jit(&first_result);

    if match_count == 1 {
        // Single match - just return the result
        return first_jit.to_bits();
    }

    // Multiple matches - create choice point for alternatives 2..N
    // First, check if we have capacity for a choice point
    if ctx_ref.choice_points.is_null() || ctx_ref.choice_point_count >= ctx_ref.choice_point_cap {
        // No choice point capacity - fall back to bailout
        ctx_ref.bailout = true;
        ctx_ref.bailout_reason = JitBailoutReason::NonDeterminism;
        ctx_ref.bailout_ip = ip as usize;
        return first_jit.to_bits();
    }

    // Optimization 5.2: Check if alternatives fit inline
    let alt_count = matches.len();
    if alt_count > MAX_ALTERNATIVES_INLINE {
        // Too many alternatives - bailout to VM
        ctx_ref.bailout = true;
        ctx_ref.bailout_reason = JitBailoutReason::NonDeterminism;
        ctx_ref.bailout_ip = ip as usize;
        return first_jit.to_bits();
    }

    // Get choice point and initialize
    let cp = &mut *ctx_ref.choice_points.add(ctx_ref.choice_point_count);
    cp.saved_sp = ctx_ref.sp as u64;
    cp.alt_count = alt_count as u64;
    cp.current_index = 0;
    cp.saved_ip = ip;
    cp.saved_chunk = ctx_ref.current_chunk;
    cp.saved_stack_pool_idx = -1;
    cp.saved_stack_count = 0;
    cp.fork_depth = ctx_ref.fork_depth;
    cp.saved_binding_frames_count = ctx_ref.binding_frames_count;
    cp.is_collect_boundary = false;

    // For each remaining match, create a SpaceMatch alternative
    for (idx, (_atom, bindings)) in matches.into_iter().enumerate() {
        // Fork current bindings for this alternative
        let saved_bindings = jit_runtime_fork_bindings(ctx as *const JitContext);
        if saved_bindings.is_null() {
            // Fork failed - cleanup previously created alternatives and bailout
            for cleanup_idx in 0..idx {
                let cleanup_alt = &cp.alternatives_inline[cleanup_idx];
                if cleanup_alt.tag == JitAlternativeTag::SpaceMatch && cleanup_alt.payload3 != 0 {
                    jit_runtime_free_saved_bindings(cleanup_alt.payload3 as *mut JitSavedBindings);
                }
            }
            ctx_ref.bailout = true;
            ctx_ref.bailout_reason = JitBailoutReason::NonDeterminism;
            ctx_ref.bailout_ip = ip as usize;
            return first_jit.to_bits();
        }

        // Apply this alternative's bindings to the forked snapshot
        apply_bindings_to_saved(saved_bindings, &bindings);

        // Pre-instantiate the result for this alternative
        let alt_result = instantiate_template_impl(&template_metta, &bindings);
        let alt_jit = metta_to_jit(&alt_result);

        // Optimization 5.2: Store alternative inline
        cp.alternatives_inline[idx] = JitAlternative {
            tag: JitAlternativeTag::SpaceMatch,
            payload: alt_jit.to_bits(), // Pre-computed result
            payload2: 0,                // Unused (bindings already applied)
            payload3: saved_bindings as u64,
        };
    }

    ctx_ref.choice_point_count += 1;

    first_jit.to_bits()
}

/// Helper: Apply bindings to a saved bindings snapshot.
///
/// This stores the bindings from a match operation into the saved frames
/// so they're available when the alternative is taken during backtracking.
unsafe fn apply_bindings_to_saved(saved: *mut JitSavedBindings, bindings: &[(String, MettaValue)]) {
    let saved_ref = match saved.as_mut() {
        Some(s) => s,
        None => return,
    };

    if saved_ref.is_empty() || bindings.is_empty() {
        return;
    }

    // Get the current (last) frame to apply bindings to
    let frame_idx = saved_ref.frames_count.saturating_sub(1);
    let frame = &mut *saved_ref.frames.add(frame_idx);

    // Ensure capacity for new bindings
    let new_count = frame.entries_count + bindings.len();
    if frame.entries.is_null() || new_count > frame.entries_cap {
        let new_cap = new_count.max(8);
        let layout = std::alloc::Layout::array::<JitBindingEntry>(new_cap)
            .expect("Layout calculation failed");
        let new_entries = std::alloc::alloc(layout) as *mut JitBindingEntry;
        if new_entries.is_null() {
            return; // Allocation failed - bindings won't be stored
        }

        // Copy existing entries
        if !frame.entries.is_null() && frame.entries_count > 0 {
            std::ptr::copy_nonoverlapping(frame.entries, new_entries, frame.entries_count);
            if frame.entries_cap > 0 {
                let old_layout = std::alloc::Layout::array::<JitBindingEntry>(frame.entries_cap)
                    .expect("Layout calculation failed");
                std::alloc::dealloc(frame.entries as *mut u8, old_layout);
            }
        }
        frame.entries = new_entries;
        frame.entries_cap = new_cap;
    }

    // Add bindings as entries.
    // T0-T3-010 / Z.A.3 (2026-05-12): full 64-bit FNV-1a hash, no u32
    // truncation. Matches `pattern_matching::hash_var_name`. Eliminates
    // ≥4× collision rate that the prior u32-truncation path produced
    // on PLN's long freshened variable names (`$__fr_E_*`).
    for (name, value) in bindings {
        let name_hash: u64 = {
            const FNV_OFFSET: u64 = 0xcbf29ce484222325;
            const FNV_PRIME: u64 = 0x100000001b3;
            let mut hash = FNV_OFFSET;
            for byte in name.bytes() {
                hash ^= byte as u64;
                hash = hash.wrapping_mul(FNV_PRIME);
            }
            hash
        };

        let entry_ptr = frame.entries.add(frame.entries_count);
        *entry_ptr = JitBindingEntry {
            name_idx: name_hash,
            value: metta_to_jit(value),
        };
        frame.entries_count += 1;
    }
}

/// Pattern matching with binding extraction.
///
/// Like `pattern_matches_impl` but also collects variable bindings.
/// Variables are atoms starting with '$'.
fn pattern_matches_with_bindings_impl(
    pattern: &MettaValue,
    value: &MettaValue,
    bindings: &mut Vec<(String, MettaValue)>,
) -> bool {
    match (pattern.view(), value.view()) {
        // Wildcard - always matches (check BEFORE variable arm — both `_` and `$_`).
        (ValueView::Atom(s), _) if s == "_" || s == "$_" => true,

        // Variable pattern (atom starting with $) - always matches and binds
        (ValueView::Atom(var), _) if var.starts_with('$') => {
            bindings.push((var.to_string(), value.clone()));
            true
        }

        // Same type matching
        (ValueView::Atom(p), ValueView::Atom(v)) => p == v,
        (ValueView::Long(p), ValueView::Long(v)) => p == v,
        (ValueView::Bool(p), ValueView::Bool(v)) => p == v,
        (ValueView::Unit, ValueView::Unit) => true,
        (ValueView::String(p), ValueView::String(v)) => p == v,

        // S-expression matching - recursive with same length
        (ValueView::SExpr(pats), ValueView::SExpr(vals)) => {
            if pats.len() != vals.len() {
                return false;
            }
            for (p, v) in pats.iter().zip(vals.iter()) {
                if !pattern_matches_with_bindings_impl(p, v, bindings) {
                    return false;
                }
            }
            true
        }

        _ => false,
    }
}

/// Instantiate a template expression with bindings.
///
/// Replaces variables in the template with their bound values.
/// Variables are atoms starting with '$'.
fn instantiate_template_impl(
    template: &MettaValue,
    bindings: &[(String, MettaValue)],
) -> MettaValue {
    match template.view() {
        // Variable substitution (atoms starting with $)
        ValueView::Atom(var) if var.starts_with('$') => {
            for (name, value) in bindings {
                if name == var {
                    return value.clone();
                }
            }
            // Unbound variable - keep as-is
            template.clone()
        }

        // S-expression - recurse
        ValueView::SExpr(items) => MettaValue::SExpr(
            items
                .iter()
                .map(|item| instantiate_template_impl(item, bindings))
                .collect(),
        ),

        // Conjunction - recurse
        ValueView::Conjunction(items) => MettaValue::Conjunction(
            items
                .iter()
                .map(|item| instantiate_template_impl(item, bindings))
                .collect(),
        ),

        // All other values pass through unchanged
        _ => template.clone(),
    }
}

/// Resume space match from a SpaceMatch alternative during backtracking.
///
/// This function is called by the backtracking handler when a SpaceMatch
/// choice point is taken. It restores bindings and returns the pre-computed result.
///
/// # Arguments
/// * `ctx` - JIT context pointer
/// * `alt` - Pointer to the JitAlternative being taken
///
/// # Returns
/// The pre-computed result from the alternative's payload.
///
/// # Safety
/// The context and alternative pointers must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_resume_space_match(
    ctx: *mut JitContext,
    alt: *const JitAlternative,
) -> u64 {
    let alt_ref = match alt.as_ref() {
        Some(a) => a,
        None => return TAG_UNIT,
    };

    debug_assert_eq!(alt_ref.tag, JitAlternativeTag::SpaceMatch);

    // Restore bindings if we have a saved snapshot
    if alt_ref.payload3 != 0 {
        let saved = alt_ref.payload3 as *mut JitSavedBindings;
        // Restore and consume the saved bindings
        jit_runtime_restore_bindings(ctx, saved, true);
    }

    // Return the pre-computed result
    alt_ref.payload
}

/// Free saved bindings from SpaceMatch alternatives in a choice point.
///
/// Called when a choice point is exhausted to clean up saved bindings.
/// With Optimization 5.2, alternatives are inline so we don't free the array.
///
/// # Safety
/// The choice point pointer must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_free_space_match_alternatives(cp: *mut JitChoicePoint) {
    let cp_ref = match cp.as_ref() {
        Some(c) => c,
        None => return,
    };

    if cp_ref.alt_count == 0 {
        return;
    }

    // Free any remaining saved bindings in alternatives
    // Optimization 5.2: Alternatives are now inline, so we access alternatives_inline
    for i in cp_ref.current_index..cp_ref.alt_count {
        let alt = &cp_ref.alternatives_inline[i as usize];
        if alt.tag == JitAlternativeTag::SpaceMatch && alt.payload3 != 0 {
            jit_runtime_free_saved_bindings(alt.payload3 as *mut JitSavedBindings);
        }
    }

    // Optimization 5.2: Alternatives are inline, no need to free the array
}
