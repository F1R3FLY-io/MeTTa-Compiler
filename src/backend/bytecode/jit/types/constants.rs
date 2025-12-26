//! NaN-boxing and JIT signal constants.
//!
//! This module defines all constants used for NaN-boxed value representation
//! and JIT runtime signals.

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
pub(super) const QNAN: u64 = 0x7FF8_0000_0000_0000;

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
pub(super) const SIGN_BIT_48: u64 = 0x0000_8000_0000_0000;
pub(super) const SIGN_EXTEND_MASK: u64 = 0xFFFF_0000_0000_0000;

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
