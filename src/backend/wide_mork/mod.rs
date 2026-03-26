//! Wide MORK: S-expression encoding with no arity limit.
//!
//! MORK's 6-bit arity encoding limits S-expressions to ≤63 children. When PLN
//! accumulates ≥64 beliefs at step 10, `(Unique <65-elem-tuple> ())` can't be
//! MORK-serialized for rule matching → expression goes unreduced → PLN.Query
//! returns empty.
//!
//! Wide MORK uses a tag-byte + LEB128-varint format that supports arbitrary
//! arity, variable counts, and symbol lengths:
//!
//! | Tag Byte | Meaning      | Followed By                                    |
//! |----------|-------------|------------------------------------------------|
//! | `0x00`   | `Arity`     | LEB128-encoded arity value (0..2^64)           |
//! | `0x01`   | `NewVar`    | nothing                                        |
//! | `0x02`   | `VarRef`    | LEB128-encoded De Bruijn index (0..2^64)       |
//! | `0x03`   | `SymbolSize`| LEB128-encoded byte count, then symbol bytes   |
//!
//! This encoding has the same structural semantics as MORK (tree encoding via
//! arity + children) but no upper limit on any field.
//!
//! ## Integration Points
//!
//! - **Rule storage**: Wide rules get `lhs_wide_debruijn` in `RuleEntry`, stored
//!   in PathMap via varint key (reusing existing fallback infrastructure).
//! - **Rule matching**: `wide_extract_data()` replaces MORK's `extract_data()`.
//! - **Binding extraction**: `extract_bindings_from_wide_expr()` walks wide De
//!   Bruijn bytes + MettaValue in lockstep (same algorithm as MORK version).

pub mod codec;
pub mod decode;
pub mod encoding;
pub mod extract;

#[cfg(test)]
mod tests;
