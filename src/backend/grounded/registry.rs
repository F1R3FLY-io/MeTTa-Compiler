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
    AbsOp, AddOp, ClampOp, DivOp, MaxOp, MinOp, ModOp, MulOp, SafeDivOp, SubOp,
};
use super::comparison::{EqualOp, GreaterEqOp, GreaterOp, LessEqOp, LessOp, NotEqualOp};
use super::fileio::{
    FileGetSizeOp, FileOpenOp, FileReadExactOp, FileReadToStringOp, FileSeekOp, FileWriteOp,
};
use super::json::{JsonDecodeOp, JsonEncodeOp};
use super::logical::{AndOp, NotOp, OrOp, XorOp};
use super::math::{
    AbsMathOp, AcosMathOp, AsinMathOp, AtanMathOp, CeilMathOp, CosMathOp, FloorMathOp, IsInfMathOp,
    IsNanMathOp, LogMathOp, MaxAtomOp, MinAtomOp, PowMathOp, SinMathOp, SqrtMathOp, TanMathOp,
    TruncMathOp,
};
use super::meta::IdOp;
use super::pt_parser::{ParseOp, ReprOp, SreadOp, SwriteOp};
use super::random::{
    FlipOp, NewRandomGeneratorOp, RandomFloatOp, RandomIntOp, ResetRandomGeneratorOp,
    SetRandomSeedOp,
};
use super::state::{GroundedState, GroundedWork};
use super::string::{SortStringsOp, StringToCharsOp};
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
        "abs" => Some(AbsOp.execute_step(state, factory)),
        // PT *-math suite (Phase 6.X, 2026-05-21)
        "sqrt-math" => Some(SqrtMathOp.execute_step(state, factory)),
        "abs-math" => Some(AbsMathOp.execute_step(state, factory)),
        "trunc-math" => Some(TruncMathOp.execute_step(state, factory)),
        "ceil-math" => Some(CeilMathOp.execute_step(state, factory)),
        "floor-math" => Some(FloorMathOp.execute_step(state, factory)),
        "sin-math" => Some(SinMathOp.execute_step(state, factory)),
        "cos-math" => Some(CosMathOp.execute_step(state, factory)),
        "tan-math" => Some(TanMathOp.execute_step(state, factory)),
        "asin-math" => Some(AsinMathOp.execute_step(state, factory)),
        "acos-math" => Some(AcosMathOp.execute_step(state, factory)),
        "atan-math" => Some(AtanMathOp.execute_step(state, factory)),
        "pow-math" => Some(PowMathOp.execute_step(state, factory)),
        "log-math" => Some(LogMathOp.execute_step(state, factory)),
        "isnan-math" => Some(IsNanMathOp.execute_step(state, factory)),
        "isinf-math" => Some(IsInfMathOp.execute_step(state, factory)),
        "min-atom" => Some(MinAtomOp.execute_step(state, factory)),
        "max-atom" => Some(MaxAtomOp.execute_step(state, factory)),
        // PT parser ops (Phase 6.X, 2026-05-21)
        "parse" => Some(ParseOp.execute_step(state, factory)),
        "sread" => Some(SreadOp.execute_step(state, factory)),
        "swrite" => Some(SwriteOp.execute_step(state, factory)),
        "repr" => Some(ReprOp.execute_step(state, factory)),
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
        // String operations (Workstream X.5a)
        "stringToChars" => Some(StringToCharsOp.execute_step(state, factory)),
        // String operations (T06/060 — sort-strings, HE-aligned)
        "sort-strings" => Some(SortStringsOp.execute_step(state, factory)),
        // Meta / polymorphic operations (T06/037 — id, HE-aligned)
        "id" => Some(IdOp.execute_step(state, factory)),
        // JSON module (T07/019-020, HE-aligned: `json` builtin)
        // Phase 8.3 PT-canonical aliases: parse_json/format_json
        "json-encode" | "format_json" => Some(JsonEncodeOp.execute_step(state, factory)),
        "json-decode" | "parse_json" => Some(JsonDecodeOp.execute_step(state, factory)),
        // FileIO module (T07/021, HE-aligned: `fileio` builtin)
        "file-open!" => Some(FileOpenOp.execute_step(state, factory)),
        "file-read-to-string!" => Some(FileReadToStringOp.execute_step(state, factory)),
        "file-write!" => Some(FileWriteOp.execute_step(state, factory)),
        "file-seek!" => Some(FileSeekOp.execute_step(state, factory)),
        "file-read-exact!" => Some(FileReadExactOp.execute_step(state, factory)),
        "file-get-size!" => Some(FileGetSizeOp.execute_step(state, factory)),
        // Random module (T07/022, HE-aligned: `random` builtin)
        "new-random-generator" => Some(NewRandomGeneratorOp.execute_step(state, factory)),
        "random-int" => Some(RandomIntOp.execute_step(state, factory)),
        "random-float" => Some(RandomFloatOp.execute_step(state, factory)),
        "set-random-seed" => Some(SetRandomSeedOp.execute_step(state, factory)),
        "reset-random-generator" => Some(ResetRandomGeneratorOp.execute_step(state, factory)),
        "flip" => Some(FlipOp.execute_step(state, factory)),
        // Phase 8.5 CLP(FD) — PT exposes #+/#-/#*/#div/#//mod/#min/#max/
        // #</#>/#=/#\= for integer constraints. PT never labels (spec/15.12);
        // for ground integer args MTT routes to the underlying arithmetic/
        // comparison op so ground evaluation is identity (CLP(FD) is the
        // residual-constraint model). For non-ground args the operations
        // pass through unreduced (residual constraint leakage per spec).
        "#+" => Some(AddOp.execute_step(state, factory)),
        "#-" => Some(SubOp.execute_step(state, factory)),
        "#*" => Some(MulOp.execute_step(state, factory)),
        "#div" => Some(DivOp.execute_step(state, factory)),
        "#//" => Some(DivOp.execute_step(state, factory)),
        "#mod" => Some(ModOp.execute_step(state, factory)),
        "#min" => Some(MinOp.execute_step(state, factory)),
        "#max" => Some(MaxOp.execute_step(state, factory)),
        "#<" => Some(LessOp.execute_step(state, factory)),
        "#>" => Some(GreaterOp.execute_step(state, factory)),
        "#=" => Some(EqualOp.execute_step(state, factory)),
        "#\\=" => Some(NotEqualOp.execute_step(state, factory)),
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
        "+" | "-"
            | "*"
            | "/"
            | "%"
            | "min"
            | "max"
            | "abs"
            // PT *-math suite (Phase 6.X, 2026-05-21)
            | "sqrt-math"
            | "abs-math"
            | "trunc-math"
            | "ceil-math"
            | "floor-math"
            | "sin-math"
            | "cos-math"
            | "tan-math"
            | "asin-math"
            | "acos-math"
            | "atan-math"
            | "pow-math"
            | "log-math"
            | "isnan-math"
            | "isinf-math"
            | "min-atom"
            | "max-atom"
            // PT parser ops (Phase 6.X, 2026-05-21)
            | "parse"
            | "sread"
            | "swrite"
            | "repr"
            | "<"
            | "<="
            | ">"
            | ">="
            | "=="
            | "!="
            | "and"
            | "or"
            | "not"
            | "xor"
            | "/safe"
            | "clamp"
            | "stringToChars"
            | "sort-strings"
            | "id"
            // JSON module (T07/019-020) + PT-canonical aliases
            | "json-encode"
            | "json-decode"
            | "format_json"
            | "parse_json"
            // FileIO module (T07/021)
            | "file-open!"
            | "file-read-to-string!"
            | "file-write!"
            | "file-seek!"
            | "file-read-exact!"
            | "file-get-size!"
            // Random module (T07/022)
            | "new-random-generator"
            | "random-int"
            | "random-float"
            | "set-random-seed"
            | "reset-random-generator"
            | "flip"
            // Phase 8.5 CLP(FD) — PT integer constraint operations
            | "#+"
            | "#-"
            | "#*"
            | "#div"
            | "#//"
            | "#mod"
            | "#min"
            | "#max"
            | "#<"
            | "#>"
            | "#="
            | "#\\="
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
    // (convert) Active factory so these grounded-registry tests run under both
    // the slab `GcFactory` and the index `IndexFactory` (GC A/B differential).
    use crate::backend::models::{active_factory, MettaValue};

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
        let mut state = GroundedState::new("nonexistent".to_string(), vec![MettaValue::Long(1)]);

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

        // String ops
        assert!(has_grounded_op("stringToChars"));
        assert!(has_grounded_op("sort-strings"));

        // Meta ops
        assert!(has_grounded_op("id"));

        // Non-existent ops
        assert!(!has_grounded_op("nonexistent"));
        assert!(!has_grounded_op("if"));
        assert!(!has_grounded_op("match"));
    }

    #[test]
    fn test_execute_grounded_op_id() {
        let factory = active_factory();

        // Test id with a single argument
        let mut state = GroundedState::new("id".to_string(), vec![MettaValue::Long(42)]);

        // Step 0: Request eval of arg 0
        let work = execute_grounded_op("id", &mut state, &factory);
        match work.expect("id should dispatch") {
            GroundedWork::EvalArg { arg_idx, .. } => {
                assert_eq!(arg_idx, 0);
            }
            _ => panic!("Expected EvalArg"),
        }

        // Simulate arg 0 evaluated → still Long(42)
        state.set_arg(0, vec![MettaValue::Long(42)]);
        state.step = 1;

        // Step 1: Compute result
        let work = execute_grounded_op("id", &mut state, &factory);
        match work.expect("id should produce result") {
            GroundedWork::Done(results) => {
                assert_eq!(results.len(), 1);
                assert_eq!(results[0].0.as_long(), Some(42));
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_execute_grounded_op_sort_strings() {
        let factory = active_factory();

        // Build (sort-strings ("c" "a" "b"))
        let list = MettaValue::SExpr(vec![
            MettaValue::String("c".to_string()),
            MettaValue::String("a".to_string()),
            MettaValue::String("b".to_string()),
        ]);
        let mut state = GroundedState::new("sort-strings".to_string(), vec![list.clone()]);

        // Step 0: request eval of arg 0
        let work = execute_grounded_op("sort-strings", &mut state, &factory);
        match work.expect("sort-strings should dispatch") {
            GroundedWork::EvalArg { arg_idx, .. } => {
                assert_eq!(arg_idx, 0);
            }
            _ => panic!("Expected EvalArg"),
        }

        // Simulate arg 0 evaluated → still the list
        state.set_arg(0, vec![list]);
        state.step = 1;

        // Step 1: compute sorted result
        let work = execute_grounded_op("sort-strings", &mut state, &factory);
        match work.expect("sort-strings should produce result") {
            GroundedWork::Done(results) => {
                assert_eq!(results.len(), 1);
                let sorted = &results[0].0;
                let items = sorted.as_sexpr().expect("result is sexpr");
                assert_eq!(items.len(), 3);
                assert_eq!(items[0].as_string(), Some("a"));
                assert_eq!(items[1].as_string(), Some("b"));
                assert_eq!(items[2].as_string(), Some("c"));
            }
            _ => panic!("Expected Done"),
        }
    }

    #[test]
    fn test_execute_grounded_op_addition() {
        let factory = active_factory();

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
        let factory = active_factory();

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
        let factory = active_factory();

        // Test 'not' with static dispatch
        let mut state = GroundedState::new("not".to_string(), vec![MettaValue::Bool(true)]);

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
        let factory = active_factory();
        let mut state = GroundedState::new("nonexistent".to_string(), vec![MettaValue::Long(1)]);

        let work = execute_grounded_op("nonexistent", &mut state, &factory);
        assert!(work.is_none());
    }
}
