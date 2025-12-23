use super::models::MettaValue;
use pathmap::ring::{AlgebraicResult, Lattice};
use pathmap::{zipper::*, PathMap};

// TODO -> We serialize the entire nested structure as one key?
// From metta interpreter:
// !(union-atom (a b c (D E F (1 2 3))) ()) ->> [(a b c (D E F (1 2 3)))]
// TODO -> so answer is Yes

/*
    TODO -> extracting back to MeTTa list

    fn metta_multiset_operations() {
        // union-atom (a b b c) (d e e)
        let list1 = ["a", "b", "b", "c"];
        let list2 = ["d", "e", "e"];

        let map1 = metta_list_to_indexed_pathmap(&list1);
        let map2 = metta_list_to_indexed_pathmap(&list2);

        let union = map1.join(&map2);
        // Result preserves all elements: (a b b c d e e)

        TODO -> but how to iterate over paths?

        // To extract back to MeTTa list:
        let mut result_items = Vec::new();
        let mut rz = union.read_zipper();
        while rz.to_next_val() {
            let path = std::str::from_utf8(rz.path()).unwrap();
            // Parse "item#index" to get "item"
            if let Some(hash_pos) = path.find('#') {
                result_items.push(&path[..hash_pos]);
            }
        }
        // result_items = ["a", "b", "b", "c", "d", "e", "e"]
    }
*/

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

// TODO -> how exactly Lattice works?
impl Lattice for MultisetCount {
    fn pjoin(&self, other: &Self) -> AlgebraicResult<Self> {
        AlgebraicResult::Element(MultisetCount(self.0 + other.0))
    }

    fn pmeet(&self, other: &Self) -> AlgebraicResult<Self> {
        AlgebraicResult::Element(MultisetCount(self.0.min(other.0)))
    }
}

pub(crate) fn metta_expr_to_pathmap(value: &MettaValue) -> Result<PathMap<MultisetCount>, String> {
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
        _ => Err(format!("Cannot convert {:?} to PathMap", value)),
    }
}

pub(crate) fn pathmap_to_metta_expr(pm: PathMap<MultisetCount>) -> Result<MettaValue, String> {
    let mut rz = pm.read_zipper();

    let mut res_items: Vec<MettaValue> = vec![];

    while rz.to_next_val() {
        // TODO -> fixme unwraps
        let path = std::str::from_utf8(rz.path()).unwrap();
        let metta_val: MettaValue = path.try_into().unwrap();
        let count = rz.get_val().unwrap();

        for _ in 0..count.count() {
            res_items.push(metta_val.clone());
        }
    }

    Ok(MettaValue::SExpr(res_items))
}

// TODO -> test nested S-expr
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_conversion() {
        // Test conversion of SExpr with unique items
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("a".to_string()),
            MettaValue::Atom("b".to_string()),
            MettaValue::Atom("c".to_string()),
        ]);

        let path_map = metta_expr_to_pathmap(&value).unwrap();
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

        let path_map = metta_expr_to_pathmap(&value).unwrap();
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
        let path_map = metta_expr_to_pathmap(&value).unwrap();

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
        let path_map = metta_expr_to_pathmap(&value).unwrap();

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

        let path_map = metta_expr_to_pathmap(&value).unwrap();
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
        assert!(metta_expr_to_pathmap(&atom).is_err());

        let bool_val = MettaValue::Bool(true);
        assert!(metta_expr_to_pathmap(&bool_val).is_err());

        let long_val = MettaValue::Long(42);
        assert!(metta_expr_to_pathmap(&long_val).is_err());
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

        let path_map = metta_expr_to_pathmap(&value).unwrap();
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

        let path_map = metta_expr_to_pathmap(&value).unwrap();
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
