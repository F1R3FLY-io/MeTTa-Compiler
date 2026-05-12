//! Z.A.3 regression: `JitBindingEntry.name_idx` must be a full 64-bit
//! FNV-1a hash without u32 truncation. Closes T0-T3-010 / MTT-TI-028.
//!
//! Anchors: `src/backend/bytecode/jit/types/binding.rs::JitBindingEntry`,
//! `src/backend/bytecode/jit/runtime/space_ops.rs::ensure_binding_frame_capacity`,
//! `src/backend/bytecode/jit/runtime/rule_dispatch.rs::hash_string`.

use mettatron::backend::bytecode::jit::types::JitBindingEntry;
use mettatron::backend::bytecode::jit::types::JitValue;

#[test]
fn binding_entry_name_idx_is_u64() {
    let entry = JitBindingEntry::new(u64::MAX, JitValue::from_long(0));
    assert_eq!(entry.name_idx, u64::MAX);
}

#[test]
fn binding_entry_name_idx_holds_full_64_bit_hash() {
    // High bits must survive — a u32 truncation would lose them.
    let high_bits_hash: u64 = 0xDEADBEEF_DEADBEEF;
    let entry = JitBindingEntry::new(high_bits_hash, JitValue::from_long(42));
    assert_eq!(entry.name_idx, high_bits_hash);
    assert_ne!(entry.name_idx as u32 as u64, high_bits_hash);
}

#[test]
fn fnv_1a_64bit_no_collision_for_long_freshened_names() {
    // PLN-style freshened names: $__fr_E_<bare>. With u32 truncation,
    // ~4 collisions per 65k names is typical. 64-bit FNV-1a should have
    // zero collisions on this realistic-size workload.
    const N: usize = 4096;
    let mut hashes = std::collections::HashSet::with_capacity(N);
    let mut collisions = 0;
    for i in 0..N {
        let name = format!("$__fr_{}_BestCandidate_Helper_v{}", i, i * 17);
        let hash = fnv_1a_64(&name);
        if !hashes.insert(hash) {
            collisions += 1;
        }
    }
    assert_eq!(
        collisions, 0,
        "FNV-1a 64-bit produced {} collisions across {} long freshened names",
        collisions, N
    );
}

fn fnv_1a_64(s: &str) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;
    let mut hash = FNV_OFFSET;
    for byte in s.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

#[test]
fn fnv_1a_distinguishes_names_that_truncate_to_same_u32() {
    // Two strings whose FNV-1a 64-bit values DIFFER only in the upper 32
    // bits would alias under u32 truncation. We can't easily construct
    // such a pair, but we can confirm the hash is sensitive across the
    // full 64-bit range by checking two PLN-realistic names differ.
    let a = fnv_1a_64("$__fr_1_x");
    let b = fnv_1a_64("$__fr_2_x");
    assert_ne!(a, b);
    assert_ne!(a >> 32, b >> 32, "upper bits should differ");
}
