

//! Priority Ordering for MORK Exec Rules
//!
//! This module implements priority comparison for exec rules, supporting:
//! - Integer priorities: 0 < 1 < 2
//! - Tuple priorities: (0 0) < (0 1) < (1 0)
//! - Peano numbers: Z < (S Z) < (S (S Z))
//! - Mixed: (0 0) < (0 1) < (1 Z) < (1 (S Z)) < (2 0)

use crate::backend::models::MettaValue;
use std::cmp::Ordering;

/// Compare two priority values for ordering exec rules
///
/// Returns Ordering::{Less, Equal, Greater} for use in sorting
///
/// # Priority Comparison Rules
///
/// 1. **Integers**: Standard numeric comparison
///    - `0 < 1 < 2 < ...`
///
/// 2. **Peano Numbers**: Count S constructors
///    - `Z < (S Z) < (S (S Z)) < ...`
///    - `Z` is treated as 0
///    - `(S n)` is treated as n+1
///
/// 3. **Tuples**: Lexicographic comparison
///    - Compare element by element left-to-right
///    - `(0 0) < (0 1) < (1 0)`
///    - `(1 Z) < (1 (S Z))`
///
/// 4. **Type Precedence** (when types differ):
///    - Integers < Peano < Tuples
///    - Atoms < SExprs
///
/// # Examples
///
/// ```rust
/// use mettatron::backend::eval::priority::compare_priorities;
/// use mettatron::backend::models::MettaValue;
/// use std::cmp::Ordering;
///
/// let p1 = MettaValue::Long(0);
/// let p2 = MettaValue::Long(1);
/// assert_eq!(compare_priorities(&p1, &p2), Ordering::Less);
///
/// // Peano comparison
/// let z = MettaValue::Atom("Z".to_string());
/// let s_z = MettaValue::SExpr(vec![
///     MettaValue::Atom("S".to_string()),
///     MettaValue::Atom("Z".to_string())
/// ]);
/// assert_eq!(compare_priorities(&z, &s_z), Ordering::Less);
///
/// // Tuple comparison
/// let t1 = MettaValue::SExpr(vec![MettaValue::Long(0), MettaValue::Long(0)]);
/// let t2 = MettaValue::SExpr(vec![MettaValue::Long(0), MettaValue::Long(1)]);
/// assert_eq!(compare_priorities(&t1, &t2), Ordering::Less);
/// ```
pub fn compare_priorities(p1: &MettaValue, p2: &MettaValue) -> Ordering {
    match (p1, p2) {
        // Both integers
        (MettaValue::Long(n1), MettaValue::Long(n2)) => n1.cmp(n2),

        // Both Peano numbers or atoms
        (MettaValue::Atom(a1), MettaValue::Atom(a2)) => {
            if a1 == "Z" && a2 == "Z" {
                Ordering::Equal
            } else if a1 == "Z" {
                Ordering::Less // Z is smallest
            } else if a2 == "Z" {
                Ordering::Greater
            } else {
                // Non-Peano atoms: lexicographic
                a1.cmp(a2)
            }
        }

        // Peano: Z vs (S ...)
        (MettaValue::Atom(a), MettaValue::SExpr(_)) if a == "Z" => {
            if is_peano(p2) {
                Ordering::Less // Z < (S ...)
            } else {
                // Atom < SExpr (type precedence)
                Ordering::Less
            }
        }

        // Peano: (S ...) vs Z
        (MettaValue::SExpr(_), MettaValue::Atom(a)) if a == "Z" => {
            if is_peano(p1) {
                Ordering::Greater // (S ...) > Z
            } else {
                // SExpr > Atom (type precedence)
                Ordering::Greater
            }
        }

        // Both S-expressions: could be Peano or tuples
        (MettaValue::SExpr(items1), MettaValue::SExpr(items2)) => {
            let p1_is_peano = is_peano(p1);
            let p2_is_peano = is_peano(p2);

            match (p1_is_peano, p2_is_peano) {
                // Both Peano
                (true, true) => {
                    let count1 = count_peano_depth(p1);
                    let count2 = count_peano_depth(p2);
                    count1.cmp(&count2)
                }
                // One Peano, one tuple: Peano < Tuple (type precedence)
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
                // Both tuples: lexicographic comparison
                (false, false) => compare_tuples(items1, items2),
            }
        }

        // Mixed types: use type precedence
        // Order: Integer < Atom < SExpr
        (MettaValue::Long(_), MettaValue::Atom(_)) => Ordering::Less,
        (MettaValue::Long(_), MettaValue::SExpr(_)) => Ordering::Less,
        (MettaValue::Atom(_), MettaValue::Long(_)) => Ordering::Greater,
        (MettaValue::Atom(_), MettaValue::SExpr(_)) => Ordering::Less,
        (MettaValue::SExpr(_), MettaValue::Long(_)) => Ordering::Greater,
        (MettaValue::SExpr(_), MettaValue::Atom(_)) => Ordering::Greater,

        // All other types: not comparable priorities, use Equal
        _ => Ordering::Equal,
    }
}

/// Check if a MettaValue is a Peano number (Z or (S ...))
fn is_peano(value: &MettaValue) -> bool {
    match value {
        MettaValue::Atom(a) => a == "Z",
        MettaValue::SExpr(items) => {
            // Must be (S ...) where ... is also Peano
            if items.len() == 2 {
                if let MettaValue::Atom(op) = &items[0] {
                    if op == "S" {
                        return is_peano(&items[1]);
                    }
                }
            }
            false
        }
        _ => false,
    }
}

/// Count the depth of a Peano number
/// - Z = 0
/// - (S Z) = 1
/// - (S (S Z)) = 2
/// - etc.
fn count_peano_depth(value: &MettaValue) -> usize {
    match value {
        MettaValue::Atom(a) if a == "Z" => 0,
        MettaValue::SExpr(items) if items.len() == 2 => {
            if let MettaValue::Atom(op) = &items[0] {
                if op == "S" {
                    return 1 + count_peano_depth(&items[1]);
                }
            }
            0
        }
        _ => 0,
    }
}

/// Compare two tuples lexicographically
fn compare_tuples(items1: &[MettaValue], items2: &[MettaValue]) -> Ordering {
    // Compare element by element
    for (e1, e2) in items1.iter().zip(items2.iter()) {
        match compare_priorities(e1, e2) {
            Ordering::Less => return Ordering::Less,
            Ordering::Greater => return Ordering::Greater,
            Ordering::Equal => continue,
        }
    }

    // If all compared elements are equal, shorter tuple is less
    items1.len().cmp(&items2.len())
}

/// Sort a vector of (priority, data) pairs by priority
///
/// Returns a new vector sorted from lowest to highest priority.
///
/// # Example
///
/// ```rust
/// use mettatron::backend::eval::priority::sort_by_priority;
/// use mettatron::backend::models::MettaValue;
///
/// let items = vec![
///     (MettaValue::Long(2), "high"),
///     (MettaValue::Long(0), "low"),
///     (MettaValue::Long(1), "medium"),
/// ];
///
/// let sorted = sort_by_priority(items);
/// assert_eq!(sorted[0].1, "low");
/// assert_eq!(sorted[1].1, "medium");
/// assert_eq!(sorted[2].1, "high");
/// ```
pub fn sort_by_priority<T>(mut items: Vec<(MettaValue, T)>) -> Vec<(MettaValue, T)> {
    items.sort_by(|(p1, _), (p2, _)| compare_priorities(p1, p2));
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_peano(n: usize) -> MettaValue {
        if n == 0 {
            MettaValue::Atom("Z".to_string())
        } else {
            MettaValue::SExpr(vec![
                MettaValue::Atom("S".to_string()),
                make_peano(n - 1),
            ])
        }
    }

    fn make_tuple(values: Vec<MettaValue>) -> MettaValue {
        MettaValue::SExpr(values)
    }

    #[test]
    fn test_integer_priorities() {
        let p0 = MettaValue::Long(0);
        let p1 = MettaValue::Long(1);
        let p2 = MettaValue::Long(2);

        assert_eq!(compare_priorities(&p0, &p1), Ordering::Less);
        assert_eq!(compare_priorities(&p1, &p2), Ordering::Less);
        assert_eq!(compare_priorities(&p0, &p2), Ordering::Less);
        assert_eq!(compare_priorities(&p1, &p1), Ordering::Equal);
    }

    #[test]
    fn test_peano_zero() {
        let z1 = MettaValue::Atom("Z".to_string());
        let z2 = MettaValue::Atom("Z".to_string());

        assert_eq!(compare_priorities(&z1, &z2), Ordering::Equal);
    }

    #[test]
    fn test_peano_successors() {
        let z = make_peano(0);
        let s_z = make_peano(1);
        let s_s_z = make_peano(2);
        let s_s_s_z = make_peano(3);

        assert_eq!(compare_priorities(&z, &s_z), Ordering::Less);
        assert_eq!(compare_priorities(&s_z, &s_s_z), Ordering::Less);
        assert_eq!(compare_priorities(&s_s_z, &s_s_s_z), Ordering::Less);
        assert_eq!(compare_priorities(&z, &s_s_z), Ordering::Less);
    }

    #[test]
    fn test_tuple_priorities() {
        let t00 = make_tuple(vec![MettaValue::Long(0), MettaValue::Long(0)]);
        let t01 = make_tuple(vec![MettaValue::Long(0), MettaValue::Long(1)]);
        let t10 = make_tuple(vec![MettaValue::Long(1), MettaValue::Long(0)]);
        let t20 = make_tuple(vec![MettaValue::Long(2), MettaValue::Long(0)]);

        assert_eq!(compare_priorities(&t00, &t01), Ordering::Less);
        assert_eq!(compare_priorities(&t01, &t10), Ordering::Less);
        assert_eq!(compare_priorities(&t10, &t20), Ordering::Less);
        assert_eq!(compare_priorities(&t00, &t00), Ordering::Equal);
    }

    #[test]
    fn test_mixed_tuple_peano() {
        // (1 Z) < (1 (S Z)) < (2 Z)
        let t1z = make_tuple(vec![MettaValue::Long(1), make_peano(0)]);
        let t1sz = make_tuple(vec![MettaValue::Long(1), make_peano(1)]);
        let t2z = make_tuple(vec![MettaValue::Long(2), make_peano(0)]);

        assert_eq!(compare_priorities(&t1z, &t1sz), Ordering::Less);
        assert_eq!(compare_priorities(&t1sz, &t2z), Ordering::Less);
        assert_eq!(compare_priorities(&t1z, &t2z), Ordering::Less);
    }

    #[test]
    fn test_ancestor_mm2_priorities() {
        // Test the exact priorities from ancestor.mm2
        // (0 0) < (0 1) < (1 Z) < (2 0) < (2 1) < (2 2)

        let p00 = make_tuple(vec![MettaValue::Long(0), MettaValue::Long(0)]);
        let p01 = make_tuple(vec![MettaValue::Long(0), MettaValue::Long(1)]);
        let p1z = make_tuple(vec![MettaValue::Long(1), make_peano(0)]);
        let p20 = make_tuple(vec![MettaValue::Long(2), MettaValue::Long(0)]);
        let p21 = make_tuple(vec![MettaValue::Long(2), MettaValue::Long(1)]);
        let p22 = make_tuple(vec![MettaValue::Long(2), MettaValue::Long(2)]);

        assert_eq!(compare_priorities(&p00, &p01), Ordering::Less);
        assert_eq!(compare_priorities(&p01, &p1z), Ordering::Less);
        assert_eq!(compare_priorities(&p1z, &p20), Ordering::Less);
        assert_eq!(compare_priorities(&p20, &p21), Ordering::Less);
        assert_eq!(compare_priorities(&p21, &p22), Ordering::Less);
    }

    #[test]
    fn test_is_peano() {
        assert!(is_peano(&make_peano(0)));
        assert!(is_peano(&make_peano(1)));
        assert!(is_peano(&make_peano(5)));

        assert!(!is_peano(&MettaValue::Long(0)));
        assert!(!is_peano(&make_tuple(vec![MettaValue::Long(1), MettaValue::Long(2)])));
    }

    #[test]
    fn test_count_peano_depth() {
        assert_eq!(count_peano_depth(&make_peano(0)), 0);
        assert_eq!(count_peano_depth(&make_peano(1)), 1);
        assert_eq!(count_peano_depth(&make_peano(5)), 5);
        assert_eq!(count_peano_depth(&make_peano(10)), 10);
    }

    #[test]
    fn test_sort_by_priority() {
        let items = vec![
            (MettaValue::Long(2), "high"),
            (MettaValue::Long(0), "low"),
            (MettaValue::Long(1), "medium"),
            (make_peano(0), "zero"),
        ];

        let sorted = sort_by_priority(items);

        // Type precedence: Integer < Atom < SExpr
        // So: 0 < 1 < 2 < Z
        assert_eq!(sorted[0].1, "low");      // 0 (Integer)
        assert_eq!(sorted[1].1, "medium");   // 1 (Integer)
        assert_eq!(sorted[2].1, "high");     // 2 (Integer)
        assert_eq!(sorted[3].1, "zero");     // Z (Atom)
    }
}
