//! Unit tests for Wide MORK encoding, extraction, and binding.

use super::decode::{wide_bytes_to_generic_value, wide_debruijn_to_generic_value};
use super::encoding::*;
use super::extract::*;

// ============================================================================
// LEB128 round-trip tests
// ============================================================================

#[test]
fn test_leb128_roundtrip_small() {
    for n in 0..=127u64 {
        let mut buf = Vec::new();
        encode_leb128(&mut buf, n);
        let (decoded, consumed) = decode_leb128(&buf).expect("decode failed");
        assert_eq!(decoded, n, "mismatch for {n}");
        assert_eq!(consumed, buf.len(), "consumed mismatch for {n}");
    }
}

#[test]
fn test_leb128_roundtrip_large() {
    let values = [128, 255, 256, 1000, 0x3FFF, 0xFFFF, 0xFFFFFF, 0xFFFFFFFF, u64::MAX];
    for n in values {
        let mut buf = Vec::new();
        encode_leb128(&mut buf, n);
        let (decoded, consumed) = decode_leb128(&buf).expect("decode failed");
        assert_eq!(decoded, n, "mismatch for {n}");
        assert_eq!(consumed, buf.len(), "consumed mismatch for {n}");
    }
}

#[test]
fn test_leb128_encoding_sizes() {
    // 0 -> 1 byte
    let mut buf = Vec::new();
    encode_leb128(&mut buf, 0);
    assert_eq!(buf.len(), 1);
    assert_eq!(buf[0], 0x00);

    // 63 -> 1 byte
    buf.clear();
    encode_leb128(&mut buf, 63);
    assert_eq!(buf.len(), 1);
    assert_eq!(buf[0], 63);

    // 64 -> 1 byte (still fits in 7 bits)
    buf.clear();
    encode_leb128(&mut buf, 64);
    assert_eq!(buf.len(), 1);
    assert_eq!(buf[0], 64);

    // 127 -> 1 byte
    buf.clear();
    encode_leb128(&mut buf, 127);
    assert_eq!(buf.len(), 1);
    assert_eq!(buf[0], 127);

    // 128 -> 2 bytes
    buf.clear();
    encode_leb128(&mut buf, 128);
    assert_eq!(buf.len(), 2);
    assert_eq!(buf, vec![0x80, 0x01]);

    // 65 -> 1 byte (0x41)
    buf.clear();
    encode_leb128(&mut buf, 65);
    assert_eq!(buf.len(), 1);
    assert_eq!(buf[0], 0x41);
}

// ============================================================================
// WideTag tests
// ============================================================================

#[test]
fn test_wide_tag_from_byte() {
    assert_eq!(WideTag::from_byte(0x00), Ok(WideTag::Arity));
    assert_eq!(WideTag::from_byte(0x01), Ok(WideTag::NewVar));
    assert_eq!(WideTag::from_byte(0x02), Ok(WideTag::VarRef));
    assert_eq!(WideTag::from_byte(0x03), Ok(WideTag::SymbolSize));
    assert_eq!(WideTag::from_byte(0x04), Err(0x04));
    assert_eq!(WideTag::from_byte(0xFF), Err(0xFF));
}

// ============================================================================
// Storage encoding tests
// ============================================================================

#[test]
fn test_encode_wide_storage_atom() {
    use crate::backend::models::MettaValue;

    let val = MettaValue::Atom("hello".to_string());
    let mut buf = Vec::new();
    encode_wide_storage(&val, &mut buf);

    // Expected: [TAG_SYMBOL_SIZE][LEB128(5)]["hello"]
    assert_eq!(buf[0], TAG_SYMBOL_SIZE);
    let (size, consumed) = decode_leb128(&buf[1..]).expect("decode size");
    assert_eq!(size, 5);
    assert_eq!(&buf[1 + consumed..], b"hello");
}

#[test]
fn test_encode_wide_storage_unit() {
    use crate::backend::models::MettaValue;

    let val = MettaValue::Unit();
    let mut buf = Vec::new();
    encode_wide_storage(&val, &mut buf);

    // Expected: [TAG_ARITY][LEB128(0)]
    assert_eq!(buf[0], TAG_ARITY);
    let (arity, _) = decode_leb128(&buf[1..]).expect("decode arity");
    assert_eq!(arity, 0);
}

#[test]
fn test_encode_wide_storage_sexpr() {
    use crate::backend::models::MettaValue;

    let val = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]);
    let mut buf = Vec::new();
    encode_wide_storage(&val, &mut buf);

    // Should start with Arity(3)
    assert_eq!(buf[0], TAG_ARITY);
    let (arity, _) = decode_leb128(&buf[1..]).expect("decode arity");
    assert_eq!(arity, 3);
}

#[test]
fn test_encode_wide_storage_large_sexpr() {
    use crate::backend::models::MettaValue;

    // Create an S-expression with 100 children (exceeds MORK's 63 limit)
    let items: Vec<MettaValue> = (0..100).map(|i| MettaValue::Long(i)).collect();
    let val = MettaValue::SExpr(items);
    let mut buf = Vec::new();
    encode_wide_storage(&val, &mut buf);

    // Should encode arity=100 just fine
    assert_eq!(buf[0], TAG_ARITY);
    let (arity, _) = decode_leb128(&buf[1..]).expect("decode arity");
    assert_eq!(arity, 100);
}

// ============================================================================
// De Bruijn encoding tests
// ============================================================================

#[test]
fn test_encode_wide_debruijn_variable() {
    use crate::backend::models::MettaValue;

    let val = MettaValue::Atom("$x".to_string());
    let mut ctx = WideConversionContext::new();
    let mut buf = Vec::new();
    encode_wide_debruijn(&val, &mut ctx, &mut buf);

    // First occurrence of $x → NewVar
    assert_eq!(buf, vec![TAG_NEWVAR]);
    assert_eq!(ctx.var_names, vec!["$x"]);
}

#[test]
fn test_encode_wide_debruijn_repeated_variable() {
    use crate::backend::models::MettaValue;

    let val = MettaValue::SExpr(vec![
        MettaValue::Atom("f".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Atom("$x".to_string()),
    ]);
    let mut ctx = WideConversionContext::new();
    let mut buf = Vec::new();
    encode_wide_debruijn(&val, &mut ctx, &mut buf);

    // Arity(3), Symbol("f"), NewVar, VarRef(0)
    let mut expected = Vec::new();
    expected.push(TAG_ARITY);
    encode_leb128(&mut expected, 3);
    expected.push(TAG_SYMBOL_SIZE);
    encode_leb128(&mut expected, 1);
    expected.push(b'f');
    expected.push(TAG_NEWVAR);
    expected.push(TAG_VARREF);
    encode_leb128(&mut expected, 0);

    assert_eq!(buf, expected);
}

#[test]
fn test_encode_wide_debruijn_wildcard() {
    use crate::backend::models::MettaValue;

    let val = MettaValue::Atom("_".to_string());
    let mut ctx = WideConversionContext::new();
    let mut buf = Vec::new();
    encode_wide_debruijn(&val, &mut ctx, &mut buf);

    // Wildcard → NewVar (anonymous, not tracked in var_names the same way
    // as named variables, but the ctx still doesn't add it)
    assert_eq!(buf, vec![TAG_NEWVAR]);
    // Wildcards don't get added to the conversion context's var_map
    assert!(ctx.var_names.is_empty());
}

#[test]
fn test_encode_wide_debruijn_large_sexpr() {
    use crate::backend::models::MettaValue;

    // 65-element S-expr with a variable
    let mut items: Vec<MettaValue> = Vec::with_capacity(65);
    items.push(MettaValue::Atom("Unique".to_string()));
    for i in 0..63 {
        items.push(MettaValue::Long(i));
    }
    items.push(MettaValue::Atom("$result".to_string()));

    let val = MettaValue::SExpr(items);
    let mut ctx = WideConversionContext::new();
    let mut buf = Vec::new();
    encode_wide_debruijn(&val, &mut ctx, &mut buf);

    // Should encode with arity=65
    assert_eq!(buf[0], TAG_ARITY);
    let (arity, _) = decode_leb128(&buf[1..]).expect("decode arity");
    assert_eq!(arity, 65);

    // And $result should be a NewVar
    assert_eq!(ctx.var_names, vec!["$result"]);
}

// ============================================================================
// wide_expr_byte_len tests
// ============================================================================

#[test]
fn test_wide_expr_byte_len_symbol() {
    let mut buf = Vec::new();
    buf.push(TAG_SYMBOL_SIZE);
    encode_leb128(&mut buf, 5);
    buf.extend_from_slice(b"hello");

    assert_eq!(wide_expr_byte_len(&buf), buf.len());
}

#[test]
fn test_wide_expr_byte_len_newvar() {
    let buf = vec![TAG_NEWVAR];
    assert_eq!(wide_expr_byte_len(&buf), 1);
}

#[test]
fn test_wide_expr_byte_len_varref() {
    let mut buf = Vec::new();
    buf.push(TAG_VARREF);
    encode_leb128(&mut buf, 42);
    assert_eq!(wide_expr_byte_len(&buf), buf.len());
}

#[test]
fn test_wide_expr_byte_len_arity_zero() {
    let mut buf = Vec::new();
    buf.push(TAG_ARITY);
    encode_leb128(&mut buf, 0);
    assert_eq!(wide_expr_byte_len(&buf), buf.len());
}

#[test]
fn test_wide_expr_byte_len_nested() {
    // (f x) → Arity(2), Symbol("f"), Symbol("x")
    let mut buf = Vec::new();
    buf.push(TAG_ARITY);
    encode_leb128(&mut buf, 2);
    buf.push(TAG_SYMBOL_SIZE);
    encode_leb128(&mut buf, 1);
    buf.push(b'f');
    buf.push(TAG_SYMBOL_SIZE);
    encode_leb128(&mut buf, 1);
    buf.push(b'x');

    assert_eq!(wide_expr_byte_len(&buf), buf.len());
}

#[test]
fn test_wide_expr_byte_len_with_trailing_data() {
    // An element followed by extra bytes should only measure the first element
    let mut buf = Vec::new();
    buf.push(TAG_SYMBOL_SIZE);
    encode_leb128(&mut buf, 3);
    buf.extend_from_slice(b"abc");
    let expected_len = buf.len();
    buf.extend_from_slice(b"EXTRA_JUNK"); // should not be counted

    assert_eq!(wide_expr_byte_len(&buf), expected_len);
}

// ============================================================================
// count_wide_newvar_tags tests
// ============================================================================

#[test]
fn test_count_wide_newvar_tags() {
    // Arity(2), NewVar, NewVar
    let mut buf = Vec::new();
    buf.push(TAG_ARITY);
    encode_leb128(&mut buf, 2);
    buf.push(TAG_NEWVAR);
    buf.push(TAG_NEWVAR);

    assert_eq!(count_wide_newvar_tags(&buf), 2);
}

#[test]
fn test_count_wide_newvar_tags_with_varref() {
    // Arity(3), NewVar, VarRef(0), NewVar
    let mut buf = Vec::new();
    buf.push(TAG_ARITY);
    encode_leb128(&mut buf, 3);
    buf.push(TAG_NEWVAR);
    buf.push(TAG_VARREF);
    encode_leb128(&mut buf, 0);
    buf.push(TAG_NEWVAR);

    assert_eq!(count_wide_newvar_tags(&buf), 2);
}

// ============================================================================
// wide_extract_data tests
// ============================================================================

#[test]
fn test_extract_newvar_symbol() {
    // Template: NewVar
    let template = vec![TAG_NEWVAR];

    // Data: Symbol("hello")
    let mut data = Vec::new();
    data.push(TAG_SYMBOL_SIZE);
    encode_leb128(&mut data, 5);
    data.extend_from_slice(b"hello");

    let result = wide_extract_data(&template, &data).expect("extract should succeed");
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].offset, 0);
    assert_eq!(result[0].len, data.len());
}

#[test]
fn test_extract_newvar_compound() {
    // Template: NewVar
    let template = vec![TAG_NEWVAR];

    // Data: (f x) → Arity(2), Symbol("f"), Symbol("x")
    let mut data = Vec::new();
    data.push(TAG_ARITY);
    encode_leb128(&mut data, 2);
    data.push(TAG_SYMBOL_SIZE);
    encode_leb128(&mut data, 1);
    data.push(b'f');
    data.push(TAG_SYMBOL_SIZE);
    encode_leb128(&mut data, 1);
    data.push(b'x');

    let result = wide_extract_data(&template, &data).expect("extract should succeed");
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].offset, 0);
    assert_eq!(result[0].len, data.len());
}

#[test]
fn test_extract_symbol_match() {
    // Template: Symbol("hello")
    let mut template = Vec::new();
    template.push(TAG_SYMBOL_SIZE);
    encode_leb128(&mut template, 5);
    template.extend_from_slice(b"hello");

    // Data: Symbol("hello")
    let data = template.clone();

    let result = wide_extract_data(&template, &data).expect("extract should succeed");
    assert!(result.is_empty()); // No bindings, just a match
}

#[test]
fn test_extract_symbol_mismatch() {
    // Template: Symbol("hello")
    let mut template = Vec::new();
    template.push(TAG_SYMBOL_SIZE);
    encode_leb128(&mut template, 5);
    template.extend_from_slice(b"hello");

    // Data: Symbol("world")
    let mut data = Vec::new();
    data.push(TAG_SYMBOL_SIZE);
    encode_leb128(&mut data, 5);
    data.extend_from_slice(b"world");

    let result = wide_extract_data(&template, &data);
    assert!(result.is_err());
}

#[test]
fn test_extract_arity_match() {
    // Template: (f $x) → Arity(2), Symbol("f"), NewVar
    let mut template = Vec::new();
    template.push(TAG_ARITY);
    encode_leb128(&mut template, 2);
    template.push(TAG_SYMBOL_SIZE);
    encode_leb128(&mut template, 1);
    template.push(b'f');
    template.push(TAG_NEWVAR);

    // Data: (f hello) → Arity(2), Symbol("f"), Symbol("hello")
    let mut data = Vec::new();
    data.push(TAG_ARITY);
    encode_leb128(&mut data, 2);
    data.push(TAG_SYMBOL_SIZE);
    encode_leb128(&mut data, 1);
    data.push(b'f');
    data.push(TAG_SYMBOL_SIZE);
    encode_leb128(&mut data, 5);
    data.extend_from_slice(b"hello");

    let result = wide_extract_data(&template, &data).expect("extract should succeed");
    assert_eq!(result.len(), 1);
    // The binding should capture "hello" symbol
    let binding = &result[0];
    assert_eq!(&data[binding.offset..binding.offset + binding.len],
               &[TAG_SYMBOL_SIZE, 5, b'h', b'e', b'l', b'l', b'o']);
}

#[test]
fn test_extract_arity_mismatch() {
    // Template: Arity(2) ...
    let mut template = Vec::new();
    template.push(TAG_ARITY);
    encode_leb128(&mut template, 2);
    template.push(TAG_NEWVAR);
    template.push(TAG_NEWVAR);

    // Data: Arity(3) ...
    let mut data = Vec::new();
    data.push(TAG_ARITY);
    encode_leb128(&mut data, 3);
    data.push(TAG_SYMBOL_SIZE);
    encode_leb128(&mut data, 1);
    data.push(b'a');
    data.push(TAG_SYMBOL_SIZE);
    encode_leb128(&mut data, 1);
    data.push(b'b');
    data.push(TAG_SYMBOL_SIZE);
    encode_leb128(&mut data, 1);
    data.push(b'c');

    let result = wide_extract_data(&template, &data);
    assert!(result.is_err());
}

#[test]
fn test_extract_varref_match() {
    // Template: (f $x $x) → Arity(3), Symbol("f"), NewVar, VarRef(0)
    let mut template = Vec::new();
    template.push(TAG_ARITY);
    encode_leb128(&mut template, 3);
    template.push(TAG_SYMBOL_SIZE);
    encode_leb128(&mut template, 1);
    template.push(b'f');
    template.push(TAG_NEWVAR);
    template.push(TAG_VARREF);
    encode_leb128(&mut template, 0);

    // Data: (f hello hello) → Arity(3), Symbol("f"), Symbol("hello"), Symbol("hello")
    let mut data = Vec::new();
    data.push(TAG_ARITY);
    encode_leb128(&mut data, 3);
    data.push(TAG_SYMBOL_SIZE);
    encode_leb128(&mut data, 1);
    data.push(b'f');
    data.push(TAG_SYMBOL_SIZE);
    encode_leb128(&mut data, 5);
    data.extend_from_slice(b"hello");
    data.push(TAG_SYMBOL_SIZE);
    encode_leb128(&mut data, 5);
    data.extend_from_slice(b"hello");

    let result = wide_extract_data(&template, &data).expect("extract should succeed");
    assert_eq!(result.len(), 1); // One NewVar captured
}

#[test]
fn test_extract_varref_mismatch() {
    // Template: (f $x $x) → Arity(3), Symbol("f"), NewVar, VarRef(0)
    let mut template = Vec::new();
    template.push(TAG_ARITY);
    encode_leb128(&mut template, 3);
    template.push(TAG_SYMBOL_SIZE);
    encode_leb128(&mut template, 1);
    template.push(b'f');
    template.push(TAG_NEWVAR);
    template.push(TAG_VARREF);
    encode_leb128(&mut template, 0);

    // Data: (f hello world) — different values at VarRef position
    let mut data = Vec::new();
    data.push(TAG_ARITY);
    encode_leb128(&mut data, 3);
    data.push(TAG_SYMBOL_SIZE);
    encode_leb128(&mut data, 1);
    data.push(b'f');
    data.push(TAG_SYMBOL_SIZE);
    encode_leb128(&mut data, 5);
    data.extend_from_slice(b"hello");
    data.push(TAG_SYMBOL_SIZE);
    encode_leb128(&mut data, 5);
    data.extend_from_slice(b"world");

    let result = wide_extract_data(&template, &data);
    assert!(matches!(result, Err(WideExtractFailure::VarRefMismatch { var_idx: 0 })));
}

#[test]
fn test_extract_large_arity() {
    // Template: Arity(65) with 65 NewVars
    let mut template = Vec::new();
    template.push(TAG_ARITY);
    encode_leb128(&mut template, 65);
    for _ in 0..65 {
        template.push(TAG_NEWVAR);
    }

    // Data: Arity(65) with 65 symbols
    let mut data = Vec::new();
    data.push(TAG_ARITY);
    encode_leb128(&mut data, 65);
    for i in 0..65u8 {
        data.push(TAG_SYMBOL_SIZE);
        encode_leb128(&mut data, 1);
        data.push(b'a' + (i % 26));
    }

    let result = wide_extract_data(&template, &data).expect("extract should succeed");
    assert_eq!(result.len(), 65);
}

#[test]
fn test_extract_nested_compound_binding() {
    // Template: ($x) → Arity(1), NewVar
    let mut template = Vec::new();
    template.push(TAG_ARITY);
    encode_leb128(&mut template, 1);
    template.push(TAG_NEWVAR);

    // Data: ((a b)) → Arity(1), Arity(2), Symbol("a"), Symbol("b")
    let mut data = Vec::new();
    data.push(TAG_ARITY);
    encode_leb128(&mut data, 1);
    // Inner: (a b)
    data.push(TAG_ARITY);
    encode_leb128(&mut data, 2);
    data.push(TAG_SYMBOL_SIZE);
    encode_leb128(&mut data, 1);
    data.push(b'a');
    data.push(TAG_SYMBOL_SIZE);
    encode_leb128(&mut data, 1);
    data.push(b'b');

    let result = wide_extract_data(&template, &data).expect("extract should succeed");
    assert_eq!(result.len(), 1);
    // The binding should capture the entire inner (a b) sub-expression
    let inner_start = 2; // After Arity(1) tag+LEB128
    let binding = &result[0];
    assert_eq!(binding.offset, inner_start);
    // The captured bytes should be the entire (a b) sub-expression
    let inner_bytes = &data[binding.offset..binding.offset + binding.len];
    assert_eq!(inner_bytes[0], TAG_ARITY);
}

#[test]
fn test_extract_data_contains_variables() {
    // Template: Symbol("f")
    let mut template = Vec::new();
    template.push(TAG_SYMBOL_SIZE);
    encode_leb128(&mut template, 1);
    template.push(b'f');

    // Data: NewVar — invalid! Data must not contain variables
    let data = vec![TAG_NEWVAR];

    let result = wide_extract_data(&template, &data);
    assert!(matches!(result, Err(WideExtractFailure::DataContainsVariables)));
}

// ============================================================================
// Decode round-trip tests (encode_wide_storage → wide_bytes_to_generic_value)
// ============================================================================

use crate::backend::models::{global_factory, MettaValue, MettaValueInner, GcFactory};

/// Helper: encode a value and decode it, returning the decoded value.
fn roundtrip(val: &MettaValue) -> MettaValue {
    let mut buf = Vec::new();
    encode_wide_storage(val, &mut buf);
    wide_bytes_to_generic_value::<MettaValue, GcFactory>(&buf, &global_factory())
        .expect("decode should succeed")
}

/// Helper to extract atom name from decoded value.
fn assert_atom(val: &MettaValue, expected: &str) {
    match val.inner() {
        MettaValueInner::Atom(s) => assert_eq!(*s, expected),
        other => panic!("Expected Atom({expected}), got {:?}", other),
    }
}

/// Helper to extract Long from decoded value.
fn assert_long(val: &MettaValue, expected: i64) {
    match val.inner() {
        MettaValueInner::Long(n) => assert_eq!(*n, expected),
        other => panic!("Expected Long({expected}), got {:?}", other),
    }
}

#[test]
fn test_wide_decode_atom() {
    let val = MettaValue::Atom("foo".to_string());
    let decoded = roundtrip(&val);
    assert_atom(&decoded, "foo");
}

#[test]
fn test_wide_decode_long() {
    for n in [0i64, 1, -1, 42, -7, i64::MAX, i64::MIN] {
        let val = MettaValue::Long(n);
        let decoded = roundtrip(&val);
        assert_long(&decoded, n);
    }
}

#[test]
fn test_wide_decode_float() {
    for f in [0.0f64, 1.5, -3.14, f64::MAX, f64::MIN_POSITIVE] {
        let val = MettaValue::Float(f);
        let decoded = roundtrip(&val);
        match decoded.inner() {
            MettaValueInner::Float(v) => assert_eq!(*v, f, "Float({f}) round-trip failed"),
            other => panic!("Expected Float({f}), got {:?}", other),
        }
    }
}

#[test]
fn test_wide_decode_bool() {
    let t = MettaValue::Bool(true);
    let decoded_t = roundtrip(&t);
    assert!(matches!(decoded_t.inner(), MettaValueInner::Bool(true)));

    let f = MettaValue::Bool(false);
    let decoded_f = roundtrip(&f);
    assert!(matches!(decoded_f.inner(), MettaValueInner::Bool(false)));
}

#[test]
fn test_wide_decode_string() {
    let val = MettaValue::String("hello world".to_string());
    let decoded = roundtrip(&val);
    match decoded.inner() {
        MettaValueInner::String(s) => assert_eq!(*s, "hello world"),
        other => panic!("Expected String, got {:?}", other),
    }
}

#[test]
fn test_wide_decode_unit() {
    let val = MettaValue::Unit();
    let decoded = roundtrip(&val);
    assert!(decoded.is_unit());
}

#[test]
fn test_wide_decode_sexpr_small() {
    // (+ 1 2) — arity 3
    let val = MettaValue::SExpr(vec![
        MettaValue::Atom("+".to_string()),
        MettaValue::Long(1),
        MettaValue::Long(2),
    ]);
    let decoded = roundtrip(&val);
    match decoded.inner() {
        MettaValueInner::SExpr(items) => {
            assert_eq!(items.len(), 3);
            assert_atom(&items[0], "+");
            assert_long(&items[1], 1);
            assert_long(&items[2], 2);
        }
        other => panic!("Expected SExpr, got {:?}", other),
    }
}

#[test]
fn test_wide_decode_sexpr_nested() {
    // (f (g x) (h y z))
    let val = MettaValue::SExpr(vec![
        MettaValue::Atom("f".to_string()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("g".to_string()),
            MettaValue::Atom("x".to_string()),
        ]),
        MettaValue::SExpr(vec![
            MettaValue::Atom("h".to_string()),
            MettaValue::Atom("y".to_string()),
            MettaValue::Atom("z".to_string()),
        ]),
    ]);
    let decoded = roundtrip(&val);
    match decoded.inner() {
        MettaValueInner::SExpr(items) => {
            assert_eq!(items.len(), 3);
            assert_atom(&items[0], "f");
            match items[1].inner() {
                MettaValueInner::SExpr(inner1) => {
                    assert_eq!(inner1.len(), 2);
                    assert_atom(&inner1[0], "g");
                    assert_atom(&inner1[1], "x");
                }
                other => panic!("Expected SExpr for inner1, got {:?}", other),
            }
            match items[2].inner() {
                MettaValueInner::SExpr(inner2) => {
                    assert_eq!(inner2.len(), 3);
                    assert_atom(&inner2[0], "h");
                }
                other => panic!("Expected SExpr for inner2, got {:?}", other),
            }
        }
        other => panic!("Expected SExpr, got {:?}", other),
    }
}

#[test]
fn test_wide_decode_sexpr_boundary_63() {
    // Arity 63 (MORK max)
    let items: Vec<MettaValue> = (0..63).map(|i| MettaValue::Long(i)).collect();
    let val = MettaValue::SExpr(items);
    let decoded = roundtrip(&val);
    match decoded.inner() {
        MettaValueInner::SExpr(dec_items) => {
            assert_eq!(dec_items.len(), 63);
            for (i, item) in dec_items.iter().enumerate() {
                assert_long(item, i as i64);
            }
        }
        other => panic!("Expected SExpr, got {:?}", other),
    }
}

#[test]
fn test_wide_decode_sexpr_boundary_64() {
    // Arity 64 (first Wide-only)
    let items: Vec<MettaValue> = (0..64).map(|i| MettaValue::Long(i)).collect();
    let val = MettaValue::SExpr(items);
    let decoded = roundtrip(&val);
    match decoded.inner() {
        MettaValueInner::SExpr(dec_items) => {
            assert_eq!(dec_items.len(), 64);
            for (i, item) in dec_items.iter().enumerate() {
                assert_long(item, i as i64);
            }
        }
        other => panic!("Expected SExpr, got {:?}", other),
    }
}

#[test]
fn test_wide_decode_sexpr_large_100() {
    let items: Vec<MettaValue> = (0..100).map(|i| MettaValue::Long(i)).collect();
    let val = MettaValue::SExpr(items);
    let decoded = roundtrip(&val);
    match decoded.inner() {
        MettaValueInner::SExpr(dec_items) => assert_eq!(dec_items.len(), 100),
        other => panic!("Expected SExpr, got {:?}", other),
    }
}

#[test]
fn test_wide_decode_sexpr_large_1000() {
    let items: Vec<MettaValue> = (0..1000).map(|i| MettaValue::Long(i)).collect();
    let val = MettaValue::SExpr(items);
    let decoded = roundtrip(&val);
    match decoded.inner() {
        MettaValueInner::SExpr(dec_items) => {
            assert_eq!(dec_items.len(), 1000);
            assert_long(&dec_items[0], 0);
            assert_long(&dec_items[999], 999);
        }
        other => panic!("Expected SExpr, got {:?}", other),
    }
}

#[test]
fn test_wide_decode_debruijn_variables() {
    // Test De Bruijn variable decoding: (f $x $x $y)
    let val = MettaValue::SExpr(vec![
        MettaValue::Atom("f".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Atom("$x".to_string()),
        MettaValue::Atom("$y".to_string()),
    ]);
    let mut ctx = WideConversionContext::new();
    let mut buf = Vec::new();
    encode_wide_debruijn(&val, &mut ctx, &mut buf);

    // Decode De Bruijn bytes — variables get epoch-suffixed names
    let decoded = wide_debruijn_to_generic_value::<MettaValue, GcFactory>(&buf, &global_factory())
        .expect("decode should succeed");

    match decoded.inner() {
        MettaValueInner::SExpr(items) => {
            assert_eq!(items.len(), 4);
            assert_atom(&items[0], "f");
            // $x appears twice with same name, $y is different
            let var_x = match items[1].inner() {
                MettaValueInner::Atom(s) => *s,
                other => panic!("Expected Atom, got {:?}", other),
            };
            let var_x2 = match items[2].inner() {
                MettaValueInner::Atom(s) => *s,
                other => panic!("Expected Atom, got {:?}", other),
            };
            let var_y = match items[3].inner() {
                MettaValueInner::Atom(s) => *s,
                other => panic!("Expected Atom, got {:?}", other),
            };
            assert_eq!(var_x, var_x2, "VarRef should produce same name as NewVar");
            assert_ne!(var_x, var_y, "Different variables should have different names");
            assert!(var_x.starts_with('$'), "Variable should start with $");
            assert!(var_y.starts_with('$'), "Variable should start with $");
        }
        other => panic!("Expected SExpr, got {:?}", other),
    }
}

#[test]
fn test_wide_decode_empty_marker() {
    // %Empty% symbol round-trips as empty()
    let val = MettaValue::Empty();
    let decoded = roundtrip(&val);
    assert!(decoded.is_empty());
}

#[test]
fn test_wide_decode_mixed_types() {
    // S-expr with mixed types: (foo 42 True "bar" 3.14)
    let val = MettaValue::SExpr(vec![
        MettaValue::Atom("foo".to_string()),
        MettaValue::Long(42),
        MettaValue::Bool(true),
        MettaValue::String("bar".to_string()),
        MettaValue::Float(3.14),
    ]);
    let decoded = roundtrip(&val);
    match decoded.inner() {
        MettaValueInner::SExpr(items) => {
            assert_eq!(items.len(), 5);
            assert_atom(&items[0], "foo");
            assert_long(&items[1], 42);
            assert!(matches!(items[2].inner(), MettaValueInner::Bool(true)));
            match items[3].inner() {
                MettaValueInner::String(s) => assert_eq!(*s, "bar"),
                other => panic!("Expected String, got {:?}", other),
            }
            match items[4].inner() {
                MettaValueInner::Float(f) => assert_eq!(*f, 3.14),
                other => panic!("Expected Float, got {:?}", other),
            }
        }
        other => panic!("Expected SExpr, got {:?}", other),
    }
}
