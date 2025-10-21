/// Collection types for PathMap parsing
///
/// Provides type-safe collection handling for sets, lists, tuples, and maps
/// parsed from Rholang PathMap output.
use mettatron::backend::models::MettaValue;

/// Collection value types that extend MettaValue with collection semantics
#[derive(Debug, Clone, PartialEq)]
pub enum CollectionValue {
    /// Par set: {|a, b, c|} - represented as Vec (ordering preserved for testing)
    Set(Vec<MettaValue>),

    /// List: [a, b, c]
    List(Vec<MettaValue>),

    /// Tuple/S-expression: (a, b, c)
    Tuple(Vec<MettaValue>),

    /// Map: {k1: v1, k2: v2} - represented as Vec of pairs
    Map(Vec<(MettaValue, MettaValue)>),

    /// Single value (not a collection)
    Single(MettaValue),
}

impl CollectionValue {
    /// Create a set from values
    pub fn set(values: Vec<MettaValue>) -> Self {
        CollectionValue::Set(values)
    }

    /// Create a list from values
    pub fn list(values: Vec<MettaValue>) -> Self {
        CollectionValue::List(values)
    }

    /// Create a tuple from values
    pub fn tuple(values: Vec<MettaValue>) -> Self {
        CollectionValue::Tuple(values)
    }

    /// Create a map from key-value pairs
    pub fn map(pairs: Vec<(MettaValue, MettaValue)>) -> Self {
        CollectionValue::Map(pairs)
    }

    /// Create a single value
    pub fn single(value: MettaValue) -> Self {
        CollectionValue::Single(value)
    }

    /// Get as list (converts from other collection types)
    pub fn as_list(&self) -> Vec<MettaValue> {
        match self {
            CollectionValue::List(v) => v.clone(),
            CollectionValue::Set(s) => s.clone(),
            CollectionValue::Tuple(t) => t.clone(),
            CollectionValue::Map(m) => {
                // Convert map to list of tuples
                m.iter()
                    .map(|(k, v)| MettaValue::SExpr(vec![k.clone(), v.clone()]))
                    .collect()
            }
            CollectionValue::Single(v) => vec![v.clone()],
        }
    }

    /// Get the number of elements
    pub fn len(&self) -> usize {
        match self {
            CollectionValue::List(v) => v.len(),
            CollectionValue::Set(s) => s.len(),
            CollectionValue::Tuple(t) => t.len(),
            CollectionValue::Map(m) => m.len(),
            CollectionValue::Single(_) => 1,
        }
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
