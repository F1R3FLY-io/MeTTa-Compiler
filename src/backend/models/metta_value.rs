//! Arena-allocated MeTTa values.
//!
//! MettaValue provides Copy-semantic MeTTa values allocated from a
//! global slab allocator (`SlabAllocator`) via `GcFactory`.
//!
//! ## Key Benefits
//!
//! - **Copy semantics**: MettaValue is just a pointer (8 bytes), so Clone/Copy is free.
//! - **No reference counting**: Values don't need Arc overhead or atomic operations.
//! - **Lock-free allocation**: Multiple threads can allocate concurrently.
//! - **Background GC**: Dead values are reclaimed by a snapshot-based mark-sweep collector.
//! - **Cache-friendly**: Contiguous 64KB page layout.
//!
//! ## Memory Safety
//!
//! All references within MettaValue are `'static`, tied to the global slab allocator.
//! Values live for the program duration and are reclaimed by the GC when no longer reachable.

use std::fmt;

use super::metta_value_trait::{MettaValueTrait, MettaValueFactory};
use super::{MemoHandle, SpaceHandle};

/// Arena-allocated MeTTa value with O(1) clone (just copies the pointer).
///
/// This is a thin wrapper around a reference to MettaValueInner, providing
/// the same interface as MettaValue but using arena allocation.
#[derive(Clone, Copy)]
pub struct MettaValue {
    inner: &'static MettaValueInner,
}

/// The actual value enum, allocated in the arena.
///
/// This mirrors MettaValueInner but uses arena-allocated collections.
#[derive(Debug)]
pub enum MettaValueInner {
    /// An atom (symbol, variable, or literal) - string allocated in arena
    Atom(&'static str),
    /// A boolean literal
    Bool(bool),
    /// An integer literal
    Long(i64),
    /// A floating point literal
    Float(f64),
    /// A string literal - string content allocated in arena
    String(&'static str),
    /// An s-expression (list of values) - slice allocated in arena
    SExpr(&'static [MettaValue]),
    /// An error with message and details
    Error(&'static str, MettaValue),
    /// A type (first-class types as atoms)
    Type(MettaValue),
    /// A conjunction of goals (MORK-style logical AND)
    Conjunction(&'static [MettaValue]),
    /// A first-class space value - reuses existing SpaceHandle
    Space(SpaceHandle),
    /// A reference to a mutable state cell (id)
    State(u64),
    /// Unit value for side-effecting operations
    Unit,
    /// A memoization table - reuses existing MemoHandle
    Memo(MemoHandle),
    /// Quoted expression — prevents evaluation, preserves the quote wrapper.
    /// Transparent to introspection: car-atom sees "quote", get-metatype sees "Expression".
    Quoted(MettaValue),
    /// Empty sentinel
    Empty,
}

// ============================================================================
// Thread Safety for Static Arena Values
// ============================================================================
//
// MettaValue is safe to share across threads because:
// 1. The global SlabAllocator is lock-free and thread-safe
// 2. MettaValue contains only immutable references to slab-allocated data
// 3. Once created, arena values are never mutated
// 4. The 'static lifetime ensures the referenced data lives until GC reclaims it
//
// The unsafe impl is required because:
// - MettaValueInner contains raw references (&'static [MettaValue]) from slab allocation
// - The Rust compiler requires explicit Send/Sync for types with certain reference patterns
// - However, we only use the slab for allocation, never for mutation after creation
//
// SAFETY INVARIANT: Values must only be read, never mutated, after creation.
// This is enforced by MettaValue's API which provides no mutation methods.

// SAFETY: MettaValue can be sent between threads because:
// - It's an immutable reference to 'static slab-allocated data
// - The global SlabAllocator is thread-safe (lock-free Treiber stack + atomic bump)
// - Once created, the data is never mutated
unsafe impl Send for MettaValue {}

// SAFETY: MettaValue can be shared between threads because:
// - It only provides immutable access to the underlying data
// - No mutation methods exist on MettaValue
// - The referenced data is immutable after creation
unsafe impl Sync for MettaValue {}

// SAFETY: MettaValueInner can be sent between threads for the same reasons
unsafe impl Send for MettaValueInner {}

// SAFETY: MettaValueInner can be shared between threads for the same reasons
unsafe impl Sync for MettaValueInner {}

impl MettaValue {
    /// Access the inner enum for pattern matching
    #[inline]
    pub fn inner(&self) -> &MettaValueInner {
        self.inner
    }

    /// Construct an MettaValue from a reference to an MettaValueInner.
    #[inline]
    pub fn from_inner(inner: &'static MettaValueInner) -> Self {
        Self { inner }
    }

    /// Get a raw pointer to the inner value (used by GC for slot identification).
    #[inline]
    pub fn inner_ptr(&self) -> *const MettaValueInner {
        self.inner as *const MettaValueInner
    }

    // ========================================================================
    // Type checking and inspection methods
    // ========================================================================

    /// Check if this is an Atom variant
    #[inline]
    pub fn is_atom(&self) -> bool {
        matches!(self.inner, MettaValueInner::Atom(_))
    }

    /// Check if this is a Bool variant
    #[inline]
    pub fn is_bool(&self) -> bool {
        matches!(self.inner, MettaValueInner::Bool(_))
    }

    /// Check if this is a Long variant
    #[inline]
    pub fn is_long(&self) -> bool {
        matches!(self.inner, MettaValueInner::Long(_))
    }

    /// Check if this is a Float variant
    #[inline]
    pub fn is_float(&self) -> bool {
        matches!(self.inner, MettaValueInner::Float(_))
    }

    /// Check if this is a String variant
    #[inline]
    pub fn is_string(&self) -> bool {
        matches!(self.inner, MettaValueInner::String(_))
    }

    /// Check if this is an SExpr variant
    #[inline]
    pub fn is_sexpr(&self) -> bool {
        matches!(self.inner, MettaValueInner::SExpr(_))
    }

    /// Check if this is an Error variant
    #[inline]
    pub fn is_error(&self) -> bool {
        matches!(self.inner, MettaValueInner::Error(_, _))
    }

    /// Check if this is a Type variant
    #[inline]
    pub fn is_type(&self) -> bool {
        matches!(self.inner, MettaValueInner::Type(_))
    }

    /// Check if this is a Conjunction variant
    #[inline]
    pub fn is_conjunction(&self) -> bool {
        matches!(self.inner, MettaValueInner::Conjunction(_))
    }

    /// Check if this is a Space variant
    #[inline]
    pub fn is_space(&self) -> bool {
        matches!(self.inner, MettaValueInner::Space(_))
    }

    /// Check if this is a State variant
    #[inline]
    pub fn is_state(&self) -> bool {
        matches!(self.inner, MettaValueInner::State(_))
    }

    /// Check if this is a Unit variant
    #[inline]
    pub fn is_unit(&self) -> bool {
        matches!(self.inner, MettaValueInner::Unit)
    }

    /// Check if this is a Memo variant
    #[inline]
    pub fn is_memo(&self) -> bool {
        matches!(self.inner, MettaValueInner::Memo(_))
    }

    /// Check if this is a Quoted variant
    #[inline]
    pub fn is_quoted(&self) -> bool {
        matches!(self.inner, MettaValueInner::Quoted(_))
    }

    /// Check if this is an Empty variant
    #[inline]
    pub fn is_empty(&self) -> bool {
        matches!(self.inner, MettaValueInner::Empty)
    }

    /// Check if this value is a variable (Atom starting with $)
    #[inline]
    pub fn is_variable(&self) -> bool {
        matches!(self.inner, MettaValueInner::Atom(s) if s.starts_with('$'))
    }

    // ========================================================================
    // Accessor methods for extracting inner values
    // ========================================================================

    /// Try to extract as atom string
    #[inline]
    pub fn as_atom(&self) -> Option<&'static str> {
        match self.inner {
            MettaValueInner::Atom(s) => Some(s),
            _ => None,
        }
    }

    /// Try to extract as bool
    #[inline]
    pub fn as_bool(&self) -> Option<bool> {
        match self.inner {
            MettaValueInner::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Try to extract as i64
    #[inline]
    pub fn as_long(&self) -> Option<i64> {
        match self.inner {
            MettaValueInner::Long(n) => Some(*n),
            _ => None,
        }
    }

    /// Try to extract as f64
    #[inline]
    pub fn as_float(&self) -> Option<f64> {
        match self.inner {
            MettaValueInner::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// Try to extract as string
    #[inline]
    pub fn as_string(&self) -> Option<&'static str> {
        match self.inner {
            MettaValueInner::String(s) => Some(s),
            _ => None,
        }
    }

    /// Try to extract as sexpr items
    #[inline]
    pub fn as_sexpr(&self) -> Option<&[MettaValue]> {
        match self.inner {
            MettaValueInner::SExpr(items) => Some(items),
            _ => None,
        }
    }

    /// Try to extract as error (message, details)
    #[inline]
    pub fn as_error(&self) -> Option<(&'static str, MettaValue)> {
        match self.inner {
            MettaValueInner::Error(msg, details) => Some((msg, *details)),
            _ => None,
        }
    }

    /// Try to extract as type inner value
    #[inline]
    pub fn as_type(&self) -> Option<MettaValue> {
        match self.inner {
            MettaValueInner::Type(inner) => Some(*inner),
            _ => None,
        }
    }

    /// Try to extract as conjunction goals
    #[inline]
    pub fn as_conjunction(&self) -> Option<&[MettaValue]> {
        match self.inner {
            MettaValueInner::Conjunction(goals) => Some(goals),
            _ => None,
        }
    }

    /// Try to extract as space handle
    #[inline]
    pub fn as_space(&self) -> Option<&SpaceHandle> {
        match self.inner {
            MettaValueInner::Space(handle) => Some(handle),
            _ => None,
        }
    }

    /// Try to extract as state id
    #[inline]
    pub fn as_state(&self) -> Option<u64> {
        match self.inner {
            MettaValueInner::State(id) => Some(*id),
            _ => None,
        }
    }

    /// Try to extract as memo handle
    #[inline]
    pub fn as_memo(&self) -> Option<&MemoHandle> {
        match self.inner {
            MettaValueInner::Memo(handle) => Some(handle),
            _ => None,
        }
    }

    /// Try to extract the inner value of a Quoted variant (owned copy)
    #[inline]
    pub fn as_quoted(&self) -> Option<MettaValue> {
        match self.inner {
            MettaValueInner::Quoted(inner) => Some(*inner),
            _ => None,
        }
    }

    /// Try to extract a reference to the inner value of a Quoted variant
    #[inline]
    pub fn as_quoted_ref(&self) -> Option<&MettaValue> {
        match self.inner {
            MettaValueInner::Quoted(inner) => Some(inner),
            _ => None,
        }
    }

    /// Get the type name of this value as a string slice
    pub fn type_name(&self) -> &'static str {
        match self.inner {
            MettaValueInner::Atom(s) if s.starts_with('$') => "Variable",
            MettaValueInner::Atom(_) => "Symbol",
            MettaValueInner::Bool(_) => "Bool",
            MettaValueInner::Long(_) => "Number",
            MettaValueInner::Float(_) => "Number",
            MettaValueInner::String(_) => "String",
            MettaValueInner::SExpr(_) => "Expression",
            MettaValueInner::Unit => "Unit",
            MettaValueInner::Error(_, _) => "Error",
            MettaValueInner::Type(_) => "Type",
            MettaValueInner::Conjunction(_) => "Conjunction",
            MettaValueInner::Space(_) => "Space",
            MettaValueInner::State(_) => "State",
            MettaValueInner::Quoted(_) => "Expression",
            MettaValueInner::Memo(_) => "Memo",
            MettaValueInner::Empty => "Empty",
        }
    }
}

// ============================================================================
// Backward-Compat Constructors for MettaValue = MettaValue
// ============================================================================
//
// These associated functions preserve the old `MettaValue::Atom(s)` construction
// syntax now that MettaValue is a type alias for MettaValue.
// They delegate to the global GC slab allocator.

impl MettaValue {
    /// Create an Atom variant via global allocator.
    /// Backward-compat: `MettaValue::Atom("symbol")` or `MettaValue::Atom(owned_string)`
    #[allow(non_snake_case)]
    #[inline]
    pub fn Atom(s: impl AsRef<str>) -> Self {
        super::gc_allocator::global_factory().atom(s.as_ref())
    }

    /// Create a Bool variant via global allocator.
    #[allow(non_snake_case)]
    #[inline]
    pub fn Bool(b: bool) -> Self {
        super::gc_allocator::global_factory().bool(b)
    }

    /// Create a Long variant via global allocator.
    #[allow(non_snake_case)]
    #[inline]
    pub fn Long(n: i64) -> Self {
        super::gc_allocator::global_factory().long(n)
    }

    /// Create a Float variant via global allocator.
    #[allow(non_snake_case)]
    #[inline]
    pub fn Float(f: f64) -> Self {
        super::gc_allocator::global_factory().float(f)
    }

    /// Create a String variant via global allocator.
    #[allow(non_snake_case)]
    #[inline]
    pub fn String(s: impl AsRef<str>) -> Self {
        super::gc_allocator::global_factory().string(s.as_ref())
    }

    /// Create an SExpr variant via global allocator.
    #[allow(non_snake_case)]
    #[inline]
    pub fn SExpr(items: Vec<MettaValue>) -> Self {
        super::gc_allocator::global_factory().sexpr(items)
    }

    /// Create an Error variant via global allocator.
    #[allow(non_snake_case)]
    #[inline]
    pub fn Error(msg: impl AsRef<str>, details: MettaValue) -> Self {
        super::gc_allocator::global_factory().error(msg.as_ref(), details)
    }

    /// Create a Type variant via global allocator.
    #[allow(non_snake_case)]
    #[inline]
    pub fn Type(inner: MettaValue) -> Self {
        super::gc_allocator::global_factory().type_value(inner)
    }

    /// Create a Conjunction variant via global allocator.
    #[allow(non_snake_case)]
    #[inline]
    pub fn Conjunction(goals: Vec<MettaValue>) -> Self {
        super::gc_allocator::global_factory().conjunction(goals)
    }

    /// Create a Space variant via global allocator.
    #[allow(non_snake_case)]
    #[inline]
    pub fn Space(handle: SpaceHandle) -> Self {
        super::gc_allocator::global_factory().space(handle)
    }

    /// Create a State variant via global allocator.
    #[allow(non_snake_case)]
    #[inline]
    pub fn State(id: u64) -> Self {
        super::gc_allocator::global_factory().state(id)
    }

    /// Create a Unit variant via global allocator.
    #[allow(non_snake_case)]
    #[inline]
    pub fn Unit() -> Self {
        super::gc_allocator::global_factory().unit()
    }

    /// Create a Memo variant via global allocator.
    #[allow(non_snake_case)]
    #[inline]
    pub fn Memo(handle: MemoHandle) -> Self {
        super::gc_allocator::global_factory().memo(handle)
    }

    /// Create an Empty variant via global allocator.
    #[allow(non_snake_case)]
    #[inline]
    pub fn Empty() -> Self {
        super::gc_allocator::global_factory().empty()
    }

    // ========================================================================
    // Helper constructors (backward compat)
    // ========================================================================

    /// Create a symbol atom from a string slice.
    #[inline]
    pub fn sym(s: &str) -> Self {
        super::gc_allocator::global_factory().atom(s)
    }

    /// Create a variable atom (prefixed with $).
    #[inline]
    pub fn var(name: &str) -> Self {
        super::gc_allocator::global_factory().atom(&format!("${}", name))
    }

    /// Create a Quoted variant via global allocator.
    #[allow(non_snake_case)]
    #[inline]
    pub fn Quoted(inner: Self) -> Self {
        super::gc_allocator::global_factory().quote(inner)
    }

    /// Create a quoted expression.
    pub fn quote(inner: Self) -> Self {
        super::gc_allocator::global_factory().quote(inner)
    }

    /// Check if two values point to the same inner allocation (pointer equality).
    #[inline]
    pub fn ptr_eq(&self, other: &Self) -> bool {
        self.inner_ptr() == other.inner_ptr()
    }

    /// Backward-compat no-op: returns self reference.
    /// Previously returned `&Arc<MettaValueInner>`.
    #[inline]
    pub fn arc(&self) -> &Self {
        self
    }

    // ========================================================================
    // Methods formerly only on heap MettaValue
    // ========================================================================

    /// Convert to canonical MeTTa string representation.
    /// Produces syntax that can be round-trip parsed by the MeTTa parser.
    /// Guarantees: parse(to_metta_string(value)) == value
    pub fn to_metta_string(&self) -> String {
        match self.inner() {
            MettaValueInner::Atom(s) => s.to_string(),
            MettaValueInner::Bool(true) => "True".to_string(),
            MettaValueInner::Bool(false) => "False".to_string(),
            MettaValueInner::Long(n) => n.to_string(),
            MettaValueInner::Float(f) => {
                let s = f.to_string();
                // Ensure float representation is unambiguous
                if s.contains('.') || s.contains('e') || s.contains('E') {
                    s
                } else {
                    format!("{}.0", s)
                }
            }
            MettaValueInner::String(s) => format!("\"{}\"", escape_metta_string(s)),
            MettaValueInner::SExpr(items) => {
                let inner = items
                    .iter()
                    .map(|v| v.to_metta_string())
                    .collect::<Vec<_>>()
                    .join(" ");
                format!("({})", inner)
            }
            MettaValueInner::Unit => "()".to_string(),
            MettaValueInner::Error(msg, details) => {
                format!(
                    "(error \"{}\" {})",
                    escape_metta_string(msg),
                    details.to_metta_string()
                )
            }
            MettaValueInner::Type(t) => t.to_metta_string(),
            MettaValueInner::Conjunction(goals) => {
                if goals.is_empty() {
                    "(,)".to_string()
                } else {
                    let inner = goals
                        .iter()
                        .map(|v| v.to_metta_string())
                        .collect::<Vec<_>>()
                        .join(" ");
                    format!("(, {})", inner)
                }
            }
            MettaValueInner::Quoted(inner) => format!("(quote {})", inner.to_metta_string()),
            MettaValueInner::Space(h) => format!("(Space {} \"{}\")", h.id, h.name),
            MettaValueInner::State(id) => format!("(State {})", id),
            MettaValueInner::Memo(h) => format!("(Memo {} \"{}\")", h.id, h.name),
            MettaValueInner::Empty => "Empty".to_string(),
        }
    }

    /// Convert to MORK s-expression string format.
    pub fn to_mork_string(&self) -> String {
        match self.inner() {
            MettaValueInner::Atom(s) => {
                if *s == "&" || *s == "&self" || *s == "&kb" || *s == "&stack" {
                    s.to_string()
                } else if s.starts_with('$') || s.starts_with('&') || s.starts_with('\'') {
                    format!("${}", &s[1..])
                } else if *s == "_" {
                    "$".to_string()
                } else {
                    s.to_string()
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
            MettaValueInner::Unit => "()".to_string(),
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
            MettaValueInner::Quoted(inner) => format!("(quote {})", inner.to_mork_string()),
            MettaValueInner::Memo(handle) => format!("(Memo {} \"{}\")", handle.id, handle.name),
            MettaValueInner::Empty => "Empty".to_string(),
        }
    }

    /// Convert to a JSON-like string representation.
    pub fn to_json_string(&self) -> String {
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
            MettaValueInner::Unit => r#"{"type":"unit"}"#.to_string(),
            MettaValueInner::SExpr(items) => {
                let items_json: Vec<String> =
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
                let goals_json: Vec<String> =
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
            MettaValueInner::Memo(handle) => {
                format!(
                    r#"{{"type":"memo","id":{},"name":"{}"}}"#,
                    handle.id,
                    escape_json(&handle.name)
                )
            }
            MettaValueInner::Quoted(inner) => {
                format!(r#"{{"type":"quoted","value":{}}}"#, inner.to_json_string())
            }
            MettaValueInner::Empty => r#"{"type":"empty"}"#.to_string(),
        }
    }
}

/// Escape special characters in a string for JSON encoding.
pub fn escape_json(s: &str) -> String {
    s.replace('\\', r"\\")
        .replace('"', r#"\""#)
        .replace('\n', r"\n")
        .replace('\r', r"\r")
        .replace('\t', r"\t")
}

/// Escape string content for MeTTa string literals.
/// Reverses the logic in the parser's unescape_string().
/// Supports: \n, \t, \r, \\, \", \x##, \u{...}
pub fn escape_metta_string(s: &str) -> String {
    let mut result = String::new();
    for ch in s.chars() {
        match ch {
            '\n' => result.push_str(r"\n"),
            '\t' => result.push_str(r"\t"),
            '\r' => result.push_str(r"\r"),
            '\\' => result.push_str(r"\\"),
            '"' => result.push_str(r#"\""#),
            // ASCII control characters — use hex escape
            c if c.is_control() && (c as u32) < 256 => {
                result.push_str(&format!(r"\x{:02x}", c as u8));
            }
            // Non-ASCII characters — use unicode escape if needed
            c if !c.is_ascii() => {
                result.push_str(&format!(r"\u{{{:x}}}", c as u32));
            }
            // Regular printable ASCII — no escaping needed
            c => result.push(c),
        }
    }
    result
}

// ============================================================================
// Trait implementations
// ============================================================================

impl fmt::Debug for MettaValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.inner.fmt(f)
    }
}

impl fmt::Display for MettaValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.inner {
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
            MettaValueInner::Unit => write!(f, "()"),
            MettaValueInner::Error(msg, details) => write!(f, "(Error {} {})", msg, details),
            MettaValueInner::Type(inner) => write!(f, "(: {})", inner),
            MettaValueInner::Conjunction(goals) => {
                write!(f, "(,")?;
                for goal in goals.iter() {
                    write!(f, " {}", goal)?;
                }
                write!(f, ")")
            }
            MettaValueInner::Space(handle) => write!(f, "<Space:{}>", handle.name),
            MettaValueInner::State(id) => write!(f, "<State:{}>", id),
            MettaValueInner::Quoted(inner) => write!(f, "(quote {})", inner),
            MettaValueInner::Memo(handle) => write!(f, "<Memo:{}>", handle.name),
            MettaValueInner::Empty => write!(f, "Empty"),
        }
    }
}

impl PartialEq for MettaValue {
    fn eq(&self, other: &Self) -> bool {
        // Fast path: pointer equality
        std::ptr::eq(self.inner, other.inner) || self.inner == other.inner
    }
}

impl PartialEq for MettaValueInner {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (MettaValueInner::Atom(a), MettaValueInner::Atom(b)) => a == b,
            (MettaValueInner::Bool(a), MettaValueInner::Bool(b)) => a == b,
            (MettaValueInner::Long(a), MettaValueInner::Long(b)) => a == b,
            (MettaValueInner::Float(a), MettaValueInner::Float(b)) => a == b,
            (MettaValueInner::String(a), MettaValueInner::String(b)) => a == b,
            (MettaValueInner::SExpr(a), MettaValueInner::SExpr(b)) => a == b,
            (MettaValueInner::Unit, MettaValueInner::Unit) => true,
            (MettaValueInner::Error(ma, da), MettaValueInner::Error(mb, db)) => ma == mb && da == db,
            (MettaValueInner::Type(a), MettaValueInner::Type(b)) => a == b,
            (MettaValueInner::Conjunction(a), MettaValueInner::Conjunction(b)) => a == b,
            (MettaValueInner::Space(a), MettaValueInner::Space(b)) => a.id == b.id,
            (MettaValueInner::State(a), MettaValueInner::State(b)) => a == b,
            (MettaValueInner::Quoted(a), MettaValueInner::Quoted(b)) => a == b,
            (MettaValueInner::Memo(a), MettaValueInner::Memo(b)) => a.id == b.id,
            (MettaValueInner::Empty, MettaValueInner::Empty) => true,
            _ => false,
        }
    }
}

impl Eq for MettaValue {}
impl Eq for MettaValueInner {}

impl std::hash::Hash for MettaValue {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        // Delegate to the MettaValueTrait::hash_value() method which provides
        // a high-quality xxh3 hash of the value's structure.
        self.hash_value().hash(state);
    }
}

// ============================================================================
// MettaValue trait implementation for MettaValue
// ============================================================================

impl MettaValueTrait for MettaValue {
    type SExprSlice = [MettaValue];

    #[inline]
    fn inner_ptr(&self) -> *const MettaValueInner {
        self.inner as *const MettaValueInner
    }

    #[inline]
    unsafe fn from_inner_ptr(ptr: *const MettaValueInner) -> Self {
        // SAFETY: The pointer is slab-allocated with 'static lifetime (managed by GC).
        MettaValue::from_inner(&*ptr)
    }

    #[inline]
    fn is_atom(&self) -> bool {
        matches!(self.inner, MettaValueInner::Atom(_))
    }

    #[inline]
    fn is_bool(&self) -> bool {
        matches!(self.inner, MettaValueInner::Bool(_))
    }

    #[inline]
    fn is_long(&self) -> bool {
        matches!(self.inner, MettaValueInner::Long(_))
    }

    #[inline]
    fn is_float(&self) -> bool {
        matches!(self.inner, MettaValueInner::Float(_))
    }

    #[inline]
    fn is_string(&self) -> bool {
        matches!(self.inner, MettaValueInner::String(_))
    }

    #[inline]
    fn is_sexpr(&self) -> bool {
        matches!(self.inner, MettaValueInner::SExpr(_))
    }

    #[inline]
    fn is_error(&self) -> bool {
        matches!(self.inner, MettaValueInner::Error(_, _))
    }

    #[inline]
    fn is_type(&self) -> bool {
        matches!(self.inner, MettaValueInner::Type(_))
    }

    #[inline]
    fn is_conjunction(&self) -> bool {
        matches!(self.inner, MettaValueInner::Conjunction(_))
    }

    #[inline]
    fn is_space(&self) -> bool {
        matches!(self.inner, MettaValueInner::Space(_))
    }

    #[inline]
    fn is_state(&self) -> bool {
        matches!(self.inner, MettaValueInner::State(_))
    }

    #[inline]
    fn is_unit(&self) -> bool {
        matches!(self.inner, MettaValueInner::Unit)
    }

    #[inline]
    fn is_memo(&self) -> bool {
        matches!(self.inner, MettaValueInner::Memo(_))
    }

    #[inline]
    fn is_quoted(&self) -> bool {
        matches!(self.inner, MettaValueInner::Quoted(_))
    }

    #[inline]
    fn is_empty(&self) -> bool {
        matches!(self.inner, MettaValueInner::Empty)
    }

    #[inline]
    fn is_variable(&self) -> bool {
        matches!(self.inner, MettaValueInner::Atom(s) if s.starts_with('$'))
    }

    #[inline]
    fn is_ground_type(&self) -> bool {
        // Unit/() is NOT a ground type — it's an expression in MeTTa HE
        matches!(
            self.inner,
            MettaValueInner::Bool(_)
                | MettaValueInner::Long(_)
                | MettaValueInner::Float(_)
                | MettaValueInner::String(_)
        )
    }

    #[inline]
    fn as_atom(&self) -> Option<&str> {
        match self.inner {
            MettaValueInner::Atom(s) => Some(s),
            _ => None,
        }
    }

    #[inline]
    fn as_bool(&self) -> Option<bool> {
        match self.inner {
            MettaValueInner::Bool(b) => Some(*b),
            _ => None,
        }
    }

    #[inline]
    fn as_long(&self) -> Option<i64> {
        match self.inner {
            MettaValueInner::Long(n) => Some(*n),
            _ => None,
        }
    }

    #[inline]
    fn as_float(&self) -> Option<f64> {
        match self.inner {
            MettaValueInner::Float(f) => Some(*f),
            _ => None,
        }
    }

    #[inline]
    fn as_string(&self) -> Option<&str> {
        match self.inner {
            MettaValueInner::String(s) => Some(s),
            _ => None,
        }
    }

    #[inline]
    fn as_sexpr(&self) -> Option<&[Self]> {
        match self.inner {
            MettaValueInner::SExpr(items) => Some(items),
            _ => None,
        }
    }

    #[inline]
    fn as_error(&self) -> Option<(&str, &Self)> {
        match self.inner {
            MettaValueInner::Error(msg, details) => Some((msg, details)),
            _ => None,
        }
    }

    #[inline]
    fn as_type(&self) -> Option<&Self> {
        match self.inner {
            MettaValueInner::Type(inner) => Some(inner),
            _ => None,
        }
    }

    #[inline]
    fn as_conjunction(&self) -> Option<&[Self]> {
        match self.inner {
            MettaValueInner::Conjunction(goals) => Some(goals),
            _ => None,
        }
    }

    #[inline]
    fn as_space(&self) -> Option<&SpaceHandle> {
        match self.inner {
            MettaValueInner::Space(handle) => Some(handle),
            _ => None,
        }
    }

    #[inline]
    fn as_state(&self) -> Option<u64> {
        match self.inner {
            MettaValueInner::State(id) => Some(*id),
            _ => None,
        }
    }

    #[inline]
    fn as_memo(&self) -> Option<&MemoHandle> {
        match self.inner {
            MettaValueInner::Memo(handle) => Some(handle),
            _ => None,
        }
    }

    #[inline]
    fn as_quoted(&self) -> Option<Self> {
        match self.inner {
            MettaValueInner::Quoted(inner) => Some(*inner),
            _ => None,
        }
    }

    #[inline]
    fn as_quoted_ref(&self) -> Option<&Self> {
        match self.inner {
            MettaValueInner::Quoted(inner) => Some(inner),
            _ => None,
        }
    }

    fn type_name(&self) -> &'static str {
        match self.inner {
            MettaValueInner::Atom(s) if s.starts_with('$') => "Variable",
            MettaValueInner::Atom(_) => "Symbol",
            MettaValueInner::Bool(_) => "Bool",
            MettaValueInner::Long(_) => "Number",
            MettaValueInner::Float(_) => "Number",
            MettaValueInner::String(_) => "String",
            MettaValueInner::SExpr(_) => "Expression",
            MettaValueInner::Unit => "Unit",
            MettaValueInner::Error(_, _) => "Error",
            MettaValueInner::Type(_) => "Type",
            MettaValueInner::Conjunction(_) => "Conjunction",
            MettaValueInner::Space(_) => "Space",
            MettaValueInner::State(_) => "State",
            MettaValueInner::Quoted(_) => "Expression",
            MettaValueInner::Memo(_) => "Memo",
            MettaValueInner::Empty => "Empty",
        }
    }

    fn friendly_type_name(&self) -> &'static str {
        match self.inner {
            MettaValueInner::Long(_) => "Number (integer)",
            MettaValueInner::Float(_) => "Number (float)",
            MettaValueInner::Bool(_) => "Bool",
            MettaValueInner::String(_) => "String",
            MettaValueInner::Atom(_) => "Atom",
            MettaValueInner::Unit => "Unit",
            MettaValueInner::SExpr(_) => "S-expression",
            MettaValueInner::Quoted(_) => "Quoted expression",
            MettaValueInner::Error(_, _) => "Error",
            MettaValueInner::Type(_) => "Type",
            MettaValueInner::Conjunction(_) => "Conjunction",
            MettaValueInner::Space(_) => "Space",
            MettaValueInner::State(_) => "State",
            MettaValueInner::Memo(_) => "Memo",
            MettaValueInner::Empty => "Empty",
        }
    }

    fn get_head_symbol(&self) -> Option<&str> {
        // Helper to check if an atom is a space reference (not a variable)
        fn is_space_ref(s: &str) -> bool {
            s == "&" || s == "&self" || s == "&kb" || s == "&stack"
        }

        match self.inner {
            // For s-expressions like (double $x), extract "double"
            MettaValueInner::SExpr(items) if !items.is_empty() => match items[0].inner {
                MettaValueInner::Atom(head)
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
            MettaValueInner::Atom(head)
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
            MettaValueInner::SExpr(items) if !items.is_empty() => items.len() - 1, // Exclude head
            _ => 0,
        }
    }

    fn serialize(&self) -> Vec<u8> {
        let mut buf = Vec::new();
        serialize_value(self, &mut buf);
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
        hash_value_for_trait(self, &mut hasher);
        hasher.finish()
    }

    fn friendly_repr(&self) -> std::string::String {
        // Stack-based implementation to avoid recursion on deeply nested structures
        enum ReprWork<'a> {
            Process(&'a MettaValue),
            Join {
                count: usize,
                prefix: &'static str,
                suffix: &'static str,
                separator: &'static str,
            },
        }

        let mut work_stack: Vec<ReprWork<'_>> = Vec::with_capacity(16);
        let mut result_stack: Vec<std::string::String> = Vec::with_capacity(16);

        work_stack.push(ReprWork::Process(self));

        while let Some(work) = work_stack.pop() {
            match work {
                ReprWork::Process(val) => match &val.inner {
                    MettaValueInner::Long(n) => result_stack.push(n.to_string()),
                    MettaValueInner::Float(f) => result_stack.push(f.to_string()),
                    MettaValueInner::Bool(b) => {
                        result_stack.push(if *b { "True" } else { "False" }.to_string());
                    }
                    MettaValueInner::String(s) => result_stack.push(format!("\"{}\"", s)),
                    MettaValueInner::Atom(a) => result_stack.push(a.to_string()),
                    MettaValueInner::Unit => result_stack.push("()".to_string()),
                    MettaValueInner::Empty => result_stack.push("Empty".to_string()),
                    MettaValueInner::Space(handle) => {
                        result_stack.push(format!("(Space {} \"{}\")", handle.id, handle.name));
                    }
                    MettaValueInner::State(id) => {
                        result_stack.push(format!("(State {})", id));
                    }
                    MettaValueInner::Memo(handle) => {
                        result_stack.push(format!("(Memo {} \"{}\")", handle.id, handle.name));
                    }
                    MettaValueInner::Error(msg, _) => {
                        result_stack.push(format!("(error \"{}\")", msg));
                    }
                    MettaValueInner::Type(t) => {
                        work_stack.push(ReprWork::Join {
                            count: 1,
                            prefix: "(: ",
                            suffix: ")",
                            separator: "",
                        });
                        work_stack.push(ReprWork::Process(t));
                    }
                    MettaValueInner::Quoted(inner) => {
                        work_stack.push(ReprWork::Join {
                            count: 1,
                            prefix: "(quote ",
                            suffix: ")",
                            separator: "",
                        });
                        work_stack.push(ReprWork::Process(inner));
                    }
                    MettaValueInner::SExpr(items) => {
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
                    MettaValueInner::Conjunction(goals) => {
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
        enum ReprWork<'a> {
            Process(&'a MettaValue),
            Join {
                count: usize,
                prefix: &'static str,
                suffix: &'static str,
                separator: &'static str,
            },
        }

        let mut work_stack: Vec<ReprWork<'_>> = Vec::with_capacity(16);
        let mut result_stack: Vec<std::string::String> = Vec::with_capacity(16);

        work_stack.push(ReprWork::Process(self));

        while let Some(work) = work_stack.pop() {
            match work {
                ReprWork::Process(val) => match &val.inner {
                    MettaValueInner::Long(n) => result_stack.push(n.to_string()),
                    MettaValueInner::Float(f) => result_stack.push(f.to_string()),
                    MettaValueInner::Bool(b) => {
                        result_stack.push(if *b { "True" } else { "False" }.to_string());
                    }
                    // Key difference: strings printed without quotes for display
                    MettaValueInner::String(s) => result_stack.push(s.to_string()),
                    MettaValueInner::Atom(a) => result_stack.push(a.to_string()),
                    MettaValueInner::Unit => result_stack.push("()".to_string()),
                    MettaValueInner::Empty => result_stack.push("Empty".to_string()),
                    MettaValueInner::Space(handle) => {
                        result_stack.push(format!("(Space {} \"{}\")", handle.id, handle.name));
                    }
                    MettaValueInner::State(id) => {
                        result_stack.push(format!("(State {})", id));
                    }
                    MettaValueInner::Memo(handle) => {
                        result_stack.push(format!("(Memo {} \"{}\")", handle.id, handle.name));
                    }
                    MettaValueInner::Error(msg, _) => {
                        result_stack.push(format!("(Error \"{}\")", msg));
                    }
                    MettaValueInner::Type(t) => {
                        work_stack.push(ReprWork::Join {
                            count: 1,
                            prefix: "(: ",
                            suffix: ")",
                            separator: "",
                        });
                        work_stack.push(ReprWork::Process(t));
                    }
                    MettaValueInner::Quoted(inner) => {
                        work_stack.push(ReprWork::Join {
                            count: 1,
                            prefix: "(quote ",
                            suffix: ")",
                            separator: "",
                        });
                        work_stack.push(ReprWork::Process(inner));
                    }
                    MettaValueInner::SExpr(items) => {
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
                    MettaValueInner::Conjunction(goals) => {
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
// Serialization helpers for MettaValue
// ============================================================================

/// Tag bytes for serialization format (same as MettaValue)
pub mod serialize_tags {
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
    pub const QUOTED: u8 = 0x10;
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
pub(crate) fn read_varint(bytes: &[u8]) -> Result<(usize, usize), std::string::String> {
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

/// Recursively hash an MettaValue using trait-based accessors.
///
/// Used by `MettaValue::hash_value()` for complex types (strings, atoms, s-expressions).
fn hash_value_for_trait<H: std::hash::Hasher>(value: &MettaValue, hasher: &mut H) {
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
            hash_value_for_trait(item, hasher);
        }
    } else if let Some(inner) = value.as_quoted() {
        // Hash Quoted as ("quote", inner) so it matches the S-expr representation
        "quote".hash(hasher);
        hash_value_for_trait(&inner, hasher);
    }
}

/// Serialize an MettaValue to bytes
fn serialize_value(value: &MettaValue, buf: &mut Vec<u8>) {
    use serialize_tags::*;
    match value.inner {
        MettaValueInner::Atom(s) => {
            buf.push(ATOM);
            write_varint(buf, s.len());
            buf.extend_from_slice(s.as_bytes());
        }
        MettaValueInner::Bool(b) => {
            buf.push(BOOL);
            buf.push(if *b { 1 } else { 0 });
        }
        MettaValueInner::Long(n) => {
            buf.push(LONG);
            buf.extend_from_slice(&n.to_le_bytes());
        }
        MettaValueInner::Float(f) => {
            buf.push(FLOAT);
            buf.extend_from_slice(&f.to_le_bytes());
        }
        MettaValueInner::String(s) => {
            buf.push(STRING);
            write_varint(buf, s.len());
            buf.extend_from_slice(s.as_bytes());
        }
        MettaValueInner::SExpr(items) => {
            buf.push(SEXPR);
            write_varint(buf, items.len());
            for item in items.iter() {
                serialize_value(item, buf);
            }
        }
        MettaValueInner::Unit => {
            buf.push(UNIT_LEGACY);
        }
        MettaValueInner::Error(msg, details) => {
            buf.push(ERROR);
            write_varint(buf, msg.len());
            buf.extend_from_slice(msg.as_bytes());
            serialize_value(details, buf);
        }
        MettaValueInner::Type(inner) => {
            buf.push(TYPE);
            serialize_value(inner, buf);
        }
        MettaValueInner::Conjunction(goals) => {
            buf.push(CONJUNCTION);
            write_varint(buf, goals.len());
            for goal in goals.iter() {
                serialize_value(goal, buf);
            }
        }
        MettaValueInner::Empty => {
            buf.push(EMPTY);
        }
        MettaValueInner::Space(handle) => {
            buf.push(SPACE);
            buf.extend_from_slice(&handle.id.to_le_bytes());
            // Serialize name length and name bytes
            let name_bytes = handle.name.as_bytes();
            write_varint(buf, name_bytes.len());
            buf.extend_from_slice(name_bytes);
            // Serialize is_module_space flag
            buf.push(if handle.is_module_space() { 1 } else { 0 });
        }
        MettaValueInner::State(id) => {
            buf.push(STATE);
            buf.extend_from_slice(&id.to_le_bytes());
        }
        MettaValueInner::Quoted(inner) => {
            buf.push(QUOTED);
            serialize_value(inner, buf);
        }
        MettaValueInner::Memo(handle) => {
            buf.push(MEMO);
            buf.extend_from_slice(&handle.id.to_le_bytes());
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use super::super::gc_allocator::global_factory;
    use super::super::metta_value_trait::MettaValueFactory;

    // ========================================================================
    // Basic Constructor and Accessor Tests
    // ========================================================================

    #[test]
    fn test_arena_atom() {
        let factory = global_factory();
        let v = factory.atom("hello");
        assert!(v.is_atom());
        assert_eq!(v.as_atom(), Some("hello"));
    }

    #[test]
    fn test_arena_long() {
        let factory = global_factory();
        let v = factory.long(42);
        assert!(v.is_long());
        assert_eq!(v.as_long(), Some(42));
    }

    #[test]
    fn test_arena_bool() {
        let factory = global_factory();
        let v = factory.bool(true);
        assert!(v.is_bool());
        assert_eq!(v.as_bool(), Some(true));
    }

    #[test]
    fn test_arena_sexpr() {
        let factory = global_factory();
        let items = vec![
            factory.atom("+"),
            factory.long(1),
            factory.long(2),
        ];
        let v = factory.sexpr(items);
        assert!(v.is_sexpr());
        let items = v.as_sexpr().expect("should be sexpr");
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn test_copy_semantics() {
        let factory = global_factory();
        let v1 = factory.long(42);
        let v2 = v1; // Copy, not move
        let v3 = v1; // Can copy again
        assert_eq!(v1.as_long(), Some(42));
        assert_eq!(v2.as_long(), Some(42));
        assert_eq!(v3.as_long(), Some(42));
    }

    #[test]
    fn test_display() {
        let factory = global_factory();
        let v = factory.sexpr(vec![
            factory.atom("+"),
            factory.long(1),
            factory.long(2),
        ]);
        assert_eq!(format!("{}", v), "(+ 1 2)");
    }

    // ========================================================================
    // All Type Variant Constructor Tests (Phase 1)
    // ========================================================================

    #[test]
    fn test_arena_float() {
        let factory = global_factory();
        let v = factory.float(3.14);
        assert!(v.is_float());
        assert!(!v.is_long());
        assert_eq!(v.as_float(), Some(3.14));
        assert_eq!(v.as_long(), None);
    }

    #[test]
    fn test_arena_string() {
        let factory = global_factory();
        let v = factory.string("hello world");
        assert!(v.is_string());
        assert!(!v.is_atom());
        assert_eq!(v.as_string(), Some("hello world"));
        assert_eq!(v.as_atom(), None);
    }

    #[test]
    fn test_arena_nil() {
        let factory = global_factory();
        let v = factory.unit();
        // After Nil/Unit merge, nil() returns Unit
        assert!(v.is_unit());
        assert!(!v.is_empty());
    }

    #[test]
    fn test_arena_unit() {
        let factory = global_factory();
        let v = factory.unit();
        assert!(v.is_unit());
        assert!(!v.is_empty());
    }

    #[test]
    fn test_arena_empty() {
        let factory = global_factory();
        let v = factory.empty();
        assert!(v.is_empty());
        assert!(!v.is_unit());
    }

    #[test]
    fn test_arena_error() {
        let factory = global_factory();
        let details = factory.atom("details");
        let v = factory.error("test error", details);
        assert!(v.is_error());
        let (msg, det) = v.as_error().expect("should be error");
        assert_eq!(msg, "test error");
        assert_eq!(det.as_atom(), Some("details"));
    }

    #[test]
    fn test_arena_type() {
        let factory = global_factory();
        let inner = factory.atom("Number");
        let v = factory.type_value(inner);
        assert!(v.is_type());
        let t = v.as_type().expect("should be type");
        assert_eq!(t.as_atom(), Some("Number"));
    }

    #[test]
    fn test_arena_conjunction() {
        let factory = global_factory();
        let goals = vec![
            factory.atom("goal1"),
            factory.atom("goal2"),
        ];
        let v = factory.conjunction(goals);
        assert!(v.is_conjunction());
        let conj = v.as_conjunction().expect("should be conjunction");
        assert_eq!(conj.len(), 2);
    }

    #[test]
    fn test_state() {
        let factory = global_factory();
        let v = factory.state(12345);
        assert!(v.is_state());
        assert_eq!(v.as_state(), Some(12345));
    }

    #[test]
    fn test_arena_sexpr_empty() {
        let factory = global_factory();
        // Empty sexpr via factory.sexpr(vec![]) normalizes to Unit
        let v = factory.sexpr(vec![]);
        // After Nil/Unit merge + BumpVec->slice: empty sexpr normalizes to Unit
        assert!(v.is_unit());
    }

    // ========================================================================
    // Type Check Method Coverage
    // ========================================================================

    #[test]
    fn test_is_variable_true() {
        let factory = global_factory();
        let v = factory.atom("$x");
        assert!(v.is_variable());
    }

    #[test]
    fn test_is_variable_false() {
        let factory = global_factory();
        let v = factory.atom("foo");
        assert!(!v.is_variable());
    }

    #[test]
    fn test_is_ground_type() {
        let factory = global_factory();

        // Ground types
        assert!(factory.bool(true).is_ground_type());
        assert!(factory.long(42).is_ground_type());
        assert!(factory.float(3.14).is_ground_type());
        assert!(factory.string("hello").is_ground_type());

        // Non-ground types
        assert!(!factory.atom("foo").is_ground_type());
        assert!(!factory.sexpr(vec![]).is_ground_type());
        // After Nil/Unit merge, Unit/() is NOT a ground type (it's an expression in MeTTa HE)
        assert!(!factory.unit().is_ground_type());
        assert!(!factory.unit().is_ground_type());
    }

    // ========================================================================
    // PartialEq Tests for All Variant Combinations
    // ========================================================================

    #[test]
    fn test_eq_long_long() {
        let factory = global_factory();
        let v1 = factory.long(42);
        let v2 = factory.long(42);
        let v3 = factory.long(99);
        assert_eq!(v1, v2);
        assert_ne!(v1, v3);
    }

    #[test]
    fn test_eq_float_float() {
        let factory = global_factory();
        let v1 = factory.float(3.14);
        let v2 = factory.float(3.14);
        let v3 = factory.float(2.71);
        assert_eq!(v1, v2);
        assert_ne!(v1, v3);
    }

    #[test]
    fn test_eq_bool_bool() {
        let factory = global_factory();
        let t1 = factory.bool(true);
        let t2 = factory.bool(true);
        let f = factory.bool(false);
        assert_eq!(t1, t2);
        assert_ne!(t1, f);
    }

    #[test]
    fn test_eq_atom_atom() {
        let factory = global_factory();
        let v1 = factory.atom("foo");
        let v2 = factory.atom("foo");
        let v3 = factory.atom("bar");
        assert_eq!(v1, v2);
        assert_ne!(v1, v3);
    }

    #[test]
    fn test_eq_string_string() {
        let factory = global_factory();
        let v1 = factory.string("hello");
        let v2 = factory.string("hello");
        let v3 = factory.string("world");
        assert_eq!(v1, v2);
        assert_ne!(v1, v3);
    }

    #[test]
    fn test_eq_nil_nil() {
        let factory = global_factory();
        let v1 = factory.unit();
        let v2 = factory.unit();
        assert_eq!(v1, v2);
    }

    #[test]
    fn test_eq_unit_unit() {
        let factory = global_factory();
        let v1 = factory.unit();
        let v2 = factory.unit();
        assert_eq!(v1, v2);
    }

    #[test]
    fn test_eq_empty_empty() {
        let factory = global_factory();
        let v1 = factory.empty();
        let v2 = factory.empty();
        assert_eq!(v1, v2);
    }

    #[test]
    fn test_eq_sexpr_sexpr() {
        let factory = global_factory();
        let v1 = factory.sexpr(vec![
            factory.atom("+"),
            factory.long(1),
        ]);
        let v2 = factory.sexpr(vec![
            factory.atom("+"),
            factory.long(1),
        ]);
        let v3 = factory.sexpr(vec![
            factory.atom("+"),
            factory.long(2),
        ]);
        assert_eq!(v1, v2);
        assert_ne!(v1, v3);
    }

    #[test]
    fn test_eq_error_error() {
        let factory = global_factory();
        let d1 = factory.atom("d");
        let d2 = factory.atom("d");
        let v1 = factory.error("err", d1);
        let v2 = factory.error("err", d2);
        assert_eq!(v1, v2);
    }

    #[test]
    fn test_eq_type_type() {
        let factory = global_factory();
        let i1 = factory.atom("Number");
        let i2 = factory.atom("Number");
        let v1 = factory.type_value(i1);
        let v2 = factory.type_value(i2);
        assert_eq!(v1, v2);
    }

    #[test]
    fn test_eq_conjunction_conjunction() {
        let factory = global_factory();
        let v1 = factory.conjunction(vec![
            factory.atom("a"),
        ]);
        let v2 = factory.conjunction(vec![
            factory.atom("a"),
        ]);
        assert_eq!(v1, v2);
    }

    #[test]
    fn test_eq_state_state() {
        let factory = global_factory();
        let v1 = factory.state(100);
        let v2 = factory.state(100);
        let v3 = factory.state(200);
        assert_eq!(v1, v2);
        assert_ne!(v1, v3);
    }

    #[test]
    fn test_ne_different_types() {
        let factory = global_factory();
        let long = factory.long(42);
        let float = factory.float(42.0);
        let bool_val = factory.bool(true);
        let atom = factory.atom("42");
        let string = factory.string("42");

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
        let factory = global_factory();
        let v = factory.long(42);
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
        let factory = global_factory();
        let v = factory.atom("$x");
        assert_eq!(v.type_name(), "Variable");
    }

    #[test]
    fn test_type_name_symbol() {
        let factory = global_factory();
        let v = factory.atom("foo");
        assert_eq!(v.type_name(), "Symbol");
    }

    #[test]
    fn test_type_name_bool() {
        let factory = global_factory();
        let v = factory.bool(true);
        assert_eq!(v.type_name(), "Bool");
    }

    #[test]
    fn test_type_name_long() {
        let factory = global_factory();
        let v = factory.long(42);
        assert_eq!(v.type_name(), "Number");
    }

    #[test]
    fn test_type_name_float() {
        let factory = global_factory();
        let v = factory.float(3.14);
        assert_eq!(v.type_name(), "Number");
    }

    #[test]
    fn test_type_name_string() {
        let factory = global_factory();
        let v = factory.string("hello");
        assert_eq!(v.type_name(), "String");
    }

    #[test]
    fn test_type_name_sexpr() {
        let factory = global_factory();
        // Use a non-empty sexpr for the Expression type name test
        let v = factory.sexpr(vec![factory.long(1)]);
        assert_eq!(v.type_name(), "Expression");
    }

    #[test]
    fn test_type_name_nil() {
        // After Nil/Unit merge, nil() returns Unit which has type_name "Unit"
        let factory = global_factory();
        let v = factory.unit();
        assert_eq!(v.type_name(), "Unit");
    }

    #[test]
    fn test_type_name_error() {
        let factory = global_factory();
        let d = factory.unit();
        let v = factory.error("err", d);
        assert_eq!(v.type_name(), "Error");
    }

    #[test]
    fn test_type_name_type() {
        let factory = global_factory();
        let i = factory.atom("Int");
        let v = factory.type_value(i);
        assert_eq!(v.type_name(), "Type");
    }

    #[test]
    fn test_type_name_conjunction() {
        let factory = global_factory();
        let v = factory.conjunction(vec![]);
        assert_eq!(v.type_name(), "Conjunction");
    }

    #[test]
    fn test_type_name_state() {
        let factory = global_factory();
        let v = factory.state(1);
        assert_eq!(v.type_name(), "State");
    }

    #[test]
    fn test_type_name_unit() {
        let factory = global_factory();
        let v = factory.unit();
        assert_eq!(v.type_name(), "Unit");
    }

    #[test]
    fn test_type_name_empty() {
        let factory = global_factory();
        let v = factory.empty();
        assert_eq!(v.type_name(), "Empty");
    }

    // ========================================================================
    // Display Formatting Tests
    // ========================================================================

    #[test]
    fn test_display_bool_true() {
        let factory = global_factory();
        let v = factory.bool(true);
        assert_eq!(format!("{}", v), "True");
    }

    #[test]
    fn test_display_bool_false() {
        let factory = global_factory();
        let v = factory.bool(false);
        assert_eq!(format!("{}", v), "False");
    }

    #[test]
    fn test_display_long() {
        let factory = global_factory();
        let v = factory.long(-42);
        assert_eq!(format!("{}", v), "-42");
    }

    #[test]
    fn test_display_float() {
        let factory = global_factory();
        let v = factory.float(3.14);
        assert_eq!(format!("{}", v), "3.14");
    }

    #[test]
    fn test_display_string() {
        let factory = global_factory();
        let v = factory.string("hello");
        assert_eq!(format!("{}", v), "\"hello\"");
    }

    #[test]
    fn test_display_nil() {
        // After Nil/Unit merge, nil() returns Unit which displays as "()"
        let factory = global_factory();
        let v = factory.unit();
        assert_eq!(format!("{}", v), "()");
    }

    #[test]
    fn test_display_unit() {
        let factory = global_factory();
        let v = factory.unit();
        assert_eq!(format!("{}", v), "()");
    }

    #[test]
    fn test_display_empty() {
        let factory = global_factory();
        let v = factory.empty();
        assert_eq!(format!("{}", v), "Empty");
    }

    #[test]
    fn test_display_empty_sexpr() {
        let factory = global_factory();
        // Empty sexpr via factory normalizes to Unit
        let v = factory.sexpr(vec![]);
        assert_eq!(format!("{}", v), "()");
    }

    #[test]
    fn test_display_error() {
        let factory = global_factory();
        let d = factory.atom("details");
        let v = factory.error("msg", d);
        assert_eq!(format!("{}", v), "(Error msg details)");
    }

    #[test]
    fn test_display_type() {
        let factory = global_factory();
        let i = factory.atom("Int");
        let v = factory.type_value(i);
        assert_eq!(format!("{}", v), "(: Int)");
    }

    #[test]
    fn test_display_conjunction() {
        let factory = global_factory();
        let v = factory.conjunction(vec![
            factory.atom("a"),
            factory.atom("b"),
        ]);
        assert_eq!(format!("{}", v), "(, a b)");
    }

    #[test]
    fn test_display_state() {
        let factory = global_factory();
        let v = factory.state(123);
        assert_eq!(format!("{}", v), "<State:123>");
    }

    // ========================================================================
    // Serialization Round-Trip Tests
    // ========================================================================

    #[test]
    fn test_serialize_roundtrip_long() {
        let factory = global_factory();
        let original = factory.long(12345);
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_float() {
        let factory = global_factory();
        let original = factory.float(3.14159);
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_bool() {
        let factory = global_factory();
        let original = factory.bool(true);
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_atom() {
        let factory = global_factory();
        let original = factory.atom("hello-world");
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_string() {
        let factory = global_factory();
        let original = factory.string("test string");
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_nil() {
        let factory = global_factory();
        let original = factory.unit();
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_unit() {
        let factory = global_factory();
        let original = factory.unit();
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_empty() {
        let factory = global_factory();
        let original = factory.empty();
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_sexpr() {
        let factory = global_factory();
        let original = factory.sexpr(vec![
            factory.atom("+"),
            factory.long(1),
            factory.long(2),
        ]);
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_nested_sexpr() {
        let factory = global_factory();
        let inner = factory.sexpr(vec![
            factory.atom("*"),
            factory.long(2),
            factory.long(3),
        ]);
        let original = factory.sexpr(vec![
            factory.atom("+"),
            factory.long(1),
            inner,
        ]);
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_error() {
        let factory = global_factory();
        let details = factory.atom("details");
        let original = factory.error("test error", details);
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_type() {
        let factory = global_factory();
        let inner = factory.atom("Number");
        let original = factory.type_value(inner);
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_conjunction() {
        let factory = global_factory();
        let original = factory.conjunction(vec![
            factory.atom("a"),
            factory.atom("b"),
        ]);
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_serialize_roundtrip_state() {
        let factory = global_factory();
        let original = factory.state(999);
        let bytes = original.serialize();
        let (decoded, _) = factory.deserialize(&bytes).expect("deserialize");
        assert_eq!(original, decoded);
    }

    // ========================================================================
    // Hash Value Tests
    // ========================================================================

    #[test]
    fn test_hash_equal_values() {
        let factory = global_factory();
        let v1 = factory.long(42);
        let v2 = factory.long(42);
        assert_eq!(v1.hash_value(), v2.hash_value());
    }

    #[test]
    fn test_hash_different_values() {
        let factory = global_factory();
        let v1 = factory.long(42);
        let v2 = factory.long(43);
        assert_ne!(v1.hash_value(), v2.hash_value());
    }

    #[test]
    fn test_hash_different_types() {
        let factory = global_factory();
        let long = factory.long(42);
        let float = factory.float(42.0);
        // Different types should (almost certainly) have different hashes
        assert_ne!(long.hash_value(), float.hash_value());
    }

    #[test]
    fn test_hash_nil_unit_empty() {
        let factory = global_factory();
        let nil = factory.unit();
        let unit = factory.unit();
        let empty = factory.empty();
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
        let factory = global_factory();
        let v = factory.sexpr(vec![
            factory.atom("foo"),
            factory.long(1),
        ]);
        assert_eq!(v.get_head_symbol(), Some("foo"));
    }

    #[test]
    fn test_get_head_symbol_variable_head() {
        let factory = global_factory();
        let v = factory.sexpr(vec![
            factory.atom("$x"),
            factory.long(1),
        ]);
        // Variable as head returns None
        assert_eq!(v.get_head_symbol(), None);
    }

    #[test]
    fn test_get_head_symbol_bare_atom() {
        let factory = global_factory();
        let v = factory.atom("foo");
        assert_eq!(v.get_head_symbol(), Some("foo"));
    }

    #[test]
    fn test_get_arity_sexpr() {
        let factory = global_factory();
        let v = factory.sexpr(vec![
            factory.atom("foo"),
            factory.long(1),
            factory.long(2),
        ]);
        // Arity is len - 1 (excluding head)
        assert_eq!(v.get_arity(), 2);
    }

    #[test]
    fn test_get_arity_atom() {
        let factory = global_factory();
        let v = factory.atom("foo");
        assert_eq!(v.get_arity(), 0);
    }

    #[test]
    fn test_friendly_repr() {
        let factory = global_factory();
        let v = factory.sexpr(vec![
            factory.atom("+"),
            factory.long(1),
            factory.long(2),
        ]);
        assert_eq!(v.friendly_repr(), "(+ 1 2)");
    }

    #[test]
    fn test_friendly_type_name() {
        let factory = global_factory();
        assert_eq!(factory.long(1).friendly_type_name(), "Number (integer)");
        assert_eq!(factory.float(1.0).friendly_type_name(), "Number (float)");
        assert_eq!(factory.bool(true).friendly_type_name(), "Bool");
    }

    // ========================================================================
    // Factory Tests
    // ========================================================================

    #[test]
    fn test_factory_creates_values() {
        let factory = global_factory();

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
        let factory = global_factory();

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
        let factory = global_factory();

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
        let factory = global_factory();
        let result = factory.deserialize(&[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_unknown_tag() {
        let factory = global_factory();
        let result = factory.deserialize(&[0xFF]);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("unknown tag"));
    }

    #[test]
    fn test_deserialize_truncated_long() {
        let factory = global_factory();
        // LONG tag but only 4 bytes (needs 8)
        let result = factory.deserialize(&[0x03, 0x00, 0x00, 0x00, 0x00]);
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_truncated_float() {
        let factory = global_factory();
        // FLOAT tag but only 4 bytes (needs 8)
        let result = factory.deserialize(&[0x04, 0x00, 0x00, 0x00, 0x00]);
        assert!(result.is_err());
    }

    // ========================================================================
    // Pointer Equality Fast Path
    // ========================================================================

    #[test]
    fn test_pointer_equality_fast_path() {
        let factory = global_factory();
        let v = factory.long(42);
        let v_copy = v; // Copy, same pointer
        // Both should be equal via pointer comparison fast path
        assert_eq!(v, v_copy);
    }
}
