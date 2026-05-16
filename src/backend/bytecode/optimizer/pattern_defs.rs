//! Declarative peephole pattern definitions.
//!
//! Each `PatternDef` describes a byte sequence to match, optional post-conditions,
//! and the replacement action. These definitions are the single source of truth —
//! the DFA transition tables are generated from them.

use crate::backend::bytecode::opcodes::Opcode;

/// A byte-level constraint at one position in a pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteMatch {
    /// Match this exact byte value.
    Exact(u8),
    /// Match any byte value (wildcard). DFA: all 256 transitions → same next state.
    Any,
}

/// Post-conditions checked after the DFA structural match succeeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostCondition {
    /// `code[offset + a]` must equal `code[offset + b]`.
    BytesEqual(u8, u8),
    /// The opcode immediately before the match must be a numeric producer.
    NumericProducerGuard,
}

/// What to do when a pattern matches.
#[derive(Debug, Clone)]
pub enum PatternAction {
    /// Remove the entire matched region.
    Remove,
    /// Remove only the first `n` bytes of the matched region (partial removal).
    RemoveFirst(usize),
    /// Replace the matched region with a single opcode.
    ReplaceOpcode(Opcode),
    /// Replace the matched region with a static byte sequence.
    ReplaceBytes(&'static [u8]),
    /// Replace using a template with captured bytes from the match region.
    /// Each `(template_pos, source_offset)` pair copies `code[offset + source_offset]`
    /// into `template[template_pos]`.
    ReplaceBytesWithCapture {
        template: &'static [u8],
        captures: &'static [(usize, usize)],
    },
    /// Custom logic (e.g., arithmetic on operand).
    /// Returns `Some(replacement_bytes)` on success, `None` to skip.
    Custom(fn(code: &[u8], offset: usize, match_end: usize) -> Option<Vec<u8>>),
}

/// Which statistics counter to increment when a pattern fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatKind {
    NopsRemoved,
    SwapSwapRemoved,
    DupPopRemoved,
    NotNotRemoved,
    ConstNotFolded,
    PushPopRemoved,
    NegNegRemoved,
    IdempotentRemoved,
    ComparisonFolded,
    IdentityOpsRemoved,
    MulZeroFolded,
    PowFolded,
    ConstBranchFolded,
    DeadBranchRemoved,
    LoadDeduplicated,
    BranchInverted,
    ComparisonBranchFused,
    OverDupFolded,
    PopFused,
    BuildDeconstructFolded,
    StoreLoadFolded,
}

/// A complete declarative peephole pattern definition.
#[derive(Debug, Clone)]
pub struct PatternDef {
    /// Byte constraints at each position.
    pub bytes: &'static [ByteMatch],
    /// Post-conditions checked after DFA structural match.
    pub postconditions: &'static [PostCondition],
    /// Replacement action.
    pub action: PatternAction,
    /// Statistics counter to increment.
    pub stat: StatKind,
}

/// Check if an opcode is known to always produce a numeric (Long) value.
///
/// Used to guard arithmetic identity/absorber optimizations that assume
/// the preceding stack value is numeric.
pub fn is_numeric_producer(opcode: Option<Opcode>) -> bool {
    matches!(
        opcode,
        Some(Opcode::PushLongSmall)
            | Some(Opcode::PushLong)
            | Some(Opcode::Add)
            | Some(Opcode::Sub)
            | Some(Opcode::Mul)
            | Some(Opcode::Div)
            | Some(Opcode::Mod)
            | Some(Opcode::Neg)
            | Some(Opcode::Abs)
            | Some(Opcode::Pow)
            | Some(Opcode::PowMath)
            | Some(Opcode::FloorDiv)
    )
}

// Convenience aliases for readability
const fn exact(b: u8) -> ByteMatch {
    ByteMatch::Exact(b)
}

const ANY: ByteMatch = ByteMatch::Any;

/// All peephole pattern definitions, ordered by priority (lower index = higher priority).
///
/// When two patterns match at the same position and length, the one with lower
/// index wins. Longer matches always win over shorter ones (longest-match semantics).
///
/// Patterns are grouped by category for readability but ordering matters for priority.
pub fn all_pattern_defs() -> Vec<PatternDef> {
    let nop = Opcode::Nop.to_byte();
    let pop = Opcode::Pop.to_byte();
    let dup = Opcode::Dup.to_byte();
    let swap = Opcode::Swap.to_byte();
    let over = Opcode::Over.to_byte();
    let pop_n = Opcode::PopN.to_byte();
    let push_true = Opcode::PushTrue.to_byte();
    let push_false = Opcode::PushFalse.to_byte();
    let push_unit = Opcode::PushUnit.to_byte();
    let push_empty = Opcode::PushEmpty.to_byte();
    let push_long_small = Opcode::PushLongSmall.to_byte();
    let push_long = Opcode::PushLong.to_byte();
    let push_atom = Opcode::PushAtom.to_byte();
    let push_string = Opcode::PushString.to_byte();
    let push_uri = Opcode::PushUri.to_byte();
    let push_constant = Opcode::PushConstant.to_byte();
    let push_variable = Opcode::PushVariable.to_byte();
    let load_local = Opcode::LoadLocal.to_byte();
    let store_local = Opcode::StoreLocal.to_byte();
    let not = Opcode::Not.to_byte();
    let neg = Opcode::Neg.to_byte();
    let abs = Opcode::Abs.to_byte();
    let add = Opcode::Add.to_byte();
    let sub = Opcode::Sub.to_byte();
    let mul = Opcode::Mul.to_byte();
    let div = Opcode::Div.to_byte();
    let pow = Opcode::Pow.to_byte();
    let lt = Opcode::Lt.to_byte();
    let le = Opcode::Le.to_byte();
    let gt = Opcode::Gt.to_byte();
    let ge = Opcode::Ge.to_byte();
    let eq = Opcode::Eq.to_byte();
    let ne = Opcode::Ne.to_byte();
    let jump_if_true = Opcode::JumpIfTrue.to_byte();
    let jump_if_false = Opcode::JumpIfFalse.to_byte();
    let make_sexpr = Opcode::MakeSExpr.to_byte();
    let get_head = Opcode::GetHead.to_byte();

    // Leak the byte arrays so they have 'static lifetime.
    // This function is called once during DFA construction; the leak is intentional.
    let leak = |v: Vec<ByteMatch>| -> &'static [ByteMatch] { Box::leak(v.into_boxed_slice()) };
    let leak_pc =
        |v: Vec<PostCondition>| -> &'static [PostCondition] { Box::leak(v.into_boxed_slice()) };

    let no_post: &'static [PostCondition] = leak_pc(vec![]);
    let numeric_guard: &'static [PostCondition] =
        leak_pc(vec![PostCondition::NumericProducerGuard]);

    vec![
        // =====================================================================
        // TIER 1: New high-ROI patterns (longer patterns first for priority)
        // N3-N8: Comparison + Not + JumpIfFalse → InvComparison + JumpIfTrue
        // These 5-byte patterns MUST come before the 2-byte comparison fold patterns
        // so longest-match wins.
        // =====================================================================

        // P0: Lt; Not; JumpIfFalse hi lo → Ge; JumpIfTrue hi lo
        PatternDef {
            bytes: leak(vec![exact(lt), exact(not), exact(jump_if_false), ANY, ANY]),
            postconditions: no_post,
            action: PatternAction::ReplaceBytesWithCapture {
                template: &[0 /* Ge */, 0 /* JumpIfTrue */, 0, 0],
                captures: &[(2, 3), (3, 4)], // copy jump offset bytes
            },
            stat: StatKind::ComparisonBranchFused,
        },
        // P1: Le; Not; JumpIfFalse hi lo → Gt; JumpIfTrue hi lo
        PatternDef {
            bytes: leak(vec![exact(le), exact(not), exact(jump_if_false), ANY, ANY]),
            postconditions: no_post,
            action: PatternAction::ReplaceBytesWithCapture {
                template: &[0 /* Gt */, 0 /* JumpIfTrue */, 0, 0],
                captures: &[(2, 3), (3, 4)],
            },
            stat: StatKind::ComparisonBranchFused,
        },
        // P2: Gt; Not; JumpIfFalse hi lo → Le; JumpIfTrue hi lo
        PatternDef {
            bytes: leak(vec![exact(gt), exact(not), exact(jump_if_false), ANY, ANY]),
            postconditions: no_post,
            action: PatternAction::ReplaceBytesWithCapture {
                template: &[0 /* Le */, 0 /* JumpIfTrue */, 0, 0],
                captures: &[(2, 3), (3, 4)],
            },
            stat: StatKind::ComparisonBranchFused,
        },
        // P3: Ge; Not; JumpIfFalse hi lo → Lt; JumpIfTrue hi lo
        PatternDef {
            bytes: leak(vec![exact(ge), exact(not), exact(jump_if_false), ANY, ANY]),
            postconditions: no_post,
            action: PatternAction::ReplaceBytesWithCapture {
                template: &[0 /* Lt */, 0 /* JumpIfTrue */, 0, 0],
                captures: &[(2, 3), (3, 4)],
            },
            stat: StatKind::ComparisonBranchFused,
        },
        // P4: Eq; Not; JumpIfFalse hi lo → Ne; JumpIfTrue hi lo
        PatternDef {
            bytes: leak(vec![exact(eq), exact(not), exact(jump_if_false), ANY, ANY]),
            postconditions: no_post,
            action: PatternAction::ReplaceBytesWithCapture {
                template: &[0 /* Ne */, 0 /* JumpIfTrue */, 0, 0],
                captures: &[(2, 3), (3, 4)],
            },
            stat: StatKind::ComparisonBranchFused,
        },
        // P5: Ne; Not; JumpIfFalse hi lo → Eq; JumpIfTrue hi lo
        PatternDef {
            bytes: leak(vec![exact(ne), exact(not), exact(jump_if_false), ANY, ANY]),
            postconditions: no_post,
            action: PatternAction::ReplaceBytesWithCapture {
                template: &[0 /* Eq */, 0 /* JumpIfTrue */, 0, 0],
                captures: &[(2, 3), (3, 4)],
            },
            stat: StatKind::ComparisonBranchFused,
        },
        // P6-P11: Cmp; Not; JumpIfTrue hi lo → InvCmp; JumpIfFalse hi lo
        PatternDef {
            bytes: leak(vec![exact(lt), exact(not), exact(jump_if_true), ANY, ANY]),
            postconditions: no_post,
            action: PatternAction::ReplaceBytesWithCapture {
                template: &[0 /* Ge */, 0 /* JumpIfFalse */, 0, 0],
                captures: &[(2, 3), (3, 4)],
            },
            stat: StatKind::ComparisonBranchFused,
        },
        PatternDef {
            bytes: leak(vec![exact(le), exact(not), exact(jump_if_true), ANY, ANY]),
            postconditions: no_post,
            action: PatternAction::ReplaceBytesWithCapture {
                template: &[0 /* Gt */, 0 /* JumpIfFalse */, 0, 0],
                captures: &[(2, 3), (3, 4)],
            },
            stat: StatKind::ComparisonBranchFused,
        },
        PatternDef {
            bytes: leak(vec![exact(gt), exact(not), exact(jump_if_true), ANY, ANY]),
            postconditions: no_post,
            action: PatternAction::ReplaceBytesWithCapture {
                template: &[0 /* Le */, 0 /* JumpIfFalse */, 0, 0],
                captures: &[(2, 3), (3, 4)],
            },
            stat: StatKind::ComparisonBranchFused,
        },
        PatternDef {
            bytes: leak(vec![exact(ge), exact(not), exact(jump_if_true), ANY, ANY]),
            postconditions: no_post,
            action: PatternAction::ReplaceBytesWithCapture {
                template: &[0 /* Lt */, 0 /* JumpIfFalse */, 0, 0],
                captures: &[(2, 3), (3, 4)],
            },
            stat: StatKind::ComparisonBranchFused,
        },
        PatternDef {
            bytes: leak(vec![exact(eq), exact(not), exact(jump_if_true), ANY, ANY]),
            postconditions: no_post,
            action: PatternAction::ReplaceBytesWithCapture {
                template: &[0 /* Ne */, 0 /* JumpIfFalse */, 0, 0],
                captures: &[(2, 3), (3, 4)],
            },
            stat: StatKind::ComparisonBranchFused,
        },
        PatternDef {
            bytes: leak(vec![exact(ne), exact(not), exact(jump_if_true), ANY, ANY]),
            postconditions: no_post,
            action: PatternAction::ReplaceBytesWithCapture {
                template: &[0 /* Eq */, 0 /* JumpIfFalse */, 0, 0],
                captures: &[(2, 3), (3, 4)],
            },
            stat: StatKind::ComparisonBranchFused,
        },
        // N1: Not; JumpIfFalse hi lo → JumpIfTrue hi lo
        PatternDef {
            bytes: leak(vec![exact(not), exact(jump_if_false), ANY, ANY]),
            postconditions: no_post,
            action: PatternAction::ReplaceBytesWithCapture {
                template: &[0 /* JumpIfTrue */, 0, 0],
                captures: &[(1, 2), (2, 3)],
            },
            stat: StatKind::BranchInverted,
        },
        // N2: Not; JumpIfTrue hi lo → JumpIfFalse hi lo
        PatternDef {
            bytes: leak(vec![exact(not), exact(jump_if_true), ANY, ANY]),
            postconditions: no_post,
            action: PatternAction::ReplaceBytesWithCapture {
                template: &[0 /* JumpIfFalse */, 0, 0],
                captures: &[(1, 2), (2, 3)],
            },
            stat: StatKind::BranchInverted,
        },
        // =====================================================================
        // Constant branch folding (4 bytes) — before simpler push-pop patterns
        // =====================================================================

        // PushTrue; JumpIfTrue hi lo → Jump hi lo (always taken)
        PatternDef {
            bytes: leak(vec![exact(push_true), exact(jump_if_true), ANY, ANY]),
            postconditions: no_post,
            action: PatternAction::ReplaceBytesWithCapture {
                template: &[0 /* Jump */, 0, 0],
                captures: &[(1, 2), (2, 3)],
            },
            stat: StatKind::ConstBranchFolded,
        },
        // PushFalse; JumpIfFalse hi lo → Jump hi lo (always taken)
        PatternDef {
            bytes: leak(vec![exact(push_false), exact(jump_if_false), ANY, ANY]),
            postconditions: no_post,
            action: PatternAction::ReplaceBytesWithCapture {
                template: &[0 /* Jump */, 0, 0],
                captures: &[(1, 2), (2, 3)],
            },
            stat: StatKind::ConstBranchFolded,
        },
        // PushTrue; JumpIfFalse hi lo → remove all (never taken)
        PatternDef {
            bytes: leak(vec![exact(push_true), exact(jump_if_false), ANY, ANY]),
            postconditions: no_post,
            action: PatternAction::Remove,
            stat: StatKind::DeadBranchRemoved,
        },
        // PushFalse; JumpIfTrue hi lo → remove all (never taken)
        PatternDef {
            bytes: leak(vec![exact(push_false), exact(jump_if_true), ANY, ANY]),
            postconditions: no_post,
            action: PatternAction::Remove,
            stat: StatKind::DeadBranchRemoved,
        },
        // =====================================================================
        // Load deduplication (4 bytes)
        // =====================================================================

        // LoadLocal X; LoadLocal X → LoadLocal X; Dup
        PatternDef {
            bytes: leak(vec![exact(load_local), ANY, exact(load_local), ANY]),
            postconditions: leak_pc(vec![PostCondition::BytesEqual(1, 3)]),
            action: PatternAction::ReplaceBytesWithCapture {
                template: &[0 /* LoadLocal */, 0, 0 /* Dup */],
                captures: &[(1, 1)], // copy slot byte
            },
            stat: StatKind::LoadDeduplicated,
        },
        // =====================================================================
        // Four-byte push-pop patterns (PushLong/PushAtom/etc with u16 + Pop)
        // =====================================================================

        // PushLong X X; Pop → remove
        PatternDef {
            bytes: leak(vec![exact(push_long), ANY, ANY, exact(pop)]),
            postconditions: no_post,
            action: PatternAction::Remove,
            stat: StatKind::PushPopRemoved,
        },
        // PushAtom X X; Pop → remove
        PatternDef {
            bytes: leak(vec![exact(push_atom), ANY, ANY, exact(pop)]),
            postconditions: no_post,
            action: PatternAction::Remove,
            stat: StatKind::PushPopRemoved,
        },
        // PushString X X; Pop → remove
        PatternDef {
            bytes: leak(vec![exact(push_string), ANY, ANY, exact(pop)]),
            postconditions: no_post,
            action: PatternAction::Remove,
            stat: StatKind::PushPopRemoved,
        },
        // PushUri X X; Pop → remove
        PatternDef {
            bytes: leak(vec![exact(push_uri), ANY, ANY, exact(pop)]),
            postconditions: no_post,
            action: PatternAction::Remove,
            stat: StatKind::PushPopRemoved,
        },
        // PushConstant X X; Pop → remove
        PatternDef {
            bytes: leak(vec![exact(push_constant), ANY, ANY, exact(pop)]),
            postconditions: no_post,
            action: PatternAction::Remove,
            stat: StatKind::PushPopRemoved,
        },
        // PushVariable X X; Pop → remove
        PatternDef {
            bytes: leak(vec![exact(push_variable), ANY, ANY, exact(pop)]),
            postconditions: no_post,
            action: PatternAction::Remove,
            stat: StatKind::PushPopRemoved,
        },
        // =====================================================================
        // Three-byte arithmetic patterns (guarded by numeric producer)
        // =====================================================================

        // PushLongSmall 0; Add → remove (x + 0 = x)
        PatternDef {
            bytes: leak(vec![exact(push_long_small), exact(0), exact(add)]),
            postconditions: numeric_guard,
            action: PatternAction::Remove,
            stat: StatKind::IdentityOpsRemoved,
        },
        // PushLongSmall 0; Sub → remove (x - 0 = x)
        PatternDef {
            bytes: leak(vec![exact(push_long_small), exact(0), exact(sub)]),
            postconditions: numeric_guard,
            action: PatternAction::Remove,
            stat: StatKind::IdentityOpsRemoved,
        },
        // PushLongSmall 1; Mul → remove (x * 1 = x)
        PatternDef {
            bytes: leak(vec![exact(push_long_small), exact(1), exact(mul)]),
            postconditions: numeric_guard,
            action: PatternAction::Remove,
            stat: StatKind::IdentityOpsRemoved,
        },
        // PushLongSmall 1; Div → remove (x / 1 = x)
        PatternDef {
            bytes: leak(vec![exact(push_long_small), exact(1), exact(div)]),
            postconditions: numeric_guard,
            action: PatternAction::Remove,
            stat: StatKind::IdentityOpsRemoved,
        },
        // PushLongSmall 0; Mul → Pop; PushLongSmall 0 (x * 0 = 0)
        PatternDef {
            bytes: leak(vec![exact(push_long_small), exact(0), exact(mul)]),
            postconditions: numeric_guard,
            action: PatternAction::ReplaceBytes(&[
                0x01, /* Pop */
                0x14, /* PushLongSmall */
                0,
            ]),
            stat: StatKind::MulZeroFolded,
        },
        // PushLongSmall 0; Pow → Pop; PushLongSmall 1 (x ^ 0 = 1)
        PatternDef {
            bytes: leak(vec![exact(push_long_small), exact(0), exact(pow)]),
            postconditions: numeric_guard,
            action: PatternAction::ReplaceBytes(&[
                0x01, /* Pop */
                0x14, /* PushLongSmall */
                1,
            ]),
            stat: StatKind::PowFolded,
        },
        // PushLongSmall 1; Pow → remove (x ^ 1 = x)
        PatternDef {
            bytes: leak(vec![exact(push_long_small), exact(1), exact(pow)]),
            postconditions: numeric_guard,
            action: PatternAction::Remove,
            stat: StatKind::PowFolded,
        },
        // PushLongSmall X; Pop → remove
        PatternDef {
            bytes: leak(vec![exact(push_long_small), ANY, exact(pop)]),
            postconditions: no_post,
            action: PatternAction::Remove,
            stat: StatKind::PushPopRemoved,
        },
        // =====================================================================
        // Two-byte patterns
        // =====================================================================

        // Swap; Swap → remove
        PatternDef {
            bytes: leak(vec![exact(swap), exact(swap)]),
            postconditions: no_post,
            action: PatternAction::Remove,
            stat: StatKind::SwapSwapRemoved,
        },
        // Dup; Pop → remove
        PatternDef {
            bytes: leak(vec![exact(dup), exact(pop)]),
            postconditions: no_post,
            action: PatternAction::Remove,
            stat: StatKind::DupPopRemoved,
        },
        // Not; Not → remove
        PatternDef {
            bytes: leak(vec![exact(not), exact(not)]),
            postconditions: no_post,
            action: PatternAction::Remove,
            stat: StatKind::NotNotRemoved,
        },
        // PushTrue; Not → PushFalse
        PatternDef {
            bytes: leak(vec![exact(push_true), exact(not)]),
            postconditions: no_post,
            action: PatternAction::ReplaceOpcode(Opcode::PushFalse),
            stat: StatKind::ConstNotFolded,
        },
        // PushFalse; Not → PushTrue
        PatternDef {
            bytes: leak(vec![exact(push_false), exact(not)]),
            postconditions: no_post,
            action: PatternAction::ReplaceOpcode(Opcode::PushTrue),
            stat: StatKind::ConstNotFolded,
        },
        // PushUnit; Pop → remove
        PatternDef {
            bytes: leak(vec![exact(push_unit), exact(pop)]),
            postconditions: no_post,
            action: PatternAction::Remove,
            stat: StatKind::PushPopRemoved,
        },
        // PushTrue; Pop → remove
        PatternDef {
            bytes: leak(vec![exact(push_true), exact(pop)]),
            postconditions: no_post,
            action: PatternAction::Remove,
            stat: StatKind::PushPopRemoved,
        },
        // PushFalse; Pop → remove
        PatternDef {
            bytes: leak(vec![exact(push_false), exact(pop)]),
            postconditions: no_post,
            action: PatternAction::Remove,
            stat: StatKind::PushPopRemoved,
        },
        // PushEmpty; Pop → remove
        PatternDef {
            bytes: leak(vec![exact(push_empty), exact(pop)]),
            postconditions: no_post,
            action: PatternAction::Remove,
            stat: StatKind::PushPopRemoved,
        },
        // Neg; Neg → remove
        PatternDef {
            bytes: leak(vec![exact(neg), exact(neg)]),
            postconditions: no_post,
            action: PatternAction::Remove,
            stat: StatKind::NegNegRemoved,
        },
        // Abs; Abs → remove first Abs (idempotent)
        PatternDef {
            bytes: leak(vec![exact(abs), exact(abs)]),
            postconditions: no_post,
            action: PatternAction::RemoveFirst(1),
            stat: StatKind::IdempotentRemoved,
        },
        // Lt; Not → Ge
        PatternDef {
            bytes: leak(vec![exact(lt), exact(not)]),
            postconditions: no_post,
            action: PatternAction::ReplaceOpcode(Opcode::Ge),
            stat: StatKind::ComparisonFolded,
        },
        // Le; Not → Gt
        PatternDef {
            bytes: leak(vec![exact(le), exact(not)]),
            postconditions: no_post,
            action: PatternAction::ReplaceOpcode(Opcode::Gt),
            stat: StatKind::ComparisonFolded,
        },
        // Gt; Not → Le
        PatternDef {
            bytes: leak(vec![exact(gt), exact(not)]),
            postconditions: no_post,
            action: PatternAction::ReplaceOpcode(Opcode::Le),
            stat: StatKind::ComparisonFolded,
        },
        // Ge; Not → Lt
        PatternDef {
            bytes: leak(vec![exact(ge), exact(not)]),
            postconditions: no_post,
            action: PatternAction::ReplaceOpcode(Opcode::Lt),
            stat: StatKind::ComparisonFolded,
        },
        // Eq; Not → Ne
        PatternDef {
            bytes: leak(vec![exact(eq), exact(not)]),
            postconditions: no_post,
            action: PatternAction::ReplaceOpcode(Opcode::Ne),
            stat: StatKind::ComparisonFolded,
        },
        // Ne; Not → Eq
        PatternDef {
            bytes: leak(vec![exact(ne), exact(not)]),
            postconditions: no_post,
            action: PatternAction::ReplaceOpcode(Opcode::Eq),
            stat: StatKind::ComparisonFolded,
        },
        // =====================================================================
        // New Tier 2 patterns
        // =====================================================================

        // N9: Over; Over → Over; Dup
        PatternDef {
            bytes: leak(vec![exact(over), exact(over)]),
            postconditions: no_post,
            action: PatternAction::ReplaceBytes(&[0x05 /* Over */, 0x02 /* Dup */]),
            stat: StatKind::OverDupFolded,
        },
        // N10: Pop; PopN N → PopN N+1 (if N+1 ≤ 255)
        PatternDef {
            bytes: leak(vec![exact(pop), exact(pop_n), ANY]),
            postconditions: no_post,
            action: PatternAction::Custom(|code, offset, _match_end| {
                let n = code[offset + 2] as u16;
                if n + 1 <= 255 {
                    Some(vec![Opcode::PopN.to_byte(), (n + 1) as u8])
                } else {
                    None // Can't fit in u8, skip optimization
                }
            }),
            stat: StatKind::PopFused,
        },
        // N11: MakeSExpr 1; GetHead → remove (identity — build singleton then get its only element)
        PatternDef {
            bytes: leak(vec![exact(make_sexpr), exact(1), exact(get_head)]),
            postconditions: no_post,
            action: PatternAction::Remove,
            stat: StatKind::BuildDeconstructFolded,
        },
        // N12: StoreLocal X; LoadLocal X → Dup; StoreLocal X
        PatternDef {
            bytes: leak(vec![exact(store_local), ANY, exact(load_local), ANY]),
            postconditions: leak_pc(vec![PostCondition::BytesEqual(1, 3)]),
            action: PatternAction::ReplaceBytesWithCapture {
                template: &[0x02 /* Dup */, 0x31 /* StoreLocal */, 0],
                captures: &[(2, 1)], // copy slot byte
            },
            stat: StatKind::StoreLoadFolded,
        },
        // =====================================================================
        // Single-byte patterns (lowest priority)
        // =====================================================================

        // Nop → remove
        PatternDef {
            bytes: leak(vec![exact(nop)]),
            postconditions: no_post,
            action: PatternAction::Remove,
            stat: StatKind::NopsRemoved,
        },
    ]
}
