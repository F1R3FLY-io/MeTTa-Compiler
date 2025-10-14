// Type definitions for the MeTTa backend

use std::collections::HashMap;

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

/// The environment contains the fact database (rules) and type assertions
/// The fact database is represented as a vector of rules for now
/// (PathMap integration will be handled by another team)
#[derive(Debug, Clone)]
pub struct Environment {
    pub rules: Vec<Rule>,
    /// Type assertions: atom/expression -> type
    pub types: HashMap<String, MettaValue>,
}

impl Environment {
    pub fn new() -> Self {
        Environment {
            rules: Vec::new(),
            types: HashMap::new(),
        }
    }

    pub fn add_rule(&mut self, rule: Rule) {
        self.rules.push(rule);
    }

    /// Add a type assertion
    pub fn add_type(&mut self, name: String, typ: MettaValue) {
        self.types.insert(name, typ);
    }

    /// Get type for an atom
    pub fn get_type(&self, name: &str) -> Option<&MettaValue> {
        self.types.get(name)
    }

    /// Union two environments (monotonic merge)
    pub fn union(&self, other: &Environment) -> Environment {
        let mut rules = self.rules.clone();
        rules.extend(other.rules.clone());

        let mut types = self.types.clone();
        types.extend(other.types.clone());

        Environment { rules, types }
    }
}

impl Default for Environment {
    fn default() -> Self {
        Self::new()
    }
}

/// Result of evaluation: (result, new_environment)
pub type EvalResult = (Vec<MettaValue>, Environment);
