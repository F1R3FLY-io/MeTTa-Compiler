//! Varint-based binary key encoding for MettaValue
//!
//! This module provides binary key encoding for storing MettaValue expressions
//! in a PathMap without MORK's 63-arity limit. Uses varint encoding for lengths
//! and arities, allowing unlimited expression sizes.
//!
//! Key encoding format:
//! - Tag byte identifies the variant (0x01=SExpr, 0x02=Atom, etc.)
//! - Varints encode lengths with no upper limit
//! - Strings/bytes follow length-prefixed format

use crate::backend::models::MettaValue;
use std::sync::Arc;

/// Tag bytes for different MettaValue variants
mod tags {
    pub const SEXPR: u8 = 0x01;
    pub const ATOM: u8 = 0x02;
    pub const LONG: u8 = 0x03;
    pub const FLOAT: u8 = 0x04;
    pub const BOOL_TRUE: u8 = 0x05;
    pub const BOOL_FALSE: u8 = 0x06;
    pub const STRING: u8 = 0x07;
    pub const NIL: u8 = 0x08;
    pub const UNIT: u8 = 0x09;
    pub const ERROR: u8 = 0x0A;
    pub const TYPE: u8 = 0x0B;
    pub const CONJUNCTION: u8 = 0x0C;
    pub const SPACE: u8 = 0x0D;
    pub const STATE: u8 = 0x0E;
    pub const MEMO: u8 = 0x0F;
    pub const EMPTY: u8 = 0x10;
}

/// Encode MettaValue to binary key with varint arity (no 63 limit)
///
/// Returns a byte vector that uniquely identifies the MettaValue structure.
/// This key can be used as a PathMap key.
pub fn metta_to_varint_key(value: &MettaValue) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64);
    encode_metta(&mut buf, value);
    buf
}

/// Encode a MettaValue recursively into the buffer
fn encode_metta(buf: &mut Vec<u8>, value: &MettaValue) {
    match value {
        MettaValue::SExpr(items) => {
            buf.push(tags::SEXPR);
            encode_varint(buf, items.len() as u64); // No 63 limit!
            for item in items {
                encode_metta(buf, item);
            }
        }
        MettaValue::Atom(s) => {
            buf.push(tags::ATOM);
            encode_string(buf, s);
        }
        MettaValue::Long(n) => {
            buf.push(tags::LONG);
            buf.extend_from_slice(&n.to_le_bytes());
        }
        MettaValue::Float(f) => {
            buf.push(tags::FLOAT);
            buf.extend_from_slice(&f.to_le_bytes());
        }
        MettaValue::Bool(true) => {
            buf.push(tags::BOOL_TRUE);
        }
        MettaValue::Bool(false) => {
            buf.push(tags::BOOL_FALSE);
        }
        MettaValue::String(s) => {
            buf.push(tags::STRING);
            encode_string(buf, s);
        }
        MettaValue::Nil => {
            buf.push(tags::NIL);
        }
        MettaValue::Unit => {
            buf.push(tags::UNIT);
        }
        MettaValue::Error(msg, details) => {
            buf.push(tags::ERROR);
            encode_string(buf, msg);
            encode_metta(buf, details);
        }
        MettaValue::Type(inner) => {
            buf.push(tags::TYPE);
            encode_metta(buf, inner);
        }
        MettaValue::Conjunction(goals) => {
            buf.push(tags::CONJUNCTION);
            encode_varint(buf, goals.len() as u64);
            for goal in goals {
                encode_metta(buf, goal);
            }
        }
        MettaValue::Space(handle) => {
            // For spaces, encode the id and name as a proxy
            buf.push(tags::SPACE);
            encode_varint(buf, handle.id);
            encode_string(buf, &handle.name);
        }
        MettaValue::State(id) => {
            // For state cells, encode the id
            buf.push(tags::STATE);
            encode_varint(buf, *id);
        }
        MettaValue::Memo(handle) => {
            // For memo tables, encode the id and name
            buf.push(tags::MEMO);
            encode_varint(buf, handle.id);
            encode_string(buf, &handle.name);
        }
        MettaValue::Empty => {
            // Empty sentinel - simple tag byte
            buf.push(tags::EMPTY);
        }
    }
}

/// Encode a varint (variable-length integer) into the buffer
///
/// Uses 7 bits per byte, with high bit as continuation flag.
/// This encoding has no upper limit on the value.
fn encode_varint(buf: &mut Vec<u8>, mut n: u64) {
    while n >= 0x80 {
        buf.push((n as u8) | 0x80);
        n >>= 7;
    }
    buf.push(n as u8);
}

/// Encode a string as length-prefixed bytes
fn encode_string(buf: &mut Vec<u8>, s: &str) {
    encode_varint(buf, s.len() as u64);
    buf.extend_from_slice(s.as_bytes());
}

/// Decode a varint from a byte slice, returning (value, bytes_consumed)
pub fn decode_varint(bytes: &[u8]) -> Option<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift = 0;

    for (i, &byte) in bytes.iter().enumerate() {
        result |= ((byte & 0x7F) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some((result, i + 1));
        }
        shift += 7;
        if shift >= 64 {
            return None; // Overflow
        }
    }
    None // Incomplete
}

/// Decode a MettaValue from varint-encoded bytes
///
/// Returns the decoded value and the number of bytes consumed.
pub fn varint_key_to_metta(bytes: &[u8]) -> Option<(MettaValue, usize)> {
    if bytes.is_empty() {
        return None;
    }

    let tag = bytes[0];
    let mut offset = 1;

    match tag {
        tags::SEXPR => {
            let (arity, consumed) = decode_varint(&bytes[offset..])?;
            offset += consumed;

            let mut items = Vec::with_capacity(arity as usize);
            for _ in 0..arity {
                let (item, consumed) = varint_key_to_metta(&bytes[offset..])?;
                offset += consumed;
                items.push(item);
            }
            Some((MettaValue::SExpr(items), offset))
        }
        tags::ATOM => {
            let (s, consumed) = decode_string(&bytes[offset..])?;
            Some((MettaValue::Atom(s), offset + consumed))
        }
        tags::LONG => {
            if bytes.len() < offset + 8 {
                return None;
            }
            let n = i64::from_le_bytes(bytes[offset..offset + 8].try_into().ok()?);
            Some((MettaValue::Long(n), offset + 8))
        }
        tags::FLOAT => {
            if bytes.len() < offset + 8 {
                return None;
            }
            let f = f64::from_le_bytes(bytes[offset..offset + 8].try_into().ok()?);
            Some((MettaValue::Float(f), offset + 8))
        }
        tags::BOOL_TRUE => Some((MettaValue::Bool(true), offset)),
        tags::BOOL_FALSE => Some((MettaValue::Bool(false), offset)),
        tags::STRING => {
            let (s, consumed) = decode_string(&bytes[offset..])?;
            Some((MettaValue::String(s), offset + consumed))
        }
        tags::NIL => Some((MettaValue::Nil, offset)),
        tags::UNIT => Some((MettaValue::Unit, offset)),
        tags::ERROR => {
            let (msg, consumed1) = decode_string(&bytes[offset..])?;
            offset += consumed1;
            let (details, consumed2) = varint_key_to_metta(&bytes[offset..])?;
            Some((
                MettaValue::Error(msg, Arc::new(details)),
                offset + consumed2,
            ))
        }
        tags::TYPE => {
            let (inner, consumed) = varint_key_to_metta(&bytes[offset..])?;
            Some((MettaValue::Type(Arc::new(inner)), offset + consumed))
        }
        tags::CONJUNCTION => {
            let (count, consumed) = decode_varint(&bytes[offset..])?;
            offset += consumed;

            let mut goals = Vec::with_capacity(count as usize);
            for _ in 0..count {
                let (goal, consumed) = varint_key_to_metta(&bytes[offset..])?;
                offset += consumed;
                goals.push(goal);
            }
            Some((MettaValue::Conjunction(goals), offset))
        }
        tags::SPACE => {
            // Spaces can't be fully reconstructed from bytes - return a placeholder
            let (_id, consumed1) = decode_varint(&bytes[offset..])?;
            offset += consumed1;
            let (name, consumed2) = decode_string(&bytes[offset..])?;
            // Return as an atom representing the space reference
            Some((MettaValue::Atom(format!("&{}", name)), offset + consumed2))
        }
        tags::STATE => {
            // State cells can't be fully reconstructed - return as atom
            let (id, consumed) = decode_varint(&bytes[offset..])?;
            Some((MettaValue::Atom(format!("state:{}", id)), offset + consumed))
        }
        tags::MEMO => {
            // Memos can't be fully reconstructed - return as atom
            let (_id, consumed1) = decode_varint(&bytes[offset..])?;
            offset += consumed1;
            let (name, consumed2) = decode_string(&bytes[offset..])?;
            Some((
                MettaValue::Atom(format!("memo:{}", name)),
                offset + consumed2,
            ))
        }
        tags::EMPTY => Some((MettaValue::Empty, offset)),
        _ => None, // Unknown tag
    }
}

/// Decode a string from length-prefixed bytes
fn decode_string(bytes: &[u8]) -> Option<(String, usize)> {
    let (len, consumed) = decode_varint(bytes)?;
    let start = consumed;
    let end = start + len as usize;

    if bytes.len() < end {
        return None;
    }

    let s = String::from_utf8(bytes[start..end].to_vec()).ok()?;
    Some((s, end))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_varint_encoding() {
        let mut buf = Vec::new();

        // Small number
        encode_varint(&mut buf, 42);
        assert_eq!(buf, vec![42]);

        // Number requiring 2 bytes
        buf.clear();
        encode_varint(&mut buf, 128);
        assert_eq!(buf, vec![0x80, 0x01]);

        // Large number
        buf.clear();
        encode_varint(&mut buf, 0x3FFF);
        assert_eq!(buf, vec![0xFF, 0x7F]);
    }

    #[test]
    fn test_varint_roundtrip() {
        for n in [0, 1, 63, 64, 127, 128, 255, 256, 0xFFFF, 0xFFFFFF, u64::MAX] {
            let mut buf = Vec::new();
            encode_varint(&mut buf, n);
            let (decoded, _) = decode_varint(&buf).unwrap();
            assert_eq!(n, decoded, "Failed for {}", n);
        }
    }

    #[test]
    #[allow(clippy::approx_constant)]
    fn test_simple_values_roundtrip() {
        let test_cases = vec![
            MettaValue::Long(42),
            MettaValue::Long(-1),
            MettaValue::Bool(true),
            MettaValue::Bool(false),
            MettaValue::Nil,
            MettaValue::Unit,
            MettaValue::Atom("hello".to_string()),
            MettaValue::String("world".to_string()),
            MettaValue::Float(3.14),
        ];

        for value in test_cases {
            let key = metta_to_varint_key(&value);
            let (decoded, consumed) = varint_key_to_metta(&key).unwrap();
            assert_eq!(consumed, key.len());
            assert_eq!(value, decoded, "Failed for {:?}", value);
        }
    }

    #[test]
    fn test_sexpr_roundtrip() {
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);

        let key = metta_to_varint_key(&sexpr);
        let (decoded, consumed) = varint_key_to_metta(&key).unwrap();
        assert_eq!(consumed, key.len());
        assert_eq!(sexpr, decoded);
    }

    #[test]
    fn test_large_arity_sexpr() {
        // Create an S-expression with 100 children (exceeds MORK's 63 limit)
        let items: Vec<MettaValue> = (0..100).map(|i| MettaValue::Long(i)).collect();
        let large_sexpr = MettaValue::SExpr(items);

        let key = metta_to_varint_key(&large_sexpr);
        let (decoded, consumed) = varint_key_to_metta(&key).unwrap();
        assert_eq!(consumed, key.len());
        assert_eq!(large_sexpr, decoded);
    }

    #[test]
    fn test_nested_sexpr() {
        let nested = MettaValue::SExpr(vec![
            MettaValue::Atom("outer".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("inner".to_string()),
                MettaValue::Long(42),
            ]),
        ]);

        let key = metta_to_varint_key(&nested);
        let (decoded, consumed) = varint_key_to_metta(&key).unwrap();
        assert_eq!(consumed, key.len());
        assert_eq!(nested, decoded);
    }

    #[test]
    fn test_error_value() {
        let error = MettaValue::Error("test error".to_string(), Arc::new(MettaValue::Long(42)));

        let key = metta_to_varint_key(&error);
        let (decoded, consumed) = varint_key_to_metta(&key).unwrap();
        assert_eq!(consumed, key.len());
        assert_eq!(error, decoded);
    }
}
