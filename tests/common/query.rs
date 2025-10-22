#![allow(clippy::collapsible_match)]

use super::output_parser::PathMapOutput;
/// Query and matching system for PathMap outputs
///
/// Provides XQuery-inspired path-based queries and type-coercing matchers
/// for asserting PathMap test outputs.
///
/// # XQuery-Like Path Expressions
///
/// - `query_sexpr("steps")` - Find first s-expression with head "steps"
/// - `query_all_sexpr("plan")` - Find all s-expressions with head "plan"
/// - `query_descendant("objective")` - Find any descendant with head "objective" (recursive)
/// - `query_path(&["plan", "route", "waypoints"])` - Navigate nested s-expressions
///
/// # Predicates and Filtering
///
/// - `filter_contains("ball1")` - Filter outputs containing specific atom/string
/// - `filter_sexpr_with(predicate)` - Filter with custom predicate
///
/// # Type Coercion
///
/// - `as_atom()`, `as_string()`, `as_i64()`, `as_bool()` - Extract typed values
/// - `as_sexpr()` - Extract nested s-expression elements
use mettatron::backend::models::MettaValue;

/// Query result that can contain multiple values
#[derive(Debug, Clone, PartialEq)]
pub enum QueryResult {
    /// Single value result
    Single(MettaValue),

    /// Multiple values result
    Multiple(Vec<MettaValue>),

    /// No results
    Empty,
}

impl QueryResult {
    /// Get as a single value, or None if empty/multiple
    pub fn as_single(&self) -> Option<&MettaValue> {
        match self {
            QueryResult::Single(v) => Some(v),
            _ => None,
        }
    }

    /// Get as a vector of values
    pub fn as_vec(&self) -> Vec<MettaValue> {
        match self {
            QueryResult::Single(v) => vec![v.clone()],
            QueryResult::Multiple(v) => v.clone(),
            QueryResult::Empty => Vec::new(),
        }
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        matches!(self, QueryResult::Empty)
    }

    /// Get the number of results
    pub fn len(&self) -> usize {
        match self {
            QueryResult::Single(_) => 1,
            QueryResult::Multiple(v) => v.len(),
            QueryResult::Empty => 0,
        }
    }

    /// XQuery-like: Extract atom/symbol value
    pub fn as_atom(&self) -> Option<String> {
        match self.as_single()? {
            MettaValue::Atom(s) | MettaValue::String(s) => Some(s.clone()),
            _ => None,
        }
    }

    /// XQuery-like: Extract string value
    pub fn as_string(&self) -> Option<String> {
        self.as_atom()
    }

    /// XQuery-like: Extract integer value
    pub fn as_i64(&self) -> Option<i64> {
        match self.as_single()? {
            MettaValue::Long(n) => Some(*n),
            _ => None,
        }
    }

    /// XQuery-like: Extract boolean value
    pub fn as_bool(&self) -> Option<bool> {
        match self.as_single()? {
            MettaValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// XQuery-like: Extract s-expression elements
    pub fn as_sexpr(&self) -> Option<Vec<MettaValue>> {
        match self.as_single()? {
            MettaValue::SExpr(elements) => Some(elements.clone()),
            _ => None,
        }
    }

    /// XQuery-like: Filter results by predicate
    pub fn filter<F>(&self, predicate: F) -> QueryResult
    where
        F: Fn(&MettaValue) -> bool,
    {
        let filtered: Vec<MettaValue> =
            self.as_vec().into_iter().filter(|v| predicate(v)).collect();

        match filtered.len() {
            0 => QueryResult::Empty,
            1 => QueryResult::Single(filtered[0].clone()),
            _ => QueryResult::Multiple(filtered),
        }
    }

    /// XQuery-like: Check if any result matches predicate
    pub fn exists<F>(&self, predicate: F) -> bool
    where
        F: Fn(&MettaValue) -> bool,
    {
        self.as_vec().iter().any(predicate)
    }

    /// XQuery-like: Count results
    pub fn count(&self) -> usize {
        self.len()
    }
}

/// Extension trait for PathMapOutput with query capabilities
pub trait PathMapQuery {
    /// Query outputs by index
    ///
    /// Examples:
    /// - `query_output(0)` - Get first output
    /// - `query_output(1)` - Get second output
    fn query_output(&self, index: usize) -> QueryResult;

    /// Query all outputs
    fn query_all_outputs(&self) -> QueryResult;

    /// Query outputs that match a predicate
    fn query_outputs_where<F>(&self, predicate: F) -> QueryResult
    where
        F: Fn(&MettaValue) -> bool;

    /// Get output at index with type coercion to i64
    fn output_as_i64(&self, index: usize) -> Option<i64>;

    /// Get output at index with type coercion to bool
    fn output_as_bool(&self, index: usize) -> Option<bool>;

    /// Get output at index with type coercion to string
    fn output_as_string(&self, index: usize) -> Option<String>;

    /// Get all outputs as a sequence of i64 values
    fn outputs_as_i64_seq(&self) -> Vec<i64>;

    /// Check if outputs match a sequence of values with type coercion
    fn outputs_match_sequence<T>(&self, expected: &[T]) -> bool
    where
        T: ToMettaValue + Clone;

    // ========================================================================
    // XQuery-like Path Expressions
    // ========================================================================

    /// XQuery-like: Find first s-expression with specific head
    ///
    /// Example: `query_sexpr("plan")` finds first `(plan ...)`
    fn query_sexpr(&self, head: &str) -> QueryResult;

    /// XQuery-like: Find all s-expressions with specific head
    ///
    /// Example: `query_all_sexpr("step")` finds all `(step ...)`
    fn query_all_sexpr(&self, head: &str) -> QueryResult;

    /// XQuery-like: Find first descendant with specific head (recursive)
    ///
    /// Example: `query_descendant("objective")` finds `(objective ...)` at any depth
    fn query_descendant(&self, head: &str) -> QueryResult;

    /// XQuery-like: Find all descendants with specific head (recursive)
    ///
    /// Example: `query_all_descendants("navigate")` finds all `(navigate ...)` at any depth
    fn query_all_descendants(&self, head: &str) -> QueryResult;

    /// XQuery-like: Navigate a path through nested s-expressions
    ///
    /// Example: `query_path(&["plan", "route", "waypoints"])` navigates
    /// `(plan ... (route ... (waypoints ...)))`
    fn query_path(&self, path: &[&str]) -> QueryResult;

    /// XQuery-like: Filter outputs containing specific atom/string
    ///
    /// Example: `filter_contains("ball1")` finds outputs mentioning "ball1"
    fn filter_contains(&self, text: &str) -> QueryResult;
}

impl PathMapQuery for PathMapOutput {
    fn query_output(&self, index: usize) -> QueryResult {
        if index < self.output.len() {
            QueryResult::Single(self.output[index].clone())
        } else {
            QueryResult::Empty
        }
    }

    fn query_all_outputs(&self) -> QueryResult {
        if self.output.is_empty() {
            QueryResult::Empty
        } else if self.output.len() == 1 {
            QueryResult::Single(self.output[0].clone())
        } else {
            QueryResult::Multiple(self.output.clone())
        }
    }

    fn query_outputs_where<F>(&self, predicate: F) -> QueryResult
    where
        F: Fn(&MettaValue) -> bool,
    {
        let results: Vec<MettaValue> = self
            .output
            .iter()
            .filter(|v| predicate(v))
            .cloned()
            .collect();

        match results.len() {
            0 => QueryResult::Empty,
            1 => QueryResult::Single(results[0].clone()),
            _ => QueryResult::Multiple(results),
        }
    }

    fn output_as_i64(&self, index: usize) -> Option<i64> {
        match self.query_output(index) {
            QueryResult::Single(MettaValue::Long(n)) => Some(n),
            _ => None,
        }
    }

    fn output_as_bool(&self, index: usize) -> Option<bool> {
        match self.query_output(index) {
            QueryResult::Single(MettaValue::Bool(b)) => Some(b),
            _ => None,
        }
    }

    fn output_as_string(&self, index: usize) -> Option<String> {
        match self.query_output(index) {
            QueryResult::Single(MettaValue::String(s)) => Some(s),
            QueryResult::Single(MettaValue::Atom(s)) => Some(s),
            _ => None,
        }
    }

    fn outputs_as_i64_seq(&self) -> Vec<i64> {
        self.output
            .iter()
            .filter_map(|v| match v {
                MettaValue::Long(n) => Some(*n),
                _ => None,
            })
            .collect()
    }

    fn outputs_match_sequence<T>(&self, expected: &[T]) -> bool
    where
        T: ToMettaValue + Clone,
    {
        if self.output.len() != expected.len() {
            return false;
        }

        self.output
            .iter()
            .zip(expected.iter())
            .all(|(actual, expected_val)| actual == &expected_val.clone().to_metta_value())
    }

    // ========================================================================
    // XQuery-like Path Expressions Implementation
    // ========================================================================

    fn query_sexpr(&self, head: &str) -> QueryResult {
        for value in &self.output {
            if Self::has_head(value, head) {
                return QueryResult::Single(value.clone());
            }
        }
        QueryResult::Empty
    }

    fn query_all_sexpr(&self, head: &str) -> QueryResult {
        let results: Vec<MettaValue> = self
            .output
            .iter()
            .filter(|v| Self::has_head(v, head))
            .cloned()
            .collect();

        match results.len() {
            0 => QueryResult::Empty,
            1 => QueryResult::Single(results[0].clone()),
            _ => QueryResult::Multiple(results),
        }
    }

    fn query_descendant(&self, head: &str) -> QueryResult {
        for value in &self.output {
            if let Some(found) = Self::find_descendant(value, head) {
                return QueryResult::Single(found.clone());
            }
        }
        QueryResult::Empty
    }

    fn query_all_descendants(&self, head: &str) -> QueryResult {
        let mut results = Vec::new();
        for value in &self.output {
            Self::collect_descendants(value, head, &mut results);
        }

        match results.len() {
            0 => QueryResult::Empty,
            1 => QueryResult::Single(results[0].clone()),
            _ => QueryResult::Multiple(results),
        }
    }

    fn query_path(&self, path: &[&str]) -> QueryResult {
        for value in &self.output {
            if let Some(found) = Self::navigate_path(value, path) {
                return QueryResult::Single(found.clone());
            }
        }
        QueryResult::Empty
    }

    fn filter_contains(&self, text: &str) -> QueryResult {
        let results: Vec<MettaValue> = self
            .output
            .iter()
            .filter(|v| Self::contains_text(v, text))
            .cloned()
            .collect();

        match results.len() {
            0 => QueryResult::Empty,
            1 => QueryResult::Single(results[0].clone()),
            _ => QueryResult::Multiple(results),
        }
    }
}

// ============================================================================
// Helper functions for XQuery-like operations
// ============================================================================

impl PathMapOutput {
    /// Check if a value is an s-expression with a specific head
    fn has_head(value: &MettaValue, head: &str) -> bool {
        if let MettaValue::SExpr(elements) = value {
            if let Some(first) = elements.first() {
                match first {
                    MettaValue::String(s) | MettaValue::Atom(s) => s == head,
                    _ => false,
                }
            } else {
                false
            }
        } else {
            false
        }
    }

    /// Find first descendant with specific head (recursive)
    fn find_descendant<'a>(value: &'a MettaValue, head: &str) -> Option<&'a MettaValue> {
        // Direct match
        if Self::has_head(value, head) {
            return Some(value);
        }

        // Recursive search in nested s-expressions
        if let MettaValue::SExpr(elements) = value {
            for element in elements {
                if let Some(found) = Self::find_descendant(element, head) {
                    return Some(found);
                }
            }
        }

        None
    }

    /// Collect all descendants with specific head (recursive)
    fn collect_descendants(value: &MettaValue, head: &str, results: &mut Vec<MettaValue>) {
        // Direct match
        if Self::has_head(value, head) {
            results.push(value.clone());
        }

        // Recursive search in nested s-expressions
        if let MettaValue::SExpr(elements) = value {
            for element in elements {
                Self::collect_descendants(element, head, results);
            }
        }
    }

    /// Navigate a path through nested s-expressions
    fn navigate_path<'a>(value: &'a MettaValue, path: &[&str]) -> Option<&'a MettaValue> {
        if path.is_empty() {
            return Some(value);
        }

        // Current level must match first path element
        if !Self::has_head(value, path[0]) {
            return None;
        }

        // If this is the last path element, return this value
        if path.len() == 1 {
            return Some(value);
        }

        // Navigate deeper
        if let MettaValue::SExpr(elements) = value {
            for element in elements.iter().skip(1) {
                // Skip the head
                if let Some(found) = Self::navigate_path(element, &path[1..]) {
                    return Some(found);
                }
            }
        }

        None
    }

    /// Check if a value contains specific text (atom or string)
    fn contains_text(value: &MettaValue, text: &str) -> bool {
        match value {
            MettaValue::Atom(s) | MettaValue::String(s) => s.contains(text),
            MettaValue::SExpr(elements) => elements.iter().any(|e| Self::contains_text(e, text)),
            _ => false,
        }
    }
}

/// Trait for converting Rust types to MettaValue with type coercion
pub trait ToMettaValue {
    fn to_metta_value(self) -> MettaValue;
}

impl ToMettaValue for i64 {
    fn to_metta_value(self) -> MettaValue {
        MettaValue::Long(self)
    }
}

impl ToMettaValue for i32 {
    fn to_metta_value(self) -> MettaValue {
        MettaValue::Long(self as i64)
    }
}

impl ToMettaValue for bool {
    fn to_metta_value(self) -> MettaValue {
        MettaValue::Bool(self)
    }
}

impl ToMettaValue for &str {
    fn to_metta_value(self) -> MettaValue {
        MettaValue::String(self.to_string())
    }
}

impl ToMettaValue for String {
    fn to_metta_value(self) -> MettaValue {
        MettaValue::String(self)
    }
}

impl ToMettaValue for MettaValue {
    fn to_metta_value(self) -> MettaValue {
        self
    }
}

/// Matcher for asserting PathMap outputs with type coercion
pub struct OutputMatcher<'a> {
    pathmap: &'a PathMapOutput,
}

impl<'a> OutputMatcher<'a> {
    /// Create a new matcher for a PathMap
    pub fn new(pathmap: &'a PathMapOutput) -> Self {
        OutputMatcher { pathmap }
    }

    /// Assert that output at index matches expected value with type coercion
    pub fn assert_output_eq<T>(&self, index: usize, expected: T) -> bool
    where
        T: ToMettaValue,
    {
        match self.pathmap.query_output(index) {
            QueryResult::Single(actual) => actual == expected.to_metta_value(),
            _ => false,
        }
    }

    /// Assert that all outputs match expected sequence with type coercion
    pub fn assert_outputs_eq<T>(&self, expected: &[T]) -> bool
    where
        T: ToMettaValue + Clone,
    {
        self.pathmap.outputs_match_sequence(expected)
    }

    /// Assert that outputs contain expected value (in any position)
    pub fn assert_outputs_contain<T>(&self, expected: T) -> bool
    where
        T: ToMettaValue,
    {
        let expected_val = expected.to_metta_value();
        self.pathmap.output.iter().any(|v| v == &expected_val)
    }

    /// Assert that outputs contain all expected values (in any order)
    pub fn assert_outputs_contain_all<T>(&self, expected: &[T]) -> bool
    where
        T: ToMettaValue + Clone,
    {
        expected.iter().all(|exp| {
            let expected_val = exp.clone().to_metta_value();
            self.pathmap.output.iter().any(|v| v == &expected_val)
        })
    }

    /// Get the number of outputs
    pub fn output_count(&self) -> usize {
        self.pathmap.output.len()
    }

    /// Find an s-expression in outputs that starts with a specific head
    pub fn find_sexpr_with_head(&self, head: &str) -> Option<&MettaValue> {
        self.pathmap
            .output
            .iter()
            .find(|v| Self::sexpr_has_head(v, head))
    }

    /// Check if a value is an s-expression with a specific head
    fn sexpr_has_head(value: &MettaValue, head: &str) -> bool {
        if let MettaValue::SExpr(elements) = value {
            if let Some(first) = elements.first() {
                match first {
                    MettaValue::String(s) | MettaValue::Atom(s) => s == head,
                    _ => false,
                }
            } else {
                false
            }
        } else {
            false
        }
    }

    /// Extract the nth element from an s-expression
    fn sexpr_get(value: &MettaValue, index: usize) -> Option<&MettaValue> {
        if let MettaValue::SExpr(elements) = value {
            elements.get(index)
        } else {
            None
        }
    }

    /// Match a "steps" s-expression against expected sequence
    /// Expects structure: ("steps", (step1, step2, ...))
    pub fn match_steps_sequence(&self, expected_steps: &[Vec<&str>]) -> bool {
        self.pathmap.output.iter().any(|v| {
            // Find any s-expression that contains a "steps" sub-expression
            Self::contains_steps_match(v, expected_steps)
        })
    }

    /// Recursively search for a "steps" s-expression and match it
    fn contains_steps_match(value: &MettaValue, expected_steps: &[Vec<&str>]) -> bool {
        // Direct match: is this a "steps" s-expression?
        if let MettaValue::SExpr(elements) = value {
            if let Some(MettaValue::String(s)) | Some(MettaValue::Atom(s)) = elements.first() {
                if s == "steps" {
                    // Get the second element which should be the sequence of steps
                    if let Some(MettaValue::SExpr(steps_list)) = elements.get(1) {
                        return Self::match_steps_list(steps_list, expected_steps);
                    }
                }
            }

            // Recursively search in nested s-expressions
            for element in elements {
                if Self::contains_steps_match(element, expected_steps) {
                    return true;
                }
            }
        }

        false
    }

    /// Match a list of step tuples against expected
    fn match_steps_list(steps: &[MettaValue], expected: &[Vec<&str>]) -> bool {
        if steps.len() != expected.len() {
            return false;
        }

        steps
            .iter()
            .zip(expected.iter())
            .all(|(actual_step, expected_step)| {
                if let MettaValue::SExpr(step_elements) = actual_step {
                    Self::match_step_tuple(step_elements, expected_step)
                } else {
                    false
                }
            })
    }

    /// Match a single step tuple like ("navigate", "room_b") or ("pickup", "box2")
    fn match_step_tuple(elements: &[MettaValue], expected: &[&str]) -> bool {
        if elements.len() != expected.len() {
            return false;
        }

        elements
            .iter()
            .zip(expected.iter())
            .all(|(actual, expected_str)| match actual {
                MettaValue::String(s) | MettaValue::Atom(s) => s == expected_str,
                _ => false,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_output() {
        let pathmap = PathMapOutput {
            source: vec![],
            environment: None,
            output: vec![
                MettaValue::Long(42),
                MettaValue::Bool(true),
                MettaValue::String("hello".to_string()),
            ],
        };

        assert_eq!(
            pathmap.query_output(0),
            QueryResult::Single(MettaValue::Long(42))
        );
        assert_eq!(pathmap.output_as_i64(0), Some(42));
        assert_eq!(pathmap.output_as_bool(1), Some(true));
        assert_eq!(pathmap.output_as_string(2), Some("hello".to_string()));
    }

    #[test]
    fn test_outputs_match_sequence() {
        let pathmap = PathMapOutput {
            source: vec![],
            environment: None,
            output: vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ],
        };

        assert!(pathmap.outputs_match_sequence(&[1i64, 2i64, 3i64]));
        assert!(!pathmap.outputs_match_sequence(&[1i64, 2i64]));
        assert!(!pathmap.outputs_match_sequence(&[1i64, 2i64, 3i64, 4i64]));
    }

    #[test]
    fn test_output_matcher() {
        let pathmap = PathMapOutput {
            source: vec![],
            environment: None,
            output: vec![MettaValue::Long(42), MettaValue::Long(100)],
        };

        let matcher = OutputMatcher::new(&pathmap);
        assert!(matcher.assert_output_eq(0, 42i64));
        assert!(matcher.assert_output_eq(1, 100i64));
        assert!(matcher.assert_outputs_eq(&[42i64, 100i64]));
        assert!(matcher.assert_outputs_contain(42i64));
        assert!(matcher.assert_outputs_contain_all(&[42i64, 100i64]));
        assert_eq!(matcher.output_count(), 2);
    }

    #[test]
    fn test_type_coercion() {
        assert_eq!(42i64.to_metta_value(), MettaValue::Long(42));
        assert_eq!(42i32.to_metta_value(), MettaValue::Long(42));
        assert_eq!(true.to_metta_value(), MettaValue::Bool(true));
        assert_eq!(
            "hello".to_metta_value(),
            MettaValue::String("hello".to_string())
        );
    }

    #[test]
    fn test_query_result_type_extraction() {
        let result = QueryResult::Single(MettaValue::Long(42));
        assert_eq!(result.as_i64(), Some(42));

        let result = QueryResult::Single(MettaValue::String("hello".to_string()));
        assert_eq!(result.as_string(), Some("hello".to_string()));

        let result = QueryResult::Single(MettaValue::Bool(true));
        assert_eq!(result.as_bool(), Some(true));
    }

    #[test]
    fn test_query_sexpr() {
        let pathmap = PathMapOutput {
            source: vec![],
            environment: None,
            output: vec![
                MettaValue::SExpr(vec![
                    MettaValue::String("plan".to_string()),
                    MettaValue::String("objective1".to_string()),
                ]),
                MettaValue::SExpr(vec![
                    MettaValue::String("route".to_string()),
                    MettaValue::String("path1".to_string()),
                ]),
            ],
        };

        // Query first plan
        let result = pathmap.query_sexpr("plan");
        assert!(!result.is_empty());
        assert_eq!(
            result.as_sexpr().unwrap()[0],
            MettaValue::String("plan".to_string())
        );

        // Query all (should find one)
        let result = pathmap.query_all_sexpr("plan");
        assert_eq!(result.count(), 1);

        // Query non-existent
        let result = pathmap.query_sexpr("missing");
        assert!(result.is_empty());
    }

    #[test]
    fn test_query_descendant() {
        let nested = MettaValue::SExpr(vec![
            MettaValue::String("plan".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::String("route".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::String("waypoints".to_string()),
                    MettaValue::String("room_a".to_string()),
                ]),
            ]),
        ]);

        let pathmap = PathMapOutput {
            source: vec![],
            environment: None,
            output: vec![nested],
        };

        // Find deeply nested waypoints
        let result = pathmap.query_descendant("waypoints");
        assert!(!result.is_empty());
        let sexpr = result.as_sexpr().unwrap();
        assert_eq!(sexpr[0], MettaValue::String("waypoints".to_string()));
        assert_eq!(sexpr[1], MettaValue::String("room_a".to_string()));
    }

    #[test]
    fn test_query_path() {
        let nested = MettaValue::SExpr(vec![
            MettaValue::String("plan".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::String("route".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::String("waypoints".to_string()),
                    MettaValue::String("room_a".to_string()),
                    MettaValue::String("room_b".to_string()),
                ]),
            ]),
        ]);

        let pathmap = PathMapOutput {
            source: vec![],
            environment: None,
            output: vec![nested],
        };

        // Navigate path: plan -> route -> waypoints
        let result = pathmap.query_path(&["plan", "route", "waypoints"]);
        assert!(!result.is_empty());
        let sexpr = result.as_sexpr().unwrap();
        assert_eq!(sexpr[0], MettaValue::String("waypoints".to_string()));
    }

    #[test]
    fn test_query_all_descendants() {
        let nested = MettaValue::SExpr(vec![
            MettaValue::String("plan".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::String("steps".to_string()),
                MettaValue::SExpr(vec![
                    MettaValue::String("navigate".to_string()),
                    MettaValue::String("room_a".to_string()),
                ]),
                MettaValue::SExpr(vec![
                    MettaValue::String("navigate".to_string()),
                    MettaValue::String("room_b".to_string()),
                ]),
            ]),
        ]);

        let pathmap = PathMapOutput {
            source: vec![],
            environment: None,
            output: vec![nested],
        };

        // Find all navigate expressions
        let result = pathmap.query_all_descendants("navigate");
        assert_eq!(result.count(), 2);
    }

    #[test]
    fn test_filter_contains() {
        let pathmap = PathMapOutput {
            source: vec![],
            environment: None,
            output: vec![
                MettaValue::SExpr(vec![
                    MettaValue::String("plan".to_string()),
                    MettaValue::String("ball1".to_string()),
                ]),
                MettaValue::SExpr(vec![
                    MettaValue::String("plan".to_string()),
                    MettaValue::String("box2".to_string()),
                ]),
            ],
        };

        // Filter for ball1
        let result = pathmap.filter_contains("ball1");
        assert_eq!(result.count(), 1);

        // Filter for box2
        let result = pathmap.filter_contains("box2");
        assert_eq!(result.count(), 1);
    }
}
