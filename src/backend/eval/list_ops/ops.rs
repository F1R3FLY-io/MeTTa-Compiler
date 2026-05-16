//! Generic List Operations
//!
//! This module provides generic implementations of list operations that work
//! with any value type implementing `MettaValueTrait`. These are used by the
//! generic evaluation engine to avoid conversions between value types.
//!
//! ## Design
//!
//! Each operation:
//! - Takes generic `&[V]` items and a factory for constructing results
//! - Returns `Vec<V>` results
//! - Uses `MettaValueTrait` methods for type checking and value extraction
//! - Uses `MettaValueFactory` for constructing new values

use crate::backend::models::{MettaValueFactory, MettaValueTrait};

/// car-atom: Get the first element of an expression (head)
/// Example: (car-atom (a b c)) -> a
pub fn eval_car_atom_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() != 2 {
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string(&format!(
                "car-atom requires 1 argument, got {}. Usage: (car-atom expr)",
                items.len() - 1
            )),
        )];
    }

    let expr = &items[1];

    if let Some(elements) = expr.as_sexpr() {
        if !elements.is_empty() {
            return vec![elements[0].clone()];
        }
        // H3 (2026-05-05) hard-cut: empty sexpr → HE-bisimilar Error atom.
        // Per HE stdlib.metta:570-579, car-atom desugars to chain+decons+unify
        // and the unify-failure branch produces:
        //   (Error (car-atom $atom) "car-atom expects a non-empty expression as an argument")
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string("car-atom expects a non-empty expression as an argument"),
        )];
    }

    // H3 hard-cut: quoted-transparency extension removed (not in HE).
    // Quoted args fall through to the same Error path as non-expressions.

    if expr.is_unit() {
        // H3 hard-cut: Unit treated identically to empty sexpr.
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string("car-atom expects a non-empty expression as an argument"),
        )];
    }

    // Not an expression — same HE Error message for all non-expr inputs
    // (consistency with empty/Unit per HE's chain-desugar semantics).
    vec![factory.error(
        factory.sexpr(items.to_vec()),
        factory.string("car-atom expects a non-empty expression as an argument"),
    )]
}

/// cdr-atom: Get the rest of an expression (tail)
/// Example: (cdr-atom (a b c)) -> (b c)
pub fn eval_cdr_atom_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() != 2 {
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string(&format!(
                "cdr-atom requires 1 argument, got {}. Usage: (cdr-atom expr)",
                items.len() - 1
            )),
        )];
    }

    let expr = &items[1];

    if let Some(elements) = expr.as_sexpr() {
        if !elements.is_empty() {
            let tail: Vec<V> = elements[1..].to_vec();
            return vec![factory.sexpr(tail)];
        }
        // H3 (2026-05-05) hard-cut: empty sexpr → HE-bisimilar Error atom.
        // Per HE stdlib.metta:587-590.
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string("cdr-atom expects a non-empty expression as an argument"),
        )];
    }

    // H3 hard-cut: quoted-transparency extension removed (not in HE).

    if expr.is_unit() {
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string("cdr-atom expects a non-empty expression as an argument"),
        )];
    }

    // Not an expression — same HE Error message
    vec![factory.error(
        factory.sexpr(items.to_vec()),
        factory.string("cdr-atom expects a non-empty expression as an argument"),
    )]
}

/// cons-atom: Construct an expression by prepending head to tail
/// Example: (cons-atom a (b c)) -> (a b c)
pub fn eval_cons_atom_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() != 3 {
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string(&format!(
                "cons-atom requires 2 arguments, got {}. Usage: (cons-atom head tail)",
                items.len() - 1
            )),
        )];
    }

    let head = &items[1];
    let tail = &items[2];

    if let Some(elements) = tail.as_sexpr() {
        let mut result = vec![head.clone()];
        result.extend(elements.iter().cloned());
        return vec![factory.sexpr(result)];
    }

    if tail.is_unit() {
        return vec![factory.sexpr(vec![head.clone()])];
    }

    // Tail is not an expression
    vec![factory.error(
        tail.clone(),
        factory.string("cons-atom expected Expression as tail. Usage: (cons-atom head tail)"),
    )]
}

/// decons-atom: Deconstruct an expression into (head tail) pair
/// Example: (decons-atom (a b c)) -> (a (b c))
pub fn eval_decons_atom_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() != 2 {
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string(&format!(
                "decons-atom requires 1 argument, got {}. Usage: (decons-atom expr)",
                items.len() - 1
            )),
        )];
    }

    let expr = &items[1];

    if let Some(elements) = expr.as_sexpr() {
        if !elements.is_empty() {
            let head = elements[0].clone();
            let tail = factory.sexpr(elements[1..].to_vec());
            return vec![factory.sexpr(vec![head, tail])];
        }
        // H3 (2026-05-05) hard-cut: empty sexpr → HE-bisimilar Error atom.
        // ERR-shape align (2026-05-16): per HE interpreter.rs:843-856 the
        // detail string is `"expected: (decons-atom (: <expr> Expression)),
        // found: <call>"` where `<call>` is the canonical print of the
        // entire `(decons-atom <arg>)` form (NOT "empty expression").
        // Conformance T04-kernel/019-decons-empty expects exactly
        // `(Error (decons-atom ()) "expected: (decons-atom (: <expr>
        // Expression)), found: (decons-atom ())")`.
        let call_form = factory.sexpr(items.to_vec());
        return vec![factory.error(
            call_form.clone(),
            factory.string(&format!(
                "expected: (decons-atom (: <expr> Expression)), found: {}",
                call_form.friendly_repr()
            )),
        )];
    }

    if expr.is_unit() {
        // H3 hard-cut: Unit treated identically to empty sexpr.
        // ERR-shape align (2026-05-16): same `found: <call>` HE format.
        let call_form = factory.sexpr(items.to_vec());
        return vec![factory.error(
            call_form.clone(),
            factory.string(&format!(
                "expected: (decons-atom (: <expr> Expression)), found: {}",
                call_form.friendly_repr()
            )),
        )];
    }

    // Not an expression — HE-bisimilar Error.
    // ERR-shape align (2026-05-16): same `found: <call>` HE format.
    let call_form = factory.sexpr(items.to_vec());
    vec![factory.error(
        call_form.clone(),
        factory.string(&format!(
            "expected: (decons-atom (: <expr> Expression)), found: {}",
            call_form.friendly_repr()
        )),
    )]
}

/// size-atom: Get the number of elements in an expression
/// Example: (size-atom (a b c)) -> 3
pub fn eval_size_atom_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() != 2 {
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string(&format!(
                "size-atom requires 1 argument, got {}. Usage: (size-atom expr)",
                items.len() - 1
            )),
        )];
    }

    let expr = &items[1];

    if let Some(elements) = expr.as_sexpr() {
        return vec![factory.long(elements.len() as i64)];
    }

    if expr.is_unit() {
        return vec![factory.long(0)];
    }

    // Not an expression
    vec![factory.error(
        expr.clone(),
        factory.string("size-atom: expected expression. Usage: (size-atom expr)"),
    )]
}

/// max-atom: Get the maximum numeric value in an expression
/// Example: (max-atom (1 5 3 2)) -> 5
/// Accumulator for max-atom / min-atom that tracks numeric type promotion.
///
/// BUG-T0-014 (spec §13.4): max-atom and min-atom previously accepted only
/// Long elements, rejecting Float and mixed Long/Float inputs. The fix uses
/// an enum accumulator that promotes to Float on first Float encounter,
/// matching the Long↔Float promotion semantics of arithmetic ops.
#[derive(Clone, Copy)]
enum NumAcc {
    Long(i64),
    Float(f64),
}

impl NumAcc {
    fn to_float(self) -> f64 {
        match self {
            Self::Long(n) => n as f64,
            Self::Float(f) => f,
        }
    }
}

pub fn eval_max_atom_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() != 2 {
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string(&format!(
                "max-atom requires 1 argument, got {}. Usage: (max-atom expr)",
                items.len() - 1
            )),
        )];
    }

    let expr = &items[1];

    if let Some(elements) = expr.as_sexpr() {
        if elements.is_empty() {
            return vec![factory.error(
    expr.clone(),
    factory.string("max-atom expects a non-empty expression"),
            )];
        }

        // BUG-T0-014: accept Float and mixed Long/Float inputs.
        // Output is Long when all elements were Long; Float as soon as any
        // Float element is seen (cross-type promotion semantics).
        let mut acc: Option<NumAcc> = None;
        for elem in elements {
            let next = match (elem.as_long(), elem.as_float()) {
                (Some(n), _) => NumAcc::Long(n),
                (_, Some(f)) => NumAcc::Float(f),
                _ => {
                    return vec![factory.error(
                        elem.clone(),
                        factory.string("max-atom expects all elements to be numbers"),
                    )]
                }
            };
            acc = Some(match (acc, next) {
                (None, n) => n,
                (Some(NumAcc::Long(a)), NumAcc::Long(b)) => NumAcc::Long(a.max(b)),
                (Some(a), b) => NumAcc::Float(a.to_float().max(b.to_float())),
            });
        }

        if let Some(result) = acc {
            return match result {
                NumAcc::Long(n) => vec![factory.long(n)],
                NumAcc::Float(f) => vec![factory.float(f)],
            };
        }
    }

    if expr.is_unit() {
        return vec![factory.error(
    expr.clone(),
    factory.string("max-atom expects a non-empty expression"),
        )];
    }

    vec![factory.error(
        expr.clone(),
        factory.string("max-atom: expected expression. Usage: (max-atom expr)"),
    )]
}

/// min-atom: Get the minimum numeric value in an expression
/// Example: (min-atom (1 5 3 2)) -> 1
pub fn eval_min_atom_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() != 2 {
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string(&format!(
                "min-atom requires 1 argument, got {}. Usage: (min-atom expr)",
                items.len() - 1
            )),
        )];
    }

    let expr = &items[1];

    if let Some(elements) = expr.as_sexpr() {
        if elements.is_empty() {
            return vec![factory.error(
    expr.clone(),
    factory.string("min-atom expects a non-empty expression"),
            )];
        }

        // BUG-T0-014: mirror max-atom's Float-promotion semantics.
        let mut acc: Option<NumAcc> = None;
        for elem in elements {
            let next = match (elem.as_long(), elem.as_float()) {
                (Some(n), _) => NumAcc::Long(n),
                (_, Some(f)) => NumAcc::Float(f),
                _ => {
                    return vec![factory.error(
                        elem.clone(),
                        factory.string("min-atom expects all elements to be numbers"),
                    )]
                }
            };
            acc = Some(match (acc, next) {
                (None, n) => n,
                (Some(NumAcc::Long(a)), NumAcc::Long(b)) => NumAcc::Long(a.min(b)),
                (Some(a), b) => NumAcc::Float(a.to_float().min(b.to_float())),
            });
        }

        if let Some(result) = acc {
            return match result {
                NumAcc::Long(n) => vec![factory.long(n)],
                NumAcc::Float(f) => vec![factory.float(f)],
            };
        }
    }

    if expr.is_unit() {
        return vec![factory.error(
    expr.clone(),
    factory.string("min-atom expects a non-empty expression"),
        )];
    }

    vec![factory.error(
        expr.clone(),
        factory.string("min-atom: expected expression. Usage: (min-atom expr)"),
    )]
}

/// index-atom: Get element at index
/// Example: (index-atom (a b c) 1) -> b
pub fn eval_index_atom_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() != 3 {
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string(&format!(
                "index-atom requires 2 arguments, got {}. Usage: (index-atom expr index)",
                items.len() - 1
            )),
        )];
    }

    let expr = &items[1];
    let index_val = &items[2];

    // ERR-shape align (2026-05-16): offending-call form is the full
    // `(index-atom <expr> <idx>)`, NOT the bare index/expr operand.
    // Detail strings match HE `interpret/atom.rs index_atom` emissions.
    let call_form = factory.sexpr(items.to_vec());

    let index = match index_val.as_long() {
        Some(i) if i >= 0 => i as usize,
        Some(_) => {
            return vec![factory.error(
                call_form,
                factory.string("Index is negative"),
            )];
        }
        None => {
            return vec![factory.error(
                call_form,
                factory.string("Index is not an integer"),
            )];
        }
    };

    if let Some(elements) = expr.as_sexpr() {
        if index < elements.len() {
            return vec![elements[index].clone()];
        }
        // ERR-shape align (2026-05-16): HE empirical detail is exactly
        // `"Index is out of bounds"` (no operand counts inlined). Conformance
        // T06-stdlib/083 expects `(Error (index-atom (a b c) 10) "Index is
        // out of bounds")`.
        return vec![factory.error(
            call_form,
            factory.string("Index is out of bounds"),
        )];
    }

    vec![factory.error(
        call_form,
        factory.string("First argument must be an expression"),
    )]
}

/// tuple-concat: Concatenate two tuples
/// Example: (tuple-concat (a b) (c d)) -> (a b c d)
/// (tuple-concat () $B) -> $B, (tuple-concat $A ()) -> $A
pub fn eval_tuple_concat_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() != 3 {
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string(&format!(
                "tuple-concat requires 2 arguments, got {}. Usage: (tuple-concat tuple1 tuple2)",
                items.len() - 1
            )),
        )];
    }

    let a = &items[1];
    let b = &items[2];

    let a_elems: Vec<V> = if a.is_unit() {
        vec![]
    } else if let Some(elems) = a.as_sexpr() {
        elems.iter().cloned().collect()
    } else {
        return vec![factory.error(
            a.clone(),
            factory.string("tuple-concat: first argument must be an expression"),
        )];
    };

    let b_elems: Vec<V> = if b.is_unit() {
        vec![]
    } else if let Some(elems) = b.as_sexpr() {
        elems.iter().cloned().collect()
    } else {
        return vec![factory.error(
            b.clone(),
            factory.string("tuple-concat: second argument must be an expression"),
        )];
    };

    let mut combined = Vec::with_capacity(a_elems.len() + b_elems.len());
    combined.extend(a_elems);
    combined.extend(b_elems);
    vec![factory.sexpr(combined)]
}

/// tuple-count: Count elements in a tuple
/// Example: (tuple-count (a b c)) -> 3
/// Alias for size-atom semantics but named for tuple convention.
pub fn eval_tuple_count_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() != 2 {
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string(&format!(
                "tuple-count requires 1 argument, got {}. Usage: (tuple-count tuple)",
                items.len() - 1
            )),
        )];
    }

    let expr = &items[1];

    if expr.is_unit() {
        return vec![factory.long(0)];
    }

    if let Some(elements) = expr.as_sexpr() {
        return vec![factory.long(elements.len() as i64)];
    }

    vec![factory.error(
        expr.clone(),
        factory.string("tuple-count: expected expression. Usage: (tuple-count tuple)"),
    )]
}

/// without: Remove all occurrences of an element from a tuple
/// Example: (without (a b c b) b) -> (a c)
/// Uses structural equality (PartialEq) for comparison.
pub fn eval_without_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + PartialEq + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() != 3 {
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string(&format!(
                "without requires 2 arguments, got {}. Usage: (without tuple elem)",
                items.len() - 1
            )),
        )];
    }

    let tuple = &items[1];
    let elem = &items[2];

    let elements: Vec<V> = if tuple.is_unit() {
        vec![]
    } else if let Some(elems) = tuple.as_sexpr() {
        elems.iter().cloned().collect()
    } else {
        return vec![factory.error(
            tuple.clone(),
            factory.string("without: first argument must be an expression"),
        )];
    };

    let filtered: Vec<V> = elements.into_iter().filter(|e| e != elem).collect();
    vec![factory.sexpr(filtered)]
}

/// element-of: Membership test — is elem present in tuple?
/// Example: (element-of a (a b c)) -> True
/// Example: (element-of d (a b c)) -> False
/// Uses structural equality (PartialEq) for comparison.
pub fn eval_element_of_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + PartialEq + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() != 3 {
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string(&format!(
                "element-of requires 2 arguments, got {}. Usage: (element-of elem tuple)",
                items.len() - 1
            )),
        )];
    }

    let elem = &items[1];
    let tuple = &items[2];

    let elements: &[V] = if tuple.is_unit() {
        &[]
    } else if let Some(elems) = tuple.as_sexpr() {
        elems
    } else {
        return vec![factory.error(
            tuple.clone(),
            factory.string("element-of: second argument must be an expression"),
        )];
    };

    let found = elements.iter().any(|e| e == elem);
    vec![factory.bool(found)]
}

/// range: Generate integer tuple [start, end)
/// Example: (range 0 5) -> (0 1 2 3 4)
pub fn eval_range_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() != 3 {
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string(&format!(
                "range requires 2 arguments, got {}. Usage: (range start end)",
                items.len() - 1
            )),
        )];
    }

    let start = match items[1].as_long() {
        Some(v) => v,
        None => return vec![factory.error(
    items[1].clone(),
    factory.string("range: start must be Long"),
        )],
    };
    let end = match items[2].as_long() {
        Some(v) => v,
        None => return vec![factory.error(
    items[2].clone(),
    factory.string("range: end must be Long"),
        )],
    };

    if start >= end {
        return vec![factory.sexpr(vec![])];
    }

    let count = (end - start) as usize;
    let mut elems = Vec::with_capacity(count);
    for i in start..end {
        elems.push(factory.long(i));
    }
    vec![factory.sexpr(elems)]
}

/// reverse-atom: Reverse a tuple
/// Example: (reverse-atom (a b c)) -> (c b a)
pub fn eval_reverse_atom_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() != 2 {
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string(&format!(
                "reverse-atom requires 1 argument, got {}. Usage: (reverse-atom expr)",
                items.len() - 1
            )),
        )];
    }

    let expr = &items[1];

    if let Some(elements) = expr.as_sexpr() {
        let reversed: Vec<V> = elements.iter().rev().cloned().collect();
        return vec![factory.sexpr(reversed)];
    }

    if expr.is_unit() {
        return vec![factory.sexpr(vec![])];
    }

    vec![factory.error(
    expr.clone(),
    factory.string("reverse-atom: argument must be an expression"),
    )]
}

/// flatten-atom: Flatten one level of nesting
/// Example: (flatten-atom ((a b) (c d))) -> (a b c d)
pub fn eval_flatten_atom_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() != 2 {
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string(&format!(
                "flatten-atom requires 1 argument, got {}. Usage: (flatten-atom expr)",
                items.len() - 1
            )),
        )];
    }

    let expr = &items[1];

    if let Some(elements) = expr.as_sexpr() {
        let mut flat = Vec::new();
        for elem in elements {
            match elem.as_sexpr() {
                Some(inner) => flat.extend(inner.iter().cloned()),
                None => flat.push(elem.clone()),
            }
        }
        return vec![factory.sexpr(flat)];
    }

    if expr.is_unit() {
        return vec![factory.sexpr(vec![])];
    }

    vec![factory.error(
    expr.clone(),
    factory.string("flatten-atom: argument must be an expression"),
    )]
}

/// zip-atom: Pair-wise zip of two tuples
/// Example: (zip-atom (a b c) (1 2 3)) -> ((a 1) (b 2) (c 3))
/// Truncated to the shorter length.
pub fn eval_zip_atom_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() != 3 {
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string(&format!(
                "zip-atom requires 2 arguments, got {}. Usage: (zip-atom expr1 expr2)",
                items.len() - 1
            )),
        )];
    }

    let a_elems = match items[1].as_sexpr() {
        Some(e) => e,
        None => {
            return vec![factory.error(
                items[1].clone(),
                factory.string("zip-atom: first argument must be an expression"),
            )]
        }
    };
    let b_elems = match items[2].as_sexpr() {
        Some(e) => e,
        None => {
            return vec![factory.error(
                items[2].clone(),
                factory.string("zip-atom: second argument must be an expression"),
            )]
        }
    };

    let min_len = a_elems.len().min(b_elems.len());
    let mut pairs = Vec::with_capacity(min_len);
    for i in 0..min_len {
        pairs.push(factory.sexpr(vec![a_elems[i].clone(), b_elems[i].clone()]));
    }
    vec![factory.sexpr(pairs)]
}

/// take-atom: First n elements of a tuple
/// Example: (take-atom (a b c d) 2) -> (a b)
/// Clamped to the tuple length.
pub fn eval_take_atom_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() != 3 {
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string(&format!(
                "take-atom requires 2 arguments, got {}. Usage: (take-atom expr n)",
                items.len() - 1
            )),
        )];
    }

    let elems = match items[1].as_sexpr() {
        Some(e) => e,
        None => {
            return vec![factory.error(
                items[1].clone(),
                factory.string("take-atom: first argument must be an expression"),
            )]
        }
    };
    let n = match items[2].as_long() {
        Some(v) if v >= 0 => v as usize,
        Some(_) => {
            return vec![factory.error(
    items[2].clone(),
    factory.string("take-atom: n must be non-negative"),
            )]
        }
        None => return vec![factory.error(
    items[2].clone(),
    factory.string("take-atom: n must be Long"),
        )],
    };

    let take_count = n.min(elems.len());
    let taken: Vec<V> = elems[..take_count].to_vec();
    vec![factory.sexpr(taken)]
}

/// drop-atom: Skip first n elements of a tuple
/// Example: (drop-atom (a b c d) 2) -> (c d)
/// Clamped to the tuple length.
pub fn eval_drop_atom_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() != 3 {
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string(&format!(
                "drop-atom requires 2 arguments, got {}. Usage: (drop-atom expr n)",
                items.len() - 1
            )),
        )];
    }

    let elems = match items[1].as_sexpr() {
        Some(e) => e,
        None => {
            return vec![factory.error(
                items[1].clone(),
                factory.string("drop-atom: first argument must be an expression"),
            )]
        }
    };
    let n = match items[2].as_long() {
        Some(v) if v >= 0 => v as usize,
        Some(_) => {
            return vec![factory.error(
    items[2].clone(),
    factory.string("drop-atom: n must be non-negative"),
            )]
        }
        None => return vec![factory.error(
    items[2].clone(),
    factory.string("drop-atom: n must be Long"),
        )],
    };

    let drop_count = n.min(elems.len());
    let remaining: Vec<V> = elems[drop_count..].to_vec();
    vec![factory.sexpr(remaining)]
}

// =============================================================================
// PeTTa-compatible helpers
// =============================================================================
//
// The following functions provide PeTTa-compatible aliases and new operations
// used by lib_pln.metta and other PeTTa-style code. They are pure-MeTTa
// equivalents (in the sense that they're implemented as MeTTaTron native
// grounded operators, not by shelling out to Prolog/Python/Chicken Scheme).
//
// PeTTa argument-order conventions sometimes differ from MeTTaTron's existing
// conventions:
//   - PeTTa `(is-member elem list)` matches MeTTaTron `(element-of elem list)` ✓
//   - PeTTa `(exclude-item elem list)` is REVERSED vs MeTTaTron `(without list elem)`
//   - PeTTa `(append a b)` matches MeTTaTron `(tuple-concat a b)` ✓
//   - PeTTa `(length list)` matches MeTTaTron `(size-atom list)` ✓

/// is-member: PeTTa-compatible alias of `element-of`.
///
/// Usage: `(is-member elem tuple)` -> `True | False`
///
/// Identical semantics to MeTTaTron's `element-of` (PeTTa's `is-member` and
/// MeTTaTron's `element-of` happen to use the same arg order: element first,
/// list second). Mirrors PeTTa's `metta.pl:114` `'is-member'/2` predicate.
pub fn eval_is_member_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + PartialEq + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() != 3 {
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string(&format!(
                "is-member requires 2 arguments, got {}. Usage: (is-member elem tuple)",
                items.len() - 1
            )),
        )];
    }

    let elem = &items[1];
    let tuple = &items[2];

    let elements: &[V] = if tuple.is_unit() {
        &[]
    } else if let Some(elems) = tuple.as_sexpr() {
        elems
    } else {
        return vec![factory.error(
            tuple.clone(),
            factory.string("is-member: second argument must be an expression"),
        )];
    };

    let found = elements.iter().any(|e| e == elem);
    vec![factory.bool(found)]
}

/// append: PeTTa-compatible alias of `tuple-concat`.
///
/// Usage: `(append tuple1 tuple2)` -> `(tuple1... tuple2...)`
///
/// Mirrors PeTTa's `append/3` builtin (Prolog list append).
pub fn eval_append_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() != 3 {
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string(&format!(
                "append requires 2 arguments, got {}. Usage: (append tuple1 tuple2)",
                items.len() - 1
            )),
        )];
    }

    let a = &items[1];
    let b = &items[2];

    let a_elems: Vec<V> = if a.is_unit() {
        vec![]
    } else if let Some(elems) = a.as_sexpr() {
        elems.iter().cloned().collect()
    } else {
        return vec![factory.error(
    a.clone(),
    factory.string("append: first argument must be an expression"),
        )];
    };

    let b_elems: Vec<V> = if b.is_unit() {
        vec![]
    } else if let Some(elems) = b.as_sexpr() {
        elems.iter().cloned().collect()
    } else {
        return vec![factory.error(
    b.clone(),
    factory.string("append: second argument must be an expression"),
        )];
    };

    let mut combined = Vec::with_capacity(a_elems.len() + b_elems.len());
    combined.extend(a_elems);
    combined.extend(b_elems);
    vec![factory.sexpr(combined)]
}

/// length: PeTTa-compatible alias of `size-atom`.
///
/// Usage: `(length tuple)` -> `Number`
///
/// Mirrors PeTTa's `length/2` Prolog builtin.
pub fn eval_length_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() != 2 {
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string(&format!(
                "length requires 1 argument, got {}. Usage: (length tuple)",
                items.len() - 1
            )),
        )];
    }

    let expr = &items[1];

    if expr.is_unit() {
        return vec![factory.long(0)];
    }

    if let Some(elements) = expr.as_sexpr() {
        return vec![factory.long(elements.len() as i64)];
    }

    vec![factory.error(
    expr.clone(),
    factory.string("length: argument must be an expression"),
    )]
}

/// exclude-item: PeTTa-compatible — like MeTTaTron's `without` but with
/// **reversed argument order**.
///
/// Usage: `(exclude-item elem tuple)` -> tuple-without-elem
///
/// PeTTa's `(exclude-item elem tuple)` (element first) is the reverse of
/// MeTTaTron's `(without tuple elem)` (tuple first). This wrapper swaps the
/// args and forwards to the existing `eval_without_generic` implementation.
///
/// Mirrors PeTTa's `'exclude-item'/3` predicate.
pub fn eval_exclude_item_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + PartialEq + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() != 3 {
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string(&format!(
                "exclude-item requires 2 arguments, got {}. Usage: (exclude-item elem tuple)",
                items.len() - 1
            )),
        )];
    }
    // Swap args: (exclude-item elem tuple) -> (without tuple elem)
    // The first item (head atom name) doesn't matter for `eval_without_generic`
    // because it just inspects items[1] and items[2].
    let swapped = vec![items[0].clone(), items[2].clone(), items[1].clone()];
    eval_without_generic(&swapped, factory)
}

/// msort: numeric ascending sort of a tuple of numbers.
///
/// Usage: `(msort (3 1 2))` -> `(1 2 3)`
///
/// Mirrors PeTTa's `msort/2` (which is `sort` without dedup, i.e. sort-with-dups).
/// Empty tuple → empty tuple. Non-numeric elements produce an error MettaValue.
/// Long and Float values are sorted as if all converted to f64.
pub fn eval_msort_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + PartialEq + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() != 2 {
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string(&format!(
                "msort requires 1 argument, got {}. Usage: (msort tuple)",
                items.len() - 1
            )),
        )];
    }
    let tuple = &items[1];
    let elements: Vec<V> = if tuple.is_unit() {
        vec![]
    } else if let Some(elems) = tuple.as_sexpr() {
        elems.iter().cloned().collect()
    } else {
        return vec![factory.error(
    tuple.clone(),
    factory.string("msort: argument must be an expression"),
        )];
    };
    // Sort by numeric value (Long or Float). Non-numeric items error out.
    let mut keyed: Vec<(f64, V)> = Vec::with_capacity(elements.len());
    for e in elements {
        let key = if let Some(n) = e.as_long() {
            n as f64
        } else if let Some(f) = e.as_float() {
            f
        } else {
            return vec![factory.error(
    e,
    factory.string("msort: all elements must be numeric (Long or Float)"),
            )];
        };
        keyed.push((key, e));
    }
    keyed.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let sorted: Vec<V> = keyed.into_iter().map(|(_, v)| v).collect();
    vec![factory.sexpr(sorted)]
}

/// progn: sequential evaluation, returns the last result.
///
/// Usage: `(progn expr1 expr2 ... exprN)` -> result of `exprN`
///
/// Each preceding expression is evaluated for side effects only. Because
/// MeTTaTron uses applicative-order evaluation, the side-effecting earlier
/// arguments are already reduced by the trampoline before this function is
/// called, so this just returns the last argument.
///
/// Mirrors PeTTa's `progn/N` (sequential composition).
pub fn eval_progn_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() < 2 {
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string("progn requires at least 1 argument"),
        )];
    }
    // items[0] is the head atom "progn"; items[1..] are the args.
    // Last arg is at items[items.len() - 1]. Already pre-evaluated by the trampoline.
    vec![items.last().unwrap().clone()]
}

/// reduce: PeTTa-compatible — force evaluation of an expression.
///
/// Usage: `(reduce expr)` -> evaluated expr
///
/// In MeTTaTron's applicative-order evaluator, the argument is already
/// reduced by the trampoline by the time this function is called, so this
/// is effectively the identity function. Mirrors PeTTa's
/// `<PeTTa>/src/translator.pl:50` `reduce/2` predicate, which forces
/// evaluation of an expression to its normal form.
pub fn eval_reduce_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() != 2 {
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string(&format!(
                "reduce requires 1 argument, got {}. Usage: (reduce expr)",
                items.len() - 1
            )),
        )];
    }
    // The argument is already reduced by the time we get here (applicative order).
    vec![items[1].clone()]
}

/// cut: Prolog-style commitment marker. Returns Unit.
///
/// Usage: `(cut)` -> `()`
///
/// Implements Prolog-style cut semantics: when evaluated inside a rule's
/// RHS (typically via `(progn (cut) body)`), signals the nearest enclosing
/// nondeterministic rule dispatch to commit to the current branch and
/// discard remaining alternative matches.
///
/// Mirrors PeTTa's Prolog `cut/0` (`!`), exposed as `cut` in MeTTa code.
pub fn eval_cut_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() != 1 {
        return vec![factory.error(
            factory.sexpr(items.to_vec()),
            factory.string(&format!(
                "cut takes no arguments, got {}. Usage: (cut)",
                items.len() - 1
            )),
        )];
    }
    // Signal the nearest enclosing ProcessRuleMatches continuation to
    // discard remaining alternative matches. The flag is consumed (cleared)
    // when the continuation observes it.
    crate::backend::eval::trampoline::eval_loop::set_cut_active();
    vec![factory.unit()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{GcFactory, MettaValue};

    #[test]
    fn test_car_atom_generic() {
        let factory = GcFactory::default();
        let items = vec![
            MettaValue::Atom("car-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("a".to_string()),
                MettaValue::Atom("b".to_string()),
                MettaValue::Atom("c".to_string()),
            ]),
        ];
        let result = eval_car_atom_generic(&items, &factory);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_atom(), Some("a"));
    }

    #[test]
    fn test_cdr_atom_generic() {
        let factory = GcFactory::default();
        let items = vec![
            MettaValue::Atom("cdr-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("a".to_string()),
                MettaValue::Atom("b".to_string()),
                MettaValue::Atom("c".to_string()),
            ]),
        ];
        let result = eval_cdr_atom_generic(&items, &factory);
        assert_eq!(result.len(), 1);
        if let Some(elems) = result[0].as_sexpr() {
            assert_eq!(elems.len(), 2);
            assert_eq!(elems[0].as_atom(), Some("b"));
            assert_eq!(elems[1].as_atom(), Some("c"));
        } else {
            panic!("Expected sexpr");
        }
    }

    #[test]
    fn test_cons_atom_generic() {
        let factory = GcFactory::default();
        let items = vec![
            MettaValue::Atom("cons-atom".to_string()),
            MettaValue::Atom("a".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("b".to_string()),
                MettaValue::Atom("c".to_string()),
            ]),
        ];
        let result = eval_cons_atom_generic(&items, &factory);
        assert_eq!(result.len(), 1);
        if let Some(elems) = result[0].as_sexpr() {
            assert_eq!(elems.len(), 3);
            assert_eq!(elems[0].as_atom(), Some("a"));
            assert_eq!(elems[1].as_atom(), Some("b"));
            assert_eq!(elems[2].as_atom(), Some("c"));
        } else {
            panic!("Expected sexpr");
        }
    }

    #[test]
    fn test_size_atom_generic() {
        let factory = GcFactory::default();
        let items = vec![
            MettaValue::Atom("size-atom".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("a".to_string()),
                MettaValue::Atom("b".to_string()),
                MettaValue::Atom("c".to_string()),
            ]),
        ];
        let result = eval_size_atom_generic(&items, &factory);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_long(), Some(3));
    }

    // =========================================================================
    // tuple-concat tests
    // =========================================================================

    #[test]
    fn test_tuple_concat_two_tuples() {
        let factory = GcFactory::default();
        let items = vec![
            MettaValue::Atom("tuple-concat".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("a".to_string()),
                MettaValue::Atom("b".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("c".to_string()),
                MettaValue::Atom("d".to_string()),
            ]),
        ];
        let result = eval_tuple_concat_generic(&items, &factory);
        assert_eq!(result.len(), 1);
        let elems = result[0].as_sexpr().expect("Expected sexpr");
        assert_eq!(elems.len(), 4);
        assert_eq!(elems[0].as_atom(), Some("a"));
        assert_eq!(elems[1].as_atom(), Some("b"));
        assert_eq!(elems[2].as_atom(), Some("c"));
        assert_eq!(elems[3].as_atom(), Some("d"));
    }

    #[test]
    fn test_tuple_concat_empty_left() {
        let factory = GcFactory::default();
        let items = vec![
            MettaValue::Atom("tuple-concat".to_string()),
            MettaValue::Unit(),
            MettaValue::SExpr(vec![MettaValue::Atom("a".to_string())]),
        ];
        let result = eval_tuple_concat_generic(&items, &factory);
        assert_eq!(result.len(), 1);
        let elems = result[0].as_sexpr().expect("Expected sexpr");
        assert_eq!(elems.len(), 1);
        assert_eq!(elems[0].as_atom(), Some("a"));
    }

    #[test]
    fn test_tuple_concat_empty_both() {
        let factory = GcFactory::default();
        let items = vec![
            MettaValue::Atom("tuple-concat".to_string()),
            MettaValue::Unit(),
            MettaValue::Unit(),
        ];
        let result = eval_tuple_concat_generic(&items, &factory);
        assert_eq!(result.len(), 1);
        // factory.sexpr(vec![]) returns Unit
        assert!(result[0].is_unit());
    }

    #[test]
    fn test_tuple_concat_empty_sexpr_left() {
        let factory = GcFactory::default();
        let items = vec![
            MettaValue::Atom("tuple-concat".to_string()),
            MettaValue::SExpr(vec![]),
            MettaValue::SExpr(vec![MettaValue::Atom("x".to_string())]),
        ];
        let result = eval_tuple_concat_generic(&items, &factory);
        assert_eq!(result.len(), 1);
        let elems = result[0].as_sexpr().expect("Expected sexpr");
        assert_eq!(elems.len(), 1);
        assert_eq!(elems[0].as_atom(), Some("x"));
    }

    #[test]
    fn test_tuple_concat_type_error() {
        let factory = GcFactory::default();
        let items = vec![
            MettaValue::Atom("tuple-concat".to_string()),
            MettaValue::Long(42),
            MettaValue::Long(43),
        ];
        let result = eval_tuple_concat_generic(&items, &factory);
        assert_eq!(result.len(), 1);
        assert!(result[0].is_error());
    }

    // =========================================================================
    // tuple-count tests
    // =========================================================================

    #[test]
    fn test_tuple_count_basic() {
        let factory = GcFactory::default();
        let items = vec![
            MettaValue::Atom("tuple-count".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("a".to_string()),
                MettaValue::Atom("b".to_string()),
                MettaValue::Atom("c".to_string()),
            ]),
        ];
        let result = eval_tuple_count_generic(&items, &factory);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_long(), Some(3));
    }

    #[test]
    fn test_tuple_count_empty() {
        let factory = GcFactory::default();
        let items = vec![
            MettaValue::Atom("tuple-count".to_string()),
            MettaValue::Unit(),
        ];
        let result = eval_tuple_count_generic(&items, &factory);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_long(), Some(0));
    }

    #[test]
    fn test_tuple_count_empty_sexpr() {
        let factory = GcFactory::default();
        let items = vec![
            MettaValue::Atom("tuple-count".to_string()),
            MettaValue::SExpr(vec![]),
        ];
        let result = eval_tuple_count_generic(&items, &factory);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_long(), Some(0));
    }

    // =========================================================================
    // without tests
    // =========================================================================

    #[test]
    fn test_without_basic() {
        let factory = GcFactory::default();
        let items = vec![
            MettaValue::Atom("without".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("a".to_string()),
                MettaValue::Atom("b".to_string()),
                MettaValue::Atom("c".to_string()),
                MettaValue::Atom("b".to_string()),
            ]),
            MettaValue::Atom("b".to_string()),
        ];
        let result = eval_without_generic(&items, &factory);
        assert_eq!(result.len(), 1);
        let elems = result[0].as_sexpr().expect("Expected sexpr");
        assert_eq!(elems.len(), 2);
        assert_eq!(elems[0].as_atom(), Some("a"));
        assert_eq!(elems[1].as_atom(), Some("c"));
    }

    #[test]
    fn test_without_not_found() {
        let factory = GcFactory::default();
        let items = vec![
            MettaValue::Atom("without".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("a".to_string()),
                MettaValue::Atom("b".to_string()),
            ]),
            MettaValue::Atom("z".to_string()),
        ];
        let result = eval_without_generic(&items, &factory);
        assert_eq!(result.len(), 1);
        let elems = result[0].as_sexpr().expect("Expected sexpr");
        assert_eq!(elems.len(), 2);
    }

    #[test]
    fn test_without_all_removed() {
        let factory = GcFactory::default();
        let items = vec![
            MettaValue::Atom("without".to_string()),
            MettaValue::SExpr(vec![MettaValue::Atom("a".to_string())]),
            MettaValue::Atom("a".to_string()),
        ];
        let result = eval_without_generic(&items, &factory);
        assert_eq!(result.len(), 1);
        // factory.sexpr(vec![]) returns Unit
        assert!(result[0].is_unit());
    }

    #[test]
    fn test_without_empty_tuple() {
        let factory = GcFactory::default();
        let items = vec![
            MettaValue::Atom("without".to_string()),
            MettaValue::Unit(),
            MettaValue::Atom("x".to_string()),
        ];
        let result = eval_without_generic(&items, &factory);
        assert_eq!(result.len(), 1);
        // factory.sexpr(vec![]) returns Unit
        assert!(result[0].is_unit());
    }

    // =========================================================================
    // element-of tests
    // =========================================================================

    #[test]
    fn test_element_of_found() {
        let factory = GcFactory::default();
        let items = vec![
            MettaValue::Atom("element-of".to_string()),
            MettaValue::Atom("b".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("a".to_string()),
                MettaValue::Atom("b".to_string()),
                MettaValue::Atom("c".to_string()),
            ]),
        ];
        let result = eval_element_of_generic(&items, &factory);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_bool(), Some(true));
    }

    #[test]
    fn test_element_of_not_found() {
        let factory = GcFactory::default();
        let items = vec![
            MettaValue::Atom("element-of".to_string()),
            MettaValue::Atom("z".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("a".to_string()),
                MettaValue::Atom("b".to_string()),
            ]),
        ];
        let result = eval_element_of_generic(&items, &factory);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_bool(), Some(false));
    }

    #[test]
    fn test_element_of_empty_tuple() {
        let factory = GcFactory::default();
        let items = vec![
            MettaValue::Atom("element-of".to_string()),
            MettaValue::Atom("a".to_string()),
            MettaValue::Unit(),
        ];
        let result = eval_element_of_generic(&items, &factory);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_bool(), Some(false));
    }

    #[test]
    fn test_element_of_number() {
        let factory = GcFactory::default();
        let items = vec![
            MettaValue::Atom("element-of".to_string()),
            MettaValue::Long(2),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
        ];
        let result = eval_element_of_generic(&items, &factory);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].as_bool(), Some(true));
    }
}
