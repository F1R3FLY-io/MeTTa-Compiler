//! Bytecode VM Module
//!
//! This module provides a stack-based bytecode virtual machine for executing
//! MeTTa programs. The VM replaces the tree-walking interpreter with a more
//! efficient bytecode-based execution model.
//!
//! # Architecture
//!
//! ```text
//! ┌───────────────────────────────────────────────────────────────────┐
//! │                        MeTTa Source                                │
//! └───────────────────────────────────────────────────────────────────┘
//!                                 │
//!                                 ▼
//! ┌───────────────────────────────────────────────────────────────────┐
//! │                    Parser (Tree-Sitter)                           │
//! │              Source → MettaValue (AST)                            │
//! └───────────────────────────────────────────────────────────────────┘
//!                                 │
//!                                 ▼
//! ┌───────────────────────────────────────────────────────────────────┐
//! │                    Bytecode Compiler                              │
//! │             MettaValue → BytecodeChunk                            │
//! └───────────────────────────────────────────────────────────────────┘
//!                                 │
//!                                 ▼
//! ┌───────────────────────────────────────────────────────────────────┐
//! │                    Bytecode VM                                    │
//! │                                                                   │
//! │  ┌─────────────┐  ┌─────────────┐  ┌─────────────┐               │
//! │  │ Value Stack │  │ Call Stack  │  │ Bindings    │               │
//! │  │             │  │             │  │ Stack       │               │
//! │  └─────────────┘  └─────────────┘  └─────────────┘               │
//! │                                                                   │
//! │  ┌─────────────┐  ┌─────────────────────────────────────────┐    │
//! │  │ Choice Pts  │  │            MORK Bridge                  │    │
//! │  │ (Nondet)    │  │  - Rule lookup via PathMap              │    │
//! │  └─────────────┘  │  - Pattern matching                     │    │
//! │                   │  - Compiled rule cache                  │    │
//! │                   └─────────────────────────────────────────┘    │
//! └───────────────────────────────────────────────────────────────────┘
//! ```
//!
//! # Modules
//!
//! - [`opcodes`]: Bytecode instruction definitions (~100 opcodes)
//! - [`chunk`]: BytecodeChunk structure with constant pool
//! - [`vm`]: Virtual machine execution engine
//!
//! # Example
//!
//! ```ignore
//! use mettatron::backend::bytecode::{BytecodeChunk, BytecodeVM, Opcode, ChunkBuilder};
//!
//! // Build a simple program: 40 + 2
//! let mut builder = ChunkBuilder::new("example");
//! builder.emit_byte(Opcode::PushLongSmall, 40);
//! builder.emit_byte(Opcode::PushLongSmall, 2);
//! builder.emit(Opcode::Add);
//! builder.emit(Opcode::Return);
//!
//! let chunk = builder.build_arc();
//! let mut vm = BytecodeVM::new(chunk);
//! let results = vm.run().expect("execution failed");
//! assert_eq!(results[0], MettaValue::Long(42));
//! ```
//!
//! # Performance Goals
//!
//! The bytecode VM aims to achieve 15-30% speedup over the tree-walking
//! interpreter by:
//!
//! 1. **Eliminating dispatch overhead**: Direct opcode dispatch vs enum matching
//! 2. **Pre-compiled patterns**: Pattern structure analyzed at compile time
//! 3. **Constant pool**: Reduced allocation for repeated values
//! 4. **Inline caching**: Hot paths get specialized code
//!
//! # Nondeterminism
//!
//! MeTTa supports nondeterministic evaluation (multiple results). The VM
//! handles this via choice points and backtracking:
//!
//! - `Fork`: Create a choice point with multiple alternatives
//! - `Fail`: Backtrack to most recent choice point
//! - `Yield`: Save a result and continue searching for more
//! - `Cut`: Remove choice points (commit to current path)
//!
//! # MORK Integration
//!
//! Rule lookup still uses MORK/PathMap for efficient pattern matching:
//!
//! 1. Expression to evaluate → MORK finds matching rules
//! 2. Matched rules compiled to bytecode (cached)
//! 3. VM executes compiled rule body
//!
//! This hybrid approach keeps MORK's O(k) pattern matching while
//! gaining bytecode's execution efficiency.

pub mod opcodes;
pub mod cache;
pub mod chunk;
pub mod vm;
pub mod compiler;
pub mod mork_bridge;
pub mod native_registry;
pub mod memo_cache;
pub mod external_registry;
pub mod space_registry;
pub mod optimizer;

/// Cranelift JIT compilation module
///
/// Provides native code generation for hot bytecode paths.
/// Enabled via the `jit` feature flag.
#[cfg(feature = "jit")]
pub mod jit;

/// JIT stub module when feature is disabled
#[cfg(not(feature = "jit"))]
pub mod jit;

// Re-export main types
pub use opcodes::Opcode;
pub use chunk::{BytecodeChunk, ChunkBuilder, CompiledPattern, JumpLabel, JumpLabelShort, JumpTable};
pub use vm::{BytecodeVM, VmConfig, VmError, VmResult, CallFrame, BindingFrame, ChoicePoint, Alternative};
pub use compiler::{Compiler, CompileContext, CompileError, CompileResult, compile, compile_arc};
pub use mork_bridge::{MorkBridge, CompiledRule, BridgeStats};
pub use cache::{BytecodeCacheStats, get_stats as cache_stats, cache_sizes, clear_caches};
pub use native_registry::{NativeRegistry, NativeContext, NativeError, NativeResult, NativeFn};
pub use memo_cache::{MemoCache, CacheStats as MemoCacheStats};
pub use external_registry::{ExternalRegistry, ExternalContext, ExternalError, ExternalResult, ExternalFn};
pub use space_registry::SpaceRegistry;
pub use optimizer::{
    PeepholeOptimizer, OptimizationStats, optimize_bytecode,
    DeadCodeEliminator, DceStats, eliminate_dead_code, optimize_bytecode_full,
    PeepholeAction,
};

// JIT re-exports (when jit feature is enabled)
#[cfg(feature = "jit")]
pub use jit::{
    // Hybrid executor
    HybridExecutor, HybridConfig, HybridStats,
    // Tiered compilation
    JitCache, TieredCompiler, TieredStats, Tier, ChunkId, CacheEntry,
    // JIT types
    JitValue, JitContext, JitResult, JitError, JitBailoutReason,
    JitChoicePoint, JitAlternative, JitAlternativeTag,
    JitBindingEntry, JitBindingFrame,
    // JIT profiling
    JitProfile, JitState, HOT_THRESHOLD, STAGE2_THRESHOLD,
    // JIT signals
    JIT_SIGNAL_OK, JIT_SIGNAL_YIELD, JIT_SIGNAL_FAIL, JIT_SIGNAL_ERROR,
    JIT_SIGNAL_HALT, JIT_SIGNAL_BAILOUT,
};

/// Feature flag for enabling bytecode VM
///
/// When enabled, the evaluator will attempt to compile and execute
/// expressions via bytecode before falling back to tree-walking.
#[cfg(feature = "bytecode")]
pub const BYTECODE_ENABLED: bool = true;

#[cfg(not(feature = "bytecode"))]
pub const BYTECODE_ENABLED: bool = false;

/// Global space registry for JIT runtime
///
/// This registry is shared across all JIT executions and provides named space
/// lookup for the `eval-new` and `load-space` operations.
#[cfg(feature = "jit")]
static GLOBAL_SPACE_REGISTRY: std::sync::LazyLock<SpaceRegistry> =
    std::sync::LazyLock::new(SpaceRegistry::new);

/// Get a reference to the global space registry
#[cfg(feature = "jit")]
pub fn global_space_registry() -> &'static SpaceRegistry {
    &GLOBAL_SPACE_REGISTRY
}

use crate::backend::models::MettaValue;

/// Error type for bytecode evaluation
#[derive(Debug)]
pub enum BytecodeEvalError {
    /// Compilation failed
    CompileError(CompileError),
    /// VM execution failed
    VmError(VmError),
}

impl From<CompileError> for BytecodeEvalError {
    fn from(e: CompileError) -> Self {
        BytecodeEvalError::CompileError(e)
    }
}

impl From<VmError> for BytecodeEvalError {
    fn from(e: VmError) -> Self {
        BytecodeEvalError::VmError(e)
    }
}

impl std::fmt::Display for BytecodeEvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CompileError(e) => write!(f, "Compilation error: {}", e),
            Self::VmError(e) => write!(f, "VM error: {}", e),
        }
    }
}

impl std::error::Error for BytecodeEvalError {}

/// Check if an expression can be compiled to bytecode
///
/// Returns true for expressions that the bytecode compiler supports.
/// Currently supports:
/// - Literals (numbers, bools, strings)
/// - Variables (atoms starting with $)
/// - Arithmetic operations (+, -, *, /, %) with compilable operands
/// - Comparison operations (<, <=, >, >=, ==) with compilable operands
/// - Boolean operations (and, or, not) with compilable operands
/// - Quote (returns argument unevaluated)
/// - Superpose/collapse (nondeterminism) with compilable alternatives
///
/// **Important**: This function is recursive - it checks that ALL subexpressions
/// can also be compiled. This prevents the bytecode VM from returning wrong results
/// when a subexpression needs rule resolution.
pub fn can_compile(expr: &MettaValue) -> bool {
    match expr {
        // Always compilable literals
        MettaValue::Nil | MettaValue::Unit | MettaValue::Bool(_) |
        MettaValue::Long(_) | MettaValue::Float(_) | MettaValue::String(_) => true,

        // Atoms: only variables and known constants are safe
        // - Variables (start with $) are OK - they'll be substituted
        // - Other atoms might need rule resolution, so reject them
        MettaValue::Atom(name) => {
            // Variables are OK
            if name.starts_with('$') {
                return true;
            }
            // Known constants are OK
            match name.as_str() {
                "True" | "False" | "Nil" | "Unit" | "_" => true,
                // Other atoms could be function calls - reject
                _ => false,
            }
        }

        // S-expressions - check head AND all operands recursively
        MettaValue::SExpr(items) if items.is_empty() => true,
        MettaValue::SExpr(items) => {
            // Check head for supported operations
            if let MettaValue::Atom(head) = &items[0] {
                let head_ok = match head.as_str() {
                    // Arithmetic
                    "+" | "-" | "*" | "/" | "%" | "abs" | "pow" => true,
                    // Comparison
                    "<" | "<=" | ">" | ">=" | "==" | "!=" => true,
                    // Boolean
                    "and" | "or" | "not" | "xor" => true,
                    // Control flow - if needs compilable condition and branches
                    "if" => true,
                    // Quote - argument is NOT evaluated, so always OK
                    "quote" => return true, // Early return - don't check args
                    // Nondeterminism
                    "superpose" => true,
                    // NOTE: collapse intentionally NOT included - needs EvalCollapse VM impl
                    // List operations
                    "car-atom" | "cdr-atom" | "cons-atom" | "size-atom" => true,
                    // Extended list operations
                    "decons-atom" | "empty" => true,
                    // String operations
                    "repr" => true,
                    // Type operations (only get-metatype doesn't need env type assertions)
                    "get-metatype" => true,
                    // Note: get-type and check-type need environment type assertions
                    // Binding forms
                    "let" | "let*" => true,
                    // Chain operation (sequence/binding)
                    "chain" => return can_compile_chain(items),
                    // Higher-order list operations
                    "map-atom" => return can_compile_map_atom(items),
                    "filter-atom" => return can_compile_filter_atom(items),
                    "foldl-atom" => return can_compile_foldl_atom(items),
                    // Control flow pattern matching (case doesn't need space)
                    "case" => true,
                    // Note: match and unify need space access, use tree-walker
                    // Error handling
                    "error" | "is-error" | "catch" => true,
                    // Reject everything else
                    _ => false,
                };

                if !head_ok {
                    return false;
                }

                // IMPORTANT: Recursively check all operands
                // This ensures we don't compile (+ 1 (foo $x)) where (foo $x)
                // would need rule resolution
                items.iter().skip(1).all(can_compile)
            } else {
                // Non-atom head - this is a data list like (1 2 3), not a function call
                // All elements must be compilable
                items.iter().all(can_compile)
            }
        }

        // Errors can be compiled (they just push the error value)
        MettaValue::Error(_, _) => true,

        // Types that need environment or special runtime support
        MettaValue::Space(_) | MettaValue::State(_) | MettaValue::Type(_) |
        MettaValue::Conjunction(_) | MettaValue::Memo(_) => false,

        // Empty is a sentinel that should be filtered, but can be compiled if needed
        MettaValue::Empty => true,
    }
}

/// Check if an expression can be compiled to bytecode, with caching
///
/// This is the primary entry point for compilability checks.
/// Results are cached to avoid repeated tree traversals for the same expression.
#[inline]
pub fn can_compile_cached(expr: &MettaValue) -> bool {
    let hash = cache::hash_metta_value(expr);

    // Check cache first
    if let Some(result) = cache::get_cached_can_compile(hash) {
        return result;
    }

    // Cache miss - compute and cache
    let result = can_compile(expr);
    cache::cache_can_compile(hash, result);
    result
}

/// Check if an expression can be compiled when an environment is available.
///
/// This is more permissive than `can_compile()` - it allows:
/// - Rule definitions: `(= pattern body)` - compiled to DefineRule opcode
/// - User-defined function calls: unknown atoms become DispatchRules calls
/// - Everything that `can_compile()` allows
///
/// Use this when bytecode execution will have access to an Environment for
/// rule lookup and definition (e.g., mmverify workloads).
pub fn can_compile_with_env(expr: &MettaValue) -> bool {
    match expr {
        // Always compilable literals
        MettaValue::Nil | MettaValue::Unit | MettaValue::Bool(_) |
        MettaValue::Long(_) | MettaValue::Float(_) | MettaValue::String(_) => true,

        // Atoms: variables, known constants, AND unknown atoms (for rule dispatch)
        MettaValue::Atom(name) => {
            // Variables are OK
            if name.starts_with('$') {
                return true;
            }
            // Grounded references (&self, &kb, etc.) need special tree-walker handling
            if name.starts_with('&') {
                return false;
            }
            // Known constants are OK
            match name.as_str() {
                "True" | "False" | "Nil" | "Unit" | "_" => true,
                // With environment: unknown atoms are compilable as DispatchRules calls
                _ => true,
            }
        }

        // S-expressions - check head AND all operands recursively
        MettaValue::SExpr(items) if items.is_empty() => true,
        MettaValue::SExpr(items) => {
            // Check head for supported operations
            if let MettaValue::Atom(head) = &items[0] {
                let head_ok = match head.as_str() {
                    // Rule definitions need tree-walker (bytecode compiler doesn't emit DefineRule)
                    "=" => false,
                    // Evaluation
                    "!" => true,
                    // Arithmetic
                    "+" | "-" | "*" | "/" | "%" | "abs" | "pow" => true,
                    // Comparison
                    "<" | "<=" | ">" | ">=" | "==" | "!=" => true,
                    // Boolean
                    "and" | "or" | "not" | "xor" => true,
                    // Control flow
                    "if" => true,
                    // Quote - argument is NOT evaluated
                    "quote" => return true,
                    // Nondeterminism
                    "superpose" => true,
                    // List operations
                    "car-atom" | "cdr-atom" | "cons-atom" | "size-atom" |
                    "decons-atom" | "empty" => true,
                    // String operations
                    "repr" => true,
                    // Type operations (only get-metatype doesn't need env type assertions)
                    "get-metatype" => true,
                    // Note: get-type and check-type need environment type assertions
                    // Binding forms
                    "let" | "let*" => true,
                    // Chain operation
                    "chain" => return can_compile_chain_with_env(items),
                    // Higher-order list operations
                    "map-atom" => return can_compile_map_atom_with_env(items),
                    "filter-atom" => return can_compile_filter_atom_with_env(items),
                    "foldl-atom" => return can_compile_foldl_atom_with_env(items),
                    // Control flow pattern matching (case doesn't need space)
                    "case" => true,
                    // Note: match and unify need space access, use tree-walker
                    // Error handling
                    "error" | "is-error" | "catch" => true,
                    // Unknown operations fall back to tree-walker
                    // (switch, collapse, etc. are not yet implemented in bytecode)
                    _ => false,
                };

                if !head_ok {
                    return false;
                }

                // Recursively check all operands with env support
                items.iter().skip(1).all(can_compile_with_env)
            } else {
                // Non-atom head - data list, all elements must be compilable
                items.iter().all(can_compile_with_env)
            }
        }

        // Errors can be compiled
        MettaValue::Error(_, _) => true,

        // Types that need special runtime support (even with environment)
        MettaValue::Space(_) | MettaValue::State(_) | MettaValue::Type(_) |
        MettaValue::Conjunction(_) | MettaValue::Memo(_) => false,

        // Empty sentinel
        MettaValue::Empty => true,
    }
}

/// Check if a chain expression can be compiled with environment support
fn can_compile_chain_with_env(items: &[MettaValue]) -> bool {
    if items.len() != 4 {
        return false;
    }
    let var_ok = matches!(&items[2], MettaValue::Atom(s) if s.starts_with('$'));
    var_ok && can_compile_with_env(&items[1]) && can_compile_with_env(&items[3])
}

/// Check if a map-atom expression can be compiled with environment support
fn can_compile_map_atom_with_env(items: &[MettaValue]) -> bool {
    if items.len() != 4 {
        return false;
    }
    let var_ok = matches!(&items[2], MettaValue::Atom(s) if s.starts_with('$'));
    var_ok && can_compile_with_env(&items[1])
}

/// Check if a filter-atom expression can be compiled with environment support
fn can_compile_filter_atom_with_env(items: &[MettaValue]) -> bool {
    if items.len() != 4 {
        return false;
    }
    let var_ok = matches!(&items[2], MettaValue::Atom(s) if s.starts_with('$'));
    var_ok && can_compile_with_env(&items[1])
}

/// Check if a foldl-atom expression can be compiled with environment support
fn can_compile_foldl_atom_with_env(items: &[MettaValue]) -> bool {
    if items.len() != 5 {
        return false;
    }
    let var_ok = matches!(&items[3], MettaValue::Atom(s) if s.starts_with('$'));
    var_ok && can_compile_with_env(&items[1]) && can_compile_with_env(&items[2])
}

/// Check if a chain expression can be compiled
/// (chain expr $var body) - expr and body must be compilable, $var must be a variable
fn can_compile_chain(items: &[MettaValue]) -> bool {
    if items.len() != 4 {
        return false;
    }
    // items[0] is "chain"
    // items[1] is expr - must be compilable
    // items[2] is $var - must be a variable
    // items[3] is body - must be compilable
    let var_ok = matches!(&items[2], MettaValue::Atom(s) if s.starts_with('$'));
    var_ok && can_compile(&items[1]) && can_compile(&items[3])
}

/// Check if a map-atom expression can be compiled
/// (map-atom list $var template) - list must be compilable, $var must be a variable
fn can_compile_map_atom(items: &[MettaValue]) -> bool {
    if items.len() != 4 {
        return false;
    }
    // items[0] is "map-atom"
    // items[1] is list - must be compilable
    // items[2] is $var - must be a variable
    // items[3] is template - compiled as sub-chunk, so we accept it
    let var_ok = matches!(&items[2], MettaValue::Atom(s) if s.starts_with('$'));
    var_ok && can_compile(&items[1])
}

/// Check if a filter-atom expression can be compiled
/// (filter-atom list $var predicate) - list must be compilable, $var must be a variable
fn can_compile_filter_atom(items: &[MettaValue]) -> bool {
    if items.len() != 4 {
        return false;
    }
    let var_ok = matches!(&items[2], MettaValue::Atom(s) if s.starts_with('$'));
    var_ok && can_compile(&items[1])
}

/// Check if a foldl-atom expression can be compiled
/// (foldl-atom list init $acc $item op) - list and init must be compilable, vars must be variables
fn can_compile_foldl_atom(items: &[MettaValue]) -> bool {
    if items.len() != 6 {
        return false;
    }
    // items[0] is "foldl-atom"
    // items[1] is list
    // items[2] is init
    // items[3] is $acc
    // items[4] is $item
    // items[5] is op (compiled as sub-chunk)
    let acc_ok = matches!(&items[3], MettaValue::Atom(s) if s.starts_with('$'));
    let item_ok = matches!(&items[4], MettaValue::Atom(s) if s.starts_with('$'));
    acc_ok && item_ok && can_compile(&items[1]) && can_compile(&items[2])
}

/// Evaluate an expression using the bytecode VM
///
/// Compiles the expression to bytecode and executes it.
/// Returns the results or an error if compilation/execution fails.
///
/// # Example
/// ```ignore
/// let expr = MettaValue::SExpr(vec![
///     MettaValue::Atom("+".to_string()),
///     MettaValue::Long(1),
///     MettaValue::Long(2),
/// ]);
/// let results = eval_bytecode(&expr)?;
/// assert_eq!(results[0], MettaValue::Long(3));
/// ```
pub fn eval_bytecode(expr: &MettaValue) -> Result<Vec<MettaValue>, BytecodeEvalError> {
    // NOTE: Bytecode caching disabled - causes semantic differences due to
    // expressions with same structure but different runtime variable bindings.
    // Only can_compile caching is safe (structural check only).
    let chunk = compile_arc("eval", expr)?;
    let mut vm = BytecodeVM::new(chunk);
    let results = vm.run()?;
    Ok(results)
}

/// Evaluate an expression using the bytecode VM with configuration
pub fn eval_bytecode_with_config(
    expr: &MettaValue,
    config: VmConfig,
) -> Result<Vec<MettaValue>, BytecodeEvalError> {
    let chunk = compile_arc("eval", expr)?;
    let mut vm = BytecodeVM::with_config(chunk, config);
    let results = vm.run()?;
    Ok(results)
}

/// Evaluate an expression using the bytecode VM with an environment.
///
/// This enables bytecode compilation and execution for expressions that need
/// environment access (rule definitions, user-defined function calls).
/// The environment is threaded through execution and returned alongside results.
///
/// # Arguments
/// * `expr` - The MeTTa expression to evaluate
/// * `env` - The environment for rule definitions and lookups
///
/// # Returns
/// Tuple of (results, modified_environment) or an error
///
/// # Example
/// ```ignore
/// let env = Environment::new();
/// let rule_def = MettaValue::SExpr(vec![
///     MettaValue::Atom("=".to_string()),
///     MettaValue::SExpr(vec![
///         MettaValue::Atom("double".to_string()),
///         MettaValue::Atom("$x".to_string()),
///     ]),
///     MettaValue::SExpr(vec![
///         MettaValue::Atom("*".to_string()),
///         MettaValue::Atom("$x".to_string()),
///         MettaValue::Long(2),
///     ]),
/// ]);
/// let (results, new_env) = eval_bytecode_with_env(&rule_def, env)?;
/// ```
pub fn eval_bytecode_with_env(
    expr: &MettaValue,
    env: crate::backend::Environment,
) -> Result<(Vec<MettaValue>, crate::backend::Environment), BytecodeEvalError> {
    let chunk = compile_arc("eval", expr)?;
    let mut vm = BytecodeVM::with_env(chunk, env);
    let (results, modified_env) = vm.run_with_env()?;
    // Return the environment if present, otherwise create a new one
    let final_env = modified_env.unwrap_or_else(crate::backend::Environment::new);
    Ok((results, final_env))
}

/// Try to evaluate via bytecode with environment, falling back to provided fallback function
///
/// This is the integration point for environment-aware bytecode evaluation.
/// If bytecode is enabled and the expression is compilable (with env support),
/// it tries bytecode. On success, returns bytecode results and modified env.
/// On failure, calls the fallback.
pub fn try_bytecode_eval_with_env<F>(
    expr: &MettaValue,
    env: crate::backend::Environment,
    fallback: F,
) -> (Vec<MettaValue>, crate::backend::Environment)
where
    F: FnOnce(crate::backend::Environment) -> (Vec<MettaValue>, crate::backend::Environment),
{
    if BYTECODE_ENABLED && can_compile_with_env(expr) {
        match eval_bytecode_with_env(expr, env.clone()) {
            Ok((results, new_env)) => (results, new_env),
            Err(_) => fallback(env),
        }
    } else {
        fallback(env)
    }
}

/// Try to evaluate via bytecode, falling back to provided fallback function
///
/// This is the recommended integration point for the main eval loop.
/// If bytecode is enabled and the expression is compilable, it tries bytecode.
/// On success, returns bytecode results. On failure, calls the fallback.
pub fn try_bytecode_eval<F>(expr: &MettaValue, fallback: F) -> Vec<MettaValue>
where
    F: FnOnce() -> Vec<MettaValue>,
{
    if BYTECODE_ENABLED && can_compile(expr) {
        match eval_bytecode(expr) {
            Ok(results) => results,
            Err(_) => fallback(),
        }
    } else {
        fallback()
    }
}

// =============================================================================
// Hybrid JIT/VM Evaluation
// =============================================================================

/// Evaluate an expression using the hybrid JIT/bytecode executor
///
/// This function uses the HybridExecutor which automatically switches between
/// JIT-compiled native code and bytecode VM based on execution hotness.
///
/// # Arguments
/// * `expr` - The MeTTa expression to evaluate
///
/// # Returns
/// Results of evaluation or an error
///
/// # Example
/// ```ignore
/// let expr = MettaValue::SExpr(vec![
///     MettaValue::sym("+"),
///     MettaValue::Long(1),
///     MettaValue::Long(2),
/// ]);
/// let results = eval_bytecode_hybrid(&expr)?;
/// assert_eq!(results[0], MettaValue::Long(3));
/// ```
#[cfg(feature = "jit")]
pub fn eval_bytecode_hybrid(expr: &MettaValue) -> Result<Vec<MettaValue>, BytecodeEvalError> {
    let chunk = compile_arc("eval", expr)?;
    let mut executor = HybridExecutor::new();

    // Connect the global space registry to the executor for eval-new and load-space operations
    // Safety: The global registry has static lifetime, so the pointer remains valid
    let registry_ptr = global_space_registry() as *const SpaceRegistry as *mut ();
    unsafe {
        executor.set_space_registry(registry_ptr);
    }

    let results = executor.run(&chunk)?;
    Ok(results)
}

/// Evaluate with hybrid executor using a shared executor instance
///
/// This is more efficient when evaluating many expressions since it allows
/// the JIT cache and tiered compiler state to be reused across evaluations.
///
/// # Arguments
/// * `executor` - Shared HybridExecutor instance
/// * `expr` - The MeTTa expression to evaluate
///
/// # Returns
/// Results of evaluation or an error
#[cfg(feature = "jit")]
pub fn eval_bytecode_hybrid_shared(
    executor: &mut HybridExecutor,
    expr: &MettaValue,
) -> Result<Vec<MettaValue>, BytecodeEvalError> {
    let chunk = compile_arc("eval", expr)?;
    let results = executor.run(&chunk)?;
    Ok(results)
}

/// Evaluate with hybrid executor using custom configuration
///
/// # Arguments
/// * `expr` - The MeTTa expression to evaluate
/// * `config` - HybridConfig for customizing JIT behavior
///
/// # Returns
/// Results of evaluation or an error
#[cfg(feature = "jit")]
pub fn eval_bytecode_hybrid_with_config(
    expr: &MettaValue,
    config: HybridConfig,
) -> Result<Vec<MettaValue>, BytecodeEvalError> {
    let chunk = compile_arc("eval", expr)?;
    let mut executor = HybridExecutor::with_config(config);

    // Connect the global space registry to the executor for eval-new and load-space operations
    // Safety: The global registry has static lifetime, so the pointer remains valid
    let registry_ptr = global_space_registry() as *const SpaceRegistry as *mut ();
    unsafe {
        executor.set_space_registry(registry_ptr);
    }

    let results = executor.run(&chunk)?;
    Ok(results)
}

/// Try to evaluate via hybrid JIT/bytecode, falling back to provided fallback
///
/// Similar to `try_bytecode_eval` but uses the HybridExecutor for potential
/// JIT speedups on hot code paths.
#[cfg(feature = "jit")]
pub fn try_hybrid_eval<F>(expr: &MettaValue, fallback: F) -> Vec<MettaValue>
where
    F: FnOnce() -> Vec<MettaValue>,
{
    if BYTECODE_ENABLED && can_compile(expr) {
        match eval_bytecode_hybrid(expr) {
            Ok(results) => results,
            Err(_) => fallback(),
        }
    } else {
        fallback()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::models::MettaValue;

    #[test]
    fn test_integration_arithmetic() {
        // Test: (+ (* 3 4) 5) = 17
        let mut builder = ChunkBuilder::new("arithmetic");

        // Push 3, 4, multiply
        builder.emit_byte(Opcode::PushLongSmall, 3);
        builder.emit_byte(Opcode::PushLongSmall, 4);
        builder.emit(Opcode::Mul);

        // Push 5, add
        builder.emit_byte(Opcode::PushLongSmall, 5);
        builder.emit(Opcode::Add);

        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let results = vm.run().expect("VM should succeed");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(17));
    }

    #[test]
    fn test_integration_conditionals() {
        // Test: if (< 5 10) then 1 else 2
        let mut builder = ChunkBuilder::new("conditionals");

        // Compare 5 < 10
        builder.emit_byte(Opcode::PushLongSmall, 5);
        builder.emit_byte(Opcode::PushLongSmall, 10);
        builder.emit(Opcode::Lt);

        // Jump to else if false
        let else_jump = builder.emit_jump(Opcode::JumpIfFalse);

        // Then branch: push 1
        builder.emit_byte(Opcode::PushLongSmall, 1);
        let end_jump = builder.emit_jump(Opcode::Jump);

        // Else branch: push 2
        builder.patch_jump(else_jump);
        builder.emit_byte(Opcode::PushLongSmall, 2);

        builder.patch_jump(end_jump);
        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let results = vm.run().expect("VM should succeed");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(1)); // 5 < 10 is true
    }

    #[test]
    fn test_integration_boolean_logic() {
        // Test: (and (or True False) (not False))
        let mut builder = ChunkBuilder::new("boolean");

        // (or True False)
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::PushFalse);
        builder.emit(Opcode::Or);

        // (not False)
        builder.emit(Opcode::PushFalse);
        builder.emit(Opcode::Not);

        // (and ...)
        builder.emit(Opcode::And);

        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let results = vm.run().expect("VM should succeed");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Bool(true));
    }

    #[test]
    fn test_integration_stack_operations() {
        // Test: dup, swap, over operations
        let mut builder = ChunkBuilder::new("stack_ops");

        // Push 1, 2, swap -> [2, 1]
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit_byte(Opcode::PushLongSmall, 2);
        builder.emit(Opcode::Swap);

        // Now we have [2, 1], subtract -> 2 - 1 = 1
        builder.emit(Opcode::Sub);

        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let results = vm.run().expect("VM should succeed");

        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(1));
    }

    #[test]
    fn test_integration_sexpr_construction() {
        // Test: Build (foo 1 2)
        let mut builder = ChunkBuilder::new("sexpr");

        let foo_idx = builder.add_constant(MettaValue::sym("foo"));
        builder.emit_u16(Opcode::PushAtom, foo_idx);
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit_byte(Opcode::PushLongSmall, 2);
        builder.emit_byte(Opcode::MakeSExpr, 3);

        builder.emit(Opcode::Return);

        let chunk = builder.build_arc();
        let mut vm = BytecodeVM::new(chunk);
        let results = vm.run().expect("VM should succeed");

        assert_eq!(results.len(), 1);
        match &results[0] {
            MettaValue::SExpr(items) => {
                assert_eq!(items.len(), 3);
                assert_eq!(items[0], MettaValue::sym("foo"));
                assert_eq!(items[1], MettaValue::Long(1));
                assert_eq!(items[2], MettaValue::Long(2));
            }
            _ => panic!("Expected S-expression"),
        }
    }

    #[test]
    fn test_chunk_disassembly() {
        let mut builder = ChunkBuilder::new("disasm_test");
        builder.set_line(1);
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.set_line(2);
        builder.emit(Opcode::Dup);
        builder.emit(Opcode::Add);
        builder.set_line(3);
        builder.emit(Opcode::Return);

        let chunk = builder.build();
        let disasm = chunk.disassemble();

        assert!(disasm.contains("disasm_test"));
        assert!(disasm.contains("push_long_small 42"));
        assert!(disasm.contains("dup"));
        assert!(disasm.contains("add"));
        assert!(disasm.contains("return"));
    }

    // Integration function tests

    #[test]
    fn test_can_compile_literals() {
        // Compilable literals
        assert!(can_compile(&MettaValue::Nil));
        assert!(can_compile(&MettaValue::Unit));
        assert!(can_compile(&MettaValue::Bool(true)));
        assert!(can_compile(&MettaValue::Long(42)));
        assert!(can_compile(&MettaValue::Float(3.14)));
        assert!(can_compile(&MettaValue::String("hello".to_string())));

        // Variables are compilable
        assert!(can_compile(&MettaValue::Atom("$x".to_string())));

        // Known constants are compilable
        assert!(can_compile(&MettaValue::Atom("True".to_string())));
        assert!(can_compile(&MettaValue::Atom("False".to_string())));
        assert!(can_compile(&MettaValue::Atom("Nil".to_string())));
        assert!(can_compile(&MettaValue::Atom("Unit".to_string())));
        assert!(can_compile(&MettaValue::Atom("_".to_string()))); // Wildcard

        // Plain atoms are NOT compilable (they could be function calls)
        assert!(!can_compile(&MettaValue::Atom("foo".to_string())));
        assert!(!can_compile(&MettaValue::Atom("bar".to_string())));
    }

    #[test]
    fn test_can_compile_arithmetic() {
        // Arithmetic operations
        let add = MettaValue::SExpr(vec![
            MettaValue::sym("+"),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        assert!(can_compile(&add));

        let mul = MettaValue::SExpr(vec![
            MettaValue::sym("*"),
            MettaValue::Long(3),
            MettaValue::Long(4),
        ]);
        assert!(can_compile(&mul));
    }

    #[test]
    fn test_can_compile_comparisons() {
        let lt = MettaValue::SExpr(vec![
            MettaValue::sym("<"),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        assert!(can_compile(&lt));

        let eq = MettaValue::SExpr(vec![
            MettaValue::sym("=="),
            MettaValue::Long(1),
            MettaValue::Long(1),
        ]);
        assert!(can_compile(&eq));
    }

    #[test]
    fn test_can_compile_control_flow() {
        let if_expr = MettaValue::SExpr(vec![
            MettaValue::sym("if"),
            MettaValue::Bool(true),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        assert!(can_compile(&if_expr));

        let quote_expr = MettaValue::SExpr(vec![
            MettaValue::sym("quote"),
            MettaValue::Long(42),
        ]);
        assert!(can_compile(&quote_expr));
    }

    #[test]
    fn test_can_compile_nondeterminism() {
        let superpose = MettaValue::SExpr(vec![
            MettaValue::sym("superpose"),
            MettaValue::SExpr(vec![
                MettaValue::Long(1),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
        ]);
        assert!(can_compile(&superpose));

        // collapse is NOT compilable (EvalCollapse opcode not implemented)
        let collapse = MettaValue::SExpr(vec![
            MettaValue::sym("collapse"),
            MettaValue::Long(42),
        ]);
        assert!(!can_compile(&collapse));
    }

    #[test]
    fn test_can_compile_not_compilable() {
        use crate::backend::models::SpaceHandle;

        // Rule definition (needs environment registration)
        let rule = MettaValue::SExpr(vec![
            MettaValue::sym("="),
            MettaValue::sym("foo"),
            MettaValue::Long(42),
        ]);
        assert!(!can_compile(&rule));

        // Space values (runtime state)
        let space = MettaValue::Space(SpaceHandle::new(1, "test".to_string()));
        assert!(!can_compile(&space));

        // State values (runtime state)
        let state = MettaValue::State(123);
        assert!(!can_compile(&state));

        // User-defined function calls (need rule lookup)
        let func_call = MettaValue::SExpr(vec![
            MettaValue::sym("my-function"),
            MettaValue::Long(1),
        ]);
        assert!(!can_compile(&func_call));

        // Bind operation (needs environment mutation)
        let bind_expr = MettaValue::SExpr(vec![
            MettaValue::sym("bind!"),
            MettaValue::sym("token"),
            MettaValue::Long(42),
        ]);
        assert!(!can_compile(&bind_expr));
    }

    #[test]
    fn test_can_compile_let_forms() {
        // Simple let is now compilable
        let let_expr = MettaValue::SExpr(vec![
            MettaValue::sym("let"),
            MettaValue::sym("$x"),
            MettaValue::Long(1),
            MettaValue::sym("$x"),
        ]);
        assert!(can_compile(&let_expr));

        // Nested let with arithmetic
        let nested_let = MettaValue::SExpr(vec![
            MettaValue::sym("let"),
            MettaValue::sym("$x"),
            MettaValue::Long(10),
            MettaValue::SExpr(vec![
                MettaValue::sym("+"),
                MettaValue::sym("$x"),
                MettaValue::Long(1),
            ]),
        ]);
        assert!(can_compile(&nested_let));

        // Chain is compilable
        let chain_expr = MettaValue::SExpr(vec![
            MettaValue::sym("chain"),
            MettaValue::Long(42),
            MettaValue::sym("$x"),
            MettaValue::sym("$x"),
        ]);
        assert!(can_compile(&chain_expr));
    }

    #[test]
    fn test_eval_bytecode_simple() {
        // Test simple arithmetic: (+ 1 2) = 3
        let expr = MettaValue::SExpr(vec![
            MettaValue::sym("+"),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);

        let results = eval_bytecode(&expr).expect("eval should succeed");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(3));
    }

    #[test]
    fn test_eval_bytecode_nested() {
        // Test nested: (+ (* 2 3) 4) = 10
        let expr = MettaValue::SExpr(vec![
            MettaValue::sym("+"),
            MettaValue::SExpr(vec![
                MettaValue::sym("*"),
                MettaValue::Long(2),
                MettaValue::Long(3),
            ]),
            MettaValue::Long(4),
        ]);

        let results = eval_bytecode(&expr).expect("eval should succeed");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Long(10));
    }

    #[test]
    fn test_eval_bytecode_boolean() {
        // Test: (and True False) = False
        let expr = MettaValue::SExpr(vec![
            MettaValue::sym("and"),
            MettaValue::Bool(true),
            MettaValue::Bool(false),
        ]);

        let results = eval_bytecode(&expr).expect("eval should succeed");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Bool(false));
    }

    #[test]
    fn test_eval_bytecode_comparison() {
        // Test: (< 5 10) = True
        let expr = MettaValue::SExpr(vec![
            MettaValue::sym("<"),
            MettaValue::Long(5),
            MettaValue::Long(10),
        ]);

        let results = eval_bytecode(&expr).expect("eval should succeed");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], MettaValue::Bool(true));
    }

    #[test]
    fn test_try_bytecode_eval_success() {
        // When bytecode is enabled and expression is compilable, use bytecode
        let expr = MettaValue::SExpr(vec![
            MettaValue::sym("+"),
            MettaValue::Long(10),
            MettaValue::Long(20),
        ]);

        let results = try_bytecode_eval(&expr, || {
            vec![MettaValue::Long(999)] // Fallback should not be called
        });

        // If bytecode is enabled, result should be 30
        // If bytecode is disabled, fallback returns 999
        #[cfg(feature = "bytecode")]
        assert_eq!(results[0], MettaValue::Long(30));
        #[cfg(not(feature = "bytecode"))]
        assert_eq!(results[0], MettaValue::Long(999));
    }

    #[test]
    fn test_try_bytecode_eval_let_binding() {
        // Let expressions now work in bytecode VM
        // (let $x 1 $x) binds $x to 1 and returns $x (which is 1)
        let expr = MettaValue::SExpr(vec![
            MettaValue::sym("let"),
            MettaValue::sym("$x"),
            MettaValue::Long(1),
            MettaValue::sym("$x"),
        ]);

        let results = try_bytecode_eval(&expr, || {
            vec![MettaValue::Long(42)] // Fallback should NOT be called
        });

        // With bytecode enabled, let bindings work correctly
        #[cfg(feature = "bytecode")]
        assert_eq!(results[0], MettaValue::Long(1));
        #[cfg(not(feature = "bytecode"))]
        assert_eq!(results[0], MettaValue::Long(42));
    }

    #[test]
    fn test_try_bytecode_eval_fallback() {
        // Use 'match' which is not fully compilable to bytecode
        // (match &self pattern result) requires space operations
        let expr = MettaValue::SExpr(vec![
            MettaValue::sym("match"),
            MettaValue::sym("&self"),
            MettaValue::Long(1),
            MettaValue::Long(42),
        ]);

        let results = try_bytecode_eval(&expr, || {
            vec![MettaValue::Long(99)] // Fallback should be called
        });

        // Match requires fallback
        assert_eq!(results[0], MettaValue::Long(99));
    }
}
