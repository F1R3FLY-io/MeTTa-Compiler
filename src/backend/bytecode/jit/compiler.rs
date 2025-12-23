//! Bytecode-to-Cranelift JIT Compiler
//!
//! This module translates bytecode chunks into native code using Cranelift.
//! The compilation process:
//!
//! 1. Analyze bytecode for compilability (Stage 1: arithmetic/boolean only)
//! 2. Build Cranelift IR from bytecode opcodes
//! 3. Generate native code via Cranelift JIT module
//! 4. Return function pointer for direct execution

#[cfg(feature = "jit")]
use cranelift::prelude::*;
#[cfg(feature = "jit")]
use cranelift::codegen::ir::BlockArg;
#[cfg(feature = "jit")]
use cranelift_frontend::Switch;
#[cfg(feature = "jit")]
use cranelift_jit::{JITBuilder, JITModule};
#[cfg(feature = "jit")]
use cranelift_module::{FuncId, Linkage, Module};

use super::codegen::CodegenContext;
use super::handlers;
use super::types::{JitError, JitResult, TAG_NIL, TAG_ERROR, TAG_ATOM, TAG_VAR, TAG_HEAP, TAG_BOOL};
use crate::backend::bytecode::{BytecodeChunk, Opcode};
#[cfg(feature = "jit")]
use std::collections::HashMap;
use tracing::trace;

/// JIT Compiler for bytecode chunks
///
/// The compiler maintains a Cranelift JIT module for generating native code.
/// Each compiled function takes a `*mut JitContext` and executes the bytecode
/// logic directly on the context's stack.
pub struct JitCompiler {
    #[cfg(feature = "jit")]
    module: JITModule,

    /// Counter for generating unique function names
    func_counter: u64,

    /// Stage 2: Imported function ID for jit_runtime_pow
    #[cfg(feature = "jit")]
    pow_func_id: FuncId,

    /// Stage 2: Imported function ID for jit_runtime_load_constant
    #[cfg(feature = "jit")]
    load_const_func_id: FuncId,

    /// Stage 13: Imported function ID for jit_runtime_push_empty
    #[cfg(feature = "jit")]
    push_empty_func_id: FuncId,

    /// Stage 14: Imported function ID for jit_runtime_get_head
    #[cfg(feature = "jit")]
    get_head_func_id: FuncId,

    /// Stage 14: Imported function ID for jit_runtime_get_tail
    #[cfg(feature = "jit")]
    get_tail_func_id: FuncId,

    /// Stage 14: Imported function ID for jit_runtime_get_arity
    #[cfg(feature = "jit")]
    get_arity_func_id: FuncId,

    /// Stage 14b: Imported function ID for jit_runtime_get_element
    #[cfg(feature = "jit")]
    get_element_func_id: FuncId,

    /// Phase 1: Imported function ID for jit_runtime_get_type
    #[cfg(feature = "jit")]
    get_type_func_id: FuncId,

    /// Phase 1: Imported function ID for jit_runtime_check_type
    #[cfg(feature = "jit")]
    check_type_func_id: FuncId,

    /// Phase J: Imported function ID for jit_runtime_assert_type
    #[cfg(feature = "jit")]
    assert_type_func_id: FuncId,

    /// Phase 2a: Imported function ID for jit_runtime_make_sexpr
    #[cfg(feature = "jit")]
    make_sexpr_func_id: FuncId,

    /// Phase 2a: Imported function ID for jit_runtime_cons_atom
    #[cfg(feature = "jit")]
    cons_atom_func_id: FuncId,

    /// Phase 2b: Imported function ID for jit_runtime_push_uri (same as load_constant)
    #[cfg(feature = "jit")]
    push_uri_func_id: FuncId,

    /// Phase 2b: Imported function ID for jit_runtime_make_list
    #[cfg(feature = "jit")]
    make_list_func_id: FuncId,

    /// Phase 2b: Imported function ID for jit_runtime_make_quote
    #[cfg(feature = "jit")]
    make_quote_func_id: FuncId,

    /// Phase 3: Imported function ID for jit_runtime_call
    #[cfg(feature = "jit")]
    call_func_id: FuncId,

    /// Phase 3: Imported function ID for jit_runtime_tail_call
    #[cfg(feature = "jit")]
    tail_call_func_id: FuncId,

    /// Phase 1.2: Imported function ID for jit_runtime_call_n (stack-based head)
    #[cfg(feature = "jit")]
    call_n_func_id: FuncId,

    /// Phase 1.2: Imported function ID for jit_runtime_tail_call_n (stack-based head)
    #[cfg(feature = "jit")]
    tail_call_n_func_id: FuncId,

    /// Phase 4: Imported function ID for jit_runtime_fork
    #[cfg(feature = "jit")]
    fork_func_id: FuncId,

    /// Stage 2 JIT: Imported function ID for jit_runtime_fork_native (creates choice points natively)
    #[cfg(feature = "jit")]
    fork_native_func_id: FuncId,

    /// Phase 4: Imported function ID for jit_runtime_yield
    #[cfg(feature = "jit")]
    yield_func_id: FuncId,

    /// Phase 4: Imported function ID for jit_runtime_collect
    #[cfg(feature = "jit")]
    collect_func_id: FuncId,

    /// Stage 2 JIT: Imported function ID for jit_runtime_yield_native (returns signal)
    #[cfg(feature = "jit")]
    yield_native_func_id: FuncId,

    /// Stage 2 JIT: Imported function ID for jit_runtime_collect_native
    #[cfg(feature = "jit")]
    collect_native_func_id: FuncId,

    /// Phase A: Imported function ID for jit_runtime_load_binding
    #[cfg(feature = "jit")]
    load_binding_func_id: FuncId,

    /// Phase A: Imported function ID for jit_runtime_store_binding
    #[cfg(feature = "jit")]
    store_binding_func_id: FuncId,

    /// Phase A: Imported function ID for jit_runtime_has_binding
    #[cfg(feature = "jit")]
    has_binding_func_id: FuncId,

    /// Phase A: Imported function ID for jit_runtime_clear_bindings
    #[cfg(feature = "jit")]
    clear_bindings_func_id: FuncId,

    /// Phase A: Imported function ID for jit_runtime_push_binding_frame
    #[cfg(feature = "jit")]
    push_binding_frame_func_id: FuncId,

    /// Phase A: Imported function ID for jit_runtime_pop_binding_frame
    #[cfg(feature = "jit")]
    pop_binding_frame_func_id: FuncId,

    /// Phase B: Imported function ID for jit_runtime_pattern_match
    #[cfg(feature = "jit")]
    pattern_match_func_id: FuncId,

    /// Phase B: Imported function ID for jit_runtime_pattern_match_bind
    #[cfg(feature = "jit")]
    pattern_match_bind_func_id: FuncId,

    /// Phase B: Imported function ID for jit_runtime_match_arity
    #[cfg(feature = "jit")]
    match_arity_func_id: FuncId,

    /// Phase B: Imported function ID for jit_runtime_match_head
    #[cfg(feature = "jit")]
    match_head_func_id: FuncId,

    /// Phase B: Imported function ID for jit_runtime_unify
    #[cfg(feature = "jit")]
    unify_func_id: FuncId,

    /// Phase B: Imported function ID for jit_runtime_unify_bind
    #[cfg(feature = "jit")]
    unify_bind_func_id: FuncId,

    /// Phase D: Imported function ID for jit_runtime_space_add
    #[cfg(feature = "jit")]
    space_add_func_id: FuncId,

    /// Phase D: Imported function ID for jit_runtime_space_remove
    #[cfg(feature = "jit")]
    space_remove_func_id: FuncId,

    /// Phase D: Imported function ID for jit_runtime_space_get_atoms
    #[cfg(feature = "jit")]
    space_get_atoms_func_id: FuncId,

    /// Phase D: Imported function ID for jit_runtime_space_match
    #[cfg(feature = "jit")]
    space_match_func_id: FuncId,

    /// Phase D.1: Imported function ID for jit_runtime_new_state
    #[cfg(feature = "jit")]
    new_state_func_id: FuncId,

    /// Phase D.1: Imported function ID for jit_runtime_get_state
    #[cfg(feature = "jit")]
    get_state_func_id: FuncId,

    /// Phase D.1: Imported function ID for jit_runtime_change_state
    #[cfg(feature = "jit")]
    change_state_func_id: FuncId,

    /// Phase C: Imported function ID for jit_runtime_dispatch_rules
    #[cfg(feature = "jit")]
    dispatch_rules_func_id: FuncId,

    /// Phase C: Imported function ID for jit_runtime_try_rule
    #[cfg(feature = "jit")]
    try_rule_func_id: FuncId,

    /// Phase C: Imported function ID for jit_runtime_next_rule
    #[cfg(feature = "jit")]
    next_rule_func_id: FuncId,

    /// Phase C: Imported function ID for jit_runtime_commit_rule
    #[cfg(feature = "jit")]
    commit_rule_func_id: FuncId,

    /// Phase C: Imported function ID for jit_runtime_fail_rule
    #[cfg(feature = "jit")]
    fail_rule_func_id: FuncId,

    /// Phase C: Imported function ID for jit_runtime_lookup_rules
    #[cfg(feature = "jit")]
    lookup_rules_func_id: FuncId,

    /// Phase C: Imported function ID for jit_runtime_apply_subst
    #[cfg(feature = "jit")]
    apply_subst_func_id: FuncId,

    /// Phase C: Imported function ID for jit_runtime_define_rule
    #[cfg(feature = "jit")]
    define_rule_func_id: FuncId,

    // Phase E: Special Forms
    /// Phase E: Imported function ID for jit_runtime_eval_if
    #[cfg(feature = "jit")]
    eval_if_func_id: FuncId,

    /// Phase E: Imported function ID for jit_runtime_eval_let
    #[cfg(feature = "jit")]
    eval_let_func_id: FuncId,

    /// Phase E: Imported function ID for jit_runtime_eval_let_star
    #[cfg(feature = "jit")]
    eval_let_star_func_id: FuncId,

    /// Phase E: Imported function ID for jit_runtime_eval_match
    #[cfg(feature = "jit")]
    eval_match_func_id: FuncId,

    /// Phase E: Imported function ID for jit_runtime_eval_case
    #[cfg(feature = "jit")]
    eval_case_func_id: FuncId,

    /// Phase E: Imported function ID for jit_runtime_eval_chain
    #[cfg(feature = "jit")]
    eval_chain_func_id: FuncId,

    /// Phase E: Imported function ID for jit_runtime_eval_quote
    #[cfg(feature = "jit")]
    eval_quote_func_id: FuncId,

    /// Phase E: Imported function ID for jit_runtime_eval_unquote
    #[cfg(feature = "jit")]
    eval_unquote_func_id: FuncId,

    /// Phase E: Imported function ID for jit_runtime_eval_eval
    #[cfg(feature = "jit")]
    eval_eval_func_id: FuncId,

    /// Phase E: Imported function ID for jit_runtime_eval_bind
    #[cfg(feature = "jit")]
    eval_bind_func_id: FuncId,

    /// Phase E: Imported function ID for jit_runtime_eval_new
    #[cfg(feature = "jit")]
    eval_new_func_id: FuncId,

    /// Phase E: Imported function ID for jit_runtime_eval_collapse
    #[cfg(feature = "jit")]
    eval_collapse_func_id: FuncId,

    /// Phase E: Imported function ID for jit_runtime_eval_superpose
    #[cfg(feature = "jit")]
    eval_superpose_func_id: FuncId,

    /// Phase E: Imported function ID for jit_runtime_eval_memo
    #[cfg(feature = "jit")]
    eval_memo_func_id: FuncId,

    /// Phase E: Imported function ID for jit_runtime_eval_memo_first
    #[cfg(feature = "jit")]
    eval_memo_first_func_id: FuncId,

    /// Phase E: Imported function ID for jit_runtime_eval_pragma
    #[cfg(feature = "jit")]
    eval_pragma_func_id: FuncId,

    /// Phase E: Imported function ID for jit_runtime_eval_function
    #[cfg(feature = "jit")]
    eval_function_func_id: FuncId,

    /// Phase E: Imported function ID for jit_runtime_eval_lambda
    #[cfg(feature = "jit")]
    eval_lambda_func_id: FuncId,

    /// Phase E: Imported function ID for jit_runtime_eval_apply
    #[cfg(feature = "jit")]
    eval_apply_func_id: FuncId,

    // Phase G: Advanced Nondeterminism

    /// Phase G: Imported function ID for jit_runtime_cut
    #[cfg(feature = "jit")]
    cut_func_id: FuncId,

    /// Phase G: Imported function ID for jit_runtime_guard
    #[cfg(feature = "jit")]
    guard_func_id: FuncId,

    /// Phase G: Imported function ID for jit_runtime_amb
    #[cfg(feature = "jit")]
    amb_func_id: FuncId,

    /// Phase G: Imported function ID for jit_runtime_commit
    #[cfg(feature = "jit")]
    commit_func_id: FuncId,

    /// Phase G: Imported function ID for jit_runtime_backtrack
    #[cfg(feature = "jit")]
    backtrack_func_id: FuncId,

    // Phase F: Advanced Calls

    /// Phase F: Imported function ID for jit_runtime_call_native
    #[cfg(feature = "jit")]
    call_native_func_id: FuncId,

    /// Phase F: Imported function ID for jit_runtime_call_external
    #[cfg(feature = "jit")]
    call_external_func_id: FuncId,

    /// Phase F: Imported function ID for jit_runtime_call_cached
    #[cfg(feature = "jit")]
    call_cached_func_id: FuncId,

    // Phase H: MORK Bridge

    /// Phase H: Imported function ID for jit_runtime_mork_lookup
    #[cfg(feature = "jit")]
    mork_lookup_func_id: FuncId,

    /// Phase H: Imported function ID for jit_runtime_mork_match
    #[cfg(feature = "jit")]
    mork_match_func_id: FuncId,

    /// Phase H: Imported function ID for jit_runtime_mork_insert
    #[cfg(feature = "jit")]
    mork_insert_func_id: FuncId,

    /// Phase H: Imported function ID for jit_runtime_mork_delete
    #[cfg(feature = "jit")]
    mork_delete_func_id: FuncId,

    // Phase I: Debug/Meta

    /// Phase I: Imported function ID for jit_runtime_trace
    #[cfg(feature = "jit")]
    trace_func_id: FuncId,

    /// Phase I: Imported function ID for jit_runtime_breakpoint
    #[cfg(feature = "jit")]
    breakpoint_func_id: FuncId,

    // Phase 1.1: Core Nondeterminism Markers

    /// Phase 1.1: Imported function ID for jit_runtime_begin_nondet
    #[cfg(feature = "jit")]
    begin_nondet_func_id: FuncId,

    /// Phase 1.1: Imported function ID for jit_runtime_end_nondet
    #[cfg(feature = "jit")]
    end_nondet_func_id: FuncId,

    // Phase 1.3: Multi-value Return

    /// Phase 1.3: Imported function ID for jit_runtime_return_multi
    #[cfg(feature = "jit")]
    return_multi_func_id: FuncId,

    /// Phase 1.3: Imported function ID for jit_runtime_collect_n
    #[cfg(feature = "jit")]
    collect_n_func_id: FuncId,

    // Phase 1.5: Global/Space Access

    /// Phase 1.5: Imported function ID for jit_runtime_load_global
    #[cfg(feature = "jit")]
    load_global_func_id: FuncId,

    /// Phase 1.5: Imported function ID for jit_runtime_store_global
    #[cfg(feature = "jit")]
    store_global_func_id: FuncId,

    /// Phase 1.5: Imported function ID for jit_runtime_load_space
    #[cfg(feature = "jit")]
    load_space_func_id: FuncId,

    // Phase 1.6: Closure Support

    /// Phase 1.6: Imported function ID for jit_runtime_load_upvalue
    #[cfg(feature = "jit")]
    load_upvalue_func_id: FuncId,

    // Phase 1.7: Atom Operations

    /// Phase 1.7: Imported function ID for jit_runtime_decon_atom
    #[cfg(feature = "jit")]
    decon_atom_func_id: FuncId,

    /// Phase 1.7: Imported function ID for jit_runtime_repr
    #[cfg(feature = "jit")]
    repr_func_id: FuncId,

    // Phase 1.8: Higher-Order Operations

    /// Phase 1.8: Imported function ID for jit_runtime_map_atom
    #[cfg(feature = "jit")]
    map_atom_func_id: FuncId,

    /// Phase 1.8: Imported function ID for jit_runtime_filter_atom
    #[cfg(feature = "jit")]
    filter_atom_func_id: FuncId,

    /// Phase 1.8: Imported function ID for jit_runtime_foldl_atom
    #[cfg(feature = "jit")]
    foldl_atom_func_id: FuncId,

    // Phase 1.9: Meta-Type Operations

    /// Phase 1.9: Imported function ID for jit_runtime_get_metatype
    #[cfg(feature = "jit")]
    get_metatype_func_id: FuncId,

    // Phase 1.10: MORK and Debug

    /// Phase 1.10: Imported function ID for jit_runtime_bloom_check
    #[cfg(feature = "jit")]
    bloom_check_func_id: FuncId,

    // Phase 2.0: Extended Math Operations (PR #62)

    /// Imported function ID for jit_runtime_sqrt
    #[cfg(feature = "jit")]
    sqrt_func_id: FuncId,

    /// Imported function ID for jit_runtime_log
    #[cfg(feature = "jit")]
    log_func_id: FuncId,

    /// Imported function ID for jit_runtime_trunc
    #[cfg(feature = "jit")]
    trunc_func_id: FuncId,

    /// Imported function ID for jit_runtime_ceil
    #[cfg(feature = "jit")]
    ceil_func_id: FuncId,

    /// Imported function ID for jit_runtime_floor_math
    #[cfg(feature = "jit")]
    floor_math_func_id: FuncId,

    /// Imported function ID for jit_runtime_round
    #[cfg(feature = "jit")]
    round_func_id: FuncId,

    /// Imported function ID for jit_runtime_sin
    #[cfg(feature = "jit")]
    sin_func_id: FuncId,

    /// Imported function ID for jit_runtime_cos
    #[cfg(feature = "jit")]
    cos_func_id: FuncId,

    /// Imported function ID for jit_runtime_tan
    #[cfg(feature = "jit")]
    tan_func_id: FuncId,

    /// Imported function ID for jit_runtime_asin
    #[cfg(feature = "jit")]
    asin_func_id: FuncId,

    /// Imported function ID for jit_runtime_acos
    #[cfg(feature = "jit")]
    acos_func_id: FuncId,

    /// Imported function ID for jit_runtime_atan
    #[cfg(feature = "jit")]
    atan_func_id: FuncId,

    /// Imported function ID for jit_runtime_isnan
    #[cfg(feature = "jit")]
    isnan_func_id: FuncId,

    /// Imported function ID for jit_runtime_isinf
    #[cfg(feature = "jit")]
    isinf_func_id: FuncId,

    // Phase 2.1: Expression Manipulation Operations (PR #63)

    /// Imported function ID for jit_runtime_index_atom
    #[cfg(feature = "jit")]
    index_atom_func_id: FuncId,

    /// Imported function ID for jit_runtime_min_atom
    #[cfg(feature = "jit")]
    min_atom_func_id: FuncId,

    /// Imported function ID for jit_runtime_max_atom
    #[cfg(feature = "jit")]
    max_atom_func_id: FuncId,
}

/// Block info for JIT compilation - tracks jump targets and predecessor counts
#[cfg(feature = "jit")]
struct BlockInfo {
    /// Bytecode offsets that are jump targets
    targets: Vec<usize>,
    /// Number of predecessors for each target (for PHI detection)
    predecessor_count: HashMap<usize, usize>,
}

impl JitCompiler {
    /// Create a new JIT compiler
    #[cfg(feature = "jit")]
    pub fn new() -> JitResult<Self> {
        let mut flag_builder = settings::builder();
        // Enable optimizations
        flag_builder.set("opt_level", "speed").map_err(|e| {
            JitError::CompilationError(format!("Failed to set opt_level: {}", e))
        })?;
        // Note: SIMD is enabled by default on native ISA builder via cranelift_native::builder()

        let isa_builder = cranelift_native::builder().map_err(|e| {
            JitError::CompilationError(format!("Failed to create ISA builder: {}", e))
        })?;

        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .map_err(|e| JitError::CompilationError(format!("Failed to create ISA: {}", e)))?;

        let mut builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());

        // Register runtime support functions
        Self::register_runtime_symbols(&mut builder);

        let mut module = JITModule::new(builder);

        // Stage 2: Declare imported runtime functions
        // jit_runtime_pow: fn(base: u64, exp: u64) -> u64
        let mut pow_sig = module.make_signature();
        pow_sig.params.push(AbiParam::new(types::I64)); // base (NaN-boxed)
        pow_sig.params.push(AbiParam::new(types::I64)); // exp (NaN-boxed)
        pow_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed)
        let pow_func_id = module
            .declare_function("jit_runtime_pow", Linkage::Import, &pow_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_pow: {}", e))
            })?;

        // jit_runtime_load_constant: fn(ctx: *mut JitContext, index: u64) -> u64
        let mut load_const_sig = module.make_signature();
        load_const_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        load_const_sig.params.push(AbiParam::new(types::I64)); // constant index
        load_const_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed)
        let load_const_func_id = module
            .declare_function("jit_runtime_load_constant", Linkage::Import, &load_const_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_load_constant: {}",
                    e
                ))
            })?;

        // jit_runtime_push_empty: fn() -> u64
        let mut push_empty_sig = module.make_signature();
        push_empty_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed heap ptr)
        let push_empty_func_id = module
            .declare_function("jit_runtime_push_empty", Linkage::Import, &push_empty_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_push_empty: {}",
                    e
                ))
            })?;

        // Stage 14: S-expression operations
        // jit_runtime_get_head: fn(ctx: *mut JitContext, val: u64, ip: u64) -> u64
        let mut get_head_sig = module.make_signature();
        get_head_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        get_head_sig.params.push(AbiParam::new(types::I64)); // val (NaN-boxed)
        get_head_sig.params.push(AbiParam::new(types::I64)); // ip
        get_head_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed)
        let get_head_func_id = module
            .declare_function("jit_runtime_get_head", Linkage::Import, &get_head_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_get_head: {}",
                    e
                ))
            })?;

        // jit_runtime_get_tail: fn(ctx: *mut JitContext, val: u64, ip: u64) -> u64
        let mut get_tail_sig = module.make_signature();
        get_tail_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        get_tail_sig.params.push(AbiParam::new(types::I64)); // val (NaN-boxed)
        get_tail_sig.params.push(AbiParam::new(types::I64)); // ip
        get_tail_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed)
        let get_tail_func_id = module
            .declare_function("jit_runtime_get_tail", Linkage::Import, &get_tail_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_get_tail: {}",
                    e
                ))
            })?;

        // jit_runtime_get_arity: fn(ctx: *mut JitContext, val: u64, ip: u64) -> u64
        let mut get_arity_sig = module.make_signature();
        get_arity_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        get_arity_sig.params.push(AbiParam::new(types::I64)); // val (NaN-boxed)
        get_arity_sig.params.push(AbiParam::new(types::I64)); // ip
        get_arity_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed)
        let get_arity_func_id = module
            .declare_function("jit_runtime_get_arity", Linkage::Import, &get_arity_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_get_arity: {}",
                    e
                ))
            })?;

        // Stage 14b: jit_runtime_get_element: fn(ctx: *mut JitContext, val: u64, index: u64, ip: u64) -> u64
        let mut get_element_sig = module.make_signature();
        get_element_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        get_element_sig.params.push(AbiParam::new(types::I64)); // val (NaN-boxed)
        get_element_sig.params.push(AbiParam::new(types::I64)); // index
        get_element_sig.params.push(AbiParam::new(types::I64)); // ip
        get_element_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed)
        let get_element_func_id = module
            .declare_function("jit_runtime_get_element", Linkage::Import, &get_element_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_get_element: {}",
                    e
                ))
            })?;

        // Phase 1: jit_runtime_get_type: fn(ctx: *mut JitContext, val: u64, ip: u64) -> u64
        let mut get_type_sig = module.make_signature();
        get_type_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        get_type_sig.params.push(AbiParam::new(types::I64)); // val (NaN-boxed)
        get_type_sig.params.push(AbiParam::new(types::I64)); // ip
        get_type_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed atom)
        let get_type_func_id = module
            .declare_function("jit_runtime_get_type", Linkage::Import, &get_type_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_get_type: {}",
                    e
                ))
            })?;

        // Phase 1: jit_runtime_check_type: fn(ctx: *mut JitContext, val: u64, type_atom: u64, ip: u64) -> u64
        let mut check_type_sig = module.make_signature();
        check_type_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        check_type_sig.params.push(AbiParam::new(types::I64)); // val (NaN-boxed)
        check_type_sig.params.push(AbiParam::new(types::I64)); // type_atom (NaN-boxed)
        check_type_sig.params.push(AbiParam::new(types::I64)); // ip
        check_type_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed bool)
        let check_type_func_id = module
            .declare_function("jit_runtime_check_type", Linkage::Import, &check_type_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_check_type: {}",
                    e
                ))
            })?;

        // Phase J: jit_runtime_assert_type: fn(ctx: *mut JitContext, val: u64, type_atom: u64, ip: u64) -> u64
        let mut assert_type_sig = module.make_signature();
        assert_type_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        assert_type_sig.params.push(AbiParam::new(types::I64)); // val (NaN-boxed)
        assert_type_sig.params.push(AbiParam::new(types::I64)); // type_atom (NaN-boxed)
        assert_type_sig.params.push(AbiParam::new(types::I64)); // ip
        assert_type_sig.returns.push(AbiParam::new(types::I64)); // result (original value or bailout)
        let assert_type_func_id = module
            .declare_function("jit_runtime_assert_type", Linkage::Import, &assert_type_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_assert_type: {}",
                    e
                ))
            })?;

        // Phase 2a: jit_runtime_make_sexpr: fn(ctx: *mut JitContext, values_ptr: *const u64, count: u64, ip: u64) -> u64
        let mut make_sexpr_sig = module.make_signature();
        make_sexpr_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        make_sexpr_sig.params.push(AbiParam::new(types::I64)); // values_ptr
        make_sexpr_sig.params.push(AbiParam::new(types::I64)); // count
        make_sexpr_sig.params.push(AbiParam::new(types::I64)); // ip
        make_sexpr_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed heap ptr)
        let make_sexpr_func_id = module
            .declare_function("jit_runtime_make_sexpr", Linkage::Import, &make_sexpr_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_make_sexpr: {}",
                    e
                ))
            })?;

        // Phase 2a: jit_runtime_cons_atom: fn(ctx: *mut JitContext, head: u64, tail: u64, ip: u64) -> u64
        let mut cons_atom_sig = module.make_signature();
        cons_atom_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        cons_atom_sig.params.push(AbiParam::new(types::I64)); // head (NaN-boxed)
        cons_atom_sig.params.push(AbiParam::new(types::I64)); // tail (NaN-boxed)
        cons_atom_sig.params.push(AbiParam::new(types::I64)); // ip
        cons_atom_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed heap ptr)
        let cons_atom_func_id = module
            .declare_function("jit_runtime_cons_atom", Linkage::Import, &cons_atom_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_cons_atom: {}",
                    e
                ))
            })?;

        // Phase 2b: jit_runtime_push_uri: fn(ctx: *const JitContext, index: u64) -> u64
        let mut push_uri_sig = module.make_signature();
        push_uri_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        push_uri_sig.params.push(AbiParam::new(types::I64)); // constant index
        push_uri_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed)
        let push_uri_func_id = module
            .declare_function("jit_runtime_push_uri", Linkage::Import, &push_uri_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_push_uri: {}",
                    e
                ))
            })?;

        // Phase 2b: jit_runtime_make_list: fn(ctx: *mut JitContext, values_ptr: *const u64, count: u64, ip: u64) -> u64
        let mut make_list_sig = module.make_signature();
        make_list_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        make_list_sig.params.push(AbiParam::new(types::I64)); // values_ptr
        make_list_sig.params.push(AbiParam::new(types::I64)); // count
        make_list_sig.params.push(AbiParam::new(types::I64)); // ip
        make_list_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed)
        let make_list_func_id = module
            .declare_function("jit_runtime_make_list", Linkage::Import, &make_list_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_make_list: {}",
                    e
                ))
            })?;

        // Phase 2b: jit_runtime_make_quote: fn(ctx: *mut JitContext, val: u64, ip: u64) -> u64
        let mut make_quote_sig = module.make_signature();
        make_quote_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        make_quote_sig.params.push(AbiParam::new(types::I64)); // val (NaN-boxed)
        make_quote_sig.params.push(AbiParam::new(types::I64)); // ip
        make_quote_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed heap ptr)
        let make_quote_func_id = module
            .declare_function("jit_runtime_make_quote", Linkage::Import, &make_quote_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_make_quote: {}",
                    e
                ))
            })?;

        // Phase 3: Call - fn(ctx, head_ptr, args_ptr, arity, ip) -> result
        let mut call_sig = module.make_signature();
        call_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        call_sig.params.push(AbiParam::new(types::I64)); // head_ptr (pointer to String)
        call_sig.params.push(AbiParam::new(types::I64)); // args_ptr (pointer to u64 array)
        call_sig.params.push(AbiParam::new(types::I64)); // arity
        call_sig.params.push(AbiParam::new(types::I64)); // ip
        call_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed heap ptr)
        let call_func_id = module
            .declare_function("jit_runtime_call", Linkage::Import, &call_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_call: {}",
                    e
                ))
            })?;

        // Phase 3: TailCall - fn(ctx, head_ptr, args_ptr, arity, ip) -> result
        let mut tail_call_sig = module.make_signature();
        tail_call_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        tail_call_sig.params.push(AbiParam::new(types::I64)); // head_ptr (pointer to String)
        tail_call_sig.params.push(AbiParam::new(types::I64)); // args_ptr (pointer to u64 array)
        tail_call_sig.params.push(AbiParam::new(types::I64)); // arity
        tail_call_sig.params.push(AbiParam::new(types::I64)); // ip
        tail_call_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed heap ptr)
        let tail_call_func_id = module
            .declare_function("jit_runtime_tail_call", Linkage::Import, &tail_call_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_tail_call: {}",
                    e
                ))
            })?;

        // Phase 1.2: CallN - fn(ctx, head_val, args_ptr, arity, ip) -> result
        // head_val is NaN-boxed value from stack (not constant pool index)
        let mut call_n_sig = module.make_signature();
        call_n_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        call_n_sig.params.push(AbiParam::new(types::I64)); // head_val (NaN-boxed)
        call_n_sig.params.push(AbiParam::new(types::I64)); // args_ptr (pointer to u64 array)
        call_n_sig.params.push(AbiParam::new(types::I64)); // arity
        call_n_sig.params.push(AbiParam::new(types::I64)); // ip
        call_n_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed heap ptr)
        let call_n_func_id = module
            .declare_function("jit_runtime_call_n", Linkage::Import, &call_n_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_call_n: {}",
                    e
                ))
            })?;

        // Phase 1.2: TailCallN - fn(ctx, head_val, args_ptr, arity, ip) -> result
        // Same signature as CallN but signals TCO to VM
        let mut tail_call_n_sig = module.make_signature();
        tail_call_n_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        tail_call_n_sig.params.push(AbiParam::new(types::I64)); // head_val (NaN-boxed)
        tail_call_n_sig.params.push(AbiParam::new(types::I64)); // args_ptr (pointer to u64 array)
        tail_call_n_sig.params.push(AbiParam::new(types::I64)); // arity
        tail_call_n_sig.params.push(AbiParam::new(types::I64)); // ip
        tail_call_n_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed heap ptr)
        let tail_call_n_func_id = module
            .declare_function("jit_runtime_tail_call_n", Linkage::Import, &tail_call_n_sig)
            .map_err(|e| {
                JitError::CompilationError(format!(
                    "Failed to declare jit_runtime_tail_call_n: {}",
                    e
                ))
            })?;

        // Phase 4: Fork - fn(ctx, count, indices_ptr, ip) -> result
        let mut fork_sig = module.make_signature();
        fork_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        fork_sig.params.push(AbiParam::new(types::I64)); // count
        fork_sig.params.push(AbiParam::new(types::I64)); // indices_ptr
        fork_sig.params.push(AbiParam::new(types::I64)); // ip
        fork_sig.returns.push(AbiParam::new(types::I64)); // result
        let fork_func_id = module
            .declare_function("jit_runtime_fork", Linkage::Import, &fork_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_fork: {}", e))
            })?;

        // Stage 2 JIT: ForkNative - fn(ctx, count, indices_ptr, ip) -> result (same signature)
        // Creates choice points natively without bailing to VM
        let mut fork_native_sig = module.make_signature();
        fork_native_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        fork_native_sig.params.push(AbiParam::new(types::I64)); // count
        fork_native_sig.params.push(AbiParam::new(types::I64)); // indices_ptr
        fork_native_sig.params.push(AbiParam::new(types::I64)); // ip
        fork_native_sig.returns.push(AbiParam::new(types::I64)); // result
        let fork_native_func_id = module
            .declare_function("jit_runtime_fork_native", Linkage::Import, &fork_native_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_fork_native: {}", e))
            })?;

        // Phase 4: Yield - fn(ctx, value, ip) -> result
        let mut yield_sig = module.make_signature();
        yield_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        yield_sig.params.push(AbiParam::new(types::I64)); // value
        yield_sig.params.push(AbiParam::new(types::I64)); // ip
        yield_sig.returns.push(AbiParam::new(types::I64)); // result
        let yield_func_id = module
            .declare_function("jit_runtime_yield", Linkage::Import, &yield_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_yield: {}", e))
            })?;

        // Phase 4: Collect - fn(ctx, chunk_index, ip) -> result
        let mut collect_sig = module.make_signature();
        collect_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        collect_sig.params.push(AbiParam::new(types::I64)); // chunk_index
        collect_sig.params.push(AbiParam::new(types::I64)); // ip
        collect_sig.returns.push(AbiParam::new(types::I64)); // result
        let collect_func_id = module
            .declare_function("jit_runtime_collect", Linkage::Import, &collect_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_collect: {}", e))
            })?;

        // Stage 2 JIT: YieldNative - fn(ctx, value, ip) -> signal (i64)
        let mut yield_native_sig = module.make_signature();
        yield_native_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        yield_native_sig.params.push(AbiParam::new(types::I64)); // value (NaN-boxed)
        yield_native_sig.params.push(AbiParam::new(types::I64)); // ip
        yield_native_sig.returns.push(AbiParam::new(types::I64)); // signal (JIT_SIGNAL_*)
        let yield_native_func_id = module
            .declare_function("jit_runtime_yield_native", Linkage::Import, &yield_native_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_yield_native: {}", e))
            })?;

        // Stage 2 JIT: CollectNative - fn(ctx) -> result (NaN-boxed SExpr)
        let mut collect_native_sig = module.make_signature();
        collect_native_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        collect_native_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed heap ptr)
        let collect_native_func_id = module
            .declare_function("jit_runtime_collect_native", Linkage::Import, &collect_native_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_collect_native: {}", e))
            })?;

        // Phase A: Binding operations
        // jit_runtime_load_binding: fn(ctx, name_idx, ip) -> u64 (NaN-boxed value)
        let mut load_binding_sig = module.make_signature();
        load_binding_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        load_binding_sig.params.push(AbiParam::new(types::I64)); // name_idx
        load_binding_sig.params.push(AbiParam::new(types::I64)); // ip
        load_binding_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed)
        let load_binding_func_id = module
            .declare_function("jit_runtime_load_binding", Linkage::Import, &load_binding_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_load_binding: {}", e))
            })?;

        // jit_runtime_store_binding: fn(ctx, name_idx, value, ip) -> i64 (status)
        let mut store_binding_sig = module.make_signature();
        store_binding_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        store_binding_sig.params.push(AbiParam::new(types::I64)); // name_idx
        store_binding_sig.params.push(AbiParam::new(types::I64)); // value (NaN-boxed)
        store_binding_sig.params.push(AbiParam::new(types::I64)); // ip
        store_binding_sig.returns.push(AbiParam::new(types::I64)); // status
        let store_binding_func_id = module
            .declare_function("jit_runtime_store_binding", Linkage::Import, &store_binding_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_store_binding: {}", e))
            })?;

        // jit_runtime_has_binding: fn(ctx, name_idx) -> u64 (NaN-boxed bool)
        let mut has_binding_sig = module.make_signature();
        has_binding_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        has_binding_sig.params.push(AbiParam::new(types::I64)); // name_idx
        has_binding_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed bool)
        let has_binding_func_id = module
            .declare_function("jit_runtime_has_binding", Linkage::Import, &has_binding_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_has_binding: {}", e))
            })?;

        // jit_runtime_clear_bindings: fn(ctx) - no return
        let mut clear_bindings_sig = module.make_signature();
        clear_bindings_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        let clear_bindings_func_id = module
            .declare_function("jit_runtime_clear_bindings", Linkage::Import, &clear_bindings_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_clear_bindings: {}", e))
            })?;

        // jit_runtime_push_binding_frame: fn(ctx) -> i64 (status)
        let mut push_binding_frame_sig = module.make_signature();
        push_binding_frame_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        push_binding_frame_sig.returns.push(AbiParam::new(types::I64)); // status
        let push_binding_frame_func_id = module
            .declare_function("jit_runtime_push_binding_frame", Linkage::Import, &push_binding_frame_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_push_binding_frame: {}", e))
            })?;

        // jit_runtime_pop_binding_frame: fn(ctx) -> i64 (status)
        let mut pop_binding_frame_sig = module.make_signature();
        pop_binding_frame_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        pop_binding_frame_sig.returns.push(AbiParam::new(types::I64)); // status
        let pop_binding_frame_func_id = module
            .declare_function("jit_runtime_pop_binding_frame", Linkage::Import, &pop_binding_frame_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_pop_binding_frame: {}", e))
            })?;

        // Phase B: Pattern Matching operations
        // jit_runtime_pattern_match: fn(ctx, pattern, value, ip) -> u64 (NaN-boxed bool)
        let mut pattern_match_sig = module.make_signature();
        pattern_match_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        pattern_match_sig.params.push(AbiParam::new(types::I64)); // pattern (NaN-boxed)
        pattern_match_sig.params.push(AbiParam::new(types::I64)); // value (NaN-boxed)
        pattern_match_sig.params.push(AbiParam::new(types::I64)); // ip
        pattern_match_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed bool)
        let pattern_match_func_id = module
            .declare_function("jit_runtime_pattern_match", Linkage::Import, &pattern_match_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_pattern_match: {}", e))
            })?;

        // jit_runtime_pattern_match_bind: fn(ctx, pattern, value, ip) -> u64 (NaN-boxed bool)
        let mut pattern_match_bind_sig = module.make_signature();
        pattern_match_bind_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        pattern_match_bind_sig.params.push(AbiParam::new(types::I64)); // pattern (NaN-boxed)
        pattern_match_bind_sig.params.push(AbiParam::new(types::I64)); // value (NaN-boxed)
        pattern_match_bind_sig.params.push(AbiParam::new(types::I64)); // ip
        pattern_match_bind_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed bool)
        let pattern_match_bind_func_id = module
            .declare_function("jit_runtime_pattern_match_bind", Linkage::Import, &pattern_match_bind_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_pattern_match_bind: {}", e))
            })?;

        // jit_runtime_match_arity: fn(ctx, value, expected_arity, ip) -> u64 (NaN-boxed bool)
        let mut match_arity_sig = module.make_signature();
        match_arity_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        match_arity_sig.params.push(AbiParam::new(types::I64)); // value (NaN-boxed)
        match_arity_sig.params.push(AbiParam::new(types::I64)); // expected_arity
        match_arity_sig.params.push(AbiParam::new(types::I64)); // ip
        match_arity_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed bool)
        let match_arity_func_id = module
            .declare_function("jit_runtime_match_arity", Linkage::Import, &match_arity_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_match_arity: {}", e))
            })?;

        // jit_runtime_match_head: fn(ctx, value, expected_head_idx, ip) -> u64 (NaN-boxed bool)
        let mut match_head_sig = module.make_signature();
        match_head_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        match_head_sig.params.push(AbiParam::new(types::I64)); // value (NaN-boxed)
        match_head_sig.params.push(AbiParam::new(types::I64)); // expected_head_idx
        match_head_sig.params.push(AbiParam::new(types::I64)); // ip
        match_head_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed bool)
        let match_head_func_id = module
            .declare_function("jit_runtime_match_head", Linkage::Import, &match_head_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_match_head: {}", e))
            })?;

        // jit_runtime_unify: fn(ctx, a, b, ip) -> u64 (NaN-boxed bool)
        let mut unify_sig = module.make_signature();
        unify_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        unify_sig.params.push(AbiParam::new(types::I64)); // a (NaN-boxed)
        unify_sig.params.push(AbiParam::new(types::I64)); // b (NaN-boxed)
        unify_sig.params.push(AbiParam::new(types::I64)); // ip
        unify_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed bool)
        let unify_func_id = module
            .declare_function("jit_runtime_unify", Linkage::Import, &unify_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_unify: {}", e))
            })?;

        // jit_runtime_unify_bind: fn(ctx, a, b, ip) -> u64 (NaN-boxed bool)
        let mut unify_bind_sig = module.make_signature();
        unify_bind_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        unify_bind_sig.params.push(AbiParam::new(types::I64)); // a (NaN-boxed)
        unify_bind_sig.params.push(AbiParam::new(types::I64)); // b (NaN-boxed)
        unify_bind_sig.params.push(AbiParam::new(types::I64)); // ip
        unify_bind_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed bool)
        let unify_bind_func_id = module
            .declare_function("jit_runtime_unify_bind", Linkage::Import, &unify_bind_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_unify_bind: {}", e))
            })?;

        // =====================================================================
        // Phase D: Space Operations
        // =====================================================================

        // jit_runtime_space_add: fn(ctx, space, atom, ip) -> u64 (Unit)
        let mut space_add_sig = module.make_signature();
        space_add_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        space_add_sig.params.push(AbiParam::new(types::I64)); // space (NaN-boxed)
        space_add_sig.params.push(AbiParam::new(types::I64)); // atom (NaN-boxed)
        space_add_sig.params.push(AbiParam::new(types::I64)); // ip
        space_add_sig.returns.push(AbiParam::new(types::I64)); // result (Unit)
        let space_add_func_id = module
            .declare_function("jit_runtime_space_add", Linkage::Import, &space_add_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_space_add: {}", e))
            })?;

        // jit_runtime_space_remove: fn(ctx, space, atom, ip) -> u64 (Bool)
        let mut space_remove_sig = module.make_signature();
        space_remove_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        space_remove_sig.params.push(AbiParam::new(types::I64)); // space (NaN-boxed)
        space_remove_sig.params.push(AbiParam::new(types::I64)); // atom (NaN-boxed)
        space_remove_sig.params.push(AbiParam::new(types::I64)); // ip
        space_remove_sig.returns.push(AbiParam::new(types::I64)); // result (Bool)
        let space_remove_func_id = module
            .declare_function("jit_runtime_space_remove", Linkage::Import, &space_remove_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_space_remove: {}", e))
            })?;

        // jit_runtime_space_get_atoms: fn(ctx, space, ip) -> u64 (SExpr)
        let mut space_get_atoms_sig = module.make_signature();
        space_get_atoms_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        space_get_atoms_sig.params.push(AbiParam::new(types::I64)); // space (NaN-boxed)
        space_get_atoms_sig.params.push(AbiParam::new(types::I64)); // ip
        space_get_atoms_sig.returns.push(AbiParam::new(types::I64)); // result (SExpr)
        let space_get_atoms_func_id = module
            .declare_function("jit_runtime_space_get_atoms", Linkage::Import, &space_get_atoms_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_space_get_atoms: {}", e))
            })?;

        // jit_runtime_space_match_nondet: fn(ctx, space, pattern, template, ip) -> u64 (SExpr)
        // Uses nondeterministic semantics with choice points for multiple matches
        let mut space_match_sig = module.make_signature();
        space_match_sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        space_match_sig.params.push(AbiParam::new(types::I64)); // space (NaN-boxed)
        space_match_sig.params.push(AbiParam::new(types::I64)); // pattern (NaN-boxed)
        space_match_sig.params.push(AbiParam::new(types::I64)); // template (NaN-boxed)
        space_match_sig.params.push(AbiParam::new(types::I64)); // ip
        space_match_sig.returns.push(AbiParam::new(types::I64)); // result (SExpr)
        let space_match_func_id = module
            .declare_function("jit_runtime_space_match_nondet", Linkage::Import, &space_match_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_space_match_nondet: {}", e))
            })?;

        // Phase D.1: jit_runtime_new_state(ctx, initial_value, ip) -> state_handle
        let mut new_state_sig = module.make_signature();
        new_state_sig.params.push(AbiParam::new(types::I64)); // ctx
        new_state_sig.params.push(AbiParam::new(types::I64)); // initial_value (NaN-boxed)
        new_state_sig.params.push(AbiParam::new(types::I64)); // ip
        new_state_sig.returns.push(AbiParam::new(types::I64)); // state_handle (NaN-boxed)
        let new_state_func_id = module
            .declare_function("jit_runtime_new_state", Linkage::Import, &new_state_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_new_state: {}", e))
            })?;

        // Phase D.1: jit_runtime_get_state(ctx, state_handle, ip) -> value
        let mut get_state_sig = module.make_signature();
        get_state_sig.params.push(AbiParam::new(types::I64)); // ctx
        get_state_sig.params.push(AbiParam::new(types::I64)); // state_handle (NaN-boxed)
        get_state_sig.params.push(AbiParam::new(types::I64)); // ip
        get_state_sig.returns.push(AbiParam::new(types::I64)); // value (NaN-boxed)
        let get_state_func_id = module
            .declare_function("jit_runtime_get_state", Linkage::Import, &get_state_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_get_state: {}", e))
            })?;

        // Phase D.1: jit_runtime_change_state(ctx, state_handle, new_value, ip) -> state_handle
        let mut change_state_sig = module.make_signature();
        change_state_sig.params.push(AbiParam::new(types::I64)); // ctx
        change_state_sig.params.push(AbiParam::new(types::I64)); // state_handle (NaN-boxed)
        change_state_sig.params.push(AbiParam::new(types::I64)); // new_value (NaN-boxed)
        change_state_sig.params.push(AbiParam::new(types::I64)); // ip
        change_state_sig.returns.push(AbiParam::new(types::I64)); // state_handle (NaN-boxed)
        let change_state_func_id = module
            .declare_function("jit_runtime_change_state", Linkage::Import, &change_state_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_change_state: {}", e))
            })?;

        // Phase C: jit_runtime_dispatch_rules(ctx, expr, ip) -> count
        let mut dispatch_rules_sig = module.make_signature();
        dispatch_rules_sig.params.push(AbiParam::new(types::I64)); // ctx
        dispatch_rules_sig.params.push(AbiParam::new(types::I64)); // expr (NaN-boxed)
        dispatch_rules_sig.params.push(AbiParam::new(types::I64)); // ip
        dispatch_rules_sig.returns.push(AbiParam::new(types::I64)); // count (NaN-boxed)
        let dispatch_rules_func_id = module
            .declare_function("jit_runtime_dispatch_rules", Linkage::Import, &dispatch_rules_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_dispatch_rules: {}", e))
            })?;

        // Phase C: jit_runtime_try_rule(ctx, rule_idx, ip) -> result
        let mut try_rule_sig = module.make_signature();
        try_rule_sig.params.push(AbiParam::new(types::I64)); // ctx
        try_rule_sig.params.push(AbiParam::new(types::I64)); // rule_idx
        try_rule_sig.params.push(AbiParam::new(types::I64)); // ip
        try_rule_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed)
        let try_rule_func_id = module
            .declare_function("jit_runtime_try_rule", Linkage::Import, &try_rule_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_try_rule: {}", e))
            })?;

        // Phase C: jit_runtime_next_rule(ctx, ip) -> status
        let mut next_rule_sig = module.make_signature();
        next_rule_sig.params.push(AbiParam::new(types::I64)); // ctx
        next_rule_sig.params.push(AbiParam::new(types::I64)); // ip
        next_rule_sig.returns.push(AbiParam::new(types::I64)); // status
        let next_rule_func_id = module
            .declare_function("jit_runtime_next_rule", Linkage::Import, &next_rule_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_next_rule: {}", e))
            })?;

        // Phase C: jit_runtime_commit_rule(ctx, ip) -> status
        let mut commit_rule_sig = module.make_signature();
        commit_rule_sig.params.push(AbiParam::new(types::I64)); // ctx
        commit_rule_sig.params.push(AbiParam::new(types::I64)); // ip
        commit_rule_sig.returns.push(AbiParam::new(types::I64)); // status
        let commit_rule_func_id = module
            .declare_function("jit_runtime_commit_rule", Linkage::Import, &commit_rule_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_commit_rule: {}", e))
            })?;

        // Phase C: jit_runtime_fail_rule(ctx, ip) -> signal
        let mut fail_rule_sig = module.make_signature();
        fail_rule_sig.params.push(AbiParam::new(types::I64)); // ctx
        fail_rule_sig.params.push(AbiParam::new(types::I64)); // ip
        fail_rule_sig.returns.push(AbiParam::new(types::I64)); // signal
        let fail_rule_func_id = module
            .declare_function("jit_runtime_fail_rule", Linkage::Import, &fail_rule_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_fail_rule: {}", e))
            })?;

        // Phase C: jit_runtime_lookup_rules(ctx, head_idx, ip) -> count
        let mut lookup_rules_sig = module.make_signature();
        lookup_rules_sig.params.push(AbiParam::new(types::I64)); // ctx
        lookup_rules_sig.params.push(AbiParam::new(types::I64)); // head_idx
        lookup_rules_sig.params.push(AbiParam::new(types::I64)); // ip
        lookup_rules_sig.returns.push(AbiParam::new(types::I64)); // count (NaN-boxed)
        let lookup_rules_func_id = module
            .declare_function("jit_runtime_lookup_rules", Linkage::Import, &lookup_rules_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_lookup_rules: {}", e))
            })?;

        // Phase C: jit_runtime_apply_subst(ctx, expr, ip) -> result
        let mut apply_subst_sig = module.make_signature();
        apply_subst_sig.params.push(AbiParam::new(types::I64)); // ctx
        apply_subst_sig.params.push(AbiParam::new(types::I64)); // expr (NaN-boxed)
        apply_subst_sig.params.push(AbiParam::new(types::I64)); // ip
        apply_subst_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed)
        let apply_subst_func_id = module
            .declare_function("jit_runtime_apply_subst", Linkage::Import, &apply_subst_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_apply_subst: {}", e))
            })?;

        // Phase C: jit_runtime_define_rule(ctx, pattern_idx, ip) -> Unit
        let mut define_rule_sig = module.make_signature();
        define_rule_sig.params.push(AbiParam::new(types::I64)); // ctx
        define_rule_sig.params.push(AbiParam::new(types::I64)); // pattern_idx
        define_rule_sig.params.push(AbiParam::new(types::I64)); // ip
        define_rule_sig.returns.push(AbiParam::new(types::I64)); // result (Unit)
        let define_rule_func_id = module
            .declare_function("jit_runtime_define_rule", Linkage::Import, &define_rule_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_define_rule: {}", e))
            })?;

        // =====================================================================
        // Phase E: Special Forms
        // =====================================================================

        // Phase E: jit_runtime_eval_if(ctx, condition, then_val, else_val, ip) -> result
        let mut eval_if_sig = module.make_signature();
        eval_if_sig.params.push(AbiParam::new(types::I64)); // ctx
        eval_if_sig.params.push(AbiParam::new(types::I64)); // condition
        eval_if_sig.params.push(AbiParam::new(types::I64)); // then_val
        eval_if_sig.params.push(AbiParam::new(types::I64)); // else_val
        eval_if_sig.params.push(AbiParam::new(types::I64)); // ip
        eval_if_sig.returns.push(AbiParam::new(types::I64)); // result
        let eval_if_func_id = module
            .declare_function("jit_runtime_eval_if", Linkage::Import, &eval_if_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_eval_if: {}", e))
            })?;

        // Phase E: jit_runtime_eval_let(ctx, name_idx, value, ip) -> Unit
        let mut eval_let_sig = module.make_signature();
        eval_let_sig.params.push(AbiParam::new(types::I64)); // ctx
        eval_let_sig.params.push(AbiParam::new(types::I64)); // name_idx
        eval_let_sig.params.push(AbiParam::new(types::I64)); // value
        eval_let_sig.params.push(AbiParam::new(types::I64)); // ip
        eval_let_sig.returns.push(AbiParam::new(types::I64)); // result
        let eval_let_func_id = module
            .declare_function("jit_runtime_eval_let", Linkage::Import, &eval_let_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_eval_let: {}", e))
            })?;

        // Phase E: jit_runtime_eval_let_star(ctx, ip) -> Unit
        let mut eval_let_star_sig = module.make_signature();
        eval_let_star_sig.params.push(AbiParam::new(types::I64)); // ctx
        eval_let_star_sig.params.push(AbiParam::new(types::I64)); // ip
        eval_let_star_sig.returns.push(AbiParam::new(types::I64)); // result
        let eval_let_star_func_id = module
            .declare_function("jit_runtime_eval_let_star", Linkage::Import, &eval_let_star_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_eval_let_star: {}", e))
            })?;

        // Phase E: jit_runtime_eval_match(ctx, value, pattern, ip) -> bool
        let mut eval_match_sig = module.make_signature();
        eval_match_sig.params.push(AbiParam::new(types::I64)); // ctx
        eval_match_sig.params.push(AbiParam::new(types::I64)); // value
        eval_match_sig.params.push(AbiParam::new(types::I64)); // pattern
        eval_match_sig.params.push(AbiParam::new(types::I64)); // ip
        eval_match_sig.returns.push(AbiParam::new(types::I64)); // result
        let eval_match_func_id = module
            .declare_function("jit_runtime_eval_match", Linkage::Import, &eval_match_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_eval_match: {}", e))
            })?;

        // Phase E: jit_runtime_eval_case(ctx, value, case_count, ip) -> case_index
        let mut eval_case_sig = module.make_signature();
        eval_case_sig.params.push(AbiParam::new(types::I64)); // ctx
        eval_case_sig.params.push(AbiParam::new(types::I64)); // value
        eval_case_sig.params.push(AbiParam::new(types::I64)); // case_count
        eval_case_sig.params.push(AbiParam::new(types::I64)); // ip
        eval_case_sig.returns.push(AbiParam::new(types::I64)); // result
        let eval_case_func_id = module
            .declare_function("jit_runtime_eval_case", Linkage::Import, &eval_case_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_eval_case: {}", e))
            })?;

        // Phase E: jit_runtime_eval_chain(ctx, first, second, ip) -> second
        let mut eval_chain_sig = module.make_signature();
        eval_chain_sig.params.push(AbiParam::new(types::I64)); // ctx
        eval_chain_sig.params.push(AbiParam::new(types::I64)); // first
        eval_chain_sig.params.push(AbiParam::new(types::I64)); // second
        eval_chain_sig.params.push(AbiParam::new(types::I64)); // ip
        eval_chain_sig.returns.push(AbiParam::new(types::I64)); // result
        let eval_chain_func_id = module
            .declare_function("jit_runtime_eval_chain", Linkage::Import, &eval_chain_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_eval_chain: {}", e))
            })?;

        // Phase E: jit_runtime_eval_quote(ctx, expr, ip) -> quoted
        let mut eval_quote_sig = module.make_signature();
        eval_quote_sig.params.push(AbiParam::new(types::I64)); // ctx
        eval_quote_sig.params.push(AbiParam::new(types::I64)); // expr
        eval_quote_sig.params.push(AbiParam::new(types::I64)); // ip
        eval_quote_sig.returns.push(AbiParam::new(types::I64)); // result
        let eval_quote_func_id = module
            .declare_function("jit_runtime_eval_quote", Linkage::Import, &eval_quote_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_eval_quote: {}", e))
            })?;

        // Phase E: jit_runtime_eval_unquote(ctx, expr, ip) -> result
        let mut eval_unquote_sig = module.make_signature();
        eval_unquote_sig.params.push(AbiParam::new(types::I64)); // ctx
        eval_unquote_sig.params.push(AbiParam::new(types::I64)); // expr
        eval_unquote_sig.params.push(AbiParam::new(types::I64)); // ip
        eval_unquote_sig.returns.push(AbiParam::new(types::I64)); // result
        let eval_unquote_func_id = module
            .declare_function("jit_runtime_eval_unquote", Linkage::Import, &eval_unquote_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_eval_unquote: {}", e))
            })?;

        // Phase E: jit_runtime_eval_eval(ctx, expr, ip) -> result
        let mut eval_eval_sig = module.make_signature();
        eval_eval_sig.params.push(AbiParam::new(types::I64)); // ctx
        eval_eval_sig.params.push(AbiParam::new(types::I64)); // expr
        eval_eval_sig.params.push(AbiParam::new(types::I64)); // ip
        eval_eval_sig.returns.push(AbiParam::new(types::I64)); // result
        let eval_eval_func_id = module
            .declare_function("jit_runtime_eval_eval", Linkage::Import, &eval_eval_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_eval_eval: {}", e))
            })?;

        // Phase E: jit_runtime_eval_bind(ctx, name_idx, value, ip) -> Unit
        let mut eval_bind_sig = module.make_signature();
        eval_bind_sig.params.push(AbiParam::new(types::I64)); // ctx
        eval_bind_sig.params.push(AbiParam::new(types::I64)); // name_idx
        eval_bind_sig.params.push(AbiParam::new(types::I64)); // value
        eval_bind_sig.params.push(AbiParam::new(types::I64)); // ip
        eval_bind_sig.returns.push(AbiParam::new(types::I64)); // result
        let eval_bind_func_id = module
            .declare_function("jit_runtime_eval_bind", Linkage::Import, &eval_bind_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_eval_bind: {}", e))
            })?;

        // Phase E: jit_runtime_eval_new(ctx, ip) -> space
        let mut eval_new_sig = module.make_signature();
        eval_new_sig.params.push(AbiParam::new(types::I64)); // ctx
        eval_new_sig.params.push(AbiParam::new(types::I64)); // ip
        eval_new_sig.returns.push(AbiParam::new(types::I64)); // result
        let eval_new_func_id = module
            .declare_function("jit_runtime_eval_new", Linkage::Import, &eval_new_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_eval_new: {}", e))
            })?;

        // Phase E: jit_runtime_eval_collapse(ctx, expr, ip) -> list
        let mut eval_collapse_sig = module.make_signature();
        eval_collapse_sig.params.push(AbiParam::new(types::I64)); // ctx
        eval_collapse_sig.params.push(AbiParam::new(types::I64)); // expr
        eval_collapse_sig.params.push(AbiParam::new(types::I64)); // ip
        eval_collapse_sig.returns.push(AbiParam::new(types::I64)); // result
        let eval_collapse_func_id = module
            .declare_function("jit_runtime_eval_collapse", Linkage::Import, &eval_collapse_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_eval_collapse: {}", e))
            })?;

        // Phase E: jit_runtime_eval_superpose(ctx, list, ip) -> choice
        let mut eval_superpose_sig = module.make_signature();
        eval_superpose_sig.params.push(AbiParam::new(types::I64)); // ctx
        eval_superpose_sig.params.push(AbiParam::new(types::I64)); // list
        eval_superpose_sig.params.push(AbiParam::new(types::I64)); // ip
        eval_superpose_sig.returns.push(AbiParam::new(types::I64)); // result
        let eval_superpose_func_id = module
            .declare_function("jit_runtime_eval_superpose", Linkage::Import, &eval_superpose_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_eval_superpose: {}", e))
            })?;

        // Phase E: jit_runtime_eval_memo(ctx, expr, ip) -> result
        let mut eval_memo_sig = module.make_signature();
        eval_memo_sig.params.push(AbiParam::new(types::I64)); // ctx
        eval_memo_sig.params.push(AbiParam::new(types::I64)); // expr
        eval_memo_sig.params.push(AbiParam::new(types::I64)); // ip
        eval_memo_sig.returns.push(AbiParam::new(types::I64)); // result
        let eval_memo_func_id = module
            .declare_function("jit_runtime_eval_memo", Linkage::Import, &eval_memo_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_eval_memo: {}", e))
            })?;

        // Phase E: jit_runtime_eval_memo_first(ctx, expr, ip) -> result
        let mut eval_memo_first_sig = module.make_signature();
        eval_memo_first_sig.params.push(AbiParam::new(types::I64)); // ctx
        eval_memo_first_sig.params.push(AbiParam::new(types::I64)); // expr
        eval_memo_first_sig.params.push(AbiParam::new(types::I64)); // ip
        eval_memo_first_sig.returns.push(AbiParam::new(types::I64)); // result
        let eval_memo_first_func_id = module
            .declare_function("jit_runtime_eval_memo_first", Linkage::Import, &eval_memo_first_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_eval_memo_first: {}", e))
            })?;

        // Phase E: jit_runtime_eval_pragma(ctx, directive, ip) -> Unit
        let mut eval_pragma_sig = module.make_signature();
        eval_pragma_sig.params.push(AbiParam::new(types::I64)); // ctx
        eval_pragma_sig.params.push(AbiParam::new(types::I64)); // directive
        eval_pragma_sig.params.push(AbiParam::new(types::I64)); // ip
        eval_pragma_sig.returns.push(AbiParam::new(types::I64)); // result
        let eval_pragma_func_id = module
            .declare_function("jit_runtime_eval_pragma", Linkage::Import, &eval_pragma_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_eval_pragma: {}", e))
            })?;

        // Phase E: jit_runtime_eval_function(ctx, name_idx, param_count, ip) -> Unit
        let mut eval_function_sig = module.make_signature();
        eval_function_sig.params.push(AbiParam::new(types::I64)); // ctx
        eval_function_sig.params.push(AbiParam::new(types::I64)); // name_idx
        eval_function_sig.params.push(AbiParam::new(types::I64)); // param_count
        eval_function_sig.params.push(AbiParam::new(types::I64)); // ip
        eval_function_sig.returns.push(AbiParam::new(types::I64)); // result
        let eval_function_func_id = module
            .declare_function("jit_runtime_eval_function", Linkage::Import, &eval_function_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_eval_function: {}", e))
            })?;

        // Phase E: jit_runtime_eval_lambda(ctx, param_count, ip) -> closure
        let mut eval_lambda_sig = module.make_signature();
        eval_lambda_sig.params.push(AbiParam::new(types::I64)); // ctx
        eval_lambda_sig.params.push(AbiParam::new(types::I64)); // param_count
        eval_lambda_sig.params.push(AbiParam::new(types::I64)); // ip
        eval_lambda_sig.returns.push(AbiParam::new(types::I64)); // result
        let eval_lambda_func_id = module
            .declare_function("jit_runtime_eval_lambda", Linkage::Import, &eval_lambda_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_eval_lambda: {}", e))
            })?;

        // Phase E: jit_runtime_eval_apply(ctx, closure, arg_count, ip) -> result
        let mut eval_apply_sig = module.make_signature();
        eval_apply_sig.params.push(AbiParam::new(types::I64)); // ctx
        eval_apply_sig.params.push(AbiParam::new(types::I64)); // closure
        eval_apply_sig.params.push(AbiParam::new(types::I64)); // arg_count
        eval_apply_sig.params.push(AbiParam::new(types::I64)); // ip
        eval_apply_sig.returns.push(AbiParam::new(types::I64)); // result
        let eval_apply_func_id = module
            .declare_function("jit_runtime_eval_apply", Linkage::Import, &eval_apply_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_eval_apply: {}", e))
            })?;

        // Phase G: jit_runtime_cut(ctx, ip) -> result
        let mut cut_sig = module.make_signature();
        cut_sig.params.push(AbiParam::new(types::I64)); // ctx
        cut_sig.params.push(AbiParam::new(types::I64)); // ip
        cut_sig.returns.push(AbiParam::new(types::I64)); // result
        let cut_func_id = module
            .declare_function("jit_runtime_cut", Linkage::Import, &cut_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_cut: {}", e))
            })?;

        // Phase G: jit_runtime_guard(ctx, condition, ip) -> i64
        let mut guard_sig = module.make_signature();
        guard_sig.params.push(AbiParam::new(types::I64)); // ctx
        guard_sig.params.push(AbiParam::new(types::I64)); // condition
        guard_sig.params.push(AbiParam::new(types::I64)); // ip
        guard_sig.returns.push(AbiParam::new(types::I64)); // 1=proceed, 0=backtrack
        let guard_func_id = module
            .declare_function("jit_runtime_guard", Linkage::Import, &guard_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_guard: {}", e))
            })?;

        // Phase G: jit_runtime_amb(ctx, alt_count, ip) -> u64
        let mut amb_sig = module.make_signature();
        amb_sig.params.push(AbiParam::new(types::I64)); // ctx
        amb_sig.params.push(AbiParam::new(types::I64)); // alt_count
        amb_sig.params.push(AbiParam::new(types::I64)); // ip
        amb_sig.returns.push(AbiParam::new(types::I64)); // result
        let amb_func_id = module
            .declare_function("jit_runtime_amb", Linkage::Import, &amb_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_amb: {}", e))
            })?;

        // Phase G: jit_runtime_commit(ctx, count, ip) -> u64
        let mut commit_sig = module.make_signature();
        commit_sig.params.push(AbiParam::new(types::I64)); // ctx
        commit_sig.params.push(AbiParam::new(types::I64)); // count
        commit_sig.params.push(AbiParam::new(types::I64)); // ip
        commit_sig.returns.push(AbiParam::new(types::I64)); // result
        let commit_func_id = module
            .declare_function("jit_runtime_commit", Linkage::Import, &commit_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_commit: {}", e))
            })?;

        // Phase G: jit_runtime_backtrack(ctx, ip) -> i64
        let mut backtrack_sig = module.make_signature();
        backtrack_sig.params.push(AbiParam::new(types::I64)); // ctx
        backtrack_sig.params.push(AbiParam::new(types::I64)); // ip
        backtrack_sig.returns.push(AbiParam::new(types::I64)); // signal
        let backtrack_func_id = module
            .declare_function("jit_runtime_backtrack", Linkage::Import, &backtrack_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_backtrack: {}", e))
            })?;

        // Phase F: jit_runtime_call_native(ctx, func_id, arg_count, ip) -> u64
        let mut call_native_sig = module.make_signature();
        call_native_sig.params.push(AbiParam::new(types::I64)); // ctx
        call_native_sig.params.push(AbiParam::new(types::I64)); // func_id
        call_native_sig.params.push(AbiParam::new(types::I64)); // arg_count
        call_native_sig.params.push(AbiParam::new(types::I64)); // ip
        call_native_sig.returns.push(AbiParam::new(types::I64)); // result
        let call_native_func_id = module
            .declare_function("jit_runtime_call_native", Linkage::Import, &call_native_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_call_native: {}", e))
            })?;

        // Phase F: jit_runtime_call_external(ctx, name_idx, arg_count, ip) -> u64
        let mut call_external_sig = module.make_signature();
        call_external_sig.params.push(AbiParam::new(types::I64)); // ctx
        call_external_sig.params.push(AbiParam::new(types::I64)); // name_idx
        call_external_sig.params.push(AbiParam::new(types::I64)); // arg_count
        call_external_sig.params.push(AbiParam::new(types::I64)); // ip
        call_external_sig.returns.push(AbiParam::new(types::I64)); // result
        let call_external_func_id = module
            .declare_function("jit_runtime_call_external", Linkage::Import, &call_external_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_call_external: {}", e))
            })?;

        // Phase F: jit_runtime_call_cached(ctx, head_idx, arg_count, ip) -> u64
        let mut call_cached_sig = module.make_signature();
        call_cached_sig.params.push(AbiParam::new(types::I64)); // ctx
        call_cached_sig.params.push(AbiParam::new(types::I64)); // head_idx
        call_cached_sig.params.push(AbiParam::new(types::I64)); // arg_count
        call_cached_sig.params.push(AbiParam::new(types::I64)); // ip
        call_cached_sig.returns.push(AbiParam::new(types::I64)); // result
        let call_cached_func_id = module
            .declare_function("jit_runtime_call_cached", Linkage::Import, &call_cached_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_call_cached: {}", e))
            })?;

        // Phase H: jit_runtime_mork_lookup(ctx, path, ip) -> result
        let mut mork_lookup_sig = module.make_signature();
        mork_lookup_sig.params.push(AbiParam::new(types::I64)); // ctx
        mork_lookup_sig.params.push(AbiParam::new(types::I64)); // path
        mork_lookup_sig.params.push(AbiParam::new(types::I64)); // ip
        mork_lookup_sig.returns.push(AbiParam::new(types::I64)); // result
        let mork_lookup_func_id = module
            .declare_function("jit_runtime_mork_lookup", Linkage::Import, &mork_lookup_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_mork_lookup: {}", e))
            })?;

        // Phase H: jit_runtime_mork_match(ctx, path, pattern, ip) -> result
        let mut mork_match_sig = module.make_signature();
        mork_match_sig.params.push(AbiParam::new(types::I64)); // ctx
        mork_match_sig.params.push(AbiParam::new(types::I64)); // path
        mork_match_sig.params.push(AbiParam::new(types::I64)); // pattern
        mork_match_sig.params.push(AbiParam::new(types::I64)); // ip
        mork_match_sig.returns.push(AbiParam::new(types::I64)); // result
        let mork_match_func_id = module
            .declare_function("jit_runtime_mork_match", Linkage::Import, &mork_match_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_mork_match: {}", e))
            })?;

        // Phase H: jit_runtime_mork_insert(ctx, path, value, ip) -> result
        let mut mork_insert_sig = module.make_signature();
        mork_insert_sig.params.push(AbiParam::new(types::I64)); // ctx
        mork_insert_sig.params.push(AbiParam::new(types::I64)); // path
        mork_insert_sig.params.push(AbiParam::new(types::I64)); // value
        mork_insert_sig.params.push(AbiParam::new(types::I64)); // ip
        mork_insert_sig.returns.push(AbiParam::new(types::I64)); // result
        let mork_insert_func_id = module
            .declare_function("jit_runtime_mork_insert", Linkage::Import, &mork_insert_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_mork_insert: {}", e))
            })?;

        // Phase H: jit_runtime_mork_delete(ctx, path, ip) -> result
        let mut mork_delete_sig = module.make_signature();
        mork_delete_sig.params.push(AbiParam::new(types::I64)); // ctx
        mork_delete_sig.params.push(AbiParam::new(types::I64)); // path
        mork_delete_sig.params.push(AbiParam::new(types::I64)); // ip
        mork_delete_sig.returns.push(AbiParam::new(types::I64)); // result
        let mork_delete_func_id = module
            .declare_function("jit_runtime_mork_delete", Linkage::Import, &mork_delete_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_mork_delete: {}", e))
            })?;

        // Phase I: jit_runtime_trace(ctx, msg_idx, value, ip) -> void (no return)
        let mut trace_sig = module.make_signature();
        trace_sig.params.push(AbiParam::new(types::I64)); // ctx
        trace_sig.params.push(AbiParam::new(types::I64)); // msg_idx
        trace_sig.params.push(AbiParam::new(types::I64)); // value
        trace_sig.params.push(AbiParam::new(types::I64)); // ip
        // No return value for trace
        let trace_func_id = module
            .declare_function("jit_runtime_trace", Linkage::Import, &trace_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_trace: {}", e))
            })?;

        // Phase I: jit_runtime_breakpoint(ctx, bp_id, ip) -> i64
        let mut breakpoint_sig = module.make_signature();
        breakpoint_sig.params.push(AbiParam::new(types::I64)); // ctx
        breakpoint_sig.params.push(AbiParam::new(types::I64)); // bp_id
        breakpoint_sig.params.push(AbiParam::new(types::I64)); // ip
        breakpoint_sig.returns.push(AbiParam::new(types::I64)); // -1=pause, 0=continue
        let breakpoint_func_id = module
            .declare_function("jit_runtime_breakpoint", Linkage::Import, &breakpoint_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_breakpoint: {}", e))
            })?;

        // Phase 1.1: jit_runtime_begin_nondet(ctx, ip) -> void (no return)
        let mut begin_nondet_sig = module.make_signature();
        begin_nondet_sig.params.push(AbiParam::new(types::I64)); // ctx
        begin_nondet_sig.params.push(AbiParam::new(types::I64)); // ip
        // No return value
        let begin_nondet_func_id = module
            .declare_function("jit_runtime_begin_nondet", Linkage::Import, &begin_nondet_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_begin_nondet: {}", e))
            })?;

        // Phase 1.1: jit_runtime_end_nondet(ctx, ip) -> void (no return)
        let mut end_nondet_sig = module.make_signature();
        end_nondet_sig.params.push(AbiParam::new(types::I64)); // ctx
        end_nondet_sig.params.push(AbiParam::new(types::I64)); // ip
        // No return value
        let end_nondet_func_id = module
            .declare_function("jit_runtime_end_nondet", Linkage::Import, &end_nondet_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_end_nondet: {}", e))
            })?;

        // Phase 1.3: jit_runtime_return_multi(ctx, count, ip) -> signal
        let mut return_multi_sig = module.make_signature();
        return_multi_sig.params.push(AbiParam::new(types::I64)); // ctx
        return_multi_sig.params.push(AbiParam::new(types::I64)); // count
        return_multi_sig.params.push(AbiParam::new(types::I64)); // ip
        return_multi_sig.returns.push(AbiParam::new(types::I64)); // signal
        let return_multi_func_id = module
            .declare_function("jit_runtime_return_multi", Linkage::Import, &return_multi_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_return_multi: {}", e))
            })?;

        // Phase 1.3: jit_runtime_collect_n(ctx, max_count, ip) -> NaN-boxed SExpr
        let mut collect_n_sig = module.make_signature();
        collect_n_sig.params.push(AbiParam::new(types::I64)); // ctx
        collect_n_sig.params.push(AbiParam::new(types::I64)); // max_count
        collect_n_sig.params.push(AbiParam::new(types::I64)); // ip
        collect_n_sig.returns.push(AbiParam::new(types::I64)); // NaN-boxed result
        let collect_n_func_id = module
            .declare_function("jit_runtime_collect_n", Linkage::Import, &collect_n_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_collect_n: {}", e))
            })?;

        // Phase 1.5: jit_runtime_load_global(ctx, symbol_idx, ip) -> NaN-boxed value
        let mut load_global_sig = module.make_signature();
        load_global_sig.params.push(AbiParam::new(types::I64)); // ctx
        load_global_sig.params.push(AbiParam::new(types::I64)); // symbol_idx
        load_global_sig.params.push(AbiParam::new(types::I64)); // ip
        load_global_sig.returns.push(AbiParam::new(types::I64)); // NaN-boxed value
        let load_global_func_id = module
            .declare_function("jit_runtime_load_global", Linkage::Import, &load_global_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_load_global: {}", e))
            })?;

        // Phase 1.5: jit_runtime_store_global(ctx, symbol_idx, value, ip) -> NaN-boxed unit
        let mut store_global_sig = module.make_signature();
        store_global_sig.params.push(AbiParam::new(types::I64)); // ctx
        store_global_sig.params.push(AbiParam::new(types::I64)); // symbol_idx
        store_global_sig.params.push(AbiParam::new(types::I64)); // value
        store_global_sig.params.push(AbiParam::new(types::I64)); // ip
        store_global_sig.returns.push(AbiParam::new(types::I64)); // unit
        let store_global_func_id = module
            .declare_function("jit_runtime_store_global", Linkage::Import, &store_global_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_store_global: {}", e))
            })?;

        // Phase 1.5: jit_runtime_load_space(ctx, name_idx, ip) -> NaN-boxed space handle
        let mut load_space_sig = module.make_signature();
        load_space_sig.params.push(AbiParam::new(types::I64)); // ctx
        load_space_sig.params.push(AbiParam::new(types::I64)); // name_idx
        load_space_sig.params.push(AbiParam::new(types::I64)); // ip
        load_space_sig.returns.push(AbiParam::new(types::I64)); // space handle
        let load_space_func_id = module
            .declare_function("jit_runtime_load_space", Linkage::Import, &load_space_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_load_space: {}", e))
            })?;

        // Phase 1.6: jit_runtime_load_upvalue(ctx, depth, index, ip) -> NaN-boxed value
        let mut load_upvalue_sig = module.make_signature();
        load_upvalue_sig.params.push(AbiParam::new(types::I64)); // ctx
        load_upvalue_sig.params.push(AbiParam::new(types::I64)); // depth
        load_upvalue_sig.params.push(AbiParam::new(types::I64)); // index
        load_upvalue_sig.params.push(AbiParam::new(types::I64)); // ip
        load_upvalue_sig.returns.push(AbiParam::new(types::I64)); // NaN-boxed value
        let load_upvalue_func_id = module
            .declare_function("jit_runtime_load_upvalue", Linkage::Import, &load_upvalue_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_load_upvalue: {}", e))
            })?;

        // Phase 1.7: jit_runtime_decon_atom(ctx, value, ip) -> NaN-boxed (head, tail) pair
        let mut decon_atom_sig = module.make_signature();
        decon_atom_sig.params.push(AbiParam::new(types::I64)); // ctx
        decon_atom_sig.params.push(AbiParam::new(types::I64)); // value
        decon_atom_sig.params.push(AbiParam::new(types::I64)); // ip
        decon_atom_sig.returns.push(AbiParam::new(types::I64)); // NaN-boxed result
        let decon_atom_func_id = module
            .declare_function("jit_runtime_decon_atom", Linkage::Import, &decon_atom_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_decon_atom: {}", e))
            })?;

        // Phase 1.7: jit_runtime_repr(ctx, value, ip) -> NaN-boxed string
        let mut repr_sig = module.make_signature();
        repr_sig.params.push(AbiParam::new(types::I64)); // ctx
        repr_sig.params.push(AbiParam::new(types::I64)); // value
        repr_sig.params.push(AbiParam::new(types::I64)); // ip
        repr_sig.returns.push(AbiParam::new(types::I64)); // NaN-boxed string
        let repr_func_id = module
            .declare_function("jit_runtime_repr", Linkage::Import, &repr_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_repr: {}", e))
            })?;

        // Phase 1.8: jit_runtime_map_atom(ctx, list, func_chunk, ip) -> NaN-boxed result
        let mut map_atom_sig = module.make_signature();
        map_atom_sig.params.push(AbiParam::new(types::I64)); // ctx
        map_atom_sig.params.push(AbiParam::new(types::I64)); // list
        map_atom_sig.params.push(AbiParam::new(types::I64)); // func_chunk
        map_atom_sig.params.push(AbiParam::new(types::I64)); // ip
        map_atom_sig.returns.push(AbiParam::new(types::I64)); // NaN-boxed result
        let map_atom_func_id = module
            .declare_function("jit_runtime_map_atom", Linkage::Import, &map_atom_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_map_atom: {}", e))
            })?;

        // Phase 1.8: jit_runtime_filter_atom(ctx, list, predicate_chunk, ip) -> NaN-boxed result
        let mut filter_atom_sig = module.make_signature();
        filter_atom_sig.params.push(AbiParam::new(types::I64)); // ctx
        filter_atom_sig.params.push(AbiParam::new(types::I64)); // list
        filter_atom_sig.params.push(AbiParam::new(types::I64)); // predicate_chunk
        filter_atom_sig.params.push(AbiParam::new(types::I64)); // ip
        filter_atom_sig.returns.push(AbiParam::new(types::I64)); // NaN-boxed result
        let filter_atom_func_id = module
            .declare_function("jit_runtime_filter_atom", Linkage::Import, &filter_atom_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_filter_atom: {}", e))
            })?;

        // Phase 1.8: jit_runtime_foldl_atom(ctx, list, init, func_chunk, ip) -> NaN-boxed result
        let mut foldl_atom_sig = module.make_signature();
        foldl_atom_sig.params.push(AbiParam::new(types::I64)); // ctx
        foldl_atom_sig.params.push(AbiParam::new(types::I64)); // list
        foldl_atom_sig.params.push(AbiParam::new(types::I64)); // init
        foldl_atom_sig.params.push(AbiParam::new(types::I64)); // func_chunk
        foldl_atom_sig.params.push(AbiParam::new(types::I64)); // ip
        foldl_atom_sig.returns.push(AbiParam::new(types::I64)); // NaN-boxed result
        let foldl_atom_func_id = module
            .declare_function("jit_runtime_foldl_atom", Linkage::Import, &foldl_atom_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_foldl_atom: {}", e))
            })?;

        // Phase 1.9: jit_runtime_get_metatype(ctx, value, ip) -> NaN-boxed atom
        let mut get_metatype_sig = module.make_signature();
        get_metatype_sig.params.push(AbiParam::new(types::I64)); // ctx
        get_metatype_sig.params.push(AbiParam::new(types::I64)); // value
        get_metatype_sig.params.push(AbiParam::new(types::I64)); // ip
        get_metatype_sig.returns.push(AbiParam::new(types::I64)); // NaN-boxed metatype atom
        let get_metatype_func_id = module
            .declare_function("jit_runtime_get_metatype", Linkage::Import, &get_metatype_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_get_metatype: {}", e))
            })?;

        // Phase 1.10: jit_runtime_bloom_check(ctx, key, ip) -> NaN-boxed bool
        let mut bloom_check_sig = module.make_signature();
        bloom_check_sig.params.push(AbiParam::new(types::I64)); // ctx
        bloom_check_sig.params.push(AbiParam::new(types::I64)); // key
        bloom_check_sig.params.push(AbiParam::new(types::I64)); // ip
        bloom_check_sig.returns.push(AbiParam::new(types::I64)); // NaN-boxed bool
        let bloom_check_func_id = module
            .declare_function("jit_runtime_bloom_check", Linkage::Import, &bloom_check_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_bloom_check: {}", e))
            })?;

        // Phase 2.0: Extended Math Operations (PR #62)

        // jit_runtime_sqrt: fn(value: u64) -> u64
        let mut sqrt_sig = module.make_signature();
        sqrt_sig.params.push(AbiParam::new(types::I64)); // value (NaN-boxed)
        sqrt_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed Float)
        let sqrt_func_id = module
            .declare_function("jit_runtime_sqrt", Linkage::Import, &sqrt_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_sqrt: {}", e))
            })?;

        // jit_runtime_log: fn(base: u64, value: u64) -> u64
        let mut log_sig = module.make_signature();
        log_sig.params.push(AbiParam::new(types::I64)); // base (NaN-boxed)
        log_sig.params.push(AbiParam::new(types::I64)); // value (NaN-boxed)
        log_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed Float)
        let log_func_id = module
            .declare_function("jit_runtime_log", Linkage::Import, &log_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_log: {}", e))
            })?;

        // jit_runtime_trunc: fn(value: u64) -> u64
        let mut trunc_sig = module.make_signature();
        trunc_sig.params.push(AbiParam::new(types::I64)); // value (NaN-boxed)
        trunc_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed Long)
        let trunc_func_id = module
            .declare_function("jit_runtime_trunc", Linkage::Import, &trunc_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_trunc: {}", e))
            })?;

        // jit_runtime_ceil: fn(value: u64) -> u64
        let mut ceil_sig = module.make_signature();
        ceil_sig.params.push(AbiParam::new(types::I64)); // value (NaN-boxed)
        ceil_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed Long)
        let ceil_func_id = module
            .declare_function("jit_runtime_ceil", Linkage::Import, &ceil_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_ceil: {}", e))
            })?;

        // jit_runtime_floor_math: fn(value: u64) -> u64
        let mut floor_math_sig = module.make_signature();
        floor_math_sig.params.push(AbiParam::new(types::I64)); // value (NaN-boxed)
        floor_math_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed Long)
        let floor_math_func_id = module
            .declare_function("jit_runtime_floor_math", Linkage::Import, &floor_math_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_floor_math: {}", e))
            })?;

        // jit_runtime_round: fn(value: u64) -> u64
        let mut round_sig = module.make_signature();
        round_sig.params.push(AbiParam::new(types::I64)); // value (NaN-boxed)
        round_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed Long)
        let round_func_id = module
            .declare_function("jit_runtime_round", Linkage::Import, &round_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_round: {}", e))
            })?;

        // jit_runtime_sin: fn(value: u64) -> u64
        let mut sin_sig = module.make_signature();
        sin_sig.params.push(AbiParam::new(types::I64)); // value (NaN-boxed)
        sin_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed Float)
        let sin_func_id = module
            .declare_function("jit_runtime_sin", Linkage::Import, &sin_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_sin: {}", e))
            })?;

        // jit_runtime_cos: fn(value: u64) -> u64
        let mut cos_sig = module.make_signature();
        cos_sig.params.push(AbiParam::new(types::I64)); // value (NaN-boxed)
        cos_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed Float)
        let cos_func_id = module
            .declare_function("jit_runtime_cos", Linkage::Import, &cos_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_cos: {}", e))
            })?;

        // jit_runtime_tan: fn(value: u64) -> u64
        let mut tan_sig = module.make_signature();
        tan_sig.params.push(AbiParam::new(types::I64)); // value (NaN-boxed)
        tan_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed Float)
        let tan_func_id = module
            .declare_function("jit_runtime_tan", Linkage::Import, &tan_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_tan: {}", e))
            })?;

        // jit_runtime_asin: fn(value: u64) -> u64
        let mut asin_sig = module.make_signature();
        asin_sig.params.push(AbiParam::new(types::I64)); // value (NaN-boxed)
        asin_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed Float)
        let asin_func_id = module
            .declare_function("jit_runtime_asin", Linkage::Import, &asin_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_asin: {}", e))
            })?;

        // jit_runtime_acos: fn(value: u64) -> u64
        let mut acos_sig = module.make_signature();
        acos_sig.params.push(AbiParam::new(types::I64)); // value (NaN-boxed)
        acos_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed Float)
        let acos_func_id = module
            .declare_function("jit_runtime_acos", Linkage::Import, &acos_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_acos: {}", e))
            })?;

        // jit_runtime_atan: fn(value: u64) -> u64
        let mut atan_sig = module.make_signature();
        atan_sig.params.push(AbiParam::new(types::I64)); // value (NaN-boxed)
        atan_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed Float)
        let atan_func_id = module
            .declare_function("jit_runtime_atan", Linkage::Import, &atan_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_atan: {}", e))
            })?;

        // jit_runtime_isnan: fn(value: u64) -> u64
        let mut isnan_sig = module.make_signature();
        isnan_sig.params.push(AbiParam::new(types::I64)); // value (NaN-boxed)
        isnan_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed Bool)
        let isnan_func_id = module
            .declare_function("jit_runtime_isnan", Linkage::Import, &isnan_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_isnan: {}", e))
            })?;

        // jit_runtime_isinf: fn(value: u64) -> u64
        let mut isinf_sig = module.make_signature();
        isinf_sig.params.push(AbiParam::new(types::I64)); // value (NaN-boxed)
        isinf_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed Bool)
        let isinf_func_id = module
            .declare_function("jit_runtime_isinf", Linkage::Import, &isinf_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_isinf: {}", e))
            })?;

        // Phase 2.1: Expression Manipulation Operations (PR #63)

        // jit_runtime_index_atom: fn(ctx: *mut JitContext, expr: u64, index: u64, ip: u64) -> u64
        let mut index_atom_sig = module.make_signature();
        index_atom_sig.params.push(AbiParam::new(types::I64)); // ctx
        index_atom_sig.params.push(AbiParam::new(types::I64)); // expr (NaN-boxed)
        index_atom_sig.params.push(AbiParam::new(types::I64)); // index (NaN-boxed)
        index_atom_sig.params.push(AbiParam::new(types::I64)); // ip
        index_atom_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed)
        let index_atom_func_id = module
            .declare_function("jit_runtime_index_atom", Linkage::Import, &index_atom_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_index_atom: {}", e))
            })?;

        // jit_runtime_min_atom: fn(ctx: *mut JitContext, expr: u64, ip: u64) -> u64
        let mut min_atom_sig = module.make_signature();
        min_atom_sig.params.push(AbiParam::new(types::I64)); // ctx
        min_atom_sig.params.push(AbiParam::new(types::I64)); // expr (NaN-boxed)
        min_atom_sig.params.push(AbiParam::new(types::I64)); // ip
        min_atom_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed)
        let min_atom_func_id = module
            .declare_function("jit_runtime_min_atom", Linkage::Import, &min_atom_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_min_atom: {}", e))
            })?;

        // jit_runtime_max_atom: fn(ctx: *mut JitContext, expr: u64, ip: u64) -> u64
        let mut max_atom_sig = module.make_signature();
        max_atom_sig.params.push(AbiParam::new(types::I64)); // ctx
        max_atom_sig.params.push(AbiParam::new(types::I64)); // expr (NaN-boxed)
        max_atom_sig.params.push(AbiParam::new(types::I64)); // ip
        max_atom_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed)
        let max_atom_func_id = module
            .declare_function("jit_runtime_max_atom", Linkage::Import, &max_atom_sig)
            .map_err(|e| {
                JitError::CompilationError(format!("Failed to declare jit_runtime_max_atom: {}", e))
            })?;

        Ok(JitCompiler {
            module,
            func_counter: 0,
            pow_func_id,
            load_const_func_id,
            push_empty_func_id,
            get_head_func_id,
            get_tail_func_id,
            get_arity_func_id,
            get_element_func_id,
            get_type_func_id,
            check_type_func_id,
            assert_type_func_id,
            make_sexpr_func_id,
            cons_atom_func_id,
            push_uri_func_id,
            make_list_func_id,
            make_quote_func_id,
            call_func_id,
            tail_call_func_id,
            call_n_func_id,
            tail_call_n_func_id,
            fork_func_id,
            fork_native_func_id,
            yield_func_id,
            collect_func_id,
            yield_native_func_id,
            collect_native_func_id,
            load_binding_func_id,
            store_binding_func_id,
            has_binding_func_id,
            clear_bindings_func_id,
            push_binding_frame_func_id,
            pop_binding_frame_func_id,
            pattern_match_func_id,
            pattern_match_bind_func_id,
            match_arity_func_id,
            match_head_func_id,
            unify_func_id,
            unify_bind_func_id,
            space_add_func_id,
            space_remove_func_id,
            space_get_atoms_func_id,
            space_match_func_id,
            new_state_func_id,
            get_state_func_id,
            change_state_func_id,
            dispatch_rules_func_id,
            try_rule_func_id,
            next_rule_func_id,
            commit_rule_func_id,
            fail_rule_func_id,
            lookup_rules_func_id,
            apply_subst_func_id,
            define_rule_func_id,
            // Phase E: Special Forms
            eval_if_func_id,
            eval_let_func_id,
            eval_let_star_func_id,
            eval_match_func_id,
            eval_case_func_id,
            eval_chain_func_id,
            eval_quote_func_id,
            eval_unquote_func_id,
            eval_eval_func_id,
            eval_bind_func_id,
            eval_new_func_id,
            eval_collapse_func_id,
            eval_superpose_func_id,
            eval_memo_func_id,
            eval_memo_first_func_id,
            eval_pragma_func_id,
            eval_function_func_id,
            eval_lambda_func_id,
            eval_apply_func_id,
            // Phase G: Advanced Nondeterminism
            cut_func_id,
            guard_func_id,
            amb_func_id,
            commit_func_id,
            backtrack_func_id,
            // Phase F: Advanced Calls
            call_native_func_id,
            call_external_func_id,
            call_cached_func_id,
            // Phase H: MORK Bridge
            mork_lookup_func_id,
            mork_match_func_id,
            mork_insert_func_id,
            mork_delete_func_id,
            // Phase I: Debug/Meta
            trace_func_id,
            breakpoint_func_id,
            // Phase 1.1: Core Nondeterminism Markers
            begin_nondet_func_id,
            end_nondet_func_id,
            // Phase 1.3: Multi-value Return
            return_multi_func_id,
            collect_n_func_id,
            // Phase 1.5: Global/Space Access
            load_global_func_id,
            store_global_func_id,
            load_space_func_id,
            // Phase 1.6: Closure Support
            load_upvalue_func_id,
            // Phase 1.7: Atom Operations
            decon_atom_func_id,
            repr_func_id,
            // Phase 1.8: Higher-Order Operations
            map_atom_func_id,
            filter_atom_func_id,
            foldl_atom_func_id,
            // Phase 1.9: Meta-Type Operations
            get_metatype_func_id,
            // Phase 1.10: MORK and Debug
            bloom_check_func_id,
            // Phase 2.0: Extended Math Operations (PR #62)
            sqrt_func_id,
            log_func_id,
            trunc_func_id,
            ceil_func_id,
            floor_math_func_id,
            round_func_id,
            sin_func_id,
            cos_func_id,
            tan_func_id,
            asin_func_id,
            acos_func_id,
            atan_func_id,
            isnan_func_id,
            isinf_func_id,
            // Phase 2.1: Expression Manipulation Operations (PR #63)
            index_atom_func_id,
            min_atom_func_id,
            max_atom_func_id,
        })
    }

    /// Create a new JIT compiler (stub when feature disabled)
    #[cfg(not(feature = "jit"))]
    pub fn new() -> JitResult<Self> {
        Err(JitError::NotCompilable(
            "JIT feature not enabled".to_string(),
        ))
    }

    /// Register runtime helper functions for use from JIT code
    #[cfg(feature = "jit")]
    fn register_runtime_symbols(builder: &mut JITBuilder) {
        // Register runtime functions that JIT code can call
        builder.symbol("jit_runtime_type_error", super::runtime::jit_runtime_type_error as *const u8);
        builder.symbol(
            "jit_runtime_div_by_zero",
            super::runtime::jit_runtime_div_by_zero as *const u8,
        );
        builder.symbol(
            "jit_runtime_stack_overflow",
            super::runtime::jit_runtime_stack_overflow as *const u8,
        );
        // Stage 2: Arithmetic runtime functions
        builder.symbol(
            "jit_runtime_pow",
            super::runtime::jit_runtime_pow as *const u8,
        );
        // Stage 2: Constant loading
        builder.symbol(
            "jit_runtime_load_constant",
            super::runtime::jit_runtime_load_constant as *const u8,
        );
        // Phase 1: Type operations
        builder.symbol(
            "jit_runtime_get_type",
            super::runtime::jit_runtime_get_type as *const u8,
        );
        builder.symbol(
            "jit_runtime_check_type",
            super::runtime::jit_runtime_check_type as *const u8,
        );
        // Phase J: Type assertion
        builder.symbol(
            "jit_runtime_assert_type",
            super::runtime::jit_runtime_assert_type as *const u8,
        );
        // Phase 2a: Value creation
        builder.symbol(
            "jit_runtime_make_sexpr",
            super::runtime::jit_runtime_make_sexpr as *const u8,
        );
        builder.symbol(
            "jit_runtime_cons_atom",
            super::runtime::jit_runtime_cons_atom as *const u8,
        );
        // Phase 2b: More value creation
        builder.symbol(
            "jit_runtime_push_uri",
            super::runtime::jit_runtime_push_uri as *const u8,
        );
        builder.symbol(
            "jit_runtime_make_list",
            super::runtime::jit_runtime_make_list as *const u8,
        );
        builder.symbol(
            "jit_runtime_make_quote",
            super::runtime::jit_runtime_make_quote as *const u8,
        );

        // Phase 3: Call/TailCall support
        builder.symbol(
            "jit_runtime_call",
            super::runtime::jit_runtime_call as *const u8,
        );
        builder.symbol(
            "jit_runtime_tail_call",
            super::runtime::jit_runtime_tail_call as *const u8,
        );
        // Phase 4: Fork/Yield/Collect
        builder.symbol(
            "jit_runtime_fork",
            super::runtime::jit_runtime_fork as *const u8,
        );
        builder.symbol(
            "jit_runtime_yield",
            super::runtime::jit_runtime_yield as *const u8,
        );
        builder.symbol(
            "jit_runtime_collect",
            super::runtime::jit_runtime_collect as *const u8,
        );

        // Stage 14: S-expression operations
        builder.symbol(
            "jit_runtime_push_empty",
            super::runtime::jit_runtime_push_empty as *const u8,
        );
        builder.symbol(
            "jit_runtime_get_head",
            super::runtime::jit_runtime_get_head as *const u8,
        );
        builder.symbol(
            "jit_runtime_get_tail",
            super::runtime::jit_runtime_get_tail as *const u8,
        );
        builder.symbol(
            "jit_runtime_get_arity",
            super::runtime::jit_runtime_get_arity as *const u8,
        );
        builder.symbol(
            "jit_runtime_get_element",
            super::runtime::jit_runtime_get_element as *const u8,
        );

        // Stage 2 JIT: Native nondeterminism functions
        builder.symbol(
            "jit_runtime_save_stack",
            super::runtime::jit_runtime_save_stack as *const u8,
        );
        builder.symbol(
            "jit_runtime_restore_stack",
            super::runtime::jit_runtime_restore_stack as *const u8,
        );
        builder.symbol(
            "jit_runtime_fork_native",
            super::runtime::jit_runtime_fork_native as *const u8,
        );
        builder.symbol(
            "jit_runtime_yield_native",
            super::runtime::jit_runtime_yield_native as *const u8,
        );
        builder.symbol(
            "jit_runtime_fail_native",
            super::runtime::jit_runtime_fail_native as *const u8,
        );
        builder.symbol(
            "jit_runtime_collect_native",
            super::runtime::jit_runtime_collect_native as *const u8,
        );
        builder.symbol(
            "jit_runtime_has_alternatives",
            super::runtime::jit_runtime_has_alternatives as *const u8,
        );
        builder.symbol(
            "jit_runtime_get_resume_ip",
            super::runtime::jit_runtime_get_resume_ip as *const u8,
        );

        // Phase A: Binding operations
        builder.symbol(
            "jit_runtime_load_binding",
            super::runtime::jit_runtime_load_binding as *const u8,
        );
        builder.symbol(
            "jit_runtime_store_binding",
            super::runtime::jit_runtime_store_binding as *const u8,
        );
        builder.symbol(
            "jit_runtime_has_binding",
            super::runtime::jit_runtime_has_binding as *const u8,
        );
        builder.symbol(
            "jit_runtime_clear_bindings",
            super::runtime::jit_runtime_clear_bindings as *const u8,
        );
        builder.symbol(
            "jit_runtime_push_binding_frame",
            super::runtime::jit_runtime_push_binding_frame as *const u8,
        );
        builder.symbol(
            "jit_runtime_pop_binding_frame",
            super::runtime::jit_runtime_pop_binding_frame as *const u8,
        );

        // Phase B: Pattern matching operations
        builder.symbol(
            "jit_runtime_pattern_match",
            super::runtime::jit_runtime_pattern_match as *const u8,
        );
        builder.symbol(
            "jit_runtime_pattern_match_bind",
            super::runtime::jit_runtime_pattern_match_bind as *const u8,
        );
        builder.symbol(
            "jit_runtime_match_arity",
            super::runtime::jit_runtime_match_arity as *const u8,
        );
        builder.symbol(
            "jit_runtime_match_head",
            super::runtime::jit_runtime_match_head as *const u8,
        );
        builder.symbol(
            "jit_runtime_unify",
            super::runtime::jit_runtime_unify as *const u8,
        );
        builder.symbol(
            "jit_runtime_unify_bind",
            super::runtime::jit_runtime_unify_bind as *const u8,
        );

        // Phase D: Space operations
        builder.symbol(
            "jit_runtime_space_add",
            super::runtime::jit_runtime_space_add as *const u8,
        );
        builder.symbol(
            "jit_runtime_space_remove",
            super::runtime::jit_runtime_space_remove as *const u8,
        );
        builder.symbol(
            "jit_runtime_space_get_atoms",
            super::runtime::jit_runtime_space_get_atoms as *const u8,
        );
        builder.symbol(
            "jit_runtime_space_match_nondet",
            super::runtime::jit_runtime_space_match_nondet as *const u8,
        );

        // Phase D.1: State operations
        builder.symbol(
            "jit_runtime_new_state",
            super::runtime::jit_runtime_new_state as *const u8,
        );
        builder.symbol(
            "jit_runtime_get_state",
            super::runtime::jit_runtime_get_state as *const u8,
        );
        builder.symbol(
            "jit_runtime_change_state",
            super::runtime::jit_runtime_change_state as *const u8,
        );

        // Phase C: Rule dispatch operations
        builder.symbol(
            "jit_runtime_dispatch_rules",
            super::runtime::jit_runtime_dispatch_rules as *const u8,
        );
        builder.symbol(
            "jit_runtime_try_rule",
            super::runtime::jit_runtime_try_rule as *const u8,
        );
        builder.symbol(
            "jit_runtime_next_rule",
            super::runtime::jit_runtime_next_rule as *const u8,
        );
        builder.symbol(
            "jit_runtime_commit_rule",
            super::runtime::jit_runtime_commit_rule as *const u8,
        );
        builder.symbol(
            "jit_runtime_fail_rule",
            super::runtime::jit_runtime_fail_rule as *const u8,
        );
        builder.symbol(
            "jit_runtime_lookup_rules",
            super::runtime::jit_runtime_lookup_rules as *const u8,
        );
        builder.symbol(
            "jit_runtime_apply_subst",
            super::runtime::jit_runtime_apply_subst as *const u8,
        );
        builder.symbol(
            "jit_runtime_define_rule",
            super::runtime::jit_runtime_define_rule as *const u8,
        );

        // Phase E: Special Forms
        builder.symbol(
            "jit_runtime_eval_if",
            super::runtime::jit_runtime_eval_if as *const u8,
        );
        builder.symbol(
            "jit_runtime_eval_let",
            super::runtime::jit_runtime_eval_let as *const u8,
        );
        builder.symbol(
            "jit_runtime_eval_let_star",
            super::runtime::jit_runtime_eval_let_star as *const u8,
        );
        builder.symbol(
            "jit_runtime_eval_match",
            super::runtime::jit_runtime_eval_match as *const u8,
        );
        builder.symbol(
            "jit_runtime_eval_case",
            super::runtime::jit_runtime_eval_case as *const u8,
        );
        builder.symbol(
            "jit_runtime_eval_chain",
            super::runtime::jit_runtime_eval_chain as *const u8,
        );
        builder.symbol(
            "jit_runtime_eval_quote",
            super::runtime::jit_runtime_eval_quote as *const u8,
        );
        builder.symbol(
            "jit_runtime_eval_unquote",
            super::runtime::jit_runtime_eval_unquote as *const u8,
        );
        builder.symbol(
            "jit_runtime_eval_eval",
            super::runtime::jit_runtime_eval_eval as *const u8,
        );
        builder.symbol(
            "jit_runtime_eval_bind",
            super::runtime::jit_runtime_eval_bind as *const u8,
        );
        builder.symbol(
            "jit_runtime_eval_new",
            super::runtime::jit_runtime_eval_new as *const u8,
        );
        builder.symbol(
            "jit_runtime_eval_collapse",
            super::runtime::jit_runtime_eval_collapse as *const u8,
        );
        builder.symbol(
            "jit_runtime_eval_superpose",
            super::runtime::jit_runtime_eval_superpose as *const u8,
        );
        builder.symbol(
            "jit_runtime_eval_memo",
            super::runtime::jit_runtime_eval_memo as *const u8,
        );
        builder.symbol(
            "jit_runtime_eval_memo_first",
            super::runtime::jit_runtime_eval_memo_first as *const u8,
        );
        builder.symbol(
            "jit_runtime_eval_pragma",
            super::runtime::jit_runtime_eval_pragma as *const u8,
        );
        builder.symbol(
            "jit_runtime_eval_function",
            super::runtime::jit_runtime_eval_function as *const u8,
        );
        builder.symbol(
            "jit_runtime_eval_lambda",
            super::runtime::jit_runtime_eval_lambda as *const u8,
        );
        builder.symbol(
            "jit_runtime_eval_apply",
            super::runtime::jit_runtime_eval_apply as *const u8,
        );

        // Phase G: Advanced Nondeterminism
        builder.symbol(
            "jit_runtime_cut",
            super::runtime::jit_runtime_cut as *const u8,
        );
        builder.symbol(
            "jit_runtime_guard",
            super::runtime::jit_runtime_guard as *const u8,
        );
        builder.symbol(
            "jit_runtime_amb",
            super::runtime::jit_runtime_amb as *const u8,
        );
        builder.symbol(
            "jit_runtime_commit",
            super::runtime::jit_runtime_commit as *const u8,
        );
        builder.symbol(
            "jit_runtime_backtrack",
            super::runtime::jit_runtime_backtrack as *const u8,
        );

        // Phase F: Advanced Calls
        builder.symbol(
            "jit_runtime_call_native",
            super::runtime::jit_runtime_call_native as *const u8,
        );
        builder.symbol(
            "jit_runtime_call_external",
            super::runtime::jit_runtime_call_external as *const u8,
        );
        builder.symbol(
            "jit_runtime_call_cached",
            super::runtime::jit_runtime_call_cached as *const u8,
        );

        // Phase H: MORK Bridge
        builder.symbol(
            "jit_runtime_mork_lookup",
            super::runtime::jit_runtime_mork_lookup as *const u8,
        );
        builder.symbol(
            "jit_runtime_mork_match",
            super::runtime::jit_runtime_mork_match as *const u8,
        );
        builder.symbol(
            "jit_runtime_mork_insert",
            super::runtime::jit_runtime_mork_insert as *const u8,
        );
        builder.symbol(
            "jit_runtime_mork_delete",
            super::runtime::jit_runtime_mork_delete as *const u8,
        );

        // Phase I: Debug/Meta
        builder.symbol(
            "jit_runtime_trace",
            super::runtime::jit_runtime_trace as *const u8,
        );
        builder.symbol(
            "jit_runtime_breakpoint",
            super::runtime::jit_runtime_breakpoint as *const u8,
        );

        // Phase 1.1: Core Nondeterminism Markers
        builder.symbol(
            "jit_runtime_begin_nondet",
            super::runtime::jit_runtime_begin_nondet as *const u8,
        );
        builder.symbol(
            "jit_runtime_end_nondet",
            super::runtime::jit_runtime_end_nondet as *const u8,
        );

        // Phase 1.3: Multi-value Return
        builder.symbol(
            "jit_runtime_return_multi",
            super::runtime::jit_runtime_return_multi as *const u8,
        );
        builder.symbol(
            "jit_runtime_collect_n",
            super::runtime::jit_runtime_collect_n as *const u8,
        );

        // Phase 1.5: Global/Space Access
        builder.symbol(
            "jit_runtime_load_global",
            super::runtime::jit_runtime_load_global as *const u8,
        );
        builder.symbol(
            "jit_runtime_store_global",
            super::runtime::jit_runtime_store_global as *const u8,
        );
        builder.symbol(
            "jit_runtime_load_space",
            super::runtime::jit_runtime_load_space as *const u8,
        );

        // Phase 1.6: Closure Support
        builder.symbol(
            "jit_runtime_load_upvalue",
            super::runtime::jit_runtime_load_upvalue as *const u8,
        );

        // Phase 1.7: Atom Operations
        builder.symbol(
            "jit_runtime_decon_atom",
            super::runtime::jit_runtime_decon_atom as *const u8,
        );
        builder.symbol(
            "jit_runtime_repr",
            super::runtime::jit_runtime_repr as *const u8,
        );

        // Phase 1.8: Higher-Order Operations
        builder.symbol(
            "jit_runtime_map_atom",
            super::runtime::jit_runtime_map_atom as *const u8,
        );
        builder.symbol(
            "jit_runtime_filter_atom",
            super::runtime::jit_runtime_filter_atom as *const u8,
        );
        builder.symbol(
            "jit_runtime_foldl_atom",
            super::runtime::jit_runtime_foldl_atom as *const u8,
        );

        // Phase 1.9: Meta-Type Operations
        builder.symbol(
            "jit_runtime_get_metatype",
            super::runtime::jit_runtime_get_metatype as *const u8,
        );

        // Phase 1.10: MORK and Debug
        builder.symbol(
            "jit_runtime_bloom_check",
            super::runtime::jit_runtime_bloom_check as *const u8,
        );

        // Phase 2.0: Extended Math Operations (PR #62)
        builder.symbol(
            "jit_runtime_sqrt",
            super::runtime::jit_runtime_sqrt as *const u8,
        );
        builder.symbol(
            "jit_runtime_log",
            super::runtime::jit_runtime_log as *const u8,
        );
        builder.symbol(
            "jit_runtime_trunc",
            super::runtime::jit_runtime_trunc as *const u8,
        );
        builder.symbol(
            "jit_runtime_ceil",
            super::runtime::jit_runtime_ceil as *const u8,
        );
        builder.symbol(
            "jit_runtime_floor_math",
            super::runtime::jit_runtime_floor_math as *const u8,
        );
        builder.symbol(
            "jit_runtime_round",
            super::runtime::jit_runtime_round as *const u8,
        );
        builder.symbol(
            "jit_runtime_sin",
            super::runtime::jit_runtime_sin as *const u8,
        );
        builder.symbol(
            "jit_runtime_cos",
            super::runtime::jit_runtime_cos as *const u8,
        );
        builder.symbol(
            "jit_runtime_tan",
            super::runtime::jit_runtime_tan as *const u8,
        );
        builder.symbol(
            "jit_runtime_asin",
            super::runtime::jit_runtime_asin as *const u8,
        );
        builder.symbol(
            "jit_runtime_acos",
            super::runtime::jit_runtime_acos as *const u8,
        );
        builder.symbol(
            "jit_runtime_atan",
            super::runtime::jit_runtime_atan as *const u8,
        );
        builder.symbol(
            "jit_runtime_isnan",
            super::runtime::jit_runtime_isnan as *const u8,
        );
        builder.symbol(
            "jit_runtime_isinf",
            super::runtime::jit_runtime_isinf as *const u8,
        );

        // Phase 2.1: Expression Manipulation Operations (PR #63)
        builder.symbol(
            "jit_runtime_index_atom",
            super::runtime::jit_runtime_index_atom as *const u8,
        );
        builder.symbol(
            "jit_runtime_min_atom",
            super::runtime::jit_runtime_min_atom as *const u8,
        );
        builder.symbol(
            "jit_runtime_max_atom",
            super::runtime::jit_runtime_max_atom as *const u8,
        );
    }

    /// Check if a bytecode chunk can be JIT compiled (Stage 1-5 + Phase A-I)
    ///
    /// Supported features:
    /// - Stack ops: Nop, Pop, Dup, Swap, Rot3, Over, DupN, PopN
    /// - Arithmetic: Add, Sub, Mul, Div, Mod, Neg, Abs, FloorDiv, Pow (runtime call)
    /// - Boolean: And, Or, Not, Xor
    /// - Comparisons: Lt, Le, Gt, Ge, Eq, Ne
    /// - Constants: PushLongSmall, PushTrue, PushFalse, PushNil, PushConstant (runtime call)
    /// - Control: Return, Jump, JumpIfFalse, JumpIfTrue, JumpShort, JumpIfFalseShort, JumpIfTrueShort
    /// - Stage 4: Local variables - LoadLocal, StoreLocal, LoadLocalWide, StoreLocalWide
    /// - Stage 5: Type jumps - JumpIfNil, JumpIfError
    /// - Stage 6: Type predicates - IsVariable, IsSExpr, IsSymbol
    /// - Phase A: Bindings - LoadBinding, StoreBinding, HasBinding, ClearBindings, PushBindingFrame, PopBindingFrame
    /// - Phase B: Pattern matching - Match, MatchBind, MatchHead, MatchArity, MatchGuard, Unify, UnifyBind
    /// - Phase C: Rule dispatch - DispatchRules, TryRule, NextRule, CommitRule, FailRule, LookupRules, ApplySubst, DefineRule
    /// - Phase D: Space operations - SpaceAdd, SpaceRemove, SpaceGetAtoms, SpaceMatch
    /// - Phase G: Advanced nondeterminism - Cut
    /// - Phase H: MORK bridge - MorkLookup, MorkMatch, MorkInsert, MorkDelete
    /// - Phase I: Debug/Meta - Trace, Breakpoint
    pub fn can_compile_stage1(chunk: &BytecodeChunk) -> bool {
        // Fast path: reject nondeterministic chunks immediately
        // This avoids wasteful JIT compilation followed by bailout for
        // chunks containing Fork/Yield/Collect/etc.
        if chunk.has_nondeterminism() {
            return false;
        }

        let code = chunk.code();
        let mut offset = 0;

        while offset < code.len() {
            let Some(op) = chunk.read_opcode(offset) else {
                return false;
            };

            match op {
                // Stack operations (all Stage 1)
                Opcode::Nop
                | Opcode::Pop
                | Opcode::Dup
                | Opcode::Swap
                | Opcode::Rot3
                | Opcode::Over
                | Opcode::DupN
                | Opcode::PopN => {}

                // Value creation (Stage 1: simple constants, Stage 2+13: via runtime calls)
                Opcode::PushNil
                | Opcode::PushTrue
                | Opcode::PushFalse
                | Opcode::PushUnit
                | Opcode::PushLongSmall
                | Opcode::PushLong      // Stage 2: large integers via runtime call
                | Opcode::PushConstant  // Stage 2: generic constants via runtime call
                | Opcode::PushEmpty     // Stage 13: empty S-expr via runtime call
                | Opcode::PushAtom      // Stage 13: atom from constant pool via runtime call
                | Opcode::PushString    // Stage 13: string from constant pool via runtime call
                | Opcode::PushVariable => {} // Stage 13: variable from constant pool via runtime call

                // S-expression operations (Stage 14: via runtime calls)
                Opcode::GetHead     // Stage 14: get first element via runtime call
                | Opcode::GetTail   // Stage 14: get all but first via runtime call
                | Opcode::GetArity  // Stage 14: get element count via runtime call
                | Opcode::GetElement => {} // Stage 14b: get element by index via runtime call

                // Arithmetic (Stage 1 + Stage 2 Pow with runtime call)
                Opcode::Add
                | Opcode::Sub
                | Opcode::Mul
                | Opcode::Div
                | Opcode::Mod
                | Opcode::Neg
                | Opcode::Abs
                | Opcode::FloorDiv
                | Opcode::Pow => {} // Stage 2: Pow uses runtime call

                // Extended math operations (PR #62) - all use runtime calls
                Opcode::Sqrt
                | Opcode::Log
                | Opcode::Trunc
                | Opcode::Ceil
                | Opcode::FloorMath
                | Opcode::Round
                | Opcode::Sin
                | Opcode::Cos
                | Opcode::Tan
                | Opcode::Asin
                | Opcode::Acos
                | Opcode::Atan
                | Opcode::IsNan
                | Opcode::IsInf => {}

                // Expression manipulation (PR #63) - all use runtime calls
                Opcode::IndexAtom
                | Opcode::MinAtom
                | Opcode::MaxAtom => {}

                // Boolean
                Opcode::And | Opcode::Or | Opcode::Not | Opcode::Xor => {}

                // Comparisons
                Opcode::Lt
                | Opcode::Le
                | Opcode::Gt
                | Opcode::Ge
                | Opcode::Eq
                | Opcode::Ne
                | Opcode::StructEq => {}

                // Control (Stage 1: Return, Stage 3: Jumps)
                Opcode::Return => {}

                // Stage 3: Jump instructions
                Opcode::Jump
                | Opcode::JumpIfFalse
                | Opcode::JumpIfTrue
                | Opcode::JumpShort
                | Opcode::JumpIfFalseShort
                | Opcode::JumpIfTrueShort => {}

                // Stage 4: Local variables
                Opcode::LoadLocal
                | Opcode::StoreLocal
                | Opcode::LoadLocalWide
                | Opcode::StoreLocalWide => {}

                // Stage 5: Type-based jumps
                Opcode::JumpIfNil
                | Opcode::JumpIfError => {}

                // Stage 6: Type predicates
                Opcode::IsVariable
                | Opcode::IsSExpr
                | Opcode::IsSymbol => {}

                // Phase 1: Type operations (via runtime calls)
                Opcode::GetType
                | Opcode::CheckType
                | Opcode::IsType => {}

                // Phase J: Type assertion (via runtime call)
                Opcode::AssertType => {}

                // Phase 2a: Value creation (via runtime calls)
                Opcode::MakeSExpr
                | Opcode::MakeSExprLarge
                | Opcode::ConsAtom => {}

                // Phase 2b: More value creation (via runtime calls)
                Opcode::PushUri     // Stage 2b: URI from constant pool (same as PushConstant)
                | Opcode::MakeList  // Stage 2b: proper list (Cons elem (Cons ... Nil))
                | Opcode::MakeQuote => {} // Stage 2b: quote wrapper (quote value)

                // Phase 3: Call/TailCall (bailout to VM for rule dispatch)
                Opcode::Call        // Stage 3: call with bailout
                | Opcode::TailCall  // Stage 3: tail call with bailout
                | Opcode::CallN     // Phase 1.2: call with N args (stack-based head)
                | Opcode::TailCallN => {} // Phase 1.2: tail call with N args (stack-based head)

                // NOTE: Fork/Yield/Collect are NOT compilable - they are detected
                // statically via has_nondeterminism() and routed to bytecode tier.
                // This avoids wasteful JIT compilation followed by immediate bailout.

                // Phase A: Binding operations (via runtime calls)
                Opcode::LoadBinding       // Phase A: load binding by name index
                | Opcode::StoreBinding    // Phase A: store binding by name index
                | Opcode::HasBinding      // Phase A: check if binding exists
                | Opcode::ClearBindings   // Phase A: clear all bindings
                | Opcode::PushBindingFrame  // Phase A: push new binding frame
                | Opcode::PopBindingFrame => {} // Phase A: pop binding frame

                // Phase B: Pattern matching operations (via runtime calls)
                Opcode::Match           // Phase B: pattern match [pattern, value] -> [bool]
                | Opcode::MatchBind     // Phase B: match and bind [pattern, value] -> [bool]
                | Opcode::MatchHead     // Phase B: match head symbol [symbol, expr] -> [bool]
                | Opcode::MatchArity    // Phase B: match arity [expr] -> [bool]
                | Opcode::MatchGuard    // Phase B: match with guard condition
                | Opcode::Unify         // Phase B: unify [a, b] -> [bool]
                | Opcode::UnifyBind => {} // Phase B: unify with binding [a, b] -> [bool]

                // Phase D: Space operations (via runtime calls)
                Opcode::SpaceAdd        // Phase D: add atom to space [space, atom] -> [bool]
                | Opcode::SpaceRemove   // Phase D: remove atom from space [space, atom] -> [bool]
                | Opcode::SpaceGetAtoms // Phase D: get all atoms from space [space] -> [list]
                | Opcode::SpaceMatch => {} // Phase D: match pattern in space [space, pattern, template] -> [results]

                // Phase D.1: State operations (via runtime calls)
                Opcode::NewState        // Phase D.1: create state [initial] -> [State(id)]
                | Opcode::GetState      // Phase D.1: get state value [State(id)] -> [value]
                | Opcode::ChangeState => {} // Phase D.1: change state [State(id), value] -> [State(id)]

                // Phase C: Rule dispatch operations (via runtime calls)
                Opcode::DispatchRules   // Phase C: dispatch rules [expr] -> [count]
                | Opcode::TryRule       // Phase C: try single rule [expr] -> [result]
                | Opcode::NextRule      // Phase C: advance to next rule
                | Opcode::CommitRule    // Phase C: commit to current rule (cut)
                | Opcode::FailRule      // Phase C: signal rule failure
                | Opcode::LookupRules   // Phase C: look up rules by head [head_idx] -> [count]
                | Opcode::ApplySubst    // Phase C: apply substitution [expr] -> [result]
                | Opcode::DefineRule => {} // Phase C: define new rule [pattern, body] -> [Unit]

                // Phase E: Special Forms (via runtime calls)
                Opcode::EvalIf          // Phase E: if expression [cond, then, else] -> [result]
                | Opcode::EvalLet       // Phase E: let binding [name, value] -> [Unit]
                | Opcode::EvalLetStar   // Phase E: sequential let bindings
                | Opcode::EvalMatch     // Phase E: match expression [value, pattern] -> [bool]
                | Opcode::EvalCase      // Phase E: case expression [value] -> [case_index]
                | Opcode::EvalChain     // Phase E: chain expression [first, second] -> [second]
                | Opcode::EvalQuote     // Phase E: quote expression [expr] -> [quoted]
                | Opcode::EvalUnquote   // Phase E: unquote expression [quoted] -> [result]
                | Opcode::EvalEval      // Phase E: eval expression [expr] -> [result]
                | Opcode::EvalBind      // Phase E: bind expression [name, value] -> [Unit]
                | Opcode::EvalNew       // Phase E: new space [] -> [space]
                | Opcode::EvalCollapse  // Phase E: collapse [expr] -> [list]
                | Opcode::EvalSuperpose // Phase E: superpose [list] -> [choice]
                | Opcode::EvalMemo      // Phase E: memoized eval [expr] -> [result]
                | Opcode::EvalMemoFirst // Phase E: memoize first [expr] -> [result]
                | Opcode::EvalPragma    // Phase E: pragma directive [directive] -> [Unit]
                | Opcode::EvalFunction  // Phase E: function definition [name, params, body] -> [Unit]
                | Opcode::EvalLambda    // Phase E: lambda expression [params, body] -> [closure]
                | Opcode::EvalApply => {} // Phase E: apply closure [closure, args] -> [result]

                // Phase G: Advanced Nondeterminism (via runtime calls)
                Opcode::Cut               // Phase G: prune search space
                | Opcode::Guard           // Phase G: guard condition [bool] -> [] (backtrack if false)
                | Opcode::Amb             // Phase G: amb choice [alts...] -> [selected]
                | Opcode::Commit          // Phase G: commit (soft cut) [] -> [Unit]
                | Opcode::Backtrack => {} // Phase G: force backtracking [] -> []

                // Phase F: Advanced Calls (via runtime calls)
                Opcode::CallNative        // Phase F: call native function [args...] -> [result]
                | Opcode::CallExternal    // Phase F: call external function [args...] -> [result]
                | Opcode::CallCached => {} // Phase F: cached function call [args...] -> [result]

                // Phase H: MORK Bridge (via runtime calls)
                Opcode::MorkLookup      // Phase H: lookup in MORK [path] -> [value]
                | Opcode::MorkMatch     // Phase H: match pattern in MORK [path, pattern] -> [results]
                | Opcode::MorkInsert    // Phase H: insert into MORK [path, value] -> [bool]
                | Opcode::MorkDelete => {} // Phase H: delete from MORK [path] -> [bool]

                // Phase I: Debug/Meta (via runtime calls)
                Opcode::Trace           // Phase I: emit trace event [msg_idx, value] -> []
                | Opcode::Breakpoint => {} // Phase I: debugger breakpoint [bp_id] -> []

                // Phase 1.1: Core Nondeterminism Markers (native or runtime calls)
                Opcode::Fail            // Phase 1.1: explicit failure (return FAIL signal)
                | Opcode::BeginNondet   // Phase 1.1: mark start of nondet section
                | Opcode::EndNondet => {} // Phase 1.1: mark end of nondet section

                // Phase 1.3: Multi-value Return (via runtime calls)
                Opcode::ReturnMulti     // Phase 1.3: return multiple values [count] -> signal
                | Opcode::CollectN => {} // Phase 1.3: collect up to N results [] -> [sexpr]

                // Phase 1.4: Multi-way Branch (native jump table)
                Opcode::JumpTable => {} // Phase 1.4: switch/case dispatch [index] -> []

                // Phase 1.5: Global/Space Access (via runtime calls)
                Opcode::LoadGlobal      // Phase 1.5: load global variable [symbol_idx] -> [value]
                | Opcode::StoreGlobal   // Phase 1.5: store global variable [symbol_idx, value] -> [unit]
                | Opcode::LoadSpace => {} // Phase 1.5: load space handle [name_idx] -> [space]

                // Phase 1.6: Closure Support (via runtime calls)
                Opcode::LoadUpvalue => {} // Phase 1.6: load from enclosing scope [depth, index] -> [value]

                // Phase 1.7: Atom Operations (via runtime calls)
                Opcode::DeconAtom       // Phase 1.7: deconstruct S-expr [expr] -> [(head, tail)]
                | Opcode::Repr => {}    // Phase 1.7: string representation [value] -> [string]

                // Phase 1.8: Higher-Order Operations (via runtime calls, may bailout)
                Opcode::MapAtom         // Phase 1.8: map function over list [list, func] -> [result]
                | Opcode::FilterAtom    // Phase 1.8: filter list by predicate [list, pred] -> [result]
                | Opcode::FoldlAtom => {} // Phase 1.8: left fold over list [list, init, func] -> [result]

                // Phase 1.9: Meta-Type Operations (via runtime calls)
                Opcode::GetMetaType => {} // Phase 1.9: get meta-level type [value] -> [metatype]

                // Phase 1.10: MORK and Debug (via runtime calls)
                Opcode::BloomCheck      // Phase 1.10: bloom filter pre-check [key] -> [bool]
                | Opcode::Halt => {}    // Phase 1.10: halt execution (return HALT signal)

                // Stage 7: Stack operations and Negation
                Opcode::Pop
                | Opcode::Dup
                | Opcode::Swap
                | Opcode::Neg
                | Opcode::DupN
                | Opcode::PopN => {}

                // Stage 8: More arithmetic and stack operations
                Opcode::Abs
                | Opcode::Mod
                | Opcode::FloorDiv
                | Opcode::Rot3
                | Opcode::Over => {}

                // Anything else is not compilable
                _ => return false,
            }

            // Advance by opcode size (1 byte) + operand size
            offset += 1 + op.immediate_size();
        }

        true
    }

    /// Pre-scan bytecode to find all jump targets and their predecessor counts
    ///
    /// Jump offsets are relative to the IP after reading the instruction and its operands.
    /// For example, if a Jump is at offset 6 with size 3 (1 opcode + 2 operand bytes),
    /// then the offset is relative to position 9 (6 + 3).
    #[cfg(feature = "jit")]
    fn find_block_info(chunk: &BytecodeChunk) -> BlockInfo {
        let code = chunk.code();
        let mut targets = Vec::new();
        let mut predecessor_count: HashMap<usize, usize> = HashMap::new();
        let mut offset = 0;

        // Helper function to add a target
        fn add_target(
            target: usize,
            code_len: usize,
            targets: &mut Vec<usize>,
            predecessor_count: &mut HashMap<usize, usize>,
        ) {
            if target <= code_len {
                if !targets.contains(&target) {
                    targets.push(target);
                }
                *predecessor_count.entry(target).or_insert(0) += 1;
            }
        }

        while offset < code.len() {
            let Some(op) = chunk.read_opcode(offset) else {
                break;
            };

            let instr_size = 1 + op.immediate_size();
            let next_ip = offset + instr_size; // IP after instruction

            match op {
                Opcode::Jump | Opcode::JumpIfFalse | Opcode::JumpIfTrue
                | Opcode::JumpIfNil | Opcode::JumpIfError => {
                    // 2-byte signed offset, relative to next_ip
                    let rel_offset = chunk.read_i16(offset + 1).unwrap_or(0);
                    let target = (next_ip as isize + rel_offset as isize) as usize;
                    add_target(target, code.len(), &mut targets, &mut predecessor_count);
                    // For conditional jumps, the fallthrough is also a target
                    if op != Opcode::Jump && next_ip < code.len() {
                        add_target(next_ip, code.len(), &mut targets, &mut predecessor_count);
                    }
                }
                Opcode::JumpShort | Opcode::JumpIfFalseShort | Opcode::JumpIfTrueShort => {
                    // 1-byte signed offset, relative to next_ip
                    let rel_offset = chunk.read_byte(offset + 1).unwrap_or(0) as i8;
                    let target = (next_ip as isize + rel_offset as isize) as usize;
                    add_target(target, code.len(), &mut targets, &mut predecessor_count);
                    // For conditional jumps, the fallthrough is also a target
                    if op != Opcode::JumpShort && next_ip < code.len() {
                        add_target(next_ip, code.len(), &mut targets, &mut predecessor_count);
                    }
                }
                Opcode::JumpTable => {
                    // JumpTable: table_index:u16
                    // Read table index and add all targets from the table
                    let table_index = chunk.read_u16(offset + 1).unwrap_or(0) as usize;
                    if let Some(jump_table) = chunk.get_jump_table(table_index) {
                        // Add all entry targets
                        for &(_hash, target) in &jump_table.entries {
                            add_target(target, code.len(), &mut targets, &mut predecessor_count);
                        }
                        // Add default target
                        add_target(jump_table.default_offset, code.len(), &mut targets, &mut predecessor_count);
                    }
                }
                Opcode::Return => {
                    // Return doesn't have a target
                }
                _ => {}
            }

            offset += instr_size;
        }

        // Second pass: count fallthroughs for blocks that aren't jump targets
        // but come after non-terminating instructions
        offset = 0;
        while offset < code.len() {
            let Some(op) = chunk.read_opcode(offset) else {
                break;
            };
            let instr_size = 1 + op.immediate_size();
            let next_ip = offset + instr_size;

            // Instructions that don't fall through to next_ip
            // - Terminating: Return, Jump, JumpShort, JumpTable
            // - Conditional jumps: their fallthrough is already counted in first pass
            let has_fallthrough_to_next = !matches!(
                op,
                Opcode::Return
                    | Opcode::Jump
                    | Opcode::JumpShort
                    | Opcode::JumpTable
                    | Opcode::JumpIfFalse
                    | Opcode::JumpIfTrue
                    | Opcode::JumpIfFalseShort
                    | Opcode::JumpIfTrueShort
                    | Opcode::JumpIfNil
                    | Opcode::JumpIfError
            );
            if has_fallthrough_to_next && next_ip < code.len() && targets.contains(&next_ip) {
                // This is a fallthrough edge
                *predecessor_count.entry(next_ip).or_insert(0) += 1;
            }

            offset += instr_size;
        }

        targets.sort();
        BlockInfo {
            targets,
            predecessor_count,
        }
    }

    /// Compile a bytecode chunk to native code
    ///
    /// Returns a function pointer that can be called with a JitContext
    #[cfg(feature = "jit")]
    pub fn compile(&mut self, chunk: &BytecodeChunk) -> JitResult<*const ()> {
        if !Self::can_compile_stage1(chunk) {
            return Err(JitError::NotCompilable(
                "Chunk contains non-Stage-1 opcodes".to_string(),
            ));
        }

        // Generate unique function name
        let func_name = format!("jit_chunk_{}", self.func_counter);
        self.func_counter += 1;

        // Declare function signature: fn(*mut JitContext) -> i64
        let mut sig = self.module.make_signature();
        sig.params.push(AbiParam::new(types::I64)); // ctx pointer
        sig.returns.push(AbiParam::new(types::I64)); // return value (or 0)

        // Declare the function
        let func_id = self
            .module
            .declare_function(&func_name, Linkage::Local, &sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare function: {}", e)))?;

        // Create function builder context
        let mut ctx = self.module.make_context();
        ctx.func.signature = sig;

        // Build the function
        self.build_function(&mut ctx, chunk)?;

        // Debug: print the generated IR
        #[cfg(test)]
        trace!(target: "mettatron::jit::compiler::ir", ir = %ctx.func.display(), "Generated IR");

        // Define the function in the module
        self.module
            .define_function(func_id, &mut ctx)
            .map_err(|e| JitError::CompilationError(format!("Failed to define function: {}", e)))?;

        // Finalize and get the code pointer
        self.module.finalize_definitions().map_err(|e| {
            JitError::CompilationError(format!("Failed to finalize definitions: {}", e))
        })?;

        let code_ptr = self.module.get_finalized_function(func_id);
        Ok(code_ptr as *const ())
    }

    /// Compile a bytecode chunk (stub when feature disabled)
    #[cfg(not(feature = "jit"))]
    pub fn compile(&mut self, _chunk: &BytecodeChunk) -> JitResult<*const ()> {
        Err(JitError::NotCompilable(
            "JIT feature not enabled".to_string(),
        ))
    }

    /// Build the Cranelift IR for a bytecode chunk
    #[cfg(feature = "jit")]
    fn build_function(
        &mut self,
        ctx: &mut codegen::Context,
        chunk: &BytecodeChunk,
    ) -> JitResult<()> {
        let mut func_ctx = FunctionBuilderContext::new();
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut func_ctx);

        // Find all jump targets and predecessor counts
        let block_info = Self::find_block_info(chunk);
        let mut offset_to_block: HashMap<usize, Block> = HashMap::new();
        let mut merge_blocks: HashMap<usize, bool> = HashMap::new(); // Track which blocks are merge points

        // Create entry block
        let entry_block = builder.create_block();
        builder.append_block_params_for_function_params(entry_block);
        offset_to_block.insert(0, entry_block);

        // Create blocks for each jump target
        // Blocks with >1 predecessor need a parameter for the stack value
        for &target in &block_info.targets {
            if target != 0 && !offset_to_block.contains_key(&target) {
                let block = builder.create_block();
                let pred_count = block_info.predecessor_count.get(&target).copied().unwrap_or(0);
                if pred_count > 1 {
                    // This is a merge point - add a parameter for the stack value
                    builder.append_block_param(block, types::I64);
                    merge_blocks.insert(target, true);
                }
                offset_to_block.insert(target, block);
            }
        }

        builder.switch_to_block(entry_block);
        builder.seal_block(entry_block);

        // Get the context pointer parameter
        let ctx_ptr = builder.block_params(entry_block)[0];

        // Use a scoping block so codegen's borrow of builder ends before finalize
        {
            // Create codegen context for helper methods
            let mut codegen = CodegenContext::new(&mut builder, ctx_ptr);

            // Stage 4: Initialize local variables
            codegen.init_locals(chunk.local_count() as usize);

            // Iterate over bytecode
            let code = chunk.code();
            let mut offset = 0;

            while offset < code.len() {
                // Check if this offset starts a new basic block
                if offset > 0 {
                    if let Some(&block) = offset_to_block.get(&offset) {
                        // If current block is not terminated, fall through
                        if !codegen.is_terminated() {
                            // For merge blocks, pass the stack top as argument
                            if merge_blocks.contains_key(&offset) {
                                let stack_top = codegen.peek().unwrap_or_else(|_| {
                                    codegen.builder.ins().iconst(types::I64, 0)
                                });
                                codegen.builder.ins().jump(block, &[BlockArg::Value(stack_top)]);
                            } else {
                                codegen.builder.ins().jump(block, &[]);
                            }
                        }
                        codegen.builder.switch_to_block(block);
                        codegen.builder.seal_block(block);
                        codegen.clear_terminated();

                        // For merge blocks, use the block parameter as the stack value
                        if merge_blocks.contains_key(&offset) {
                            // Clear the simulated stack and push the block parameter
                            codegen.clear_stack();
                            let param = codegen.builder.block_params(block)[0];
                            codegen.push(param)?;
                        }
                    }
                }

                let Some(op) = chunk.read_opcode(offset) else {
                    return Err(JitError::InvalidOpcode(code[offset]));
                };

                self.translate_opcode(&mut codegen, chunk, op, offset, &offset_to_block, &merge_blocks)?;

                // Advance to next instruction
                offset += 1 + op.immediate_size();
            }

            // Ensure function ends with return
            if !codegen.is_terminated() {
                let zero = codegen.builder.ins().iconst(types::I64, 0);
                codegen.builder.ins().return_(&[zero]);
            }
            // codegen is dropped here, releasing the borrow on builder
        }

        // Finalize the function
        builder.finalize();
        Ok(())
    }

    /// Translate a single opcode to Cranelift IR
    #[cfg(feature = "jit")]
    fn translate_opcode<'a, 'b>(
        &mut self,
        codegen: &mut CodegenContext<'a, 'b>,
        chunk: &BytecodeChunk,
        op: Opcode,
        offset: usize,
        offset_to_block: &HashMap<usize, Block>,
        merge_blocks: &HashMap<usize, bool>,
    ) -> JitResult<()> {
        match op {
            // =====================================================================
            // Stack Operations (delegated to handlers module)
            // =====================================================================
            Opcode::Nop | Opcode::Pop | Opcode::Dup | Opcode::Swap |
            Opcode::Rot3 | Opcode::Over | Opcode::DupN | Opcode::PopN => {
                return handlers::compile_stack_op(codegen, chunk, op, offset);
            }

            // =====================================================================
            // Value Creation - Simple (delegated to handlers module)
            // =====================================================================
            Opcode::PushNil | Opcode::PushTrue | Opcode::PushFalse |
            Opcode::PushUnit | Opcode::PushLongSmall => {
                return handlers::compile_simple_value_op(codegen, chunk, op, offset);
            }

            // =====================================================================
            // Value Creation - Runtime calls (delegated to handlers module)
            // =====================================================================
            Opcode::PushLong | Opcode::PushConstant | Opcode::PushEmpty |
            Opcode::PushAtom | Opcode::PushString | Opcode::PushVariable => {
                let mut ctx = handlers::ValueHandlerContext {
                    module: &mut self.module,
                    load_const_func_id: self.load_const_func_id,
                    push_empty_func_id: self.push_empty_func_id,
                };
                return handlers::compile_runtime_value_op(&mut ctx, codegen, chunk, op, offset);
            }

            // =====================================================================
            // Stage 14: S-Expression Operations (delegated to handlers module)
            // =====================================================================
            Opcode::GetHead | Opcode::GetTail | Opcode::GetArity | Opcode::GetElement => {
                let mut ctx = handlers::SExprHandlerContext {
                    module: &mut self.module,
                    get_head_func_id: self.get_head_func_id,
                    get_tail_func_id: self.get_tail_func_id,
                    get_arity_func_id: self.get_arity_func_id,
                    get_element_func_id: self.get_element_func_id,
                    make_sexpr_func_id: self.make_sexpr_func_id,
                    cons_atom_func_id: self.cons_atom_func_id,
                    make_list_func_id: self.make_list_func_id,
                    make_quote_func_id: self.make_quote_func_id,
                };
                return handlers::compile_sexpr_access_op(&mut ctx, codegen, chunk, op, offset);
            }

            // =====================================================================
            // Phase 1: Type Operations
            // =====================================================================
            Opcode::GetType => {
                // Get type name of value via runtime call
                let val = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.get_type_func_id, codegen.builder.func);

                // Call jit_runtime_get_type(ctx, val, ip)
                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, val, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::CheckType | Opcode::IsType => {
                // Check if value's type matches expected type via runtime call
                // Stack: [value, type_atom] -> [bool]
                let type_atom = codegen.pop()?;
                let val = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.check_type_func_id, codegen.builder.func);

                // Call jit_runtime_check_type(ctx, val, type_atom, ip)
                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, val, type_atom, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            // =====================================================================
            // Phase J: Type Assertion
            // =====================================================================
            Opcode::AssertType => {
                // Assert that value's type matches expected type via runtime call
                // Stack: [value, type_atom] -> [value] (value stays on stack, type_atom is consumed)
                // On type mismatch, runtime signals bailout error
                let type_atom = codegen.pop()?;
                let val = codegen.peek()?; // Peek - value stays on stack

                let func_ref = self
                    .module
                    .declare_func_in_func(self.assert_type_func_id, codegen.builder.func);

                // Call jit_runtime_assert_type(ctx, val, type_atom, ip)
                // Returns the value unchanged (bailout signaled on mismatch)
                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let _call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, val, type_atom, ip_val]);
                // Note: We don't use the return value since peek already left value on stack
                // The runtime function signals bailout on error, but returns the value
            }

            // =====================================================================
            // Phase 2a: S-Expression Creation Operations (delegated to handlers module)
            // =====================================================================

            Opcode::MakeSExpr
            | Opcode::MakeSExprLarge
            | Opcode::ConsAtom
            | Opcode::MakeList
            | Opcode::MakeQuote => {
                let mut ctx = handlers::SExprHandlerContext {
                    module: &mut self.module,
                    get_head_func_id: self.get_head_func_id,
                    get_tail_func_id: self.get_tail_func_id,
                    get_arity_func_id: self.get_arity_func_id,
                    get_element_func_id: self.get_element_func_id,
                    make_sexpr_func_id: self.make_sexpr_func_id,
                    cons_atom_func_id: self.cons_atom_func_id,
                    make_list_func_id: self.make_list_func_id,
                    make_quote_func_id: self.make_quote_func_id,
                };
                return handlers::compile_sexpr_create_op(&mut ctx, codegen, chunk, op, offset);
            }

            // =====================================================================
            // Phase 2b: Value Creation (PushUri)
            // =====================================================================

            Opcode::PushUri => {
                // Load URI from constant pool (same as PushConstant)
                // Stack: [] -> [uri]
                let index = chunk.read_u16(offset + 1).unwrap_or(0);

                let func_ref = self
                    .module
                    .declare_func_in_func(self.push_uri_func_id, codegen.builder.func);

                // Call jit_runtime_push_uri(ctx, index)
                let ctx_ptr = codegen.ctx_ptr();
                let index_val = codegen.builder.ins().iconst(types::I64, index as i64);
                let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, index_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            // =====================================================================
            // Phase 3: Call/TailCall Operations
            // =====================================================================

            Opcode::Call => {
                // Call: head_index:u16 arity:u8
                // Stack: [arg1, arg2, ..., argN] -> [result]
                let head_index = chunk.read_u16(offset + 1).unwrap_or(0);
                let arity = chunk.code()[offset + 3] as usize;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.call_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let head_index_val = codegen.builder.ins().iconst(types::I64, head_index as i64);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let arity_val = codegen.builder.ins().iconst(types::I64, arity as i64);

                if arity > 0 {
                    // Allocate a stack slot for the arguments
                    let args_slot = codegen.builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        (arity * 8) as u32, // 8 bytes per JitValue
                        8,
                    ));

                    // Pop arguments and store in stack slot (they're in stack order)
                    for i in (0..arity).rev() {
                        let arg = codegen.pop()?;
                        let slot_offset = (i * 8) as i32;
                        codegen.builder.ins().stack_store(arg, args_slot, slot_offset);
                    }

                    // Get pointer to arguments array
                    let args_ptr = codegen.builder.ins().stack_addr(types::I64, args_slot, 0);

                    // Call jit_runtime_call(ctx, head_index, args_ptr, arity, ip)
                    let call_inst = codegen.builder.ins().call(
                        func_ref,
                        &[ctx_ptr, head_index_val, args_ptr, arity_val, ip_val],
                    );
                    let result = codegen.builder.inst_results(call_inst)[0];
                    codegen.push(result)?;
                } else {
                    // No args - pass null pointer
                    let null_ptr = codegen.builder.ins().iconst(types::I64, 0);
                    let call_inst = codegen.builder.ins().call(
                        func_ref,
                        &[ctx_ptr, head_index_val, null_ptr, arity_val, ip_val],
                    );
                    let result = codegen.builder.inst_results(call_inst)[0];
                    codegen.push(result)?;
                }

                // Note: Bailout is always set by jit_runtime_call, so the result
                // is the call expression for the VM to dispatch.
                // The caller should check ctx.bailout after JIT returns.
            }

            Opcode::TailCall => {
                // TailCall: head_index:u16 arity:u8
                // Same as Call but signals TCO to VM
                let head_index = chunk.read_u16(offset + 1).unwrap_or(0);
                let arity = chunk.code()[offset + 3] as usize;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.tail_call_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let head_index_val = codegen.builder.ins().iconst(types::I64, head_index as i64);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let arity_val = codegen.builder.ins().iconst(types::I64, arity as i64);

                if arity > 0 {
                    let args_slot = codegen.builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        (arity * 8) as u32,
                        8,
                    ));

                    for i in (0..arity).rev() {
                        let arg = codegen.pop()?;
                        let slot_offset = (i * 8) as i32;
                        codegen.builder.ins().stack_store(arg, args_slot, slot_offset);
                    }

                    let args_ptr = codegen.builder.ins().stack_addr(types::I64, args_slot, 0);
                    let call_inst = codegen.builder.ins().call(
                        func_ref,
                        &[ctx_ptr, head_index_val, args_ptr, arity_val, ip_val],
                    );
                    let result = codegen.builder.inst_results(call_inst)[0];
                    codegen.push(result)?;
                } else {
                    let null_ptr = codegen.builder.ins().iconst(types::I64, 0);
                    let call_inst = codegen.builder.ins().call(
                        func_ref,
                        &[ctx_ptr, head_index_val, null_ptr, arity_val, ip_val],
                    );
                    let result = codegen.builder.inst_results(call_inst)[0];
                    codegen.push(result)?;
                }
            }

            Opcode::CallN => {
                // CallN: arity:u8
                // Stack: [head, arg1, arg2, ..., argN] -> [result]
                // Unlike Call, head is on stack not in constant pool
                let arity = chunk.code()[offset + 1] as usize;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.call_n_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let arity_val = codegen.builder.ins().iconst(types::I64, arity as i64);

                if arity > 0 {
                    // Allocate a stack slot for the arguments
                    let args_slot = codegen.builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        (arity * 8) as u32, // 8 bytes per JitValue
                        8,
                    ));

                    // Pop arguments in reverse order and store in stack slot
                    for i in (0..arity).rev() {
                        let arg = codegen.pop()?;
                        let slot_offset = (i * 8) as i32;
                        codegen.builder.ins().stack_store(arg, args_slot, slot_offset);
                    }

                    // Pop head value (it's below the args on stack)
                    let head_val = codegen.pop()?;

                    // Get pointer to arguments array
                    let args_ptr = codegen.builder.ins().stack_addr(types::I64, args_slot, 0);

                    // Call jit_runtime_call_n(ctx, head_val, args_ptr, arity, ip)
                    let call_inst = codegen.builder.ins().call(
                        func_ref,
                        &[ctx_ptr, head_val, args_ptr, arity_val, ip_val],
                    );
                    let result = codegen.builder.inst_results(call_inst)[0];
                    codegen.push(result)?;
                } else {
                    // No args - just pop head
                    let head_val = codegen.pop()?;
                    let null_ptr = codegen.builder.ins().iconst(types::I64, 0);
                    let call_inst = codegen.builder.ins().call(
                        func_ref,
                        &[ctx_ptr, head_val, null_ptr, arity_val, ip_val],
                    );
                    let result = codegen.builder.inst_results(call_inst)[0];
                    codegen.push(result)?;
                }
            }

            Opcode::TailCallN => {
                // TailCallN: arity:u8
                // Stack: [head, arg1, arg2, ..., argN] -> [result]
                // Same as CallN but signals TCO to VM
                let arity = chunk.code()[offset + 1] as usize;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.tail_call_n_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let arity_val = codegen.builder.ins().iconst(types::I64, arity as i64);

                if arity > 0 {
                    let args_slot = codegen.builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        (arity * 8) as u32,
                        8,
                    ));

                    for i in (0..arity).rev() {
                        let arg = codegen.pop()?;
                        let slot_offset = (i * 8) as i32;
                        codegen.builder.ins().stack_store(arg, args_slot, slot_offset);
                    }

                    // Pop head value
                    let head_val = codegen.pop()?;

                    let args_ptr = codegen.builder.ins().stack_addr(types::I64, args_slot, 0);
                    let call_inst = codegen.builder.ins().call(
                        func_ref,
                        &[ctx_ptr, head_val, args_ptr, arity_val, ip_val],
                    );
                    let result = codegen.builder.inst_results(call_inst)[0];
                    codegen.push(result)?;
                } else {
                    let head_val = codegen.pop()?;
                    let null_ptr = codegen.builder.ins().iconst(types::I64, 0);
                    let call_inst = codegen.builder.ins().call(
                        func_ref,
                        &[ctx_ptr, head_val, null_ptr, arity_val, ip_val],
                    );
                    let result = codegen.builder.inst_results(call_inst)[0];
                    codegen.push(result)?;
                }
            }

            // =====================================================================
            // Phase 4: Fork/Yield/Collect Operations
            // =====================================================================

            Opcode::Fork => {
                // Fork: count:u16 (followed by count u16 indices in bytecode)
                // Stack: [] -> [first_alternative]
                // Stage 2 JIT: Use native fork which creates choice points without bailout
                let count = chunk.read_u16(offset + 1).unwrap_or(0) as usize;

                // Use fork_native instead of fork to avoid bailing to VM
                let func_ref = self
                    .module
                    .declare_func_in_func(self.fork_native_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let count_val = codegen.builder.ins().iconst(types::I64, count as i64);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);

                if count > 0 {
                    // Allocate stack slot for indices array
                    let indices_slot = codegen.builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        (count * 8) as u32, // 8 bytes per u64
                        8,
                    ));

                    // Read indices from bytecode and store in slot
                    for i in 0..count {
                        // Each index is at offset + 3 + (i * 2)
                        let idx = chunk.read_u16(offset + 3 + (i * 2)).unwrap_or(0);
                        let idx_val = codegen.builder.ins().iconst(types::I64, idx as i64);
                        let slot_offset = (i * 8) as i32;
                        codegen.builder.ins().stack_store(idx_val, indices_slot, slot_offset);
                    }

                    // Get pointer to indices array
                    let indices_ptr = codegen.builder.ins().stack_addr(types::I64, indices_slot, 0);

                    // Call jit_runtime_fork_native(ctx, count, indices_ptr, ip)
                    let call_inst = codegen.builder.ins().call(
                        func_ref,
                        &[ctx_ptr, count_val, indices_ptr, ip_val],
                    );
                    let result = codegen.builder.inst_results(call_inst)[0];
                    codegen.push(result)?;
                } else {
                    // No alternatives - pass null pointer
                    let null_ptr = codegen.builder.ins().iconst(types::I64, 0);
                    let call_inst = codegen.builder.ins().call(
                        func_ref,
                        &[ctx_ptr, count_val, null_ptr, ip_val],
                    );
                    let result = codegen.builder.inst_results(call_inst)[0];
                    codegen.push(result)?;
                }
            }

            Opcode::Yield => {
                // Stage 2 JIT: Yield stores result and returns signal to dispatcher
                // Stack: [value] -> []
                // Returns: JIT_SIGNAL_YIELD to signal dispatcher
                let value = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.yield_native_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);

                // Call jit_runtime_yield_native(ctx, value, ip) -> signal
                let call_inst = codegen.builder.ins().call(
                    func_ref,
                    &[ctx_ptr, value, ip_val],
                );
                let signal = codegen.builder.inst_results(call_inst)[0];

                // Return the signal to dispatcher (JIT_SIGNAL_YIELD = 2)
                // Dispatcher will handle backtracking and re-entry
                codegen.builder.ins().return_(&[signal]);
            }

            Opcode::Collect => {
                // Stage 2 JIT: Collect gathers all yielded results into SExpr
                // Stack: [] -> [SExpr of results]
                // Note: chunk_index is ignored in native version - results stored in ctx.results
                let _chunk_index = chunk.read_u16(offset + 1).unwrap_or(0);

                let func_ref = self
                    .module
                    .declare_func_in_func(self.collect_native_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();

                // Call jit_runtime_collect_native(ctx) -> NaN-boxed SExpr
                let call_inst = codegen.builder.ins().call(
                    func_ref,
                    &[ctx_ptr],
                );
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            // =====================================================================
            // Arithmetic Operations (delegated to handlers module)
            // =====================================================================
            Opcode::Add | Opcode::Sub | Opcode::Mul | Opcode::Div |
            Opcode::Mod | Opcode::Neg | Opcode::Abs | Opcode::FloorDiv => {
                return handlers::compile_simple_arithmetic_op(codegen, op, offset);
            }

            Opcode::Pow => {
                let mut ctx = handlers::ArithmeticHandlerContext {
                    module: &mut self.module,
                    pow_func_id: self.pow_func_id,
                };
                return handlers::compile_pow(&mut ctx, codegen);
            }

            // =====================================================================
            // Extended Math Operations (delegated to handlers module)
            // =====================================================================

            Opcode::Sqrt
            | Opcode::Log
            | Opcode::Trunc
            | Opcode::Ceil
            | Opcode::FloorMath
            | Opcode::Round
            | Opcode::Sin
            | Opcode::Cos
            | Opcode::Tan
            | Opcode::Asin
            | Opcode::Acos
            | Opcode::Atan
            | Opcode::IsNan
            | Opcode::IsInf => {
                let mut ctx = handlers::MathHandlerContext {
                    module: &mut self.module,
                    sqrt_func_id: self.sqrt_func_id,
                    log_func_id: self.log_func_id,
                    trunc_func_id: self.trunc_func_id,
                    ceil_func_id: self.ceil_func_id,
                    floor_math_func_id: self.floor_math_func_id,
                    round_func_id: self.round_func_id,
                    sin_func_id: self.sin_func_id,
                    cos_func_id: self.cos_func_id,
                    tan_func_id: self.tan_func_id,
                    asin_func_id: self.asin_func_id,
                    acos_func_id: self.acos_func_id,
                    atan_func_id: self.atan_func_id,
                    isnan_func_id: self.isnan_func_id,
                    isinf_func_id: self.isinf_func_id,
                };
                return handlers::compile_extended_math_op(&mut ctx, codegen, op);
            }

            // =====================================================================
            // Expression Manipulation Operations (PR #63)
            // =====================================================================

            Opcode::IndexAtom => {
                // index-atom: [expr, index] -> [element]
                let index = codegen.pop()?;
                let expr = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.index_atom_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);

                let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, expr, index, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::MinAtom => {
                // min-atom: [expr] -> [min value]
                let expr = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.min_atom_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);

                let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, expr, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::MaxAtom => {
                // max-atom: [expr] -> [max value]
                let expr = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.max_atom_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);

                let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, expr, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            // =====================================================================
            // Boolean Operations (delegated to handlers module)
            // =====================================================================
            Opcode::And | Opcode::Or | Opcode::Not | Opcode::Xor => {
                return handlers::compile_boolean_op(codegen, op, offset);
            }

            // =====================================================================
            // Comparison Operations (delegated to handlers module)
            // =====================================================================
            Opcode::Lt | Opcode::Le | Opcode::Gt | Opcode::Ge |
            Opcode::Eq | Opcode::Ne | Opcode::StructEq => {
                return handlers::compile_comparison_op(codegen, op, offset);
            }

            // =====================================================================
            // Control Flow
            // =====================================================================
            Opcode::Return => {
                // Return top of stack or 0
                let result = codegen.pop().unwrap_or_else(|_| {
                    codegen.builder.ins().iconst(types::I64, 0)
                });
                codegen.builder.ins().return_(&[result]);
                codegen.mark_terminated();
            }

            // =====================================================================
            // Stage 3: Jump Instructions
            // Jump offsets are relative to the IP AFTER reading the instruction.
            // =====================================================================
            Opcode::Jump => {
                // Unconditional jump with 2-byte signed offset
                let instr_size = 1 + op.immediate_size();
                let next_ip = offset + instr_size;
                let rel_offset = chunk.read_i16(offset + 1).unwrap_or(0);
                let target = (next_ip as isize + rel_offset as isize) as usize;

                if let Some(&target_block) = offset_to_block.get(&target) {
                    // For merge blocks, pass the stack top as argument
                    if merge_blocks.contains_key(&target) {
                        let stack_top = codegen.peek().unwrap_or_else(|_| {
                            codegen.builder.ins().iconst(types::I64, 0)
                        });
                        codegen.builder.ins().jump(target_block, &[BlockArg::Value(stack_top)]);
                    } else {
                        codegen.builder.ins().jump(target_block, &[]);
                    }
                    codegen.mark_terminated();
                } else {
                    return Err(JitError::CompilationError(format!(
                        "Jump target {} not found in block map (offset={}, next_ip={}, rel={})",
                        target, offset, next_ip, rel_offset
                    )));
                }
            }

            Opcode::JumpIfFalse => {
                // Conditional jump if top of stack is false
                let cond = codegen.pop()?;
                let instr_size = 1 + op.immediate_size();
                let next_ip = offset + instr_size;
                let rel_offset = chunk.read_i16(offset + 1).unwrap_or(0);
                let target = (next_ip as isize + rel_offset as isize) as usize;

                // Extract bool value (assumes already bool type)
                let cond_val = codegen.extract_bool(cond);
                let cond_i8 = codegen.builder.ins().ireduce(types::I8, cond_val);

                // Get stack value for merge blocks
                let stack_top = codegen.peek().unwrap_or_else(|_| {
                    codegen.builder.ins().iconst(types::I64, 0)
                });

                // Prepare arguments for each branch based on whether target is a merge block
                let target_is_merge = merge_blocks.contains_key(&target);
                let fallthrough_is_merge = merge_blocks.contains_key(&next_ip);

                if let (Some(&target_block), Some(&fallthrough_block)) =
                    (offset_to_block.get(&target), offset_to_block.get(&next_ip))
                {
                    // brif branches to first block if cond is true, second if false
                    // We want to jump to target if false, so: true -> fallthrough, false -> target
                    let fallthrough_args: &[BlockArg] = if fallthrough_is_merge {
                        &[BlockArg::Value(stack_top)]
                    } else {
                        &[]
                    };
                    let target_args: &[BlockArg] = if target_is_merge {
                        &[BlockArg::Value(stack_top)]
                    } else {
                        &[]
                    };
                    codegen.builder.ins().brif(cond_i8, fallthrough_block, fallthrough_args, target_block, target_args);
                    codegen.mark_terminated();
                } else if let Some(&target_block) = offset_to_block.get(&target) {
                    // Fallthrough is just the next instruction, no block needed
                    let cont_block = codegen.builder.create_block();
                    let target_args: &[BlockArg] = if target_is_merge {
                        &[BlockArg::Value(stack_top)]
                    } else {
                        &[]
                    };
                    codegen.builder.ins().brif(cond_i8, cont_block, &[], target_block, target_args);
                    codegen.builder.switch_to_block(cont_block);
                    codegen.builder.seal_block(cont_block);
                } else {
                    return Err(JitError::CompilationError(format!(
                        "JumpIfFalse target {} not found in block map",
                        target
                    )));
                }
            }

            Opcode::JumpIfTrue => {
                // Conditional jump if top of stack is true
                let cond = codegen.pop()?;
                let instr_size = 1 + op.immediate_size();
                let next_ip = offset + instr_size;
                let rel_offset = chunk.read_i16(offset + 1).unwrap_or(0);
                let target = (next_ip as isize + rel_offset as isize) as usize;

                let cond_val = codegen.extract_bool(cond);
                let cond_i8 = codegen.builder.ins().ireduce(types::I8, cond_val);

                // Get stack value for merge blocks
                let stack_top = codegen.peek().unwrap_or_else(|_| {
                    codegen.builder.ins().iconst(types::I64, 0)
                });

                // Prepare arguments for each branch based on whether target is a merge block
                let target_is_merge = merge_blocks.contains_key(&target);
                let fallthrough_is_merge = merge_blocks.contains_key(&next_ip);

                if let (Some(&target_block), Some(&fallthrough_block)) =
                    (offset_to_block.get(&target), offset_to_block.get(&next_ip))
                {
                    // brif branches to first block if cond is true
                    let target_args: &[BlockArg] = if target_is_merge {
                        &[BlockArg::Value(stack_top)]
                    } else {
                        &[]
                    };
                    let fallthrough_args: &[BlockArg] = if fallthrough_is_merge {
                        &[BlockArg::Value(stack_top)]
                    } else {
                        &[]
                    };
                    codegen.builder.ins().brif(cond_i8, target_block, target_args, fallthrough_block, fallthrough_args);
                    codegen.mark_terminated();
                } else if let Some(&target_block) = offset_to_block.get(&target) {
                    let cont_block = codegen.builder.create_block();
                    let target_args: &[BlockArg] = if target_is_merge {
                        &[BlockArg::Value(stack_top)]
                    } else {
                        &[]
                    };
                    codegen.builder.ins().brif(cond_i8, target_block, target_args, cont_block, &[]);
                    codegen.builder.switch_to_block(cont_block);
                    codegen.builder.seal_block(cont_block);
                } else {
                    return Err(JitError::CompilationError(format!(
                        "JumpIfTrue target {} not found in block map",
                        target
                    )));
                }
            }

            Opcode::JumpShort => {
                // Unconditional jump with 1-byte signed offset
                let instr_size = 1 + op.immediate_size();
                let next_ip = offset + instr_size;
                let rel_offset = chunk.read_byte(offset + 1).unwrap_or(0) as i8;
                let target = (next_ip as isize + rel_offset as isize) as usize;

                if let Some(&target_block) = offset_to_block.get(&target) {
                    // For merge blocks, pass the stack top as argument
                    if merge_blocks.contains_key(&target) {
                        let stack_top = codegen.peek().unwrap_or_else(|_| {
                            codegen.builder.ins().iconst(types::I64, 0)
                        });
                        codegen.builder.ins().jump(target_block, &[BlockArg::Value(stack_top)]);
                    } else {
                        codegen.builder.ins().jump(target_block, &[]);
                    }
                    codegen.mark_terminated();
                } else {
                    return Err(JitError::CompilationError(format!(
                        "JumpShort target {} not found in block map",
                        target
                    )));
                }
            }

            Opcode::JumpIfFalseShort => {
                // Conditional jump if false with 1-byte offset
                let cond = codegen.pop()?;
                let instr_size = 1 + op.immediate_size();
                let next_ip = offset + instr_size;
                let rel_offset = chunk.read_byte(offset + 1).unwrap_or(0) as i8;
                let target = (next_ip as isize + rel_offset as isize) as usize;

                let cond_val = codegen.extract_bool(cond);
                let cond_i8 = codegen.builder.ins().ireduce(types::I8, cond_val);

                // Get stack value for merge blocks
                let stack_top = codegen.peek().unwrap_or_else(|_| {
                    codegen.builder.ins().iconst(types::I64, 0)
                });

                // Prepare arguments for each branch based on whether target is a merge block
                let target_is_merge = merge_blocks.contains_key(&target);
                let fallthrough_is_merge = merge_blocks.contains_key(&next_ip);

                if let (Some(&target_block), Some(&fallthrough_block)) =
                    (offset_to_block.get(&target), offset_to_block.get(&next_ip))
                {
                    let fallthrough_args: &[BlockArg] = if fallthrough_is_merge {
                        &[BlockArg::Value(stack_top)]
                    } else {
                        &[]
                    };
                    let target_args: &[BlockArg] = if target_is_merge {
                        &[BlockArg::Value(stack_top)]
                    } else {
                        &[]
                    };
                    codegen.builder.ins().brif(cond_i8, fallthrough_block, fallthrough_args, target_block, target_args);
                    codegen.mark_terminated();
                } else if let Some(&target_block) = offset_to_block.get(&target) {
                    let cont_block = codegen.builder.create_block();
                    let target_args: &[BlockArg] = if target_is_merge {
                        &[BlockArg::Value(stack_top)]
                    } else {
                        &[]
                    };
                    codegen.builder.ins().brif(cond_i8, cont_block, &[], target_block, target_args);
                    codegen.builder.switch_to_block(cont_block);
                    codegen.builder.seal_block(cont_block);
                } else {
                    return Err(JitError::CompilationError(format!(
                        "JumpIfFalseShort target {} not found in block map",
                        target
                    )));
                }
            }

            Opcode::JumpIfTrueShort => {
                // Conditional jump if true with 1-byte offset
                let cond = codegen.pop()?;
                let instr_size = 1 + op.immediate_size();
                let next_ip = offset + instr_size;
                let rel_offset = chunk.read_byte(offset + 1).unwrap_or(0) as i8;
                let target = (next_ip as isize + rel_offset as isize) as usize;

                let cond_val = codegen.extract_bool(cond);
                let cond_i8 = codegen.builder.ins().ireduce(types::I8, cond_val);

                // Get stack value for merge blocks
                let stack_top = codegen.peek().unwrap_or_else(|_| {
                    codegen.builder.ins().iconst(types::I64, 0)
                });

                // Prepare arguments for each branch based on whether target is a merge block
                let target_is_merge = merge_blocks.contains_key(&target);
                let fallthrough_is_merge = merge_blocks.contains_key(&next_ip);

                if let (Some(&target_block), Some(&fallthrough_block)) =
                    (offset_to_block.get(&target), offset_to_block.get(&next_ip))
                {
                    let target_args: &[BlockArg] = if target_is_merge {
                        &[BlockArg::Value(stack_top)]
                    } else {
                        &[]
                    };
                    let fallthrough_args: &[BlockArg] = if fallthrough_is_merge {
                        &[BlockArg::Value(stack_top)]
                    } else {
                        &[]
                    };
                    codegen.builder.ins().brif(cond_i8, target_block, target_args, fallthrough_block, fallthrough_args);
                    codegen.mark_terminated();
                } else if let Some(&target_block) = offset_to_block.get(&target) {
                    let cont_block = codegen.builder.create_block();
                    let target_args: &[BlockArg] = if target_is_merge {
                        &[BlockArg::Value(stack_top)]
                    } else {
                        &[]
                    };
                    codegen.builder.ins().brif(cond_i8, target_block, target_args, cont_block, &[]);
                    codegen.builder.switch_to_block(cont_block);
                    codegen.builder.seal_block(cont_block);
                } else {
                    return Err(JitError::CompilationError(format!(
                        "JumpIfTrueShort target {} not found in block map",
                        target
                    )));
                }
            }

            // =====================================================================
            // Stage 5: More Jump Types
            // =====================================================================
            Opcode::JumpIfNil => {
                // Conditional jump if top of stack is nil (pops the value)
                let val = codegen.pop()?;
                let instr_size = 1 + op.immediate_size();
                let next_ip = offset + instr_size;
                let rel_offset = chunk.read_i16(offset + 1).unwrap_or(0);
                let target = (next_ip as isize + rel_offset as isize) as usize;

                // Check if value is nil: tag == TAG_NIL
                // icmp returns i8 (0 or 1), suitable for brif
                let tag = codegen.extract_tag(val);
                let nil_tag = codegen.builder.ins().iconst(types::I64, TAG_NIL as i64);
                let cond_i8 = codegen.builder.ins().icmp(IntCC::Equal, tag, nil_tag);

                // Get stack value for merge blocks
                let stack_top = codegen.peek().unwrap_or_else(|_| {
                    codegen.builder.ins().iconst(types::I64, 0)
                });

                // Prepare arguments for each branch based on whether target is a merge block
                let target_is_merge = merge_blocks.contains_key(&target);
                let fallthrough_is_merge = merge_blocks.contains_key(&next_ip);

                if let (Some(&target_block), Some(&fallthrough_block)) =
                    (offset_to_block.get(&target), offset_to_block.get(&next_ip))
                {
                    // brif branches to first block if cond is true (is_nil)
                    let target_args: &[BlockArg] = if target_is_merge {
                        &[BlockArg::Value(stack_top)]
                    } else {
                        &[]
                    };
                    let fallthrough_args: &[BlockArg] = if fallthrough_is_merge {
                        &[BlockArg::Value(stack_top)]
                    } else {
                        &[]
                    };
                    codegen.builder.ins().brif(cond_i8, target_block, target_args, fallthrough_block, fallthrough_args);
                    codegen.mark_terminated();
                } else if let Some(&target_block) = offset_to_block.get(&target) {
                    let cont_block = codegen.builder.create_block();
                    let target_args: &[BlockArg] = if target_is_merge {
                        &[BlockArg::Value(stack_top)]
                    } else {
                        &[]
                    };
                    codegen.builder.ins().brif(cond_i8, target_block, target_args, cont_block, &[]);
                    codegen.builder.switch_to_block(cont_block);
                    codegen.builder.seal_block(cont_block);
                } else {
                    return Err(JitError::CompilationError(format!(
                        "JumpIfNil target {} not found in block map",
                        target
                    )));
                }
            }

            Opcode::JumpIfError => {
                // Conditional jump if top of stack is error (peeks - does NOT pop)
                let val = codegen.peek()?;
                let instr_size = 1 + op.immediate_size();
                let next_ip = offset + instr_size;
                let rel_offset = chunk.read_i16(offset + 1).unwrap_or(0);
                let target = (next_ip as isize + rel_offset as isize) as usize;

                // Check if value is error: tag == TAG_ERROR
                // icmp returns i8 (0 or 1), suitable for brif
                let tag = codegen.extract_tag(val);
                let error_tag = codegen.builder.ins().iconst(types::I64, TAG_ERROR as i64);
                let cond_i8 = codegen.builder.ins().icmp(IntCC::Equal, tag, error_tag);

                // Get stack value for merge blocks (use val since we didn't pop)
                let stack_top = val;

                // Prepare arguments for each branch based on whether target is a merge block
                let target_is_merge = merge_blocks.contains_key(&target);
                let fallthrough_is_merge = merge_blocks.contains_key(&next_ip);

                if let (Some(&target_block), Some(&fallthrough_block)) =
                    (offset_to_block.get(&target), offset_to_block.get(&next_ip))
                {
                    // brif branches to first block if cond is true (is_error)
                    let target_args: &[BlockArg] = if target_is_merge {
                        &[BlockArg::Value(stack_top)]
                    } else {
                        &[]
                    };
                    let fallthrough_args: &[BlockArg] = if fallthrough_is_merge {
                        &[BlockArg::Value(stack_top)]
                    } else {
                        &[]
                    };
                    codegen.builder.ins().brif(cond_i8, target_block, target_args, fallthrough_block, fallthrough_args);
                    codegen.mark_terminated();
                } else if let Some(&target_block) = offset_to_block.get(&target) {
                    let cont_block = codegen.builder.create_block();
                    let target_args: &[BlockArg] = if target_is_merge {
                        &[BlockArg::Value(stack_top)]
                    } else {
                        &[]
                    };
                    codegen.builder.ins().brif(cond_i8, target_block, target_args, cont_block, &[]);
                    codegen.builder.switch_to_block(cont_block);
                    codegen.builder.seal_block(cont_block);
                } else {
                    return Err(JitError::CompilationError(format!(
                        "JumpIfError target {} not found in block map",
                        target
                    )));
                }
            }

            // =====================================================================
            // Stage 4: Local Variables (delegated to handlers module)
            // =====================================================================
            Opcode::LoadLocal | Opcode::StoreLocal |
            Opcode::LoadLocalWide | Opcode::StoreLocalWide => {
                return handlers::compile_local_op(codegen, chunk, op, offset);
            }

            // =====================================================================
            // Stage 6: Type Predicates (delegated to handlers module)
            // =====================================================================
            Opcode::IsVariable | Opcode::IsSExpr | Opcode::IsSymbol => {
                return handlers::compile_type_predicate_op(codegen, op);
            }

            // =====================================================================
            // Phase A: Binding Operations
            // =====================================================================
            Opcode::LoadBinding => {
                // Load binding by name index via runtime call
                // Stack: [] -> [value]
                let name_idx = chunk.read_u16(offset + 1).unwrap_or(0);

                let func_ref = self
                    .module
                    .declare_func_in_func(self.load_binding_func_id, codegen.builder.func);

                // Call jit_runtime_load_binding(ctx, name_idx, ip)
                let ctx_ptr = codegen.ctx_ptr();
                let name_idx_val = codegen.builder.ins().iconst(types::I64, name_idx as i64);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, name_idx_val, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::StoreBinding => {
                // Store binding by name index via runtime call
                // Stack: [value] -> []
                let name_idx = chunk.read_u16(offset + 1).unwrap_or(0);

                let value = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.store_binding_func_id, codegen.builder.func);

                // Call jit_runtime_store_binding(ctx, name_idx, value, ip)
                let ctx_ptr = codegen.ctx_ptr();
                let name_idx_val = codegen.builder.ins().iconst(types::I64, name_idx as i64);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let _call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, name_idx_val, value, ip_val]);
                // Result is status code, ignored for now
            }

            Opcode::HasBinding => {
                // Check if binding exists by name index via runtime call
                // Stack: [] -> [bool]
                let name_idx = chunk.read_u16(offset + 1).unwrap_or(0);

                let func_ref = self
                    .module
                    .declare_func_in_func(self.has_binding_func_id, codegen.builder.func);

                // Call jit_runtime_has_binding(ctx, name_idx)
                let ctx_ptr = codegen.ctx_ptr();
                let name_idx_val = codegen.builder.ins().iconst(types::I64, name_idx as i64);
                let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, name_idx_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::ClearBindings => {
                // Clear all bindings via runtime call
                // Stack: [] -> []
                let func_ref = self
                    .module
                    .declare_func_in_func(self.clear_bindings_func_id, codegen.builder.func);

                // Call jit_runtime_clear_bindings(ctx)
                let ctx_ptr = codegen.ctx_ptr();
                let _call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr]);
            }

            Opcode::PushBindingFrame => {
                // Push new binding frame via runtime call
                // Stack: [] -> []
                let func_ref = self
                    .module
                    .declare_func_in_func(self.push_binding_frame_func_id, codegen.builder.func);

                // Call jit_runtime_push_binding_frame(ctx)
                let ctx_ptr = codegen.ctx_ptr();
                let _call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr]);
                // Result is status code, ignored for now
            }

            Opcode::PopBindingFrame => {
                // Pop binding frame via runtime call
                // Stack: [] -> []
                let func_ref = self
                    .module
                    .declare_func_in_func(self.pop_binding_frame_func_id, codegen.builder.func);

                // Call jit_runtime_pop_binding_frame(ctx)
                let ctx_ptr = codegen.ctx_ptr();
                let _call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr]);
                // Result is status code, ignored for now
            }

            // =====================================================================
            // Phase B: Pattern Matching Operations
            // =====================================================================
            Opcode::Match => {
                // Pattern match without binding
                // Stack: [pattern, value] -> [bool]
                let value = codegen.pop()?;
                let pattern = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.pattern_match_func_id, codegen.builder.func);

                // Call jit_runtime_pattern_match(ctx, pattern, value, ip)
                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, pattern, value, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::MatchBind => {
                // Pattern match with variable binding
                // Stack: [pattern, value] -> [bool]
                let value = codegen.pop()?;
                let pattern = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.pattern_match_bind_func_id, codegen.builder.func);

                // Call jit_runtime_pattern_match_bind(ctx, pattern, value, ip)
                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, pattern, value, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::MatchHead => {
                // Match head symbol of S-expression
                // Stack: [expr] -> [bool]
                // Operand: 1-byte index into constant pool for expected head symbol
                let expected_head_idx = chunk.read_byte(offset + 1).unwrap_or(0);
                let expr = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.match_head_func_id, codegen.builder.func);

                // Call jit_runtime_match_head(ctx, expr, expected_head_idx, ip)
                let ctx_ptr = codegen.ctx_ptr();
                let head_idx_val = codegen
                    .builder
                    .ins()
                    .iconst(types::I64, expected_head_idx as i64);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, expr, head_idx_val, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::MatchArity => {
                // Check if S-expression has expected arity
                // Stack: [expr] -> [bool]
                // Operand: 1-byte expected arity
                let expected_arity = chunk.read_byte(offset + 1).unwrap_or(0);
                let expr = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.match_arity_func_id, codegen.builder.func);

                // Call jit_runtime_match_arity(ctx, expr, expected_arity, ip)
                let ctx_ptr = codegen.ctx_ptr();
                let arity_val = codegen
                    .builder
                    .ins()
                    .iconst(types::I64, expected_arity as i64);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, expr, arity_val, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::MatchGuard => {
                // Match with guard condition
                // Stack: [pattern, value, guard] -> [bool]
                // Operand: 2-byte guard chunk index
                // For now, we treat this similarly to MatchBind but with guard evaluation
                // The guard expression is evaluated after match succeeds
                let _guard_idx = chunk.read_u16(offset + 1).unwrap_or(0);
                let guard = codegen.pop()?;
                let value = codegen.pop()?;
                let pattern = codegen.pop()?;

                // First do the match
                let func_ref = self
                    .module
                    .declare_func_in_func(self.pattern_match_bind_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, pattern, value, ip_val]);
                let match_result = codegen.builder.inst_results(call_inst)[0];

                // AND the match result with the guard value
                // Both are NaN-boxed bools, so we need to check if both are TAG_BOOL_TRUE
                let true_val = codegen.const_bool(true);
                let false_val = codegen.const_bool(false);
                let match_is_true = codegen.builder.ins().icmp(IntCC::Equal, match_result, true_val);
                let guard_is_true = codegen.builder.ins().icmp(IntCC::Equal, guard, true_val);
                let both_true = codegen.builder.ins().band(match_is_true, guard_is_true);
                let result = codegen.builder.ins().select(both_true, true_val, false_val);
                codegen.push(result)?;
            }

            Opcode::Unify => {
                // Unify two values (bidirectional pattern matching)
                // Stack: [a, b] -> [bool]
                let b = codegen.pop()?;
                let a = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.unify_func_id, codegen.builder.func);

                // Call jit_runtime_unify(ctx, a, b, ip)
                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, a, b, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::UnifyBind => {
                // Unify two values with variable binding
                // Stack: [a, b] -> [bool]
                let b = codegen.pop()?;
                let a = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.unify_bind_func_id, codegen.builder.func);

                // Call jit_runtime_unify_bind(ctx, a, b, ip)
                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, a, b, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            // =================================================================
            // Phase D: Space Operations
            // =================================================================

            Opcode::SpaceAdd => {
                // Add atom to space
                // Stack: [space, atom] -> [bool]
                let atom = codegen.pop()?;
                let space = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.space_add_func_id, codegen.builder.func);

                // Call jit_runtime_space_add(ctx, space, atom, ip)
                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, space, atom, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::SpaceRemove => {
                // Remove atom from space
                // Stack: [space, atom] -> [bool]
                let atom = codegen.pop()?;
                let space = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.space_remove_func_id, codegen.builder.func);

                // Call jit_runtime_space_remove(ctx, space, atom, ip)
                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, space, atom, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::SpaceGetAtoms => {
                // Get all atoms from space
                // Stack: [space] -> [list]
                let space = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.space_get_atoms_func_id, codegen.builder.func);

                // Call jit_runtime_space_get_atoms(ctx, space, ip)
                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, space, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::SpaceMatch => {
                // Match pattern in space
                // Stack: [space, pattern, template] -> [results]
                let template = codegen.pop()?;
                let pattern = codegen.pop()?;
                let space = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.space_match_func_id, codegen.builder.func);

                // Call jit_runtime_space_match_nondet(ctx, space, pattern, template, ip)
                // Uses nondeterministic semantics with choice points for multiple matches
                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, space, pattern, template, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            // =================================================================
            // Phase D.1: State Operations
            // =================================================================

            Opcode::NewState => {
                // Create a new mutable state cell
                // Stack: [initial_value] -> [state_handle]
                let initial_value = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.new_state_func_id, codegen.builder.func);

                // Call jit_runtime_new_state(ctx, initial_value, ip)
                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, initial_value, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::GetState => {
                // Get current value from a state cell
                // Stack: [state_handle] -> [value]
                let state_handle = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.get_state_func_id, codegen.builder.func);

                // Call jit_runtime_get_state(ctx, state_handle, ip)
                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, state_handle, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::ChangeState => {
                // Change value of a state cell
                // Stack: [state_handle, new_value] -> [state_handle]
                let new_value = codegen.pop()?;
                let state_handle = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.change_state_func_id, codegen.builder.func);

                // Call jit_runtime_change_state(ctx, state_handle, new_value, ip)
                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, state_handle, new_value, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            // =================================================================
            // Phase C: Rule Dispatch Operations
            // =================================================================

            Opcode::DispatchRules => {
                // Dispatch rules for an expression
                // Stack: [expr] -> [count]
                let expr = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.dispatch_rules_func_id, codegen.builder.func);

                // Call jit_runtime_dispatch_rules(ctx, expr, ip)
                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, expr, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::TryRule => {
                // Try a single rule
                // Stack: [] -> [result] (using rule_idx from operand)
                let rule_idx = chunk.read_u16(offset + 1).unwrap_or(0) as i64;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.try_rule_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let rule_idx_val = codegen.builder.ins().iconst(types::I64, rule_idx);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, rule_idx_val, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::NextRule => {
                // Advance to next matching rule
                // Stack: [] -> [] (returns status)
                let func_ref = self
                    .module
                    .declare_func_in_func(self.next_rule_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let _call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, ip_val]);
                // Result is status, not pushed to stack
            }

            Opcode::CommitRule => {
                // Commit to current rule (cut)
                // Stack: [] -> []
                let func_ref = self
                    .module
                    .declare_func_in_func(self.commit_rule_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let _call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, ip_val]);
            }

            Opcode::FailRule => {
                // Signal explicit rule failure
                // Stack: [] -> [] (signals backtracking)
                let func_ref = self
                    .module
                    .declare_func_in_func(self.fail_rule_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let _call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, ip_val]);
                // Result is signal, caller handles backtracking
            }

            Opcode::LookupRules => {
                // Look up rules by head symbol
                // Stack: [] -> [count]
                let head_idx = chunk.read_u16(offset + 1).unwrap_or(0) as i64;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.lookup_rules_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let head_idx_val = codegen.builder.ins().iconst(types::I64, head_idx);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, head_idx_val, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::ApplySubst => {
                // Apply substitution to an expression
                // Stack: [expr] -> [result]
                let expr = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.apply_subst_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, expr, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::DefineRule => {
                // Define a new rule
                // Stack: [pattern, body] -> [Unit]
                let _body = codegen.pop()?;
                let _pattern = codegen.pop()?;
                let pattern_idx = chunk.read_u16(offset + 1).unwrap_or(0) as i64;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.define_rule_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let pattern_idx_val = codegen.builder.ins().iconst(types::I64, pattern_idx);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, pattern_idx_val, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            // =================================================================
            // Phase E: Special Forms (via runtime calls)
            // =================================================================

            Opcode::EvalIf => {
                // Stack: [condition, then_val, else_val] -> [result]
                // Native implementation using Cranelift select instruction.
                //
                // Semantics: Only TAG_BOOL_FALSE and TAG_NIL are falsy.
                // Everything else (including TAG_BOOL_TRUE, integers, heap values) is truthy.
                let else_val = codegen.pop()?;
                let then_val = codegen.pop()?;
                let condition = codegen.pop()?;

                // Check for falsy values: TAG_BOOL_FALSE (TAG_BOOL | 0) or TAG_NIL
                let tag_bool_false = codegen.const_bool(false);
                let tag_nil = codegen.const_nil();

                // is_false = (condition == TAG_BOOL_FALSE)
                let is_false =
                    codegen
                        .builder
                        .ins()
                        .icmp(IntCC::Equal, condition, tag_bool_false);

                // is_nil = (condition == TAG_NIL)
                let is_nil = codegen
                    .builder
                    .ins()
                    .icmp(IntCC::Equal, condition, tag_nil);

                // is_falsy = is_false || is_nil
                let is_falsy = codegen.builder.ins().bor(is_false, is_nil);

                // result = is_falsy ? else_val : then_val
                let result = codegen
                    .builder
                    .ins()
                    .select(is_falsy, else_val, then_val);
                codegen.push(result)?;
            }

            Opcode::EvalLet => {
                // Stack: [value] -> [Unit], name_idx from operand
                // Native implementation: call store_binding directly and return Unit inline.
                //
                // This avoids the wrapper function jit_runtime_eval_let which just calls
                // store_binding and returns Unit.
                let value = codegen.pop()?;
                let name_idx = chunk.read_u16(offset + 1).unwrap_or(0) as i64;

                // Call jit_runtime_store_binding(ctx, name_idx, value, ip)
                let func_ref = self
                    .module
                    .declare_func_in_func(self.store_binding_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let name_idx_val = codegen.builder.ins().iconst(types::I64, name_idx);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);

                // Store binding returns status (ignored), we always push Unit
                codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, name_idx_val, value, ip_val]);

                // Push Unit result (inline, no function call needed)
                let unit_val = codegen.const_unit();
                codegen.push(unit_val)?;
            }

            Opcode::EvalLetStar => {
                // Let* bindings are handled sequentially by the bytecode compiler.
                // This opcode is a marker/placeholder that just returns Unit.
                //
                // Native implementation: Just push Unit directly (no function call needed).
                let unit_val = codegen.const_unit();
                codegen.push(unit_val)?;
            }

            Opcode::EvalMatch => {
                // Stack: [value, pattern] -> [bool]
                // Native implementation: call pattern_match directly instead of wrapper.
                //
                // The eval_match wrapper just delegates to pattern_match, so we call
                // pattern_match directly to avoid the extra indirection.
                let pattern = codegen.pop()?;
                let value = codegen.pop()?;

                // Call jit_runtime_pattern_match(ctx, pattern, value, ip)
                let func_ref = self
                    .module
                    .declare_func_in_func(self.pattern_match_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, pattern, value, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::EvalCase => {
                // Stack: [value] -> [case_index], case_count from operand
                // Case dispatch is complex (loops over patterns, installs bindings),
                // so we keep it as a runtime call.
                let value = codegen.pop()?;
                let case_count = chunk.read_u16(offset + 1).unwrap_or(0) as i64;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.eval_case_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let case_count_val = codegen.builder.ins().iconst(types::I64, case_count);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, value, case_count_val, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::EvalChain => {
                // Stack: [first, second] -> [second]
                // Native implementation: Just discard first, keep second.
                //
                // Chain (;) evaluates both but only returns the second result.
                // The first value is already evaluated, so we just drop it.
                let second = codegen.pop()?;
                let _first = codegen.pop()?; // Discard first value
                codegen.push(second)?;
            }

            Opcode::EvalQuote => {
                // Stack: [expr] -> [quoted]
                let expr = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.eval_quote_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, expr, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::EvalUnquote => {
                // Stack: [quoted] -> [result]
                let expr = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.eval_unquote_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, expr, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::EvalEval => {
                // Stack: [expr] -> [result]
                let expr = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.eval_eval_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, expr, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::EvalBind => {
                // Stack: [value] -> [Unit], name_idx from operand
                // Native implementation: call store_binding directly and return Unit inline.
                //
                // Same optimization as EvalLet - avoid the wrapper function.
                let value = codegen.pop()?;
                let name_idx = chunk.read_u16(offset + 1).unwrap_or(0) as i64;

                // Call jit_runtime_store_binding(ctx, name_idx, value, ip)
                let func_ref = self
                    .module
                    .declare_func_in_func(self.store_binding_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let name_idx_val = codegen.builder.ins().iconst(types::I64, name_idx);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);

                // Store binding returns status (ignored), we always push Unit
                codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, name_idx_val, value, ip_val]);

                // Push Unit result (inline, no function call needed)
                let unit_val = codegen.const_unit();
                codegen.push(unit_val)?;
            }

            Opcode::EvalNew => {
                // Stack: [] -> [space]
                let func_ref = self
                    .module
                    .declare_func_in_func(self.eval_new_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::EvalCollapse => {
                // Stack: [expr] -> [list]
                let expr = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.eval_collapse_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, expr, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::EvalSuperpose => {
                // Stack: [list] -> [choice]
                let list = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.eval_superpose_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, list, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::EvalMemo => {
                // Stack: [expr] -> [result]
                let expr = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.eval_memo_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, expr, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::EvalMemoFirst => {
                // Stack: [expr] -> [result]
                let expr = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.eval_memo_first_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, expr, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::EvalPragma => {
                // Stack: [directive] -> [Unit]
                let directive = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.eval_pragma_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, directive, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::EvalFunction => {
                // Stack: [] -> [Unit], name_idx and param_count from operands
                let name_idx = chunk.read_u16(offset + 1).unwrap_or(0) as i64;
                let param_count = chunk.read_byte(offset + 3).unwrap_or(0) as i64;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.eval_function_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let name_idx_val = codegen.builder.ins().iconst(types::I64, name_idx);
                let param_count_val = codegen.builder.ins().iconst(types::I64, param_count);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen.builder.ins().call(
                    func_ref,
                    &[ctx_ptr, name_idx_val, param_count_val, ip_val],
                );
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::EvalLambda => {
                // Stack: [] -> [closure], param_count from operand
                let param_count = chunk.read_byte(offset + 1).unwrap_or(0) as i64;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.eval_lambda_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let param_count_val = codegen.builder.ins().iconst(types::I64, param_count);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, param_count_val, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::EvalApply => {
                // Stack: [closure] -> [result], arg_count from operand
                let closure = codegen.pop()?;
                let arg_count = chunk.read_byte(offset + 1).unwrap_or(0) as i64;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.eval_apply_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let arg_count_val = codegen.builder.ins().iconst(types::I64, arg_count);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, closure, arg_count_val, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            // =====================================================================
            // Phase G: Advanced Nondeterminism (via runtime calls)
            // =====================================================================

            Opcode::Cut => {
                // Stack: [] -> [Unit] - prune all choice points
                let func_ref = self
                    .module
                    .declare_func_in_func(self.cut_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::Guard => {
                // Stack: [bool] -> [] - backtrack if false
                let condition = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.guard_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, condition, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];

                // If result is 0, return FAIL signal
                let zero = codegen.builder.ins().iconst(types::I64, 0);
                let is_fail = codegen.builder.ins().icmp(IntCC::Equal, result, zero);

                let fail_block = codegen.builder.create_block();
                let cont_block = codegen.builder.create_block();

                codegen.builder.ins().brif(is_fail, fail_block, &[], cont_block, &[]);

                // Fail block - return FAIL signal
                codegen.builder.switch_to_block(fail_block);
                codegen.builder.seal_block(fail_block);
                let fail_signal = codegen.builder.ins().iconst(types::I64, super::JIT_SIGNAL_FAIL);
                codegen.builder.ins().return_(&[fail_signal]);

                // Continue block
                codegen.builder.switch_to_block(cont_block);
                codegen.builder.seal_block(cont_block);
            }

            Opcode::Amb => {
                // Stack: [alt1, alt2, ..., altN] -> [selected]
                // alt_count from operand
                let alt_count = chunk.read_byte(offset + 1).unwrap_or(0) as i64;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.amb_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let alt_count_val = codegen.builder.ins().iconst(types::I64, alt_count);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, alt_count_val, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::Commit => {
                // Stack: [] -> [Unit] - remove N choice points
                let count = chunk.read_byte(offset + 1).unwrap_or(0) as i64;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.commit_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let count_val = codegen.builder.ins().iconst(types::I64, count);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, count_val, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::Backtrack => {
                // Stack: [] -> [] - force immediate backtracking
                let func_ref = self
                    .module
                    .declare_func_in_func(self.backtrack_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, ip_val]);
                let signal = codegen.builder.inst_results(call_inst)[0];
                // Return the FAIL signal
                codegen.builder.ins().return_(&[signal]);
            }

            // =====================================================================
            // Phase F: Advanced Calls (via runtime calls)
            // =====================================================================

            Opcode::CallNative => {
                // Stack: [args...] -> [result]
                // func_id: u16, arity: u8
                let func_id = chunk.read_u16(offset + 1).unwrap_or(0) as i64;
                let arity = chunk.read_byte(offset + 3).unwrap_or(0) as i64;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.call_native_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let func_id_val = codegen.builder.ins().iconst(types::I64, func_id);
                let arity_val = codegen.builder.ins().iconst(types::I64, arity);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, func_id_val, arity_val, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::CallExternal => {
                // Stack: [args...] -> [result]
                // name_idx: u16, arity: u8
                let name_idx = chunk.read_u16(offset + 1).unwrap_or(0) as i64;
                let arity = chunk.read_byte(offset + 3).unwrap_or(0) as i64;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.call_external_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let name_idx_val = codegen.builder.ins().iconst(types::I64, name_idx);
                let arity_val = codegen.builder.ins().iconst(types::I64, arity);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, name_idx_val, arity_val, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::CallCached => {
                // Stack: [args...] -> [result]
                // head_idx: u16, arity: u8
                let head_idx = chunk.read_u16(offset + 1).unwrap_or(0) as i64;
                let arity = chunk.read_byte(offset + 3).unwrap_or(0) as i64;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.call_cached_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let head_idx_val = codegen.builder.ins().iconst(types::I64, head_idx);
                let arity_val = codegen.builder.ins().iconst(types::I64, arity);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, head_idx_val, arity_val, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            // =====================================================================
            // Phase H: MORK Bridge (via runtime calls)
            // =====================================================================

            Opcode::MorkLookup => {
                // Stack: [path] -> [value] - lookup value at MORK path
                let path = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.mork_lookup_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, path, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::MorkMatch => {
                // Stack: [path, pattern] -> [results] - match pattern at MORK path
                let pattern = codegen.pop()?;
                let path = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.mork_match_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, path, pattern, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::MorkInsert => {
                // Stack: [path, value] -> [bool] - insert value at MORK path
                let value = codegen.pop()?;
                let path = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.mork_insert_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, path, value, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::MorkDelete => {
                // Stack: [path] -> [bool] - delete value at MORK path
                let path = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.mork_delete_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, path, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            // =====================================================================
            // Phase I: Debug/Meta (via runtime calls)
            // =====================================================================

            Opcode::Trace => {
                // Stack: [value] -> [] - emit trace event
                // msg_idx from operand
                let value = codegen.pop()?;
                let msg_idx = chunk.read_u16(offset + 1).unwrap_or(0) as i64;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.trace_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let msg_idx_val = codegen.builder.ins().iconst(types::I64, msg_idx);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                // Trace has no return value, just call it
                codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, msg_idx_val, value, ip_val]);
            }

            Opcode::Breakpoint => {
                // Stack: [] -> [] - debugger breakpoint
                // bp_id from operand
                let bp_id = chunk.read_u16(offset + 1).unwrap_or(0) as i64;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.breakpoint_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let bp_id_val = codegen.builder.ins().iconst(types::I64, bp_id);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen
                    .builder
                    .ins()
                    .call(func_ref, &[ctx_ptr, bp_id_val, ip_val]);
                // Result is -1 to pause, 0 to continue (currently ignored)
                let _result = codegen.builder.inst_results(call_inst)[0];
                // For now, we just continue - pause handling would require bailout
            }

            // =====================================================================
            // Phase 1.1: Core Nondeterminism Markers
            // =====================================================================

            Opcode::Fail => {
                // Stack: [] -> [] - trigger immediate backtracking
                // Simply return the FAIL signal - semantically identical to Backtrack
                let signal = codegen.builder.ins().iconst(types::I64, super::JIT_SIGNAL_FAIL);
                codegen.builder.ins().return_(&[signal]);
            }

            Opcode::BeginNondet => {
                // Stack: [] -> [] - mark start of nondeterministic section
                // Increment fork_depth in JitContext
                let func_ref = self
                    .module
                    .declare_func_in_func(self.begin_nondet_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                codegen.builder.ins().call(func_ref, &[ctx_ptr, ip_val]);
                // No return value, just a marker
            }

            Opcode::EndNondet => {
                // Stack: [] -> [] - mark end of nondeterministic section
                // Decrement fork_depth in JitContext
                let func_ref = self
                    .module
                    .declare_func_in_func(self.end_nondet_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                codegen.builder.ins().call(func_ref, &[ctx_ptr, ip_val]);
                // No return value, just a marker
            }

            // =====================================================================
            // Phase 1.3: Multi-value Return
            // =====================================================================

            Opcode::ReturnMulti => {
                // Stack: [values...] -> signal - return all values on stack
                // Get count from stack height (or use 0 to return all above base)
                let func_ref = self
                    .module
                    .declare_func_in_func(self.return_multi_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let count = codegen.builder.ins().iconst(types::I64, 0); // 0 = return all
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, count, ip_val]);
                let signal = codegen.builder.inst_results(inst)[0];
                codegen.builder.ins().return_(&[signal]);
            }

            Opcode::CollectN => {
                // Stack: [] -> [sexpr] - collect up to N results from nondeterminism
                let max_count = chunk.code().get(offset + 1).copied().unwrap_or(0) as i64;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.collect_n_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let max_val = codegen.builder.ins().iconst(types::I64, max_count);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, max_val, ip_val]);
                let result = codegen.builder.inst_results(inst)[0];
                codegen.push(result)?;
            }

            // =====================================================================
            // Phase 1.4: Multi-way Branch (JumpTable) - Native Switch
            // =====================================================================

            Opcode::JumpTable => {
                // Stack: [selector_hash] -> [] - jump to offset based on hash match
                // JumpTable uses 2 bytes for table index
                let table_index = chunk.read_u16(offset + 1).unwrap_or(0) as usize;

                // Get the jump table from chunk
                let jump_table = match chunk.get_jump_table(table_index) {
                    Some(jt) => jt.clone(),
                    None => {
                        // No table found - bail to VM
                        let _selector = codegen.pop()?;
                        let signal = codegen.builder.ins().iconst(types::I64, super::JIT_SIGNAL_BAILOUT);
                        codegen.builder.ins().return_(&[signal]);
                        codegen.mark_terminated();
                        return Ok(());
                    }
                };

                // Pop the selector value (hash to match against entries)
                let selector = codegen.pop()?;

                // Get default block
                let default_block = match offset_to_block.get(&jump_table.default_offset) {
                    Some(&block) => block,
                    None => {
                        // Default not found - bail
                        let signal = codegen.builder.ins().iconst(types::I64, super::JIT_SIGNAL_BAILOUT);
                        codegen.builder.ins().return_(&[signal]);
                        codegen.mark_terminated();
                        return Ok(());
                    }
                };

                if jump_table.entries.is_empty() {
                    // No entries - just jump to default
                    codegen.builder.ins().jump(default_block, &[]);
                    codegen.mark_terminated();
                } else {
                    // Use Cranelift's Switch which handles both dense and sparse cases efficiently
                    // It automatically chooses between br_table, binary search, or linear scan
                    let mut switch = Switch::new();

                    for (hash, target_offset) in &jump_table.entries {
                        let target_block = match offset_to_block.get(target_offset) {
                            Some(&block) => block,
                            None => default_block,
                        };
                        // Switch uses u128 keys, convert hash
                        switch.set_entry(*hash as u128, target_block);
                    }

                    // Emit the switch - this generates optimal code (br_table for dense,
                    // binary search for sparse, etc.)
                    switch.emit(codegen.builder, selector, default_block);
                    codegen.mark_terminated();
                }
            }

            // =====================================================================
            // Phase 1.5: Global/Space Access
            // =====================================================================

            Opcode::LoadGlobal => {
                // Stack: [] -> [value] - load global variable by symbol index
                // Read 2 bytes as u16 (little-endian)
                let code = chunk.code();
                let b0 = code.get(offset + 1).copied().unwrap_or(0) as u16;
                let b1 = code.get(offset + 2).copied().unwrap_or(0) as u16;
                let symbol_idx = (b1 << 8 | b0) as i64;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.load_global_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let idx_val = codegen.builder.ins().iconst(types::I64, symbol_idx);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, idx_val, ip_val]);
                let result = codegen.builder.inst_results(inst)[0];
                codegen.push(result)?;
            }

            Opcode::StoreGlobal => {
                // Stack: [value] -> [] - store global variable by symbol index
                let code = chunk.code();
                let b0 = code.get(offset + 1).copied().unwrap_or(0) as u16;
                let b1 = code.get(offset + 2).copied().unwrap_or(0) as u16;
                let symbol_idx = (b1 << 8 | b0) as i64;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.store_global_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let value = codegen.pop()?;
                let idx_val = codegen.builder.ins().iconst(types::I64, symbol_idx);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                codegen.builder.ins().call(func_ref, &[ctx_ptr, idx_val, value, ip_val]);
                // Result is Unit, but we don't push it (store is side-effect only)
            }

            Opcode::LoadSpace => {
                // Stack: [] -> [space_handle] - load space by name index
                let code = chunk.code();
                let b0 = code.get(offset + 1).copied().unwrap_or(0) as u16;
                let b1 = code.get(offset + 2).copied().unwrap_or(0) as u16;
                let name_idx = (b1 << 8 | b0) as i64;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.load_space_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let idx_val = codegen.builder.ins().iconst(types::I64, name_idx);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, idx_val, ip_val]);
                let result = codegen.builder.inst_results(inst)[0];
                codegen.push(result)?;
            }

            // =====================================================================
            // Phase 1.6: Closure Support
            // =====================================================================

            Opcode::LoadUpvalue => {
                // Stack: [] -> [value] - load from enclosing scope
                // Operand: 2 bytes (depth: u8, index: u8)
                let code = chunk.code();
                let depth = code.get(offset + 1).copied().unwrap_or(0) as i64;
                let index = code.get(offset + 2).copied().unwrap_or(0) as i64;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.load_upvalue_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let depth_val = codegen.builder.ins().iconst(types::I64, depth);
                let index_val = codegen.builder.ins().iconst(types::I64, index);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, depth_val, index_val, ip_val]);
                let result = codegen.builder.inst_results(inst)[0];
                codegen.push(result)?;
            }

            // =====================================================================
            // Phase 1.7: Atom Operations
            // =====================================================================

            Opcode::DeconAtom => {
                // Stack: [expr] -> [(head, tail)] - deconstruct S-expression
                let func_ref = self
                    .module
                    .declare_func_in_func(self.decon_atom_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let value = codegen.pop()?;
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, value, ip_val]);
                let result = codegen.builder.inst_results(inst)[0];
                codegen.push(result)?;
            }

            Opcode::Repr => {
                // Stack: [value] -> [string] - get string representation
                let func_ref = self
                    .module
                    .declare_func_in_func(self.repr_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let value = codegen.pop()?;
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, value, ip_val]);
                let result = codegen.builder.inst_results(inst)[0];
                codegen.push(result)?;
            }

            // =====================================================================
            // Phase 1.8: Higher-Order Operations (bailout to VM)
            // =====================================================================

            Opcode::MapAtom => {
                // Stack: [list] -> [result] - map function over list
                // These require executing nested bytecode, so bailout to VM
                let code = chunk.code();
                let b0 = code.get(offset + 1).copied().unwrap_or(0) as u16;
                let b1 = code.get(offset + 2).copied().unwrap_or(0) as u16;
                let chunk_idx = (b1 << 8 | b0) as i64;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.map_atom_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let list = codegen.pop()?;
                let chunk_val = codegen.builder.ins().iconst(types::I64, chunk_idx);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, list, chunk_val, ip_val]);
                let result = codegen.builder.inst_results(inst)[0];
                codegen.push(result)?;
            }

            Opcode::FilterAtom => {
                // Stack: [list] -> [result] - filter list by predicate
                let code = chunk.code();
                let b0 = code.get(offset + 1).copied().unwrap_or(0) as u16;
                let b1 = code.get(offset + 2).copied().unwrap_or(0) as u16;
                let chunk_idx = (b1 << 8 | b0) as i64;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.filter_atom_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let list = codegen.pop()?;
                let chunk_val = codegen.builder.ins().iconst(types::I64, chunk_idx);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, list, chunk_val, ip_val]);
                let result = codegen.builder.inst_results(inst)[0];
                codegen.push(result)?;
            }

            Opcode::FoldlAtom => {
                // Stack: [list, init] -> [result] - left fold over list
                let code = chunk.code();
                let b0 = code.get(offset + 1).copied().unwrap_or(0) as u16;
                let b1 = code.get(offset + 2).copied().unwrap_or(0) as u16;
                let chunk_idx = (b1 << 8 | b0) as i64;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.foldl_atom_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let init = codegen.pop()?;
                let list = codegen.pop()?;
                let chunk_val = codegen.builder.ins().iconst(types::I64, chunk_idx);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, list, init, chunk_val, ip_val]);
                let result = codegen.builder.inst_results(inst)[0];
                codegen.push(result)?;
            }

            // =====================================================================
            // Phase 1.9: Meta-Type Operations
            // =====================================================================

            Opcode::GetMetaType => {
                // Stack: [value] -> [metatype_atom] - get meta-level type
                let func_ref = self
                    .module
                    .declare_func_in_func(self.get_metatype_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let value = codegen.pop()?;
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, value, ip_val]);
                let result = codegen.builder.inst_results(inst)[0];
                codegen.push(result)?;
            }

            // =====================================================================
            // Phase 1.10: MORK and Debug
            // =====================================================================

            Opcode::BloomCheck => {
                // Stack: [key] -> [bool] - bloom filter pre-check
                let func_ref = self
                    .module
                    .declare_func_in_func(self.bloom_check_func_id, codegen.builder.func);

                let ctx_ptr = codegen.ctx_ptr();
                let key = codegen.pop()?;
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, key, ip_val]);
                let result = codegen.builder.inst_results(inst)[0];
                codegen.push(result)?;
            }

            Opcode::Halt => {
                // Stack: [] -> signal - halt execution
                let signal = codegen.builder.ins().iconst(types::I64, super::JIT_SIGNAL_HALT);
                codegen.builder.ins().return_(&[signal]);
                codegen.mark_terminated();
            }

            // =====================================================================
            // Not Stage 1-8 + Phase A-I + Phase 1.1-1.10 compilable - should not reach here
            // =====================================================================
            _ => {
                return Err(JitError::InvalidOpcode(op.to_byte()));
            }
        }

        Ok(())
    }

    /// Get code size statistics
    #[cfg(feature = "jit")]
    pub fn code_size(&self) -> usize {
        // Note: Cranelift doesn't expose this directly, would need tracking
        0
    }

    #[cfg(not(feature = "jit"))]
    pub fn code_size(&self) -> usize {
        0
    }
}

impl Default for JitCompiler {
    fn default() -> Self {
        Self::new().expect("Failed to create JIT compiler")
    }
}

#[cfg(test)]
#[path = "compiler_tests.rs"]
mod compiler_tests;
