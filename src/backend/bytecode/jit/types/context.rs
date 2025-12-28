//! JIT runtime context.
//!
//! This module defines [`JitContext`], the runtime context passed to
//! JIT-compiled code for managing execution state.

use std::fmt;

use super::binding::JitBindingFrame;
use super::constants::{
    MAX_STACK_SAVE_VALUES, STACK_SAVE_POOL_SIZE, STATE_CACHE_MASK, STATE_CACHE_SIZE,
    VAR_INDEX_CACHE_SIZE,
};
use super::nondet::{JitBailoutReason, JitChoicePoint};
use super::value::JitValue;
use crate::backend::models::MettaValue;

// =============================================================================
// JitContext - Runtime Context
// =============================================================================

/// Runtime context passed to JIT-compiled code.
///
/// This struct is `#[repr(C)]` to ensure predictable memory layout
/// for access from Cranelift-generated code.
///
/// # Memory Layout
///
/// The context provides direct access to:
/// - Value stack (for operands and results)
/// - Constant pool (for loading literals)
/// - Bailout flags (for returning to bytecode VM)
#[repr(C)]
pub struct JitContext {
    /// Pointer to the value stack base
    pub value_stack: *mut JitValue,

    /// Current stack pointer (index of next free slot)
    pub sp: usize,

    /// Stack capacity (for bounds checking in debug mode)
    pub stack_cap: usize,

    /// Pointer to constant pool
    pub constants: *const MettaValue,

    /// Number of constants in the pool
    pub constants_len: usize,

    /// Bailout flag - set true when JIT code cannot continue
    pub bailout: bool,

    /// Instruction pointer to resume at after bailout
    pub bailout_ip: usize,

    /// Reason for bailout (error code)
    pub bailout_reason: JitBailoutReason,

    // -------------------------------------------------------------------------
    // Non-determinism support (choice points and results)
    // -------------------------------------------------------------------------
    /// Pointer to choice point stack base
    pub choice_points: *mut JitChoicePoint,

    /// Current number of choice points
    pub choice_point_count: usize,

    /// Maximum number of choice points (capacity)
    pub choice_point_cap: usize,

    /// Pointer to results buffer (for collecting non-deterministic results)
    pub results: *mut JitValue,

    /// Current number of results collected
    pub results_count: usize,

    /// Results buffer capacity
    pub results_cap: usize,

    // -------------------------------------------------------------------------
    // Call/TailCall support (Phase 3)
    // -------------------------------------------------------------------------
    /// Pointer to MorkBridge for rule dispatch (may be null)
    pub bridge_ptr: *const (),

    /// Pointer to current BytecodeChunk (for IP tracking)
    pub current_chunk: *const (),

    // -------------------------------------------------------------------------
    // Rule Dispatch support (Phase C)
    // -------------------------------------------------------------------------
    /// Pointer to Vec<CompiledRule> from last dispatch_rules call
    /// Owned by JitContext - must be freed when context is dropped
    pub current_rules: *mut (),

    /// Current rule index being tried (0..len of current_rules)
    pub current_rule_idx: usize,

    // -------------------------------------------------------------------------
    // Native nondeterminism support (Stage 2 JIT)
    // -------------------------------------------------------------------------
    /// IP to resume at when re-entering JIT after backtracking
    pub resume_ip: usize,

    /// Whether currently executing in nondeterministic mode
    pub in_nondet_mode: bool,

    /// Current fork nesting depth
    pub fork_depth: usize,

    /// Pointer to saved stack values for backtracking
    pub saved_stack: *mut JitValue,

    /// Number of saved stack values
    pub saved_stack_count: usize,

    /// Capacity of saved stack buffer
    pub saved_stack_cap: usize,

    // -------------------------------------------------------------------------
    // Binding/Environment support (Phase A)
    // -------------------------------------------------------------------------
    /// Pointer to binding frames stack base
    pub binding_frames: *mut JitBindingFrame,

    /// Current number of binding frames
    pub binding_frames_count: usize,

    /// Maximum number of binding frames (capacity)
    pub binding_frames_cap: usize,

    // -------------------------------------------------------------------------
    // Registry/Cache support (Phase A - Full JIT)
    // -------------------------------------------------------------------------
    /// Pointer to ExternalRegistry for external function calls
    pub external_registry: *const (),

    /// Pointer to MemoCache for cached function calls
    pub memo_cache: *const (),

    /// Pointer to space registry for named spaces
    pub space_registry: *mut (),

    // -------------------------------------------------------------------------
    // Grounded Space support (Space Ops - Phase 1)
    // -------------------------------------------------------------------------
    /// Pre-resolved grounded space handles [&self, &kb, &stack]
    /// These are resolved at JIT entry and stored here for O(1) access.
    /// Points to an array of SpaceHandle pointers (or equivalent).
    pub grounded_spaces: *const *const (),

    /// Number of pre-resolved grounded spaces (typically 3)
    pub grounded_spaces_count: usize,

    /// Pointer to template results buffer for space match instantiation
    /// Used to store intermediate results during template evaluation
    pub template_results: *mut JitValue,

    /// Capacity of template results buffer
    pub template_results_cap: usize,

    // -------------------------------------------------------------------------
    // Cut scope support (Phase A - Full JIT)
    // -------------------------------------------------------------------------
    /// Pointer to cut marker stack (records choice_point_count at cut scope entry)
    pub cut_markers: *mut usize,

    /// Current number of cut markers
    pub cut_marker_count: usize,

    /// Cut marker stack capacity
    pub cut_marker_cap: usize,

    // -------------------------------------------------------------------------
    // Heap allocation tracking (for cleanup)
    // -------------------------------------------------------------------------
    /// Pointer to Vec of heap allocations to be freed on cleanup.
    /// These are raw pointers to Box<MettaValue> that were allocated during JIT execution.
    /// Set to null if heap tracking is disabled.
    pub heap_tracker: *mut Vec<*mut MettaValue>,

    // -------------------------------------------------------------------------
    // State operations support (Phase D.1)
    // -------------------------------------------------------------------------
    /// Pointer to Environment for state operations (new-state, get-state, change-state!)
    /// Required for mmverify which uses &sp state extensively.
    pub env_ptr: *mut (),

    // -------------------------------------------------------------------------
    // State operations cache (Optimization 5.1)
    // -------------------------------------------------------------------------
    /// Direct-mapped cache for recently accessed state values.
    /// Avoids RwLock acquisition and HashMap lookup for hot state accesses.
    /// Cache slot = state_id % STATE_CACHE_SIZE
    /// Entry format: (state_id, cached_value_bits)
    /// cached_value_bits is the raw u64 representation of JitValue (NaN-boxed)
    pub state_cache: [(u64, u64); STATE_CACHE_SIZE],

    /// Cache validity mask: bit N = 1 if slot N is valid
    pub state_cache_valid: u8,

    // -------------------------------------------------------------------------
    // Stack save pool (Optimization 5.2)
    // -------------------------------------------------------------------------
    /// Pool of pre-allocated stack save buffers for Fork operations.
    /// Each buffer can hold up to MAX_STACK_SAVE_VALUES JitValues.
    /// This is a ring buffer - `stack_save_pool_next` points to next available slot.
    /// Eliminates Box::leak() memory leaks from jit_runtime_fork_native.
    pub stack_save_pool: *mut JitValue,

    /// Total capacity of stack save pool (STACK_SAVE_POOL_SIZE * MAX_STACK_SAVE_VALUES)
    pub stack_save_pool_cap: usize,

    /// Next available slot index in the stack save pool (ring buffer index)
    /// Wraps around when reaching STACK_SAVE_POOL_SIZE
    pub stack_save_pool_next: usize,

    // -------------------------------------------------------------------------
    // Variable name index cache (Optimization 5.3)
    // -------------------------------------------------------------------------
    /// Direct-mapped cache for variable name → constant index lookups.
    /// Avoids O(n) constant array scan for repeated variable bindings.
    /// Entry format: (name_hash, constant_index)
    /// name_hash is hash of variable name for fast comparison
    /// constant_index is index into constants array, or u32::MAX for empty slot
    pub var_index_cache: [(u64, u32); VAR_INDEX_CACHE_SIZE],
}

impl JitContext {
    /// Create a new JitContext from stack and constant pool
    ///
    /// # Safety
    /// The caller must ensure:
    /// - `stack` has at least `stack_cap` elements allocated
    /// - `constants` points to valid memory for the lifetime of execution
    ///
    /// Note: This creates a context without non-determinism support.
    /// Use `with_nondet` to add choice point and results buffers.
    pub unsafe fn new(
        stack: *mut JitValue,
        stack_cap: usize,
        constants: *const MettaValue,
        constants_len: usize,
    ) -> Self {
        JitContext {
            value_stack: stack,
            sp: 0,
            stack_cap,
            constants,
            constants_len,
            bailout: false,
            bailout_ip: 0,
            bailout_reason: JitBailoutReason::None,
            // Non-determinism disabled by default
            choice_points: std::ptr::null_mut(),
            choice_point_count: 0,
            choice_point_cap: 0,
            results: std::ptr::null_mut(),
            results_count: 0,
            results_cap: 0,
            // Call/TailCall support
            bridge_ptr: std::ptr::null(),
            current_chunk: std::ptr::null(),
            // Rule dispatch support (Phase C)
            current_rules: std::ptr::null_mut(),
            current_rule_idx: 0,
            // Native nondeterminism support (Stage 2)
            resume_ip: 0,
            in_nondet_mode: false,
            fork_depth: 0,
            saved_stack: std::ptr::null_mut(),
            saved_stack_count: 0,
            saved_stack_cap: 0,
            // Binding/Environment support (Phase A)
            binding_frames: std::ptr::null_mut(),
            binding_frames_count: 0,
            binding_frames_cap: 0,
            // Registry/Cache support (Phase A - Full JIT)
            external_registry: std::ptr::null(),
            memo_cache: std::ptr::null(),
            space_registry: std::ptr::null_mut(),
            // Grounded Space support (Space Ops - Phase 1)
            grounded_spaces: std::ptr::null(),
            grounded_spaces_count: 0,
            template_results: std::ptr::null_mut(),
            template_results_cap: 0,
            // Cut scope support (Phase A - Full JIT)
            cut_markers: std::ptr::null_mut(),
            cut_marker_count: 0,
            cut_marker_cap: 0,
            // Heap tracking disabled by default
            heap_tracker: std::ptr::null_mut(),
            // State operations support (Phase D.1)
            env_ptr: std::ptr::null_mut(),
            // State cache (Optimization 5.1)
            state_cache: [(0, 0); STATE_CACHE_SIZE],
            state_cache_valid: 0,
            // Stack save pool (Optimization 5.2) - disabled without nondet
            stack_save_pool: std::ptr::null_mut(),
            stack_save_pool_cap: 0,
            stack_save_pool_next: 0,
            // Variable index cache (Optimization 5.3)
            // u32::MAX indicates empty slot
            var_index_cache: [(0, u32::MAX); VAR_INDEX_CACHE_SIZE],
        }
    }

    /// Create a JitContext with non-determinism support
    ///
    /// # Safety
    /// The caller must ensure:
    /// - `stack` has at least `stack_cap` elements allocated
    /// - `constants` points to valid memory for the lifetime of execution
    /// - `choice_points` has at least `choice_point_cap` elements allocated
    /// - `results` has at least `results_cap` elements allocated
    pub unsafe fn with_nondet(
        stack: *mut JitValue,
        stack_cap: usize,
        constants: *const MettaValue,
        constants_len: usize,
        choice_points: *mut JitChoicePoint,
        choice_point_cap: usize,
        results: *mut JitValue,
        results_cap: usize,
    ) -> Self {
        JitContext {
            value_stack: stack,
            sp: 0,
            stack_cap,
            constants,
            constants_len,
            bailout: false,
            bailout_ip: 0,
            bailout_reason: JitBailoutReason::None,
            choice_points,
            choice_point_count: 0,
            choice_point_cap,
            results,
            results_count: 0,
            results_cap,
            // Call/TailCall support
            bridge_ptr: std::ptr::null(),
            current_chunk: std::ptr::null(),
            // Rule dispatch support (Phase C)
            current_rules: std::ptr::null_mut(),
            current_rule_idx: 0,
            // Native nondeterminism support (Stage 2)
            resume_ip: 0,
            in_nondet_mode: false,
            fork_depth: 0,
            saved_stack: std::ptr::null_mut(),
            saved_stack_count: 0,
            saved_stack_cap: 0,
            // Binding/Environment support (Phase A)
            binding_frames: std::ptr::null_mut(),
            binding_frames_count: 0,
            binding_frames_cap: 0,
            // Registry/Cache support (Phase A - Full JIT)
            external_registry: std::ptr::null(),
            memo_cache: std::ptr::null(),
            space_registry: std::ptr::null_mut(),
            // Grounded Space support (Space Ops - Phase 1)
            grounded_spaces: std::ptr::null(),
            grounded_spaces_count: 0,
            template_results: std::ptr::null_mut(),
            template_results_cap: 0,
            // Cut scope support (Phase A - Full JIT)
            cut_markers: std::ptr::null_mut(),
            cut_marker_count: 0,
            cut_marker_cap: 0,
            // Heap tracking disabled by default
            heap_tracker: std::ptr::null_mut(),
            // State operations support (Phase D.1)
            env_ptr: std::ptr::null_mut(),
            // State cache (Optimization 5.1)
            state_cache: [(0, 0); STATE_CACHE_SIZE],
            state_cache_valid: 0,
            // Stack save pool (Optimization 5.2) - will be set by HybridExecutor
            stack_save_pool: std::ptr::null_mut(),
            stack_save_pool_cap: 0,
            stack_save_pool_next: 0,
            // Variable index cache (Optimization 5.3)
            // u32::MAX indicates empty slot
            var_index_cache: [(0, u32::MAX); VAR_INDEX_CACHE_SIZE],
        }
    }

    /// Check if non-determinism is enabled
    #[inline]
    pub fn has_nondet_support(&self) -> bool {
        !self.choice_points.is_null() && self.choice_point_cap > 0
    }

    /// Signal bailout to bytecode VM
    #[inline]
    pub fn signal_bailout(&mut self, ip: usize) {
        self.bailout = true;
        self.bailout_ip = ip;
    }

    /// Signal bailout with error reason
    #[inline]
    pub fn signal_error(&mut self, ip: usize, reason: JitBailoutReason) {
        self.bailout = true;
        self.bailout_ip = ip;
        self.bailout_reason = reason;
    }

    /// Check if bailout occurred
    #[inline]
    pub fn has_bailout(&self) -> bool {
        self.bailout
    }

    /// Reset bailout state
    #[inline]
    pub fn clear_bailout(&mut self) {
        self.bailout = false;
        self.bailout_ip = 0;
        self.bailout_reason = JitBailoutReason::None;
    }

    // -------------------------------------------------------------------------
    // State cache helpers (Optimization 5.1)
    // -------------------------------------------------------------------------

    /// Try to get a cached state value.
    /// Returns Some(value) if cache hit, None if cache miss.
    #[inline]
    pub fn state_cache_get(&self, state_id: u64) -> Option<JitValue> {
        let slot = (state_id & STATE_CACHE_MASK) as usize;
        let slot_mask = 1u8 << slot;

        // Check if slot is valid and contains the right state_id
        if (self.state_cache_valid & slot_mask) != 0 {
            let (cached_id, cached_value) = self.state_cache[slot];
            if cached_id == state_id {
                return Some(JitValue(cached_value));
            }
        }
        None
    }

    /// Update the state cache with a value.
    #[inline]
    pub fn state_cache_put(&mut self, state_id: u64, value: JitValue) {
        let slot = (state_id & STATE_CACHE_MASK) as usize;
        let slot_mask = 1u8 << slot;

        self.state_cache[slot] = (state_id, value.0);
        self.state_cache_valid |= slot_mask;
    }

    /// Invalidate a cached state value (called after change-state!).
    #[inline]
    pub fn state_cache_invalidate(&mut self, state_id: u64) {
        let slot = (state_id & STATE_CACHE_MASK) as usize;
        let slot_mask = 1u8 << slot;

        // Only invalidate if this slot actually contains this state_id
        if (self.state_cache_valid & slot_mask) != 0 {
            let (cached_id, _) = self.state_cache[slot];
            if cached_id == state_id {
                self.state_cache_valid &= !slot_mask;
            }
        }
    }

    /// Clear entire state cache (e.g., on context reset)
    #[inline]
    pub fn state_cache_clear(&mut self) {
        self.state_cache_valid = 0;
    }

    // -------------------------------------------------------------------------
    // Stack save pool helpers (Optimization 5.2)
    // -------------------------------------------------------------------------

    /// Check if stack save pool is available
    #[inline]
    pub fn has_stack_save_pool(&self) -> bool {
        !self.stack_save_pool.is_null() && self.stack_save_pool_cap > 0
    }

    /// Allocate a slot in the stack save pool.
    ///
    /// Returns the slot index if successful, or -1 if the pool is not available
    /// or the stack is too large to save.
    ///
    /// # Safety
    /// Caller must ensure the pool is valid and stack_count <= MAX_STACK_SAVE_VALUES.
    #[inline]
    pub unsafe fn stack_save_pool_alloc(&mut self, stack_count: usize) -> isize {
        if !self.has_stack_save_pool() {
            return -1;
        }

        // Check if stack fits in a pool slot
        if stack_count > MAX_STACK_SAVE_VALUES {
            return -1;
        }

        // Allocate next slot (ring buffer)
        let slot_idx = self.stack_save_pool_next;
        self.stack_save_pool_next = (slot_idx + 1) % STACK_SAVE_POOL_SIZE;

        slot_idx as isize
    }

    /// Get pointer to a stack save pool slot.
    ///
    /// # Safety
    /// Caller must ensure slot_idx is valid (0..STACK_SAVE_POOL_SIZE).
    #[inline]
    pub unsafe fn stack_save_pool_slot(&self, slot_idx: usize) -> *mut JitValue {
        debug_assert!(slot_idx < STACK_SAVE_POOL_SIZE);
        self.stack_save_pool.add(slot_idx * MAX_STACK_SAVE_VALUES)
    }

    /// Save stack values to a pool slot.
    ///
    /// # Safety
    /// Caller must ensure slot_idx is valid and stack_count <= MAX_STACK_SAVE_VALUES.
    #[inline]
    pub unsafe fn stack_save_to_pool(&mut self, slot_idx: usize, stack_count: usize) {
        if stack_count == 0 || self.value_stack.is_null() {
            return;
        }

        let dest = self.stack_save_pool_slot(slot_idx);
        std::ptr::copy_nonoverlapping(self.value_stack, dest, stack_count);
    }

    /// Restore stack values from a pool slot.
    ///
    /// # Safety
    /// Caller must ensure slot_idx is valid and stack_count <= MAX_STACK_SAVE_VALUES.
    #[inline]
    pub unsafe fn stack_restore_from_pool(&mut self, slot_idx: usize, stack_count: usize) {
        if stack_count == 0 || self.value_stack.is_null() {
            return;
        }

        let src = self.stack_save_pool_slot(slot_idx);
        std::ptr::copy_nonoverlapping(src, self.value_stack, stack_count);
    }

    // -------------------------------------------------------------------------
    // Native nondeterminism helpers (Stage 2 JIT)
    // -------------------------------------------------------------------------

    /// Enter nondeterministic mode
    #[inline]
    pub fn enter_nondet_mode(&mut self) {
        self.in_nondet_mode = true;
        self.fork_depth += 1;
    }

    /// Exit nondeterministic mode
    #[inline]
    pub fn exit_nondet_mode(&mut self) {
        if self.fork_depth > 0 {
            self.fork_depth -= 1;
        }
        if self.fork_depth == 0 {
            self.in_nondet_mode = false;
        }
    }

    /// Check if there are any active choice points
    #[inline]
    pub fn has_choice_points(&self) -> bool {
        self.choice_point_count > 0
    }

    /// Set resume IP for re-entry after backtracking
    #[inline]
    pub fn set_resume_ip(&mut self, ip: usize) {
        self.resume_ip = ip;
    }

    /// Clear resume IP
    #[inline]
    pub fn clear_resume_ip(&mut self) {
        self.resume_ip = 0;
    }

    // -------------------------------------------------------------------------
    // Call/TailCall bridge access (Stage 2 JIT)
    // -------------------------------------------------------------------------

    /// Set the bridge pointer for rule dispatch
    ///
    /// # Safety
    /// The pointer must point to a valid MorkBridge for the lifetime of JIT execution
    #[inline]
    pub fn set_bridge(&mut self, bridge: *const ()) {
        self.bridge_ptr = bridge;
    }

    /// Check if a bridge is available for rule dispatch
    #[inline]
    pub fn has_bridge(&self) -> bool {
        !self.bridge_ptr.is_null()
    }

    /// Set the current bytecode chunk
    ///
    /// # Safety
    /// The pointer must point to a valid BytecodeChunk for the lifetime of JIT execution
    #[inline]
    pub fn set_current_chunk(&mut self, chunk: *const ()) {
        self.current_chunk = chunk;
    }

    // -------------------------------------------------------------------------
    // Binding/Environment helpers (Phase A)
    // -------------------------------------------------------------------------

    /// Check if binding support is enabled
    #[inline]
    pub fn has_binding_support(&self) -> bool {
        !self.binding_frames.is_null() && self.binding_frames_cap > 0
    }

    /// Get the current number of binding frames
    #[inline]
    pub fn binding_frame_count(&self) -> usize {
        self.binding_frames_count
    }

    /// Set binding frames buffer for JIT execution
    ///
    /// # Safety
    /// The pointer must point to valid memory for `cap` JitBindingFrame entries
    #[inline]
    pub unsafe fn set_binding_frames(&mut self, frames: *mut JitBindingFrame, cap: usize) {
        self.binding_frames = frames;
        self.binding_frames_cap = cap;
        self.binding_frames_count = 0;
    }

    /// Initialize binding frames with a root frame
    ///
    /// # Safety
    /// The binding_frames pointer must be valid and have capacity >= 1
    #[inline]
    pub unsafe fn init_root_binding_frame(&mut self) {
        if !self.binding_frames.is_null() && self.binding_frames_cap > 0 {
            let root_frame = JitBindingFrame::new(0);
            *self.binding_frames = root_frame;
            self.binding_frames_count = 1;
        }
    }

    // -------------------------------------------------------------------------
    // Grounded Space helpers (Space Ops - Phase 1)
    // -------------------------------------------------------------------------

    /// Check if grounded spaces are set
    #[inline]
    pub fn has_grounded_spaces(&self) -> bool {
        !self.grounded_spaces.is_null() && self.grounded_spaces_count > 0
    }

    /// Set pre-resolved grounded spaces for JIT execution
    ///
    /// # Arguments
    /// - `spaces`: Array of space handle pointers [&self, &kb, &stack]
    /// - `count`: Number of spaces (typically 3)
    ///
    /// # Safety
    /// The pointer must point to valid memory for `count` space handle pointers
    /// that will outlive the JIT execution.
    #[inline]
    pub unsafe fn set_grounded_spaces(&mut self, spaces: *const *const (), count: usize) {
        self.grounded_spaces = spaces;
        self.grounded_spaces_count = count;
    }

    /// Get a pre-resolved grounded space by index
    ///
    /// # Arguments
    /// - `index`: 0 = &self, 1 = &kb, 2 = &stack
    ///
    /// # Safety
    /// The index must be within bounds and grounded_spaces must be valid
    #[inline]
    pub unsafe fn get_grounded_space(&self, index: usize) -> *const () {
        debug_assert!(
            index < self.grounded_spaces_count,
            "Grounded space index out of bounds"
        );
        *self.grounded_spaces.add(index)
    }

    /// Set template results buffer for space match instantiation
    ///
    /// # Safety
    /// The pointer must point to valid memory for `cap` JitValue entries
    #[inline]
    pub unsafe fn set_template_results(&mut self, results: *mut JitValue, cap: usize) {
        self.template_results = results;
        self.template_results_cap = cap;
    }

    /// Check if template results buffer is available
    #[inline]
    pub fn has_template_results(&self) -> bool {
        !self.template_results.is_null() && self.template_results_cap > 0
    }

    // -------------------------------------------------------------------------
    // Heap Tracking Methods
    // -------------------------------------------------------------------------

    /// Enable heap tracking for this context.
    ///
    /// When enabled, heap allocations made during JIT execution will be tracked
    /// and can be freed by calling `cleanup_heap_allocations`.
    ///
    /// # Safety
    /// The tracker pointer must point to a valid, owned Vec that will outlive
    /// the JIT execution.
    #[inline]
    pub unsafe fn enable_heap_tracking(&mut self, tracker: *mut Vec<*mut MettaValue>) {
        self.heap_tracker = tracker;
    }

    /// Check if heap tracking is enabled
    #[inline]
    pub fn has_heap_tracking(&self) -> bool {
        !self.heap_tracker.is_null()
    }

    /// Track a heap allocation for later cleanup.
    ///
    /// # Safety
    /// - The pointer must be from a valid Box<MettaValue> allocation
    /// - Heap tracking must be enabled via `enable_heap_tracking`
    #[inline]
    pub unsafe fn track_heap_allocation(&mut self, ptr: *mut MettaValue) {
        if !self.heap_tracker.is_null() {
            (*self.heap_tracker).push(ptr);
        }
    }

    /// Free all tracked heap allocations.
    ///
    /// This should be called when JIT execution is complete to prevent memory leaks.
    ///
    /// # Safety
    /// - All tracked pointers must still be valid
    /// - This method should only be called once per execution
    #[inline]
    pub unsafe fn cleanup_heap_allocations(&mut self) {
        if !self.heap_tracker.is_null() {
            let tracker = &mut *self.heap_tracker;
            for ptr in tracker.drain(..) {
                if !ptr.is_null() {
                    // Reconstruct the Box and drop it
                    let _ = Box::from_raw(ptr);
                }
            }
        }
    }

    /// Get the number of tracked heap allocations
    #[inline]
    pub unsafe fn heap_allocation_count(&self) -> usize {
        if self.heap_tracker.is_null() {
            0
        } else {
            (*self.heap_tracker).len()
        }
    }

    // -------------------------------------------------------------------------
    // State Operations Support (Phase D.1)
    // -------------------------------------------------------------------------

    /// Set the environment pointer for state operations.
    ///
    /// Required for new-state, get-state, and change-state! operations.
    ///
    /// # Safety
    /// The pointer must point to a valid Environment that will outlive
    /// the JIT execution.
    #[inline]
    pub unsafe fn set_env(&mut self, env: *mut ()) {
        self.env_ptr = env;
    }

    /// Check if environment is available for state operations
    #[inline]
    pub fn has_env(&self) -> bool {
        !self.env_ptr.is_null()
    }
}

impl fmt::Debug for JitContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JitContext")
            .field("sp", &self.sp)
            .field("stack_cap", &self.stack_cap)
            .field("constants_len", &self.constants_len)
            .field("bailout", &self.bailout)
            .field("bailout_ip", &self.bailout_ip)
            .field("choice_point_count", &self.choice_point_count)
            .field("choice_point_cap", &self.choice_point_cap)
            .field("results_count", &self.results_count)
            .field("results_cap", &self.results_cap)
            .field("resume_ip", &self.resume_ip)
            .field("in_nondet_mode", &self.in_nondet_mode)
            .field("fork_depth", &self.fork_depth)
            .field("binding_frames_count", &self.binding_frames_count)
            .field("binding_frames_cap", &self.binding_frames_cap)
            .field("grounded_spaces_count", &self.grounded_spaces_count)
            .field("template_results_cap", &self.template_results_cap)
            .finish()
    }
}
