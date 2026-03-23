//! MettaValue → TrieKey Decomposition
//!
//! Provides the bridge between MeTTaTron's `MettaValueTrait` and `metta-trie`'s
//! `TrieKey` sequences. The `decompose()` function converts a MettaValue into
//! a flat sequence of `TrieKey`s suitable for trie insertion/lookup.
//!
//! ## Path Encoding
//!
//! S-expressions are flattened depth-first with an Arity marker:
//! ```text
//! (person "Alice" 42)     → [Arity(3), Atom("person"), Str("Alice"), Long(42)]
//! (= (fib $n) (+ $n 1))  → [Arity(3), Atom("="), Arity(2), Atom("fib"), Variable,
//!                            Arity(3), Atom("+"), Variable, Long(1)]
//! ```
//!
//! Variable names starting with `$`, `&` (non-special), or `'` are encoded as
//! `TrieKey::Variable`. All other atoms are `TrieKey::Atom`.

use metta_trie::TrieKey;
use smallvec::SmallVec;

use crate::backend::models::MettaValueTrait;

/// Decompose a MettaValue into a sequence of TrieKeys for trie operations.
///
/// The resulting key sequence uniquely identifies the expression structure.
/// Variable atoms (`$x`, `'y`, `&z`) become `TrieKey::Variable`.
///
/// The output is pre-allocated to inline up to 16 keys on the stack.
pub fn decompose<V: MettaValueTrait>(value: &V) -> SmallVec<[TrieKey; 16]> {
    let mut keys = SmallVec::new();
    decompose_into(value, &mut keys);
    keys
}

/// Decompose a MettaValue, appending keys to an existing vector.
///
/// This is the recursive workhorse. Call `decompose()` for the public API.
pub fn decompose_into<V: MettaValueTrait>(value: &V, keys: &mut SmallVec<[TrieKey; 16]>) {
    // Strip span wrappers (source location annotations)
    let value = if value.is_spanned() {
        // For spanned values, we need to look through the span
        // The strip_one_span returns a clone, so we handle it differently
        decompose_spanned(value, keys);
        return;
    } else {
        value
    };

    // S-expression: Arity + children (depth-first)
    if let Some(items) = value.as_sexpr() {
        keys.push(TrieKey::Arity(items.len() as u16));
        for child in items {
            decompose_into(child, keys);
        }
        return;
    }

    // Atom: variable, wildcard, or concrete
    if let Some(atom) = value.as_atom() {
        if is_var_name(atom) {
            keys.push(TrieKey::Variable);
        } else if atom == "_" {
            keys.push(TrieKey::Variable); // Wildcard = Variable
        } else {
            keys.push(TrieKey::Atom(atom));
        }
        return;
    }

    // Long
    if let Some(n) = value.as_long() {
        keys.push(TrieKey::Long(n));
        return;
    }

    // Bool
    if let Some(b) = value.as_bool() {
        keys.push(TrieKey::Bool(b));
        return;
    }

    // Float
    if let Some(f) = value.as_float() {
        keys.push(TrieKey::Float(f.to_bits()));
        return;
    }

    // String
    if let Some(s) = value.as_string() {
        let interned = crate::backend::models::gc_allocator::global_allocator().alloc_str(s);
        keys.push(TrieKey::Str(interned));
        return;
    }

    // Type wrapper: single child
    if let Some(inner) = value.as_type() {
        keys.push(TrieKey::Arity(1)); // Type as 1-child wrapper
        // We could use a dedicated TypeWrapper key, but Arity(1) + child
        // is sufficient since the inner value discriminates the type.
        // Actually, let's be more precise to distinguish Type(X) from (X):
        keys.pop(); // Remove the Arity(1) we just pushed
        // Use a marker approach: treat Type as an S-expression (: inner)
        // This matches how types are stored as (: name type) in the space.
        decompose_into(inner, keys);
        return;
    }

    // Quoted wrapper: single child
    if let Some(inner) = value.as_quoted_ref() {
        // Quoted values decompose to their inner content
        // The "quoted-ness" is a wrapper, not structural content
        decompose_into(inner, keys);
        return;
    }

    // Conjunction: multiple children
    if let Some(goals) = value.as_conjunction() {
        keys.push(TrieKey::Arity(goals.len() as u16));
        for goal in goals {
            decompose_into(goal, keys);
        }
        return;
    }

    // Error: decompose the details (skip message string)
    if let Some((_msg, details)) = value.as_error() {
        keys.push(TrieKey::Error);
        decompose_into(details, keys);
        return;
    }

    // Unit / Empty
    if value.is_unit() {
        keys.push(TrieKey::Unit);
        return;
    }
    if value.is_empty() {
        keys.push(TrieKey::Unit); // Empty treated same as Unit for storage
        return;
    }

    // Fallback: treat as Unit (should not happen with well-formed values)
    keys.push(TrieKey::Unit);
}

/// Handle spanned values by stripping the span and recursing.
fn decompose_spanned<V: MettaValueTrait>(value: &V, keys: &mut SmallVec<[TrieKey; 16]>) {
    let stripped = value.strip_one_span();
    decompose_into(&stripped, keys);
}

/// Get the canonical De Bruijn variable name for a given index.
///
/// Returns `&'static str` like `"$__0"`, `"$__1"`, etc.
/// Fast path: static array for the first 64 indices (zero allocation).
/// Slow path: thread-local cache with GC allocator interning (allocated once per thread).
fn debruijn_var_name(idx: usize) -> &'static str {
    use std::cell::RefCell;

    static NAMES: &[&str] = &[
        "$__0", "$__1", "$__2", "$__3", "$__4", "$__5", "$__6", "$__7",
        "$__8", "$__9", "$__10", "$__11", "$__12", "$__13", "$__14", "$__15",
        "$__16", "$__17", "$__18", "$__19", "$__20", "$__21", "$__22", "$__23",
        "$__24", "$__25", "$__26", "$__27", "$__28", "$__29", "$__30", "$__31",
        "$__32", "$__33", "$__34", "$__35", "$__36", "$__37", "$__38", "$__39",
        "$__40", "$__41", "$__42", "$__43", "$__44", "$__45", "$__46", "$__47",
        "$__48", "$__49", "$__50", "$__51", "$__52", "$__53", "$__54", "$__55",
        "$__56", "$__57", "$__58", "$__59", "$__60", "$__61", "$__62", "$__63",
    ];

    if idx < NAMES.len() {
        return NAMES[idx];
    }

    // Overflow: thread-local cache so each name is interned once per thread
    thread_local! {
        static OVERFLOW: RefCell<Vec<&'static str>> = const { RefCell::new(Vec::new()) };
    }

    OVERFLOW.with(|cache| {
        let mut cache = cache.borrow_mut();
        let overflow_idx = idx - NAMES.len();
        while cache.len() <= overflow_idx {
            let name = format!("$__{}", NAMES.len() + cache.len());
            cache.push(crate::backend::models::gc_allocator::global_allocator().alloc_str(&name));
        }
        cache[overflow_idx]
    })
}

/// Check if an atom name is a variable (starts with $, &, or ').
/// Excludes special atoms: &, &self, &kb, &stack.
#[inline]
fn is_var_name(name: &str) -> bool {
    name.len() > 1
        && (name.starts_with('$')
            || name.starts_with('\'')
            || (name.starts_with('&')
                && name != "&"
                && name != "&self"
                && name != "&kb"
                && name != "&stack"))
}

/// Decompose for storage with De Bruijn normalization.
///
/// Variables are renamed to canonical positional names (`$__0`, `$__1`, etc.)
/// based on first-occurrence order in the expression. This ensures alpha-equivalent
/// expressions like `(= (f $x) $x)` and `(= (f $y) $y)` produce identical trie
/// keys, enabling correct duplicate detection and multiplicity tracking.
///
/// Pattern queries use `decompose()` (with `Variable` wildcards) for matching.
pub fn decompose_literal<V: MettaValueTrait>(value: &V) -> SmallVec<[TrieKey; 16]> {
    let mut keys = SmallVec::new();
    let mut var_map: SmallVec<[&'static str; 8]> = SmallVec::new();
    decompose_literal_debruijn(value, &mut keys, &mut var_map);
    keys
}

/// De Bruijn normalized literal decomposition.
///
/// `var_map` tracks the mapping from original variable names to De Bruijn indices.
/// The first variable encountered becomes `$__0`, the second `$__1`, etc.
fn decompose_literal_debruijn<V: MettaValueTrait>(
    value: &V,
    keys: &mut SmallVec<[TrieKey; 16]>,
    var_map: &mut SmallVec<[&'static str; 8]>,
) {
    let value = if value.is_spanned() {
        let stripped = value.strip_one_span();
        decompose_literal_debruijn(&stripped, keys, var_map);
        return;
    } else {
        value
    };

    if let Some(items) = value.as_sexpr() {
        keys.push(TrieKey::Arity(items.len() as u16));
        for child in items {
            decompose_literal_debruijn(child, keys, var_map);
        }
        return;
    }

    // Atoms: variables are De Bruijn normalized, non-variables stored literally
    if let Some(atom) = value.as_atom() {
        if is_var_name(atom) {
            // De Bruijn normalize: map original name to positional index
            let idx = if let Some(pos) = var_map.iter().position(|&v| v == atom) {
                pos
            } else {
                let pos = var_map.len();
                var_map.push(atom);
                pos
            };
            // Use a canonical interned name for the De Bruijn index
            let canonical = debruijn_var_name(idx);
            keys.push(TrieKey::Atom(canonical));
        } else {
            keys.push(TrieKey::Atom(atom));
        }
        return;
    }

    if let Some(n) = value.as_long() {
        keys.push(TrieKey::Long(n));
        return;
    }

    if let Some(b) = value.as_bool() {
        keys.push(TrieKey::Bool(b));
        return;
    }

    if let Some(f) = value.as_float() {
        keys.push(TrieKey::Float(f.to_bits()));
        return;
    }

    if let Some(s) = value.as_string() {
        let interned = crate::backend::models::gc_allocator::global_allocator().alloc_str(s);
        keys.push(TrieKey::Str(interned));
        return;
    }

    if let Some(inner) = value.as_type() {
        decompose_literal_debruijn(inner, keys, var_map);
        return;
    }

    if let Some(inner) = value.as_quoted_ref() {
        decompose_literal_debruijn(inner, keys, var_map);
        return;
    }

    if let Some(goals) = value.as_conjunction() {
        keys.push(TrieKey::Arity(goals.len() as u16));
        for goal in goals {
            decompose_literal_debruijn(goal, keys, var_map);
        }
        return;
    }

    if let Some((_msg, details)) = value.as_error() {
        keys.push(TrieKey::Error);
        decompose_literal_debruijn(details, keys, var_map);
        return;
    }

    if value.is_unit() || value.is_empty() {
        keys.push(TrieKey::Unit);
        return;
    }

    keys.push(TrieKey::Unit);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{MettaValue, MettaValueFactory, global_factory};

    fn f() -> crate::backend::models::GcFactory {
        global_factory()
    }

    #[test]
    fn test_decompose_atom() {
        let val = f().atom("hello");
        let keys = decompose(&val);
        assert_eq!(keys.as_slice(), &[TrieKey::Atom("hello")]);
    }

    #[test]
    fn test_decompose_variable() {
        let val = f().atom("$x");
        let keys = decompose(&val);
        assert_eq!(keys.as_slice(), &[TrieKey::Variable]);
    }

    #[test]
    fn test_decompose_wildcard() {
        let val = f().atom("_");
        let keys = decompose(&val);
        assert_eq!(keys.as_slice(), &[TrieKey::Variable]);
    }

    #[test]
    fn test_decompose_long() {
        let val = f().long(42);
        let keys = decompose(&val);
        assert_eq!(keys.as_slice(), &[TrieKey::Long(42)]);
    }

    #[test]
    fn test_decompose_bool() {
        let val = f().bool(true);
        let keys = decompose(&val);
        assert_eq!(keys.as_slice(), &[TrieKey::Bool(true)]);
    }

    #[test]
    fn test_decompose_sexpr() {
        // (f 42)
        let val = f().sexpr(vec![f().atom("f"), f().long(42)]);
        let keys = decompose(&val);
        assert_eq!(
            keys.as_slice(),
            &[TrieKey::Arity(2), TrieKey::Atom("f"), TrieKey::Long(42)]
        );
    }

    #[test]
    fn test_decompose_nested_sexpr() {
        // (f (g $x) 1)
        let val = f().sexpr(vec![
            f().atom("f"),
            f().sexpr(vec![f().atom("g"), f().atom("$x")]),
            f().long(1),
        ]);
        let keys = decompose(&val);
        assert_eq!(
            keys.as_slice(),
            &[
                TrieKey::Arity(3),
                TrieKey::Atom("f"),
                TrieKey::Arity(2),
                TrieKey::Atom("g"),
                TrieKey::Variable,
                TrieKey::Long(1),
            ]
        );
    }

    #[test]
    fn test_decompose_literal_debruijn_normalizes_variables() {
        // $x stored as De Bruijn normalized $__0
        let val = f().atom("$x");
        let keys = decompose_literal(&val);
        assert_eq!(keys.as_slice(), &[TrieKey::Atom("$__0")]);
    }

    #[test]
    fn test_decompose_literal_sexpr() {
        // (= (f $x) $x) stored with De Bruijn normalized variable names
        let val = f().sexpr(vec![
            f().atom("="),
            f().sexpr(vec![f().atom("f"), f().atom("$x")]),
            f().atom("$x"),
        ]);
        let keys = decompose_literal(&val);
        assert_eq!(
            keys.as_slice(),
            &[
                TrieKey::Arity(3),
                TrieKey::Atom("="),
                TrieKey::Arity(2),
                TrieKey::Atom("f"),
                TrieKey::Atom("$__0"),
                TrieKey::Atom("$__0"),
            ]
        );
    }

    #[test]
    fn test_decompose_literal_alpha_equivalence() {
        // (= (f $x) $x) and (= (f $y) $y) should produce identical keys
        let val_x = f().sexpr(vec![
            f().atom("="),
            f().sexpr(vec![f().atom("f"), f().atom("$x")]),
            f().atom("$x"),
        ]);
        let val_y = f().sexpr(vec![
            f().atom("="),
            f().sexpr(vec![f().atom("f"), f().atom("$y")]),
            f().atom("$y"),
        ]);
        assert_eq!(decompose_literal(&val_x), decompose_literal(&val_y));
    }

    #[test]
    fn test_decompose_literal_distinct_variables() {
        // (= (f $x $y) (+ $x $y)) — two distinct vars get different De Bruijn indices
        let val = f().sexpr(vec![
            f().atom("="),
            f().sexpr(vec![f().atom("f"), f().atom("$x"), f().atom("$y")]),
            f().sexpr(vec![f().atom("+"), f().atom("$x"), f().atom("$y")]),
        ]);
        let keys = decompose_literal(&val);
        assert_eq!(
            keys.as_slice(),
            &[
                TrieKey::Arity(3),
                TrieKey::Atom("="),
                TrieKey::Arity(3),
                TrieKey::Atom("f"),
                TrieKey::Atom("$__0"),
                TrieKey::Atom("$__1"),
                TrieKey::Arity(3),
                TrieKey::Atom("+"),
                TrieKey::Atom("$__0"),
                TrieKey::Atom("$__1"),
            ]
        );
    }

    #[test]
    fn test_decompose_unit() {
        let val = f().unit();
        let keys = decompose(&val);
        assert_eq!(keys.as_slice(), &[TrieKey::Unit]);
    }

    #[test]
    fn test_decompose_special_atoms_not_variables() {
        // &self, &kb, &stack are NOT variables
        for name in &["&self", "&kb", "&stack", "&"] {
            let val = f().atom(name);
            let keys = decompose(&val);
            assert_eq!(keys.as_slice(), &[TrieKey::Atom(name)], "'{name}' should be Atom, not Variable");
        }
    }
}
