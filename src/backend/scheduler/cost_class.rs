//! Cost classes, task descriptors, and scheduling actions.
//!
//! Defines the 8-state cost classification produced by the tree automaton
//! (Layer 1) and the scheduling actions produced by the transducer (Layer 2).
//! Also defines `TaskDescriptor`, a packed u32 key for O(1) classification
//! table lookup.

use std::fmt;

// ══════════════════════════════════════════════════════════════════════════════
// CostClass — Tree automaton states
// ══════════════════════════════════════════════════════════════════════════════

/// Cost class assigned by the bottom-up tree automaton.
///
/// Each class corresponds to a tree automaton state and captures the
/// structural/semantic properties of a MeTTa expression relevant to scheduling.
///
/// The 8 classes are ordered roughly by expected execution cost (lower = cheaper).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum CostClass {
    /// Ground atoms, literals, resolved values. O(1) evaluation.
    GroundCheap = 0,
    /// Ground arithmetic/comparison operations. O(1) evaluation, memoizable.
    GroundArith = 1,
    /// Deterministic single-rule rewrite. Single rule always matches.
    SymbolicCheap = 2,
    /// Nondeterministic multi-rule rewrite. Multiple candidate rules.
    SymbolicModerate = 3,
    /// Recursive with structural termination proof. Bounded depth.
    RecursiveBounded = 4,
    /// Recursive without termination proof. Potentially divergent.
    RecursiveUnbounded = 5,
    /// Pure expression, safe for parallel evaluation.
    ParallelPure = 6,
    /// Side-effecting expression, must be sequential.
    ImpureSequential = 7,
}

impl CostClass {
    /// Number of cost classes.
    pub const COUNT: usize = 8;

    /// Convert from u8, returning None for invalid values.
    #[inline]
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(Self::GroundCheap),
            1 => Some(Self::GroundArith),
            2 => Some(Self::SymbolicCheap),
            3 => Some(Self::SymbolicModerate),
            4 => Some(Self::RecursiveBounded),
            5 => Some(Self::RecursiveUnbounded),
            6 => Some(Self::ParallelPure),
            7 => Some(Self::ImpureSequential),
            _ => None,
        }
    }

    /// Whether this class represents a ground (fully evaluated) expression.
    #[inline]
    pub fn is_ground(self) -> bool {
        matches!(self, Self::GroundCheap | Self::GroundArith)
    }

    /// Whether this class is safe for parallel evaluation.
    #[inline]
    pub fn is_parallelizable(self) -> bool {
        matches!(
            self,
            Self::GroundCheap
                | Self::GroundArith
                | Self::SymbolicCheap
                | Self::ParallelPure
                | Self::RecursiveBounded
        )
    }

    /// Whether this class benefits from memoization.
    #[inline]
    pub fn is_memoizable(self) -> bool {
        matches!(
            self,
            Self::GroundArith
                | Self::SymbolicCheap
                | Self::RecursiveBounded
                | Self::ParallelPure
        )
    }
}

impl fmt::Display for CostClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::GroundCheap => write!(f, "GroundCheap"),
            Self::GroundArith => write!(f, "GroundArith"),
            Self::SymbolicCheap => write!(f, "SymbolicCheap"),
            Self::SymbolicModerate => write!(f, "SymbolicModerate"),
            Self::RecursiveBounded => write!(f, "RecursiveBounded"),
            Self::RecursiveUnbounded => write!(f, "RecursiveUnbounded"),
            Self::ParallelPure => write!(f, "ParallelPure"),
            Self::ImpureSequential => write!(f, "ImpureSequential"),
        }
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// TaskDescriptor — Packed u32 key for O(1) lookup
// ══════════════════════════════════════════════════════════════════════════════

/// Packed task descriptor for O(1) tree automaton lookup.
///
/// Layout: `[head_hash:16 | arity:4 | depth_bucket:4 | flags:8]`
///
/// - `head_hash`: xxh3 of head symbol name, truncated to 16 bits (65536 buckets)
/// - `arity`: 0..15 (MeTTa expressions rarely exceed arity 15)
/// - `depth_bucket`: 0..15 (evaluation depth quantized into 16 levels)
/// - `flags`: bit 0 = is_pure, bit 1 = is_ground, bit 2 = is_deterministic,
///           bit 3 = is_memo_candidate, bits 4-7 reserved
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskDescriptor(u32);

/// Flag bits within TaskDescriptor.
pub mod descriptor_flags {
    /// Expression (or all sub-expressions) are provably pure.
    pub const PURE: u8 = 0x01;
    /// Expression is fully ground (no variables).
    pub const GROUND: u8 = 0x02;
    /// Expression matches exactly one rule (deterministic dispatch).
    pub const DETERMINISTIC: u8 = 0x04;
    /// Expression is a memoization candidate.
    pub const MEMO_CANDIDATE: u8 = 0x08;
}

impl TaskDescriptor {
    /// Pack a task descriptor from components.
    ///
    /// - `head_hash`: 16-bit hash of head symbol
    /// - `arity`: 0..15
    /// - `depth_bucket`: 0..15
    /// - `flags`: analysis flag bits
    #[inline]
    pub const fn pack(head_hash: u16, arity: u8, depth_bucket: u8, flags: u8) -> Self {
        let v = (head_hash as u32) << 16
            | ((arity & 0x0F) as u32) << 12
            | ((depth_bucket & 0x0F) as u32) << 8
            | (flags as u32);
        TaskDescriptor(v)
    }

    /// Extract the 16-bit head symbol hash.
    #[inline]
    pub const fn head_hash(self) -> u16 {
        (self.0 >> 16) as u16
    }

    /// Extract the 4-bit arity.
    #[inline]
    pub const fn arity(self) -> u8 {
        ((self.0 >> 12) & 0x0F) as u8
    }

    /// Extract the 4-bit depth bucket.
    #[inline]
    pub const fn depth_bucket(self) -> u8 {
        ((self.0 >> 8) & 0x0F) as u8
    }

    /// Extract the 8-bit flags.
    #[inline]
    pub const fn flags(self) -> u8 {
        (self.0 & 0xFF) as u8
    }

    /// Get the raw packed u32.
    #[inline]
    pub const fn raw(self) -> u32 {
        self.0
    }

    /// Key for the level-1 classification table (head_hash).
    #[inline]
    pub const fn l1_index(self) -> usize {
        self.head_hash() as usize
    }

    /// Key for per-(head,arity) weight tracking.
    #[inline]
    pub const fn head_arity_key(self) -> u32 {
        (self.head_hash() as u32) << 4 | (self.arity() as u32)
    }

    /// Check a flag bit.
    #[inline]
    pub const fn has_flag(self, flag: u8) -> bool {
        (self.flags() & flag) != 0
    }

    /// Compute a 16-bit hash of a head symbol string.
    ///
    /// Uses a fast FNV-1a variant truncated to 16 bits.
    #[inline]
    pub fn hash_head_symbol(s: &str) -> u16 {
        // FNV-1a 32-bit, truncated to 16
        let mut hash: u32 = 0x811c_9dc5;
        for byte in s.as_bytes() {
            hash ^= *byte as u32;
            hash = hash.wrapping_mul(0x0100_0193);
        }
        // Fold 32 → 16 bits via XOR-fold for better distribution
        ((hash >> 16) ^ (hash & 0xFFFF)) as u16
    }

    /// Quantize an evaluation depth into a 4-bit bucket (0..15).
    ///
    /// Uses logarithmic bucketing:
    /// - 0..3 → 0..3 (fine-grained for shallow depths)
    /// - 4..7 → 4
    /// - 8..15 → 5
    /// - 16..31 → 6
    /// - ...exponential...
    /// - 4096+ → 15
    #[inline]
    pub fn depth_to_bucket(depth: u32) -> u8 {
        if depth < 4 {
            depth as u8
        } else {
            // log2(depth) + 2, capped at 15
            let log2 = 31 - depth.leading_zeros();
            (log2 + 2).min(15) as u8
        }
    }
}

impl fmt::Debug for TaskDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "TaskDescriptor(head=0x{:04x}, arity={}, depth={}, flags=0x{:02x})",
            self.head_hash(),
            self.arity(),
            self.depth_bucket(),
            self.flags()
        )
    }
}

impl fmt::Display for TaskDescriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self)
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// AffinityHint
// ══════════════════════════════════════════════════════════════════════════════

/// Worker affinity hint for scheduling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AffinityHint {
    /// No preference — any worker is fine.
    Any,
    /// Prefer same worker for cache locality (e.g., recursive calls,
    /// let-chains that share bindings).
    Sticky,
}

// ══════════════════════════════════════════════════════════════════════════════
// SchedulingAction — Transducer output
// ══════════════════════════════════════════════════════════════════════════════

/// Scheduling decision produced by the WFST transducer (Layer 2).
///
/// Maps a `CostClass` to concrete scheduling parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SchedulingAction {
    /// Priority class (0 = highest/interactive, 50 = lowest/batch).
    pub priority_class: u8,
    /// How many parallel workers to allocate for this expression's branches.
    /// 1 = sequential, >1 = fan out to this many workers.
    pub parallelism_degree: u8,
    /// Worker affinity hint.
    pub affinity_hint: AffinityHint,
    /// Whether the result should be memoized.
    pub memoizable: bool,
}

impl SchedulingAction {
    /// Create a new scheduling action.
    pub const fn new(
        priority_class: u8,
        parallelism_degree: u8,
        affinity_hint: AffinityHint,
        memoizable: bool,
    ) -> Self {
        SchedulingAction {
            priority_class,
            parallelism_degree,
            affinity_hint,
            memoizable,
        }
    }
}

impl fmt::Display for SchedulingAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "SchedulingAction(pri={}, par={}, aff={:?}, memo={})",
            self.priority_class, self.parallelism_degree, self.affinity_hint, self.memoizable
        )
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_descriptor_pack_unpack() {
        let desc = TaskDescriptor::pack(0xABCD, 5, 3, 0x0F);
        assert_eq!(desc.head_hash(), 0xABCD);
        assert_eq!(desc.arity(), 5);
        assert_eq!(desc.depth_bucket(), 3);
        assert_eq!(desc.flags(), 0x0F);
    }

    #[test]
    fn test_task_descriptor_flags() {
        let desc = TaskDescriptor::pack(0, 0, 0, descriptor_flags::PURE | descriptor_flags::GROUND);
        assert!(desc.has_flag(descriptor_flags::PURE));
        assert!(desc.has_flag(descriptor_flags::GROUND));
        assert!(!desc.has_flag(descriptor_flags::DETERMINISTIC));
    }

    #[test]
    fn test_head_hash() {
        let h1 = TaskDescriptor::hash_head_symbol("+");
        let h2 = TaskDescriptor::hash_head_symbol("if");
        let h3 = TaskDescriptor::hash_head_symbol("+");
        assert_eq!(h1, h3); // deterministic
        assert_ne!(h1, h2); // different symbols
    }

    #[test]
    fn test_depth_bucketing() {
        assert_eq!(TaskDescriptor::depth_to_bucket(0), 0);
        assert_eq!(TaskDescriptor::depth_to_bucket(1), 1);
        assert_eq!(TaskDescriptor::depth_to_bucket(3), 3);
        assert_eq!(TaskDescriptor::depth_to_bucket(4), 4);
        assert_eq!(TaskDescriptor::depth_to_bucket(7), 4);
        assert_eq!(TaskDescriptor::depth_to_bucket(8), 5);
        assert_eq!(TaskDescriptor::depth_to_bucket(16), 6);
        assert_eq!(TaskDescriptor::depth_to_bucket(10000), 15);
    }

    #[test]
    fn test_cost_class_properties() {
        assert!(CostClass::GroundCheap.is_ground());
        assert!(CostClass::GroundArith.is_ground());
        assert!(!CostClass::SymbolicCheap.is_ground());

        assert!(CostClass::ParallelPure.is_parallelizable());
        assert!(!CostClass::ImpureSequential.is_parallelizable());

        assert!(CostClass::GroundArith.is_memoizable());
        assert!(!CostClass::SymbolicModerate.is_memoizable());
    }

    #[test]
    fn test_l1_index() {
        let desc = TaskDescriptor::pack(42, 3, 5, 0);
        assert_eq!(desc.l1_index(), 42);
    }

    #[test]
    fn test_head_arity_key() {
        let desc = TaskDescriptor::pack(0x1234, 7, 0, 0);
        let key = desc.head_arity_key();
        assert_eq!(key, (0x1234 << 4) | 7);
    }
}
