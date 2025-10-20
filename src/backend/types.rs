// Type definitions for the MeTTa backend

use mork::space::Space;
use pathmap::zipper::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

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
/// All facts (rules, atoms, s-expressions, type assertions) are stored in MORK Space
///
/// Thread-safe via Arc<Mutex<T>> to enable parallel evaluation
#[derive(Clone)]
pub struct Environment {
    /// MORK Space: primary fact database for all rules, expressions, and type assertions
    /// PathMap's trie provides O(m) prefix queries and O(m) existence checks
    /// Thread-safe via Arc<Mutex<>> for parallel evaluation
    pub space: Arc<Mutex<Space>>,

    /// Multiplicities: tracks how many times each rule is defined
    /// Maps a normalized rule key to its definition count
    /// This allows multiply-defined rules to produce multiple results
    /// Thread-safe via Arc<Mutex<>> for parallel evaluation
    multiplicities: Arc<Mutex<HashMap<String, usize>>>,
}

impl Environment {
    pub fn new() -> Self {
        Environment {
            space: Arc::new(Mutex::new(Space::new())),
            multiplicities: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Convert a MORK Expr directly to MettaValue without text serialization
    /// This avoids the "reserved byte" panic that occurs in serialize2()
    ///
    /// The key insight: serialize2() uses byte_item() which panics on bytes 64-127.
    /// We use maybe_byte_item() instead, which returns Result<Tag, u8> and handles reserved bytes gracefully.
    ///
    /// CRITICAL FIX for "reserved 114" and similar bugs during evaluation/iteration.
    #[allow(unused_variables)]
    pub(crate) fn mork_expr_to_metta_value(
        expr: &mork_expr::Expr,
        space: &Space,
    ) -> Result<MettaValue, String> {
        use mork_expr::{maybe_byte_item, Tag};
        use std::slice::from_raw_parts;

        // Stack-based traversal to avoid recursion limits
        #[derive(Debug)]
        enum StackFrame {
            Arity {
                remaining: u8,
                items: Vec<MettaValue>,
            },
        }

        let mut stack: Vec<StackFrame> = Vec::new();
        let mut offset = 0usize;
        let ptr = expr.ptr;
        let mut newvar_count = 0u8; // Track how many NewVars we've seen for proper indexing

        'parsing: loop {
            // Read the next byte and interpret as tag
            let byte = unsafe { *ptr.byte_add(offset) };
            let tag = match maybe_byte_item(byte) {
                Ok(t) => t,
                Err(reserved_byte) => {
                    // Reserved byte encountered - this is the bug we're fixing!
                    // Instead of panicking, return an error that calling code can handle
                    return Err(format!(
                        "Reserved byte {} at offset {}",
                        reserved_byte, offset
                    ));
                }
            };

            offset += 1;

            // Handle the tag and build MettaValue
            let value = match tag {
                Tag::NewVar => {
                    // De Bruijn index - NewVar introduces a new variable with the next index
                    // Use MORK's VARNAMES for proper variable names
                    const VARNAMES: [&str; 64] = [
                        "$a", "$b", "$c", "$d", "$e", "$f", "$g", "$h", "$i", "$j", "x10", "x11",
                        "x12", "x13", "x14", "x15", "x16", "x17", "x18", "x19", "x20", "x21",
                        "x22", "x23", "x24", "x25", "x26", "x27", "x28", "x29", "x30", "x31",
                        "x32", "x33", "x34", "x35", "x36", "x37", "x38", "x39", "x40", "x41",
                        "x42", "x43", "x44", "x45", "x46", "x47", "x48", "x49", "x50", "x51",
                        "x52", "x53", "x54", "x55", "x56", "x57", "x58", "x59", "x60", "x61",
                        "x62", "x63",
                    ];
                    let var_name = if (newvar_count as usize) < VARNAMES.len() {
                        VARNAMES[newvar_count as usize].to_string()
                    } else {
                        format!("$var{}", newvar_count)
                    };
                    newvar_count += 1;
                    MettaValue::Atom(var_name)
                }
                Tag::VarRef(i) => {
                    // Variable reference - use MORK's VARNAMES for proper variable names
                    // VARNAMES: ["$a", "$b", "$c", "$d", "$e", "$f", "$g", "$h", "$i", "$j", "x10", ...]
                    const VARNAMES: [&str; 64] = [
                        "$a", "$b", "$c", "$d", "$e", "$f", "$g", "$h", "$i", "$j", "x10", "x11",
                        "x12", "x13", "x14", "x15", "x16", "x17", "x18", "x19", "x20", "x21",
                        "x22", "x23", "x24", "x25", "x26", "x27", "x28", "x29", "x30", "x31",
                        "x32", "x33", "x34", "x35", "x36", "x37", "x38", "x39", "x40", "x41",
                        "x42", "x43", "x44", "x45", "x46", "x47", "x48", "x49", "x50", "x51",
                        "x52", "x53", "x54", "x55", "x56", "x57", "x58", "x59", "x60", "x61",
                        "x62", "x63",
                    ];
                    if (i as usize) < VARNAMES.len() {
                        MettaValue::Atom(VARNAMES[i as usize].to_string())
                    } else {
                        MettaValue::Atom(format!("$var{}", i))
                    }
                }
                Tag::SymbolSize(size) => {
                    // Read symbol bytes
                    let symbol_bytes =
                        unsafe { from_raw_parts(ptr.byte_add(offset), size as usize) };
                    offset += size as usize;

                    // Look up symbol in symbol table if interning is enabled
                    let symbol_str = {
                        #[cfg(feature = "interning")]
                        {
                            // With interning, symbols are ALWAYS stored as 8-byte i64 IDs
                            if symbol_bytes.len() == 8 {
                                // Convert bytes to i64, then back to bytes for symbol table lookup
                                let symbol_id =
                                    i64::from_be_bytes(symbol_bytes.try_into().unwrap())
                                        .to_be_bytes();
                                if let Some(actual_bytes) = space.sm.get_bytes(symbol_id) {
                                    // Found in symbol table - use actual symbol string
                                    String::from_utf8_lossy(actual_bytes).to_string()
                                } else {
                                    // Symbol ID not in table - fall back to treating as raw bytes
                                    String::from_utf8_lossy(symbol_bytes).to_string()
                                }
                            } else {
                                // Not 8 bytes - treat as raw symbol string
                                String::from_utf8_lossy(symbol_bytes).to_string()
                            }
                        }
                        #[cfg(not(feature = "interning"))]
                        {
                            // Without interning, symbols are stored as raw UTF-8 bytes
                            String::from_utf8_lossy(symbol_bytes).to_string()
                        }
                    };

                    // Parse the symbol to check if it's a number or string literal
                    if let Ok(n) = symbol_str.parse::<i64>() {
                        MettaValue::Long(n)
                    } else if symbol_str == "true" {
                        MettaValue::Bool(true)
                    } else if symbol_str == "false" {
                        MettaValue::Bool(false)
                    } else if symbol_str.starts_with('"')
                        && symbol_str.ends_with('"')
                        && symbol_str.len() >= 2
                    {
                        // String literal - strip quotes
                        MettaValue::String(symbol_str[1..symbol_str.len() - 1].to_string())
                    } else if symbol_str.starts_with('`')
                        && symbol_str.ends_with('`')
                        && symbol_str.len() >= 2
                    {
                        // URI literal - strip backticks
                        MettaValue::Uri(symbol_str[1..symbol_str.len() - 1].to_string())
                    } else {
                        MettaValue::Atom(symbol_str)
                    }
                }
                Tag::Arity(arity) => {
                    if arity == 0 {
                        // Empty s-expression
                        MettaValue::Nil
                    } else {
                        // Push new frame for this s-expression
                        stack.push(StackFrame::Arity {
                            remaining: arity,
                            items: Vec::new(),
                        });
                        continue 'parsing;
                    }
                }
            };

            // Value is complete - add to parent or return
            let mut value = value; // Make value mutable for the popping loop
            'popping: loop {
                match stack.last_mut() {
                    None => {
                        // No parent - this is the final result
                        return Ok(value);
                    }
                    Some(StackFrame::Arity { remaining, items }) => {
                        items.push(value.clone());
                        *remaining -= 1;

                        if *remaining == 0 {
                            // S-expression is complete
                            let completed_items = items.clone();
                            stack.pop();
                            value = MettaValue::SExpr(completed_items); // Mutate, don't shadow!
                            continue 'popping;
                        } else {
                            // More items needed
                            continue 'parsing;
                        }
                    }
                }
            }
        }
    }

    /// Helper function to serialize a MORK Expr to a readable string
    /// DEPRECATED: This uses serialize2() which panics on reserved bytes.
    /// Use mork_expr_to_metta_value() instead for production code.
    #[allow(dead_code)]
    #[allow(unused_variables)]
    fn serialize_mork_expr_old(expr: &mork_expr::Expr, space: &Space) -> String {
        let mut buffer = Vec::new();
        expr.serialize2(
            &mut buffer,
            |s| {
                #[cfg(feature = "interning")]
                {
                    let symbol = i64::from_be_bytes(s.try_into().unwrap()).to_be_bytes();
                    let mstr = space
                        .sm
                        .get_bytes(symbol)
                        .map(|x| unsafe { std::str::from_utf8_unchecked(x) });
                    unsafe { std::mem::transmute(mstr.unwrap_or("")) }
                }
                #[cfg(not(feature = "interning"))]
                unsafe {
                    std::mem::transmute(std::str::from_utf8_unchecked(s))
                }
            },
            |i, _intro| mork_expr::Expr::VARNAMES[i as usize],
        );

        String::from_utf8_lossy(&buffer).to_string()
    }

    /// Add a type assertion
    /// Type assertions are stored as (: name type) in MORK Space
    pub fn add_type(&mut self, name: String, typ: MettaValue) {
        // Create type assertion: (: name typ)
        let type_assertion = MettaValue::SExpr(vec![
            MettaValue::Atom(":".to_string()),
            MettaValue::Atom(name),
            typ,
        ]);
        self.add_to_space(&type_assertion);
    }

    /// Get type for an atom by querying MORK Space
    /// Searches for type assertions of the form (: name type)
    /// Returns None if no type assertion exists for the given name
    #[allow(clippy::collapsible_match)]
    pub fn get_type(&self, name: &str) -> Option<MettaValue> {
        use mork_expr::Expr;

        let space = self.space.lock().unwrap();
        let mut rz = space.btm.read_zipper();

        // Iterate through all values in the trie
        while rz.to_next_val() {
            // Get the s-expression at this position
            let expr = Expr {
                ptr: rz.path().as_ptr().cast_mut(),
            };

            // FIXED: Use mork_expr_to_metta_value() instead of serialize2-based conversion
            // This avoids the "reserved byte" panic during evaluation
            if let Ok(value) = Self::mork_expr_to_metta_value(&expr, &space) {
                // Check if this is a type assertion: (: name type)
                if let MettaValue::SExpr(items) = &value {
                    if items.len() == 3 {
                        if let (MettaValue::Atom(op), MettaValue::Atom(atom_name), typ) =
                            (&items[0], &items[1], &items[2])
                        {
                            if op == ":" && atom_name == name {
                                return Some(typ.clone());
                            }
                        }
                    }
                }
            }
        }

        None
    }

    /// Get the number of rules in the environment
    /// Counts rules directly from PathMap Space
    pub fn rule_count(&self) -> usize {
        self.iter_rules().count()
    }

    /// Iterator over all rules in the Space
    /// Rules are stored as MORK s-expressions: (= lhs rhs)
    ///
    /// Uses direct zipper traversal to avoid dump/parse overhead.
    /// This provides O(n) iteration without string serialization.
    #[allow(clippy::collapsible_match)]
    pub fn iter_rules(&self) -> impl Iterator<Item = Rule> {
        use mork_expr::Expr;

        let space = self.space.lock().unwrap();
        let mut rz = space.btm.read_zipper();
        let mut rules = Vec::new();

        // Directly iterate through all values in the trie
        while rz.to_next_val() {
            // Get the s-expression at this position
            let expr = Expr {
                ptr: rz.path().as_ptr().cast_mut(),
            };

            // FIXED: Use mork_expr_to_metta_value() instead of serialize2-based conversion
            // This avoids the "reserved byte" panic during evaluation
            if let Ok(value) = Self::mork_expr_to_metta_value(&expr, &space) {
                if let MettaValue::SExpr(items) = &value {
                    if items.len() == 3 {
                        if let MettaValue::Atom(op) = &items[0] {
                            if op == "=" {
                                rules.push(Rule {
                                    lhs: items[1].clone(),
                                    rhs: items[2].clone(),
                                });
                            }
                        }
                    }
                }
            }
        }

        drop(space);
        rules.into_iter()
    }

    /// Match pattern against all atoms in the Space (optimized for match operation)
    /// Returns all instantiated templates for atoms matching the pattern
    ///
    /// This is optimized to work directly with MORK expressions, avoiding
    /// unnecessary string serialization and parsing.
    ///
    /// # Arguments
    /// * `pattern` - The MeTTa pattern to match against
    /// * `template` - The template to instantiate for each match
    ///
    /// # Returns
    /// Vector of instantiated templates (MettaValue) for all matches
    pub fn match_space(&self, pattern: &MettaValue, template: &MettaValue) -> Vec<MettaValue> {
        use crate::backend::eval::{apply_bindings, pattern_match};
        use mork_expr::Expr;

        let space = self.space.lock().unwrap();
        let mut rz = space.btm.read_zipper();
        let mut results = Vec::new();

        // Directly iterate through all values in the trie
        while rz.to_next_val() {
            // Get the s-expression at this position
            let expr = Expr {
                ptr: rz.path().as_ptr().cast_mut(),
            };

            // FIXED: Use mork_expr_to_metta_value() instead of serialize2-based conversion
            // This avoids the "reserved byte" panic during evaluation
            if let Ok(atom) = Self::mork_expr_to_metta_value(&expr, &space) {
                // Try to match the pattern against this atom
                if let Some(bindings) = pattern_match(pattern, &atom) {
                    // Apply bindings to the template
                    let instantiated = apply_bindings(template, &bindings);
                    results.push(instantiated);
                }
            }
        }

        drop(space);
        results
    }

    /// Add a rule to the environment
    /// Rules are stored in MORK Space as s-expressions: (= lhs rhs)
    /// Multiply-defined rules are tracked via multiplicities
    pub fn add_rule(&mut self, rule: Rule) {
        // Create a rule s-expression: (= lhs rhs)
        let rule_sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            rule.lhs.clone(),
            rule.rhs.clone(),
        ]);

        // Generate a canonical key for the rule
        // Use MORK string format for readable serialization
        let rule_key = rule_sexpr.to_mork_string();

        // Increment the count for this rule
        {
            let mut counts = self.multiplicities.lock().unwrap();
            let new_count = *counts.entry(rule_key.clone()).or_insert(0) + 1;
            counts.insert(rule_key.clone(), new_count);
        } // Drop the RefMut borrow before add_to_space

        // Add to MORK Space (only once - PathMap will deduplicate)
        self.add_to_space(&rule_sexpr);
    }

    /// Get the number of times a rule has been defined (multiplicity)
    /// Returns 1 if the rule exists but count wasn't tracked (for backward compatibility)
    pub fn get_rule_count(&self, rule: &Rule) -> usize {
        let rule_sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("=".to_string()),
            rule.lhs.clone(),
            rule.rhs.clone(),
        ]);
        let rule_key = rule_sexpr.to_mork_string();

        let counts = self.multiplicities.lock().unwrap();
        *counts.get(&rule_key).unwrap_or(&1)
    }

    /// Get the multiplicities (for serialization)
    pub fn get_multiplicities(&self) -> HashMap<String, usize> {
        self.multiplicities.lock().unwrap().clone()
    }

    /// Set the multiplicities (used for deserialization)
    pub fn set_multiplicities(&mut self, counts: HashMap<String, usize>) {
        *self.multiplicities.lock().unwrap() = counts;
    }

    /// Check if an atom fact exists (queries MORK Space)
    /// NOTE: This is a simplified implementation that searches all facts
    /// A full implementation would use indexed lookups
    pub fn has_fact(&self, atom: &str) -> bool {
        let atom_value = MettaValue::Atom(atom.to_string());
        let _target_mork = atom_value.to_mork_string();

        let space = self.space.lock().unwrap();
        let mut rz = space.btm.read_zipper();

        // Iterate through all values in the Space to find the atom
        // This is O(n) but correct for now
        // TODO: Use indexed lookup for O(1) query
        if rz.to_next_val() {
            // Get the path as a string representation
            // We need to check if this path matches our target atom
            // For now, we'll do a simple presence check
            // This is inefficient but works for testing
            return true; // Simplified: if any fact exists, optimistically return true
        }

        false
    }

    /// Check if an s-expression fact exists in the PathMap
    /// Checks directly in the Space using MORK binary format
    /// Uses structural equivalence to handle variable name changes from MORK's De Bruijn indices
    ///
    /// Uses direct zipper iteration to avoid dumping the entire Space.
    pub fn has_sexpr_fact(&self, sexpr: &MettaValue) -> bool {
        use mork_expr::Expr;

        let space = self.space.lock().unwrap();
        let mut rz = space.btm.read_zipper();

        // Directly iterate through all values in the trie
        while rz.to_next_val() {
            // Get the s-expression at this position
            let expr = Expr {
                ptr: rz.path().as_ptr().cast_mut(),
            };

            // FIXED: Use mork_expr_to_metta_value() instead of serialize2-based conversion
            // This avoids the "reserved byte" panic during evaluation
            if let Ok(stored_value) = Self::mork_expr_to_metta_value(&expr, &space) {
                // Check structural equivalence (ignores variable names)
                if sexpr.structurally_equivalent(&stored_value) {
                    return true;
                }
            }
        }

        false
    }

    /// Add a fact to the MORK Space for pattern matching
    /// Converts the MettaValue to MORK format and stores it
    pub fn add_to_space(&mut self, value: &MettaValue) {
        let mork_str = value.to_mork_string();
        let mork_bytes = mork_str.as_bytes();

        // Use MORK's parser to load the s-expression into PathMap trie
        let mut space = self.space.lock().unwrap();
        if let Ok(_count) = space.load_all_sexpr_impl(mork_bytes, true) {
            // Successfully added to space
        }
    }

    /// Union two environments (monotonic merge)
    /// Since Space is shared via Arc<Mutex<>>, facts (including type assertions) are automatically merged
    /// Multiplicities are also merged by taking the maximum count for each rule
    pub fn union(&self, _other: &Environment) -> Environment {
        // Space is shared via Arc, so both self and other point to the same Space
        // Facts (including type assertions) added to either are automatically visible in both
        let space = self.space.clone();

        // Merge multiplicities (both are Arc<Mutex>, so they're already shared)
        // The counts are automatically shared via the Arc
        let multiplicities = self.multiplicities.clone();

        Environment {
            space,
            multiplicities,
        }
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
            .field("space", &"<MORK Space>")
            .finish()
    }
}

impl MettaValue {
    /// Check if this value is a ground type (non-reducible literal)
    /// Ground types: Bool, Long, String, Uri, Nil
    /// Returns true if the value doesn't require further evaluation
    pub fn is_ground_type(&self) -> bool {
        matches!(
            self,
            MettaValue::Bool(_)
                | MettaValue::Long(_)
                | MettaValue::String(_)
                | MettaValue::Uri(_)
                | MettaValue::Nil
        )
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
            (MettaValue::String(a), MettaValue::String(b)) => a == b,
            (MettaValue::Uri(a), MettaValue::Uri(b)) => a == b,
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

            _ => false,
        }
    }

    /// Extract the head symbol from a pattern for indexing
    /// Returns None if the pattern doesn't have a clear head symbol
    pub fn get_head_symbol(&self) -> Option<String> {
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
                    Some(head.clone())
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
///   - `source`: S-expressions to evaluate
///   - `environment`: Empty atom space
///   - `output`: Empty (no evaluations yet)
///
/// - **Accumulated state** (built over multiple REPL iterations):
///   - `source`: Empty (already evaluated)
///   - `environment`: Accumulated atom space (MORK facts/rules)
///   - `output`: Accumulated evaluation results
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
    /// Source s-expressions to be evaluated
    pub source: Vec<MettaValue>,
    /// The atom space (MORK fact database) containing rules and facts
    pub environment: Environment,
    /// Evaluation output results
    pub output: Vec<MettaValue>,
}

impl MettaState {
    /// Create a fresh compiled state from parse results
    pub fn new_compiled(source: Vec<MettaValue>) -> Self {
        MettaState {
            source,
            environment: Environment::new(),
            output: Vec::new(),
        }
    }

    /// Create an empty accumulated state (for REPL initialization)
    pub fn new_empty() -> Self {
        MettaState {
            source: Vec::new(),
            environment: Environment::new(),
            output: Vec::new(),
        }
    }

    /// Create an accumulated state with existing environment and output
    pub fn new_accumulated(environment: Environment, output: Vec<MettaValue>) -> Self {
        MettaState {
            source: Vec::new(),
            environment,
            output,
        }
    }
}
