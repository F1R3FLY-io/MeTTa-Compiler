use super::models::MettaValue;
use pathmap::ring::{AlgebraicResult, DistributiveLattice, Lattice};
use pathmap::{zipper::*, PathMap};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MultisetCount(pub u32);

impl MultisetCount {
    pub fn increment(&mut self) {
        self.0 += 1;
    }

    pub fn count(&self) -> u32 {
        self.0
    }
}

// TODO -> need to make sure it works correctly
impl Lattice for MultisetCount {
    fn pjoin(&self, other: &Self) -> AlgebraicResult<Self> {
        AlgebraicResult::Element(MultisetCount(self.0 + other.0))
    }

    fn pmeet(&self, other: &Self) -> AlgebraicResult<Self> {
        AlgebraicResult::Element(MultisetCount(self.0.min(other.0)))
    }
}

// TODO -> need to make sure it works correctly
impl DistributiveLattice for MultisetCount {
    fn psubtract(&self, other: &Self) -> AlgebraicResult<Self> {
        let diff = self.0.saturating_sub(other.0);
        if diff == 0 {
            AlgebraicResult::None
        } else {
            AlgebraicResult::Element(MultisetCount(diff))
        }
    }
}

// TODO -> will be used for unique_atom
pub(crate) fn metta_expr_to_pathmap_set(value: &MettaValue) -> Result<PathMap<()>, String> {
    // TODO -> what about Conjunction?
    match value {
        MettaValue::SExpr(items) => {
            todo!()
        }
        MettaValue::Nil => Ok(PathMap::<()>::new()),
        _ => Err(format!("Cannot convert {:?} to PathMap set", value)),
    }
}

pub(crate) fn metta_expr_to_pathmap_multiset(
    value: &MettaValue,
) -> Result<PathMap<MultisetCount>, String> {
    // TODO -> what about Conjunction?
    match value {
        MettaValue::SExpr(items) => {
            let mut path_map = PathMap::<MultisetCount>::new();

            // TODO: consider optimization in case of frequent path duplication
            for item in items {
                let path = item.to_path_map_string();
                let path_bytes = path.as_bytes();

                let mut wz = path_map.write_zipper_at_path(path_bytes);

                if let Some(count) = wz.get_val_mut() {
                    count.increment();
                } else {
                    wz.get_val_or_set_mut(MultisetCount(1));
                }
            }

            Ok(path_map)
        }
        MettaValue::Nil => Ok(PathMap::<MultisetCount>::new()),
        _ => Err(format!("Cannot convert {:?} to PathMap multiset", value)),
    }
}

pub(crate) fn pathmap_to_metta_expr(pm: PathMap<MultisetCount>) -> Result<MettaValue, String> {
    let mut rz = pm.read_zipper();

    let mut res_items: Vec<MettaValue> = vec![];

    while rz.to_next_val() {
        // TODO -> fixme unwraps
        let path = std::str::from_utf8(rz.path()).unwrap();
        let metta_val = parse_pathmap_path(path)?;
        let count = rz.get_val().unwrap();

        for _ in 0..count.count() {
            res_items.push(metta_val.clone());
        }
    }

    Ok(MettaValue::SExpr(res_items))
}

fn parse_pathmap_path(s: &str) -> Result<MettaValue, String> {
    // Try simple types first
    if let Ok(n) = s.parse::<i64>() {
        return Ok(MettaValue::Long(n));
    }
    if let Ok(f) = s.parse::<f64>() {
        return Ok(MettaValue::Float(f));
    }

    // For S-expressions, don't parse - treat as atomic string
    Ok(MettaValue::Atom(s.to_string()))
}

// /// Parse a string path from PathMap back into a MettaValue
// ///
// /// This function handles parsing flat s-expressions (no nested structures) from PathMap paths.
// /// It supports:
// /// - Nil: "()"
// /// - Booleans: "true", "false"
// /// - String literals: quoted strings with escape sequences
// /// - Integers and floats
// /// - Flat s-expressions: "(a b c)" parsed as space-separated tokens
// /// - Conjunctions: "(, a b)" parsed as Conjunction variant
// /// - Atoms: everything else
// fn parse_pathmap_path(s: &str) -> Result<MettaValue, String> {
//     if s == "()" {
//         return Ok(MettaValue::Nil);
//     }

//     if s == "true" {
//         return Ok(MettaValue::Bool(true));
//     }
//     if s == "false" {
//         return Ok(MettaValue::Bool(false));
//     }

//     // Parse string literals (quoted)
//     if s.starts_with('"') && s.ends_with('"') && s.len() >= 2 {
//         let inner = &s[1..s.len() - 1];
//         // Handle escape sequences
//         let unescaped = inner
//             .replace("\\\"", "\"")
//             .replace("\\\\", "\\")
//             .replace("\\n", "\n")
//             .replace("\\r", "\r")
//             .replace("\\t", "\t");
//         return Ok(MettaValue::String(unescaped));
//     }

//     if let Ok(n) = s.parse::<i64>() {
//         return Ok(MettaValue::Long(n));
//     }

//     if let Ok(f) = s.parse::<f64>() {
//         return Ok(MettaValue::Float(f));
//     }

//     if s.starts_with('(') && s.ends_with(')') {
//         let content = s[1..s.len() - 1].trim();

//         if content.is_empty() {
//             return Ok(MettaValue::Nil);
//         }

//         // FIXME: this is wrong
//         // Split by whitespace and parse each token (flat, no nested parsing)
//         let items: Result<Vec<_>, _> = content
//             .split_whitespace()
//             .map(|token| parse_pathmap_path(token))
//             .collect();

//         let items = items?;

//         if items.is_empty() {
//             return Ok(MettaValue::Nil);
//         }

//         // Check if this is a conjunction: (,) or (, expr1 expr2 ...)
//         if let Some(MettaValue::Atom(first)) = items.first() {
//             if first == "," {
//                 // Skip the comma operator and create Conjunction
//                 let goals = items[1..].to_vec();
//                 return Ok(MettaValue::Conjunction(goals));
//             }
//         }

//         // Regular S-expression
//         return Ok(MettaValue::SExpr(items));
//     }

//     Ok(MettaValue::Atom(s.to_string()))
// }

// TODO -> test nested S-expr
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_pathmap_path_nil() {
        assert_eq!(parse_pathmap_path("()").unwrap(), MettaValue::Nil);
    }

    #[test]
    fn test_parse_pathmap_path_booleans() {
        assert_eq!(parse_pathmap_path("true").unwrap(), MettaValue::Bool(true));
        assert_eq!(
            parse_pathmap_path("false").unwrap(),
            MettaValue::Bool(false)
        );
    }

    #[test]
    fn test_parse_pathmap_path_integers() {
        assert_eq!(parse_pathmap_path("0").unwrap(), MettaValue::Long(0));
        assert_eq!(parse_pathmap_path("42").unwrap(), MettaValue::Long(42));
        assert_eq!(parse_pathmap_path("-10").unwrap(), MettaValue::Long(-10));
        assert_eq!(
            parse_pathmap_path("123456789").unwrap(),
            MettaValue::Long(123456789)
        );
    }

    #[test]
    fn test_parse_pathmap_path_floats() {
        assert_eq!(parse_pathmap_path("3.14").unwrap(), MettaValue::Float(3.14));
        assert_eq!(parse_pathmap_path("-2.5").unwrap(), MettaValue::Float(-2.5));
        assert_eq!(parse_pathmap_path("0.0").unwrap(), MettaValue::Float(0.0));
        assert_eq!(parse_pathmap_path("1e10").unwrap(), MettaValue::Float(1e10));
        assert_eq!(
            parse_pathmap_path("1.5e-3").unwrap(),
            MettaValue::Float(1.5e-3)
        );
    }

    #[test]
    fn test_parse_pathmap_path_strings() {
        assert_eq!(
            parse_pathmap_path(r#""hello""#).unwrap(),
            MettaValue::String("hello".to_string())
        );
        assert_eq!(
            parse_pathmap_path(r#""world""#).unwrap(),
            MettaValue::String("world".to_string())
        );
        assert_eq!(
            parse_pathmap_path(r#""""#).unwrap(),
            MettaValue::String("".to_string())
        );
    }

    #[test]
    fn test_parse_pathmap_path_string_escapes() {
        assert_eq!(
            parse_pathmap_path(r#""hello\nworld""#).unwrap(),
            MettaValue::String("hello\nworld".to_string())
        );
        assert_eq!(
            parse_pathmap_path(r#""tab\there""#).unwrap(),
            MettaValue::String("tab\there".to_string())
        );
        assert_eq!(
            parse_pathmap_path(r#""quote\"here""#).unwrap(),
            MettaValue::String("quote\"here".to_string())
        );
        assert_eq!(
            parse_pathmap_path(r#""back\\slash""#).unwrap(),
            MettaValue::String("back\\slash".to_string())
        );
        assert_eq!(
            parse_pathmap_path(r#""carriage\rreturn""#).unwrap(),
            MettaValue::String("carriage\rreturn".to_string())
        );
    }

    #[test]
    fn test_parse_pathmap_path_flat_sexpr() {
        let result = parse_pathmap_path("(a b c)").unwrap();
        assert!(matches!(result, MettaValue::SExpr(_)));
        if let MettaValue::SExpr(items) = result {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0], MettaValue::Atom("a".to_string()));
            assert_eq!(items[1], MettaValue::Atom("b".to_string()));
            assert_eq!(items[2], MettaValue::Atom("c".to_string()));
        }
    }

    #[test]
    fn test_parse_pathmap_path_sexpr_with_numbers() {
        let result = parse_pathmap_path("(add 1 2 3)").unwrap();
        assert!(matches!(result, MettaValue::SExpr(_)));
        if let MettaValue::SExpr(items) = result {
            assert_eq!(items.len(), 4);
            assert_eq!(items[0], MettaValue::Atom("add".to_string()));
            assert_eq!(items[1], MettaValue::Long(1));
            assert_eq!(items[2], MettaValue::Long(2));
            assert_eq!(items[3], MettaValue::Long(3));
        }
    }

    #[test]
    fn test_parse_pathmap_path_sexpr_with_mixed_types() {
        let result = parse_pathmap_path(r#"("hello" 42 3.14 true)"#).unwrap();
        assert!(matches!(result, MettaValue::SExpr(_)));
        if let MettaValue::SExpr(items) = result {
            assert_eq!(items.len(), 4);
            assert_eq!(items[0], MettaValue::String("hello".to_string()));
            assert_eq!(items[1], MettaValue::Long(42));
            assert_eq!(items[2], MettaValue::Float(3.14));
            assert_eq!(items[3], MettaValue::Bool(true));
        }
    }

    #[test]
    fn test_parse_pathmap_path_nested_sexpr_as_atom() {
        // Nested s-expressions are parsed as atoms (flat parsing)
        // When splitting "(a (b c) d)" by whitespace, we get: ["a", "(b", "c)", "d"]
        let result = parse_pathmap_path("(a (b c) d)").unwrap();
        assert!(matches!(result, MettaValue::SExpr(_)));
        if let MettaValue::SExpr(items) = result {
            assert_eq!(items.len(), 4);
            assert_eq!(items[0], MettaValue::Atom("a".to_string()));
            // "(b" and "c)" are parsed as separate atoms due to flat parsing
            assert_eq!(items[1], MettaValue::Atom("(b".to_string()));
            assert_eq!(items[2], MettaValue::Atom("c)".to_string()));
            assert_eq!(items[3], MettaValue::Atom("d".to_string()));
        }
    }

    #[test]
    fn test_parse_pathmap_path_conjunction() {
        let result = parse_pathmap_path("(, a b)").unwrap();
        assert!(matches!(result, MettaValue::Conjunction(_)));
        if let MettaValue::Conjunction(goals) = result {
            assert_eq!(goals.len(), 2);
            assert_eq!(goals[0], MettaValue::Atom("a".to_string()));
            assert_eq!(goals[1], MettaValue::Atom("b".to_string()));
        }
    }

    #[test]
    fn test_parse_pathmap_path_empty_conjunction() {
        let result = parse_pathmap_path("(,)").unwrap();
        assert!(matches!(result, MettaValue::Conjunction(_)));
        if let MettaValue::Conjunction(goals) = result {
            assert_eq!(goals.len(), 0);
        }
    }

    #[test]
    fn test_parse_pathmap_path_atom() {
        assert_eq!(
            parse_pathmap_path("foo").unwrap(),
            MettaValue::Atom("foo".to_string())
        );
        assert_eq!(
            parse_pathmap_path("bar123").unwrap(),
            MettaValue::Atom("bar123".to_string())
        );
    }

    // Round-trip tests (MettaValue -> PathMap -> MettaValue)
    #[test]
    fn test_round_trip_with_strings() {
        let value = MettaValue::SExpr(vec![
            MettaValue::String("hello".to_string()),
            MettaValue::String("world".to_string()),
            MettaValue::String("test".to_string()),
        ]);

        let path_map = metta_expr_to_pathmap_multiset(&value).unwrap();
        let result = pathmap_to_metta_expr(path_map).unwrap();

        // Result should contain all strings (order may vary, so we check counts)
        if let MettaValue::SExpr(items) = result {
            assert_eq!(items.len(), 3);
            let string_count: usize = items
                .iter()
                .filter(|v| matches!(v, MettaValue::String(_)))
                .count();
            assert_eq!(string_count, 3);
        } else {
            panic!("Expected SExpr");
        }
    }

    #[test]
    fn test_round_trip_with_numbers() {
        let value = MettaValue::SExpr(vec![
            MettaValue::Long(0),
            MettaValue::Long(42),
            MettaValue::Long(-100),
            MettaValue::Float(3.14),
            MettaValue::Float(-2.5),
        ]);

        let path_map = metta_expr_to_pathmap_multiset(&value).unwrap();
        let result = pathmap_to_metta_expr(path_map).unwrap();

        if let MettaValue::SExpr(items) = result {
            assert_eq!(items.len(), 5);
            // Check that all numbers are present
            let has_zero = items.iter().any(|v| v == &MettaValue::Long(0));
            let has_42 = items.iter().any(|v| v == &MettaValue::Long(42));
            let has_neg100 = items.iter().any(|v| v == &MettaValue::Long(-100));
            let has_314 = items.iter().any(|v| v == &MettaValue::Float(3.14));
            let has_neg25 = items.iter().any(|v| v == &MettaValue::Float(-2.5));
            assert!(has_zero && has_42 && has_neg100 && has_314 && has_neg25);
        } else {
            panic!("Expected SExpr");
        }
    }

    #[test]
    fn test_round_trip_with_nested_sexpr() {
        // Test that nested s-expressions are preserved as single paths
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("a".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("b".to_string()),
                MettaValue::Atom("c".to_string()),
            ]),
            MettaValue::Atom("d".to_string()),
        ]);

        let path_map = metta_expr_to_pathmap_multiset(&value).unwrap();
        let result = pathmap_to_metta_expr(path_map).unwrap();

        // The nested SExpr should be serialized as "(b c)" and parsed back
        if let MettaValue::SExpr(items) = result {
            assert_eq!(items.len(), 3);
            // One item should be the nested s-expr "(b c)"
            let has_nested = items
                .iter()
                .any(|v| matches!(v, MettaValue::SExpr(inner) if inner.len() == 2));
            assert!(has_nested, "Should contain nested s-expression");
        } else {
            panic!("Expected SExpr");
        }
    }

    #[test]
    fn test_round_trip_with_escaped_strings() {
        let value = MettaValue::SExpr(vec![
            MettaValue::String("hello\nworld".to_string()),
            MettaValue::String("tab\there".to_string()),
            MettaValue::String("quote\"here".to_string()),
        ]);

        let path_map = metta_expr_to_pathmap_multiset(&value).unwrap();
        let result = pathmap_to_metta_expr(path_map).unwrap();

        if let MettaValue::SExpr(items) = result {
            assert_eq!(items.len(), 3);
            // Verify escape sequences are preserved
            let has_newline = items
                .iter()
                .any(|v| matches!(v, MettaValue::String(s) if s.contains('\n')));
            let has_tab = items
                .iter()
                .any(|v| matches!(v, MettaValue::String(s) if s.contains('\t')));
            let has_quote = items
                .iter()
                .any(|v| matches!(v, MettaValue::String(s) if s.contains('"')));
            assert!(has_newline && has_tab && has_quote);
        } else {
            panic!("Expected SExpr");
        }
    }

    #[test]
    fn test_simple_conversion() {
        // Test conversion of SExpr with unique items
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("a".to_string()),
            MettaValue::Atom("b".to_string()),
            MettaValue::Atom("c".to_string()),
        ]);

        let path_map = metta_expr_to_pathmap_multiset(&value).unwrap();
        let mut rz = path_map.read_zipper();

        // Collect all paths and counts
        let mut items = Vec::new();
        while rz.to_next_val() {
            let path = String::from_utf8(rz.path().to_vec()).unwrap();
            let count = rz.get_val().unwrap().count();
            items.push((path, count));
        }

        // Sort for comparison (order may vary)
        items.sort();

        assert_eq!(items.len(), 3);
        assert_eq!(items[0], ("a".to_string(), 1));
        assert_eq!(items[1], ("b".to_string(), 1));
        assert_eq!(items[2], ("c".to_string(), 1));
    }

    #[test]
    fn test_multiset_counting() {
        // Test that duplicate items increment the count
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("a".to_string()),
            MettaValue::Atom("b".to_string()),
            MettaValue::Atom("a".to_string()),
            MettaValue::Atom("b".to_string()),
            MettaValue::Atom("b".to_string()),
        ]);

        let path_map = metta_expr_to_pathmap_multiset(&value).unwrap();
        let mut rz = path_map.read_zipper();

        // Collect all paths and counts
        let mut items = Vec::new();
        while rz.to_next_val() {
            let path = String::from_utf8(rz.path().to_vec()).unwrap();
            let count = rz.get_val().unwrap().count();
            items.push((path, count));
        }

        // Sort for comparison
        items.sort();

        assert_eq!(items.len(), 2);
        assert_eq!(items[0], ("a".to_string(), 2)); // "a" appears twice
        assert_eq!(items[1], ("b".to_string(), 3)); // "b" appears three times
    }

    #[test]
    fn test_nil_conversion() {
        // Test that Nil converts to empty PathMap
        let value = MettaValue::Nil;
        let path_map = metta_expr_to_pathmap_multiset(&value).unwrap();

        let mut rz = path_map.read_zipper();
        let mut count = 0;
        while rz.to_next_val() {
            count += 1;
        }

        assert_eq!(count, 0);
    }

    #[test]
    fn test_empty_sexpr() {
        // Test empty SExpr (should be empty PathMap)
        let value = MettaValue::SExpr(vec![]);
        let path_map = metta_expr_to_pathmap_multiset(&value).unwrap();

        let mut rz = path_map.read_zipper();
        let mut count = 0;
        while rz.to_next_val() {
            count += 1;
        }

        assert_eq!(count, 0);
    }

    #[test]
    fn test_different_value_types() {
        // Test conversion with different MettaValue types
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("atom".to_string()),
            MettaValue::Bool(true),
            MettaValue::Bool(false),
            MettaValue::Long(42),
            MettaValue::Long(-10),
            MettaValue::Float(3.14),
            MettaValue::String("hello".to_string()),
        ]);

        let path_map = metta_expr_to_pathmap_multiset(&value).unwrap();
        let mut rz = path_map.read_zipper();

        // Collect all paths and counts into a HashMap for easier lookup
        let mut items = std::collections::HashMap::new();
        while rz.to_next_val() {
            let path = String::from_utf8(rz.path().to_vec()).unwrap();
            let count = rz.get_val().unwrap().count();
            items.insert(path, count);
        }

        assert_eq!(items.len(), 7);
        assert_eq!(items.get("atom"), Some(&1));
        assert_eq!(items.get("true"), Some(&1));
        assert_eq!(items.get("false"), Some(&1));
        assert_eq!(items.get("42"), Some(&1));
        assert_eq!(items.get("-10"), Some(&1));
        assert_eq!(items.get("3.14"), Some(&1));
        assert_eq!(items.get("\"hello\""), Some(&1));
    }

    #[test]
    fn test_error_cases() {
        // Test that non-SExpr, non-Nil values return errors
        let atom = MettaValue::Atom("test".to_string());
        assert!(metta_expr_to_pathmap_multiset(&atom).is_err());

        let bool_val = MettaValue::Bool(true);
        assert!(metta_expr_to_pathmap_multiset(&bool_val).is_err());

        let long_val = MettaValue::Long(42);
        assert!(metta_expr_to_pathmap_multiset(&long_val).is_err());
    }

    #[test]
    fn test_multiset_count_increment() {
        // Test MultisetCount::increment()
        let mut count = MultisetCount(5);
        count.increment();
        assert_eq!(count.count(), 6);
    }

    #[test]
    fn test_multiset_count_lattice_join() {
        // Test Lattice::pjoin (addition)
        let count1 = MultisetCount(3);
        let count2 = MultisetCount(5);

        if let AlgebraicResult::Element(result) = count1.pjoin(&count2) {
            assert_eq!(result.count(), 8);
        } else {
            panic!("Expected Element result");
        }
    }

    #[test]
    fn test_multiset_count_lattice_meet() {
        // Test Lattice::pmeet (minimum)
        let count1 = MultisetCount(3);
        let count2 = MultisetCount(5);

        if let AlgebraicResult::Element(result) = count1.pmeet(&count2) {
            assert_eq!(result.count(), 3); // min(3, 5) = 3
        } else {
            panic!("Expected Element result");
        }

        // Test reverse order
        if let AlgebraicResult::Element(result) = count2.pmeet(&count1) {
            assert_eq!(result.count(), 3); // min(5, 3) = 3
        } else {
            panic!("Expected Element result");
        }
    }

    #[test]
    fn test_mixed_duplicates() {
        // Test mixed types with duplicates
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("x".to_string()),
            MettaValue::Long(100),
            MettaValue::Atom("x".to_string()),
            MettaValue::Long(100),
            MettaValue::Long(100),
            MettaValue::Bool(true),
        ]);

        let path_map = metta_expr_to_pathmap_multiset(&value).unwrap();
        let mut rz = path_map.read_zipper();

        // Collect all paths and counts
        let mut items = Vec::new();
        while rz.to_next_val() {
            let path = String::from_utf8(rz.path().to_vec()).unwrap();
            let count = rz.get_val().unwrap().count();
            items.push((path, count));
        }

        // Sort for comparison
        items.sort();

        assert_eq!(items.len(), 3);
        assert_eq!(items[0], ("100".to_string(), 3)); // Long(100) appears 3 times
        assert_eq!(items[1], ("true".to_string(), 1));
        assert_eq!(items[2], ("x".to_string(), 2)); // Atom("x") appears 2 times
    }

    #[test]
    fn test_conjunction_values() {
        // Test that Conjunction values are converted (they have to_path_map_string)
        let value = MettaValue::SExpr(vec![
            MettaValue::Conjunction(vec![
                MettaValue::Atom("a".to_string()),
                MettaValue::Atom("b".to_string()),
            ]),
            MettaValue::Conjunction(vec![
                MettaValue::Atom("a".to_string()),
                MettaValue::Atom("b".to_string()),
            ]),
        ]);

        let path_map = metta_expr_to_pathmap_multiset(&value).unwrap();
        let mut rz = path_map.read_zipper();

        let mut items = Vec::new();
        while rz.to_next_val() {
            let path = String::from_utf8(rz.path().to_vec()).unwrap();
            let count = rz.get_val().unwrap().count();
            items.push((path, count));
        }

        assert_eq!(items.len(), 1);
        // Conjunction should serialize to "(, a b)"
        assert_eq!(items[0].1, 2); // Should appear twice
        assert!(items[0].0.contains("a"));
        assert!(items[0].0.contains("b"));
    }
}
