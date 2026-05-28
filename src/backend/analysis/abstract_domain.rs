//! Abstract Domain Definitions for AAM Analysis
//!
//! Defines the abstract counterparts of the concrete SECK machine components:
//! abstract addresses, abstract values, abstract value sets (with widening),
//! abstract stores, and abstract environments.

use std::collections::{BTreeSet, HashMap};

use smallvec::SmallVec;

use crate::backend::models::{MettaValue, MettaValueTrait};

// ============================================================================
// Abstract Type Tags
// ============================================================================

/// Abstract type tags — coarser than `AbstractValue`, used for widening
/// and type specialization analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AbstractType {
    Atom,
    Bool,
    Long,
    Float,
    String,
    SExpr,
    Error,
    Unit,
    Empty,
    Space,
    State,
    Quoted,
    /// Union of all types (total loss of precision).
    Top,
}

impl AbstractType {
    /// Join two types into their least upper bound.
    pub fn join(self, other: Self) -> Self {
        if self == other {
            self
        } else {
            AbstractType::Top
        }
    }
}

// ============================================================================
// Abstract Address
// ============================================================================

/// Abstract address: identifies a location in the abstract store.
///
/// Uses allocation-site (expression hash) + k-CFA context.
/// k=0: monovariant (context is empty — fastest, least precise).
/// k=1: 1-CFA (context holds the most recent call site).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AbstractAddr {
    /// Content hash of the expression at the allocation site.
    pub site: u64,
    /// Call-site context: last k call sites.
    pub context: SmallVec<[u64; 2]>,
}

impl AbstractAddr {
    /// Create a monovariant address (k=0, no context).
    #[inline]
    pub fn mono(site: u64) -> Self {
        Self {
            site,
            context: SmallVec::new(),
        }
    }

    /// Create a context-sensitive address (k-CFA).
    pub fn with_context(site: u64, ctx: &[u64], k: u8) -> Self {
        let k = k as usize;
        let start = if ctx.len() > k { ctx.len() - k } else { 0 };
        Self {
            site,
            context: SmallVec::from_slice(&ctx[start..]),
        }
    }
}

// ============================================================================
// Abstract Value
// ============================================================================

/// Abstract value: represents a possible concrete value at an abstract address.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AbstractValue {
    /// Known concrete atom symbol.
    Atom(&'static str),
    /// Known concrete boolean.
    Bool(bool),
    /// Known concrete integer.
    Long(i64),
    /// Known concrete float (stored as bits for Eq+Hash).
    Float(u64),
    /// Known concrete string.
    String(&'static str),
    /// S-expression with known head and arity.
    SExpr {
        head: Option<&'static str>,
        arity: u16,
    },
    /// Error value.
    Error,
    /// Unit value.
    Unit,
    /// Empty value.
    Empty,
    /// Any value of a known type (precision lost via widening).
    AnyOfType(AbstractType),
    /// Top: any value at all.
    Top,
}

impl AbstractValue {
    /// Get the abstract type of this value.
    pub fn abstract_type(&self) -> AbstractType {
        match self {
            Self::Atom(_) => AbstractType::Atom,
            Self::Bool(_) => AbstractType::Bool,
            Self::Long(_) => AbstractType::Long,
            Self::Float(_) => AbstractType::Float,
            Self::String(_) => AbstractType::String,
            Self::SExpr { .. } => AbstractType::SExpr,
            Self::Error => AbstractType::Error,
            Self::Unit => AbstractType::Unit,
            Self::Empty => AbstractType::Empty,
            Self::AnyOfType(t) => *t,
            Self::Top => AbstractType::Top,
        }
    }
}

/// Inject a concrete `MettaValue` into the abstract domain.
pub fn alpha(value: &MettaValue) -> AbstractValue {
    if let Some(name) = value.as_atom() {
        return AbstractValue::Atom(name);
    }
    if let Some(b) = value.as_bool() {
        return AbstractValue::Bool(b);
    }
    if let Some(n) = value.as_long() {
        return AbstractValue::Long(n);
    }
    if let Some(f) = value.as_float() {
        return AbstractValue::Float(f.to_bits());
    }
    if let Some(_s) = value.as_string() {
        // String may not be 'static in abstract domain; use type tag
        return AbstractValue::AnyOfType(AbstractType::String);
    }
    if let Some(items) = value.as_sexpr() {
        let head = items.first().and_then(|h| h.as_atom());
        return AbstractValue::SExpr {
            head,
            arity: items.len() as u16,
        };
    }
    if value.is_error() {
        return AbstractValue::Error;
    }
    if value.is_unit() {
        return AbstractValue::Unit;
    }
    if value.is_empty() {
        return AbstractValue::Empty;
    }
    AbstractValue::Top
}

// ============================================================================
// Abstract Value Set (with widening)
// ============================================================================

/// Maximum set size before widening to `AnyOfType`.
const DEFAULT_MAX_SET_SIZE: usize = 32;

/// Set of abstract values at an abstract address.
///
/// When multiple concrete values map to the same abstract address, we take
/// their join (union). If the set exceeds `MAX_SET_SIZE`, it is widened to
/// `AnyOfType(join of all types)` to guarantee termination.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbstractValueSet {
    values: BTreeSet<AbstractValue>,
    max_size: usize,
}

impl AbstractValueSet {
    pub fn new() -> Self {
        Self {
            values: BTreeSet::new(),
            max_size: DEFAULT_MAX_SET_SIZE,
        }
    }

    pub fn with_max_size(max_size: usize) -> Self {
        Self {
            values: BTreeSet::new(),
            max_size,
        }
    }

    pub fn singleton(v: AbstractValue) -> Self {
        let mut s = Self::new();
        s.values.insert(v);
        s
    }

    /// Join another value set into this one. Returns `true` if this set changed.
    pub fn join(&mut self, other: &AbstractValueSet) -> bool {
        let before = self.values.len();
        for v in &other.values {
            self.values.insert(v.clone());
        }
        self.maybe_widen();
        self.values.len() != before
    }

    /// Insert a single value. Returns `true` if the set changed.
    pub fn insert(&mut self, v: AbstractValue) -> bool {
        let changed = self.values.insert(v);
        self.maybe_widen();
        changed
    }

    /// Widen if set exceeds max size.
    fn maybe_widen(&mut self) {
        if self.values.len() > self.max_size {
            let widened_type = self
                .values
                .iter()
                .map(|v| v.abstract_type())
                .fold(None, |acc: Option<AbstractType>, t| {
                    Some(match acc {
                        None => t,
                        Some(prev) => prev.join(t),
                    })
                })
                .unwrap_or(AbstractType::Top);
            self.values.clear();
            self.values.insert(AbstractValue::AnyOfType(widened_type));
        }
    }

    /// Check if this set contains exactly one value (deterministic).
    #[inline]
    pub fn is_singleton(&self) -> bool {
        self.values.len() == 1
    }

    /// Get the single value if this is a singleton set.
    pub fn as_singleton(&self) -> Option<&AbstractValue> {
        if self.values.len() == 1 {
            self.values.iter().next()
        } else {
            None
        }
    }

    /// Get the singleton type if all values have the same type.
    pub fn singleton_type(&self) -> Option<AbstractType> {
        let mut iter = self.values.iter();
        let first_type = iter.next()?.abstract_type();
        for v in iter {
            if v.abstract_type() != first_type {
                return None;
            }
        }
        Some(first_type)
    }

    /// Number of values in the set.
    #[inline]
    pub fn len(&self) -> usize {
        self.values.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Iterate over values.
    pub fn iter(&self) -> impl Iterator<Item = &AbstractValue> {
        self.values.iter()
    }
}

impl Default for AbstractValueSet {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Abstract Store
// ============================================================================

/// Abstract store: maps abstract addresses to abstract value sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbstractStore {
    store: HashMap<AbstractAddr, AbstractValueSet>,
}

impl AbstractStore {
    pub fn new() -> Self {
        Self {
            store: HashMap::new(),
        }
    }

    /// Allocate a value at an abstract address. Returns `true` if store changed.
    pub fn alloc(&mut self, addr: AbstractAddr, value: AbstractValue) -> bool {
        self.store
            .entry(addr)
            .or_insert_with(AbstractValueSet::new)
            .insert(value)
    }

    /// Look up the value set at an address.
    pub fn lookup(&self, addr: &AbstractAddr) -> Option<&AbstractValueSet> {
        self.store.get(addr)
    }

    /// Number of addresses in the store.
    #[inline]
    pub fn len(&self) -> usize {
        self.store.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.store.is_empty()
    }
}

impl Default for AbstractStore {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Abstract Environment
// ============================================================================

/// Abstract environment: maps variable names to abstract addresses.
///
/// Uses `BTreeMap` instead of `HashMap` so the environment can derive `Hash`
/// (needed because `AbstractState` contains `AbstractEnv` and must be hashable
/// for the worklist's `seen` set).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AbstractEnv {
    bindings: std::collections::BTreeMap<&'static str, AbstractAddr>,
}

impl AbstractEnv {
    pub fn new() -> Self {
        Self {
            bindings: std::collections::BTreeMap::new(),
        }
    }

    pub fn lookup(&self, name: &str) -> Option<&AbstractAddr> {
        self.bindings.get(name)
    }

    pub fn extend(&self, name: &'static str, addr: AbstractAddr) -> Self {
        let mut new_env = self.clone();
        new_env.bindings.insert(name, addr);
        new_env
    }

    pub fn len(&self) -> usize {
        self.bindings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }
}

impl Default for AbstractEnv {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Analysis Configuration
// ============================================================================

/// Configuration for the abstract analysis.
#[derive(Debug, Clone)]
pub struct AnalysisConfig {
    /// Context sensitivity: 0 = monovariant, 1 = 1-CFA.
    pub k: u8,
    /// Maximum abstract value set size before widening.
    pub max_set_size: usize,
    /// Maximum fixed-point iterations before giving up.
    pub max_iterations: u32,
    /// Maximum number of abstract states to explore.
    pub max_states: usize,
    /// Whether to track purity information.
    pub track_purity: bool,
    /// Whether to track determinism information.
    pub track_determinism: bool,
    /// Whether to track type specialization.
    pub track_types: bool,
}

impl Default for AnalysisConfig {
    fn default() -> Self {
        Self {
            k: 0,
            max_set_size: 32,
            max_iterations: 10_000,
            max_states: 100_000,
            track_purity: true,
            track_determinism: true,
            track_types: true,
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{global_factory, MettaValueFactory};

    fn f() -> crate::backend::models::ActiveFactory {
        global_factory()
    }

    #[test]
    fn test_alpha_atom() {
        let v = f().atom("hello");
        assert_eq!(alpha(&v), AbstractValue::Atom("hello"));
    }

    #[test]
    fn test_alpha_long() {
        let v = f().long(42);
        assert_eq!(alpha(&v), AbstractValue::Long(42));
    }

    #[test]
    fn test_alpha_bool() {
        let v = f().bool(true);
        assert_eq!(alpha(&v), AbstractValue::Bool(true));
    }

    #[test]
    fn test_alpha_sexpr() {
        let v = f().sexpr(vec![f().atom("+"), f().long(1), f().long(2)]);
        match alpha(&v) {
            AbstractValue::SExpr { head, arity } => {
                assert_eq!(head, Some("+"));
                assert_eq!(arity, 3);
            }
            other => panic!("Expected SExpr, got {:?}", other),
        }
    }

    #[test]
    fn test_alpha_unit() {
        assert_eq!(alpha(&f().unit()), AbstractValue::Unit);
    }

    #[test]
    fn test_alpha_empty() {
        assert_eq!(alpha(&f().empty()), AbstractValue::Empty);
    }

    #[test]
    fn test_abstract_addr_mono() {
        let addr = AbstractAddr::mono(12345);
        assert_eq!(addr.site, 12345);
        assert!(addr.context.is_empty());
    }

    #[test]
    fn test_abstract_addr_context() {
        let addr = AbstractAddr::with_context(100, &[1, 2, 3, 4, 5], 2);
        assert_eq!(addr.site, 100);
        assert_eq!(addr.context.as_slice(), &[4, 5]);
    }

    #[test]
    fn test_value_set_singleton() {
        let set = AbstractValueSet::singleton(AbstractValue::Long(42));
        assert!(set.is_singleton());
        assert_eq!(set.as_singleton(), Some(&AbstractValue::Long(42)));
    }

    #[test]
    fn test_value_set_join() {
        let mut set1 = AbstractValueSet::singleton(AbstractValue::Long(1));
        let set2 = AbstractValueSet::singleton(AbstractValue::Long(2));

        let changed = set1.join(&set2);
        assert!(changed);
        assert_eq!(set1.len(), 2);
        assert!(!set1.is_singleton());
    }

    #[test]
    fn test_value_set_join_idempotent() {
        let mut set1 = AbstractValueSet::singleton(AbstractValue::Long(1));
        let set2 = AbstractValueSet::singleton(AbstractValue::Long(1));

        let changed = set1.join(&set2);
        assert!(!changed);
        assert_eq!(set1.len(), 1);
    }

    #[test]
    fn test_value_set_widening() {
        let mut set = AbstractValueSet::with_max_size(3);
        set.insert(AbstractValue::Long(1));
        set.insert(AbstractValue::Long(2));
        set.insert(AbstractValue::Long(3));
        assert_eq!(set.len(), 3);

        // Fourth insert triggers widening
        set.insert(AbstractValue::Long(4));
        assert_eq!(set.len(), 1); // Widened to AnyOfType
        assert_eq!(
            set.as_singleton(),
            Some(&AbstractValue::AnyOfType(AbstractType::Long))
        );
    }

    #[test]
    fn test_value_set_singleton_type() {
        let mut set = AbstractValueSet::new();
        set.insert(AbstractValue::Long(1));
        set.insert(AbstractValue::Long(2));
        assert_eq!(set.singleton_type(), Some(AbstractType::Long));

        set.insert(AbstractValue::Bool(true));
        assert_eq!(set.singleton_type(), None); // Mixed types
    }

    #[test]
    fn test_store_alloc_lookup() {
        let mut store = AbstractStore::new();
        let addr = AbstractAddr::mono(42);

        let changed = store.alloc(addr.clone(), AbstractValue::Long(1));
        assert!(changed);

        let set = store.lookup(&addr).expect("should exist");
        assert!(set.is_singleton());
        assert_eq!(set.as_singleton(), Some(&AbstractValue::Long(1)));
    }

    #[test]
    fn test_store_alloc_join() {
        let mut store = AbstractStore::new();
        let addr = AbstractAddr::mono(42);

        store.alloc(addr.clone(), AbstractValue::Long(1));
        let changed = store.alloc(addr.clone(), AbstractValue::Long(2));
        assert!(changed);

        let set = store.lookup(&addr).expect("should exist");
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_env_extend_lookup() {
        let env = AbstractEnv::new();
        let addr = AbstractAddr::mono(42);

        let env2 = env.extend("$x", addr.clone());
        assert_eq!(env2.lookup("$x"), Some(&addr));
        assert!(env.lookup("$x").is_none()); // Original unchanged
    }

    #[test]
    fn test_config_default() {
        let config = AnalysisConfig::default();
        assert_eq!(config.k, 0);
        assert_eq!(config.max_set_size, 32);
        assert!(config.track_purity);
    }

    #[test]
    fn test_type_join() {
        assert_eq!(
            AbstractType::Long.join(AbstractType::Long),
            AbstractType::Long
        );
        assert_eq!(
            AbstractType::Long.join(AbstractType::Bool),
            AbstractType::Top
        );
    }
}
