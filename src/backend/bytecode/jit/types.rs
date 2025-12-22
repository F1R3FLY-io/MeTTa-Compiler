//! JIT Type Definitions
//!
//! This module defines the core types used by the Cranelift JIT compiler:
//! - [`JitValue`]: NaN-boxed 64-bit value representation
//! - [`JitContext`]: Runtime context passed to compiled code
//! - [`JitResult`] and [`JitError`]: Result types for JIT operations

use std::fmt;

use crate::backend::models::MettaValue;

// =============================================================================
// NaN-Boxing Constants
// =============================================================================
//
// IEEE 754 double-precision NaN has the form:
//   Sign(1) | Exponent(11) | Mantissa(52)
//   where Exponent = 0x7FF and Mantissa != 0 for NaN
//
// We use quiet NaN (bit 51 set) with tag bits in bits 48-50:
//   0x7FF8_xxxx_xxxx_xxxx = quiet NaN base
//
// Layout: [0x7FF (11 bits)][Quiet bit (1)][Tag (3 bits)][Payload (48 bits)]
//
// This gives us 8 possible tags (0-7) and 48-bit payloads.
// 48 bits is enough for:
//   - 48-bit integers (most common range)
//   - 48-bit pointers (x86-64 canonical addresses use 48 bits)

/// Quiet NaN base - all values with this prefix are NaN-boxed
const QNAN: u64 = 0x7FF8_0000_0000_0000;

/// Tag for 48-bit signed integers (most i64 values fit)
pub const TAG_LONG: u64 = QNAN | (0 << 48); // 0x7FF8_0000_0000_0000

/// Tag for boolean values (payload: 0 = false, 1 = true)
pub const TAG_BOOL: u64 = QNAN | (1 << 48); // 0x7FF9_0000_0000_0000

/// Tag for nil/unit value (payload ignored)
pub const TAG_NIL: u64 = QNAN | (2 << 48); // 0x7FFA_0000_0000_0000

/// Tag for unit value () - distinct from nil
pub const TAG_UNIT: u64 = QNAN | (3 << 48); // 0x7FFB_0000_0000_0000

/// Tag for heap pointers to MettaValue (48-bit pointer)
pub const TAG_HEAP: u64 = QNAN | (4 << 48); // 0x7FFC_0000_0000_0000

/// Tag for error values (pointer to error MettaValue)
pub const TAG_ERROR: u64 = QNAN | (5 << 48); // 0x7FFD_0000_0000_0000

/// Tag for atoms/symbols (pointer to interned string)
pub const TAG_ATOM: u64 = QNAN | (6 << 48); // 0x7FFE_0000_0000_0000

/// Tag for variables (pointer to variable name)
pub const TAG_VAR: u64 = QNAN | (7 << 48); // 0x7FFF_0000_0000_0000

/// Mask to extract the tag (upper 16 bits)
pub const TAG_MASK: u64 = 0xFFFF_0000_0000_0000;

/// Mask to extract the payload (lower 48 bits)
pub const PAYLOAD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;

/// Sign extension mask for 48-bit to 64-bit integer conversion
const SIGN_BIT_48: u64 = 0x0000_8000_0000_0000;
const SIGN_EXTEND_MASK: u64 = 0xFFFF_0000_0000_0000;

// =============================================================================
// JIT Signal Constants - For Native Nondeterminism (Stage 2)
// =============================================================================
//
// JIT-compiled functions return these signals to the dispatcher loop,
// which handles backtracking without returning to the bytecode VM.
//
// The dispatcher pattern:
// 1. JIT code returns a signal
// 2. Dispatcher checks signal:
//    - OK: If choice points exist, backtrack; else done
//    - YIELD: Result saved, backtrack to try next alternative
//    - FAIL: Backtrack immediately

/// Normal completion - execution finished successfully
pub const JIT_SIGNAL_OK: i64 = 0;

/// Result saved, backtrack for more alternatives
/// The JIT has saved a result and wants to try the next alternative
pub const JIT_SIGNAL_YIELD: i64 = 2;

/// Backtrack immediately - current path failed
/// No result was produced, try the next alternative
pub const JIT_SIGNAL_FAIL: i64 = 3;

/// Error occurred - stop execution
pub const JIT_SIGNAL_ERROR: i64 = -1;

/// Halt execution - explicit stop requested by Halt opcode
pub const JIT_SIGNAL_HALT: i64 = -2;

/// Bailout to VM - JIT cannot handle this operation
pub const JIT_SIGNAL_BAILOUT: i64 = -3;

// =============================================================================
// State Cache Constants (Optimization 5.1)
// =============================================================================

/// Size of the state cache (power of 2 for fast modulo via bitmask)
/// Learned from Optimization 3.3: HashMap adds too much overhead for small N
pub const STATE_CACHE_SIZE: usize = 8;

/// Bitmask for cache slot calculation (STATE_CACHE_SIZE - 1)
pub const STATE_CACHE_MASK: u64 = (STATE_CACHE_SIZE - 1) as u64;

// =============================================================================
// Choice Point Pre-allocation Constants (Optimization 5.2)
// =============================================================================

/// Maximum number of alternatives that can be embedded inline in a JitChoicePoint.
/// Fork operations with more alternatives than this will fall back to VM handling.
/// 32 alternatives × 32 bytes = 1KB per choice point.
pub const MAX_ALTERNATIVES_INLINE: usize = 32;

/// Maximum number of stacks that can be saved in the stack save pool.
/// This is a ring buffer - older saves are overwritten when full.
/// Must be >= max fork depth to avoid corruption during backtracking.
pub const STACK_SAVE_POOL_SIZE: usize = 64;

/// Maximum stack values per saved stack snapshot.
/// Forks with larger stacks will fall back to VM handling.
pub const MAX_STACK_SAVE_VALUES: usize = 256;

/// Size of the variable name index cache in JitContext.
/// Direct-mapped cache: slot = name_hash % VAR_INDEX_CACHE_SIZE
/// 32 slots × 12 bytes = 384 bytes overhead per JitContext.
pub const VAR_INDEX_CACHE_SIZE: usize = 32;

// =============================================================================
// JitBindingEntry - Individual Binding Entry
// =============================================================================

/// A single binding entry for pattern variables.
///
/// This is `#[repr(C)]` for FFI compatibility with JIT-generated code.
#[repr(C)]
#[derive(Debug, Clone)]
pub struct JitBindingEntry {
    /// Index into the constant pool for the variable name
    pub name_idx: u32,
    /// The bound value (NaN-boxed)
    pub value: JitValue,
}

impl JitBindingEntry {
    /// Create a new binding entry
    #[inline]
    pub fn new(name_idx: u32, value: JitValue) -> Self {
        Self { name_idx, value }
    }
}

// =============================================================================
// JitBindingFrame - Stack of Bindings for a Scope
// =============================================================================

/// A frame of bindings for pattern variables at a particular scope level.
///
/// This is `#[repr(C)]` for FFI compatibility with JIT-generated code.
/// Mirrors the VM's BindingFrame structure.
#[repr(C)]
#[derive(Debug, Clone)]
pub struct JitBindingFrame {
    /// Pointer to array of binding entries
    pub entries: *mut JitBindingEntry,
    /// Number of entries in this frame
    pub entries_count: usize,
    /// Capacity of the entries array
    pub entries_cap: usize,
    /// Scope depth (0 = root frame)
    pub scope_depth: u32,
}

impl Default for JitBindingFrame {
    fn default() -> Self {
        Self {
            entries: std::ptr::null_mut(),
            entries_count: 0,
            entries_cap: 0,
            scope_depth: 0,
        }
    }
}

impl JitBindingFrame {
    /// Create a new empty binding frame
    pub fn new(scope_depth: u32) -> Self {
        Self {
            entries: std::ptr::null_mut(),
            entries_count: 0,
            entries_cap: 0,
            scope_depth,
        }
    }

    /// Create a binding frame with pre-allocated capacity
    ///
    /// # Safety
    /// The caller must ensure the allocated memory is valid for the frame's lifetime
    pub unsafe fn with_capacity(scope_depth: u32, capacity: usize) -> Self {
        if capacity == 0 {
            return Self::new(scope_depth);
        }
        let layout = std::alloc::Layout::array::<JitBindingEntry>(capacity)
            .expect("Layout calculation failed");
        let entries = std::alloc::alloc(layout) as *mut JitBindingEntry;
        Self {
            entries,
            entries_count: 0,
            entries_cap: capacity,
            scope_depth,
        }
    }
}

// =============================================================================
// JitBailoutReason - Error codes for bailout
// =============================================================================

/// Reason for JIT code bailing out to the bytecode VM.
///
/// This is `#[repr(u8)]` for efficient storage and FFI compatibility.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JitBailoutReason {
    /// No bailout occurred
    None = 0,
    /// Type error (expected different type)
    TypeError = 1,
    /// Division by zero
    DivisionByZero = 2,
    /// Stack overflow
    StackOverflow = 3,
    /// Stack underflow
    StackUnderflow = 4,
    /// Invalid opcode encountered
    InvalidOpcode = 5,
    /// Unsupported operation (requires bytecode VM)
    UnsupportedOperation = 6,
    /// Integer overflow
    IntegerOverflow = 7,
    /// Non-determinism (Fork/Choice opcodes require VM)
    NonDeterminism = 8,
    /// Call operation needs VM for rule dispatch
    Call = 9,
    /// TailCall operation needs VM for rule dispatch
    TailCall = 10,
    /// Fork operation needs VM for choice point management
    Fork = 11,
    /// Yield operation needs VM for backtracking
    Yield = 12,
    /// Collect operation needs VM to gather results
    Collect = 13,
    /// Invalid binding (variable not found in any scope)
    InvalidBinding = 14,
    /// Binding frame stack overflow
    BindingFrameOverflow = 15,
    /// Higher-order operation (map, filter, fold) needs VM
    HigherOrderOp = 16,
}

// =============================================================================
// JIT Choice Point Types for Non-Determinism
// =============================================================================

/// Type tag for JitAlternative
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JitAlternativeTag {
    /// Alternative is a NaN-boxed value to push
    Value = 0,
    /// Alternative is a pointer to a BytecodeChunk to execute
    Chunk = 1,
    /// Alternative is a rule match (chunk + bindings pointer)
    RuleMatch = 2,
    /// Alternative is a space match result (template + bindings + saved frames)
    /// - payload: NaN-boxed template expression pointer
    /// - payload2: pointer to bindings array (JitBindingEntry*)
    /// - payload3: pointer to saved binding frames for restoration
    SpaceMatch = 3,
}

/// An alternative in a JIT choice point.
///
/// This is `#[repr(C)]` for FFI compatibility with JIT-generated code.
/// Each alternative represents one branch in a non-deterministic choice.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct JitAlternative {
    /// Type tag indicating what kind of alternative this is
    pub tag: JitAlternativeTag,
    /// Primary payload - interpretation depends on tag:
    /// - Value: NaN-boxed JitValue bits
    /// - Chunk: pointer to BytecodeChunk
    /// - RuleMatch: pointer to BytecodeChunk
    /// - SpaceMatch: NaN-boxed template expression pointer
    pub payload: u64,
    /// Secondary payload:
    /// - RuleMatch: pointer to Bindings
    /// - SpaceMatch: pointer to bindings array (JitBindingEntry*)
    pub payload2: u64,
    /// Tertiary payload (only used for SpaceMatch):
    /// - SpaceMatch: pointer to saved binding frames for restoration
    pub payload3: u64,
}

impl JitAlternative {
    /// Create a value alternative
    #[inline]
    pub fn value(val: JitValue) -> Self {
        Self {
            tag: JitAlternativeTag::Value,
            payload: val.to_bits(),
            payload2: 0,
            payload3: 0,
        }
    }

    /// Create a chunk alternative
    #[inline]
    pub fn chunk(chunk_ptr: *const ()) -> Self {
        Self {
            tag: JitAlternativeTag::Chunk,
            payload: chunk_ptr as u64,
            payload2: 0,
            payload3: 0,
        }
    }

    /// Create a rule match alternative
    #[inline]
    pub fn rule_match(chunk_ptr: *const (), bindings_ptr: *const ()) -> Self {
        Self {
            tag: JitAlternativeTag::RuleMatch,
            payload: chunk_ptr as u64,
            payload2: bindings_ptr as u64,
            payload3: 0,
        }
    }

    /// Create a space match alternative
    ///
    /// # Arguments
    /// - `template_ptr`: Pointer to the template expression to instantiate
    /// - `bindings_ptr`: Pointer to bindings array (JitBindingEntry*)
    /// - `saved_frames_ptr`: Pointer to saved binding frames for restoration
    #[inline]
    pub fn space_match(
        template_ptr: *const (),
        bindings_ptr: *const JitBindingEntry,
        saved_frames_ptr: *const JitBindingFrame,
    ) -> Self {
        Self {
            tag: JitAlternativeTag::SpaceMatch,
            payload: template_ptr as u64,
            payload2: bindings_ptr as u64,
            payload3: saved_frames_ptr as u64,
        }
    }
}

/// A JIT choice point for native non-determinism.
///
/// This is `#[repr(C)]` for FFI compatibility with JIT-generated code.
/// Choice points are created by Fork opcodes and consumed by Fail/Yield.
///
/// # Optimization 5.2: Pre-allocation
///
/// Alternatives are embedded inline to avoid per-fork allocation.
/// Saved stack uses a pool index instead of leaked Box allocation.
/// This eliminates memory leaks and reduces allocation overhead.
#[repr(C)]
#[derive(Debug, Clone)]
pub struct JitChoicePoint {
    /// Saved stack pointer (for restoring on backtrack)
    pub saved_sp: u64,
    /// Number of alternatives in this choice point (max MAX_ALTERNATIVES_INLINE)
    pub alt_count: u64,
    /// Current alternative index (0..alt_count)
    pub current_index: u64,
    /// Saved instruction pointer for continuation
    pub saved_ip: u64,
    /// Pointer to saved chunk (for chunk switching)
    pub saved_chunk: *const (),
    /// Number of saved stack values (max MAX_STACK_SAVE_VALUES)
    pub saved_stack_count: usize,
    /// Fork depth at creation (for nested nondeterminism)
    pub fork_depth: usize,
    // Phase 1.4: Fields for enhanced backtracking
    /// Saved binding frames count for nested scope restoration
    pub saved_binding_frames_count: usize,
    /// Whether this choice point is a Collect boundary
    /// When true, backtracking from this point should collect results
    pub is_collect_boundary: bool,

    // Optimization 5.2: Embedded alternatives (eliminates Box::leak allocation)
    /// Inline array of alternatives (avoids heap allocation per Fork)
    pub alternatives_inline: [JitAlternative; MAX_ALTERNATIVES_INLINE],

    // Optimization 5.2: Pool-based stack save (eliminates Box::leak allocation)
    /// Index into JitContext.stack_save_pool (-1 = no saved stack)
    /// Using isize to allow -1 sentinel for "no saved stack"
    pub saved_stack_pool_idx: isize,
}

impl Default for JitChoicePoint {
    fn default() -> Self {
        Self {
            saved_sp: 0,
            alt_count: 0,
            current_index: 0,
            saved_ip: 0,
            saved_chunk: std::ptr::null(),
            saved_stack_count: 0,
            fork_depth: 0,
            saved_binding_frames_count: 0,
            is_collect_boundary: false,
            // Initialize all alternatives to empty value alternatives
            alternatives_inline: [JitAlternative::value(JitValue::nil()); MAX_ALTERNATIVES_INLINE],
            saved_stack_pool_idx: -1, // No saved stack
        }
    }
}

// =============================================================================
// JitClosure - Lambda Closure Representation
// =============================================================================

/// A closure for lambda expressions in JIT-compiled code.
///
/// This is `#[repr(C)]` for FFI compatibility with JIT-generated code.
/// Closures capture their environment at creation time.
#[repr(C)]
#[derive(Debug)]
pub struct JitClosure {
    /// Number of parameters expected
    pub param_count: u32,
    /// Pointer to parameter name indices (into constant pool)
    pub param_names: *const u32,
    /// Pointer to the bytecode body chunk
    pub body_chunk: *const (),
    /// Captured binding frames - copy of bindings at closure creation
    pub captured_frames: *mut JitBindingFrame,
    /// Number of captured frames
    pub captured_frame_count: usize,
}

impl Default for JitClosure {
    fn default() -> Self {
        Self {
            param_count: 0,
            param_names: std::ptr::null(),
            body_chunk: std::ptr::null(),
            captured_frames: std::ptr::null_mut(),
            captured_frame_count: 0,
        }
    }
}

impl JitClosure {
    /// Create a new closure with no captured environment
    #[inline]
    pub fn new(param_count: u32, body_chunk: *const ()) -> Self {
        Self {
            param_count,
            param_names: std::ptr::null(),
            body_chunk,
            captured_frames: std::ptr::null_mut(),
            captured_frame_count: 0,
        }
    }

    /// Create a closure with captured binding frames
    ///
    /// # Safety
    /// The caller must ensure `captured_frames` points to valid JitBindingFrame data
    /// that will outlive this closure.
    #[inline]
    pub unsafe fn with_captured_env(
        param_count: u32,
        body_chunk: *const (),
        captured_frames: *mut JitBindingFrame,
        captured_frame_count: usize,
    ) -> Self {
        Self {
            param_count,
            param_names: std::ptr::null(),
            body_chunk,
            captured_frames,
            captured_frame_count,
        }
    }
}

// =============================================================================
// JitValue - NaN-Boxed Value
// =============================================================================

/// A NaN-boxed 64-bit value for efficient JIT code generation.
///
/// This representation allows type checking with simple bit operations:
/// - Check if Long: `(v & TAG_MASK) == TAG_LONG`
/// - Check if Bool: `(v & TAG_MASK) == TAG_BOOL`
/// - Extract payload: `v & PAYLOAD_MASK`
///
/// # Performance
///
/// NaN-boxing provides several advantages for JIT code:
/// 1. Single 64-bit register holds both type and value
/// 2. Type checks are cheap bitwise AND + compare
/// 3. No pointer indirection for primitives
/// 4. Compatible with Cranelift's i64 type
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct JitValue(pub u64);

impl JitValue {
    // -------------------------------------------------------------------------
    // Constructors
    // -------------------------------------------------------------------------

    /// Create a JitValue from a raw 64-bit representation
    #[inline(always)]
    pub const fn from_raw(bits: u64) -> Self {
        JitValue(bits)
    }

    /// Create a Long (integer) value
    ///
    /// Note: Only 48-bit signed integers are supported directly.
    /// Larger values should use heap allocation.
    #[inline(always)]
    pub const fn from_long(n: i64) -> Self {
        // Truncate to 48 bits (preserving sign in the truncated representation)
        let payload = (n as u64) & PAYLOAD_MASK;
        JitValue(TAG_LONG | payload)
    }

    /// Create a boolean value
    #[inline(always)]
    pub const fn from_bool(b: bool) -> Self {
        JitValue(TAG_BOOL | (b as u64))
    }

    /// Create nil value
    #[inline(always)]
    pub const fn nil() -> Self {
        JitValue(TAG_NIL)
    }

    /// Create unit value
    #[inline(always)]
    pub const fn unit() -> Self {
        JitValue(TAG_UNIT)
    }

    /// Create a heap pointer to a MettaValue
    ///
    /// # Safety
    /// The pointer must be valid for the lifetime of the JIT execution
    #[inline(always)]
    pub fn from_heap_ptr(ptr: *const MettaValue) -> Self {
        let addr = ptr as u64;
        debug_assert!(
            addr & TAG_MASK == 0,
            "Pointer uses more than 48 bits: {:#x}",
            addr
        );
        JitValue(TAG_HEAP | (addr & PAYLOAD_MASK))
    }

    /// Create an error value
    #[inline(always)]
    pub fn from_error_ptr(ptr: *const MettaValue) -> Self {
        let addr = ptr as u64;
        debug_assert!(
            addr & TAG_MASK == 0,
            "Pointer uses more than 48 bits: {:#x}",
            addr
        );
        JitValue(TAG_ERROR | (addr & PAYLOAD_MASK))
    }

    /// Create an atom/symbol value from a String pointer
    #[inline(always)]
    pub fn from_atom_ptr(ptr: *const String) -> Self {
        let addr = ptr as u64;
        debug_assert!(
            addr & TAG_MASK == 0,
            "Pointer uses more than 48 bits: {:#x}",
            addr
        );
        JitValue(TAG_ATOM | (addr & PAYLOAD_MASK))
    }

    /// Create a variable value from a String pointer
    #[inline(always)]
    pub fn from_var_ptr(ptr: *const String) -> Self {
        let addr = ptr as u64;
        debug_assert!(
            addr & TAG_MASK == 0,
            "Pointer uses more than 48 bits: {:#x}",
            addr
        );
        JitValue(TAG_VAR | (addr & PAYLOAD_MASK))
    }

    // -------------------------------------------------------------------------
    // Type Predicates
    // -------------------------------------------------------------------------

    /// Get the tag bits
    #[inline(always)]
    pub const fn tag(self) -> u64 {
        self.0 & TAG_MASK
    }

    /// Check if this is a Long (integer)
    #[inline(always)]
    pub const fn is_long(self) -> bool {
        self.tag() == TAG_LONG
    }

    /// Check if this is a Bool
    #[inline(always)]
    pub const fn is_bool(self) -> bool {
        self.tag() == TAG_BOOL
    }

    /// Check if this is nil
    #[inline(always)]
    pub const fn is_nil(self) -> bool {
        self.tag() == TAG_NIL
    }

    /// Check if this is unit
    #[inline(always)]
    pub const fn is_unit(self) -> bool {
        self.tag() == TAG_UNIT
    }

    /// Check if this is a heap pointer
    #[inline(always)]
    pub const fn is_heap(self) -> bool {
        self.tag() == TAG_HEAP
    }

    /// Check if this is an error
    #[inline(always)]
    pub const fn is_error(self) -> bool {
        self.tag() == TAG_ERROR
    }

    /// Check if this is an atom/symbol
    #[inline(always)]
    pub const fn is_atom(self) -> bool {
        self.tag() == TAG_ATOM
    }

    /// Check if this is a variable
    #[inline(always)]
    pub const fn is_var(self) -> bool {
        self.tag() == TAG_VAR
    }

    // -------------------------------------------------------------------------
    // Value Extraction
    // -------------------------------------------------------------------------

    /// Extract as Long (sign-extended from 48 bits)
    ///
    /// # Panics
    /// Panics in debug mode if the value is not a Long
    #[inline(always)]
    pub const fn as_long(self) -> i64 {
        debug_assert!(self.is_long(), "JitValue is not a Long");
        let payload = self.0 & PAYLOAD_MASK;
        // Sign-extend from 48 bits to 64 bits
        if payload & SIGN_BIT_48 != 0 {
            (payload | SIGN_EXTEND_MASK) as i64
        } else {
            payload as i64
        }
    }

    /// Extract as Long without sign extension (raw 48-bit value)
    #[inline(always)]
    pub const fn as_long_raw(self) -> u64 {
        self.0 & PAYLOAD_MASK
    }

    /// Extract as Bool
    ///
    /// # Panics
    /// Panics in debug mode if the value is not a Bool
    #[inline(always)]
    pub const fn as_bool(self) -> bool {
        debug_assert!(self.is_bool(), "JitValue is not a Bool");
        (self.0 & 1) != 0
    }

    /// Extract as heap pointer
    ///
    /// # Safety
    /// The caller must ensure the pointer is still valid
    #[inline(always)]
    pub fn as_heap_ptr(self) -> *const MettaValue {
        debug_assert!(self.is_heap(), "JitValue is not a heap pointer");
        (self.0 & PAYLOAD_MASK) as *const MettaValue
    }

    /// Extract as error pointer
    #[inline(always)]
    pub fn as_error_ptr(self) -> *const MettaValue {
        debug_assert!(self.is_error(), "JitValue is not an error");
        (self.0 & PAYLOAD_MASK) as *const MettaValue
    }

    /// Extract as atom pointer
    #[inline(always)]
    pub fn as_atom_ptr(self) -> *const String {
        debug_assert!(self.is_atom(), "JitValue is not an atom");
        (self.0 & PAYLOAD_MASK) as *const String
    }

    /// Extract as variable pointer
    #[inline(always)]
    pub fn as_var_ptr(self) -> *const String {
        debug_assert!(self.is_var(), "JitValue is not a variable");
        (self.0 & PAYLOAD_MASK) as *const String
    }

    /// Get the raw bits
    #[inline(always)]
    pub const fn to_bits(self) -> u64 {
        self.0
    }

    // -------------------------------------------------------------------------
    // Conversion
    // -------------------------------------------------------------------------

    /// Try to convert from MettaValue to JitValue
    ///
    /// Returns None for values that cannot be NaN-boxed (e.g., large integers)
    pub fn try_from_metta(value: &MettaValue) -> Option<Self> {
        match value {
            MettaValue::Long(n) => {
                // Check if fits in 48 bits (signed)
                let min_48 = -(1i64 << 47);
                let max_48 = (1i64 << 47) - 1;
                if *n >= min_48 && *n <= max_48 {
                    Some(JitValue::from_long(*n))
                } else {
                    // Large integer - needs heap allocation
                    None
                }
            }
            MettaValue::Bool(b) => Some(JitValue::from_bool(*b)),
            MettaValue::Nil => Some(JitValue::nil()),
            MettaValue::Unit => Some(JitValue::unit()),
            // Other types need heap allocation
            _ => None,
        }
    }

    /// Convert JitValue back to MettaValue
    ///
    /// # Safety
    /// For heap pointers, the referenced MettaValue must be valid
    pub unsafe fn to_metta(self) -> MettaValue {
        match self.tag() {
            TAG_LONG => MettaValue::Long(self.as_long()),
            TAG_BOOL => MettaValue::Bool(self.as_bool()),
            TAG_NIL => MettaValue::Nil,
            TAG_UNIT => MettaValue::Unit,
            TAG_HEAP => (*self.as_heap_ptr()).clone(),
            TAG_ERROR => (*self.as_error_ptr()).clone(),
            TAG_ATOM => {
                let s = &*self.as_atom_ptr();
                MettaValue::Atom(s.clone())
            }
            TAG_VAR => {
                // Variables in MeTTa are atoms that start with $
                let s = &*self.as_var_ptr();
                MettaValue::Atom(s.clone())
            }
            _ => unreachable!("Invalid JitValue tag"),
        }
    }
}

impl fmt::Debug for JitValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.tag() {
            TAG_LONG => write!(f, "JitValue::Long({})", self.as_long()),
            TAG_BOOL => write!(f, "JitValue::Bool({})", self.as_bool()),
            TAG_NIL => write!(f, "JitValue::Nil"),
            TAG_UNIT => write!(f, "JitValue::Unit"),
            TAG_HEAP => write!(f, "JitValue::Heap({:p})", self.as_heap_ptr()),
            TAG_ERROR => write!(f, "JitValue::Error({:p})", self.as_error_ptr()),
            TAG_ATOM => write!(f, "JitValue::Atom({:p})", self.as_atom_ptr()),
            TAG_VAR => write!(f, "JitValue::Var({:p})", self.as_var_ptr()),
            _ => write!(f, "JitValue::Unknown({:#x})", self.0),
        }
    }
}

impl Default for JitValue {
    fn default() -> Self {
        JitValue::nil()
    }
}

// Pre-defined constants for common values
impl JitValue {
    /// Constant for boolean true
    pub const TRUE: JitValue = JitValue::from_bool(true);

    /// Constant for boolean false
    pub const FALSE: JitValue = JitValue::from_bool(false);

    /// Constant for nil
    pub const NIL: JitValue = JitValue::nil();

    /// Constant for unit
    pub const UNIT: JitValue = JitValue::unit();

    /// Constant for zero
    pub const ZERO: JitValue = JitValue::from_long(0);

    /// Constant for one
    pub const ONE: JitValue = JitValue::from_long(1);
}

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
    pub unsafe fn set_binding_frames(
        &mut self,
        frames: *mut JitBindingFrame,
        cap: usize,
    ) {
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
        debug_assert!(index < self.grounded_spaces_count, "Grounded space index out of bounds");
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

// =============================================================================
// JitResult and JitError
// =============================================================================

/// Error types for JIT compilation and execution
#[derive(Debug, Clone)]
pub enum JitError {
    /// Bytecode chunk cannot be JIT compiled
    NotCompilable(String),

    /// Cranelift compilation error
    CompilationError(String),

    /// Type error during JIT execution
    TypeError { expected: &'static str, got: u64 },

    /// Stack overflow
    StackOverflow,

    /// Stack underflow
    StackUnderflow,

    /// Division by zero
    DivisionByZero,

    /// Invalid opcode encountered
    InvalidOpcode(u8),

    /// Bailout to bytecode VM required
    Bailout { ip: usize, reason: String },

    /// Invalid local variable index
    InvalidLocalIndex(usize),

    /// Invalid binding (variable not found)
    InvalidBinding(String),

    /// Binding frame overflow
    BindingFrameOverflow,
}

impl fmt::Display for JitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            JitError::NotCompilable(msg) => write!(f, "Not compilable: {}", msg),
            JitError::CompilationError(msg) => write!(f, "Compilation error: {}", msg),
            JitError::TypeError { expected, got } => {
                write!(f, "Type error: expected {}, got tag {:#x}", expected, got)
            }
            JitError::StackOverflow => write!(f, "Stack overflow"),
            JitError::StackUnderflow => write!(f, "Stack underflow"),
            JitError::DivisionByZero => write!(f, "Division by zero"),
            JitError::InvalidOpcode(op) => write!(f, "Invalid opcode: {:#x}", op),
            JitError::Bailout { ip, reason } => {
                write!(f, "Bailout at ip {}: {}", ip, reason)
            }
            JitError::InvalidLocalIndex(idx) => write!(f, "Invalid local index: {}", idx),
            JitError::InvalidBinding(name) => write!(f, "Invalid binding: {}", name),
            JitError::BindingFrameOverflow => write!(f, "Binding frame stack overflow"),
        }
    }
}

impl std::error::Error for JitError {}

/// Result type for JIT operations
pub type JitResult<T> = Result<T, JitError>;

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nan_boxing_long() {
        // Test positive integers
        let v = JitValue::from_long(42);
        assert!(v.is_long());
        assert!(!v.is_bool());
        assert_eq!(v.as_long(), 42);

        // Test zero
        let v = JitValue::from_long(0);
        assert!(v.is_long());
        assert_eq!(v.as_long(), 0);

        // Test negative integers
        let v = JitValue::from_long(-123);
        assert!(v.is_long());
        assert_eq!(v.as_long(), -123);

        // Test max 48-bit positive
        let max_48 = (1i64 << 47) - 1;
        let v = JitValue::from_long(max_48);
        assert!(v.is_long());
        assert_eq!(v.as_long(), max_48);

        // Test min 48-bit negative
        let min_48 = -(1i64 << 47);
        let v = JitValue::from_long(min_48);
        assert!(v.is_long());
        assert_eq!(v.as_long(), min_48);
    }

    #[test]
    fn test_nan_boxing_bool() {
        let t = JitValue::from_bool(true);
        assert!(t.is_bool());
        assert!(!t.is_long());
        assert!(t.as_bool());

        let f = JitValue::from_bool(false);
        assert!(f.is_bool());
        assert!(!f.as_bool());
    }

    #[test]
    fn test_nan_boxing_nil_unit() {
        let nil = JitValue::nil();
        assert!(nil.is_nil());
        assert!(!nil.is_unit());

        let unit = JitValue::unit();
        assert!(unit.is_unit());
        assert!(!unit.is_nil());
    }

    #[test]
    fn test_nan_boxing_constants() {
        assert!(JitValue::TRUE.is_bool());
        assert!(JitValue::TRUE.as_bool());

        assert!(JitValue::FALSE.is_bool());
        assert!(!JitValue::FALSE.as_bool());

        assert!(JitValue::NIL.is_nil());
        assert!(JitValue::UNIT.is_unit());

        assert!(JitValue::ZERO.is_long());
        assert_eq!(JitValue::ZERO.as_long(), 0);

        assert!(JitValue::ONE.is_long());
        assert_eq!(JitValue::ONE.as_long(), 1);
    }

    #[test]
    fn test_try_from_metta() {
        // Long
        let v = JitValue::try_from_metta(&MettaValue::Long(42));
        assert!(v.is_some());
        assert_eq!(v.unwrap().as_long(), 42);

        // Bool
        let v = JitValue::try_from_metta(&MettaValue::Bool(true));
        assert!(v.is_some());
        assert!(v.unwrap().as_bool());

        // Nil
        let v = JitValue::try_from_metta(&MettaValue::Nil);
        assert!(v.is_some());
        assert!(v.unwrap().is_nil());

        // Unit
        let v = JitValue::try_from_metta(&MettaValue::Unit);
        assert!(v.is_some());
        assert!(v.unwrap().is_unit());
    }

    #[test]
    fn test_to_metta_roundtrip() {
        // Long
        let orig = MettaValue::Long(42);
        let jit = JitValue::try_from_metta(&orig).unwrap();
        let back = unsafe { jit.to_metta() };
        assert_eq!(back, orig);

        // Bool
        let orig = MettaValue::Bool(true);
        let jit = JitValue::try_from_metta(&orig).unwrap();
        let back = unsafe { jit.to_metta() };
        assert_eq!(back, orig);

        // Nil
        let orig = MettaValue::Nil;
        let jit = JitValue::try_from_metta(&orig).unwrap();
        let back = unsafe { jit.to_metta() };
        assert_eq!(back, orig);
    }

    #[test]
    fn test_tag_extraction() {
        assert_eq!(JitValue::from_long(42).tag(), TAG_LONG);
        assert_eq!(JitValue::from_bool(true).tag(), TAG_BOOL);
        assert_eq!(JitValue::nil().tag(), TAG_NIL);
        assert_eq!(JitValue::unit().tag(), TAG_UNIT);
    }
}
