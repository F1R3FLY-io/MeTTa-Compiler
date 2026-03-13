//! WAM Binding Frame: Stack-allocated variable binding storage.
//!
//! Replaces `GenericBindings<V>` for the WAM evaluation path. Variables are
//! mapped to slot indices at rule compilation time, enabling O(1) indexed
//! access instead of O(n) name-based lookup.
//!
//! # Performance Comparison
//!
//! | Operation | GenericBindings | WamBindingFrame |
//! |-----------|----------------|-----------------|
//! | Lookup    | O(n) linear scan | O(1) index |
//! | Insert    | O(1) amortized | O(1) index + trail push |
//! | Clone     | O(n) SmallVec clone | Not needed (trail undo) |
//! | Memory    | 8 × (ptr + V) stack | num_slots × V stack |

use smallvec::SmallVec;

use crate::backend::models::{GenericBindings, MettaValue};

/// WAM-style binding frame with indexed slot access.
///
/// Each rule's LHS compilation assigns variable names to slot indices.
/// At match time, `BindSlot` instructions write directly to the slot
/// and log the previous value on the trail for undo.
#[derive(Clone, Debug)]
pub struct WamBindingFrame {
    /// Variable binding slots, indexed by compile-time slot number.
    /// UNBOUND sentinel for unbound slots.
    pub(crate) slots: SmallVec<[MettaValue; 8]>,
    /// Trail mark at frame creation (for unwinding on backtrack).
    pub(crate) trail_mark: usize,
    /// Variable name → slot index mapping.
    /// Index in this vec corresponds to the slot index.
    /// Used for converting back to GenericBindings and for debugging.
    pub(crate) names: SmallVec<[&'static str; 8]>,
}

impl WamBindingFrame {
    /// Create the sentinel value for unbound slots.
    ///
    /// Uses `MettaValue::inline_empty()` (the zero-result sentinel) since it is
    /// never a valid binding value in normal evaluation. Variables can be bound to
    /// Unit, but EMPTY represents "no value/no results" which is never stored
    /// as a pattern variable binding.
    #[inline(always)]
    pub fn unbound() -> MettaValue {
        MettaValue::inline_empty()
    }

    /// Create a new binding frame with `num_slots` slots, all initialized to UNBOUND.
    pub fn new(num_slots: usize) -> Self {
        let mut slots = SmallVec::with_capacity(num_slots);
        let unbound = Self::unbound();
        slots.resize(num_slots, unbound);
        WamBindingFrame {
            slots,
            trail_mark: 0,
            names: SmallVec::new(),
        }
    }

    /// Create a new binding frame with slot names (from WamCode compilation).
    pub fn with_names(names: &[&'static str], trail_mark: usize) -> Self {
        let num_slots = names.len();
        let mut slots = SmallVec::with_capacity(num_slots);
        let unbound = Self::unbound();
        slots.resize(num_slots, unbound);
        WamBindingFrame {
            slots,
            trail_mark,
            names: SmallVec::from_slice(names),
        }
    }

    /// Get the value at a slot index.
    #[inline]
    pub fn get_slot(&self, index: u16) -> MettaValue {
        self.slots[index as usize]
    }

    /// Check if a slot is bound (not UNBOUND).
    #[inline]
    pub fn is_bound(&self, index: u16) -> bool {
        self.slots[index as usize] != Self::unbound()
    }

    /// Set a slot value without trail logging. Used by Trail::unwind_to().
    #[inline]
    pub fn set_slot_unchecked(&mut self, index: u16, value: MettaValue) {
        self.slots[index as usize] = value;
    }

    /// Number of slots in this frame.
    #[inline]
    pub fn num_slots(&self) -> usize {
        self.slots.len()
    }

    /// Reset all slots to UNBOUND, keeping capacity.
    pub fn reset(&mut self) {
        let unbound = Self::unbound();
        for slot in &mut self.slots {
            *slot = unbound;
        }
    }

    /// Convert this binding frame to `GenericBindings<MettaValue>` for interop
    /// with the existing trampoline evaluation path.
    ///
    /// Only includes bound slots (skips UNBOUND). Uses the `names` mapping
    /// to produce named bindings compatible with `apply_bindings_generic`.
    pub fn to_generic_bindings(&self) -> GenericBindings<MettaValue> {
        // Phase 3: Count bound slots first to pre-size the SmallVec,
        // avoiding reallocation during insert.
        let unbound = Self::unbound();
        let bound_count = self.slots.iter().filter(|&&v| v != unbound).count();
        if bound_count == 0 {
            return GenericBindings::Empty;
        }
        let mut bindings = GenericBindings::with_capacity(bound_count);
        for (i, &name) in self.names.iter().enumerate() {
            let value = self.slots[i];
            if value != unbound {
                bindings.insert(name, value);
            }
        }
        bindings
    }

    /// Look up a variable name and return its bound value (if any).
    ///
    /// Returns `None` if the variable is not in this frame or is unbound.
    /// O(n) scan over names — suitable for small frames (2-8 slots typical).
    #[inline]
    pub fn get_by_name(&self, name: &str) -> Option<MettaValue> {
        let unbound = Self::unbound();
        for (i, &slot_name) in self.names.iter().enumerate() {
            if slot_name == name {
                let value = self.slots[i];
                if value != unbound {
                    return Some(value);
                }
                return None;
            }
        }
        None
    }

    /// Apply bindings from this frame directly to a template, producing a
    /// substituted value. This avoids creating an intermediate `GenericBindings`.
    ///
    /// Uses iterative postorder traversal (same algorithm as `apply_bindings_generic`)
    /// but looks up variables directly in the frame's slot array.
    pub fn apply_to_template(
        &self,
        template: &MettaValue,
        factory: &crate::backend::models::GcFactory,
    ) -> MettaValue {
        use crate::backend::models::MettaValueFactory;

        // Fast path: no bound slots → return template as-is
        let unbound = Self::unbound();
        if self.slots.iter().all(|&v| v == unbound) {
            return *template;
        }

        // Iterative postorder traversal with two stacks
        enum Work {
            Process(MettaValue),
            BuildSExpr(usize),
        }

        let mut work: Vec<Work> = vec![Work::Process(*template)];
        let mut result_stack: Vec<MettaValue> = Vec::new();

        while let Some(item) = work.pop() {
            match item {
                Work::Process(val) => {
                    // Check for variable atoms
                    if let Some(name) = val.as_atom() {
                        if name.starts_with('$') {
                            if let Some(bound) = self.get_by_name(name) {
                                result_stack.push(bound);
                                continue;
                            }
                        }
                        result_stack.push(val);
                        continue;
                    }

                    // Recurse into S-expressions
                    if let Some(items) = val.as_sexpr() {
                        if items.is_empty() {
                            result_stack.push(val);
                            continue;
                        }
                        // Check if any child has variables (short-circuit)
                        if !val.has_variables_fast() {
                            result_stack.push(val);
                            continue;
                        }
                        let n = items.len();
                        work.push(Work::BuildSExpr(n));
                        // Push children in reverse order (first child processed first)
                        for child in items.iter().rev() {
                            work.push(Work::Process(*child));
                        }
                        continue;
                    }

                    // Everything else: push as-is
                    result_stack.push(val);
                }
                Work::BuildSExpr(n) => {
                    let start = result_stack.len() - n;
                    let children: Vec<MettaValue> = result_stack.drain(start..).collect();
                    result_stack.push(factory.sexpr(children));
                }
            }
        }

        result_stack.pop().unwrap_or(*template)
    }

    /// Collect all bound values for GC root reporting.
    pub fn collect_gc_roots(&self, out: &mut Vec<MettaValue>) {
        let unbound = Self::unbound();
        for &slot in &self.slots {
            if slot != unbound {
                out.push(slot);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::MettaValue;

    #[test]
    fn test_new_frame_all_unbound() {
        let frame = WamBindingFrame::new(4);
        assert_eq!(frame.num_slots(), 4);
        for i in 0..4 {
            assert!(!frame.is_bound(i));
            assert_eq!(frame.get_slot(i), WamBindingFrame::unbound());
        }
    }

    #[test]
    fn test_with_names() {
        let frame = WamBindingFrame::with_names(&["$x", "$y", "$z"], 0);
        assert_eq!(frame.num_slots(), 3);
        assert_eq!(frame.names.len(), 3);
        assert_eq!(frame.names[0], "$x");
        assert_eq!(frame.names[1], "$y");
        assert_eq!(frame.names[2], "$z");
    }

    #[test]
    fn test_set_and_get_slot() {
        let mut frame = WamBindingFrame::new(3);
        frame.set_slot_unchecked(1, MettaValue::Long(42));

        assert!(!frame.is_bound(0));
        assert!(frame.is_bound(1));
        assert!(!frame.is_bound(2));
        assert_eq!(frame.get_slot(1), MettaValue::Long(42));
    }

    #[test]
    fn test_reset() {
        let mut frame = WamBindingFrame::new(3);
        frame.set_slot_unchecked(0, MettaValue::Long(1));
        frame.set_slot_unchecked(1, MettaValue::Long(2));
        frame.set_slot_unchecked(2, MettaValue::Long(3));

        frame.reset();
        for i in 0..3 {
            assert!(!frame.is_bound(i));
        }
    }

    #[test]
    fn test_to_generic_bindings_empty() {
        let frame = WamBindingFrame::with_names(&["$x", "$y"], 0);
        let bindings = frame.to_generic_bindings();
        assert!(bindings.is_empty());
    }

    #[test]
    fn test_to_generic_bindings_partial() {
        let mut frame = WamBindingFrame::with_names(&["$x", "$y", "$z"], 0);
        frame.set_slot_unchecked(0, MettaValue::Long(10));
        // slot 1 left unbound
        frame.set_slot_unchecked(2, MettaValue::Long(30));

        let bindings = frame.to_generic_bindings();
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings.get("$x"), Some(&MettaValue::Long(10)));
        assert_eq!(bindings.get("$y"), None);
        assert_eq!(bindings.get("$z"), Some(&MettaValue::Long(30)));
    }

    #[test]
    fn test_to_generic_bindings_full() {
        let mut frame = WamBindingFrame::with_names(&["$a", "$b"], 0);
        frame.set_slot_unchecked(0, MettaValue::Long(1));
        frame.set_slot_unchecked(1, MettaValue::Long(2));

        let bindings = frame.to_generic_bindings();
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings.get("$a"), Some(&MettaValue::Long(1)));
        assert_eq!(bindings.get("$b"), Some(&MettaValue::Long(2)));
    }

    #[test]
    fn test_gc_roots_only_bound() {
        let mut frame = WamBindingFrame::new(4);
        frame.set_slot_unchecked(1, MettaValue::Long(42));
        frame.set_slot_unchecked(3, MettaValue::Long(99));

        let mut roots = Vec::new();
        frame.collect_gc_roots(&mut roots);
        assert_eq!(roots.len(), 2);
        assert!(roots.contains(&MettaValue::Long(42)));
        assert!(roots.contains(&MettaValue::Long(99)));
    }

    // ═══════════════════════════════════════════════════════════════════
    // Phase 3: get_by_name and apply_to_template tests
    // ═══════════════════════════════════════════════════════════════════

    #[test]
    fn test_get_by_name_found() {
        let mut frame = WamBindingFrame::with_names(&["$x", "$y"], 0);
        frame.set_slot_unchecked(0, MettaValue::Long(42));
        frame.set_slot_unchecked(1, MettaValue::Long(99));

        assert_eq!(frame.get_by_name("$x"), Some(MettaValue::Long(42)));
        assert_eq!(frame.get_by_name("$y"), Some(MettaValue::Long(99)));
    }

    #[test]
    fn test_get_by_name_not_found() {
        let frame = WamBindingFrame::with_names(&["$x", "$y"], 0);
        assert_eq!(frame.get_by_name("$z"), None);
    }

    #[test]
    fn test_get_by_name_unbound() {
        let frame = WamBindingFrame::with_names(&["$x", "$y"], 0);
        // $x is in frame but unbound
        assert_eq!(frame.get_by_name("$x"), None);
    }

    #[test]
    fn test_apply_to_template_variable_substitution() {
        use crate::backend::models::gc_allocator::global_factory;
        use crate::backend::models::MettaValueFactory;

        let f = global_factory();
        let mut frame = WamBindingFrame::with_names(&["$x", "$y"], 0);
        frame.set_slot_unchecked(0, MettaValue::Long(42));
        frame.set_slot_unchecked(1, MettaValue::Long(99));

        // Template: (+ $x $y)
        let template = f.sexpr(vec![f.atom("+"), f.atom("$x"), f.atom("$y")]);
        let result = frame.apply_to_template(&template, &f);

        // Should produce: (+ 42 99)
        let items = result.as_sexpr().expect("should be sexpr");
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].as_atom(), Some("+"));
        assert_eq!(items[1], MettaValue::Long(42));
        assert_eq!(items[2], MettaValue::Long(99));
    }

    #[test]
    fn test_apply_to_template_no_variables() {
        use crate::backend::models::gc_allocator::global_factory;
        use crate::backend::models::MettaValueFactory;

        let f = global_factory();
        let frame = WamBindingFrame::with_names(&["$x"], 0);

        // Template with no variables: (+ 1 2)
        let template = f.sexpr(vec![f.atom("+"), MettaValue::Long(1), MettaValue::Long(2)]);
        let result = frame.apply_to_template(&template, &f);

        // Should return unchanged (no bound slots)
        assert_eq!(result, template);
    }

    #[test]
    fn test_apply_to_template_nested() {
        use crate::backend::models::gc_allocator::global_factory;
        use crate::backend::models::MettaValueFactory;

        let f = global_factory();
        let mut frame = WamBindingFrame::with_names(&["$x"], 0);
        frame.set_slot_unchecked(0, MettaValue::Long(5));

        // Template: (f (g $x))
        let template = f.sexpr(vec![
            f.atom("f"),
            f.sexpr(vec![f.atom("g"), f.atom("$x")]),
        ]);
        let result = frame.apply_to_template(&template, &f);

        // Should produce: (f (g 5))
        let items = result.as_sexpr().expect("should be sexpr");
        assert_eq!(items.len(), 2);
        let inner = items[1].as_sexpr().expect("inner should be sexpr");
        assert_eq!(inner.len(), 2);
        assert_eq!(inner[0].as_atom(), Some("g"));
        assert_eq!(inner[1], MettaValue::Long(5));
    }

    #[test]
    fn test_apply_to_template_unbound_variable_passthrough() {
        use crate::backend::models::gc_allocator::global_factory;
        use crate::backend::models::MettaValueFactory;

        let f = global_factory();
        let mut frame = WamBindingFrame::with_names(&["$x", "$y"], 0);
        frame.set_slot_unchecked(0, MettaValue::Long(42));
        // $y is unbound

        // Template: (f $x $y)
        let template = f.sexpr(vec![f.atom("f"), f.atom("$x"), f.atom("$y")]);
        let result = frame.apply_to_template(&template, &f);

        // Should produce: (f 42 $y) — $y stays as variable
        let items = result.as_sexpr().expect("should be sexpr");
        assert_eq!(items.len(), 3);
        assert_eq!(items[1], MettaValue::Long(42));
        assert_eq!(items[2].as_atom(), Some("$y"));
    }
}
