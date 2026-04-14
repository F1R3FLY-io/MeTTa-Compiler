//! Per-environment override bitset for grounded helpers that user rules may shadow.
//!
//! ## Background
//!
//! MeTTaTron grounds many list/tuple helpers (`append`, `length`, `is-member`,
//! `map-atom`, `filter-atom`, `car-atom`, …) for performance. MeTTa HE either
//! defines these as MeTTa-level rules in `stdlib.metta` or has no definition at
//! all. To stay HE-compatible, MeTTaTron must let user rules take precedence
//! over its grounded fast path for these names — but only for names HE itself
//! considers overridable.
//!
//! True HE primitives (`cons-atom`, `decons-atom`, `size-atom`, `+`, `if`, …)
//! and side-effecting MeTTaTron internals (state, memo, modules, I/O, tests)
//! remain non-overridable: they are NOT in `OverridableOpId` and never go
//! through this gate.
//!
//! ## Design
//!
//! A fixed-size bitset, one bit per overridable name. The bit is set when at
//! least one user rule exists with that head; clear otherwise. Reads at
//! dispatch time use a single relaxed `AtomicU32::load` + bit test (~1 ns).
//! Writes from `add_rule` / `remove_rule` use a per-name refcount so the
//! bit can be cleared exactly when the last user rule is removed.
//!
//! ## Lockstep invariant
//!
//! `OverridableOpId`, `NUM_OVERRIDABLE_OPS`, and `overridable_op_id` MUST stay
//! in lockstep. Adding a name requires updating all three. The dispatch arm
//! in `src/backend/eval/step/sexpr.rs` must also list the name in its
//! overridable match arm.

use std::sync::atomic::{AtomicU16, AtomicU32, Ordering};

/// Compile-time IDs for every overridable operator name.
///
/// The set is the "Class B + selected Class C" partition documented in
/// `/Users/dylon/.claude/plans/twinkling-discovering-scott.md`. Class A
/// (TRUE-PRIMITIVE-IN-HE) names and side-effecting Class C ops are NOT in
/// this enum — they are non-overridable and skip the bitset entirely.
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OverridableOpId {
    // Class B (METTA-RULE in HE)
    CarAtom       = 0,
    CdrAtom       = 1,
    MapAtom       = 2,
    FilterAtom    = 3,
    FoldlAtom     = 4,
    // Class C (NOT-IN-HE, pure helpers)
    TupleConcat   = 5,
    TupleCount    = 6,
    Without       = 7,
    ElementOf     = 8,
    Range         = 9,
    ReverseAtom   = 10,
    FlattenAtom   = 11,
    ZipAtom       = 12,
    TakeAtom      = 13,
    DropAtom      = 14,
    IsMember      = 15,
    Append        = 16,
    Length        = 17,
    ExcludeItem   = 18,
    Msort         = 19,
    SortTuple     = 20,
    BestCandidate = 21,
    Cut           = 22,
}

/// Number of overridable operator names. Must equal the highest
/// `OverridableOpId` discriminant + 1.
pub const NUM_OVERRIDABLE_OPS: usize = 23;

const _: () = {
    assert!(
        NUM_OVERRIDABLE_OPS <= 32,
        "OverridableOpId set exceeds AtomicU32 bitset capacity. Promote to AtomicU64."
    );
};

/// Per-environment override bitset.
///
/// One bit per overridable operator name (indexed by `OverridableOpId`),
/// plus a per-name refcount so removal can clear the bit only when the last
/// user rule for that name disappears.
///
/// Cache-aligned (`#[repr(align(64))]`) to avoid false sharing with other
/// hot per-environment fields.
///
/// ## Memory ordering
///
/// All reads use `Ordering::Relaxed`. The worst case from observing a stale
/// bit is one call taking the wrong dispatch path — both paths are
/// semantically valid, only the speed differs. Writes also use `Relaxed`
/// because the count and the bit are updated together within a single
/// `add_rule` / `remove_rule` invocation that already holds higher-level
/// rule-index synchronization.
#[repr(align(64))]
pub struct DispatchOverrides {
    /// Bit `i` is set iff at least one user rule exists with head matching
    /// `OverridableOpId` value `i`. Read at dispatch time, written by
    /// add/remove hooks.
    overridden: AtomicU32,

    /// Refcount of user rules per `OverridableOpId`. `counts[i] > 0`
    /// implies bit `i` is set in `overridden`. `AtomicU16` is plenty —
    /// 65535 user rules per name is well beyond any realistic program.
    counts: [AtomicU16; NUM_OVERRIDABLE_OPS],
}

impl Default for DispatchOverrides {
    fn default() -> Self {
        // Cannot derive Default for arrays of atomics; build manually.
        // `AtomicU16::new(0)` is const, so this is cheap.
        const ZERO: AtomicU16 = AtomicU16::new(0);
        Self {
            overridden: AtomicU32::new(0),
            counts: [ZERO; NUM_OVERRIDABLE_OPS],
        }
    }
}

impl DispatchOverrides {
    /// Returns `true` iff at least one user rule exists for the given op.
    ///
    /// Single relaxed atomic load + bit test. Inlined into the dispatch
    /// site for ~1 ns per call.
    #[inline(always)]
    pub fn is_overridden(&self, id: OverridableOpId) -> bool {
        let bits = self.overridden.load(Ordering::Relaxed);
        (bits >> (id as u8)) & 1 != 0
    }

    /// Called from `add_rule` after a rule with head `id` is inserted.
    ///
    /// Sets the override bit when transitioning from 0 → 1 user rules.
    #[inline]
    pub fn note_user_rule_added(&self, id: OverridableOpId) {
        let prev = self.counts[id as usize].fetch_add(1, Ordering::Relaxed);
        if prev == 0 {
            self.overridden
                .fetch_or(1u32 << (id as u8), Ordering::Relaxed);
        }
    }

    /// Called from `remove_rule` after a rule with head `id` is removed.
    ///
    /// Clears the override bit when transitioning from 1 → 0 user rules.
    #[inline]
    pub fn note_user_rule_removed(&self, id: OverridableOpId) {
        let prev = self.counts[id as usize].fetch_sub(1, Ordering::Relaxed);
        debug_assert!(
            prev > 0,
            "note_user_rule_removed called more times than note_user_rule_added for id {:?}",
            id
        );
        if prev == 1 {
            self.overridden
                .fetch_and(!(1u32 << (id as u8)), Ordering::Relaxed);
        }
    }

    /// Build an independent copy with the same bits and counts.
    ///
    /// Used by `make_owned` and `union` to give each cloned environment its
    /// own override state. Read order is intentional: we capture `counts`
    /// first so any concurrent `note_user_rule_added` between count and
    /// bitset reads only over-reports (we always end up with `bit set`
    /// when `count > 0`, even if the writer hasn't updated the bit yet).
    pub fn snapshot(&self) -> Self {
        let new = Self::default();
        for i in 0..NUM_OVERRIDABLE_OPS {
            let c = self.counts[i].load(Ordering::Relaxed);
            new.counts[i].store(c, Ordering::Relaxed);
        }
        let bits = self.overridden.load(Ordering::Relaxed);
        new.overridden.store(bits, Ordering::Relaxed);
        new
    }

    /// Reset all bits and counts to zero in place.
    ///
    /// Used by paths that rebuild the rule index from scratch (e.g.
    /// `rebuild_bloom_filter` after deserialization). The caller is
    /// responsible for re-issuing `note_user_rule_added` for every rule
    /// re-inserted into the index.
    pub fn reset(&self) {
        self.overridden.store(0, Ordering::Relaxed);
        for slot in self.counts.iter() {
            slot.store(0, Ordering::Relaxed);
        }
    }
}

/// Map an operator name to its `OverridableOpId`, or `None` if the name is
/// not in the overridable set (and therefore takes the built-in / special-form
/// path unconditionally).
///
/// Compiles to a perfect-hash / jump table — single comparison + branch.
#[inline(always)]
pub fn overridable_op_id(op: &str) -> Option<OverridableOpId> {
    use OverridableOpId::*;
    Some(match op {
        // Class B (METTA-RULE in HE)
        "car-atom"       => CarAtom,
        "cdr-atom"       => CdrAtom,
        "map-atom"       => MapAtom,
        "filter-atom"    => FilterAtom,
        "foldl-atom"     => FoldlAtom,
        // Class C (NOT-IN-HE, pure helpers)
        "tuple-concat"   => TupleConcat,
        "tuple-count"    => TupleCount,
        "without"        => Without,
        "element-of"     => ElementOf,
        "range"          => Range,
        "reverse-atom"   => ReverseAtom,
        "flatten-atom"   => FlattenAtom,
        "zip-atom"       => ZipAtom,
        "take-atom"      => TakeAtom,
        "drop-atom"      => DropAtom,
        "is-member"      => IsMember,
        "append"         => Append,
        "length"         => Length,
        "exclude-item"   => ExcludeItem,
        "msort"          => Msort,
        "sort-tuple"     => SortTuple,
        "best-candidate" => BestCandidate,
        "cut"            => Cut,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_no_overrides() {
        let d = DispatchOverrides::default();
        for i in 0..NUM_OVERRIDABLE_OPS as u8 {
            // SAFETY: i < NUM_OVERRIDABLE_OPS, all variants are valid.
            let id: OverridableOpId = unsafe { std::mem::transmute(i) };
            assert!(!d.is_overridden(id));
        }
    }

    #[test]
    fn add_then_remove_clears_bit() {
        let d = DispatchOverrides::default();
        d.note_user_rule_added(OverridableOpId::Append);
        assert!(d.is_overridden(OverridableOpId::Append));
        assert!(!d.is_overridden(OverridableOpId::Length));

        d.note_user_rule_removed(OverridableOpId::Append);
        assert!(!d.is_overridden(OverridableOpId::Append));
    }

    #[test]
    fn refcount_keeps_bit_set() {
        let d = DispatchOverrides::default();
        d.note_user_rule_added(OverridableOpId::IsMember);
        d.note_user_rule_added(OverridableOpId::IsMember);
        d.note_user_rule_added(OverridableOpId::IsMember);
        assert!(d.is_overridden(OverridableOpId::IsMember));

        d.note_user_rule_removed(OverridableOpId::IsMember);
        assert!(d.is_overridden(OverridableOpId::IsMember));
        d.note_user_rule_removed(OverridableOpId::IsMember);
        assert!(d.is_overridden(OverridableOpId::IsMember));
        d.note_user_rule_removed(OverridableOpId::IsMember);
        assert!(!d.is_overridden(OverridableOpId::IsMember));
    }

    #[test]
    fn name_mapping_round_trip() {
        let names = [
            ("car-atom", OverridableOpId::CarAtom),
            ("cdr-atom", OverridableOpId::CdrAtom),
            ("map-atom", OverridableOpId::MapAtom),
            ("filter-atom", OverridableOpId::FilterAtom),
            ("foldl-atom", OverridableOpId::FoldlAtom),
            ("tuple-concat", OverridableOpId::TupleConcat),
            ("tuple-count", OverridableOpId::TupleCount),
            ("without", OverridableOpId::Without),
            ("element-of", OverridableOpId::ElementOf),
            ("range", OverridableOpId::Range),
            ("reverse-atom", OverridableOpId::ReverseAtom),
            ("flatten-atom", OverridableOpId::FlattenAtom),
            ("zip-atom", OverridableOpId::ZipAtom),
            ("take-atom", OverridableOpId::TakeAtom),
            ("drop-atom", OverridableOpId::DropAtom),
            ("is-member", OverridableOpId::IsMember),
            ("append", OverridableOpId::Append),
            ("length", OverridableOpId::Length),
            ("exclude-item", OverridableOpId::ExcludeItem),
            ("msort", OverridableOpId::Msort),
            ("sort-tuple", OverridableOpId::SortTuple),
            ("best-candidate", OverridableOpId::BestCandidate),
            ("cut", OverridableOpId::Cut),
        ];
        assert_eq!(names.len(), NUM_OVERRIDABLE_OPS);
        for (name, id) in names {
            assert_eq!(overridable_op_id(name), Some(id), "name {}", name);
        }
    }

    #[test]
    fn non_overridable_names_return_none() {
        // True HE primitives — must NOT be in the overridable set.
        assert_eq!(overridable_op_id("cons-atom"), None);
        assert_eq!(overridable_op_id("decons-atom"), None);
        assert_eq!(overridable_op_id("size-atom"), None);
        assert_eq!(overridable_op_id("max-atom"), None);
        assert_eq!(overridable_op_id("min-atom"), None);
        assert_eq!(overridable_op_id("index-atom"), None);
        // Syntactic forms.
        assert_eq!(overridable_op_id("if"), None);
        assert_eq!(overridable_op_id("let"), None);
        assert_eq!(overridable_op_id("match"), None);
        assert_eq!(overridable_op_id("="), None);
        // Arithmetic.
        assert_eq!(overridable_op_id("+"), None);
        assert_eq!(overridable_op_id("=="), None);
        // Side-effecting Class C.
        assert_eq!(overridable_op_id("println!"), None);
        assert_eq!(overridable_op_id("import!"), None);
        assert_eq!(overridable_op_id("new-state"), None);
        // Truly unknown name.
        assert_eq!(overridable_op_id("definitely-not-an-op"), None);
    }
}
