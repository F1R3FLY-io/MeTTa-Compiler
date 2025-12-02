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
