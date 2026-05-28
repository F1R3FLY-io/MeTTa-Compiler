//! Z.A.2 regression: `JitValue::from_long` must NOT silently truncate
//! values outside the inline 48-bit signed range. Out-of-range values
//! are now routed to a slab-allocated `MettaValueInner::Long(n)` and
//! tagged as TAG_PTR.
//!
//! Anchor: `src/backend/bytecode/jit/types/value.rs::{from_long,
//! try_from_long_inline, from_long_inline_unchecked,
//! INLINE_LONG_MAX, INLINE_LONG_MIN}`.

use mettatron::backend::bytecode::jit::types::JitValue;
use mettatron::backend::models::ValueView;

#[test]
fn inline_range_constants_match_2_pow_47() {
    assert_eq!(JitValue::INLINE_LONG_MAX, (1i64 << 47) - 1);
    assert_eq!(JitValue::INLINE_LONG_MIN, -(1i64 << 47));
}

#[test]
fn try_inline_accepts_max_48() {
    let v = JitValue::try_from_long_inline(JitValue::INLINE_LONG_MAX);
    assert!(v.is_some());
}

#[test]
fn try_inline_accepts_min_48() {
    let v = JitValue::try_from_long_inline(JitValue::INLINE_LONG_MIN);
    assert!(v.is_some());
}

#[test]
fn try_inline_rejects_just_above_max() {
    let v = JitValue::try_from_long_inline(JitValue::INLINE_LONG_MAX + 1);
    assert!(v.is_none());
}

#[test]
fn try_inline_rejects_just_below_min() {
    let v = JitValue::try_from_long_inline(JitValue::INLINE_LONG_MIN - 1);
    assert!(v.is_none());
}

#[test]
fn try_inline_rejects_i64_max() {
    let v = JitValue::try_from_long_inline(i64::MAX);
    assert!(v.is_none());
}

#[test]
fn try_inline_rejects_i64_min() {
    let v = JitValue::try_from_long_inline(i64::MIN);
    assert!(v.is_none());
}

#[test]
fn from_long_round_trips_inline() {
    for n in [0i64, 1, -1, 42, -42, 1 << 30, -(1 << 30)] {
        let v = JitValue::from_long(n);
        let metta = unsafe { v.to_metta() };
        let actual = match metta.view() {
            ValueView::Long(x) => x,
            other => panic!("expected ValueView::Long, got {:?}", other),
        };
        assert_eq!(actual, n, "inline round-trip failed for {}", n);
    }
}

// JIT T2/T3 FFI is VM-fallback-gated under index mode (Inc 2b); JIT-direct test runs in the slab build only.
#[cfg(not(feature = "index-gc"))]
#[test]
fn from_long_round_trips_heap_path() {
    // Values just past 2^47 force the heap path. The MettaValue::Long
    // round-trip must preserve the FULL 64-bit value.
    let big_positives = [
        JitValue::INLINE_LONG_MAX + 1,
        (1i64 << 50),
        (1i64 << 62),
        i64::MAX,
    ];
    let big_negatives = [
        JitValue::INLINE_LONG_MIN - 1,
        -(1i64 << 50),
        -(1i64 << 62),
        i64::MIN,
    ];
    for n in big_positives.iter().chain(big_negatives.iter()).copied() {
        let v = JitValue::from_long(n);
        let metta = unsafe { v.to_metta() };
        let actual = match metta.view() {
            ValueView::Long(x) => x,
            other => panic!("expected ValueView::Long, got {:?} for {}", other, n),
        };
        assert_eq!(actual, n, "heap round-trip failed for {}", n);
    }
}

#[test]
fn from_long_inline_unchecked_is_const_friendly() {
    // Used by JitValue::ZERO / ONE. Only valid for compile-time-known
    // values guaranteed to fit in 48 bits.
    const ZERO: JitValue = JitValue::from_long_inline_unchecked(0);
    const ONE: JitValue = JitValue::from_long_inline_unchecked(1);
    assert_eq!(ZERO, JitValue::ZERO);
    assert_eq!(ONE, JitValue::ONE);
}
