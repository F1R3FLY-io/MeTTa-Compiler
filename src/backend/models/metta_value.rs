use crate::ir::MettaExpr;

use std::sync::Arc;

use super::MemoHandle;
use super::SpaceHandle;

// Re-import String to avoid shadowing by MettaValue::String associated function
use std::string::String as StdString;

/// Reference-counted MettaValue with O(1) clone.
/// Clone increments Arc reference count; drop decrements it.
///
/// This wrapper provides:
/// - O(1) clone operations (just Arc reference count increment)
/// - Efficient sharing of immutable values
/// - Transparent construction via associated functions (MettaValue::Atom(), etc.)
#[derive(Clone)]
pub struct MettaValue(Arc<MettaValueInner>);

/// Internal enum containing the actual value variants.
/// All the MeTTa value types are represented here.
#[derive(Debug, Clone, PartialEq)]
pub enum MettaValueInner {
    /// An atom (symbol, variable, or literal)
    Atom(StdString),
    /// A boolean literal
    Bool(bool),
    /// An integer literal
    Long(i64),
    /// A floating point literal
    Float(f64),
    /// A string literal
    String(StdString),
    /// An s-expression (list of values)
    SExpr(Vec<MettaValue>),
    /// Nil/empty
    Nil,
    /// An error with message and details
    Error(StdString, MettaValue),
    /// A type (first-class types as atoms)
    Type(MettaValue),
    /// A conjunction of goals (MORK-style logical AND)
    /// Represents (,), (, expr), or (, expr1 expr2 ...)
    /// Goals are evaluated left-to-right with variable binding threading
    Conjunction(Vec<MettaValue>),
    /// A first-class space value with queryable data
    /// Used for space operations: new-space, add-atom, remove-atom, collapse, match
    Space(SpaceHandle),
    /// A reference to a mutable state cell (id)
    /// Used for state operations: new-state, get-state, change-state!
    State(u64),
    /// Unit value for side-effecting operations that return nothing meaningful
    /// Displayed as () in output
    Unit,
    /// A memoization table for caching evaluation results
    /// Used for memo operations: new-memo, memo, memo-first, clear-memo!, memo-stats
    Memo(MemoHandle),
    /// Empty sentinel - represents "no result to report" that gets filtered at result collection.
    /// This is distinct from:
    /// - Empty result set (vec![]) - no alternatives exist, evaluation branch is dead
    /// - Unit (()) - a valid result representing "success with no value"
    /// Empty is returned by (empty) and filtered out at final result collection (HE-compatible).
    Empty,
}

/// Arc-wrapped MettaValue for O(1) cloning in evaluation hot paths.
/// Note: With the new MettaValue wrapper, this is now redundant since
/// MettaValue itself is already O(1) to clone. Kept for API compatibility.
pub type ArcValue = MettaValue;

// ============================================================================
// MettaValue wrapper implementation - Associated functions for construction
// ============================================================================

impl MettaValue {
    /// Access the inner enum for pattern matching
    #[inline]
    pub fn inner(&self) -> &MettaValueInner {
        &self.0
    }

    /// Get the raw Arc for advanced use cases
    #[inline]
    pub fn arc(&self) -> &Arc<MettaValueInner> {
        &self.0
    }

    /// Check if two MettaValues point to the same Arc (pointer equality)
    #[inline]
    pub fn ptr_eq(&self, other: &MettaValue) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    // ========================================================================
    // Associated functions to preserve construction syntax
    // These allow: MettaValue::Atom("foo".to_string()) to continue working
    // ========================================================================

    /// Create an Atom variant
    #[allow(non_snake_case)]
    #[inline]
    pub fn Atom(s: StdString) -> Self {
        MettaValue(Arc::new(MettaValueInner::Atom(s)))
    }

    /// Create a Bool variant
    #[allow(non_snake_case)]
    #[inline]
    pub fn Bool(b: bool) -> Self {
        MettaValue(Arc::new(MettaValueInner::Bool(b)))
    }

    /// Create a Long variant
    #[allow(non_snake_case)]
    #[inline]
    pub fn Long(n: i64) -> Self {
        MettaValue(Arc::new(MettaValueInner::Long(n)))
    }

    /// Create a Float variant
    #[allow(non_snake_case)]
    #[inline]
    pub fn Float(f: f64) -> Self {
        MettaValue(Arc::new(MettaValueInner::Float(f)))
    }

    /// Create a String variant
    #[allow(non_snake_case)]
    #[inline]
    pub fn String(s: StdString) -> Self {
        MettaValue(Arc::new(MettaValueInner::String(s)))
    }

    /// Create an SExpr variant
    #[allow(non_snake_case)]
    #[inline]
    pub fn SExpr(items: Vec<MettaValue>) -> Self {
        MettaValue(Arc::new(MettaValueInner::SExpr(items)))
    }

    /// Create a Nil variant
    #[allow(non_snake_case)]
    #[inline]
    pub fn Nil() -> Self {
        MettaValue(Arc::new(MettaValueInner::Nil))
    }

    /// Create an Error variant
    #[allow(non_snake_case)]
    #[inline]
    pub fn Error(msg: StdString, details: MettaValue) -> Self {
        MettaValue(Arc::new(MettaValueInner::Error(msg, details)))
    }

    /// Create a Type variant
    #[allow(non_snake_case)]
    #[inline]
    pub fn Type(inner: MettaValue) -> Self {
        MettaValue(Arc::new(MettaValueInner::Type(inner)))
    }

    /// Create a Conjunction variant
    #[allow(non_snake_case)]
    #[inline]
    pub fn Conjunction(goals: Vec<MettaValue>) -> Self {
        MettaValue(Arc::new(MettaValueInner::Conjunction(goals)))
    }

    /// Create a Space variant
    #[allow(non_snake_case)]
    #[inline]
    pub fn Space(handle: SpaceHandle) -> Self {
        MettaValue(Arc::new(MettaValueInner::Space(handle)))
    }

    /// Create a State variant
    #[allow(non_snake_case)]
    #[inline]
    pub fn State(id: u64) -> Self {
        MettaValue(Arc::new(MettaValueInner::State(id)))
    }

    /// Create a Unit variant
    #[allow(non_snake_case)]
    #[inline]
    pub fn Unit() -> Self {
        MettaValue(Arc::new(MettaValueInner::Unit))
    }

    /// Create a Memo variant
    #[allow(non_snake_case)]
    #[inline]
    pub fn Memo(handle: MemoHandle) -> Self {
        MettaValue(Arc::new(MettaValueInner::Memo(handle)))
    }

    /// Create an Empty variant
    #[allow(non_snake_case)]
    #[inline]
    pub fn Empty() -> Self {
        MettaValue(Arc::new(MettaValueInner::Empty))
    }

    // ========================================================================
    // Helper constructors
    // ========================================================================

    /// Create a symbol atom from a string slice
    ///
    /// # Example
    /// ```ignore
    /// let sym = MettaValue::sym("foo");
    /// // Produces: Atom("foo")
    /// ```
    #[inline]
    pub fn sym(s: &str) -> Self {
        MettaValue::Atom(s.to_string())
    }

    /// Create a variable atom (prefixed with $)
    ///
    /// # Example
    /// ```ignore
    /// let var = MettaValue::var("x");
    /// // Produces: Atom("$x")
    /// ```
    #[inline]
    pub fn var(name: &str) -> Self {
        MettaValue::Atom(format!("${}", name))
    }

    /// Create an S-expression from a vector of values
    ///
    /// HE-compatible: Empty S-expression () is distinct from Nil.
    ///
    /// # Example
    /// ```ignore
    /// let sexpr = MettaValue::sexpr(vec![MettaValue::sym("+"), MettaValue::Long(1), MettaValue::Long(2)]);
    /// // Produces: SExpr([Atom("+"), Long(1), Long(2)])
    /// let empty = MettaValue::sexpr(vec![]);
    /// // Produces: SExpr([]) - distinct from Nil
    /// ```
    #[inline]
    pub fn sexpr(items: Vec<MettaValue>) -> Self {
        MettaValue::SExpr(items)
    }

    // ========================================================================
    // Type checking and inspection methods
    // ========================================================================

    /// Check if this is an Atom variant
    #[inline]
    pub fn is_atom(&self) -> bool {
        matches!(self.inner(), MettaValueInner::Atom(_))
    }

    /// Check if this is a Bool variant
    #[inline]
    pub fn is_bool(&self) -> bool {
        matches!(self.inner(), MettaValueInner::Bool(_))
    }

    /// Check if this is a Long variant
    #[inline]
    pub fn is_long(&self) -> bool {
        matches!(self.inner(), MettaValueInner::Long(_))
    }

    /// Check if this is a Float variant
    #[inline]
    pub fn is_float(&self) -> bool {
        matches!(self.inner(), MettaValueInner::Float(_))
    }

    /// Check if this is a String variant
    #[inline]
    pub fn is_string(&self) -> bool {
        matches!(self.inner(), MettaValueInner::String(_))
    }

    /// Check if this is an SExpr variant
    #[inline]
    pub fn is_sexpr(&self) -> bool {
        matches!(self.inner(), MettaValueInner::SExpr(_))
    }

    /// Check if this is a Nil variant
    #[inline]
    pub fn is_nil(&self) -> bool {
        matches!(self.inner(), MettaValueInner::Nil)
    }

    /// Check if this is an Error variant
    #[inline]
    pub fn is_error(&self) -> bool {
        matches!(self.inner(), MettaValueInner::Error(_, _))
    }

    /// Check if this is a Type variant
    #[inline]
    pub fn is_type(&self) -> bool {
        matches!(self.inner(), MettaValueInner::Type(_))
    }

    /// Check if this is a Conjunction variant
    #[inline]
    pub fn is_conjunction(&self) -> bool {
        matches!(self.inner(), MettaValueInner::Conjunction(_))
    }

    /// Check if this is a Space variant
    #[inline]
    pub fn is_space(&self) -> bool {
        matches!(self.inner(), MettaValueInner::Space(_))
    }

    /// Check if this is a State variant
    #[inline]
    pub fn is_state(&self) -> bool {
        matches!(self.inner(), MettaValueInner::State(_))
    }

    /// Check if this is a Unit variant
    #[inline]
    pub fn is_unit(&self) -> bool {
        matches!(self.inner(), MettaValueInner::Unit)
    }

    /// Check if this is a Memo variant
    #[inline]
    pub fn is_memo(&self) -> bool {
        matches!(self.inner(), MettaValueInner::Memo(_))
    }

    /// Check if this is an Empty variant
    #[inline]
    pub fn is_empty(&self) -> bool {
        matches!(self.inner(), MettaValueInner::Empty)
    }

    // ========================================================================
    // Accessor methods for extracting inner values
    // ========================================================================

    /// Try to extract as atom string
    #[inline]
    pub fn as_atom(&self) -> Option<&str> {
        match self.inner() {
            MettaValueInner::Atom(s) => Some(s),
            _ => None,
        }
    }

    /// Try to extract as bool
    #[inline]
    pub fn as_bool(&self) -> Option<bool> {
        match self.inner() {
            MettaValueInner::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Try to extract as i64
    #[inline]
    pub fn as_long(&self) -> Option<i64> {
        match self.inner() {
            MettaValueInner::Long(n) => Some(*n),
            _ => None,
        }
    }

    /// Try to extract as f64
    #[inline]
    pub fn as_float(&self) -> Option<f64> {
        match self.inner() {
            MettaValueInner::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// Try to extract as string
    #[inline]
    pub fn as_string(&self) -> Option<&str> {
        match self.inner() {
            MettaValueInner::String(s) => Some(s),
            _ => None,
        }
    }

    /// Try to extract as sexpr items
    #[inline]
    pub fn as_sexpr(&self) -> Option<&[MettaValue]> {
        match self.inner() {
            MettaValueInner::SExpr(items) => Some(items),
            _ => None,
        }
    }

    /// Try to extract as error (message, details)
    #[inline]
    pub fn as_error(&self) -> Option<(&str, &MettaValue)> {
        match self.inner() {
            MettaValueInner::Error(msg, details) => Some((msg, details)),
            _ => None,
        }
    }

    /// Try to extract as type inner value
    #[inline]
    pub fn as_type(&self) -> Option<&MettaValue> {
        match self.inner() {
            MettaValueInner::Type(inner) => Some(inner),
            _ => None,
        }
    }

    /// Try to extract as conjunction goals
    #[inline]
    pub fn as_conjunction(&self) -> Option<&[MettaValue]> {
        match self.inner() {
            MettaValueInner::Conjunction(goals) => Some(goals),
            _ => None,
        }
    }

    /// Try to extract as space handle
    #[inline]
    pub fn as_space(&self) -> Option<&SpaceHandle> {
        match self.inner() {
            MettaValueInner::Space(handle) => Some(handle),
            _ => None,
        }
    }

    /// Try to extract as state id
    #[inline]
    pub fn as_state(&self) -> Option<u64> {
        match self.inner() {
            MettaValueInner::State(id) => Some(*id),
            _ => None,
        }
    }

    /// Try to extract as memo handle
    #[inline]
    pub fn as_memo(&self) -> Option<&MemoHandle> {
        match self.inner() {
            MettaValueInner::Memo(handle) => Some(handle),
            _ => None,
        }
    }

    // ========================================================================
    // Type name and classification methods
    // ========================================================================

    /// Get the type name of this value as a string slice
    ///
    /// Returns the MeTTa type name for this value variant.
    pub fn type_name(&self) -> &'static str {
        match self.inner() {
            MettaValueInner::Atom(s) if s.starts_with('$') => "Variable",
            MettaValueInner::Atom(_) => "Symbol",
            MettaValueInner::Bool(_) => "Bool",
            MettaValueInner::Long(_) => "Number",
            MettaValueInner::Float(_) => "Number",
            MettaValueInner::String(_) => "String",
            MettaValueInner::SExpr(_) => "Expression",
            MettaValueInner::Nil => "Nil",
            MettaValueInner::Error(_, _) => "Error",
            MettaValueInner::Type(_) => "Type",
            MettaValueInner::Conjunction(_) => "Conjunction",
            MettaValueInner::Space(_) => "Space",
            MettaValueInner::State(_) => "State",
            MettaValueInner::Unit => "Unit",
            MettaValueInner::Memo(_) => "Memo",
            MettaValueInner::Empty => "Empty",
        }
    }

    /// Check if this value is a variable (Atom starting with $)
    #[inline]
    pub fn is_variable(&self) -> bool {
        matches!(self.inner(), MettaValueInner::Atom(s) if s.starts_with('$'))
    }

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
            self.inner(),
            MettaValueInner::Bool(_)
                | MettaValueInner::Long(_)
                | MettaValueInner::Float(_)
                | MettaValueInner::String(_)
                | MettaValueInner::Nil
        )
    }

    /// Convert MettaValue to a friendly type name for error messages
    /// This provides user-friendly type names instead of debug format like "Long(5)"
    pub fn friendly_type_name(&self) -> &'static str {
        match self.inner() {
            MettaValueInner::Long(_) => "Number (integer)",
            MettaValueInner::Float(_) => "Number (float)",
            MettaValueInner::Bool(_) => "Bool",
            MettaValueInner::String(_) => "String",
            MettaValueInner::Atom(_) => "Atom",
            MettaValueInner::Nil => "Nil",
            MettaValueInner::SExpr(_) => "S-expression",
            MettaValueInner::Error(_, _) => "Error",
            MettaValueInner::Type(_) => "Type",
            MettaValueInner::Conjunction(_) => "Conjunction",
            MettaValueInner::Space(_) => "Space",
            MettaValueInner::State(_) => "State",
            MettaValueInner::Unit => "Unit",
            MettaValueInner::Memo(_) => "Memo",
            MettaValueInner::Empty => "Empty",
        }
    }

    /// Check if this is an evaluation expression (starts with "!")
    /// Evaluation expressions like `!(+ 1 2)` should produce output
    pub fn is_eval_expr(&self) -> bool {
        match self.inner() {
            MettaValueInner::SExpr(items) => items
                .first()
                .map(|v| matches!(v.inner(), MettaValueInner::Atom(s) if s == "!"))
                .unwrap_or(false),
            _ => false,
        }
    }

    /// Check if this is a rule definition (starts with "=")
    /// Rule definitions like `(= (double $x) (* $x 2))` add rules to the environment
    pub fn is_rule_def(&self) -> bool {
        match self.inner() {
            MettaValueInner::SExpr(items) => items
                .first()
                .map(|v| matches!(v.inner(), MettaValueInner::Atom(s) if s == "="))
                .unwrap_or(false),
            _ => false,
        }
    }

    /// Check structural equivalence (ignoring variable names)
    /// Two expressions are structurally equivalent if they have the same structure,
    /// with variables in the same positions (regardless of variable names)
    pub fn structurally_equivalent(&self, other: &MettaValue) -> bool {
        // Helper to check if an atom is a variable (not a space reference or operator)
        fn is_variable(s: &str) -> bool {
            if s == "&" || s == "&self" || s == "&kb" || s == "&stack" {
                return false; // Space references are NOT variables
            }
            s.starts_with('$') || s.starts_with('&') || s.starts_with('\'')
        }

        match (self.inner(), other.inner()) {
            // Variables match any other variable (names don't matter)
            // EXCEPT: space references like "&self" must match exactly
            (MettaValueInner::Atom(a), MettaValueInner::Atom(b))
                if is_variable(a) && is_variable(b) =>
            {
                true
            }

            // Wildcards match wildcards
            (MettaValueInner::Atom(a), MettaValueInner::Atom(b)) if a == "_" && b == "_" => true,

            // Non-variable atoms must match exactly (including standalone "&")
            (MettaValueInner::Atom(a), MettaValueInner::Atom(b)) => a == b,

            // Other ground types must match exactly
            (MettaValueInner::Bool(a), MettaValueInner::Bool(b)) => a == b,
            (MettaValueInner::Long(a), MettaValueInner::Long(b)) => a == b,
            (MettaValueInner::Float(a), MettaValueInner::Float(b)) => a == b,
            (MettaValueInner::String(a), MettaValueInner::String(b)) => a == b,
            (MettaValueInner::Nil, MettaValueInner::Nil) => true,

            // S-expressions must have same structure
            (MettaValueInner::SExpr(a_items), MettaValueInner::SExpr(b_items)) => {
                if a_items.len() != b_items.len() {
                    return false;
                }
                a_items
                    .iter()
                    .zip(b_items.iter())
                    .all(|(a, b)| a.structurally_equivalent(b))
            }

            // Errors must have same message and equivalent details
            (
                MettaValueInner::Error(a_msg, a_details),
                MettaValueInner::Error(b_msg, b_details),
            ) => a_msg == b_msg && a_details.structurally_equivalent(b_details),

            // Types must be structurally equivalent
            (MettaValueInner::Type(a), MettaValueInner::Type(b)) => a.structurally_equivalent(b),

            // Conjunctions must have same structure
            (MettaValueInner::Conjunction(a_goals), MettaValueInner::Conjunction(b_goals)) => {
                if a_goals.len() != b_goals.len() {
                    return false;
                }
                a_goals
                    .iter()
                    .zip(b_goals.iter())
                    .all(|(a, b)| a.structurally_equivalent(b))
            }

            // Spaces are equal if they have the same id
            (MettaValueInner::Space(a), MettaValueInner::Space(b)) => a.id == b.id,

            // States must have same id
            (MettaValueInner::State(a_id), MettaValueInner::State(b_id)) => a_id == b_id,

            // Unit matches unit
            (MettaValueInner::Unit, MettaValueInner::Unit) => true,

            // Empty matches empty
            (MettaValueInner::Empty, MettaValueInner::Empty) => true,

            _ => false,
        }
    }

    /// Extract the head symbol from a pattern for indexing
    /// Returns None if the pattern doesn't have a clear head symbol
    pub fn get_head_symbol(&self) -> Option<&str> {
        // Helper to check if an atom is a space reference (not a variable)
        fn is_space_ref(s: &str) -> bool {
            s == "&" || s == "&self" || s == "&kb" || s == "&stack"
        }

        match self.inner() {
            // For s-expressions like (double $x), extract "double"
            // Space references like "&self" are allowed as head symbols
            MettaValueInner::SExpr(items) if !items.is_empty() => match items[0].inner() {
                MettaValueInner::Atom(head)
                    if !head.starts_with('$')
                        && (!head.starts_with('&') || is_space_ref(head))
                        && !head.starts_with('\'')
                        && head != "_" =>
                {
                    Some(head.as_str())
                }
                _ => None,
            },
            // For bare atoms like foo, use the atom itself
            // Space references like "&self" are allowed as head symbols
            MettaValueInner::Atom(head)
                if !head.starts_with('$')
                    && (!head.starts_with('&') || is_space_ref(head))
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
        match self.inner() {
            MettaValueInner::SExpr(items) if !items.is_empty() => items.len() - 1, // Exclude head
            _ => 0,
        }
    }

    /// Convert MettaValue to MORK s-expression string format
    /// This format can be parsed by MORK's parser
    pub fn to_mork_string(&self) -> StdString {
        match self.inner() {
            MettaValueInner::Atom(s) => {
                // Variables need to start with $ in MORK format
                // EXCEPT: standalone "&" is a literal operator (used in match), not a variable
                // EXCEPT: "&self" and other space references should be preserved as-is
                if s == "&" || s == "&self" || s == "&kb" || s == "&stack" {
                    // Space references and standalone & are NOT variables - preserve as-is
                    s.clone()
                } else if s.starts_with('$') || s.starts_with('&') || s.starts_with('\'') {
                    format!("${}", &s[1..]) // Keep $ prefix, remove original prefix
                } else if s == "_" {
                    "$".to_string() // Wildcard becomes $
                } else {
                    s.clone()
                }
            }
            MettaValueInner::Bool(b) => b.to_string(),
            MettaValueInner::Long(n) => n.to_string(),
            MettaValueInner::Float(f) => f.to_string(),
            MettaValueInner::String(s) => format!("\"{}\"", s),
            MettaValueInner::SExpr(items) => {
                let inner = items
                    .iter()
                    .map(|v| v.to_mork_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("({})", inner)
            }
            MettaValueInner::Nil => "()".to_string(),
            MettaValueInner::Error(msg, details) => {
                format!("(error \"{}\" {})", msg, details.to_mork_string())
            }
            MettaValueInner::Type(t) => t.to_mork_string(),
            MettaValueInner::Conjunction(goals) => {
                let inner = goals
                    .iter()
                    .map(|v| v.to_mork_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("(, {})", inner)
            }
            MettaValueInner::Space(handle) => format!("(Space {} \"{}\")", handle.id, handle.name),
            MettaValueInner::State(id) => format!("(State {})", id),
            MettaValueInner::Unit => "()".to_string(),
            MettaValueInner::Memo(handle) => format!("(Memo {} \"{}\")", handle.id, handle.name),
            MettaValueInner::Empty => "Empty".to_string(),
        }
    }

    /// Convert MettaValue to a JSON-like string representation
    /// Used for debugging and human-readable output
    pub fn to_json_string(&self) -> StdString {
        match self.inner() {
            MettaValueInner::Atom(s) => {
                format!(r#"{{"type":"atom","value":"{}"}}"#, escape_json(s))
            }
            MettaValueInner::Bool(b) => format!(r#"{{"type":"bool","value":{}}}"#, b),
            MettaValueInner::Long(n) => format!(r#"{{"type":"number","value":{}}}"#, n),
            MettaValueInner::Float(f) => format!(r#"{{"type":"float","value":{}}}"#, f),
            MettaValueInner::String(s) => {
                format!(r#"{{"type":"string","value":"{}"}}"#, escape_json(s))
            }
            MettaValueInner::Nil => r#"{"type":"nil"}"#.to_string(),
            MettaValueInner::SExpr(items) => {
                let items_json: Vec<StdString> =
                    items.iter().map(|value| value.to_json_string()).collect();
                format!(r#"{{"type":"sexpr","items":[{}]}}"#, items_json.join(","))
            }
            MettaValueInner::Error(msg, details) => {
                format!(
                    r#"{{"type":"error","message":"{}","details":{}}}"#,
                    escape_json(msg),
                    details.to_json_string()
                )
            }
            MettaValueInner::Type(t) => {
                format!(r#"{{"type":"metatype","value":{}}}"#, t.to_json_string())
            }
            MettaValueInner::Conjunction(goals) => {
                let goals_json: Vec<StdString> =
                    goals.iter().map(|value| value.to_json_string()).collect();
                format!(
                    r#"{{"type":"conjunction","goals":[{}]}}"#,
                    goals_json.join(",")
                )
            }
            MettaValueInner::Space(handle) => {
                format!(
                    r#"{{"type":"space","id":{},"name":"{}"}}"#,
                    handle.id,
                    escape_json(&handle.name)
                )
            }
            MettaValueInner::State(id) => {
                format!(r#"{{"type":"state","id":{}}}"#, id)
            }
            MettaValueInner::Unit => r#"{"type":"unit"}"#.to_string(),
            MettaValueInner::Memo(handle) => {
                format!(
                    r#"{{"type":"memo","id":{},"name":"{}"}}"#,
                    handle.id,
                    escape_json(&handle.name)
                )
            }
            MettaValueInner::Empty => r#"{"type":"empty"}"#.to_string(),
        }
    }
}

pub fn escape_json(s: &str) -> StdString {
    s.replace('\\', r"\\")
        .replace('"', r#"\""#)
        .replace('\n', r"\n")
        .replace('\r', r"\r")
        .replace('\t', r"\t")
}

// ============================================================================
// Trait implementations for MettaValue wrapper
// ============================================================================

impl PartialEq for MettaValue {
    fn eq(&self, other: &Self) -> bool {
        // Fast path: check if same Arc
        Arc::ptr_eq(&self.0, &other.0) || self.0 == other.0
    }
}

impl Eq for MettaValue {}

impl std::hash::Hash for MettaValue {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl std::fmt::Debug for MettaValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl std::fmt::Display for MettaValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.inner() {
            MettaValueInner::Atom(s) => write!(f, "{}", s),
            MettaValueInner::Bool(b) => write!(f, "{}", if *b { "True" } else { "False" }),
            MettaValueInner::Long(n) => write!(f, "{}", n),
            MettaValueInner::Float(v) => write!(f, "{}", v),
            MettaValueInner::String(s) => write!(f, "\"{}\"", s),
            MettaValueInner::SExpr(items) => {
                write!(f, "(")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{}", item)?;
                }
                write!(f, ")")
            }
            MettaValueInner::Nil => write!(f, "Nil"),
            MettaValueInner::Error(msg, details) => write!(f, "(Error {} {})", msg, details),
            MettaValueInner::Type(inner) => write!(f, "(: {})", inner),
            MettaValueInner::Conjunction(goals) => {
                write!(f, "(,")?;
                for goal in goals {
                    write!(f, " {}", goal)?;
                }
                write!(f, ")")
            }
            MettaValueInner::Space(handle) => write!(f, "<Space:{}>", handle.name),
            MettaValueInner::State(id) => write!(f, "<State:{}>", id),
            MettaValueInner::Unit => write!(f, "()"),
            MettaValueInner::Memo(handle) => write!(f, "<Memo:{}>", handle.name),
            MettaValueInner::Empty => write!(f, "Empty"),
        }
    }
}

// ============================================================================
// Hash implementation for MettaValueInner
// ============================================================================

impl std::hash::Hash for MettaValueInner {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            MettaValueInner::Atom(s) => {
                0u8.hash(state);
                s.hash(state);
            }
            MettaValueInner::Bool(b) => {
                1u8.hash(state);
                b.hash(state);
            }
            MettaValueInner::Long(n) => {
                2u8.hash(state);
                n.hash(state);
            }
            MettaValueInner::Float(f) => {
                3u8.hash(state);
                // Hash float as its bit representation for deterministic hashing
                f.to_bits().hash(state);
            }
            MettaValueInner::String(s) => {
                4u8.hash(state);
                s.hash(state);
            }
            MettaValueInner::SExpr(items) => {
                5u8.hash(state);
                items.hash(state);
            }
            MettaValueInner::Nil => {
                6u8.hash(state);
            }
            MettaValueInner::Error(msg, details) => {
                7u8.hash(state);
                msg.hash(state);
                details.hash(state);
            }
            MettaValueInner::Type(t) => {
                8u8.hash(state);
                t.hash(state);
            }
            MettaValueInner::Conjunction(goals) => {
                10u8.hash(state);
                goals.hash(state);
            }
            MettaValueInner::Space(handle) => {
                11u8.hash(state);
                handle.hash(state);
            }
            MettaValueInner::State(id) => {
                13u8.hash(state);
                id.hash(state);
            }
            MettaValueInner::Unit => {
                12u8.hash(state);
            }
            MettaValueInner::Memo(handle) => {
                14u8.hash(state);
                handle.hash(state);
            }
            MettaValueInner::Empty => {
                15u8.hash(state);
            }
        }
    }
}

// ============================================================================
// From trait implementations for convenient MettaValue construction
// ============================================================================

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

impl From<StdString> for MettaValue {
    fn from(s: StdString) -> Self {
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
            MettaValue::Nil()
        } else {
            MettaValue::SExpr(items)
        }
    }
}

// ============================================================================
// Export MettaValueInner for pattern matching
// ============================================================================

pub use MettaValueInner::*;

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
        assert!(MettaValue::Nil().is_ground_type());
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
        assert!(!MettaValue::Error("msg".to_string(), MettaValue::Nil()).is_ground_type());
    }

    #[test]
    fn test_is_ground_type_type() {
        assert!(!MettaValue::Type(MettaValue::Atom("Int".to_string())).is_ground_type());
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
        assert!(!MettaValue::Nil().is_eval_expr());
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
        assert!(!MettaValue::Nil().is_rule_def());
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

        assert!(MettaValue::Nil().structurally_equivalent(&MettaValue::Nil()));
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
        let e1 = MettaValue::Error("msg".to_string(), MettaValue::Long(1));
        let e2 = MettaValue::Error("msg".to_string(), MettaValue::Long(1));
        assert!(e1.structurally_equivalent(&e2));

        let e3 = MettaValue::Error("msg".to_string(), MettaValue::Long(2));
        assert!(!e1.structurally_equivalent(&e3));

        let e4 = MettaValue::Error("other".to_string(), MettaValue::Long(1));
        assert!(!e1.structurally_equivalent(&e4));
    }

    #[test]
    fn test_structurally_equivalent_types() {
        let t1 = MettaValue::Type(MettaValue::Atom("Int".to_string()));
        let t2 = MettaValue::Type(MettaValue::Atom("Int".to_string()));
        assert!(t1.structurally_equivalent(&t2));

        let t3 = MettaValue::Type(MettaValue::Atom("String".to_string()));
        assert!(!t1.structurally_equivalent(&t3));
    }

    #[test]
    fn test_structurally_equivalent_different_types() {
        // Different enum variants are not equivalent
        assert!(!MettaValue::Bool(true).structurally_equivalent(&MettaValue::Long(1)));
        assert!(!MettaValue::Atom("x".to_string()).structurally_equivalent(&MettaValue::Long(1)));
        assert!(!MettaValue::Nil().structurally_equivalent(&MettaValue::Long(0)));
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
        assert_eq!(MettaValue::Nil().get_head_symbol(), None);
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
    fn test_to_mork_string_space_references() {
        // Space references like &self are NOT variables - they should be preserved
        assert_eq!(
            MettaValue::Atom("&self".to_string()).to_mork_string(),
            "&self"
        );
        assert_eq!(MettaValue::Atom("&kb".to_string()).to_mork_string(), "&kb");
        assert_eq!(
            MettaValue::Atom("&stack".to_string()).to_mork_string(),
            "&stack"
        );

        // But regular &-prefixed atoms ARE variables and get converted
        assert_eq!(MettaValue::Atom("&x".to_string()).to_mork_string(), "$x");
        assert_eq!(
            MettaValue::Atom("&foo".to_string()).to_mork_string(),
            "$foo"
        );
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
        assert_eq!(MettaValue::Nil().to_mork_string(), "()");
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
        let value = MettaValue::Error("test error".to_string(), MettaValue::Long(42));
        assert_eq!(value.to_mork_string(), "(error \"test error\" 42)");
    }

    #[test]
    fn test_to_mork_string_type() {
        let value = MettaValue::Type(MettaValue::Atom("Int".to_string()));
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
        let value = MettaValue::Nil();
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

    // Tests for O(1) clone
    #[test]
    fn test_clone_is_o1() {
        // Create a large nested structure
        let large = MettaValue::SExpr(vec![
            MettaValue::Atom("root".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("nested".to_string()),
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
        ]);

        // Clone should just increment reference count
        let cloned = large.clone();

        // Both should point to the same Arc
        assert!(large.ptr_eq(&cloned));
    }

    #[test]
    fn test_accessor_methods() {
        assert_eq!(MettaValue::Atom("foo".to_string()).as_atom(), Some("foo"));
        assert_eq!(MettaValue::Bool(true).as_bool(), Some(true));
        assert_eq!(MettaValue::Long(42).as_long(), Some(42));
        assert_eq!(MettaValue::Float(3.14).as_float(), Some(3.14));
        assert_eq!(
            MettaValue::String("bar".to_string()).as_string(),
            Some("bar")
        );

        // Cross-type access returns None
        assert_eq!(MettaValue::Long(42).as_atom(), None);
        assert_eq!(MettaValue::Atom("foo".to_string()).as_long(), None);
    }
}
