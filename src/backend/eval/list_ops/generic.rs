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
            &format!(
                "car-atom requires 1 argument, got {}. Usage: (car-atom expr)",
                items.len() - 1
            ),
            factory.sexpr(items.to_vec()),
        )];
    }

    let expr = &items[1];

    if let Some(elements) = expr.as_sexpr() {
        if !elements.is_empty() {
            return vec![elements[0].clone()];
        }
        // Empty sexpr
        return vec![factory.error(
            "car-atom expects a non-empty expression as argument",
            expr.clone(),
        )];
    }

    // Quoted is transparent to car-atom: (car-atom (quote X)) → quote
    if expr.is_quoted() {
        return vec![factory.atom("quote")];
    }

    if expr.is_unit() {
        return vec![factory.error(
            "car-atom expects a non-empty expression as argument",
            expr.clone(),
        )];
    }

    // Not an expression
    vec![factory.error(
        "car-atom: expected expression. Usage: (car-atom expr)",
        expr.clone(),
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
            &format!(
                "cdr-atom requires 1 argument, got {}. Usage: (cdr-atom expr)",
                items.len() - 1
            ),
            factory.sexpr(items.to_vec()),
        )];
    }

    let expr = &items[1];

    if let Some(elements) = expr.as_sexpr() {
        if !elements.is_empty() {
            let tail: Vec<V> = elements[1..].to_vec();
            return vec![factory.sexpr(tail)];
        }
        // Empty sexpr
        return vec![factory.error(
            "cdr-atom expects a non-empty expression as argument",
            expr.clone(),
        )];
    }

    // Quoted is transparent to cdr-atom: (cdr-atom (quote X)) → (X)
    if let Some(inner) = expr.as_quoted() {
        return vec![factory.sexpr(vec![inner])];
    }

    if expr.is_unit() {
        return vec![factory.error(
            "cdr-atom expects a non-empty expression as argument",
            expr.clone(),
        )];
    }

    // Not an expression
    vec![factory.error(
        "cdr-atom: expected expression. Usage: (cdr-atom expr)",
        expr.clone(),
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
            &format!(
                "cons-atom requires 2 arguments, got {}. Usage: (cons-atom head tail)",
                items.len() - 1
            ),
            factory.sexpr(items.to_vec()),
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
        "cons-atom expected Expression as tail. Usage: (cons-atom head tail)",
        tail.clone(),
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
            &format!(
                "decons-atom requires 1 argument, got {}. Usage: (decons-atom expr)",
                items.len() - 1
            ),
            factory.sexpr(items.to_vec()),
        )];
    }

    let expr = &items[1];

    if let Some(elements) = expr.as_sexpr() {
        if !elements.is_empty() {
            let head = elements[0].clone();
            let tail = factory.sexpr(elements[1..].to_vec());
            return vec![factory.sexpr(vec![head, tail])];
        }
        // Empty sexpr - nondeterministic failure (return nothing)
        return vec![];
    }

    if expr.is_unit() || expr.is_unit() {
        // Empty/Unit - nondeterministic failure (return nothing)
        return vec![];
    }

    // Not an expression
    vec![factory.error(
        "decons-atom: expected expression. Usage: (decons-atom expr)",
        expr.clone(),
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
            &format!(
                "size-atom requires 1 argument, got {}. Usage: (size-atom expr)",
                items.len() - 1
            ),
            factory.sexpr(items.to_vec()),
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
        "size-atom: expected expression. Usage: (size-atom expr)",
        expr.clone(),
    )]
}

/// max-atom: Get the maximum numeric value in an expression
/// Example: (max-atom (1 5 3 2)) -> 5
pub fn eval_max_atom_generic<V, F>(items: &[V], factory: &F) -> Vec<V>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    if items.len() != 2 {
        return vec![factory.error(
            &format!(
                "max-atom requires 1 argument, got {}. Usage: (max-atom expr)",
                items.len() - 1
            ),
            factory.sexpr(items.to_vec()),
        )];
    }

    let expr = &items[1];

    if let Some(elements) = expr.as_sexpr() {
        if elements.is_empty() {
            return vec![factory.error(
                "max-atom expects a non-empty expression",
                expr.clone(),
            )];
        }

        let mut max_val: Option<i64> = None;
        for elem in elements {
            if let Some(n) = elem.as_long() {
                max_val = Some(max_val.map_or(n, |m| m.max(n)));
            } else {
                return vec![factory.error(
                    "max-atom expects all elements to be numbers",
                    elem.clone(),
                )];
            }
        }

        if let Some(max) = max_val {
            return vec![factory.long(max)];
        }
    }

    if expr.is_unit() {
        return vec![factory.error(
            "max-atom expects a non-empty expression",
            expr.clone(),
        )];
    }

    vec![factory.error(
        "max-atom: expected expression. Usage: (max-atom expr)",
        expr.clone(),
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
            &format!(
                "min-atom requires 1 argument, got {}. Usage: (min-atom expr)",
                items.len() - 1
            ),
            factory.sexpr(items.to_vec()),
        )];
    }

    let expr = &items[1];

    if let Some(elements) = expr.as_sexpr() {
        if elements.is_empty() {
            return vec![factory.error(
                "min-atom expects a non-empty expression",
                expr.clone(),
            )];
        }

        let mut min_val: Option<i64> = None;
        for elem in elements {
            if let Some(n) = elem.as_long() {
                min_val = Some(min_val.map_or(n, |m| m.min(n)));
            } else {
                return vec![factory.error(
                    "min-atom expects all elements to be numbers",
                    elem.clone(),
                )];
            }
        }

        if let Some(min) = min_val {
            return vec![factory.long(min)];
        }
    }

    if expr.is_unit() {
        return vec![factory.error(
            "min-atom expects a non-empty expression",
            expr.clone(),
        )];
    }

    vec![factory.error(
        "min-atom: expected expression. Usage: (min-atom expr)",
        expr.clone(),
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
            &format!(
                "index-atom requires 2 arguments, got {}. Usage: (index-atom expr index)",
                items.len() - 1
            ),
            factory.sexpr(items.to_vec()),
        )];
    }

    let expr = &items[1];
    let index_val = &items[2];

    let index = match index_val.as_long() {
        Some(i) if i >= 0 => i as usize,
        Some(_) => {
            return vec![factory.error(
                "index-atom: index must be non-negative",
                index_val.clone(),
            )];
        }
        None => {
            return vec![factory.error(
                "index-atom: index must be an integer",
                index_val.clone(),
            )];
        }
    };

    if let Some(elements) = expr.as_sexpr() {
        if index < elements.len() {
            return vec![elements[index].clone()];
        }
        return vec![factory.error(
            &format!(
                "index-atom: index {} out of bounds for expression of size {}",
                index,
                elements.len()
            ),
            expr.clone(),
        )];
    }

    vec![factory.error(
        "index-atom: expected expression. Usage: (index-atom expr index)",
        expr.clone(),
    )]
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
}
