//! NaN-boxed value type for JIT execution.
//!
//! This module defines [`JitValue`], the core NaN-boxed 64-bit value type
//! used for efficient JIT code generation.

use std::fmt;

use super::constants::{
    PAYLOAD_MASK, SIGN_BIT_48, SIGN_EXTEND_MASK, TAG_ATOM, TAG_BOOL, TAG_EMPTY, TAG_ERROR,
    TAG_LONG, TAG_MASK, TAG_PTR, TAG_UNIT, TAG_VAR,
};
use crate::backend::models::{MettaValue, MettaValueInner, ValueView};

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

    /// Create a Long (integer) value.
    ///
    /// Z.A.2 (2026-05-12): for values outside the inline 48-bit signed
    /// range `[INLINE_LONG_MIN, INLINE_LONG_MAX]`, the value is allocated
    /// in the active compiled store and returned as TAG_PTR. Eliminates silent truncation
    /// (former code did `(n as u64) & PAYLOAD_MASK` without bounds check,
    /// which corrupted Long arithmetic past 2^47).
    ///
    /// For `const` contexts (e.g. [`JitValue::ZERO`], [`JitValue::ONE`]),
    /// use [`Self::from_long_inline_unchecked`] which preserves the
    /// const-fn property but is only safe for values guaranteed in range
    /// at compile time.
    #[inline]
    pub fn from_long(n: i64) -> Self {
        if let Some(inline) = Self::try_from_long_inline(n) {
            return inline;
        }
        // Index fence: mint a genuine index heap Long via the factory and pack
        // its `inner_ptr()` form (`INDEX_KEY_TAG | tagged >> 4`). This is the
        // only TAG_PTR payload shape index-mode unpack trusts.
        #[cfg(feature = "index-gc")]
        {
            use crate::backend::models::MettaValueFactory;
            let v = crate::backend::models::global_factory().long(n);
            return JitValue::from_inner_ptr(v.inner_ptr());
        }
    }

    /// Maximum signed value representable as an inline 48-bit Long.
    pub const INLINE_LONG_MAX: i64 = (1i64 << 47) - 1;

    /// Minimum signed value representable as an inline 48-bit Long.
    pub const INLINE_LONG_MIN: i64 = -(1i64 << 47);

    /// Try to encode `n` inline as a 48-bit signed Long.
    ///
    /// Returns `None` if `|n|` exceeds `2^47 - 1` (positive) or `n < -2^47`
    /// (negative). Use [`Self::from_long`] for the heap-fallback path.
    #[inline]
    pub const fn try_from_long_inline(n: i64) -> Option<Self> {
        if n >= Self::INLINE_LONG_MIN && n <= Self::INLINE_LONG_MAX {
            let payload = (n as u64) & PAYLOAD_MASK;
            Some(JitValue(TAG_LONG | payload))
        } else {
            None
        }
    }

    /// `const fn` variant of [`Self::from_long`] that does NOT range-check.
    ///
    /// **Only use for compile-time constants known to fit in 48 bits.**
    /// At runtime, prefer [`Self::from_long`] which routes overflow to the
    /// heap path. Z.A.2 retains this for [`Self::ZERO`] / [`Self::ONE`].
    #[inline(always)]
    pub const fn from_long_inline_unchecked(n: i64) -> Self {
        let payload = (n as u64) & PAYLOAD_MASK;
        JitValue(TAG_LONG | payload)
    }

    /// Create a boolean value
    #[inline(always)]
    pub const fn from_bool(b: bool) -> Self {
        JitValue(TAG_BOOL | (b as u64))
    }

    /// Create unit value
    #[inline(always)]
    pub const fn unit() -> Self {
        JitValue(TAG_UNIT)
    }

    /// Create empty (zero-result) value
    #[inline(always)]
    pub const fn empty() -> Self {
        JitValue(TAG_EMPTY)
    }

    /// Create a TAG_PTR value from an active-store inner payload.
    ///
    /// In legacy slab builds the payload is a valid `MettaValueInner` pointer.
    /// In index-gc builds the payload is the arena `Addr` bits returned by
    /// `inner_ptr`.
    #[inline(always)]
    pub fn from_inner_ptr(ptr: *const MettaValueInner) -> Self {
        let addr = ptr as u64;
        // In index mode the "ptr" is `inner_ptr()` = INDEX_KEY_TAG(bit 48) | addr.raw();
        // `& PAYLOAD_MASK` drops bit 48 → exactly `addr.raw()`, so the computation is
        // already correct — only the slab-pointer width assert must be relaxed (Inc 2b).
        debug_assert!(
            addr & TAG_MASK == 0 || crate::backend::models::metta_value::gc_mode_is_index(),
            "Pointer uses more than 48 bits: {:#x}",
            addr
        );
        JitValue(TAG_PTR | (addr & PAYLOAD_MASK))
    }

    /// Create an error value from an active-store inner payload.
    #[inline(always)]
    pub fn from_error_ptr(ptr: *const MettaValueInner) -> Self {
        let addr = ptr as u64;
        debug_assert!(
            addr & TAG_MASK == 0 || crate::backend::models::metta_value::gc_mode_is_index(),
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

    /// Check if this value has a valid NaN-boxed tag.
    ///
    /// Valid tags are in the range 0x7FF8..=0x7FFF (quiet NaN with tag bits 0-7).
    /// This is useful for detecting corrupted or uninitialized values.
    #[inline(always)]
    pub const fn is_valid_tag(self) -> bool {
        let tag = self.tag();
        // Valid tags are TAG_LONG through TAG_VAR (0x7FF8_xxxx through 0x7FFF_xxxx)
        tag == TAG_LONG
            || tag == TAG_BOOL
            || tag == TAG_EMPTY
            || tag == TAG_UNIT
            || tag == TAG_PTR
            || tag == TAG_ERROR
            || tag == TAG_ATOM
            || tag == TAG_VAR
    }

    /// Validate that this value has a valid tag, panicking with debug info if not.
    ///
    /// This is a debug helper to catch corrupted values early.
    #[inline]
    pub fn assert_valid(&self) {
        debug_assert!(
            self.is_valid_tag(),
            "Invalid JitValue: raw={:#018x}, tag={:#06x} (expected 0x7FF8..0x7FFF)",
            self.0,
            (self.0 >> 48) as u16
        );
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

    /// Check if this is unit
    #[inline(always)]
    pub const fn is_unit(self) -> bool {
        self.tag() == TAG_UNIT
    }

    /// Check if this is a heap pointer
    #[inline(always)]
    pub const fn is_heap(self) -> bool {
        self.tag() == TAG_PTR
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

    /// Extract the active-store inner payload.
    ///
    /// # Safety
    /// The caller must decode the payload according to the active store.
    #[inline(always)]
    pub fn as_inner_ptr(self) -> *const MettaValueInner {
        debug_assert!(self.is_heap(), "JitValue is not a TAG_PTR value");
        (self.0 & PAYLOAD_MASK) as *const MettaValueInner
    }

    /// Extract the active-store error payload.
    #[inline(always)]
    pub fn as_error_ptr(self) -> *const MettaValueInner {
        debug_assert!(self.is_error(), "JitValue is not an error");
        (self.0 & PAYLOAD_MASK) as *const MettaValueInner
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
        match value.view() {
            ValueView::Long(n) => {
                // Check if fits in 48 bits (signed)
                let min_48 = -(1i64 << 47);
                let max_48 = (1i64 << 47) - 1;
                if n >= min_48 && n <= max_48 {
                    Some(JitValue::from_long(n))
                } else {
                    // Large integer - needs heap allocation
                    None
                }
            }
            ValueView::Bool(b) => Some(JitValue::from_bool(b)),
            ValueView::Unit => Some(JitValue::unit()),
            ValueView::Empty => Some(JitValue::empty()),
            // Other types need heap allocation
            _ => None,
        }
    }

    /// Convert JitValue back to MettaValue.
    ///
    /// The TAG_PTR/TAG_ERROR store policy is verified by
    /// `formal/rocq/gc/JitPayloadConversionStorePolicy.v`.
    ///
    /// # Safety
    /// For heap payloads, the encoded value must belong to the active store.
    pub unsafe fn to_metta(self) -> MettaValue {
        // Validate tag before any operations
        debug_assert!(
            self.is_valid_tag(),
            "to_metta: Invalid JitValue tag: raw={:#018x}, tag={:#06x}",
            self.0,
            (self.0 >> 48) as u16
        );

        match self.tag() {
            TAG_LONG => MettaValue::Long(self.as_long()),
            TAG_BOOL => MettaValue::Bool(self.as_bool()),
            TAG_UNIT => MettaValue::Unit(),
            TAG_EMPTY => MettaValue::Empty(),
            TAG_PTR => {
                let ptr = self.as_inner_ptr();
                // Index mode (Inc 2b): `ptr` carries the bare arena `Addr` bits, not
                // a slab pointer — reconstruct the handle (do NOT deref / assert
                // slab-pointer invariants on it). Slab arm below is byte-identical.
                if crate::backend::models::metta_value::gc_mode_is_index() {
                    let addr = crate::backend::eval::cesk::index_arena::Addr::from_raw(ptr as u32);
                    // exp18: TAG5 rides the inner_ptr pack at payload [36:32].
                    let tag = ((self.0 >> 32) & 0x1F) as u8;
                    debug_assert!(tag <= 18, "non-inner_ptr-packed TAG_PTR payload leak");
                    MettaValue::from_addr(
                        addr,
                        crate::backend::models::metta_value::FLAG_HAS_VARIABLES,
                        tag,
                    )
                } else {
                    debug_assert!(
                        !ptr.is_null(),
                        "to_metta: Null inner pointer in JitValue: raw={:#018x}",
                        self.0
                    );
                    debug_assert!(
                        (ptr as usize) % std::mem::align_of::<MettaValueInner>() == 0,
                        "to_metta: Misaligned inner pointer: {:p} (raw={:#018x})",
                        ptr,
                        self.0
                    );
                    MettaValue::from_inner(&*ptr)
                }
            }
            TAG_ERROR => {
                let ptr = (self.0 & PAYLOAD_MASK) as *const MettaValueInner;
                if crate::backend::models::metta_value::gc_mode_is_index() {
                    let addr = crate::backend::eval::cesk::index_arena::Addr::from_raw(ptr as u32);
                    // exp18: TAG5 recovery (see TAG_PTR arm above).
                    let tag = ((self.0 >> 32) & 0x1F) as u8;
                    debug_assert!(tag <= 18, "non-inner_ptr-packed TAG_ERROR payload leak");
                    MettaValue::from_addr(
                        addr,
                        crate::backend::models::metta_value::FLAG_HAS_VARIABLES,
                        tag,
                    )
                } else {
                    debug_assert!(
                        !ptr.is_null(),
                        "to_metta: Null error pointer in JitValue: raw={:#018x}",
                        self.0
                    );
                    debug_assert!(
                        (ptr as usize) % std::mem::align_of::<MettaValueInner>() == 0,
                        "to_metta: Misaligned error pointer: {:p} (raw={:#018x})",
                        ptr,
                        self.0
                    );
                    MettaValue::from_inner(&*ptr)
                }
            }
            TAG_ATOM => {
                let ptr = self.as_atom_ptr();
                debug_assert!(
                    !ptr.is_null(),
                    "to_metta: Null atom pointer in JitValue: raw={:#018x}",
                    self.0
                );
                debug_assert!(
                    (ptr as usize) % std::mem::align_of::<String>() == 0,
                    "to_metta: Misaligned atom pointer: {:p} (raw={:#018x})",
                    ptr,
                    self.0
                );
                let s = &*ptr;
                MettaValue::Atom(s.clone())
            }
            TAG_VAR => {
                let ptr = self.as_var_ptr();
                debug_assert!(
                    !ptr.is_null(),
                    "to_metta: Null var pointer in JitValue: raw={:#018x}",
                    self.0
                );
                debug_assert!(
                    (ptr as usize) % std::mem::align_of::<String>() == 0,
                    "to_metta: Misaligned var pointer: {:p} (raw={:#018x})",
                    ptr,
                    self.0
                );
                // Variables in MeTTa are atoms that start with $
                let s = &*ptr;
                MettaValue::Atom(s.clone())
            }
            _ => {
                // In release builds, return an error value instead of panicking
                #[cfg(debug_assertions)]
                unreachable!(
                    "Invalid JitValue tag: raw={:#018x}, tag={:#06x}",
                    self.0,
                    (self.0 >> 48) as u16
                );
                #[cfg(not(debug_assertions))]
                MettaValue::Error(
                    MettaValue::String(format!("{:#018x}", self.0)),
                    MettaValue::String("JIT: Invalid JitValue tag"),
                )
            }
        }
    }
}

impl fmt::Debug for JitValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.tag() {
            TAG_LONG => write!(f, "JitValue::Long({})", self.as_long()),
            TAG_BOOL => write!(f, "JitValue::Bool({})", self.as_bool()),
            TAG_EMPTY => write!(f, "JitValue::Empty"),
            TAG_UNIT => write!(f, "JitValue::Unit"),
            TAG_PTR => write!(f, "JitValue::Ptr({:p})", self.as_inner_ptr()),
            TAG_ERROR => write!(f, "JitValue::Error({:p})", self.as_error_ptr()),
            TAG_ATOM => write!(f, "JitValue::Atom({:p})", self.as_atom_ptr()),
            TAG_VAR => write!(f, "JitValue::Var({:p})", self.as_var_ptr()),
            _ => write!(f, "JitValue::Unknown({:#x})", self.0),
        }
    }
}

impl Default for JitValue {
    fn default() -> Self {
        JitValue::unit()
    }
}

// Pre-defined constants for common values
impl JitValue {
    /// Constant for boolean true
    pub const TRUE: JitValue = JitValue::from_bool(true);

    /// Constant for boolean false
    pub const FALSE: JitValue = JitValue::from_bool(false);

    /// Constant for unit
    pub const UNIT: JitValue = JitValue::unit();

    /// Constant for zero
    pub const ZERO: JitValue = JitValue::from_long_inline_unchecked(0);

    /// Constant for one
    pub const ONE: JitValue = JitValue::from_long_inline_unchecked(1);
}

#[cfg(all(test, feature = "index-gc"))]
mod index_fence_tests {
    use super::*;
    use crate::backend::eval::cesk::index_heap::enter_index_mode_for_test;

    /// Experiment #18 fence (design v4.1 revision 4): an out-of-inline-range
    /// Long must round-trip through the JIT in index mode. PRE-fence,
    /// `from_long` slab-allocated the overflow value even in index mode, and
    /// every index unpack misread the TAG_PTR payload's low 32 bits as an
    /// arena `Addr` — a garbage handle (the pre-existing latent bug this
    /// fence fixes). This is the index-build coverage the slab-only
    /// `tests/jit_long_range.rs` cfg gap left open.
    #[test]
    fn index_mode_big_long_round_trips_through_jit() {
        let _mode = enter_index_mode_for_test();
        let big = JitValue::INLINE_LONG_MAX + 12_345;
        let v = unsafe { JitValue::from_long(big).to_metta() };
        assert_eq!(
            v.as_long(),
            Some(big),
            "out-of-range Long must survive the index JIT pack/unpack"
        );
        let neg = JitValue::INLINE_LONG_MIN - 99;
        let v2 = unsafe { JitValue::from_long(neg).to_metta() };
        assert_eq!(v2.as_long(), Some(neg), "negative overflow Long round-trip");
    }
}
