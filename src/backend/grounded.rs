//! Grounded operations for lazy evaluation.
//!
//! This module provides the `GroundedOperation` trait and implementations for
//! built-in operations that receive unevaluated arguments and evaluate them internally.
//! This matches Hyperon Experimental's (HE) `execute_bindings()` pattern.
//!
//! # Key Concepts
//!
//! - **Lazy Evaluation**: Arguments are passed unevaluated to grounded operations
//! - **Internal Evaluation**: Operations decide when/if to evaluate their arguments
//! - **Cartesian Products**: When arguments produce multiple results, operations
//!   compute Cartesian products of all result combinations
//!
//! # Example
//!
//! ```ignore
//! // With lazy evaluation:
//! (= (f) 1) (= (f) 2)
//! !(+ (f) 10)
//! // The + operation receives (f) unevaluated, evaluates it to [1, 2],
//! // then computes [1+10, 2+10] = [11, 12]
//! ```

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use super::environment::Environment;
use super::models::MettaValue;

/// Bindings from pattern matching (variable name -> value)
pub type Bindings = HashMap<String, MettaValue>;

/// Result type for grounded operations
/// Each result is a (value, optional_bindings) pair
pub type GroundedResult = Result<Vec<(MettaValue, Option<Bindings>)>, ExecError>;

/// Error type for grounded operation execution
#[derive(Debug, Clone)]
pub enum ExecError {
    /// Operation is not applicable to these arguments - try other rules
    /// This is NOT an error, just signals "I can't handle this"
    NoReduce,

    /// Runtime error during execution
    Runtime(String),

    /// Incorrect argument type or arity
    IncorrectArgument(String),
}

impl fmt::Display for ExecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExecError::NoReduce => write!(f, "NoReduce"),
            ExecError::Runtime(msg) => write!(f, "Runtime error: {}", msg),
            ExecError::IncorrectArgument(msg) => write!(f, "Incorrect argument: {}", msg),
        }
    }
}

impl std::error::Error for ExecError {}

/// Check if any result is an error and return it if so
/// Used for error propagation through grounded operations
fn find_error(results: &[MettaValue]) -> Option<&MettaValue> {
    results.iter().find(|v| matches!(v, MettaValue::Error(_, _)))
}

/// Helper function to get a friendly type name for error messages
fn friendly_type_name(value: &MettaValue) -> &'static str {
    match value {
        MettaValue::Long(_) => "Number (integer)",
        MettaValue::Float(_) => "Number (float)",
        MettaValue::Bool(_) => "Bool",
        MettaValue::String(_) => "String",
        MettaValue::Atom(_) => "Symbol",
        MettaValue::SExpr(_) => "Expression",
        MettaValue::Nil => "Nil",
        MettaValue::Unit => "Unit",
        MettaValue::Error(_, _) => "Error",
        MettaValue::Type(_) => "Type",
        MettaValue::Conjunction(_) => "Conjunction",
        MettaValue::Space(_) => "Space",
        MettaValue::State(_) => "State",
    }
}

/// HE-compatible trait for grounded (built-in) operations.
///
/// Operations receive RAW (unevaluated) arguments and evaluate them internally
/// when concrete values are needed. This matches HE's lazy evaluation semantics.
///
/// # Implementing a Grounded Operation
///
/// ```ignore
/// struct AddOp;
///
/// impl GroundedOperation for AddOp {
///     fn name(&self) -> &str { "+" }
///
///     fn execute_raw(
///         &self,
///         args: &[MettaValue],
///         env: &Environment,
///         eval_fn: &EvalFn,
///     ) -> GroundedResult {
///         if args.len() != 2 {
///             return Err(ExecError::IncorrectArgument(
///                 format!("+ requires 2 arguments, got {}", args.len())
///             ));
///         }
///
///         // Evaluate arguments internally
///         let (a_results, _) = eval_fn(args[0].clone(), env.clone());
///         let (b_results, _) = eval_fn(args[1].clone(), env.clone());
///
///         // Compute Cartesian product of results
///         let mut results = Vec::new();
///         for a in &a_results {
///             for b in &b_results {
///                 if let (MettaValue::Long(x), MettaValue::Long(y)) = (a, b) {
///                     results.push((MettaValue::Long(x + y), None));
///                 } else {
///                     return Err(ExecError::NoReduce);
///                 }
///             }
///         }
///         Ok(results)
///     }
/// }
/// ```
pub trait GroundedOperation: Send + Sync {
    /// The name of this operation (e.g., "+", "-", "and")
    fn name(&self) -> &str;

    /// Execute the operation with unevaluated arguments.
    ///
    /// # Arguments
    /// * `args` - The unevaluated argument expressions
    /// * `env` - The current environment for evaluation
    /// * `eval_fn` - Function to evaluate sub-expressions when needed
    ///
    /// # Returns
    /// * `Ok(results)` - List of (value, optional_bindings) pairs
    /// * `Err(NoReduce)` - Operation not applicable, try other rules
    /// * `Err(...)` - Actual error during execution
    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult;
}

/// Function type for evaluating MeTTa expressions
/// Used by grounded operations to evaluate their arguments when needed
pub type EvalFn = dyn Fn(MettaValue, Environment) -> (Vec<MettaValue>, Environment) + Send + Sync;

/// Registry of grounded operations, keyed by name
pub struct GroundedRegistry {
    operations: HashMap<String, Arc<dyn GroundedOperation>>,
}

impl GroundedRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        GroundedRegistry {
            operations: HashMap::new(),
        }
    }

    /// Create a registry with standard operations (+, -, *, /, comparisons, logical)
    pub fn with_standard_ops() -> Self {
        let mut registry = Self::new();

        // Arithmetic operations
        registry.register(Arc::new(AddOp));
        registry.register(Arc::new(SubOp));
        registry.register(Arc::new(MulOp));
        registry.register(Arc::new(DivOp));
        registry.register(Arc::new(ModOp));

        // Comparison operations
        registry.register(Arc::new(LessOp));
        registry.register(Arc::new(LessEqOp));
        registry.register(Arc::new(GreaterOp));
        registry.register(Arc::new(GreaterEqOp));
        registry.register(Arc::new(EqualOp));
        registry.register(Arc::new(NotEqualOp));

        // Logical operations
        registry.register(Arc::new(AndOp));
        registry.register(Arc::new(OrOp));
        registry.register(Arc::new(NotOp));

        registry
    }

    /// Register a grounded operation
    pub fn register(&mut self, op: Arc<dyn GroundedOperation>) {
        self.operations.insert(op.name().to_string(), op);
    }

    /// Look up a grounded operation by name
    pub fn get(&self, name: &str) -> Option<Arc<dyn GroundedOperation>> {
        self.operations.get(name).cloned()
    }
}

impl Default for GroundedRegistry {
    fn default() -> Self {
        Self::with_standard_ops()
    }
}

impl Clone for GroundedRegistry {
    fn clone(&self) -> Self {
        GroundedRegistry {
            operations: self.operations.clone(),
        }
    }
}

// =============================================================================
// Arithmetic Operations
// =============================================================================

/// Addition operation: (+ a b)
pub struct AddOp;

impl GroundedOperation for AddOp {
    fn name(&self) -> &str {
        "+"
    }

    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult {
        if args.len() != 2 {
            return Err(ExecError::IncorrectArgument(format!(
                "+ requires 2 arguments, got {}",
                args.len()
            )));
        }

        // Evaluate arguments internally (lazy -> eager for concrete computation)
        let (a_results, env1) = eval_fn(args[0].clone(), env.clone());
        let (b_results, _) = eval_fn(args[1].clone(), env1);

        // Propagate errors from sub-expressions
        if let Some(err) = find_error(&a_results) {
            return Ok(vec![(err.clone(), None)]);
        }
        if let Some(err) = find_error(&b_results) {
            return Ok(vec![(err.clone(), None)]);
        }

        // Compute Cartesian product of results
        let mut results = Vec::new();
        for a in &a_results {
            for b in &b_results {
                match (a, b) {
                    (MettaValue::Long(x), MettaValue::Long(y)) => {
                        match x.checked_add(*y) {
                            Some(sum) => results.push((MettaValue::Long(sum), None)),
                            None => {
                                return Err(ExecError::Runtime(format!(
                                    "Integer overflow: {} + {}",
                                    x, y
                                )))
                            }
                        }
                    }
                    (MettaValue::Float(x), MettaValue::Float(y)) => {
                        results.push((MettaValue::Float(x + y), None));
                    }
                    (MettaValue::Long(x), MettaValue::Float(y)) => {
                        results.push((MettaValue::Float(*x as f64 + y), None));
                    }
                    (MettaValue::Float(x), MettaValue::Long(y)) => {
                        results.push((MettaValue::Float(x + *y as f64), None));
                    }
                    _ => {
                        return Err(ExecError::Runtime(format!(
                            "Cannot perform '+': expected Number (integer), got {}",
                            friendly_type_name(if !matches!(a, MettaValue::Long(_) | MettaValue::Float(_)) { a } else { b })
                        )))
                    }
                }
            }
        }
        Ok(results)
    }
}

/// Subtraction operation: (- a b)
pub struct SubOp;

impl GroundedOperation for SubOp {
    fn name(&self) -> &str {
        "-"
    }

    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult {
        if args.len() != 2 {
            return Err(ExecError::IncorrectArgument(format!(
                "- requires 2 arguments, got {}",
                args.len()
            )));
        }

        let (a_results, env1) = eval_fn(args[0].clone(), env.clone());
        let (b_results, _) = eval_fn(args[1].clone(), env1);

        // Propagate errors from sub-expressions
        if let Some(err) = find_error(&a_results) {
            return Ok(vec![(err.clone(), None)]);
        }
        if let Some(err) = find_error(&b_results) {
            return Ok(vec![(err.clone(), None)]);
        }

        let mut results = Vec::new();
        for a in &a_results {
            for b in &b_results {
                match (a, b) {
                    (MettaValue::Long(x), MettaValue::Long(y)) => {
                        match x.checked_sub(*y) {
                            Some(diff) => results.push((MettaValue::Long(diff), None)),
                            None => {
                                return Err(ExecError::Runtime(format!(
                                    "Integer overflow: {} - {}",
                                    x, y
                                )))
                            }
                        }
                    }
                    (MettaValue::Float(x), MettaValue::Float(y)) => {
                        results.push((MettaValue::Float(x - y), None));
                    }
                    (MettaValue::Long(x), MettaValue::Float(y)) => {
                        results.push((MettaValue::Float(*x as f64 - y), None));
                    }
                    (MettaValue::Float(x), MettaValue::Long(y)) => {
                        results.push((MettaValue::Float(x - *y as f64), None));
                    }
                    _ => {
                        return Err(ExecError::Runtime(format!(
                            "Cannot perform '-': expected Number (integer), got {}",
                            friendly_type_name(if !matches!(a, MettaValue::Long(_) | MettaValue::Float(_)) { a } else { b })
                        )))
                    }
                }
            }
        }
        Ok(results)
    }
}

/// Multiplication operation: (* a b)
pub struct MulOp;

impl GroundedOperation for MulOp {
    fn name(&self) -> &str {
        "*"
    }

    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult {
        if args.len() != 2 {
            return Err(ExecError::IncorrectArgument(format!(
                "* requires 2 arguments, got {}",
                args.len()
            )));
        }

        let (a_results, env1) = eval_fn(args[0].clone(), env.clone());
        let (b_results, _) = eval_fn(args[1].clone(), env1);

        // Propagate errors from sub-expressions
        if let Some(err) = find_error(&a_results) {
            return Ok(vec![(err.clone(), None)]);
        }
        if let Some(err) = find_error(&b_results) {
            return Ok(vec![(err.clone(), None)]);
        }

        let mut results = Vec::new();
        for a in &a_results {
            for b in &b_results {
                match (a, b) {
                    (MettaValue::Long(x), MettaValue::Long(y)) => {
                        match x.checked_mul(*y) {
                            Some(prod) => results.push((MettaValue::Long(prod), None)),
                            None => {
                                return Err(ExecError::Runtime(format!(
                                    "Integer overflow: {} * {}",
                                    x, y
                                )))
                            }
                        }
                    }
                    (MettaValue::Float(x), MettaValue::Float(y)) => {
                        results.push((MettaValue::Float(x * y), None));
                    }
                    (MettaValue::Long(x), MettaValue::Float(y)) => {
                        results.push((MettaValue::Float(*x as f64 * y), None));
                    }
                    (MettaValue::Float(x), MettaValue::Long(y)) => {
                        results.push((MettaValue::Float(x * *y as f64), None));
                    }
                    _ => {
                        return Err(ExecError::Runtime(format!(
                            "Cannot perform '*': expected Number (integer), got {}",
                            friendly_type_name(if !matches!(a, MettaValue::Long(_) | MettaValue::Float(_)) { a } else { b })
                        )))
                    }
                }
            }
        }
        Ok(results)
    }
}

/// Division operation: (/ a b)
pub struct DivOp;

impl GroundedOperation for DivOp {
    fn name(&self) -> &str {
        "/"
    }

    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult {
        if args.len() != 2 {
            return Err(ExecError::IncorrectArgument(format!(
                "/ requires 2 arguments, got {}",
                args.len()
            )));
        }

        let (a_results, env1) = eval_fn(args[0].clone(), env.clone());
        let (b_results, _) = eval_fn(args[1].clone(), env1);

        // Propagate errors from sub-expressions
        if let Some(err) = find_error(&a_results) {
            return Ok(vec![(err.clone(), None)]);
        }
        if let Some(err) = find_error(&b_results) {
            return Ok(vec![(err.clone(), None)]);
        }

        let mut results = Vec::new();
        for a in &a_results {
            for b in &b_results {
                match (a, b) {
                    (MettaValue::Long(x), MettaValue::Long(y)) => {
                        if *y == 0 {
                            return Err(ExecError::Runtime("Division by zero".to_string()));
                        }
                        results.push((MettaValue::Long(x / y), None));
                    }
                    (MettaValue::Float(x), MettaValue::Float(y)) => {
                        if *y == 0.0 {
                            return Err(ExecError::Runtime("Division by zero".to_string()));
                        }
                        results.push((MettaValue::Float(x / y), None));
                    }
                    (MettaValue::Long(x), MettaValue::Float(y)) => {
                        if *y == 0.0 {
                            return Err(ExecError::Runtime("Division by zero".to_string()));
                        }
                        results.push((MettaValue::Float(*x as f64 / y), None));
                    }
                    (MettaValue::Float(x), MettaValue::Long(y)) => {
                        if *y == 0 {
                            return Err(ExecError::Runtime("Division by zero".to_string()));
                        }
                        results.push((MettaValue::Float(x / *y as f64), None));
                    }
                    _ => {
                        return Err(ExecError::Runtime(format!(
                            "Cannot perform '/': expected Number (integer), got {}",
                            friendly_type_name(if !matches!(a, MettaValue::Long(_) | MettaValue::Float(_)) { a } else { b })
                        )))
                    }
                }
            }
        }
        Ok(results)
    }
}

/// Modulo operation: (% a b)
pub struct ModOp;

impl GroundedOperation for ModOp {
    fn name(&self) -> &str {
        "%"
    }

    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult {
        if args.len() != 2 {
            return Err(ExecError::IncorrectArgument(format!(
                "% requires 2 arguments, got {}",
                args.len()
            )));
        }

        let (a_results, env1) = eval_fn(args[0].clone(), env.clone());
        let (b_results, _) = eval_fn(args[1].clone(), env1);

        let mut results = Vec::new();
        for a in &a_results {
            for b in &b_results {
                match (a, b) {
                    (MettaValue::Long(x), MettaValue::Long(y)) => {
                        if *y == 0 {
                            return Err(ExecError::Runtime("Modulo by zero".to_string()));
                        }
                        results.push((MettaValue::Long(x % y), None));
                    }
                    _ => {
                        return Err(ExecError::Runtime(format!(
                            "Cannot perform '%': expected Number (integer), got {}",
                            friendly_type_name(if !matches!(a, MettaValue::Long(_)) { a } else { b })
                        )))
                    }
                }
            }
        }
        Ok(results)
    }
}

// =============================================================================
// Comparison Operations
// =============================================================================

/// Less than operation: (< a b)
pub struct LessOp;

impl GroundedOperation for LessOp {
    fn name(&self) -> &str {
        "<"
    }

    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult {
        eval_comparison(args, env, eval_fn, CompareKind::Less)
    }
}

/// Less than or equal operation: (<= a b)
pub struct LessEqOp;

impl GroundedOperation for LessEqOp {
    fn name(&self) -> &str {
        "<="
    }

    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult {
        eval_comparison(args, env, eval_fn, CompareKind::LessEq)
    }
}

/// Greater than operation: (> a b)
pub struct GreaterOp;

impl GroundedOperation for GreaterOp {
    fn name(&self) -> &str {
        ">"
    }

    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult {
        eval_comparison(args, env, eval_fn, CompareKind::Greater)
    }
}

/// Greater than or equal operation: (>= a b)
pub struct GreaterEqOp;

impl GroundedOperation for GreaterEqOp {
    fn name(&self) -> &str {
        ">="
    }

    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult {
        eval_comparison(args, env, eval_fn, CompareKind::GreaterEq)
    }
}

/// Equality operation: (== a b)
pub struct EqualOp;

impl GroundedOperation for EqualOp {
    fn name(&self) -> &str {
        "=="
    }

    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult {
        eval_equality(args, env, eval_fn, true)
    }
}

/// Inequality operation: (!= a b)
pub struct NotEqualOp;

impl GroundedOperation for NotEqualOp {
    fn name(&self) -> &str {
        "!="
    }

    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult {
        eval_equality(args, env, eval_fn, false)
    }
}

/// Comparison kind for ordering operations
enum CompareKind {
    Less,
    LessEq,
    Greater,
    GreaterEq,
}

impl CompareKind {
    #[inline]
    fn compare<T: PartialOrd>(&self, a: &T, b: &T) -> bool {
        match self {
            CompareKind::Less => a < b,
            CompareKind::LessEq => a <= b,
            CompareKind::Greater => a > b,
            CompareKind::GreaterEq => a >= b,
        }
    }
}

/// Helper function for comparison operations (supports numbers and strings)
fn eval_comparison(
    args: &[MettaValue],
    env: &Environment,
    eval_fn: &EvalFn,
    kind: CompareKind,
) -> GroundedResult {
    if args.len() != 2 {
        return Err(ExecError::IncorrectArgument(format!(
            "Comparison requires 2 arguments, got {}",
            args.len()
        )));
    }

    let (a_results, env1) = eval_fn(args[0].clone(), env.clone());
    let (b_results, _) = eval_fn(args[1].clone(), env1);

    let mut results = Vec::new();
    for a in &a_results {
        for b in &b_results {
            match (a, b) {
                (MettaValue::Long(x), MettaValue::Long(y)) => {
                    results.push((MettaValue::Bool(kind.compare(x, y)), None));
                }
                (MettaValue::Float(x), MettaValue::Float(y)) => {
                    results.push((MettaValue::Bool(kind.compare(x, y)), None));
                }
                (MettaValue::Long(x), MettaValue::Float(y)) => {
                    results.push((MettaValue::Bool(kind.compare(&(*x as f64), y)), None));
                }
                (MettaValue::Float(x), MettaValue::Long(y)) => {
                    results.push((MettaValue::Bool(kind.compare(x, &(*y as f64))), None));
                }
                // String comparison (lexicographic)
                (MettaValue::String(x), MettaValue::String(y)) => {
                    results.push((MettaValue::Bool(kind.compare(x, y)), None));
                }
                _ => {
                    return Err(ExecError::Runtime(format!(
                        "Cannot compare: type mismatch between {} and {}",
                        friendly_type_name(a),
                        friendly_type_name(b)
                    )))
                }
            }
        }
    }
    Ok(results)
}

/// Helper function for equality/inequality operations
/// Supports comparing all value types, not just numeric
fn eval_equality(
    args: &[MettaValue],
    env: &Environment,
    eval_fn: &EvalFn,
    is_equal: bool,
) -> GroundedResult {
    if args.len() != 2 {
        return Err(ExecError::IncorrectArgument(format!(
            "Equality comparison requires 2 arguments, got {}",
            args.len()
        )));
    }

    let (a_results, env1) = eval_fn(args[0].clone(), env.clone());
    let (b_results, _) = eval_fn(args[1].clone(), env1);

    let mut results = Vec::new();
    for a in &a_results {
        for b in &b_results {
            let equal = values_equal(a, b);
            let result = if is_equal { equal } else { !equal };
            results.push((MettaValue::Bool(result), None));
        }
    }
    Ok(results)
}

/// Check if two MettaValues are equal
fn values_equal(a: &MettaValue, b: &MettaValue) -> bool {
    match (a, b) {
        (MettaValue::Long(x), MettaValue::Long(y)) => x == y,
        (MettaValue::Float(x), MettaValue::Float(y)) => (x - y).abs() < f64::EPSILON,
        (MettaValue::Bool(x), MettaValue::Bool(y)) => x == y,
        (MettaValue::String(x), MettaValue::String(y)) => x == y,
        (MettaValue::Atom(x), MettaValue::Atom(y)) => x == y,
        (MettaValue::Nil, MettaValue::Nil) => true,
        (MettaValue::Unit, MettaValue::Unit) => true,
        // HE compatibility: Nil equals empty SExpr
        (MettaValue::Nil, MettaValue::SExpr(items))
        | (MettaValue::SExpr(items), MettaValue::Nil) => items.is_empty(),
        // HE compatibility: Nil equals Unit
        (MettaValue::Nil, MettaValue::Unit) | (MettaValue::Unit, MettaValue::Nil) => true,
        (MettaValue::SExpr(x), MettaValue::SExpr(y)) => {
            x.len() == y.len() && x.iter().zip(y.iter()).all(|(a, b)| values_equal(a, b))
        }
        // Different types are not equal
        _ => false,
    }
}

// =============================================================================
// Logical Operations
// =============================================================================

/// Logical AND operation: (and a b)
pub struct AndOp;

impl GroundedOperation for AndOp {
    fn name(&self) -> &str {
        "and"
    }

    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult {
        if args.len() != 2 {
            return Err(ExecError::IncorrectArgument(format!(
                "and requires 2 arguments, got {}",
                args.len()
            )));
        }

        // Short-circuit: only evaluate second arg if first is true
        let (a_results, env1) = eval_fn(args[0].clone(), env.clone());

        let mut results = Vec::new();
        for a in &a_results {
            match a {
                MettaValue::Bool(false) => {
                    // Short-circuit: false AND anything = false
                    results.push((MettaValue::Bool(false), None));
                }
                MettaValue::Bool(true) => {
                    // Need to evaluate second argument
                    let (b_results, _) = eval_fn(args[1].clone(), env1.clone());
                    for b in &b_results {
                        match b {
                            MettaValue::Bool(bv) => {
                                results.push((MettaValue::Bool(*bv), None));
                            }
                            _ => {
                                return Err(ExecError::Runtime(format!(
                                    "Cannot perform 'and': expected Bool, got {}",
                                    friendly_type_name(b)
                                )))
                            }
                        }
                    }
                }
                _ => {
                    return Err(ExecError::Runtime(format!(
                        "Cannot perform 'and': expected Bool, got {}",
                        friendly_type_name(a)
                    )))
                }
            }
        }
        Ok(results)
    }
}

/// Logical OR operation: (or a b)
pub struct OrOp;

impl GroundedOperation for OrOp {
    fn name(&self) -> &str {
        "or"
    }

    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult {
        if args.len() != 2 {
            return Err(ExecError::IncorrectArgument(format!(
                "or requires 2 arguments, got {}",
                args.len()
            )));
        }

        // Short-circuit: only evaluate second arg if first is false
        let (a_results, env1) = eval_fn(args[0].clone(), env.clone());

        let mut results = Vec::new();
        for a in &a_results {
            match a {
                MettaValue::Bool(true) => {
                    // Short-circuit: true OR anything = true
                    results.push((MettaValue::Bool(true), None));
                }
                MettaValue::Bool(false) => {
                    // Need to evaluate second argument
                    let (b_results, _) = eval_fn(args[1].clone(), env1.clone());
                    for b in &b_results {
                        match b {
                            MettaValue::Bool(bv) => {
                                results.push((MettaValue::Bool(*bv), None));
                            }
                            _ => {
                                return Err(ExecError::Runtime(format!(
                                    "Cannot perform 'or': expected Bool, got {}",
                                    friendly_type_name(b)
                                )))
                            }
                        }
                    }
                }
                _ => {
                    return Err(ExecError::Runtime(format!(
                        "Cannot perform 'or': expected Bool, got {}",
                        friendly_type_name(a)
                    )))
                }
            }
        }
        Ok(results)
    }
}

/// Logical NOT operation: (not a)
pub struct NotOp;

impl GroundedOperation for NotOp {
    fn name(&self) -> &str {
        "not"
    }

    fn execute_raw(
        &self,
        args: &[MettaValue],
        env: &Environment,
        eval_fn: &EvalFn,
    ) -> GroundedResult {
        if args.len() != 1 {
            return Err(ExecError::IncorrectArgument(format!(
                "not requires 1 argument, got {}",
                args.len()
            )));
        }

        let (a_results, _) = eval_fn(args[0].clone(), env.clone());

        let mut results = Vec::new();
        for a in &a_results {
            match a {
                MettaValue::Bool(v) => {
                    results.push((MettaValue::Bool(!v), None));
                }
                _ => {
                    return Err(ExecError::Runtime(format!(
                        "Cannot perform 'not': expected Bool, got {}",
                        friendly_type_name(a)
                    )))
                }
            }
        }
        Ok(results)
    }
}

// =============================================================================
// TCO (Tail Call Optimization) Support for Grounded Operations
// =============================================================================
//
// The standard `GroundedOperation` trait calls `eval_fn` internally, which
// creates nested Rust stack frames and bypasses the trampoline. This causes
// stack overflow for deep recursion using grounded ops like `(- $n 1)`.
//
// The TCO versions return `GroundedWork` enum that describes what work needs
// to be done. The trampoline processes this work and calls the operation back
// with results, keeping evaluation on the work stack instead of the Rust stack.

/// Work returned by grounded operations for TCO.
///
/// Instead of calling `eval_fn` internally, operations return this enum
/// to request argument evaluation. The trampoline processes the request
/// and calls the operation back with results.
#[derive(Debug, Clone)]
pub enum GroundedWork {
    /// Operation complete - return these results
    Done(Vec<(MettaValue, Option<Bindings>)>),

    /// Need to evaluate an argument before continuing
    EvalArg {
        /// Which argument index to evaluate (0-based)
        arg_idx: usize,
        /// Operation state to restore when resuming
        state: GroundedState,
    },

    /// Error during execution
    Error(ExecError),
}

/// State saved between grounded operation steps.
///
/// This struct is passed to `execute_step` and contains all state needed
/// to resume the operation after argument evaluation completes.
#[derive(Debug, Clone)]
pub struct GroundedState {
    /// Operation name (to look up the operation again)
    pub op_name: String,
    /// Original unevaluated arguments (Arc-wrapped for O(1) clone)
    pub args: Arc<Vec<MettaValue>>,
    /// Results from previously evaluated arguments: arg_idx -> Vec<MettaValue>
    pub evaluated_args: HashMap<usize, Vec<MettaValue>>,
    /// Current step in the operation's state machine
    pub step: usize,
    /// Accumulated results so far (for short-circuit ops like `and`/`or`)
    pub accumulated_results: Vec<(MettaValue, Option<Bindings>)>,
}

impl GroundedState {
    /// Create a new state for starting an operation
    pub fn new(op_name: String, args: Vec<MettaValue>) -> Self {
        GroundedState {
            op_name,
            args: Arc::new(args),
            evaluated_args: HashMap::new(),
            step: 0,
            accumulated_results: Vec::new(),
        }
    }

    /// Create a new state from pre-wrapped Arc args (avoids re-wrapping)
    pub fn from_arc(op_name: String, args: Arc<Vec<MettaValue>>) -> Self {
        GroundedState {
            op_name,
            args,
            evaluated_args: HashMap::new(),
            step: 0,
            accumulated_results: Vec::new(),
        }
    }

    /// Get the args slice
    #[inline]
    pub fn args(&self) -> &[MettaValue] {
        &self.args
    }

    /// Get evaluated arg results, or None if not yet evaluated
    pub fn get_arg(&self, idx: usize) -> Option<&Vec<MettaValue>> {
        self.evaluated_args.get(&idx)
    }

    /// Set evaluated arg results
    pub fn set_arg(&mut self, idx: usize, results: Vec<MettaValue>) {
        self.evaluated_args.insert(idx, results);
    }
}

/// TCO-compatible trait for grounded operations.
///
/// Unlike `GroundedOperation`, this trait does NOT receive an `eval_fn`.
/// Instead, operations return `GroundedWork::EvalArg` to request argument
/// evaluation, and the trampoline handles it.
///
/// # State Machine Pattern
///
/// Operations are implemented as state machines:
/// - Step 0: Validate args, request first argument evaluation
/// - Step 1: Process first arg results, request second argument (or compute)
/// - Step 2+: Continue until `GroundedWork::Done` or `GroundedWork::Error`
///
/// # Example
///
/// ```ignore
/// impl GroundedOperationTCO for AddOpTCO {
///     fn name(&self) -> &str { "+" }
///
///     fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
///         match state.step {
///             0 => {
///                 if state.args.len() != 2 {
///                     return GroundedWork::Error(...);
///                 }
///                 state.step = 1;
///                 GroundedWork::EvalArg { arg_idx: 0, state: state.clone() }
///             }
///             1 => {
///                 state.step = 2;
///                 GroundedWork::EvalArg { arg_idx: 1, state: state.clone() }
///             }
///             2 => {
///                 // Compute result from evaluated args
///                 let a = state.get_arg(0).unwrap();
///                 let b = state.get_arg(1).unwrap();
///                 // ... compute Cartesian product ...
///                 GroundedWork::Done(results)
///             }
///             _ => unreachable!()
///         }
///     }
/// }
/// ```
pub trait GroundedOperationTCO: Send + Sync {
    /// The name of this operation (e.g., "+", "-", "and")
    fn name(&self) -> &str;

    /// Execute one step of the operation.
    ///
    /// Called initially with `state.step == 0` and empty `state.evaluated_args`.
    /// Called again after each `EvalArg` request with the results added.
    ///
    /// # Arguments
    /// * `state` - Mutable state that persists across steps
    ///
    /// # Returns
    /// * `Done(results)` - Operation complete, return these values
    /// * `EvalArg { arg_idx, state }` - Evaluate argument at index, then call again
    /// * `Error(e)` - Operation failed with error
    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork;
}

/// Registry of TCO-compatible grounded operations
pub struct GroundedRegistryTCO {
    operations: HashMap<String, Arc<dyn GroundedOperationTCO>>,
}

impl GroundedRegistryTCO {
    /// Create a new empty registry
    pub fn new() -> Self {
        GroundedRegistryTCO {
            operations: HashMap::new(),
        }
    }

    /// Create a registry with standard TCO operations
    pub fn with_standard_ops() -> Self {
        let mut registry = Self::new();

        // Arithmetic operations
        registry.register(Arc::new(AddOpTCO));
        registry.register(Arc::new(SubOpTCO));
        registry.register(Arc::new(MulOpTCO));
        registry.register(Arc::new(DivOpTCO));
        registry.register(Arc::new(ModOpTCO));

        // Comparison operations
        registry.register(Arc::new(LessOpTCO));
        registry.register(Arc::new(LessEqOpTCO));
        registry.register(Arc::new(GreaterOpTCO));
        registry.register(Arc::new(GreaterEqOpTCO));
        registry.register(Arc::new(EqualOpTCO));
        registry.register(Arc::new(NotEqualOpTCO));

        // Logical operations
        registry.register(Arc::new(AndOpTCO));
        registry.register(Arc::new(OrOpTCO));
        registry.register(Arc::new(NotOpTCO));

        registry
    }

    /// Register a TCO grounded operation
    pub fn register(&mut self, op: Arc<dyn GroundedOperationTCO>) {
        self.operations.insert(op.name().to_string(), op);
    }

    /// Look up a TCO grounded operation by name
    pub fn get(&self, name: &str) -> Option<Arc<dyn GroundedOperationTCO>> {
        self.operations.get(name).cloned()
    }
}

impl Default for GroundedRegistryTCO {
    fn default() -> Self {
        Self::with_standard_ops()
    }
}

impl Clone for GroundedRegistryTCO {
    fn clone(&self) -> Self {
        GroundedRegistryTCO {
            operations: self.operations.clone(),
        }
    }
}

// =============================================================================
// TCO Arithmetic Operations
// =============================================================================

/// TCO Addition operation: (+ a b)
pub struct AddOpTCO;

impl GroundedOperationTCO for AddOpTCO {
    fn name(&self) -> &str {
        "+"
    }

    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
        match state.step {
            0 => {
                // Step 0: Validate arity and request first argument
                if state.args.len() != 2 {
                    return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                        "+ requires 2 arguments, got {}",
                        state.args.len()
                    )));
                }
                state.step = 1;
                GroundedWork::EvalArg {
                    arg_idx: 0,
                    state: state.clone(),
                }
            }
            1 => {
                // Step 1: Check first arg for errors, request second argument
                let a_results = state.get_arg(0).unwrap();
                if let Some(err) = find_error(a_results) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }
                state.step = 2;
                GroundedWork::EvalArg {
                    arg_idx: 1,
                    state: state.clone(),
                }
            }
            2 => {
                // Step 2: Compute Cartesian product of results
                let a_results = state.get_arg(0).unwrap();
                let b_results = state.get_arg(1).unwrap();

                if let Some(err) = find_error(b_results) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut results = Vec::new();
                for a in a_results {
                    for b in b_results {
                        match (a, b) {
                            (MettaValue::Long(x), MettaValue::Long(y)) => {
                                match x.checked_add(*y) {
                                    Some(sum) => results.push((MettaValue::Long(sum), None)),
                                    None => {
                                        return GroundedWork::Error(ExecError::Runtime(format!(
                                            "Integer overflow: {} + {}",
                                            x, y
                                        )))
                                    }
                                }
                            }
                            (MettaValue::Float(x), MettaValue::Float(y)) => {
                                results.push((MettaValue::Float(x + y), None));
                            }
                            (MettaValue::Long(x), MettaValue::Float(y)) => {
                                results.push((MettaValue::Float(*x as f64 + y), None));
                            }
                            (MettaValue::Float(x), MettaValue::Long(y)) => {
                                results.push((MettaValue::Float(x + *y as f64), None));
                            }
                            _ => {
                                return GroundedWork::Error(ExecError::Runtime(format!(
                                    "Cannot perform '+': expected Number (integer), got {}",
                                    friendly_type_name(
                                        if !matches!(a, MettaValue::Long(_) | MettaValue::Float(_)) {
                                            a
                                        } else {
                                            b
                                        }
                                    )
                                )))
                            }
                        }
                    }
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for AddOpTCO", state.step),
        }
    }
}

/// TCO Subtraction operation: (- a b)
pub struct SubOpTCO;

impl GroundedOperationTCO for SubOpTCO {
    fn name(&self) -> &str {
        "-"
    }

    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
        match state.step {
            0 => {
                if state.args.len() != 2 {
                    return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                        "- requires 2 arguments, got {}",
                        state.args.len()
                    )));
                }
                state.step = 1;
                GroundedWork::EvalArg {
                    arg_idx: 0,
                    state: state.clone(),
                }
            }
            1 => {
                let a_results = state.get_arg(0).unwrap();
                if let Some(err) = find_error(a_results) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }
                state.step = 2;
                GroundedWork::EvalArg {
                    arg_idx: 1,
                    state: state.clone(),
                }
            }
            2 => {
                let a_results = state.get_arg(0).unwrap();
                let b_results = state.get_arg(1).unwrap();

                if let Some(err) = find_error(b_results) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut results = Vec::new();
                for a in a_results {
                    for b in b_results {
                        match (a, b) {
                            (MettaValue::Long(x), MettaValue::Long(y)) => {
                                match x.checked_sub(*y) {
                                    Some(diff) => results.push((MettaValue::Long(diff), None)),
                                    None => {
                                        return GroundedWork::Error(ExecError::Runtime(format!(
                                            "Integer overflow: {} - {}",
                                            x, y
                                        )))
                                    }
                                }
                            }
                            (MettaValue::Float(x), MettaValue::Float(y)) => {
                                results.push((MettaValue::Float(x - y), None));
                            }
                            (MettaValue::Long(x), MettaValue::Float(y)) => {
                                results.push((MettaValue::Float(*x as f64 - y), None));
                            }
                            (MettaValue::Float(x), MettaValue::Long(y)) => {
                                results.push((MettaValue::Float(x - *y as f64), None));
                            }
                            _ => {
                                return GroundedWork::Error(ExecError::Runtime(format!(
                                    "Cannot perform '-': expected Number (integer), got {}",
                                    friendly_type_name(
                                        if !matches!(a, MettaValue::Long(_) | MettaValue::Float(_)) {
                                            a
                                        } else {
                                            b
                                        }
                                    )
                                )))
                            }
                        }
                    }
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for SubOpTCO", state.step),
        }
    }
}

/// TCO Multiplication operation: (* a b)
pub struct MulOpTCO;

impl GroundedOperationTCO for MulOpTCO {
    fn name(&self) -> &str {
        "*"
    }

    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
        match state.step {
            0 => {
                if state.args.len() != 2 {
                    return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                        "* requires 2 arguments, got {}",
                        state.args.len()
                    )));
                }
                state.step = 1;
                GroundedWork::EvalArg {
                    arg_idx: 0,
                    state: state.clone(),
                }
            }
            1 => {
                let a_results = state.get_arg(0).unwrap();
                if let Some(err) = find_error(a_results) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }
                state.step = 2;
                GroundedWork::EvalArg {
                    arg_idx: 1,
                    state: state.clone(),
                }
            }
            2 => {
                let a_results = state.get_arg(0).unwrap();
                let b_results = state.get_arg(1).unwrap();

                if let Some(err) = find_error(b_results) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut results = Vec::new();
                for a in a_results {
                    for b in b_results {
                        match (a, b) {
                            (MettaValue::Long(x), MettaValue::Long(y)) => {
                                match x.checked_mul(*y) {
                                    Some(prod) => results.push((MettaValue::Long(prod), None)),
                                    None => {
                                        return GroundedWork::Error(ExecError::Runtime(format!(
                                            "Integer overflow: {} * {}",
                                            x, y
                                        )))
                                    }
                                }
                            }
                            (MettaValue::Float(x), MettaValue::Float(y)) => {
                                results.push((MettaValue::Float(x * y), None));
                            }
                            (MettaValue::Long(x), MettaValue::Float(y)) => {
                                results.push((MettaValue::Float(*x as f64 * y), None));
                            }
                            (MettaValue::Float(x), MettaValue::Long(y)) => {
                                results.push((MettaValue::Float(x * *y as f64), None));
                            }
                            _ => {
                                return GroundedWork::Error(ExecError::Runtime(format!(
                                    "Cannot perform '*': expected Number (integer), got {}",
                                    friendly_type_name(
                                        if !matches!(a, MettaValue::Long(_) | MettaValue::Float(_)) {
                                            a
                                        } else {
                                            b
                                        }
                                    )
                                )))
                            }
                        }
                    }
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for MulOpTCO", state.step),
        }
    }
}

/// TCO Division operation: (/ a b)
pub struct DivOpTCO;

impl GroundedOperationTCO for DivOpTCO {
    fn name(&self) -> &str {
        "/"
    }

    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
        match state.step {
            0 => {
                if state.args.len() != 2 {
                    return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                        "/ requires 2 arguments, got {}",
                        state.args.len()
                    )));
                }
                state.step = 1;
                GroundedWork::EvalArg {
                    arg_idx: 0,
                    state: state.clone(),
                }
            }
            1 => {
                let a_results = state.get_arg(0).unwrap();
                if let Some(err) = find_error(a_results) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }
                state.step = 2;
                GroundedWork::EvalArg {
                    arg_idx: 1,
                    state: state.clone(),
                }
            }
            2 => {
                let a_results = state.get_arg(0).unwrap();
                let b_results = state.get_arg(1).unwrap();

                if let Some(err) = find_error(b_results) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut results = Vec::new();
                for a in a_results {
                    for b in b_results {
                        match (a, b) {
                            (MettaValue::Long(x), MettaValue::Long(y)) => {
                                if *y == 0 {
                                    return GroundedWork::Error(ExecError::Runtime(
                                        "Division by zero".to_string(),
                                    ));
                                }
                                results.push((MettaValue::Long(x / y), None));
                            }
                            (MettaValue::Float(x), MettaValue::Float(y)) => {
                                if *y == 0.0 {
                                    return GroundedWork::Error(ExecError::Runtime(
                                        "Division by zero".to_string(),
                                    ));
                                }
                                results.push((MettaValue::Float(x / y), None));
                            }
                            (MettaValue::Long(x), MettaValue::Float(y)) => {
                                if *y == 0.0 {
                                    return GroundedWork::Error(ExecError::Runtime(
                                        "Division by zero".to_string(),
                                    ));
                                }
                                results.push((MettaValue::Float(*x as f64 / y), None));
                            }
                            (MettaValue::Float(x), MettaValue::Long(y)) => {
                                if *y == 0 {
                                    return GroundedWork::Error(ExecError::Runtime(
                                        "Division by zero".to_string(),
                                    ));
                                }
                                results.push((MettaValue::Float(x / *y as f64), None));
                            }
                            _ => {
                                return GroundedWork::Error(ExecError::Runtime(format!(
                                    "Cannot perform '/': expected Number (integer), got {}",
                                    friendly_type_name(
                                        if !matches!(a, MettaValue::Long(_) | MettaValue::Float(_)) {
                                            a
                                        } else {
                                            b
                                        }
                                    )
                                )))
                            }
                        }
                    }
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for DivOpTCO", state.step),
        }
    }
}

/// TCO Modulo operation: (% a b)
pub struct ModOpTCO;

impl GroundedOperationTCO for ModOpTCO {
    fn name(&self) -> &str {
        "%"
    }

    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
        match state.step {
            0 => {
                if state.args.len() != 2 {
                    return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                        "% requires 2 arguments, got {}",
                        state.args.len()
                    )));
                }
                state.step = 1;
                GroundedWork::EvalArg {
                    arg_idx: 0,
                    state: state.clone(),
                }
            }
            1 => {
                let a_results = state.get_arg(0).unwrap();
                if let Some(err) = find_error(a_results) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }
                state.step = 2;
                GroundedWork::EvalArg {
                    arg_idx: 1,
                    state: state.clone(),
                }
            }
            2 => {
                let a_results = state.get_arg(0).unwrap();
                let b_results = state.get_arg(1).unwrap();

                if let Some(err) = find_error(b_results) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut results = Vec::new();
                for a in a_results {
                    for b in b_results {
                        match (a, b) {
                            (MettaValue::Long(x), MettaValue::Long(y)) => {
                                if *y == 0 {
                                    return GroundedWork::Error(ExecError::Runtime(
                                        "Modulo by zero".to_string(),
                                    ));
                                }
                                results.push((MettaValue::Long(x % y), None));
                            }
                            _ => {
                                return GroundedWork::Error(ExecError::Runtime(format!(
                                    "Cannot perform '%': expected Number (integer), got {}",
                                    friendly_type_name(if !matches!(a, MettaValue::Long(_)) {
                                        a
                                    } else {
                                        b
                                    })
                                )))
                            }
                        }
                    }
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for ModOpTCO", state.step),
        }
    }
}

// =============================================================================
// TCO Comparison Operations
// =============================================================================

/// TCO Less than operation: (< a b)
pub struct LessOpTCO;

impl GroundedOperationTCO for LessOpTCO {
    fn name(&self) -> &str {
        "<"
    }

    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
        eval_comparison_tco(state, CompareKind::Less)
    }
}

/// TCO Less than or equal operation: (<= a b)
pub struct LessEqOpTCO;

impl GroundedOperationTCO for LessEqOpTCO {
    fn name(&self) -> &str {
        "<="
    }

    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
        eval_comparison_tco(state, CompareKind::LessEq)
    }
}

/// TCO Greater than operation: (> a b)
pub struct GreaterOpTCO;

impl GroundedOperationTCO for GreaterOpTCO {
    fn name(&self) -> &str {
        ">"
    }

    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
        eval_comparison_tco(state, CompareKind::Greater)
    }
}

/// TCO Greater than or equal operation: (>= a b)
pub struct GreaterEqOpTCO;

impl GroundedOperationTCO for GreaterEqOpTCO {
    fn name(&self) -> &str {
        ">="
    }

    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
        eval_comparison_tco(state, CompareKind::GreaterEq)
    }
}

/// TCO Equality operation: (== a b)
pub struct EqualOpTCO;

impl GroundedOperationTCO for EqualOpTCO {
    fn name(&self) -> &str {
        "=="
    }

    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
        eval_equality_tco(state, true)
    }
}

/// TCO Inequality operation: (!= a b)
pub struct NotEqualOpTCO;

impl GroundedOperationTCO for NotEqualOpTCO {
    fn name(&self) -> &str {
        "!="
    }

    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
        eval_equality_tco(state, false)
    }
}

/// Helper function for TCO comparison operations
fn eval_comparison_tco(state: &mut GroundedState, kind: CompareKind) -> GroundedWork {
    match state.step {
        0 => {
            if state.args.len() != 2 {
                return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                    "Comparison requires 2 arguments, got {}",
                    state.args.len()
                )));
            }
            state.step = 1;
            GroundedWork::EvalArg {
                arg_idx: 0,
                state: state.clone(),
            }
        }
        1 => {
            let a_results = state.get_arg(0).unwrap();
            if let Some(err) = find_error(a_results) {
                return GroundedWork::Done(vec![(err.clone(), None)]);
            }
            state.step = 2;
            GroundedWork::EvalArg {
                arg_idx: 1,
                state: state.clone(),
            }
        }
        2 => {
            let a_results = state.get_arg(0).unwrap();
            let b_results = state.get_arg(1).unwrap();

            if let Some(err) = find_error(b_results) {
                return GroundedWork::Done(vec![(err.clone(), None)]);
            }

            let mut results = Vec::new();
            for a in a_results {
                for b in b_results {
                    match (a, b) {
                        (MettaValue::Long(x), MettaValue::Long(y)) => {
                            results.push((MettaValue::Bool(kind.compare(x, y)), None));
                        }
                        (MettaValue::Float(x), MettaValue::Float(y)) => {
                            results.push((MettaValue::Bool(kind.compare(x, y)), None));
                        }
                        (MettaValue::Long(x), MettaValue::Float(y)) => {
                            results.push((MettaValue::Bool(kind.compare(&(*x as f64), y)), None));
                        }
                        (MettaValue::Float(x), MettaValue::Long(y)) => {
                            results.push((MettaValue::Bool(kind.compare(x, &(*y as f64))), None));
                        }
                        (MettaValue::String(x), MettaValue::String(y)) => {
                            results.push((MettaValue::Bool(kind.compare(x, y)), None));
                        }
                        _ => {
                            return GroundedWork::Error(ExecError::Runtime(format!(
                                "Cannot compare: type mismatch between {} and {}",
                                friendly_type_name(a),
                                friendly_type_name(b)
                            )))
                        }
                    }
                }
            }
            GroundedWork::Done(results)
        }
        _ => unreachable!("Invalid step {} for comparison", state.step),
    }
}

/// Helper function for TCO equality/inequality operations
fn eval_equality_tco(state: &mut GroundedState, is_equal: bool) -> GroundedWork {
    match state.step {
        0 => {
            if state.args.len() != 2 {
                return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                    "Equality comparison requires 2 arguments, got {}",
                    state.args.len()
                )));
            }
            state.step = 1;
            GroundedWork::EvalArg {
                arg_idx: 0,
                state: state.clone(),
            }
        }
        1 => {
            let a_results = state.get_arg(0).unwrap();
            if let Some(err) = find_error(a_results) {
                return GroundedWork::Done(vec![(err.clone(), None)]);
            }
            state.step = 2;
            GroundedWork::EvalArg {
                arg_idx: 1,
                state: state.clone(),
            }
        }
        2 => {
            let a_results = state.get_arg(0).unwrap();
            let b_results = state.get_arg(1).unwrap();

            if let Some(err) = find_error(b_results) {
                return GroundedWork::Done(vec![(err.clone(), None)]);
            }

            let mut results = Vec::new();
            for a in a_results {
                for b in b_results {
                    let equal = values_equal(a, b);
                    let result = if is_equal { equal } else { !equal };
                    results.push((MettaValue::Bool(result), None));
                }
            }
            GroundedWork::Done(results)
        }
        _ => unreachable!("Invalid step {} for equality", state.step),
    }
}

// =============================================================================
// TCO Logical Operations (with short-circuit support)
// =============================================================================

/// TCO Logical AND operation: (and a b)
/// Preserves short-circuit semantics: False AND _ = False without evaluating second arg
pub struct AndOpTCO;

impl GroundedOperationTCO for AndOpTCO {
    fn name(&self) -> &str {
        "and"
    }

    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
        match state.step {
            0 => {
                // Step 0: Validate arity and request first argument
                if state.args.len() != 2 {
                    return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                        "and requires 2 arguments, got {}",
                        state.args.len()
                    )));
                }
                state.step = 1;
                GroundedWork::EvalArg {
                    arg_idx: 0,
                    state: state.clone(),
                }
            }
            1 => {
                // Step 1: Check first arg - short-circuit if all False
                // Clone the results to avoid borrow conflict with accumulated_results
                let a_results: Vec<_> = state.get_arg(0).unwrap().iter().cloned().collect();
                if let Some(err) = find_error(&a_results) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut need_second_arg = false;
                for a in &a_results {
                    match a {
                        MettaValue::Bool(false) => {
                            // SHORT-CIRCUIT: False and _ = False
                            state.accumulated_results.push((MettaValue::Bool(false), None));
                        }
                        MettaValue::Bool(true) => {
                            // Need to evaluate second argument for this branch
                            need_second_arg = true;
                        }
                        _ => {
                            return GroundedWork::Error(ExecError::Runtime(format!(
                                "Cannot perform 'and': expected Bool, got {}",
                                friendly_type_name(a)
                            )));
                        }
                    }
                }

                if need_second_arg {
                    // At least one True - need to evaluate second arg
                    state.step = 2;
                    GroundedWork::EvalArg {
                        arg_idx: 1,
                        state: state.clone(),
                    }
                } else {
                    // All results were False (short-circuited)
                    GroundedWork::Done(state.accumulated_results.clone())
                }
            }
            2 => {
                // Step 2: Second arg evaluated for True branches
                // Clone both to avoid borrow conflict with accumulated_results
                let a_results: Vec<_> = state.get_arg(0).unwrap().iter().cloned().collect();
                let b_results: Vec<_> = state.get_arg(1).unwrap().iter().cloned().collect();

                if let Some(err) = find_error(&b_results) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                // For each True in first arg, add second arg's results
                for a in &a_results {
                    if matches!(a, MettaValue::Bool(true)) {
                        for b in &b_results {
                            match b {
                                MettaValue::Bool(val) => {
                                    state.accumulated_results.push((MettaValue::Bool(*val), None));
                                }
                                _ => {
                                    return GroundedWork::Error(ExecError::Runtime(format!(
                                        "Cannot perform 'and': expected Bool, got {}",
                                        friendly_type_name(b)
                                    )));
                                }
                            }
                        }
                    }
                }

                GroundedWork::Done(state.accumulated_results.clone())
            }
            _ => unreachable!("Invalid step {} for AndOpTCO", state.step),
        }
    }
}

/// TCO Logical OR operation: (or a b)
/// Preserves short-circuit semantics: True OR _ = True without evaluating second arg
pub struct OrOpTCO;

impl GroundedOperationTCO for OrOpTCO {
    fn name(&self) -> &str {
        "or"
    }

    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
        match state.step {
            0 => {
                if state.args.len() != 2 {
                    return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                        "or requires 2 arguments, got {}",
                        state.args.len()
                    )));
                }
                state.step = 1;
                GroundedWork::EvalArg {
                    arg_idx: 0,
                    state: state.clone(),
                }
            }
            1 => {
                // Clone to avoid borrow conflict with accumulated_results
                let a_results: Vec<_> = state.get_arg(0).unwrap().iter().cloned().collect();
                if let Some(err) = find_error(&a_results) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut need_second_arg = false;
                for a in &a_results {
                    match a {
                        MettaValue::Bool(true) => {
                            // SHORT-CIRCUIT: True or _ = True
                            state.accumulated_results.push((MettaValue::Bool(true), None));
                        }
                        MettaValue::Bool(false) => {
                            // Need to evaluate second argument for this branch
                            need_second_arg = true;
                        }
                        _ => {
                            return GroundedWork::Error(ExecError::Runtime(format!(
                                "Cannot perform 'or': expected Bool, got {}",
                                friendly_type_name(a)
                            )));
                        }
                    }
                }

                if need_second_arg {
                    state.step = 2;
                    GroundedWork::EvalArg {
                        arg_idx: 1,
                        state: state.clone(),
                    }
                } else {
                    // All results were True (short-circuited)
                    GroundedWork::Done(state.accumulated_results.clone())
                }
            }
            2 => {
                // Clone both to avoid borrow conflict with accumulated_results
                let a_results: Vec<_> = state.get_arg(0).unwrap().iter().cloned().collect();
                let b_results: Vec<_> = state.get_arg(1).unwrap().iter().cloned().collect();

                if let Some(err) = find_error(&b_results) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                // For each False in first arg, add second arg's results
                for a in &a_results {
                    if matches!(a, MettaValue::Bool(false)) {
                        for b in &b_results {
                            match b {
                                MettaValue::Bool(val) => {
                                    state.accumulated_results.push((MettaValue::Bool(*val), None));
                                }
                                _ => {
                                    return GroundedWork::Error(ExecError::Runtime(format!(
                                        "Cannot perform 'or': expected Bool, got {}",
                                        friendly_type_name(b)
                                    )));
                                }
                            }
                        }
                    }
                }

                GroundedWork::Done(state.accumulated_results.clone())
            }
            _ => unreachable!("Invalid step {} for OrOpTCO", state.step),
        }
    }
}

/// TCO Logical NOT operation: (not a)
pub struct NotOpTCO;

impl GroundedOperationTCO for NotOpTCO {
    fn name(&self) -> &str {
        "not"
    }

    fn execute_step(&self, state: &mut GroundedState) -> GroundedWork {
        match state.step {
            0 => {
                if state.args.len() != 1 {
                    return GroundedWork::Error(ExecError::IncorrectArgument(format!(
                        "not requires 1 argument, got {}",
                        state.args.len()
                    )));
                }
                state.step = 1;
                GroundedWork::EvalArg {
                    arg_idx: 0,
                    state: state.clone(),
                }
            }
            1 => {
                let a_results = state.get_arg(0).unwrap();
                if let Some(err) = find_error(a_results) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut results = Vec::new();
                for a in a_results {
                    match a {
                        MettaValue::Bool(v) => {
                            results.push((MettaValue::Bool(!v), None));
                        }
                        _ => {
                            return GroundedWork::Error(ExecError::Runtime(format!(
                                "Cannot perform 'not': expected Bool, got {}",
                                friendly_type_name(a)
                            )));
                        }
                    }
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for NotOpTCO", state.step),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mock eval function for testing
    fn mock_eval(value: MettaValue, env: Environment) -> (Vec<MettaValue>, Environment) {
        // Just return the value as-is (no evaluation)
        (vec![value], env)
    }

    #[test]
    fn test_add_op() {
        let add = AddOp;
        let env = Environment::new();

        let args = vec![MettaValue::Long(2), MettaValue::Long(3)];
        let result = add.execute_raw(&args, &env, &mock_eval).unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, MettaValue::Long(5));
    }

    #[test]
    fn test_add_float() {
        let add = AddOp;
        let env = Environment::new();

        let args = vec![MettaValue::Float(2.5), MettaValue::Float(3.5)];
        let result = add.execute_raw(&args, &env, &mock_eval).unwrap();

        assert_eq!(result.len(), 1);
        if let MettaValue::Float(f) = result[0].0 {
            assert!((f - 6.0).abs() < f64::EPSILON);
        } else {
            panic!("Expected Float");
        }
    }

    #[test]
    fn test_comparison_less() {
        let less = LessOp;
        let env = Environment::new();

        let args = vec![MettaValue::Long(2), MettaValue::Long(3)];
        let result = less.execute_raw(&args, &env, &mock_eval).unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, MettaValue::Bool(true));
    }

    #[test]
    fn test_logical_and_short_circuit() {
        let and = AndOp;
        let env = Environment::new();

        // false AND <anything> should return false without evaluating second arg
        let args = vec![MettaValue::Bool(false), MettaValue::Atom("error".to_string())];
        let result = and.execute_raw(&args, &env, &mock_eval).unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, MettaValue::Bool(false));
    }

    #[test]
    fn test_equality() {
        let eq = EqualOp;
        let env = Environment::new();

        // Test Nil == ()
        let args = vec![MettaValue::Nil, MettaValue::SExpr(vec![])];
        let result = eq.execute_raw(&args, &env, &mock_eval).unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, MettaValue::Bool(true));
    }

    #[test]
    fn test_division_by_zero() {
        let div = DivOp;
        let env = Environment::new();

        let args = vec![MettaValue::Long(10), MettaValue::Long(0)];
        let result = div.execute_raw(&args, &env, &mock_eval);

        assert!(matches!(result, Err(ExecError::Runtime(_))));
    }

    #[test]
    fn test_incorrect_arity() {
        let add = AddOp;
        let env = Environment::new();

        let args = vec![MettaValue::Long(1)];
        let result = add.execute_raw(&args, &env, &mock_eval);

        assert!(matches!(result, Err(ExecError::IncorrectArgument(_))));
    }

    #[test]
    fn test_type_error_on_type_mismatch() {
        let add = AddOp;
        let env = Environment::new();

        let args = vec![
            MettaValue::Long(1),
            MettaValue::Atom("not-a-number".to_string()),
        ];
        let result = add.execute_raw(&args, &env, &mock_eval);

        // Type mismatch should return a Runtime error, not NoReduce
        assert!(matches!(result, Err(ExecError::Runtime(_))));
    }
}
