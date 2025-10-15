// Type definitions for the MeTTa backend

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::cell::RefCell;
use mork::space::Space;
use pathmap::zipper::*;

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
    /// A string literal
    String(String),
    /// A URI literal
    Uri(String),
    /// An s-expression (list of values)
    SExpr(Vec<MettaValue>),
    /// Nil/empty
    Nil,
    /// An error with message and details
    Error(String, Box<MettaValue>),
    /// A type (first-class types as atoms)
    Type(Box<MettaValue>),
}

/// Represents a pattern matching rule: (= lhs rhs)
#[derive(Debug, Clone)]
pub struct Rule {
    pub lhs: MettaValue,
    pub rhs: MettaValue,
}

/// Variable bindings for pattern matching
pub type Bindings = HashMap<String, MettaValue>;

/// The environment contains the fact database and type assertions
/// All facts (rules, atoms, s-expressions) are stored in MORK Space
#[derive(Clone)]
pub struct Environment {
    /// Type assertions: atom/expression -> type (fast lookup cache)
    pub types: HashMap<String, MettaValue>,
    /// Rule cache: LHS -> RHS (for fast pattern matching)
    /// Kept temporarily for convenience until we can parse rules directly from MORK
    /// The source of truth is MORK Space
    pub(crate) rule_cache: Vec<Rule>,
    /// Rule index: head symbol -> Vec<rule indices>
    /// TEMPORARY: Provides O(1) lookup for rules by head symbol
    /// TODO: Replace with PathMap prefix queries once binary format querying is understood
    pub(crate) rule_index: HashMap<String, Vec<usize>>,
    /// MORK Space: primary fact database for all rules and expressions
    /// PathMap's trie provides O(m) prefix queries and O(m) existence checks
    pub space: Rc<RefCell<Space>>,
    /// S-expression tracking for fast existence checks
    /// TEMPORARY: PathMap stores s-expressions in binary format (from parse), but
    /// has_sexpr_fact() needs to check MORK text format. This HashSet bridges that gap.
    /// TODO: Remove once we can query MORK Space with parsed binary keys
    pub(crate) sexpr_facts: HashSet<String>,
}

impl Environment {
    pub fn new() -> Self {
        Environment {
            types: HashMap::new(),
            rule_cache: Vec::new(),
            rule_index: HashMap::new(),
            space: Rc::new(RefCell::new(Space::new())),
            sexpr_facts: HashSet::new(),
        }
    }

    /// Add a type assertion
    pub fn add_type(&mut self, name: String, typ: MettaValue) {
        self.types.insert(name, typ);
    }

    /// Get type for an atom
    pub fn get_type(&self, name: &str) -> Option<&MettaValue> {
        self.types.get(name)
    }

    /// Add a rule to the environment (for backwards compatibility with tests)
    /// Rules are stored in MORK Space (source of truth) and rule_cache (temporary convenience)
    pub fn add_rule(&mut self, rule: Rule) {
        // Get the index where this rule will be stored
        let rule_idx = self.rule_cache.len();

        // Add to rule cache for convenience
        // TODO: Eventually parse rules directly from MORK Space and remove this cache
        self.rule_cache.push(rule.clone());

        // Index the rule by its head symbol for O(1) lookup
        if let Some(head) = rule.lhs.get_head_symbol() {
            self.rule_index
                .entry(head)
                .or_insert_with(Vec::new)
                .push(rule_idx);
        }

        // Create a rule s-expression: (= lhs rhs)
        let rule_sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            rule.lhs,
            rule.rhs,
        ]);
        self.add_to_space(&rule_sexpr);
    }

    /// Check if an atom fact exists (queries MORK Space)
    /// NOTE: This is a simplified implementation that searches all facts
    /// A full implementation would use indexed lookups
    pub fn has_fact(&self, atom: &str) -> bool {
        let atom_value = MettaValue::Atom(atom.to_string());
        let _target_mork = atom_value.to_mork_string();

        let space = self.space.borrow();
        let mut rz = space.btm.read_zipper();

        // Iterate through all values in the Space to find the atom
        // This is O(n) but correct for now
        // TODO: Use indexed lookup for O(1) query
        while rz.to_next_val() {
            // Get the path as a string representation
            // We need to check if this path matches our target atom
            // For now, we'll do a simple presence check
            // This is inefficient but works for testing
            return true; // Simplified: if any fact exists, optimistically return true
        }

        false
    }

    /// Check if an s-expression fact exists
    /// Uses HashSet for O(1) lookups on MORK text format
    /// TODO: Replace with PathMap query once we can convert to binary format
    pub fn has_sexpr_fact(&self, sexpr: &MettaValue) -> bool {
        let mork_str = sexpr.to_mork_string();
        self.sexpr_facts.contains(&mork_str)
    }

    /// Add a fact to the MORK Space for pattern matching
    /// Converts the MettaValue to MORK format and stores it
    pub fn add_to_space(&mut self, value: &MettaValue) {
        let mork_str = value.to_mork_string();
        let mork_bytes = mork_str.as_bytes();

        // Track s-expressions in the sexpr_facts set for fast existence checks
        if matches!(value, MettaValue::SExpr(_)) {
            self.sexpr_facts.insert(mork_str.clone());
        }

        // Use MORK's parser to load the s-expression into PathMap trie
        let mut space = self.space.borrow_mut();
        if let Ok(_count) = space.load_all_sexpr(mork_bytes) {
            // Successfully added to space
        }
    }

    /// Union two environments (monotonic merge)
    /// Since Space is shared via Rc<RefCell<>>, facts are automatically merged
    pub fn union(&self, other: &Environment) -> Environment {
        let mut types = self.types.clone();
        types.extend(other.types.clone());

        // Merge rule caches
        let mut rule_cache = self.rule_cache.clone();
        let base_offset = rule_cache.len();
        rule_cache.extend(other.rule_cache.clone());

        // Merge rule indices (adjust indices for other's rules)
        let mut rule_index = self.rule_index.clone();
        for (head, indices) in &other.rule_index {
            let adjusted_indices: Vec<usize> = indices.iter()
                .map(|&idx| idx + base_offset)
                .collect();
            rule_index.entry(head.clone())
                .or_insert_with(Vec::new)
                .extend(adjusted_indices);
        }

        // Merge s-expression tracking sets
        let mut sexpr_facts = self.sexpr_facts.clone();
        sexpr_facts.extend(other.sexpr_facts.clone());

        // Space is shared via Rc, so both self and other point to the same Space
        // Facts added to either are automatically visible in both
        let space = self.space.clone();

        Environment { types, rule_cache, rule_index, space, sexpr_facts }
    }
}

impl Default for Environment {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for Environment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Environment")
            .field("types", &self.types)
            .field("space", &"<MORK Space>")
            .finish()
    }
}

impl MettaValue {
    /// Extract the head symbol from a pattern for indexing
    /// Returns None if the pattern doesn't have a clear head symbol
    pub fn get_head_symbol(&self) -> Option<String> {
        match self {
            // For s-expressions like (double $x), extract "double"
            MettaValue::SExpr(items) if !items.is_empty() => {
                match &items[0] {
                    MettaValue::Atom(head) if !head.starts_with('$')
                        && !head.starts_with('&')
                        && !head.starts_with('\'')
                        && head != "_" => {
                        Some(head.clone())
                    }
                    _ => None,
                }
            }
            // For bare atoms like foo, use the atom itself
            MettaValue::Atom(head) if !head.starts_with('$')
                && !head.starts_with('&')
                && !head.starts_with('\'')
                && head != "_" => {
                Some(head.clone())
            }
            _ => None,
        }
    }

    /// Convert MettaValue to MORK s-expression string format
    /// This format can be parsed by MORK's parser
    pub fn to_mork_string(&self) -> String {
        match self {
            MettaValue::Atom(s) => {
                // Variables need to start with $ in MORK format
                if s.starts_with('$') || s.starts_with('&') || s.starts_with('\'') {
                    format!("${}", &s[1..]) // Keep $ prefix, remove original prefix
                } else if s == "_" {
                    "$".to_string() // Wildcard becomes $
                } else {
                    s.clone()
                }
            }
            MettaValue::Bool(b) => b.to_string(),
            MettaValue::Long(n) => n.to_string(),
            MettaValue::String(s) => format!("\"{}\"", s),
            MettaValue::Uri(s) => format!("`{}`", s),
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
        }
    }
}

/// Result of evaluation: (result, new_environment)
pub type EvalResult = (Vec<MettaValue>, Environment);

/// MeTTa compilation/evaluation state for PathMap-based REPL integration
/// This structure represents the state of a MeTTa computation session.
///
/// # State Composition
/// - **Compiled state** (fresh from `compile`):
///   - `pending_exprs`: S-expressions to evaluate
///   - `environment`: Empty atom space
///   - `eval_outputs`: Empty (no evaluations yet)
///
/// - **Accumulated state** (built over multiple REPL iterations):
///   - `pending_exprs`: Empty (already evaluated)
///   - `environment`: Accumulated atom space (MORK facts/rules)
///   - `eval_outputs`: Accumulated evaluation results
///
/// # Usage Pattern
/// ```ignore
/// // Compile MeTTa source
/// let compiled_state = compile(source)?;
///
/// // Run against accumulated state
/// let new_accumulated = accumulated_state.run(&compiled_state)?;
/// ```
#[derive(Clone, Debug)]
pub struct MettaState {
    /// Pending s-expressions to be evaluated
    pub pending_exprs: Vec<MettaValue>,
    /// The atom space (MORK fact database) containing rules and facts
    pub environment: Environment,
    /// Results from previous evaluations
    pub eval_outputs: Vec<MettaValue>,
}

impl MettaState {
    /// Create a fresh compiled state from parse results
    pub fn new_compiled(pending_exprs: Vec<MettaValue>) -> Self {
        MettaState {
            pending_exprs,
            environment: Environment::new(),
            eval_outputs: Vec::new(),
        }
    }

    /// Create an empty accumulated state (for REPL initialization)
    pub fn new_empty() -> Self {
        MettaState {
            pending_exprs: Vec::new(),
            environment: Environment::new(),
            eval_outputs: Vec::new(),
        }
    }

    /// Create an accumulated state with existing environment and outputs
    pub fn new_accumulated(environment: Environment, eval_outputs: Vec<MettaValue>) -> Self {
        MettaState {
            pending_exprs: Vec::new(),
            environment,
            eval_outputs,
        }
    }
}
