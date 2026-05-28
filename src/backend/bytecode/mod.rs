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

pub mod builtin_chunks;
pub mod cache;
pub mod chunk;
pub mod compiler;
pub mod external_registry;
pub(crate) mod instruction;
pub mod memo_cache;
pub mod mork_bridge;
pub mod native_registry;
pub mod opcodes;
pub mod optimizer;
pub mod runtime_profile;
pub mod space_registry;
pub mod tiered_cache;
pub mod vm;

/// Cranelift JIT compilation module
///
/// Provides native code generation for hot bytecode paths.
/// Always enabled - tiered compilation uses this for hot code paths.
pub mod jit;

// Re-export main types
#[cfg(feature = "track-stats")]
pub use cache::get_stats as cache_stats;
pub use cache::{cache_sizes, clear_caches, BytecodeCacheStatsSnapshot};
pub use chunk::{
    BytecodeChunk,
    ChunkBuilder,
    CompiledPattern,
    // Generic chunk types for zero-conversion support
    GenericBytecodeChunk,
    GenericChunkBuilder,
    JumpLabel,
    JumpLabelShort,
    JumpTable,
};
pub use compiler::{compile, compile_arc, CompileContext, CompileError, CompileResult, Compiler};
// Generic compiler for zero-conversion support
pub use compiler::core::{
    compile_bytecode, compile_bytecode_arc, compile_generic, compile_generic_arc, GenericCompiler,
    MettaCompiler,
};
pub use external_registry::{
    ExternalContext,
    ExternalError,
    ExternalFn,
    ExternalRegistry,
    ExternalResult,
    // Generic external registry for zero-conversion support
    GenericExternalContext,
    GenericExternalFn,
    GenericExternalRegistry,
    GenericExternalResult,
};
pub use memo_cache::{
    ensure_memo_cache_roots_registered, global_memo_cache, CacheStats, MemoCache,
};
pub use mork_bridge::{BridgeStats, CompiledRule, MorkBridge};
pub use native_registry::{
    // Generic native registry for zero-conversion support
    GenericNativeContext,
    GenericNativeFn,
    GenericNativeRegistry,
    GenericNativeResult,
    NativeContext,
    NativeError,
    NativeFn,
    NativeRegistry,
    NativeResult,
};
pub use opcodes::Opcode;
pub use optimizer::{
    eliminate_dead_code, optimize_bytecode, optimize_bytecode_full, DceStats, DeadCodeEliminator,
    OptimizationStats, PeepholeAction, PeepholeOptimizer,
};
pub use space_registry::SpaceRegistry;
pub use vm::{
    Alternative,
    BindingFrame,
    BytecodeVM,
    CallFrame,
    ChoicePoint,
    // Generic VM types for zero-conversion support
    GenericAlternative,
    GenericBindingFrame,
    GenericBytecodeVM,
    GenericCallFrame,
    GenericChoicePoint,
    VmConfig,
    VmError,
    VmResult,
};

// JIT re-exports - always available with tiered compilation
pub use jit::{
    CacheEntry,
    ChunkId,
    HybridConfig,
    // Hybrid executor
    HybridExecutor,
    HybridStats,
    JitAlternative,
    JitAlternativeTag,
    JitBailoutReason,
    JitBindingEntry,
    JitBindingFrame,
    // Tiered compilation
    JitCache,
    JitChoicePoint,
    JitContext,
    JitError,
    // JIT profiling
    JitProfile,
    JitResult,
    JitState,
    // JIT types
    JitValue,
    Tier,
    TieredCompiler,
    TieredStats,
    HOT_THRESHOLD,
    JIT_SIGNAL_BAILOUT,
    JIT_SIGNAL_ERROR,
    JIT_SIGNAL_FAIL,
    JIT_SIGNAL_HALT,
    // JIT signals
    JIT_SIGNAL_OK,
    JIT_SIGNAL_YIELD,
    STAGE2_THRESHOLD,
};

// Runtime type profile re-exports
pub use runtime_profile::{
    ArgTypeFeedback, BranchFeedback, GuardFeedback, RuleMatchFeedback, RuntimeTypeProfile, TypeTag,
};

// Unified tiered compilation cache re-exports
pub use tiered_cache::{
    get_slot_compilation_hash,
    global_tiered_cache,
    hash_value,
    increment_and_get_hash,
    // Per-slot atomic counter infrastructure for sub-expression tiering
    increment_exec_count,
    try_sub_expr_dispatch,
    try_sub_expr_dispatch_with_hash,
    ExecutionTier,
    ExprCompilationState,
    NativeCode,
    TierStatusKind,
    TieredCache,
    TieredCacheStats,
    BYTECODE_THRESHOLD,
    FLUSH_INTERVAL,
    JIT1_THRESHOLD,
    JIT2_THRESHOLD,
};

/// Bytecode VM is always enabled with tiered compilation
///
/// The evaluator will attempt to compile and execute expressions via
/// bytecode before falling back to tree-walking. Background compilation
/// via the WorkPool ensures no blocking on first execution.
///
/// # Note
/// This constant is always `true` since tiered compilation is unconditionally
/// enabled. It's kept for backward API compatibility but should not be used
/// for conditional checks (which would be dead code).
#[deprecated(
    since = "0.3.0",
    note = "Bytecode is always enabled; this constant will be removed in a future version"
)]
pub const BYTECODE_ENABLED: bool = true;

/// Global space registry for JIT runtime
///
/// This registry is shared across all JIT executions and provides named space
/// lookup for the `eval-new` and `load-space` operations.
static GLOBAL_SPACE_REGISTRY: std::sync::LazyLock<SpaceRegistry> =
    std::sync::LazyLock::new(SpaceRegistry::new);

/// Get a reference to the global space registry.
///
/// On first access, registers the space registry as a GC root provider so that
/// atoms stored in JIT-created spaces are visible to the garbage collector.
pub fn global_space_registry() -> &'static SpaceRegistry {
    space_registry::ensure_space_registry_roots_registered();
    &GLOBAL_SPACE_REGISTRY
}

use crate::backend::models::{MettaValue, ValueView};

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
    match expr.view() {
        // Always compilable inline literals
        ValueView::Unit
        | ValueView::Bool(_)
        | ValueView::Long(_)
        | ValueView::Float(_)
        | ValueView::Empty
        | ValueView::NotReducible => true,

        ValueView::String(_) => true,

        // Atoms: only variables and known constants are safe
        ValueView::Atom(name) => {
            if name.starts_with('$') {
                return true;
            }
            match name {
                "True" | "False" | "Nil" | "Unit" | "_" => true,
                _ => false,
            }
        }

        // S-expressions - check head AND all operands recursively
        ValueView::SExpr(items) if items.is_empty() => true,
        ValueView::SExpr(items) => {
            if let ValueView::Atom(head) = items[0].view() {
                let head_ok = match head {
                    "+" | "-" | "*" | "/" | "%" | "abs" | "pow" | "pow-math" => true,
                    "<" | "<=" | ">" | ">=" | "==" | "!=" => true,
                    "and" | "or" | "not" | "xor" => true,
                    "if" => true,
                    "quote" => return true,
                    "unquote" => true,
                    "superpose" => return true,
                    "car-atom" | "cdr-atom" | "size-atom" => true,
                    "empty" => true,
                    // cons-atom and decons-atom: HE treats their args as
                    // literal structure (T04/020, T04/061 verify). Route to
                    // T0 always (see `can_compile_with_env` for rationale).
                    "cons-atom" | "decons-atom" => false,
                    "repr" => true,
                    "get-metatype" => true,
                    "let" | "let*" => true,
                    "chain" => return can_compile_chain(items),
                    "map-atom" => return can_compile_map_atom(items),
                    "filter-atom" => return can_compile_filter_atom(items),
                    "foldl-atom" => return can_compile_foldl_atom(items),
                    "case" => true,
                    "error" | "is-error" | "catch" => true,
                    _ => false,
                };

                if !head_ok {
                    return false;
                }

                items.iter().skip(1).all(can_compile)
            } else {
                // PT outer-form-is-data: when items[0] is a sub-SExpr (not an
                // atom), the OUTER form is structural data — the head must be
                // preserved verbatim. The bytecode VM would eagerly evaluate
                // it. Route to T0 trampoline where EvalSExprTail handles it.
                if items[0].as_sexpr().is_some() {
                    return false;
                }
                items.iter().all(can_compile)
            }
        }

        // Errors can be compiled
        ValueView::Error(_, _) => true,

        // Quoted values can be compiled
        ValueView::Quoted(inner) => can_compile(&inner),

        // Types that need environment or special runtime support
        ValueView::Space(_)
        | ValueView::State(_)
        | ValueView::Type(_)
        | ValueView::Conjunction(_)
        | ValueView::Memo(_) => false,

        // PT-canonical Lazy: transparent — delegate to the wrapped value.
        ValueView::Lazy(inner) => can_compile(&inner),
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
    match expr.view() {
        // Always compilable inline literals
        ValueView::Unit
        | ValueView::Bool(_)
        | ValueView::Long(_)
        | ValueView::Float(_)
        | ValueView::Empty
        | ValueView::NotReducible => true,

        ValueView::String(_) => true,

        // Atoms: variables, known constants, AND unknown atoms (for rule dispatch)
        // Atoms: variables ($x), space references (&self), known constants — all compilable.
        // The compiler emits PushAtom for all non-variable atoms; the VM resolves
        // space references at runtime via the environment.
        ValueView::Atom(_name) => true,

        // S-expressions - check head AND all operands recursively
        ValueView::SExpr(items) if items.is_empty() => true,
        ValueView::SExpr(items) => {
            if let ValueView::Atom(head) = items[0].view() {
                let head_ok = match head {
                    // Rule definitions: compiler quotes both LHS and RHS (data form)
                    "=" => true,
                    "!" => true,
                    "+" | "-" | "*" | "/" | "%" | "abs" | "pow" => true,
                    "<" | "<=" | ">" | ">=" | "==" | "!=" => true,
                    "and" | "or" | "not" | "xor" => true,
                    "if" => true,
                    "quote" => return true,
                    "unquote" => true,
                    "superpose" => return true,
                    "car-atom" | "cdr-atom" | "size-atom" | "empty" => true,
                    // cons-atom and decons-atom: HE treats their args as
                    // literal structure (T04/020, T04/061 verify). The T1
                    // ConsAtom/DeconsAtom opcodes both evaluate the args
                    // via `self.compile(...)` before applying, diverging
                    // from HE. Route to T0 instead — sexpr.rs Arm A1 (the
                    // cons-atom / decons-atom dispatch) calls the native
                    // generic op directly on UNREDUCED items, matching
                    // HE byte-for-byte.
                    "cons-atom" | "decons-atom" => false,
                    "repr" => true,
                    "get-metatype" => true,
                    "let" | "let*" => true,
                    "chain" => return can_compile_chain_with_env(items),
                    // Y.2 (2026-05-12): higher-order forms are not handled
                    // by the GenericCompiler (used by `compile_bytecode_arc`).
                    // The kernel-step opcodes Opcode::MapAtom etc. require
                    // template-chunk infrastructure only the IterativeCompiler
                    // emits. Until that path is exposed at the env-aware
                    // entry, gate these out — T0 trampoline handles them
                    // correctly with `[(2 3 4)]` etc.
                    "map-atom" | "filter-atom" | "foldl-atom" => return false,
                    "case" => true,
                    "error" | "is-error" | "catch" => true,
                    "get-type" => true,
                    // if-reducible: native VM handling — compile(expr) + JumpIfIdentical
                    // compares result to original, branches without trampoline.
                    "if-reducible" => true,
                    // if-equal: VM's op_eval_if_equal does alpha-equiv natively.
                    // Returns unevaluated branch; eval_inner trampoline pass reduces it.
                    "if-equal" => true,
                    // match/match-or with &self: native MatchSelf opcode calls
                    // env.match_space() directly. Non-&self stays in tree-walker.
                    // match / match-or: &self uses MatchSelf/MatchSelfOr (already native);
                    // non-&self uses MatchExternal / MatchExternalOr (Phase E).
                    "match" => items.len() >= 4,
                    "match-or" => items.len() >= 5,
                    // Phase A: 4-arg `(unify val1 pattern2 success failure)` compiles
                    // to Unify4. Non-space val1 only — space-val1 falls through to
                    // tree-walker. Compiler gate: items.len() == 5.
                    "unify" => items.len() == 5,
                    // Phase C: `(collapse-bind expr)` compiles to CollapseBindBegin/End.
                    "collapse-bind" => items.len() == 2,
                    "ground-with-bindings" => false,
                    "amb" => false,
                    "sealed" | "atom-subst" => false,
                    "remove-atom" | "get-atoms" => true,
                    "add-atom" => true,
                    // S9 PRAGMA-RET (2026-05-14): `add-reduct`/`add-reducts`/
                    // `add-atoms` are HE stdlib.metta rules whose semantics are
                    // implemented in T0 (sexpr.rs) as compile-time rewrites to
                    // (chain ... (add-atom ...)) / (foldl-atom ...). Route to
                    // T0 explicitly so the bytecode tier doesn't compile them
                    // as user-defined Call opcodes. Compile-time tier-selection
                    // routing — no runtime downgrade.
                    "add-reduct" | "add-reducts" | "add-atoms" => false,
                    // State/space operations: compiler emits opcodes with
                    // VM handlers (op_new_state, op_get_state, etc.)
                    "new-space" | "new-state" | "get-state" | "change-state!" => true,
                    "import!" | "include" | "bind!" | "pragma!" => false,
                    // println! builds S-expr for trampoline; nop emits PushUnit
                    "println!" | "nop" => true,
                    "mod-space!" | "print-mods!" => false,
                    // BUG T0-T1-005 / plan T1.D — MORK forms.
                    //
                    // exec/coalg/lookup/rulify use PathMap query machinery
                    // (env.match_space, RuleIndex::match_rules_native) and
                    // multi-result fan-out that has no measurable performance
                    // benefit from bytecode compilation. Per plan invariant #3,
                    // these are an explicit *compile-time* tier-selection
                    // exception: `can_compile_with_env` returns false here, so
                    // the bytecode dispatch never sees these forms — they
                    // evaluate on T0 from the start. This is compile-time
                    // tier selection, not a runtime down-bail mid-execution,
                    // which keeps the spec K T0-T1-005 row satisfied without
                    // re-implementing ~600 LOC of PathMap-walking bytecode.
                    "exec" | "coalg" | "lookup" | "rulify" => false,
                    // Stage 5a ACT out-of-core (2026-05-27): `save-space!` /
                    // `load-space!` / `query-act` are T0-only special forms
                    // (PathMap ArenaCompactTree dump/mmap + `query_multi_act`
                    // ProductZipper join). They have no bytecode lowering — like
                    // `exec` above, route to the T0 `eval/step/sexpr.rs` arms via
                    // compile-time tier selection so the VM never sees them as
                    // unknown `Call`s (which would push the raw S-expr back).
                    "save-space!" | "load-space!" | "query-act" => false,
                    // Stage 5a LSM-tiered ACT base: `attach-act-base!` /
                    // `detach-act-base!` / `compact-space!` are T0-only special forms
                    // (mmap attach + overlay/tombstone bookkeeping + ACT compaction).
                    // No bytecode lowering — route to T0 via compile-time tier
                    // selection, same as the ACT ops above.
                    "attach-act-base!" | "detach-act-base!" | "compact-space!" => false,
                    // S2 NOEVAL (2026-05-13): `noeval` strips itself and
                    // returns the bare argument unevaluated. Distinct from
                    // `noreduce` (which preserves the wrapper). Bytecode
                    // lacks the "skip evaluation, return raw arg" lowering,
                    // so route to T0 which has the explicit arm in
                    // `eval/step/sexpr.rs`. Similarly `noreduce` returns the
                    // full wrapper unevaluated — also T0-only.
                    "noeval" | "noreduce" => false,
                    // S14b/e (2026-05-14): `format-args` (dyn-fmt semantics)
                    // and `context-space` (current-space accessor) live only
                    // in T0 trampoline. Route from T1 to T0 — compile-time
                    // tier selection, no runtime down-bail.
                    "format-args" | "context-space" => false,
                    // T06/108-111 (MTT-FN-SET-BARE): bare set-op aliases
                    // desugar in T0 (`eval/step/sexpr.rs`) to
                    //   `(let $u (op-atom (collapse arg)…) (superpose $u))`
                    // before any rule dispatch can apply. If the bytecode tier
                    // compiles `(unique X)` as a generic `Call`, the desugar
                    // never runs and the args are evaluated as if `unique` were
                    // an ordinary user-defined function — multi-result inputs
                    // leak out unchanged. Route to T0 explicitly.
                    "unique" | "union" | "intersection" | "subtraction" => false,
                    // T04/117 + T04/050 (2026-05-17): embedded kernel ops
                    // `function`, `return`, `evalc` have no bytecode lowering.
                    // When the bytecode VM's `op_dispatch_rules` encounters
                    // them, finds no matching user rules, and falls through
                    // to the "data constructor / normal form" path
                    // (`vm/mod.rs:7233-7239`) — pushing the raw S-expr back
                    // unchanged. This breaks tree-walker bisimilarity:
                    //   T04/117: `(case (function) ((Error $a $c) caught) ...)`
                    //     bytecode returns `[other]` (case pattern-matches
                    //     against raw `(function)` SExpr, not the Error variant
                    //     the tree-walker `"function"` arity-guard would emit).
                    //   T04/050: `(evalc (bar) &s)` returns `[(evalc (bar) ...)]`
                    //     instead of `[42]` (evalc never dispatches into the
                    //     given space).
                    //   T04/105 (indirect): `(eval (eval 5))` requires top-level
                    //     NotReducible→original conversion which only happens in
                    //     the tree-walker post-eval path.
                    // Compile-time tier selection: route to T0 trampoline
                    // where the special-form arms in `eval/step/sexpr.rs`
                    // produce correct HE-bisim semantics.
                    //
                    // Phase 6 (2026-05-19) cross-cutting case-scrutinee fix:
                    // extended this exclusion arm with testing operations,
                    // assertion ops, and corelib helpers — they have no T1
                    // bytecode lowering, so when compiled-into-bytecode they
                    // hit the VM's data-constructor fallthrough at
                    // `vm/mod.rs:7264-7312` (op_dispatch_rules returns 0
                    // matches → push raw SExpr unchanged). Routing these
                    // heads to T0 restores correct HE-bisim Error emission
                    // from `eval/step/sexpr.rs:3302+` (testing_ops) and
                    // resolves the corelib RHS function/return chain that
                    // executes in T0 context. Same bug-class as the existing
                    // function/return/evalc precedent. Plan agents (2026-05-19)
                    // confirmed root cause is in `can_compile_with_env`, NOT
                    // in `is_normal_form_bounded` — the latter correctly
                    // marks `assertEqual` as reducible.
                    "function" | "return" | "evalc"
                    | "test" | "=alpha"
                    | "assertEqual" | "assertAlphaEqual"
                    | "assertEqualMsg" | "assertAlphaEqualMsg"
                    | "assertEqualToResult" | "assertAlphaEqualToResult"
                    | "assertEqualToResultMsg" | "assertAlphaEqualToResultMsg"
                    | "if-decons-expr" | "if-error" | "return-on-error"
                    | "assertIncludes" | "noreduce-eq"
                    | "trace" | "trace!" | "filter-atom" | "assert"
                    // Phase I (2026-05-20): concurrency + DAS primitives
                    // are special forms implemented natively in T0
                    // (eval/step/sexpr.rs); routing through T1 bytecode
                    // would lose the special-form behavior and treat them
                    // as user-rule Call opcodes. Route to T0 explicitly.
                    | "spawn!" | "await!" | "await-barrier!"
                    | "compare-and-swap-state!" | "loop-until-state"
                    | "new-das!" | "new-distributed-space" | "das-barrier!"
                    | "add-observer!"
                    | "snapshot!" | "partition-space"
                    // Phase 2 (2026-05-26): `(once X)` is a T0-only control
                    // form (StartOnce/ProcessOnceRestore in eval/step/sexpr.rs +
                    // trampoline). It has NO bytecode lowering — the T1 VM would
                    // hit op_dispatch_rules's data-constructor fallthrough and
                    // push the raw `(once …)` SExpr unchanged (the same
                    // function/return/evalc bug class above, observed as
                    // `(collapse (once (xs)))` → `((once 1) 2 3)`). Route any
                    // expression containing `once` to T0 where the cut-barrier
                    // substrate commits X to its first answer. Compile-time tier
                    // selection — not a runtime down-bail.
                    | "once" => false,
                    // User-defined functions: compiled as Call opcodes.
                    // The VM dispatches via op_dispatch_rules → match_rules_native.
                    // eval_inner completes evaluation via trampoline re-eval.
                    _ => true,
                };

                if !head_ok {
                    return false;
                }

                items.iter().skip(1).all(can_compile_with_env)
            } else {
                // PT outer-form-is-data: when items[0] is a sub-SExpr (not an
                // atom), the OUTER form is structural data — the head must be
                // preserved verbatim. The bytecode VM would eagerly evaluate
                // it. Route to T0 trampoline where EvalSExprTail handles it.
                if items[0].as_sexpr().is_some() {
                    return false;
                }
                items.iter().all(can_compile_with_env)
            }
        }

        // Errors can be compiled
        ValueView::Error(_, _) => true,

        // Quoted values can be compiled
        ValueView::Quoted(inner) => can_compile_with_env(&inner),

        // Types that need special runtime support
        ValueView::Space(_)
        | ValueView::State(_)
        | ValueView::Type(_)
        | ValueView::Conjunction(_)
        | ValueView::Memo(_) => false,

        // PT-canonical Lazy: transparent — delegate to the wrapped value.
        ValueView::Lazy(inner) => can_compile_with_env(&inner),
    }
}

/// Check if a chain expression can be compiled with environment support
fn can_compile_chain_with_env(items: &[MettaValue]) -> bool {
    if items.len() != 4 {
        return false;
    }
    let var_ok = matches!(items[2].view(), ValueView::Atom(s) if s.starts_with('$'));
    // T1 `compile_chain` calls `compile(expr)` followed by a single
    // `bind_pattern(var)` StoreLocal — works only when expr is single-valued.
    // For multi-valued expressions (superpose), the bind sees one element
    // per branch but downstream body opcodes (e.g. `(* $v 10)`) do not
    // properly fan out through the binding, yielding the raw superpose
    // values instead of evaluating the body per branch. T04/044 verifies
    // (HE: [10, 20, 30]; T1: [1, 2, 3]; T0 and T2/T3: [10, 20, 30]).
    // Route chain-with-superpose-expr to T0 until T1 chain learns
    // multi-value binding propagation.
    let expr_is_superpose = items[1]
        .as_sexpr()
        .and_then(|i| i.first())
        .and_then(|h| h.as_atom())
        .map(|op| op == "superpose")
        .unwrap_or(false);
    !expr_is_superpose
        && var_ok
        && can_compile_with_env(&items[1])
        && can_compile_with_env(&items[3])
}

// Y.2 (2026-05-12): can_compile_{map,filter,foldl}_atom_with_env predicates
// were removed — the GenericCompiler (used by `compile_bytecode_arc`) lacks
// the template-chunk infrastructure needed to compile these forms correctly.
// The `_with_env` arm at line 478 above now returns `false` directly,
// routing higher-order list ops to the T0 trampoline where they work.

/// Check if a chain expression can be compiled
/// (chain expr $var body) - expr and body must be compilable, $var must be a variable
fn can_compile_chain(items: &[MettaValue]) -> bool {
    if items.len() != 4 {
        return false;
    }
    let var_ok = matches!(items[2].view(), ValueView::Atom(s) if s.starts_with('$'));
    // Same superpose-expr opt-out as `can_compile_chain_with_env` above —
    // see T04/044 rationale there.
    let expr_is_superpose = items[1]
        .as_sexpr()
        .and_then(|i| i.first())
        .and_then(|h| h.as_atom())
        .map(|op| op == "superpose")
        .unwrap_or(false);
    !expr_is_superpose && var_ok && can_compile(&items[1]) && can_compile(&items[3])
}

/// Check if a map-atom expression can be compiled
/// (map-atom list $var template) - list must be compilable, $var must be a variable
fn can_compile_map_atom(items: &[MettaValue]) -> bool {
    if items.len() != 4 {
        return false;
    }
    let var_ok = matches!(items[2].view(), ValueView::Atom(s) if s.starts_with('$'));
    var_ok && can_compile(&items[1])
}

/// Check if a filter-atom expression can be compiled
/// (filter-atom list $var predicate) - list must be compilable, $var must be a variable
fn can_compile_filter_atom(items: &[MettaValue]) -> bool {
    if items.len() != 4 {
        return false;
    }
    let var_ok = matches!(items[2].view(), ValueView::Atom(s) if s.starts_with('$'));
    var_ok && can_compile(&items[1])
}

/// Check if a foldl-atom expression can be compiled
/// (foldl-atom list init $acc $item op) - list and init must be compilable, vars must be variables
fn can_compile_foldl_atom(items: &[MettaValue]) -> bool {
    if items.len() != 6 {
        return false;
    }
    let acc_ok = matches!(items[3].view(), ValueView::Atom(s) if s.starts_with('$'));
    let item_ok = matches!(items[4].view(), ValueView::Atom(s) if s.starts_with('$'));
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
/// let env = Environment::default();
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
    env: GenericEnvironment<MettaValue, GcFactory>,
) -> Result<(Vec<MettaValue>, GenericEnvironment<MettaValue, GcFactory>), BytecodeEvalError> {
    let chunk = compile_arc("eval", expr)?;
    let mut vm = BytecodeVM::with_env(chunk, env);
    let (results, modified_env) = vm.run_with_env()?;
    // Return the environment if present, otherwise create a new one
    let final_env = modified_env.unwrap_or_else(|| GenericEnvironment::new(GcFactory::default()));
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
    env: GenericEnvironment<MettaValue, GcFactory>,
    fallback: F,
) -> (Vec<MettaValue>, GenericEnvironment<MettaValue, GcFactory>)
where
    F: FnOnce(
        GenericEnvironment<MettaValue, GcFactory>,
    ) -> (Vec<MettaValue>, GenericEnvironment<MettaValue, GcFactory>),
{
    if can_compile_with_env(expr) {
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
/// If the expression is compilable, it tries bytecode.
/// On success, returns bytecode results. On failure, calls the fallback.
pub fn try_bytecode_eval<F>(expr: &MettaValue, fallback: F) -> Vec<MettaValue>
where
    F: FnOnce() -> Vec<MettaValue>,
{
    if can_compile(expr) {
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
pub fn try_hybrid_eval<F>(expr: &MettaValue, fallback: F) -> Vec<MettaValue>
where
    F: FnOnce() -> Vec<MettaValue>,
{
    if can_compile(expr) {
        match eval_bytecode_hybrid(expr) {
            Ok(results) => results,
            Err(_) => fallback(),
        }
    } else {
        fallback()
    }
}

// =============================================================================
// Arena Bytecode Evaluation - Zero-Conversion Support
// =============================================================================

use crate::backend::environment::GenericEnvironment;
use crate::backend::models::{GcFactory, MettaValueFactory, MettaValueTrait};

/// Type alias for arena-based environment
pub type MettaEnvironment = GenericEnvironment<MettaValue, GcFactory>;

/// Execute bytecode with arena allocation (zero-conversion).
///
/// This function provides true zero-conversion execution by using
/// `GenericBytecodeVM<MettaValue, GcFactory>`.
/// The entire execution path stays within MettaValue without any conversions.
///
/// # Arguments
/// * `chunk` - The generic bytecode chunk with MettaValue constants
/// * `env` - The arena-based generic environment
///
/// # Returns
/// Tuple of (results, modified_environment) or a VM error.
/// If the VM consumed the environment, a new default environment is created.
///
/// # Example
/// ```ignore
/// use mettatron::backend::bytecode::{execute_arena, GenericBytecodeChunk, MettaEnvironment};
/// use mettatron::backend::eval::trampoline::arena_engine::get_static_factory;
///
/// let factory = get_static_factory();
/// let env = MettaEnvironment::new(factory.clone());
/// let chunk = Arc::new(GenericBytecodeChunk::new("example"));
///
/// let (results, new_env) = execute_arena(chunk, env)?;
/// ```
pub fn execute_arena(
    chunk: std::sync::Arc<GenericBytecodeChunk<MettaValue>>,
    env: MettaEnvironment,
) -> VmResult<(Vec<MettaValue>, MettaEnvironment, bool)> {
    let factory = env.factory().clone();
    let mut vm = GenericBytecodeVM::with_env_and_factory(chunk, env, factory.clone());
    let (results, env_opt) = vm.run_with_env()?;
    let unreduced = vm.unreduced || vm.had_unreduced_result;
    let final_env = env_opt.unwrap_or_else(|| MettaEnvironment::new(factory));
    Ok((results, final_env, unreduced))
}

/// 2026-05-23 PT-canonical binding-thread variant of `execute_arena`.
/// Returns each result paired with its per-result bindings snapshot.
pub fn execute_arena_with_bindings(
    chunk: std::sync::Arc<GenericBytecodeChunk<MettaValue>>,
    env: MettaEnvironment,
) -> VmResult<(
    Vec<(
        MettaValue,
        crate::backend::models::GenericBindings<MettaValue>,
    )>,
    MettaEnvironment,
    bool,
)> {
    let factory = env.factory().clone();
    let mut vm = GenericBytecodeVM::with_env_and_factory(chunk, env, factory.clone());
    let paired = vm.run_with_bindings()?;
    let unreduced = vm.unreduced || vm.had_unreduced_result;
    let final_env = vm
        .env
        .take()
        .unwrap_or_else(|| MettaEnvironment::new(factory));
    Ok((paired, final_env, unreduced))
}

/// Evaluate MettaValue expression with environment threading.
///
/// This is the arena equivalent of `eval_bytecode_with_env()`.
/// It compiles the expression to bytecode, runs it via GenericBytecodeVM,
/// and returns both results AND the modified environment.
///
/// Use this for expressions that need rule dispatch or environment access.
///
/// # Arguments
/// * `expr` - The MettaValue expression to evaluate
/// * `env` - The arena-based environment
///
/// # Returns
/// Tuple of (results, modified_environment) or an error
///
/// # Example
/// ```ignore
/// use mettatron::backend::bytecode::{eval_bytecode_arena_with_env, MettaEnvironment};
/// use mettatron::backend::eval::trampoline::get_static_factory;
///
/// let factory = get_static_factory();
/// let env = MettaEnvironment::new(factory.clone());
/// let expr = /* ... MettaValue expression ... */;
///
/// let (results, new_env) = eval_bytecode_arena_with_env(&expr, env)?;
/// ```
pub fn eval_bytecode_arena_with_env(
    expr: &MettaValue,
    env: MettaEnvironment,
) -> VmResult<(Vec<MettaValue>, MettaEnvironment, bool, bool)> {
    // Compile the expression to bytecode
    let chunk = compile_bytecode_arc("arena_with_env", expr).map_err(|_| VmError::CompileError)?;

    // Execute via GenericBytecodeVM with environment.
    // yield_on_top_return exhausts all nondeterministic alternatives within run().
    let factory = env.factory().clone();
    let mut vm = GenericBytecodeVM::with_env_and_factory(chunk, env, factory.clone());
    vm.yield_on_top_return = true;
    let results = vm.run()?;
    let unreduced = vm.unreduced || vm.had_unreduced_result;
    let has_unexplored_choices = vm.choice_points_len() > 0;

    let final_env = vm
        .env
        .take()
        .unwrap_or_else(|| MettaEnvironment::new(factory));
    Ok((results, final_env, unreduced, has_unexplored_choices))
}

/// Execute bytecode with generic value type and environment.
///
/// This is the fully generic entry point that works with any value type
/// implementing `MettaValueTrait`. It enables zero-conversion evaluation
/// for both heap (`MettaValue`) and arena (`MettaValue`) modes.
///
/// # Type Parameters
/// * `V` - The value type (e.g., `MettaValue` or `MettaValue`)
/// * `F` - The factory type (e.g., `GcFactory`)
///
/// # Arguments
/// * `chunk` - The generic bytecode chunk with V-typed constants
/// * `env` - The generic environment
///
/// # Returns
/// Tuple of (results, modified_environment) or a VM error.
/// If the VM consumed the environment, a new default environment is created.
pub fn execute_generic<V, F>(
    chunk: std::sync::Arc<GenericBytecodeChunk<V>>,
    env: GenericEnvironment<V, F>,
) -> VmResult<(Vec<V>, GenericEnvironment<V, F>)>
where
    V: MettaValueTrait + Clone + Send + Sync + std::marker::Unpin + PartialEq + 'static,
    F: MettaValueFactory<V> + Copy + Clone + Send + Sync + 'static,
{
    let factory = env.factory().clone();
    let mut vm = GenericBytecodeVM::with_env_and_factory(chunk, env, factory.clone());
    let (results, env_opt) = vm.run_with_env()?;
    let final_env = env_opt.unwrap_or_else(|| GenericEnvironment::new(factory));
    Ok((results, final_env))
}

/// Execute bytecode without environment using generic VM.
///
/// This is useful for simple expressions that don't need rule lookup.
///
/// # Type Parameters
/// * `V` - The value type
/// * `F` - The factory type
///
/// # Arguments
/// * `chunk` - The generic bytecode chunk
/// * `factory` - Factory for creating values
///
/// # Returns
/// Results or a VM error
pub fn execute_generic_simple<V, F>(
    chunk: std::sync::Arc<GenericBytecodeChunk<V>>,
    factory: F,
) -> VmResult<Vec<V>>
where
    V: MettaValueTrait + Clone + Send + Sync + std::marker::Unpin + PartialEq + 'static,
    F: MettaValueFactory<V> + Copy + Clone + Send + Sync + 'static,
{
    let mut vm = GenericBytecodeVM::with_factory(chunk, factory);
    vm.run()
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::backend::models::{MettaValue, MettaValueInner, SpaceHandle};

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
        match results[0].inner() {
            MettaValueInner::SExpr(items) => {
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
    #[allow(clippy::approx_constant)]
    fn test_can_compile_literals() {
        // Compilable literals
        assert!(can_compile(&MettaValue::Unit()));
        assert!(can_compile(&MettaValue::Unit()));
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

        let quote_expr = MettaValue::SExpr(vec![MettaValue::sym("quote"), MettaValue::Long(42)]);
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
        let collapse = MettaValue::SExpr(vec![MettaValue::sym("collapse"), MettaValue::Long(42)]);
        assert!(!can_compile(&collapse));
    }

    #[test]
    fn test_can_compile_not_compilable() {
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
        let func_call =
            MettaValue::SExpr(vec![MettaValue::sym("my-function"), MettaValue::Long(1)]);
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

        // Bytecode is always enabled - result should be 30
        assert_eq!(results[0], MettaValue::Long(30));
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

        // Bytecode is always enabled - let bindings work correctly
        assert_eq!(results[0], MettaValue::Long(1));
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
