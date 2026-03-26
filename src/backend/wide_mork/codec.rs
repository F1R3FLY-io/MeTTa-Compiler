//! `MorkCodec` trait — unified interface for MORK-family byte encodings.
//!
//! Both standard MORK (6-bit tags, arity ≤ 63) and Wide MORK (byte tags + LEB128,
//! unlimited arity) follow identical operational semantics.  This trait abstracts
//! over the encoding format so generic helpers (`add_atom_to`, `get_all_atoms_from`,
//! etc.) can be written once and dispatch to the appropriate encoder/decoder.

use crate::backend::models::{MettaValueFactory, MettaValueTrait};

use super::decode::wide_bytes_to_generic_value;
use super::encoding::{encode_wide_storage, wide_expr_byte_len};

/// Unified interface for MORK-family byte encodings.
///
/// Standard MORK (6-bit tags, arity ≤ 63) and Wide MORK (byte tags + LEB128,
/// unlimited arity).  Implementors provide encode/decode/length primitives;
/// generic functions operate on `PathMap<Multiplicity>` regardless of encoding.
pub trait MorkCodec {
    /// Encode a value to storage bytes (no De Bruijn — variables written as literal symbols).
    fn encode_storage<V: MettaValueTrait>(value: &V, buf: &mut Vec<u8>);

    /// Decode storage bytes back to a typed MettaValue (heuristic type detection).
    fn decode_storage<V, F>(bytes: &[u8], factory: &F) -> Result<V, String>
    where
        V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
        F: MettaValueFactory<V>;

    /// Compute byte length of one expression at the start of `bytes`.
    fn expr_byte_len(bytes: &[u8]) -> usize;
}

/// Wide MORK codec implementation (byte tags + LEB128, unlimited arity).
pub struct WideMorkCodec;

impl MorkCodec for WideMorkCodec {
    #[inline]
    fn encode_storage<V: MettaValueTrait>(value: &V, buf: &mut Vec<u8>) {
        encode_wide_storage(value, buf);
    }

    #[inline]
    fn decode_storage<V, F>(bytes: &[u8], factory: &F) -> Result<V, String>
    where
        V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
        F: MettaValueFactory<V>,
    {
        wide_bytes_to_generic_value(bytes, factory)
    }

    #[inline]
    fn expr_byte_len(bytes: &[u8]) -> usize {
        wide_expr_byte_len(bytes)
    }
}
