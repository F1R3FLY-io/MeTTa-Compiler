//! WAM-specific allocation: bypass hash-consing for intermediate S-expressions.
//!
//! During WAM RHS body construction (`BuildSExpr`), S-expressions are built
//! from register values. These intermediates are typically consumed immediately
//! by the caller (either returned as results or passed to further instructions).
//!
//! The standard `GcFactory::sexpr()` path performs hash-consing for ground
//! S-expressions (~100 cycles per lookup). Since WAM intermediates are rarely
//! duplicates of existing expressions, this is wasted work.
//!
//! `wam_sexpr()` bypasses hash-consing and uses `alloc_slice_copy()` directly
//! (no iterator adapter overhead). Combined with the stack-local array in the
//! `BuildSExpr` handler, this eliminates both Vec heap allocation and hash-cons
//! lookup for the common case (count <= 16).

use crate::backend::models::{MettaValue, MettaValueInner};
use crate::backend::models::gc_allocator::global_allocator;
use crate::backend::models::metta_value::FLAG_HAS_VARIABLES;

/// Build an S-expression from a slice of MettaValues, bypassing hash-consing.
///
/// Uses `alloc_slice_copy()` + `alloc_value()` directly on the global slab.
/// Equivalent to `GcFactory::sexpr_from_slice()` minus the hash-cons lookup/insert.
///
/// # Safety invariants (same as GcFactory)
/// - All input values must be valid MettaValues
/// - Returned value has `&'static` lifetime (slab-owned)
/// - GC can see the allocated page (standard slab page tracking)
#[inline]
pub fn wam_sexpr(items: &[MettaValue]) -> MettaValue {
    if items.is_empty() {
        return MettaValue::inline_unit();
    }
    let alloc = global_allocator();
    let has_vars = items.iter().any(|i| i.has_variables_fast());
    let slice = alloc.alloc_slice_copy(items);
    let inner = alloc.alloc_value(MettaValueInner::SExpr(slice));
    if has_vars {
        MettaValue::from_inner_tagged(inner, FLAG_HAS_VARIABLES as u8)
    } else {
        MettaValue::from_inner(inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{MettaValueFactory, gc_allocator::global_factory};

    #[test]
    fn test_wam_sexpr_basic() {
        let f = global_factory();
        let items = [f.atom("f"), MettaValue::Long(42)];
        let result = wam_sexpr(&items);

        let sexpr_items = result.as_sexpr().expect("should be S-expression");
        assert_eq!(sexpr_items.len(), 2);
        assert_eq!(sexpr_items[0].as_atom(), Some("f"));
        assert_eq!(sexpr_items[1], MettaValue::Long(42));
    }

    #[test]
    fn test_wam_sexpr_with_variables() {
        let f = global_factory();
        let items = [f.atom("g"), f.atom("$x"), MettaValue::Long(1)];
        let result = wam_sexpr(&items);

        assert!(result.has_variables_fast(), "should have variables flag");
        let sexpr_items = result.as_sexpr().expect("should be S-expression");
        assert_eq!(sexpr_items.len(), 3);
    }

    #[test]
    fn test_wam_sexpr_ground() {
        let f = global_factory();
        let items = [f.atom("h"), MettaValue::Long(1), MettaValue::Long(2)];
        let result = wam_sexpr(&items);

        assert!(!result.has_variables_fast(), "should not have variables flag");
        let sexpr_items = result.as_sexpr().expect("should be S-expression");
        assert_eq!(sexpr_items.len(), 3);
    }

    #[test]
    fn test_wam_sexpr_empty() {
        let result = wam_sexpr(&[]);
        assert_eq!(result, MettaValue::inline_unit(), "empty should be unit");
    }

    #[test]
    fn test_wam_sexpr_distinct_pointers() {
        let f = global_factory();
        let items = [f.atom("a"), MettaValue::Long(1)];
        let r1 = wam_sexpr(&items);
        let r2 = wam_sexpr(&items);

        // Without hash-consing, each call allocates a distinct value
        // (unlike GcFactory::sexpr which deduplicates ground expressions)
        let r1_items = r1.as_sexpr().expect("r1 sexpr");
        let r2_items = r2.as_sexpr().expect("r2 sexpr");
        assert_eq!(r1_items.len(), r2_items.len());
        // Both should have correct content
        assert_eq!(r1_items[0].as_atom(), Some("a"));
        assert_eq!(r2_items[0].as_atom(), Some("a"));
    }

    #[test]
    fn test_wam_sexpr_nested() {
        let f = global_factory();
        let inner = wam_sexpr(&[f.atom("g"), MettaValue::Long(5)]);
        let outer = wam_sexpr(&[f.atom("f"), inner, MettaValue::Long(10)]);

        let outer_items = outer.as_sexpr().expect("outer sexpr");
        assert_eq!(outer_items.len(), 3);
        let inner_items = outer_items[1].as_sexpr().expect("inner sexpr");
        assert_eq!(inner_items.len(), 2);
        assert_eq!(inner_items[1], MettaValue::Long(5));
    }
}
