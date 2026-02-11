use mettatron::MettaValue;
use proptest::prelude::*;

use super::primitive::*;
use super::utils::*;

pub fn valid_flat_conjunction(limit: usize) -> impl Strategy<Value = String> {
    prop::collection::vec(primitive(), 1..limit).prop_map(|vec| _build_conj_string(vec))
}

pub fn invalid_flat_conjunction(limit: usize) -> impl Strategy<Value = String> {
    let atoms = prop::collection::vec(primitive(), 1..limit);
    _wrap_invalid_conjunction(atoms)
}

pub fn valid_nested_conjunction(
    depth: u32,
    size: u32,
    branch_size: u32,
) -> impl Strategy<Value = String> {
    metta_atom()
        .prop_recursive(depth, size, branch_size, |inner| {
            prop::collection::vec(inner.clone(), 0..10).prop_map(MettaValue::Conjunction)
        })
        .prop_map(|sexpr| sexpr.to_mork_string())
}

pub fn invalid_nested_conjunction(
    depth: u32,
    size: u32,
    branch_size: u32,
) -> impl Strategy<Value = String> {
    // Use invalid base case
    let invalid_base = _wrap_invalid_conjunction(prop::collection::vec(primitive(), 0..3));

    invalid_base.prop_recursive(depth, size, branch_size, |inner| {
        let atoms = prop::collection::vec(inner.clone(), 0..10);
        _wrap_invalid_conjunction(atoms)
    })
}

pub fn mixed_conjunction_sexpr(
    depth: u32,
    size: u32,
    branch_size: u32,
) -> impl Strategy<Value = String> {
    // Start with primitives and recursively build mixed structures
    metta_atom()
        .prop_recursive(depth, size, branch_size, |inner| {
            prop_oneof![
                // Regular S-expressions
                prop::collection::vec(inner.clone(), 0..10).prop_map(MettaValue::SExpr),
                // Conjunctions
                prop::collection::vec(inner.clone(), 0..10).prop_map(MettaValue::Conjunction),
            ]
        })
        .prop_map(|expr| expr.to_mork_string())
}

pub fn empty_conjunction() -> impl Strategy<Value = String> {
    Just("(,)".to_string())
}

pub fn unary_conjunction() -> impl Strategy<Value = String> {
    primitive().prop_map(|expr| format!("(, {})", expr))
}

pub fn multiline_conjunction() -> impl Strategy<Value = String> {
    metta_atom()
        .prop_recursive(8, 256, 100, |inner| {
            prop::collection::vec(inner.clone(), 0..10).prop_map(MettaValue::Conjunction)
        })
        .prop_map(|expr| format!("{}\n", expr.to_mork_string()))
}

fn _build_conj_string(items: Vec<String>) -> String {
    _build_conj_string_with_delimiters("(", ")", items)
}

fn _wrap_invalid_conjunction(
    children_strategy: impl Strategy<Value = Vec<String>>,
) -> impl Strategy<Value = String> {
    (any::<EnclosedDelimiter>(), children_strategy)
        .prop_flat_map(|(open, atoms)| {
            let close = match open {
                EnclosedDelimiter::OpenParen => Just(EnclosedDelimiter::OpenParen).boxed(),
                EnclosedDelimiter::CloseParen => any::<EnclosedDelimiter>(),
            };

            (Just(open), close, Just(atoms))
        })
        .prop_map(|(open, close, atoms)| {
            _build_conj_string_with_delimiters(open.to_str(), close.to_str(), atoms)
        })
}

fn _build_conj_string_with_delimiters(open: &str, close: &str, items: Vec<String>) -> String {
    let mut res = String::new();
    res.push_str(open);
    res.push_str(", ");

    for item in items {
        res.push_str(&item);
        res.push_str(" ")
    }

    res.push_str(close);
    res
}
