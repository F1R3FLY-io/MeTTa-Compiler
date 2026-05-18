//! Bytecode Virtual Machine
//!
//! The VM executes compiled bytecode using a stack-based architecture with
//! support for nondeterminism via choice points and backtracking.
//!
//! ## Architecture
//!
//! `GenericBytecodeVM<V, F>` is the generic VM parameterized by value type `V`
//! and factory type `F`. The primary concrete type is:
//!
//! ```text
//! pub type BytecodeVM = GenericBytecodeVM<MettaValue, GcFactory>;
//! ```
//!
//! When `F: Default`, convenience constructors (`new`, `with_config`, `with_env`)
//! are available that use `F::default()` for the factory.
//!
//! ## Submodules
//!
//! - `types`: Core type definitions (VmError, VmConfig, CallFrame, etc.)
//! - `pattern`: Pattern matching helpers

use std::any::TypeId;
use std::collections::HashMap;
use std::fmt;
use std::marker::Unpin;
use std::ops::ControlFlow;
use std::sync::Arc;

use tracing::trace;
use xxhash_rust::xxh3::xxh3_64;

use super::chunk::GenericBytecodeChunk;
use super::external_registry::{ExternalError, GenericExternalContext};
use super::native_registry::GenericNativeContext;
use super::opcodes::Opcode;

use crate::backend::environment::GenericEnvironment;
use crate::backend::eval::bindings::apply_bindings_generic;
use crate::backend::models::{
    numeric_equal_generic, GenericBindings, MettaValue, MettaValueFactory, MettaValueTrait,
    SpaceHandle, ValueView,
};

// === Submodules ===

mod types;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod proptests;

// === Re-exports ===

pub use types::{VmConfig, VmError, VmResult};
// Generic types
pub use types::{
    Alternative, BindingFrame, CallFrame, ChoicePoint, CollapseBindFrame, CollapseFrame,
    GenericAlternative, GenericBindingFrame, GenericCallFrame, GenericChoicePoint,
    GenericCollapseBindFrame, GenericCollapseFrame, TrailEntry, VmBoundValue,
};

// ============================================================================
// VmEvalContext — Lightweight EvalContext Adapter for Trampoline Calls
// ============================================================================

use crate::backend::eval::trampoline::EvalContext;

/// Lightweight `EvalContext` adapter for calling `eval_trampoline`
/// from within the bytecode VM.
///
/// The bytecode VM needs to evaluate sub-expressions during type-driven
/// applicative pre-evaluation (Phase 5). Rather than one-step rule matching,
/// this adapter enables full trampolined, TCO, CPS-based evaluation via the
/// canonical `eval_trampoline` engine.
///
/// # Why not `StaticEvalContext`?
///
/// `StaticEvalContext` is concrete (`MettaValue`, `GcFactory`), but the VM
/// is generic over `V` and `F`. This adapter bridges the gap, allowing the
/// generic VM to use the generic trampoline.
struct VmEvalContext {
    factory: crate::backend::models::GcFactory,
}

impl EvalContext for VmEvalContext {
    #[inline]
    fn factory(&self) -> &crate::backend::models::GcFactory {
        &self.factory
    }

    // should_safepoint / perform_safepoint inherit the EvalContext trait
    // defaults: honor `is_gc_requested()` and run the canonical quiescent
    // protocol so the bytecode VM cooperates with the slab GC.
}

/// Get or create a memo cache for a generic VM instance.
///
/// When `V` is `MettaValue`, returns the global singleton memo cache (which is
/// registered as a GC root provider). For other value types, returns a fresh
/// per-VM cache (no GC registration needed since non-MettaValue types are not
/// slab-allocated).
fn get_or_create_memo_cache<V, F>() -> Arc<super::memo_cache::MemoCache<V>>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + PartialEq + 'static,
    F: MettaValueFactory<V> + Copy + Clone + Send + Sync + 'static,
{
    if TypeId::of::<V>() == TypeId::of::<MettaValue>() {
        // V is MettaValue — use the global singleton (GC-root-registered)
        let global = super::memo_cache::global_memo_cache();
        // SAFETY: We've verified V == MettaValue via TypeId. Arc<MemoCache<MettaValue>>
        // and Arc<MemoCache<V>> have identical layouts when V = MettaValue.
        let global_ref: &Arc<super::memo_cache::MemoCache<MettaValue>> = global;
        let ptr = global_ref as *const Arc<super::memo_cache::MemoCache<MettaValue>>
            as *const Arc<super::memo_cache::MemoCache<V>>;
        unsafe { (*ptr).clone() }
    } else {
        // V is some other type — create a fresh per-VM cache
        Arc::new(super::memo_cache::MemoCache::default())
    }
}

/// Generic bytecode virtual machine that works with any value type.
///
/// This is the generic version of `BytecodeVM` that enables zero-conversion
/// evaluation with both heap-allocated (`MettaValue`) and arena-allocated
/// (`MettaValue`) values.
///
/// # Type Parameters
///
/// - `V`: The value type (must implement `MettaValueTrait`)
/// - `F`: The factory type for constructing values
///
/// # Design
///
/// The generic VM uses:
/// - `GenericBytecodeChunk<V>` for constant storage
/// - `GenericBindingFrame<V>` for pattern variable bindings
/// - `GenericChoicePoint<V, GenericBytecodeChunk<V>>` for nondeterminism
/// - `GenericEnvironment<V, F>` for rule storage
/// - `MettaValueFactory<V>` trait methods for value construction
///
/// This ensures NO conversions are needed between value types during execution.
pub struct GenericBytecodeVM<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + PartialEq + 'static,
    F: MettaValueFactory<V> + Copy + Clone + Send + Sync + 'static,
{
    /// Value stack for operands and results.
    ///
    /// Bug-Fix Phase 1 (2026-04): operands only — local variable slots
    /// moved to the dedicated `locals` vector below so `StoreLocal` /
    /// `LoadLocal` do not alias with operand positions. Previously, the
    /// chunk's `local_count` slots were pre-allocated at the bottom of
    /// `value_stack`, causing `compile_let`'s trailing `Swap; Pop`
    /// cleanup to move the body result INTO the local slot instead of
    /// above it. This made `value_stack.len()` at `CollapseEnd`/
    /// `CollapseBindEnd` equal to the pre-body height, so the barrier
    /// checks missed the body result entirely.
    pub(crate) value_stack: Vec<V>,

    /// Local variable storage, indexed by slot number.
    ///
    /// Bug-Fix Phase 1 (2026-04): dedicated storage separate from
    /// `value_stack`, matching the JIT tier's existing `CodegenContext::locals`
    /// design. `StoreLocal` / `LoadLocal` / `StoreLocalWide` / `LoadLocalWide`
    /// all read and write here; `value_stack` only holds operands. Pre-allocated
    /// to `chunk.local_count()` Unit values at `run()` entry. Choice points
    /// snapshot/restore `locals.len()` (or content) to ensure backtracking
    /// honours scope boundaries.
    pub(crate) locals: Vec<V>,

    /// Base index into `self.locals` for the currently executing chunk.
    ///
    /// Bug-fix 2026-04-follow-up: every `LoadLocal slot` reads
    /// `self.locals[self.locals_base + slot]`; every `StoreLocal slot` writes
    /// there (with resize-on-demand). On call: caller's `locals_base` is saved
    /// into the call frame and `self.locals_base = self.locals.len()` bumps
    /// forward so the callee's slots are allocated beyond the caller's.
    /// On return: `self.locals.truncate(frame.caller_locals_len);
    /// self.locals_base = frame.locals_base` drops callee slots while preserving
    /// caller locals that live above the caller's base.
    /// Mirrors the pre-Phase-1 `value_stack + base_ptr` discipline for the
    /// now-separated locals vector.
    pub(crate) locals_base: usize,

    /// Call stack for function frames
    pub(crate) call_stack: Vec<GenericCallFrame<V, GenericBytecodeChunk<V>>>,

    /// Phase 1b-A: VM-level "current bindings" register. Holds the
    /// bindings active for the most-recently-produced result on the
    /// value stack (or the ambient context when the stack is empty at
    /// an applicative boundary). Mirrors HE's `Bindings` travelling
    /// alongside `InterpretedAtom.Stack`.
    ///
    /// Conceptually: every `Call/TailCall/DispatchRules` path that
    /// evaluates a sub-expression composes that sub-expression's
    /// bindings into this register (via `compose_outer_inner_generic`)
    /// and then applies the composed bindings to remaining arguments
    /// before dispatching further. On backtrack to a
    /// `Alternative::BoundValue` choice point, `current_bindings` is
    /// restored to the alternative's captured bindings.
    ///
    /// Starts empty. Cleared after top-level Return. Saved/restored
    /// around template helper invocations (Phase 1b-B).
    pub(crate) current_bindings: GenericBindings<V>,

    /// Bindings stack for pattern variables
    pub(crate) bindings_stack: Vec<GenericBindingFrame<V>>,

    /// Choice points for nondeterminism
    pub(crate) choice_points: Vec<GenericChoicePoint<V, GenericBytecodeChunk<V>>>,

    /// Collected results (for nondeterministic evaluation)
    pub(crate) results: Vec<V>,

    /// Current instruction pointer
    pub(crate) ip: usize,

    /// Current bytecode chunk
    pub(crate) chunk: Arc<GenericBytecodeChunk<V>>,

    /// VM configuration
    pub(crate) config: VmConfig,

    /// Factory for constructing values
    pub(crate) factory: F,

    /// Optional environment for rule definitions and lookups
    pub(crate) env: Option<GenericEnvironment<V, F>>,

    /// Native function registry for CallNative opcode
    pub(crate) native_registry: Arc<super::native_registry::GenericNativeRegistry<V, F>>,

    /// External function registry for CallExternal opcode
    pub(crate) external_registry: Arc<super::external_registry::GenericExternalRegistry<V, F>>,

    /// Memoization cache for CallCached opcode
    pub(crate) memo_cache: Arc<super::memo_cache::MemoCache<V>>,

    /// Phase 9.2/9.3: Expected return type for the current dispatch, used for branch pruning.
    /// Set before sub-expression evaluation in `vm_type_driven_pre_eval()`, consumed
    /// after `match_rules_native()` to filter out type-incompatible rule matches.
    pub(crate) expected_type: Option<V>,

    /// Phase 8b: Optional runtime type profile for collecting branch/dispatch/guard
    /// feedback during bytecode VM execution. Only active for expressions with
    /// `execution_count >= PROFILING_THRESHOLD` (50+). When active, branch opcodes
    /// record taken/not-taken counts, DispatchRules records rule match frequencies,
    /// and guard opcodes record pass/fail outcomes.
    ///
    /// V8 equivalent: FeedbackVector (per-function IC slot array).
    /// HotSpot equivalent: MethodData (MDO).
    pub(crate) runtime_profile:
        Option<std::sync::Arc<parking_lot::Mutex<super::runtime_profile::RuntimeTypeProfile>>>,

    /// Set to `true` when the VM reaches semantics it cannot finish itself and
    /// tier dispatch must retry in the tree-walker.
    ///
    /// Plain no-rule data constructors are complete normal forms, not fallback
    /// triggers; this flag is reserved for unsupported control paths and failed
    /// function dispatches that need interpreter semantics.
    pub unreduced: bool,

    /// Sticky companion to `unreduced` for `yield_on_top_return`.
    ///
    /// Backtracking restores `unreduced` from each choice point, so the final
    /// flag can be false even after one yielded top-level result was unreduced.
    /// Tier dispatch callers must reject that whole VM run and fall back to the
    /// tree-walker.
    pub had_unreduced_result: bool,

    /// When `true`, top-level Return/chunk-end yields results and backtracks
    /// via `op_fail` instead of breaking, exhausting all nondeterministic
    /// alternatives within a single `run()` call. Set by `eval_inner`.
    /// Only affects top-level returns (no call frame); sub-chunk returns via
    /// call frames are never affected.
    pub(crate) yield_on_top_return: bool,

    /// Collapse frames for nondeterminism sandboxing.
    /// Each `(collapse ...)` pushes a frame; backtracking cannot escape past the barrier.
    pub(crate) collapse_frames: Vec<GenericCollapseFrame<V>>,

    /// Phase C: collapse-bind frames (Task #26). Each `(collapse-bind expr)`
    /// pushes a frame that pairs each nondet result with its current_bindings
    /// snapshot for the sidecar `(value (Bindings …))` encoding.
    pub(crate) collapse_bind_frames: Vec<GenericCollapseBindFrame<V>>,

    /// Phase C: per-result bindings, parallel to `self.results`. When a
    /// `collapse-bind` scope is active, each push to `self.results` also
    /// pushes the current_bindings snapshot here. Swapped in/out by
    /// `op_collapse_bind_begin` / `op_collapse_bind_end`.
    pub(crate) per_result_bindings: Vec<GenericBindings<V>>,

    /// Per-execution dispatch memo for nondeterministic call caching.
    /// When backtracking causes the same expression to be re-dispatched
    /// (Cartesian product scenario like `(op (nd1) (nd2))`), return cached
    /// results instead of re-matching + re-evaluating all RHS bodies.
    /// Key: expression hash via hash_value(). Value: pre-evaluated results.
    pub(crate) dispatch_memo: std::collections::HashMap<u64, (u64, Vec<V>)>,

    /// Trail for fine-grained undo of bindings during backtracking.
    ///
    /// Each entry records a binding that was made (either new or overwrite),
    /// enabling precise undo when `op_fail` backtracks to a choice point.
    /// Complements the coarse `bindings_stack.truncate()` mechanism.
    pub(crate) trail: Vec<TrailEntry<V>>,

    /// Trail mark stack for `TrailMark`/`TrailUndo` opcode pairs.
    ///
    /// `TrailMark` pushes the current trail height; `TrailUndo` pops and
    /// unwinds the trail to that height. Used by compiled unification
    /// sequences to undo partial bindings on match failure.
    pub(crate) trail_marks: Vec<usize>,

    /// Case scrutinee fail barrier frames. When `Fail` fires during a case
    /// scrutinee evaluation, the nearest barrier catches it, restores state,
    /// and jumps to the handler which pushes `Empty`.
    case_barrier_frames: Vec<CaseBarrierFrame>,

    /// Monotonic id used to order nested failure barriers when their
    /// choice-point floors are equal.
    next_barrier_id: u64,

    /// HE runner-mode flag — Plan S0c (2026-05-13).
    ///
    /// `false` (default) = HE `MettaRunnerMode::ADD` — bare top-level S-exprs
    /// are silent side-effecting facts; the VM emits empty result lists.
    ///
    /// `true` = HE `MettaRunnerMode::INTERPRET` — set when compiled bytecode
    /// for a `(! expr)` directive emits `Opcode::EnterInterpretMode` and
    /// cleared on `Opcode::ExitInterpretMode`. Threaded through from the
    /// environment per tier-locality. See spec §S0c.
    pub(crate) interpret_mode: bool,

    /// S2 BANG-WORD / decl-atom dispatch (2026-05-13): true when we're
    /// executing the BODY of a `(! ...)` directive in this VM. Set when
    /// `Opcode::EnterInterpretMode` fires, cleared by
    /// `Opcode::ExitInterpretMode`. Used by `op_define_rule` (and the
    /// future `op_define_type` equivalent for the `:` decl-atom path) to
    /// skip the registration side-effect and push the form's data instead.
    /// See `eval/step/sexpr.rs` for the T0 mirror via
    /// `MettaEnvironment::in_bang_body()`.
    pub(crate) bang_body: bool,

    /// Equivalence-class table — Plan S0d.2 (2026-05-13).
    ///
    /// Lazily allocated when the first user `(unify ...)` form creates a
    /// var-var-distinct equivalence (HE M-VAR-VAR-DISTINCT, spec §4.3.1).
    /// Shared via `Arc` so cross-tier (JIT) reads can pin the table for the
    /// duration of their span (see S0d.3).
    ///
    /// Cleared on `run()` entry to give each top-level invocation a fresh
    /// table. Consulted by `op_push_variable` after ordinary frame-stack
    /// lookup fails — value-less class members yield the original
    /// lookup-key; value-bearing classes yield the class value.
    pub(crate) class_table: Option<std::sync::Arc<crate::backend::models::ClassTable<V>>>,
}

/// Frame for case scrutinee fail barriers.
#[derive(Debug, Clone)]
struct CaseBarrierFrame {
    handler_ip: usize,
    choice_point_floor: usize,
    barrier_id: u64,
    value_stack_height: usize,
    call_stack_height: usize,
    bindings_stack_height: usize,
    saved_unreduced: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FailureBarrier {
    Case,
    Collapse,
}

impl<V, F> fmt::Debug for GenericBytecodeVM<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + PartialEq + fmt::Debug + 'static,
    F: MettaValueFactory<V> + Copy + Clone + Send + Sync + fmt::Debug + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GenericBytecodeVM")
            .field("value_stack_len", &self.value_stack.len())
            .field("call_stack_len", &self.call_stack.len())
            .field("bindings_stack_len", &self.bindings_stack.len())
            .field("choice_points_len", &self.choice_points.len())
            .field("results_len", &self.results.len())
            .field("ip", &self.ip)
            .field("config", &self.config)
            .field("has_env", &self.env.is_some())
            .finish()
    }
}

impl<V, F> GenericBytecodeVM<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + PartialEq + 'static,
    F: MettaValueFactory<V> + Copy + Clone + Send + Sync + 'static,
{
    /// Create a new generic VM with the given chunk and explicit factory.
    pub fn with_factory(chunk: Arc<GenericBytecodeChunk<V>>, factory: F) -> Self {
        Self::with_config_and_factory(chunk, VmConfig::default(), factory)
    }

    /// Create a new generic VM with custom configuration and explicit factory.
    pub fn with_config_and_factory(
        chunk: Arc<GenericBytecodeChunk<V>>,
        config: VmConfig,
        factory: F,
    ) -> Self {
        Self {
            value_stack: Vec::with_capacity(256),
            locals: Vec::new(),
            locals_base: 0,
            call_stack: Vec::with_capacity(64),
            bindings_stack: vec![GenericBindingFrame::new(0)],
            choice_points: Vec::new(),
            results: Vec::new(),
            ip: 0,
            chunk,
            config,
            native_registry: Arc::new(super::native_registry::GenericNativeRegistry::with_stdlib(
                factory.clone(),
            )),
            external_registry: Arc::new(super::external_registry::GenericExternalRegistry::new()),
            memo_cache: get_or_create_memo_cache::<V, F>(),
            factory,
            env: None,
            expected_type: None,
            runtime_profile: None,
            unreduced: false,
            had_unreduced_result: false,
            yield_on_top_return: false,
            collapse_frames: Vec::new(),
            collapse_bind_frames: Vec::new(),
            per_result_bindings: Vec::new(),
            dispatch_memo: std::collections::HashMap::new(),
            trail: Vec::new(),
            trail_marks: Vec::new(),
            case_barrier_frames: Vec::new(),
            next_barrier_id: 0,
            current_bindings: GenericBindings::new(),
            interpret_mode: false,
            // S2 BANG-WORD (2026-05-13): bang_body defaults false; toggled
            // by EnterInterpretMode/ExitInterpretMode opcodes.
            bang_body: false,
            // S0d.2 (2026-05-13): class table lazily allocated on first
            // user `(unify ...)` form that creates an equivalence.
            class_table: None,
        }
    }

    /// Create a new generic VM with an environment and explicit factory.
    pub fn with_env_and_factory(
        chunk: Arc<GenericBytecodeChunk<V>>,
        env: GenericEnvironment<V, F>,
        factory: F,
    ) -> Self {
        // S1 TOPLEVEL (2026-05-13): inherit HE INTERPRET mode from env.
        // When a tier-promoted VM spawns from a trampoline thread that's
        // already inside `(! expr)`, the env carries interpret_mode=true.
        // Without this propagation, the VM's op_dispatch_rules would
        // swallow the rule's reduction under ADD-mode semantics.
        //
        // S2 BANG-WORD (2026-05-13): same propagation for bang_body so
        // op_define_rule sees the correct mode if a tier-promoted VM
        // enters the body of `(! ...)` via env inheritance rather than
        // via the EnterInterpretMode opcode.
        let env_interpret_mode = env.in_interpret_mode();
        let env_bang_body = env.in_bang_body();
        Self {
            value_stack: Vec::with_capacity(256),
            locals: Vec::new(),
            locals_base: 0,
            call_stack: Vec::with_capacity(64),
            bindings_stack: vec![GenericBindingFrame::new(0)],
            choice_points: Vec::new(),
            results: Vec::new(),
            ip: 0,
            chunk,
            config: VmConfig::default(),
            native_registry: Arc::new(super::native_registry::GenericNativeRegistry::with_stdlib(
                factory.clone(),
            )),
            external_registry: Arc::new(super::external_registry::GenericExternalRegistry::new()),
            memo_cache: get_or_create_memo_cache::<V, F>(),
            factory,
            env: Some(env),
            expected_type: None,
            runtime_profile: None,
            unreduced: false,
            had_unreduced_result: false,
            yield_on_top_return: false,
            collapse_frames: Vec::new(),
            collapse_bind_frames: Vec::new(),
            per_result_bindings: Vec::new(),
            dispatch_memo: std::collections::HashMap::new(),
            trail: Vec::new(),
            trail_marks: Vec::new(),
            case_barrier_frames: Vec::new(),
            next_barrier_id: 0,
            current_bindings: GenericBindings::new(),
            interpret_mode: env_interpret_mode,
            bang_body: env_bang_body,
            // S0d.2 (2026-05-13): see field doc on GenericBytecodeVM.
            class_table: None,
        }
    }

    /// Create a new generic VM with full configuration including registries.
    pub fn with_registries(
        chunk: Arc<GenericBytecodeChunk<V>>,
        env: GenericEnvironment<V, F>,
        factory: F,
        native_registry: Arc<super::native_registry::GenericNativeRegistry<V, F>>,
        external_registry: Arc<super::external_registry::GenericExternalRegistry<V, F>>,
        memo_cache: Arc<super::memo_cache::MemoCache<V>>,
    ) -> Self {
        // S1 TOPLEVEL (2026-05-13): inherit HE INTERPRET mode from env.
        // S2 BANG-WORD (2026-05-13): also inherit bang_body for tier-promoted
        // VMs spawning into the middle of a `(! ...)` body.
        let env_interpret_mode = env.in_interpret_mode();
        let env_bang_body = env.in_bang_body();
        Self {
            value_stack: Vec::with_capacity(256),
            locals: Vec::new(),
            locals_base: 0,
            call_stack: Vec::with_capacity(64),
            bindings_stack: vec![GenericBindingFrame::new(0)],
            choice_points: Vec::new(),
            results: Vec::new(),
            ip: 0,
            chunk,
            config: VmConfig::default(),
            factory,
            env: Some(env),
            native_registry,
            external_registry,
            memo_cache,
            expected_type: None,
            runtime_profile: None,
            unreduced: false,
            had_unreduced_result: false,
            yield_on_top_return: false,
            collapse_frames: Vec::new(),
            collapse_bind_frames: Vec::new(),
            per_result_bindings: Vec::new(),
            dispatch_memo: std::collections::HashMap::new(),
            trail: Vec::new(),
            trail_marks: Vec::new(),
            case_barrier_frames: Vec::new(),
            next_barrier_id: 0,
            current_bindings: GenericBindings::new(),
            interpret_mode: env_interpret_mode,
            bang_body: env_bang_body,
            // S0d.2 (2026-05-13): see field doc on GenericBytecodeVM.
            class_table: None,
        }
    }

    fn allocate_failure_barrier_id(&mut self) -> u64 {
        let id = self.next_barrier_id;
        self.next_barrier_id = self.next_barrier_id.wrapping_add(1);
        id
    }

    /// Set the external function registry (builder pattern).
    pub fn with_external_registry(
        mut self,
        registry: Arc<super::external_registry::GenericExternalRegistry<V, F>>,
    ) -> Self {
        self.external_registry = registry;
        self
    }

    /// Set the environment (builder pattern).
    pub fn with_environment(mut self, env: GenericEnvironment<V, F>) -> Self {
        self.env = Some(env);
        self
    }

    /// Set the runtime type profile for collecting execution feedback (builder pattern).
    ///
    /// When set, the VM instruments branch, dispatch, and guard opcodes to record
    /// runtime feedback for profile-guided JIT compilation.
    pub fn with_runtime_profile(
        mut self,
        profile: std::sync::Arc<parking_lot::Mutex<super::runtime_profile::RuntimeTypeProfile>>,
    ) -> Self {
        self.runtime_profile = Some(profile);
        self
    }

    /// Record a branch outcome in the runtime profile (if profiling is active).
    ///
    /// Called at JumpIfFalse, JumpIfTrue, JumpIfNotBool, JumpIfUnit, JumpIfError
    /// opcodes to track taken/not-taken frequencies.
    #[inline]
    fn profile_branch(&self, offset: u32, taken: bool) {
        if let Some(ref profile_arc) = self.runtime_profile {
            let mut profile = profile_arc.lock();
            // Find or create feedback entry for this offset
            let entry = profile
                .branch_frequencies
                .iter()
                .position(|bf| bf.offset == offset);
            match entry {
                Some(idx) => {
                    if taken {
                        profile.branch_frequencies[idx].record_taken();
                    } else {
                        profile.branch_frequencies[idx].record_not_taken();
                    }
                }
                None => {
                    use super::runtime_profile::BranchFeedback;
                    let bf = BranchFeedback::new(offset);
                    if taken {
                        bf.record_taken();
                    } else {
                        bf.record_not_taken();
                    }
                    profile.branch_frequencies.push(bf);
                }
            }
            profile.sample_count += 1;
        }
    }

    /// Record a guard outcome in the runtime profile (if profiling is active).
    #[inline]
    fn profile_guard(&self, offset: u16, passed: bool) {
        if let Some(ref profile_arc) = self.runtime_profile {
            let mut profile = profile_arc.lock();
            let entry = profile
                .guard_outcomes
                .iter_mut()
                .find(|g| g.offset == offset);
            match entry {
                Some(gf) => {
                    if passed {
                        gf.pass_count += 1;
                    } else {
                        gf.fail_count += 1;
                    }
                }
                None => {
                    use super::runtime_profile::GuardFeedback;
                    let gf = GuardFeedback {
                        offset,
                        pass_count: if passed { 1 } else { 0 },
                        fail_count: if passed { 0 } else { 1 },
                    };
                    profile.guard_outcomes.push(gf);
                }
            }
        }
    }

    /// Record a rule match in the runtime profile (if profiling is active).
    #[inline]
    fn profile_rule_match(&self, site_hash: u64, match_count: usize) {
        if let Some(ref profile_arc) = self.runtime_profile {
            let mut profile = profile_arc.lock();
            for rule_index in 0..match_count.min(u16::MAX as usize) {
                let idx = rule_index as u16;
                let entry = profile
                    .rule_match_hits
                    .iter_mut()
                    .find(|r| r.site_hash == site_hash && r.rule_index == idx);
                match entry {
                    Some(rmf) => {
                        rmf.match_count += 1;
                    }
                    None => {
                        use super::runtime_profile::RuleMatchFeedback;
                        profile.rule_match_hits.push(RuleMatchFeedback {
                            site_hash,
                            rule_index: idx,
                            match_count: 1,
                        });
                    }
                }
            }
        }
    }

    /// Push an initial value onto the stack before execution.
    ///
    /// Used for template execution where a binding value needs to be
    /// available as local slot 0.
    #[inline]
    pub fn push_initial_value(&mut self, value: V) {
        self.value_stack.push(value);
    }

    /// Resume VM execution after JIT bailout for non-determinism.
    ///
    /// Allows JIT to compile deterministic parts and then bail out to VM
    /// for Fork/Choice opcodes that require backtracking.
    pub fn resume_from_bailout(
        &mut self,
        bailout_ip: usize,
        value_stack: Vec<V>,
    ) -> VmResult<Vec<V>> {
        self.ip = bailout_ip;
        self.value_stack = value_stack;
        self.run_without_jit()
    }

    /// Run the VM to completion without attempting JIT execution.
    /// Used for resuming after JIT bailout.
    fn run_without_jit(&mut self) -> VmResult<Vec<V>> {
        loop {
            match self.step()? {
                ControlFlow::Continue(()) => continue,
                ControlFlow::Break(results) => return Ok(results),
            }
        }
    }

    /// Get the factory reference.
    #[inline]
    pub fn factory(&self) -> &F {
        &self.factory
    }

    /// Get a reference to the environment, if present.
    #[inline]
    pub fn environment(&self) -> Option<&GenericEnvironment<V, F>> {
        self.env.as_ref()
    }

    /// Take ownership of the environment.
    #[inline]
    pub fn take_environment(&mut self) -> Option<GenericEnvironment<V, F>> {
        self.env.take()
    }

    // === Stack Operations ===

    /// Push a value onto the value stack.
    #[inline]
    pub fn push(&mut self, value: V) {
        self.value_stack.push(value);
    }

    /// Pop a value from the value stack.
    #[inline]
    pub fn pop(&mut self) -> VmResult<V> {
        self.value_stack.pop().ok_or(VmError::StackUnderflow)
    }

    /// Peek at the top value on the stack.
    #[inline]
    pub fn peek(&self) -> VmResult<&V> {
        self.value_stack.last().ok_or(VmError::StackUnderflow)
    }

    /// Peek at a value n positions from the top.
    #[inline]
    pub fn peek_n(&self, n: usize) -> VmResult<&V> {
        let len = self.value_stack.len();
        if n >= len {
            return Err(VmError::StackUnderflow);
        }
        Ok(&self.value_stack[len - 1 - n])
    }

    /// Duplicate the top value.
    #[inline]
    pub fn dup(&mut self) -> VmResult<()> {
        let value = self.peek()?.clone();
        self.push(value);
        Ok(())
    }

    /// Swap the top two values.
    #[inline]
    pub fn swap(&mut self) -> VmResult<()> {
        let len = self.value_stack.len();
        if len < 2 {
            return Err(VmError::StackUnderflow);
        }
        self.value_stack.swap(len - 1, len - 2);
        Ok(())
    }

    // === Bytecode Reading Helpers ===

    /// Read a single byte and advance IP.
    #[inline]
    pub fn read_u8(&mut self) -> VmResult<u8> {
        let byte = self
            .chunk
            .read_byte(self.ip)
            .ok_or(VmError::IpOutOfBounds)?;
        self.ip += 1;
        Ok(byte)
    }

    /// Read a signed byte and advance IP.
    #[inline]
    pub fn read_i8(&mut self) -> VmResult<i8> {
        Ok(self.read_u8()? as i8)
    }

    /// Read a u16 and advance IP.
    #[inline]
    pub fn read_u16(&mut self) -> VmResult<u16> {
        let value = self.chunk.read_u16(self.ip).ok_or(VmError::IpOutOfBounds)?;
        self.ip += 2;
        Ok(value)
    }

    /// Read a signed i16 and advance IP.
    #[inline]
    pub fn read_i16(&mut self) -> VmResult<i16> {
        Ok(self.read_u16()? as i16)
    }

    // === Value Construction Helpers ===

    /// Create a unit value using the factory.
    #[inline]
    pub fn make_unit(&self) -> V {
        self.factory.unit()
    }

    /// Create a boolean value using the factory.
    #[inline]
    pub fn make_bool(&self, b: bool) -> V {
        self.factory.bool(b)
    }

    /// Create a long value using the factory.
    #[inline]
    pub fn make_long(&self, n: i64) -> V {
        self.factory.long(n)
    }

    /// Create a float value using the factory.
    #[inline]
    pub fn make_float(&self, f: f64) -> V {
        self.factory.float(f)
    }

    /// Create an atom value using the factory.
    #[inline]
    pub fn make_atom(&self, name: &str) -> V {
        self.factory.atom(name)
    }

    /// Create a string value using the factory.
    #[inline]
    pub fn make_string(&self, s: &str) -> V {
        self.factory.string(s)
    }

    /// Create an s-expression using the factory.
    #[inline]
    pub fn make_sexpr(&self, items: Vec<V>) -> V {
        self.factory.sexpr(items)
    }

    /// Create an error value using the factory.
    ///
    /// HE-bisimilar shape: the `msg` becomes a `String` detail; the `offending`
    /// value sits in the first slot of `Error(offending, detail)`.
    #[inline]
    pub fn make_error(&self, msg: &str, offending: V) -> V {
        self.factory.error(offending, self.factory.string(msg))
    }

    // === Binding Operations ===

    /// Get a binding by name from the bindings stack.
    pub fn get_binding(&self, name: &str) -> Option<&V> {
        for frame in self.bindings_stack.iter().rev() {
            if let Some(value) = frame.get(name) {
                return Some(value);
            }
        }
        None
    }

    /// Set a binding in the current frame.
    pub fn set_binding(&mut self, name: String, value: V) {
        if let Some(frame) = self.bindings_stack.last_mut() {
            frame.set(name, value);
        }
    }

    // === Equivalence-Class Operations (S0d.2, 2026-05-13) ===

    /// True iff the class table is empty (or absent). Hot-path fast skip for
    /// the ~95% of expressions that never form an equivalence class.
    #[inline]
    pub(crate) fn class_table_is_empty(&self) -> bool {
        self.class_table
            .as_ref()
            .map_or(true, |t| t.is_empty())
    }

    /// Push a new binding frame.
    pub fn push_binding_frame(&mut self) {
        let depth = self.bindings_stack.len() as u32;
        self.bindings_stack.push(GenericBindingFrame::new(depth));
    }

    /// Pop the current binding frame.
    pub fn pop_binding_frame(&mut self) {
        if self.bindings_stack.len() > 1 {
            self.bindings_stack.pop();
        }
    }

    // === Trail Operations ===

    /// Unwind the trail from its current length back to `target_height`,
    /// undoing each binding in reverse order.
    ///
    /// This restores bindings to their state at the time the trail mark was
    /// created, enabling fine-grained undo within surviving binding frames.
    fn unwind_trail(&mut self, target_height: usize) {
        while self.trail.len() > target_height {
            if let Some(entry) = self.trail.pop() {
                match entry {
                    TrailEntry::NewBinding { frame_index, name } => {
                        // Remove the binding from the frame
                        if let Some(frame) = self.bindings_stack.get_mut(frame_index) {
                            frame.remove(&name);
                        }
                    }
                    TrailEntry::Rebinding {
                        frame_index,
                        name,
                        old_value,
                    } => {
                        // Restore the old value
                        if let Some(frame) = self.bindings_stack.get_mut(frame_index) {
                            frame.set(name, old_value);
                        }
                    }
                }
            }
        }
    }

    /// Push a trail mark (saves current trail height for later undo).
    fn trail_mark(&mut self) {
        self.trail_marks.push(self.trail.len());
    }

    /// Pop a trail mark and unwind the trail to that height.
    fn trail_undo(&mut self) {
        if let Some(height) = self.trail_marks.pop() {
            self.unwind_trail(height);
        }
    }

    // === Full Execution ===

    /// Pre-allocate `self.locals` so the currently executing chunk's slot range
    /// `[locals_base, locals_base + chunk.local_count())` is fully backed by
    /// Unit. Called at `run()` entry and at every chunk-switch (call-push and
    /// `op_fail`-style chunk restore).
    ///
    /// Bug-fix 2026-04-follow-up: this is the single source of truth for
    /// "callee's slots are reserved past the caller's", replacing the previous
    /// one-time resize at `run()` entry.
    #[inline]
    fn ensure_locals_for_current_chunk(&mut self) {
        let need = self.locals_base + self.chunk.local_count() as usize;
        if self.locals.len() < need {
            let nil = self.make_unit();
            self.locals.resize(need, nil);
        }
    }

    /// Append every live `V` reachable from this VM's transient state into `out`.
    ///
    /// Plan 2 (2026-05-06): the slab GC cooperative drop protocol requires the
    /// worker to surrender its `EvalGuard` while still holding live slab pointers
    /// in stack/locals/choice-points. Without registering those values as
    /// temporary roots, the next mark-sweep frees the slab slots whose pointers
    /// we still hold, causing UAF on resume.
    ///
    /// Walks every `V`-bearing field of the VM exhaustively:
    /// - current/call/choice/alternative/collapse chunk constants
    /// - `value_stack`, `locals`, `current_bindings`, `bindings_stack`
    /// - `results`, `expected_type`
    /// - `choice_points` and their `alternatives` (5 variant decode arms)
    /// - `call_stack[i].saved_bindings`
    /// - `collapse_frames[i].saved_results`
    /// - `collapse_bind_frames[i].saved_results` + `saved_per_result_bindings`
    /// - `per_result_bindings`
    /// - `dispatch_memo` (HashMap values)
    /// - `trail` `Rebinding.old_value` (skip `NewBinding`)
    ///
    /// Deliberately does NOT walk:
    /// - `env`: registered as `RootProvider` separately via `try_register_env_roots`
    /// - `native_registry`/`external_registry`: hold function pointers, no V values
    /// - `memo_cache`: registered as `MemoCacheRoots` separately
    pub(crate) fn collect_roots_into(&self, out: &mut Vec<V>) {
        // Chunks are immutable, but their constant pools can contain slab
        // values. Transient VM-owned chunks are not necessarily present in the
        // global bytecode cache, so the VM must root their constants directly.
        super::cache::collect_generic_chunk_constants(&self.chunk, out);

        // Direct V-bearing fields
        out.extend(self.value_stack.iter().cloned());
        out.extend(self.locals.iter().cloned());
        out.extend(self.results.iter().cloned());
        if let Some(ref et) = self.expected_type {
            out.push(et.clone());
        }

        // current_bindings: GenericBindings<V>
        for (_scope, _name, val) in self.current_bindings.iter_full() {
            out.push(val.clone());
        }

        // bindings_stack: Vec<GenericBindingFrame<V>>
        for frame in self.bindings_stack.iter() {
            for (_name, val) in frame.iter() {
                out.push(val.clone());
            }
        }

        // call_stack: Vec<GenericCallFrame<V, _>> — only saved_bindings holds V
        for frame in self.call_stack.iter() {
            super::cache::collect_generic_chunk_constants(&frame.return_chunk, out);
            for (_scope, _name, val) in frame.saved_bindings.iter_full() {
                out.push(val.clone());
            }
        }

        // choice_points: Vec<GenericChoicePoint<V, _>> — alternatives + saved_current_bindings
        for cp in self.choice_points.iter() {
            super::cache::collect_generic_chunk_constants(&cp.chunk, out);
            for alt in cp.alternatives.iter() {
                match alt {
                    GenericAlternative::Value(v) => out.push(v.clone()),
                    GenericAlternative::Chunk(chunk) => {
                        super::cache::collect_generic_chunk_constants(chunk, out);
                    }
                    GenericAlternative::Index(_) => {}
                    GenericAlternative::RuleMatch { chunk, bindings } => {
                        super::cache::collect_generic_chunk_constants(chunk, out);
                        for (_scope, _name, val) in bindings.iter_full() {
                            out.push(val.clone());
                        }
                    }
                    GenericAlternative::BoundValue { value, bindings } => {
                        out.push(value.clone());
                        for (_scope, _name, val) in bindings.iter_full() {
                            out.push(val.clone());
                        }
                    }
                }
            }
            for (_scope, _name, val) in cp.saved_current_bindings.iter_full() {
                out.push(val.clone());
            }
        }

        // collapse_frames: saved_results
        for frame in self.collapse_frames.iter() {
            super::cache::collect_generic_chunk_constants(&frame.continuation_chunk, out);
            out.extend(frame.saved_results.iter().cloned());
        }

        // collapse_bind_frames: saved_results + saved_per_result_bindings
        for frame in self.collapse_bind_frames.iter() {
            if let Some(chunk) = &frame.continuation_chunk {
                super::cache::collect_generic_chunk_constants(chunk, out);
            }
            out.extend(frame.saved_results.iter().cloned());
            for bindings in frame.saved_per_result_bindings.iter() {
                for (_scope, _name, val) in bindings.iter_full() {
                    out.push(val.clone());
                }
            }
        }

        // per_result_bindings
        for bindings in self.per_result_bindings.iter() {
            for (_scope, _name, val) in bindings.iter_full() {
                out.push(val.clone());
            }
        }

        // dispatch_memo: HashMap values are (u64, Vec<V>)
        for (_, vs) in self.dispatch_memo.values() {
            out.extend(vs.iter().cloned());
        }

        // trail: only Rebinding entries hold V
        for entry in self.trail.iter() {
            if let TrailEntry::Rebinding { old_value, .. } = entry {
                out.push(old_value.clone());
            }
        }
    }

    /// Plan 2 helper (2026-05-06): on a parallel-branch worker thread under
    /// GC pressure, build the VM's roots into a `Vec<MettaValue>` and call
    /// `worker_cooperative_safepoint`. TypeId-gated so the cost is paid only
    /// for `V == MettaValue`; non-MettaValue monomorphizations dead-code-
    /// eliminate the branch.
    fn run_cooperative_safepoint(&self) {
        use std::any::TypeId;
        if TypeId::of::<V>() != TypeId::of::<MettaValue>() {
            return;
        }
        let mut buf: Vec<V> = Vec::with_capacity(
            self.value_stack.len() + self.locals.len() + self.results.len() + 64,
        );
        self.collect_roots_into(&mut buf);
        // SAFETY: V == MettaValue verified via TypeId; Vec<V> and
        // Vec<MettaValue> have identical layout. Same pattern as
        // `get_or_create_memo_cache` at the top of mod.rs.
        let buf_mv: Vec<MettaValue> =
            unsafe { std::mem::transmute::<Vec<V>, Vec<MettaValue>>(buf) };
        crate::backend::eval::trampoline::eval_loop::worker_cooperative_safepoint(&buf_mv);
    }

    /// Build an Error atom from a runtime-data VmError per spec K T1.A
    /// (errors-as-values). The resulting value is pushed onto the operand
    /// stack instead of bubbling up as a Rust `Err`. Used by the dispatch
    /// loop's error interceptor.
    fn materialize_runtime_error_atom(&self, err: &VmError) -> V {
        let (msg, kind) = err.as_error_strings();
        let offending = self.factory.atom(kind);
        self.factory.error(offending, self.factory.string(&msg))
    }

    /// Run the VM to completion, returning all results.
    ///
    /// This is the complete generic implementation that handles all opcodes
    /// using trait methods for value construction and inspection. NO conversions
    /// between value types occur during execution.
    pub fn run(&mut self) -> VmResult<Vec<V>> {
        // Bug-Fix Phase 1 (2026-04): Pre-allocate local-variable slots in the
        // dedicated `locals` storage. Previously at the bottom of `value_stack`,
        // which aliased with operand positions and broke collapse-barrier checks.
        //
        // Bug-fix 2026-04-follow-up: pre-allocation is keyed off `locals_base` so
        // that nested `run()` re-entries (if any) and the initial chunk both
        // correctly reserve only the slots they need, at the right offset.
        self.ensure_locals_for_current_chunk();

        // S0d.2 (2026-05-13): give each top-level invocation a fresh
        // equivalence-class table. The table is populated by user
        // `(unify ...)` forms (op_unify_bind / op_unify_deep /
        // op_unify_deep_bind) and consulted by op_push_variable.
        self.class_table = None;

        // Plan 2 (2026-05-06): periodic cooperative GC safepoint for parallel-
        // branch workers in the bytecode VM tier. Every 256 instructions, check
        // `IS_PARALLEL_WORKER && is_gc_requested()` and surrender the EvalGuard
        // (with all live VM state registered as roots via `collect_roots_into`)
        // so quiescence-driven GC can fire. Hot-path cost: 1 TLS load + 1
        // Relaxed atomic load + branch (~2 cycles per 256-iter slot).
        let mut iter_counter: u32 = 0;
        loop {
            iter_counter = iter_counter.wrapping_add(1);
            if iter_counter & 0xFF == 0 {
                let is_worker = crate::backend::eval::trampoline::eval_loop::IS_PARALLEL_WORKER
                    .with(|f| f.get());
                if is_worker && crate::backend::models::gc_allocator::is_gc_requested() {
                    self.run_cooperative_safepoint();
                }
            }
            // BUG-T0-T1-001/011 (errors-as-values, plan T1.A):
            // Convert runtime-data errors (DivisionByZero, TypeError,
            // ArithmeticOverflow) into Error atoms pushed onto the value
            // stack instead of propagating them as VM errors. This matches
            // T0's HE-aligned behavior: errors are first-class values that
            // downstream pattern-matching / dispatch can consume. Genuine
            // internal-VM failures (StackUnderflow, IpOutOfBounds, etc.)
            // still propagate.
            match self.step() {
                Ok(ControlFlow::Continue(())) => continue,
                Ok(ControlFlow::Break(results)) => return Ok(results),
                Err(err) if err.is_runtime_data_error() => {
                    let error_atom = self.materialize_runtime_error_atom(&err);
                    self.push(error_atom);
                    continue;
                }
                Err(err) => return Err(err),
            }
        }
    }

    /// Run the VM with environment, returning results and modified environment.
    pub fn run_with_env(&mut self) -> VmResult<(Vec<V>, Option<GenericEnvironment<V, F>>)> {
        let results = self.run()?;
        let env = self.env.take();
        Ok((results, env))
    }

    /// Exhaust all remaining choice points by repeatedly backtracking and
    /// re-running the VM. Each alternative is executed through the full
    /// chunk instruction sequence, producing one result set per alternative.
    ///
    /// Called from `eval_inner` when `choice_points_len() > 0` after the
    /// initial `run()` completes, to handle nondeterministic rule dispatch
    /// entirely within the bytecode VM.
    pub fn resume_alternatives(&mut self) -> VmResult<Vec<V>> {
        let mut all_results = Vec::new();
        while !self.choice_points.is_empty() {
            match self.op_fail()? {
                ControlFlow::Continue(()) => {
                    let results = self.run()?;
                    all_results.extend(results);
                }
                ControlFlow::Break(final_results) => {
                    all_results.extend(final_results);
                    break;
                }
            }
        }
        Ok(all_results)
    }

    /// Execute a single instruction.
    pub fn step(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        // Bounds check
        if self.ip >= self.chunk.len() {
            return self.handle_chunk_end();
        }

        // Read opcode
        let opcode_byte = self.read_u8()?;
        let opcode = Opcode::from_byte(opcode_byte).ok_or(VmError::InvalidOpcode(opcode_byte))?;

        // Execute opcode
        match opcode {
            // === Stack Operations ===
            Opcode::Nop => {}
            Opcode::Pop => {
                self.pop()?;
            }
            Opcode::Dup => self.dup()?,
            Opcode::Swap => self.swap()?,
            Opcode::Rot3 => self.op_rot3()?,
            Opcode::Over => self.op_over()?,
            Opcode::DupN => self.op_dup_n()?,
            Opcode::PopN => self.op_pop_n()?,

            // === Value Creation ===
            Opcode::PushUnit => self.push(self.make_unit()),
            Opcode::PushTrue => self.push(self.make_bool(true)),
            Opcode::PushFalse => self.push(self.make_bool(false)),
            Opcode::PushEmpty => {
                let empty = self.make_sexpr(vec![]);
                self.push(empty);
            }
            Opcode::PushLongSmall => {
                let n = self.read_i8()? as i64;
                self.push(self.make_long(n));
            }
            Opcode::PushLong
            | Opcode::PushString
            | Opcode::PushUri
            | Opcode::PushConstant => {
                let index = self.read_u16()?;
                let value = self
                    .chunk
                    .get_constant(index)
                    .ok_or(VmError::InvalidConstant(index))?
                    .clone();
                self.push(value);
            }
            Opcode::PushAtom => {
                let index = self.read_u16()?;
                let value = self
                    .chunk
                    .get_constant(index)
                    .ok_or(VmError::InvalidConstant(index))?
                    .clone();
                // Y.4 (2026-05-12): if this atom is bound as a token in the
                // environment (e.g. via `(bind! x-val 100)`), resolve to the
                // bound value, mirroring T0's eval at `step/step.rs:130`.
                // Variables starting with `$` are handled by PushVariable, so
                // PushAtom always sees non-variable symbols here. Lookup miss
                // returns None and we push the literal atom.
                let to_push = match (self.env.as_ref(), value.as_atom()) {
                    (Some(env), Some(name)) => env
                        .lookup_token_generic(name, &self.factory)
                        .unwrap_or(value),
                    _ => value,
                };
                self.push(to_push);
            }
            Opcode::PushVariable => self.op_push_variable()?,
            Opcode::MakeSExpr => {
                let arity = self.read_u8()? as usize;
                let len = self.value_stack.len();
                if arity > len {
                    return Err(VmError::StackUnderflow);
                }
                let items: Vec<V> = self.value_stack.drain((len - arity)..).collect();
                self.push(self.make_sexpr(items));
            }
            Opcode::MakeSExprLarge => {
                let arity = self.read_u16()? as usize;
                let len = self.value_stack.len();
                if arity > len {
                    return Err(VmError::StackUnderflow);
                }
                let items: Vec<V> = self.value_stack.drain((len - arity)..).collect();
                self.push(self.make_sexpr(items));
            }
            Opcode::MakeList => self.op_make_list()?,
            Opcode::MakeQuote => self.op_make_quote()?,

            // === Variable Operations ===
            // Bug-Fix Phase 1 (2026-04): Local variables live in `self.locals`,
            // a dedicated storage vector disjoint from `value_stack`. This mirrors
            // the JIT tier's `CodegenContext::locals` design and eliminates the
            // operand/local aliasing that made `CollapseEnd`/`CollapseBindEnd`
            // barrier checks miss body results for `(collapse (let ...))` forms.
            //
            // Bug-fix 2026-04-follow-up: slot indices are CHUNK-RELATIVE (each chunk's
            // compiler assigns `next_local` starting at 0). `self.locals_base` is the
            // absolute offset for the currently executing chunk; caller's locals sit
            // at `[0, locals_base)` and are preserved across the call.
            Opcode::LoadLocal => {
                let slot = self.read_u8()? as usize;
                let abs = self.locals_base + slot;
                let value = self
                    .locals
                    .get(abs)
                    .ok_or(VmError::InvalidLocal(slot as u16))?
                    .clone();
                self.push(value);
            }
            Opcode::StoreLocal => {
                let slot = self.read_u8()? as usize;
                let value = self.pop()?;
                let abs = self.locals_base + slot;
                if abs >= self.locals.len() {
                    self.locals.resize(abs + 1, self.make_unit());
                }
                self.locals[abs] = value;
            }
            Opcode::LoadLocalWide => {
                let slot = self.read_u16()? as usize;
                let abs = self.locals_base + slot;
                let value = self
                    .locals
                    .get(abs)
                    .ok_or(VmError::InvalidLocal(slot as u16))?
                    .clone();
                self.push(value);
            }
            Opcode::StoreLocalWide => {
                let slot = self.read_u16()? as usize;
                let value = self.pop()?;
                let abs = self.locals_base + slot;
                if abs >= self.locals.len() {
                    self.locals.resize(abs + 1, self.make_unit());
                }
                self.locals[abs] = value;
            }
            Opcode::LoadBinding => {
                let index = self.read_u16()?;
                let name = self
                    .chunk
                    .get_constant(index)
                    .and_then(|v| v.as_atom().map(|s| s.to_string()))
                    .ok_or(VmError::InvalidConstant(index))?;
                // Search bindings from innermost to outermost
                if let Some(value) = self.get_binding(&name).cloned() {
                    self.push(value);
                } else {
                    return Err(VmError::InvalidBinding(name));
                }
            }
            Opcode::StoreBinding => {
                let index = self.read_u16()?;
                let name = self
                    .chunk
                    .get_constant(index)
                    .and_then(|v| v.as_atom().map(|s| s.to_string()))
                    .ok_or(VmError::InvalidConstant(index))?;
                let value = self.pop()?;
                self.set_binding(name, value);
            }
            Opcode::HasBinding => {
                let index = self.read_u16()?;
                let name = self
                    .chunk
                    .get_constant(index)
                    .and_then(|v| v.as_atom().map(|s| s.to_string()))
                    .ok_or(VmError::InvalidConstant(index))?;
                let has = self.get_binding(&name).is_some();
                self.push(self.make_bool(has));
            }
            Opcode::ClearBindings => {
                if let Some(frame) = self.bindings_stack.last_mut() {
                    frame.clear();
                }
            }
            Opcode::PushBindingFrame => self.push_binding_frame(),
            Opcode::PopBindingFrame => {
                if self.bindings_stack.len() <= 1 {
                    return Err(VmError::Runtime("Cannot pop root binding frame".into()));
                }
                self.pop_binding_frame();
            }
            Opcode::LoadUpvalue => {
                // Upvalues are stored as constants in parent scopes
                let index = self.read_u16()?;
                let value = self
                    .chunk
                    .get_constant(index)
                    .ok_or(VmError::InvalidConstant(index))?
                    .clone();
                self.push(value);
            }

            // === Control Flow ===
            Opcode::Jump => {
                let offset = self.read_i16()?;
                self.ip = (self.ip as isize + offset as isize) as usize;
            }
            Opcode::JumpIfFalse => {
                let branch_offset = self.ip as u32;
                let offset = self.read_i16()?;
                let cond = self.pop()?;
                // MeTTa HE: only Bool(false) is falsy. Unit is NOT falsy —
                // it falls through to JumpIfNotBool which returns unreduced.
                let taken = matches!(cond.view(), ValueView::Bool(false));
                if taken {
                    self.ip = (self.ip as isize + offset as isize) as usize;
                }
                self.profile_branch(branch_offset, taken);
            }
            Opcode::JumpIfTrue => {
                let offset = self.read_i16()?;
                let cond = self.pop()?;
                if matches!(cond.view(), ValueView::Bool(true)) {
                    self.ip = (self.ip as isize + offset as isize) as usize;
                }
            }
            // JumpIfIdentical: pops two values, jumps if they are PartialEq equal.
            // Used for native if-reducible: compares evaluated result to original.
            Opcode::JumpIfIdentical => {
                let offset = self.read_i16()?;
                let b = self.pop()?; // original (unevaluated)
                let a = self.pop()?; // result (evaluated)
                if a == b {
                    // PartialEq — exact match including variable names
                    self.ip = (self.ip as isize + offset as isize) as usize;
                }
            }
            Opcode::JumpIfUnit => {
                let offset = self.read_i16()?;
                let value = self.pop()?;
                if matches!(value.view(), ValueView::Unit) {
                    self.ip = (self.ip as isize + offset as isize) as usize;
                }
            }
            Opcode::JumpIfError => {
                let offset = self.read_i16()?;
                let value = self.peek()?;
                if matches!(value.view(), ValueView::Error(..)) {
                    self.ip = (self.ip as isize + offset as isize) as usize;
                }
            }
            Opcode::JumpIfNotBool => {
                // MeTTa HE: if condition is not Bool, jump to non-bool handler
                // to return unreduced (if cond then else). Peek, not pop.
                let branch_offset = self.ip as u32;
                let offset = self.read_i16()?;
                let value = self.peek()?;
                let taken = !matches!(value.view(), ValueView::Bool(_));
                if taken {
                    self.ip = (self.ip as isize + offset as isize) as usize;
                }
                self.profile_branch(branch_offset, taken);
            }
            Opcode::JumpShort => {
                let offset = self.read_i8()?;
                self.ip = (self.ip as isize + offset as isize) as usize;
            }
            Opcode::JumpIfFalseShort => {
                let offset = self.read_i8()?;
                let cond = self.pop()?;
                // MeTTa HE: only Bool(false) is falsy (consistent with JumpIfFalse).
                if matches!(cond.view(), ValueView::Bool(false)) {
                    self.ip = (self.ip as isize + offset as isize) as usize;
                }
            }
            Opcode::JumpIfTrueShort => {
                let offset = self.read_i8()?;
                let cond = self.pop()?;
                if matches!(cond.view(), ValueView::Bool(true)) {
                    self.ip = (self.ip as isize + offset as isize) as usize;
                }
            }
            Opcode::JumpTable => self.op_jump_table()?,
            Opcode::Call => self.op_call()?,
            Opcode::TailCall => self.op_tail_call()?,
            Opcode::CallN => self.op_call_n()?,
            Opcode::TailCallN => self.op_tail_call_n()?,
            Opcode::Return => return self.op_return(),
            Opcode::ReturnMulti => return self.op_return_multi(),

            // === Arithmetic ===
            Opcode::Add => self.op_binary_num("+", |a, b| a.wrapping_add(b), |a, b| a + b)?,
            Opcode::Sub => self.op_binary_num("-", |a, b| a.wrapping_sub(b), |a, b| a - b)?,
            Opcode::Mul => self.op_binary_num("*", |a, b| a.wrapping_mul(b), |a, b| a * b)?,
            Opcode::Div => {
                // Spec §13.2: integer / 0 → DivisionByZero; otherwise wrapping_div
                // (so i64::MIN / -1 wraps to i64::MIN, no SIGFPE/error).
                // Float / 0.0 → IEEE 754 (±Inf or NaN), no error.
                let b = self.pop()?;
                let a = self.pop()?;
                // BUG-T0-T1-003: Empty annihilation per spec §14.1.1 Ext-3.
                // X.4 MTT-EMPTY-ANNIHILATION: recognise the literal `Empty`
                // symbol in addition to the Empty sentinel (HE
                // interpret_tuple return_on_error semantics).
                if a.is_empty() || b.is_empty()
                    || a.as_atom() == Some("Empty") || b.as_atom() == Some("Empty") {
                    self.push(self.factory.empty());
                } else {
                match (a.as_long(), b.as_long()) {
                    (Some(_), Some(0)) => {
                        // ERR-shape align (2026-05-16): push HE-aligned
                        // `(Error (/ a b) DivisionByZero)` and continue VM.
                        // Previously returned `Err(DivisionByZero)` whose
                        // shape via `materialize_runtime_error_atom` was
                        // inverted (`(Error DivisionByZero "Division by
                        // zero")`).
                        let call = self.make_sexpr(vec![
                            self.make_atom("/"),
                            a.clone(),
                            b.clone(),
                        ]);
                        let err = self.factory.error(call, self.make_atom("DivisionByZero"));
                        self.push(err);
                    }
                    (Some(x), Some(y)) => self.push(self.make_long(x.wrapping_div(y))),
                    _ => match (a.as_float(), b.as_float()) {
                        (Some(x), Some(y)) => self.push(self.make_float(x / y)),
                        _ => {
                            // Mixed Long/Float type promotion → IEEE 754
                            match (a.as_long(), b.as_float()) {
                                (Some(x), Some(y)) => self.push(self.make_float(x as f64 / y)),
                                _ => match (a.as_float(), b.as_long()) {
                                    (Some(x), Some(y)) => self.push(self.make_float(x / y as f64)),
                                    _ => {
                                        return Err(VmError::TypeError {
                                            expected: "number",
                                            got: "other",
                                        })
                                    }
                                },
                            }
                        }
                    },
                }
                }
            }
            Opcode::Mod => {
                // Spec §13.2: integer % 0 → DivisionByZero; otherwise wrapping_rem
                // (so i64::MIN % -1 wraps to 0). Float % 0.0 → NaN per IEEE/HE.
                let b = self.pop()?;
                let a = self.pop()?;
                // BUG-T0-T1-003: Empty annihilation per spec §14.1.1 Ext-3.
                // X.4 MTT-EMPTY-ANNIHILATION: recognise the literal `Empty`
                // symbol in addition to the Empty sentinel (HE
                // interpret_tuple return_on_error semantics).
                if a.is_empty() || b.is_empty()
                    || a.as_atom() == Some("Empty") || b.as_atom() == Some("Empty") {
                    self.push(self.factory.empty());
                } else {
                    match (a.as_long(), a.as_float(), b.as_long(), b.as_float()) {
                        (Some(_), _, Some(0), _) => {
                            // ERR-shape align (2026-05-16): same HE shape
                            // as `/` — `(Error (% a b) DivisionByZero)`.
                            let call = self.make_sexpr(vec![
                                self.make_atom("%"),
                                a.clone(),
                                b.clone(),
                            ]);
                            let err = self.factory.error(call, self.make_atom("DivisionByZero"));
                            self.push(err);
                        }
                        (Some(x), _, Some(y), _) => self.push(self.make_long(x.wrapping_rem(y))),
                        (_, Some(x), _, Some(y)) => self.push(self.make_float(x % y)),
                        (Some(x), _, _, Some(y)) => self.push(self.make_float(x as f64 % y)),
                        (_, Some(x), Some(y), _) => self.push(self.make_float(x % y as f64)),
                        _ => {
                            return Err(VmError::TypeError {
                                expected: "number",
                                got: "other",
                            })
                        }
                    }
                }
            }
            Opcode::Neg => {
                // Spec §13.2: unary minus wraps; -(i64::MIN) → i64::MIN.
                let a = self.pop()?;
                if let Some(x) = a.as_long() {
                    self.push(self.make_long(x.wrapping_neg()));
                } else if let Some(x) = a.as_float() {
                    self.push(self.make_float(-x));
                } else {
                    return Err(VmError::TypeError {
                        expected: "number",
                        got: "other",
                    });
                }
            }
            Opcode::Abs => {
                // Spec is silent on abs(i64::MIN); choose wrapping_abs for tier-consistency
                // with trampoline + JIT: abs(i64::MIN) → i64::MIN (mathematically negative,
                // but bit-pattern matches i64::MIN), no error.
                let a = self.pop()?;
                if let Some(x) = a.as_long() {
                    self.push(self.make_long(x.wrapping_abs()));
                } else if let Some(x) = a.as_float() {
                    self.push(self.make_float(x.abs()));
                } else {
                    return Err(VmError::TypeError {
                        expected: "number",
                        got: "other",
                    });
                }
            }
            Opcode::FloorDiv => {
                // Spec §13.2: floor-div wraps on overflow per §C.7g.
                // wrapping_div_euclid handles i64::MIN.div_euclid(-1) → i64::MIN cleanly
                // in both debug and release builds.
                let b = self.pop()?;
                let a = self.pop()?;
                // ERR-shape align (2026-05-16): closure to push the HE
                // `(Error (// a b) DivisionByZero)` form. Inlined to keep
                // the operand context intact across all four 0-divisor
                // arms.
                let push_div_by_zero = |vm: &mut Self| {
                    let call = vm.make_sexpr(vec![
                        vm.make_atom("//"),
                        a.clone(),
                        b.clone(),
                    ]);
                    let err = vm.factory.error(call, vm.make_atom("DivisionByZero"));
                    vm.push(err);
                };
                match (a.as_long(), b.as_long()) {
                    (Some(_), Some(0)) => {
                        push_div_by_zero(self);
                    }
                    (Some(x), Some(y)) => {
                        self.push(self.make_long(x.wrapping_div_euclid(y)));
                    }
                    _ => match (a.as_float(), b.as_float()) {
                        (Some(x), Some(y)) if y != 0.0 => {
                            self.push(self.make_long((x / y).floor() as i64));
                        }
                        (Some(_), Some(_)) => {
                            push_div_by_zero(self);
                        }
                        _ => {
                            // Mixed Long/Float type promotion
                            match (a.as_long(), b.as_float()) {
                                (Some(x), Some(y)) => {
                                    if y == 0.0 {
                                        push_div_by_zero(self);
                                    } else {
                                        self.push(
                                            self.make_long((x as f64 / y).floor() as i64),
                                        );
                                    }
                                }
                                _ => match (a.as_float(), b.as_long()) {
                                    (Some(x), Some(y)) => {
                                        if y == 0 {
                                            push_div_by_zero(self);
                                        } else {
                                            self.push(
                                                self.make_long((x / y as f64).floor() as i64),
                                            );
                                        }
                                    }
                                    _ => {
                                        return Err(VmError::TypeError {
                                            expected: "number",
                                            got: "other",
                                        })
                                    }
                                },
                            }
                        }
                    },
                }
            }
            Opcode::Pow => {
                // Spec §13.2: integer pow wraps on overflow.
                // i64::wrapping_pow handles overflow without panicking.
                // `pow` (short-name) preserves Long×Long → Long.
                let b = self.pop()?;
                let a = self.pop()?;
                // BUG-T0-T1-003: Empty annihilation per spec §14.1.1 Ext-3.
                // X.4 MTT-EMPTY-ANNIHILATION: recognise the literal `Empty`
                // symbol in addition to the Empty sentinel (HE
                // interpret_tuple return_on_error semantics).
                if a.is_empty() || b.is_empty()
                    || a.as_atom() == Some("Empty") || b.as_atom() == Some("Empty") {
                    self.push(self.factory.empty());
                } else {
                    match (a.as_long(), b.as_long()) {
                        (Some(x), Some(y)) if y >= 0 => {
                            self.push(self.make_long(x.wrapping_pow(y as u32)));
                        }
                        _ => match (a.as_float(), b.as_float()) {
                            (Some(x), Some(y)) => self.push(self.make_float(x.powf(y))),
                            _ => match (a.as_long(), b.as_float()) {
                                (Some(x), Some(y)) => {
                                    self.push(self.make_float((x as f64).powf(y)))
                                }
                                _ => match (a.as_float(), b.as_long()) {
                                    (Some(x), Some(y)) => {
                                        self.push(self.make_float(x.powi(y as i32)))
                                    }
                                    _ => {
                                        return Err(VmError::TypeError {
                                            expected: "number (Long or Float)",
                                            got: "other",
                                        })
                                    }
                                },
                            },
                        },
                    }
                }
            }
            Opcode::PowMath => {
                // HE-aligned `pow-math` (lib/src/metta/runner/stdlib/math.rs:21-37):
                // always promote both operands to f64 and return Float.
                // Even Long×Long inputs (e.g. `(pow-math 2 10)`) yield
                // Float(1024.0), not Long(1024).
                let b = self.pop()?;
                let a = self.pop()?;
                if a.is_empty() || b.is_empty()
                    || a.as_atom() == Some("Empty") || b.as_atom() == Some("Empty") {
                    self.push(self.factory.empty());
                } else {
                    // Promote both to f64; powf for Float exp, powi for Long exp.
                    let base = match a.as_float() {
                        Some(x) => x,
                        None => match a.as_long() {
                            Some(x) => x as f64,
                            None => {
                                return Err(VmError::TypeError {
                                    expected: "number (Long or Float)",
                                    got: "other",
                                })
                            }
                        },
                    };
                    // Exponent: use powi for Long (matches HE try_into::<i32>),
                    // powf for Float. Result is always Float.
                    let result = match b.as_long() {
                        Some(y) => match i32::try_from(y) {
                            Ok(y_i32) => base.powi(y_i32),
                            Err(_) => base.powf(y as f64),
                        },
                        None => match b.as_float() {
                            Some(y) => base.powf(y),
                            None => {
                                return Err(VmError::TypeError {
                                    expected: "number (Long or Float)",
                                    got: "other",
                                })
                            }
                        },
                    };
                    self.push(self.make_float(result));
                }
            }
            Opcode::Sqrt => {
                let a = self.pop()?;
                if let Some(x) = a.as_float() {
                    self.push(self.make_float(x.sqrt()));
                } else if let Some(x) = a.as_long() {
                    self.push(self.make_float((x as f64).sqrt()));
                } else {
                    return Err(VmError::TypeError {
                        expected: "number",
                        got: "other",
                    });
                }
            }
            Opcode::Log => {
                // Binary log(base, value) - stack: [base, value] -> pops value first, then base
                let value = self.pop()?;
                let base = self.pop()?;
                match (base.as_float(), value.as_float()) {
                    (Some(b), Some(v)) => self.push(self.make_float(v.log(b))),
                    _ => match (base.as_long(), value.as_float()) {
                        (Some(b), Some(v)) => self.push(self.make_float(v.log(b as f64))),
                        _ => match (base.as_float(), value.as_long()) {
                            (Some(b), Some(v)) => self.push(self.make_float((v as f64).log(b))),
                            _ => match (base.as_long(), value.as_long()) {
                                (Some(b), Some(v)) => {
                                    self.push(self.make_float((v as f64).log(b as f64)))
                                }
                                _ => {
                                    return Err(VmError::TypeError {
                                        expected: "Float or Long",
                                        got: "other",
                                    })
                                }
                            },
                        },
                    },
                }
            }
            Opcode::Trunc => {
                let a = self.pop()?;
                match a.view() {
                    ValueView::Float(x) => self.push(self.make_long(x.trunc() as i64)),
                    ValueView::Long(_) => self.push(a),
                    _ => {
                        return Err(VmError::TypeError {
                            expected: "number",
                            got: "other",
                        })
                    }
                }
            }
            Opcode::Ceil => {
                // HE-aligned `ceil-math` (lib/src/metta/runner/stdlib/math.rs:153-162):
                // Float(f) → Float(f.ceil()). For Long input, we deliberately
                // promote to Float to keep return type uniform (MTT spec: *-math
                // ops return Float). Conformance fixture T06/070 expects "4.0".
                let a = self.pop()?;
                match a.view() {
                    ValueView::Float(x) => self.push(self.make_float(x.ceil())),
                    ValueView::Long(n) => self.push(self.make_float(n as f64)),
                    _ => {
                        return Err(VmError::TypeError {
                            expected: "Float or Long",
                            got: "other",
                        })
                    }
                }
            }
            Opcode::FloorMath => {
                // HE-aligned `floor-math`. See Ceil note above. Conformance
                // fixture T06/069 expects "3.0".
                let a = self.pop()?;
                match a.view() {
                    ValueView::Float(x) => self.push(self.make_float(x.floor())),
                    ValueView::Long(n) => self.push(self.make_float(n as f64)),
                    _ => {
                        return Err(VmError::TypeError {
                            expected: "Float or Long",
                            got: "other",
                        })
                    }
                }
            }
            Opcode::Round => {
                // HE-aligned `round-math`. See Ceil note above. Conformance
                // fixture T06/071 expects "4.0".
                let a = self.pop()?;
                match a.view() {
                    ValueView::Float(x) => self.push(self.make_float(x.round())),
                    ValueView::Long(n) => self.push(self.make_float(n as f64)),
                    _ => {
                        return Err(VmError::TypeError {
                            expected: "Float or Long",
                            got: "other",
                        })
                    }
                }
            }
            Opcode::Sin => self.op_unary_float(f64::sin)?,
            Opcode::Cos => self.op_unary_float(f64::cos)?,
            Opcode::Tan => self.op_unary_float(f64::tan)?,
            Opcode::Asin => self.op_unary_float(f64::asin)?,
            Opcode::Acos => self.op_unary_float(f64::acos)?,
            Opcode::Atan => self.op_unary_float(f64::atan)?,
            Opcode::IsNan => {
                let a = self.pop()?;
                match a.view() {
                    ValueView::Float(x) => self.push(self.make_bool(x.is_nan())),
                    ValueView::Long(_) => self.push(self.make_bool(false)),
                    _ => {
                        return Err(VmError::TypeError {
                            expected: "Float or Long",
                            got: "other",
                        })
                    }
                }
            }
            Opcode::IsInf => {
                let a = self.pop()?;
                match a.view() {
                    ValueView::Float(x) => self.push(self.make_bool(x.is_infinite())),
                    ValueView::Long(_) => self.push(self.make_bool(false)),
                    _ => {
                        return Err(VmError::TypeError {
                            expected: "Float or Long",
                            got: "other",
                        })
                    }
                }
            }

            // === Comparison ===
            Opcode::Lt => self.op_comparison(|a, b| a < b, |a, b| a < b, |a, b| a < b)?,
            Opcode::Le => self.op_comparison(|a, b| a <= b, |a, b| a <= b, |a, b| a <= b)?,
            Opcode::Gt => self.op_comparison(|a, b| a > b, |a, b| a > b, |a, b| a > b)?,
            Opcode::Ge => self.op_comparison(|a, b| a >= b, |a, b| a >= b, |a, b| a >= b)?,
            Opcode::Eq => {
                let b = self.pop()?;
                let a = self.pop()?;
                // MeTTa HE-compatible numeric equality: Long(2) == Float(2.0) -> true.
                // Uses numeric promotion with epsilon tolerance for float comparison.
                // StructEq opcode retains structural equivalence for internal use.
                let equal = numeric_equal_generic(&a, &b);
                self.push(self.make_bool(equal));
            }
            Opcode::Ne => {
                let b = self.pop()?;
                let a = self.pop()?;
                // MeTTa HE-compatible numeric inequality.
                let not_equal = !numeric_equal_generic(&a, &b);
                self.push(self.make_bool(not_equal));
            }
            Opcode::StructEq => {
                let b = self.pop()?;
                let a = self.pop()?;
                let equal = a.structurally_equivalent(&b);
                self.push(self.make_bool(equal));
            }

            // === Boolean ===
            Opcode::And => {
                let b = self.pop()?;
                let a = self.pop()?;
                match (a.as_bool(), b.as_bool()) {
                    (Some(x), Some(y)) => self.push(self.make_bool(x && y)),
                    _ => {
                        return Err(VmError::TypeError {
                            expected: "bool",
                            got: "other",
                        })
                    }
                }
            }
            Opcode::Or => {
                let b = self.pop()?;
                let a = self.pop()?;
                match (a.as_bool(), b.as_bool()) {
                    (Some(x), Some(y)) => self.push(self.make_bool(x || y)),
                    _ => {
                        return Err(VmError::TypeError {
                            expected: "bool",
                            got: "other",
                        })
                    }
                }
            }
            Opcode::Not => {
                let a = self.pop()?;
                match a.as_bool() {
                    Some(x) => self.push(self.make_bool(!x)),
                    None => {
                        return Err(VmError::TypeError {
                            expected: "bool",
                            got: "other",
                        })
                    }
                }
            }
            Opcode::Xor => {
                let b = self.pop()?;
                let a = self.pop()?;
                match (a.as_bool(), b.as_bool()) {
                    (Some(x), Some(y)) => self.push(self.make_bool(x ^ y)),
                    _ => {
                        return Err(VmError::TypeError {
                            expected: "bool",
                            got: "other",
                        })
                    }
                }
            }

            // === Type Operations ===
            Opcode::GetType => self.op_get_type()?,
            Opcode::CheckType => self.op_check_type()?,
            Opcode::IsType => self.op_is_type()?,
            Opcode::AssertType => self.op_assert_type()?,

            // === Pattern Matching ===
            // Compiled unification opcodes
            Opcode::UCheckSExpr => self.op_u_check_sexpr()?,
            Opcode::UCheckArity => self.op_u_check_arity()?,
            Opcode::UCheckAtom => self.op_u_check_atom()?,
            Opcode::UGetChild => self.op_u_get_child()?,
            Opcode::UBindVar => self.op_u_bind_var()?,
            Opcode::UCheckLong => self.op_u_check_long()?,
            Opcode::UCheckValue => self.op_u_check_value()?,
            Opcode::UWildcard => self.op_u_wildcard()?,

            Opcode::Match => self.op_match()?,
            Opcode::MatchBind => self.op_match_bind()?,
            Opcode::MatchHead => self.op_match_head()?,
            Opcode::MatchArity => self.op_match_arity()?,
            Opcode::MatchGuard => self.op_match_guard()?,
            Opcode::Unify => self.op_unify()?,
            Opcode::UnifyBind => self.op_unify_bind()?,
            Opcode::IsVariable => {
                let a = self.pop()?;
                let is_var = a.is_variable();
                self.push(self.make_bool(is_var));
            }
            Opcode::IsSExpr => {
                let a = self.pop()?;
                let is_sexpr = a.as_sexpr().is_some();
                self.push(self.make_bool(is_sexpr));
            }
            Opcode::IsSymbol => {
                let a = self.pop()?;
                let is_symbol = a.as_atom().is_some();
                self.push(self.make_bool(is_symbol));
            }
            Opcode::GetHead => {
                // H3 (2026-05-05) hard-cut: empty/non-expr → push HE Error atom,
                // do NOT halt VM. Quoted-transparency extension removed.
                let a = self.pop()?;
                if let Some(items) = a.as_sexpr() {
                    if let Some(first) = items.first() {
                        self.push(first.clone());
                    } else {
                        let call = self.make_sexpr(vec![self.make_atom("car-atom"), a.clone()]);
                        let err = self.make_error(
                            "car-atom expects a non-empty expression as an argument",
                            call,
                        );
                        self.push(err);
                    }
                } else {
                    let call = self.make_sexpr(vec![self.make_atom("car-atom"), a.clone()]);
                    let err = self.make_error(
                        "car-atom expects a non-empty expression as an argument",
                        call,
                    );
                    self.push(err);
                }
            }
            Opcode::GetTail => {
                // H3 hard-cut: same shape as GetHead.
                let a = self.pop()?;
                if let Some(items) = a.as_sexpr() {
                    if !items.is_empty() {
                        let tail: Vec<V> = items[1..].to_vec();
                        self.push(self.make_sexpr(tail));
                    } else {
                        let call = self.make_sexpr(vec![self.make_atom("cdr-atom"), a.clone()]);
                        let err = self.make_error(
                            "cdr-atom expects a non-empty expression as an argument",
                            call,
                        );
                        self.push(err);
                    }
                } else {
                    let call = self.make_sexpr(vec![self.make_atom("cdr-atom"), a.clone()]);
                    let err = self.make_error(
                        "cdr-atom expects a non-empty expression as an argument",
                        call,
                    );
                    self.push(err);
                }
            }
            Opcode::StructuralHead => {
                let raw = self.pop()?;
                let reduced = self.maybe_pre_eval_structural(raw)?;
                self.push_head_of(reduced)?;
            }
            Opcode::StructuralTail => {
                let raw = self.pop()?;
                let reduced = self.maybe_pre_eval_structural(raw)?;
                self.push_tail_of(reduced)?;
            }
            Opcode::GetArity => {
                let a = self.pop()?;
                if let Some(items) = a.as_sexpr() {
                    self.push(self.make_long(items.len() as i64));
                } else {
                    return Err(VmError::TypeError {
                        expected: "S-expression",
                        got: "other",
                    });
                }
            }
            Opcode::GetElement => {
                let index = self.read_u8()? as usize;
                let value = self.pop()?;
                if let Some(items) = value.as_sexpr() {
                    if index < items.len() {
                        self.push(items[index].clone());
                    } else {
                        return Err(VmError::TypeError {
                            expected: "S-expression with valid index",
                            got: "other",
                        });
                    }
                } else {
                    return Err(VmError::TypeError {
                        expected: "S-expression with valid index",
                        got: "other",
                    });
                }
            }
            Opcode::DeconsAtom => self.op_decons_atom()?,
            Opcode::Repr => self.op_repr()?,
            Opcode::GetMetaType => self.op_get_metatype()?,
            // validate-atom and get-type-space require full type inference with
            // environment access — fall back to tree-walker for correct semantics
            Opcode::ValidateAtom | Opcode::GetTypeSpace => {
                return Err(VmError::Halted);
            }
            Opcode::IsFunction => self.op_is_function()?,
            // type-cast requires full type inference with environment access
            Opcode::TypeCast => {
                return Err(VmError::Halted);
            }
            Opcode::ConsAtom => self.op_cons_atom()?,
            Opcode::TrailMark => self.trail_mark(),
            Opcode::TrailUndo => self.trail_undo(),
            Opcode::UnifyDeep => self.op_unify_deep()?,
            Opcode::UnifyDeepBind => self.op_unify_deep_bind()?,
            Opcode::Unify4 => self.op_unify4()?,
            Opcode::MatchExternal => self.op_match_external()?,
            Opcode::MatchExternalOr => self.op_match_external_or()?,
            Opcode::CollapseBindBegin => self.op_collapse_bind_begin()?,
            Opcode::CollapseBindEnd => return self.op_collapse_bind_end(),
            // S1 TOPLEVEL (2026-05-13): HE runner-mode directives. Inline
            // handlers — flip the flag, no value-stack effect.
            //
            // S2 BANG-WORD (2026-05-13): also toggle `bang_body` so
            // op_define_rule (and any future `:` decl-atom opcode) skips
            // registration inside `(! ...)` directives.
            Opcode::EnterInterpretMode => {
                self.interpret_mode = true;
                self.bang_body = true;
                if let Some(env) = self.env.as_mut() {
                    env.set_bang_body(true);
                }
            }
            Opcode::ExitInterpretMode => {
                self.interpret_mode = false;
                self.bang_body = false;
                if let Some(env) = self.env.as_mut() {
                    env.set_bang_body(false);
                }
            }
            // S5: HE-bisimilar superpose-bind — decompose collapse-bind tuple
            // and fan out as bare nondet with merged bindings.
            Opcode::SuperposeBind => self.op_superpose_bind()?,
            Opcode::OccursCheck => self.op_occurs_check()?,
            Opcode::MapAtom => self.op_map_atom()?,
            Opcode::FilterAtom => self.op_filter_atom()?,
            Opcode::FoldlAtom => self.op_foldl_atom()?,
            Opcode::IndexAtom => self.op_index_atom()?,
            Opcode::MinAtom => self.op_min_atom()?,
            Opcode::MaxAtom => self.op_max_atom()?,

            // === Nondeterminism ===
            Opcode::Fork => return self.op_fork(),
            Opcode::ForkInline => return self.op_fork_inline(),
            Opcode::EvalSuperpose => return self.op_eval_superpose(),
            Opcode::Fail => return self.op_fail(),
            Opcode::Cut => self.op_cut(),
            Opcode::Collect => self.op_collect()?,
            Opcode::CollectN => self.op_collect_n()?,
            Opcode::Yield => return self.op_yield(),
            Opcode::BeginNondet => self.op_begin_nondet(),
            Opcode::EndNondet => self.op_end_nondet()?,
            Opcode::Amb => self.op_amb()?,
            Opcode::Guard => return self.op_guard(),
            Opcode::Commit => self.op_commit(),
            Opcode::Backtrack => return self.op_fail(),
            Opcode::CaseBarrierBegin => self.op_case_barrier_begin()?,
            Opcode::CaseBarrierEnd => self.op_case_barrier_end()?,

            // === Advanced Calls ===
            Opcode::CallNative => self.op_call_native()?,
            Opcode::CallExternal => self.op_call_external()?,
            Opcode::CallCached => self.op_call_cached()?,

            // === Environment Operations ===
            Opcode::DefineRule => self.op_define_rule()?,
            Opcode::LoadGlobal => self.op_load_global()?,
            Opcode::StoreGlobal => self.op_store_global()?,
            Opcode::DispatchRules => self.op_dispatch_rules()?,

            // === Space Operations ===
            Opcode::SpaceAdd => self.op_space_add()?,
            Opcode::SpaceRemove => self.op_space_remove()?,
            Opcode::SpaceGetAtoms => self.op_space_get_atoms()?,
            Opcode::SpaceMatch => self.op_space_match()?,
            Opcode::LoadSpace => self.op_load_space()?,

            // === State Operations ===
            Opcode::NewState => self.op_new_state()?,
            Opcode::GetState => self.op_get_state()?,
            Opcode::ChangeState => self.op_change_state()?,

            // === If-Reducible, Match, Match-Or (trampoline fallback) ===
            Opcode::EvalIfReducible => self.op_eval_if_reducible()?,
            Opcode::EvalMatch => self.op_eval_match()?,
            Opcode::EvalMatchOr => self.op_eval_match_or()?,
            // Native match against &self — calls env.match_space() directly
            Opcode::MatchSelf => self.op_match_self()?,
            Opcode::MatchSelfOr => self.op_match_self_or()?,

            // === Set Operations & Alpha-Equivalence ===
            Opcode::EvalIfEqual => self.op_eval_if_equal()?,
            Opcode::UniqueAtom => self.op_unique_atom()?,
            Opcode::AlphaUniqueAtom => self.op_alpha_unique_atom()?,
            Opcode::StructUniqueAtom => self.op_struct_unique_atom()?,
            Opcode::Msort => self.op_msort()?,
            Opcode::UnionAtom => self.op_union_atom()?,
            Opcode::IntersectionAtom => self.op_intersection_atom()?,
            Opcode::SubtractionAtom => self.op_subtraction_atom()?,

            // === Tuple & List Operations ===
            Opcode::TupleConcat => self.op_tuple_concat()?,
            Opcode::TupleCount => self.op_tuple_count()?,
            Opcode::Without => self.op_without()?,
            Opcode::ElementOf => self.op_element_of()?,
            Opcode::Range => self.op_range()?,
            Opcode::ReverseAtom => self.op_reverse_atom()?,
            Opcode::FlattenAtom => self.op_flatten_atom()?,
            Opcode::ZipAtom => self.op_zip_atom()?,
            Opcode::TakeAtom => self.op_take_atom()?,
            Opcode::DropAtom => self.op_drop_atom()?,

            // === Quote/Unquote ===
            Opcode::EvalQuote => self.op_eval_quote()?,
            Opcode::EvalUnquote => self.op_eval_unquote()?,

            // === Case & Collapse (trampoline fallback) ===
            Opcode::EvalCase => self.op_eval_case()?,
            Opcode::EvalCollapse => self.op_eval_collapse()?,

            // === Native Collapse (nondeterminism sandboxing) ===
            Opcode::CollapseBegin => self.op_collapse_begin()?,
            Opcode::CollapseEnd => return self.op_collapse_end(),

            // === Debug ===
            Opcode::Breakpoint => self.op_breakpoint()?,
            Opcode::Trace => self.op_trace()?,
            Opcode::Halt => {
                // Trace: BytecodeHalt
                #[cfg(feature = "trace")]
                {
                    use crate::backend::trace::thread_local_sink::with_thread_trace_collector;
                    with_thread_trace_collector(|tc| {
                        tc.emit_converted(
                            trace_format::TraceTier::BytecodeVM,
                            0,
                            trace_format::TraceValue::Unit,
                            vec![],
                            None,
                            trace_format::TraceEventKind::BytecodeHalt {
                                ip: self.ip as u32,
                                reason: "Halt opcode".to_string(),
                            },
                        );
                    });
                }
                return Err(VmError::Halted);
            }

            // Catch-all for any unhandled opcodes
            _ => {
                return Err(VmError::Runtime(format!(
                    "Opcode {:?} not yet implemented in generic VM",
                    opcode
                )));
            }
        }

        Ok(ControlFlow::Continue(()))
    }

    /// Handle reaching the end of a bytecode chunk.
    fn handle_chunk_end(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        if let Some(frame) = self.call_stack.pop() {
            let value = self.pop().unwrap_or_else(|_| self.make_unit());

            // Return to caller
            self.ip = frame.return_ip;
            self.chunk = frame.return_chunk;
            self.value_stack.truncate(frame.base_ptr);
            self.locals.truncate(frame.caller_locals_len);
            self.locals_base = frame.locals_base;

            // Pop binding frame
            if self.bindings_stack.len() > frame.bindings_base {
                self.bindings_stack.truncate(frame.bindings_base + 1);
                self.bindings_stack.pop();
            }

            self.push(value);
            Ok(ControlFlow::Continue(()))
        } else {
            // End of top-level
            if !self.value_stack.is_empty() {
                if self.unreduced {
                    self.had_unreduced_result = true;
                }
                self.results.extend(self.value_stack.drain(..));
            }
            // yield_on_top_return: exhaust remaining alternatives
            if self.yield_on_top_return && !self.choice_points.is_empty() {
                if !self.collapse_frames.is_empty() {
                    return self.op_fail_within_collapse();
                }
                return self.op_fail();
            }
            Ok(ControlFlow::Break(std::mem::take(&mut self.results)))
        }
    }

    // === Helper Methods for Opcodes ===

    /// Binary numeric operation helper.
    ///
    /// `op_name` is the printable op symbol ("+", "-", "*", …) used to build
    /// the HE-aligned `(Error (op a b) (BadArgType pos Number ErrorType))`
    /// shape when one of the arguments is itself an `Error` atom.
    #[inline]
    fn op_binary_num(
        &mut self,
        op_name: &str,
        int_op: impl Fn(i64, i64) -> i64,
        float_op: impl Fn(f64, f64) -> f64,
    ) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        // Error propagation per HE semantics: when either operand is an
        // Error atom, emit `(Error (op a b) (BadArgType POS Number ErrorType))`
        // (1-indexed position) rather than forwarding the inner Error.
        // T04/046 verifies; matches T0's `error_to_bad_arg_type` helper in
        // `grounded/state.rs`. Use the *sentinel* check so we catch the
        // SExpr-with-`Error`-head form that T1 compiles literal Error atoms
        // into (the dedicated `is_error()` variant only fires after a
        // runtime Error has been raised by another opcode).
        if a.is_error_sentinel() || b.is_error_sentinel() {
            let arg_idx = if a.is_error_sentinel() { 1 } else { 2 };
            let call = self.factory.sexpr(vec![
                self.factory.atom(op_name),
                a.clone(),
                b.clone(),
            ]);
            let detail = self.factory.sexpr(vec![
                self.factory.atom("BadArgType"),
                self.factory.long(arg_idx as i64),
                self.factory.atom("Number"),
                self.factory.atom("ErrorType"),
            ]);
            let err = self.factory.error(call, detail);
            self.push(err);
            return Ok(());
        }
        // BUG-T0-T1-003 (spec §14.1.1 Ext-3): Empty annihilation in arithmetic.
        // When either operand is the Empty sentinel, the entire arithmetic
        // expression yields no result (branch annihilation), matching T0's
        // canonical behavior in `src/backend/grounded/arithmetic.rs:94-97`.
        // Push Empty so downstream handlers propagate the annihilation.
        //
        // X.4 MTT-EMPTY-ANNIHILATION: also recognise the literal `Empty`
        // symbol (HE interpret_tuple return_on_error semantics).
        if a.is_empty() || b.is_empty()
            || a.as_atom() == Some("Empty") || b.as_atom() == Some("Empty")
        {
            self.push(self.factory.empty());
            return Ok(());
        }
        match (a.as_long(), b.as_long()) {
            (Some(x), Some(y)) => self.push(self.make_long(int_op(x, y))),
            _ => match (a.as_float(), b.as_float()) {
                (Some(x), Some(y)) => self.push(self.make_float(float_op(x, y))),
                _ => {
                    // Mixed Long/Float type promotion
                    match (a.as_long(), b.as_float()) {
                        (Some(x), Some(y)) => self.push(self.make_float(float_op(x as f64, y))),
                        _ => match (a.as_float(), b.as_long()) {
                            (Some(x), Some(y)) => self.push(self.make_float(float_op(x, y as f64))),
                            _ => {
                                return Err(VmError::TypeError {
                                    expected: "number",
                                    got: "other",
                                })
                            }
                        },
                    }
                }
            },
        }
        Ok(())
    }

    /// Unary float operation helper.
    #[inline]
    fn op_unary_float(&mut self, op: impl Fn(f64) -> f64) -> VmResult<()> {
        let a = self.pop()?;
        if let Some(x) = a.as_float() {
            self.push(self.make_float(op(x)));
        } else if let Some(x) = a.as_long() {
            self.push(self.make_float(op(x as f64)));
        } else {
            return Err(VmError::TypeError {
                expected: "number",
                got: "other",
            });
        }
        Ok(())
    }

    /// Comparison operation helper.
    ///
    /// `string_cmp` is invoked when both operands are Strings (BUG-T0-T1-004,
    /// spec §14.2.1 Ext-5). Callers pass the matching comparison combinator;
    /// `String::cmp` then yields the lexicographic ordering Bool.
    #[inline]
    fn op_comparison(
        &mut self,
        int_cmp: impl Fn(i64, i64) -> bool,
        float_cmp: impl Fn(f64, f64) -> bool,
        string_cmp: impl Fn(&str, &str) -> bool,
    ) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        // Error propagation: preserve nested errors instead of synthesizing
        // a fresh TypeError. Matches T0's trampoline behavior.
        if a.is_error() {
            self.push(a);
            return Ok(());
        }
        if b.is_error() {
            self.push(b);
            return Ok(());
        }
        // BUG-T0-T1-003: Empty annihilation in comparisons per spec §14.2.1.
        // X.4 MTT-EMPTY-ANNIHILATION: also recognise the literal `Empty` symbol.
        if a.is_empty() || b.is_empty()
            || a.as_atom() == Some("Empty") || b.as_atom() == Some("Empty")
        {
            self.push(self.factory.empty());
            return Ok(());
        }
        // BUG-T0-T1-004: String comparison (lex order) per spec §14.2.1 Ext-5.
        if let (Some(x), Some(y)) = (a.as_string(), b.as_string()) {
            self.push(self.make_bool(string_cmp(x, y)));
            return Ok(());
        }
        match (a.as_long(), b.as_long()) {
            (Some(x), Some(y)) => self.push(self.make_bool(int_cmp(x, y))),
            _ => match (a.as_float(), b.as_float()) {
                (Some(x), Some(y)) => self.push(self.make_bool(float_cmp(x, y))),
                _ => {
                    // Mixed Long/Float type promotion
                    match (a.as_long(), b.as_float()) {
                        (Some(x), Some(y)) => self.push(self.make_bool(float_cmp(x as f64, y))),
                        _ => match (a.as_float(), b.as_long()) {
                            (Some(x), Some(y)) => self.push(self.make_bool(float_cmp(x, y as f64))),
                            _ => {
                                return Err(VmError::TypeError {
                                    expected: "number",
                                    got: "other",
                                })
                            }
                        },
                    }
                }
            },
        }
        Ok(())
    }

    /// Rot3: rotate top 3 stack elements [a, b, c] -> [c, a, b].
    fn op_rot3(&mut self) -> VmResult<()> {
        let len = self.value_stack.len();
        if len < 3 {
            return Err(VmError::StackUnderflow);
        }
        // [a, b, c] -> [c, a, b]
        let c = self.value_stack.pop().expect("length checked");
        let b = self.value_stack.pop().expect("length checked");
        let a = self.value_stack.pop().expect("length checked");
        self.value_stack.push(c);
        self.value_stack.push(a);
        self.value_stack.push(b);
        Ok(())
    }

    /// Over: copy second element to top (a b -> a b a).
    fn op_over(&mut self) -> VmResult<()> {
        let value = self.peek_n(1)?.clone();
        self.push(value);
        Ok(())
    }

    /// DupN: duplicate top N values.
    fn op_dup_n(&mut self) -> VmResult<()> {
        let n = self.read_u8()? as usize;
        let len = self.value_stack.len();
        if n > len {
            return Err(VmError::StackUnderflow);
        }
        for i in (len - n)..len {
            let value = self.value_stack[i].clone();
            self.push(value);
        }
        Ok(())
    }

    /// PopN: pop n elements from stack.
    fn op_pop_n(&mut self) -> VmResult<()> {
        let n = self.read_u8()? as usize;
        let len = self.value_stack.len();
        if n > len {
            return Err(VmError::StackUnderflow);
        }
        self.value_stack.truncate(len - n);
        Ok(())
    }

    // === Stub implementations for complex opcodes ===
    // These need full implementation with trait-based logic

    fn op_push_variable(&mut self) -> VmResult<()> {
        let index = self.read_u16()?;
        let var = self
            .chunk
            .get_constant(index)
            .ok_or(VmError::InvalidConstant(index))?
            .clone();

        // Check if it's a pattern variable that should be resolved from bindings
        if let Some(name) = var.as_atom() {
            if name.starts_with('$') {
                // Step 1: ordinary frame-stack binding takes precedence
                // (HE invariant: one slot per name; class lookup is a
                // fallback when no entry exists).
                for frame in self.bindings_stack.iter().rev() {
                    if let Some(value) = frame.get(name) {
                        self.push(value.clone());
                        return Ok(());
                    }
                }
                // Step 2: S0d.2 (2026-05-13) class-aware lookup. If the
                // variable is a member of an equivalence class created by
                // a prior `(unify ...)` form, return the class value
                // (if any) or the ORIGINAL lookup-key (preserves T03/004
                // distinct-vars semantics — HE M-VAR-VAR-DISTINCT).
                if !self.class_table_is_empty() {
                    if let Some(table) = self.class_table.as_ref() {
                        if let Some(cid) = table.class_of(name) {
                            if let Some(v) = table.class_value(cid) {
                                self.push(v.clone());
                                return Ok(());
                            }
                            // Value-less class → push ORIGINAL lookup-key.
                            self.push(var);
                            return Ok(());
                        }
                    }
                }
            }
        }

        // Not found in bindings or not a pattern variable - push as-is
        self.push(var);
        Ok(())
    }

    fn op_make_list(&mut self) -> VmResult<()> {
        let arity = self.read_u8()? as usize;
        let len = self.value_stack.len();
        if arity > len {
            return Err(VmError::StackUnderflow);
        }
        let elements: Vec<V> = self.value_stack.drain((len - arity)..).collect();
        // Build proper Cons-list by folding right
        let mut list = self.make_unit();
        for elem in elements.into_iter().rev() {
            let cons_atom = self.make_atom("Cons");
            list = self.make_sexpr(vec![cons_atom, elem, list]);
        }
        self.push(list);
        Ok(())
    }

    fn op_make_quote(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let quoted = self.factory.quote(value);
        self.push(quoted);
        Ok(())
    }

    fn op_eval_quote(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let quoted = self.factory.quote(value);
        self.push(quoted);
        Ok(())
    }

    fn op_eval_unquote(&mut self) -> VmResult<()> {
        let val = self.pop()?;
        if let Some(inner) = val.as_quoted() {
            self.push(inner);
        } else {
            self.push(val);
        }
        Ok(())
    }

    /// Multi-way branch via jump table.
    ///
    /// Reads a table index from bytecode, pops a selector value from stack,
    /// looks up the corresponding offset in the jump table, and jumps to it.
    /// If the selector doesn't match any entry, jumps to the default offset.
    ///
    /// Stack: [selector] -> []
    /// Bytecode: JumpTable table_index:u16
    fn op_jump_table(&mut self) -> VmResult<()> {
        let table_index = self.read_u16()? as usize;
        let selector = self.pop()?;

        // Get jump table from chunk
        let jump_table = self.chunk.get_jump_table(table_index).ok_or_else(|| {
            VmError::Runtime(format!("Invalid jump table index: {}", table_index))
        })?;

        // Compute hash of selector value for table lookup
        // Hash the selector value using debug repr for consistent hashing
        let selector_hash = xxh3_64(format!("{:?}", selector).as_bytes());

        // Look up in jump table entries
        let target_offset = jump_table
            .entries
            .iter()
            .find(|(hash, _)| *hash == selector_hash)
            .map(|(_, offset)| *offset)
            .unwrap_or(jump_table.default_offset);

        // Jump to target
        self.ip = target_offset;
        Ok(())
    }

    fn op_call(&mut self) -> VmResult<()> {
        let head_idx = self.read_u16()?;
        let arity = self.read_u8()? as usize;

        // Pop arguments in reverse order
        let mut args = Vec::with_capacity(arity);
        for _ in 0..arity {
            args.push(self.pop()?);
        }
        args.reverse();

        // Build call expression from head + args
        let head = self
            .chunk
            .get_constant(head_idx)
            .cloned()
            .ok_or(VmError::InvalidConstant(head_idx))?;
        let mut items = Vec::with_capacity(arity + 1);
        items.push(head);
        items.extend(args);
        let expr = self.make_sexpr(items);

        // Dispatch via environment rules
        self.push(expr);
        self.op_dispatch_rules()?;

        Ok(())
    }

    fn op_tail_call(&mut self) -> VmResult<()> {
        let head_idx = self.read_u16()?;
        let arity = self.read_u8()? as usize;

        // Pop arguments in reverse order
        let mut args = Vec::with_capacity(arity);
        for _ in 0..arity {
            args.push(self.pop()?);
        }
        args.reverse();

        // Build call expression from head + args
        let head = self
            .chunk
            .get_constant(head_idx)
            .cloned()
            .ok_or(VmError::InvalidConstant(head_idx))?;
        let mut items = Vec::with_capacity(arity + 1);
        items.push(head);
        items.extend(args);
        let expr = self.make_sexpr(items);

        // Dispatch via environment rules (tail call - no new frame)
        self.push(expr);
        self.op_dispatch_rules()?;
        Ok(())
    }

    /// Call with N arguments where the head is on the stack (not constant pool).
    ///
    /// Pops the head and N arguments from the stack, builds a call expression,
    /// and dispatches to environment for rule matching.
    ///
    /// Stack: [head, arg1, arg2, ..., argN] -> [result]
    /// Bytecode: CallN arity:u8
    fn op_call_n(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::call", ip = self.ip, "call_n");
        let arity = self.read_u8()? as usize;

        // Pop arguments and head from stack
        // Stack order: head is pushed first, then args left-to-right
        // So we need to pop args first, then head
        if self.value_stack.len() < arity + 1 {
            return Err(VmError::StackUnderflow);
        }

        // Pop arguments
        let args: Vec<V> = self
            .value_stack
            .drain(self.value_stack.len() - arity..)
            .collect();

        // Pop head
        let head = self.pop()?;

        // Extract head symbol if it's an atom
        let is_atom = head.as_atom().is_some();

        // Build the call expression
        let mut items = Vec::with_capacity(arity + 1);
        items.push(head);
        items.extend(args);
        let expr = self.make_sexpr(items);

        if !is_atom {
            // Head is not an atom - return expression as data
            self.push(expr);
            return Ok(());
        }

        // Dispatch via environment rules
        self.push(expr);
        self.op_dispatch_rules()?;
        Ok(())
    }

    /// Tail call with N arguments where the head is on the stack.
    /// Same as CallN but reuses current call frame for TCO.
    ///
    /// Stack: [head, arg1, arg2, ..., argN] -> [result]
    /// Bytecode: TailCallN arity:u8
    fn op_tail_call_n(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::call", ip = self.ip, "tail_call_n");
        let arity = self.read_u8()? as usize;

        // Pop arguments and head from stack
        if self.value_stack.len() < arity + 1 {
            return Err(VmError::StackUnderflow);
        }

        // Pop arguments
        let args: Vec<V> = self
            .value_stack
            .drain(self.value_stack.len() - arity..)
            .collect();

        // Pop head
        let head = self.pop()?;

        // Build the call expression
        let mut items = Vec::with_capacity(arity + 1);
        items.push(head);
        items.extend(args);
        let expr = self.make_sexpr(items);

        // Dispatch via environment rules (tail call - no new frame)
        self.push(expr);
        self.op_dispatch_rules()?;
        Ok(())
    }

    fn op_return(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        trace!(target: "mettatron::vm::call", ip = self.ip, "return");
        // Empty stack at top-level Return means the chunk had a side-effect-only
        // terminator (e.g., `(= lhs rhs)` → DefineRule + Pop). Treat as "no
        // result" and finalize: without this the caller falls through to the
        // trampoline, which re-evaluates the expression and re-fires the
        // side-effect (double-add for rule defs).
        if self.call_stack.is_empty() && self.value_stack.is_empty() {
            if self.yield_on_top_return && !self.choice_points.is_empty() {
                if !self.collapse_bind_frames.is_empty() {
                    return self.op_fail_within_collapse_bind();
                }
                if !self.collapse_frames.is_empty() {
                    return self.op_fail_within_collapse();
                }
                return self.op_fail();
            }
            return Ok(ControlFlow::Break(std::mem::take(&mut self.results)));
        }
        let value = self.pop()?;
        if let Some(frame) = self.call_stack.pop() {
            // Return to caller - restore chunk/ip
            self.ip = frame.return_ip;
            self.chunk = frame.return_chunk;
            self.value_stack.truncate(frame.base_ptr);
            // Drop callee locals while preserving the caller's local slots.
            self.locals.truncate(frame.caller_locals_len);
            self.locals_base = frame.locals_base;

            // Pop binding frames down to caller's level
            while self.bindings_stack.len() > frame.bindings_base + 1 {
                self.bindings_stack.pop();
            }

            // Phase 1b-F: compose saved_bindings (caller's ambient) with
            // current_bindings (bindings accumulated inside the RHS) so the
            // caller sees the combined effect. Strict compose: on conflict,
            // emit fail (branch dies) matching HE's BindingsSet::empty()
            // silent pruning. Without this, bindings established during RHS
            // execution either leak uncontrolled or get lost on return.
            let composed = crate::backend::eval::bindings::compose_outer_inner_generic(
                &frame.saved_bindings,
                &self.current_bindings,
                &self.factory,
            );
            let conflict = composed.is_empty()
                && !frame.saved_bindings.is_empty()
                && !self.current_bindings.is_empty();
            if conflict {
                // Branch dies: caller's context and RHS bindings are
                // inconsistent. Backtrack to the next alternative.
                return if !self.collapse_bind_frames.is_empty() {
                    self.op_fail_within_collapse_bind()
                } else if !self.collapse_frames.is_empty() {
                    self.op_fail_within_collapse()
                } else {
                    self.op_fail()
                };
            }
            self.current_bindings = composed;

            self.push(value);
            Ok(ControlFlow::Continue(()))
        } else {
            // Return from top-level — record the result + its bindings snapshot
            // (Phase C: sidecar encoding for collapse-bind).
            if self.unreduced {
                self.had_unreduced_result = true;
            }
            self.results.push(value);
            if !self.collapse_bind_frames.is_empty() {
                self.per_result_bindings.push(self.current_bindings.clone());
            }
            // yield_on_top_return: exhaust all nondeterministic alternatives
            // within this single run() call — no VM exit/re-enter overhead.
            if self.yield_on_top_return && !self.choice_points.is_empty() {
                if !self.collapse_bind_frames.is_empty() {
                    return self.op_fail_within_collapse_bind();
                }
                if !self.collapse_frames.is_empty() {
                    return self.op_fail_within_collapse();
                }
                return self.op_fail();
            }
            Ok(ControlFlow::Break(std::mem::take(&mut self.results)))
        }
    }

    fn op_return_multi(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        // Return all values on stack above base_ptr
        let base = self.call_stack.last().map(|f| f.base_ptr).unwrap_or(0);
        let values: Vec<V> = self.value_stack.drain(base..).collect();

        if let Some(frame) = self.call_stack.pop() {
            self.ip = frame.return_ip;
            self.chunk = frame.return_chunk;
            self.value_stack.truncate(frame.base_ptr);
            // Drop callee locals while preserving the caller's local slots.
            self.locals.truncate(frame.caller_locals_len);
            self.locals_base = frame.locals_base;

            // Pop binding frames down to caller's level
            while self.bindings_stack.len() > frame.bindings_base + 1 {
                self.bindings_stack.pop();
            }

            // Phase 1b-F: strict compose of saved_bindings + current_bindings.
            // See op_return for rationale.
            let composed = crate::backend::eval::bindings::compose_outer_inner_generic(
                &frame.saved_bindings,
                &self.current_bindings,
                &self.factory,
            );
            let conflict = composed.is_empty()
                && !frame.saved_bindings.is_empty()
                && !self.current_bindings.is_empty();
            if conflict {
                return if !self.collapse_bind_frames.is_empty() {
                    self.op_fail_within_collapse_bind()
                } else if !self.collapse_frames.is_empty() {
                    self.op_fail_within_collapse()
                } else {
                    self.op_fail()
                };
            }
            self.current_bindings = composed;

            for v in values {
                self.push(v);
            }
            Ok(ControlFlow::Continue(()))
        } else {
            // Phase C: record each result's bindings snapshot when inside a
            // collapse-bind scope so the sidecar encoding pairs them later.
            if !self.collapse_bind_frames.is_empty() {
                for _ in 0..values.len() {
                    self.per_result_bindings.push(self.current_bindings.clone());
                }
            }
            self.results.extend(values);
            Ok(ControlFlow::Break(std::mem::take(&mut self.results)))
        }
    }

    fn op_get_type(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        // S6 (RC-GET-TYPE-CONSULTS-ENV): HE parity — get-type consults the
        // environment for `(: name TypeName)` assertions before falling back
        // to syntactic type. Delegates to the shared `infer_types_generic`
        // helper used by T0 (single source of truth across tiers).
        //
        // HE dispatch order (lib/src/metta/types.rs `get_atom_types_internal`):
        //   1. Typed primitives (Long/Float→Number, Bool→Bool, String→String)
        //   2. Atom symbol → query_types(space, atom) for `(: atom $T)`
        //   3. SExpr → arrow return type lookup via env
        //   4. Fallback `%Undefined%` for untyped atoms
        //
        // T04/068 (2026-05-17): HE-bisimilar nondeterministic enumeration.
        // When a symbol has multiple `(: name T)` assertions, HE returns
        // every type via superpose-style fan-out. The VM mirrors that by
        // pushing the first type onto the value stack and registering the
        // remaining alternatives as a `GenericChoicePoint` (same pattern as
        // `op_eval_superpose`). On backtrack each remaining alternative is
        // restored, yielding the full type set as separate results.
        if let Some(env) = self.env.as_ref() {
            use crate::backend::eval::types::infer_types_generic;
            let factory = self.factory.clone();
            let types = infer_types_generic(&value, &factory, env);

            if types.is_empty() {
                // HE parity fallback: untyped atom → %Undefined%.
                self.push(self.make_atom("%Undefined%"));
            } else {
                // Fan out alternatives via choice points so the VM enumerates
                // all declared types under nondet (HE superpose semantics).
                if types.len() > 1 {
                    let remaining: Vec<
                        GenericAlternative<V, GenericBytecodeChunk<V>>,
                    > = types[1..]
                        .iter()
                        .cloned()
                        .map(GenericAlternative::Value)
                        .collect();
                    self.choice_points.push(GenericChoicePoint {
                        value_stack_height: self.value_stack.len(),
                        call_stack_height: self.call_stack.len(),
                        bindings_stack_height: self.bindings_stack.len(),
                        ip: self.ip,
                        chunk: Arc::clone(&self.chunk),
                        alternatives: remaining,
                        saved_unreduced: self.unreduced,
                        trail_height: self.trail.len(),
                        saved_current_bindings: self.current_bindings.clone(),
                        locals_height: self.locals.len(),
                        locals_base_at_cp: self.locals_base,
                    });
                }
                self.push(types.into_iter().next().expect("non-empty checked"));
            }
        } else {
            // No env attached — fall back to syntactic type_name() (legacy path
            // for tests/utilities that construct a VM without an environment).
            let type_name = value.type_name();
            self.push(self.make_atom(type_name));
        }
        Ok(())
    }

    fn op_check_type(&mut self) -> VmResult<()> {
        let type_val = self.pop()?;
        let value = self.pop()?;

        let expected = if let Some(name) = type_val.as_atom() {
            name
        } else {
            return Err(VmError::TypeError {
                expected: "type symbol",
                got: "other",
            });
        };

        // Type variables match anything (consistent with tree-visitor)
        let matches = if expected.starts_with('$') {
            true
        } else {
            value.type_name() == expected
        };
        self.push(self.make_bool(matches));
        Ok(())
    }

    fn op_is_type(&mut self) -> VmResult<()> {
        // Same as check_type for now
        self.op_check_type()
    }

    fn op_assert_type(&mut self) -> VmResult<()> {
        let type_val = self.pop()?;
        let value = self.peek()?;

        let expected = if let Some(name) = type_val.as_atom() {
            name
        } else {
            return Err(VmError::TypeError {
                expected: "type symbol",
                got: "other",
            });
        };

        if value.type_name() != expected {
            return Err(VmError::TypeError {
                expected: "matching type",
                got: value.type_name(),
            });
        }
        Ok(())
    }

    fn op_match(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let pattern = self.pop()?;
        let matches = self.pattern_matches_generic(&pattern, &value);
        self.push(self.make_bool(matches));
        Ok(())
    }

    fn op_match_bind(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let pattern = self.pop()?;

        if let Some(bindings) = self.pattern_match_bind_generic(&pattern, &value) {
            for (name, val) in bindings {
                self.set_binding(name, val);
            }
            self.push(self.make_bool(true));
        } else {
            self.push(self.make_bool(false));
        }
        Ok(())
    }

    /// Match head symbol of an S-expression for fast dispatch optimization.
    ///
    /// Reads an expected symbol index from the bytecode, pops a value from the stack,
    /// and checks if the value is an S-expression whose first element matches the
    /// expected symbol. Pushes Bool(true) if it matches, Bool(false) otherwise.
    ///
    /// Stack: [value] -> [bool]
    /// Bytecode: MatchHead expected_index:u8
    fn op_match_head(&mut self) -> VmResult<()> {
        let expected_index = self.read_u8()? as u16;

        // Get expected symbol from constant pool and clone it to avoid borrow issues
        let expected = self
            .chunk
            .get_constant(expected_index)
            .ok_or(VmError::InvalidConstant(expected_index))?
            .clone();

        let value = self.pop()?;

        // Check if value is an S-expression with matching head
        let matches = if let Some(items) = value.as_sexpr() {
            if let Some(head) = items.first() {
                // Compare expected atom against head atom
                if let (Some(exp_sym), Some(head_sym)) = (expected.as_atom(), head.as_atom()) {
                    exp_sym == head_sym
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };

        self.push(self.make_bool(matches));
        Ok(())
    }

    fn op_match_arity(&mut self) -> VmResult<()> {
        let arity = self.read_u8()? as usize;
        let value = self.pop()?;

        let matches = if let Some(items) = value.as_sexpr() {
            items.len() == arity
        } else {
            false
        };
        self.push(self.make_bool(matches));
        Ok(())
    }

    fn op_match_guard(&mut self) -> VmResult<()> {
        let guard_offset = self.ip as u16;
        let guard_result = self.pop()?;
        let passed = guard_result.as_bool() == Some(true);
        self.push(self.make_bool(passed));
        self.profile_guard(guard_offset, passed);
        Ok(())
    }

    // =========================================================================
    // Compiled Unification Opcodes (0x08-0x0F)
    // =========================================================================

    /// UCheckSExpr: Check TOS is S-expression; jump to fail_offset on failure.
    fn op_u_check_sexpr(&mut self) -> VmResult<()> {
        let offset = self.read_i16()?;
        let top = self.peek()?;
        if top.as_sexpr().is_none() && !top.is_unit() {
            // Not an S-expression — jump to fail target
            let jump_from = self.ip;
            self.ip = (jump_from as isize + offset as isize) as usize;
        }
        Ok(())
    }

    /// UCheckArity: Check S-expr length; jump to fail_offset on mismatch.
    fn op_u_check_arity(&mut self) -> VmResult<()> {
        let expected_arity = self.read_u8()? as usize;
        let offset = self.read_i16()?;
        let top = self.peek()?;
        let matches = if let Some(items) = top.as_sexpr() {
            items.len() == expected_arity
        } else if top.is_unit() {
            expected_arity == 0
        } else {
            false
        };
        if !matches {
            let jump_from = self.ip;
            self.ip = (jump_from as isize + offset as isize) as usize;
        }
        Ok(())
    }

    /// UCheckAtom: Pop, check atom equals constant; jump on mismatch.
    fn op_u_check_atom(&mut self) -> VmResult<()> {
        let const_idx = self.read_u16()?;
        let offset = self.read_i16()?;
        let value = self.pop()?;
        let expected = self
            .chunk
            .get_constant(const_idx)
            .ok_or(VmError::InvalidConstant(const_idx))?;
        let matches = if let (Some(v_name), Some(e_name)) = (value.as_atom(), expected.as_atom()) {
            v_name == e_name
        } else {
            false
        };
        if !matches {
            let jump_from = self.ip;
            self.ip = (jump_from as isize + offset as isize) as usize;
        }
        Ok(())
    }

    /// UGetChild: Peek S-expr, push children[index].
    fn op_u_get_child(&mut self) -> VmResult<()> {
        let index = self.read_u8()? as usize;
        let top = self.peek()?;
        if let Some(items) = top.as_sexpr() {
            if index < items.len() {
                let child = items[index].clone();
                self.push(child);
                return Ok(());
            }
        }
        Err(VmError::TypeError {
            expected: "S-expression with sufficient arity",
            got: "invalid index or non-S-expression",
        })
    }

    /// UBindVar: Pop, bind var (or check consistency); trail the binding.
    fn op_u_bind_var(&mut self) -> VmResult<()> {
        let name_idx = self.read_u16()?;
        let offset = self.read_i16()?;
        let value = self.pop()?;
        let name_val = self
            .chunk
            .get_constant(name_idx)
            .ok_or(VmError::InvalidConstant(name_idx))?;
        let var_name = name_val.as_atom().ok_or(VmError::TypeError {
            expected: "atom (variable name)",
            got: "non-atom constant",
        })?;

        // Check if variable is already bound in the current frame.
        //
        // BUG T0-T1-009 (per plan invariant #2 tier-locality): use iterative
        // work-stack unification inline in the VM handler — NOT a call into
        // T0's bidirectional_unify_generic. Each tier owns its semantics.
        // The previously-bound value may itself contain unbound variables;
        // testing structural equality misses cases where they would still
        // unify (e.g., `$a := f($x)` then `$a vs f(1)` must unify `$x → 1`).
        // Stack-safe: iterative work-stack, no recursion.
        if let Some(frame) = self.bindings_stack.last() {
            if let Some(existing) = frame.get(var_name) {
                if !structurally_unify(existing, &value) {
                    let jump_from = self.ip;
                    self.ip = (jump_from as isize + offset as isize) as usize;
                    return Ok(());
                }
                return Ok(());
            }
        }

        // New binding — trail it and store
        let frame_index = self.bindings_stack.len().saturating_sub(1);
        self.trail.push(TrailEntry::NewBinding {
            frame_index,
            name: var_name.to_string(),
        });
        self.set_binding(var_name.to_string(), value);
        Ok(())
    }

    /// UCheckLong: Pop, check Long equals constant; jump on mismatch.
    fn op_u_check_long(&mut self) -> VmResult<()> {
        let const_idx = self.read_u16()?;
        let offset = self.read_i16()?;
        let value = self.pop()?;
        let expected = self
            .chunk
            .get_constant(const_idx)
            .ok_or(VmError::InvalidConstant(const_idx))?;
        let matches = if let (Some(v_long), Some(e_long)) = (value.as_long(), expected.as_long()) {
            v_long == e_long
        } else {
            false
        };
        if !matches {
            let jump_from = self.ip;
            self.ip = (jump_from as isize + offset as isize) as usize;
        }
        Ok(())
    }

    /// UCheckValue: Pop, structural equality against constant; jump on mismatch.
    fn op_u_check_value(&mut self) -> VmResult<()> {
        let const_idx = self.read_u16()?;
        let offset = self.read_i16()?;
        let value = self.pop()?;
        let expected = self
            .chunk
            .get_constant(const_idx)
            .ok_or(VmError::InvalidConstant(const_idx))?;
        if !value.structurally_equivalent(expected) {
            let jump_from = self.ip;
            self.ip = (jump_from as isize + offset as isize) as usize;
        }
        Ok(())
    }

    /// UWildcard: Pop and discard (wildcard match).
    fn op_u_wildcard(&mut self) -> VmResult<()> {
        self.pop()?;
        Ok(())
    }

    fn op_unify(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        // Use the corrected M-M generic core for bidirectional unification
        let unified = crate::backend::eval::bindings::bidirectional_unify_generic(&a, &b).is_some();
        self.push(self.make_bool(unified));
        Ok(())
    }

    fn op_unify_bind(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;

        // S0d.2 (2026-05-13): user-facing `(unify ...)` form uses
        // `UnifyMode::Unify` so var-var-distinct creates an equivalence class
        // (HE M-VAR-VAR-DISTINCT, spec §4.3.1). Class memberships are merged
        // into the VM's `class_table` so subsequent `op_push_variable` for a
        // value-less class member yields the original lookup-key, preserving
        // T03/004 distinct-vars semantics under strict alpha.
        use crate::backend::models::UnifyMode;
        match crate::backend::eval::bindings::bidirectional_unify_generic_with_mode(
            &a,
            &b,
            UnifyMode::Unify,
        ) {
            Some(bindings) => {
                // Install ordinary entries into the current VM frame.
                for (name, val) in bindings.entries.iter() {
                    self.set_binding(name.to_string(), val.clone());
                }
                // Adopt any new class table produced by this unify call.
                // The unifier returns an isolated table (it does not see the
                // VM's prior classes) — for the single-form usage `(unify a b
                // body fail)` this is correct because each form's classes
                // are scoped to its body. If a future S0d.x step needs
                // cross-form class accumulation, switch to a class-aware
                // unifier seeded with `self.class_table`.
                if bindings.classes.is_some() {
                    self.class_table = bindings.classes;
                }
                self.push(self.make_bool(true));
            }
            None => {
                self.push(self.make_bool(false));
            }
        }
        Ok(())
    }

    // =========================================================================
    // Runtime Unification Opcodes (UnifyDeep, UnifyDeepBind, OccursCheck)
    // =========================================================================

    /// UnifyDeep: Full bidirectional M-M unification with fail-offset jump.
    ///
    /// S0d.2 (2026-05-13): uses `UnifyMode::Unify` so var-var-distinct pairs
    /// form equivalence classes. Class memberships are merged into the VM's
    /// `class_table` for downstream `op_push_variable` lookups.
    fn op_unify_deep(&mut self) -> VmResult<()> {
        let offset = self.read_i16()?;
        let b = self.pop()?;
        let a = self.pop()?;

        use crate::backend::models::UnifyMode;
        match crate::backend::eval::bindings::bidirectional_unify_generic_with_mode(
            &a,
            &b,
            UnifyMode::Unify,
        ) {
            Some(bindings) => {
                // Trail all new bindings for backtrack undo.
                let frame_index = self.bindings_stack.len().saturating_sub(1);
                for (name, _val) in bindings.entries.iter() {
                    self.trail.push(TrailEntry::NewBinding {
                        frame_index,
                        name: name.to_string(),
                    });
                }
                // Install bindings in current frame.
                for (name, val) in bindings.entries.iter() {
                    self.set_binding(name.to_string(), val.clone());
                }
                // Adopt any new class table produced by this unify call.
                if bindings.classes.is_some() {
                    self.class_table = bindings.classes;
                }
            }
            None => {
                let jump_from = self.ip;
                self.ip = (jump_from as isize + offset as isize) as usize;
            }
        }
        Ok(())
    }

    /// UnifyDeepBind: Like UnifyDeep but installs bindings into current frame.
    fn op_unify_deep_bind(&mut self) -> VmResult<()> {
        // Same implementation as UnifyDeep — both install bindings
        self.op_unify_deep()
    }

    /// OccursCheck: Check if variable occurs in term; jump if it does.
    fn op_occurs_check(&mut self) -> VmResult<()> {
        let offset = self.read_i16()?;
        let term = self.pop()?;
        let var = self.pop()?;

        if let Some(var_name) = var.as_atom() {
            let bindings = crate::backend::models::GenericBindings::new();
            if crate::backend::eval::bindings::occurs_in_generic_pub(var_name, &term, &bindings) {
                let jump_from = self.ip;
                self.ip = (jump_from as isize + offset as isize) as usize;
            }
        }
        Ok(())
    }

    fn op_decons_atom(&mut self) -> VmResult<()> {
        // H3 (2026-05-05) hard-cut: empty/non-expr → push HE Error atom,
        // do NOT halt VM.
        // ERR-shape align (2026-05-16): HE empirical detail is `"expected:
        // (decons-atom (: <expr> Expression)), found: <call>"` where
        // `<call>` is the canonical print of the full `(decons-atom <arg>)`
        // form. Matches conformance T04-kernel/019-decons-empty.
        let value = self.pop()?;
        if let Some(items) = value.as_sexpr() {
            if !items.is_empty() {
                let head = items[0].clone();
                let tail = self.make_sexpr(items[1..].to_vec());
                self.push(self.factory.sexpr(vec![head, tail]));
                return Ok(());
            }
        }
        let call = self.make_sexpr(vec![self.make_atom("decons-atom"), value.clone()]);
        let detail = format!(
            "expected: (decons-atom (: <expr> Expression)), found: {}",
            call.friendly_repr()
        );
        let err = self.make_error(&detail, call);
        self.push(err);
        Ok(())
    }

    fn op_repr(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let repr = value.friendly_repr();
        self.push(self.make_string(&repr));
        Ok(())
    }

    fn op_get_metatype(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let metatype = metatype_of_view(value.view());
        self.push(self.make_atom(metatype));
        Ok(())
    }

    /// is-function: check if a value is an arrow type (-> ...)
    fn op_is_function(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let is_fn = match value.view() {
            ValueView::SExpr(items) => items.first().and_then(|v| v.as_atom()) == Some("->"),
            _ => false,
        };
        self.push(self.make_bool(is_fn));
        Ok(())
    }

    /// cons-atom: prepend head to tail S-expression
    /// Matches tree-visitor semantics in list_ops.rs:118-126
    fn op_cons_atom(&mut self) -> VmResult<()> {
        let tail = self.pop()?;
        let head = self.pop()?;

        let tail_items: &[V] = if tail.is_unit() {
            &[]
        } else {
            tail.as_sexpr().ok_or(VmError::TypeError {
                expected: "S-expression or Unit",
                got: "other",
            })?
        };
        let mut items = Vec::with_capacity(tail_items.len() + 1);
        items.push(head);
        items.extend(tail_items.iter().cloned());
        self.push(self.make_sexpr(items));
        Ok(())
    }

    /// Map a template chunk over each element of an S-expression.
    /// Operand: u16 chunk_idx
    /// Stack: [list] -> [mapped_list]
    ///
    /// Phase 1b-D (HE-bisimilarity): each item is substituted with the
    /// VM's ambient `current_bindings` before template dispatch so
    /// caller-scope free variables in the item expression resolve
    /// correctly. Iterations are independent — per-item template
    /// bindings do NOT thread across iterations (unlike `foldl-atom`),
    /// matching HE's `metta/runner/stdlib/core.rs` `MapAtomOp`
    /// semantics. Each iteration's template bindings are discarded
    /// after the iteration's value is collected; only the value
    /// flows into the mapped list.
    fn op_map_atom(&mut self) -> VmResult<()> {
        use crate::backend::eval::bindings::{
            apply_bindings_generic, apply_chain_generic, compose_outer_inner_generic,
        };

        let chunk_idx = self.read_u16()?;
        let list = self.pop()?;

        let items = list.as_sexpr().ok_or(VmError::TypeError {
            expected: "list/S-expression",
            got: "other",
        })?;

        let template_chunk = self
            .chunk
            .get_chunk_constant(chunk_idx)
            .ok_or(VmError::InvalidConstant(chunk_idx))?;

        // H13 (2026-05-05) mirror — items' free variables are the
        // caller-scope names that must thread across iterations.
        let items_free_vars: Vec<&'static str> = {
            let mut keys: Vec<&'static str> = Vec::new();
            for item in items {
                for v in item.free_variables() {
                    if !keys.contains(&v) {
                        keys.push(v);
                    }
                }
            }
            keys
        };

        let mut results = Vec::with_capacity(items.len());
        let mut acc_bindings: GenericBindings<V> = GenericBindings::new();
        let ambient = self.current_bindings.clone();
        for item in items {
            // Apply both ambient (caller's bindings at fold start) and
            // acc_bindings (per-iter threaded so far) before dispatch.
            let mut effective = item.clone();
            if !ambient.is_empty() {
                effective = apply_bindings_generic(&effective, &ambient, &self.factory);
            }
            if !acc_bindings.is_empty() {
                effective = apply_bindings_generic(&effective, &acc_bindings, &self.factory);
            }
            let (result, tmpl_bindings) =
                self.execute_generic_template_with_binding(Arc::clone(&template_chunk), effective)?;
            results.push(result);

            // H13 mirror — filter per-iter bindings to caller-scope names
            // and compose. Drops the template's per-invocation freshened
            // pattern vars (`$__fr_*`) unless they appear as free vars in
            // the items list — same coarse filter as op_foldl_atom.
            let mut step_propagating: GenericBindings<V> = GenericBindings::new();
            for (name, val) in tmpl_bindings.iter() {
                let is_user = !name.starts_with("$__fr_");
                let is_caller_freshened = items_free_vars.contains(&name);
                if is_user || is_caller_freshened {
                    step_propagating.insert_or_replace(name, val.clone());
                }
            }
            if !step_propagating.is_empty() {
                let mut composed =
                    compose_outer_inner_generic(&acc_bindings, &step_propagating, &self.factory);
                if composed.is_empty() && !acc_bindings.is_empty() && !step_propagating.is_empty() {
                    self.push(self.factory.empty());
                    return Ok(());
                }
                apply_chain_generic(&mut composed, &self.factory);
                acc_bindings = composed;
            }
        }

        // Compose acc_bindings into VM's current_bindings so caller
        // sees the threaded result (mirror op_foldl_atom).
        if !acc_bindings.is_empty() {
            self.current_bindings =
                compose_outer_inner_generic(&self.current_bindings, &acc_bindings, &self.factory);
            apply_chain_generic(&mut self.current_bindings, &self.factory);
        }

        self.push(self.factory.sexpr(results));
        Ok(())
    }

    /// Filter elements of an S-expression using a predicate chunk.
    /// Operand: u16 chunk_idx
    /// Stack: [list] -> [filtered_list]
    ///
    /// Phase 1b-D (HE-bisimilarity): each item is substituted with the
    /// VM's ambient `current_bindings` before predicate dispatch so
    /// caller-scope free variables resolve correctly. Iterations are
    /// independent. When the predicate yields `true`, the ORIGINAL
    /// item (not substituted) is pushed to the filtered list —
    /// matching HE's `FilterAtomOp` semantics where the filter
    /// preserves structural identity.
    fn op_filter_atom(&mut self) -> VmResult<()> {
        use crate::backend::eval::bindings::{
            apply_bindings_generic, apply_chain_generic, compose_outer_inner_generic,
        };

        let chunk_idx = self.read_u16()?;
        let list = self.pop()?;

        let items = list.as_sexpr().ok_or(VmError::TypeError {
            expected: "list/S-expression",
            got: "other",
        })?;

        let predicate_chunk = self
            .chunk
            .get_chunk_constant(chunk_idx)
            .ok_or(VmError::InvalidConstant(chunk_idx))?;

        // H13 (2026-05-05) mirror — items' free variables are the
        // caller-scope names that must thread across iterations.
        let items_free_vars: Vec<&'static str> = {
            let mut keys: Vec<&'static str> = Vec::new();
            for item in items {
                for v in item.free_variables() {
                    if !keys.contains(&v) {
                        keys.push(v);
                    }
                }
            }
            keys
        };

        let mut results = Vec::new();
        let mut acc_bindings: GenericBindings<V> = GenericBindings::new();
        let ambient = self.current_bindings.clone();
        for item in items {
            let mut effective = item.clone();
            if !ambient.is_empty() {
                effective = apply_bindings_generic(&effective, &ambient, &self.factory);
            }
            if !acc_bindings.is_empty() {
                effective = apply_bindings_generic(&effective, &acc_bindings, &self.factory);
            }
            let (result, pred_bindings) = self
                .execute_generic_template_with_binding(Arc::clone(&predicate_chunk), effective)?;

            // H13 mirror — compose per-iter bindings BEFORE keep/drop test.
            let mut step_propagating: GenericBindings<V> = GenericBindings::new();
            for (name, val) in pred_bindings.iter() {
                let is_user = !name.starts_with("$__fr_");
                let is_caller_freshened = items_free_vars.contains(&name);
                if is_user || is_caller_freshened {
                    step_propagating.insert_or_replace(name, val.clone());
                }
            }
            if !step_propagating.is_empty() {
                let mut composed =
                    compose_outer_inner_generic(&acc_bindings, &step_propagating, &self.factory);
                if composed.is_empty() && !acc_bindings.is_empty() && !step_propagating.is_empty() {
                    self.push(self.factory.empty());
                    return Ok(());
                }
                apply_chain_generic(&mut composed, &self.factory);
                acc_bindings = composed;
            }

            // Check if predicate returned true
            if result.as_bool() == Some(true) {
                results.push(item.clone());
            }
        }

        // Compose acc_bindings into VM's current_bindings.
        if !acc_bindings.is_empty() {
            self.current_bindings =
                compose_outer_inner_generic(&self.current_bindings, &acc_bindings, &self.factory);
            apply_chain_generic(&mut self.current_bindings, &self.factory);
        }

        self.push(self.factory.sexpr(results));
        Ok(())
    }

    /// Left fold over an S-expression using a template chunk.
    /// Operand: u16 chunk_idx
    /// Stack: [list, init] -> [result]
    ///
    /// Phase 1b-C (HE-bisimilarity): each fold step's `current_bindings`
    /// is composed into an `acc_bindings` register via
    /// `compose_outer_inner_generic`. Before dispatching the next
    /// iteration, the accumulated bindings are applied to the item
    /// (via `apply_bindings_generic`) so rules matched in iteration K
    /// resolve shared variables before iteration K+1 evaluates its item.
    ///
    /// This mirrors HE's recursive `foldl-atom` definition:
    ///
    ///   (= (foldl-atom $list $init $op)
    ///      (if (== $list ())
    ///          $init
    ///          (foldl-atom (cdr-atom $list) ($op $init (car-atom $list)) $op)))
    ///
    /// where the recursive structure naturally threads bindings via
    /// normal rule dispatch: iteration K+1's `(car-atom $list)` is
    /// evaluated under whatever bindings iteration K established.
    ///
    /// On compose producing empty bindings (genuine ground/ground
    /// conflict per `compose_outer_inner_generic` — see bindings.rs
    /// Phase 2B), the branch is inconsistent and the fold fails with
    /// no result. Matches HE's strict `Bindings::merge` rejection.
    fn op_foldl_atom(&mut self) -> VmResult<()> {
        use crate::backend::eval::bindings::{
            apply_bindings_generic, apply_chain_generic, compose_outer_inner_generic,
        };
        use crate::backend::models::GenericBindings;

        let chunk_idx = self.read_u16()?;
        let init = self.pop()?;
        let list = self.pop()?;

        let items = list.as_sexpr().ok_or(VmError::TypeError {
            expected: "list/S-expression",
            got: "other",
        })?;

        let op_chunk = self
            .chunk
            .get_chunk_constant(chunk_idx)
            .ok_or(VmError::InvalidConstant(chunk_idx))?;

        let mut acc = init;
        // Phase 1b-C: per-fold accumulated propagating bindings.
        // Retains user-level bindings AND any freshened bindings whose
        // names appear as free variables in remaining fold items. The
        // op's per-invocation freshened vars (e.g. Truth_ModusPonens's
        // pattern vars) are dropped to avoid spurious cross-iteration
        // ground/ground conflicts (MeTTaTron one-time-freshening
        // artifact; HE freshens per-invocation so equivalent names
        // don't collide).
        let mut acc_bindings: GenericBindings<V> = GenericBindings::new();

        // Collect item free variables (remaining items). These are the
        // caller-scope freshened vars that must thread across
        // iterations. Items' vars are stable: the full list at fold
        // start is a superset of any single iteration's remaining.
        let items_free_vars: Vec<&'static str> = {
            let mut keys: Vec<&'static str> = Vec::new();
            for item in items {
                for v in item.free_variables() {
                    if !keys.contains(&v) {
                        keys.push(v);
                    }
                }
            }
            keys
        };

        for item in items {
            // Substitute any previously-accumulated bindings into this
            // item before dispatching the template. If iteration K-1
            // bound `$b=c`, iteration K's `(father $b c)` becomes
            // `(father c c)` and evaluates (or fails to match) under
            // that constraint.
            let substituted_item = if acc_bindings.is_empty() {
                item.clone()
            } else {
                apply_bindings_generic(item, &acc_bindings, &self.factory)
            };

            let (new_acc, step_bindings) =
                self.execute_generic_foldl_template(Arc::clone(&op_chunk), acc, substituted_item)?;

            // Filter step_bindings: retain user-level vars AND caller-
            // scope freshened vars (appearing in items' free variables).
            // Drop the op's per-invocation rule-match bindings.
            let mut step_propagating: GenericBindings<V> = GenericBindings::new();
            for (name, val) in step_bindings.iter() {
                let is_user = !name.starts_with("$__fr_");
                let is_caller_freshened = items_free_vars.contains(&name);
                if is_user || is_caller_freshened {
                    step_propagating.insert_or_replace(name, val.clone());
                }
            }

            // Compose into running acc_bindings. Returns empty on
            // genuine ground/ground conflict (Phase 2B). A branch with
            // such a conflict dies — HE-faithful.
            let mut composed =
                compose_outer_inner_generic(&acc_bindings, &step_propagating, &self.factory);
            if composed.is_empty() && !acc_bindings.is_empty() && !step_propagating.is_empty() {
                self.push(self.factory.empty());
                return Ok(());
            }
            apply_chain_generic(&mut composed, &self.factory);
            acc_bindings = composed;
            acc = new_acc;
        }

        // Preserve the fold's accumulated bindings as the VM's
        // current_bindings so the caller (which composed its outer
        // context into `acc_bindings` implicitly via the per-step
        // template exec) continues with the right context.
        self.current_bindings =
            compose_outer_inner_generic(&self.current_bindings, &acc_bindings, &self.factory);
        apply_chain_generic(&mut self.current_bindings, &self.factory);

        self.push(acc);
        Ok(())
    }

    // === Template Execution Helpers ===

    /// Execute a template chunk with a single bound value (for map/filter).
    /// Saves and restores VM state around execution.
    ///
    /// Phase 1b-B: returns a `VmBoundValue<V>` — the produced value paired
    /// with the `current_bindings` captured at the end of template execution.
    /// Callers that don't yet plumb bindings can ignore `.1`; callers that
    /// do (Phase 1b-C onwards) compose it into their per-iteration state.
    ///
    /// The caller's `current_bindings` is saved on entry and restored on
    /// exit — the template's own bindings are returned separately.
    fn execute_generic_template_with_binding(
        &mut self,
        chunk: Arc<GenericBytecodeChunk<V>>,
        binding: V,
    ) -> VmResult<VmBoundValue<V>> {
        // Save state
        let saved_ip = self.ip;
        let saved_chunk = Arc::clone(&self.chunk);
        let saved_stack_base = self.value_stack.len();
        // Phase 1b-B: isolate template's `current_bindings` from caller's.
        // Template starts with empty bindings; anything it discovers is
        // returned separately and the caller's register is restored.
        let saved_current_bindings = std::mem::take(&mut self.current_bindings);

        // Plan 2 (2026-05-06): protect `saved_current_bindings` from GC
        // during the inner step loop. The saved bindings are a Rust stack
        // local — `collect_roots_into` walks `self` only, so without an
        // explicit frame the values are invisible to the mark phase. If
        // a worker-thread safepoint fires inside the inner step loop and
        // GC runs, the saved bindings' slab slots can be reaped, causing
        // UAF when we restore them at the return points below.
        // TypeId-gated to MettaValue (the only V where slab GC matters).
        let _saved_bindings_guard = {
            use std::any::TypeId;
            if TypeId::of::<V>() == TypeId::of::<MettaValue>() {
                let materialized: Vec<V> = saved_current_bindings
                    .iter_full()
                    .map(|(_scope, _n, v)| v.clone())
                    .collect();
                // SAFETY: V == MettaValue verified via TypeId; Vec<V> and
                // Vec<MettaValue> have identical layout. The guard ties
                // the frame's lifetime to this scope, ensuring the Vec
                // outlives any safepoint that might collect roots from it.
                let materialized_mv: Vec<MettaValue> =
                    unsafe { std::mem::transmute::<Vec<V>, Vec<MettaValue>>(materialized) };
                let materialized_box = Box::new(materialized_mv);
                let ptr = &*materialized_box as *const Vec<MettaValue>;
                let guard = unsafe {
                    crate::backend::eval::frame_chain::EvalFrameGuard::push_vec(
                        crate::backend::eval::frame_chain::FrameLabel::Custom(
                            "vm-template-saved-bindings",
                        ),
                        ptr,
                    )
                };
                Some((guard, materialized_box))
            } else {
                None
            }
        };

        // Y.2 (2026-05-12): set up the template chunk's locals frame.
        // LoadLocal slot at `self.locals[locals_base + slot]` — template slot
        // 0 holds the iter-var (e.g. `$x` for `(map-atom (1 2 3) $x ...)`).
        // The prior implementation `push(binding)` wrote to value_stack which
        // is not where LoadLocal reads → the template body saw $x as
        // uninitialised → TypeError on `(+ $x 1)`.
        let saved_locals_base = self.locals_base;
        let saved_locals_len = self.locals.len();

        // Setup for template execution
        self.chunk = chunk;
        self.ip = 0;
        self.locals_base = saved_locals_len;
        let template_local_count = self.chunk.local_count() as usize;
        let need = self.locals_base + template_local_count.max(1);
        if self.locals.len() < need {
            let pad = need - self.locals.len();
            for _ in 0..pad {
                let u = self.make_unit();
                self.locals.push(u);
            }
        }
        // Write binding into local slot 0.
        self.locals[self.locals_base] = binding;

        // Execute until Return or end of chunk
        let exec_result: VmResult<VmBoundValue<V>> = loop {
            if self.ip >= self.chunk.len() {
                break Ok((self.factory.unit(), GenericBindings::new()));
            }
            let opcode_byte = self
                .chunk
                .read_byte(self.ip)
                .ok_or(VmError::IpOutOfBounds)?;
            let opcode =
                Opcode::from_byte(opcode_byte).ok_or(VmError::InvalidOpcode(opcode_byte))?;

            if opcode == Opcode::Return {
                break Ok((self.factory.unit(), GenericBindings::new()));
            }

            match self.step() {
                Ok(ControlFlow::Continue(())) => {}
                Ok(ControlFlow::Break(results)) => {
                    let template_bindings =
                        std::mem::replace(&mut self.current_bindings, saved_current_bindings.clone());
                    let value = results
                        .into_iter()
                        .next()
                        .unwrap_or_else(|| self.factory.unit());
                    break Ok((value, template_bindings));
                }
                Err(e) => {
                    self.current_bindings = saved_current_bindings.clone();
                    break Err(e);
                }
            }
        };

        // Get result from value-stack if we exited via Return / end-of-chunk
        // with no early-Break result above.
        let (result, template_bindings) = match exec_result {
            Ok((v, b)) if !v.is_unit() || !b.is_empty() => (v, b),
            Ok(_) => {
                let v = self.pop().unwrap_or_else(|_| self.factory.unit());
                let b = std::mem::replace(&mut self.current_bindings, saved_current_bindings);
                (v, b)
            }
            Err(e) => {
                // Restore locals frame before error return.
                self.locals_base = saved_locals_base;
                self.locals.truncate(saved_locals_len);
                self.ip = saved_ip;
                self.chunk = saved_chunk;
                self.value_stack.truncate(saved_stack_base);
                return Err(e);
            }
        };

        // Restore locals frame, IP, chunk, and value-stack high water.
        self.locals_base = saved_locals_base;
        self.locals.truncate(saved_locals_len);
        self.ip = saved_ip;
        self.chunk = saved_chunk;
        self.value_stack.truncate(saved_stack_base);

        Ok((result, template_bindings))
    }

    /// Execute a foldl template chunk with accumulator and item bindings.
    /// Saves and restores VM state around execution.
    ///
    /// Phase 1b-B: returns `VmBoundValue<V>` so the caller (Phase 1b-C
    /// `op_foldl_atom`) can thread bindings between fold iterations.
    fn execute_generic_foldl_template(
        &mut self,
        chunk: Arc<GenericBytecodeChunk<V>>,
        acc: V,
        item: V,
    ) -> VmResult<VmBoundValue<V>> {
        // Save state
        let saved_ip = self.ip;
        let saved_chunk = Arc::clone(&self.chunk);
        let saved_stack_base = self.value_stack.len();
        let saved_current_bindings = std::mem::take(&mut self.current_bindings);

        // Plan 2 (2026-05-06): same EvalFrameGuard protection as
        // execute_generic_template_with_binding above. See rationale there.
        let _saved_bindings_guard = {
            use std::any::TypeId;
            if TypeId::of::<V>() == TypeId::of::<MettaValue>() {
                let materialized: Vec<V> = saved_current_bindings
                    .iter_full()
                    .map(|(_scope, _n, v)| v.clone())
                    .collect();
                let materialized_mv: Vec<MettaValue> =
                    unsafe { std::mem::transmute::<Vec<V>, Vec<MettaValue>>(materialized) };
                let materialized_box = Box::new(materialized_mv);
                let ptr = &*materialized_box as *const Vec<MettaValue>;
                let guard = unsafe {
                    crate::backend::eval::frame_chain::EvalFrameGuard::push_vec(
                        crate::backend::eval::frame_chain::FrameLabel::Custom(
                            "vm-foldl-template-saved-bindings",
                        ),
                        ptr,
                    )
                };
                Some((guard, materialized_box))
            } else {
                None
            }
        };

        // Y.2 (2026-05-12): set up the template's locals frame. Slot 0 is
        // the accumulator; slot 1 is the item. See execute_generic_template_
        // with_binding for the full rationale.
        let saved_locals_base = self.locals_base;
        let saved_locals_len = self.locals.len();

        // Setup for template execution
        self.chunk = chunk;
        self.ip = 0;
        self.locals_base = saved_locals_len;
        let template_local_count = self.chunk.local_count() as usize;
        let need = self.locals_base + template_local_count.max(2);
        if self.locals.len() < need {
            let pad = need - self.locals.len();
            for _ in 0..pad {
                let u = self.make_unit();
                self.locals.push(u);
            }
        }
        self.locals[self.locals_base] = acc;
        self.locals[self.locals_base + 1] = item;

        // Execute until Return or end of chunk
        let exec_result: VmResult<VmBoundValue<V>> = loop {
            if self.ip >= self.chunk.len() {
                break Ok((self.factory.unit(), GenericBindings::new()));
            }
            let opcode_byte = self
                .chunk
                .read_byte(self.ip)
                .ok_or(VmError::IpOutOfBounds)?;
            let opcode =
                Opcode::from_byte(opcode_byte).ok_or(VmError::InvalidOpcode(opcode_byte))?;

            if opcode == Opcode::Return {
                break Ok((self.factory.unit(), GenericBindings::new()));
            }

            match self.step() {
                Ok(ControlFlow::Continue(())) => {}
                Ok(ControlFlow::Break(results)) => {
                    let template_bindings =
                        std::mem::replace(&mut self.current_bindings, saved_current_bindings.clone());
                    let value = results
                        .into_iter()
                        .next()
                        .unwrap_or_else(|| self.factory.unit());
                    break Ok((value, template_bindings));
                }
                Err(e) => {
                    self.current_bindings = saved_current_bindings.clone();
                    break Err(e);
                }
            }
        };

        // Resolve result from value-stack vs early-Break.
        let (result, template_bindings) = match exec_result {
            Ok((v, b)) if !v.is_unit() || !b.is_empty() => (v, b),
            Ok(_) => {
                let v = self.pop().unwrap_or_else(|_| self.factory.unit());
                let b = std::mem::replace(&mut self.current_bindings, saved_current_bindings);
                (v, b)
            }
            Err(e) => {
                self.locals_base = saved_locals_base;
                self.locals.truncate(saved_locals_len);
                self.ip = saved_ip;
                self.chunk = saved_chunk;
                self.value_stack.truncate(saved_stack_base);
                return Err(e);
            }
        };
        // Restore locals frame.
        self.locals_base = saved_locals_base;
        self.locals.truncate(saved_locals_len);

        // Restore state
        self.ip = saved_ip;
        self.chunk = saved_chunk;
        self.value_stack.truncate(saved_stack_base);

        Ok((result, template_bindings))
    }

    fn op_index_atom(&mut self) -> VmResult<()> {
        let index = self.pop()?;
        let expr = self.pop()?;

        let idx = match index.as_long() {
            Some(i) => i,
            None => {
                return Err(VmError::TypeError {
                    expected: "Long (index)",
                    got: "other",
                });
            }
        };

        match expr.as_sexpr() {
            Some(items) => {
                if idx < 0 || idx as usize >= items.len() {
                    return Err(VmError::IndexOutOfBounds {
                        index: idx as usize,
                        len: items.len(),
                    });
                }
                self.push(items[idx as usize].clone());
            }
            None => {
                return Err(VmError::TypeError {
                    expected: "S-expression",
                    got: "other",
                });
            }
        }
        Ok(())
    }

    fn op_min_atom(&mut self) -> VmResult<()> {
        let expr = self.pop()?;
        let items = match expr.as_sexpr() {
            Some(items) => items,
            None => {
                return Err(VmError::TypeError {
                    expected: "S-expression",
                    got: "other",
                });
            }
        };

        if items.is_empty() {
            return Err(VmError::TypeError {
                expected: "non-empty S-expression",
                got: "empty expression",
            });
        }

        // Find minimum among numeric values
        let mut min_val: Option<f64> = None;
        let mut min_is_long = true;

        for item in items {
            if let Some(x) = item.as_long() {
                let val = x as f64;
                min_val = Some(min_val.map_or(val, |m: f64| m.min(val)));
            } else if let Some(x) = item.as_float() {
                min_is_long = false;
                min_val = Some(min_val.map_or(x, |m: f64| m.min(x)));
            }
            // Skip non-numeric values
        }

        match min_val {
            Some(v) if min_is_long && v == (v as i64) as f64 => {
                self.push(self.make_long(v as i64));
            }
            Some(v) => {
                self.push(self.make_float(v));
            }
            None => {
                return Err(VmError::TypeError {
                    expected: "numeric values in expression",
                    got: "no numeric values",
                });
            }
        }
        Ok(())
    }

    fn op_max_atom(&mut self) -> VmResult<()> {
        let expr = self.pop()?;
        let items = match expr.as_sexpr() {
            Some(items) => items,
            None => {
                return Err(VmError::TypeError {
                    expected: "S-expression",
                    got: "other",
                });
            }
        };

        if items.is_empty() {
            return Err(VmError::TypeError {
                expected: "non-empty S-expression",
                got: "empty expression",
            });
        }

        // Find maximum among numeric values
        let mut max_val: Option<f64> = None;
        let mut max_is_long = true;

        for item in items {
            if let Some(x) = item.as_long() {
                let val = x as f64;
                max_val = Some(max_val.map_or(val, |m: f64| m.max(val)));
            } else if let Some(x) = item.as_float() {
                max_is_long = false;
                max_val = Some(max_val.map_or(x, |m: f64| m.max(x)));
            }
            // Skip non-numeric values
        }

        match max_val {
            Some(v) if max_is_long && v == (v as i64) as f64 => {
                self.push(self.make_long(v as i64));
            }
            Some(v) => {
                self.push(self.make_float(v));
            }
            None => {
                return Err(VmError::TypeError {
                    expected: "numeric values in expression",
                    got: "no numeric values",
                });
            }
        }
        Ok(())
    }

    // === If-Reducible & Match-Or (trampoline fallback) ===

    /// if-reducible: evaluate expr, check if reduced, branch accordingly.
    /// Falls back to full trampoline evaluation since this requires recursive eval.
    /// Stack: [expr, then, else] -> [result]
    fn op_eval_if_reducible(&mut self) -> VmResult<()> {
        let else_branch = self.pop()?;
        let then_branch = self.pop()?;
        let expr = self.pop()?;

        // Construct (if-reducible expr then else) and delegate to trampoline
        let sexpr = self.factory.sexpr(vec![
            self.factory.atom("if-reducible"),
            expr,
            then_branch,
            else_branch,
        ]);
        let env = self.env.clone().ok_or_else(|| {
            VmError::Runtime("if-reducible: no environment available".to_string())
        })?;
        let result = self.eval_sub_expr_vm(sexpr, env)?;
        self.push(result);
        Ok(())
    }

    /// match: pattern matching against a space.
    /// Falls back to full trampoline evaluation since this requires space matching.
    /// Stack: [space, pattern, template] -> [result]
    fn op_eval_match(&mut self) -> VmResult<()> {
        let template = self.pop()?;
        let pattern = self.pop()?;
        let space = self.pop()?;

        // Construct (match space pattern template) and delegate to trampoline
        let sexpr = self
            .factory
            .sexpr(vec![self.factory.atom("match"), space, pattern, template]);
        let env = self
            .env
            .clone()
            .ok_or_else(|| VmError::Runtime("match: no environment available".to_string()))?;
        let result = self.eval_sub_expr_vm(sexpr, env)?;
        self.push(result);
        Ok(())
    }

    /// match-or: match with default fallback.
    /// Falls back to full trampoline evaluation since this requires space matching.
    /// Stack: [space, pattern, default, template] -> [result]
    fn op_eval_match_or(&mut self) -> VmResult<()> {
        let template = self.pop()?;
        let default = self.pop()?;
        let pattern = self.pop()?;
        let space = self.pop()?;

        // Construct (match-or space pattern default template) and delegate to trampoline
        let sexpr = self.factory.sexpr(vec![
            self.factory.atom("match-or"),
            space,
            pattern,
            default,
            template,
        ]);
        let env = self
            .env
            .clone()
            .ok_or_else(|| VmError::Runtime("match-or: no environment available".to_string()))?;
        let result = self.eval_sub_expr_vm(sexpr, env)?;
        self.push(result);
        Ok(())
    }

    // === Native Match (no trampoline delegation) ===

    /// Native match against &self space.
    /// Stack: [pattern, template] → [result]
    /// Calls `env.match_space()` directly. Multiple results create choice points.
    /// Phase A: native 4-arg `unify` — non-space val1 path.
    ///
    /// Stack: [val1, pattern2] → []
    /// Operand: i16 fail_offset (relative to position after reading operand).
    ///
    /// On successful unification: installs bindings into current frame (trailed),
    /// then falls through to the success body bytecode.
    /// On no match: jumps to fail_offset where the failure body lives.
    ///
    /// **Limitation (documented)**: space-val1 `(unify &space pat succ fail)` is
    /// handled as unify-failure in this native path — `bidirectional_unify_generic`
    /// returns None for space values. The compiler should statically gate out
    /// space-val1 cases at emission time; runtime space-val1 therefore represents
    /// a dynamically-typed code path and semantically falls to the failure body.
    /// Full HE-bisimilar space-val1 unify (matching ProcessUnifyPattern1Iter's
    /// 3-way split) remains in the trampoline path.
    fn op_unify4(&mut self) -> VmResult<()> {
        let offset = self.read_i16()?;
        let pattern2 = self.pop()?;
        let val1 = self.pop()?;

        // S0d.2 (2026-05-13): user-facing 4-arg `(unify val1 pattern2 body
        // fail)` form. Use `UnifyMode::Unify` so var-var-distinct creates an
        // equivalence class (HE M-VAR-VAR-DISTINCT, spec §4.3.1). Class
        // memberships are merged into the VM's `class_table` so the success
        // body's `op_push_variable` for a value-less class member yields the
        // ORIGINAL lookup-key, preserving T03/004 distinct-vars semantics.
        use crate::backend::models::UnifyMode;
        match crate::backend::eval::bindings::bidirectional_unify_generic_with_mode(
            &val1,
            &pattern2,
            UnifyMode::Unify,
        ) {
            Some(bindings) => {
                // Trail all new bindings so backtracking can restore state.
                let frame_index = self.bindings_stack.len().saturating_sub(1);
                for (name, _val) in bindings.entries.iter() {
                    self.trail.push(TrailEntry::NewBinding {
                        frame_index,
                        name: name.to_string(),
                    });
                }
                // Install ordinary bindings in current frame.
                for (name, val) in bindings.entries.iter() {
                    self.set_binding(name.to_string(), val.clone());
                }
                // Adopt the class table produced by this unify call (if any).
                // Cleared on `run()` entry, so each top-level invocation
                // starts fresh.
                if bindings.classes.is_some() {
                    self.class_table = bindings.classes;
                }
                // Fall through to success body.
            }
            None => {
                // No match: jump to failure body.
                let jump_from = self.ip;
                self.ip = (jump_from as isize + offset as isize) as usize;
            }
        }
        Ok(())
    }

    /// Phase E: native `match` for external / non-`&self` named spaces.
    ///
    /// Stack: [space_ref, pattern, template] → [result(s)] (choice point for N>1)
    ///
    /// Delegates to `env.match_space` for module/self spaces (already handled
    /// by `op_match_self`) OR `SpaceHandle::collapse_with_multiplicity_generic`
    /// for external named spaces.
    fn op_match_external(&mut self) -> VmResult<()> {
        let template = self.pop()?;
        let pattern = self.pop()?;
        let space_ref = self.pop()?;

        // X.6 MTT-FN-SPACE-RESOLVE: resolve env-bound atom names like
        // `&space` (introduced via `(bind! &space (new-space))`) to their
        // SpaceHandle. Without this, the bare `as_space()` branch fails and
        // a TypeError leaks instead of dispatching to the bound handle.
        let Some(handle) = self.resolve_to_space_handle_owned(&space_ref) else {
            // Not a space — signal unreduced for tree-walker fallback.
            self.unreduced = true;
            self.push(self.make_sexpr(vec![]));
            return Ok(());
        };

        // Route by space kind:
        let matches: Vec<V> = if handle.is_module_space() || handle.name == "self" {
            // Module / &self space: use env.match_space (native to op_match_self path).
            let env = self
                .env
                .as_ref()
                .ok_or_else(|| VmError::Runtime("match: no environment available".to_string()))?;
            env.match_space(&pattern, &template)
                .into_iter()
                .flat_map(|m| std::iter::repeat(m.value).take(m.count))
                .collect()
        } else {
            // External named space: collapse with multiplicity, then unify each atom.
            let atoms = handle.collapse_with_multiplicity_generic(&self.factory);
            let mut out = Vec::new();
            for m in atoms.into_iter() {
                if let Some(bindings) =
                    crate::backend::eval::bindings::bidirectional_unify_generic(&pattern, &m.value)
                {
                    // Apply bindings to template for each match.
                    let instantiated = crate::backend::eval::bindings::apply_bindings_generic(
                        &template,
                        &bindings,
                        &self.factory,
                    );
                    for _ in 0..m.count {
                        out.push(instantiated.clone());
                    }
                }
            }
            out
        };

        if matches.is_empty() {
            self.unreduced = true;
            self.push(self.make_sexpr(vec![]));
        } else if matches.len() == 1 {
            self.push(matches.into_iter().next().expect("non-empty"));
        } else {
            let mut iter = matches.into_iter();
            let first = iter.next().expect("non-empty");
            let alternatives: Vec<GenericAlternative<V, GenericBytecodeChunk<V>>> =
                iter.map(GenericAlternative::Value).collect();
            self.choice_points.push(GenericChoicePoint {
                ip: self.ip,
                chunk: Arc::clone(&self.chunk),
                value_stack_height: self.value_stack.len(),
                call_stack_height: self.call_stack.len(),
                bindings_stack_height: self.bindings_stack.len(),
                alternatives,
                saved_unreduced: self.unreduced,
                trail_height: self.trail.len(),
                saved_current_bindings: self.current_bindings.clone(),
                locals_height: self.locals.len(),
                locals_base_at_cp: self.locals_base,
            });
            self.push(first);
        }
        Ok(())
    }

    /// Phase E: native `match-or` for external / non-`&self` named spaces.
    /// Stack: [space_ref, pattern, template, default] → [result(s)]
    fn op_match_external_or(&mut self) -> VmResult<()> {
        let template = self.pop()?;
        let default = self.pop()?;
        let pattern = self.pop()?;
        let space_ref = self.pop()?;

        // X.6 MTT-FN-SPACE-RESOLVE: resolve env-bound atom names like `&space`.
        let Some(handle) = self.resolve_to_space_handle_owned(&space_ref) else {
            self.unreduced = true;
            self.push(default);
            return Ok(());
        };

        let matches: Vec<V> = if handle.is_module_space() || handle.name == "self" {
            let env = self.env.as_ref().ok_or_else(|| {
                VmError::Runtime("match-or: no environment available".to_string())
            })?;
            env.match_space(&pattern, &template)
                .into_iter()
                .flat_map(|m| std::iter::repeat(m.value).take(m.count))
                .collect()
        } else {
            let atoms = handle.collapse_with_multiplicity_generic(&self.factory);
            let mut out = Vec::new();
            for m in atoms.into_iter() {
                if let Some(bindings) =
                    crate::backend::eval::bindings::bidirectional_unify_generic(&pattern, &m.value)
                {
                    let instantiated = crate::backend::eval::bindings::apply_bindings_generic(
                        &template,
                        &bindings,
                        &self.factory,
                    );
                    for _ in 0..m.count {
                        out.push(instantiated.clone());
                    }
                }
            }
            out
        };

        if matches.is_empty() {
            self.push(default);
        } else if matches.len() == 1 {
            self.push(matches.into_iter().next().expect("non-empty"));
        } else {
            let mut iter = matches.into_iter();
            let first = iter.next().expect("non-empty");
            let alternatives: Vec<GenericAlternative<V, GenericBytecodeChunk<V>>> =
                iter.map(GenericAlternative::Value).collect();
            self.choice_points.push(GenericChoicePoint {
                ip: self.ip,
                chunk: Arc::clone(&self.chunk),
                value_stack_height: self.value_stack.len(),
                call_stack_height: self.call_stack.len(),
                bindings_stack_height: self.bindings_stack.len(),
                alternatives,
                saved_unreduced: self.unreduced,
                trail_height: self.trail.len(),
                saved_current_bindings: self.current_bindings.clone(),
                locals_height: self.locals.len(),
                locals_base_at_cp: self.locals_base,
            });
            self.push(first);
        }
        Ok(())
    }

    /// Phase C: native `collapse-bind` scope begin (Task #26).
    ///
    /// Operand: u16 tracked_vars_const_idx (constant pool index of a
    /// tracked_vars S-expression — `(Atom(var1) Atom(var2) …)`).
    ///
    /// Pushes a `GenericCollapseBindFrame<V>` onto the VM so that subsequent
    /// `op_yield` / `op_return` opcodes that push to `self.results` *also*
    /// push the current bindings snapshot into `self.per_result_bindings`.
    /// `op_collapse_bind_end` then pairs values with bindings and encodes the
    /// sidecar `(value (Bindings ($var val) …))` format.
    fn op_collapse_bind_begin(&mut self) -> VmResult<()> {
        let tracked_vars_idx = self.read_u16()? as usize;

        // Load tracked_vars from constant pool. Accepts either:
        //   - S-expression of atoms: `(Atom(x) Atom(y) …)` → `["x", "y"]`
        //   - Unit (empty tracked_vars placeholder for the current scaffold).
        let tracked_vars: Vec<&'static str> = self
            .chunk
            .get_constant(tracked_vars_idx as u16)
            .and_then(|v| {
                v.as_sexpr().map(|items| {
                    items
                        .iter()
                        .filter_map(|v| v.as_atom())
                        .map(|s| {
                            // Leak-free: rely on `as_atom` returning an interned
                            // `&'static str` (SharedMapping). If non-static, fall
                            // back to an empty slice later.
                            let ptr: &'static str =
                                unsafe { std::mem::transmute::<&str, &'static str>(s) };
                            ptr
                        })
                        .collect()
                })
            })
            .unwrap_or_default();

        let frame: GenericCollapseBindFrame<V> = GenericCollapseBindFrame {
            saved_results: std::mem::take(&mut self.results),
            saved_per_result_bindings: std::mem::take(&mut self.per_result_bindings),
            choice_point_base: self.choice_points.len(),
            value_stack_height: self.value_stack.len(),
            continuation_ip: 0,       // set lazily on first CollapseBindEnd
            continuation_chunk: None, // set with continuation_ip
            tracked_vars,
        };
        self.collapse_bind_frames.push(frame);
        Ok(())
    }

    /// Phase C: native `collapse-bind` scope end.
    ///
    /// Dual entry point — called either:
    /// 1. Sequentially after the body produced one result; loops back into
    ///    `op_fail_within_collapse_bind` if more choice points remain.
    /// 2. After all alternatives exhaust (via `op_fail_within_collapse_bind`
    ///    falling through) — this finalizes the scope by encoding each
    ///    collected `(value, bindings)` pair and pushing the result list.
    fn op_collapse_bind_end(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        // Lazy set continuation_ip on first entry — we now know our own
        // position (self.ip is past the CollapseBindEnd opcode byte).
        {
            let frame = self.collapse_bind_frames.last_mut().ok_or_else(|| {
                VmError::Runtime("CollapseBindEnd without matching CollapseBindBegin".to_string())
            })?;
            if frame.continuation_chunk.is_none() {
                frame.continuation_ip = self.ip;
                frame.continuation_chunk = Some(Arc::clone(&self.chunk));
            }
        }

        let (value_stack_height, choice_point_base, tracked_vars) = {
            let frame = self.collapse_bind_frames.last().expect("checked above");
            (
                frame.value_stack_height,
                frame.choice_point_base,
                frame.tracked_vars.clone(),
            )
        };

        // Collect the current result (one iteration of the body), preserving
        // its bindings snapshot in parallel.
        if self.value_stack.len() > value_stack_height {
            let value = self.pop()?;
            if !value.is_unit() {
                if self.unreduced {
                    self.had_unreduced_result = true;
                }
                if let Some(projected) =
                    crate::backend::eval::bindings::project_bindings_for_consumer_generic(
                        &self.current_bindings,
                        &[&value],
                        Some(&tracked_vars),
                        &self.factory,
                    )
                {
                    self.results.push(value);
                    self.per_result_bindings.push(projected);
                }
            }
        }

        // Continue backtracking through remaining alternatives within scope.
        if self.choice_points.len() > choice_point_base {
            return self.op_fail_within_collapse_bind();
        }

        // All exhausted — finalize scope.
        let frame = self.collapse_bind_frames.pop().expect("checked above");
        let values: Vec<V> = std::mem::take(&mut self.results);
        let bindings_list: Vec<GenericBindings<V>> = std::mem::take(&mut self.per_result_bindings);

        // Restore outer scope state.
        self.results = frame.saved_results;
        self.per_result_bindings = frame.saved_per_result_bindings;
        self.value_stack.truncate(frame.value_stack_height);

        // Build sidecar-encoded `(value (Bindings …))` pairs for each result.
        let mut pairs: Vec<V> = Vec::with_capacity(values.len());
        for (i, value) in values.into_iter().enumerate() {
            if value.is_unit() {
                continue;
            }
            let raw_bindings = bindings_list.get(i).cloned().unwrap_or_default();

            let projected = crate::backend::eval::bindings::project_bindings_for_consumer_generic(
                &raw_bindings,
                &[&value],
                Some(&frame.tracked_vars),
                &self.factory,
            )
            .unwrap_or_default();

            // Filter `$__fr_*` (per-invocation freshening artifacts).
            let filtered = if projected.iter().any(|(k, _)| k.starts_with("$__fr_")) {
                let mut f = GenericBindings::new();
                for (name, val) in projected.iter() {
                    if !name.starts_with("$__fr_") {
                        f.insert_or_replace(name, val.clone());
                    }
                }
                f
            } else {
                projected
            };

            let bindings_sexpr = crate::backend::eval::bindings::encode_bindings_as_sexpr_generic(
                &filtered,
                &self.factory,
            );
            pairs.push(self.make_sexpr(vec![value, bindings_sexpr]));
        }

        // Continue at the resolved continuation_ip, if set.
        if let Some(cont_chunk) = frame.continuation_chunk.clone() {
            self.chunk = cont_chunk;
            self.ip = frame.continuation_ip;
        }
        // Push the result list onto the (now-restored outer) value stack.
        self.push(self.make_sexpr(pairs));
        Ok(ControlFlow::Continue(()))
    }

    /// S5: HE-bisimilar `superpose-bind`.
    ///
    /// Pops a collapse-bind-shaped expression `((atom (Bindings ...)) ...)`
    /// from the stack, decodes each pair, and fans out as bare nondet
    /// alternatives. Each pair's saved bindings are merged into the
    /// current_bindings context (composed via compose_outer_inner_generic).
    ///
    /// Stack: [collapsed_arg] -> [first_atom] + choice points for remaining
    ///
    /// HE reference: `lib/src/metta/interpreter.rs:893-918`.
    fn op_superpose_bind(&mut self) -> VmResult<()> {
        let collapsed = self.pop()?;

        // Decompose: each child should be `(atom (Bindings ...))`.
        // Preallocate to children.len() to avoid reallocation.
        let pairs: Vec<(V, GenericBindings<V>)> = if let Some(children) = collapsed.as_sexpr() {
            let mut out: Vec<(V, GenericBindings<V>)> = Vec::with_capacity(children.len());
            for child in children.iter() {
                if let Some(items) = child.as_sexpr() {
                    match items.len() {
                        2 => {
                            // (atom (Bindings ...))
                            let atom = items[0].clone();
                            let bindings_sexpr = &items[1];
                            let bindings =
                                crate::backend::eval::trampoline::eval_loop::decode_bindings_from_sexpr_generic(
                                    bindings_sexpr,
                                    &self.factory,
                                );
                            out.push((atom, bindings));
                        }
                        _ => {
                            // Malformed: treat as atom with empty bindings.
                            out.push((child.clone(), GenericBindings::new()));
                        }
                    }
                } else {
                    out.push((child.clone(), GenericBindings::new()));
                }
            }
            out
        } else {
            // Non-SExpr: treat as single result, no bindings.
            vec![(collapsed.clone(), GenericBindings::new())]
        };

        // Empty: empty nondet output (HE: no result).
        if pairs.is_empty() {
            self.unreduced = true;
            self.push(self.factory.empty());
            return Ok(());
        }

        // Compose each pair's bindings with current_bindings. Mirrors HE's
        // `b.merge(&bindings)` at lib/src/metta/interpreter.rs:909.
        let outer = self.current_bindings.clone();
        let mut composed_pairs: Vec<(V, GenericBindings<V>)> = Vec::with_capacity(pairs.len());
        for (atom, b) in pairs {
            let composed = if b.is_empty() {
                outer.clone()
            } else if outer.is_empty() {
                b
            } else {
                crate::backend::eval::bindings::compose_outer_inner_generic(&outer, &b, &self.factory)
            };
            composed_pairs.push((atom, composed));
        }

        // Single result: bind composed bindings into current_bindings and push.
        if composed_pairs.len() == 1 {
            let (atom, bindings) = composed_pairs.into_iter().next().expect("non-empty");
            self.current_bindings = bindings;
            self.push(atom);
            return Ok(());
        }

        // Multi: fan out. First alt's bindings replace current_bindings now;
        // subsequent alts saved in BoundValue alternatives, restored on backtrack.
        let mut iter = composed_pairs.into_iter();
        let (first_atom, first_bindings) = iter.next().expect("non-empty");
        let alternatives: Vec<GenericAlternative<V, GenericBytecodeChunk<V>>> = iter
            .map(|(value, bindings)| GenericAlternative::BoundValue { value, bindings })
            .collect();

        self.choice_points.push(GenericChoicePoint {
            ip: self.ip,
            chunk: Arc::clone(&self.chunk),
            value_stack_height: self.value_stack.len(),
            call_stack_height: self.call_stack.len(),
            bindings_stack_height: self.bindings_stack.len(),
            alternatives,
            saved_unreduced: self.unreduced,
            trail_height: self.trail.len(),
            saved_current_bindings: self.current_bindings.clone(),
            locals_height: self.locals.len(),
            locals_base_at_cp: self.locals_base,
        });

        self.current_bindings = first_bindings;
        self.push(first_atom);
        Ok(())
    }

    /// Phase C: backtrack within a `collapse-bind` scope, respecting its
    /// barrier. Mirrors `op_fail_within_collapse` but also handles the
    /// nested-frames interaction — if exhaustion reaches the barrier, we
    /// finalize via the lazy continuation_ip rather than re-entering end.
    fn op_fail_within_collapse_bind(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        let collapse_base = self
            .collapse_bind_frames
            .last()
            .map(|f| f.choice_point_base)
            .unwrap_or(0);

        while let Some(mut cp) = self.choice_points.pop() {
            self.value_stack.truncate(cp.value_stack_height);
            self.call_stack.truncate(cp.call_stack_height);
            self.unwind_trail(cp.trail_height);
            self.bindings_stack.truncate(cp.bindings_stack_height);
            self.unreduced = cp.saved_unreduced;
            // Bug-fix 2026-04-follow-up: restore locals to pre-CP state.
            self.locals.truncate(cp.locals_height);
            self.locals_base = cp.locals_base_at_cp;
            self.current_bindings = cp.saved_current_bindings.clone();

            if cp.alternatives.is_empty() {
                if self.choice_points.len() < collapse_base {
                    break;
                }
                continue;
            }

            let alt = cp.alternatives.remove(0);
            self.ip = cp.ip;
            self.chunk = Arc::clone(&cp.chunk);
            if !cp.alternatives.is_empty() {
                self.choice_points.push(cp);
            }

            match alt {
                GenericAlternative::Value(v) => self.push(v),
                GenericAlternative::Chunk(chunk) => {
                    self.chunk = chunk;
                    self.ip = 0;
                }
                GenericAlternative::Index(offset) => {
                    self.ip = offset;
                }
                GenericAlternative::RuleMatch { chunk, bindings } => {
                    // Bug-fix 2026-04-follow-up: stash caller's locals_base.
                    let caller_locals_base = self.locals_base;
                    let caller_locals_len_snap = self.locals.len();
                    let caller_trail_len_snap = self.trail.len();
                    self.call_stack.push(GenericCallFrame {
                        return_ip: self.ip,
                        return_chunk: Arc::clone(&self.chunk),
                        base_ptr: self.value_stack.len(),
                        bindings_base: self.bindings_stack.len().saturating_sub(1),
                        yield_on_return: false,
                        saved_bindings: self.current_bindings.clone(),
                        locals_base: caller_locals_base,
                        caller_locals_len: caller_locals_len_snap,
                        caller_trail_len: caller_trail_len_snap,
                    });
                    let depth = self.bindings_stack.len() as u32;
                    let mut frame = GenericBindingFrame::new(depth);
                    for (name, val) in bindings.iter() {
                        frame.set(name.to_string(), val.clone());
                    }
                    self.bindings_stack.push(frame);
                    self.chunk = chunk;
                    self.ip = 0;
                    self.locals_base = self.locals.len();
                    self.ensure_locals_for_current_chunk();
                }
                GenericAlternative::BoundValue { value, bindings } => {
                    self.value_stack.push(value);
                    self.current_bindings = bindings;
                }
            }
            return Ok(ControlFlow::Continue(()));
        }

        // All alternatives exhausted within scope — finalize via the
        // lazy continuation_ip captured at first CollapseBindEnd.
        let frame = self.collapse_bind_frames.pop().ok_or_else(|| {
            VmError::Runtime("collapse-bind barrier lost during backtracking".to_string())
        })?;
        let values: Vec<V> = std::mem::take(&mut self.results);
        let bindings_list: Vec<GenericBindings<V>> = std::mem::take(&mut self.per_result_bindings);
        self.results = frame.saved_results;
        self.per_result_bindings = frame.saved_per_result_bindings;
        self.value_stack.truncate(frame.value_stack_height);

        let mut pairs: Vec<V> = Vec::with_capacity(values.len());
        for (i, value) in values.into_iter().enumerate() {
            if value.is_unit() {
                continue;
            }
            let raw_bindings = bindings_list.get(i).cloned().unwrap_or_default();
            let projected = crate::backend::eval::bindings::project_bindings_for_consumer_generic(
                &raw_bindings,
                &[&value],
                Some(&frame.tracked_vars),
                &self.factory,
            )
            .unwrap_or_default();
            let filtered = if projected.iter().any(|(k, _)| k.starts_with("$__fr_")) {
                let mut f = GenericBindings::new();
                for (name, val) in projected.iter() {
                    if !name.starts_with("$__fr_") {
                        f.insert_or_replace(name, val.clone());
                    }
                }
                f
            } else {
                projected
            };
            let bindings_sexpr = crate::backend::eval::bindings::encode_bindings_as_sexpr_generic(
                &filtered,
                &self.factory,
            );
            pairs.push(self.make_sexpr(vec![value, bindings_sexpr]));
        }

        if let Some(cont_chunk) = frame.continuation_chunk {
            self.chunk = cont_chunk;
            self.ip = frame.continuation_ip;
        }
        self.push(self.make_sexpr(pairs));
        Ok(ControlFlow::Continue(()))
    }

    fn op_match_self(&mut self) -> VmResult<()> {
        let template = self.pop()?;
        let pattern = self.pop()?;

        let env = self
            .env
            .as_ref()
            .ok_or_else(|| VmError::Runtime("match: no environment available".to_string()))?;

        let matches = env.match_space(&pattern, &template);

        // Expand multiplicities into flat results
        let flat: Vec<V> = matches
            .into_iter()
            .flat_map(|m| std::iter::repeat(m.value).take(m.count))
            .collect();

        if flat.is_empty() {
            // No matches — return empty (same as tree-walker: empty result set).
            // Signal unreduced so eval_inner falls through to tree-walker which
            // returns empty results.
            self.unreduced = true;
            self.push(self.make_sexpr(vec![]));
        } else if flat.len() == 1 {
            self.push(flat.into_iter().next().expect("flat is non-empty"));
        } else {
            // Multiple matches — Fork-style choice points
            let mut iter = flat.into_iter();
            let first = iter.next().expect("flat is non-empty");
            let alternatives: Vec<GenericAlternative<V, GenericBytecodeChunk<V>>> =
                iter.map(GenericAlternative::Value).collect();
            self.choice_points.push(GenericChoicePoint {
                ip: self.ip,
                chunk: Arc::clone(&self.chunk),
                value_stack_height: self.value_stack.len(),
                call_stack_height: self.call_stack.len(),
                bindings_stack_height: self.bindings_stack.len(),
                alternatives,
                saved_unreduced: self.unreduced,
                trail_height: self.trail.len(),
                saved_current_bindings: self.current_bindings.clone(),
                locals_height: self.locals.len(),
                locals_base_at_cp: self.locals_base,
            });
            self.push(first);
        }
        Ok(())
    }

    /// Native match-or against &self space with default fallback.
    /// Stack: [pattern, default, template] → [result]
    fn op_match_self_or(&mut self) -> VmResult<()> {
        let template = self.pop()?;
        let default = self.pop()?;
        let pattern = self.pop()?;

        let env = self
            .env
            .as_ref()
            .ok_or_else(|| VmError::Runtime("match-or: no environment available".to_string()))?;

        let matches = env.match_space(&pattern, &template);

        let flat: Vec<V> = matches
            .into_iter()
            .flat_map(|m| std::iter::repeat(m.value).take(m.count))
            .collect();

        if flat.is_empty() {
            // No matches — use default
            self.push(default);
        } else if flat.len() == 1 {
            self.push(flat.into_iter().next().expect("flat is non-empty"));
        } else {
            let mut iter = flat.into_iter();
            let first = iter.next().expect("flat is non-empty");
            let alternatives: Vec<GenericAlternative<V, GenericBytecodeChunk<V>>> =
                iter.map(GenericAlternative::Value).collect();
            self.choice_points.push(GenericChoicePoint {
                ip: self.ip,
                chunk: Arc::clone(&self.chunk),
                value_stack_height: self.value_stack.len(),
                call_stack_height: self.call_stack.len(),
                bindings_stack_height: self.bindings_stack.len(),
                alternatives,
                saved_unreduced: self.unreduced,
                trail_height: self.trail.len(),
                saved_current_bindings: self.current_bindings.clone(),
                locals_height: self.locals.len(),
                locals_base_at_cp: self.locals_base,
            });
            self.push(first);
        }
        Ok(())
    }

    // === Set Operations & Alpha-Equivalence ===

    /// case: pattern-matching dispatch.
    /// Stack: [scrutinee], constant pool: case branches -> [result]
    /// Delegates to trampoline via eval_sub_expr_vm.
    /// Phase G: native `case` malformed-fallback. The well-formed-branches case
    /// is handled by `CaseBarrierBegin` + `MatchBind` opcodes emitted by
    /// `compile_case`. `EvalCase` is only reached for branches the compiler
    /// couldn't recognize as pair-structured — handled natively via
    /// `eval_switch`, which reuses the trampoline's pattern-match + template
    /// instantiation logic without dispatching through `eval_sub_expr_vm`.
    /// The instantiated template is pushed onto the stack and flagged as
    /// `unreduced = true` so the surrounding eval loop drives further
    /// reduction in the VM tier.
    fn op_eval_case(&mut self) -> VmResult<()> {
        let case_branches_idx = self.read_u16()?;
        let scrutinee = self.pop()?;

        let case_branches = self
            .chunk
            .get_constant(case_branches_idx)
            .cloned()
            .ok_or(VmError::InvalidConstant(case_branches_idx))?;

        // Only the concrete-typed MettaValue path has a shared `eval_switch`
        // helper; other generic V types fall through to an unreduced push
        // (preserves the pre-native behavior for non-default factories).
        //
        // Safety: `eval_switch` expects `&MettaValue` — we check at runtime via
        // as_any. If V is not MettaValue, fall back to constructing the case
        // expression and signalling unreduced (the eval_inner loop will
        // dispatch correctly from the reconstructed form).
        use crate::backend::eval::trampoline::engine::{eval_switch, SwitchResult};
        use crate::backend::models::MettaValue as ConcreteMV;

        // Attempt native path if V == MettaValue.
        let scrutinee_concrete = (&scrutinee as &dyn std::any::Any).downcast_ref::<ConcreteMV>();
        let branches_concrete = (&case_branches as &dyn std::any::Any).downcast_ref::<ConcreteMV>();
        let factory_concrete = (&self.factory as &dyn std::any::Any)
            .downcast_ref::<crate::backend::models::GcFactory>();

        if let (Some(atom), Some(cases), Some(factory)) =
            (scrutinee_concrete, branches_concrete, factory_concrete)
        {
            match eval_switch(atom, cases, factory) {
                SwitchResult::Match(instantiated, _bindings) => {
                    // Cast the concrete MettaValue back to V (same type).
                    let instantiated_v: &V = (&instantiated as &dyn std::any::Any)
                        .downcast_ref::<V>()
                        .expect(
                            "concrete MettaValue → V downcast must succeed when V == MettaValue",
                        );
                    self.push(instantiated_v.clone());
                    // Instantiated template may require further reduction;
                    // eval_inner will drive it natively within the VM tier.
                    self.unreduced = true;
                    return Ok(());
                }
                SwitchResult::NoMatch => {
                    self.push(self.factory.sexpr(vec![]));
                    return Ok(());
                }
                SwitchResult::Error(err) => {
                    let err_v: &V = (&err as &dyn std::any::Any)
                        .downcast_ref::<V>()
                        .expect("concrete MettaValue error → V downcast must succeed");
                    self.push(err_v.clone());
                    return Ok(());
                }
            }
        }

        // Non-MettaValue V: reconstruct the case expression and push with
        // unreduced flag so the surrounding loop handles it generically.
        let mut items = vec![self.factory.atom("case"), scrutinee];
        if let Some(branch_items) = case_branches.as_sexpr() {
            items.extend(branch_items.iter().cloned());
        } else {
            items.push(case_branches);
        }
        let sexpr = self.factory.sexpr(items);
        self.push(sexpr);
        self.unreduced = true;
        Ok(())
    }

    /// collapse: collect nondeterministic results.
    /// Stack: [expr] -> [result tuple]
    /// Delegates to trampoline via eval_sub_expr_vm for correct nondeterministic handling.
    fn op_eval_collapse(&mut self) -> VmResult<()> {
        let expr = self.pop()?;

        let sexpr = self
            .factory
            .sexpr(vec![self.factory.atom("collapse"), expr]);
        let env = self
            .env
            .clone()
            .ok_or_else(|| VmError::Runtime("collapse: no environment available".to_string()))?;
        let result = self.eval_sub_expr_vm(sexpr, env)?;
        self.push(result);
        Ok(())
    }

    // === Native Collapse (nondeterminism sandboxing) ===

    /// Begin a collapse scope: saves the current nondeterministic context
    /// (results vector and choice point stack height) so that backtracking
    /// within the collapse body cannot escape past this barrier.
    fn op_collapse_begin(&mut self) -> VmResult<()> {
        // Read the i16 relative offset to the instruction after CollapseEnd
        let offset = self.read_u16()? as i16;
        let jump_from = self.ip; // IP is now past the operand bytes
        let continuation_ip = (jump_from as isize + offset as isize) as usize;

        let barrier_id = self.allocate_failure_barrier_id();
        let frame = GenericCollapseFrame {
            saved_results: std::mem::take(&mut self.results),
            choice_point_base: self.choice_points.len(),
            barrier_id,
            value_stack_height: self.value_stack.len(),
            continuation_ip,
            continuation_chunk: Arc::clone(&self.chunk),
        };
        self.collapse_frames.push(frame);
        Ok(())
    }

    /// End a collapse scope: collects all results from the body, restores
    /// the outer nondeterministic context, and pushes the collected results
    /// as an S-expression.
    ///
    /// This opcode is reached in two ways:
    /// 1. Sequential flow — the body produced a single value (no nondeterminism)
    /// 2. After backtracking exhausted all choice points within the collapse
    ///    scope — `op_fail` detected the barrier and jumped here
    ///
    /// Returns `ControlFlow` because it may need to trigger backtracking
    /// within the collapse scope to collect more results.
    fn op_collapse_end(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        // Extract barrier values before mutable operations
        let (value_stack_height, choice_point_base) = {
            let frame = self.collapse_frames.last().ok_or_else(|| {
                VmError::Runtime("CollapseEnd without matching CollapseBegin".to_string())
            })?;
            (frame.value_stack_height, frame.choice_point_base)
        };

        // Collect the current value from the stack (one result from the body).
        // Phase C: snapshot bindings in parallel when inside collapse-bind.
        // Filter Empty sentinels (both `ValueView::Empty` and user-visible
        // `Atom("Empty")`) — HE-bisim §06.4.5; T04/063 verifies.
        if self.value_stack.len() > value_stack_height {
            let value = self.pop()?;
            if !value.is_unit() && !value.is_empty_sentinel() {
                if self.unreduced {
                    self.had_unreduced_result = true;
                }
                self.results.push(value);
                if !self.collapse_bind_frames.is_empty() {
                    self.per_result_bindings.push(self.current_bindings.clone());
                }
            }
        }

        // Check if there are more choice points to exhaust within this scope
        if self.choice_points.len() > choice_point_base {
            // More alternatives exist — backtrack within scope to try them.
            // op_fail_within_collapse restores state from the choice point and
            // returns Continue. The restored IP points to the Fork's resume
            // point, which eventually reaches CollapseEnd again.
            return self.op_fail_within_collapse();
        }

        // All alternatives exhausted — finalize collapse.
        // Filter both Unit and Empty sentinels (HE-bisim §06.4.5).
        let frame = self.collapse_frames.pop().expect("checked above");
        let collected: Vec<V> = std::mem::take(&mut self.results)
            .into_iter()
            .filter(|v| !v.is_unit() && !v.is_empty_sentinel())
            .collect();

        // Restore outer results
        self.results = frame.saved_results;

        // Push collected results as S-expression
        self.push(self.make_sexpr(collected));

        Ok(ControlFlow::Continue(()))
    }

    /// Backtrack within a collapse scope, respecting the barrier.
    /// Same logic as `op_fail` but stops at the collapse frame's `choice_point_base`.
    fn op_fail_within_collapse(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        let collapse_base = self
            .collapse_frames
            .last()
            .map(|f| f.choice_point_base)
            .unwrap_or(0);

        while let Some(mut cp) = self.choice_points.pop() {
            // Restore state
            self.value_stack.truncate(cp.value_stack_height);
            self.call_stack.truncate(cp.call_stack_height);
            self.unwind_trail(cp.trail_height);
            self.bindings_stack.truncate(cp.bindings_stack_height);
            self.unreduced = cp.saved_unreduced;
            // Bug-fix 2026-04-follow-up: restore locals to pre-CP state.
            self.locals.truncate(cp.locals_height);
            self.locals_base = cp.locals_base_at_cp;
            // Phase 1b-E3: restore VM's current_bindings to what it
            // was when this choice point was pushed. A BoundValue alt
            // (below) may OVERWRITE this with its per-alt bindings.
            self.current_bindings = cp.saved_current_bindings.clone();

            if cp.alternatives.is_empty() {
                // No more alternatives at this choice point — continue popping
                // but don't go below the barrier
                if self.choice_points.len() < collapse_base {
                    break;
                }
                continue;
            }

            // Try next alternative
            let alt = cp.alternatives.remove(0);

            // Restore instruction pointer and chunk
            self.ip = cp.ip;
            self.chunk = Arc::clone(&cp.chunk);

            // Put choice point back if more alternatives remain
            if !cp.alternatives.is_empty() {
                self.choice_points.push(cp);
            }

            // Process alternative
            match alt {
                GenericAlternative::Value(v) => self.push(v),
                GenericAlternative::Chunk(chunk) => {
                    self.chunk = chunk;
                    self.ip = 0;
                }
                GenericAlternative::Index(offset) => {
                    self.ip = offset;
                }
                GenericAlternative::RuleMatch { chunk, bindings } => {
                    // Push call frame for compiled RHS (mirrors op_fail logic).
                    // Bug-fix 2026-04-follow-up: stash caller's locals_base.
                    let caller_locals_base = self.locals_base;
                    let caller_locals_len_snap = self.locals.len();
                    let caller_trail_len_snap = self.trail.len();
                    self.call_stack.push(GenericCallFrame {
                        return_ip: self.ip,
                        return_chunk: Arc::clone(&self.chunk),
                        base_ptr: self.value_stack.len(),
                        bindings_base: self.bindings_stack.len().saturating_sub(1),
                        yield_on_return: false,
                        saved_bindings: self.current_bindings.clone(),
                        locals_base: caller_locals_base,
                        caller_locals_len: caller_locals_len_snap,
                        caller_trail_len: caller_trail_len_snap,
                    });
                    let depth = self.bindings_stack.len() as u32;
                    let mut frame = GenericBindingFrame::new(depth);
                    for (name, val) in bindings.iter() {
                        frame.set(name.to_string(), val.clone());
                    }
                    self.bindings_stack.push(frame);
                    self.chunk = chunk;
                    self.ip = 0;
                    self.locals_base = self.locals.len();
                    self.ensure_locals_for_current_chunk();
                }
                GenericAlternative::BoundValue { value, bindings } => {
                    // Phase 1b-A: backtrack to a (value, bindings) pair
                    // produced by an earlier nondeterministic branch.
                    // Restore bindings alongside the value.
                    self.value_stack.push(value);
                    self.current_bindings = bindings;
                }
            }

            return Ok(ControlFlow::Continue(()));
        }

        // No more choice points above barrier — all exhausted.
        // Finalize the collapse and jump to the continuation IP.
        let frame = self.collapse_frames.pop().ok_or_else(|| {
            VmError::Runtime("collapse barrier lost during backtracking".to_string())
        })?;
        let collected: Vec<V> = std::mem::take(&mut self.results)
            .into_iter()
            .filter(|v| !v.is_unit())
            .collect();
        self.results = frame.saved_results;
        self.value_stack.truncate(frame.value_stack_height);
        self.push(self.make_sexpr(collected));
        // Resume execution after CollapseEnd
        self.ip = frame.continuation_ip;
        self.chunk = frame.continuation_chunk;

        Ok(ControlFlow::Continue(()))
    }

    /// if-equal: alpha-equivalence conditional
    /// Stack: [pred1, pred2, then_val, else_val] -> [result]
    fn op_eval_if_equal(&mut self) -> VmResult<()> {
        let else_val = self.pop()?;
        let then_val = self.pop()?;
        let pred2 = self.pop()?;
        let pred1 = self.pop()?;

        // Use alpha-equivalence comparison (matches MeTTa HE semantics)
        let result = if self.alpha_equiv(&pred1, &pred2) {
            then_val
        } else {
            else_val
        };
        self.push(result);
        Ok(())
    }

    /// `unique-atom`: deduplicate a list by **alpha-equivalence**.
    ///
    /// Stack: `[list] -> [deduped_list]`
    ///
    /// **Semantics**: matches **MeTTa HE's `UniqueAtomOp`** (in
    /// `lib/src/metta/runner/stdlib/atom.rs`). Two atoms are duplicates
    /// iff one can be obtained from the other by consistent variable
    /// renaming, and variable-repetition patterns must match.
    ///
    /// `alpha-unique-atom` is an explicit alias of this opcode with
    /// identical semantics. For PeTTa-compatible structural dedup
    /// (where variables with different names are NOT considered equal),
    /// use `struct-unique-atom` (`Opcode::StructUniqueAtom`).
    ///
    /// Note: an earlier in-branch B10 work had temporarily flipped this
    /// opcode to structural equality. That divergence has been reverted
    /// in favor of MeTTa HE-faithfulness.
    fn op_unique_atom(&mut self) -> VmResult<()> {
        let list = self.pop()?;

        // Handle Unit as empty list
        if list.is_unit() {
            self.push(list);
            return Ok(());
        }

        let items = list.as_sexpr().ok_or(VmError::TypeError {
            expected: "S-expression",
            got: "other",
        })?;

        // O(n²) alpha-equivalence dedup (matches MeTTa HE).
        let mut unique: Vec<V> = Vec::with_capacity(items.len());
        for item in items {
            let already_seen = unique.iter().any(|seen| self.alpha_equiv(seen, item));
            if !already_seen {
                unique.push(item.clone());
            }
        }
        self.push(self.make_sexpr(unique));
        Ok(())
    }

    /// `alpha-unique-atom`: explicit alias of `unique-atom`.
    ///
    /// Stack: `[list] -> [deduped_list]`
    ///
    /// Semantics are identical to `op_unique_atom` (alpha-equivalence
    /// dedup, matching MeTTa HE). Kept as a separate opcode so callers
    /// can spell out their intent and the bytecode is self-documenting.
    fn op_alpha_unique_atom(&mut self) -> VmResult<()> {
        // Identical body to op_unique_atom — kept inline rather than
        // delegating to avoid an extra function-call frame in the hot path.
        let list = self.pop()?;

        if list.is_unit() {
            self.push(list);
            return Ok(());
        }

        let items = list.as_sexpr().ok_or(VmError::TypeError {
            expected: "S-expression",
            got: "other",
        })?;

        let mut unique: Vec<V> = Vec::with_capacity(items.len());
        for item in items {
            let already_seen = unique.iter().any(|seen| self.alpha_equiv(seen, item));
            if !already_seen {
                unique.push(item.clone());
            }
        }
        self.push(self.make_sexpr(unique));
        Ok(())
    }

    /// `struct-unique-atom`: deduplicate a list by **structural equality**.
    ///
    /// Stack: `[list] -> [deduped_list]`
    ///
    /// **Semantics**: PeTTa-compatible byte-identity dedup (Rust
    /// `PartialEq`). Variables with the same name match; variables with
    /// different names do NOT. Matches PeTTa's `unique-atom`
    /// (`metta.pl:114`, `list_to_set/2`).
    ///
    /// MeTTaTron's `unique-atom` uses alpha-equivalence (matching MeTTa
    /// HE). `struct-unique-atom` is the explicit name for callers who
    /// want byte-identity semantics.
    fn op_struct_unique_atom(&mut self) -> VmResult<()> {
        let list = self.pop()?;

        if list.is_unit() {
            self.push(list);
            return Ok(());
        }

        let items = list.as_sexpr().ok_or(VmError::TypeError {
            expected: "S-expression",
            got: "other",
        })?;

        // O(n²) structural-equality dedup — matches PeTTa's `list_to_set/2`.
        let mut unique: Vec<V> = Vec::with_capacity(items.len());
        for item in items {
            let already_seen = unique.iter().any(|seen| seen == item);
            if !already_seen {
                unique.push(item.clone());
            }
        }
        self.push(self.make_sexpr(unique));
        Ok(())
    }

    /// `msort`: numeric ascending sort of a tuple.
    ///
    /// Stack: `[tuple] -> [sorted_tuple]`
    ///
    /// PeTTa-compatible. Empty tuple returns empty tuple. All elements
    /// must be numeric (Long or Float); non-numeric elements produce an
    /// error MettaValue. Long and Float values are compared as f64.
    /// Mirrors `eval_msort_generic` in `src/backend/eval/list_ops/ops.rs`.
    fn op_msort(&mut self) -> VmResult<()> {
        let tuple = self.pop()?;

        let elements: Vec<V> = if tuple.is_unit() {
            Vec::new()
        } else if let Some(elems) = tuple.as_sexpr() {
            elems.iter().cloned().collect()
        } else {
            let err = self.make_error("msort: argument must be an expression", tuple);
            self.push(err);
            return Ok(());
        };

        // Pair each element with its numeric key, propagating an error
        // value on the first non-numeric element (matching the
        // tree-walker's eager-error semantics).
        let mut keyed: Vec<(f64, V)> = Vec::with_capacity(elements.len());
        for e in elements {
            let key = if let Some(n) = e.as_long() {
                n as f64
            } else if let Some(f) = e.as_float() {
                f
            } else {
                let err = self.make_error("msort: all elements must be numeric (Long or Float)", e);
                self.push(err);
                return Ok(());
            };
            keyed.push((key, e));
        }

        keyed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        let sorted: Vec<V> = keyed.into_iter().map(|(_, v)| v).collect();
        self.push(self.make_sexpr(sorted));
        Ok(())
    }

    /// union-atom: concatenate two lists
    /// Stack: [left, right] -> [combined]
    fn op_union_atom(&mut self) -> VmResult<()> {
        let right = self.pop()?;
        let left = self.pop()?;

        // Handle Unit as empty list
        let left_items: &[V] = if left.is_unit() {
            &[]
        } else {
            left.as_sexpr().ok_or(VmError::TypeError {
                expected: "S-expression or Unit",
                got: "other",
            })?
        };
        let right_items: &[V] = if right.is_unit() {
            &[]
        } else {
            right.as_sexpr().ok_or(VmError::TypeError {
                expected: "S-expression or Unit",
                got: "other",
            })?
        };

        let mut combined = Vec::with_capacity(left_items.len() + right_items.len());
        combined.extend(left_items.iter().cloned());
        combined.extend(right_items.iter().cloned());
        self.push(self.make_sexpr(combined));
        Ok(())
    }

    /// intersection-atom: multiset intersection
    /// Stack: [left, right] -> [intersection]
    fn op_intersection_atom(&mut self) -> VmResult<()> {
        let right = self.pop()?;
        let left = self.pop()?;

        // Handle Unit as empty list
        let left_items: &[V] = if left.is_unit() {
            &[]
        } else {
            left.as_sexpr().ok_or(VmError::TypeError {
                expected: "S-expression or Unit",
                got: "other",
            })?
        };
        let right_items: &[V] = if right.is_unit() {
            &[]
        } else {
            right.as_sexpr().ok_or(VmError::TypeError {
                expected: "S-expression or Unit",
                got: "other",
            })?
        };

        // Build count map from right list (structural equality)
        let mut right_remaining: Vec<(V, usize)> = Vec::new();
        for item in right_items {
            let mut found = false;
            for entry in right_remaining.iter_mut() {
                if entry.0 == *item {
                    entry.1 += 1;
                    found = true;
                    break;
                }
            }
            if !found {
                right_remaining.push((item.clone(), 1));
            }
        }

        // Iterate left, emit if found in right (decrementing count)
        let mut result = Vec::new();
        for item in left_items {
            for entry in right_remaining.iter_mut() {
                if entry.0 == *item && entry.1 > 0 {
                    entry.1 -= 1;
                    result.push(item.clone());
                    break;
                }
            }
        }
        self.push(self.make_sexpr(result));
        Ok(())
    }

    /// subtraction-atom: multiset subtraction
    /// Stack: [left, right] -> [difference]
    fn op_subtraction_atom(&mut self) -> VmResult<()> {
        let right = self.pop()?;
        let left = self.pop()?;

        // Handle Unit as empty list
        let left_items: &[V] = if left.is_unit() {
            &[]
        } else {
            left.as_sexpr().ok_or(VmError::TypeError {
                expected: "S-expression or Unit",
                got: "other",
            })?
        };
        let right_items: &[V] = if right.is_unit() {
            &[]
        } else {
            right.as_sexpr().ok_or(VmError::TypeError {
                expected: "S-expression or Unit",
                got: "other",
            })?
        };

        // Build count map from right list (structural equality)
        let mut right_remaining: Vec<(V, usize)> = Vec::new();
        for item in right_items {
            let mut found = false;
            for entry in right_remaining.iter_mut() {
                if entry.0 == *item {
                    entry.1 += 1;
                    found = true;
                    break;
                }
            }
            if !found {
                right_remaining.push((item.clone(), 1));
            }
        }

        // Iterate left, skip items found in right (decrementing count)
        let mut result = Vec::new();
        for item in left_items {
            let mut subtracted = false;
            for entry in right_remaining.iter_mut() {
                if entry.0 == *item && entry.1 > 0 {
                    entry.1 -= 1;
                    subtracted = true;
                    break;
                }
            }
            if !subtracted {
                result.push(item.clone());
            }
        }
        self.push(self.make_sexpr(result));
        Ok(())
    }

    // === Tuple & List Operations ===

    /// tuple-concat: concatenate two tuples
    /// Stack: [a, b] -> [combined]
    fn op_tuple_concat(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;

        // Handle Unit as empty list
        let a_items: &[V] = if a.is_unit() {
            &[]
        } else {
            a.as_sexpr().ok_or(VmError::TypeError {
                expected: "S-expression or Unit",
                got: "other",
            })?
        };
        let b_items: &[V] = if b.is_unit() {
            &[]
        } else {
            b.as_sexpr().ok_or(VmError::TypeError {
                expected: "S-expression or Unit",
                got: "other",
            })?
        };

        let mut combined = Vec::with_capacity(a_items.len() + b_items.len());
        combined.extend(a_items.iter().cloned());
        combined.extend(b_items.iter().cloned());
        self.push(self.make_sexpr(combined));
        Ok(())
    }

    /// tuple-count: count elements in tuple
    /// Stack: [tuple] -> [count]
    fn op_tuple_count(&mut self) -> VmResult<()> {
        let tuple = self.pop()?;

        // Handle Unit as empty list (count = 0)
        let items: &[V] = if tuple.is_unit() {
            &[]
        } else {
            tuple.as_sexpr().ok_or(VmError::TypeError {
                expected: "S-expression or Unit",
                got: "other",
            })?
        };
        self.push(self.make_long(items.len() as i64));
        Ok(())
    }

    /// without: remove all occurrences of elem from tuple
    /// Stack: [tuple, elem] -> [filtered]
    fn op_without(&mut self) -> VmResult<()> {
        let elem = self.pop()?;
        let tuple = self.pop()?;

        // Handle Unit as empty list
        let items: &[V] = if tuple.is_unit() {
            &[]
        } else {
            tuple.as_sexpr().ok_or(VmError::TypeError {
                expected: "S-expression or Unit",
                got: "other",
            })?
        };
        let filtered: Vec<V> = items
            .iter()
            .filter(|item| **item != elem)
            .cloned()
            .collect();
        self.push(self.make_sexpr(filtered));
        Ok(())
    }

    /// element-of: membership test
    /// Stack: [elem, tuple] -> [bool]
    fn op_element_of(&mut self) -> VmResult<()> {
        let tuple = self.pop()?;
        let elem = self.pop()?;

        // Handle Unit as empty list (nothing is an element of empty)
        if tuple.is_unit() {
            self.push(self.make_bool(false));
            return Ok(());
        }

        let items = tuple.as_sexpr().ok_or(VmError::TypeError {
            expected: "S-expression",
            got: "other",
        })?;
        let found = items.iter().any(|item| *item == elem);
        self.push(self.make_bool(found));
        Ok(())
    }

    /// range: generate integer range [start, end)
    /// Stack: [start, end] -> [tuple]
    fn op_range(&mut self) -> VmResult<()> {
        let end = self.pop()?;
        let start = self.pop()?;
        let s = start.as_long().ok_or(VmError::TypeError {
            expected: "Long",
            got: "other",
        })?;
        let e = end.as_long().ok_or(VmError::TypeError {
            expected: "Long",
            got: "other",
        })?;
        if s >= e {
            self.push(self.make_sexpr(vec![]));
        } else {
            let count = (e - s) as usize;
            let mut elems = Vec::with_capacity(count);
            for i in s..e {
                elems.push(self.make_long(i));
            }
            self.push(self.make_sexpr(elems));
        }
        Ok(())
    }

    /// reverse-atom: reverse a tuple
    /// Stack: [tuple] -> [reversed]
    fn op_reverse_atom(&mut self) -> VmResult<()> {
        let tuple = self.pop()?;
        let items = tuple.as_sexpr().ok_or(VmError::TypeError {
            expected: "S-expression",
            got: "other",
        })?;
        let reversed: Vec<V> = items.iter().rev().cloned().collect();
        self.push(self.make_sexpr(reversed));
        Ok(())
    }

    /// flatten-atom: flatten one level of nesting
    /// Stack: [nested] -> [flat]
    fn op_flatten_atom(&mut self) -> VmResult<()> {
        let nested = self.pop()?;
        let items = nested.as_sexpr().ok_or(VmError::TypeError {
            expected: "S-expression",
            got: "other",
        })?;
        let mut flat = Vec::new();
        for item in items {
            match item.as_sexpr() {
                Some(inner) => flat.extend(inner.iter().cloned()),
                None => flat.push(item.clone()),
            }
        }
        self.push(self.make_sexpr(flat));
        Ok(())
    }

    /// zip-atom: pair-wise zip of two tuples
    /// Stack: [a, b] -> [pairs]
    fn op_zip_atom(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let a_items = a.as_sexpr().ok_or(VmError::TypeError {
            expected: "S-expression",
            got: "other",
        })?;
        let b_items = b.as_sexpr().ok_or(VmError::TypeError {
            expected: "S-expression",
            got: "other",
        })?;
        let min_len = a_items.len().min(b_items.len());
        let mut pairs = Vec::with_capacity(min_len);
        for i in 0..min_len {
            pairs.push(self.make_sexpr(vec![a_items[i].clone(), b_items[i].clone()]));
        }
        self.push(self.make_sexpr(pairs));
        Ok(())
    }

    /// take-atom: first n elements of tuple
    /// Stack: [tuple, n] -> [prefix]
    fn op_take_atom(&mut self) -> VmResult<()> {
        let n_val = self.pop()?;
        let tuple = self.pop()?;
        let items = tuple.as_sexpr().ok_or(VmError::TypeError {
            expected: "S-expression",
            got: "other",
        })?;
        let n = n_val.as_long().ok_or(VmError::TypeError {
            expected: "Long",
            got: "other",
        })?;
        let take_count = if n < 0 {
            0
        } else {
            (n as usize).min(items.len())
        };
        let taken: Vec<V> = items[..take_count].to_vec();
        self.push(self.make_sexpr(taken));
        Ok(())
    }

    /// drop-atom: skip first n elements of tuple
    /// Stack: [tuple, n] -> [suffix]
    fn op_drop_atom(&mut self) -> VmResult<()> {
        let n_val = self.pop()?;
        let tuple = self.pop()?;
        let items = tuple.as_sexpr().ok_or(VmError::TypeError {
            expected: "S-expression",
            got: "other",
        })?;
        let n = n_val.as_long().ok_or(VmError::TypeError {
            expected: "Long",
            got: "other",
        })?;
        let drop_count = if n < 0 {
            0
        } else {
            (n as usize).min(items.len())
        };
        let remaining: Vec<V> = items[drop_count..].to_vec();
        self.push(self.make_sexpr(remaining));
        Ok(())
    }

    /// Alpha-equivalence check for VM values.
    /// Two values are alpha-equivalent if they are structurally identical
    /// except that $-prefixed variables can be consistently renamed.
    fn alpha_equiv(&self, a: &V, b: &V) -> bool {
        let mut l2r: HashMap<String, String> = HashMap::new();
        let mut r2l: HashMap<String, String> = HashMap::new();
        self.alpha_equiv_inner(a, b, &mut l2r, &mut r2l)
    }

    /// **Stack-safety mandate (2026-05-15)**: refactored to iterative pair-stack.
    /// Audit item T2.2.
    fn alpha_equiv_inner(
        &self,
        a: &V,
        b: &V,
        l2r: &mut std::collections::HashMap<String, String>,
        r2l: &mut std::collections::HashMap<String, String>,
    ) -> bool {
        let mut work: Vec<(V, V)> = Vec::with_capacity(8);
        work.push((a.clone(), b.clone()));

        while let Some((a, b)) = work.pop() {
            if a == b {
                continue;
            }
            if let (Some(sa), Some(sb)) = (a.as_atom(), b.as_atom()) {
                let a_is_var = sa.starts_with('$');
                let b_is_var = sb.starts_with('$');
                if a_is_var && b_is_var {
                    match l2r.get(sa) {
                        Some(mapped) => {
                            if mapped != sb {
                                return false;
                            }
                        }
                        None => {
                            l2r.insert(sa.to_string(), sb.to_string());
                        }
                    }
                    match r2l.get(sb) {
                        Some(mapped) => {
                            if mapped != sa {
                                return false;
                            }
                        }
                        None => {
                            r2l.insert(sb.to_string(), sa.to_string());
                        }
                    }
                    continue;
                }
                if sa != sb {
                    return false;
                }
                continue;
            }
            if let (Some(items_a), Some(items_b)) = (a.as_sexpr(), b.as_sexpr()) {
                if items_a.len() != items_b.len() {
                    return false;
                }
                for (ia, ib) in items_a.iter().zip(items_b.iter()).rev() {
                    work.push((ia.clone(), ib.clone()));
                }
                continue;
            }
            if let (Some(ba), Some(bb)) = (a.as_bool(), b.as_bool()) {
                if ba != bb {
                    return false;
                }
                continue;
            }
            if let (Some(la), Some(lb)) = (a.as_long(), b.as_long()) {
                if la != lb {
                    return false;
                }
                continue;
            }
            if let (Some(fa), Some(fb)) = (a.as_float(), b.as_float()) {
                if fa != fb {
                    return false;
                }
                continue;
            }
            if let (Some(sa), Some(sb)) = (a.as_string(), b.as_string()) {
                if sa != sb {
                    return false;
                }
                continue;
            }
            if a.is_unit() && b.is_unit() {
                continue;
            }
            return false;
        }
        true
    }

    // === Nondeterminism Stubs ===

    fn op_fork(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        trace!(target: "mettatron::vm::nondet", ip = self.ip, "fork");
        let count = self.read_u16()? as usize;
        if count == 0 {
            // Zero alternatives means immediate failure
            return self.op_fail();
        }

        // Read constant indices from bytecode (compiler emits u16 indices after Fork)
        let mut alternatives = Vec::with_capacity(count);
        for _ in 0..count {
            let const_idx = self.read_u16()?;
            let value = self
                .chunk
                .get_constant(const_idx)
                .ok_or(VmError::InvalidConstant(const_idx))?
                .clone();
            alternatives.push(GenericAlternative::Value(value));
        }

        // Trace: NondeterministicFork
        #[cfg(feature = "trace")]
        {
            use crate::backend::trace::thread_local_sink::with_thread_trace_collector;
            with_thread_trace_collector(|tc| {
                tc.emit_converted(
                    trace_format::TraceTier::BytecodeVM,
                    0,
                    trace_format::TraceValue::Unit,
                    vec![],
                    None,
                    trace_format::TraceEventKind::NondeterministicFork {
                        branch_count: count as u32,
                    },
                );
            });
        }

        // Save IP pointing past all constant indices (where execution should resume)
        let resume_ip = self.ip;

        // Create choice point with remaining alternatives
        if alternatives.len() > 1 {
            let cp = GenericChoicePoint {
                value_stack_height: self.value_stack.len(),
                call_stack_height: self.call_stack.len(),
                bindings_stack_height: self.bindings_stack.len(),
                ip: resume_ip,
                chunk: Arc::clone(&self.chunk),
                alternatives: alternatives[1..].to_vec(),
                saved_unreduced: self.unreduced,
                trail_height: self.trail.len(),
                saved_current_bindings: self.current_bindings.clone(),
                locals_height: self.locals.len(),
                locals_base_at_cp: self.locals_base,
            };
            self.choice_points.push(cp);
        }

        // Push first alternative and continue execution
        if let GenericAlternative::Value(v) = &alternatives[0] {
            self.push(v.clone());
        }

        Ok(ControlFlow::Continue(()))
    }

    fn op_fork_inline(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        trace!(target: "mettatron::vm::nondet", ip = self.ip, "fork_inline");
        let count = self.read_u16()? as usize;
        if count == 0 {
            return self.op_fail();
        }

        let mut targets = Vec::with_capacity(count);
        for _ in 0..count {
            targets.push(self.read_u16()? as usize);
        }

        #[cfg(feature = "trace")]
        {
            use crate::backend::trace::thread_local_sink::with_thread_trace_collector;
            with_thread_trace_collector(|tc| {
                tc.emit_converted(
                    trace_format::TraceTier::BytecodeVM,
                    0,
                    trace_format::TraceValue::Unit,
                    vec![],
                    None,
                    trace_format::TraceEventKind::NondeterministicFork {
                        branch_count: count as u32,
                    },
                );
            });
        }

        if targets.len() > 1 {
            let alternatives = targets[1..]
                .iter()
                .copied()
                .map(GenericAlternative::Index)
                .collect();
            self.choice_points.push(GenericChoicePoint {
                ip: self.ip,
                chunk: Arc::clone(&self.chunk),
                value_stack_height: self.value_stack.len(),
                call_stack_height: self.call_stack.len(),
                bindings_stack_height: self.bindings_stack.len(),
                alternatives,
                saved_unreduced: self.unreduced,
                trail_height: self.trail.len(),
                saved_current_bindings: self.current_bindings.clone(),
                locals_height: self.locals.len(),
                locals_base_at_cp: self.locals_base,
            });
        }

        self.ip = targets[0];
        Ok(ControlFlow::Continue(()))
    }

    fn op_eval_superpose(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        trace!(target: "mettatron::vm::nondet", ip = self.ip, "eval_superpose");

        let value = self.pop()?;
        let alternatives = if value.is_unit() {
            Vec::new()
        } else if let Some(items) = value.as_sexpr() {
            items.to_vec()
        } else {
            self.push(value);
            return Ok(ControlFlow::Continue(()));
        };

        if alternatives.is_empty() {
            return self.op_fail();
        }

        #[cfg(feature = "trace")]
        {
            use crate::backend::trace::thread_local_sink::with_thread_trace_collector;
            with_thread_trace_collector(|tc| {
                tc.emit_converted(
                    trace_format::TraceTier::BytecodeVM,
                    0,
                    trace_format::TraceValue::Unit,
                    vec![],
                    None,
                    trace_format::TraceEventKind::NondeterministicFork {
                        branch_count: alternatives.len() as u32,
                    },
                );
            });
        }

        if alternatives.len() > 1 {
            let remaining: Vec<GenericAlternative<V, GenericBytecodeChunk<V>>> = alternatives[1..]
                .iter()
                .cloned()
                .map(GenericAlternative::Value)
                .collect();
            self.choice_points.push(GenericChoicePoint {
                value_stack_height: self.value_stack.len(),
                call_stack_height: self.call_stack.len(),
                bindings_stack_height: self.bindings_stack.len(),
                ip: self.ip,
                chunk: Arc::clone(&self.chunk),
                alternatives: remaining,
                saved_unreduced: self.unreduced,
                trail_height: self.trail.len(),
                saved_current_bindings: self.current_bindings.clone(),
                locals_height: self.locals.len(),
                locals_base_at_cp: self.locals_base,
            });
        }

        self.push(alternatives.into_iter().next().expect("non-empty checked"));
        Ok(ControlFlow::Continue(()))
    }

    fn op_fail(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        trace!(target: "mettatron::vm::nondet", ip = self.ip, choice_points = self.choice_points.len(), "fail");

        // Respect the nearest active failure barrier. Choice-point stack
        // heights identify most nesting; barrier ids break ties when two
        // barriers were entered at the same choice-point depth.
        let mut active_barrier: Option<(usize, u64, FailureBarrier)> = None;
        let mut consider_barrier =
            |floor: usize, barrier_id: u64, kind: FailureBarrier| match active_barrier {
                Some((active_floor, active_id, _))
                    if active_floor > floor
                        || (active_floor == floor && active_id > barrier_id) => {}
                _ => active_barrier = Some((floor, barrier_id, kind)),
            };

        if let Some(barrier) = self.case_barrier_frames.last() {
            consider_barrier(
                barrier.choice_point_floor,
                barrier.barrier_id,
                FailureBarrier::Case,
            );
        }
        if let Some(frame) = self.collapse_frames.last() {
            consider_barrier(
                frame.choice_point_base,
                frame.barrier_id,
                FailureBarrier::Collapse,
            );
        }

        let barrier_floor = active_barrier.map(|(floor, _, _)| floor).unwrap_or(0);

        // Backtrack to most recent choice point (above barrier floor)
        while self.choice_points.len() > barrier_floor {
            let mut cp = self.choice_points.pop().expect("len > floor");
            // Restore state
            self.value_stack.truncate(cp.value_stack_height);
            self.call_stack.truncate(cp.call_stack_height);
            self.unwind_trail(cp.trail_height);
            self.bindings_stack.truncate(cp.bindings_stack_height);
            self.unreduced = cp.saved_unreduced;
            // Bug-fix 2026-04-follow-up: restore locals to pre-CP state.
            self.locals.truncate(cp.locals_height);
            self.locals_base = cp.locals_base_at_cp;
            // Phase 1b-E3: restore VM's current_bindings to what it
            // was when this choice point was pushed. The picked
            // alternative (below) may OVERWRITE this for BoundValue
            // alts — that's intentional: per-alt bindings are the
            // authoritative ambient for that alternative's branch.
            self.current_bindings = cp.saved_current_bindings.clone();

            if cp.alternatives.is_empty() {
                // No more alternatives at this choice point
                continue;
            }

            // Try next alternative (remove first, preserving order)
            let alt = cp.alternatives.remove(0);

            // Restore instruction pointer and chunk from choice point
            self.ip = cp.ip;
            self.chunk = Arc::clone(&cp.chunk);

            // Put choice point back if more alternatives remain
            if !cp.alternatives.is_empty() {
                self.choice_points.push(cp);
            }

            // Process alternative
            match alt {
                GenericAlternative::Value(v) => self.push(v),
                GenericAlternative::Chunk(chunk) => {
                    self.chunk = chunk;
                    self.ip = 0;
                }
                GenericAlternative::Index(offset) => {
                    // Set IP to indexed offset for computed gotos / rule dispatch
                    self.ip = offset;
                }
                GenericAlternative::RuleMatch { chunk, bindings } => {
                    // Push call frame for compiled RHS execution.
                    // The RHS returns normally and the calling chunk continues.
                    // Bug-fix 2026-04-follow-up: stash caller's locals_base.
                    let caller_locals_base = self.locals_base;
                    let caller_locals_len_snap = self.locals.len();
                    let caller_trail_len_snap = self.trail.len();
                    self.call_stack.push(GenericCallFrame {
                        return_ip: self.ip,
                        return_chunk: Arc::clone(&self.chunk),
                        base_ptr: self.value_stack.len(),
                        bindings_base: self.bindings_stack.len().saturating_sub(1),
                        yield_on_return: false,
                        saved_bindings: self.current_bindings.clone(),
                        caller_locals_len: caller_locals_len_snap,
                        caller_trail_len: caller_trail_len_snap,
                        locals_base: caller_locals_base,
                    });
                    // Push new binding frame (don't pollute existing frames)
                    let depth = self.bindings_stack.len() as u32;
                    let mut frame = GenericBindingFrame::new(depth);
                    for (name, val) in bindings.iter() {
                        frame.set(name.to_string(), val.clone());
                    }
                    self.bindings_stack.push(frame);
                    self.chunk = chunk;
                    self.ip = 0;
                    self.locals_base = self.locals.len();
                    self.ensure_locals_for_current_chunk();
                }
                GenericAlternative::BoundValue { value, bindings } => {
                    // Phase 1b-A: restore (value, bindings) pair together.
                    // Phase 1b-E3: overwrites saved_current_bindings
                    // restored above — per-alt bindings take priority.
                    self.value_stack.push(value);
                    self.current_bindings = bindings;
                }
            }

            return Ok(ControlFlow::Continue(()));
        }

        // No choice points above the nearest barrier floor. Let that barrier
        // handle the failure rather than escaping to the enclosing scope.
        if let Some((_, _, kind)) = active_barrier {
            match kind {
                FailureBarrier::Case => {
                    let barrier = self.case_barrier_frames.pop().ok_or_else(|| {
                        VmError::Runtime("case barrier lost during backtracking".to_string())
                    })?;
                    self.value_stack.truncate(barrier.value_stack_height);
                    self.call_stack.truncate(barrier.call_stack_height);
                    self.bindings_stack.truncate(barrier.bindings_stack_height);
                    self.choice_points.truncate(barrier.choice_point_floor);
                    self.unreduced = barrier.saved_unreduced;
                    self.ip = barrier.handler_ip;
                    return Ok(ControlFlow::Continue(()));
                }
                FailureBarrier::Collapse => {
                    return self.op_fail_within_collapse();
                }
            }
        }

        // No more choice points or barriers - return collected results
        Ok(ControlFlow::Break(std::mem::take(&mut self.results)))
    }

    fn op_case_barrier_begin(&mut self) -> VmResult<()> {
        let offset = self.read_u16()? as i16;
        let jump_from = self.ip;
        let handler_ip = (jump_from as isize + offset as isize) as usize;
        let barrier_id = self.allocate_failure_barrier_id();
        self.case_barrier_frames.push(CaseBarrierFrame {
            handler_ip,
            choice_point_floor: self.choice_points.len(),
            barrier_id,
            value_stack_height: self.value_stack.len(),
            call_stack_height: self.call_stack.len(),
            bindings_stack_height: self.bindings_stack.len(),
            saved_unreduced: self.unreduced,
        });
        Ok(())
    }

    fn op_case_barrier_end(&mut self) -> VmResult<()> {
        self.case_barrier_frames.pop().ok_or_else(|| {
            VmError::Runtime("CaseBarrierEnd without matching CaseBarrierBegin".into())
        })?;
        Ok(())
    }

    fn op_cut(&mut self) {
        // Remove all choice points
        self.choice_points.clear();
    }

    /// Collect all nondeterministic results from current evaluation.
    /// The chunk_index parameter is reserved for future use (sub-chunk execution).
    /// Currently, this collects all results accumulated via Yield and returns them as SExpr.
    ///
    /// Stack: [] -> [SExpr of collected results]
    fn op_collect(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::nondet", ip = self.ip, results = self.results.len(), "collect");
        let _chunk_index = self.read_u16()?;

        // Collect all results accumulated so far via Yield
        // Filter out Unit values (matches collapse semantics)
        let collected: Vec<V> = std::mem::take(&mut self.results)
            .into_iter()
            .filter(|v| !v.is_unit())
            .collect();

        // Push the collected results as a single S-expression
        self.push(self.make_sexpr(collected));
        Ok(())
    }

    /// Collect up to N nondeterministic results.
    /// Stack: [] -> [SExpr of collected results (up to N)]
    fn op_collect_n(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::nondet", ip = self.ip, "collect_n");
        let n = self.read_u8()? as usize;

        // Take up to N results, filtering out Unit values
        let collected: Vec<V> = std::mem::take(&mut self.results)
            .into_iter()
            .filter(|v| !v.is_unit())
            .take(n)
            .collect();

        // Push the collected results as a single S-expression
        self.push(self.make_sexpr(collected));
        Ok(())
    }

    fn op_yield(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        let value = self.pop()?;
        if self.unreduced {
            self.had_unreduced_result = true;
        }
        self.results.push(value);
        // Phase C: record bindings snapshot inside a collapse-bind scope.
        if !self.collapse_bind_frames.is_empty() {
            self.per_result_bindings.push(self.current_bindings.clone());
        }
        // Continue to next alternative — respect collapse-bind barrier if active.
        if !self.collapse_bind_frames.is_empty() {
            self.op_fail_within_collapse_bind()
        } else {
            self.op_fail()
        }
    }

    fn op_begin_nondet(&mut self) {
        // Mark beginning of nondeterministic section
        // Nothing to do in simple implementation
    }

    fn op_end_nondet(&mut self) -> VmResult<()> {
        // End nondeterministic section
        Ok(())
    }

    /// Amb - ambiguous choice from N alternatives on stack.
    /// Creates a choice point with alternatives 2..N and returns alternative 1.
    /// Stack: [alt1, alt2, ..., altN] -> [selected]
    fn op_amb(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::nondet", ip = self.ip, "amb");
        let count = self.read_u8()? as usize;

        if count == 0 {
            // Empty amb - push Unit (will fail on subsequent op_fail)
            self.push(self.make_unit());
            return Ok(());
        }

        // Pop all alternatives
        let mut alts = Vec::with_capacity(count);
        for _ in 0..count {
            alts.push(self.pop()?);
        }
        alts.reverse(); // Now in original order: [alt1, alt2, ..., altN]

        if count == 1 {
            // Single alternative - no choice point needed
            self.push(alts.into_iter().next().expect("count checked"));
            return Ok(());
        }

        // Create choice point with alternatives 1..N (skipping first)
        let alternatives: Vec<GenericAlternative<V, GenericBytecodeChunk<V>>> = alts[1..]
            .iter()
            .cloned()
            .map(GenericAlternative::Value)
            .collect();

        self.choice_points.push(GenericChoicePoint {
            ip: self.ip, // Resume at current IP for alternatives
            chunk: Arc::clone(&self.chunk),
            value_stack_height: self.value_stack.len(), // After popping alts
            call_stack_height: self.call_stack.len(),
            bindings_stack_height: self.bindings_stack.len(),
            alternatives,
            saved_unreduced: self.unreduced,
            trail_height: self.trail.len(),
            saved_current_bindings: self.current_bindings.clone(),
            locals_height: self.locals.len(),
            locals_base_at_cp: self.locals_base,
        });

        // Push first alternative
        self.push(alts.into_iter().next().expect("count checked"));

        Ok(())
    }

    fn op_guard(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        let guard = self.pop()?;
        match guard.as_bool() {
            Some(true) => Ok(ControlFlow::Continue(())),
            Some(false) => self.op_fail(),
            None => Err(VmError::TypeError {
                expected: "Bool",
                got: guard.type_name(),
            }),
        }
    }

    /// Commit - remove N choice points (soft cut).
    /// If count is 0, remove all choice points (like full cut).
    /// Stack: [] -> []
    fn op_commit(&mut self) {
        trace!(target: "mettatron::vm::nondet", ip = self.ip, "commit");
        let count = self.read_u8().unwrap_or(0);
        if count == 0 {
            // Remove all choice points (full cut)
            self.choice_points.clear();
        } else {
            // Remove N most recent choice points
            let to_remove = (count as usize).min(self.choice_points.len());
            let new_len = self.choice_points.len().saturating_sub(to_remove);
            self.choice_points.truncate(new_len);
        }
    }

    // === Advanced Calls ===

    /// Call a native Rust function by ID.
    /// Stack: [arg1, arg2, ..., argN] -> [result]
    fn op_call_native(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::call", ip = self.ip, "call_native (generic)");

        let func_id = self.read_u16()?;
        let arity = self.read_u8()? as usize;

        // Pop arguments in reverse order
        let mut args = Vec::with_capacity(arity);
        for _ in 0..arity {
            args.push(self.pop()?);
        }
        args.reverse();

        // Create context for native function
        let env = self
            .env
            .clone()
            .unwrap_or_else(|| GenericEnvironment::new(self.factory.clone()));
        let ctx = GenericNativeContext::new(env, self.factory.clone());

        // Call through registry
        let call_result = self.native_registry.call(func_id, &args, &ctx);

        match call_result {
            Ok(result) => {
                // Trace: GroundedOp success
                #[cfg(feature = "trace")]
                {
                    use crate::backend::trace::thread_local_sink::with_thread_trace_collector;
                    use crate::backend::trace::trace_value_generic;
                    with_thread_trace_collector(|tc| {
                        let op_name = self
                            .native_registry
                            .name_for_id(func_id)
                            .unwrap_or("?")
                            .to_string();
                        tc.emit_converted(
                            trace_format::TraceTier::BytecodeVM,
                            0,
                            trace_format::TraceValue::SExpr(
                                std::iter::once(trace_format::TraceValue::Atom(op_name.clone()))
                                    .chain(args.iter().map(|a| trace_value_generic(a)))
                                    .collect(),
                            ),
                            result.iter().map(|v| trace_value_generic(v)).collect(),
                            None,
                            trace_format::TraceEventKind::GroundedOp {
                                op_name,
                                args: args.iter().map(|a| trace_value_generic(a)).collect(),
                            },
                        );
                    });
                }

                // Push result(s)
                if result.len() == 1 {
                    self.push(result.into_iter().next().expect("result has 1 element"));
                } else if result.is_empty() {
                    self.push(self.factory.unit());
                } else {
                    // Multiple results - push as S-expression
                    self.push(self.factory.sexpr(result));
                }

                Ok(())
            }
            Err(e) => {
                // Trace: GroundedOpError
                #[cfg(feature = "trace")]
                {
                    use crate::backend::trace::thread_local_sink::with_thread_trace_collector;
                    use crate::backend::trace::trace_value_generic;
                    with_thread_trace_collector(|tc| {
                        let op_name = self
                            .native_registry
                            .name_for_id(func_id)
                            .unwrap_or("?")
                            .to_string();
                        tc.emit_converted(
                            trace_format::TraceTier::BytecodeVM,
                            0,
                            trace_format::TraceValue::SExpr(
                                std::iter::once(trace_format::TraceValue::Atom(op_name.clone()))
                                    .chain(args.iter().map(|a| trace_value_generic(a)))
                                    .collect(),
                            ),
                            vec![],
                            None,
                            trace_format::TraceEventKind::GroundedOpError {
                                op_name,
                                error_kind: "Runtime".to_string(),
                                message: e.to_string(),
                                args: args.iter().map(|a| trace_value_generic(a)).collect(),
                            },
                        );
                    });
                }

                Err(VmError::Runtime(e.to_string()))
            }
        }
    }

    /// Call an external FFI function by name.
    /// Stack: [arg1, arg2, ..., argN] -> [result]
    fn op_call_external(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::call", ip = self.ip, "call_external (generic)");

        let symbol_idx = self.read_u16()?;
        let arity = self.read_u8()? as usize;

        // Get function name from constant pool
        let func_name = self
            .chunk
            .get_constant(symbol_idx)
            .and_then(|v| v.as_atom().map(|s| s.to_string()))
            .ok_or(VmError::InvalidConstant(symbol_idx))?;

        // Pop arguments in reverse order
        let mut args = Vec::with_capacity(arity);
        for _ in 0..arity {
            args.push(self.pop()?);
        }
        args.reverse();

        // Create context for external function
        let env = self
            .env
            .clone()
            .unwrap_or_else(|| GenericEnvironment::new(self.factory.clone()));
        let ctx = GenericExternalContext::new(env, self.factory.clone());

        // Call through registry
        match self.external_registry.call(&func_name, &args, &ctx) {
            Ok(results) => {
                if results.len() == 1 {
                    self.push(results.into_iter().next().expect("results has 1 element"));
                } else if results.is_empty() {
                    self.push(self.factory.unit());
                } else {
                    self.push(self.factory.sexpr(results));
                }
                Ok(())
            }
            Err(ExternalError::NotFound(_)) => Err(VmError::Runtime(format!(
                "External function '{}' not registered",
                func_name
            ))),
            Err(e) => Err(VmError::Runtime(format!("External call error: {}", e))),
        }
    }

    /// Call a function with memoization.
    /// Stack: [arg1, arg2, ..., argN] -> [result]
    ///
    /// Checks the memo cache first. On miss, builds the call expression,
    /// dispatches via environment rules, and caches the result.
    fn op_call_cached(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::call", ip = self.ip, "call_cached (generic)");

        let head_idx = self.read_u16()?;
        let arity = self.read_u8()? as usize;

        let head = self
            .chunk
            .get_constant(head_idx)
            .cloned()
            .ok_or(VmError::InvalidConstant(head_idx))?;

        // Extract head as string for cache key
        let head_str = head
            .as_atom()
            .map(|s| s.to_string())
            .unwrap_or_else(|| head.type_name().to_string());

        // Pop arguments in reverse order
        let mut args = Vec::with_capacity(arity);
        for _ in 0..arity {
            args.push(self.pop()?);
        }
        args.reverse();

        // Check memo cache first
        if let Some(cached) = self.memo_cache.get(&head_str, &args) {
            self.push(cached);
            return Ok(());
        }

        // Build the call expression
        let mut items = Vec::with_capacity(arity + 1);
        items.push(head);
        items.extend(args.clone());
        let expr = self.factory.sexpr(items);

        // Dispatch via environment rules (push expr, dispatch pops and pushes result)
        self.push(expr);
        self.op_dispatch_rules()?;

        // Cache the result (peek at top of stack)
        if let Some(result) = self.value_stack.last() {
            self.memo_cache.insert(&head_str, &args, result.clone());
        }

        Ok(())
    }

    // === Environment Operations ===

    fn op_define_rule(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::rules", ip = self.ip, "define_rule (generic)");

        let body = self.pop()?;
        let pattern = self.pop()?;

        // Environment is required for DefineRule
        let env = self.env.as_mut().ok_or_else(|| {
            VmError::Runtime(
                "DefineRule requires environment (use GenericBytecodeVM::with_env)".to_string(),
            )
        })?;

        // S2 BANG-WORD (2026-05-13): when called inside `(! ...)` body, the
        // `(= lhs rhs)` form is data (not a rule definition). Reconstruct the
        // S-expression and push it as the result instead of registering.
        // Mirrors the T0 `=` arm in `eval/step/sexpr.rs`. The compiler emits
        // `DefineRule + Pop` (matching tree-walker "rule defs return empty");
        // we push a sacrificial Unit AFTER the datum so the subsequent Pop
        // eats the Unit and leaves the datum on the stack for the outer
        // `op_return` / output gate.
        if self.bang_body {
            let factory = env.factory().clone();
            let eq_atom = factory.atom("=");
            let sexpr = factory.sexpr(vec![eq_atom, pattern, body]);
            self.push(sexpr);
            self.push(self.make_unit());
            return Ok(());
        }

        // Add the rule (lhs=pattern, rhs=body)
        env.add_rule(pattern, body);
        crate::backend::eval::trampoline::dispatch_hints::increment_mutation_epoch();

        // Push Unit to indicate success (VM-level convention).
        // The compiler emits Pop after DefineRule to match tree-walker
        // semantics (rule definitions return empty).
        self.push(self.make_unit());
        Ok(())
    }

    fn op_load_global(&mut self) -> VmResult<()> {
        let index = self.read_u16()?;
        let name = self
            .chunk
            .get_constant(index)
            .ok_or(VmError::InvalidConstant(index))?
            .clone();

        // Try to load from environment bindings
        if let Some(ref env) = self.env {
            if let Some(sym) = name.as_atom() {
                if let Some(value) = env.get_binding(sym) {
                    self.push(value);
                    return Ok(());
                }
            }
        }

        // No binding found - push the atom itself
        self.push(name);
        Ok(())
    }

    fn op_store_global(&mut self) -> VmResult<()> {
        let index = self.read_u16()?;
        let name = self
            .chunk
            .get_constant(index)
            .and_then(|v| v.as_atom().map(|s| s.to_string()))
            .ok_or(VmError::InvalidConstant(index))?;
        let value = self.pop()?;

        // Store in environment using bind
        if let Some(env) = &mut self.env {
            env.bind(&name, value);
        }
        Ok(())
    }

    /// Compute choice-point coordinates that correctly resume at the OUTER
    /// chunk's post-dispatch position when the CP is being installed inside
    /// an active compiled-RHS call frame.
    ///
    /// **Why this exists.** When `op_dispatch_rules` is executing inside a
    /// compiled-RHS chunk (entered via the fast path at lines ~5688-5753) and
    /// installs a `BoundValue` choice point, naively using `self.ip` /
    /// `self.chunk` and `self.call_stack.len()` would resume INTO the inner
    /// chunk on backtrack, fire its `op_return` on every alt, and route
    /// `BoundValue` alternatives to the top-level Return branch — leaking
    /// them as `self.results` and bypassing `CollapseBindEnd`'s pair
    /// encoding (failing `within_query_cache_isolation_contract`).
    ///
    /// **Fix.** When inside a callee frame, snapshot the *outer* frame's
    /// resume coordinates and pre-pop the frame as part of backtrack
    /// (`call_stack_height = self.call_stack.len() - 1`). The truncate in
    /// `op_fail*` then drops the inner frame; execution resumes at the
    /// outer chunk's post-dispatch position with the BoundValue on the
    /// value stack — exactly as if the inner chunk had returned that alt
    /// directly.
    ///
    /// Returns `(cp_ip, cp_chunk, cp_call_stack_height,
    ///          cp_value_stack_height, cp_bindings_stack_height,
    ///          cp_locals_base_at_cp, cp_locals_height, cp_trail_height)`.
    fn cp_install_coords_for_bound_value(
        &self,
    ) -> (
        usize,
        Arc<GenericBytecodeChunk<V>>,
        usize,
        usize,
        usize,
        usize,
        usize,
        usize,
    ) {
        if let Some(top) = self.call_stack.last() {
            // Inside a compiled-RHS callee. Backtrack must pre-pop this frame
            // and resume at the outer chunk's post-dispatch position.
            (
                top.return_ip,
                Arc::clone(&top.return_chunk),
                self.call_stack.len() - 1,
                top.base_ptr,
                top.bindings_base + 1,
                top.locals_base,
                top.caller_locals_len,
                top.caller_trail_len,
            )
        } else {
            // Top level — current chunk is the outer; current ip is the
            // resume point. Existing behavior preserved verbatim.
            (
                self.ip,
                Arc::clone(&self.chunk),
                0,
                self.value_stack.len(),
                self.bindings_stack.len(),
                self.locals_base,
                self.locals.len(),
                self.trail.len(),
            )
        }
    }

    fn op_dispatch_rules(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::rules", ip = self.ip, "dispatch_rules (generic)");

        // Pop the call expression from the stack
        let expr = self.pop()?;

        // Phase 9.5: Normal-form memoization — skip dispatch for known-irreducible S-exprs
        if expr.as_sexpr().is_some()
            && crate::backend::eval::trampoline::is_memoized_normal_form(&expr)
        {
            self.push(expr);
            return Ok(());
        }

        // Guard: only dispatch on callable expressions (s-exprs with atom head, or bare atoms)
        if let Some(items) = expr.as_sexpr() {
            if items.is_empty() {
                // Empty expression - return unchanged
                self.push(expr);
                return Ok(());
            }
            if items[0].as_atom().is_none() {
                // Head is not an atom - return expression unchanged
                self.push(expr);
                return Ok(());
            }
            // Phase 9.1: Variable-head guard — expressions like ($f x) are data,
            // not callable. Rule dispatch only applies to concrete-headed S-exprs.
            if let Some(head_atom) = items[0].as_atom() {
                if head_atom.starts_with('$') {
                    self.push(expr);
                    return Ok(());
                }
            }
        } else if expr.as_atom().is_none() {
            // Not a callable expression - return unchanged
            self.push(expr);
            return Ok(());
        }

        // Phase 9.6: All-error-types early exit — if every declared type for
        // the head is an Error type, the expression can never produce a useful
        // result. Short-circuit with an error value.
        if let Some(items) = expr.as_sexpr() {
            if let Some(head) = items.first().and_then(|v| v.as_atom()) {
                if let Some(env) = &self.env {
                    let op_types = env.get_types_generic(head);
                    if !op_types.is_empty()
                        && op_types.iter().all(|t| {
                            t.as_sexpr().map_or(false, |ti| {
                                ti.first().and_then(|v| v.as_atom()) == Some("Error")
                            })
                        })
                    {
                        let err = self.factory.error(
                            expr,
                            self.factory.string(&format!("All types for '{}' are errors", head)),
                        );
                        self.push(err);
                        return Ok(());
                    }
                }
            }
        }

        // Type-driven applicative evaluation (MeTTa HE parity):
        // Pre-evaluate non-meta-typed S-expr arguments, producing ALL
        // Cartesian-product combinations over nondeterministic args.
        //
        // HE bisimilarity: when an arg at a concrete-typed position
        // reduces to N > 1 values, the parent expression fans out into
        // N copies (one per result). For multi-arg nondet, we fan out
        // across the Cartesian product, with per-combination bindings
        // composed from each sub-result.
        //
        // Return shape:
        //   [(expr, bindings)]        — deterministic (len == 1)
        //   [(e1, b1), (e2, b2), …]   — nondeterministic (len > 1)
        //   []                        — impossible; function always
        //                               returns at least the original
        //                               expr when no changes apply
        let saved_pre_eval_bindings = self.current_bindings.clone();
        let pre_eval_combinations = self.vm_type_driven_pre_eval(expr.clone())?;

        if pre_eval_combinations.len() > 1 {
            // Multi-combination fanout: iterate each combination
            // through the full dispatch pipeline, collect results, and
            // expose them as a choice point with BoundValue alternatives.
            return self
                .op_dispatch_rules_multi_combo(pre_eval_combinations, saved_pre_eval_bindings);
        }

        // Single combination (len == 1 after pre-eval) — fall through
        // to the existing deterministic dispatch with the combined
        // bindings already composed into `self.current_bindings`.
        let (expr, combo_b) = pre_eval_combinations
            .into_iter()
            .next()
            .expect("pre_eval returns at least one combination");
        if !combo_b.is_empty() {
            self.current_bindings = combo_b;
        }

        // Dispatch memo: check if we've already evaluated this exact expression.
        // When backtracking causes re-dispatch (Cartesian product scenario like
        // `(op (nd1) (nd2))`), return cached results instead of re-matching.
        let expr_hash = expr.hash_value();
        let current_epoch = crate::backend::eval::trampoline::dispatch_hints::mutation_epoch();
        if let Some((cached_epoch, cached)) = self.dispatch_memo.get(&expr_hash) {
            if *cached_epoch == current_epoch {
                match cached.len() {
                    0 => {
                        self.unreduced = true;
                        self.push(expr);
                    }
                    1 => {
                        self.push(cached[0].clone());
                    }
                    _ => {
                        let mut iter = cached.iter().cloned();
                        let first = iter.next().expect("non-empty cached");
                        let alternatives: Vec<GenericAlternative<V, GenericBytecodeChunk<V>>> =
                            iter.map(GenericAlternative::Value).collect();
                        if !alternatives.is_empty() {
                            self.choice_points.push(GenericChoicePoint {
                                ip: self.ip,
                                chunk: Arc::clone(&self.chunk),
                                value_stack_height: self.value_stack.len(),
                                call_stack_height: self.call_stack.len(),
                                bindings_stack_height: self.bindings_stack.len(),
                                alternatives,
                                saved_unreduced: self.unreduced,
                                trail_height: self.trail.len(),
                                saved_current_bindings: self.current_bindings.clone(),
                                locals_height: self.locals.len(),
                                locals_base_at_cp: self.locals_base,
                            });
                        }
                        self.push(first);
                    }
                }
                return Ok(());
            }
            // Epoch mismatch — stale entry, fall through to re-dispatch
        }

        // Get environment reference
        let env = match &self.env {
            Some(e) => e,
            None => {
                // No environment - return expression unchanged (irreducible)
                self.push(expr);
                return Ok(());
            }
        };

        // Use native byte-level matching via RuleIndex + extract_data
        // VM-tier bidirectional-unify fallback: when structural matching fails
        // for a free-variable query (e.g. `(father $who b)` against rule
        // `(father a b)`), engage Prolog-style unification to produce matches
        // with full caller-side bindings. Mirrors the trampoline tier's Step
        // 3.5 (`step/sexpr.rs:2442-2478`) but emits `RuleMatchResult` with
        // `original_bindings` populated for `BindingFrame` lookup, AND with
        // `compiled_rhs = None` so the unify-instantiated RHS flows through
        // `eval_sub_expr_vm` rather than re-executing stale bytecode.
        // Phase 5 (Bug 1): thread caller-side bindings so `apply_bindings_with_rename_scoped`
        // can resolve caller variables in captured rule bodies (e.g.
        // `(uncle $a $b)` substituted into `$C` via the `=>` template).
        //
        // P2 (2026-05-12): aligned with T0's `step/sexpr.rs:2618-2725`
        // Step 3 + Step 3.5 semantics — try native structural matching
        // first; fall back to `match_rules_via_unify` only when native
        // returned ZERO matches AND the query has free variables. This is
        // HE-faithful: `enumerate_rules_via_unification_detailed` is the
        // EXACT analog of T0's Step 3.5 fallback. Repeated-var rules like
        // PLN's modus ponens (`(|- ($A ...) ((Implication $A $B) ...))`)
        // are already handled by native's `EqualCheck` arm at line 1337,
        // which calls `bidirectional_unify_generic` and emits both
        // rule-side AND query-side bindings — `export_rule_match_bindings`
        // then threads both via the `[dispatch_scope, ROOT_SCOPE]` chain.
        //
        // Replaces Y.5's "always run both and prefer unify" — which paid
        // 2× dispatch cost on every free-var query AND lost `compiled_rhs`
        // by routing native-eligible RHS through the trampoline.
        let matches =
            env.match_rules_native(&expr, apply_bindings_generic, &self.current_bindings);
        let native_count = matches.len() as u32;
        let expr_has_variables = expr.has_variables_fast();
        let mut unify_count: u32 = 0;
        let mut matches = matches;
        let dispatch_path = if !matches.is_empty() {
            "native"
        } else if expr_has_variables {
            let unified = env.match_rules_via_unify(&expr);
            unify_count = unified.len() as u32;
            if !unified.is_empty() {
                matches = unified;
                "unify"
            } else {
                "neither"
            }
        } else {
            "neither"
        };

        // P1 trace event: record which dispatch path produced the matches.
        // Feature-gated; no cost outside `trace`.
        #[cfg(feature = "trace")]
        {
            use crate::backend::trace::thread_local_sink::with_thread_trace_collector;
            use crate::backend::trace::trace_value_generic;
            let (call_head, call_arity) = match expr.as_sexpr() {
                Some(items) => {
                    let head = items
                        .first()
                        .and_then(|v| v.as_atom())
                        .unwrap_or("")
                        .to_string();
                    (head, items.len().saturating_sub(1) as u32)
                }
                None => (expr.as_atom().unwrap_or("").to_string(), 0u32),
            };
            with_thread_trace_collector(|tc| {
                tc.emit_converted(
                    trace_format::TraceTier::BytecodeVM,
                    0,
                    trace_value_generic(&expr),
                    vec![],
                    None,
                    trace_format::TraceEventKind::RuleMatchDispatchPath {
                        call_head: call_head.clone(),
                        call_arity,
                        path: dispatch_path.to_string(),
                        native_count,
                        unify_count,
                        expr_has_variables,
                    },
                );
            });
        }
        let _ = (native_count, unify_count, dispatch_path, expr_has_variables);

        // Phase 9.2/9.3: expected_type branch pruning — filter out rule matches
        // whose rhs_type is incompatible with the expected return type.
        let matches = if let Some(ref expected) = self.expected_type {
            use crate::backend::eval::types::types_match_generic;
            matches
                .into_iter()
                .filter(|m| match &m.rhs_type {
                    Some(rt) => types_match_generic(rt, expected),
                    None => true, // No rhs_type → can't prune, keep it
                })
                .collect()
        } else {
            matches
        };
        self.expected_type = None; // Clear after use

        // Trace: Emit RuleMatchSet for all matching rules
        #[cfg(feature = "trace")]
        {
            use crate::backend::trace::thread_local_sink::with_thread_trace_collector;
            use crate::backend::trace::trace_value_generic;
            with_thread_trace_collector(|tc| {
                tc.emit_converted(
                    trace_format::TraceTier::BytecodeVM,
                    0,
                    trace_value_generic(&expr),
                    vec![],
                    None,
                    trace_format::TraceEventKind::RuleMatchSet {
                        match_count: matches.len() as u32,
                        matches: matches
                            .iter()
                            .map(|m| (trace_value_generic(&m.instantiated_rhs), None))
                            .collect(),
                    },
                );
            });
        }

        // Phase 8b: Profile rule match frequencies for PGO JIT compilation.
        // Uses bytecode IP as site identifier within this chunk.
        if !matches.is_empty() {
            self.profile_rule_match(self.ip as u64, matches.len());
        }

        if matches.is_empty() {
            // S-step (2026-05-17): Call-site type checking (T1 in-tier).
            //
            // Mirrors T0 Step 3.6 (`eval/step/sexpr.rs:3413-3427`). After
            // both native + unify rule-matching produce zero matches, check
            // whether the call site is ill-typed against the head's declared
            // arrow type. Tier-locality: reuses generic-over-V helper
            // `check_call_site_types` from `eval/types.rs:1868` — same status
            // as `env.match_space()` / `apply_bindings_generic` (shared
            // environment infrastructure, not a tier delegate). Permissive
            // mode (default) only fires when both head has a concrete
            // `(-> ...)` declaration and arg types are determinable. Strict
            // (`auto`) mode also fires on `%Undefined%` arg types.
            //
            // HE parity: `hyperon-experimental/lib/src/metta/types.rs::check_type`.
            // Errors are shaped as
            //   `(Error <call-form> (BadArgType <1-indexed-N> <expected> <inferred>))`
            // or `(Error <call-form> IncorrectNumberOfArguments)`.
            if let Some(items) = expr.as_sexpr() {
                if let Some(env) = &self.env {
                    if let Some(err) =
                        crate::backend::eval::types::check_call_site_types(items, &self.factory, env)
                    {
                        self.push(err);
                        return Ok(());
                    }
                }
            }

            // Phase 2.B HE-bisimilarity (three-tier parity with tree-walker):
            // distinguish "function with no matching rules" (→ empty) from
            // "data constructor" (→ unreduced data). Data constructors
            // (heads with NO rules in the environment) evaluate to
            // themselves — MeTTa's ADD-mode data semantics. Functions
            // (heads with SOME rule but none unify with these args) produce
            // EMPTY at nested call depth, preventing ghost results in
            // conjunctions where a sub-call fails to match.
            //
            // `call_stack.is_empty()` means we're at the top-level query
            // (equivalent to the tree-walker's depth == 0): retain ADD-mode
            // behavior (push unreduced so the caller can add to space).
            //
            // Y.3 (2026-05-12): also treat "inside collapse / collapse-bind"
            // as non-top-level. The body of `(collapse (f c))` runs without
            // a call_stack frame (collapse opcodes use their own frames in
            // collapse_frames/collapse_bind_frames), but semantically it IS
            // nested — pattern-fail must produce empty so collapse-of-empty
            // returns `()`, mirroring T0's HE-aligned behavior.
            let inside_collapse =
                !self.collapse_frames.is_empty() || !self.collapse_bind_frames.is_empty();
            let nested = !self.call_stack.is_empty() || inside_collapse;
            let has_any_rules = if nested {
                if let Some(head) = expr
                    .as_sexpr()
                    .and_then(|items| items.first())
                    .and_then(|v| v.as_atom())
                {
                    let arity = expr
                        .as_sexpr()
                        .map(|it| it.len().saturating_sub(1))
                        .unwrap_or(0);
                    self.env
                        .as_ref()
                        .map(|env| {
                            env.shared
                                .rule_index
                                .read()
                                .get_candidates(head, arity, None)
                                .next()
                                .is_some()
                        })
                        .unwrap_or(false)
                } else {
                    false
                }
            } else {
                false
            };

            if has_any_rules {
                // Function with no matching rules at nested depth → empty.
                // HE-bisimilar silent pruning: the branch dies and contributes
                // nothing to the caller's result set.
                self.unreduced = true;
                // Do NOT push. Caller (outer dispatch or choice-point consumer)
                // will observe an empty value stack entry for this call.
                // Convention: push the expr with unreduced flag so downstream
                // can distinguish "no result" from "unhandled case" by the
                // flag; but semantic callers should treat unreduced + no-rule
                // as "this branch dies".
                //
                // Pragmatic choice: retain the push (backwards compatibility
                // with existing callers that expect a value on the stack)
                // but flag unreduced. Future cleanup: switch to a true empty
                // push when all VM consumers handle the unreduced flag.
                if expr.as_sexpr().is_some() {
                    crate::backend::eval::trampoline::memoize_normal_form(&expr);
                }
                self.push(expr);
                return Ok(());
            }

            // Phase 9.5: Memoize as normal form — no rules matched, so this
            // S-expression is irreducible. Future dispatches will skip it.
            if expr.as_sexpr().is_some() {
                crate::backend::eval::trampoline::memoize_normal_form(&expr);
            }
            // HE ADD-mode parity: at top-level, bare S-expressions with no
            // matching rules are facts and must be inserted into env's
            // PathMap so subsequent `match &self` queries find them. The
            // trampoline post-pass at `eval/mod.rs:780` is short-circuited
            // by the `memoize_normal_form` bloom we set above, so we must
            // do the insert here. Only add when the fact is not already
            // present, to avoid multiplicity inflation when the same
            // expression is evaluated multiple times (e.g., tier-promotion
            // re-runs).
            if self.call_stack.is_empty() && expr.as_sexpr().is_some() {
                if let Some(env_mut) = self.env.as_mut() {
                    if !env_mut.match_space_exists(&expr) {
                        env_mut.add_to_space(&expr);
                        crate::backend::eval::trampoline::dispatch_hints::increment_mutation_epoch();
                    }
                }
                // S1 TOPLEVEL (2026-05-13): HE ADD-mode emits NOTHING for
                // bare top-level S-exprs. Side-effect (add-to-space) is
                // already performed above. INTERPRET-mode (set by `(! ...)`)
                // takes the unchanged-data path so observable output flows.
                if !self.interpret_mode {
                    self.push(self.factory.empty());
                    return Ok(());
                }
            }
            // No rules match and no rules exist for this head: this is a data
            // constructor / normal form. Returning it unchanged is a complete
            // VM result, matching the JIT runtime's no-bailout behavior for
            // irreducible calls. Do not set `unreduced`, or one inert data
            // sub-expression forces whole-expression tree-walker fallback.
            self.push(expr);
            return Ok(());
        }

        if matches.len() == 1 {
            // Single match - push the instantiated body for further evaluation
            let result = matches.into_iter().next().expect("matches has 1 element");

            // Trace: RuleApplication for single match
            #[cfg(feature = "trace")]
            {
                use crate::backend::trace::thread_local_sink::with_thread_trace_collector;
                use crate::backend::trace::trace_value_generic;
                with_thread_trace_collector(|tc| {
                    let bindings_tv: Vec<(String, trace_format::TraceValue)> = result
                        .bindings
                        .iter()
                        .map(|(k, v)| (k.to_string(), trace_value_generic(v)))
                        .collect();
                    tc.emit_converted(
                        trace_format::TraceTier::BytecodeVM,
                        0,
                        trace_value_generic(&expr),
                        vec![trace_value_generic(&result.instantiated_rhs)],
                        None,
                        trace_format::TraceEventKind::RuleApplication {
                            rule_lhs: trace_value_generic(&expr),
                            rule_rhs: trace_value_generic(&result.instantiated_rhs),
                            bindings: bindings_tv,
                            rule_span: None,
                        },
                    );
                });
            }

            // Attempt compiled RHS direct execution (compile-on-add).
            // If the rule has a pre-compiled RHS chunk, execute it in-VM
            // via call frame switching — no trampoline round-trip.
            if let Some(compiled_arc) = result.compiled_rhs {
                // Downcast from Arc<dyn Any + Send + Sync> to Arc<GenericBytecodeChunk<V>>
                if let Ok(rhs_chunk) = compiled_arc.downcast::<GenericBytecodeChunk<V>>() {
                    // P2 follow-up: compose the rule-match's scope-tagged
                    // alias bindings into `current_bindings` BEFORE we snapshot
                    // it into the call frame's `saved_bindings`. Without this
                    // step, caller-side aliases like
                    // `(dispatch_scope, $rule_var) → $caller_var` are visible
                    // only in the bare-name `BindingFrame` populated below
                    // (used by PushVariable lookups), and vanish on return —
                    // breaking collapse-bind sidecar projection of caller-typed
                    // query vars (e.g. Direct.metta `(? (grandfather $who c))`).
                    if !result.bindings.is_empty() {
                        let composed = crate::backend::eval::bindings::compose_outer_inner_generic(
                            &self.current_bindings,
                            &result.bindings,
                            &self.factory,
                        );
                        let conflict = composed.is_empty()
                            && !self.current_bindings.is_empty()
                            && !result.bindings.is_empty();
                        if conflict {
                            return Err(VmError::Runtime(
                                "op_dispatch_rules: rule-match bindings conflict with current_bindings".to_string()
                            ));
                        }
                        self.current_bindings = composed;
                    }
                    // Push call frame to save current execution state.
                    // Bug-fix 2026-04-follow-up: stash caller's locals_base, bump ours.
                    let caller_locals_base = self.locals_base;
                    let caller_locals_len_snap = self.locals.len();
                    let caller_trail_len_snap = self.trail.len();
                    self.call_stack.push(GenericCallFrame {
                        return_ip: self.ip,
                        return_chunk: Arc::clone(&self.chunk),
                        base_ptr: self.value_stack.len(),
                        bindings_base: self.bindings_stack.len().saturating_sub(1),
                        yield_on_return: false,
                        caller_locals_len: caller_locals_len_snap,
                        caller_trail_len: caller_trail_len_snap,
                        saved_bindings: self.current_bindings.clone(),
                        locals_base: caller_locals_base,
                    });

                    // Push new binding frame with match bindings.
                    //
                    // PushVariable opcodes in the compiled chunk resolve through
                    // this frame (searching innermost to outermost) using
                    // bare-name lookup against ORIGINAL rule-LHS variable
                    // names. `compiled_rhs` was built at rule-insertion time
                    // against `entry.rhs` (original names), so we MUST seed
                    // with `result.original_bindings` (pre-freshen, pre-scope-
                    // tag, ROOT_SCOPE-keyed). `result.bindings` carries the
                    // freshened + scope-tagged form for the trampoline tier
                    // and is NOT compatible with the compiled bytecode here.
                    let depth = self.bindings_stack.len() as u32;
                    let mut frame = GenericBindingFrame::new(depth);
                    for (name, value) in result.original_bindings.iter() {
                        frame.set(name.to_string(), value.clone());
                    }
                    self.bindings_stack.push(frame);

                    // Switch to compiled RHS chunk — VM loop continues here.
                    // Return opcode will pop the call frame and restore caller state.
                    self.chunk = rhs_chunk;
                    self.ip = 0;
                    // Callee's slots live past the caller's: bump locals_base
                    // to the current end of locals, then ensure capacity for callee.
                    self.locals_base = self.locals.len();
                    self.ensure_locals_for_current_chunk();
                    return Ok(());
                }
                // Downcast failed — fall through to instantiated_rhs path
            }

            // Fallback: no compiled RHS chunk — evaluate the instantiated RHS
            // via the trampoline, matching the tree-walker's WorkItem::Eval behavior
            // and the multi-match path's eval_sub_expr_vm_all pattern.
            let rhs = result.instantiated_rhs;

            // Fast path: skip trampoline for values memoized as normal form.
            if crate::backend::eval::trampoline::is_memoized_normal_form(&rhs) {
                self.dispatch_memo
                    .insert(expr_hash, (current_epoch, vec![rhs.clone()]));
                self.push(rhs);
                return Ok(());
            }

            // P2 follow-up: compose rule-match alias bindings into
            // `current_bindings` before invoking the trampoline. The
            // trampoline-evaluated RHS may emit bindings for renamed body-
            // local atoms; the matcher's caller-side aliases must already
            // be present in `current_bindings` so apply_chain post-compose
            // can resolve `$caller_var → $rule_renamed → ground` for the
            // caller. Without this, e.g. Direct.metta `(? (grandfather $who c))`
            // produces a result with no `($who a)` projection.
            if !result.bindings.is_empty() {
                let composed = crate::backend::eval::bindings::compose_outer_inner_generic(
                    &self.current_bindings,
                    &result.bindings,
                    &self.factory,
                );
                let conflict = composed.is_empty()
                    && !self.current_bindings.is_empty()
                    && !result.bindings.is_empty();
                if conflict {
                    return Err(VmError::Runtime(
                        "op_dispatch_rules: rule-match bindings conflict with current_bindings"
                            .to_string(),
                    ));
                }
                self.current_bindings = composed;
            }

            // Evaluate the RHS through the trampoline. Use the
            // bindings-preserving variant so nondeterministic alternatives
            // are surfaced as `BoundValue` choice points rather than
            // truncated to the first result (Defect B fix). Single-result
            // case still composes the sub-expr's bindings into
            // `current_bindings` (matching the prior `eval_sub_expr_vm`
            // semantics for deterministic callers).
            let epoch_before = crate::backend::eval::trampoline::dispatch_hints::mutation_epoch();
            let env = self
                .env
                .as_ref()
                .expect("op_dispatch_rules requires env")
                .clone();
            let saved_outer_bindings = self.current_bindings.clone();
            let sub_outcomes = self.eval_sub_expr_vm_all_with_bindings(rhs.clone(), env);

            if sub_outcomes.is_empty() {
                // No reduction — push the unevaluated RHS as a data
                // constructor. Matches prior `eval_sub_expr_vm` "no results"
                // arm.
                if crate::backend::eval::trampoline::dispatch_hints::mutation_epoch()
                    == epoch_before
                {
                    self.dispatch_memo
                        .insert(expr_hash, (epoch_before, vec![rhs.clone()]));
                }
                self.push(rhs);
                return Ok(());
            }

            if sub_outcomes.len() == 1 {
                let (v, sub_b) = sub_outcomes.into_iter().next().expect("len==1");
                if !sub_b.is_empty() {
                    let mut composed = crate::backend::eval::bindings::compose_outer_inner_generic(
                        &self.current_bindings,
                        &sub_b,
                        &self.factory,
                    );
                    if composed.is_empty() && !self.current_bindings.is_empty() && !sub_b.is_empty()
                    {
                        return Err(VmError::Runtime(
                            "op_dispatch_rules: ground/ground binding conflict (branch inconsistent)".to_string(),
                        ));
                    }
                    crate::backend::eval::bindings::apply_chain_generic(
                        &mut composed,
                        &self.factory,
                    );
                    self.current_bindings = composed;
                }
                if crate::backend::eval::trampoline::dispatch_hints::mutation_epoch()
                    == epoch_before
                {
                    self.dispatch_memo
                        .insert(expr_hash, (epoch_before, vec![v.clone()]));
                }
                self.push(v);
                return Ok(());
            }

            // Multiple inner results — install a BoundValue choice point.
            // Each per-alt bindings = caller's saved_outer_bindings ∘ sub_b.
            let mut merged_outcomes: Vec<(V, crate::backend::models::GenericBindings<V>)> =
                Vec::with_capacity(sub_outcomes.len());
            for (v, sub_b) in sub_outcomes {
                let merged = if sub_b.is_empty() {
                    saved_outer_bindings.clone()
                } else if saved_outer_bindings.is_empty() {
                    sub_b
                } else {
                    let composed = crate::backend::eval::bindings::compose_outer_inner_generic(
                        &saved_outer_bindings,
                        &sub_b,
                        &self.factory,
                    );
                    if composed.is_empty() && !saved_outer_bindings.is_empty() && !sub_b.is_empty()
                    {
                        // Per-alt branch inconsistent — drop, do not poison siblings.
                        continue;
                    }
                    composed
                };
                merged_outcomes.push((v, merged));
            }

            if merged_outcomes.is_empty() {
                self.unreduced = true;
                self.push(rhs);
                return Ok(());
            }

            if crate::backend::eval::trampoline::dispatch_hints::mutation_epoch() == epoch_before {
                let cache_values: Vec<V> = merged_outcomes.iter().map(|(v, _)| v.clone()).collect();
                self.dispatch_memo
                    .insert(expr_hash, (epoch_before, cache_values));
            }

            if merged_outcomes.len() == 1 {
                let (v, b) = merged_outcomes.into_iter().next().expect("len==1");
                self.current_bindings = b;
                self.push(v);
                return Ok(());
            }

            let mut iter = merged_outcomes.into_iter();
            let (first_v, first_b) = iter.next().expect("non-empty");
            let alternatives: Vec<GenericAlternative<V, GenericBytecodeChunk<V>>> = iter
                .map(|(v, b)| GenericAlternative::BoundValue {
                    value: v,
                    bindings: b,
                })
                .collect();
            if !alternatives.is_empty() {
                // Pre-pop the active compiled-RHS callee frame on backtrack
                // so BoundValue alts resume at the OUTER chunk's
                // post-dispatch position (not the inner chunk's op_return,
                // which would leak alts to the top-level Return branch and
                // break collapse-bind pair encoding).
                let (
                    cp_ip,
                    cp_chunk,
                    cp_call_stack_height,
                    cp_value_stack_height,
                    cp_bindings_stack_height,
                    cp_locals_base_at_cp,
                    cp_locals_height,
                    cp_trail_height,
                ) = self.cp_install_coords_for_bound_value();
                self.choice_points.push(GenericChoicePoint {
                    ip: cp_ip,
                    chunk: cp_chunk,
                    value_stack_height: cp_value_stack_height,
                    call_stack_height: cp_call_stack_height,
                    bindings_stack_height: cp_bindings_stack_height,
                    alternatives,
                    saved_unreduced: self.unreduced,
                    trail_height: cp_trail_height,
                    saved_current_bindings: saved_outer_bindings.clone(),
                    locals_height: cp_locals_height,
                    locals_base_at_cp: cp_locals_base_at_cp,
                });
            }
            self.current_bindings = first_b;
            self.push(first_v);
            return Ok(());
        }

        // Multiple matches — eager evaluation of all matched RHS expressions.
        // Instead of creating choice points (which sets has_choices=true and
        // forces tree-walker fallback), evaluate each matched RHS via the
        // trampoline and collect all results. This mirrors superpose semantics
        // and avoids re-executing shared sub-expressions.

        // Trace: NondeterministicFork for multiple matches
        #[cfg(feature = "trace")]
        {
            use crate::backend::trace::thread_local_sink::with_thread_trace_collector;
            use crate::backend::trace::trace_value_generic;
            with_thread_trace_collector(|tc| {
                tc.emit_converted(
                    trace_format::TraceTier::BytecodeVM,
                    0,
                    trace_value_generic(&expr),
                    vec![],
                    None,
                    trace_format::TraceEventKind::NondeterministicFork {
                        branch_count: matches.len() as u32,
                    },
                );
            });
        }

        // Eagerly evaluate all matched RHS bodies and collect (value, bindings)
        // outcomes. Mirrors `op_dispatch_rules_multi_combo`'s per-iteration
        // bindings discipline: snapshot the outer `current_bindings`, compose
        // each match's rule-match aliases, evaluate, then merge sub-result
        // bindings on top — restoring the snapshot before the next match so
        // per-match outcomes stay isolated.
        let env = self
            .env
            .as_ref()
            .expect("op_dispatch_rules requires env")
            .clone();
        let saved_outer_bindings = self.current_bindings.clone();

        let epoch_before_multi = crate::backend::eval::trampoline::dispatch_hints::mutation_epoch();
        let mut all_outcomes: Vec<(V, crate::backend::models::GenericBindings<V>)> = Vec::new();

        for result in matches {
            // Restore the outer ambient before each iteration so prior matches
            // don't leak into this one.
            self.current_bindings = saved_outer_bindings.clone();

            // Compose this match's rule-match alias bindings into
            // `current_bindings` BEFORE invoking the trampoline. Mirrors the
            // single-match path. Without this, caller-typed query vars
            // (e.g. `(? (grandfather $who c))`) see no projection.
            let per_match_ambient = if !result.bindings.is_empty() {
                let composed = crate::backend::eval::bindings::compose_outer_inner_generic(
                    &self.current_bindings,
                    &result.bindings,
                    &self.factory,
                );
                let conflict = composed.is_empty()
                    && !self.current_bindings.is_empty()
                    && !result.bindings.is_empty();
                if conflict {
                    // Skip this match — caller-side aliases inconsistent
                    // with ambient. HE-faithful pruning, not hard error.
                    continue;
                }
                self.current_bindings = composed.clone();
                composed
            } else {
                self.current_bindings.clone()
            };

            // Evaluate the instantiated RHS through the trampoline,
            // preserving each sub-result's bindings.
            let sub_results =
                self.eval_sub_expr_vm_all_with_bindings(result.instantiated_rhs, env.clone());
            if sub_results.is_empty() {
                continue;
            }

            for (v, sub_b) in sub_results {
                let merged = if sub_b.is_empty() {
                    per_match_ambient.clone()
                } else if per_match_ambient.is_empty() {
                    sub_b
                } else {
                    let composed = crate::backend::eval::bindings::compose_outer_inner_generic(
                        &per_match_ambient,
                        &sub_b,
                        &self.factory,
                    );
                    if composed.is_empty() && !per_match_ambient.is_empty() && !sub_b.is_empty() {
                        // Sub-result inconsistent — drop, don't poison siblings.
                        continue;
                    }
                    composed
                };
                all_outcomes.push((v, merged));
            }
        }

        // Restore the outer ambient. The chosen alternative below
        // overwrites `current_bindings` with its own per-alt bindings.
        self.current_bindings = saved_outer_bindings.clone();

        // Cache values-only for re-dispatch memoization. Per the
        // `within_query_cache_isolation_contract` test, caches MUST NOT
        // carry bindings — each retrieving caller layers its own
        // `current_bindings` on hit. Discard per-outcome bindings here.
        if !all_outcomes.is_empty()
            && crate::backend::eval::trampoline::dispatch_hints::mutation_epoch()
                == epoch_before_multi
        {
            let cache_values: Vec<V> = all_outcomes.iter().map(|(v, _)| v.clone()).collect();
            self.dispatch_memo
                .insert(expr_hash, (epoch_before_multi, cache_values));
        }

        if all_outcomes.len() == 1 {
            let (v, b) = all_outcomes.into_iter().next().expect("len==1");
            self.current_bindings = b;
            self.push(v);
        } else if !all_outcomes.is_empty() {
            // Multiple outcomes — push first, expose the rest as
            // BoundValue alternatives. `op_fail`'s BoundValue arm
            // restores the per-alt bindings on backtrack.
            let mut iter = all_outcomes.into_iter();
            let (first_v, first_b) = iter.next().expect("non-empty");

            let alternatives: Vec<GenericAlternative<V, GenericBytecodeChunk<V>>> = iter
                .map(|(v, b)| GenericAlternative::BoundValue {
                    value: v,
                    bindings: b,
                })
                .collect();

            if !alternatives.is_empty() {
                // Pre-pop the active compiled-RHS callee frame on backtrack
                // so BoundValue alts resume at the OUTER chunk's
                // post-dispatch position (see cp_install_coords_for_bound_value).
                let (
                    cp_ip,
                    cp_chunk,
                    cp_call_stack_height,
                    cp_value_stack_height,
                    cp_bindings_stack_height,
                    cp_locals_base_at_cp,
                    cp_locals_height,
                    cp_trail_height,
                ) = self.cp_install_coords_for_bound_value();
                self.choice_points.push(GenericChoicePoint {
                    ip: cp_ip,
                    chunk: cp_chunk,
                    value_stack_height: cp_value_stack_height,
                    call_stack_height: cp_call_stack_height,
                    bindings_stack_height: cp_bindings_stack_height,
                    alternatives,
                    saved_unreduced: self.unreduced,
                    trail_height: cp_trail_height,
                    saved_current_bindings: saved_outer_bindings.clone(),
                    locals_height: cp_locals_height,
                    locals_base_at_cp: cp_locals_base_at_cp,
                });
            }

            self.current_bindings = first_b;
            self.push(first_v);
        }
        // If no outcomes, push nothing (expression is irreducible)

        Ok(())
    }

    /// Dispatch rules for a multi-combination pre-eval result.
    ///
    /// HE parity for nondeterministic applicative pre-evaluation: when
    /// an arg at a concrete-typed position reduces to N > 1 values, the
    /// parent expression fans out into N copies. This function takes
    /// the full Cartesian product of combinations, runs the rule-match
    /// + RHS-eval pipeline for each, and exposes the union of all
    /// results via a choice point with `BoundValue` alternatives so
    /// per-combination bindings are restored on backtrack.
    ///
    /// Mirrors the tree-walker's `CollectGroundedArg` →
    /// `CollectApplicativeResults` chain.
    fn op_dispatch_rules_multi_combo(
        &mut self,
        combinations: Vec<(V, crate::backend::models::GenericBindings<V>)>,
        saved_bindings: crate::backend::models::GenericBindings<V>,
    ) -> VmResult<()> {
        use crate::backend::eval::bindings::apply_bindings_generic;

        let env = match &self.env {
            Some(e) => e.clone(),
            None => {
                // No env — push first expr unchanged, stale combinations.
                let first = combinations
                    .into_iter()
                    .next()
                    .expect("multi-combo called with non-empty list")
                    .0;
                self.push(first);
                return Ok(());
            }
        };

        // For each combination, run the rule-match + RHS-eval pipeline
        // and collect (value, bindings) outcomes. Combinations whose
        // rules fail to match contribute themselves as irreducible
        // outcomes (HE: the unreduced expr is a valid evaluation
        // result for data constructors or unmatched calls).
        let mut all_outcomes: Vec<(V, crate::backend::models::GenericBindings<V>)> =
            Vec::with_capacity(combinations.len());

        for (combo_expr, combo_b) in combinations {
            // Per-combination, install combo_b as the ambient bindings
            // before dispatch. Restore to `saved_bindings` before the
            // next iteration so combinations don't leak into each other.
            self.current_bindings = combo_b.clone();

            // VM-tier bidirectional-unify fallback (mirrors single-combo path
            // at line ~5526) — see commentary there for rationale.
            // Phase 5 (Bug 1): thread caller-side outer bindings.
            let mut matches =
                env.match_rules_native(&combo_expr, apply_bindings_generic, &self.current_bindings);
            if matches.is_empty() && combo_expr.has_variables_fast() {
                matches = env.match_rules_via_unify(&combo_expr);
            }
            // Phase 9.2/9.3 expected_type pruning
            let matches = if let Some(ref expected) = self.expected_type {
                use crate::backend::eval::types::types_match_generic;
                matches
                    .into_iter()
                    .filter(|m| match &m.rhs_type {
                        Some(rt) => types_match_generic(rt, expected),
                        None => true,
                    })
                    .collect()
            } else {
                matches
            };

            if matches.is_empty() {
                // S-step (2026-05-17): Call-site type checking (T1 multi-combo).
                //
                // Same logic as the single-combo path — see commentary at the
                // analogous `matches.is_empty()` site above for tier-locality
                // rationale. We check per-combination because each combo has
                // its own substituted expression (different concrete arg
                // shapes) and may independently trigger BadArgType /
                // IncorrectNumberOfArguments.
                if let Some(items) = combo_expr.as_sexpr() {
                    if let Some(err) =
                        crate::backend::eval::types::check_call_site_types(items, &self.factory, &env)
                    {
                        // Emit the error as this combination's outcome with
                        // its per-combo bindings, then continue to the next
                        // combination. The error flows through the choice-
                        // point machinery the same as any other outcome.
                        all_outcomes.push((err, combo_b));
                        continue;
                    }
                }
                // No rules for this combination — the expression is
                // irreducible. Contribute it as a result with its own
                // per-combination bindings.
                all_outcomes.push((combo_expr, combo_b));
                continue;
            }

            // For each matched rule, evaluate the instantiated RHS via
            // the trampoline, collecting all sub-results with bindings.
            for m in matches {
                let rhs = m.instantiated_rhs;
                let sub_results = self.eval_sub_expr_vm_all_with_bindings(rhs, env.clone());
                if sub_results.is_empty() {
                    continue;
                }
                for (v, sub_b) in sub_results {
                    // Per-result bindings layer on top of the combo's
                    // ambient. sub_b may be empty for deterministic
                    // ground results.
                    let merged = if sub_b.is_empty() {
                        combo_b.clone()
                    } else if combo_b.is_empty() {
                        sub_b
                    } else {
                        use crate::backend::eval::bindings::compose_outer_inner_generic;
                        let composed = compose_outer_inner_generic(&combo_b, &sub_b, &self.factory);
                        if composed.is_empty() && !combo_b.is_empty() && !sub_b.is_empty() {
                            continue;
                        }
                        composed
                    };
                    all_outcomes.push((v, merged));
                }
            }
        }

        self.expected_type = None;

        // Restore the outer ambient bindings; the chosen alternative
        // below will overwrite it with its own per-alt bindings.
        self.current_bindings = saved_bindings.clone();

        if all_outcomes.is_empty() {
            self.unreduced = true;
            // No combination produced any result — push the first
            // original combination's expression as the irreducible form.
            // (Shouldn't happen given the per-combo fallback above.)
            return Ok(());
        }

        if all_outcomes.len() == 1 {
            let (v, b) = all_outcomes.into_iter().next().expect("len == 1");
            self.current_bindings = b;
            self.push(v);
            return Ok(());
        }

        // Multiple outcomes — push first, create a choice point with
        // BoundValue alternatives for the rest. Per-alt bindings are
        // restored when the alt is picked (`op_fail`'s BoundValue arm).
        let mut iter = all_outcomes.into_iter();
        let (first_v, first_b) = iter.next().expect("non-empty");
        let alternatives: Vec<GenericAlternative<V, GenericBytecodeChunk<V>>> = iter
            .map(|(v, b)| GenericAlternative::BoundValue {
                value: v,
                bindings: b,
            })
            .collect();
        if !alternatives.is_empty() {
            // Pre-pop the active compiled-RHS callee frame on backtrack
            // so BoundValue alts resume at the OUTER chunk's
            // post-dispatch position (see cp_install_coords_for_bound_value).
            let (
                cp_ip,
                cp_chunk,
                cp_call_stack_height,
                cp_value_stack_height,
                cp_bindings_stack_height,
                cp_locals_base_at_cp,
                cp_locals_height,
                cp_trail_height,
            ) = self.cp_install_coords_for_bound_value();
            self.choice_points.push(GenericChoicePoint {
                ip: cp_ip,
                chunk: cp_chunk,
                value_stack_height: cp_value_stack_height,
                call_stack_height: cp_call_stack_height,
                bindings_stack_height: cp_bindings_stack_height,
                alternatives,
                saved_unreduced: self.unreduced,
                trail_height: cp_trail_height,
                saved_current_bindings: saved_bindings.clone(),
                locals_height: cp_locals_height,
                locals_base_at_cp: cp_locals_base_at_cp,
            });
        }
        self.current_bindings = first_b;
        self.push(first_v);
        Ok(())
    }

    /// Type-driven applicative pre-evaluation producing ALL combinations.
    ///
    /// HE bisimilarity: when an S-expression arg at a non-meta-typed
    /// parameter position reduces nondeterministically to multiple
    /// values, the parent expression MUST fan out into one variant per
    /// combination (Cartesian product across args). The tree-walker
    /// realizes this via `CollectGroundedArg` → `CollectApplicativeResults`;
    /// the VM must match.
    ///
    /// Return semantics:
    /// - `vec![(expr, empty_bindings)]` — no pre-eval fired (no arg types,
    ///   no concrete formal types, or no arg was an S-expression).
    /// - `vec![(expr', combined_bindings)]` — deterministic pre-eval: every
    ///   arg reduced to a single value; combined bindings merge all sub-
    ///   result bindings under `self.current_bindings`.
    /// - `vec![(expr_i, bindings_i); N]` with N > 1 — Cartesian fanout;
    ///   each combination has its own substituted expression and merged
    ///   bindings. Conflicting combinations (ground/ground binding clash)
    ///   are dropped, not errored.
    fn vm_type_driven_pre_eval(
        &mut self,
        expr: V,
    ) -> VmResult<Vec<(V, crate::backend::models::GenericBindings<V>)>> {
        use crate::backend::eval::bindings::{apply_bindings_generic, compose_outer_inner_generic};
        use crate::backend::eval::step::{
            extract_arg_types, find_grounded_arg_indices_generic, is_meta_type,
        };
        use crate::backend::models::GenericBindings;

        let empty_b = GenericBindings::new();

        let items = match expr.as_sexpr() {
            Some(items) => items,
            None => return Ok(vec![(expr, empty_b)]),
        };

        let head = match items.first().and_then(|v| v.as_atom()) {
            Some(h) => h,
            None => return Ok(vec![(expr, empty_b)]),
        };

        let env = match &self.env {
            Some(e) => e,
            None => return Ok(vec![(expr, empty_b)]),
        };

        let op_types = env.get_types_generic(head);
        let mut all_arg_types: Vec<Vec<V>> = op_types
            .iter()
            .filter_map(|t| extract_arg_types(t))
            .collect();

        // Phase 9.4: Inferred-type fallback from Phase 10 deep type inference.
        if all_arg_types.is_empty() {
            if env.has_inferred_type(head) {
                let inferred = env.get_inferred_fn_types(head);
                all_arg_types = inferred
                    .iter()
                    .filter_map(|t| extract_arg_types(t))
                    .collect();
            }
        }

        // Y.6 (2026-05-12): When no arg types are declared OR inferred for the
        // head, fall back to the bloom-filter strategy used by T0's tree-walker
        // (`step/sexpr.rs:2572`, `step/grounded.rs:126`). This handles cases
        // like the syntactic conjunction head `,` whose args have rule-bearing
        // sub-heads (e.g. `(, (father $a $b) (father $b c))` — `father` has
        // rules even though `,` does not). Without this fallback, T1 returned
        // the unreduced expression with empty bindings; the tree-walker
        // returns the reduced expression with composed bindings.
        let bloom_indices_opt: Option<Vec<usize>> = if all_arg_types.is_empty() {
            let indices = find_grounded_arg_indices_generic(items, env);
            if indices.is_empty() {
                return Ok(vec![(expr, empty_b)]);
            }
            Some(indices)
        } else {
            None
        };

        // Per-argument results: one inner Vec<(V, bindings)> per arg
        // position. For args that are meta-typed or not S-exprs, the
        // inner vec has a single element (the unchanged item + empty
        // bindings). For pre-eval'd args, it has the sub-VM's full
        // result list (1..N entries).
        //
        // `per_arg_results[0]` is the HEAD (always a single entry).
        let mut per_arg_results: Vec<Vec<(V, GenericBindings<V>)>> =
            Vec::with_capacity(items.len());
        per_arg_results.push(vec![(items[0].clone(), empty_b.clone())]);

        let mut any_multi = false;
        let mut any_changed = false;

        for i in 1..items.len() {
            let arg_idx = i - 1;

            // Decide whether to pre-eval this argument. Two modes:
            //   - Type-driven (all_arg_types non-empty): skip if meta-typed
            //     in EVERY arrow type at this position.
            //   - Bloom-fallback (all_arg_types empty): skip unless this
            //     arg index appears in the bloom-filter result.
            let should_pre_eval = match &bloom_indices_opt {
                Some(bloom) => bloom.contains(&i),
                None => {
                    let all_meta = all_arg_types.iter().all(|arg_types| {
                        arg_idx < arg_types.len() && is_meta_type(&arg_types[arg_idx])
                    });
                    !all_meta
                }
            };
            if !should_pre_eval {
                per_arg_results.push(vec![(items[i].clone(), empty_b.clone())]);
                continue;
            }

            // Substitute current ambient bindings into the arg before
            // pre-eval so earlier-argument bindings materialize in
            // later-argument expression form.
            let mut item_to_eval = items[i].clone();
            if !self.current_bindings.is_empty() {
                item_to_eval =
                    apply_bindings_generic(&item_to_eval, &self.current_bindings, &self.factory);
                if item_to_eval != items[i] {
                    any_changed = true;
                }
            }

            if item_to_eval.as_sexpr().is_none() {
                per_arg_results.push(vec![(item_to_eval, empty_b.clone())]);
                continue;
            }

            // Derive expected_type for this argument position.
            {
                use crate::backend::builtin_signatures;
                self.expected_type = builtin_signatures::get_signature(head)
                    .and_then(|sig| builtin_signatures::get_expected_type_at_position(sig, arg_idx))
                    .and_then(builtin_signatures::type_expr_to_expected_type_name)
                    .map(|name| self.factory.atom(name));
            }

            // Multi-result pre-eval — collect ALL sub-VM results for
            // this arg. Empty vec means irreducible; treat as literal.
            let sub_env = self.env.as_ref().expect("env checked above").clone();
            let sub_results =
                self.eval_sub_expr_vm_all_with_bindings(item_to_eval.clone(), sub_env);

            self.expected_type = None;

            if sub_results.is_empty() {
                per_arg_results.push(vec![(item_to_eval, empty_b.clone())]);
                continue;
            }

            if sub_results.len() > 1 {
                any_multi = true;
                any_changed = true;
            } else if sub_results[0].0 != item_to_eval {
                any_changed = true;
            }
            per_arg_results.push(sub_results);
        }

        if !any_changed && !any_multi {
            return Ok(vec![(expr, empty_b)]);
        }

        // Cartesian product across per-arg results. Each combo carries
        // merged bindings; ground/ground conflicts drop the combination.
        // Start with the outer ambient bindings already in
        // `self.current_bindings` so sub-result bindings compose ON TOP.
        let initial_b = self.current_bindings.clone();
        let mut combinations: Vec<(Vec<V>, GenericBindings<V>)> =
            vec![(Vec::with_capacity(items.len()), initial_b)];

        for arg_results in per_arg_results.into_iter() {
            let mut new_combos: Vec<(Vec<V>, GenericBindings<V>)> =
                Vec::with_capacity(combinations.len() * arg_results.len());
            for (items_so_far, bindings_so_far) in combinations.iter() {
                for (val, sub_b) in arg_results.iter() {
                    let merged = if sub_b.is_empty() {
                        bindings_so_far.clone()
                    } else {
                        let composed =
                            compose_outer_inner_generic(bindings_so_far, sub_b, &self.factory);
                        if composed.is_empty() && !bindings_so_far.is_empty() && !sub_b.is_empty() {
                            // Ground/ground conflict — drop this
                            // combination (tree-walker parity).
                            continue;
                        }
                        composed
                    };
                    let mut new_items = items_so_far.clone();
                    new_items.push(val.clone());
                    new_combos.push((new_items, merged));
                }
            }
            combinations = new_combos;
        }

        let result: Vec<(V, GenericBindings<V>)> = combinations
            .into_iter()
            .map(|(items, b)| (self.factory.sexpr(items), b))
            .collect();

        Ok(result)
    }

    /// Decide whether a `car-atom`/`cdr-atom` argument should be pre-evaluated
    /// before the structural head/tail is taken. Mirrors exactly the tree-walker
    /// predicate `is_reducible_structural_arg` (src/backend/eval/step/sexpr.rs)
    /// — the four reducer conditions:
    ///   1. head starts with `$` (variable — binding may resolve to a reducer)
    ///   2. `is_grounded_op(head)` (e.g. `+`, `cons-atom`, `get-type`)
    ///   3. `is_eager_special_form(head)` (e.g. `collapse`, `reduce`, `unquote`)
    ///   4. `should_pre_eval_by_type(head, env)` (head has `(-> ...)` arrow type)
    ///
    /// If the arg's head matches any of these (at runtime, against the VM's
    /// current env), we evaluate via the trampoline and return the reduced
    /// value. Otherwise the raw s-expression is preserved — structural
    /// operations see the syntactic form (e.g. `(car-atom (grandfather a b))`
    /// returns `grandfather`, never forcing a rule call on user-defined heads).
    fn maybe_pre_eval_structural(&mut self, v: V) -> VmResult<V> {
        use crate::backend::eval::step::should_pre_eval_by_type;
        use crate::backend::eval::{is_eager_special_form, is_grounded_op};

        let items = match v.as_sexpr() {
            Some(s) => s,
            None => return Ok(v),
        };
        let head = match items.first().and_then(|h| h.as_atom()) {
            Some(h) => h,
            None => return Ok(v),
        };
        let env_ref = match self.env.as_ref() {
            Some(e) => e,
            None => return Ok(v),
        };
        let should_reduce = head.starts_with('$')
            || is_grounded_op(head)
            || is_eager_special_form(head)
            || should_pre_eval_by_type::<V, F>(head, env_ref);
        if !should_reduce {
            return Ok(v);
        }

        // Reduce via the trampoline, mirroring `eval_sub_expr_vm` pattern
        // used by `op_eval_if_reducible`/`op_eval_match` (line 2805+).
        let env = self.env.clone().ok_or_else(|| {
            VmError::Runtime("structural op: no environment available".to_string())
        })?;
        self.eval_sub_expr_vm(v, env)
    }

    /// Shared implementation of `car-atom` semantics: given a (possibly
    /// pre-evaluated) value, push the head onto the VM stack. Mirrors the
    /// `GetHead` arm at line 1345 of `step` and the quoted-transparency case.
    fn push_head_of(&mut self, a: V) -> VmResult<()> {
        // H3 (2026-05-05) hard-cut: empty/non-expr → push HE Error,
        // do NOT halt VM. Quoted-transparency extension removed.
        if let Some(items) = a.as_sexpr() {
            if let Some(first) = items.first() {
                self.push(first.clone());
                return Ok(());
            }
        }
        let call = self.make_sexpr(vec![self.make_atom("car-atom"), a.clone()]);
        let err = self.make_error(
            "car-atom expects a non-empty expression as an argument",
            call,
        );
        self.push(err);
        Ok(())
    }

    /// Shared implementation of `cdr-atom` semantics. H3 hard-cut: same shape.
    fn push_tail_of(&mut self, a: V) -> VmResult<()> {
        if let Some(items) = a.as_sexpr() {
            if !items.is_empty() {
                let tail: Vec<V> = items[1..].to_vec();
                self.push(self.make_sexpr(tail));
                return Ok(());
            }
        }
        let call = self.make_sexpr(vec![self.make_atom("cdr-atom"), a.clone()]);
        let err = self.make_error(
            "cdr-atom expects a non-empty expression as an argument",
            call,
        );
        self.push(err);
        Ok(())
    }

    /// Evaluate a sub-expression using the full trampoline evaluator.
    ///
    /// Uses `eval_trampoline` for proper recursive evaluation with
    /// TCO (tail-call optimization) and CPS (continuation-passing style).
    /// This ensures sub-expression evaluation is lazy, handles nondeterminism
    /// correctly, and doesn't stack-overflow on deeply nested expressions.
    ///
    /// Returns the first result from the trampoline (deterministic selection
    /// for applicative pre-evaluation). Data constructors are returned unchanged
    /// since the trampoline returns them as-is when no rules match.
    /// Phase 1b-E: evaluate a sub-expression via the trampoline and
    /// COMPOSE its resulting bindings into the VM's `current_bindings`.
    /// Returns just the value. The caller composes nothing further —
    /// subsequent operations in the same applicative pre-eval context
    /// automatically observe the bindings established here through
    /// `self.current_bindings`, matching HE's InterpretedAtom
    /// per-alt (Stack, Bindings) threading.
    ///
    /// When no results are produced, returns the expression unchanged
    /// (self-evaluating data constructor) and leaves `current_bindings`
    /// unchanged.
    ///
    /// On genuine ground/ground conflict between the existing
    /// `current_bindings` and the sub-expression's bindings, this
    /// method returns `VmError::Runtime` so the VM's Fail path can
    /// prune the inconsistent branch — HE-faithful.
    ///
    /// `forgetting_copy_types`: V may be Copy in some monomorphizations
    /// (e.g., V == MettaValue) — the `mem::forget` calls below are correct
    /// for the non-Copy case and a harmless no-op for the Copy case.
    #[allow(forgetting_copy_types)]
    fn eval_sub_expr_vm(&mut self, sub_expr: V, env: GenericEnvironment<V, F>) -> VmResult<V> {
        use crate::backend::eval::bindings::{apply_chain_generic, compose_outer_inner_generic};
        use crate::backend::eval::trampoline::eval_loop::eval_trampoline;
        use crate::backend::models::GenericBindings;

        // Create a lightweight EvalContext adapter for the trampoline.
        let ctx = VmEvalContext {
            factory: crate::backend::models::global_factory(),
        };

        // eval_trampoline now takes MettaValue + MettaEnvironment.
        // Transmute via TypeId check — in practice V is always MettaValue.
        assert_eq!(
            TypeId::of::<V>(),
            TypeId::of::<MettaValue>(),
            "eval_sub_expr_vm: V must be MettaValue"
        );
        // SAFETY: V == MettaValue verified above. Identical layouts.
        let metta_sub_expr: MettaValue =
            unsafe { std::ptr::read(&sub_expr as *const V as *const MettaValue) };
        let metta_env: crate::backend::eval::trampoline::MettaEnvironment = unsafe {
            std::ptr::read(
                &env as *const GenericEnvironment<V, F>
                    as *const crate::backend::eval::trampoline::MettaEnvironment,
            )
        };
        std::mem::forget(sub_expr);
        std::mem::forget(env);

        // Full trampoline evaluation: trampolined, TCO, CPS-based.
        // Returns (Vec<results>, final_env).
        let (results, _final_env) = eval_trampoline(metta_sub_expr.clone(), metta_env, &ctx);

        if let Some((first_metta, first_b_metta)) = results.into_iter().next() {
            // SAFETY: V == MettaValue verified above. Transmute back.
            let value: V = unsafe { std::ptr::read(&first_metta as *const MettaValue as *const V) };
            let sub_bindings: GenericBindings<V> = unsafe {
                std::ptr::read(
                    &first_b_metta as *const GenericBindings<MettaValue>
                        as *const GenericBindings<V>,
                )
            };
            std::mem::forget(first_metta);
            std::mem::forget(first_b_metta);

            // Phase 1b-E: compose the sub-expression's bindings into
            // current_bindings so subsequent arg pre-evals see this
            // arg's ambient context.
            if !sub_bindings.is_empty() {
                let mut composed = compose_outer_inner_generic(
                    &self.current_bindings,
                    &sub_bindings,
                    &self.factory,
                );
                if composed.is_empty()
                    && !self.current_bindings.is_empty()
                    && !sub_bindings.is_empty()
                {
                    // Genuine user-level binding inconsistency — the
                    // applicative pre-eval branch is inconsistent.
                    // Signal via Runtime error so the VM's Fail path
                    // can prune (HE-faithful strict unification).
                    return Err(VmError::Runtime(
                        "eval_sub_expr_vm: ground/ground binding conflict (branch inconsistent)"
                            .to_string(),
                    ));
                }
                apply_chain_generic(&mut composed, &self.factory);
                self.current_bindings = composed;
            }
            Ok(value)
        } else {
            // No results — return expression unchanged (data constructor).
            // current_bindings unchanged.
            let value: V =
                unsafe { std::ptr::read(&metta_sub_expr as *const MettaValue as *const V) };
            std::mem::forget(metta_sub_expr);
            Ok(value)
        }
    }

    /// Evaluate a sub-expression via the trampoline, returning ALL
    /// `(value, bindings)` pairs. Companion to `eval_sub_expr_vm_all`
    /// that preserves per-result bindings for downstream Cartesian-
    /// product composition (HE-bisimilar multi-result pre-eval).
    #[allow(forgetting_copy_types)]
    fn eval_sub_expr_vm_all_with_bindings(
        &self,
        sub_expr: V,
        env: GenericEnvironment<V, F>,
    ) -> Vec<(V, crate::backend::models::GenericBindings<V>)> {
        use crate::backend::eval::trampoline::eval_loop::eval_trampoline;
        use crate::backend::models::GenericBindings;

        let ctx = VmEvalContext {
            factory: crate::backend::models::global_factory(),
        };

        assert_eq!(
            TypeId::of::<V>(),
            TypeId::of::<MettaValue>(),
            "eval_sub_expr_vm_all_with_bindings: V must be MettaValue"
        );
        // SAFETY: V == MettaValue verified above. Identical layouts.
        let metta_sub_expr: MettaValue =
            unsafe { std::ptr::read(&sub_expr as *const V as *const MettaValue) };
        let metta_env: crate::backend::eval::trampoline::MettaEnvironment = unsafe {
            std::ptr::read(
                &env as *const GenericEnvironment<V, F>
                    as *const crate::backend::eval::trampoline::MettaEnvironment,
            )
        };
        // Suppress the old-value drops — `metta_sub_expr` and
        // `metta_env` now own those Arc refs (ptr::read doesn't
        // increment the refcount). Dropping `sub_expr` / `env` would
        // under-decrement and trigger an Arc-counter underflow later.
        std::mem::forget(sub_expr);
        std::mem::forget(env);

        let (results, _final_env) = eval_trampoline(metta_sub_expr, metta_env, &ctx);
        // eval_trampoline returns SmallVec; materialize into Vec so the
        // caller can consume via into_iter() regardless of inline size.
        let metta_results: Vec<(MettaValue, GenericBindings<MettaValue>)> = results.into_vec();
        // SAFETY: V == MettaValue verified above. Transmute the Vec element
        // type from MettaValue to V (identical layout).
        unsafe {
            let mut v_results = std::mem::ManuallyDrop::new(metta_results);
            Vec::from_raw_parts(
                v_results.as_mut_ptr() as *mut (V, GenericBindings<V>),
                v_results.len(),
                v_results.capacity(),
            )
        }
    }


    // === Space Operations ===

    /// Add an atom to a space.
    ///
    /// Stack: [space, atom] -> [Unit]
    ///
    /// Three resolution paths matching the trampoline reference at
    /// `eval/trampoline/eval_loop.rs::eval_add_atom_*`:
    ///   1. `Space(handle)` → if module-space or `&self`, route through env's
    ///      PathMap (`env.add_to_space`) so rule definitions populate
    ///      RuleIndex; else write to the SpaceHandle directly.
    ///   2. `Atom("&self")` (un-evaluated form) → same env path.
    ///   3. `Atom(name)` → resolve token; recurse on the resolved Space.
    ///   4. Otherwise → TypeError.
    fn op_space_add(&mut self) -> VmResult<()> {
        let atom = self.pop()?;
        let space = self.pop()?;

        if let Some(handle) = space.as_space() {
            if handle.is_module_space() || handle.name == "self" {
                let env = self.env.as_mut().ok_or_else(|| {
                    VmError::Runtime("add-atom: no environment for &self/module space".to_string())
                })?;
                env.add_to_space(&atom);
            } else {
                handle.add_atom_generic(&atom);
            }
            crate::backend::eval::trampoline::dispatch_hints::increment_mutation_epoch();
            self.push(self.make_unit());
            return Ok(());
        }

        if let Some(name) = space.as_atom() {
            if name == "&self" {
                let env = self.env.as_mut().ok_or_else(|| {
                    VmError::Runtime("add-atom: no environment for &self".to_string())
                })?;
                env.add_to_space(&atom);
                crate::backend::eval::trampoline::dispatch_hints::increment_mutation_epoch();
                self.push(self.make_unit());
                return Ok(());
            }
            if let Some(env) = self.env.as_ref() {
                if let Some(resolved) = env.lookup_token_generic(name, &self.factory) {
                    if let Some(handle) = resolved.as_space() {
                        if handle.is_module_space() || handle.name == "self" {
                            // Rare: a token resolves to a module/self handle.
                            let env_mut = self.env.as_mut().expect("env present");
                            env_mut.add_to_space(&atom);
                        } else {
                            handle.add_atom_generic(&atom);
                        }
                        crate::backend::eval::trampoline::dispatch_hints::increment_mutation_epoch(
                        );
                        self.push(self.make_unit());
                        return Ok(());
                    }
                }
            }
        }

        Err(VmError::TypeError {
            expected: "Space",
            got: space.type_name(),
        })
    }

    /// Remove an atom from a space.
    ///
    /// Stack: [space, atom] -> [Unit]
    ///
    /// Returns `Unit` per HE / spec §9.2 (the boolean removed-flag is discarded —
    /// HE's `RemoveAtomOp::execute` ignores it). Same three resolution paths as
    /// `op_space_add`.
    fn op_space_remove(&mut self) -> VmResult<()> {
        let atom = self.pop()?;
        let space = self.pop()?;

        if let Some(handle) = space.as_space() {
            if handle.is_module_space() || handle.name == "self" {
                let env = self.env.as_mut().ok_or_else(|| {
                    VmError::Runtime(
                        "remove-atom: no environment for &self/module space".to_string(),
                    )
                })?;
                env.remove_from_space(&atom);
            } else {
                let _removed = handle.remove_atom_generic(&atom);
            }
            crate::backend::eval::trampoline::dispatch_hints::increment_mutation_epoch();
            self.push(self.make_unit());
            return Ok(());
        }

        if let Some(name) = space.as_atom() {
            if name == "&self" {
                let env = self.env.as_mut().ok_or_else(|| {
                    VmError::Runtime("remove-atom: no environment for &self".to_string())
                })?;
                env.remove_from_space(&atom);
                crate::backend::eval::trampoline::dispatch_hints::increment_mutation_epoch();
                self.push(self.make_unit());
                return Ok(());
            }
            if let Some(env) = self.env.as_ref() {
                if let Some(resolved) = env.lookup_token_generic(name, &self.factory) {
                    if let Some(handle) = resolved.as_space() {
                        if handle.is_module_space() || handle.name == "self" {
                            let env_mut = self.env.as_mut().expect("env present");
                            env_mut.remove_from_space(&atom);
                        } else {
                            let _removed = handle.remove_atom_generic(&atom);
                        }
                        crate::backend::eval::trampoline::dispatch_hints::increment_mutation_epoch(
                        );
                        self.push(self.make_unit());
                        return Ok(());
                    }
                }
            }
        }

        Err(VmError::TypeError {
            expected: "Space",
            got: space.type_name(),
        })
    }

    /// Get all atoms from a space (collapse).
    /// Stack: [space] -> [SExpr with atoms]
    fn op_space_get_atoms(&mut self) -> VmResult<()> {
        let space_val = self.pop()?;

        // Resolve the space: either a direct Space handle or a named token (e.g., &kb)
        let atoms: Vec<V> = if let Some(handle) = space_val.as_space() {
            handle.collapse_generic(&self.factory)
        } else if let Some(name) = space_val.as_atom() {
            if name == "&self" {
                if let Some(env) = &self.env {
                    env.get_all_atoms()
                } else {
                    Vec::new()
                }
            } else if let Some(env) = &self.env {
                // Named space (e.g., &kb) → resolve through tokenizer
                if let Some(resolved) = env.lookup_token_generic(name, &self.factory) {
                    if let Some(handle) = resolved.as_space() {
                        handle.collapse_generic(&self.factory)
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
        };

        // Return atoms as nondeterministic superposition via choice points
        // (same pattern as op_match_self for multi-result operations).
        if atoms.is_empty() {
            self.unreduced = true;
            self.push(self.make_sexpr(vec![]));
        } else if atoms.len() == 1 {
            self.push(atoms.into_iter().next().expect("atoms is non-empty"));
        } else {
            let mut iter = atoms.into_iter();
            let first = iter.next().expect("atoms is non-empty");
            let alternatives: Vec<GenericAlternative<V, GenericBytecodeChunk<V>>> =
                iter.map(GenericAlternative::Value).collect();
            self.choice_points.push(GenericChoicePoint {
                ip: self.ip,
                chunk: Arc::clone(&self.chunk),
                value_stack_height: self.value_stack.len(),
                call_stack_height: self.call_stack.len(),
                bindings_stack_height: self.bindings_stack.len(),
                alternatives,
                saved_unreduced: self.unreduced,
                trail_height: self.trail.len(),
                saved_current_bindings: self.current_bindings.clone(),
                locals_height: self.locals.len(),
                locals_base_at_cp: self.locals_base,
            });
            self.push(first);
        }
        Ok(())
    }

    /// Match pattern against atoms in a space and instantiate template with bindings.
    ///
    /// Stack: [space, pattern, template] -> [results...]
    fn op_space_match(&mut self) -> VmResult<()> {
        let template = self.pop()?;
        let pattern = self.pop()?;
        let space = self.pop()?;

        // X.6 MTT-FN-SPACE-RESOLVE: resolve env-bound atom names like
        // `&space` to their SpaceHandle (mirror of resolve_to_state_id).
        if let Some(handle) = self.resolve_to_space_handle_owned(&space) {
            let atoms: Vec<V> = handle.collapse_generic(&self.factory);
            // Preallocate to atoms.len() — upper bound on matches.
            let mut results: Vec<V> = Vec::with_capacity(atoms.len());

            // Match pattern against each atom and instantiate template
            for atom in &atoms {
                if let Some(bindings) = self.pattern_match_bind_generic(&pattern, atom) {
                    // Substitute bindings into template
                    let instantiated = self.substitute_bindings_generic(&template, &bindings);
                    results.push(instantiated);
                }
            }

            // S5: HE-bisimilar `match` returns bare nondet results (NOT a
            // tuple wrap). Mirrors `op_space_get_atoms` fan-out at L8266-8292.
            // HE reference: hyperon-experimental/lib/src/metta/runner/stdlib/
            // core.rs:155-167 returns Vec<(Atom, Option<Bindings>)> bare.
            if results.is_empty() {
                // No match: empty nondet result. Push Empty sentinel so
                // downstream opcodes see a value (HE returns no results,
                // which is the empty superposition).
                self.unreduced = true;
                self.push(self.factory.empty());
            } else if results.len() == 1 {
                self.push(results.into_iter().next().expect("results non-empty"));
            } else {
                let mut iter = results.into_iter();
                let first = iter.next().expect("results non-empty");
                let alternatives: Vec<GenericAlternative<V, GenericBytecodeChunk<V>>> =
                    iter.map(GenericAlternative::Value).collect();
                self.choice_points.push(GenericChoicePoint {
                    ip: self.ip,
                    chunk: Arc::clone(&self.chunk),
                    value_stack_height: self.value_stack.len(),
                    call_stack_height: self.call_stack.len(),
                    bindings_stack_height: self.bindings_stack.len(),
                    alternatives,
                    saved_unreduced: self.unreduced,
                    trail_height: self.trail.len(),
                    saved_current_bindings: self.current_bindings.clone(),
                    locals_height: self.locals.len(),
                    locals_base_at_cp: self.locals_base,
                });
                self.push(first);
            }
            Ok(())
        } else {
            // Per T1.A errors-as-values pattern: push an Error atom rather
            // than returning VmError, so downstream opcodes can short-circuit.
            let err = self.factory.error(
                space,
                self.factory.string("match: first argument must be a space"),
            );
            self.push(err);
            Ok(())
        }
    }

    /// Load a space by name from the constant pool.
    fn op_load_space(&mut self) -> VmResult<()> {
        let const_idx = self.read_u16()?;
        let name = self
            .chunk
            .get_constant(const_idx)
            .ok_or(VmError::InvalidConstant(const_idx))?
            .clone();

        if let Some(space_name) = name.as_atom() {
            let handle = SpaceHandle::new(xxh3_64(space_name.as_bytes()), space_name.to_string());
            self.push(self.factory.space(handle));
            Ok(())
        } else {
            Err(VmError::TypeError {
                expected: "Atom (space name)",
                got: name.type_name(),
            })
        }
    }

    /// Substitute variable bindings into a template expression.
    fn substitute_bindings_generic(&self, template: &V, bindings: &[(String, V)]) -> V {
        // Variables are substituted with bound values
        if let Some(name) = template.as_atom() {
            if name.starts_with('$') {
                // Look up the variable in bindings
                if let Some((_, v)) = bindings.iter().find(|(n, _)| n == name) {
                    return v.clone();
                }
            }
            return template.clone();
        }

        // S-expressions are recursively substituted
        if let Some(items) = template.as_sexpr() {
            let substituted: Vec<V> = items
                .iter()
                .map(|item| self.substitute_bindings_generic(item, bindings))
                .collect();
            return self.make_sexpr(substituted);
        }

        // All other values pass through unchanged
        template.clone()
    }

    // === State Operations ===

    fn op_new_state(&mut self) -> VmResult<()> {
        let initial = self.pop()?;

        let env = self
            .env
            .as_mut()
            .ok_or_else(|| VmError::Runtime("new-state requires environment".to_string()))?;

        let state_id = env.create_state(&initial);
        crate::backend::eval::trampoline::dispatch_hints::increment_mutation_epoch();
        self.push(self.factory.state(state_id));
        Ok(())
    }

    /// Resolve a stack value to a State id, looking up env-bound atoms.
    ///
    /// Post-T1.A, op_get_state / op_change_state can encounter the bound
    /// atom name (e.g. `&c`) rather than its resolved State value, because
    /// errors no longer propagate as VmError to trigger T0 fallback. This
    /// helper does the env lookup inline so state ops succeed in T1.
    fn resolve_to_state_id(&self, state_ref: &V) -> Option<u64> {
        if let Some(id) = state_ref.as_state() {
            return Some(id);
        }
        if let Some(name) = state_ref.as_atom() {
            if let Some(env) = self.env.as_ref() {
                if let Some(resolved) = env.lookup_token_generic(name, &self.factory) {
                    return resolved.as_state();
                }
            }
        }
        None
    }

    /// Resolve a stack value to an owned SpaceHandle, looking up env-bound atoms.
    ///
    /// X.6 MTT-FN-SPACE-RESOLVE: mirror of `resolve_to_state_id` for
    /// `op_match_external`, `op_match_external_or`, `op_space_match`. When a
    /// user `bind!`s a fresh space (e.g. `(bind! &space (new-space))`) the
    /// downstream match opcodes pop the bound atom name rather than the
    /// Space value; without an inline env lookup the bare `as_space()` fails
    /// and a TypeError leaks instead of dispatching to the bound handle.
    /// `SpaceHandle` derives `Clone` (cheap Arc share) so we return owned.
    fn resolve_to_space_handle_owned(&self, space_ref: &V) -> Option<SpaceHandle> {
        if let Some(handle) = space_ref.as_space() {
            return Some(handle.clone());
        }
        if let Some(name) = space_ref.as_atom() {
            if let Some(env) = self.env.as_ref() {
                if let Some(resolved) = env.lookup_token_generic(name, &self.factory) {
                    return resolved.as_space().cloned();
                }
            }
        }
        None
    }

    fn op_get_state(&mut self) -> VmResult<()> {
        let state_ref = self.pop()?;

        if let Some(state_id) = self.resolve_to_state_id(&state_ref) {
            let env = self
                .env
                .as_ref()
                .ok_or_else(|| VmError::Runtime("get-state requires environment".to_string()))?;

            if let Some(value) = env.get_state(state_id) {
                self.push(value);
                Ok(())
            } else {
                Err(VmError::Runtime(format!(
                    "get-state: state {} not found",
                    state_id
                )))
            }
        } else {
            Err(VmError::TypeError {
                expected: "State",
                got: state_ref.type_name(),
            })
        }
    }

    fn op_change_state(&mut self) -> VmResult<()> {
        let new_value = self.pop()?;
        let state_ref = self.pop()?;

        if let Some(state_id) = self.resolve_to_state_id(&state_ref) {
            let env = self.env.as_mut().ok_or_else(|| {
                VmError::Runtime("change-state! requires environment".to_string())
            })?;

            if env.change_state(state_id, &new_value) {
                crate::backend::eval::trampoline::dispatch_hints::increment_mutation_epoch();
                self.push(self.factory.state(state_id));
                Ok(())
            } else {
                Err(VmError::Runtime(format!(
                    "change-state!: state {} not found",
                    state_id
                )))
            }
        } else {
            Err(VmError::TypeError {
                expected: "State",
                got: state_ref.type_name(),
            })
        }
    }

    // === Debug Operations ===

    fn op_breakpoint(&mut self) -> VmResult<()> {
        // Breakpoint - just continue
        Ok(())
    }

    fn op_trace(&mut self) -> VmResult<()> {
        // Trace - print stack top
        if let Ok(value) = self.peek() {
            trace!(target: "mettatron::vm::trace", value = %value.friendly_repr());
        }
        Ok(())
    }

    // === Pattern Matching Helpers ===

    /// Check if pattern matches value using trait methods.
    fn pattern_matches_generic(&self, pattern: &V, value: &V) -> bool {
        match pattern.view() {
            ValueView::Atom(s) if s.starts_with('$') || s == "_" || s == "$_" => true,
            ValueView::SExpr(_) => {
                let p_items = pattern.as_sexpr().expect("matched SExpr");
                // S3 (ERROR-MATCH cross-shape): HE represents errors as
                // 3-element `(Error <offending> <detail>)` SExprs; MeTTaTron
                // stores them as a dedicated Error variant. Project the
                // variant into the pseudo-SExpr shape before matching.
                if let Some((v_off, v_detail)) = value.as_error() {
                    if p_items.len() == 3
                        && matches!(p_items[0].view(), ValueView::Atom(s) if s == "Error")
                    {
                        return self.pattern_matches_generic(&p_items[1], v_off)
                            && self.pattern_matches_generic(&p_items[2], v_detail);
                    }
                    return false;
                }
                if let Some(v_items) = value.as_sexpr() {
                    p_items.len() == v_items.len()
                        && p_items
                            .iter()
                            .zip(v_items.iter())
                            .all(|(p, v)| self.pattern_matches_generic(p, v))
                } else {
                    false
                }
            }
            // S3 symmetric: Error-variant pattern vs SExpr-shaped value.
            ValueView::Error(_, _) => {
                if let Some((p_off, p_detail)) = pattern.as_error() {
                    if let Some(v_items) = value.as_sexpr() {
                        if v_items.len() == 3
                            && matches!(v_items[0].view(), ValueView::Atom(s) if s == "Error")
                        {
                            return self.pattern_matches_generic(p_off, &v_items[1])
                                && self.pattern_matches_generic(p_detail, &v_items[2]);
                        }
                    }
                }
                pattern.structurally_equivalent(value)
            }
            _ => pattern.structurally_equivalent(value),
        }
    }

    /// Pattern match with binding extraction.
    fn pattern_match_bind_generic(&self, pattern: &V, value: &V) -> Option<Vec<(String, V)>> {
        let mut bindings = Vec::new();
        if self.pattern_match_bind_recursive(pattern, value, &mut bindings) {
            Some(bindings)
        } else {
            None
        }
    }

    fn pattern_match_bind_recursive(
        &self,
        pattern: &V,
        value: &V,
        bindings: &mut Vec<(String, V)>,
    ) -> bool {
        // **Stack-safety mandate (2026-05-15)**: iterative pair-stack (audit
        // item T1.1). Preserves the S3 Error/SExpr cross-shape branches verbatim.
        let mut work: Vec<(V, V)> = Vec::with_capacity(8);
        work.push((pattern.clone(), value.clone()));
        while let Some((pat, val)) = work.pop() {
            match pat.view() {
                ValueView::Atom(s) if s == "_" || s == "$_" => {}
                ValueView::Atom(s) if s.starts_with('$') => {
                    bindings.push((s.to_string(), val.clone()));
                }
                ValueView::SExpr(_) => {
                    let p_items = pat.as_sexpr().expect("matched SExpr");
                    if let Some((v_off, v_detail)) = val.as_error() {
                        if p_items.len() == 3
                            && matches!(p_items[0].view(), ValueView::Atom(s) if s == "Error")
                        {
                            work.push((p_items[2].clone(), v_detail.clone()));
                            work.push((p_items[1].clone(), v_off.clone()));
                            continue;
                        }
                        return false;
                    }
                    if let Some(v_items) = val.as_sexpr() {
                        if p_items.len() != v_items.len() {
                            return false;
                        }
                        for (p, v) in p_items.iter().zip(v_items.iter()).rev() {
                            work.push((p.clone(), v.clone()));
                        }
                    } else {
                        return false;
                    }
                }
                ValueView::Error(_, _) => {
                    if let Some((p_off, p_detail)) = pat.as_error() {
                        if let Some(v_items) = val.as_sexpr() {
                            if v_items.len() == 3
                                && matches!(v_items[0].view(), ValueView::Atom(s) if s == "Error")
                            {
                                work.push((p_detail.clone(), v_items[2].clone()));
                                work.push((p_off.clone(), v_items[1].clone()));
                                continue;
                            }
                        }
                    }
                    if !pat.structurally_equivalent(&val) {
                        return false;
                    }
                }
                _ => {
                    if !pat.structurally_equivalent(&val) {
                        return false;
                    }
                }
            }
        }
        true
    }
}

// ============================================================================
// Convenience Constructors (F: Default)
// ============================================================================

impl<V, F> GenericBytecodeVM<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + PartialEq + 'static,
    F: MettaValueFactory<V> + Copy + Clone + Send + Sync + Default + 'static,
{
    /// Create a new VM with the given chunk, using the default factory.
    pub fn new(chunk: Arc<GenericBytecodeChunk<V>>) -> Self {
        Self::with_factory(chunk, F::default())
    }

    /// Create a new VM with custom configuration, using the default factory.
    pub fn with_config(chunk: Arc<GenericBytecodeChunk<V>>, config: VmConfig) -> Self {
        Self::with_config_and_factory(chunk, config, F::default())
    }

    /// Create a new VM with an environment, using the default factory.
    pub fn with_env(chunk: Arc<GenericBytecodeChunk<V>>, env: GenericEnvironment<V, F>) -> Self {
        Self::with_env_and_factory(chunk, env, F::default())
    }

    /// Push a value onto the results vector (for testing).
    #[cfg(test)]
    pub fn push_result(&mut self, value: V) {
        self.results.push(value);
    }

    /// Get the number of unexplored choice points.
    /// Used by eval_inner to detect nondeterministic dispatch that needs
    /// TreeWalker fallback.
    pub fn choice_points_len(&self) -> usize {
        self.choice_points.len()
    }

    /// Get the number of entries in the memo cache (for testing).
    #[cfg(test)]
    pub fn memo_cache_len(&self) -> usize {
        self.memo_cache.len()
    }

    /// Get memo cache statistics (for testing).
    #[cfg(all(test, feature = "track-stats"))]
    pub fn memo_cache_stats(&self) -> super::memo_cache::CacheStats {
        self.memo_cache.stats()
    }
}

// ============================================================================
// Free Helper Functions
// ============================================================================

/// In-tier structural unification for VM-side repeated-variable consistency
/// (BUG T0-T1-009 fix, plan invariant #2 tier-locality).
///
/// Returns true iff `a` and `b` unify structurally. Iterative work-stack —
/// no recursion. Matches the T0 canonical matcher's structural compare but
/// is implemented inline in the VM tier (no FFI to T0). Both inputs are
/// already evaluated values, so the check is structural equality with
/// cross-tier Long↔Float promotion (HE-aligned per spec §I.4.3).
///
/// Generic over `V: MettaValueTrait` so the VM continues to work for any
/// value type the bytecode VM is parameterized over.
fn structurally_unify<V: MettaValueTrait + Clone>(a: &V, b: &V) -> bool {
    let mut work: smallvec::SmallVec<[(V, V); 8]> = smallvec::SmallVec::new();
    work.push((a.clone(), b.clone()));

    while let Some((lhs, rhs)) = work.pop() {
        // Ground types: trait-method dispatch (no ValueView coupling).
        if let (Some(x), Some(y)) = (lhs.as_bool(), rhs.as_bool()) {
            if x != y {
                return false;
            }
            continue;
        }
        // Long↔Float promotion: try numeric coercion via as_long/as_float.
        match (lhs.as_long(), lhs.as_float(), rhs.as_long(), rhs.as_float()) {
            (Some(x), _, Some(y), _) => {
                if x != y {
                    return false;
                }
                continue;
            }
            (_, Some(x), _, Some(y)) => {
                if x != y {
                    return false;
                }
                continue;
            }
            (Some(x), _, _, Some(y)) => {
                if (x as f64) != y {
                    return false;
                }
                continue;
            }
            (_, Some(x), Some(y), _) => {
                if x != (y as f64) {
                    return false;
                }
                continue;
            }
            _ => {}
        }
        if let (Some(x), Some(y)) = (lhs.as_string(), rhs.as_string()) {
            if x != y {
                return false;
            }
            continue;
        }
        if let (Some(x), Some(y)) = (lhs.as_atom(), rhs.as_atom()) {
            if x != y {
                return false;
            }
            continue;
        }
        if lhs.is_unit() && rhs.is_unit() {
            continue;
        }
        if lhs.is_empty() && rhs.is_empty() {
            continue;
        }
        if let (Some(xs), Some(ys)) = (lhs.as_sexpr(), rhs.as_sexpr()) {
            if xs.len() != ys.len() {
                return false;
            }
            for (x, y) in xs.iter().zip(ys.iter()).rev() {
                work.push((x.clone(), y.clone()));
            }
            continue;
        }
        if let (Some(xs), Some(ys)) = (lhs.as_conjunction(), rhs.as_conjunction()) {
            if xs.len() != ys.len() {
                return false;
            }
            for (x, y) in xs.iter().zip(ys.iter()).rev() {
                work.push((x.clone(), y.clone()));
            }
            continue;
        }
        if let (Some((xmsg, xdet)), Some((ymsg, ydet))) = (lhs.as_error(), rhs.as_error()) {
            if xmsg != ymsg {
                return false;
            }
            work.push((xdet.clone(), ydet.clone()));
            continue;
        }
        if let (Some(x), Some(y)) = (lhs.as_type(), rhs.as_type()) {
            work.push((x.clone(), y.clone()));
            continue;
        }
        // Cross-variant or unsupported: not structurally equal.
        return false;
    }
    true
}

/// Map a `ValueView` to its MeTTa metatype string.
///
/// Thin wrapper over `ValueView::metatype()` (the single source of truth shared
/// with the T0 trampoline). Kept as a free function for the existing call site
/// at `op_get_metatype` to minimize diff churn.
#[inline]
fn metatype_of_view(view: ValueView) -> &'static str {
    view.metatype()
}

// ============================================================================
// Type Aliases
// ============================================================================

use crate::backend::models::GcFactory;

/// The primary bytecode VM type, backed by the global GC slab allocator.
///
/// This is a type alias for `GenericBytecodeVM<MettaValue, GcFactory>`.
/// All existing `BytecodeVM::new(chunk)` call sites continue to work
/// because `GcFactory` implements `Default` (returning `global_factory()`).
pub type BytecodeVM = GenericBytecodeVM<MettaValue, GcFactory>;
