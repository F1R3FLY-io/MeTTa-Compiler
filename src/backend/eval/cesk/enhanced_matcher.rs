//! Enhanced Structural Matcher with Indexed Slots
//!
//! This module provides `EnhancedMatcher`, an improved pattern matcher that
//! addresses limitations of the existing `StructuralMatcher`:
//!
//! - **No depth limit**: Uses dynamically-sized `MatchPathDyn` instead of fixed `[u8; 8]`
//! - **Indexed binding slots**: Array-indexed variable access instead of name-keyed SmallVec
//! - **Check reordering**: Most discriminative checks first for faster fail-fast
//! - **Precomputed slot count**: Known binding count at compile time for pre-allocation
//!
//! ## Design
//!
//! The enhanced matcher compiles an LHS pattern into two parallel instruction
//! sequences:
//!
//! 1. **Structural checks**: Arity, atom, literal equality (fail-fast ordered)
//! 2. **Slot operations**: Bind-to-slot and slot-equality-check
//!
//! Variable bindings use a flat `Vec<Option<V>>` indexed by slot number (u8),
//! rather than the name-keyed `GenericBindings` map. This provides O(1) access
//! for repeated-variable equality checks and O(1) binding insertion.
//!
//! At the end of matching, the slot array is converted to `GenericBindings<V>`
//! for compatibility with the continuation system.
//!
//! ## Integration
//!
//! `EnhancedMatcher` is a drop-in replacement for `StructuralMatcher` in
//! `RuleEntry`. It can analyze patterns that `StructuralMatcher` rejects
//! (depth > 8) and produces the same `GenericBindings<V>` output.

use smallvec::SmallVec;

use crate::backend::models::{BindingName, GenericBindings, MettaValueTrait};

// ============================================================================
// Dynamic Match Path (no depth limit)
// ============================================================================

/// Navigation path from root to a node in the expression tree.
///
/// Unlike `MatchPath` (fixed `[u8; 8]`), this uses a SmallVec that inlines
/// up to 8 indices but can grow dynamically for deeper patterns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MatchPathDyn {
    /// Child indices from root to target node.
    /// Inlines up to 8 (covers >99% of patterns), spills to heap for deeper.
    indices: SmallVec<[u8; 8]>,
}

impl MatchPathDyn {
    /// Empty path (root node).
    #[inline]
    pub fn root() -> Self {
        Self {
            indices: SmallVec::new(),
        }
    }

    /// Create a child path by appending an index.
    #[inline]
    pub fn child(&self, idx: u8) -> Self {
        let mut new = self.clone();
        new.indices.push(idx);
        new
    }

    /// Navigate to the target node in an expression tree.
    ///
    /// Supports S-expressions, Type wrappers, Quoted wrappers, Conjunctions,
    /// and Error nodes by trying each structural accessor in order.
    #[inline]
    pub fn navigate<'a, V: MettaValueTrait>(&self, root: &'a V) -> Option<&'a V> {
        let mut current = root;
        for &idx in &self.indices {
            current = get_child(current, idx as usize)?;
        }
        Some(current)
    }

    /// Navigate with variable resolution through outer bindings.
    #[inline]
    pub fn navigate_resolving<V: MettaValueTrait + Clone>(
        &self,
        root: &V,
        bindings: &GenericBindings<V>,
    ) -> Option<V> {
        let mut current = resolve_var(root, bindings);
        for &idx in &self.indices {
            let child = get_child_owned(&current, idx as usize)?;
            current = resolve_var(&child, bindings);
        }
        Some(current)
    }

    /// Path depth.
    #[inline]
    pub fn depth(&self) -> usize {
        self.indices.len()
    }
}

/// Resolve a variable through bindings (single level).
#[inline]
fn resolve_var<V: MettaValueTrait + Clone>(val: &V, bindings: &GenericBindings<V>) -> V {
    if let Some(name) = val.as_atom() {
        if is_var_name(name) {
            if let Some(bound) = bindings.get(name) {
                return bound.clone();
            }
        }
    }
    val.clone()
}

/// Get the `idx`-th child of a value, trying all structural types:
/// S-expression children, Type inner (idx 0), Quoted inner (idx 0),
/// Conjunction goals (idx n), Error fields (idx 0 = details).
#[inline]
fn get_child<'a, V: MettaValueTrait>(value: &'a V, idx: usize) -> Option<&'a V> {
    // S-expression (most common)
    if let Some(items) = value.as_sexpr() {
        return items.get(idx);
    }
    // Type wrapper: single child at index 0
    if let Some(inner) = value.as_type() {
        return if idx == 0 { Some(inner) } else { None };
    }
    // Quoted wrapper: single child at index 0
    if let Some(inner) = value.as_quoted_ref() {
        return if idx == 0 { Some(inner) } else { None };
    }
    // Conjunction: N children
    if let Some(goals) = value.as_conjunction() {
        return goals.get(idx);
    }
    // Error: child 0 = offending expression, child 1 = detail value
    if let Some((offending, detail)) = value.as_error() {
        return match idx {
            0 => Some(offending),
            1 => Some(detail),
            _ => None,
        };
    }
    None
}

/// Owned version of get_child for navigate_resolving (which works with owned values).
#[inline]
fn get_child_owned<V: MettaValueTrait + Clone>(value: &V, idx: usize) -> Option<V> {
    if let Some(items) = value.as_sexpr() {
        return items.get(idx).cloned();
    }
    if let Some(inner) = value.as_type() {
        return if idx == 0 { Some(inner.clone()) } else { None };
    }
    if let Some(inner) = value.as_quoted_ref() {
        return if idx == 0 { Some(inner.clone()) } else { None };
    }
    if let Some(goals) = value.as_conjunction() {
        return goals.get(idx).cloned();
    }
    if let Some((offending, detail)) = value.as_error() {
        return match idx {
            0 => Some(offending.clone()),
            1 => Some(detail.clone()),
            _ => None,
        };
    }
    None
}

/// Check if an atom name is a variable (starts with $, &, or ').
///
/// Excludes the `$_` wildcard (treated as a match-anything, no-binding atom).
#[inline]
fn is_var_name(name: &str) -> bool {
    name.len() > 1
        && name != "$_"
        && (name.starts_with('$')
            || name.starts_with('\'')
            || (name.starts_with('&')
                && name != "&"
                && name != "&self"
                && name != "&kb"
                && name != "&stack"))
}

// ============================================================================
// Enhanced Check Instructions
// ============================================================================

/// A structural check instruction (fail-fast, ordered by discriminative power).
#[derive(Clone, Debug)]
pub enum ECheck {
    /// Verify S-expression arity at path.
    Arity { path: MatchPathDyn, expected: u16 },
    /// Verify atom equality at path (interned pointer comparison).
    Atom {
        path: MatchPathDyn,
        expected: &'static str,
    },
    /// Verify i64 equality at path.
    Long { path: MatchPathDyn, expected: i64 },
    /// Verify bool equality at path.
    Bool { path: MatchPathDyn, expected: bool },
    /// Verify f64 bitwise equality at path.
    Float {
        path: MatchPathDyn,
        expected_bits: u64,
    },
    /// Verify string equality at path (interned pointer comparison).
    Str {
        path: MatchPathDyn,
        expected: &'static str,
    },
    /// Verify value at path is a Type wrapper.
    IsType { path: MatchPathDyn },
    /// Verify value at path is a Quoted wrapper.
    IsQuoted { path: MatchPathDyn },
    /// Verify value at path is an Error with the given message.
    IsError {
        path: MatchPathDyn,
        expected_msg: &'static str,
    },
    /// Verify value at path is a Conjunction with the given number of goals.
    IsConjunction {
        path: MatchPathDyn,
        expected_len: u16,
    },
    /// Verify value at path is Unit (empty tuple `()`).
    IsUnit { path: MatchPathDyn },
}

/// A variable slot operation, executed after all structural checks pass.
#[derive(Clone, Debug)]
pub enum SlotOp {
    /// Bind value at path to slot index.
    Bind {
        path: MatchPathDyn,
        slot: u8,
        name: BindingName,
    },
    /// Check that value at path equals the value already in the given slot.
    EqualCheck { path: MatchPathDyn, slot: u8 },
}

// ============================================================================
// Enhanced Matcher
// ============================================================================

/// Enhanced structural matcher with indexed slots and no depth limit.
///
/// Compiled from an LHS pattern at rule insertion time. Produces the same
/// `GenericBindings<V>` as `StructuralMatcher` but with:
/// - O(1) slot-indexed variable access (vs O(n) SmallVec search)
/// - No 8-level depth restriction
/// - Checks ordered by discriminative power (arity first, then atoms, then literals)
#[derive(Clone, Debug)]
pub struct EnhancedMatcher {
    /// Structural checks — ordered: arity checks first, then atoms, then literals.
    /// Fail-fast: first failing check aborts the match.
    checks: Vec<ECheck>,

    /// Variable slot operations — executed only if all checks pass.
    slot_ops: Vec<SlotOp>,

    /// Number of binding slots (distinct variables in the pattern).
    slot_count: u8,

    /// Mapping from slot index to variable name (for export to GenericBindings).
    slot_names: SmallVec<[BindingName; 8]>,

    /// Maximum pattern depth (for diagnostics).
    max_depth: u16,
}

impl EnhancedMatcher {
    /// Analyze an LHS pattern and compile into an EnhancedMatcher.
    ///
    /// Returns `None` only for truly unsupported patterns (Type, Conjunction,
    /// Error, Quoted nodes). Unlike `StructuralMatcher`, there is no depth limit.
    pub fn analyze<V: MettaValueTrait + Clone>(lhs: &V) -> Option<Self> {
        let mut arity_checks = Vec::new();
        let mut atom_checks = Vec::new();
        let mut literal_checks = Vec::new();
        let mut slot_ops = Vec::new();
        let mut seen_vars: SmallVec<[(BindingName, u8); 8]> = SmallVec::new();
        let mut slot_count: u8 = 0;
        let mut max_depth: u16 = 0;

        if !Self::analyze_node(
            lhs,
            MatchPathDyn::root(),
            &mut arity_checks,
            &mut atom_checks,
            &mut literal_checks,
            &mut slot_ops,
            &mut seen_vars,
            &mut slot_count,
            &mut max_depth,
            0,
        ) {
            return None;
        }

        // Order checks: arity first (cheapest, most discriminative), then atoms,
        // then literals. This maximizes fail-fast effectiveness.
        let mut checks =
            Vec::with_capacity(arity_checks.len() + atom_checks.len() + literal_checks.len());
        checks.extend(arity_checks);
        checks.extend(atom_checks);
        checks.extend(literal_checks);

        let slot_names: SmallVec<[BindingName; 8]> =
            seen_vars.iter().map(|(name, _)| name.clone()).collect();

        Some(EnhancedMatcher {
            checks,
            slot_ops,
            slot_count,
            slot_names,
            max_depth,
        })
    }

    /// Match an expression against this compiled pattern.
    ///
    /// Returns `Some(bindings)` on match, `None` on mismatch.
    pub fn try_match<V>(&self, expr: &V) -> Option<GenericBindings<V>>
    where
        V: MettaValueTrait + Clone + PartialEq,
    {
        // Phase 1: Structural checks (fail-fast)
        for check in &self.checks {
            if !self.execute_check(check, expr) {
                return None;
            }
        }

        // Phase 2: Slot operations
        let mut slots: SmallVec<[Option<V>; 8]> =
            smallvec::smallvec![None; self.slot_count as usize];
        // Extra bindings produced by bidirectional unification at EqualCheck
        // (variables not in the rule's slot table — typically free input
        // variables matched against already-bound rule variables).
        let mut extra: SmallVec<[(BindingName, V); 4]> = SmallVec::new();

        for op in &self.slot_ops {
            match op {
                SlotOp::Bind { path, slot, .. } => {
                    let val = path.navigate(expr)?;
                    slots[*slot as usize] = Some(val.clone());
                }
                SlotOp::EqualCheck { path, slot } => {
                    let val = path.navigate(expr)?;
                    let bound = slots[*slot as usize].as_ref()?;
                    if val != bound {
                        // Structural equality failed. Fall back to bidirectional
                        // (Martelli-Montanari) unification — see
                        // `StructuralMatcher::try_match` for the rationale (PLN
                        // Modus Ponens with free vars in implications).
                        let unify_bindings =
                            crate::backend::eval::bindings::bidirectional_unify_generic(
                                bound, val,
                            )?;
                        for (var_name, var_val) in unify_bindings.iter() {
                            // Check existing extras for conflict
                            if let Some((_, existing)) =
                                extra.iter().find(|(n, _)| n.matches(var_name))
                            {
                                if existing != var_val {
                                    return None;
                                }
                            } else {
                                extra.push((BindingName::from(var_name), var_val.clone()));
                            }
                        }
                    }
                }
            }
        }

        // Phase 3: Export to GenericBindings, including any extras
        let mut bindings = self.export_bindings(&slots);
        for (name, val) in extra {
            if bindings.get(name.as_str()).is_none() {
                bindings.insert(name, val);
            }
        }
        Some(bindings)
    }

    /// Match a template expression with lazy variable resolution.
    ///
    /// Equivalent to `apply_bindings(template, outer) |> try_match` but
    /// without materializing the substituted expression.
    pub fn try_match_with_bindings<V>(
        &self,
        template: &V,
        outer_bindings: &GenericBindings<V>,
    ) -> Option<GenericBindings<V>>
    where
        V: MettaValueTrait + Clone + PartialEq,
    {
        // Phase 1: Structural checks with resolution
        for check in &self.checks {
            if !self.execute_check_resolving(check, template, outer_bindings) {
                return None;
            }
        }

        // Phase 2: Slot operations with resolution
        let mut slots: SmallVec<[Option<V>; 8]> =
            smallvec::smallvec![None; self.slot_count as usize];
        let mut extra: SmallVec<[(BindingName, V); 4]> = SmallVec::new();

        for op in &self.slot_ops {
            match op {
                SlotOp::Bind { path, slot, .. } => {
                    let val = path.navigate_resolving(template, outer_bindings)?;
                    slots[*slot as usize] = Some(val);
                }
                SlotOp::EqualCheck { path, slot } => {
                    let val = path.navigate_resolving(template, outer_bindings)?;
                    let bound = slots[*slot as usize].as_ref()?;
                    if val != *bound {
                        // Bidirectional unification fallback (see try_match above).
                        let unify_bindings =
                            crate::backend::eval::bindings::bidirectional_unify_generic(
                                bound, &val,
                            )?;
                        for (var_name, var_val) in unify_bindings.iter() {
                            if let Some((_, existing)) =
                                extra.iter().find(|(n, _)| n.matches(var_name))
                            {
                                if existing != var_val {
                                    return None;
                                }
                            } else {
                                extra.push((BindingName::from(var_name), var_val.clone()));
                            }
                        }
                    }
                }
            }
        }

        let mut bindings = self.export_bindings(&slots);
        for (name, val) in extra {
            if bindings.get(name.as_str()).is_none() {
                bindings.insert(name, val);
            }
        }
        Some(bindings)
    }

    /// Return the number of variable slots.
    #[inline]
    pub fn slot_count(&self) -> u8 {
        self.slot_count
    }

    /// Return the number of structural checks.
    #[inline]
    pub fn check_count(&self) -> usize {
        self.checks.len()
    }

    /// Return the maximum pattern depth.
    #[inline]
    pub fn max_depth(&self) -> u16 {
        self.max_depth
    }

    // ── Private helpers ──────────────────────────────────────────────

    fn execute_check<V: MettaValueTrait>(&self, check: &ECheck, expr: &V) -> bool {
        match check {
            ECheck::Arity { path, expected } => path
                .navigate(expr)
                .and_then(|v| v.as_sexpr())
                .map_or(false, |items| items.len() == *expected as usize),
            ECheck::Atom { path, expected } => path
                .navigate(expr)
                .and_then(|v| v.as_atom())
                .map_or(false, |a| a == *expected),
            ECheck::Long { path, expected } => path
                .navigate(expr)
                .and_then(|v| v.as_long())
                .map_or(false, |n| n == *expected),
            ECheck::Bool { path, expected } => path
                .navigate(expr)
                .and_then(|v| v.as_bool())
                .map_or(false, |b| b == *expected),
            ECheck::Float {
                path,
                expected_bits,
            } => path
                .navigate(expr)
                .and_then(|v| v.as_float())
                .map_or(false, |f| f.to_bits() == *expected_bits),
            ECheck::Str { path, expected } => path
                .navigate(expr)
                .and_then(|v| v.as_string())
                .map_or(false, |s| s == *expected),
            ECheck::IsType { path } => path.navigate(expr).map_or(false, |v| v.is_type()),
            ECheck::IsQuoted { path } => path.navigate(expr).map_or(false, |v| v.is_quoted()),
            ECheck::IsError { path, expected_msg } => path
                .navigate(expr)
                .and_then(|v| v.as_error())
                .map_or(false, |(_, detail)| {
                    detail.as_string().map_or(false, |s| s == *expected_msg)
                }),
            ECheck::IsConjunction { path, expected_len } => path
                .navigate(expr)
                .and_then(|v| v.as_conjunction())
                .map_or(false, |goals| goals.len() == *expected_len as usize),
            ECheck::IsUnit { path } => path
                .navigate(expr)
                .map_or(false, |v| v.is_unit() || v.is_empty()),
        }
    }

    fn execute_check_resolving<V: MettaValueTrait + Clone>(
        &self,
        check: &ECheck,
        template: &V,
        bindings: &GenericBindings<V>,
    ) -> bool {
        match check {
            ECheck::Arity { path, expected } => path
                .navigate_resolving(template, bindings)
                .and_then(|v| v.as_sexpr().map(|items| items.len() == *expected as usize))
                .unwrap_or(false),
            ECheck::Atom { path, expected } => path
                .navigate_resolving(template, bindings)
                .and_then(|v| v.as_atom().map(|a| a == *expected))
                .unwrap_or(false),
            ECheck::Long { path, expected } => path
                .navigate_resolving(template, bindings)
                .and_then(|v| v.as_long().map(|n| n == *expected))
                .unwrap_or(false),
            ECheck::Bool { path, expected } => path
                .navigate_resolving(template, bindings)
                .and_then(|v| v.as_bool().map(|b| b == *expected))
                .unwrap_or(false),
            ECheck::Float {
                path,
                expected_bits,
            } => path
                .navigate_resolving(template, bindings)
                .and_then(|v| v.as_float().map(|f| f.to_bits() == *expected_bits))
                .unwrap_or(false),
            ECheck::Str { path, expected } => path
                .navigate_resolving(template, bindings)
                .and_then(|v| v.as_string().map(|s| s == *expected))
                .unwrap_or(false),
            ECheck::IsType { path } => path
                .navigate_resolving(template, bindings)
                .map_or(false, |v| v.is_type()),
            ECheck::IsQuoted { path } => path
                .navigate_resolving(template, bindings)
                .map_or(false, |v| v.is_quoted()),
            ECheck::IsError { path, expected_msg } => path
                .navigate_resolving(template, bindings)
                .and_then(|v| {
                    v.as_error()
                        .and_then(|(_, detail)| detail.as_string().map(|s| s == *expected_msg))
                })
                .unwrap_or(false),
            ECheck::IsConjunction { path, expected_len } => path
                .navigate_resolving(template, bindings)
                .and_then(|v| {
                    v.as_conjunction()
                        .map(|g| g.len() == *expected_len as usize)
                })
                .unwrap_or(false),
            ECheck::IsUnit { path } => path
                .navigate_resolving(template, bindings)
                .map_or(false, |v| v.is_unit() || v.is_empty()),
        }
    }

    fn export_bindings<V: MettaValueTrait + Clone>(
        &self,
        slots: &SmallVec<[Option<V>; 8]>,
    ) -> GenericBindings<V> {
        let mut bindings = GenericBindings::new();
        for (i, name) in self.slot_names.iter().enumerate() {
            if let Some(ref val) = slots[i] {
                bindings.insert(name.clone(), val.clone());
            }
        }
        bindings
    }

    fn analyze_node<V: MettaValueTrait + Clone>(
        value: &V,
        path: MatchPathDyn,
        arity_checks: &mut Vec<ECheck>,
        atom_checks: &mut Vec<ECheck>,
        literal_checks: &mut Vec<ECheck>,
        slot_ops: &mut Vec<SlotOp>,
        seen_vars: &mut SmallVec<[(BindingName, u8); 8]>,
        slot_count: &mut u8,
        max_depth: &mut u16,
        current_depth: u16,
    ) -> bool {
        if current_depth > *max_depth {
            *max_depth = current_depth;
        }

        // Strip span wrappers
        let value = if value.is_spanned() {
            value.strip_one_span()
        } else {
            value.clone()
        };

        // S-expression: arity check + recurse children.
        //
        // Dotted-pair pattern detection (2026-05-11): patterns of the form
        // `(a1 ... a_{n-2} . $rest)` are not supported by the EnhancedMatcher
        // (which assumes exact arity). Bail to pattern_match fallback.
        if let Some(items) = value.as_sexpr() {
            if items.len() >= 2 && items[items.len() - 2].as_atom() == Some(".") {
                return false;
            }
            // Cons-pattern detection (2026-05-21, PLN-Direct fix): `(cons HEAD
            // TAIL)` is a PT-canonical destructuring pattern binding HEAD↦v1
            // and TAIL↦(v2 ... vn). EnhancedMatcher assumes fixed arity; bail
            // to the factory-aware pattern_match fallback. See StructuralMatcher
            // bail-out for full PT-semantic citation.
            if items.len() == 3 && items[0].as_atom() == Some("cons") {
                return false;
            }
            arity_checks.push(ECheck::Arity {
                path: path.clone(),
                expected: items.len() as u16,
            });
            for (i, child) in items.iter().enumerate() {
                if !Self::analyze_node(
                    child,
                    path.child(i as u8),
                    arity_checks,
                    atom_checks,
                    literal_checks,
                    slot_ops,
                    seen_vars,
                    slot_count,
                    max_depth,
                    current_depth + 1,
                ) {
                    return false;
                }
            }
            return true;
        }

        // Atom: variable, wildcard, or concrete
        if let Some(atom) = value.as_atom() {
            if is_var_name(atom) {
                // Variable — check for repeats
                if let Some(pos) = seen_vars.iter().position(|(name, _)| name.matches(atom)) {
                    slot_ops.push(SlotOp::EqualCheck {
                        path,
                        slot: seen_vars[pos].1,
                    });
                } else {
                    let slot = *slot_count;
                    let name = BindingName::from(atom);
                    seen_vars.push((name.clone(), slot));
                    *slot_count += 1;
                    slot_ops.push(SlotOp::Bind {
                        path,
                        slot,
                        name,
                    });
                }
                return true;
            }
            if atom == "_" || atom == "$_" {
                return true; // Wildcard — no check
            }
            atom_checks.push(ECheck::Atom {
                path,
                expected: atom,
            });
            return true;
        }

        // Long
        if let Some(n) = value.as_long() {
            literal_checks.push(ECheck::Long { path, expected: n });
            return true;
        }

        // Bool
        if let Some(b) = value.as_bool() {
            literal_checks.push(ECheck::Bool { path, expected: b });
            return true;
        }

        // Float
        if let Some(f) = value.as_float() {
            literal_checks.push(ECheck::Float {
                path,
                expected_bits: f.to_bits(),
            });
            return true;
        }

        // String
        if let Some(s) = value.as_string() {
            let interned = crate::backend::models::gc_allocator::global_allocator().alloc_str(s);
            literal_checks.push(ECheck::Str {
                path,
                expected: interned,
            });
            return true;
        }

        // Unit / Empty — must match Unit exactly (NOT a wildcard).
        // GcFactory::sexpr(vec![]) converts empty S-expressions to Unit,
        // so () in rule patterns appears as Unit here.
        if value.is_unit() || value.is_empty() {
            literal_checks.push(ECheck::IsUnit { path });
            return true;
        }

        // Type wrapper: single child (the inner type value)
        if let Some(inner) = value.as_type() {
            arity_checks.push(ECheck::IsType { path: path.clone() });
            return Self::analyze_node(
                inner,
                path.child(0),
                arity_checks,
                atom_checks,
                literal_checks,
                slot_ops,
                seen_vars,
                slot_count,
                max_depth,
                current_depth + 1,
            );
        }

        // Quoted wrapper: single child (the quoted inner value)
        if let Some(inner) = value.as_quoted_ref() {
            arity_checks.push(ECheck::IsQuoted { path: path.clone() });
            return Self::analyze_node(
                inner,
                path.child(0),
                arity_checks,
                atom_checks,
                literal_checks,
                slot_ops,
                seen_vars,
                slot_count,
                max_depth,
                current_depth + 1,
            );
        }

        // Conjunction: N children (the goals)
        if let Some(goals) = value.as_conjunction() {
            arity_checks.push(ECheck::IsConjunction {
                path: path.clone(),
                expected_len: goals.len() as u16,
            });
            for (i, goal) in goals.iter().enumerate() {
                if !Self::analyze_node(
                    goal,
                    path.child(i as u8),
                    arity_checks,
                    atom_checks,
                    literal_checks,
                    slot_ops,
                    seen_vars,
                    slot_count,
                    max_depth,
                    current_depth + 1,
                ) {
                    return false;
                }
            }
            return true;
        }

        // Error: HE-bisimilar shape `(offending, detail)`. Slot 1 is the
        // offending expression (we navigate it as child 0); slot 2 is the
        // detail atom, typically a String carrying the human message that we
        // intern for the `IsError` literal-check.
        if let Some((offending, detail)) = value.as_error() {
            let detail_msg = detail.as_string().unwrap_or("");
            let interned_msg =
                crate::backend::models::gc_allocator::global_allocator().alloc_str(detail_msg);
            arity_checks.push(ECheck::IsError {
                path: path.clone(),
                expected_msg: interned_msg,
            });
            // Recurse into the offending expression as child 0
            return Self::analyze_node(
                offending,
                path.child(0),
                arity_checks,
                atom_checks,
                literal_checks,
                slot_ops,
                seen_vars,
                slot_count,
                max_depth,
                current_depth + 1,
            );
        }

        // Truly unsupported (should not occur with current MettaValue variants)
        false
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{global_factory, MettaValue, MettaValueFactory};

    fn f() -> crate::backend::models::GcFactory {
        global_factory()
    }

    #[test]
    fn test_simple_atom_pattern() {
        // Pattern: (f $x)
        let pattern = f().sexpr(vec![f().atom("f"), f().atom("$x")]);
        let matcher = EnhancedMatcher::analyze(&pattern).expect("should analyze");

        assert_eq!(matcher.slot_count(), 1);
        assert_eq!(matcher.check_count(), 2); // Arity + Atom("f")

        // Match (f 42) → {$x: 42}
        let expr = f().sexpr(vec![f().atom("f"), f().long(42)]);
        let bindings = matcher.try_match(&expr).expect("should match");
        assert_eq!(bindings.get("$x").expect("$x").as_long(), Some(42));
    }

    #[test]
    fn test_concrete_pattern() {
        // Pattern: (+ 1 2)
        let pattern = f().sexpr(vec![f().atom("+"), f().long(1), f().long(2)]);
        let matcher = EnhancedMatcher::analyze(&pattern).expect("should analyze");

        assert_eq!(matcher.slot_count(), 0);

        // Match (+ 1 2) → empty bindings
        let expr = f().sexpr(vec![f().atom("+"), f().long(1), f().long(2)]);
        assert!(matcher.try_match(&expr).is_some());

        // No match (+ 1 3)
        let expr2 = f().sexpr(vec![f().atom("+"), f().long(1), f().long(3)]);
        assert!(matcher.try_match(&expr2).is_none());
    }

    #[test]
    fn test_repeated_variable() {
        // Pattern: (eq $x $x) — repeated variable
        let pattern = f().sexpr(vec![f().atom("eq"), f().atom("$x"), f().atom("$x")]);
        let matcher = EnhancedMatcher::analyze(&pattern).expect("should analyze");

        assert_eq!(matcher.slot_count(), 1); // Only one distinct variable

        // Match (eq 5 5) → {$x: 5}
        let expr = f().sexpr(vec![f().atom("eq"), f().long(5), f().long(5)]);
        let bindings = matcher.try_match(&expr).expect("should match");
        assert_eq!(bindings.get("$x").expect("$x").as_long(), Some(5));

        // No match (eq 5 6) — different values for $x
        let expr2 = f().sexpr(vec![f().atom("eq"), f().long(5), f().long(6)]);
        assert!(matcher.try_match(&expr2).is_none());
    }

    #[test]
    fn test_nested_pattern() {
        // Pattern: (f (g $x) $y)
        let pattern = f().sexpr(vec![
            f().atom("f"),
            f().sexpr(vec![f().atom("g"), f().atom("$x")]),
            f().atom("$y"),
        ]);
        let matcher = EnhancedMatcher::analyze(&pattern).expect("should analyze");

        assert_eq!(matcher.slot_count(), 2);

        // Match (f (g 42) hello)
        let expr = f().sexpr(vec![
            f().atom("f"),
            f().sexpr(vec![f().atom("g"), f().long(42)]),
            f().atom("hello"),
        ]);
        let bindings = matcher.try_match(&expr).expect("should match");
        assert_eq!(bindings.get("$x").expect("$x").as_long(), Some(42));
        assert_eq!(bindings.get("$y").expect("$y").as_atom(), Some("hello"));
    }

    #[test]
    fn test_wildcard() {
        // Pattern: (f _ $x) — wildcard matches anything without binding
        let pattern = f().sexpr(vec![f().atom("f"), f().atom("_"), f().atom("$x")]);
        let matcher = EnhancedMatcher::analyze(&pattern).expect("should analyze");

        assert_eq!(matcher.slot_count(), 1); // Only $x, not _

        let expr = f().sexpr(vec![f().atom("f"), f().long(999), f().long(42)]);
        let bindings = matcher.try_match(&expr).expect("should match");
        assert_eq!(bindings.get("$x").expect("$x").as_long(), Some(42));
        assert!(bindings.get("_").is_none()); // Wildcard not bound
    }

    #[test]
    fn test_arity_mismatch() {
        // Pattern: (f $x $y) — arity 3
        let pattern = f().sexpr(vec![f().atom("f"), f().atom("$x"), f().atom("$y")]);
        let matcher = EnhancedMatcher::analyze(&pattern).expect("should analyze");

        // No match: (f 1) — arity 2
        let expr = f().sexpr(vec![f().atom("f"), f().long(1)]);
        assert!(matcher.try_match(&expr).is_none());

        // No match: (f 1 2 3) — arity 4
        let expr2 = f().sexpr(vec![f().atom("f"), f().long(1), f().long(2), f().long(3)]);
        assert!(matcher.try_match(&expr2).is_none());
    }

    #[test]
    fn test_deep_pattern_no_limit() {
        // Build a pattern 12 levels deep — exceeds old 8-level limit
        // (a (b (c (d (e (f (g (h (i (j (k (l $x))))))))))))
        let mut pattern = f().atom("$x");
        for name in ["l", "k", "j", "i", "h", "g", "f", "e", "d", "c", "b", "a"] {
            pattern = f().sexpr(vec![f().atom(name), pattern]);
        }

        let matcher = EnhancedMatcher::analyze(&pattern).expect("should handle depth > 8");
        assert!(matcher.max_depth() >= 12);
        assert_eq!(matcher.slot_count(), 1);

        // Build matching expression
        let mut expr = f().long(42);
        for name in ["l", "k", "j", "i", "h", "g", "f", "e", "d", "c", "b", "a"] {
            expr = f().sexpr(vec![f().atom(name), expr]);
        }

        let bindings = matcher.try_match(&expr).expect("should match at depth 12");
        assert_eq!(bindings.get("$x").expect("$x").as_long(), Some(42));
    }

    #[test]
    fn test_check_ordering() {
        // Pattern: (f 42 $x) — should have arity check first, then atom, then long
        let pattern = f().sexpr(vec![f().atom("f"), f().long(42), f().atom("$x")]);
        let matcher = EnhancedMatcher::analyze(&pattern).expect("should analyze");

        // Verify arity checks come first
        let first_check = &matcher.checks[0];
        assert!(
            matches!(first_check, ECheck::Arity { .. }),
            "First check should be Arity, got {:?}",
            first_check
        );
    }

    #[test]
    fn test_bool_pattern() {
        let pattern = f().sexpr(vec![f().atom("test"), f().bool(true)]);
        let matcher = EnhancedMatcher::analyze(&pattern).expect("should analyze");

        let expr = f().sexpr(vec![f().atom("test"), f().bool(true)]);
        assert!(matcher.try_match(&expr).is_some());

        let expr2 = f().sexpr(vec![f().atom("test"), f().bool(false)]);
        assert!(matcher.try_match(&expr2).is_none());
    }

    #[test]
    fn test_match_with_bindings() {
        // Pattern: (f 1 $y)
        let pattern = f().sexpr(vec![f().atom("f"), f().long(1), f().atom("$y")]);
        let matcher = EnhancedMatcher::analyze(&pattern).expect("should analyze");

        // Template: (f $a $b) with outer bindings {$a: 1, $b: hello}
        let template = f().sexpr(vec![f().atom("f"), f().atom("$a"), f().atom("$b")]);
        let mut outer = GenericBindings::new();
        outer.insert("$a", f().long(1));
        outer.insert("$b", f().atom("hello"));

        let bindings = matcher
            .try_match_with_bindings(&template, &outer)
            .expect("should match after resolution");
        assert_eq!(bindings.get("$y").expect("$y").as_atom(), Some("hello"));
    }

    #[test]
    fn test_match_with_bindings_conflict() {
        // Pattern: (f 1 $y)
        let pattern = f().sexpr(vec![f().atom("f"), f().long(1), f().atom("$y")]);
        let matcher = EnhancedMatcher::analyze(&pattern).expect("should analyze");

        // Template: (f $a $b) with outer bindings {$a: 2} — doesn't match pattern's 1
        let template = f().sexpr(vec![f().atom("f"), f().atom("$a"), f().atom("$b")]);
        let mut outer = GenericBindings::new();
        outer.insert("$a", f().long(2));
        outer.insert("$b", f().atom("hello"));

        assert!(matcher.try_match_with_bindings(&template, &outer).is_none());
    }

    #[test]
    fn test_multiple_variables() {
        // Pattern: (op $a $b $c $d $e)
        let pattern = f().sexpr(vec![
            f().atom("op"),
            f().atom("$a"),
            f().atom("$b"),
            f().atom("$c"),
            f().atom("$d"),
            f().atom("$e"),
        ]);
        let matcher = EnhancedMatcher::analyze(&pattern).expect("should analyze");
        assert_eq!(matcher.slot_count(), 5);

        let expr = f().sexpr(vec![
            f().atom("op"),
            f().long(1),
            f().long(2),
            f().long(3),
            f().long(4),
            f().long(5),
        ]);
        let bindings = matcher.try_match(&expr).expect("should match");
        assert_eq!(bindings.get("$a").expect("a").as_long(), Some(1));
        assert_eq!(bindings.get("$e").expect("e").as_long(), Some(5));
    }
}
