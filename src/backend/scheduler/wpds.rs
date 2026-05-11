//! Weighted Pushdown System (WPDS) for stack-context weight refinement.
//!
//! Extracted from `mettail-rust/prattail/src/wpds.rs` (~200 LOC core types).
//! The WPDS models the relationship between the evaluation continuation stack
//! (K in SECK) and expression cost:
//!
//! - **Push** (nested eval): entering a subexpression evaluation
//! - **Pop** (return): completing a subexpression and returning to parent
//! - **Replace** (step): moving to the next continuation in the chain
//!
//! ## Integration with MeTTaTron
//!
//! Each `GenericContinuation` frame maps to a `SchedulerStackSymbol`.
//! At scheduling time, the top-3 continuation frames are hashed into a
//! 64-bit context key for precomputed weight lookup.
//!
//! ## References
//!
//! - Reps, Lal & Kidd (2007), "Program analysis using weighted pushdown systems"
//! - Droste, Dziadek & Kuich (2019), "Simple reset weighted pushdown automata"

use std::collections::HashMap;
use std::fmt;

use super::semiring::Semiring;

// ══════════════════════════════════════════════════════════════════════════════
// Stack symbols
// ══════════════════════════════════════════════════════════════════════════════

/// A stack symbol in the WPDS, representing a continuation frame category.
///
/// Maps MeTTaTron's `GenericContinuation` variants to compact identifiers
/// for WPDS rule matching and context hashing.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SchedulerStackSymbol {
    /// Top-level evaluation (Done continuation).
    Root,
    /// Rule match processing: `ProcessRuleMatches{head,arity}`.
    RuleMatch {
        /// 16-bit hash of the head symbol.
        head_hash: u16,
        /// Arity of the match.
        arity: u8,
    },
    /// Binding application: `ApplyBindings{rule_idx}`.
    Binding {
        /// Index of the rule being applied.
        rule_idx: u16,
    },
    /// Argument evaluation: `EvalArgs{position}` or `CollectSExpr`.
    ArgEval {
        /// Position of the argument being evaluated.
        position: u8,
    },
    /// Conditional evaluation: `ProcessIfCondition`.
    Conditional {
        /// Branch indicator (0=condition, 1=then, 2=else).
        branch: u8,
    },
    /// Let-binding chain: `ProcessLet` or `ProcessLetStar`.
    LetChain {
        /// Nesting depth of the let chain.
        depth: u8,
    },
    /// Collapse evaluation: `ProcessCollapseEvalResults`.
    Collapse,
    /// Case/switch evaluation: `ProcessCaseAtom`.
    CaseSwitch,
    /// Grounded operation: `ProcessGroundedOp`.
    GroundedOp,
    /// Cartesian product combination: `ProcessCombinations`.
    Combinations,
    /// Memoization result recording: `MemoizeResult`.
    Memoize,
}

impl SchedulerStackSymbol {
    /// Compact numeric tag for hashing (one byte per symbol type).
    #[inline]
    pub fn tag(&self) -> u8 {
        match self {
            Self::Root => 0,
            Self::RuleMatch { .. } => 1,
            Self::Binding { .. } => 2,
            Self::ArgEval { .. } => 3,
            Self::Conditional { .. } => 4,
            Self::LetChain { .. } => 5,
            Self::Collapse => 6,
            Self::CaseSwitch => 7,
            Self::GroundedOp => 8,
            Self::Combinations => 9,
            Self::Memoize => 10,
        }
    }

    /// Pack this symbol into a u32 for fast hashing.
    ///
    /// Layout: `[tag:8 | payload:24]`
    #[inline]
    pub fn pack(&self) -> u32 {
        let tag = self.tag() as u32;
        let payload: u32 = match self {
            Self::Root => 0,
            Self::RuleMatch { head_hash, arity } => (*head_hash as u32) << 8 | (*arity as u32),
            Self::Binding { rule_idx } => *rule_idx as u32,
            Self::ArgEval { position } => *position as u32,
            Self::Conditional { branch } => *branch as u32,
            Self::LetChain { depth } => *depth as u32,
            Self::Collapse => 0,
            Self::CaseSwitch => 0,
            Self::GroundedOp => 0,
            Self::Combinations => 0,
            Self::Memoize => 0,
        };
        (tag << 24) | (payload & 0x00FF_FFFF)
    }
}

impl fmt::Display for SchedulerStackSymbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Root => write!(f, "⟨root⟩"),
            Self::RuleMatch { head_hash, arity } => {
                write!(f, "⟨match.{:04x}@{}⟩", head_hash, arity)
            }
            Self::Binding { rule_idx } => write!(f, "⟨bind.rule@{}⟩", rule_idx),
            Self::ArgEval { position } => write!(f, "⟨args@{}⟩", position),
            Self::Conditional { branch } => write!(f, "⟨if.{}⟩", branch),
            Self::LetChain { depth } => write!(f, "⟨let@{}⟩", depth),
            Self::Collapse => write!(f, "⟨collapse⟩"),
            Self::CaseSwitch => write!(f, "⟨case⟩"),
            Self::GroundedOp => write!(f, "⟨grounded⟩"),
            Self::Combinations => write!(f, "⟨combinations⟩"),
            Self::Memoize => write!(f, "⟨memoize⟩"),
        }
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// WPDS Rule types
// ══════════════════════════════════════════════════════════════════════════════

/// A PDS rule type, following Reps et al. (2007) Definition 1.
///
/// Rules have the form `⟨p, γ⟩ ↪ ⟨p', u⟩` where `|u| ∈ {0, 1, 2}`.
#[derive(Debug, Clone)]
pub enum WpdsRule<W: Semiring> {
    /// Pop rule: `⟨p, γ⟩ ↪ ⟨p', ε⟩` — removes top of stack (return from eval).
    Pop {
        /// Stack symbol consumed.
        from: SchedulerStackSymbol,
        /// Rule weight.
        weight: W,
    },
    /// Replace rule: `⟨p, γ⟩ ↪ ⟨p', γ'⟩` — replaces top of stack (continuation step).
    Replace {
        /// Stack symbol consumed.
        from: SchedulerStackSymbol,
        /// Stack symbol produced.
        to: SchedulerStackSymbol,
        /// Rule weight.
        weight: W,
    },
    /// Push rule: `⟨p, γ⟩ ↪ ⟨p', γ' γ''⟩` — pushes onto stack (nested eval).
    Push {
        /// Stack symbol consumed.
        from: SchedulerStackSymbol,
        /// Bottom of the two symbols pushed (continuation after return).
        to_bottom: SchedulerStackSymbol,
        /// Top of the two symbols pushed (callee entry).
        to_top: SchedulerStackSymbol,
        /// Rule weight.
        weight: W,
    },
}

impl<W: Semiring> WpdsRule<W> {
    /// The source stack symbol for this rule.
    pub fn from_symbol(&self) -> &SchedulerStackSymbol {
        match self {
            WpdsRule::Pop { from, .. }
            | WpdsRule::Replace { from, .. }
            | WpdsRule::Push { from, .. } => from,
        }
    }

    /// The weight of this rule.
    pub fn weight(&self) -> &W {
        match self {
            WpdsRule::Pop { weight, .. }
            | WpdsRule::Replace { weight, .. }
            | WpdsRule::Push { weight, .. } => weight,
        }
    }
}

impl<W: Semiring + fmt::Display> fmt::Display for WpdsRule<W> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WpdsRule::Pop { from, weight } => {
                write!(f, "⟨p, {}⟩ ↪ ⟨p', ε⟩  [w={}]", from, weight)
            }
            WpdsRule::Replace { from, to, weight } => {
                write!(f, "⟨p, {}⟩ ↪ ⟨p', {}⟩  [w={}]", from, to, weight)
            }
            WpdsRule::Push {
                from,
                to_bottom,
                to_top,
                weight,
            } => {
                write!(
                    f,
                    "⟨p, {}⟩ ↪ ⟨p', {} {}⟩  [w={}]",
                    from, to_bottom, to_top, weight
                )
            }
        }
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// WPDS structure
// ══════════════════════════════════════════════════════════════════════════════

/// A Weighted Pushdown System for scheduler context analysis.
///
/// Single control location `p` (context-free process encoding per Reps et al.).
/// The stack alphabet consists of `SchedulerStackSymbol` values corresponding
/// to MeTTaTron's continuation frames.
#[derive(Debug, Clone)]
pub struct Wpds<W: Semiring> {
    /// All WPDS rules.
    pub rules: Vec<WpdsRule<W>>,
    /// Rules indexed by source stack symbol for efficient lookup.
    pub rules_by_source: HashMap<SchedulerStackSymbol, Vec<usize>>,
    /// Initial stack symbol (typically Root).
    pub initial_symbol: SchedulerStackSymbol,
}

impl<W: Semiring> Wpds<W> {
    /// Create a new WPDS with the given initial stack symbol.
    pub fn new(initial: SchedulerStackSymbol) -> Self {
        Wpds {
            rules: Vec::new(),
            rules_by_source: HashMap::new(),
            initial_symbol: initial,
        }
    }

    /// Add a rule and index it by source symbol.
    pub fn add_rule(&mut self, rule: WpdsRule<W>) {
        let source = rule.from_symbol().clone();
        let idx = self.rules.len();
        self.rules.push(rule);
        self.rules_by_source.entry(source).or_default().push(idx);
    }

    /// Get all rules applicable from a given stack symbol.
    pub fn rules_from(&self, symbol: &SchedulerStackSymbol) -> &[usize] {
        self.rules_by_source
            .get(symbol)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// Number of rules.
    pub fn num_rules(&self) -> usize {
        self.rules.len()
    }

    /// Compute forward reachability weights (simplified poststar).
    ///
    /// Given an initial configuration `⟨p, γ₀⟩` with weight `W::one()`,
    /// computes the weight of reaching each stack symbol. This is a
    /// simplified version that tracks only the top-of-stack weight
    /// (sufficient for the scheduler's context weight computation).
    ///
    /// Returns a map from stack symbol → accumulated weight.
    pub fn poststar_top_of_stack(&self) -> HashMap<SchedulerStackSymbol, W> {
        let mut weights: HashMap<SchedulerStackSymbol, W> = HashMap::new();
        weights.insert(self.initial_symbol.clone(), W::one());

        // Fixed-point iteration (bounded by number of stack symbols)
        let max_iterations = self.rules.len() * 2 + 1;
        for _ in 0..max_iterations {
            let mut changed = false;

            for rule in &self.rules {
                let source = rule.from_symbol();
                let source_weight = match weights.get(source) {
                    Some(w) => *w,
                    None => continue,
                };

                match rule {
                    WpdsRule::Pop { weight, .. } => {
                        // Pop: weight contributes to "completed" state
                        let new_weight = source_weight.times(weight);
                        if !new_weight.is_zero() {
                            // Pop doesn't produce a new top-of-stack symbol
                            // (the parent's symbol is exposed)
                        }
                    }
                    WpdsRule::Replace { to, weight, .. } => {
                        let new_weight = source_weight.times(weight);
                        let entry = weights.entry(to.clone()).or_insert_with(W::zero);
                        let combined = entry.plus(&new_weight);
                        if !combined.approx_eq(entry, 1e-10) {
                            *entry = combined;
                            changed = true;
                        }
                    }
                    WpdsRule::Push { to_top, weight, .. } => {
                        let new_weight = source_weight.times(weight);
                        let entry = weights.entry(to_top.clone()).or_insert_with(W::zero);
                        let combined = entry.plus(&new_weight);
                        if !combined.approx_eq(entry, 1e-10) {
                            *entry = combined;
                            changed = true;
                        }
                    }
                }
            }

            if !changed {
                break;
            }
        }

        weights
    }
}

impl<W: Semiring> Default for Wpds<W> {
    fn default() -> Self {
        Self::new(SchedulerStackSymbol::Root)
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Context hashing
// ══════════════════════════════════════════════════════════════════════════════

/// Hash the top-k continuation frames into a 64-bit context key.
///
/// Uses FNV-1a mixing to combine up to 3 packed stack symbols.
/// This hash is used as the key into the precomputed context weight table.
#[inline]
pub fn hash_context(frames: &[SchedulerStackSymbol]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325; // FNV-1a 64-bit offset basis
    let limit = frames.len().min(3);
    for frame in &frames[..limit] {
        let packed = frame.pack() as u64;
        hash ^= packed;
        hash = hash.wrapping_mul(0x0100_0000_01b3); // FNV-1a 64-bit prime
    }
    hash
}

/// Hash the top-k continuation frames from raw packed u32 values.
///
/// This variant avoids allocating `SchedulerStackSymbol` when the frames
/// are already packed.
#[inline]
pub fn hash_context_packed(packed_frames: &[u32]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let limit = packed_frames.len().min(3);
    for &packed in &packed_frames[..limit] {
        hash ^= packed as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

// ══════════════════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::super::semiring::TropicalWeight;
    use super::*;

    #[test]
    fn test_stack_symbol_pack() {
        let root = SchedulerStackSymbol::Root;
        assert_eq!(root.pack() >> 24, 0); // tag = 0

        let rule_match = SchedulerStackSymbol::RuleMatch {
            head_hash: 0x1234,
            arity: 3,
        };
        let packed = rule_match.pack();
        assert_eq!(packed >> 24, 1); // tag = 1
    }

    #[test]
    fn test_context_hashing() {
        let frames = vec![
            SchedulerStackSymbol::RuleMatch {
                head_hash: 0x1234,
                arity: 2,
            },
            SchedulerStackSymbol::Conditional { branch: 1 },
            SchedulerStackSymbol::LetChain { depth: 3 },
        ];

        let hash1 = hash_context(&frames);
        let hash2 = hash_context(&frames);
        assert_eq!(hash1, hash2); // deterministic

        // Different frames → different hash
        let frames2 = vec![SchedulerStackSymbol::Root];
        assert_ne!(hash_context(&frames), hash_context(&frames2));
    }

    #[test]
    fn test_wpds_add_rule() {
        let mut wpds: Wpds<TropicalWeight> = Wpds::new(SchedulerStackSymbol::Root);

        wpds.add_rule(WpdsRule::Replace {
            from: SchedulerStackSymbol::Root,
            to: SchedulerStackSymbol::RuleMatch {
                head_hash: 0,
                arity: 2,
            },
            weight: TropicalWeight::new(1.0),
        });

        wpds.add_rule(WpdsRule::Push {
            from: SchedulerStackSymbol::RuleMatch {
                head_hash: 0,
                arity: 2,
            },
            to_bottom: SchedulerStackSymbol::RuleMatch {
                head_hash: 0,
                arity: 2,
            },
            to_top: SchedulerStackSymbol::ArgEval { position: 0 },
            weight: TropicalWeight::new(0.5),
        });

        assert_eq!(wpds.num_rules(), 2);
        assert_eq!(wpds.rules_from(&SchedulerStackSymbol::Root).len(), 1);
    }

    #[test]
    fn test_poststar_simple() {
        let mut wpds: Wpds<TropicalWeight> = Wpds::new(SchedulerStackSymbol::Root);

        // Root → RuleMatch with cost 2.0
        wpds.add_rule(WpdsRule::Replace {
            from: SchedulerStackSymbol::Root,
            to: SchedulerStackSymbol::RuleMatch {
                head_hash: 0,
                arity: 2,
            },
            weight: TropicalWeight::new(2.0),
        });

        // Root → LetChain with cost 5.0
        wpds.add_rule(WpdsRule::Replace {
            from: SchedulerStackSymbol::Root,
            to: SchedulerStackSymbol::LetChain { depth: 1 },
            weight: TropicalWeight::new(5.0),
        });

        let weights = wpds.poststar_top_of_stack();

        // Root has identity weight
        assert_eq!(
            weights.get(&SchedulerStackSymbol::Root),
            Some(&TropicalWeight::new(0.0))
        );

        // RuleMatch reachable with cost 2.0
        let rm_weight = weights
            .get(&SchedulerStackSymbol::RuleMatch {
                head_hash: 0,
                arity: 2,
            })
            .expect("RuleMatch should be reachable");
        assert_eq!(*rm_weight, TropicalWeight::new(2.0));

        // LetChain reachable with cost 5.0
        let let_weight = weights
            .get(&SchedulerStackSymbol::LetChain { depth: 1 })
            .expect("LetChain should be reachable");
        assert_eq!(*let_weight, TropicalWeight::new(5.0));
    }
}
