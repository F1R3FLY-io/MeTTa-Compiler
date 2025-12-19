use crate::ir::MettaExpr;

use std::sync::Arc;

/// Represents a MeTTa value as an s-expression
/// S-expressions are nested lists with textual operator names
#[derive(Debug, Clone, PartialEq)]
pub enum MettaValue {
    /// An atom (symbol, variable, or literal)
    Atom(String),
    /// A boolean literal
    Bool(bool),
    /// An integer literal
    Long(i64),
    /// A floating point literal
    Float(f64),
    /// A string literal
    String(String),
    /// An s-expression (list of values)
    SExpr(Vec<MettaValue>),
    /// Nil/empty
    Nil,
    /// An error with message and details (Arc for O(1) clone)
    Error(String, Arc<MettaValue>),
    /// A type (first-class types as atoms, Arc for O(1) clone)
    Type(Arc<MettaValue>),
    /// A conjunction of goals (MORK-style logical AND)
    /// Represents (,), (, expr), or (, expr1 expr2 ...)
    /// Goals are evaluated left-to-right with variable binding threading
    Conjunction(Vec<MettaValue>),
}

impl MettaValue {
    /// Create a quoted expression: (quote inner)
    ///
    /// Returns a quote special form that prevents evaluation of the inner expression.
    /// Equivalent to the MeTTa syntax: 'inner
    ///
    /// # Example
    /// ```ignore
    /// let expr = MettaValue::Atom("x".to_string());
    /// let quoted = MettaValue::quote(expr);
    /// // Produces: (quote x)
    /// ```
    pub fn quote(inner: Self) -> Self {
        MettaValue::SExpr(vec![MettaValue::Atom("quote".to_string()), inner])
    }

    /// Check if this value is a ground type (non-reducible literal)
    /// Ground types: Bool, Long, Float, String, Nil
    /// Returns true if the value doesn't require further evaluation
    pub fn is_ground_type(&self) -> bool {
        matches!(
            self,
            MettaValue::Bool(_)
                | MettaValue::Long(_)
                | MettaValue::Float(_)
                | MettaValue::String(_)
                | MettaValue::Nil
        )
    }

    /// Convert MettaValue to a friendly type name for error messages
    /// This provides user-friendly type names instead of debug format like "Long(5)"
    pub fn friendly_type_name(&self) -> &'static str {
        match self {
            MettaValue::Long(_) => "Number (integer)",
            MettaValue::Float(_) => "Number (float)",
            MettaValue::Bool(_) => "Bool",
            MettaValue::String(_) => "String",
            MettaValue::Atom(_) => "Atom",
            MettaValue::Nil => "Nil",
            MettaValue::SExpr(_) => "S-expression",
            MettaValue::Error(_, _) => "Error",
            MettaValue::Type(_) => "Type",
            MettaValue::Conjunction(_) => "Conjunction",
        }
    }

    /// Check if this is an evaluation expression (starts with "!")
    /// Evaluation expressions like `!(+ 1 2)` should produce output
    pub fn is_eval_expr(&self) -> bool {
        matches!(self, MettaValue::SExpr(items)
            if items.first().map(|v| matches!(v, MettaValue::Atom(s) if s == "!")).unwrap_or(false))
    }

    /// Check if this is a rule definition (starts with "=")
    /// Rule definitions like `(= (double $x) (* $x 2))` add rules to the environment
    pub fn is_rule_def(&self) -> bool {
        matches!(self, MettaValue::SExpr(items)
            if items.first().map(|v| matches!(v, MettaValue::Atom(s) if s == "=")).unwrap_or(false))
    }

    /// Check structural equivalence (ignoring variable names)
    /// Two expressions are structurally equivalent if they have the same structure,
    /// with variables in the same positions (regardless of variable names)
    pub fn structurally_equivalent(&self, other: &MettaValue) -> bool {
        match (self, other) {
            // Variables match any other variable (names don't matter)
            // EXCEPT: standalone "&" is a literal operator (used in match), not a variable
            (MettaValue::Atom(a), MettaValue::Atom(b))
                if (a.starts_with('$') || a.starts_with('&') || a.starts_with('\''))
                    && (b.starts_with('$') || b.starts_with('&') || b.starts_with('\''))
                    && a != "&"
                    && b != "&" =>
            {
                true
            }

            // Wildcards match wildcards
            (MettaValue::Atom(a), MettaValue::Atom(b)) if a == "_" && b == "_" => true,

            // Non-variable atoms must match exactly (including standalone "&")
            (MettaValue::Atom(a), MettaValue::Atom(b)) => a == b,

            // Other ground types must match exactly
            (MettaValue::Bool(a), MettaValue::Bool(b)) => a == b,
            (MettaValue::Long(a), MettaValue::Long(b)) => a == b,
            (MettaValue::Float(a), MettaValue::Float(b)) => a == b,
            (MettaValue::String(a), MettaValue::String(b)) => a == b,
            (MettaValue::Nil, MettaValue::Nil) => true,

            // S-expressions must have same structure
            (MettaValue::SExpr(a_items), MettaValue::SExpr(b_items)) => {
                if a_items.len() != b_items.len() {
                    return false;
                }
                a_items
                    .iter()
                    .zip(b_items.iter())
                    .all(|(a, b)| a.structurally_equivalent(b))
            }

            // Errors must have same message and equivalent details
            (MettaValue::Error(a_msg, a_details), MettaValue::Error(b_msg, b_details)) => {
                a_msg == b_msg && a_details.structurally_equivalent(b_details)
            }

            // Types must be structurally equivalent
            (MettaValue::Type(a), MettaValue::Type(b)) => a.structurally_equivalent(b),

            // Conjunctions must have same structure
            (MettaValue::Conjunction(a_goals), MettaValue::Conjunction(b_goals)) => {
                if a_goals.len() != b_goals.len() {
                    return false;
                }
                a_goals
                    .iter()
                    .zip(b_goals.iter())
                    .all(|(a, b)| a.structurally_equivalent(b))
            }

            _ => false,
        }
    }

    /// Extract the head symbol from a pattern for indexing
    /// Returns None if the pattern doesn't have a clear head symbol
    pub fn get_head_symbol(&self) -> Option<&str> {
        match self {
            // For s-expressions like (double $x), extract "double"
            // EXCEPT: standalone "&" is allowed as a head symbol (used in match)
            MettaValue::SExpr(items) if !items.is_empty() => match &items[0] {
                MettaValue::Atom(head)
                    if !head.starts_with('$')
                        && (!head.starts_with('&') || head == "&")
                        && !head.starts_with('\'')
                        && head != "_" =>
                {
                    Some(head.as_str())
                }
                _ => None,
            },
            // For bare atoms like foo, use the atom itself
            // EXCEPT: standalone "&" is allowed as a head symbol (used in match)
            MettaValue::Atom(head)
                if !head.starts_with('$')
                    && (!head.starts_with('&') || head == "&")
                    && !head.starts_with('\'')
                    && head != "_" =>
            {
                Some(head.as_str())
            }
            _ => None,
        }
    }

    /// Get the arity (number of arguments) for an s-expression
    /// For (head arg1 arg2 arg3), arity is 3
    /// For bare atoms, arity is 0
    pub fn get_arity(&self) -> usize {
        match self {
            MettaValue::SExpr(items) if !items.is_empty() => items.len() - 1, // Exclude head
            _ => 0,
        }
    }

    /// Convert MettaValue to MORK s-expression string format
    /// This format can be parsed by MORK's parser
    pub fn to_mork_string(&self) -> String {
        match self {
            MettaValue::Atom(s) => {
                // Variables need to start with $ in MORK format
                // EXCEPT: standalone "&" is a literal operator (used in match), not a variable
                if (s.starts_with('$') || s.starts_with('&') || s.starts_with('\'')) && s != "&" {
                    format!("${}", &s[1..]) // Keep $ prefix, remove original prefix
                } else if s == "_" {
                    "$".to_string() // Wildcard becomes $
                } else {
                    s.clone()
                }
            }
            MettaValue::Bool(b) => b.to_string(),
            MettaValue::Long(n) => n.to_string(),
            MettaValue::Float(f) => f.to_string(),
            MettaValue::String(s) => format!("\"{}\"", s),
            MettaValue::SExpr(items) => {
                let inner = items
                    .iter()
                    .map(|v| v.to_mork_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("({})", inner)
            }
            MettaValue::Nil => "()".to_string(),
            MettaValue::Error(msg, details) => {
                format!("(error \"{}\" {})", msg, details.to_mork_string())
            }
            MettaValue::Type(t) => t.to_mork_string(),
            MettaValue::Conjunction(goals) => {
                let inner = goals
                    .iter()
                    .map(|v| v.to_mork_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("(, {})", inner)
            }
        }
    }

    /// Convert MettaValue to a JSON-like string representation
    /// Used for debugging and human-readable output
    pub fn to_json_string(&self) -> String {
        match self {
            MettaValue::Atom(s) => format!(r#"{{"type":"atom","value":"{}"}}"#, escape_json(s)),
            MettaValue::Bool(b) => format!(r#"{{"type":"bool","value":{}}}"#, b),
            MettaValue::Long(n) => format!(r#"{{"type":"number","value":{}}}"#, n),
            MettaValue::Float(f) => format!(r#"{{"type":"float","value":{}}}"#, f),
            MettaValue::String(s) => format!(r#"{{"type":"string","value":"{}"}}"#, escape_json(s)),
            MettaValue::Nil => r#"{"type":"nil"}"#.to_string(),
            MettaValue::SExpr(items) => {
                let items_json: Vec<String> =
                    items.iter().map(|value| value.to_json_string()).collect();
                format!(r#"{{"type":"sexpr","items":[{}]}}"#, items_json.join(","))
            }
            MettaValue::Error(msg, details) => {
                format!(
                    r#"{{"type":"error","message":"{}","details":{}}}"#,
                    escape_json(msg),
                    details.to_json_string()
                )
            }
            MettaValue::Type(t) => {
                format!(r#"{{"type":"metatype","value":{}}}"#, t.to_json_string())
            }
            MettaValue::Conjunction(goals) => {
                let goals_json: Vec<String> =
                    goals.iter().map(|value| value.to_json_string()).collect();
                format!(
                    r#"{{"type":"conjunction","goals":[{}]}}"#,
                    goals_json.join(",")
                )
            }
        }
    }
}

pub fn escape_json(s: &str) -> String {
    s.replace('\\', r"\\")
        .replace('"', r#"\""#)
        .replace('\n', r"\n")
        .replace('\r', r"\r")
        .replace('\t', r"\t")
}

// Implement Eq for MettaValue (required for HashMap keys)
// Note: Float uses bit-level comparison for hashing purposes
impl Eq for MettaValue {}

// Implement Hash for MettaValue to enable use as HashMap key
impl std::hash::Hash for MettaValue {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            MettaValue::Atom(s) => {
                0u8.hash(state);
                s.hash(state);
            }
            MettaValue::Bool(b) => {
                1u8.hash(state);
                b.hash(state);
            }
            MettaValue::Long(n) => {
                2u8.hash(state);
                n.hash(state);
            }
            MettaValue::Float(f) => {
                3u8.hash(state);
                // Hash float as its bit representation for deterministic hashing
                f.to_bits().hash(state);
            }
            MettaValue::String(s) => {
                4u8.hash(state);
                s.hash(state);
            }
            MettaValue::SExpr(items) => {
                5u8.hash(state);
                items.hash(state);
            }
            MettaValue::Nil => {
                6u8.hash(state);
            }
            MettaValue::Error(msg, details) => {
                7u8.hash(state);
                msg.hash(state);
                details.hash(state);
            }
            MettaValue::Type(t) => {
                8u8.hash(state);
                t.hash(state);
            }
            MettaValue::Conjunction(goals) => {
                10u8.hash(state);
                goals.hash(state);
            }
        }
    }
}

impl TryFrom<&MettaExpr> for MettaValue {
    type Error = String;

    fn try_from(sexpr: &MettaExpr) -> Result<Self, String> {
        match sexpr {
            MettaExpr::Atom(s, _span) => {
                // Parse literals (MeTTa uses capitalized True/False per hyperon-experimental)
                match s.as_str() {
                    "True" => Ok(MettaValue::Bool(true)),
                    "False" => Ok(MettaValue::Bool(false)),
                    _ => {
                        // Keep the original symbol as-is (including operators like +, -, *, etc.)
                        Ok(MettaValue::Atom(s.clone()))
                    }
                }
            }
            MettaExpr::String(s, _span) => Ok(MettaValue::String(s.clone())),
            MettaExpr::Integer(n, _span) => Ok(MettaValue::Long(*n)),
            MettaExpr::Float(f, _span) => Ok(MettaValue::Float(*f)),
            MettaExpr::List(items, _span) => {
                if items.is_empty() {
                    Ok(MettaValue::Nil)
                } else {
                    // Check if this is a conjunction: (,) or (, expr1 expr2 ...)
                    let is_conjunction = items
                        .first()
                        .is_some_and(|first| matches!(first, MettaExpr::Atom(s, _) if s == ","));

                    if is_conjunction {
                        // Convert to Conjunction variant (skip the comma operator)
                        let goals: Result<Vec<_>, _> =
                            items[1..].iter().map(MettaValue::try_from).collect();
                        Ok(MettaValue::Conjunction(goals?))
                    } else {
                        // Regular S-expression
                        let values: Result<Vec<_>, _> =
                            items.iter().map(MettaValue::try_from).collect();
                        Ok(MettaValue::SExpr(values?))
                    }
                }
            }
            MettaExpr::Quoted(expr, _span) => {
                // For quoted expressions, wrap in a quote operator
                let inner = MettaValue::try_from(expr.as_ref())?;
                Ok(MettaValue::quote(inner))
            }
        }
    }
}

// Idiomatic From trait implementations for convenient MettaValue construction
impl From<bool> for MettaValue {
    fn from(b: bool) -> Self {
        MettaValue::Bool(b)
    }
}

impl From<i64> for MettaValue {
    fn from(n: i64) -> Self {
        MettaValue::Long(n)
    }
}

impl From<f64> for MettaValue {
    fn from(f: f64) -> Self {
        MettaValue::Float(f)
    }
}

impl From<String> for MettaValue {
    fn from(s: String) -> Self {
        MettaValue::String(s)
    }
}

impl From<&str> for MettaValue {
    fn from(s: &str) -> Self {
        MettaValue::Atom(s.to_string())
    }
}

impl From<Vec<MettaValue>> for MettaValue {
    fn from(items: Vec<MettaValue>) -> Self {
        if items.is_empty() {
            MettaValue::Nil
        } else {
            MettaValue::SExpr(items)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests for is_ground_type
    #[test]
    fn test_is_ground_type_bool() {
        assert!(MettaValue::Bool(true).is_ground_type());
        assert!(MettaValue::Bool(false).is_ground_type());
    }

    #[test]
    fn test_is_ground_type_long() {
        assert!(MettaValue::Long(0).is_ground_type());
        assert!(MettaValue::Long(42).is_ground_type());
        assert!(MettaValue::Long(-100).is_ground_type());
    }

    #[test]
    fn test_is_ground_type_string() {
        assert!(MettaValue::String("hello".to_string()).is_ground_type());
        assert!(MettaValue::String("".to_string()).is_ground_type());
    }

    #[test]
    fn test_is_ground_type_nil() {
        assert!(MettaValue::Nil.is_ground_type());
    }

    #[test]
    fn test_is_ground_type_atom() {
        assert!(!MettaValue::Atom("test".to_string()).is_ground_type());
    }

    #[test]
    fn test_is_ground_type_sexpr() {
        assert!(!MettaValue::SExpr(vec![MettaValue::Long(1)]).is_ground_type());
    }

    #[test]
    fn test_is_ground_type_error() {
        assert!(!MettaValue::Error("msg".to_string(), Arc::new(MettaValue::Nil)).is_ground_type());
    }

    #[test]
    fn test_is_ground_type_type() {
        assert!(!MettaValue::Type(Arc::new(MettaValue::Atom("Int".to_string()))).is_ground_type());
    }

    // Tests for is_eval_expr
    #[test]
    fn test_is_eval_expr_with_bang() {
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("!".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
            ]),
        ]);
        assert!(value.is_eval_expr());
    }

    #[test]
    fn test_is_eval_expr_with_bang_and_atom() {
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("!".to_string()),
            MettaValue::Atom("foo".to_string()),
        ]);
        assert!(value.is_eval_expr());
    }

    #[test]
    fn test_is_eval_expr_without_bang() {
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        assert!(!value.is_eval_expr());
    }

    #[test]
    fn test_is_eval_expr_empty_sexpr() {
        let value = MettaValue::SExpr(vec![]);
        assert!(!value.is_eval_expr());
    }

    #[test]
    fn test_is_eval_expr_with_equals() {
        // Rule definition should not be eval expr
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::Atom("x".to_string()),
            MettaValue::Long(1),
        ]);
        assert!(!value.is_eval_expr());
    }

    #[test]
    fn test_is_eval_expr_non_sexpr_types() {
        // Non-SExpr types should return false
        assert!(!MettaValue::Atom("!".to_string()).is_eval_expr());
        assert!(!MettaValue::Bool(true).is_eval_expr());
        assert!(!MettaValue::Long(42).is_eval_expr());
        assert!(!MettaValue::String("!".to_string()).is_eval_expr());
        assert!(!MettaValue::Nil.is_eval_expr());
    }

    #[test]
    fn test_is_eval_expr_with_non_atom_first() {
        // SExpr with non-atom first element should return false
        let value = MettaValue::SExpr(vec![MettaValue::Long(1), MettaValue::Long(2)]);
        assert!(!value.is_eval_expr());
    }

    // Tests for is_rule_def
    #[test]
    fn test_is_rule_def_with_equals() {
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("double".to_string()),
                MettaValue::Atom("$x".to_string()),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Atom("$x".to_string()),
                MettaValue::Long(2),
            ]),
        ]);
        assert!(value.is_rule_def());
    }

    #[test]
    fn test_is_rule_def_with_equals_simple() {
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::Atom("x".to_string()),
            MettaValue::Long(1),
        ]);
        assert!(value.is_rule_def());
    }

    #[test]
    fn test_is_rule_def_without_equals() {
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        assert!(!value.is_rule_def());
    }

    #[test]
    fn test_is_rule_def_with_bang() {
        // Eval expression should not be rule def
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("!".to_string()),
            MettaValue::Atom("foo".to_string()),
        ]);
        assert!(!value.is_rule_def());
    }

    #[test]
    fn test_is_rule_def_empty_sexpr() {
        let value = MettaValue::SExpr(vec![]);
        assert!(!value.is_rule_def());
    }

    #[test]
    fn test_is_rule_def_non_sexpr_types() {
        // Non-SExpr types should return false
        assert!(!MettaValue::Atom("=".to_string()).is_rule_def());
        assert!(!MettaValue::Bool(true).is_rule_def());
        assert!(!MettaValue::Long(42).is_rule_def());
        assert!(!MettaValue::String("=".to_string()).is_rule_def());
        assert!(!MettaValue::Nil.is_rule_def());
    }

    #[test]
    fn test_is_rule_def_with_non_atom_first() {
        // SExpr with non-atom first element should return false
        let value = MettaValue::SExpr(vec![MettaValue::Long(1), MettaValue::Long(2)]);
        assert!(!value.is_rule_def());
    }

    #[test]
    fn test_is_eval_expr_and_rule_def_mutually_exclusive() {
        // An expression cannot be both eval expr and rule def
        let eval_expr = MettaValue::SExpr(vec![
            MettaValue::Atom("!".to_string()),
            MettaValue::Atom("foo".to_string()),
        ]);
        assert!(eval_expr.is_eval_expr());
        assert!(!eval_expr.is_rule_def());

        let rule_def = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            MettaValue::Atom("x".to_string()),
            MettaValue::Long(1),
        ]);
        assert!(!rule_def.is_eval_expr());
        assert!(rule_def.is_rule_def());
    }

    // Tests for structurally_equivalent
    #[test]
    fn test_structurally_equivalent_variables() {
        // Variables match regardless of name
        let v1 = MettaValue::Atom("$x".to_string());
        let v2 = MettaValue::Atom("$y".to_string());
        assert!(v1.structurally_equivalent(&v2));

        let v3 = MettaValue::Atom("&a".to_string());
        let v4 = MettaValue::Atom("&b".to_string());
        assert!(v3.structurally_equivalent(&v4));

        let v5 = MettaValue::Atom("'x".to_string());
        let v6 = MettaValue::Atom("'y".to_string());
        assert!(v5.structurally_equivalent(&v6));
    }

    #[test]
    fn test_structurally_equivalent_variables_mixed_prefixes() {
        // Variables with different prefixes still match
        let v1 = MettaValue::Atom("$x".to_string());
        let v2 = MettaValue::Atom("&y".to_string());
        assert!(v1.structurally_equivalent(&v2));

        let v3 = MettaValue::Atom("'a".to_string());
        let v4 = MettaValue::Atom("$b".to_string());
        assert!(v3.structurally_equivalent(&v4));
    }

    #[test]
    fn test_structurally_equivalent_standalone_ampersand() {
        // Standalone "&" is NOT a variable, it's a literal operator
        let op = MettaValue::Atom("&".to_string());
        let var = MettaValue::Atom("$x".to_string());
        assert!(!op.structurally_equivalent(&var));

        // Standalone "&" matches itself
        assert!(op.structurally_equivalent(&op));
    }

    #[test]
    fn test_structurally_equivalent_wildcards() {
        let w1 = MettaValue::Atom("_".to_string());
        let w2 = MettaValue::Atom("_".to_string());
        assert!(w1.structurally_equivalent(&w2));

        // Wildcard doesn't match variable
        let var = MettaValue::Atom("$x".to_string());
        assert!(!w1.structurally_equivalent(&var));
    }

    #[test]
    fn test_structurally_equivalent_atoms() {
        // Non-variable atoms must match exactly
        assert!(MettaValue::Atom("foo".to_string())
            .structurally_equivalent(&MettaValue::Atom("foo".to_string())));
        assert!(!MettaValue::Atom("foo".to_string())
            .structurally_equivalent(&MettaValue::Atom("bar".to_string())));
    }

    #[test]
    fn test_structurally_equivalent_ground_types() {
        assert!(MettaValue::Bool(true).structurally_equivalent(&MettaValue::Bool(true)));
        assert!(!MettaValue::Bool(true).structurally_equivalent(&MettaValue::Bool(false)));

        assert!(MettaValue::Long(42).structurally_equivalent(&MettaValue::Long(42)));
        assert!(!MettaValue::Long(42).structurally_equivalent(&MettaValue::Long(43)));

        assert!(MettaValue::String("hello".to_string())
            .structurally_equivalent(&MettaValue::String("hello".to_string())));
        assert!(!MettaValue::String("hello".to_string())
            .structurally_equivalent(&MettaValue::String("world".to_string())));

        assert!(MettaValue::Nil.structurally_equivalent(&MettaValue::Nil));
    }

    #[test]
    fn test_structurally_equivalent_sexpr() {
        // Same structure
        let s1 = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let s2 = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        assert!(s1.structurally_equivalent(&s2));

        // Different structure
        let s3 = MettaValue::SExpr(vec![MettaValue::Atom("+".to_string()), MettaValue::Long(1)]);
        assert!(!s1.structurally_equivalent(&s3));
    }

    #[test]
    fn test_structurally_equivalent_sexpr_with_variables() {
        // Variables in same positions match
        let s1 = MettaValue::SExpr(vec![
            MettaValue::Atom("double".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);
        let s2 = MettaValue::SExpr(vec![
            MettaValue::Atom("double".to_string()),
            MettaValue::Atom("$y".to_string()),
        ]);
        assert!(s1.structurally_equivalent(&s2));
    }

    #[test]
    fn test_structurally_equivalent_errors() {
        let e1 = MettaValue::Error("msg".to_string(), Arc::new(MettaValue::Long(1)));
        let e2 = MettaValue::Error("msg".to_string(), Arc::new(MettaValue::Long(1)));
        assert!(e1.structurally_equivalent(&e2));

        let e3 = MettaValue::Error("msg".to_string(), Arc::new(MettaValue::Long(2)));
        assert!(!e1.structurally_equivalent(&e3));

        let e4 = MettaValue::Error("other".to_string(), Arc::new(MettaValue::Long(1)));
        assert!(!e1.structurally_equivalent(&e4));
    }

    #[test]
    fn test_structurally_equivalent_types() {
        let t1 = MettaValue::Type(Arc::new(MettaValue::Atom("Int".to_string())));
        let t2 = MettaValue::Type(Arc::new(MettaValue::Atom("Int".to_string())));
        assert!(t1.structurally_equivalent(&t2));

        let t3 = MettaValue::Type(Arc::new(MettaValue::Atom("String".to_string())));
        assert!(!t1.structurally_equivalent(&t3));
    }

    #[test]
    fn test_structurally_equivalent_different_types() {
        // Different enum variants are not equivalent
        assert!(!MettaValue::Bool(true).structurally_equivalent(&MettaValue::Long(1)));
        assert!(!MettaValue::Atom("x".to_string()).structurally_equivalent(&MettaValue::Long(1)));
        assert!(!MettaValue::Nil.structurally_equivalent(&MettaValue::Long(0)));
    }

    // Tests for get_head_symbol
    #[test]
    fn test_get_head_symbol_sexpr() {
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("double".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);
        assert_eq!(value.get_head_symbol(), Some("double"));
    }

    #[test]
    fn test_get_head_symbol_bare_atom() {
        let value = MettaValue::Atom("foo".to_string());
        assert_eq!(value.get_head_symbol(), Some("foo"));
    }

    #[test]
    fn test_get_head_symbol_standalone_ampersand() {
        // Standalone "&" is allowed as head symbol
        let value = MettaValue::Atom("&".to_string());
        assert_eq!(value.get_head_symbol(), Some("&"));

        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("&".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);
        assert_eq!(sexpr.get_head_symbol(), Some("&"));
    }

    #[test]
    fn test_get_head_symbol_variable_atom() {
        // Variables cannot be head symbols
        assert_eq!(MettaValue::Atom("$x".to_string()).get_head_symbol(), None);
        assert_eq!(MettaValue::Atom("&y".to_string()).get_head_symbol(), None);
        assert_eq!(MettaValue::Atom("'z".to_string()).get_head_symbol(), None);
    }

    #[test]
    fn test_get_head_symbol_wildcard() {
        // Wildcard cannot be head symbol
        assert_eq!(MettaValue::Atom("_".to_string()).get_head_symbol(), None);
    }

    #[test]
    fn test_get_head_symbol_sexpr_with_variable_head() {
        // S-expression with variable as first element has no head symbol
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("$x".to_string()),
            MettaValue::Long(1),
        ]);
        assert_eq!(value.get_head_symbol(), None);
    }

    #[test]
    fn test_get_head_symbol_sexpr_with_non_atom_head() {
        // S-expression with non-atom first element has no head symbol
        let value = MettaValue::SExpr(vec![MettaValue::Long(1), MettaValue::Long(2)]);
        assert_eq!(value.get_head_symbol(), None);
    }

    #[test]
    fn test_get_head_symbol_empty_sexpr() {
        let value = MettaValue::SExpr(vec![]);
        assert_eq!(value.get_head_symbol(), None);
    }

    #[test]
    fn test_get_head_symbol_nested_sexpr() {
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("add".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("mul".to_string()),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
        ]);
        assert_eq!(value.get_head_symbol(), Some("add"));
    }

    #[test]
    fn test_get_head_symbol_other_types() {
        // Non-atom, non-sexpr types have no head symbol
        assert_eq!(MettaValue::Bool(true).get_head_symbol(), None);
        assert_eq!(MettaValue::Long(42).get_head_symbol(), None);
        assert_eq!(
            MettaValue::String("test".to_string()).get_head_symbol(),
            None
        );
        assert_eq!(MettaValue::Nil.get_head_symbol(), None);
    }

    // Tests for to_mork_string
    #[test]
    fn test_to_mork_string_atom() {
        assert_eq!(MettaValue::Atom("foo".to_string()).to_mork_string(), "foo");
    }

    #[test]
    fn test_to_mork_string_variable_dollar() {
        assert_eq!(MettaValue::Atom("$x".to_string()).to_mork_string(), "$x");
    }

    #[test]
    fn test_to_mork_string_variable_ampersand() {
        // & prefix becomes $ in MORK format
        assert_eq!(MettaValue::Atom("&y".to_string()).to_mork_string(), "$y");
    }

    #[test]
    fn test_to_mork_string_variable_quote() {
        // ' prefix becomes $ in MORK format
        assert_eq!(MettaValue::Atom("'z".to_string()).to_mork_string(), "$z");
    }

    #[test]
    fn test_to_mork_string_standalone_ampersand() {
        // Standalone "&" is NOT converted (it's a literal operator)
        assert_eq!(MettaValue::Atom("&".to_string()).to_mork_string(), "&");
    }

    #[test]
    fn test_to_mork_string_wildcard() {
        // Wildcard "_" becomes "$" in MORK format
        assert_eq!(MettaValue::Atom("_".to_string()).to_mork_string(), "$");
    }

    #[test]
    fn test_to_mork_string_bool() {
        assert_eq!(MettaValue::Bool(true).to_mork_string(), "true");
        assert_eq!(MettaValue::Bool(false).to_mork_string(), "false");
    }

    #[test]
    fn test_to_mork_string_long() {
        assert_eq!(MettaValue::Long(42).to_mork_string(), "42");
        assert_eq!(MettaValue::Long(-10).to_mork_string(), "-10");
        assert_eq!(MettaValue::Long(0).to_mork_string(), "0");
    }

    #[test]
    fn test_to_mork_string_string() {
        assert_eq!(
            MettaValue::String("hello".to_string()).to_mork_string(),
            "\"hello\""
        );
        assert_eq!(MettaValue::String("".to_string()).to_mork_string(), "\"\"");
    }

    #[test]
    fn test_to_mork_string_nil() {
        assert_eq!(MettaValue::Nil.to_mork_string(), "()");
    }

    #[test]
    fn test_to_mork_string_sexpr() {
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        assert_eq!(value.to_mork_string(), "(+ 1 2)");
    }

    #[test]
    fn test_to_mork_string_sexpr_nested() {
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
        ]);
        assert_eq!(value.to_mork_string(), "(+ 1 (* 2 3))");
    }

    #[test]
    fn test_to_mork_string_sexpr_with_variables() {
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("double".to_string()),
            MettaValue::Atom("$x".to_string()),
        ]);
        assert_eq!(value.to_mork_string(), "(double $x)");
    }

    #[test]
    fn test_to_mork_string_sexpr_with_ampersand_variable() {
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("f".to_string()),
            MettaValue::Atom("&y".to_string()),
        ]);
        assert_eq!(value.to_mork_string(), "(f $y)");
    }

    #[test]
    fn test_to_mork_string_error() {
        let value = MettaValue::Error("test error".to_string(), Arc::new(MettaValue::Long(42)));
        assert_eq!(value.to_mork_string(), "(error \"test error\" 42)");
    }

    #[test]
    fn test_to_mork_string_type() {
        let value = MettaValue::Type(Arc::new(MettaValue::Atom("Int".to_string())));
        assert_eq!(value.to_mork_string(), "Int");
    }

    #[test]
    fn test_to_mork_string_empty_sexpr() {
        let value = MettaValue::SExpr(vec![]);
        assert_eq!(value.to_mork_string(), "()");
    }

    #[test]
    fn test_to_json_string_atom() {
        let value = MettaValue::Atom("test".to_string());
        let json = value.to_json_string();
        assert_eq!(json, r#"{"type":"atom","value":"test"}"#);
    }

    #[test]
    fn test_to_json_string_number() {
        let value = MettaValue::Long(42);
        let json = value.to_json_string();
        assert_eq!(json, r#"{"type":"number","value":42}"#);
    }

    #[test]
    fn test_to_json_string_bool() {
        let value = MettaValue::Bool(true);
        let json = value.to_json_string();
        assert_eq!(json, r#"{"type":"bool","value":true}"#);
    }

    #[test]
    fn test_to_json_string_string() {
        let value = MettaValue::String("hello".to_string());
        let json = value.to_json_string();
        assert_eq!(json, r#"{"type":"string","value":"hello"}"#);
    }

    #[test]
    fn test_to_json_string_nil() {
        let value = MettaValue::Nil;
        let json = value.to_json_string();
        assert_eq!(json, r#"{"type":"nil"}"#);
    }

    #[test]
    fn test_to_json_string_sexpr() {
        let value = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let json = value.to_json_string();
        assert!(json.contains(r#""type":"sexpr""#));
        assert!(json.contains(r#""items""#));
    }

    #[test]
    fn test_to_json_string_escape_json() {
        // Test escape_json indirectly through to_json_string
        let value = MettaValue::String("hello\n\"world\"\\test".to_string());
        let json = value.to_json_string();
        // The escaped string should be properly escaped in the JSON
        assert!(json.contains(r#"\n"#));
        assert!(json.contains(r#"\""#));
        assert!(json.contains(r#"\\"#));
    }
}
