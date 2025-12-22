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
use super::types::{JitError, JitResult, TAG_NIL, TAG_ERROR, TAG_ATOM, TAG_VAR, TAG_HEAP, TAG_BOOL};
use crate::backend::bytecode::{BytecodeChunk, Opcode};
#[cfg(feature = "jit")]
use std::collections::HashMap;

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
        eprintln!("Generated IR:\n{}", ctx.func.display());

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
            // Stack Operations
            // =====================================================================
            Opcode::Nop => {
                // No operation
            }

            Opcode::Pop => {
                // Handle scope cleanup: if stack is empty, the value being popped
                // was a local stored via StoreLocal (in JIT's separate locals storage)
                if codegen.stack_depth() > 0 {
                    codegen.pop()?;
                }
                // If stack is empty, this is a no-op (the "local" isn't on our stack)
            }

            Opcode::Dup => {
                let val = codegen.peek()?;
                codegen.push(val)?;
            }

            Opcode::Swap => {
                // Handle scope cleanup pattern: when StoreLocal stores values
                // to separate local slots, subsequent Swap has nothing to swap with.
                if codegen.stack_depth() >= 2 {
                    let a = codegen.pop()?;
                    let b = codegen.pop()?;
                    codegen.push(a)?;
                    codegen.push(b)?;
                }
                // If stack_depth < 2, this is a no-op (scope cleanup for JIT-stored locals)
            }

            Opcode::Rot3 => {
                // [a, b, c] -> [c, a, b] (VM semantics)
                let c = codegen.pop()?;
                let b = codegen.pop()?;
                let a = codegen.pop()?;
                codegen.push(c)?;
                codegen.push(a)?;
                codegen.push(b)?;
            }

            Opcode::Over => {
                // (a b -- a b a)
                let b = codegen.pop()?;
                let a = codegen.peek()?;
                codegen.push(b)?;
                codegen.push(a)?;
            }

            Opcode::DupN => {
                // Read operand from bytecode
                let n = chunk.read_byte(offset + 1).unwrap_or(0) as usize;
                let mut vals = Vec::with_capacity(n);
                for _ in 0..n {
                    vals.push(codegen.pop()?);
                }
                vals.reverse();
                // Push original values
                for &v in &vals {
                    codegen.push(v)?;
                }
                // Push duplicates
                for &v in &vals {
                    codegen.push(v)?;
                }
            }

            Opcode::PopN => {
                let n = chunk.read_byte(offset + 1).unwrap_or(0);
                for _ in 0..n {
                    codegen.pop()?;
                }
            }

            // =====================================================================
            // Value Creation
            // =====================================================================
            Opcode::PushNil => {
                let nil = codegen.const_nil();
                codegen.push(nil)?;
            }

            Opcode::PushTrue => {
                let t = codegen.const_bool(true);
                codegen.push(t)?;
            }

            Opcode::PushFalse => {
                let f = codegen.const_bool(false);
                codegen.push(f)?;
            }

            Opcode::PushUnit => {
                let unit = codegen.const_unit();
                codegen.push(unit)?;
            }

            Opcode::PushLongSmall => {
                let n = chunk.read_byte(offset + 1).unwrap_or(0) as i8;
                let val = codegen.const_long(n as i64);
                codegen.push(val)?;
            }

            Opcode::PushLong => {
                // Stage 2: Load large integer from constant pool via runtime call
                let idx = chunk.read_u16(offset + 1).unwrap_or(0) as i64;

                // Import the load_constant function into this function's context
                let func_ref = self
                    .module
                    .declare_func_in_func(self.load_const_func_id, codegen.builder.func);

                // Call jit_runtime_load_constant(ctx, index)
                let ctx_ptr = codegen.ctx_ptr();
                let idx_val = codegen.builder.ins().iconst(types::I64, idx);
                let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, idx_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::PushConstant => {
                // Stage 2: Load generic constant via runtime call
                let idx = chunk.read_u16(offset + 1).unwrap_or(0) as i64;

                // Import the load_constant function into this function's context
                let func_ref = self
                    .module
                    .declare_func_in_func(self.load_const_func_id, codegen.builder.func);

                // Call jit_runtime_load_constant(ctx, index)
                let ctx_ptr = codegen.ctx_ptr();
                let idx_val = codegen.builder.ins().iconst(types::I64, idx);
                let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, idx_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            // =====================================================================
            // Stage 13: Value Creation Opcodes
            // =====================================================================
            Opcode::PushEmpty => {
                // Create empty S-expression via runtime call
                let func_ref = self
                    .module
                    .declare_func_in_func(self.push_empty_func_id, codegen.builder.func);

                // Call jit_runtime_push_empty()
                let call_inst = codegen.builder.ins().call(func_ref, &[]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::PushAtom | Opcode::PushString | Opcode::PushVariable => {
                // Load atom/string/variable from constant pool via runtime call
                // All three use the same load_constant function - the constant pool
                // already contains the correctly typed MettaValue
                let idx = chunk.read_u16(offset + 1).unwrap_or(0) as i64;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.load_const_func_id, codegen.builder.func);

                // Call jit_runtime_load_constant(ctx, index)
                let ctx_ptr = codegen.ctx_ptr();
                let idx_val = codegen.builder.ins().iconst(types::I64, idx);
                let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, idx_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            // =====================================================================
            // Stage 14: S-Expression Operations
            // =====================================================================
            Opcode::GetHead => {
                // Get first element of S-expression via runtime call
                let val = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.get_head_func_id, codegen.builder.func);

                // Call jit_runtime_get_head(ctx, val, ip)
                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, val, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::GetTail => {
                // Get tail (all but first) of S-expression via runtime call
                let val = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.get_tail_func_id, codegen.builder.func);

                // Call jit_runtime_get_tail(ctx, val, ip)
                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, val, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::GetArity => {
                // Get arity (element count) of S-expression via runtime call
                let val = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.get_arity_func_id, codegen.builder.func);

                // Call jit_runtime_get_arity(ctx, val, ip)
                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, val, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::GetElement => {
                // Stage 14b: Get element by index from S-expression via runtime call
                // Read the index from the bytecode (1-byte operand)
                let index = chunk.read_byte(offset + 1).unwrap_or(0) as i64;
                let val = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.get_element_func_id, codegen.builder.func);

                // Call jit_runtime_get_element(ctx, val, index, ip)
                let ctx_ptr = codegen.ctx_ptr();
                let index_val = codegen.builder.ins().iconst(types::I64, index);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, val, index_val, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
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
            // Phase 2a: Value Creation Operations
            // =====================================================================

            Opcode::MakeSExpr => {
                // Create S-expression from N stack values
                // Stack: [v1, v2, ..., vN] -> [sexpr]
                let arity = chunk.read_byte(offset + 1).unwrap_or(0) as usize;

                // Pop all values in reverse order (they'll be stored bottom-up)
                let mut values = Vec::with_capacity(arity);
                for _ in 0..arity {
                    values.push(codegen.pop()?);
                }
                values.reverse(); // Restore original order

                // Create a stack slot to hold the array of values
                let slot_size = (arity * 8) as u32; // 8 bytes per u64
                let slot = codegen.builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    slot_size,
                    0,
                ));

                // Store values to the stack slot
                for (i, val) in values.iter().enumerate() {
                    let slot_offset = (i * 8) as i32;
                    codegen.builder.ins().stack_store(*val, slot, slot_offset);
                }

                // Get pointer to the stack slot
                let values_ptr = codegen.builder.ins().stack_addr(types::I64, slot, 0);

                let func_ref = self
                    .module
                    .declare_func_in_func(self.make_sexpr_func_id, codegen.builder.func);

                // Call jit_runtime_make_sexpr(ctx, values_ptr, count, ip)
                let ctx_ptr = codegen.ctx_ptr();
                let count_val = codegen.builder.ins().iconst(types::I64, arity as i64);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, values_ptr, count_val, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::MakeSExprLarge => {
                // Same as MakeSExpr but with u16 arity
                // Stack: [v1, v2, ..., vN] -> [sexpr]
                let arity = chunk.read_u16(offset + 1).unwrap_or(0) as usize;

                // Pop all values in reverse order
                let mut values = Vec::with_capacity(arity);
                for _ in 0..arity {
                    values.push(codegen.pop()?);
                }
                values.reverse();

                // Create a stack slot to hold the array of values
                let slot_size = (arity * 8) as u32;
                let slot = codegen.builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    slot_size,
                    0,
                ));

                // Store values to the stack slot
                for (i, val) in values.iter().enumerate() {
                    let slot_offset = (i * 8) as i32;
                    codegen.builder.ins().stack_store(*val, slot, slot_offset);
                }

                // Get pointer to the stack slot
                let values_ptr = codegen.builder.ins().stack_addr(types::I64, slot, 0);

                let func_ref = self
                    .module
                    .declare_func_in_func(self.make_sexpr_func_id, codegen.builder.func);

                // Call jit_runtime_make_sexpr(ctx, values_ptr, count, ip)
                let ctx_ptr = codegen.ctx_ptr();
                let count_val = codegen.builder.ins().iconst(types::I64, arity as i64);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, values_ptr, count_val, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::ConsAtom => {
                // Prepend head to tail S-expression
                // Stack: [head, tail] -> [sexpr]
                let tail = codegen.pop()?;
                let head = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.cons_atom_func_id, codegen.builder.func);

                // Call jit_runtime_cons_atom(ctx, head, tail, ip)
                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, head, tail, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            // =====================================================================
            // Phase 2b: Value Creation (PushUri, MakeList, MakeQuote)
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

            Opcode::MakeList => {
                // Create proper list from N stack values
                // Stack: [v1, v2, ..., vN] -> [(Cons v1 (Cons v2 ... Nil))]
                let arity = chunk.read_byte(offset + 1).unwrap_or(0) as usize;

                // Pop all values in reverse order
                let mut values = Vec::with_capacity(arity);
                for _ in 0..arity {
                    values.push(codegen.pop()?);
                }
                values.reverse(); // Restore original order

                // Create a stack slot to hold the array of values
                let slot_size = (arity * 8).max(8) as u32; // At least 8 bytes
                let slot = codegen.builder.create_sized_stack_slot(StackSlotData::new(
                    StackSlotKind::ExplicitSlot,
                    slot_size,
                    0,
                ));

                // Store values to the stack slot
                for (i, val) in values.iter().enumerate() {
                    let slot_offset = (i * 8) as i32;
                    codegen.builder.ins().stack_store(*val, slot, slot_offset);
                }

                // Get pointer to the stack slot
                let values_ptr = codegen.builder.ins().stack_addr(types::I64, slot, 0);

                let func_ref = self
                    .module
                    .declare_func_in_func(self.make_list_func_id, codegen.builder.func);

                // Call jit_runtime_make_list(ctx, values_ptr, count, ip)
                let ctx_ptr = codegen.ctx_ptr();
                let count_val = codegen.builder.ins().iconst(types::I64, arity as i64);
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, values_ptr, count_val, ip_val]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            Opcode::MakeQuote => {
                // Wrap value in quote expression
                // Stack: [val] -> [(quote val)]
                let val = codegen.pop()?;

                let func_ref = self
                    .module
                    .declare_func_in_func(self.make_quote_func_id, codegen.builder.func);

                // Call jit_runtime_make_quote(ctx, val, ip)
                let ctx_ptr = codegen.ctx_ptr();
                let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);
                let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, val, ip_val]);
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
            // Arithmetic Operations
            // =====================================================================
            Opcode::Add => {
                let b = codegen.pop()?;
                let a = codegen.pop()?;

                // Type guards
                codegen.guard_long(a, offset)?;
                codegen.guard_long(b, offset)?;

                // Extract payloads (lower 48 bits)
                let a_val = codegen.extract_long(a);
                let b_val = codegen.extract_long(b);

                // Perform addition
                let result = codegen.builder.ins().iadd(a_val, b_val);

                // Box result as Long
                let boxed = codegen.box_long(result);
                codegen.push(boxed)?;
            }

            Opcode::Sub => {
                let b = codegen.pop()?;
                let a = codegen.pop()?;

                codegen.guard_long(a, offset)?;
                codegen.guard_long(b, offset)?;

                let a_val = codegen.extract_long(a);
                let b_val = codegen.extract_long(b);
                let result = codegen.builder.ins().isub(a_val, b_val);
                let boxed = codegen.box_long(result);
                codegen.push(boxed)?;
            }

            Opcode::Mul => {
                let b = codegen.pop()?;
                let a = codegen.pop()?;

                codegen.guard_long(a, offset)?;
                codegen.guard_long(b, offset)?;

                let a_val = codegen.extract_long(a);
                let b_val = codegen.extract_long(b);
                let result = codegen.builder.ins().imul(a_val, b_val);
                let boxed = codegen.box_long(result);
                codegen.push(boxed)?;
            }

            Opcode::Div => {
                let b = codegen.pop()?;
                let a = codegen.pop()?;

                codegen.guard_long(a, offset)?;
                codegen.guard_long(b, offset)?;

                let a_val = codegen.extract_long(a);
                let b_val = codegen.extract_long(b);

                // Guard against division by zero
                codegen.guard_nonzero(b_val, offset)?;

                let result = codegen.builder.ins().sdiv(a_val, b_val);
                let boxed = codegen.box_long(result);
                codegen.push(boxed)?;
            }

            Opcode::Mod => {
                let b = codegen.pop()?;
                let a = codegen.pop()?;

                codegen.guard_long(a, offset)?;
                codegen.guard_long(b, offset)?;

                let a_val = codegen.extract_long(a);
                let b_val = codegen.extract_long(b);

                codegen.guard_nonzero(b_val, offset)?;

                let result = codegen.builder.ins().srem(a_val, b_val);
                let boxed = codegen.box_long(result);
                codegen.push(boxed)?;
            }

            Opcode::Neg => {
                let a = codegen.pop()?;
                codegen.guard_long(a, offset)?;

                let a_val = codegen.extract_long(a);
                let result = codegen.builder.ins().ineg(a_val);
                let boxed = codegen.box_long(result);
                codegen.push(boxed)?;
            }

            Opcode::Abs => {
                let a = codegen.pop()?;
                codegen.guard_long(a, offset)?;

                let a_val = codegen.extract_long(a);

                // abs(x) = x < 0 ? -x : x
                let zero = codegen.builder.ins().iconst(types::I64, 0);
                let is_neg = codegen.builder.ins().icmp(IntCC::SignedLessThan, a_val, zero);
                let negated = codegen.builder.ins().ineg(a_val);
                let result = codegen.builder.ins().select(is_neg, negated, a_val);

                let boxed = codegen.box_long(result);
                codegen.push(boxed)?;
            }

            Opcode::FloorDiv => {
                // For integers, floor division is the same as truncated division
                let b = codegen.pop()?;
                let a = codegen.pop()?;

                codegen.guard_long(a, offset)?;
                codegen.guard_long(b, offset)?;

                let a_val = codegen.extract_long(a);
                let b_val = codegen.extract_long(b);

                codegen.guard_nonzero(b_val, offset)?;

                let result = codegen.builder.ins().sdiv(a_val, b_val);
                let boxed = codegen.box_long(result);
                codegen.push(boxed)?;
            }

            Opcode::Pow => {
                // Stage 2: Pow via runtime call
                let exp = codegen.pop()?;
                let base = codegen.pop()?;

                // Import the pow function into this function's context
                let func_ref = self
                    .module
                    .declare_func_in_func(self.pow_func_id, codegen.builder.func);

                // Call jit_runtime_pow(base, exp) - both are NaN-boxed
                let call_inst = codegen.builder.ins().call(func_ref, &[base, exp]);
                let result = codegen.builder.inst_results(call_inst)[0];
                codegen.push(result)?;
            }

            // =====================================================================
            // Boolean Operations
            // =====================================================================
            Opcode::And => {
                let b = codegen.pop()?;
                let a = codegen.pop()?;

                codegen.guard_bool(a, offset)?;
                codegen.guard_bool(b, offset)?;

                let a_val = codegen.extract_bool(a);
                let b_val = codegen.extract_bool(b);
                let result = codegen.builder.ins().band(a_val, b_val);
                let boxed = codegen.box_bool(result);
                codegen.push(boxed)?;
            }

            Opcode::Or => {
                let b = codegen.pop()?;
                let a = codegen.pop()?;

                codegen.guard_bool(a, offset)?;
                codegen.guard_bool(b, offset)?;

                let a_val = codegen.extract_bool(a);
                let b_val = codegen.extract_bool(b);
                let result = codegen.builder.ins().bor(a_val, b_val);
                let boxed = codegen.box_bool(result);
                codegen.push(boxed)?;
            }

            Opcode::Not => {
                let a = codegen.pop()?;
                codegen.guard_bool(a, offset)?;

                let a_val = codegen.extract_bool(a);
                let one = codegen.builder.ins().iconst(types::I64, 1);
                let result = codegen.builder.ins().bxor(a_val, one);
                let boxed = codegen.box_bool(result);
                codegen.push(boxed)?;
            }

            Opcode::Xor => {
                let b = codegen.pop()?;
                let a = codegen.pop()?;

                codegen.guard_bool(a, offset)?;
                codegen.guard_bool(b, offset)?;

                let a_val = codegen.extract_bool(a);
                let b_val = codegen.extract_bool(b);
                let result = codegen.builder.ins().bxor(a_val, b_val);
                let boxed = codegen.box_bool(result);
                codegen.push(boxed)?;
            }

            // =====================================================================
            // Comparison Operations
            // =====================================================================
            Opcode::Lt => {
                let b = codegen.pop()?;
                let a = codegen.pop()?;

                codegen.guard_long(a, offset)?;
                codegen.guard_long(b, offset)?;

                let a_val = codegen.extract_long(a);
                let b_val = codegen.extract_long(b);
                let cmp = codegen.builder.ins().icmp(IntCC::SignedLessThan, a_val, b_val);

                // Convert i8 comparison result to i64
                let result = codegen.builder.ins().uextend(types::I64, cmp);
                let boxed = codegen.box_bool(result);
                codegen.push(boxed)?;
            }

            Opcode::Le => {
                let b = codegen.pop()?;
                let a = codegen.pop()?;

                codegen.guard_long(a, offset)?;
                codegen.guard_long(b, offset)?;

                let a_val = codegen.extract_long(a);
                let b_val = codegen.extract_long(b);
                let cmp = codegen
                    .builder
                    .ins()
                    .icmp(IntCC::SignedLessThanOrEqual, a_val, b_val);
                let result = codegen.builder.ins().uextend(types::I64, cmp);
                let boxed = codegen.box_bool(result);
                codegen.push(boxed)?;
            }

            Opcode::Gt => {
                let b = codegen.pop()?;
                let a = codegen.pop()?;

                codegen.guard_long(a, offset)?;
                codegen.guard_long(b, offset)?;

                let a_val = codegen.extract_long(a);
                let b_val = codegen.extract_long(b);
                let cmp = codegen.builder.ins().icmp(IntCC::SignedGreaterThan, a_val, b_val);
                let result = codegen.builder.ins().uextend(types::I64, cmp);
                let boxed = codegen.box_bool(result);
                codegen.push(boxed)?;
            }

            Opcode::Ge => {
                let b = codegen.pop()?;
                let a = codegen.pop()?;

                codegen.guard_long(a, offset)?;
                codegen.guard_long(b, offset)?;

                let a_val = codegen.extract_long(a);
                let b_val = codegen.extract_long(b);
                let cmp = codegen
                    .builder
                    .ins()
                    .icmp(IntCC::SignedGreaterThanOrEqual, a_val, b_val);
                let result = codegen.builder.ins().uextend(types::I64, cmp);
                let boxed = codegen.box_bool(result);
                codegen.push(boxed)?;
            }

            Opcode::Eq => {
                let b = codegen.pop()?;
                let a = codegen.pop()?;

                // For equality, we can compare the raw bits
                // (same tag + same payload = equal)
                let cmp = codegen.builder.ins().icmp(IntCC::Equal, a, b);
                let result = codegen.builder.ins().uextend(types::I64, cmp);
                let boxed = codegen.box_bool(result);
                codegen.push(boxed)?;
            }

            Opcode::Ne => {
                let b = codegen.pop()?;
                let a = codegen.pop()?;

                let cmp = codegen.builder.ins().icmp(IntCC::NotEqual, a, b);
                let result = codegen.builder.ins().uextend(types::I64, cmp);
                let boxed = codegen.box_bool(result);
                codegen.push(boxed)?;
            }

            Opcode::StructEq => {
                // Structural equality: compare NaN-boxed values directly
                // For primitive types (Long, Bool, Nil, Unit), bit comparison is correct
                // For heap types, this compares references (deep comparison would need runtime)
                let b = codegen.pop()?;
                let a = codegen.pop()?;

                let cmp = codegen.builder.ins().icmp(IntCC::Equal, a, b);
                let result = codegen.builder.ins().uextend(types::I64, cmp);
                let boxed = codegen.box_bool(result);
                codegen.push(boxed)?;
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
            // Stage 4: Local Variables
            // =====================================================================
            Opcode::LoadLocal => {
                let index = chunk.read_byte(offset + 1).unwrap_or(0) as usize;
                codegen.load_local(index)?;
            }

            Opcode::StoreLocal => {
                let index = chunk.read_byte(offset + 1).unwrap_or(0) as usize;
                codegen.store_local(index)?;
            }

            Opcode::LoadLocalWide => {
                let index = chunk.read_u16(offset + 1).unwrap_or(0) as usize;
                codegen.load_local(index)?;
            }

            Opcode::StoreLocalWide => {
                let index = chunk.read_u16(offset + 1).unwrap_or(0) as usize;
                codegen.store_local(index)?;
            }

            // =====================================================================
            // Stage 6: Type Predicates
            // =====================================================================
            Opcode::IsVariable => {
                // Check if value is a variable (TAG_VAR)
                let val = codegen.pop()?;
                let tag = codegen.extract_tag(val);
                let var_tag = codegen.builder.ins().iconst(types::I64, TAG_VAR as i64);
                let is_var = codegen.builder.ins().icmp(IntCC::Equal, tag, var_tag);
                // icmp returns i8, extend to i64 for boxing
                let is_var_i64 = codegen.builder.ins().uextend(types::I64, is_var);
                let result = codegen.box_bool(is_var_i64);
                codegen.push(result)?;
            }

            Opcode::IsSExpr => {
                // Check if value is an S-expression (TAG_HEAP)
                let val = codegen.pop()?;
                let tag = codegen.extract_tag(val);
                let heap_tag = codegen.builder.ins().iconst(types::I64, TAG_HEAP as i64);
                let is_sexpr = codegen.builder.ins().icmp(IntCC::Equal, tag, heap_tag);
                // icmp returns i8, extend to i64 for boxing
                let is_sexpr_i64 = codegen.builder.ins().uextend(types::I64, is_sexpr);
                let result = codegen.box_bool(is_sexpr_i64);
                codegen.push(result)?;
            }

            Opcode::IsSymbol => {
                // Check if value is a symbol/atom (TAG_ATOM)
                let val = codegen.pop()?;
                let tag = codegen.extract_tag(val);
                let atom_tag = codegen.builder.ins().iconst(types::I64, TAG_ATOM as i64);
                let is_sym = codegen.builder.ins().icmp(IntCC::Equal, tag, atom_tag);
                // icmp returns i8, extend to i64 for boxing
                let is_sym_i64 = codegen.builder.ins().uextend(types::I64, is_sym);
                let result = codegen.box_bool(is_sym_i64);
                codegen.push(result)?;
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

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::bytecode::ChunkBuilder;

    #[test]
    fn test_can_compile_stage1_arithmetic() {
        let mut builder = ChunkBuilder::new("test_arith");
        builder.emit_byte(Opcode::PushLongSmall, 10);
        builder.emit_byte(Opcode::PushLongSmall, 20);
        builder.emit(Opcode::Add);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk));
    }

    #[test]
    fn test_can_compile_stage1_boolean() {
        let mut builder = ChunkBuilder::new("test_bool");
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::PushFalse);
        builder.emit(Opcode::Or);
        builder.emit(Opcode::Not);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk));
    }

    #[test]
    fn test_can_compile_calls_with_bailout() {
        // Phase 3: Call is now compilable with bailout semantics
        let mut builder = ChunkBuilder::new("test_call");
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit_call(0, 1); // head_index=0, arity=1
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        // Call IS compilable in Stage 1 (with bailout)
        assert!(JitCompiler::can_compile_stage1(&chunk));
    }

    #[test]
    fn test_can_compile_fork_with_bailout() {
        // Phase 9: Fork is detected as nondeterminism and routed to bytecode tier
        let mut builder = ChunkBuilder::new("test_nondet");
        builder.emit_byte(Opcode::Fork, 2);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        // Fork is NOT compilable - static nondeterminism detection routes to bytecode
        assert!(!JitCompiler::can_compile_stage1(&chunk), "Fork chunks should not be JIT compilable (Phase 9 optimization)");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_compile_simple_addition() {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("test_add");
        builder.emit_byte(Opcode::PushLongSmall, 10);
        builder.emit_byte(Opcode::PushLongSmall, 20);
        builder.emit(Opcode::Add);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let result = compiler.compile(&chunk);
        assert!(result.is_ok(), "Compilation failed: {:?}", result.err());
    }

    // =========================================================================
    // End-to-End JIT Execution Tests
    // =========================================================================

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_addition() {
        use crate::backend::bytecode::jit::{JitContext, JitValue, JitBailoutReason};

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Build bytecode: 10 + 20 = 30
        let mut builder = ChunkBuilder::new("e2e_add");
        builder.emit_byte(Opcode::PushLongSmall, 10);
        builder.emit_byte(Opcode::PushLongSmall, 20);
        builder.emit(Opcode::Add);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        // Compile to native code
        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        // Set up JIT context (needed for bailout signaling, constants, etc.)
        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                64,
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Execute native code - returns NaN-boxed result directly
        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        // Verify result
        assert!(!ctx.bailout, "JIT execution bailed out unexpectedly");

        // Interpret the return value as a JitValue
        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 30, "Expected 10 + 20 = 30");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_arithmetic_chain() {
        use crate::backend::bytecode::jit::{JitContext, JitValue, JitBailoutReason};

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Build bytecode: ((10 + 20) * 3) - 5 = 85
        let mut builder = ChunkBuilder::new("e2e_chain");
        builder.emit_byte(Opcode::PushLongSmall, 10);
        builder.emit_byte(Opcode::PushLongSmall, 20);
        builder.emit(Opcode::Add);           // 30
        builder.emit_byte(Opcode::PushLongSmall, 3);
        builder.emit(Opcode::Mul);           // 90
        builder.emit_byte(Opcode::PushLongSmall, 5);
        builder.emit(Opcode::Sub);           // 85
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                64,
                constants.as_ptr(),
                constants.len(),
            )
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout);

        let result = JitValue::from_raw(result_bits as u64);
        assert_eq!(result.as_long(), 85);
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_boolean_logic() {
        use crate::backend::bytecode::jit::{JitContext, JitValue, JitBailoutReason};

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Build bytecode: (True or False) and (not False) = True
        let mut builder = ChunkBuilder::new("e2e_bool");
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::PushFalse);
        builder.emit(Opcode::Or);            // True
        builder.emit(Opcode::PushFalse);
        builder.emit(Opcode::Not);           // True
        builder.emit(Opcode::And);           // True
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                64,
                constants.as_ptr(),
                constants.len(),
            )
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout);

        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_bool(), "Expected Bool result");
        assert!(result.as_bool(), "Expected True");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_comparison() {
        use crate::backend::bytecode::jit::{JitContext, JitValue, JitBailoutReason};

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Build bytecode: 5 < 10 = True
        let mut builder = ChunkBuilder::new("e2e_cmp");
        builder.emit_byte(Opcode::PushLongSmall, 5);
        builder.emit_byte(Opcode::PushLongSmall, 10);
        builder.emit(Opcode::Lt);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                64,
                constants.as_ptr(),
                constants.len(),
            )
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout);

        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_bool(), "Expected Bool result");
        assert!(result.as_bool(), "Expected 5 < 10 = True");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_division() {
        use crate::backend::bytecode::jit::{JitContext, JitValue, JitBailoutReason};

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Build bytecode: 100 / 4 = 25
        let mut builder = ChunkBuilder::new("e2e_div");
        builder.emit_byte(Opcode::PushLongSmall, 100);
        builder.emit_byte(Opcode::PushLongSmall, 4);
        builder.emit(Opcode::Div);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                64,
                constants.as_ptr(),
                constants.len(),
            )
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout);

        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 25, "Expected 100 / 4 = 25");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_modulo() {
        use crate::backend::bytecode::jit::{JitContext, JitValue, JitBailoutReason};

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Build bytecode: 17 % 5 = 2
        let mut builder = ChunkBuilder::new("e2e_mod");
        builder.emit_byte(Opcode::PushLongSmall, 17);
        builder.emit_byte(Opcode::PushLongSmall, 5);
        builder.emit(Opcode::Mod);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                64,
                constants.as_ptr(),
                constants.len(),
            )
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout);

        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 2, "Expected 17 % 5 = 2");
    }

    // =========================================================================
    // Stage 2: Pow (Runtime Call) Tests
    // =========================================================================

    #[cfg(feature = "jit")]
    #[test]
    fn test_can_compile_pow() {
        // Test that Pow is now compilable (Stage 2)
        let mut builder = ChunkBuilder::new("test_pow_compilable");
        builder.emit_byte(Opcode::PushLongSmall, 2);
        builder.emit_byte(Opcode::PushLongSmall, 10);
        builder.emit(Opcode::Pow);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk), "Pow should now be Stage 2 compilable");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_pow() {
        use crate::backend::bytecode::jit::{JitContext, JitValue, JitBailoutReason};

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Build bytecode: 2^10 = 1024
        let mut builder = ChunkBuilder::new("e2e_pow");
        builder.emit_byte(Opcode::PushLongSmall, 2);
        builder.emit_byte(Opcode::PushLongSmall, 10);
        builder.emit(Opcode::Pow);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk), "Pow should be compilable");

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                64,
                constants.as_ptr(),
                constants.len(),
            )
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 1024, "Expected 2^10 = 1024");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_pow_zero_exponent() {
        use crate::backend::bytecode::jit::{JitContext, JitValue, JitBailoutReason};

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Build bytecode: 5^0 = 1
        let mut builder = ChunkBuilder::new("e2e_pow_zero");
        builder.emit_byte(Opcode::PushLongSmall, 5);
        builder.emit_byte(Opcode::PushLongSmall, 0);
        builder.emit(Opcode::Pow);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                64,
                constants.as_ptr(),
                constants.len(),
            )
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout);

        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 1, "Expected 5^0 = 1");
    }

    // =========================================================================
    // Stage 2: PushConstant (Runtime Call) Tests
    // =========================================================================

    #[cfg(feature = "jit")]
    #[test]
    fn test_can_compile_push_constant() {
        use crate::backend::MettaValue;

        // Test that PushConstant is now compilable (Stage 2)
        let mut builder = ChunkBuilder::new("test_const_compilable");
        let idx = builder.add_constant(MettaValue::Long(1_000_000));
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk), "PushConstant should now be Stage 2 compilable");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_push_constant() {
        use crate::backend::bytecode::jit::{JitContext, JitValue, JitBailoutReason};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Build bytecode: load constant 1_000_000
        let mut builder = ChunkBuilder::new("e2e_const");
        let idx = builder.add_constant(MettaValue::Long(1_000_000));
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk));

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                64,
                constants.as_ptr(),
                constants.len(),
            )
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 1_000_000, "Expected constant 1_000_000");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_push_constant_arithmetic() {
        use crate::backend::bytecode::jit::{JitContext, JitValue, JitBailoutReason};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Build bytecode: 1_000_000 + 500_000 = 1_500_000
        let mut builder = ChunkBuilder::new("e2e_const_arith");
        let idx1 = builder.add_constant(MettaValue::Long(1_000_000));
        let idx2 = builder.add_constant(MettaValue::Long(500_000));
        builder.emit_u16(Opcode::PushConstant, idx1);
        builder.emit_u16(Opcode::PushConstant, idx2);
        builder.emit(Opcode::Add);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                64,
                constants.as_ptr(),
                constants.len(),
            )
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout);

        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 1_500_000, "Expected 1_000_000 + 500_000 = 1_500_000");
    }

    // =========================================================================
    // Integration Tests: MeTTa Expression → Bytecode → JIT
    // =========================================================================

    /// Helper to execute JIT code and return the result
    #[cfg(feature = "jit")]
    fn exec_jit(code_ptr: *const (), constants: &[crate::backend::MettaValue]) -> crate::backend::bytecode::jit::JitValue {
        use crate::backend::bytecode::jit::{JitContext, JitValue, JitBailoutReason};

        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                64,
                constants.as_ptr(),
                constants.len(),
            )
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution bailed out unexpectedly");
        JitValue::from_raw(result_bits as u64)
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_integration_simple_arithmetic() {
        use crate::backend::bytecode::compile;
        use crate::backend::MettaValue;

        // Build MeTTa expression: (+ 10 20) = 30
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::Long(10),
            MettaValue::Long(20),
        ]);

        let chunk = compile("test", &expr).expect("Compilation failed");

        if !JitCompiler::can_compile_stage1(&chunk) {
            // Skip if not JIT-compilable (e.g., uses non-primitive ops)
            return;
        }

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");
        let code_ptr = compiler.compile(&chunk).expect("JIT compilation failed");

        let result = exec_jit(code_ptr, chunk.constants());
        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 30, "Expected (+ 10 20) = 30");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_integration_nested_arithmetic() {
        use crate::backend::bytecode::compile;
        use crate::backend::MettaValue;

        // Build MeTTa expression: (+ (- 100 50) (* 5 3)) = 50 + 15 = 65
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("+".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("-".to_string()),
                MettaValue::Long(100),
                MettaValue::Long(50),
            ]),
            MettaValue::SExpr(vec![
                MettaValue::Atom("*".to_string()),
                MettaValue::Long(5),
                MettaValue::Long(3),
            ]),
        ]);

        let chunk = compile("test", &expr).expect("Compilation failed");

        if !JitCompiler::can_compile_stage1(&chunk) {
            return;
        }

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");
        let code_ptr = compiler.compile(&chunk).expect("JIT compilation failed");

        let result = exec_jit(code_ptr, chunk.constants());
        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 65, "Expected (+ (- 100 50) (* 5 3)) = 65");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_integration_comparison_chain() {
        use crate::backend::bytecode::compile;
        use crate::backend::MettaValue;

        // Build MeTTa expression: (< (+ 5 5) 20) = True
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("<".to_string()),
            MettaValue::SExpr(vec![
                MettaValue::Atom("+".to_string()),
                MettaValue::Long(5),
                MettaValue::Long(5),
            ]),
            MettaValue::Long(20),
        ]);

        let chunk = compile("test", &expr).expect("Compilation failed");

        if !JitCompiler::can_compile_stage1(&chunk) {
            return;
        }

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");
        let code_ptr = compiler.compile(&chunk).expect("JIT compilation failed");

        let result = exec_jit(code_ptr, chunk.constants());
        assert!(result.is_bool(), "Expected Bool result");
        assert!(result.as_bool(), "Expected (< (+ 5 5) 20) = True");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_integration_pow() {
        use crate::backend::bytecode::compile;
        use crate::backend::MettaValue;

        // Build MeTTa expression: (pow 2 8) = 256
        let expr = MettaValue::SExpr(vec![
            MettaValue::Atom("pow".to_string()),
            MettaValue::Long(2),
            MettaValue::Long(8),
        ]);

        let chunk = compile("test", &expr).expect("Compilation failed");

        if !JitCompiler::can_compile_stage1(&chunk) {
            return;
        }

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");
        let code_ptr = compiler.compile(&chunk).expect("JIT compilation failed");

        let result = exec_jit(code_ptr, chunk.constants());
        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 256, "Expected (pow 2 8) = 256");
    }

    // =========================================================================
    // JIT vs VM Equivalence Tests
    // =========================================================================

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_vm_equivalence_arithmetic() {
        use crate::backend::bytecode::{compile, BytecodeVM};
        use crate::backend::MettaValue;
        use std::sync::Arc;

        // Test various arithmetic expressions
        let test_cases = vec![
            (vec!["+", "10", "20"], 30i64),
            (vec!["-", "100", "45"], 55i64),
            (vec!["*", "7", "8"], 56i64),
            (vec!["/", "100", "4"], 25i64),
            (vec!["%", "17", "5"], 2i64),
        ];

        for (ops, expected) in test_cases {
            let op = ops[0];
            let a: i64 = ops[1].parse().unwrap();
            let b: i64 = ops[2].parse().unwrap();

            let expr = MettaValue::SExpr(vec![
                MettaValue::Atom(op.to_string()),
                MettaValue::Long(a),
                MettaValue::Long(b),
            ]);

            let chunk = Arc::new(compile("equiv", &expr).expect("Compilation failed"));

            // VM result
            let mut vm = BytecodeVM::new(Arc::clone(&chunk));
            let vm_results = vm.run().expect("VM execution failed");

            // JIT result (if compilable)
            if JitCompiler::can_compile_stage1(&chunk) {
                let mut compiler = JitCompiler::new().expect("Failed to create compiler");
                let code_ptr = compiler.compile(&chunk).expect("JIT compilation failed");
                let jit_result = exec_jit(code_ptr, chunk.constants());

                assert!(jit_result.is_long(), "JIT: Expected Long for ({} {} {})", op, a, b);
                let jit_val = jit_result.as_long();

                // Compare with VM
                assert_eq!(vm_results.len(), 1, "VM should return single value");
                if let MettaValue::Long(vm_val) = &vm_results[0] {
                    assert_eq!(jit_val, *vm_val, "JIT vs VM mismatch for ({} {} {})", op, a, b);
                    assert_eq!(jit_val, expected, "Expected {} for ({} {} {})", expected, op, a, b);
                } else {
                    panic!("VM returned non-Long value");
                }
            }
        }
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_vm_equivalence_comparisons() {
        use crate::backend::bytecode::{compile, BytecodeVM};
        use crate::backend::MettaValue;
        use std::sync::Arc;

        // Test comparison operators
        let test_cases = vec![
            ("<", 5, 10, true),
            ("<", 10, 5, false),
            ("<=", 5, 5, true),
            ("<=", 6, 5, false),
            (">", 10, 5, true),
            (">", 5, 10, false),
            (">=", 5, 5, true),
            (">=", 4, 5, false),
            ("==", 42, 42, true),
            ("==", 42, 43, false),
        ];

        for (op, a, b, expected) in test_cases {
            let expr = MettaValue::SExpr(vec![
                MettaValue::Atom(op.to_string()),
                MettaValue::Long(a),
                MettaValue::Long(b),
            ]);

            let chunk = Arc::new(compile("equiv_cmp", &expr).expect("Compilation failed"));

            // VM result
            let mut vm = BytecodeVM::new(Arc::clone(&chunk));
            let vm_results = vm.run().expect("VM execution failed");

            // JIT result
            if JitCompiler::can_compile_stage1(&chunk) {
                let mut compiler = JitCompiler::new().expect("Failed to create compiler");
                let code_ptr = compiler.compile(&chunk).expect("JIT compilation failed");
                let jit_result = exec_jit(code_ptr, chunk.constants());

                assert!(jit_result.is_bool(), "JIT: Expected Bool for ({} {} {})", op, a, b);
                let jit_val = jit_result.as_bool();

                assert_eq!(vm_results.len(), 1);
                if let MettaValue::Bool(vm_val) = &vm_results[0] {
                    assert_eq!(jit_val, *vm_val, "JIT vs VM mismatch for ({} {} {})", op, a, b);
                    assert_eq!(jit_val, expected, "Expected {} for ({} {} {})", expected, op, a, b);
                } else {
                    panic!("VM returned non-Bool value");
                }
            }
        }
    }

    // =========================================================================
    // Edge Cases and Boundary Tests
    // =========================================================================

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_negative_numbers() {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Build bytecode: -10 + 5 = -5
        // PushLongSmall operand is i8, cast to u8 for emit_byte
        let mut builder = ChunkBuilder::new("e2e_neg");
        builder.emit_byte(Opcode::PushLongSmall, (-10i8) as u8);
        builder.emit_byte(Opcode::PushLongSmall, 5);
        builder.emit(Opcode::Add);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), -5, "Expected -10 + 5 = -5");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_all_comparisons() {
        let test_cases = vec![
            (Opcode::Lt, 5, 10, true),   // 5 < 10
            (Opcode::Lt, 10, 5, false),  // 10 < 5
            (Opcode::Le, 5, 5, true),    // 5 <= 5
            (Opcode::Le, 6, 5, false),   // 6 <= 5
            (Opcode::Gt, 10, 5, true),   // 10 > 5
            (Opcode::Gt, 5, 10, false),  // 5 > 10
            (Opcode::Ge, 5, 5, true),    // 5 >= 5
            (Opcode::Ge, 4, 5, false),   // 4 >= 5
            (Opcode::Eq, 42, 42, true),  // 42 == 42
            (Opcode::Eq, 42, 43, false), // 42 == 43
            (Opcode::Ne, 42, 43, true),  // 42 != 43
            (Opcode::Ne, 42, 42, false), // 42 != 42
        ];

        for (op, a, b, expected) in test_cases {
            let mut compiler = JitCompiler::new().expect("Failed to create compiler");

            let mut builder = ChunkBuilder::new("cmp_test");
            builder.emit_byte(Opcode::PushLongSmall, a);
            builder.emit_byte(Opcode::PushLongSmall, b);
            builder.emit(op);
            builder.emit(Opcode::Return);
            let chunk = builder.build();

            let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
            let result = exec_jit(code_ptr, chunk.constants());

            assert!(result.is_bool(), "Expected Bool for {:?}({}, {})", op, a, b);
            assert_eq!(result.as_bool(), expected, "{:?}({}, {}) should be {}", op, a, b, expected);
        }
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_stack_operations() {
        // Test Dup: duplicate top of stack
        {
            let mut compiler = JitCompiler::new().expect("Failed to create compiler");
            let mut builder = ChunkBuilder::new("dup_test");
            builder.emit_byte(Opcode::PushLongSmall, 42);
            builder.emit(Opcode::Dup);
            builder.emit(Opcode::Add);  // 42 + 42 = 84
            builder.emit(Opcode::Return);
            let chunk = builder.build();

            let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
            let result = exec_jit(code_ptr, chunk.constants());
            assert_eq!(result.as_long(), 84, "Dup: 42 + 42 = 84");
        }

        // Test Swap: swap top two values
        {
            let mut compiler = JitCompiler::new().expect("Failed to create compiler");
            let mut builder = ChunkBuilder::new("swap_test");
            builder.emit_byte(Opcode::PushLongSmall, 10);  // bottom
            builder.emit_byte(Opcode::PushLongSmall, 3);   // top
            builder.emit(Opcode::Swap);
            builder.emit(Opcode::Sub);  // 3 - 10 = -7
            builder.emit(Opcode::Return);
            let chunk = builder.build();

            let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
            let result = exec_jit(code_ptr, chunk.constants());
            assert_eq!(result.as_long(), -7, "Swap: 3 - 10 = -7");
        }

        // Test Over: copy second value to top
        {
            let mut compiler = JitCompiler::new().expect("Failed to create compiler");
            let mut builder = ChunkBuilder::new("over_test");
            builder.emit_byte(Opcode::PushLongSmall, 5);   // bottom
            builder.emit_byte(Opcode::PushLongSmall, 10);  // top
            builder.emit(Opcode::Over);  // copies 5 to top: [5, 10, 5]
            builder.emit(Opcode::Add);   // 10 + 5 = 15: [5, 15]
            builder.emit(Opcode::Add);   // 5 + 15 = 20: [20]
            builder.emit(Opcode::Return);
            let chunk = builder.build();

            let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
            let result = exec_jit(code_ptr, chunk.constants());
            assert_eq!(result.as_long(), 20, "Over: 5 + (10 + 5) = 20");
        }
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_special_values() {
        // Test Nil
        {
            let mut compiler = JitCompiler::new().expect("Failed to create compiler");
            let mut builder = ChunkBuilder::new("nil_test");
            builder.emit(Opcode::PushNil);
            builder.emit(Opcode::Return);
            let chunk = builder.build();

            let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
            let result = exec_jit(code_ptr, chunk.constants());
            assert!(result.is_nil(), "Expected Nil");
        }

        // Test Unit
        {
            let mut compiler = JitCompiler::new().expect("Failed to create compiler");
            let mut builder = ChunkBuilder::new("unit_test");
            builder.emit(Opcode::PushUnit);
            builder.emit(Opcode::Return);
            let chunk = builder.build();

            let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
            let result = exec_jit(code_ptr, chunk.constants());
            assert!(result.is_unit(), "Expected Unit");
        }

        // Test True and False
        {
            let mut compiler = JitCompiler::new().expect("Failed to create compiler");
            let mut builder = ChunkBuilder::new("bool_test");
            builder.emit(Opcode::PushTrue);
            builder.emit(Opcode::PushFalse);
            builder.emit(Opcode::And);  // True and False = False
            builder.emit(Opcode::Return);
            let chunk = builder.build();

            let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
            let result = exec_jit(code_ptr, chunk.constants());
            assert!(result.is_bool(), "Expected Bool");
            assert!(!result.as_bool(), "True and False = False");
        }
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_floor_div() {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Build bytecode: 17 // 5 = 3 (floor division)
        let mut builder = ChunkBuilder::new("floor_div_test");
        builder.emit_byte(Opcode::PushLongSmall, 17);
        builder.emit_byte(Opcode::PushLongSmall, 5);
        builder.emit(Opcode::FloorDiv);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 3, "Expected 17 // 5 = 3");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_neg_and_abs() {
        // Test Neg
        {
            let mut compiler = JitCompiler::new().expect("Failed to create compiler");
            let mut builder = ChunkBuilder::new("neg_test");
            builder.emit_byte(Opcode::PushLongSmall, 42);
            builder.emit(Opcode::Neg);
            builder.emit(Opcode::Return);
            let chunk = builder.build();

            let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
            let result = exec_jit(code_ptr, chunk.constants());
            assert_eq!(result.as_long(), -42, "Neg(42) = -42");
        }

        // Test Abs of negative
        {
            let mut compiler = JitCompiler::new().expect("Failed to create compiler");
            let mut builder = ChunkBuilder::new("abs_neg_test");
            builder.emit_byte(Opcode::PushLongSmall, (-42i8) as u8);
            builder.emit(Opcode::Abs);
            builder.emit(Opcode::Return);
            let chunk = builder.build();

            let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
            let result = exec_jit(code_ptr, chunk.constants());
            assert_eq!(result.as_long(), 42, "Abs(-42) = 42");
        }

        // Test Abs of positive (unchanged)
        {
            let mut compiler = JitCompiler::new().expect("Failed to create compiler");
            let mut builder = ChunkBuilder::new("abs_pos_test");
            builder.emit_byte(Opcode::PushLongSmall, 42);
            builder.emit(Opcode::Abs);
            builder.emit(Opcode::Return);
            let chunk = builder.build();

            let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
            let result = exec_jit(code_ptr, chunk.constants());
            assert_eq!(result.as_long(), 42, "Abs(42) = 42");
        }
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_pow_chain() {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Build bytecode: pow(pow(2, 3), 2) = pow(8, 2) = 64
        let mut builder = ChunkBuilder::new("pow_chain_test");
        builder.emit_byte(Opcode::PushLongSmall, 2);
        builder.emit_byte(Opcode::PushLongSmall, 3);
        builder.emit(Opcode::Pow);  // 2^3 = 8
        builder.emit_byte(Opcode::PushLongSmall, 2);
        builder.emit(Opcode::Pow);  // 8^2 = 64
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 64, "Expected pow(pow(2,3),2) = 64");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_deep_expression() {
        use crate::backend::bytecode::compile;
        use crate::backend::MettaValue;

        // Build a deeply nested expression: ((((1 + 2) + 3) + 4) + 5) = 15
        fn build_nested_add(depth: usize) -> MettaValue {
            let mut expr = MettaValue::Long(1);
            for i in 2..=depth {
                expr = MettaValue::SExpr(vec![
                    MettaValue::Atom("+".to_string()),
                    expr,
                    MettaValue::Long(i as i64),
                ]);
            }
            expr
        }

        let expr = build_nested_add(5);  // 1 + 2 + 3 + 4 + 5 = 15
        let chunk = compile("deep", &expr).expect("Compilation failed");

        if !JitCompiler::can_compile_stage1(&chunk) {
            return;
        }

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");
        let code_ptr = compiler.compile(&chunk).expect("JIT compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 15, "Expected 1+2+3+4+5 = 15");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_large_constant_arithmetic() {
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Build bytecode with large constants: 1_000_000 * 1000 = 1_000_000_000
        let mut builder = ChunkBuilder::new("large_const_arith");
        let idx1 = builder.add_constant(MettaValue::Long(1_000_000));
        let idx2 = builder.add_constant(MettaValue::Long(1000));
        builder.emit_u16(Opcode::PushConstant, idx1);
        builder.emit_u16(Opcode::PushConstant, idx2);
        builder.emit(Opcode::Mul);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 1_000_000_000, "Expected 1M * 1K = 1B");
    }

    // =========================================================================
    // JIT Bailout Tests for Non-Determinism Opcodes (Phase 4)
    // =========================================================================

    #[test]
    fn test_can_compile_fail() {
        // Phase 9: Fail is detected as nondeterminism and routed to bytecode tier
        let mut builder = ChunkBuilder::new("test_fail");
        builder.emit(Opcode::Fail);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        // Fail is NOT compilable - static nondeterminism detection routes to bytecode
        assert!(!JitCompiler::can_compile_stage1(&chunk), "Fail chunks should not be JIT compilable (Phase 9 optimization)");
    }

    #[test]
    fn test_can_compile_cut() {
        // Phase 9: Cut is detected as nondeterminism and routed to bytecode tier
        let mut builder = ChunkBuilder::new("test_cut");
        builder.emit(Opcode::Cut);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        // Cut is NOT compilable - static nondeterminism detection routes to bytecode
        assert!(!JitCompiler::can_compile_stage1(&chunk), "Cut chunks should not be JIT compilable (Phase 9 optimization)");
    }

    #[test]
    fn test_can_compile_collect_with_bailout() {
        // Phase 9: Collect is detected as nondeterminism and routed to bytecode tier
        let mut builder = ChunkBuilder::new("test_collect");
        builder.emit(Opcode::Collect);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        // Collect is NOT compilable - static nondeterminism detection routes to bytecode
        assert!(!JitCompiler::can_compile_stage1(&chunk), "Collect chunks should not be JIT compilable (Phase 9 optimization)");
    }

    #[test]
    fn test_can_compile_collect_n() {
        // Phase 9: CollectN is detected as nondeterminism and routed to bytecode tier
        let mut builder = ChunkBuilder::new("test_collect_n");
        builder.emit_byte(Opcode::CollectN, 5);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        // CollectN is NOT compilable - static nondeterminism detection routes to bytecode
        assert!(!JitCompiler::can_compile_stage1(&chunk), "CollectN chunks should not be JIT compilable (Phase 9 optimization)");
    }

    #[test]
    fn test_can_compile_yield_with_bailout() {
        // Phase 9: Yield is detected as nondeterminism and routed to bytecode tier
        let mut builder = ChunkBuilder::new("test_yield");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit(Opcode::Yield);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        // Yield is NOT compilable - static nondeterminism detection routes to bytecode
        assert!(!JitCompiler::can_compile_stage1(&chunk), "Yield chunks should not be JIT compilable (Phase 9 optimization)");
    }

    #[test]
    fn test_can_compile_begin_nondet() {
        // Phase 9: BeginNondet is detected as nondeterminism and routed to bytecode tier
        let mut builder = ChunkBuilder::new("test_begin_nondet");
        builder.emit(Opcode::BeginNondet);
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit(Opcode::EndNondet);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        // BeginNondet is NOT compilable - static nondeterminism detection routes to bytecode
        assert!(!JitCompiler::can_compile_stage1(&chunk), "BeginNondet chunks should not be JIT compilable (Phase 9 optimization)");
    }

    #[test]
    fn test_can_compile_end_nondet() {
        // Phase 9: EndNondet is detected as nondeterminism and routed to bytecode tier
        let mut builder = ChunkBuilder::new("test_end_nondet");
        builder.emit(Opcode::EndNondet);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        // EndNondet is NOT compilable - static nondeterminism detection routes to bytecode
        assert!(!JitCompiler::can_compile_stage1(&chunk), "EndNondet chunks should not be JIT compilable (Phase 9 optimization)");
    }

    #[test]
    fn test_can_compile_call_n() {
        // Phase 1.2: CallN is compilable (stack-based head + bailout)
        let mut builder = ChunkBuilder::new("test_call_n");
        builder.emit_u16(Opcode::PushConstant, 0); // Push head
        builder.emit_u16(Opcode::PushConstant, 1); // Push arg
        builder.emit_byte(Opcode::CallN, 1);       // CallN with arity = 1
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk), "CallN opcode should be JIT compilable (Phase 1.2)");
    }

    #[test]
    fn test_can_compile_tail_call_n() {
        // Phase 1.2: TailCallN is compilable (stack-based head + bailout + TCO)
        let mut builder = ChunkBuilder::new("test_tail_call_n");
        builder.emit_u16(Opcode::PushConstant, 0); // Push head
        builder.emit_u16(Opcode::PushConstant, 1); // Push arg1
        builder.emit_u16(Opcode::PushConstant, 2); // Push arg2
        builder.emit_byte(Opcode::TailCallN, 2);   // TailCallN with arity = 2
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk), "TailCallN opcode should be JIT compilable (Phase 1.2)");
    }

    #[test]
    fn test_can_compile_call_n_zero_arity() {
        // Phase 1.2: CallN with zero arity (just head, no args)
        let mut builder = ChunkBuilder::new("test_call_n_zero_arity");
        builder.emit_u16(Opcode::PushConstant, 0); // Push head
        builder.emit_byte(Opcode::CallN, 0);       // CallN with arity = 0
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk), "CallN with arity 0 should be JIT compilable (Phase 1.2)");
    }

    #[test]
    fn test_can_compile_fork_in_middle_with_bailout() {
        // Phase 9: Fork anywhere in chunk is detected as nondeterminism
        let mut builder = ChunkBuilder::new("test_fork_middle");
        builder.emit_byte(Opcode::PushLongSmall, 10);
        builder.emit_byte(Opcode::PushLongSmall, 20);
        builder.emit(Opcode::Add);
        builder.emit_byte(Opcode::Fork, 2);  // Fork in the middle
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        // Fork anywhere triggers nondeterminism detection - routes to bytecode tier
        assert!(!JitCompiler::can_compile_stage1(&chunk), "Fork in chunk should not be JIT compilable (Phase 9 optimization)");
    }

    #[test]
    fn test_jit_bailout_reason_nondeterminism_exists() {
        // Verify that JitBailoutReason::NonDeterminism exists and has correct value
        use crate::backend::bytecode::jit::JitBailoutReason;

        let reason = JitBailoutReason::NonDeterminism;
        assert_eq!(reason as u8, 8, "NonDeterminism should have discriminant 8");
    }

    #[test]
    fn test_jit_bailout_reason_phase4_values() {
        // Verify Phase 4 bailout reasons have correct discriminant values
        use crate::backend::bytecode::jit::JitBailoutReason;

        assert_eq!(JitBailoutReason::Fork as u8, 11, "Fork should have discriminant 11");
        assert_eq!(JitBailoutReason::Yield as u8, 12, "Yield should have discriminant 12");
        assert_eq!(JitBailoutReason::Collect as u8, 13, "Collect should have discriminant 13");
    }

    // =========================================================================
    // Phase 4: Fork/Yield/Collect Execution Tests (Native Semantics)
    // =========================================================================

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_fork_creates_choice_points_native() {
        // Phase 9: Fork chunks are now detected statically and routed to bytecode tier
        // This test verifies that Fork chunks are properly rejected by can_compile_stage1()
        use crate::backend::MettaValue;

        // Create chunk with Fork opcode
        // Format: Fork count:u16 idx0:u16 idx1:u16
        let mut builder = ChunkBuilder::new("test_fork_native");
        let idx0 = builder.add_constant(MettaValue::Long(1));
        let idx1 = builder.add_constant(MettaValue::Long(2));
        builder.emit_u16(Opcode::Fork, 2); // Fork with 2 alternatives
        builder.emit_raw(&idx0.to_be_bytes()); // index for alternative 0
        builder.emit_raw(&idx1.to_be_bytes()); // index for alternative 1
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        // Verify nondeterminism is detected
        assert!(chunk.has_nondeterminism(), "Fork should be detected as nondeterminism");

        // Verify JIT compilation is rejected
        assert!(!JitCompiler::can_compile_stage1(&chunk),
            "Fork chunks should not be JIT compilable (Phase 9: static nondeterminism routing)");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_yield_signals_bailout() {
        // Stage 2 JIT: Yield now returns JIT_SIGNAL_YIELD to dispatcher instead of bailout
        // Note: This test verifies that Yield stores result and returns signal
        use crate::backend::bytecode::jit::{JitContext, JitValue, JitChoicePoint, JIT_SIGNAL_YIELD};
        use crate::backend::bytecode::jit::runtime::jit_runtime_yield_native;

        // Test the runtime function directly instead of JIT code generation
        // (JIT code gen for Yield returns immediately, which breaks block filling)
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];
        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };

        // Yield value 42
        let value = JitValue::from_long(42).0; // Get raw u64
        let signal = unsafe { jit_runtime_yield_native(&mut ctx, value, 0) };

        // Stage 2: Yield stores result and returns JIT_SIGNAL_YIELD
        assert_eq!(signal, JIT_SIGNAL_YIELD, "Yield should return JIT_SIGNAL_YIELD");
        assert_eq!(ctx.results_count, 1, "Yield should have stored one result");

        // Verify the stored result
        let stored_result = unsafe { *ctx.results };
        assert_eq!(stored_result.as_long(), 42, "Yield should have stored 42");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_collect_signals_bailout() {
        // Stage 2 JIT: Collect now uses native function and pushes result to stack
        // Note: The return value is the NaN-boxed SExpr result, not a signal
        use crate::backend::bytecode::jit::{JitContext, JitValue, JitChoicePoint};
        use crate::backend::bytecode::jit::runtime::jit_runtime_collect_native;

        // Test the runtime function directly
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut choice_points: Vec<JitChoicePoint> = vec![JitChoicePoint::default(); 8];
        let mut results: Vec<JitValue> = vec![JitValue::nil(); 16];
        let mut ctx = unsafe {
            JitContext::with_nondet(
                stack.as_mut_ptr(),
                stack.len(),
                std::ptr::null(),
                0,
                choice_points.as_mut_ptr(),
                choice_points.len(),
                results.as_mut_ptr(),
                results.len(),
            )
        };

        // Store some results to collect
        unsafe {
            *ctx.results.add(0) = JitValue::from_long(1);
            *ctx.results.add(1) = JitValue::from_long(2);
            *ctx.results.add(2) = JitValue::from_long(3);
        }
        ctx.results_count = 3;

        // Collect the results
        let result = unsafe { jit_runtime_collect_native(&mut ctx) };

        // Stage 2: Collect returns NaN-boxed SExpr with collected results
        // The result should be a heap pointer (TAG_HEAP)
        let jv = JitValue::from_raw(result);
        assert!(jv.is_heap(), "Collect should return a heap pointer (SExpr)");

        // Verify the collected SExpr (should be (1 2 3))
        let metta = unsafe { jv.to_metta() };
        if let crate::backend::models::MettaValue::SExpr(items) = metta {
            assert_eq!(items.len(), 3, "Collected SExpr should have 3 items");
        } else {
            panic!("Collect should return SExpr");
        }
    }

    // =========================================================================
    // Stage 3: Jump Instructions Tests
    // =========================================================================

    #[test]
    fn test_can_compile_jump() {
        // Test that Jump opcode is now compilable (Stage 3)
        let mut builder = ChunkBuilder::new("test_jump");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        // Jump forward past the next instruction
        builder.emit_u16(Opcode::Jump, 4); // Jump over the next instruction
        builder.emit_byte(Opcode::PushLongSmall, 0); // Skipped
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk), "Jump should be Stage 3 compilable");
    }

    #[test]
    fn test_can_compile_jump_if_false() {
        // Test that JumpIfFalse opcode is now compilable (Stage 3)
        let mut builder = ChunkBuilder::new("test_jump_if_false");
        builder.emit(Opcode::PushTrue);
        builder.emit_u16(Opcode::JumpIfFalse, 5); // Skip if false
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk), "JumpIfFalse should be Stage 3 compilable");
    }

    #[test]
    fn test_can_compile_jump_if_true() {
        // Test that JumpIfTrue opcode is now compilable (Stage 3)
        let mut builder = ChunkBuilder::new("test_jump_if_true");
        builder.emit(Opcode::PushFalse);
        builder.emit_u16(Opcode::JumpIfTrue, 5); // Skip if true
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk), "JumpIfTrue should be Stage 3 compilable");
    }

    #[test]
    fn test_can_compile_jump_short() {
        // Test that JumpShort opcode is now compilable (Stage 3)
        let mut builder = ChunkBuilder::new("test_jump_short");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit_byte(Opcode::JumpShort, 3); // Short jump
        builder.emit_byte(Opcode::PushLongSmall, 0); // Skipped
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk), "JumpShort should be Stage 3 compilable");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_conditional_jump_true_path() {
        // Test conditional jump: if True then 42 else 0
        // When condition is true, JumpIfFalse should NOT jump (fallthrough to then branch)
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Build bytecode: if True then 42 else 0
        // Layout (instruction sizes):
        // 0: PushTrue (1 byte)
        // 1: JumpIfFalse +5 (3 bytes: 1 opcode + 2 i16) -> next_ip=4, target=4+5=9 (else branch)
        // 4: PushLongSmall 42 (2 bytes: then branch)
        // 6: Jump +2 (3 bytes: 1 opcode + 2 i16) -> next_ip=9, target=9+2=11 (Return, skip else)
        // 9: PushLongSmall 0 (2 bytes: else branch)
        // 11: Return (1 byte)

        let mut builder = ChunkBuilder::new("e2e_cond_true");
        builder.emit(Opcode::PushTrue);                    // offset 0
        builder.emit_u16(Opcode::JumpIfFalse, 5);          // offset 1, jumps to 9 if false
        builder.emit_byte(Opcode::PushLongSmall, 42);      // offset 4, then branch
        builder.emit_u16(Opcode::Jump, 2);                 // offset 6, skip else -> target 11
        builder.emit_byte(Opcode::PushLongSmall, 0);       // offset 9, else branch
        builder.emit(Opcode::Return);                       // offset 11
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk));

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 42, "Expected true branch result 42");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_conditional_jump_false_path() {
        // Test conditional jump: if False then 42 else 99
        // When condition is false, JumpIfFalse should jump to else branch
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Layout (same as true path but with PushFalse):
        // 0: PushFalse (1 byte)
        // 1: JumpIfFalse +5 (3 bytes) -> next_ip=4, target=4+5=9 (else branch)
        // 4: PushLongSmall 42 (2 bytes: then branch, skipped)
        // 6: Jump +2 (3 bytes) -> next_ip=9, target=9+2=11 (Return, skip else)
        // 9: PushLongSmall 99 (2 bytes: else branch)
        // 11: Return (1 byte)

        let mut builder = ChunkBuilder::new("e2e_cond_false");
        builder.emit(Opcode::PushFalse);                   // offset 0
        builder.emit_u16(Opcode::JumpIfFalse, 5);          // offset 1, jumps to 9 if false
        builder.emit_byte(Opcode::PushLongSmall, 42);      // offset 4, then branch (skipped)
        builder.emit_u16(Opcode::Jump, 2);                 // offset 6, skip else (skipped)
        builder.emit_byte(Opcode::PushLongSmall, 99);      // offset 9, else branch
        builder.emit(Opcode::Return);                       // offset 11
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk));

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 99, "Expected false branch result 99");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_comparison_with_jump() {
        // Test: if 10 < 20 then 1 else 0
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Layout:
        // 0: PushLongSmall 10 (2 bytes)
        // 2: PushLongSmall 20 (2 bytes)
        // 4: Lt (1 byte) -> stack: [true]
        // 5: JumpIfFalse +5 (3 bytes) -> next_ip=8, target=8+5=13 (else branch)
        // 8: PushLongSmall 1 (2 bytes: then branch)
        // 10: Jump +2 (3 bytes) -> next_ip=13, target=13+2=15 (Return, skip else)
        // 13: PushLongSmall 0 (2 bytes: else branch)
        // 15: Return (1 byte)

        let mut builder = ChunkBuilder::new("e2e_cmp_jump");
        builder.emit_byte(Opcode::PushLongSmall, 10);      // offset 0
        builder.emit_byte(Opcode::PushLongSmall, 20);      // offset 2
        builder.emit(Opcode::Lt);                           // offset 4, stack: [true]
        builder.emit_u16(Opcode::JumpIfFalse, 5);          // offset 5, jump to 13 if false
        builder.emit_byte(Opcode::PushLongSmall, 1);       // offset 8, then branch
        builder.emit_u16(Opcode::Jump, 2);                 // offset 10, skip else -> target 15
        builder.emit_byte(Opcode::PushLongSmall, 0);       // offset 13, else branch
        builder.emit(Opcode::Return);                       // offset 15
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk));

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 1, "Expected 10 < 20 = true, result 1");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_jump_if_true() {
        // Test JumpIfTrue: if True then jump to return, keeping 99 on stack
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Layout:
        // 0: PushLongSmall 99 (2 bytes) - push result first
        // 2: PushTrue (1 byte)
        // 3: JumpIfTrue +3 (3 bytes) -> next_ip=6, target=6+3=9 (Return)
        // 6: Pop (1 byte) - pop 99 (not executed if jump taken)
        // 7: PushLongSmall 0 (2 bytes) - push 0 (not executed if jump taken)
        // 9: Return (1 byte)
        //
        // When true: jump to 9, stack=[99], return 99
        // When false: pop 99, push 0, stack=[0], return 0

        let mut builder = ChunkBuilder::new("e2e_jump_if_true");
        builder.emit_byte(Opcode::PushLongSmall, 99);     // offset 0, push result first
        builder.emit(Opcode::PushTrue);                   // offset 2
        builder.emit_u16(Opcode::JumpIfTrue, 3);          // offset 3, jumps to 9 if true
        builder.emit(Opcode::Pop);                         // offset 6, pop 99 (not executed)
        builder.emit_byte(Opcode::PushLongSmall, 0);      // offset 7, push 0 (not executed)
        builder.emit(Opcode::Return);                      // offset 9
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk));

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 99, "Expected jump taken, return 99");
    }

    // =========================================================================
    // Stage 4: Local Variable Tests
    // =========================================================================

    #[test]
    fn test_can_compile_stage4_local_variables() {
        let mut builder = ChunkBuilder::new("test_locals");
        builder.set_local_count(2);
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit_byte(Opcode::StoreLocal, 0);  // Store 42 in local 0
        builder.emit_byte(Opcode::PushLongSmall, 10);
        builder.emit_byte(Opcode::StoreLocal, 1);  // Store 10 in local 1
        builder.emit_byte(Opcode::LoadLocal, 0);   // Load local 0 (42)
        builder.emit_byte(Opcode::LoadLocal, 1);   // Load local 1 (10)
        builder.emit(Opcode::Add);                 // 42 + 10 = 52
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk));
    }

    #[test]
    fn test_can_compile_halt_opcode() {
        // Phase 1.10: Halt is now compilable (returns HALT signal)
        let mut builder = ChunkBuilder::new("test_halt");
        builder.emit_byte(Opcode::LoadLocal, 0);
        builder.emit(Opcode::Halt); // Halt is now compilable
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk), "Halt opcode should be JIT compilable");
    }

    // =====================================================================
    // Phase G: Advanced Nondeterminism Tests
    // =====================================================================

    #[test]
    fn test_can_compile_phase_g_cut() {
        // Phase 9: Cut is detected as nondeterminism and routed to bytecode tier
        let mut builder = ChunkBuilder::new("test_phase_g_cut");
        builder.emit(Opcode::Cut);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        // Cut is NOT compilable - static nondeterminism detection routes to bytecode
        assert!(
            !JitCompiler::can_compile_stage1(&chunk),
            "Phase 9: Cut chunks should not be JIT compilable (static nondeterminism detection)"
        );
    }

    // =====================================================================
    // Phase H: MORK Bridge Tests
    // =====================================================================

    #[test]
    fn test_can_compile_phase_h_mork_lookup() {
        // Phase H: MorkLookup opcode is compilable
        let mut builder = ChunkBuilder::new("test_phase_h_mork_lookup");
        builder.emit(Opcode::PushNil); // path placeholder
        builder.emit(Opcode::MorkLookup);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "Phase H: MorkLookup should be JIT compilable"
        );
    }

    #[test]
    fn test_can_compile_phase_h_mork_match() {
        // Phase H: MorkMatch opcode is compilable
        let mut builder = ChunkBuilder::new("test_phase_h_mork_match");
        builder.emit(Opcode::PushNil); // path placeholder
        builder.emit(Opcode::PushNil); // pattern placeholder
        builder.emit(Opcode::MorkMatch);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "Phase H: MorkMatch should be JIT compilable"
        );
    }

    #[test]
    fn test_can_compile_phase_h_mork_insert() {
        // Phase H: MorkInsert opcode is compilable
        let mut builder = ChunkBuilder::new("test_phase_h_mork_insert");
        builder.emit(Opcode::PushNil); // path placeholder
        builder.emit(Opcode::PushNil); // value placeholder
        builder.emit(Opcode::MorkInsert);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "Phase H: MorkInsert should be JIT compilable"
        );
    }

    #[test]
    fn test_can_compile_phase_h_mork_delete() {
        // Phase H: MorkDelete opcode is compilable
        let mut builder = ChunkBuilder::new("test_phase_h_mork_delete");
        builder.emit(Opcode::PushNil); // path placeholder
        builder.emit(Opcode::MorkDelete);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "Phase H: MorkDelete should be JIT compilable"
        );
    }

    // =====================================================================
    // Phase I: Debug/Meta Tests
    // =====================================================================

    #[test]
    fn test_can_compile_phase_i_trace() {
        // Phase I: Trace opcode is compilable
        let mut builder = ChunkBuilder::new("test_phase_i_trace");
        builder.emit_byte(Opcode::PushLongSmall, 42); // value to trace
        builder.emit_u16(Opcode::Trace, 0); // msg_idx = 0
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "Phase I: Trace should be JIT compilable"
        );
    }

    #[test]
    fn test_can_compile_phase_i_breakpoint() {
        // Phase I: Breakpoint opcode is compilable
        let mut builder = ChunkBuilder::new("test_phase_i_breakpoint");
        builder.emit_u16(Opcode::Breakpoint, 1); // bp_id = 1
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "Phase I: Breakpoint should be JIT compilable"
        );
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_local_store_load() {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Store 42 in local 0, then load and return it
        let mut builder = ChunkBuilder::new("e2e_locals_basic");
        builder.set_local_count(1);
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit_byte(Opcode::StoreLocal, 0);
        builder.emit_byte(Opcode::LoadLocal, 0);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk));

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 42, "Expected local variable value 42");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_local_arithmetic() {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Store values in two locals, load them, and perform arithmetic
        // local 0 = 100, local 1 = 58
        // result = local0 - local1 = 42
        let mut builder = ChunkBuilder::new("e2e_locals_arith");
        builder.set_local_count(2);
        builder.emit_byte(Opcode::PushLongSmall, 100);
        builder.emit_byte(Opcode::StoreLocal, 0);
        builder.emit_byte(Opcode::PushLongSmall, 58);
        builder.emit_byte(Opcode::StoreLocal, 1);
        builder.emit_byte(Opcode::LoadLocal, 0);
        builder.emit_byte(Opcode::LoadLocal, 1);
        builder.emit(Opcode::Sub);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 42, "Expected 100 - 58 = 42");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_local_overwrite() {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Test that storing to a local twice overwrites the value
        // local 0 = 10, then local 0 = 99
        let mut builder = ChunkBuilder::new("e2e_locals_overwrite");
        builder.set_local_count(1);
        builder.emit_byte(Opcode::PushLongSmall, 10);
        builder.emit_byte(Opcode::StoreLocal, 0);
        builder.emit_byte(Opcode::PushLongSmall, 99);
        builder.emit_byte(Opcode::StoreLocal, 0);  // Overwrite
        builder.emit_byte(Opcode::LoadLocal, 0);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 99, "Expected overwritten value 99");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_local_with_control_flow() {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Store a value, then use conditional jump
        // if true, load the local and return; else load 0 and return
        // local 0 = 77, condition = true, should return 77
        //
        // Layout:
        // 0-1: PushLongSmall 77 (2 bytes)
        // 2-3: StoreLocal 0 (2 bytes)
        // 4: PushTrue (1 byte)
        // 5-7: JumpIfFalse +5 (3 bytes) -> next_ip=8, target=8+5=13
        // 8-9: LoadLocal 0 (2 bytes) - true path
        // 10: Return (1 byte) - true path exits
        // 11-12: PushLongSmall 0 (2 bytes) - false path
        // 13: Return (1 byte)
        //
        // When true: load local (77), return
        // When false: jump to 13, but we actually want to push 0 first

        let mut builder = ChunkBuilder::new("e2e_locals_control");
        builder.set_local_count(1);
        builder.emit_byte(Opcode::PushLongSmall, 77);   // offset 0-1
        builder.emit_byte(Opcode::StoreLocal, 0);       // offset 2-3
        builder.emit(Opcode::PushTrue);                 // offset 4
        builder.emit_u16(Opcode::JumpIfFalse, 3);       // offset 5-7, jump to offset 11 (8+3)
        builder.emit_byte(Opcode::LoadLocal, 0);        // offset 8-9, true: load 77
        builder.emit(Opcode::Return);                    // offset 10, true: return 77
        builder.emit_byte(Opcode::PushLongSmall, 0);    // offset 11-12, false: push 0
        builder.emit(Opcode::Return);                    // offset 13, false: return 0
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk));

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 77, "Expected local value from true branch");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_many_locals() {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Test with multiple locals (up to 8)
        let mut builder = ChunkBuilder::new("e2e_many_locals");
        builder.set_local_count(8);

        // Store values 0-7 in locals 0-7
        for i in 0..8u8 {
            builder.emit_byte(Opcode::PushLongSmall, i);
            builder.emit_byte(Opcode::StoreLocal, i);
        }

        // Sum all locals: 0 + 1 + 2 + 3 + 4 + 5 + 6 + 7 = 28
        builder.emit_byte(Opcode::LoadLocal, 0);
        for i in 1..8u8 {
            builder.emit_byte(Opcode::LoadLocal, i);
            builder.emit(Opcode::Add);
        }
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 28, "Expected sum 0+1+2+3+4+5+6+7 = 28");
    }

    // =========================================================================
    // Stage 5: JumpIfNil and JumpIfError Tests
    // =========================================================================

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_can_compile_jump_if_nil() {
        // JumpIfNil should be compilable in Stage 5
        let mut builder = ChunkBuilder::new("can_compile_jump_if_nil");
        builder.emit(Opcode::PushNil);
        builder.emit_u16(Opcode::JumpIfNil, 2);  // Jump forward 2 bytes
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk));
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_can_compile_jump_if_error() {
        // JumpIfError should be compilable in Stage 5
        let mut builder = ChunkBuilder::new("can_compile_jump_if_error");
        builder.emit(Opcode::PushNil);  // Use nil as placeholder (no PushError opcode)
        builder.emit_u16(Opcode::JumpIfError, 2);  // Jump forward 2 bytes
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk));
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_jump_if_nil_takes_jump() {
        // When value is nil, JumpIfNil should take the jump
        // Layout:
        // 0: PushNil (1 byte)
        // 1-3: JumpIfNil +5 (3 bytes) -> next_ip=4, target=4+5=9
        // 4-5: PushLongSmall 42 (2 bytes) - fallthrough path (skipped)
        // 6-8: Jump +2 (3 bytes) -> skip else
        // 9-10: PushLongSmall 99 (2 bytes) - jump target
        // 11: Return (1 byte)
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_jump_if_nil_takes");
        builder.emit(Opcode::PushNil);                    // offset 0
        builder.emit_u16(Opcode::JumpIfNil, 5);           // offset 1-3, jump to 9 if nil
        builder.emit_byte(Opcode::PushLongSmall, 42);     // offset 4-5, not nil path (skipped)
        builder.emit_u16(Opcode::Jump, 2);                // offset 6-8, skip else
        builder.emit_byte(Opcode::PushLongSmall, 99);     // offset 9-10, nil path
        builder.emit(Opcode::Return);                     // offset 11
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk));

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 99, "Expected nil branch result 99");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_jump_if_nil_fallthrough() {
        // When value is not nil, JumpIfNil should NOT jump (fallthrough)
        // Same layout but with non-nil value
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_jump_if_nil_fallthrough");
        builder.emit_byte(Opcode::PushLongSmall, 1);      // offset 0-1, not nil
        builder.emit_u16(Opcode::JumpIfNil, 5);           // offset 2-4, jump to 10 if nil
        builder.emit_byte(Opcode::PushLongSmall, 42);     // offset 5-6, not nil path
        builder.emit_u16(Opcode::Jump, 2);                // offset 7-9, skip else
        builder.emit_byte(Opcode::PushLongSmall, 99);     // offset 10-11, nil path (skipped)
        builder.emit(Opcode::Return);                     // offset 12
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk));

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 42, "Expected non-nil branch result 42");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_jump_if_nil_with_bool_false() {
        // False is NOT nil, so should fallthrough
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_jump_if_nil_bool_false");
        builder.emit(Opcode::PushFalse);                  // offset 0, False (not nil)
        builder.emit_u16(Opcode::JumpIfNil, 5);           // offset 1-3, jump to 9 if nil
        builder.emit_byte(Opcode::PushLongSmall, 42);     // offset 4-5, not nil path
        builder.emit_u16(Opcode::Jump, 2);                // offset 6-8, skip else
        builder.emit_byte(Opcode::PushLongSmall, 99);     // offset 9-10, nil path (skipped)
        builder.emit(Opcode::Return);                     // offset 11
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 42, "False is not nil, expected fallthrough to 42");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_jump_if_error_no_error() {
        // When value is not an error, JumpIfError should NOT jump (fallthrough)
        // Note: JumpIfError PEEKS, doesn't pop
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Simple test: push non-error, JumpIfError peeks and continues (value stays on stack)
        let mut builder = ChunkBuilder::new("e2e_jump_if_error_no_error");
        builder.emit_byte(Opcode::PushLongSmall, 42);     // offset 0-1, not an error
        builder.emit_u16(Opcode::JumpIfError, 1);         // offset 2-4, peek: stack still has 42
        builder.emit(Opcode::Return);                     // offset 5
        builder.emit(Opcode::Return);                     // offset 6 (jump target, should not reach)
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk));

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        // The value should still be on stack (peek doesn't pop)
        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 42, "JumpIfError should peek, not pop; stack should have 42");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_jump_if_nil_pops_value() {
        // JumpIfNil should POP the value being tested
        // Test: push 42, push nil, JumpIfNil takes jump (pops nil), return 42
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Layout:
        // 0-1: PushLongSmall 42 (2 bytes)
        // 2: PushNil (1 byte)
        // 3-5: JumpIfNil +2 (3 bytes) -> next_ip=6, target=8 (Jump over the not-nil-path)
        // 6-7: PushLongSmall 99 (2 bytes) - not reached since nil
        // 8: Return (1 byte) -> returns 42 (nil was popped, 42 stays on stack)
        let mut builder = ChunkBuilder::new("e2e_jump_if_nil_pops");
        builder.emit_byte(Opcode::PushLongSmall, 42);     // offset 0-1
        builder.emit(Opcode::PushNil);                    // offset 2
        builder.emit_u16(Opcode::JumpIfNil, 2);           // offset 3-5, pops nil, jumps to 8
        builder.emit_byte(Opcode::PushLongSmall, 99);     // offset 6-7, not-nil path (skipped)
        builder.emit(Opcode::Return);                     // offset 8
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        // Stack should be: [42] after nil is popped by JumpIfNil
        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 42, "JumpIfNil should pop; result should be 42");
    }

    // =========================================================================
    // Stage 6: Type Predicate Tests
    // =========================================================================

    #[test]
    fn test_jit_can_compile_is_variable() {
        let mut builder = ChunkBuilder::new("test_is_variable");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit(Opcode::IsVariable);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "IsVariable should be Stage 1 compilable"
        );
    }

    #[test]
    fn test_jit_can_compile_is_sexpr() {
        let mut builder = ChunkBuilder::new("test_is_sexpr");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit(Opcode::IsSExpr);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "IsSExpr should be Stage 1 compilable"
        );
    }

    #[test]
    fn test_jit_can_compile_is_symbol() {
        let mut builder = ChunkBuilder::new("test_is_symbol");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit(Opcode::IsSymbol);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "IsSymbol should be Stage 1 compilable"
        );
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_is_variable_false_for_long() {
        // IsVariable(42) should return false
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_is_variable_long");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit(Opcode::IsVariable);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_bool(), "Expected Bool result, got: {:?}", result);
        assert!(!result.as_bool(), "IsVariable(Long) should return false");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_is_sexpr_false_for_long() {
        // IsSExpr(42) should return false
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_is_sexpr_long");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit(Opcode::IsSExpr);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_bool(), "Expected Bool result, got: {:?}", result);
        assert!(!result.as_bool(), "IsSExpr(Long) should return false");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_is_symbol_false_for_long() {
        // IsSymbol(42) should return false
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_is_symbol_long");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit(Opcode::IsSymbol);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_bool(), "Expected Bool result, got: {:?}", result);
        assert!(!result.as_bool(), "IsSymbol(Long) should return false");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_is_variable_false_for_bool() {
        // IsVariable(true) should return false
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_is_variable_bool");
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::IsVariable);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_bool(), "Expected Bool result, got: {:?}", result);
        assert!(!result.as_bool(), "IsVariable(Bool) should return false");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_is_sexpr_false_for_nil() {
        // IsSExpr(nil) should return false
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_is_sexpr_nil");
        builder.emit(Opcode::PushNil);
        builder.emit(Opcode::IsSExpr);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_bool(), "Expected Bool result, got: {:?}", result);
        assert!(!result.as_bool(), "IsSExpr(Nil) should return false");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_is_symbol_false_for_unit() {
        // IsSymbol(unit) should return false
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_is_symbol_unit");
        builder.emit(Opcode::PushUnit);
        builder.emit(Opcode::IsSymbol);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_bool(), "Expected Bool result, got: {:?}", result);
        assert!(!result.as_bool(), "IsSymbol(Unit) should return false");
    }

    // =========================================================================
    // Stage 7: Stack Operations and Negation Tests
    // =========================================================================

    #[test]
    fn test_jit_can_compile_pop() {
        let mut builder = ChunkBuilder::new("test_pop");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit_byte(Opcode::PushLongSmall, 10);
        builder.emit(Opcode::Pop);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "Pop should be Stage 1 compilable"
        );
    }

    #[test]
    fn test_jit_can_compile_dup() {
        let mut builder = ChunkBuilder::new("test_dup");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit(Opcode::Dup);
        builder.emit(Opcode::Add);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "Dup should be Stage 1 compilable"
        );
    }

    #[test]
    fn test_jit_can_compile_swap() {
        let mut builder = ChunkBuilder::new("test_swap");
        builder.emit_byte(Opcode::PushLongSmall, 10);
        builder.emit_byte(Opcode::PushLongSmall, 5);
        builder.emit(Opcode::Swap);
        builder.emit(Opcode::Sub);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "Swap should be Stage 1 compilable"
        );
    }

    #[test]
    fn test_jit_can_compile_neg() {
        let mut builder = ChunkBuilder::new("test_neg");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit(Opcode::Neg);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "Neg should be Stage 1 compilable"
        );
    }

    #[test]
    fn test_jit_can_compile_dup_n() {
        let mut builder = ChunkBuilder::new("test_dup_n");
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit_byte(Opcode::PushLongSmall, 2);
        builder.emit_byte(Opcode::DupN, 2);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "DupN should be Stage 1 compilable"
        );
    }

    #[test]
    fn test_jit_can_compile_pop_n() {
        let mut builder = ChunkBuilder::new("test_pop_n");
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit_byte(Opcode::PushLongSmall, 2);
        builder.emit_byte(Opcode::PushLongSmall, 3);
        builder.emit_byte(Opcode::PopN, 2);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "PopN should be Stage 1 compilable"
        );
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_pop() {
        // Push 42, push 10, pop -> result should be 42
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_pop");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit_byte(Opcode::PushLongSmall, 10);
        builder.emit(Opcode::Pop);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 42, "Pop should discard 10, leaving 42");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_dup() {
        // Push 21, dup, add -> result should be 42
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_dup");
        builder.emit_byte(Opcode::PushLongSmall, 21);
        builder.emit(Opcode::Dup);
        builder.emit(Opcode::Add);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 42, "Dup should duplicate 21, then add: 21+21=42");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_swap() {
        // Push 10, push 5, swap, sub -> 5 - 10 = -5
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_swap");
        builder.emit_byte(Opcode::PushLongSmall, 10);
        builder.emit_byte(Opcode::PushLongSmall, 5);
        builder.emit(Opcode::Swap);
        builder.emit(Opcode::Sub);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        // Stack after push: [10, 5]
        // Stack after swap: [5, 10]
        // Sub pops b=10, a=5, computes a-b = 5-10 = -5
        assert_eq!(result.as_long(), -5, "Swap should swap, then sub: 5-10=-5");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_neg_positive() {
        // Neg(42) = -42
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_neg_positive");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit(Opcode::Neg);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), -42, "Neg(42) should be -42");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_neg_negative() {
        // Neg(-10) = 10
        // We can't push negative with PushLongSmall, so use 10 - 20 = -10, then negate
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_neg_negative");
        builder.emit_byte(Opcode::PushLongSmall, 10);
        builder.emit_byte(Opcode::PushLongSmall, 20);
        builder.emit(Opcode::Sub); // 10 - 20 = -10
        builder.emit(Opcode::Neg); // Neg(-10) = 10
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 10, "Neg(-10) should be 10");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_dup_n() {
        // Push 1, push 2, DupN 2 -> [1, 2, 1, 2], add (2+1=3), add (3+2=5), add (5+1=6)
        // Actually let's do simpler: push 3, push 4, DupN 2 -> [3, 4, 3, 4]
        // Then add top two: [3, 4, 7], then return 7
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_dup_n");
        builder.emit_byte(Opcode::PushLongSmall, 3);
        builder.emit_byte(Opcode::PushLongSmall, 4);
        builder.emit_byte(Opcode::DupN, 2); // Stack: [3, 4, 3, 4]
        builder.emit(Opcode::Add); // Stack: [3, 4, 7]
        builder.emit(Opcode::Return); // Returns 7
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 7, "DupN should duplicate top 2, then add: 3+4=7");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_pop_n() {
        // Push 100, push 1, push 2, push 3, PopN 3 -> [100], return 100
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_pop_n");
        builder.emit_byte(Opcode::PushLongSmall, 100);
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit_byte(Opcode::PushLongSmall, 2);
        builder.emit_byte(Opcode::PushLongSmall, 3);
        builder.emit_byte(Opcode::PopN, 3); // Pop 3, 2, 1 -> [100]
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 100, "PopN 3 should leave 100");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_neg_zero() {
        // Neg(0) = 0
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_neg_zero");
        builder.emit_byte(Opcode::PushLongSmall, 0);
        builder.emit(Opcode::Neg);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 0, "Neg(0) should be 0");
    }

    // =========================================================================
    // Stage 8: More Arithmetic and Stack Operations Tests
    // =========================================================================

    #[test]
    fn test_jit_can_compile_abs() {
        let mut builder = ChunkBuilder::new("test_abs");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit(Opcode::Abs);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "Abs should be Stage 1 compilable"
        );
    }

    #[test]
    fn test_jit_can_compile_mod() {
        let mut builder = ChunkBuilder::new("test_mod");
        builder.emit_byte(Opcode::PushLongSmall, 17);
        builder.emit_byte(Opcode::PushLongSmall, 5);
        builder.emit(Opcode::Mod);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "Mod should be Stage 1 compilable"
        );
    }

    #[test]
    fn test_jit_can_compile_floor_div() {
        let mut builder = ChunkBuilder::new("test_floor_div");
        builder.emit_byte(Opcode::PushLongSmall, 17);
        builder.emit_byte(Opcode::PushLongSmall, 5);
        builder.emit(Opcode::FloorDiv);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "FloorDiv should be Stage 1 compilable"
        );
    }

    #[test]
    fn test_jit_can_compile_rot3() {
        let mut builder = ChunkBuilder::new("test_rot3");
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit_byte(Opcode::PushLongSmall, 2);
        builder.emit_byte(Opcode::PushLongSmall, 3);
        builder.emit(Opcode::Rot3);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "Rot3 should be Stage 1 compilable"
        );
    }

    #[test]
    fn test_jit_can_compile_over() {
        let mut builder = ChunkBuilder::new("test_over");
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit_byte(Opcode::PushLongSmall, 2);
        builder.emit(Opcode::Over);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "Over should be Stage 1 compilable"
        );
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_abs_positive() {
        // Abs(42) = 42
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_abs_positive");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit(Opcode::Abs);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 42, "Abs(42) should be 42");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_abs_negative() {
        // Abs(-10) = 10
        // Create -10 via subtraction: 5 - 15 = -10
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_abs_negative");
        builder.emit_byte(Opcode::PushLongSmall, 5);
        builder.emit_byte(Opcode::PushLongSmall, 15);
        builder.emit(Opcode::Sub); // 5 - 15 = -10
        builder.emit(Opcode::Abs); // Abs(-10) = 10
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 10, "Abs(-10) should be 10");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_mod() {
        // 17 % 5 = 2
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_mod");
        builder.emit_byte(Opcode::PushLongSmall, 17);
        builder.emit_byte(Opcode::PushLongSmall, 5);
        builder.emit(Opcode::Mod);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 2, "17 % 5 should be 2");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_floor_div_stage8() {
        // 17 / 5 = 3 (floor)
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_floor_div_s8");
        builder.emit_byte(Opcode::PushLongSmall, 17);
        builder.emit_byte(Opcode::PushLongSmall, 5);
        builder.emit(Opcode::FloorDiv);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 3, "17 / 5 (floor) should be 3");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_rot3() {
        // Stack: [1, 2, 3] -> Rot3 -> [3, 1, 2]
        // Return top: 2
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_rot3");
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit_byte(Opcode::PushLongSmall, 2);
        builder.emit_byte(Opcode::PushLongSmall, 3);
        builder.emit(Opcode::Rot3); // [3, 1, 2]
        builder.emit(Opcode::Return); // Returns 2
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 2, "Rot3([1,2,3]) top should be 2");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_rot3_verify_order() {
        // Verify the full rotation by adding: [a,b,c] -> [c,a,b]
        // Push 10, 20, 30 -> Rot3 -> [30, 10, 20]
        // Then: 20 - 10 = 10 (top - second), pop the 10, return 30
        // Actually let's do: sub twice to verify order
        // [30, 10, 20] -> sub(10, 20) = 10-20 = -10 -> [30, -10] -> return -10
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_rot3_verify");
        builder.emit_byte(Opcode::PushLongSmall, 10); // a
        builder.emit_byte(Opcode::PushLongSmall, 20); // b
        builder.emit_byte(Opcode::PushLongSmall, 30); // c
        builder.emit(Opcode::Rot3); // [c=30, a=10, b=20]
        builder.emit(Opcode::Sub); // a - b = 10 - 20 = -10 -> [30, -10]
        builder.emit(Opcode::Return); // Returns -10
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), -10, "Rot3 should make a-b = 10-20 = -10");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_over() {
        // Stack: [1, 2] -> Over -> [1, 2, 1]
        // Add: 1 + 2 = 3 -> [1, 3] -> Return 3
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_over");
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit_byte(Opcode::PushLongSmall, 2);
        builder.emit(Opcode::Over); // [1, 2, 1]
        builder.emit(Opcode::Add); // [1, 3]
        builder.emit(Opcode::Return); // Returns 3
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 3, "Over should copy 1, then add: 2+1=3");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_abs_zero() {
        // Abs(0) = 0
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_abs_zero");
        builder.emit_byte(Opcode::PushLongSmall, 0);
        builder.emit(Opcode::Abs);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 0, "Abs(0) should be 0");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_mod_exact() {
        // 15 % 5 = 0
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_mod_exact");
        builder.emit_byte(Opcode::PushLongSmall, 15);
        builder.emit_byte(Opcode::PushLongSmall, 5);
        builder.emit(Opcode::Mod);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 0, "15 % 5 should be 0");
    }

    // =========================================================================
    // Stage 9: Boolean Logic Tests
    // =========================================================================

    #[test]
    fn test_jit_can_compile_boolean_ops_stage9() {
        // And
        let mut builder = ChunkBuilder::new("bool_and");
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::PushFalse);
        builder.emit(Opcode::And);
        builder.emit(Opcode::Return);
        assert!(JitCompiler::can_compile_stage1(&builder.build()), "And should be compilable");

        // Or
        let mut builder = ChunkBuilder::new("bool_or");
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::PushFalse);
        builder.emit(Opcode::Or);
        builder.emit(Opcode::Return);
        assert!(JitCompiler::can_compile_stage1(&builder.build()), "Or should be compilable");

        // Not
        let mut builder = ChunkBuilder::new("bool_not");
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::Not);
        builder.emit(Opcode::Return);
        assert!(JitCompiler::can_compile_stage1(&builder.build()), "Not should be compilable");

        // Xor
        let mut builder = ChunkBuilder::new("bool_xor");
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::Xor);
        builder.emit(Opcode::Return);
        assert!(JitCompiler::can_compile_stage1(&builder.build()), "Xor should be compilable");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_and() {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // True AND True = True
        let mut builder = ChunkBuilder::new("and_tt");
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::And);
        builder.emit(Opcode::Return);
        let chunk = builder.build();
        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());
        assert!(result.is_bool(), "Expected Bool result");
        assert_eq!(result.as_bool(), true, "True AND True should be True");

        // True AND False = False
        let mut builder = ChunkBuilder::new("and_tf");
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::PushFalse);
        builder.emit(Opcode::And);
        builder.emit(Opcode::Return);
        let chunk = builder.build();
        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());
        assert!(result.is_bool(), "Expected Bool result");
        assert_eq!(result.as_bool(), false, "True AND False should be False");

        // False AND False = False
        let mut builder = ChunkBuilder::new("and_ff");
        builder.emit(Opcode::PushFalse);
        builder.emit(Opcode::PushFalse);
        builder.emit(Opcode::And);
        builder.emit(Opcode::Return);
        let chunk = builder.build();
        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());
        assert!(result.is_bool(), "Expected Bool result");
        assert_eq!(result.as_bool(), false, "False AND False should be False");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_or() {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // True OR False = True
        let mut builder = ChunkBuilder::new("or_tf");
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::PushFalse);
        builder.emit(Opcode::Or);
        builder.emit(Opcode::Return);
        let chunk = builder.build();
        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());
        assert!(result.is_bool(), "Expected Bool result");
        assert_eq!(result.as_bool(), true, "True OR False should be True");

        // False OR False = False
        let mut builder = ChunkBuilder::new("or_ff");
        builder.emit(Opcode::PushFalse);
        builder.emit(Opcode::PushFalse);
        builder.emit(Opcode::Or);
        builder.emit(Opcode::Return);
        let chunk = builder.build();
        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());
        assert!(result.is_bool(), "Expected Bool result");
        assert_eq!(result.as_bool(), false, "False OR False should be False");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_not() {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // NOT True = False
        let mut builder = ChunkBuilder::new("not_t");
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::Not);
        builder.emit(Opcode::Return);
        let chunk = builder.build();
        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());
        assert!(result.is_bool(), "Expected Bool result");
        assert_eq!(result.as_bool(), false, "NOT True should be False");

        // NOT False = True
        let mut builder = ChunkBuilder::new("not_f");
        builder.emit(Opcode::PushFalse);
        builder.emit(Opcode::Not);
        builder.emit(Opcode::Return);
        let chunk = builder.build();
        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());
        assert!(result.is_bool(), "Expected Bool result");
        assert_eq!(result.as_bool(), true, "NOT False should be True");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_xor() {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // True XOR True = False
        let mut builder = ChunkBuilder::new("xor_tt");
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::Xor);
        builder.emit(Opcode::Return);
        let chunk = builder.build();
        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());
        assert!(result.is_bool(), "Expected Bool result");
        assert_eq!(result.as_bool(), false, "True XOR True should be False");

        // True XOR False = True
        let mut builder = ChunkBuilder::new("xor_tf");
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::PushFalse);
        builder.emit(Opcode::Xor);
        builder.emit(Opcode::Return);
        let chunk = builder.build();
        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());
        assert!(result.is_bool(), "Expected Bool result");
        assert_eq!(result.as_bool(), true, "True XOR False should be True");

        // False XOR False = False
        let mut builder = ChunkBuilder::new("xor_ff");
        builder.emit(Opcode::PushFalse);
        builder.emit(Opcode::PushFalse);
        builder.emit(Opcode::Xor);
        builder.emit(Opcode::Return);
        let chunk = builder.build();
        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());
        assert!(result.is_bool(), "Expected Bool result");
        assert_eq!(result.as_bool(), false, "False XOR False should be False");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_boolean_chain() {
        // Complex: (True AND False) OR (NOT False) = False OR True = True
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("bool_chain");
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::PushFalse);
        builder.emit(Opcode::And);     // False
        builder.emit(Opcode::PushFalse);
        builder.emit(Opcode::Not);     // True
        builder.emit(Opcode::Or);      // False OR True = True
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_bool(), "Expected Bool result");
        assert_eq!(result.as_bool(), true, "(T AND F) OR (NOT F) should be True");
    }

    // =========================================================================
    // Stage 11: StructEq Tests
    // =========================================================================

    #[test]
    fn test_jit_can_compile_struct_eq() {
        let mut builder = ChunkBuilder::new("struct_eq");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit(Opcode::StructEq);
        builder.emit(Opcode::Return);
        assert!(JitCompiler::can_compile_stage1(&builder.build()), "StructEq should be compilable");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_struct_eq_equal_longs() {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // 42 == 42 structurally
        let mut builder = ChunkBuilder::new("struct_eq_longs");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit(Opcode::StructEq);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_bool(), "Expected Bool result");
        assert_eq!(result.as_bool(), true, "42 == 42 should be true");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_struct_eq_different_longs() {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // 42 == 43 structurally
        let mut builder = ChunkBuilder::new("struct_eq_diff");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit_byte(Opcode::PushLongSmall, 43);
        builder.emit(Opcode::StructEq);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_bool(), "Expected Bool result");
        assert_eq!(result.as_bool(), false, "42 == 43 should be false");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_struct_eq_bools() {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // True == True
        let mut builder = ChunkBuilder::new("struct_eq_true");
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::StructEq);
        builder.emit(Opcode::Return);
        let chunk = builder.build();
        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());
        assert!(result.is_bool(), "Expected Bool result");
        assert_eq!(result.as_bool(), true, "True == True should be true");

        // True == False
        let mut builder = ChunkBuilder::new("struct_eq_tf");
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::PushFalse);
        builder.emit(Opcode::StructEq);
        builder.emit(Opcode::Return);
        let chunk = builder.build();
        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());
        assert!(result.is_bool(), "Expected Bool result");
        assert_eq!(result.as_bool(), false, "True == False should be false");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_struct_eq_nil() {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Nil == Nil
        let mut builder = ChunkBuilder::new("struct_eq_nil");
        builder.emit(Opcode::PushNil);
        builder.emit(Opcode::PushNil);
        builder.emit(Opcode::StructEq);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_bool(), "Expected Bool result");
        assert_eq!(result.as_bool(), true, "Nil == Nil should be true");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_struct_eq_different_types() {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // 1 == True (different types)
        let mut builder = ChunkBuilder::new("struct_eq_diff_types");
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::StructEq);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_bool(), "Expected Bool result");
        assert_eq!(result.as_bool(), false, "Long(1) == Bool(True) should be false");
    }

    // =========================================================================
    // Stage 10: More Pow Tests (extend existing tests)
    // =========================================================================

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_pow_zero_exp() {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // 5^0 = 1
        let mut builder = ChunkBuilder::new("pow_5_0");
        builder.emit_byte(Opcode::PushLongSmall, 5);
        builder.emit_byte(Opcode::PushLongSmall, 0);
        builder.emit(Opcode::Pow);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 1, "5^0 should be 1");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_pow_one_exp() {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // 7^1 = 7
        let mut builder = ChunkBuilder::new("pow_7_1");
        builder.emit_byte(Opcode::PushLongSmall, 7);
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit(Opcode::Pow);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 7, "7^1 should be 7");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_pow_small() {
        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // 3^4 = 81
        let mut builder = ChunkBuilder::new("pow_3_4");
        builder.emit_byte(Opcode::PushLongSmall, 3);
        builder.emit_byte(Opcode::PushLongSmall, 4);
        builder.emit(Opcode::Pow);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");
        let result = exec_jit(code_ptr, chunk.constants());

        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 81, "3^4 should be 81");
    }

    // =========================================================================
    // Stage 13: Value Creation Tests
    // =========================================================================

    #[test]
    fn test_jit_can_compile_push_empty() {
        let mut builder = ChunkBuilder::new("push_empty");
        builder.emit(Opcode::PushEmpty);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "PushEmpty should be Stage 13 compilable"
        );
    }

    #[test]
    fn test_jit_can_compile_push_atom() {
        use crate::backend::MettaValue;

        let mut builder = ChunkBuilder::new("push_atom");
        let idx = builder.add_constant(MettaValue::Atom("foo".to_string()));
        builder.emit_u16(Opcode::PushAtom, idx);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "PushAtom should be Stage 13 compilable"
        );
    }

    #[test]
    fn test_jit_can_compile_push_string() {
        use crate::backend::MettaValue;

        let mut builder = ChunkBuilder::new("push_string");
        let idx = builder.add_constant(MettaValue::String("hello".to_string()));
        builder.emit_u16(Opcode::PushString, idx);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "PushString should be Stage 13 compilable"
        );
    }

    #[test]
    fn test_jit_can_compile_push_variable() {
        use crate::backend::MettaValue;

        let mut builder = ChunkBuilder::new("push_variable");
        let idx = builder.add_constant(MettaValue::Atom("$x".to_string()));
        builder.emit_u16(Opcode::PushVariable, idx);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "PushVariable should be Stage 13 compilable"
        );
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_push_empty() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_push_empty");
        builder.emit(Opcode::PushEmpty);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        // PushEmpty returns a heap pointer (TAG_HEAP) to an empty S-expression
        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_heap(), "Expected Heap result for empty S-expr, got: {:?}", result);
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_push_atom() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_push_atom");
        let idx = builder.add_constant(MettaValue::Atom("foo".to_string()));
        builder.emit_u16(Opcode::PushAtom, idx);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        // PushAtom returns a heap pointer to the Atom
        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_heap(), "Expected Heap result for atom, got: {:?}", result);
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_push_string() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_push_string");
        let idx = builder.add_constant(MettaValue::String("hello world".to_string()));
        builder.emit_u16(Opcode::PushString, idx);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        // PushString returns a heap pointer to the String
        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_heap(), "Expected Heap result for string, got: {:?}", result);
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_push_variable() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("e2e_push_variable");
        let idx = builder.add_constant(MettaValue::Atom("$x".to_string()));
        builder.emit_u16(Opcode::PushVariable, idx);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        // PushVariable returns a heap pointer to the Variable (which is an Atom starting with $)
        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_heap(), "Expected Heap result for variable, got: {:?}", result);
    }

    // =========================================================================
    // Stage 14: S-Expression Operations Tests
    // =========================================================================

    #[test]
    fn test_jit_can_compile_get_head() {
        let mut builder = ChunkBuilder::new("get_head");
        builder.emit(Opcode::PushEmpty);  // Dummy S-expr - real test needs heap value
        builder.emit(Opcode::GetHead);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "GetHead should be Stage 14 compilable"
        );
    }

    #[test]
    fn test_jit_can_compile_get_tail() {
        let mut builder = ChunkBuilder::new("get_tail");
        builder.emit(Opcode::PushEmpty);
        builder.emit(Opcode::GetTail);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "GetTail should be Stage 14 compilable"
        );
    }

    #[test]
    fn test_jit_can_compile_get_arity() {
        let mut builder = ChunkBuilder::new("get_arity");
        builder.emit(Opcode::PushEmpty);
        builder.emit(Opcode::GetArity);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "GetArity should be Stage 14 compilable"
        );
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_get_arity_empty() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create chunk that pushes an empty S-expr and gets its arity (should be 0)
        let mut builder = ChunkBuilder::new("e2e_get_arity_empty");
        builder.emit(Opcode::PushEmpty);
        builder.emit(Opcode::GetArity);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        // GetArity on empty S-expr should return 0
        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_long(), "Expected Long result for arity, got: {:?}", result);
        assert_eq!(result.as_long(), 0, "Arity of empty S-expr should be 0");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_get_arity_nonempty() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create chunk with a 3-element S-expr in constant pool
        let mut builder = ChunkBuilder::new("e2e_get_arity_nonempty");
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Long(1),
            MettaValue::Long(2),
            MettaValue::Long(3),
        ]);
        let idx = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::GetArity);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        // GetArity on 3-element S-expr should return 3
        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_long(), "Expected Long result for arity, got: {:?}", result);
        assert_eq!(result.as_long(), 3, "Arity of (1 2 3) should be 3");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_get_head() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create chunk with a 3-element S-expr where head is Long(42)
        let mut builder = ChunkBuilder::new("e2e_get_head");
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Long(42),
            MettaValue::Long(2),
            MettaValue::Long(3),
        ]);
        let idx = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::GetHead);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        // GetHead on (42 2 3) should return 42
        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_long(), "Expected Long result for head, got: {:?}", result);
        assert_eq!(result.as_long(), 42, "Head of (42 2 3) should be 42");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_get_tail() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create chunk with a 3-element S-expr, then get tail
        let mut builder = ChunkBuilder::new("e2e_get_tail");
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Long(1),
            MettaValue::Long(2),
            MettaValue::Long(3),
        ]);
        let idx = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::GetTail);
        builder.emit(Opcode::GetArity);  // Get arity of tail to verify it's (2 3) = length 2
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        // GetTail on (1 2 3) gives (2 3) which has arity 2
        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_long(), "Expected Long result for arity of tail, got: {:?}", result);
        assert_eq!(result.as_long(), 2, "Arity of tail of (1 2 3) should be 2");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_get_tail_get_head() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create chunk: get head of tail of (1 2 3) = 2
        let mut builder = ChunkBuilder::new("e2e_get_tail_get_head");
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Long(1),
            MettaValue::Long(2),
            MettaValue::Long(3),
        ]);
        let idx = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::GetTail);    // (2 3)
        builder.emit(Opcode::GetHead);    // 2
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        // Head of tail of (1 2 3) should be 2
        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_long(), "Expected Long result, got: {:?}", result);
        assert_eq!(result.as_long(), 2, "Head of tail of (1 2 3) should be 2");
    }

    // =========================================================================
    // Stage 14b: GetElement Tests
    // =========================================================================

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_can_compile_get_element() {
        use crate::backend::MettaValue;

        let mut builder = ChunkBuilder::new("get_element");
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Long(10),
            MettaValue::Long(20),
            MettaValue::Long(30),
        ]);
        let idx = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit_byte(Opcode::GetElement, 1); // Get element at index 1
        builder.emit(Opcode::Return);

        let chunk = builder.build();
        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "GetElement should be compilable"
        );
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_get_element_first() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create chunk: get element at index 0 of (10 20 30) = 10
        let mut builder = ChunkBuilder::new("e2e_get_element_first");
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Long(10),
            MettaValue::Long(20),
            MettaValue::Long(30),
        ]);
        let idx = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit_byte(Opcode::GetElement, 0); // Get element at index 0
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        // Element at index 0 of (10 20 30) should be 10
        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_long(), "Expected Long result, got: {:?}", result);
        assert_eq!(result.as_long(), 10, "Element at index 0 of (10 20 30) should be 10");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_get_element_middle() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create chunk: get element at index 1 of (10 20 30) = 20
        let mut builder = ChunkBuilder::new("e2e_get_element_middle");
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Long(10),
            MettaValue::Long(20),
            MettaValue::Long(30),
        ]);
        let idx = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit_byte(Opcode::GetElement, 1); // Get element at index 1
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        // Element at index 1 of (10 20 30) should be 20
        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_long(), "Expected Long result, got: {:?}", result);
        assert_eq!(result.as_long(), 20, "Element at index 1 of (10 20 30) should be 20");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_get_element_last() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create chunk: get element at index 2 of (10 20 30) = 30
        let mut builder = ChunkBuilder::new("e2e_get_element_last");
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Long(10),
            MettaValue::Long(20),
            MettaValue::Long(30),
        ]);
        let idx = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit_byte(Opcode::GetElement, 2); // Get element at index 2
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        // Element at index 2 of (10 20 30) should be 30
        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_long(), "Expected Long result, got: {:?}", result);
        assert_eq!(result.as_long(), 30, "Element at index 2 of (10 20 30) should be 30");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_get_element_combined() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create chunk: (get-element 1) + (get-element 2) of (10 20 30) = 20 + 30 = 50
        let mut builder = ChunkBuilder::new("e2e_get_element_combined");
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Long(10),
            MettaValue::Long(20),
            MettaValue::Long(30),
        ]);
        let idx = builder.add_constant(sexpr.clone());
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit_byte(Opcode::GetElement, 1); // Get element at index 1 -> 20
        let idx2 = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, idx2);
        builder.emit_byte(Opcode::GetElement, 2); // Get element at index 2 -> 30
        builder.emit(Opcode::Add); // 20 + 30 = 50
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        // Element[1] + Element[2] of (10 20 30) should be 20 + 30 = 50
        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_long(), "Expected Long result, got: {:?}", result);
        assert_eq!(result.as_long(), 50, "Element[1] + Element[2] of (10 20 30) should be 50");
    }

    // =========================================================================
    // Phase 1: Type Operations Tests (GetType, CheckType, IsType)
    // =========================================================================

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_get_type_long() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create chunk: GetType(42) should return "Number" atom
        let mut builder = ChunkBuilder::new("e2e_get_type_long");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit(Opcode::GetType);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        // GetType(42) should return "Number" atom
        let result = JitValue::from_raw(result_bits as u64);
        let metta_val = unsafe { result.to_metta() };
        match metta_val {
            MettaValue::Atom(s) => assert_eq!(s, "Number", "GetType(Long) should return 'Number'"),
            other => panic!("Expected Atom('Number'), got: {:?}", other),
        }
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_get_type_bool() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create chunk: GetType(True) should return "Bool" atom
        let mut builder = ChunkBuilder::new("e2e_get_type_bool");
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::GetType);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        // GetType(True) should return "Bool" atom
        let result = JitValue::from_raw(result_bits as u64);
        let metta_val = unsafe { result.to_metta() };
        match metta_val {
            MettaValue::Atom(s) => assert_eq!(s, "Bool", "GetType(Bool) should return 'Bool'"),
            other => panic!("Expected Atom('Bool'), got: {:?}", other),
        }
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_get_type_sexpr() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create chunk: GetType((1 2 3)) should return "Expression" atom
        let mut builder = ChunkBuilder::new("e2e_get_type_sexpr");
        let sexpr = MettaValue::SExpr(vec![
            MettaValue::Long(1),
            MettaValue::Long(2),
            MettaValue::Long(3),
        ]);
        let idx = builder.add_constant(sexpr);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::GetType);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        // GetType((1 2 3)) should return "Expression" atom
        let result = JitValue::from_raw(result_bits as u64);
        let metta_val = unsafe { result.to_metta() };
        match metta_val {
            MettaValue::Atom(s) => assert_eq!(s, "Expression", "GetType(SExpr) should return 'Expression'"),
            other => panic!("Expected Atom('Expression'), got: {:?}", other),
        }
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_check_type_match() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create chunk: CheckType(42, "Number") should return True
        let mut builder = ChunkBuilder::new("e2e_check_type_match");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        let type_atom = MettaValue::Atom("Number".to_string());
        let idx = builder.add_constant(type_atom);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::CheckType);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        // CheckType(42, "Number") should return True
        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_bool(), "Expected Bool result, got: {:?}", result);
        assert_eq!(result.as_bool(), true, "CheckType(Long, 'Number') should return true");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_check_type_mismatch() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create chunk: CheckType(42, "Bool") should return False
        let mut builder = ChunkBuilder::new("e2e_check_type_mismatch");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        let type_atom = MettaValue::Atom("Bool".to_string());
        let idx = builder.add_constant(type_atom);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::CheckType);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        // CheckType(42, "Bool") should return False
        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_bool(), "Expected Bool result, got: {:?}", result);
        assert_eq!(result.as_bool(), false, "CheckType(Long, 'Bool') should return false");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_is_type_match() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create chunk: IsType(True, "Bool") should return True
        let mut builder = ChunkBuilder::new("e2e_is_type_match");
        builder.emit(Opcode::PushTrue);
        let type_atom = MettaValue::Atom("Bool".to_string());
        let idx = builder.add_constant(type_atom);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::IsType);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        // IsType(True, "Bool") should return True
        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_bool(), "Expected Bool result, got: {:?}", result);
        assert_eq!(result.as_bool(), true, "IsType(Bool, 'Bool') should return true");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_check_type_variable_matches_any() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create chunk: CheckType(42, $T) should return True (type variables match anything)
        let mut builder = ChunkBuilder::new("e2e_check_type_var");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        // Variables are represented as Atom with $ prefix
        let type_var = MettaValue::Atom("$T".to_string());
        let idx = builder.add_constant(type_var);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::CheckType);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        // CheckType(42, $T) should return True (type variables are polymorphic)
        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_bool(), "Expected Bool result, got: {:?}", result);
        assert_eq!(result.as_bool(), true, "CheckType with type variable should return true");
    }

    // =========================================================================
    // Phase J: AssertType Tests
    // =========================================================================

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_assert_type_match() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create chunk: AssertType(42, "Number") should return 42 (value stays on stack)
        let mut builder = ChunkBuilder::new("e2e_assert_type_match");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        let type_atom = MettaValue::Atom("Number".to_string());
        let idx = builder.add_constant(type_atom);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::AssertType);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout on type match");

        // AssertType(42, "Number") should return 42 (value unchanged)
        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_long(), "Expected Long result, got: {:?}", result);
        assert_eq!(result.as_long(), 42, "AssertType should return the original value");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_assert_type_mismatch() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create chunk: AssertType(42, "Bool") should signal bailout (type mismatch)
        let mut builder = ChunkBuilder::new("e2e_assert_type_mismatch");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        let type_atom = MettaValue::Atom("Bool".to_string());
        let idx = builder.add_constant(type_atom);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::AssertType);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let _result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        // AssertType(42, "Bool") should signal bailout due to type mismatch
        assert!(ctx.bailout, "JIT execution should bailout on type mismatch");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_assert_type_variable_matches_any() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create chunk: AssertType(42, $T) should return 42 (type variables match anything)
        let mut builder = ChunkBuilder::new("e2e_assert_type_var");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        let type_var = MettaValue::Atom("$T".to_string());
        let idx = builder.add_constant(type_var);
        builder.emit_u16(Opcode::PushConstant, idx);
        builder.emit(Opcode::AssertType);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout with type variable");

        // AssertType(42, $T) should return 42 (type variables are polymorphic)
        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_long(), "Expected Long result, got: {:?}", result);
        assert_eq!(result.as_long(), 42, "AssertType with type variable should return the value");
    }

    #[test]
    fn test_jit_can_compile_assert_type() {
        // Test that AssertType is recognized as JIT compilable
        let mut builder = ChunkBuilder::new("assert_type_compilable");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit_byte(Opcode::PushLongSmall, 1); // Placeholder type
        builder.emit(Opcode::AssertType);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "AssertType should be Stage 1 compilable"
        );
    }

    // =========================================================================
    // Phase 2a: MakeSExpr and ConsAtom Tests
    // =========================================================================

    #[test]
    fn test_jit_can_compile_make_sexpr() {
        // Test that MakeSExpr is recognized as Stage 1 compilable
        let mut builder = ChunkBuilder::new("make_sexpr_test");
        builder.emit_byte(Opcode::PushLongSmall, 10);
        builder.emit_byte(Opcode::PushLongSmall, 20);
        builder.emit_byte(Opcode::PushLongSmall, 30);
        builder.emit_byte(Opcode::MakeSExpr, 3);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "MakeSExpr should be compilable"
        );
    }

    #[test]
    fn test_jit_can_compile_cons_atom() {
        // Test that ConsAtom is recognized as Stage 1 compilable
        let mut builder = ChunkBuilder::new("cons_atom_test");
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit(Opcode::PushNil);
        builder.emit(Opcode::ConsAtom);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "ConsAtom should be compilable"
        );
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_make_sexpr_empty() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create chunk: MakeSExpr(0) -> ()
        let mut builder = ChunkBuilder::new("make_sexpr_empty");
        builder.emit_byte(Opcode::MakeSExpr, 0);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        let result = JitValue::from_raw(result_bits as u64);
        let metta = unsafe { result.to_metta() };

        match &metta {
            MettaValue::SExpr(items) => {
                assert!(items.is_empty(), "Expected empty S-expression, got {:?}", metta);
            }
            _ => panic!("Expected SExpr, got: {:?}", metta),
        }
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_make_sexpr_single() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create chunk: Push 42, MakeSExpr(1) -> (42)
        let mut builder = ChunkBuilder::new("make_sexpr_single");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit_byte(Opcode::MakeSExpr, 1);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        let result = JitValue::from_raw(result_bits as u64);
        let metta = unsafe { result.to_metta() };

        match &metta {
            MettaValue::SExpr(items) => {
                assert_eq!(items.len(), 1, "Expected 1 element");
                match &items[0] {
                    MettaValue::Long(v) => assert_eq!(*v, 42),
                    _ => panic!("Expected Long, got: {:?}", items[0]),
                }
            }
            _ => panic!("Expected SExpr, got: {:?}", metta),
        }
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_make_sexpr_multiple() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create chunk: Push 10, 20, 30, MakeSExpr(3) -> (10 20 30)
        let mut builder = ChunkBuilder::new("make_sexpr_multiple");
        builder.emit_byte(Opcode::PushLongSmall, 10);
        builder.emit_byte(Opcode::PushLongSmall, 20);
        builder.emit_byte(Opcode::PushLongSmall, 30);
        builder.emit_byte(Opcode::MakeSExpr, 3);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        let result = JitValue::from_raw(result_bits as u64);
        let metta = unsafe { result.to_metta() };

        match &metta {
            MettaValue::SExpr(items) => {
                assert_eq!(items.len(), 3, "Expected 3 elements");
                match (&items[0], &items[1], &items[2]) {
                    (MettaValue::Long(a), MettaValue::Long(b), MettaValue::Long(c)) => {
                        assert_eq!((*a, *b, *c), (10, 20, 30));
                    }
                    _ => panic!("Expected three Longs, got: {:?}", items),
                }
            }
            _ => panic!("Expected SExpr, got: {:?}", metta),
        }
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_cons_atom_to_nil() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create chunk: Push 1, PushNil, ConsAtom -> (1)
        let mut builder = ChunkBuilder::new("cons_atom_to_nil");
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit(Opcode::PushNil);
        builder.emit(Opcode::ConsAtom);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        let result = JitValue::from_raw(result_bits as u64);
        let metta = unsafe { result.to_metta() };

        match &metta {
            MettaValue::SExpr(items) => {
                assert_eq!(items.len(), 1, "Expected 1 element");
                match &items[0] {
                    MettaValue::Long(v) => assert_eq!(*v, 1),
                    _ => panic!("Expected Long, got: {:?}", items[0]),
                }
            }
            _ => panic!("Expected SExpr, got: {:?}", metta),
        }
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_cons_atom_to_sexpr() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create chunk: Create (2 3), then cons 1 to get (1 2 3)
        // Stack: Push 2, Push 3, MakeSExpr(2) -> (2 3)
        // Then: Push 1, Swap, ConsAtom -> (1 2 3)
        let mut builder = ChunkBuilder::new("cons_atom_to_sexpr");
        builder.emit_byte(Opcode::PushLongSmall, 2);
        builder.emit_byte(Opcode::PushLongSmall, 3);
        builder.emit_byte(Opcode::MakeSExpr, 2);
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit(Opcode::Swap);
        builder.emit(Opcode::ConsAtom);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        let result = JitValue::from_raw(result_bits as u64);
        let metta = unsafe { result.to_metta() };

        match &metta {
            MettaValue::SExpr(items) => {
                assert_eq!(items.len(), 3, "Expected 3 elements, got {:?}", items);
                match (&items[0], &items[1], &items[2]) {
                    (MettaValue::Long(a), MettaValue::Long(b), MettaValue::Long(c)) => {
                        assert_eq!((*a, *b, *c), (1, 2, 3));
                    }
                    _ => panic!("Expected three Longs, got: {:?}", items),
                }
            }
            _ => panic!("Expected SExpr, got: {:?}", metta),
        }
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_make_sexpr_nested() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create chunk: ((1 2) (3 4))
        let mut builder = ChunkBuilder::new("make_sexpr_nested");
        // Create (1 2)
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit_byte(Opcode::PushLongSmall, 2);
        builder.emit_byte(Opcode::MakeSExpr, 2);
        // Create (3 4)
        builder.emit_byte(Opcode::PushLongSmall, 3);
        builder.emit_byte(Opcode::PushLongSmall, 4);
        builder.emit_byte(Opcode::MakeSExpr, 2);
        // Create ((1 2) (3 4))
        builder.emit_byte(Opcode::MakeSExpr, 2);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        let result = JitValue::from_raw(result_bits as u64);
        let metta = unsafe { result.to_metta() };

        match &metta {
            MettaValue::SExpr(items) => {
                assert_eq!(items.len(), 2, "Expected 2 elements");
                // Check first element is (1 2)
                match &items[0] {
                    MettaValue::SExpr(inner) => {
                        assert_eq!(inner.len(), 2);
                        match (&inner[0], &inner[1]) {
                            (MettaValue::Long(a), MettaValue::Long(b)) => {
                                assert_eq!((*a, *b), (1, 2));
                            }
                            _ => panic!("Expected Longs in inner, got: {:?}", inner),
                        }
                    }
                    _ => panic!("Expected inner SExpr, got: {:?}", items[0]),
                }
                // Check second element is (3 4)
                match &items[1] {
                    MettaValue::SExpr(inner) => {
                        assert_eq!(inner.len(), 2);
                        match (&inner[0], &inner[1]) {
                            (MettaValue::Long(a), MettaValue::Long(b)) => {
                                assert_eq!((*a, *b), (3, 4));
                            }
                            _ => panic!("Expected Longs in inner, got: {:?}", inner),
                        }
                    }
                    _ => panic!("Expected inner SExpr, got: {:?}", items[1]),
                }
            }
            _ => panic!("Expected SExpr, got: {:?}", metta),
        }
    }

    // =========================================================================
    // Phase 2b: PushUri, MakeList, MakeQuote Tests
    // =========================================================================

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_can_compile_push_uri() {
        use crate::backend::MettaValue;

        // Test that PushUri is compilable
        let mut builder = ChunkBuilder::new("push_uri");
        let idx = builder.add_constant(MettaValue::Atom("test-uri".to_string()));
        builder.emit_u16(Opcode::PushUri, idx);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk), "PushUri should be compilable");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_can_compile_make_list() {
        // Test that MakeList is compilable
        let mut builder = ChunkBuilder::new("make_list");
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit_byte(Opcode::PushLongSmall, 2);
        builder.emit_byte(Opcode::MakeList, 2);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk), "MakeList should be compilable");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_can_compile_make_quote() {
        // Test that MakeQuote is compilable
        let mut builder = ChunkBuilder::new("make_quote");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit(Opcode::MakeQuote);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk), "MakeQuote should be compilable");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_push_uri() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("push_uri");
        let idx = builder.add_constant(MettaValue::Atom("http://example.com".to_string()));
        builder.emit_u16(Opcode::PushUri, idx);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        let result = JitValue::from_raw(result_bits as u64);
        let metta = unsafe { result.to_metta() };

        match &metta {
            MettaValue::Atom(s) => assert_eq!(s, "http://example.com"),
            _ => panic!("Expected Atom, got: {:?}", metta),
        }
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_make_list_empty() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("make_list_empty");
        builder.emit_byte(Opcode::MakeList, 0);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        let result = JitValue::from_raw(result_bits as u64);
        let metta = unsafe { result.to_metta() };

        // Empty list is Nil
        assert!(matches!(metta, MettaValue::Nil), "Expected Nil, got: {:?}", metta);
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_make_list_single() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("make_list_single");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit_byte(Opcode::MakeList, 1);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        let result = JitValue::from_raw(result_bits as u64);
        let metta = unsafe { result.to_metta() };

        // Should be (Cons 42 Nil)
        match &metta {
            MettaValue::SExpr(items) => {
                assert_eq!(items.len(), 3, "Expected (Cons elem Nil) structure");
                match (&items[0], &items[1], &items[2]) {
                    (MettaValue::Atom(cons), MettaValue::Long(v), MettaValue::Nil) => {
                        assert_eq!(cons, "Cons");
                        assert_eq!(*v, 42);
                    }
                    _ => panic!("Expected (Cons 42 Nil), got: {:?}", items),
                }
            }
            _ => panic!("Expected SExpr, got: {:?}", metta),
        }
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_make_list_multiple() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("make_list_multiple");
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit_byte(Opcode::PushLongSmall, 2);
        builder.emit_byte(Opcode::PushLongSmall, 3);
        builder.emit_byte(Opcode::MakeList, 3);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        let result = JitValue::from_raw(result_bits as u64);
        let metta = unsafe { result.to_metta() };

        // Should be (Cons 1 (Cons 2 (Cons 3 Nil)))
        // Just check the outer structure
        match &metta {
            MettaValue::SExpr(items) => {
                assert_eq!(items.len(), 3, "Expected (Cons elem rest) structure");
                match &items[0] {
                    MettaValue::Atom(s) => assert_eq!(s, "Cons"),
                    _ => panic!("Expected Cons atom, got: {:?}", items[0]),
                }
                match &items[1] {
                    MettaValue::Long(v) => assert_eq!(*v, 1),
                    _ => panic!("Expected Long 1, got: {:?}", items[1]),
                }
            }
            _ => panic!("Expected SExpr, got: {:?}", metta),
        }
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_make_quote() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("make_quote");
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit(Opcode::MakeQuote);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        let result = JitValue::from_raw(result_bits as u64);
        let metta = unsafe { result.to_metta() };

        // Should be (quote 42)
        match &metta {
            MettaValue::SExpr(items) => {
                assert_eq!(items.len(), 2, "Expected (quote value)");
                match (&items[0], &items[1]) {
                    (MettaValue::Atom(q), MettaValue::Long(v)) => {
                        assert_eq!(q, "quote");
                        assert_eq!(*v, 42);
                    }
                    _ => panic!("Expected (quote 42), got: {:?}", items),
                }
            }
            _ => panic!("Expected SExpr, got: {:?}", metta),
        }
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_make_quote_nested() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create (quote (1 2))
        let mut builder = ChunkBuilder::new("make_quote_nested");
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit_byte(Opcode::PushLongSmall, 2);
        builder.emit_byte(Opcode::MakeSExpr, 2);
        builder.emit(Opcode::MakeQuote);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bailout");

        let result = JitValue::from_raw(result_bits as u64);
        let metta = unsafe { result.to_metta() };

        // Should be (quote (1 2))
        match &metta {
            MettaValue::SExpr(items) => {
                assert_eq!(items.len(), 2, "Expected (quote expr)");
                match &items[0] {
                    MettaValue::Atom(q) => assert_eq!(q, "quote"),
                    _ => panic!("Expected quote atom, got: {:?}", items[0]),
                }
                match &items[1] {
                    MettaValue::SExpr(inner) => {
                        assert_eq!(inner.len(), 2);
                        match (&inner[0], &inner[1]) {
                            (MettaValue::Long(a), MettaValue::Long(b)) => {
                                assert_eq!((*a, *b), (1, 2));
                            }
                            _ => panic!("Expected (1 2), got: {:?}", inner),
                        }
                    }
                    _ => panic!("Expected SExpr inside quote, got: {:?}", items[1]),
                }
            }
            _ => panic!("Expected SExpr, got: {:?}", metta),
        }
    }

    // =====================================================================
    // Phase 3: Call/TailCall Tests
    // =====================================================================

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_can_compile_call() {
        use crate::backend::MettaValue;

        // Test that Call opcode is compilable
        let mut builder = ChunkBuilder::new("call_test");
        let head_idx = builder.add_constant(MettaValue::Atom("my-func".to_string()));
        builder.emit_byte(Opcode::PushLongSmall, 1); // arg1
        builder.emit_byte(Opcode::PushLongSmall, 2); // arg2
        builder.emit_call(head_idx, 2); // Call with 2 args
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "Call should be compilable in Stage 1"
        );
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_can_compile_tail_call() {
        use crate::backend::MettaValue;

        // Test that TailCall opcode is compilable
        let mut builder = ChunkBuilder::new("tail_call_test");
        let head_idx = builder.add_constant(MettaValue::Atom("my-func".to_string()));
        builder.emit_byte(Opcode::PushLongSmall, 1); // arg1
        builder.emit_tail_call(head_idx, 2); // TailCall with 2 args
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "TailCall should be compilable in Stage 1"
        );
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_call_with_bailout() {
        use crate::backend::bytecode::jit::{JitBailoutReason, JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create: (my-func 1 2)
        let mut builder = ChunkBuilder::new("call_bailout");
        let head_idx = builder.add_constant(MettaValue::Atom("my-func".to_string()));
        builder.emit_byte(Opcode::PushLongSmall, 1); // arg1
        builder.emit_byte(Opcode::PushLongSmall, 2); // arg2
        builder.emit_call(head_idx, 2);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(code_ptr) };
        let _result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        // Call should always set bailout
        assert!(ctx.bailout, "Call should set bailout flag");
        assert_eq!(
            ctx.bailout_reason,
            JitBailoutReason::Call,
            "Bailout reason should be Call"
        );

        // The result should be a heap pointer to the call expression
        // For bailout, the VM will dispatch this expression
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_call_no_args() {
        use crate::backend::bytecode::jit::{JitBailoutReason, JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create: (my-func) with no args
        let mut builder = ChunkBuilder::new("call_no_args");
        let head_idx = builder.add_constant(MettaValue::Atom("my-func".to_string()));
        builder.emit_call(head_idx, 0); // 0 args
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(code_ptr) };
        let _result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(ctx.bailout, "Call with no args should set bailout flag");
        assert_eq!(
            ctx.bailout_reason,
            JitBailoutReason::Call,
            "Bailout reason should be Call"
        );
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_execute_tail_call_with_bailout() {
        use crate::backend::bytecode::jit::{JitBailoutReason, JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create: (my-func 1) as tail call
        let mut builder = ChunkBuilder::new("tail_call_bailout");
        let head_idx = builder.add_constant(MettaValue::Atom("my-func".to_string()));
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit_tail_call(head_idx, 1);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(code_ptr) };
        let _result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(ctx.bailout, "TailCall should set bailout flag");
        assert_eq!(
            ctx.bailout_reason,
            JitBailoutReason::TailCall,
            "Bailout reason should be TailCall"
        );
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_call_builds_correct_expression() {
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create: (add 5 3)
        let mut builder = ChunkBuilder::new("call_expr");
        let head_idx = builder.add_constant(MettaValue::Atom("add".to_string()));
        builder.emit_byte(Opcode::PushLongSmall, 5);
        builder.emit_byte(Opcode::PushLongSmall, 3);
        builder.emit_call(head_idx, 2);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(code_ptr) };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(ctx.bailout, "Call should bailout");

        // The result should be the call expression
        let result = JitValue::from_raw(result_bits as u64);
        let metta = unsafe { result.to_metta() };

        match &metta {
            MettaValue::SExpr(items) => {
                assert_eq!(items.len(), 3, "Expected (add 5 3)");
                match &items[0] {
                    MettaValue::Atom(s) => assert_eq!(s, "add"),
                    _ => panic!("Expected 'add' atom, got: {:?}", items[0]),
                }
                match &items[1] {
                    MettaValue::Long(n) => assert_eq!(*n, 5),
                    _ => panic!("Expected 5, got: {:?}", items[1]),
                }
                match &items[2] {
                    MettaValue::Long(n) => assert_eq!(*n, 3),
                    _ => panic!("Expected 3, got: {:?}", items[2]),
                }
            }
            _ => panic!("Expected SExpr, got: {:?}", metta),
        }
    }

    // =========================================================================
    // Phase 2.3: Call with Rule Dispatch Integration Tests
    // =========================================================================
    // Tests that verify Call/TailCall opcodes construct valid expressions
    // for rule dispatch integration with the VM.
    // =========================================================================

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_call_with_mixed_argument_types() {
        // Test Call with mixed argument types: atom, long, bool, nested sexpr
        // This validates the expression structure for rule pattern matching
        use crate::backend::bytecode::jit::{JitBailoutReason, JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create: (process atom-arg 42 True (nested 1 2))
        let mut builder = ChunkBuilder::new("call_mixed_args");
        let head_idx = builder.add_constant(MettaValue::Atom("process".to_string()));
        let atom_idx = builder.add_constant(MettaValue::Atom("atom-arg".to_string()));
        let nested_idx = builder.add_constant(MettaValue::SExpr(vec![
            MettaValue::Atom("nested".to_string()),
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]));

        // Push arguments in order
        builder.emit_u16(Opcode::PushConstant, atom_idx); // atom-arg
        builder.emit_byte(Opcode::PushLongSmall, 42); // 42
        builder.emit(Opcode::PushTrue); // True
        builder.emit_u16(Opcode::PushConstant, nested_idx); // (nested 1 2)
        builder.emit_call(head_idx, 4); // Call with 4 args
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(code_ptr) };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        // Verify bailout occurred
        assert!(ctx.bailout, "Call should set bailout flag");
        assert_eq!(
            ctx.bailout_reason,
            JitBailoutReason::Call,
            "Bailout reason should be Call"
        );

        // Verify the constructed expression
        let result = JitValue::from_raw(result_bits as u64);
        let metta = unsafe { result.to_metta() };

        match &metta {
            MettaValue::SExpr(items) => {
                assert_eq!(items.len(), 5, "Expected (process atom-arg 42 True (nested 1 2))");

                // Head: process
                match &items[0] {
                    MettaValue::Atom(s) => assert_eq!(s, "process"),
                    _ => panic!("Expected 'process' atom"),
                }

                // Arg 1: atom-arg
                match &items[1] {
                    MettaValue::Atom(s) => assert_eq!(s, "atom-arg"),
                    _ => panic!("Expected 'atom-arg' atom"),
                }

                // Arg 2: 42
                match &items[2] {
                    MettaValue::Long(n) => assert_eq!(*n, 42),
                    _ => panic!("Expected Long(42)"),
                }

                // Arg 3: True
                match &items[3] {
                    MettaValue::Bool(b) => assert!(*b),
                    _ => panic!("Expected Bool(true)"),
                }

                // Arg 4: (nested 1 2)
                match &items[4] {
                    MettaValue::SExpr(nested) => {
                        assert_eq!(nested.len(), 3);
                        match &nested[0] {
                            MettaValue::Atom(s) => assert_eq!(s, "nested"),
                            _ => panic!("Expected 'nested' atom"),
                        }
                        match &nested[1] {
                            MettaValue::Long(n) => assert_eq!(*n, 1),
                            _ => panic!("Expected Long(1)"),
                        }
                        match &nested[2] {
                            MettaValue::Long(n) => assert_eq!(*n, 2),
                            _ => panic!("Expected Long(2)"),
                        }
                    }
                    _ => panic!("Expected nested SExpr"),
                }
            }
            _ => panic!("Expected SExpr"),
        }
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_call_expression_valid_for_rule_pattern() {
        // Test that JIT-constructed expressions match expected rule patterns
        // Pattern: (fib $n) -> matches (fib 10)
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create: (fib 10) - a typical recursive call pattern
        let mut builder = ChunkBuilder::new("call_fib");
        let head_idx = builder.add_constant(MettaValue::Atom("fib".to_string()));
        builder.emit_byte(Opcode::PushLongSmall, 10);
        builder.emit_call(head_idx, 1);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(code_ptr) };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        let result = JitValue::from_raw(result_bits as u64);
        let call_expr = unsafe { result.to_metta() };

        // Verify the expression can be matched against a rule pattern
        // Rule pattern: (= (fib $n) ...)
        let pattern = MettaValue::SExpr(vec![
            MettaValue::Atom("fib".to_string()),
            MettaValue::Atom("$n".to_string()),
        ]);

        // The call expression should have the same structure
        match (&call_expr, &pattern) {
            (MettaValue::SExpr(expr_items), MettaValue::SExpr(pattern_items)) => {
                assert_eq!(expr_items.len(), pattern_items.len(), "Arity mismatch");

                // Head should match exactly
                match (&expr_items[0], &pattern_items[0]) {
                    (MettaValue::Atom(e), MettaValue::Atom(p)) => {
                        assert_eq!(e, p, "Head atoms should match");
                    }
                    _ => panic!("Expected both heads to be atoms"),
                }

                // Argument should be a Long that would bind to $n
                match &expr_items[1] {
                    MettaValue::Long(n) => assert_eq!(*n, 10, "Argument should be 10"),
                    _ => panic!("Expected Long argument"),
                }
            }
            _ => panic!("Both should be SExprs"),
        }
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_tail_call_preserves_tco_flag() {
        // Test that TailCall sets the correct bailout reason for TCO
        use crate::backend::bytecode::jit::{JitBailoutReason, JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create a tail call with multiple arguments
        let mut builder = ChunkBuilder::new("tail_call_tco");
        let head_idx = builder.add_constant(MettaValue::Atom("recurse".to_string()));
        builder.emit_byte(Opcode::PushLongSmall, 5); // countdown
        builder.emit_byte(Opcode::PushLongSmall, 100); // accumulator
        builder.emit_tail_call(head_idx, 2);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(code_ptr) };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        // Verify bailout with TailCall reason (for TCO)
        assert!(ctx.bailout, "TailCall should set bailout flag");
        assert_eq!(
            ctx.bailout_reason,
            JitBailoutReason::TailCall,
            "Bailout reason should be TailCall for TCO"
        );

        // Verify expression structure
        let result = JitValue::from_raw(result_bits as u64);
        let metta = unsafe { result.to_metta() };

        match &metta {
            MettaValue::SExpr(items) => {
                assert_eq!(items.len(), 3, "Expected (recurse 5 100)");
                match &items[0] {
                    MettaValue::Atom(s) => assert_eq!(s, "recurse"),
                    _ => panic!("Expected 'recurse' atom"),
                }
                match &items[1] {
                    MettaValue::Long(n) => assert_eq!(*n, 5),
                    _ => panic!("Expected Long(5)"),
                }
                match &items[2] {
                    MettaValue::Long(n) => assert_eq!(*n, 100),
                    _ => panic!("Expected Long(100)"),
                }
            }
            _ => panic!("Expected SExpr"),
        }
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_call_with_zero_args_returns_head_only() {
        // Test Call with no arguments returns just the head in an SExpr
        use crate::backend::bytecode::jit::{JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create: (get-value) - call with no args
        let mut builder = ChunkBuilder::new("call_zero_args");
        let head_idx = builder.add_constant(MettaValue::Atom("get-value".to_string()));
        builder.emit_call(head_idx, 0); // 0 args
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(code_ptr) };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        let result = JitValue::from_raw(result_bits as u64);
        let metta = unsafe { result.to_metta() };

        match &metta {
            MettaValue::SExpr(items) => {
                assert_eq!(items.len(), 1, "Expected (get-value) with just head");
                match &items[0] {
                    MettaValue::Atom(s) => assert_eq!(s, "get-value"),
                    _ => panic!("Expected 'get-value' atom"),
                }
            }
            _ => panic!("Expected SExpr"),
        }
    }

    // =========================================================================
    // Phase A: Binding Operations Tests
    // =========================================================================

    #[test]
    fn test_can_compile_binding_opcodes() {
        // Phase A: Binding operations should be compilable
        let mut builder = ChunkBuilder::new("test_bindings");
        builder.emit(Opcode::PushBindingFrame);
        builder.emit_u16(Opcode::StoreBinding, 0); // Store to binding 0
        builder.emit_u16(Opcode::LoadBinding, 0);  // Load from binding 0
        builder.emit_u16(Opcode::HasBinding, 0);   // Check binding 0
        builder.emit(Opcode::PopBindingFrame);
        builder.emit(Opcode::ClearBindings);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(JitCompiler::can_compile_stage1(&chunk), "Binding opcodes should be compilable");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_binding_frame_operations() {
        use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};
        use crate::backend::models::metta_value::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Build bytecode: Push frame, store 42, load it, return
        let mut builder = ChunkBuilder::new("test_binding_frame");
        builder.emit(Opcode::PushBindingFrame);       // Create new frame
        builder.emit_byte(Opcode::PushLongSmall, 42); // Push 42
        builder.emit_u16(Opcode::StoreBinding, 0);    // Store to binding index 0
        builder.emit_u16(Opcode::LoadBinding, 0);     // Load from binding index 0
        builder.emit(Opcode::PopBindingFrame);        // Pop frame
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        // Compile to native code
        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        // Set up JIT context with binding support
        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        // Allocate binding frames array (capacity 16)
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                64,
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Set up binding frames pointer and capacity
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = 16;

        // Initialize root binding frame
        unsafe { ctx.init_root_binding_frame() };

        // Execute JIT code
        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(code_ptr) };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        // Result should be 42 (the loaded binding value)
        let result = JitValue::from_raw(result_bits as u64);
        let metta = unsafe { result.to_metta() };

        match metta {
            MettaValue::Long(n) => assert_eq!(n, 42, "Expected 42 from binding"),
            other => panic!("Expected Long(42), got: {:?}", other),
        }
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_has_binding() {
        use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};
        use crate::backend::models::metta_value::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Build bytecode: Push frame, check binding (should be false), store, check again (should be true)
        let mut builder = ChunkBuilder::new("test_has_binding");
        builder.emit(Opcode::PushBindingFrame);       // Create new frame
        builder.emit_byte(Opcode::PushLongSmall, 99); // Push 99
        builder.emit_u16(Opcode::StoreBinding, 5);    // Store to binding index 5
        builder.emit_u16(Opcode::HasBinding, 5);      // Check binding 5 exists
        builder.emit(Opcode::PopBindingFrame);        // Pop frame
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        // Compile to native code
        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        // Set up JIT context with binding support
        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        // Allocate binding frames array (capacity 16)
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                64,
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Set up binding frames pointer and capacity
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = 16;

        // Initialize root binding frame
        unsafe { ctx.init_root_binding_frame() };

        // Execute JIT code
        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(code_ptr) };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        // Result should be True (binding exists)
        let result = JitValue::from_raw(result_bits as u64);
        let metta = unsafe { result.to_metta() };

        match metta {
            MettaValue::Bool(b) => assert!(b, "Expected True for has_binding after store"),
            other => panic!("Expected Bool(true), got: {:?}", other),
        }
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_clear_bindings() {
        use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};
        use crate::backend::models::metta_value::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Build bytecode: Store binding, clear, check (should be false)
        let mut builder = ChunkBuilder::new("test_clear_bindings");
        builder.emit(Opcode::PushBindingFrame);       // Create new frame
        builder.emit_byte(Opcode::PushLongSmall, 77); // Push 77
        builder.emit_u16(Opcode::StoreBinding, 3);    // Store to binding index 3
        builder.emit(Opcode::ClearBindings);          // Clear all bindings
        builder.emit_u16(Opcode::HasBinding, 3);      // Check binding 3 exists
        builder.emit(Opcode::PopBindingFrame);        // Pop frame
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        // Compile to native code
        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        // Set up JIT context with binding support
        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        // Allocate binding frames array (capacity 16)
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                64,
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Set up binding frames pointer and capacity
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = 16;

        // Initialize root binding frame
        unsafe { ctx.init_root_binding_frame() };

        // Execute JIT code
        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(code_ptr) };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        // Result should be False (binding cleared)
        let result = JitValue::from_raw(result_bits as u64);
        let metta = unsafe { result.to_metta() };

        match metta {
            MettaValue::Bool(b) => assert!(!b, "Expected False for has_binding after clear"),
            other => panic!("Expected Bool(false), got: {:?}", other),
        }
    }

    // =========================================================================
    // Phase B: Pattern Matching Tests
    // =========================================================================

    #[test]
    fn test_can_compile_pattern_matching_opcodes() {
        // Test that all pattern matching opcodes are compilable
        let mut builder = ChunkBuilder::new("pattern_match_test");

        // Match opcode (no operands)
        builder.emit(Opcode::PushNil); // pattern
        builder.emit(Opcode::PushNil); // value
        builder.emit(Opcode::Match);

        // MatchBind opcode (no operands)
        builder.emit(Opcode::PushNil); // pattern
        builder.emit(Opcode::PushNil); // value
        builder.emit(Opcode::MatchBind);

        // MatchHead opcode (1 byte operand)
        builder.emit(Opcode::PushNil); // expr
        builder.emit_byte(Opcode::MatchHead, 0); // expected head idx

        // MatchArity opcode (1 byte operand)
        builder.emit(Opcode::PushNil); // expr
        builder.emit_byte(Opcode::MatchArity, 3); // expected arity

        // Unify opcode (no operands)
        builder.emit(Opcode::PushNil); // a
        builder.emit(Opcode::PushNil); // b
        builder.emit(Opcode::Unify);

        // UnifyBind opcode (no operands)
        builder.emit(Opcode::PushNil); // a
        builder.emit(Opcode::PushNil); // b
        builder.emit(Opcode::UnifyBind);

        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "Pattern matching opcodes should be compilable"
        );
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_pattern_match_simple() {
        use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Test: Match 42 against 42 should return True
        let mut builder = ChunkBuilder::new("match_simple");
        builder.emit_byte(Opcode::PushLongSmall, 42); // pattern
        builder.emit_byte(Opcode::PushLongSmall, 42); // value
        builder.emit(Opcode::Match);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = 16;
        unsafe { ctx.init_root_binding_frame() };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(code_ptr) };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        let result = JitValue::from_raw(result_bits as u64);
        let metta = unsafe { result.to_metta() };

        match metta {
            MettaValue::Bool(b) => assert!(b, "Matching 42 against 42 should return True"),
            other => panic!("Expected Bool(true), got: {:?}", other),
        }
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_pattern_match_mismatch() {
        use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Test: Match 42 against 99 should return False
        let mut builder = ChunkBuilder::new("match_mismatch");
        builder.emit_byte(Opcode::PushLongSmall, 42); // pattern
        builder.emit_byte(Opcode::PushLongSmall, 99); // value
        builder.emit(Opcode::Match);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = 16;
        unsafe { ctx.init_root_binding_frame() };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(code_ptr) };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        let result = JitValue::from_raw(result_bits as u64);
        let metta = unsafe { result.to_metta() };

        match metta {
            MettaValue::Bool(b) => assert!(!b, "Matching 42 against 99 should return False"),
            other => panic!("Expected Bool(false), got: {:?}", other),
        }
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_unify_simple() {
        use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Test: Unify 42 with 42 should return True
        let mut builder = ChunkBuilder::new("unify_simple");
        builder.emit_byte(Opcode::PushLongSmall, 42); // a
        builder.emit_byte(Opcode::PushLongSmall, 42); // b
        builder.emit(Opcode::Unify);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = 16;
        unsafe { ctx.init_root_binding_frame() };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(code_ptr) };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        let result = JitValue::from_raw(result_bits as u64);
        let metta = unsafe { result.to_metta() };

        match metta {
            MettaValue::Bool(b) => assert!(b, "Unifying 42 with 42 should return True"),
            other => panic!("Expected Bool(true), got: {:?}", other),
        }
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_match_arity() {
        use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create an S-expression with 3 elements: (a b c)
        let mut builder = ChunkBuilder::new("match_arity");
        let a_idx = builder.add_constant(MettaValue::Atom("a".to_string()));
        let b_idx = builder.add_constant(MettaValue::Atom("b".to_string()));
        let c_idx = builder.add_constant(MettaValue::Atom("c".to_string()));

        // Push atoms and make S-expr
        builder.emit_u16(Opcode::PushAtom, a_idx);
        builder.emit_u16(Opcode::PushAtom, b_idx);
        builder.emit_u16(Opcode::PushAtom, c_idx);
        builder.emit_byte(Opcode::MakeSExpr, 3);

        // Check arity is 3
        builder.emit_byte(Opcode::MatchArity, 3);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = 16;
        unsafe { ctx.init_root_binding_frame() };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(code_ptr) };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        let result = JitValue::from_raw(result_bits as u64);
        let metta = unsafe { result.to_metta() };

        match metta {
            MettaValue::Bool(b) => assert!(b, "S-expr (a b c) should have arity 3"),
            other => panic!("Expected Bool(true), got: {:?}", other),
        }
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_match_head() {
        use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Create an S-expression: (foo bar baz)
        // Then check if head matches "foo"
        let mut builder = ChunkBuilder::new("match_head");
        let foo_idx = builder.add_constant(MettaValue::Atom("foo".to_string()));
        let bar_idx = builder.add_constant(MettaValue::Atom("bar".to_string()));
        let baz_idx = builder.add_constant(MettaValue::Atom("baz".to_string()));

        // Push atoms and make S-expr
        builder.emit_u16(Opcode::PushAtom, foo_idx);
        builder.emit_u16(Opcode::PushAtom, bar_idx);
        builder.emit_u16(Opcode::PushAtom, baz_idx);
        builder.emit_byte(Opcode::MakeSExpr, 3);

        // Match head against "foo" (index 0 in constant pool)
        builder.emit_byte(Opcode::MatchHead, foo_idx as u8);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = 16;
        unsafe { ctx.init_root_binding_frame() };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(code_ptr) };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        let result = JitValue::from_raw(result_bits as u64);
        let metta = unsafe { result.to_metta() };

        match metta {
            MettaValue::Bool(b) => assert!(b, "S-expr (foo bar baz) should have head 'foo'"),
            other => panic!("Expected Bool(true), got: {:?}", other),
        }
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_match_bind_variable_extraction() {
        // Test Phase 2.1: MatchBind with variable extraction
        // Pattern: ($x 2) matches against (1 2), should bind $x = 1
        use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};
        use crate::backend::MettaValue;

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("match_bind_var_extract");

        // Add constant for variable name $x (needed for binding lookup)
        let x_var_idx = builder.add_constant(MettaValue::Atom("$x".to_string()));

        // Create pattern S-expression: ($x 2)
        let pattern_sexpr = MettaValue::SExpr(vec![
            MettaValue::Atom("$x".to_string()),
            MettaValue::Long(2),
        ]);
        let pattern_idx = builder.add_constant(pattern_sexpr);

        // Create value S-expression: (1 2)
        let value_sexpr = MettaValue::SExpr(vec![
            MettaValue::Long(1),
            MettaValue::Long(2),
        ]);
        let value_idx = builder.add_constant(value_sexpr);

        // Bytecode:
        // 1. Push binding frame (for variable bindings)
        // 2. Push pattern
        // 3. Push value
        // 4. MatchBind (should bind $x to 1 and return true)
        // 5. Pop the bool result
        // 6. Load binding for $x (at index x_var_idx)
        // 7. Return the loaded value
        builder.emit(Opcode::PushBindingFrame);
        builder.emit_u16(Opcode::PushConstant, pattern_idx);
        builder.emit_u16(Opcode::PushConstant, value_idx);
        builder.emit(Opcode::MatchBind);
        builder.emit(Opcode::Pop); // Pop the bool result
        builder.emit_u16(Opcode::LoadBinding, x_var_idx);
        builder.emit(Opcode::PopBindingFrame);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = 16;
        unsafe { ctx.init_root_binding_frame() };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(code_ptr) };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        let result = JitValue::from_raw(result_bits as u64);
        let metta = unsafe { result.to_metta() };

        match metta {
            MettaValue::Long(n) => assert_eq!(n, 1, "MatchBind should have bound $x to 1"),
            other => panic!("Expected Long(1) from bound variable, got: {:?}", other),
        }
    }

    // =========================================================================
    // Phase D: Space Operations Tests
    // =========================================================================

    #[test]
    fn test_can_compile_space_opcodes() {
        // Phase D: Space operations should be compilable
        let mut builder = ChunkBuilder::new("test_space_ops");

        // SpaceAdd opcode (no operands)
        builder.emit(Opcode::PushNil); // space
        builder.emit(Opcode::PushNil); // atom
        builder.emit(Opcode::SpaceAdd);
        builder.emit(Opcode::Pop);

        // SpaceRemove opcode (no operands)
        builder.emit(Opcode::PushNil); // space
        builder.emit(Opcode::PushNil); // atom
        builder.emit(Opcode::SpaceRemove);
        builder.emit(Opcode::Pop);

        // SpaceGetAtoms opcode (no operands)
        builder.emit(Opcode::PushNil); // space
        builder.emit(Opcode::SpaceGetAtoms);
        builder.emit(Opcode::Pop);

        // SpaceMatch opcode (no operands)
        builder.emit(Opcode::PushNil); // space
        builder.emit(Opcode::PushNil); // pattern
        builder.emit(Opcode::PushNil); // template
        builder.emit(Opcode::SpaceMatch);

        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "Space operations should be compilable"
        );
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_space_add_returns_result() {
        // Test that SpaceAdd JIT compilation produces valid code
        // Note: With nil space, this should return error/fail gracefully
        use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("space_add");
        builder.emit(Opcode::PushNil); // space (nil = invalid)
        builder.emit(Opcode::PushNil); // atom
        builder.emit(Opcode::SpaceAdd);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = 16;
        unsafe { ctx.init_root_binding_frame() };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(code_ptr) };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        // With nil space, we expect unit (success) or error
        let result = JitValue::from_raw(result_bits as u64);
        // Just verify we got a valid JIT value back (no crash)
        assert!(result.is_bool() || result.is_unit() || result.is_nil() || result.is_error(),
            "SpaceAdd should return bool, unit, nil, or error");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_space_remove_returns_result() {
        // Test that SpaceRemove JIT compilation produces valid code
        use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("space_remove");
        builder.emit(Opcode::PushNil); // space (nil = invalid)
        builder.emit(Opcode::PushNil); // atom
        builder.emit(Opcode::SpaceRemove);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = 16;
        unsafe { ctx.init_root_binding_frame() };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(code_ptr) };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_bool() || result.is_unit() || result.is_nil() || result.is_error(),
            "SpaceRemove should return bool, unit, nil, or error");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_space_get_atoms_returns_result() {
        // Test that SpaceGetAtoms JIT compilation produces valid code
        use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("space_get_atoms");
        builder.emit(Opcode::PushNil); // space (nil = invalid)
        builder.emit(Opcode::SpaceGetAtoms);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = 16;
        unsafe { ctx.init_root_binding_frame() };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(code_ptr) };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        let result = JitValue::from_raw(result_bits as u64);
        // Nil space returns nil, empty list, or unit
        assert!(result.is_nil() || result.is_unit() || result.is_heap() || result.is_error(),
            "SpaceGetAtoms should return list, nil, unit, or error");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_space_match_returns_result() {
        // Test that SpaceMatch JIT compilation produces valid code
        use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("space_match");
        builder.emit(Opcode::PushNil); // space (nil = invalid)
        builder.emit(Opcode::PushNil); // pattern
        builder.emit(Opcode::PushNil); // template
        builder.emit(Opcode::SpaceMatch);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = 16;
        unsafe { ctx.init_root_binding_frame() };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(code_ptr) };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        let result = JitValue::from_raw(result_bits as u64);
        // Nil space returns nil, empty results, or unit
        assert!(result.is_nil() || result.is_unit() || result.is_heap() || result.is_error(),
            "SpaceMatch should return results list, nil, unit, or error");
    }

    // =========================================================================
    // Phase C: Rule Dispatch Tests
    // =========================================================================

    #[test]
    fn test_can_compile_rule_dispatch_opcodes() {
        // Phase C: Rule dispatch operations should be compilable
        let mut builder = ChunkBuilder::new("test_rule_dispatch_ops");

        // DispatchRules opcode - dispatches rules for an expression
        builder.emit(Opcode::PushNil); // expression to dispatch
        builder.emit(Opcode::DispatchRules);
        builder.emit(Opcode::Pop);

        // TryRule opcode with operand (rule index)
        builder.emit_u16(Opcode::TryRule, 0); // try rule at index 0
        builder.emit(Opcode::Pop);

        // NextRule opcode (no operands)
        builder.emit(Opcode::NextRule);

        // CommitRule opcode (no operands)
        builder.emit(Opcode::CommitRule);

        // FailRule opcode (no operands)
        builder.emit(Opcode::FailRule);

        // LookupRules opcode with operand (head index)
        builder.emit_u16(Opcode::LookupRules, 0); // lookup rules for head at index 0
        builder.emit(Opcode::Pop);

        // ApplySubst opcode (no operands)
        builder.emit(Opcode::PushNil); // expression to substitute
        builder.emit(Opcode::ApplySubst);
        builder.emit(Opcode::Pop);

        // DefineRule opcode with operand (pattern index)
        builder.emit(Opcode::PushNil); // pattern
        builder.emit(Opcode::PushNil); // body
        builder.emit_u16(Opcode::DefineRule, 0); // define rule with pattern at index 0

        builder.emit(Opcode::Return);
        let chunk = builder.build();

        assert!(
            JitCompiler::can_compile_stage1(&chunk),
            "Rule dispatch operations should be compilable"
        );
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_dispatch_rules_returns_result() {
        // Test that DispatchRules JIT compilation produces valid code
        use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("dispatch_rules");
        builder.emit(Opcode::PushNil); // expression (nil = no rules)
        builder.emit(Opcode::DispatchRules);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = 16;
        unsafe { ctx.init_root_binding_frame() };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(code_ptr) };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        // DispatchRules returns the count of matching rules (as Long)
        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_long() || result.is_nil() || result.is_unit() || result.is_error(),
            "DispatchRules should return count (Long), nil, unit, or error");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_try_rule_returns_result() {
        // Test that TryRule JIT compilation produces valid code
        use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("try_rule");
        builder.emit_u16(Opcode::TryRule, 0); // try rule at index 0
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = 16;
        unsafe { ctx.init_root_binding_frame() };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(code_ptr) };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        // TryRule returns the result of applying the rule
        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_unit() || result.is_nil() || result.is_heap() || result.is_error(),
            "TryRule should return unit, nil, heap, or error");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_lookup_rules_returns_result() {
        // Test that LookupRules JIT compilation produces valid code
        use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("lookup_rules");
        builder.emit_u16(Opcode::LookupRules, 0); // lookup rules for head at index 0
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = 16;
        unsafe { ctx.init_root_binding_frame() };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(code_ptr) };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        // LookupRules returns the count of matching rules (as Long)
        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_long() || result.is_nil() || result.is_unit() || result.is_error(),
            "LookupRules should return count (Long), nil, unit, or error");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_apply_subst_returns_result() {
        // Test that ApplySubst JIT compilation produces valid code
        use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("apply_subst");
        builder.emit(Opcode::PushNil); // expression to substitute
        builder.emit(Opcode::ApplySubst);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = 16;
        unsafe { ctx.init_root_binding_frame() };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(code_ptr) };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        // ApplySubst returns the substituted expression
        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_nil() || result.is_unit() || result.is_heap() || result.is_error(),
            "ApplySubst should return substituted expr, nil, unit, or error");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_define_rule_returns_result() {
        // Test that DefineRule JIT compilation produces valid code
        use crate::backend::bytecode::jit::{JitBindingFrame, JitContext, JitValue};

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("define_rule");
        builder.emit(Opcode::PushNil); // pattern
        builder.emit(Opcode::PushNil); // body
        builder.emit_u16(Opcode::DefineRule, 0); // define rule
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("Compilation failed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];
        let mut binding_frames: Vec<JitBindingFrame> = vec![JitBindingFrame::default(); 16];

        let mut ctx = unsafe {
            JitContext::new(stack.as_mut_ptr(), 64, constants.as_ptr(), constants.len())
        };
        ctx.binding_frames = binding_frames.as_mut_ptr();
        ctx.binding_frames_cap = 16;
        unsafe { ctx.init_root_binding_frame() };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 =
            unsafe { std::mem::transmute(code_ptr) };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        // DefineRule returns unit on success
        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_unit() || result.is_nil() || result.is_error(),
            "DefineRule should return unit, nil, or error");
    }

    // =========================================================================
    // Let Binding Scope Cleanup Tests (StackUnderflow fix)
    // =========================================================================

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_let_binding_scope_cleanup() {
        // Test that (let $x 1 0) compiles and executes correctly.
        // This was previously failing with StackUnderflow because:
        // - StoreLocal removes the value from the simulated stack (into locals vec)
        // - The subsequent Swap/Pop scope cleanup expected the value on stack
        // The fix makes Swap/Pop gracefully handle this case.
        use crate::backend::bytecode::jit::{JitContext, JitValue};

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        // Bytecode for: (let $x 1 0)
        // Pattern: Push value, StoreLocal, Push body result, Swap (no-op), Pop (no-op), Return
        let mut builder = ChunkBuilder::new("test_let_scope_cleanup");
        builder.set_local_count(1);
        builder.emit_byte(Opcode::PushLongSmall, 1);   // Push value for binding
        builder.emit_byte(Opcode::StoreLocal, 0);      // Store to local (removes from stack)
        builder.emit_byte(Opcode::PushLongSmall, 0);   // Push body result
        builder.emit(Opcode::Swap);                     // Scope cleanup - should be no-op
        builder.emit(Opcode::Pop);                      // Scope cleanup - should be no-op
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        // Verify it can compile (was failing before fix)
        assert!(JitCompiler::can_compile_stage1(&chunk));

        let code_ptr = compiler.compile(&chunk).expect("JIT compilation should succeed");

        // Execute and verify result
        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                64,
                constants.as_ptr(),
                constants.len(),
            )
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bail out");

        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 0, "Body result should be 0");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_nested_let_bindings() {
        // Test nested let bindings: (let $x 1 (let $y 2 $y))
        // Verifies multiple scope cleanup sequences work correctly
        use crate::backend::bytecode::jit::{JitContext, JitValue};

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("test_nested_let");
        builder.set_local_count(2);

        // Outer let: bind $x = 1
        builder.emit_byte(Opcode::PushLongSmall, 1);
        builder.emit_byte(Opcode::StoreLocal, 0);

        // Inner let: bind $y = 2
        builder.emit_byte(Opcode::PushLongSmall, 2);
        builder.emit_byte(Opcode::StoreLocal, 1);

        // Body: load $y (returns 2)
        builder.emit_byte(Opcode::LoadLocal, 1);

        // Inner scope cleanup (no-op since local is in separate storage)
        builder.emit(Opcode::Swap);
        builder.emit(Opcode::Pop);

        // Outer scope cleanup (no-op)
        builder.emit(Opcode::Swap);
        builder.emit(Opcode::Pop);

        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("JIT compilation should succeed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                64,
                constants.as_ptr(),
                constants.len(),
            )
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "JIT execution should not bail out");

        let result = JitValue::from_raw(result_bits as u64);
        assert!(result.is_long(), "Expected Long result");
        assert_eq!(result.as_long(), 2, "Should return $y = 2");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_pop_empty_stack_is_noop() {
        // Test that Pop on empty stack is a no-op (not an error)
        use crate::backend::bytecode::jit::{JitContext, JitValue};

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("test_pop_empty");
        builder.emit(Opcode::Pop);  // Should be no-op on empty stack
        builder.emit_byte(Opcode::PushLongSmall, 42);
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("JIT compilation should succeed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                64,
                constants.as_ptr(),
                constants.len(),
            )
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "Pop on empty stack should not bail out");

        let result = JitValue::from_raw(result_bits as u64);
        assert_eq!(result.as_long(), 42, "Should return 42");
    }

    #[cfg(feature = "jit")]
    #[test]
    fn test_jit_swap_single_value_is_noop() {
        // Test that Swap with single value on stack is a no-op
        use crate::backend::bytecode::jit::{JitContext, JitValue};

        let mut compiler = JitCompiler::new().expect("Failed to create compiler");

        let mut builder = ChunkBuilder::new("test_swap_single");
        builder.emit_byte(Opcode::PushLongSmall, 99);
        builder.emit(Opcode::Swap);  // Should be no-op with only one value
        builder.emit(Opcode::Return);
        let chunk = builder.build();

        let code_ptr = compiler.compile(&chunk).expect("JIT compilation should succeed");

        let constants = chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); 64];

        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                64,
                constants.as_ptr(),
                constants.len(),
            )
        };

        let native_fn: unsafe extern "C" fn(*mut JitContext) -> i64 = unsafe {
            std::mem::transmute(code_ptr)
        };
        let result_bits = unsafe { native_fn(&mut ctx as *mut JitContext) };

        assert!(!ctx.bailout, "Swap with single value should not bail out");

        let result = JitValue::from_raw(result_bits as u64);
        assert_eq!(result.as_long(), 99, "Should return 99 unchanged");
    }
}
