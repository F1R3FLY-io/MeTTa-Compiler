//! NaN-boxed value type for JIT execution.
//!
//! This module defines [`JitValue`], the core NaN-boxed 64-bit value type
//! used for efficient JIT code generation.

use std::fmt;

use super::constants::{
    PAYLOAD_MASK, SIGN_BIT_48, SIGN_EXTEND_MASK, TAG_ATOM, TAG_BOOL, TAG_ERROR, TAG_HEAP, TAG_LONG,
    TAG_MASK, TAG_NIL, TAG_UNIT, TAG_VAR,
};
use crate::backend::models::MettaValue;

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
