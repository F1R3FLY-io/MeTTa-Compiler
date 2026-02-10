//! Arena-allocated MeTTa values for transient evaluation.
//!
//! ArenaValue<'a> provides the same semantics as MettaValue but allocates from
//! a bumpalo arena instead of using Arc-wrapped heap allocations. This eliminates
//! per-node allocation and deallocation overhead.
//!
//! ## Key Benefits
//!
//! - **O(1) bulk deallocation**: When the arena is dropped, all values are freed
//!   instantly without traversing the tree structure.
//! - **No reference counting**: Values don't need Arc overhead or atomic operations.
//! - **Copy semantics**: ArenaValue is just a pointer, so Clone/Copy is free.
//! - **Better cache locality**: Arena allocations are contiguous in memory.
//!
//! ## Memory Safety
//!
//! The lifetime parameter `'a` ensures values cannot outlive their arena.
//! All ArenaValue references are tied to the arena's lifetime.

use bumpalo::Bump;
use bumpalo::collections::Vec as BumpVec;
use std::fmt;

use super::metta_value_trait::{MettaValue as MettaValueTrait, MettaValueFactory};
use super::{MemoHandle, SpaceHandle};

/// Arena-allocated MeTTa value with O(1) clone (just copies the pointer).
///
/// This is a thin wrapper around a reference to ArenaValueInner, providing
/// the same interface as MettaValue but using arena allocation.
#[derive(Clone, Copy)]
pub struct ArenaValue<'a> {
    inner: &'a ArenaValueInner<'a>,
}

/// The actual value enum, allocated in the arena.
///
/// This mirrors MettaValueInner but uses arena-allocated collections.
#[derive(Debug)]
pub enum ArenaValueInner<'a> {
    /// An atom (symbol, variable, or literal) - string allocated in arena
    Atom(&'a str),
    /// A boolean literal
    Bool(bool),
    /// An integer literal
    Long(i64),
    /// A floating point literal
    Float(f64),
    /// A string literal - string content allocated in arena
    String(&'a str),
    /// An s-expression (list of values) - Vec allocated in arena
    SExpr(BumpVec<'a, ArenaValue<'a>>),
    /// An error with message and details
    Error(&'a str, ArenaValue<'a>),
    /// A type (first-class types as atoms)
    Type(ArenaValue<'a>),
    /// A conjunction of goals (MORK-style logical AND)
    Conjunction(BumpVec<'a, ArenaValue<'a>>),
    /// A first-class space value - reuses existing SpaceHandle
    Space(SpaceHandle),
    /// A reference to a mutable state cell (id)
    State(u64),
    /// Unit value for side-effecting operations
    Unit,
    /// A memoization table - reuses existing MemoHandle
    Memo(MemoHandle),
    /// Empty sentinel
    Empty,
}

// ============================================================================
// Thread Safety for Static Arena Values
// ============================================================================
//
// ArenaValue<'static> is safe to share across threads because:
// 1. The static arena is thread-local (each thread has its own via thread_local!)
// 2. ArenaValue contains only immutable references to arena-allocated data
// 3. Once created, arena values are never mutated
// 4. The 'static lifetime ensures the referenced data lives for the program duration
//
// The unsafe impl is required because:
// - BumpVec internally holds a &Bump which has Cell (interior mutability)
// - The Rust compiler conservatively marks types with Cell as !Sync
// - However, we only use the arena for allocation, never for mutation after creation
//
// SAFETY INVARIANT: Values must only be read, never mutated, after creation.
// This is enforced by ArenaValue's API which provides no mutation methods.

// SAFETY: ArenaValue<'static> can be sent between threads because:
// - It's an immutable reference to 'static data
// - The 'static arena is thread-local, so values created in one thread
//   are not accessible from another thread's arena
// - Once created, the data is never mutated
unsafe impl Send for ArenaValue<'static> {}

// SAFETY: ArenaValue<'static> can be shared between threads because:
// - It only provides immutable access to the underlying data
// - No mutation methods exist on ArenaValue
// - The referenced data is immutable after creation
unsafe impl Sync for ArenaValue<'static> {}

// SAFETY: ArenaValueInner<'static> can be sent between threads for the same reasons
unsafe impl Send for ArenaValueInner<'static> {}

// SAFETY: ArenaValueInner<'static> can be shared between threads for the same reasons
unsafe impl Sync for ArenaValueInner<'static> {}

impl<'a> ArenaValue<'a> {
    /// Access the inner enum for pattern matching
    #[inline]
    pub fn inner(&self) -> &ArenaValueInner<'a> {
        self.inner
    }

    // ========================================================================
    // Constructors - allocate in arena
    // ========================================================================

    /// Create an Atom variant
    #[inline]
    pub fn atom(arena: &'a Bump, s: &str) -> Self {
        let s = arena.alloc_str(s);
        ArenaValue {
            inner: arena.alloc(ArenaValueInner::Atom(s)),
        }
    }

    /// Create a Bool variant
    #[inline]
    pub fn bool(arena: &'a Bump, b: bool) -> Self {
        ArenaValue {
            inner: arena.alloc(ArenaValueInner::Bool(b)),
        }
    }

    /// Create a Long variant
    #[inline]
    pub fn long(arena: &'a Bump, n: i64) -> Self {
        ArenaValue {
            inner: arena.alloc(ArenaValueInner::Long(n)),
        }
    }

    /// Create a Float variant
    #[inline]
    pub fn float(arena: &'a Bump, f: f64) -> Self {
        ArenaValue {
            inner: arena.alloc(ArenaValueInner::Float(f)),
        }
    }

    /// Create a String variant
    #[inline]
    pub fn string(arena: &'a Bump, s: &str) -> Self {
        let s = arena.alloc_str(s);
        ArenaValue {
            inner: arena.alloc(ArenaValueInner::String(s)),
        }
    }

    /// Create an SExpr variant from an iterator
    #[inline]
    pub fn sexpr<I>(arena: &'a Bump, items: I) -> Self
    where
        I: IntoIterator<Item = ArenaValue<'a>>,
    {
        let mut vec = BumpVec::new_in(arena);
        vec.extend(items);
        ArenaValue {
            inner: arena.alloc(ArenaValueInner::SExpr(vec)),
        }
    }

    /// Create an empty SExpr variant
    #[inline]
    pub fn sexpr_empty(arena: &'a Bump) -> Self {
        ArenaValue {
            inner: arena.alloc(ArenaValueInner::SExpr(BumpVec::new_in(arena))),
        }
    }

    /// Create an Error variant
    #[inline]
    pub fn error(arena: &'a Bump, msg: &str, details: ArenaValue<'a>) -> Self {
        let msg = arena.alloc_str(msg);
        ArenaValue {
            inner: arena.alloc(ArenaValueInner::Error(msg, details)),
        }
    }

    /// Create a Type variant
    #[inline]
    pub fn r#type(arena: &'a Bump, inner: ArenaValue<'a>) -> Self {
        ArenaValue {
            inner: arena.alloc(ArenaValueInner::Type(inner)),
        }
    }

    /// Create a Conjunction variant
    #[inline]
    pub fn conjunction<I>(arena: &'a Bump, goals: I) -> Self
    where
        I: IntoIterator<Item = ArenaValue<'a>>,
    {
        let mut vec = BumpVec::new_in(arena);
        vec.extend(goals);
        ArenaValue {
            inner: arena.alloc(ArenaValueInner::Conjunction(vec)),
        }
    }

    /// Create a Space variant
    #[inline]
    pub fn space(arena: &'a Bump, handle: SpaceHandle) -> Self {
        ArenaValue {
            inner: arena.alloc(ArenaValueInner::Space(handle)),
        }
    }

    /// Create a State variant
    #[inline]
    pub fn state(arena: &'a Bump, id: u64) -> Self {
        ArenaValue {
            inner: arena.alloc(ArenaValueInner::State(id)),
        }
    }

    /// Create a Unit variant
    #[inline]
    pub fn unit(arena: &'a Bump) -> Self {
        ArenaValue {
            inner: arena.alloc(ArenaValueInner::Unit),
        }
    }

    /// Create a Memo variant
    #[inline]
    pub fn memo(arena: &'a Bump, handle: MemoHandle) -> Self {
        ArenaValue {
            inner: arena.alloc(ArenaValueInner::Memo(handle)),
        }
    }

    /// Create an Empty variant
    #[inline]
    pub fn empty(arena: &'a Bump) -> Self {
        ArenaValue {
            inner: arena.alloc(ArenaValueInner::Empty),
        }
    }

    // ========================================================================
    // Type checking and inspection methods
    // ========================================================================

    /// Check if this is an Atom variant
    #[inline]
    pub fn is_atom(&self) -> bool {
        matches!(self.inner, ArenaValueInner::Atom(_))
    }

    /// Check if this is a Bool variant
    #[inline]
    pub fn is_bool(&self) -> bool {
        matches!(self.inner, ArenaValueInner::Bool(_))
    }

    /// Check if this is a Long variant
    #[inline]
    pub fn is_long(&self) -> bool {
        matches!(self.inner, ArenaValueInner::Long(_))
    }

    /// Check if this is a Float variant
    #[inline]
    pub fn is_float(&self) -> bool {
        matches!(self.inner, ArenaValueInner::Float(_))
    }

    /// Check if this is a String variant
    #[inline]
    pub fn is_string(&self) -> bool {
        matches!(self.inner, ArenaValueInner::String(_))
    }

    /// Check if this is an SExpr variant
    #[inline]
    pub fn is_sexpr(&self) -> bool {
        matches!(self.inner, ArenaValueInner::SExpr(_))
    }

    /// Check if this is an Error variant
    #[inline]
    pub fn is_error(&self) -> bool {
        matches!(self.inner, ArenaValueInner::Error(_, _))
    }

    /// Check if this is a Type variant
    #[inline]
    pub fn is_type(&self) -> bool {
        matches!(self.inner, ArenaValueInner::Type(_))
    }

    /// Check if this is a Conjunction variant
    #[inline]
    pub fn is_conjunction(&self) -> bool {
        matches!(self.inner, ArenaValueInner::Conjunction(_))
    }

    /// Check if this is a Space variant
    #[inline]
    pub fn is_space(&self) -> bool {
        matches!(self.inner, ArenaValueInner::Space(_))
    }

    /// Check if this is a State variant
    #[inline]
    pub fn is_state(&self) -> bool {
        matches!(self.inner, ArenaValueInner::State(_))
    }

    /// Check if this is a Unit variant
    #[inline]
    pub fn is_unit(&self) -> bool {
        matches!(self.inner, ArenaValueInner::Unit)
    }

    /// Check if this is a Memo variant
    #[inline]
    pub fn is_memo(&self) -> bool {
        matches!(self.inner, ArenaValueInner::Memo(_))
    }

    /// Check if this is an Empty variant
    #[inline]
    pub fn is_empty(&self) -> bool {
        matches!(self.inner, ArenaValueInner::Empty)
    }

    /// Check if this value is a variable (Atom starting with $)
    #[inline]
    pub fn is_variable(&self) -> bool {
        matches!(self.inner, ArenaValueInner::Atom(s) if s.starts_with('$'))
    }

    // ========================================================================
    // Accessor methods for extracting inner values
    // ========================================================================

    /// Try to extract as atom string
    #[inline]
    pub fn as_atom(&self) -> Option<&'a str> {
        match self.inner {
            ArenaValueInner::Atom(s) => Some(s),
            _ => None,
        }
    }

    /// Try to extract as bool
    #[inline]
    pub fn as_bool(&self) -> Option<bool> {
        match self.inner {
            ArenaValueInner::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Try to extract as i64
    #[inline]
    pub fn as_long(&self) -> Option<i64> {
        match self.inner {
            ArenaValueInner::Long(n) => Some(*n),
            _ => None,
        }
    }

    /// Try to extract as f64
    #[inline]
    pub fn as_float(&self) -> Option<f64> {
        match self.inner {
            ArenaValueInner::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// Try to extract as string
    #[inline]
    pub fn as_string(&self) -> Option<&'a str> {
        match self.inner {
            ArenaValueInner::String(s) => Some(s),
            _ => None,
        }
    }

    /// Try to extract as sexpr items
    #[inline]
    pub fn as_sexpr(&self) -> Option<&[ArenaValue<'a>]> {
        match self.inner {
            ArenaValueInner::SExpr(items) => Some(items.as_slice()),
            _ => None,
        }
    }

    /// Try to extract as error (message, details)
    #[inline]
    pub fn as_error(&self) -> Option<(&'a str, ArenaValue<'a>)> {
        match self.inner {
            ArenaValueInner::Error(msg, details) => Some((msg, *details)),
            _ => None,
        }
    }

    /// Try to extract as type inner value
    #[inline]
    pub fn as_type(&self) -> Option<ArenaValue<'a>> {
        match self.inner {
            ArenaValueInner::Type(inner) => Some(*inner),
            _ => None,
        }
    }

    /// Try to extract as conjunction goals
    #[inline]
    pub fn as_conjunction(&self) -> Option<&[ArenaValue<'a>]> {
        match self.inner {
            ArenaValueInner::Conjunction(goals) => Some(goals.as_slice()),
            _ => None,
        }
    }

    /// Try to extract as space handle
    #[inline]
    pub fn as_space(&self) -> Option<&SpaceHandle> {
        match self.inner {
            ArenaValueInner::Space(handle) => Some(handle),
            _ => None,
        }
    }

    /// Try to extract as state id
    #[inline]
    pub fn as_state(&self) -> Option<u64> {
        match self.inner {
            ArenaValueInner::State(id) => Some(*id),
            _ => None,
        }
    }

    /// Try to extract as memo handle
    #[inline]
    pub fn as_memo(&self) -> Option<&MemoHandle> {
        match self.inner {
            ArenaValueInner::Memo(handle) => Some(handle),
            _ => None,
        }
    }

    /// Get the type name of this value as a string slice
    pub fn type_name(&self) -> &'static str {
        match self.inner {
            ArenaValueInner::Atom(s) if s.starts_with('$') => "Variable",
            ArenaValueInner::Atom(_) => "Symbol",
            ArenaValueInner::Bool(_) => "Bool",
            ArenaValueInner::Long(_) => "Number",
            ArenaValueInner::Float(_) => "Number",
            ArenaValueInner::String(_) => "String",
            ArenaValueInner::SExpr(_) => "Expression",
            ArenaValueInner::Unit => "Unit",
            ArenaValueInner::Error(_, _) => "Error",
            ArenaValueInner::Type(_) => "Type",
            ArenaValueInner::Conjunction(_) => "Conjunction",
            ArenaValueInner::Space(_) => "Space",
            ArenaValueInner::State(_) => "State",
            ArenaValueInner::Memo(_) => "Memo",
            ArenaValueInner::Empty => "Empty",
        }
    }
}

// ============================================================================
// Trait implementations
// ============================================================================

impl<'a> fmt::Debug for ArenaValue<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

impl<'a> fmt::Display for ArenaValue<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.inner {
            ArenaValueInner::Atom(s) => write!(f, "{}", s),
            ArenaValueInner::Bool(b) => write!(f, "{}", if *b { "True" } else { "False" }),
            ArenaValueInner::Long(n) => write!(f, "{}", n),
            ArenaValueInner::Float(v) => write!(f, "{}", v),
            ArenaValueInner::String(s) => write!(f, "\"{}\"", s),
            ArenaValueInner::SExpr(items) => {
                write!(f, "(")?;
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "{}", item)?;
                }
                write!(f, ")")
            }
            ArenaValueInner::Unit => write!(f, "()"),
            ArenaValueInner::Error(msg, details) => write!(f, "(Error {} {})", msg, details),
            ArenaValueInner::Type(inner) => write!(f, "(: {})", inner),
            ArenaValueInner::Conjunction(goals) => {
                write!(f, "(,")?;
                for goal in goals.iter() {
                    write!(f, " {}", goal)?;
                }
                write!(f, ")")
            }
            ArenaValueInner::Space(handle) => write!(f, "<Space:{}>", handle.name),
            ArenaValueInner::State(id) => write!(f, "<State:{}>", id),
            ArenaValueInner::Memo(handle) => write!(f, "<Memo:{}>", handle.name),
            ArenaValueInner::Empty => write!(f, "Empty"),
        }
    }
}

impl<'a> PartialEq for ArenaValue<'a> {
    fn eq(&self, other: &Self) -> bool {
        // Fast path: pointer equality
        std::ptr::eq(self.inner, other.inner) || self.inner == other.inner
    }
}

impl<'a> PartialEq for ArenaValueInner<'a> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ArenaValueInner::Atom(a), ArenaValueInner::Atom(b)) => a == b,
            (ArenaValueInner::Bool(a), ArenaValueInner::Bool(b)) => a == b,
            (ArenaValueInner::Long(a), ArenaValueInner::Long(b)) => a == b,
            (ArenaValueInner::Float(a), ArenaValueInner::Float(b)) => a == b,
            (ArenaValueInner::String(a), ArenaValueInner::String(b)) => a == b,
            (ArenaValueInner::SExpr(a), ArenaValueInner::SExpr(b)) => a == b,
            (ArenaValueInner::Unit, ArenaValueInner::Unit) => true,
            (ArenaValueInner::Error(ma, da), ArenaValueInner::Error(mb, db)) => ma == mb && da == db,
            (ArenaValueInner::Type(a), ArenaValueInner::Type(b)) => a == b,
            (ArenaValueInner::Conjunction(a), ArenaValueInner::Conjunction(b)) => a == b,
            (ArenaValueInner::Space(a), ArenaValueInner::Space(b)) => a.id == b.id,
            (ArenaValueInner::State(a), ArenaValueInner::State(b)) => a == b,
            (ArenaValueInner::Memo(a), ArenaValueInner::Memo(b)) => a.id == b.id,
            (ArenaValueInner::Empty, ArenaValueInner::Empty) => true,
            _ => false,
        }
    }
}

impl<'a> Eq for ArenaValue<'a> {}
impl<'a> Eq for ArenaValueInner<'a> {}

// ============================================================================
// MettaValue trait implementation for ArenaValue
// ============================================================================

impl<'a> MettaValueTrait for ArenaValue<'a> {
    type SExprSlice = [ArenaValue<'a>];

    #[inline]
    fn is_atom(&self) -> bool {
        matches!(self.inner, ArenaValueInner::Atom(_))
    }

    #[inline]
    fn is_bool(&self) -> bool {
        matches!(self.inner, ArenaValueInner::Bool(_))
    }

    #[inline]
    fn is_long(&self) -> bool {
        matches!(self.inner, ArenaValueInner::Long(_))
    }

    #[inline]
    fn is_float(&self) -> bool {
        matches!(self.inner, ArenaValueInner::Float(_))
    }

    #[inline]
    fn is_string(&self) -> bool {
        matches!(self.inner, ArenaValueInner::String(_))
    }

    #[inline]
    fn is_sexpr(&self) -> bool {
        matches!(self.inner, ArenaValueInner::SExpr(_))
    }

    #[inline]
    fn is_error(&self) -> bool {
        matches!(self.inner, ArenaValueInner::Error(_, _))
    }

    #[inline]
    fn is_type(&self) -> bool {
        matches!(self.inner, ArenaValueInner::Type(_))
    }

    #[inline]
    fn is_conjunction(&self) -> bool {
        matches!(self.inner, ArenaValueInner::Conjunction(_))
    }

    #[inline]
    fn is_space(&self) -> bool {
        matches!(self.inner, ArenaValueInner::Space(_))
    }

    #[inline]
    fn is_state(&self) -> bool {
        matches!(self.inner, ArenaValueInner::State(_))
    }

    #[inline]
    fn is_unit(&self) -> bool {
        matches!(self.inner, ArenaValueInner::Unit)
    }

    #[inline]
    fn is_memo(&self) -> bool {
        matches!(self.inner, ArenaValueInner::Memo(_))
    }

    #[inline]
    fn is_empty(&self) -> bool {
        matches!(self.inner, ArenaValueInner::Empty)
    }

    #[inline]
    fn is_variable(&self) -> bool {
        matches!(self.inner, ArenaValueInner::Atom(s) if s.starts_with('$'))
    }

    #[inline]
    fn is_ground_type(&self) -> bool {
        // Unit/() is NOT a ground type — it's an expression in MeTTa HE
        matches!(
            self.inner,
            ArenaValueInner::Bool(_)
                | ArenaValueInner::Long(_)
                | ArenaValueInner::Float(_)
                | ArenaValueInner::String(_)
        )
    }

    #[inline]
    fn as_atom(&self) -> Option<&str> {
        match self.inner {
            ArenaValueInner::Atom(s) => Some(s),
            _ => None,
        }
    }

    #[inline]
    fn as_bool(&self) -> Option<bool> {
        match self.inner {
            ArenaValueInner::Bool(b) => Some(*b),
            _ => None,
        }
    }

    #[inline]
    fn as_long(&self) -> Option<i64> {
        match self.inner {
            ArenaValueInner::Long(n) => Some(*n),
            _ => None,
        }
    }

    #[inline]
    fn as_float(&self) -> Option<f64> {
        match self.inner {
            ArenaValueInner::Float(f) => Some(*f),
            _ => None,
        }
    }

    #[inline]
    fn as_string(&self) -> Option<&str> {
        match self.inner {
            ArenaValueInner::String(s) => Some(s),
            _ => None,
        }
    }

    #[inline]
    fn as_sexpr(&self) -> Option<&[Self]> {
        match self.inner {
            ArenaValueInner::SExpr(items) => Some(items.as_slice()),
            _ => None,
        }
    }

    #[inline]
    fn as_error(&self) -> Option<(&str, &Self)> {
        match self.inner {
            ArenaValueInner::Error(msg, details) => Some((msg, details)),
            _ => None,
        }
    }

    #[inline]
    fn as_type(&self) -> Option<&Self> {
        match self.inner {
            ArenaValueInner::Type(inner) => Some(inner),
            _ => None,
        }
    }

    #[inline]
    fn as_conjunction(&self) -> Option<&[Self]> {
        match self.inner {
            ArenaValueInner::Conjunction(goals) => Some(goals.as_slice()),
            _ => None,
        }
    }

    #[inline]
    fn as_space(&self) -> Option<&SpaceHandle> {
        match self.inner {
            ArenaValueInner::Space(handle) => Some(handle),
            _ => None,
        }
    }

    #[inline]
    fn as_state(&self) -> Option<u64> {
        match self.inner {
            ArenaValueInner::State(id) => Some(*id),
            _ => None,
        }
    }

    #[inline]
    fn as_memo(&self) -> Option<&MemoHandle> {
        match self.inner {
            ArenaValueInner::Memo(handle) => Some(handle),
            _ => None,
        }
    }

    fn type_name(&self) -> &'static str {
        match self.inner {
            ArenaValueInner::Atom(s) if s.starts_with('$') => "Variable",
            ArenaValueInner::Atom(_) => "Symbol",
            ArenaValueInner::Bool(_) => "Bool",
            ArenaValueInner::Long(_) => "Number",
            ArenaValueInner::Float(_) => "Number",
            ArenaValueInner::String(_) => "String",
            ArenaValueInner::SExpr(_) => "Expression",
            ArenaValueInner::Unit => "Unit",
            ArenaValueInner::Error(_, _) => "Error",
            ArenaValueInner::Type(_) => "Type",
            ArenaValueInner::Conjunction(_) => "Conjunction",
            ArenaValueInner::Space(_) => "Space",
            ArenaValueInner::State(_) => "State",
            ArenaValueInner::Memo(_) => "Memo",
            ArenaValueInner::Empty => "Empty",
        }
    }

    fn friendly_type_name(&self) -> &'static str {
        match self.inner {
            ArenaValueInner::Long(_) => "Number (integer)",
            ArenaValueInner::Float(_) => "Number (float)",
            ArenaValueInner::Bool(_) => "Bool",
            ArenaValueInner::String(_) => "String",
            ArenaValueInner::Atom(_) => "Atom",
            ArenaValueInner::Unit => "Unit",
            ArenaValueInner::SExpr(_) => "S-expression",
            ArenaValueInner::Error(_, _) => "Error",
            ArenaValueInner::Type(_) => "Type",
            ArenaValueInner::Conjunction(_) => "Conjunction",
            ArenaValueInner::Space(_) => "Space",
            ArenaValueInner::State(_) => "State",
            ArenaValueInner::Memo(_) => "Memo",
            ArenaValueInner::Empty => "Empty",
        }
    }

    fn get_head_symbol(&self) -> Option<&str> {
        // Helper to check if an atom is a space reference (not a variable)
        fn is_space_ref(s: &str) -> bool {
            s == "&" || s == "&self" || s == "&kb" || s == "&stack"
        }

        match self.inner {
            // For s-expressions like (double $x), extract "double"
            ArenaValueInner::SExpr(items) if !items.is_empty() => match items[0].inner {
                ArenaValueInner::Atom(head)
                    if !head.starts_with('$')
                        && (!head.starts_with('&') || is_space_ref(head))
                        && !head.starts_with('\'')
                        && *head != "_" =>
                {
                    Some(head)
                }
                _ => None,
            },
            // For bare atoms like foo, use the atom itself
            ArenaValueInner::Atom(head)
                if !head.starts_with('$')
                    && (!head.starts_with('&') || is_space_ref(head))
                    && !head.starts_with('\'')
                    && *head != "_" =>
            {
                Some(head)
            }
            _ => None,
        }
    }

    fn get_arity(&self) -> usize {
        match self.inner {
            ArenaValueInner::SExpr(items) if !items.is_empty() => items.len() - 1, // Exclude head
            _ => 0,
        }
    }

    fn serialize(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        serialize_arena_value(self, &mut buf);
        buf
    }

    #[inline]
    fn hash_value(&self) -> u64 {
        use std::hash::Hasher;
        use xxhash_rust::xxh3::Xxh3;

        // Golden ratio constant for good hash distribution
        const GOLDEN_RATIO: u64 = 0x9e3779b97f4a7c15;
        const LONG_SEED: u64 = 0x517cc1b727220a95;
        const BOOL_SEED: u64 = 0x2d358dccaa6c78a5;
        const FLOAT_SEED: u64 = 0x85ebca77c2b2ae63;
        const UNIT_HASH: u64 = 0x756e6974_68617368;

        // Fast path for primitives
        if self.is_unit() { return UNIT_HASH; }
        if let Some(b) = self.as_bool() {
            return if b { BOOL_SEED.wrapping_mul(GOLDEN_RATIO) } else { BOOL_SEED };
        }
        if let Some(n) = self.as_long() {
            let x = (n as u64).wrapping_add(LONG_SEED).wrapping_mul(GOLDEN_RATIO);
            return x ^ (x >> 32);
        }
        if let Some(f) = self.as_float() {
            let bits = f.to_bits();
            let x = bits.wrapping_add(FLOAT_SEED).wrapping_mul(GOLDEN_RATIO);
            return x ^ (x >> 32);
        }

        // Slow path - use xxHash3 for complex types
        let mut hasher = Xxh3::new();
        hash_arena_value_for_trait(self, &mut hasher);
        hasher.finish()
    }

    fn friendly_repr(&self) -> std::string::String {
        // Stack-based implementation to avoid recursion on deeply nested structures
        enum ReprWork<'a, 'bump> {
            Process(&'a ArenaValue<'bump>),
            Join {
                count: usize,
                prefix: &'static str,
                suffix: &'static str,
                separator: &'static str,
            },
        }

        let mut work_stack: Vec<ReprWork<'_, '_>> = Vec::with_capacity(16);
        let mut result_stack: Vec<std::string::String> = Vec::with_capacity(16);

        work_stack.push(ReprWork::Process(self));

        while let Some(work) = work_stack.pop() {
            match work {
                ReprWork::Process(val) => match &val.inner {
                    ArenaValueInner::Long(n) => result_stack.push(n.to_string()),
                    ArenaValueInner::Float(f) => result_stack.push(f.to_string()),
                    ArenaValueInner::Bool(b) => {
                        result_stack.push(if *b { "True" } else { "False" }.to_string());
                    }
                    ArenaValueInner::String(s) => result_stack.push(format!("\"{}\"", s)),
                    ArenaValueInner::Atom(a) => result_stack.push(a.to_string()),
                    ArenaValueInner::Unit => result_stack.push("()".to_string()),
                    ArenaValueInner::Empty => result_stack.push("Empty".to_string()),
                    ArenaValueInner::Space(handle) => {
                        result_stack.push(format!("(Space {} \"{}\")", handle.id, handle.name));
                    }
                    ArenaValueInner::State(id) => {
                        result_stack.push(format!("(State {})", id));
                    }
                    ArenaValueInner::Memo(handle) => {
                        result_stack.push(format!("(Memo {} \"{}\")", handle.id, handle.name));
                    }
                    ArenaValueInner::Error(msg, _) => {
                        result_stack.push(format!("(error \"{}\")", msg));
                    }
                    ArenaValueInner::Type(t) => {
                        work_stack.push(ReprWork::Join {
                            count: 1,
                            prefix: "(: ",
                            suffix: ")",
                            separator: "",
                        });
                        work_stack.push(ReprWork::Process(t));
                    }
                    ArenaValueInner::SExpr(items) => {
                        if items.is_empty() {
                            result_stack.push("()".to_string());
                        } else {
                            work_stack.push(ReprWork::Join {
                                count: items.len(),
                                prefix: "(",
                                suffix: ")",
                                separator: " ",
                            });
                            for item in items.iter().rev() {
                                work_stack.push(ReprWork::Process(item));
                            }
                        }
                    }
                    ArenaValueInner::Conjunction(goals) => {
                        if goals.is_empty() {
                            result_stack.push("(,)".to_string());
                        } else {
                            work_stack.push(ReprWork::Join {
                                count: goals.len(),
                                prefix: "(, ",
                                suffix: ")",
                                separator: " ",
                            });
                            for goal in goals.iter().rev() {
                                work_stack.push(ReprWork::Process(goal));
                            }
                        }
                    }
                },
                ReprWork::Join {
                    count,
                    prefix,
                    suffix,
                    separator,
                } => {
                    let start = result_stack.len() - count;
                    let parts: Vec<std::string::String> = result_stack.drain(start..).collect();
                    result_stack.push(format!("{}{}{}", prefix, parts.join(separator), suffix));
                }
            }
        }

        result_stack.pop().unwrap_or_default()
    }

    fn to_display_string(&self) -> std::string::String {
        // Stack-based implementation to avoid recursion on deeply nested structures
        // Similar to friendly_repr but strings are printed WITHOUT quotes
        enum ReprWork<'a, 'bump> {
            Process(&'a ArenaValue<'bump>),
            Join {
                count: usize,
                prefix: &'static str,
                suffix: &'static str,
                separator: &'static str,
            },
        }

        let mut work_stack: Vec<ReprWork<'_, '_>> = Vec::with_capacity(16);
        let mut result_stack: Vec<std::string::String> = Vec::with_capacity(16);

        work_stack.push(ReprWork::Process(self));

        while let Some(work) = work_stack.pop() {
            match work {
                ReprWork::Process(val) => match &val.inner {
                    ArenaValueInner::Long(n) => result_stack.push(n.to_string()),
                    ArenaValueInner::Float(f) => result_stack.push(f.to_string()),
                    ArenaValueInner::Bool(b) => {
                        result_stack.push(if *b { "True" } else { "False" }.to_string());
                    }
                    // Key difference: strings printed without quotes for display
                    ArenaValueInner::String(s) => result_stack.push(s.to_string()),
                    ArenaValueInner::Atom(a) => result_stack.push(a.to_string()),
                    ArenaValueInner::Unit => result_stack.push("()".to_string()),
                    ArenaValueInner::Empty => result_stack.push("Empty".to_string()),
                    ArenaValueInner::Space(handle) => {
                        result_stack.push(format!("(Space {} \"{}\")", handle.id, handle.name));
                    }
                    ArenaValueInner::State(id) => {
                        result_stack.push(format!("(State {})", id));
                    }
                    ArenaValueInner::Memo(handle) => {
                        result_stack.push(format!("(Memo {} \"{}\")", handle.id, handle.name));
                    }
                    ArenaValueInner::Error(msg, _) => {
                        result_stack.push(format!("(Error \"{}\")", msg));
                    }
                    ArenaValueInner::Type(t) => {
                        work_stack.push(ReprWork::Join {
                            count: 1,
                            prefix: "(: ",
                            suffix: ")",
                            separator: "",
                        });
                        work_stack.push(ReprWork::Process(t));
                    }
                    ArenaValueInner::SExpr(items) => {
                        if items.is_empty() {
                            result_stack.push("()".to_string());
                        } else {
                            work_stack.push(ReprWork::Join {
                                count: items.len(),
                                prefix: "(",
                                suffix: ")",
                                separator: " ",
                            });
                            for item in items.iter().rev() {
                                work_stack.push(ReprWork::Process(item));
                            }
                        }
                    }
                    ArenaValueInner::Conjunction(goals) => {
                        if goals.is_empty() {
                            result_stack.push("(,)".to_string());
                        } else {
                            work_stack.push(ReprWork::Join {
                                count: goals.len(),
                                prefix: "(, ",
                                suffix: ")",
                                separator: " ",
                            });
                            for goal in goals.iter().rev() {
                                work_stack.push(ReprWork::Process(goal));
                            }
                        }
                    }
                },
                ReprWork::Join {
                    count,
                    prefix,
                    suffix,
                    separator,
                } => {
                    let start = result_stack.len() - count;
                    let parts: Vec<std::string::String> = result_stack.drain(start..).collect();
                    result_stack.push(format!("{}{}{}", prefix, parts.join(separator), suffix));
                }
            }
        }

        result_stack.pop().unwrap_or_default()
    }
}

// ============================================================================
// Serialization helpers for ArenaValue
// ============================================================================

/// Tag bytes for serialization format (same as MettaValue)
mod serialize_tags {
    pub const ATOM: u8 = 0x01;
    pub const BOOL: u8 = 0x02;
    pub const LONG: u8 = 0x03;
    pub const FLOAT: u8 = 0x04;
    pub const STRING: u8 = 0x05;
    pub const SEXPR: u8 = 0x06;
    pub const UNIT_LEGACY: u8 = 0x07;
    pub const ERROR: u8 = 0x08;
    pub const TYPE: u8 = 0x09;
    pub const CONJUNCTION: u8 = 0x0A;
    pub const UNIT: u8 = 0x0B;
    pub const EMPTY: u8 = 0x0C;
    pub const SPACE: u8 = 0x0D;
    pub const STATE: u8 = 0x0E;
    pub const MEMO: u8 = 0x0F;
}

/// Write a varint to buffer
fn write_varint(buf: &mut Vec<u8>, mut n: usize) {
    loop {
        let byte = (n & 0x7F) as u8;
        n >>= 7;
        if n == 0 {
            buf.push(byte);
            break;
        } else {
            buf.push(byte | 0x80);
        }
    }
}

/// Read a varint from bytes
fn read_varint(bytes: &[u8]) -> Result<(usize, usize), std::string::String> {
    let mut result: usize = 0;
    let mut shift = 0;
    for (i, &byte) in bytes.iter().enumerate() {
        result |= ((byte & 0x7F) as usize) << shift;
        if byte & 0x80 == 0 {
            return Ok((result, i + 1));
        }
        shift += 7;
        if shift > 63 {
            return Err("varint overflow".to_string());
        }
    }
    Err("unexpected end of varint".to_string())
}

/// Recursively hash an ArenaValue using trait-based accessors.
///
/// Used by `ArenaValue::hash_value()` for complex types (strings, atoms, s-expressions).
fn hash_arena_value_for_trait<H: std::hash::Hasher>(value: &ArenaValue, hasher: &mut H) {
    use std::hash::Hash;

    // Hash type discriminant
    let type_tag: u8 = if value.is_unit() { 0 }
        else if value.is_bool() { 2 }
        else if value.is_long() { 3 }
        else if value.is_float() { 4 }
        else if value.is_string() { 5 }
        else if value.is_atom() { 6 }
        else if value.is_sexpr() { 7 }
        else if value.is_error() { 8 }
        else if value.is_empty() { 9 }
        else { 10 };
    type_tag.hash(hasher);

    // Hash content
    if let Some(b) = value.as_bool() {
        b.hash(hasher);
    } else if let Some(n) = value.as_long() {
        n.hash(hasher);
    } else if let Some(f) = value.as_float() {
        f.to_bits().hash(hasher);
    } else if let Some(s) = value.as_string() {
        s.hash(hasher);
    } else if let Some(s) = value.as_atom() {
        s.hash(hasher);
    } else if let Some(items) = value.as_sexpr() {
        items.len().hash(hasher);
        for item in items {
            hash_arena_value_for_trait(item, hasher);
        }
    }
}

/// Serialize an ArenaValue to bytes
fn serialize_arena_value(value: &ArenaValue, buf: &mut Vec<u8>) {
    use serialize_tags::*;
    match value.inner {
        ArenaValueInner::Atom(s) => {
            buf.push(ATOM);
            write_varint(buf, s.len());
            buf.extend_from_slice(s.as_bytes());
        }
        ArenaValueInner::Bool(b) => {
            buf.push(BOOL);
            buf.push(if *b { 1 } else { 0 });
        }
        ArenaValueInner::Long(n) => {
            buf.push(LONG);
            buf.extend_from_slice(&n.to_le_bytes());
        }
        ArenaValueInner::Float(f) => {
            buf.push(FLOAT);
            buf.extend_from_slice(&f.to_le_bytes());
        }
        ArenaValueInner::String(s) => {
            buf.push(STRING);
            write_varint(buf, s.len());
            buf.extend_from_slice(s.as_bytes());
        }
        ArenaValueInner::SExpr(items) => {
            buf.push(SEXPR);
            write_varint(buf, items.len());
            for item in items.iter() {
                serialize_arena_value(item, buf);
            }
        }
        ArenaValueInner::Unit => {
            buf.push(UNIT_LEGACY);
        }
        ArenaValueInner::Error(msg, details) => {
            buf.push(ERROR);
            write_varint(buf, msg.len());
            buf.extend_from_slice(msg.as_bytes());
            serialize_arena_value(details, buf);
        }
        ArenaValueInner::Type(inner) => {
            buf.push(TYPE);
            serialize_arena_value(inner, buf);
        }
        ArenaValueInner::Conjunction(goals) => {
            buf.push(CONJUNCTION);
            write_varint(buf, goals.len());
            for goal in goals.iter() {
                serialize_arena_value(goal, buf);
            }
        }
        ArenaValueInner::Empty => {
            buf.push(EMPTY);
        }
        ArenaValueInner::Space(handle) => {
            buf.push(SPACE);
            buf.extend_from_slice(&handle.id.to_le_bytes());
            // Serialize name length and name bytes
            let name_bytes = handle.name.as_bytes();
            write_varint(buf, name_bytes.len());
            buf.extend_from_slice(name_bytes);
            // Serialize is_module_space flag
            buf.push(if handle.is_module_space() { 1 } else { 0 });
        }
        ArenaValueInner::State(id) => {
            buf.push(STATE);
            buf.extend_from_slice(&id.to_le_bytes());
        }
        ArenaValueInner::Memo(handle) => {
            buf.push(MEMO);
            buf.extend_from_slice(&handle.id.to_le_bytes());
        }
    }
}

// ============================================================================
// ArenaValueFactory - Factory for arena-allocated values
// ============================================================================

/// Factory for creating arena-allocated ArenaValue instances.
///
/// This holds a reference to the arena and uses it for all allocations.
/// The factory has a single pointer-sized field (8 bytes on 64-bit systems).
#[derive(Debug, Clone, Copy)]
pub struct ArenaValueFactory<'a> {
    arena: &'a Bump,
}

// SAFETY: ArenaValueFactory<'static> can be sent between threads because:
// - It's a reference to a 'static arena
// - Allocation is thread-safe for bumpalo arenas in single-threaded per-arena use
// - The factory is only used for value construction, not mutation
unsafe impl Send for ArenaValueFactory<'static> {}

// SAFETY: ArenaValueFactory<'static> can be shared between threads because:
// - It provides the same functionality whether accessed from one thread or many
// - The underlying arena is thread-local (not shared between threads)
unsafe impl Sync for ArenaValueFactory<'static> {}

impl<'a> ArenaValueFactory<'a> {
    /// Create a new factory for the given arena
    #[inline]
    pub fn new(arena: &'a Bump) -> Self {
        Self { arena }
    }

    /// Get the underlying arena
    #[inline]
    pub fn arena(&self) -> &'a Bump {
        self.arena
    }
}

impl<'a> MettaValueFactory<ArenaValue<'a>> for ArenaValueFactory<'a> {
    #[inline]
    fn atom(&self, s: &str) -> ArenaValue<'a> {
        ArenaValue::atom(self.arena, s)
    }

    #[inline]
    fn bool(&self, b: bool) -> ArenaValue<'a> {
        ArenaValue::bool(self.arena, b)
    }

    #[inline]
    fn long(&self, n: i64) -> ArenaValue<'a> {
        ArenaValue::long(self.arena, n)
    }

    #[inline]
    fn float(&self, f: f64) -> ArenaValue<'a> {
        ArenaValue::float(self.arena, f)
    }

    #[inline]
    fn string(&self, s: &str) -> ArenaValue<'a> {
        ArenaValue::string(self.arena, s)
    }

    #[inline]
    fn sexpr(&self, items: Vec<ArenaValue<'a>>) -> ArenaValue<'a> {
        ArenaValue::sexpr(self.arena, items)
    }

    #[inline]
    fn sexpr_from_slice(&self, items: &[ArenaValue<'a>]) -> ArenaValue<'a> {
        ArenaValue::sexpr(self.arena, items.iter().copied())
    }

    #[inline]
    fn error(&self, msg: &str, details: ArenaValue<'a>) -> ArenaValue<'a> {
        ArenaValue::error(self.arena, msg, details)
    }

    #[inline]
    fn type_value(&self, inner: ArenaValue<'a>) -> ArenaValue<'a> {
        ArenaValue::r#type(self.arena, inner)
    }

    #[inline]
    fn conjunction(&self, goals: Vec<ArenaValue<'a>>) -> ArenaValue<'a> {
        ArenaValue::conjunction(self.arena, goals)
    }

    #[inline]
    fn space(&self, handle: SpaceHandle) -> ArenaValue<'a> {
        ArenaValue::space(self.arena, handle)
    }

    #[inline]
    fn state(&self, id: u64) -> ArenaValue<'a> {
        ArenaValue::state(self.arena, id)
    }

    #[inline]
    fn unit(&self) -> ArenaValue<'a> {
        ArenaValue::unit(self.arena)
    }

    #[inline]
    fn memo(&self, handle: MemoHandle) -> ArenaValue<'a> {
        ArenaValue::memo(self.arena, handle)
    }

    #[inline]
    fn empty(&self) -> ArenaValue<'a> {
        ArenaValue::empty(self.arena)
    }

    fn deserialize(&self, bytes: &[u8]) -> Result<(ArenaValue<'a>, usize), std::string::String> {
        deserialize_arena_value(self.arena, bytes)
    }
}

/// Deserialize an ArenaValue from bytes, allocating directly in the arena
fn deserialize_arena_value<'a>(
    arena: &'a Bump,
    bytes: &[u8],
) -> Result<(ArenaValue<'a>, usize), std::string::String> {
    use serialize_tags::*;

    if bytes.is_empty() {
        return Err("unexpected end of input".to_string());
    }

    let tag = bytes[0];
    let rest = &bytes[1..];

    match tag {
        ATOM => {
            let (len, varint_size) = read_varint(rest)?;
            let start = varint_size;
            let end = start + len;
            if rest.len() < end {
                return Err("unexpected end of atom data".to_string());
            }
            let s = std::str::from_utf8(&rest[start..end])
                .map_err(|e| format!("invalid UTF-8 in atom: {}", e))?;
            Ok((ArenaValue::atom(arena, s), 1 + end))
        }
        BOOL => {
            if rest.is_empty() {
                return Err("unexpected end of bool data".to_string());
            }
            Ok((ArenaValue::bool(arena, rest[0] != 0), 2))
        }
        LONG => {
            if rest.len() < 8 {
                return Err("unexpected end of long data".to_string());
            }
            let n = i64::from_le_bytes(rest[..8].try_into().unwrap());
            Ok((ArenaValue::long(arena, n), 9))
        }
        FLOAT => {
            if rest.len() < 8 {
                return Err("unexpected end of float data".to_string());
            }
            let f = f64::from_le_bytes(rest[..8].try_into().unwrap());
            Ok((ArenaValue::float(arena, f), 9))
        }
        STRING => {
            let (len, varint_size) = read_varint(rest)?;
            let start = varint_size;
            let end = start + len;
            if rest.len() < end {
                return Err("unexpected end of string data".to_string());
            }
            let s = std::str::from_utf8(&rest[start..end])
                .map_err(|e| format!("invalid UTF-8 in string: {}", e))?;
            Ok((ArenaValue::string(arena, s), 1 + end))
        }
        SEXPR => {
            let (count, varint_size) = read_varint(rest)?;
            let mut items = Vec::with_capacity(count);
            let mut offset = 1 + varint_size;
            for _ in 0..count {
                let (item, consumed) = deserialize_arena_value(arena, &bytes[offset..])?;
                items.push(item);
                offset += consumed;
            }
            Ok((ArenaValue::sexpr(arena, items), offset))
        }
        UNIT_LEGACY => Ok((ArenaValue::unit(arena), 1)),
        ERROR => {
            let (msg_len, varint_size) = read_varint(rest)?;
            let msg_start = varint_size;
            let msg_end = msg_start + msg_len;
            if rest.len() < msg_end {
                return Err("unexpected end of error message".to_string());
            }
            let msg = std::str::from_utf8(&rest[msg_start..msg_end])
                .map_err(|e| format!("invalid UTF-8 in error message: {}", e))?;
            let (details, details_consumed) = deserialize_arena_value(arena, &bytes[1 + msg_end..])?;
            Ok((ArenaValue::error(arena, msg, details), 1 + msg_end + details_consumed))
        }
        TYPE => {
            let (inner, consumed) = deserialize_arena_value(arena, rest)?;
            Ok((ArenaValue::r#type(arena, inner), 1 + consumed))
        }
        CONJUNCTION => {
            let (count, varint_size) = read_varint(rest)?;
            let mut goals = Vec::with_capacity(count);
            let mut offset = 1 + varint_size;
            for _ in 0..count {
                let (goal, consumed) = deserialize_arena_value(arena, &bytes[offset..])?;
                goals.push(goal);
                offset += consumed;
            }
            Ok((ArenaValue::conjunction(arena, goals), offset))
        }
        UNIT => Ok((ArenaValue::unit(arena), 1)),
        EMPTY => Ok((ArenaValue::empty(arena), 1)),
        SPACE => {
            if rest.len() < 8 {
                return Err("unexpected end of space id".to_string());
            }
            let id = u64::from_le_bytes(rest[..8].try_into().unwrap());
            let mut offset = 9; // 1 (tag) + 8 (id)

            // Read name length and name bytes
            let (name_len, consumed) = read_varint(&rest[8..])?;
            offset += consumed;
            let name_start = 8 + consumed;
            if rest.len() < name_start + name_len {
                return Err("unexpected end of space name".to_string());
            }
            let name = std::str::from_utf8(&rest[name_start..name_start + name_len])
                .map_err(|e| format!("invalid UTF-8 in space name: {}", e))?
                .to_string();
            offset += name_len;

            // Read is_module_space flag
            if rest.len() < name_start + name_len + 1 {
                return Err("unexpected end of space is_module_space flag".to_string());
            }
            let is_module = rest[name_start + name_len] != 0;
            offset += 1;

            // Reconstruct SpaceHandle with available info
            let handle = SpaceHandle::new_from_serialized(id, name, is_module);
            Ok((ArenaValue::space(arena, handle), offset))
        }
        STATE => {
            if rest.len() < 8 {
                return Err("unexpected end of state id".to_string());
            }
            let id = u64::from_le_bytes(rest[..8].try_into().unwrap());
            Ok((ArenaValue::state(arena, id), 9))
        }
        MEMO => {
            if rest.len() < 8 {
                return Err("unexpected end of memo id".to_string());
            }
            let _id = u64::from_le_bytes(rest[..8].try_into().unwrap());
            // Note: We can only deserialize the ID, not the full MemoHandle
            Ok((ArenaValue::unit(arena), 9)) // Placeholder - real impl needs handle registry
        }
        _ => Err(format!("unknown tag byte: 0x{:02X}", tag)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ========================================================================
    // Basic Constructor and Accessor Tests
    // ========================================================================

    #[test]
    fn test_arena_atom() {
        let arena = Bump::new();
        let v = ArenaValue::atom(&arena, "hello");
        assert!(v.is_atom());
        assert_eq!(v.as_atom(), Some("hello"));
    }

    #[test]
    fn test_arena_long() {
        let arena = Bump::new();
        let v = ArenaValue::long(&arena, 42);
        assert!(v.is_long());
        assert_eq!(v.as_long(), Some(42));
    }

    #[test]
    fn test_arena_bool() {
        let arena = Bump::new();
        let v = ArenaValue::bool(&arena, true);
        assert!(v.is_bool());
        assert_eq!(v.as_bool(), Some(true));
    }

    #[test]
    fn test_arena_sexpr() {
        let arena = Bump::new();
        let items = vec![
            ArenaValue::atom(&arena, "+"),
            ArenaValue::long(&arena, 1),
            ArenaValue::long(&arena, 2),
        ];
        let v = ArenaValue::sexpr(&arena, items);
        assert!(v.is_sexpr());
        let items = v.as_sexpr().expect("should be sexpr");
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn test_copy_semantics() {
        let arena = Bump::new();
        let v1 = ArenaValue::long(&arena, 42);
        let v2 = v1; // Copy, not move
        let v3 = v1; // Can copy again
        assert_eq!(v1.as_long(), Some(42));
        assert_eq!(v2.as_long(), Some(42));
        assert_eq!(v3.as_long(), Some(42));
    }

    #[test]
    fn test_display() {
        let arena = Bump::new();
        let v = ArenaValue::sexpr(
            &arena,
            vec![
                ArenaValue::atom(&arena, "+"),
                ArenaValue::long(&arena, 1),
                ArenaValue::long(&arena, 2),
            ],
        );
        assert_eq!(format!("{}", v), "(+ 1 2)");
    }

    // ========================================================================
    // All Type Variant Constructor Tests (Phase 1)
    // ========================================================================

    #[test]
    fn test_arena_float() {
        let arena = Bump::new();
        let v = ArenaValue::float(&arena, 3.14);
        assert!(v.is_float());
        assert!(!v.is_long());
        assert_eq!(v.as_float(), Some(3.14));
        assert_eq!(v.as_long(), None);
    }

    #[test]
    fn test_arena_string() {
        let arena = Bump::new();
        let v = ArenaValue::string(&arena, "hello world");
        assert!(v.is_string());
        assert!(!v.is_atom());
        assert_eq!(v.as_string(), Some("hello world"));
        assert_eq!(v.as_atom(), None);
    }

    #[test]
    fn test_arena_nil() {
        let arena = Bump::new();
        let v = ArenaValue::unit(&arena);
        // After Nil/Unit merge, nil() returns Unit
        assert!(v.is_unit());
        assert!(!v.is_empty());
    }

    #[test]
    fn test_arena_unit() {
        let arena = Bump::new();
        let v = ArenaValue::unit(&arena);
        assert!(v.is_unit());
        assert!(!v.is_empty());
    }

    #[test]
    fn test_arena_empty() {
        let arena = Bump::new();
        let v = ArenaValue::empty(&arena);
        assert!(v.is_empty());
        assert!(!v.is_unit());
    }

    #[test]
    fn test_arena_error() {
        let arena = Bump::new();
        let details = ArenaValue::atom(&arena, "details");
        let v = ArenaValue::error(&arena, "test error", details);
        assert!(v.is_error());
        let (msg, det) = v.as_error().expect("should be error");
        assert_eq!(msg, "test error");
        assert_eq!(det.as_atom(), Some("details"));
    }

    #[test]
    fn test_arena_type() {
        let arena = Bump::new();
        let inner = ArenaValue::atom(&arena, "Number");
        let v = ArenaValue::r#type(&arena, inner);
        assert!(v.is_type());
        let t = v.as_type().expect("should be type");
        assert_eq!(t.as_atom(), Some("Number"));
    }

    #[test]
    fn test_arena_conjunction() {
        let arena = Bump::new();
        let goals = vec![
            ArenaValue::atom(&arena, "goal1"),
            ArenaValue::atom(&arena, "goal2"),
        ];
        let v = ArenaValue::conjunction(&arena, goals);
        assert!(v.is_conjunction());
        let conj = v.as_conjunction().expect("should be conjunction");
        assert_eq!(conj.len(), 2);
    }

    #[test]
    fn test_arena_state() {
        let arena = Bump::new();
        let v = ArenaValue::state(&arena, 12345);
        assert!(v.is_state());
        assert_eq!(v.as_state(), Some(12345));
    }

    #[test]
    fn test_arena_sexpr_empty() {
        let arena = Bump::new();
        let v = ArenaValue::sexpr_empty(&arena);
        assert!(v.is_sexpr());
        let items = v.as_sexpr().expect("should be sexpr");
        assert!(items.is_empty());
    }

    // ========================================================================
    // Type Check Method Coverage
    // ========================================================================

    #[test]
    fn test_is_variable_true() {
        let arena = Bump::new();
        let v = ArenaValue::atom(&arena, "$x");
        assert!(v.is_variable());
    }

    #[test]
    fn test_is_variable_false() {
        let arena = Bump::new();
        let v = ArenaValue::atom(&arena, "foo");
        assert!(!v.is_variable());
    }

    #[test]
    fn test_is_ground_type() {
        let arena = Bump::new();

        // Ground types
        assert!(ArenaValue::bool(&arena, true).is_ground_type());
        assert!(ArenaValue::long(&arena, 42).is_ground_type());
        assert!(ArenaValue::float(&arena, 3.14).is_ground_type());
        assert!(ArenaValue::string(&arena, "hello").is_ground_type());

        // Non-ground types
        assert!(!ArenaValue::atom(&arena, "foo").is_ground_type());
        assert!(!ArenaValue::sexpr_empty(&arena).is_ground_type());
        // After Nil/Unit merge, Unit/() is NOT a ground type (it's an expression in MeTTa HE)
        assert!(!ArenaValue::unit(&arena).is_ground_type());
        assert!(!ArenaValue::unit(&arena).is_ground_type());
    }

    // ========================================================================
    // PartialEq Tests for All Variant Combinations
    // ========================================================================

    #[test]
    fn test_eq_long_long() {
        let arena = Bump::new();
        let v1 = ArenaValue::long(&arena, 42);
        let v2 = ArenaValue::long(&arena, 42);
        let v3 = ArenaValue::long(&arena, 99);
        assert_eq!(v1, v2);
        assert_ne!(v1, v3);
    }

    #[test]
    fn test_eq_float_float() {
        let arena = Bump::new();
        let v1 = ArenaValue::float(&arena, 3.14);
        let v2 = ArenaValue::float(&arena, 3.14);
        let v3 = ArenaValue::float(&arena, 2.71);
        assert_eq!(v1, v2);
        assert_ne!(v1, v3);
    }

    #[test]
    fn test_eq_bool_bool() {
        let arena = Bump::new();
        let t1 = ArenaValue::bool(&arena, true);
        let t2 = ArenaValue::bool(&arena, true);
        let f = ArenaValue::bool(&arena, false);
        assert_eq!(t1, t2);
        assert_ne!(t1, f);
    }

    #[test]
    fn test_eq_atom_atom() {
        let arena = Bump::new();
        let v1 = ArenaValue::atom(&arena, "foo");
        let v2 = ArenaValue::atom(&arena, "foo");
        let v3 = ArenaValue::atom(&arena, "bar");
        assert_eq!(v1, v2);
        assert_ne!(v1, v3);
    }

    #[test]
    fn test_eq_string_string() {
        let arena = Bump::new();
        let v1 = ArenaValue::string(&arena, "hello");
        let v2 = ArenaValue::string(&arena, "hello");
        let v3 = ArenaValue::string(&arena, "world");
        assert_eq!(v1, v2);
        assert_ne!(v1, v3);
    }

    #[test]
    fn test_eq_nil_nil() {
        let arena = Bump::new();
        let v1 = ArenaValue::unit(&arena);
        let v2 = ArenaValue::unit(&arena);
        assert_eq!(v1, v2);
    }

    #[test]
    fn test_eq_unit_unit() {
        let arena = Bump::new();
        let v1 = ArenaValue::unit(&arena);
        let v2 = ArenaValue::unit(&arena);
        assert_eq!(v1, v2);
    }

    #[test]
    fn test_eq_empty_empty() {
        let arena = Bump::new();
        let v1 = ArenaValue::empty(&arena);
        let v2 = ArenaValue::empty(&arena);
        assert_eq!(v1, v2);
    }

    #[test]
    fn test_eq_sexpr_sexpr() {
        let arena = Bump::new();
        let v1 = ArenaValue::sexpr(&arena, vec![
            ArenaValue::atom(&arena, "+"),
            ArenaValue::long(&arena, 1),
        ]);
        let v2 = ArenaValue::sexpr(&arena, vec![
            ArenaValue::atom(&arena, "+"),
            ArenaValue::long(&arena, 1),
        ]);
        let v3 = ArenaValue::sexpr(&arena, vec![
            ArenaValue::atom(&arena, "+"),
            ArenaValue::long(&arena, 2),
        ]);
        assert_eq!(v1, v2);
        assert_ne!(v1, v3);
    }

    #[test]
    fn test_eq_error_error() {
        let arena = Bump::new();
        let d1 = ArenaValue::atom(&arena, "d");
        let d2 = ArenaValue::atom(&arena, "d");
        let v1 = ArenaValue::error(&arena, "err", d1);
        let v2 = ArenaValue::error(&arena, "err", d2);
        assert_eq!(v1, v2);
    }

    #[test]
    fn test_eq_type_type() {
        let arena = Bump::new();
        let i1 = ArenaValue::atom(&arena, "Number");
        let i2 = ArenaValue::atom(&arena, "Number");
        let v1 = ArenaValue::r#type(&arena, i1);
        let v2 = ArenaValue::r#type(&arena, i2);
        assert_eq!(v1, v2);
    }

    #[test]
    fn test_eq_conjunction_conjunction() {
        let arena = Bump::new();
        let v1 = ArenaValue::conjunction(&arena, vec![
            ArenaValue::atom(&arena, "a"),
        ]);
        let v2 = ArenaValue::conjunction(&arena, vec![
            ArenaValue::atom(&arena, "a"),
        ]);
        assert_eq!(v1, v2);
    }

    #[test]
    fn test_eq_state_state() {
        let arena = Bump::new();
        let v1 = ArenaValue::state(&arena, 100);
        let v2 = ArenaValue::state(&arena, 100);
        let v3 = ArenaValue::state(&arena, 200);
        assert_eq!(v1, v2);
        assert_ne!(v1, v3);
    }

    #[test]
    fn test_ne_different_types() {
        let arena = Bump::new();
        let long = ArenaValue::long(&arena, 42);
        let float = ArenaValue::float(&arena, 42.0);
        let bool_val = ArenaValue::bool(&arena, true);
        let atom = ArenaValue::atom(&arena, "42");
        let string = ArenaValue::string(&arena, "42");

        // Different types should never be equal
        assert_ne!(long, float);
        assert_ne!(long, bool_val);
        assert_ne!(long, atom);
        assert_ne!(float, string);
        assert_ne!(atom, string);
    }

    // ========================================================================
    // Accessor Method Edge Cases
    // ========================================================================

    #[test]
    fn test_accessor_wrong_type_returns_none() {
        let arena = Bump::new();
        let v = ArenaValue::long(&arena, 42);
        assert_eq!(v.as_atom(), None);
        assert_eq!(v.as_bool(), None);
        assert_eq!(v.as_float(), None);
        assert_eq!(v.as_string(), None);
        assert_eq!(v.as_sexpr(), None);
        assert_eq!(v.as_error(), None);
        assert_eq!(v.as_type(), None);
        assert_eq!(v.as_conjunction(), None);
        assert_eq!(v.as_space(), None);
        assert_eq!(v.as_state(), None);
        assert_eq!(v.as_memo(), None);
    }

    // ========================================================================
    // Type Name Tests
    // ========================================================================

    #[test]
    fn test_type_name_variable() {
        let arena = Bump::new();
        let v = ArenaValue::atom(&arena, "$x");
        assert_eq!(v.type_name(), "Variable");
    }

    #[test]
    fn test_type_name_symbol() {
        let arena = Bump::new();
        let v = ArenaValue::atom(&arena, "foo");
        assert_eq!(v.type_name(), "Symbol");
    }

    #[test]
    fn test_type_name_bool() {
        let arena = Bump::new();
        let v = ArenaValue::bool(&arena, true);
        assert_eq!(v.type_name(), "Bool");
    }

    #[test]
    fn test_type_name_long() {
        let arena = Bump::new();
        let v = ArenaValue::long(&arena, 42);
        assert_eq!(v.type_name(), "Number");
    }

    #[test]
    fn test_type_name_float() {
        let arena = Bump::new();
        let v = ArenaValue::float(&arena, 3.14);
        assert_eq!(v.type_name(), "Number");
    }

    #[test]
    fn test_type_name_string() {
        let arena = Bump::new();
        let v = ArenaValue::string(&arena, "hello");
        assert_eq!(v.type_name(), "String");
    }

    #[test]
    fn test_type_name_sexpr() {
        let arena = Bump::new();
        let v = ArenaValue::sexpr_empty(&arena);
        assert_eq!(v.type_name(), "Expression");
    }

    #[test]
    fn test_type_name_nil() {
        // After Nil/Unit merge, nil() returns Unit which has type_name "Unit"
        let arena = Bump::new();
        let v = ArenaValue::unit(&arena);
        assert_eq!(v.type_name(), "Unit");
    }

    #[test]
    fn test_type_name_error() {
        let arena = Bump::new();
        let d = ArenaValue::unit(&arena);
        let v = ArenaValue::error(&arena, "err", d);
        assert_eq!(v.type_name(), "Error");
    }

    #[test]
    fn test_type_name_type() {
        let arena = Bump::new();
        let i = ArenaValue::atom(&arena, "Int");
        let v = ArenaValue::r#type(&arena, i);
        assert_eq!(v.type_name(), "Type");
    }

    #[test]
    fn test_type_name_conjunction() {
        let arena = Bump::new();
        let v = ArenaValue::conjunction(&arena, vec![]);
        assert_eq!(v.type_name(), "Conjunction");
    }

    #[test]
    fn test_type_name_state() {
        let arena = Bump::new();
        let v = ArenaValue::state(&arena, 1);
        assert_eq!(v.type_name(), "State");
    }

    #[test]
    fn test_type_name_unit() {
        let arena = Bump::new();
        let v = ArenaValue::unit(&arena);
        assert_eq!(v.type_name(), "Unit");
    }

    #[test]
    fn test_type_name_empty() {
        let arena = Bump::new();
        let v = ArenaValue::empty(&arena);
        assert_eq!(v.type_name(), "Empty");
    }

    // ========================================================================
    // Display Formatting Tests
    // ========================================================================

    #[test]
    fn test_display_bool_true() {
        let arena = Bump::new();
        let v = ArenaValue::bool(&arena, true);
        assert_eq!(format!("{}", v), "True");
    }

    #[test]
    fn test_display_bool_false() {
        let arena = Bump::new();
        let v = ArenaValue::bool(&arena, false);
        assert_eq!(format!("{}", v), "False");
    }

    #[test]
    fn test_display_long() {
        let arena = Bump::new();
        let v = ArenaValue::long(&arena, -42);
        assert_eq!(format!("{}", v), "-42");
    }

    #[test]
    fn test_display_float() {
        let arena = Bump::new();
        let v = ArenaValue::float(&arena, 3.14);
        assert_eq!(format!("{}", v), "3.14");
    }

    #[test]
    fn test_display_string() {
        let arena = Bump::new();
        let v = ArenaValue::string(&arena, "hello");
        assert_eq!(format!("{}", v), "\"hello\"");
    }

    #[test]
    fn test_display_nil() {
        // After Nil/Unit merge, nil() returns Unit which displays as "()"
        let arena = Bump::new();
        let v = ArenaValue::unit(&arena);
        assert_eq!(format!("{}", v), "()");
    }

    #[test]
    fn test_display_unit() {
        let arena = Bump::new();
        let v = ArenaValue::unit(&arena);
        assert_eq!(format!("{}", v), "()");
    }

    #[test]
    fn test_display_empty() {
        let arena = Bump::new();
        let v = ArenaValue::empty(&arena);
        assert_eq!(format!("{}", v), "Empty");
    }

    #[test]
    fn test_display_empty_sexpr() {
        let arena = Bump::new();
        let v = ArenaValue::sexpr_empty(&arena);
        assert_eq!(format!("{}", v), "()");
    }

    #[test]
    fn test_display_error() {
        let arena = Bump::new();
        let d = ArenaValue::atom(&arena, "details");
        let v = ArenaValue::error(&arena, "msg", d);
        assert_eq!(format!("{}", v), "(Error msg details)");
    }

    #[test]
    fn test_display_type() {
        let arena = Bump::new();
        let i = ArenaValue::atom(&arena, "Int");
        let v = ArenaValue::r#type(&arena, i);
        assert_eq!(format!("{}", v), "(: Int)");
    }

    #[test]
    fn test_display_conjunction() {
        let arena = Bump::new();
        let v = ArenaValue::conjunction(&arena, vec![
            ArenaValue::atom(&arena, "a"),
            ArenaValue::atom(&arena, "b"),
        ]);
        assert_eq!(format!("{}", v), "(, a b)");
    }

    #[test]
    fn test_display_state() {
        let arena = Bump::new();
        let v = ArenaValue::state(&arena, 123);
        assert_eq!(format!("{}", v), "<State:123>");
    }

    // ========================================================================
    // Serialization Round-Trip Tests
    // ========================================================================

    #[test]
    fn test_serialize_roundtrip_long() {
        let arena = Bump::new();
        let original = ArenaValue::long(&arena, 12345);
        let bytes = original.serialize();
        let (decoded, _) = deserialize_arena_value(&arena, &bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_float() {
        let arena = Bump::new();
        let original = ArenaValue::float(&arena, 3.14159);
        let bytes = original.serialize();
        let (decoded, _) = deserialize_arena_value(&arena, &bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_bool() {
        let arena = Bump::new();
        let original = ArenaValue::bool(&arena, true);
        let bytes = original.serialize();
        let (decoded, _) = deserialize_arena_value(&arena, &bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_atom() {
        let arena = Bump::new();
        let original = ArenaValue::atom(&arena, "hello-world");
        let bytes = original.serialize();
        let (decoded, _) = deserialize_arena_value(&arena, &bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_string() {
        let arena = Bump::new();
        let original = ArenaValue::string(&arena, "test string");
        let bytes = original.serialize();
        let (decoded, _) = deserialize_arena_value(&arena, &bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_nil() {
        let arena = Bump::new();
        let original = ArenaValue::unit(&arena);
        let bytes = original.serialize();
        let (decoded, _) = deserialize_arena_value(&arena, &bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_unit() {
        let arena = Bump::new();
        let original = ArenaValue::unit(&arena);
        let bytes = original.serialize();
        let (decoded, _) = deserialize_arena_value(&arena, &bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_empty() {
        let arena = Bump::new();
        let original = ArenaValue::empty(&arena);
        let bytes = original.serialize();
        let (decoded, _) = deserialize_arena_value(&arena, &bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_sexpr() {
        let arena = Bump::new();
        let original = ArenaValue::sexpr(&arena, vec![
            ArenaValue::atom(&arena, "+"),
            ArenaValue::long(&arena, 1),
            ArenaValue::long(&arena, 2),
        ]);
        let bytes = original.serialize();
        let (decoded, _) = deserialize_arena_value(&arena, &bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_nested_sexpr() {
        let arena = Bump::new();
        let inner = ArenaValue::sexpr(&arena, vec![
            ArenaValue::atom(&arena, "*"),
            ArenaValue::long(&arena, 2),
            ArenaValue::long(&arena, 3),
        ]);
        let original = ArenaValue::sexpr(&arena, vec![
            ArenaValue::atom(&arena, "+"),
            ArenaValue::long(&arena, 1),
            inner,
        ]);
        let bytes = original.serialize();
        let (decoded, _) = deserialize_arena_value(&arena, &bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_error() {
        let arena = Bump::new();
        let details = ArenaValue::atom(&arena, "details");
        let original = ArenaValue::error(&arena, "test error", details);
        let bytes = original.serialize();
        let (decoded, _) = deserialize_arena_value(&arena, &bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_type() {
        let arena = Bump::new();
        let inner = ArenaValue::atom(&arena, "Number");
        let original = ArenaValue::r#type(&arena, inner);
        let bytes = original.serialize();
        let (decoded, _) = deserialize_arena_value(&arena, &bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_conjunction() {
        let arena = Bump::new();
        let original = ArenaValue::conjunction(&arena, vec![
            ArenaValue::atom(&arena, "a"),
            ArenaValue::atom(&arena, "b"),
        ]);
        let bytes = original.serialize();
        let (decoded, _) = deserialize_arena_value(&arena, &bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_state() {
        let arena = Bump::new();
        let original = ArenaValue::state(&arena, 999);
        let bytes = original.serialize();
        let (decoded, _) = deserialize_arena_value(&arena, &bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    // ========================================================================
    // Hash Value Tests
    // ========================================================================

    #[test]
    fn test_hash_equal_values() {
        let arena = Bump::new();
        let v1 = ArenaValue::long(&arena, 42);
        let v2 = ArenaValue::long(&arena, 42);
        assert_eq!(v1.hash_value(), v2.hash_value());
    }

    #[test]
    fn test_hash_different_values() {
        let arena = Bump::new();
        let v1 = ArenaValue::long(&arena, 42);
        let v2 = ArenaValue::long(&arena, 43);
        assert_ne!(v1.hash_value(), v2.hash_value());
    }

    #[test]
    fn test_hash_different_types() {
        let arena = Bump::new();
        let long = ArenaValue::long(&arena, 42);
        let float = ArenaValue::float(&arena, 42.0);
        // Different types should (almost certainly) have different hashes
        assert_ne!(long.hash_value(), float.hash_value());
    }

    #[test]
    fn test_hash_nil_unit_empty() {
        let arena = Bump::new();
        let nil = ArenaValue::unit(&arena);
        let unit = ArenaValue::unit(&arena);
        let empty = ArenaValue::empty(&arena);
        // After Nil/Unit merge, nil and unit are the same value
        assert_eq!(nil.hash_value(), unit.hash_value());
        // Empty is still distinct from unit
        assert_ne!(unit.hash_value(), empty.hash_value());
    }

    // ========================================================================
    // MettaValueTrait Method Tests
    // ========================================================================

    #[test]
    fn test_get_head_symbol_sexpr() {
        let arena = Bump::new();
        let v = ArenaValue::sexpr(&arena, vec![
            ArenaValue::atom(&arena, "foo"),
            ArenaValue::long(&arena, 1),
        ]);
        assert_eq!(v.get_head_symbol(), Some("foo"));
    }

    #[test]
    fn test_get_head_symbol_variable_head() {
        let arena = Bump::new();
        let v = ArenaValue::sexpr(&arena, vec![
            ArenaValue::atom(&arena, "$x"),
            ArenaValue::long(&arena, 1),
        ]);
        // Variable as head returns None
        assert_eq!(v.get_head_symbol(), None);
    }

    #[test]
    fn test_get_head_symbol_bare_atom() {
        let arena = Bump::new();
        let v = ArenaValue::atom(&arena, "foo");
        assert_eq!(v.get_head_symbol(), Some("foo"));
    }

    #[test]
    fn test_get_arity_sexpr() {
        let arena = Bump::new();
        let v = ArenaValue::sexpr(&arena, vec![
            ArenaValue::atom(&arena, "foo"),
            ArenaValue::long(&arena, 1),
            ArenaValue::long(&arena, 2),
        ]);
        // Arity is len - 1 (excluding head)
        assert_eq!(v.get_arity(), 2);
    }

    #[test]
    fn test_get_arity_atom() {
        let arena = Bump::new();
        let v = ArenaValue::atom(&arena, "foo");
        assert_eq!(v.get_arity(), 0);
    }

    #[test]
    fn test_friendly_repr() {
        let arena = Bump::new();
        let v = ArenaValue::sexpr(&arena, vec![
            ArenaValue::atom(&arena, "+"),
            ArenaValue::long(&arena, 1),
            ArenaValue::long(&arena, 2),
        ]);
        assert_eq!(v.friendly_repr(), "(+ 1 2)");
    }

    #[test]
    fn test_friendly_type_name() {
        let arena = Bump::new();
        assert_eq!(ArenaValue::long(&arena, 1).friendly_type_name(), "Number (integer)");
        assert_eq!(ArenaValue::float(&arena, 1.0).friendly_type_name(), "Number (float)");
        assert_eq!(ArenaValue::bool(&arena, true).friendly_type_name(), "Bool");
    }

    // ========================================================================
    // Factory Tests
    // ========================================================================

    #[test]
    fn test_factory_creates_values() {
        let arena = Bump::new();
        let factory = ArenaValueFactory::new(&arena);

        let atom = factory.atom("test");
        assert!(atom.is_atom());

        let long = factory.long(42);
        assert!(long.is_long());

        let bool_val = factory.bool(true);
        assert!(bool_val.is_bool());

        let nil = factory.unit();
        assert!(nil.is_unit()); // nil() returns Unit after Nil/Unit merge

        let unit = factory.unit();
        assert!(unit.is_unit());
    }

    #[test]
    fn test_factory_sexpr_from_vec() {
        let arena = Bump::new();
        let factory = ArenaValueFactory::new(&arena);

        let items = vec![
            factory.atom("+"),
            factory.long(1),
            factory.long(2),
        ];
        let sexpr = factory.sexpr(items);
        assert!(sexpr.is_sexpr());
        assert_eq!(sexpr.as_sexpr().map(|s| s.len()), Some(3));
    }

    #[test]
    fn test_factory_sexpr_from_slice() {
        let arena = Bump::new();
        let factory = ArenaValueFactory::new(&arena);

        let items = [
            factory.atom("+"),
            factory.long(1),
            factory.long(2),
        ];
        let sexpr = factory.sexpr_from_slice(&items);
        assert!(sexpr.is_sexpr());
        assert_eq!(sexpr.as_sexpr().map(|s| s.len()), Some(3));
    }

    // ========================================================================
    // Deserialization Error Handling
    // ========================================================================

    #[test]
    fn test_deserialize_empty_input() {
        let arena = Bump::new();
        let result = deserialize_arena_value(&arena, &[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_unknown_tag() {
        let arena = Bump::new();
        let result = deserialize_arena_value(&arena, &[0xFF]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown tag"));
    }

    #[test]
    fn test_deserialize_truncated_long() {
        let arena = Bump::new();
        // LONG tag but only 4 bytes (needs 8)
        let result = deserialize_arena_value(&arena, &[0x03, 0x00, 0x00, 0x00, 0x00]);
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_truncated_float() {
        let arena = Bump::new();
        // FLOAT tag but only 4 bytes (needs 8)
        let result = deserialize_arena_value(&arena, &[0x04, 0x00, 0x00, 0x00, 0x00]);
        assert!(result.is_err());
    }

    // ========================================================================
    // Pointer Equality Fast Path
    // ========================================================================

    #[test]
    fn test_pointer_equality_fast_path() {
        let arena = Bump::new();
        let v = ArenaValue::long(&arena, 42);
        let v_copy = v; // Copy, same pointer
        // Both should be equal via pointer comparison fast path
        assert_eq!(v, v_copy);
    }
}
