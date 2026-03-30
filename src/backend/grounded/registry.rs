//! Grounded Operations Registry.
//!
//! This module provides two approaches for executing generic grounded operations:
//!
//! 1. **Static Dispatch Functions** (preferred) - Zero-cost abstraction via match statements
//! 2. **Type-Erased Registry** (legacy) - HashMap-based lookup with dynamic dispatch
//!
//! ## Design
//!
//! The static dispatch approach uses match expressions for zero-cost abstraction:
//! - **True genericity**: Works with any `V: MettaValueTrait`, not just `MettaValue`
//! - **Zero runtime overhead**: Monomorphization generates specialized code
//! - **No trait object allocation**: No vtable dispatch, no `Box<dyn ...>`
//! - **Inlinable**: Compiler can inline the entire operation
//!
//! ## Zero-Conversion Pattern
//!
//! By using the static dispatch functions, evaluation code can:
//! 1. Check operation existence with `has_grounded_op()`
//! 2. Execute operations with `execute_grounded_op()`
//! 3. Work with any value type without conversion
//!
//! ## Usage
//!
//! ```ignore
//! use crate::backend::grounded::{
//!     execute_grounded_op, has_grounded_op, GroundedState,
//! };
//!
//! // Check if operation exists (O(1) via match)
//! if has_grounded_op("+") {
//!     // Execute with generic factory - works with any V: MettaValueTrait
//!     let work = execute_grounded_op("+", &mut state, &factory);
//! }
//! ```

use std::collections::HashMap;

use super::arithmetic::{
    AddOp, ClampOp, DivOp, MaxOp, MinOp, ModOp,
    MulOp, SafeDivOp, SubOp,
};
use super::comparison::{
    EqualOp, GreaterEqOp, GreaterOp, LessEqOp, LessOp,
    NotEqualOp,
};
use super::logical::{AndOp, NotOp, OrOp, XorOp};
use super::state::{GroundedState, GroundedWork};
use super::traits::GroundedOperationTCO;
use crate::backend::models::{MettaValueFactory, MettaValueTrait};

// ============================================================================
// Static Dispatch Functions (Zero-Cost Abstraction)
// ============================================================================

/// Execute a generic grounded operation using static dispatch.
///
/// This function uses a match statement for zero-cost abstraction - the compiler
/// monomorphizes this for each value type V, eliminating all trait object overhead.
///
/// # Type Parameters
/// - `V`: Any value type implementing `MettaValueTrait`
/// - `F`: Factory for constructing values of type `V`
///
/// # Returns
/// - `Some(work)` if the operation was found and executed
/// - `None` if the operation was not found (not a known grounded op)
///
/// # Example
/// ```ignore
/// let work = execute_grounded_op("+", &mut state, &factory);
/// ```
#[inline]
pub fn execute_grounded_op<V, F>(
    name: &str,
    state: &mut GroundedState<V>,
    factory: &F,
) -> Option<GroundedWork<V>>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + 'static,
    F: MettaValueFactory<V>,
{
    match name {
        // Arithmetic operations
        "+" => Some(AddOp.execute_step(state, factory)),
        "-" => Some(SubOp.execute_step(state, factory)),
        "*" => Some(MulOp.execute_step(state, factory)),
        "/" => Some(DivOp.execute_step(state, factory)),
        "%" => Some(ModOp.execute_step(state, factory)),
        "min" => Some(MinOp.execute_step(state, factory)),
        "max" => Some(MaxOp.execute_step(state, factory)),
        // Comparison operations
        "<" => Some(LessOp.execute_step(state, factory)),
        "<=" => Some(LessEqOp.execute_step(state, factory)),
        ">" => Some(GreaterOp.execute_step(state, factory)),
        ">=" => Some(GreaterEqOp.execute_step(state, factory)),
        "==" => Some(EqualOp.execute_step(state, factory)),
        "!=" => Some(NotEqualOp.execute_step(state, factory)),
        // Logical operations
        "and" => Some(AndOp.execute_step(state, factory)),
        "or" => Some(OrOp.execute_step(state, factory)),
        "not" => Some(NotOp.execute_step(state, factory)),
        "xor" => Some(XorOp.execute_step(state, factory)),
        // Safe arithmetic utilities
        "/safe" => Some(SafeDivOp.execute_step(state, factory)),
        "clamp" => Some(ClampOp.execute_step(state, factory)),
        // Unknown operation - not a grounded op
        _ => None,
    }
}

/// Check if a generic grounded operation exists.
///
/// Uses a match statement for O(1) lookup. This is used to determine if an
/// S-expression should be dispatched to the grounded operation path.
///
/// # Example
/// ```ignore
/// if has_grounded_op("+") {
///     // Dispatch to grounded op handling
/// }
/// ```
#[inline]
pub fn has_grounded_op(name: &str) -> bool {
    matches!(
        name,
        "+" | "-" | "*" | "/" | "%" | "min" | "max" |
        "<" | "<=" | ">" | ">=" | "==" | "!=" |
        "and" | "or" | "not" | "xor" |
        "/safe" | "clamp"
    )
}

// ============================================================================
// Type-Erased Registry (Legacy - for backward compatibility)
// ============================================================================

/// Trait for type-erased generic grounded operations.
///
/// This trait enables storing operations in a HashMap while supporting
/// any value type at execution time.
trait GroundedOpErased: Send + Sync {
    /// Get the operation name.
    fn name(&self) -> &str;

    /// Execute one step of the operation with heap values.
    ///
    /// This is a type-erased wrapper that works with `MettaValue` (the concrete heap type).
    fn execute_step_erased(
        &self,
        state: &mut GroundedState<crate::backend::models::MettaValue>,
        factory: &crate::backend::models::GcFactory,
    ) -> GroundedWork<crate::backend::models::MettaValue>;
}

/// Wrapper struct to implement `GroundedOpErased` for any `GroundedOperationTCO`.
struct OpWrapper<Op>(Op);

impl<Op> GroundedOpErased for OpWrapper<Op>
where
    Op: GroundedOperationTCO<crate::backend::models::MettaValue> + Send + Sync,
{
    fn name(&self) -> &str {
        self.0.name()
    }

    fn execute_step_erased(
        &self,
        state: &mut GroundedState<crate::backend::models::MettaValue>,
        factory: &crate::backend::models::GcFactory,
    ) -> GroundedWork<crate::backend::models::MettaValue> {
        self.0.execute_step(state, factory)
    }
}

/// Registry of generic grounded operations.
///
/// Operations are stored as type-erased trait objects that can execute
/// with any value type via the provided factory.
pub struct GroundedRegistry {
    operations: HashMap<String, Box<dyn GroundedOpErased>>,
}

impl GroundedRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        GroundedRegistry {
            operations: HashMap::new(),
        }
    }

    /// Create a registry with all standard generic operations.
    ///
    /// Includes:
    /// - Arithmetic: +, -, *, /, %
    /// - Comparison: <, <=, >, >=, ==, !=
    /// - Logical: and, or, not
    pub fn with_standard_ops() -> Self {
        let mut registry = Self::new();

        // Arithmetic operations
        registry.register(Box::new(OpWrapper(AddOp)));
        registry.register(Box::new(OpWrapper(SubOp)));
        registry.register(Box::new(OpWrapper(MulOp)));
        registry.register(Box::new(OpWrapper(DivOp)));
        registry.register(Box::new(OpWrapper(ModOp)));
        registry.register(Box::new(OpWrapper(MinOp)));
        registry.register(Box::new(OpWrapper(MaxOp)));

        // Comparison operations
        registry.register(Box::new(OpWrapper(LessOp)));
        registry.register(Box::new(OpWrapper(LessEqOp)));
        registry.register(Box::new(OpWrapper(GreaterOp)));
        registry.register(Box::new(OpWrapper(GreaterEqOp)));
        registry.register(Box::new(OpWrapper(EqualOp)));
        registry.register(Box::new(OpWrapper(NotEqualOp)));

        // Logical operations
        registry.register(Box::new(OpWrapper(AndOp)));
        registry.register(Box::new(OpWrapper(OrOp)));
        registry.register(Box::new(OpWrapper(NotOp)));
        registry.register(Box::new(OpWrapper(XorOp)));

        // Safe arithmetic utilities
        registry.register(Box::new(OpWrapper(SafeDivOp)));
        registry.register(Box::new(OpWrapper(ClampOp)));

        registry
    }

    /// Register a generic grounded operation.
    fn register(&mut self, op: Box<dyn GroundedOpErased>) {
        self.operations.insert(op.name().to_string(), op);
    }

    /// Look up a generic grounded operation by name.
    ///
    /// This is internal - use `execute_step_heap` for the public API.
    fn get(&self, name: &str) -> Option<&dyn GroundedOpErased> {
        self.operations.get(name).map(|b| b.as_ref())
    }

    /// Check if an operation exists in the registry.
    pub fn contains(&self, name: &str) -> bool {
        self.operations.contains_key(name)
    }

    /// Get the number of registered operations.
    pub fn len(&self) -> usize {
        self.operations.len()
    }

    /// Check if the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }

    /// Get an iterator over operation names.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.operations.keys().map(|s| s.as_str())
    }

    /// Execute one step of a grounded operation using heap values.
    ///
    /// This is a convenience method that looks up the operation and executes it.
    ///
    /// # Returns
    ///
    /// - `Some(work)` if the operation was found and executed
    /// - `None` if the operation was not found
    pub fn execute_step_heap(
        &self,
        name: &str,
        state: &mut GroundedState<crate::backend::models::MettaValue>,
    ) -> Option<GroundedWork<crate::backend::models::MettaValue>> {
        let op = self.get(name)?;
        let factory = crate::backend::models::GcFactory::default();
        Some(op.execute_step_erased(state, &factory))
    }
}

impl Default for GroundedRegistry {
    fn default() -> Self {
        Self::with_standard_ops()
    }
}

impl Clone for GroundedRegistry {
    fn clone(&self) -> Self {
        // Recreate with standard ops since we can't clone trait objects
        Self::with_standard_ops()
    }
}

/// Static generic registry for global access.
///
/// This provides a singleton-like pattern for the standard generic operations.
/// Used by the trampoline engine when executing grounded operations.
pub fn get_grounded_registry() -> &'static GroundedRegistry {
    use std::sync::OnceLock;
    static REGISTRY: OnceLock<GroundedRegistry> = OnceLock::new();
    REGISTRY.get_or_init(GroundedRegistry::with_standard_ops)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::{GcFactory, MettaValue};

    #[test]
    fn test_registry_with_standard_ops() {
        let registry = GroundedRegistry::with_standard_ops();

        // Check all arithmetic ops are registered
        assert!(registry.contains("+"));
        assert!(registry.contains("-"));
        assert!(registry.contains("*"));
        assert!(registry.contains("/"));
        assert!(registry.contains("%"));
        assert!(registry.contains("min"));
        assert!(registry.contains("max"));

        // Check all comparison ops are registered
        assert!(registry.contains("<"));
        assert!(registry.contains("<="));
        assert!(registry.contains(">"));
        assert!(registry.contains(">="));
        assert!(registry.contains("=="));
        assert!(registry.contains("!="));

        // Check all logical ops are registered
        assert!(registry.contains("and"));
        assert!(registry.contains("or"));
        assert!(registry.contains("not"));

        // Check safe arithmetic utilities are registered
        assert!(registry.contains("/safe"));
        assert!(registry.contains("clamp"));
    }

    #[test]
    fn test_execute_step_heap() {
        let registry = GroundedRegistry::with_standard_ops();
        // Test addition
        let mut state = GroundedState::new(
            "+".to_string(),
            vec![MettaValue::Long(2), MettaValue::Long(3)],
        );

        // Step 0: Request eval of arg 0
        let work = registry.execute_step_heap("+", &mut state);
        assert!(work.is_some());
        match work.unwrap() {
            GroundedWork::EvalArg { arg_idx, .. } => {
                assert_eq!(arg_idx, 0);
            }
            _ => panic!("Expected EvalArg"),
        }

        // Simulate arg 0 evaluated
        state.set_arg(0, vec![MettaValue::Long(2)]);
        state.step = 1;

        // Step 1: Request eval of arg 1
        let work = registry.execute_step_heap("+", &mut state);
        match work.unwrap() {
            GroundedWork::EvalArg { arg_idx, .. } => {
                assert_eq!(arg_idx, 1);
            }
            _ => panic!("Expected EvalArg"),
        }

        // Simulate arg 1 evaluated
        state.set_arg(1, vec![MettaValue::Long(3)]);
        state.step = 2;

        // Step 2: Compute result
        let work = registry.execute_step_heap("+", &mut state);
        match work.unwrap() {
            GroundedWork::Done(results) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].0.as_long(), Some(5));
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_get_grounded_registry() {
        // Verify it's the same instance
        let registry = get_grounded_registry();
        let registry2 = get_grounded_registry();
        assert!(std::ptr::eq(registry, registry2));
    }

    #[test]
    fn test_operation_not_found() {
        let registry = GroundedRegistry::with_standard_ops();
        let mut state = GroundedState::new(
            "nonexistent".to_string(),
            vec![MettaValue::Long(1)],
        );

        let work = registry.execute_step_heap("nonexistent", &mut state);
        assert!(work.is_none());
    }

    // =========================================================================
    // Tests for static dispatch functions
    // =========================================================================

    #[test]
    fn test_has_grounded_op() {
        // Arithmetic ops
        assert!(has_grounded_op("+"));
        assert!(has_grounded_op("-"));
        assert!(has_grounded_op("*"));
        assert!(has_grounded_op("/"));
        assert!(has_grounded_op("%"));
        assert!(has_grounded_op("min"));
        assert!(has_grounded_op("max"));

        // Comparison ops
        assert!(has_grounded_op("<"));
        assert!(has_grounded_op("<="));
        assert!(has_grounded_op(">"));
        assert!(has_grounded_op(">="));
        assert!(has_grounded_op("=="));
        assert!(has_grounded_op("!="));

        // Logical ops
        assert!(has_grounded_op("and"));
        assert!(has_grounded_op("or"));
        assert!(has_grounded_op("not"));

        // Non-existent ops
        assert!(!has_grounded_op("nonexistent"));
        assert!(!has_grounded_op("if"));
        assert!(!has_grounded_op("match"));
    }

    #[test]
    fn test_execute_grounded_op_addition() {
        let factory = GcFactory::default();

        // Test addition with static dispatch
        let mut state = GroundedState::new(
            "+".to_string(),
            vec![MettaValue::Long(2), MettaValue::Long(3)],
        );

        // Step 0: Request eval of arg 0
        let work = execute_grounded_op("+", &mut state, &factory);
        assert!(work.is_some());
        match work.unwrap() {
            GroundedWork::EvalArg { arg_idx, .. } => {
                assert_eq!(arg_idx, 0);
            }
            _ => panic!("Expected EvalArg"),
        }

        // Simulate arg 0 evaluated
        state.set_arg(0, vec![MettaValue::Long(2)]);
        state.step = 1;

        // Step 1: Request eval of arg 1
        let work = execute_grounded_op("+", &mut state, &factory);
        match work.unwrap() {
            GroundedWork::EvalArg { arg_idx, .. } => {
                assert_eq!(arg_idx, 1);
            }
            _ => panic!("Expected EvalArg"),
        }

        // Simulate arg 1 evaluated
        state.set_arg(1, vec![MettaValue::Long(3)]);
        state.step = 2;

        // Step 2: Compute result
        let work = execute_grounded_op("+", &mut state, &factory);
        match work.unwrap() {
            GroundedWork::Done(results) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].0.as_long(), Some(5));
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_execute_grounded_op_comparison() {
        let factory = GcFactory::default();

        // Test less-than with static dispatch
        let mut state = GroundedState::new(
            "<".to_string(),
            vec![MettaValue::Long(2), MettaValue::Long(5)],
        );

        // Run all steps
        execute_grounded_op("<", &mut state, &factory);
        state.set_arg(0, vec![MettaValue::Long(2)]);
        state.step = 1;

        execute_grounded_op("<", &mut state, &factory);
        state.set_arg(1, vec![MettaValue::Long(5)]);
        state.step = 2;

        let work = execute_grounded_op("<", &mut state, &factory);
        match work.unwrap() {
            GroundedWork::Done(results) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].0.as_bool(), Some(true)); // 2 < 5
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_execute_grounded_op_logical() {
        let factory = GcFactory::default();

        // Test 'not' with static dispatch
        let mut state = GroundedState::new(
            "not".to_string(),
            vec![MettaValue::Bool(true)],
        );

        // Run all steps
        execute_grounded_op("not", &mut state, &factory);
        state.set_arg(0, vec![MettaValue::Bool(true)]);
        state.step = 1;

        let work = execute_grounded_op("not", &mut state, &factory);
        match work.unwrap() {
            GroundedWork::Done(results) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].0.as_bool(), Some(false)); // not true = false
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_execute_grounded_op_not_found() {
        let factory = GcFactory::default();
        let mut state = GroundedState::new(
            "nonexistent".to_string(),
            vec![MettaValue::Long(1)],
        );

        let work = execute_grounded_op("nonexistent", &mut state, &factory);
        assert!(work.is_none());
    }
}
