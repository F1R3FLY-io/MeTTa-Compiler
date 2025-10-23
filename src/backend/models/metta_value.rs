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

impl MettaValue {
    /// Check if this value is a ground type (non-reducible literal)
    /// Ground types: Bool, Long, Float, String, Uri, Nil
    /// Returns true if the value doesn't require further evaluation
    pub fn is_ground_type(&self) -> bool {
        matches!(
            self,
            MettaValue::Bool(_)
                | MettaValue::Long(_)
                | MettaValue::Float(_)
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
            MettaValue::Float(f) => f.to_string(),
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
