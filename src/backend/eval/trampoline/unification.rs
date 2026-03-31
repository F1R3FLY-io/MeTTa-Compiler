//! WAM-style Union-Find Unification
//!
//! Implements bidirectional structural unification using a flat heap of cells
//! with deref (path following) and bind (union) operations, inspired by
//! Warren's Abstract Machine (WAM).
//!
//! ## Algorithm
//!
//! Terms are loaded onto a flat `Vec<Cell>` heap. Variables become `Ref(self)`
//! cells (unbound). Unification walks both terms simultaneously:
//!
//! - Two identical addresses → trivially unified
//! - Variable vs anything → bind (point variable at target), push to trail
//! - S-expression vs S-expression → decompose element-wise, recurse
//! - Ground term vs ground term → check equality
//! - Mismatch → failure
//!
//! ## Occurs Check
//!
//! Before binding a variable to a term, checks that the variable does not
//! appear within the term (prevents infinite terms like `$x = f($x)`).
//!
//! ## Complexity
//!
//! O(n·α(n)) amortized with path compression, where α is the inverse
//! Ackermann function (effectively constant for practical inputs).

use crate::backend::models::{MettaValue, MettaValueFactory, MettaValueTrait};
use super::engine::Bindings;

// ============================================================================
// Heap Cells
// ============================================================================

/// A cell on the unification heap.
#[derive(Clone, Debug)]
enum Cell {
    /// Unbound variable: points to itself. Bound variable: points to target.
    Ref(usize),
    /// Named variable reference (stores the interned name for binding extraction).
    /// The usize is a forwarding index: initially self, updated by bind.
    NamedRef(usize, &'static str),
    /// Ground atom (interned string).
    Atom(&'static str),
    /// Integer literal.
    Long(i64),
    /// Float literal.
    Float(u64), // stored as bits for Eq
    /// Boolean literal.
    Bool(bool),
    /// String literal.
    Str(&'static str),
    /// S-expression header: (first_child_index, arity).
    /// Children are at consecutive heap positions starting at first_child_index.
    SExpr(usize, usize),
    /// Unit / empty S-expression.
    Unit,
    /// Wildcard — unifies with anything without creating a binding.
    Wildcard,
}

// ============================================================================
// Unification Heap
// ============================================================================

/// WAM-style unification heap with trail for backtracking.
struct UnificationHeap {
    cells: Vec<Cell>,
    trail: Vec<usize>,
}

impl UnificationHeap {
    fn new() -> Self {
        Self {
            cells: Vec::with_capacity(64),
            trail: Vec::with_capacity(16),
        }
    }

    /// Allocate a new cell, return its address.
    #[inline]
    fn alloc(&mut self, cell: Cell) -> usize {
        let addr = self.cells.len();
        self.cells.push(cell);
        addr
    }

    /// Dereference a Ref chain to find the canonical cell address.
    /// Applies path compression: intermediate Refs are updated to point
    /// directly at the root (amortized O(α(n))).
    fn deref(&mut self, mut addr: usize) -> usize {
        loop {
            match &self.cells[addr] {
                Cell::Ref(target) if *target != addr => {
                    let target = *target;
                    // Path compression: update intermediate to skip one level
                    if let Cell::Ref(next) = &self.cells[target] {
                        if *next != target {
                            self.cells[addr] = Cell::Ref(*next);
                        }
                    }
                    addr = target;
                }
                Cell::NamedRef(target, _) if *target != addr => {
                    let target = *target;
                    addr = target;
                }
                _ => return addr,
            }
        }
    }

    /// Bind variable at `var_addr` to point at `target_addr`.
    /// Records the binding on the trail for potential backtracking.
    fn bind(&mut self, var_addr: usize, target_addr: usize) {
        self.trail.push(var_addr);
        match &self.cells[var_addr] {
            Cell::Ref(_) => self.cells[var_addr] = Cell::Ref(target_addr),
            Cell::NamedRef(_, name) => {
                let name = *name;
                self.cells[var_addr] = Cell::NamedRef(target_addr, name);
            }
            _ => unreachable!("bind called on non-variable cell"),
        }
    }

    /// Check if variable at `var_addr` occurs within the term rooted at `term_addr`.
    /// Returns true if it does (meaning unification would create an infinite term).
    fn occurs_in(&mut self, var_addr: usize, term_addr: usize) -> bool {
        let term_addr = self.deref(term_addr);
        if var_addr == term_addr {
            return true;
        }
        if let Cell::SExpr(start, arity) = self.cells[term_addr].clone() {
            for i in 0..arity {
                if self.occurs_in(var_addr, start + i) {
                    return true;
                }
            }
        }
        false
    }

    /// Returns true if the cell at `addr` is an unbound variable.
    fn is_variable(&self, addr: usize) -> bool {
        matches!(&self.cells[addr], Cell::Ref(t) | Cell::NamedRef(t, _) if *t == addr)
    }

    /// Core WAM unification algorithm.
    ///
    /// Returns true if terms at `a` and `b` unify, false on failure.
    /// On success, variable bindings are recorded in the heap (follow Ref chains)
    /// and on the trail (for backtracking).
    fn unify(&mut self, a: usize, b: usize) -> bool {
        let mut stack: Vec<(usize, usize)> = Vec::with_capacity(16);
        stack.push((a, b));

        while let Some((lhs, rhs)) = stack.pop() {
            let lhs = self.deref(lhs);
            let rhs = self.deref(rhs);

            if lhs == rhs {
                continue; // Same cell — trivially unified
            }

            // Wildcard unifies with anything
            if matches!(self.cells[lhs], Cell::Wildcard) || matches!(self.cells[rhs], Cell::Wildcard) {
                continue;
            }

            let lhs_is_var = self.is_variable(lhs);
            let rhs_is_var = self.is_variable(rhs);

            if lhs_is_var {
                // Occurs check: prevent $x = f($x)
                if !rhs_is_var && self.occurs_in(lhs, rhs) {
                    return false;
                }
                self.bind(lhs, rhs);
                continue;
            }

            if rhs_is_var {
                // Occurs check: prevent $x = f($x)
                if self.occurs_in(rhs, lhs) {
                    return false;
                }
                self.bind(rhs, lhs);
                continue;
            }

            // Both are non-variable — structural comparison
            // Clone cells to avoid borrow issues
            let lhs_cell = self.cells[lhs].clone();
            let rhs_cell = self.cells[rhs].clone();

            match (&lhs_cell, &rhs_cell) {
                (Cell::Atom(a), Cell::Atom(b)) => {
                    if a != b { return false; }
                }
                (Cell::Long(a), Cell::Long(b)) => {
                    if a != b { return false; }
                }
                (Cell::Float(a), Cell::Float(b)) => {
                    if a != b { return false; }
                }
                (Cell::Bool(a), Cell::Bool(b)) => {
                    if a != b { return false; }
                }
                (Cell::Str(a), Cell::Str(b)) => {
                    if a != b { return false; }
                }
                (Cell::Unit, Cell::Unit) => {}
                (Cell::SExpr(start_a, arity_a), Cell::SExpr(start_b, arity_b)) => {
                    if arity_a != arity_b {
                        return false; // Arity mismatch
                    }
                    // Decompose: push child pairs onto work stack
                    for i in 0..*arity_a {
                        stack.push((start_a + i, start_b + i));
                    }
                }
                // Unit matches empty S-expression
                (Cell::Unit, Cell::SExpr(_, 0)) | (Cell::SExpr(_, 0), Cell::Unit) => {}
                _ => return false, // Type mismatch
            }
        }

        true
    }

    /// Load a MettaValue term onto the heap, returning the root address.
    ///
    /// Variables (`$x`, `&y`, `'z`) become NamedRef cells.
    /// S-expressions are flattened: header at position N, children at N+1..N+arity.
    /// The `var_map` tracks previously seen variable names so repeated occurrences
    /// share the same cell (essential for unification: `($x $x)` means both must unify).
    fn push_term(
        &mut self,
        value: &MettaValue,
        var_map: &mut Vec<(&'static str, usize)>,
    ) -> usize {
        // Check if it's a variable
        if let Some(name) = value.as_atom() {
            if name == "_" {
                return self.alloc(Cell::Wildcard);
            }
            if (name.starts_with('$') || name.starts_with('&') || name.starts_with('\''))
                && name != "&"
            {
                // Check if this variable was already seen
                for &(n, addr) in var_map.iter() {
                    if n == name {
                        return addr;
                    }
                }
                // New variable — allocate NamedRef pointing to self
                let addr = self.cells.len();
                self.cells.push(Cell::NamedRef(addr, name));
                var_map.push((name, addr));
                return addr;
            }
            return self.alloc(Cell::Atom(name));
        }

        // S-expression
        if let Some(items) = value.as_sexpr() {
            if items.is_empty() {
                return self.alloc(Cell::Unit);
            }
            // Reserve space: header + children
            let header_addr = self.cells.len();
            let arity = items.len();
            // Push placeholder header
            self.cells.push(Cell::SExpr(header_addr + 1, arity));
            // Reserve child slots (will be filled below)
            let children_start = self.cells.len();
            for _ in 0..arity {
                self.cells.push(Cell::Unit); // placeholder
            }
            // Fill children recursively
            for (i, item) in items.iter().enumerate() {
                let child_addr = self.push_term(item, var_map);
                // If child was allocated at a different position, update the slot
                if child_addr != children_start + i {
                    self.cells[children_start + i] = Cell::Ref(child_addr);
                }
            }
            return header_addr;
        }

        // Ground types
        if let Some(n) = value.as_long() {
            return self.alloc(Cell::Long(n));
        }
        if let Some(f) = value.as_float() {
            return self.alloc(Cell::Float(f.to_bits()));
        }
        if let Some(b) = value.as_bool() {
            return self.alloc(Cell::Bool(b));
        }
        if let Some(s) = value.as_string() {
            return self.alloc(Cell::Str(s));
        }
        if value.is_unit() {
            return self.alloc(Cell::Unit);
        }

        // Fallback: treat as opaque atom via friendly repr
        // This handles Space, State, Type, etc.
        self.alloc(Cell::Atom(value.as_atom().unwrap_or("_unknown")))
    }

    /// Extract bindings from the heap after successful unification.
    ///
    /// Walks all NamedRef cells. If a NamedRef points to something other than
    /// itself, extract the binding `name → MettaValue`.
    fn extract_bindings(
        &mut self,
        var_map: &[(&'static str, usize)],
        factory: &crate::backend::models::GcFactory,
    ) -> Bindings {
        let mut bindings = Bindings::new();
        for &(name, addr) in var_map {
            let resolved = self.deref(addr);
            if resolved != addr {
                // Variable was bound — convert resolved cell back to MettaValue
                let value = self.cell_to_metta(resolved, factory);
                bindings.insert(name, value);
            }
        }
        bindings
    }

    /// Convert a heap cell (and its substructure) back to a MettaValue.
    fn cell_to_metta(
        &mut self,
        addr: usize,
        factory: &crate::backend::models::GcFactory,
    ) -> MettaValue {
        let addr = self.deref(addr);
        match self.cells[addr].clone() {
            Cell::Ref(_) | Cell::NamedRef(_, _) => {
                // Unbound variable — return it as-is
                if let Cell::NamedRef(_, name) = &self.cells[addr] {
                    factory.atom(name)
                } else {
                    factory.atom("_")
                }
            }
            Cell::Atom(s) => factory.atom(s),
            Cell::Long(n) => factory.long(n),
            Cell::Float(bits) => factory.float(f64::from_bits(bits)),
            Cell::Bool(b) => factory.bool(b),
            Cell::Str(s) => factory.string(s),
            Cell::Unit => factory.unit(),
            Cell::Wildcard => factory.atom("_"),
            Cell::SExpr(start, arity) => {
                let mut children = Vec::with_capacity(arity);
                for i in 0..arity {
                    children.push(self.cell_to_metta(start + i, factory));
                }
                factory.sexpr(children)
            }
        }
    }
}

// ============================================================================
// Public API
// ============================================================================

/// Bidirectional structural unification using WAM-style union-find.
///
/// Returns `Some(bindings)` if `a` and `b` unify, `None` on failure.
/// Handles variables on both sides simultaneously, performs occurs check,
/// and uses path compression for O(n·α(n)) amortized performance.
///
/// # Examples
///
/// ```text
/// bidirectional_unify((a $x), ($y b)) → Some({$x → b, $y → a})
/// bidirectional_unify(($x $x), (a b)) → None (conflict: $x can't be both a and b)
/// bidirectional_unify($x, (f $x))     → None (occurs check)
/// ```
pub fn bidirectional_unify(a: &MettaValue, b: &MettaValue) -> Option<Bindings> {
    let factory = crate::backend::models::global_factory();
    let mut heap = UnificationHeap::new();
    let mut var_map = Vec::with_capacity(8);

    let addr_a = heap.push_term(a, &mut var_map);
    let addr_b = heap.push_term(b, &mut var_map);

    if heap.unify(addr_a, addr_b) {
        Some(heap.extract_bindings(&var_map, &factory))
    } else {
        None
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{global_factory, init_global_allocator, MettaValue, MettaValueFactory};

    fn setup() {
        let _ = init_global_allocator();
    }

    fn atom(s: &str) -> MettaValue {
        global_factory().atom(s)
    }

    fn sexpr(items: Vec<MettaValue>) -> MettaValue {
        global_factory().sexpr(items)
    }

    fn long(n: i64) -> MettaValue {
        global_factory().long(n)
    }

    #[test]
    fn test_identical_atoms() {
        setup();
        let result = bidirectional_unify(&atom("a"), &atom("a"));
        assert!(result.is_some());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_different_atoms_fail() {
        setup();
        assert!(bidirectional_unify(&atom("a"), &atom("b")).is_none());
    }

    #[test]
    fn test_variable_left() {
        setup();
        let result = bidirectional_unify(&atom("$x"), &atom("a")).unwrap();
        assert_eq!(result.get("$x").unwrap().as_atom().unwrap(), "a");
    }

    #[test]
    fn test_variable_right() {
        setup();
        let result = bidirectional_unify(&atom("a"), &atom("$x")).unwrap();
        assert_eq!(result.get("$x").unwrap().as_atom().unwrap(), "a");
    }

    #[test]
    fn test_both_sides_variables() {
        setup();
        // (a $x) unify ($y b) → {$x → b, $y → a}
        let lhs = sexpr(vec![atom("a"), atom("$x")]);
        let rhs = sexpr(vec![atom("$y"), atom("b")]);
        let result = bidirectional_unify(&lhs, &rhs).unwrap();
        assert_eq!(result.get("$x").unwrap().as_atom().unwrap(), "b");
        assert_eq!(result.get("$y").unwrap().as_atom().unwrap(), "a");
    }

    #[test]
    fn test_conflict_same_variable() {
        setup();
        // ($x $x) unify (a b) → fail (conflict: $x can't be both a and b)
        let lhs = sexpr(vec![atom("$x"), atom("$x")]);
        let rhs = sexpr(vec![atom("a"), atom("b")]);
        assert!(bidirectional_unify(&lhs, &rhs).is_none());
    }

    #[test]
    fn test_same_variable_consistent() {
        setup();
        // ($x $x) unify (a a) → {$x → a}
        let lhs = sexpr(vec![atom("$x"), atom("$x")]);
        let rhs = sexpr(vec![atom("a"), atom("a")]);
        let result = bidirectional_unify(&lhs, &rhs).unwrap();
        assert_eq!(result.get("$x").unwrap().as_atom().unwrap(), "a");
    }

    #[test]
    fn test_occurs_check() {
        setup();
        // $x unify (f $x) → fail (infinite term)
        let lhs = atom("$x");
        let rhs = sexpr(vec![atom("f"), atom("$x")]);
        assert!(bidirectional_unify(&lhs, &rhs).is_none());
    }

    #[test]
    fn test_nested_sexpr() {
        setup();
        // (f (g $x)) unify (f (g a)) → {$x → a}
        let lhs = sexpr(vec![atom("f"), sexpr(vec![atom("g"), atom("$x")])]);
        let rhs = sexpr(vec![atom("f"), sexpr(vec![atom("g"), atom("a")])]);
        let result = bidirectional_unify(&lhs, &rhs).unwrap();
        assert_eq!(result.get("$x").unwrap().as_atom().unwrap(), "a");
    }

    #[test]
    fn test_arity_mismatch() {
        setup();
        let lhs = sexpr(vec![atom("a"), atom("b")]);
        let rhs = sexpr(vec![atom("a"), atom("b"), atom("c")]);
        assert!(bidirectional_unify(&lhs, &rhs).is_none());
    }

    #[test]
    fn test_two_variables() {
        setup();
        // $x unify $y → binds one to the other (both unbound)
        let result = bidirectional_unify(&atom("$x"), &atom("$y"));
        assert!(result.is_some());
    }

    #[test]
    fn test_wildcard() {
        setup();
        let result = bidirectional_unify(&atom("_"), &atom("a"));
        assert!(result.is_some());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_integers() {
        setup();
        assert!(bidirectional_unify(&long(42), &long(42)).is_some());
        assert!(bidirectional_unify(&long(42), &long(43)).is_none());
    }

    #[test]
    fn test_integer_in_sexpr() {
        setup();
        // (2 $list) unify (2 (Cons a b)) → {$list → (Cons a b)}
        let lhs = sexpr(vec![long(2), atom("$list")]);
        let rhs = sexpr(vec![long(2), sexpr(vec![atom("Cons"), atom("a"), atom("b")])]);
        let result = bidirectional_unify(&lhs, &rhs).unwrap();
        let list = result.get("$list").unwrap();
        assert!(list.as_sexpr().is_some());
    }

    #[test]
    fn test_ampersand_variable() {
        setup();
        let result = bidirectional_unify(&atom("&x"), &atom("hello")).unwrap();
        assert_eq!(result.get("&x").unwrap().as_atom().unwrap(), "hello");
    }

    #[test]
    fn test_quote_variable() {
        setup();
        let result = bidirectional_unify(&atom("'x"), &atom("hello")).unwrap();
        assert_eq!(result.get("'x").unwrap().as_atom().unwrap(), "hello");
    }
}
