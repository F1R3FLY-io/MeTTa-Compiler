//! MettaValueTrait - Common Interface for MeTTa Values
//!
//! This module defines the `MettaValueTrait` trait which provides a unified interface
//! for MeTTa values regardless of their allocation strategy. Both heap-allocated
//! (`MettaValue`) and arena-allocated (`MettaValue`) types implement
//! this trait, enabling generic evaluation code.
//!
//! ## Design Principles
//!
//! 1. **Same variants, different allocation**: Both implementations have the same
//!    value variants (Atom, Bool, Long, SExpr, etc.) with identical semantics.
//!
//! 2. **Accessors hide representation**: Methods like `as_atom() -> Option<&str>`
//!    work uniformly regardless of whether the underlying storage is `String` or `&str`.
//!
//! 3. **Separate Factory trait**: Construction needs different signatures
//!    (heap needs no context, arena needs `&Bump`), so it's a separate trait.
//!
//! 4. **Zero-cost abstractions**: All trait methods are `#[inline]`, enabling
//!    monomorphization and direct field access after inlining.

use std::fmt::Debug;

use super::{MemoHandle, MettaValue, MettaValueInner, SpaceHandle, ValueView};

/// Core trait for MeTTa values.
///
/// Both heap and arena implementations provide the same interface,
/// differing only in allocation strategy. This trait enables writing
/// generic evaluation code that works with either allocation mode.
///
/// # Type Checking Methods
///
/// These methods check the variant type of the value:
/// - `is_atom()`, `is_bool()`, `is_long()`, etc.
///
/// # Accessor Methods
///
/// These methods extract the inner value if the variant matches:
/// - `as_atom() -> Option<&str>`, `as_bool() -> Option<bool>`, etc.
///
/// # Example
///
/// ```ignore
/// fn process_value<V: MettaValueTrait>(value: &V) {
///     if let Some(name) = value.as_atom() {
///         if name.starts_with('$') {
///             // Handle variable
///         }
///     } else if let Some(items) = value.as_sexpr() {
///         // Handle S-expression
///     }
/// }
/// ```
pub trait MettaValueTrait: Clone + Debug + PartialEq + Sized {
    /// The slice type for S-expressions.
    /// For MettaValue this is `[Self]`, for MettaValue it's `[Self]` as well.
    type SExprSlice: AsRef<[Self]> + ?Sized;

    // =========================================================================
    // Type checking methods
    // =========================================================================

    /// Check if this is an Atom variant
    fn is_atom(&self) -> bool;

    /// Check if this is a Bool variant
    fn is_bool(&self) -> bool;

    /// Check if this is a Long variant
    fn is_long(&self) -> bool;

    /// Check if this is a Float variant
    fn is_float(&self) -> bool;

    /// Check if this is a String variant
    fn is_string(&self) -> bool;

    /// Check if this is an SExpr variant
    fn is_sexpr(&self) -> bool;

    /// Check if this is an Error variant
    fn is_error(&self) -> bool;

    /// Check if this is a Type variant
    fn is_type(&self) -> bool;

    /// Check if this is a Conjunction variant
    fn is_conjunction(&self) -> bool;

    /// Check if this is a Space variant
    fn is_space(&self) -> bool;

    /// Check if this is a State variant
    fn is_state(&self) -> bool;

    /// Check if this is a Unit variant
    fn is_unit(&self) -> bool;


    /// Check if this is a Memo variant
    fn is_memo(&self) -> bool;

    /// Check if this is a Quoted variant
    fn is_quoted(&self) -> bool;

    /// Check if this is an Empty variant
    fn is_empty(&self) -> bool;

    /// Check if this value has a Spanned wrapper (carries source location)
    fn is_spanned(&self) -> bool;

    /// Get the outermost source span if this value is Spanned
    fn span(&self) -> Option<&'static crate::ir::Span>;

    /// Strip one layer of Spanned wrapper, returning the inner value.
    /// Returns self unchanged if not Spanned.
    fn strip_one_span(&self) -> Self;

    /// Check if this value is a variable (Atom starting with $)
    fn is_variable(&self) -> bool;

    /// Check if this value is a ground type (non-reducible literal)
    /// Ground types: Bool, Long, Float, String, Nil
    fn is_ground_type(&self) -> bool;

    // =========================================================================
    // Accessor methods
    // =========================================================================

    /// Try to extract as atom string.
    /// Returns `&'static str` because atom strings are slab-allocated with static lifetime.
    fn as_atom(&self) -> Option<&'static str>;

    /// Try to extract as bool
    fn as_bool(&self) -> Option<bool>;

    /// Try to extract as i64
    fn as_long(&self) -> Option<i64>;

    /// Try to extract as f64
    fn as_float(&self) -> Option<f64>;

    /// Try to extract as string
    fn as_string(&self) -> Option<&str>;

    /// Try to extract as sexpr items slice
    fn as_sexpr(&self) -> Option<&[Self]>;

    /// Try to extract as error (message, details)
    fn as_error(&self) -> Option<(&str, &Self)>;

    /// Try to extract as type inner value
    fn as_type(&self) -> Option<&Self>;

    /// Try to extract as conjunction goals
    fn as_conjunction(&self) -> Option<&[Self]>;

    /// Try to extract as space handle
    fn as_space(&self) -> Option<&SpaceHandle>;

    /// Try to extract as state id
    fn as_state(&self) -> Option<u64>;

    /// Try to extract as memo handle
    fn as_memo(&self) -> Option<&MemoHandle>;

    /// Try to extract the inner value of a Quoted variant (owned copy)
    fn as_quoted(&self) -> Option<Self>;

    /// Try to extract a reference to the inner value of a Quoted variant.
    /// Returns a reference with the same lifetime as `self`, unlike `as_quoted()`
    /// which returns an owned copy. Needed for generic functions with lifetime
    /// constraints (e.g., alpha_equiv with shared HashMap entries).
    fn as_quoted_ref(&self) -> Option<&Self>;

    // =========================================================================
    // Utility methods
    // =========================================================================

    /// Get the type name of this value as a string slice
    ///
    /// Returns the MeTTa type name for this value variant.
    fn type_name(&self) -> &'static str;

    /// Convert MettaValue to a friendly type name for error messages
    fn friendly_type_name(&self) -> &'static str;

    /// Extract the head symbol from a pattern for indexing.
    /// Returns None if the pattern doesn't have a clear head symbol.
    fn get_head_symbol(&self) -> Option<&str>;

    /// Get the arity (number of arguments) for an s-expression.
    /// For (head arg1 arg2 arg3), arity is 3.
    /// For bare atoms, arity is 0.
    fn get_arity(&self) -> usize;

    /// Access the raw inner enum for pattern matching.
    ///
    /// Returns a reference to the underlying `MettaValueInner` without
    /// stripping `Spanned` layers. Callers must handle `Spanned` explicitly.
    fn inner_raw(&self) -> &MettaValueInner;

    /// Unified dispatch view for pattern matching.
    ///
    /// Returns a `ValueView` with one variant per logical type.
    /// Spanned layers are stripped automatically.
    ///
    /// The default implementation dispatches on `inner_raw()` with Spanned
    /// stripping. Concrete types may override for optimized decode paths.
    fn view(&self) -> ValueView {
        let mut inner = self.inner_raw();
        // Strip Spanned layers (mirrors MettaValue::inner() behavior)
        loop {
            match inner {
                MettaValueInner::Spanned(wrapped, _) => inner = wrapped.inner_raw(),
                _ => break,
            }
        }
        match inner {
            MettaValueInner::Float(f) => ValueView::Float(*f),
            MettaValueInner::Bool(b) => ValueView::Bool(*b),
            MettaValueInner::Long(n) => ValueView::Long(*n),
            MettaValueInner::Unit => ValueView::Unit,
            MettaValueInner::Empty => ValueView::Empty,
            MettaValueInner::Atom(s) => ValueView::Atom(s),
            MettaValueInner::String(s) => ValueView::String(s),
            // SAFETY: All MettaValueInner references are slab-allocated with
            // 'static lifetime. The trait's inner_raw() uses an anonymous
            // lifetime tied to &self, but the actual data outlives all callers.
            MettaValueInner::SExpr(items) => {
                ValueView::SExpr(unsafe { &*((*items) as *const [MettaValue]) })
            }
            MettaValueInner::Error(msg, details) => ValueView::Error(msg, *details),
            MettaValueInner::Type(inner_val) => ValueView::Type(*inner_val),
            MettaValueInner::Conjunction(goals) => {
                ValueView::Conjunction(unsafe { &*((*goals) as *const [MettaValue]) })
            }
            MettaValueInner::Space(handle) => {
                ValueView::Space(unsafe { &*(handle as *const SpaceHandle) })
            }
            MettaValueInner::State(id) => ValueView::State(*id),
            MettaValueInner::Memo(handle) => {
                ValueView::Memo(unsafe { &*(handle as *const MemoHandle) })
            }
            MettaValueInner::Quoted(inner_val) => ValueView::Quoted(*inner_val),
            MettaValueInner::Spanned(..) => unreachable!("Spanned stripped above"),
        }
    }

    /// Get a raw pointer to the slab-allocated inner representation.
    ///
    /// This pointer is valid for 'static (managed by the GC) and can be
    /// stored in NaN-boxed JitValues without risk of dangling.
    fn inner_ptr(&self) -> *const MettaValueInner;

    /// Reconstruct a value from a raw pointer to its inner representation.
    ///
    /// # Safety
    /// The pointer must point to valid, slab-allocated `MettaValueInner` data
    /// with 'static lifetime (managed by the GC).
    unsafe fn from_inner_ptr(ptr: *const MettaValueInner) -> Self;

    /// Get a user-friendly string representation of this value.
    ///
    /// This is used for error messages, debug output, and `repr` operations.
    /// Unlike `Debug::fmt`, this produces human-readable output suitable for
    /// display to users.
    ///
    /// # Example Output
    ///
    /// - Atoms: `foo`, `$x`
    /// - Numbers: `42`, `3.14`
    /// - Strings: `"hello"`
    /// - Booleans: `True`, `False`
    /// - S-expressions: `(+ 1 2)`
    /// - Errors: `(Error "message" details)`
    fn friendly_repr(&self) -> String;

    /// Convert to a display string for println output.
    ///
    /// This is similar to `friendly_repr()` but prints strings WITHOUT quotes,
    /// making it suitable for user-facing output like println!.
    ///
    /// # Example Output
    ///
    /// - Atoms: `foo`, `$x`
    /// - Numbers: `42`, `3.14`
    /// - Strings: `hello` (no quotes)
    /// - Booleans: `True`, `False`
    /// - S-expressions: `(+ 1 2)`
    fn to_display_string(&self) -> String;

    // =========================================================================
    // Serialization
    // =========================================================================

    /// Serialize this value to bytes for storage in PathMap/MORK.
    ///
    /// The byte format is context-independent - both MettaValue and MettaValue
    /// serialize to the same format.
    ///
    /// # Format
    ///
    /// Uses a tag byte followed by the payload:
    /// - 0x01 Atom: length (varint) + UTF-8 bytes
    /// - 0x02 Bool: 0x00 (false) or 0x01 (true)
    /// - 0x03 Long: i64 (little-endian)
    /// - 0x04 Float: f64 (little-endian)
    /// - 0x05 String: length (varint) + UTF-8 bytes
    /// - 0x06 SExpr: count (varint) + serialized children
    /// - 0x07 Nil
    /// - 0x08 Error: msg_len (varint) + msg + serialized details
    /// - 0x09 Type: serialized inner
    /// - 0x0A Conjunction: count (varint) + serialized goals
    /// - 0x0B Unit
    /// - 0x0C Empty
    /// - 0x0D Space: space_id (u64 little-endian)
    /// - 0x0E State: state_id (u64 little-endian)
    /// - 0x0F Memo: memo_id (u64 little-endian)
    fn serialize(&self) -> Vec<u8>;

    // =========================================================================
    // Hashing
    // =========================================================================

    /// Compute a u64 hash of this value.
    ///
    /// This avoids requiring `Hash` as a supertrait (which causes orphan rule
    /// issues with arena types) while still enabling hash-based caches like
    /// `GenericMemoCache<V>`.
    ///
    /// Implementations should produce consistent hashes for structurally equal
    /// values, matching the semantics of `PartialEq`.
    fn hash_value(&self) -> u64;

    // =========================================================================
    // Variable detection
    // =========================================================================

    /// Check if this value contains any variables (`$x`, `&y`, `'z`, or `_`).
    ///
    /// O(1) flag check: does this value (or any sub-value) contain variables?
    ///
    /// Default implementation falls back to the O(depth) `contains_variables()` tree walk.
    /// Concrete types with tagged pointer flags (e.g., `MettaValue`) should override this
    /// for true O(1) behavior via a single bitwise AND instruction.
    #[inline]
    fn has_variables_fast(&self) -> bool {
        self.contains_variables()
    }

    /// Collect the set of free variable names referenced in this value.
    ///
    /// Returns the names of all variables (`$x`, `&y`, `'z`) that appear in
    /// the expression. Wildcards (`_`) are excluded since they don't bind.
    /// Space references (`&self`, `&kb`, `&stack`) are also excluded.
    ///
    /// Used for environment trimming (Phase 2.4): bindings not referenced
    /// by the body expression can be removed before evaluation.
    ///
    /// # Performance
    ///
    /// O(n) in expression size. For typical PLN bodies (5-20 subexpressions),
    /// this is ~10-50ns. Short-circuits on ground values via `has_variables_fast()`.
    fn free_variables(&self) -> smallvec::SmallVec<[&'static str; 8]> {
        let mut vars = smallvec::SmallVec::new();
        self.collect_free_variables(&mut vars);
        vars
    }

    /// Collect free variable names into the provided buffer.
    ///
    /// Helper for `free_variables()`. Avoids allocating intermediate SmallVecs
    /// during recursive traversal.
    fn collect_free_variables(&self, out: &mut smallvec::SmallVec<[&'static str; 8]>) {
        // Fast path: no variables in this value
        if !self.has_variables_fast() {
            return;
        }

        if let Some(name) = self.as_atom() {
            // Variables start with $, &, or ' (excluding special space refs and wildcards)
            if name != "_"
                && name != "&"
                && name != "&self"
                && name != "&kb"
                && name != "&stack"
                && name.len() > 1
                && (name.starts_with('$') || name.starts_with('&') || name.starts_with('\''))
            {
                // Deduplicate: only add if not already present
                if !out.contains(&name) {
                    out.push(name);
                }
            }
            return;
        }

        if let Some(items) = self.as_sexpr() {
            for item in items {
                item.collect_free_variables(out);
            }
            return;
        }

        if let Some(goals) = self.as_conjunction() {
            for g in goals {
                g.collect_free_variables(out);
            }
            return;
        }

        if let Some((_, details)) = self.as_error() {
            details.collect_free_variables(out);
            return;
        }

        if let Some(t) = self.as_type() {
            t.collect_free_variables(out);
            return;
        }

        if let Some(q) = self.as_quoted_ref() {
            q.collect_free_variables(out);
        }
    }

    /// Space references (`&self`, `&kb`, `&stack`) are NOT variables.
    /// Ground types (Bool, Long, Float, String, Unit, Space, State, Memo, Empty)
    /// never contain variables.
    ///
    /// Used by `apply_bindings_generic_inner` to short-circuit recursion and
    /// avoid allocating new S-expressions for variable-free values.
    fn contains_variables(&self) -> bool {
        if let Some(s) = self.as_atom() {
            if s == "&" || s == "&self" || s == "&kb" || s == "&stack" {
                return false;
            }
            return s == "_" || s.starts_with('$') || s.starts_with('&') || s.starts_with('\'');
        }
        if let Some(items) = self.as_sexpr() {
            return items.iter().any(|item| item.contains_variables());
        }
        if let Some(goals) = self.as_conjunction() {
            return goals.iter().any(|g| g.contains_variables());
        }
        if let Some((_, details)) = self.as_error() {
            return details.contains_variables();
        }
        if let Some(t) = self.as_type() {
            return t.contains_variables();
        }
        if let Some(q) = self.as_quoted_ref() {
            return q.contains_variables();
        }
        false // Ground types: Bool, Long, Float, String, Unit, Space, State, Memo, Empty
    }

    // =========================================================================
    // Comparison methods
    // =========================================================================

    /// Check if two values are structurally equivalent.
    ///
    /// Structural equivalence treats all variables as equivalent to each other
    /// (regardless of name), but requires exact match for atoms, ground types,
    /// and space references. This is used for rule deduplication.
    ///
    /// # Semantics
    ///
    /// - Variables (`$x`, `&x`, `'x`) are equivalent to each other
    /// - Space references (`&self`, `&kb`, `&stack`) must match exactly
    /// - Wildcards (`_`) match only other wildcards
    /// - Non-variable atoms must match exactly
    /// - Ground types (Bool, Long, Float, String, Nil) must match exactly
    /// - S-expressions must have same length and pairwise equivalent elements
    /// - Errors must have same message and equivalent details
    /// - Types/Conjunctions must have structurally equivalent children
    ///
    /// # Zero-Conversion
    ///
    /// This method uses only `MettaValueTrait` accessors, enabling comparison
    /// without converting to a common heap representation.
    fn structurally_equivalent(&self, other: &Self) -> bool {
        // Helper to check if an atom is a variable (not a space reference)
        fn is_variable(s: &str) -> bool {
            if s == "&" || s == "&self" || s == "&kb" || s == "&stack" {
                return false; // Space references are NOT variables
            }
            s.starts_with('$') || s.starts_with('&') || s.starts_with('\'')
        }

        // Both atoms?
        if let (Some(a), Some(b)) = (self.as_atom(), other.as_atom()) {
            // Variables match any other variable
            if is_variable(a) && is_variable(b) {
                return true;
            }
            // Wildcards match wildcards
            if a == "_" && b == "_" {
                return true;
            }
            // Non-variable atoms must match exactly
            return a == b;
        }

        // Both bools?
        if let (Some(a), Some(b)) = (self.as_bool(), other.as_bool()) {
            return a == b;
        }

        // Both longs?
        if let (Some(a), Some(b)) = (self.as_long(), other.as_long()) {
            return a == b;
        }

        // Both floats?
        if let (Some(a), Some(b)) = (self.as_float(), other.as_float()) {
            return a == b;
        }

        // Both strings?
        if let (Some(a), Some(b)) = (self.as_string(), other.as_string()) {
            return a == b;
        }

        // Both s-expressions?
        if let (Some(a_items), Some(b_items)) = (self.as_sexpr(), other.as_sexpr()) {
            if a_items.len() != b_items.len() {
                return false;
            }
            return a_items
                .iter()
                .zip(b_items.iter())
                .all(|(a, b)| a.structurally_equivalent(b));
        }

        // Both errors?
        if let (Some((a_msg, a_details)), Some((b_msg, b_details))) =
            (self.as_error(), other.as_error())
        {
            return a_msg == b_msg && a_details.structurally_equivalent(b_details);
        }

        // Both types?
        if let (Some(a), Some(b)) = (self.as_type(), other.as_type()) {
            return a.structurally_equivalent(b);
        }

        // Both conjunctions?
        if let (Some(a_goals), Some(b_goals)) = (self.as_conjunction(), other.as_conjunction()) {
            if a_goals.len() != b_goals.len() {
                return false;
            }
            return a_goals
                .iter()
                .zip(b_goals.iter())
                .all(|(a, b)| a.structurally_equivalent(b));
        }

        // Both spaces?
        if let (Some(a), Some(b)) = (self.as_space(), other.as_space()) {
            return a.id == b.id;
        }

        // Both states?
        if let (Some(a), Some(b)) = (self.as_state(), other.as_state()) {
            return a == b;
        }

        // Both unit?
        if self.is_unit() && other.is_unit() {
            return true;
        }

        // Both empty?
        if self.is_empty() && other.is_empty() {
            return true;
        }

        // Both unit?
        if self.is_unit() && other.is_unit() {
            return true;
        }

        // Different types = not equivalent
        false
    }
}

/// Trait for constructing MettaValue instances.
///
/// This is separated from `MettaValueTrait` because construction signatures differ:
/// - `GcFactory`: backed by the global slab allocator (implements Default)
/// - `MettaValueFactory`: requires arena reference (deprecated)
///
/// # Example
///
/// ```ignore
/// fn create_error<V: MettaValueTrait, F: MettaValueFactory<V>>(
///     factory: &F,
///     msg: &str,
///     detail: V,
/// ) -> V {
///     factory.error(msg, detail)
/// }
/// ```
pub trait MettaValueFactory<V: MettaValueTrait> {
    /// Create an Atom variant from a string slice
    fn atom(&self, s: &str) -> V;

    /// Create a Bool variant
    fn bool(&self, b: bool) -> V;

    /// Create a Long variant
    fn long(&self, n: i64) -> V;

    /// Create a Float variant
    fn float(&self, f: f64) -> V;

    /// Create a String variant from a string slice
    fn string(&self, s: &str) -> V;

    /// Create an SExpr variant from a vector of values
    fn sexpr(&self, items: Vec<V>) -> V;

    /// Create an SExpr variant from a slice of values
    fn sexpr_from_slice(&self, items: &[V]) -> V;

    /// Create an Error variant
    fn error(&self, msg: &str, details: V) -> V;

    /// Create a Type variant
    fn type_value(&self, inner: V) -> V;

    /// Create a Conjunction variant
    fn conjunction(&self, goals: Vec<V>) -> V;

    /// Create a Space variant
    fn space(&self, handle: SpaceHandle) -> V;

    /// Create a State variant
    fn state(&self, id: u64) -> V;

    /// Create a Unit variant
    fn unit(&self) -> V;

    /// Create a Memo variant
    fn memo(&self, handle: MemoHandle) -> V;

    /// Create an Empty variant
    fn empty(&self) -> V;

    // =========================================================================
    // Convenience methods with default implementations
    // =========================================================================

    /// Create a symbol atom from a string slice
    #[inline]
    fn sym(&self, s: &str) -> V {
        self.atom(s)
    }

    /// Create a variable atom (prefixed with $)
    #[inline]
    fn var(&self, name: &str) -> V {
        self.atom(&format!("${}", name))
    }

    /// Create a quoted expression using the Quoted variant.
    fn quote(&self, inner: V) -> V;

    /// Create a Spanned variant wrapping a value with a source location.
    ///
    /// The span is allocated in the slab and has `'static` lifetime.
    /// This is used during compilation to attach source positions to values.
    fn spanned(&self, value: V, span: crate::ir::Span) -> V;

    // =========================================================================
    // Conversion from MettaValue
    // =========================================================================

    /// Convert from `MettaValue` to `V`.
    ///
    /// When `V = MettaValue` (the production path via GcFactory), this is a
    /// zero-cost identity — no serialization or deserialization occurs.
    /// For other value types, this falls back to serialize/deserialize.
    fn from_metta_value(&self, value: MettaValue) -> V {
        // Default implementation: serialize/deserialize round-trip.
        // Overridden to identity by GcFactory where V = MettaValue.
        let bytes = value.serialize();
        match self.deserialize(&bytes) {
            Ok((v, _)) => v,
            Err(_) => self.atom("?conversion_error?"),
        }
    }

    // =========================================================================
    // Deserialization
    // =========================================================================

    /// Deserialize a value from bytes.
    ///
    /// This is the inverse of `MettaValue::serialize()`. The factory allocates
    /// the value in its native context:
    /// - GcFactory: allocates directly in global slab allocator
    /// - GcFactory::default(): convenient construction via Default
    ///
    /// # Returns
    ///
    /// Returns `Ok((value, bytes_consumed))` on success, or `Err(msg)` on failure.
    fn deserialize(&self, bytes: &[u8]) -> Result<(V, usize), String>;
}

/// Blanket implementation for references to factories.
///
/// This enables calling factory methods through references like `ctx.factory().atom("x")`
/// where `ctx.factory()` returns `&Self::Factory`. The implementation simply delegates
/// to the underlying factory.
impl<V: MettaValueTrait, F: MettaValueFactory<V>> MettaValueFactory<V> for &F {
    #[inline]
    fn atom(&self, s: &str) -> V {
        (*self).atom(s)
    }

    #[inline]
    fn bool(&self, b: bool) -> V {
        (*self).bool(b)
    }

    #[inline]
    fn long(&self, n: i64) -> V {
        (*self).long(n)
    }

    #[inline]
    fn float(&self, f: f64) -> V {
        (*self).float(f)
    }

    #[inline]
    fn string(&self, s: &str) -> V {
        (*self).string(s)
    }

    #[inline]
    fn sexpr(&self, items: Vec<V>) -> V {
        (*self).sexpr(items)
    }

    #[inline]
    fn sexpr_from_slice(&self, items: &[V]) -> V {
        (*self).sexpr_from_slice(items)
    }

    #[inline]
    fn error(&self, msg: &str, details: V) -> V {
        (*self).error(msg, details)
    }

    #[inline]
    fn type_value(&self, inner: V) -> V {
        (*self).type_value(inner)
    }

    #[inline]
    fn conjunction(&self, goals: Vec<V>) -> V {
        (*self).conjunction(goals)
    }

    #[inline]
    fn space(&self, handle: SpaceHandle) -> V {
        (*self).space(handle)
    }

    #[inline]
    fn state(&self, id: u64) -> V {
        (*self).state(id)
    }

    #[inline]
    fn unit(&self) -> V {
        (*self).unit()
    }

    #[inline]
    fn memo(&self, handle: MemoHandle) -> V {
        (*self).memo(handle)
    }

    #[inline]
    fn quote(&self, inner: V) -> V {
        (*self).quote(inner)
    }

    #[inline]
    fn spanned(&self, value: V, span: crate::ir::Span) -> V {
        (*self).spanned(value, span)
    }

    #[inline]
    fn empty(&self) -> V {
        (*self).empty()
    }

    #[inline]
    fn from_metta_value(&self, value: MettaValue) -> V {
        (*self).from_metta_value(value)
    }

    #[inline]
    fn deserialize(&self, bytes: &[u8]) -> Result<(V, usize), String> {
        (*self).deserialize(bytes)
    }
}

#[cfg(test)]
mod tests {
    use crate::backend::models::{GcFactory, MettaValueFactory};
    use super::MettaValueTrait;

    #[test]
    fn test_structurally_equivalent_atoms() {
        let factory = GcFactory::default();

        // Non-variable atoms must match exactly
        let a = factory.atom("foo");
        let b = factory.atom("foo");
        let c = factory.atom("bar");
        assert!(a.structurally_equivalent(&b));
        assert!(!a.structurally_equivalent(&c));
    }

    #[test]
    fn test_structurally_equivalent_variables() {
        let factory = GcFactory::default();

        // Variables are equivalent regardless of name
        let x = factory.atom("$x");
        let y = factory.atom("$y");
        let z = factory.atom("&z");
        let w = factory.atom("'w");

        assert!(x.structurally_equivalent(&y));
        assert!(x.structurally_equivalent(&z));
        assert!(x.structurally_equivalent(&w));
    }

    #[test]
    fn test_structurally_equivalent_space_refs() {
        let factory = GcFactory::default();

        // Space references are NOT variables and must match exactly
        let self1 = factory.atom("&self");
        let self2 = factory.atom("&self");
        let kb = factory.atom("&kb");
        let var = factory.atom("$x");

        assert!(self1.structurally_equivalent(&self2));
        assert!(!self1.structurally_equivalent(&kb));
        assert!(!self1.structurally_equivalent(&var));
    }

    #[test]
    fn test_structurally_equivalent_sexpr() {
        let factory = GcFactory::default();

        // S-expressions must have same structure
        let e1 = factory.sexpr(vec![
            factory.atom("foo"),
            factory.atom("$x"),
            factory.long(42),
        ]);
        let e2 = factory.sexpr(vec![
            factory.atom("foo"),
            factory.atom("$y"), // Different variable name, but equivalent
            factory.long(42),
        ]);
        let e3 = factory.sexpr(vec![
            factory.atom("foo"),
            factory.atom("bar"), // Non-variable, different
            factory.long(42),
        ]);

        assert!(e1.structurally_equivalent(&e2));
        assert!(!e1.structurally_equivalent(&e3));
    }
}
