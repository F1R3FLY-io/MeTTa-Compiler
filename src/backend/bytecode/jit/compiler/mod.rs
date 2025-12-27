//! Bytecode-to-Cranelift JIT Compiler
//!
//! This module translates bytecode chunks into native code using Cranelift.
//! The compilation process:
//!
//! 1. Analyze bytecode for compilability (Stage 1: arithmetic/boolean only)
//! 2. Build Cranelift IR from bytecode opcodes
//! 3. Generate native code via Cranelift JIT module
//! 4. Return function pointer for direct execution

// Submodules
pub mod init;
mod analysis;

use cranelift::prelude::*;
use cranelift::codegen::ir::BlockArg;
use cranelift_frontend::Switch;
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_module::{FuncId, Linkage, Module};

use super::codegen::CodegenContext;
use super::handlers;
use super::types::{JitError, JitResult, TAG_NIL, TAG_ERROR, TAG_ATOM, TAG_VAR, TAG_HEAP, TAG_BOOL};
use crate::backend::bytecode::{BytecodeChunk, Opcode};
use std::collections::HashMap;
use tracing::trace;

// Import initialization traits for zero-cost static dispatch
use init::{
    ArithmeticFuncIds, ArithmeticInit,
    BindingFuncIds, BindingsInit,
    CallFuncIds, CallsInit,
    NondetFuncIds, NondetInit,
    PatternMatchingFuncIds, PatternMatchingInit,
    RulesFuncIds, RulesInit,
    SpaceFuncIds, SpaceInit,
    SpecialFormsFuncIds, SpecialFormsInit,
    TypeOpsFuncIds, TypeOpsInit,
    SExprFuncIds, SExprInit,
    HigherOrderFuncIds, HigherOrderInit,
    GlobalsFuncIds, GlobalsInit,
    DebugFuncIds, DebugInit,
};

/// JIT Compiler for bytecode chunks
///
/// The compiler maintains a Cranelift JIT module for generating native code.
/// Each compiled function takes a `*mut JitContext` and executes the bytecode
/// logic directly on the context's stack.
///
/// # FuncId Organization
///
/// Function IDs are organized into logical groups via compiler_init/ traits:
/// - `arithmetic`: pow, sqrt, log, trig functions (15 ops)
/// - `bindings`: load/store_binding, push/pop_frame (6 ops)
/// - `calls`: call, tail_call, call_n, call_native, etc. (7 ops)
/// - `nondet`: fork, yield, collect, cut, guard, amb (13 ops)
/// - `pattern_matching`: pattern_match, unify, match_arity (6 ops)
/// - `rules`: dispatch_rules, try/commit/fail_rule (8 ops)
/// - `space`: space_add/remove, new/get/change_state (7 ops)
/// - `special_forms`: eval_if, eval_let, eval_match, etc. (19 ops)
/// - `type_ops`: get_type, check_type, assert_type (3 ops)
/// - `sexpr`: get_head/tail, make_sexpr, cons_atom (9 ops)
/// - `higher_order`: map_atom, filter_atom, foldl_atom (5 ops)
/// - `globals`: load_global, store_global, load_space (4 ops)
/// - `debug`: trace, breakpoint, bloom_check, etc. (6 ops)
pub struct JitCompiler {
        module: JITModule,

    /// Counter for generating unique function names
    func_counter: u64,

    // =========================================================================
    // Grouped FuncIds - organized by trait-based initialization
    // =========================================================================

    /// Arithmetic operations (pow, sqrt, log, trig, rounding)
        pub(crate) arithmetic: ArithmeticFuncIds,

    /// Variable binding operations
        pub(crate) bindings: BindingFuncIds,

    /// Function call operations
        pub(crate) calls: CallFuncIds,

    /// Nondeterminism operations (fork, yield, collect, cut)
        pub(crate) nondet: NondetFuncIds,

    /// Pattern matching operations
        pub(crate) pattern_matching: PatternMatchingFuncIds,

    /// Rule dispatch operations
        pub(crate) rules: RulesFuncIds,

    /// Space operations (add, remove, match, state)
        pub(crate) space: SpaceFuncIds,

    /// Special form operations (if, let, match, quote)
        pub(crate) special_forms: SpecialFormsFuncIds,

    /// Type operations (get_type, check_type, assert_type)
        pub(crate) type_ops: TypeOpsFuncIds,

    /// S-expression operations (head, tail, cons, make)
        pub(crate) sexpr: SExprFuncIds,

    /// Higher-order operations (map, filter, fold)
        pub(crate) higher_order: HigherOrderFuncIds,

    /// Global/space access operations
        pub(crate) globals: GlobalsFuncIds,

    /// Debug and meta operations
        pub(crate) debug: DebugFuncIds,

    // =========================================================================
    // Miscellaneous FuncIds - not yet grouped
    // =========================================================================

    /// Load constant from constant pool
        load_const_func_id: FuncId,

    /// Push URI from constant pool
        push_uri_func_id: FuncId,

    /// Index into expression
        index_atom_func_id: FuncId,

    /// Get minimum element
        min_atom_func_id: FuncId,

    /// Get maximum element
        max_atom_func_id: FuncId,

    // MORK Bridge operations (to be grouped later)
        mork_lookup_func_id: FuncId,
        mork_match_func_id: FuncId,
        mork_insert_func_id: FuncId,
        mork_delete_func_id: FuncId,
}

/// Block info for JIT compilation - tracks jump targets and predecessor counts
pub(super) struct BlockInfo {
    /// Bytecode offsets that are jump targets
    targets: Vec<usize>,
    /// Number of predecessors for each target (for PHI detection)
    predecessor_count: HashMap<usize, usize>,
}

impl JitCompiler {
    /// Create a new JIT compiler
    ///
    /// This method uses trait-based initialization for grouped FuncIds,
    /// providing zero-cost abstraction through static dispatch.
        pub fn new() -> JitResult<Self> {
        use super::runtime;

        let mut flag_builder = settings::builder();
        // Enable optimizations
        flag_builder.set("opt_level", "speed").map_err(|e| {
            JitError::CompilationError(format!("Failed to set opt_level: {}", e))
        })?;

        let isa_builder = cranelift_native::builder().map_err(|e| {
            JitError::CompilationError(format!("Failed to create ISA builder: {}", e))
        })?;

        let isa = isa_builder
            .finish(settings::Flags::new(flag_builder))
            .map_err(|e| JitError::CompilationError(format!("Failed to create ISA: {}", e)))?;

        let mut builder = JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());

        // Register all runtime symbols using trait-based initialization
        Self::register_runtime_symbols(&mut builder);

        let mut module = JITModule::new(builder);

        // Declare grouped function imports using trait methods
        let arithmetic = Self::declare_arithmetic_funcs(&mut module)?;
        let bindings = Self::declare_bindings_funcs(&mut module)?;
        let calls = Self::declare_calls_funcs(&mut module)?;
        let nondet = Self::declare_nondet_funcs(&mut module)?;
        let pattern_matching = Self::declare_pattern_matching_funcs(&mut module)?;
        let rules = Self::declare_rules_funcs(&mut module)?;
        let space = Self::declare_space_funcs(&mut module)?;
        let special_forms = Self::declare_special_forms_funcs(&mut module)?;
        let type_ops = Self::declare_type_ops_funcs(&mut module)?;
        let sexpr = Self::declare_sexpr_funcs(&mut module)?;
        let higher_order = Self::declare_higher_order_funcs(&mut module)?;
        let globals = Self::declare_globals_funcs(&mut module)?;
        let debug = Self::declare_debug_funcs(&mut module)?;

        // Declare miscellaneous functions not in groups
        // load_constant: fn(ctx, index) -> value
        let mut load_const_sig = module.make_signature();
        load_const_sig.params.push(AbiParam::new(types::I64));
        load_const_sig.params.push(AbiParam::new(types::I64));
        load_const_sig.returns.push(AbiParam::new(types::I64));
        let load_const_func_id = module
            .declare_function("jit_runtime_load_constant", Linkage::Import, &load_const_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_load_constant: {}", e)))?;

        // push_uri: fn(ctx, uri_idx) -> value
        let mut push_uri_sig = module.make_signature();
        push_uri_sig.params.push(AbiParam::new(types::I64));
        push_uri_sig.params.push(AbiParam::new(types::I64));
        push_uri_sig.returns.push(AbiParam::new(types::I64));
        let push_uri_func_id = module
            .declare_function("jit_runtime_push_uri", Linkage::Import, &push_uri_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_push_uri: {}", e)))?;

        // Expression manipulation: index_atom, min_atom, max_atom
        // index_atom: fn(ctx, expr, index, ip) -> value
        let mut index_sig = module.make_signature();
        index_sig.params.push(AbiParam::new(types::I64));
        index_sig.params.push(AbiParam::new(types::I64));
        index_sig.params.push(AbiParam::new(types::I64));
        index_sig.params.push(AbiParam::new(types::I64));
        index_sig.returns.push(AbiParam::new(types::I64));
        let index_atom_func_id = module
            .declare_function("jit_runtime_index_atom", Linkage::Import, &index_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_index_atom: {}", e)))?;

        // min_atom/max_atom: fn(ctx, expr, ip) -> value
        let mut minmax_sig = module.make_signature();
        minmax_sig.params.push(AbiParam::new(types::I64));
        minmax_sig.params.push(AbiParam::new(types::I64));
        minmax_sig.params.push(AbiParam::new(types::I64));
        minmax_sig.returns.push(AbiParam::new(types::I64));

        let min_atom_func_id = module
            .declare_function("jit_runtime_min_atom", Linkage::Import, &minmax_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_min_atom: {}", e)))?;

        let max_atom_func_id = module
            .declare_function("jit_runtime_max_atom", Linkage::Import, &minmax_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_max_atom: {}", e)))?;

        // MORK bridge functions: fn(ctx, pattern/key, ip) -> result
        let mut mork_sig = module.make_signature();
        mork_sig.params.push(AbiParam::new(types::I64));
        mork_sig.params.push(AbiParam::new(types::I64));
        mork_sig.params.push(AbiParam::new(types::I64));
        mork_sig.returns.push(AbiParam::new(types::I64));

        let mork_lookup_func_id = module
            .declare_function("jit_runtime_mork_lookup", Linkage::Import, &mork_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_mork_lookup: {}", e)))?;

        let mork_match_func_id = module
            .declare_function("jit_runtime_mork_match", Linkage::Import, &mork_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_mork_match: {}", e)))?;

        // MORK insert/delete: fn(ctx, key, value, ip) -> result
        let mut mork_mut_sig = module.make_signature();
        mork_mut_sig.params.push(AbiParam::new(types::I64));
        mork_mut_sig.params.push(AbiParam::new(types::I64));
        mork_mut_sig.params.push(AbiParam::new(types::I64));
        mork_mut_sig.params.push(AbiParam::new(types::I64));
        mork_mut_sig.returns.push(AbiParam::new(types::I64));

        let mork_insert_func_id = module
            .declare_function("jit_runtime_mork_insert", Linkage::Import, &mork_mut_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_mork_insert: {}", e)))?;

        let mork_delete_func_id = module
            .declare_function("jit_runtime_mork_delete", Linkage::Import, &mork_mut_sig)
            .map_err(|e| JitError::CompilationError(format!("Failed to declare jit_runtime_mork_delete: {}", e)))?;

        Ok(JitCompiler {
            module,
            func_counter: 0,
            // Grouped FuncIds
            arithmetic,
            bindings,
            calls,
            nondet,
            pattern_matching,
            rules,
            space,
            special_forms,
            type_ops,
            sexpr,
            higher_order,
            globals,
            debug,
            // Miscellaneous FuncIds
            load_const_func_id,
            push_uri_func_id,
            index_atom_func_id,
            min_atom_func_id,
            max_atom_func_id,
            mork_lookup_func_id,
            mork_match_func_id,
            mork_insert_func_id,
            mork_delete_func_id,
        })
    }

    /// Register runtime helper functions for use from JIT code
    ///
    /// Uses trait-based initialization for grouped symbols with zero-cost
    /// static dispatch. Miscellaneous symbols are registered directly.
        fn register_runtime_symbols(builder: &mut JITBuilder) {
        use super::runtime;

        // Register grouped symbols using trait methods
        Self::register_arithmetic_symbols(builder);
        Self::register_bindings_symbols(builder);
        Self::register_calls_symbols(builder);
        Self::register_nondet_symbols(builder);
        Self::register_pattern_matching_symbols(builder);
        Self::register_rules_symbols(builder);
        Self::register_space_symbols(builder);
        Self::register_special_forms_symbols(builder);
        Self::register_type_ops_symbols(builder);
        Self::register_sexpr_symbols(builder);
        Self::register_higher_order_symbols(builder);
        Self::register_globals_symbols(builder);
        Self::register_debug_symbols(builder);

        // Register error handling symbols (used for bailout)
        builder.symbol("jit_runtime_type_error", runtime::jit_runtime_type_error as *const u8);
        builder.symbol("jit_runtime_div_by_zero", runtime::jit_runtime_div_by_zero as *const u8);
        builder.symbol("jit_runtime_stack_overflow", runtime::jit_runtime_stack_overflow as *const u8);

        // Register miscellaneous symbols
        builder.symbol("jit_runtime_load_constant", runtime::jit_runtime_load_constant as *const u8);
        builder.symbol("jit_runtime_push_uri", runtime::jit_runtime_push_uri as *const u8);

        // Expression manipulation operations
        builder.symbol("jit_runtime_index_atom", runtime::jit_runtime_index_atom as *const u8);
        builder.symbol("jit_runtime_min_atom", runtime::jit_runtime_min_atom as *const u8);
        builder.symbol("jit_runtime_max_atom", runtime::jit_runtime_max_atom as *const u8);

        // MORK bridge operations
        builder.symbol("jit_runtime_mork_lookup", runtime::jit_runtime_mork_lookup as *const u8);
        builder.symbol("jit_runtime_mork_match", runtime::jit_runtime_mork_match as *const u8);
        builder.symbol("jit_runtime_mork_insert", runtime::jit_runtime_mork_insert as *const u8);
        builder.symbol("jit_runtime_mork_delete", runtime::jit_runtime_mork_delete as *const u8);

        // Legacy nondeterminism helpers (used internally by runtime)
        builder.symbol("jit_runtime_save_stack", runtime::jit_runtime_save_stack as *const u8);
        builder.symbol("jit_runtime_restore_stack", runtime::jit_runtime_restore_stack as *const u8);
        builder.symbol("jit_runtime_fail_native", runtime::jit_runtime_fail_native as *const u8);
        builder.symbol("jit_runtime_has_alternatives", runtime::jit_runtime_has_alternatives as *const u8);
        builder.symbol("jit_runtime_get_resume_ip", runtime::jit_runtime_get_resume_ip as *const u8);

        // Legacy fork/yield/collect (non-native versions for backward compatibility)
        builder.symbol("jit_runtime_fork", runtime::jit_runtime_fork as *const u8);
        builder.symbol("jit_runtime_yield", runtime::jit_runtime_yield as *const u8);
        builder.symbol("jit_runtime_collect", runtime::jit_runtime_collect as *const u8);
    }

    /// Check if a bytecode chunk can be JIT compiled (Stage 1-5 + Phase A-I)
    ///
    /// Delegates to `analysis::can_compile_stage1` for the actual implementation.
    /// See that function for detailed documentation of supported features.
    #[inline]
    pub fn can_compile_stage1(chunk: &BytecodeChunk) -> bool {
        analysis::can_compile_stage1(chunk)
    }

    /// Pre-scan bytecode to find all jump targets and their predecessor counts
    ///
    /// Delegates to `analysis::find_block_info` for the actual implementation.
        #[inline]
    fn find_block_info(chunk: &BytecodeChunk) -> BlockInfo {
        analysis::find_block_info(chunk)
    }

    /// Compile a bytecode chunk to native code
    ///
    /// Returns a function pointer that can be called with a JitContext
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

    /// Build the Cranelift IR for a bytecode chunk
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
            Opcode::PushAtom | Opcode::PushString | Opcode::PushVariable | Opcode::PushUri => {
                let mut ctx = handlers::ValueHandlerContext {
                    module: &mut self.module,
                    load_const_func_id: self.load_const_func_id,
                    push_empty_func_id: self.sexpr.push_empty_func_id,
                    push_uri_func_id: self.push_uri_func_id,
                };
                return handlers::compile_runtime_value_op(&mut ctx, codegen, chunk, op, offset);
            }

            // =====================================================================
            // Stage 14: S-Expression Operations (delegated to handlers module)
            // =====================================================================
            Opcode::GetHead | Opcode::GetTail | Opcode::GetArity | Opcode::GetElement => {
                let mut ctx = handlers::SExprHandlerContext {
                    module: &mut self.module,
                    get_head_func_id: self.sexpr.get_head_func_id,
                    get_tail_func_id: self.sexpr.get_tail_func_id,
                    get_arity_func_id: self.sexpr.get_arity_func_id,
                    get_element_func_id: self.sexpr.get_element_func_id,
                    make_sexpr_func_id: self.sexpr.make_sexpr_func_id,
                    cons_atom_func_id: self.sexpr.cons_atom_func_id,
                    make_list_func_id: self.sexpr.make_list_func_id,
                    make_quote_func_id: self.sexpr.make_quote_func_id,
                };
                return handlers::compile_sexpr_access_op(&mut ctx, codegen, chunk, op, offset);
            }

            // =====================================================================
            // Phase 1: Type Operations (delegated to handlers module)
            // =====================================================================
            Opcode::GetType => {
                let mut ctx = handlers::TypeOpsHandlerContext {
                    module: &mut self.module,
                    get_type_func_id: self.type_ops.get_type_func_id,
                    check_type_func_id: self.type_ops.check_type_func_id,
                    assert_type_func_id: self.type_ops.assert_type_func_id,
                };
                return handlers::compile_get_type(&mut ctx, codegen, offset);
            }

            Opcode::CheckType | Opcode::IsType => {
                let mut ctx = handlers::TypeOpsHandlerContext {
                    module: &mut self.module,
                    get_type_func_id: self.type_ops.get_type_func_id,
                    check_type_func_id: self.type_ops.check_type_func_id,
                    assert_type_func_id: self.type_ops.assert_type_func_id,
                };
                return handlers::compile_check_type(&mut ctx, codegen, offset);
            }

            Opcode::AssertType => {
                let mut ctx = handlers::TypeOpsHandlerContext {
                    module: &mut self.module,
                    get_type_func_id: self.type_ops.get_type_func_id,
                    check_type_func_id: self.type_ops.check_type_func_id,
                    assert_type_func_id: self.type_ops.assert_type_func_id,
                };
                return handlers::compile_assert_type(&mut ctx, codegen, offset);
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
                    get_head_func_id: self.sexpr.get_head_func_id,
                    get_tail_func_id: self.sexpr.get_tail_func_id,
                    get_arity_func_id: self.sexpr.get_arity_func_id,
                    get_element_func_id: self.sexpr.get_element_func_id,
                    make_sexpr_func_id: self.sexpr.make_sexpr_func_id,
                    cons_atom_func_id: self.sexpr.cons_atom_func_id,
                    make_list_func_id: self.sexpr.make_list_func_id,
                    make_quote_func_id: self.sexpr.make_quote_func_id,
                };
                return handlers::compile_sexpr_create_op(&mut ctx, codegen, chunk, op, offset);
            }

            // =====================================================================
            // Phase 3: Call/TailCall Operations (delegated to handlers module)
            // =====================================================================

            Opcode::Call => {
                let mut call_ctx = handlers::CallHandlerContext {
                    module: &mut self.module,
                    call_func_id: self.calls.call_func_id,
                    tail_call_func_id: self.calls.tail_call_func_id,
                    call_n_func_id: self.calls.call_n_func_id,
                    tail_call_n_func_id: self.calls.tail_call_n_func_id,
                    call_native_func_id: self.calls.call_native_func_id,
                    call_external_func_id: self.calls.call_external_func_id,
                    call_cached_func_id: self.calls.call_cached_func_id,
                };
                return handlers::compile_call(&mut call_ctx, codegen, chunk, offset);
            }

            Opcode::TailCall => {
                let mut call_ctx = handlers::CallHandlerContext {
                    module: &mut self.module,
                    call_func_id: self.calls.call_func_id,
                    tail_call_func_id: self.calls.tail_call_func_id,
                    call_n_func_id: self.calls.call_n_func_id,
                    tail_call_n_func_id: self.calls.tail_call_n_func_id,
                    call_native_func_id: self.calls.call_native_func_id,
                    call_external_func_id: self.calls.call_external_func_id,
                    call_cached_func_id: self.calls.call_cached_func_id,
                };
                return handlers::compile_tail_call(&mut call_ctx, codegen, chunk, offset);
            }

            Opcode::CallN => {
                let mut call_ctx = handlers::CallHandlerContext {
                    module: &mut self.module,
                    call_func_id: self.calls.call_func_id,
                    tail_call_func_id: self.calls.tail_call_func_id,
                    call_n_func_id: self.calls.call_n_func_id,
                    tail_call_n_func_id: self.calls.tail_call_n_func_id,
                    call_native_func_id: self.calls.call_native_func_id,
                    call_external_func_id: self.calls.call_external_func_id,
                    call_cached_func_id: self.calls.call_cached_func_id,
                };
                return handlers::compile_call_n(&mut call_ctx, codegen, chunk, offset);
            }

            Opcode::TailCallN => {
                let mut call_ctx = handlers::CallHandlerContext {
                    module: &mut self.module,
                    call_func_id: self.calls.call_func_id,
                    tail_call_func_id: self.calls.tail_call_func_id,
                    call_n_func_id: self.calls.call_n_func_id,
                    tail_call_n_func_id: self.calls.tail_call_n_func_id,
                    call_native_func_id: self.calls.call_native_func_id,
                    call_external_func_id: self.calls.call_external_func_id,
                    call_cached_func_id: self.calls.call_cached_func_id,
                };
                return handlers::compile_tail_call_n(&mut call_ctx, codegen, chunk, offset);
            }

            // =====================================================================
            // Phase 4: Fork/Yield/Collect Operations (delegated to handlers module)
            // =====================================================================

            Opcode::Fork => {
                let mut nondet_ctx = handlers::NondetHandlerContext {
                    module: &mut self.module,
                    fork_native_func_id: self.nondet.fork_native_func_id,
                    yield_native_func_id: self.nondet.yield_native_func_id,
                    collect_native_func_id: self.nondet.collect_native_func_id,
                    cut_func_id: self.nondet.cut_func_id,
                    guard_func_id: self.nondet.guard_func_id,
                    amb_func_id: self.nondet.amb_func_id,
                    commit_func_id: self.nondet.commit_func_id,
                    backtrack_func_id: self.nondet.backtrack_func_id,
                    begin_nondet_func_id: self.nondet.begin_nondet_func_id,
                    end_nondet_func_id: self.nondet.end_nondet_func_id,
                };
                return handlers::compile_fork(&mut nondet_ctx, codegen, chunk, offset);
            }

            Opcode::Yield => {
                let mut nondet_ctx = handlers::NondetHandlerContext {
                    module: &mut self.module,
                    fork_native_func_id: self.nondet.fork_native_func_id,
                    yield_native_func_id: self.nondet.yield_native_func_id,
                    collect_native_func_id: self.nondet.collect_native_func_id,
                    cut_func_id: self.nondet.cut_func_id,
                    guard_func_id: self.nondet.guard_func_id,
                    amb_func_id: self.nondet.amb_func_id,
                    commit_func_id: self.nondet.commit_func_id,
                    backtrack_func_id: self.nondet.backtrack_func_id,
                    begin_nondet_func_id: self.nondet.begin_nondet_func_id,
                    end_nondet_func_id: self.nondet.end_nondet_func_id,
                };
                return handlers::compile_yield(&mut nondet_ctx, codegen, offset);
            }

            Opcode::Collect => {
                let mut nondet_ctx = handlers::NondetHandlerContext {
                    module: &mut self.module,
                    fork_native_func_id: self.nondet.fork_native_func_id,
                    yield_native_func_id: self.nondet.yield_native_func_id,
                    collect_native_func_id: self.nondet.collect_native_func_id,
                    cut_func_id: self.nondet.cut_func_id,
                    guard_func_id: self.nondet.guard_func_id,
                    amb_func_id: self.nondet.amb_func_id,
                    commit_func_id: self.nondet.commit_func_id,
                    backtrack_func_id: self.nondet.backtrack_func_id,
                    begin_nondet_func_id: self.nondet.begin_nondet_func_id,
                    end_nondet_func_id: self.nondet.end_nondet_func_id,
                };
                return handlers::compile_collect(&mut nondet_ctx, codegen, chunk, offset);
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
                    pow_func_id: self.arithmetic.pow_func_id,
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
                    sqrt_func_id: self.arithmetic.sqrt_func_id,
                    log_func_id: self.arithmetic.log_func_id,
                    trunc_func_id: self.arithmetic.trunc_func_id,
                    ceil_func_id: self.arithmetic.ceil_func_id,
                    floor_math_func_id: self.arithmetic.floor_math_func_id,
                    round_func_id: self.arithmetic.round_func_id,
                    sin_func_id: self.arithmetic.sin_func_id,
                    cos_func_id: self.arithmetic.cos_func_id,
                    tan_func_id: self.arithmetic.tan_func_id,
                    asin_func_id: self.arithmetic.asin_func_id,
                    acos_func_id: self.arithmetic.acos_func_id,
                    atan_func_id: self.arithmetic.atan_func_id,
                    isnan_func_id: self.arithmetic.isnan_func_id,
                    isinf_func_id: self.arithmetic.isinf_func_id,
                };
                return handlers::compile_extended_math_op(&mut ctx, codegen, op);
            }

            // =====================================================================
            // Expression Manipulation Operations (delegated to handlers module)
            // =====================================================================

            Opcode::IndexAtom | Opcode::MinAtom | Opcode::MaxAtom => {
                let mut ctx = handlers::ExprHandlerContext {
                    module: &mut self.module,
                    index_atom_func_id: self.index_atom_func_id,
                    min_atom_func_id: self.min_atom_func_id,
                    max_atom_func_id: self.max_atom_func_id,
                };
                return handlers::compile_expr_op(&mut ctx, codegen, op, offset);
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
            // Control Flow (delegated to handlers module)
            // =====================================================================
            Opcode::Return => {
                return handlers::compile_return(codegen);
            }

            Opcode::Jump => {
                return handlers::compile_jump(codegen, chunk, op, offset, offset_to_block, merge_blocks);
            }

            Opcode::JumpIfFalse => {
                return handlers::compile_jump_if_false(codegen, chunk, op, offset, offset_to_block, merge_blocks);
            }

            Opcode::JumpIfTrue => {
                return handlers::compile_jump_if_true(codegen, chunk, op, offset, offset_to_block, merge_blocks);
            }

            Opcode::JumpShort => {
                return handlers::compile_jump_short(codegen, chunk, op, offset, offset_to_block, merge_blocks);
            }

            Opcode::JumpIfFalseShort => {
                return handlers::compile_jump_if_false_short(codegen, chunk, op, offset, offset_to_block, merge_blocks);
            }

            Opcode::JumpIfTrueShort => {
                return handlers::compile_jump_if_true_short(codegen, chunk, op, offset, offset_to_block, merge_blocks);
            }

            Opcode::JumpIfNil => {
                return handlers::compile_jump_if_nil(codegen, chunk, op, offset, offset_to_block, merge_blocks);
            }

            Opcode::JumpIfError => {
                return handlers::compile_jump_if_error(codegen, chunk, op, offset, offset_to_block, merge_blocks);
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
            // Phase A: Binding Operations (delegated to handlers module)
            // =====================================================================
            Opcode::LoadBinding => {
                let mut binding_ctx = handlers::BindingHandlerContext {
                    module: &mut self.module,
                    load_binding_func_id: self.bindings.load_binding_func_id,
                    store_binding_func_id: self.bindings.store_binding_func_id,
                    has_binding_func_id: self.bindings.has_binding_func_id,
                    clear_bindings_func_id: self.bindings.clear_bindings_func_id,
                    push_binding_frame_func_id: self.bindings.push_binding_frame_func_id,
                    pop_binding_frame_func_id: self.bindings.pop_binding_frame_func_id,
                };
                return handlers::compile_load_binding(&mut binding_ctx, codegen, chunk, offset);
            }

            Opcode::StoreBinding => {
                let mut binding_ctx = handlers::BindingHandlerContext {
                    module: &mut self.module,
                    load_binding_func_id: self.bindings.load_binding_func_id,
                    store_binding_func_id: self.bindings.store_binding_func_id,
                    has_binding_func_id: self.bindings.has_binding_func_id,
                    clear_bindings_func_id: self.bindings.clear_bindings_func_id,
                    push_binding_frame_func_id: self.bindings.push_binding_frame_func_id,
                    pop_binding_frame_func_id: self.bindings.pop_binding_frame_func_id,
                };
                return handlers::compile_store_binding(&mut binding_ctx, codegen, chunk, offset);
            }

            Opcode::HasBinding => {
                let mut binding_ctx = handlers::BindingHandlerContext {
                    module: &mut self.module,
                    load_binding_func_id: self.bindings.load_binding_func_id,
                    store_binding_func_id: self.bindings.store_binding_func_id,
                    has_binding_func_id: self.bindings.has_binding_func_id,
                    clear_bindings_func_id: self.bindings.clear_bindings_func_id,
                    push_binding_frame_func_id: self.bindings.push_binding_frame_func_id,
                    pop_binding_frame_func_id: self.bindings.pop_binding_frame_func_id,
                };
                return handlers::compile_has_binding(&mut binding_ctx, codegen, chunk, offset);
            }

            Opcode::ClearBindings => {
                let mut binding_ctx = handlers::BindingHandlerContext {
                    module: &mut self.module,
                    load_binding_func_id: self.bindings.load_binding_func_id,
                    store_binding_func_id: self.bindings.store_binding_func_id,
                    has_binding_func_id: self.bindings.has_binding_func_id,
                    clear_bindings_func_id: self.bindings.clear_bindings_func_id,
                    push_binding_frame_func_id: self.bindings.push_binding_frame_func_id,
                    pop_binding_frame_func_id: self.bindings.pop_binding_frame_func_id,
                };
                return handlers::compile_clear_bindings(&mut binding_ctx, codegen);
            }

            Opcode::PushBindingFrame => {
                let mut binding_ctx = handlers::BindingHandlerContext {
                    module: &mut self.module,
                    load_binding_func_id: self.bindings.load_binding_func_id,
                    store_binding_func_id: self.bindings.store_binding_func_id,
                    has_binding_func_id: self.bindings.has_binding_func_id,
                    clear_bindings_func_id: self.bindings.clear_bindings_func_id,
                    push_binding_frame_func_id: self.bindings.push_binding_frame_func_id,
                    pop_binding_frame_func_id: self.bindings.pop_binding_frame_func_id,
                };
                return handlers::compile_push_binding_frame(&mut binding_ctx, codegen);
            }

            Opcode::PopBindingFrame => {
                let mut binding_ctx = handlers::BindingHandlerContext {
                    module: &mut self.module,
                    load_binding_func_id: self.bindings.load_binding_func_id,
                    store_binding_func_id: self.bindings.store_binding_func_id,
                    has_binding_func_id: self.bindings.has_binding_func_id,
                    clear_bindings_func_id: self.bindings.clear_bindings_func_id,
                    push_binding_frame_func_id: self.bindings.push_binding_frame_func_id,
                    pop_binding_frame_func_id: self.bindings.pop_binding_frame_func_id,
                };
                return handlers::compile_pop_binding_frame(&mut binding_ctx, codegen);
            }

            // =====================================================================
            // Phase B: Pattern Matching Operations (delegated to handlers module)
            // =====================================================================
            Opcode::Match => {
                let mut pm_ctx = handlers::PatternMatchingHandlerContext {
                    module: &mut self.module,
                    pattern_match_func_id: self.pattern_matching.pattern_match_func_id,
                    pattern_match_bind_func_id: self.pattern_matching.pattern_match_bind_func_id,
                    match_head_func_id: self.pattern_matching.match_head_func_id,
                    match_arity_func_id: self.pattern_matching.match_arity_func_id,
                    unify_func_id: self.pattern_matching.unify_func_id,
                    unify_bind_func_id: self.pattern_matching.unify_bind_func_id,
                };
                return handlers::compile_match(&mut pm_ctx, codegen, offset);
            }

            Opcode::MatchBind => {
                let mut pm_ctx = handlers::PatternMatchingHandlerContext {
                    module: &mut self.module,
                    pattern_match_func_id: self.pattern_matching.pattern_match_func_id,
                    pattern_match_bind_func_id: self.pattern_matching.pattern_match_bind_func_id,
                    match_head_func_id: self.pattern_matching.match_head_func_id,
                    match_arity_func_id: self.pattern_matching.match_arity_func_id,
                    unify_func_id: self.pattern_matching.unify_func_id,
                    unify_bind_func_id: self.pattern_matching.unify_bind_func_id,
                };
                return handlers::compile_match_bind(&mut pm_ctx, codegen, offset);
            }

            Opcode::MatchHead => {
                let mut pm_ctx = handlers::PatternMatchingHandlerContext {
                    module: &mut self.module,
                    pattern_match_func_id: self.pattern_matching.pattern_match_func_id,
                    pattern_match_bind_func_id: self.pattern_matching.pattern_match_bind_func_id,
                    match_head_func_id: self.pattern_matching.match_head_func_id,
                    match_arity_func_id: self.pattern_matching.match_arity_func_id,
                    unify_func_id: self.pattern_matching.unify_func_id,
                    unify_bind_func_id: self.pattern_matching.unify_bind_func_id,
                };
                return handlers::compile_match_head(&mut pm_ctx, codegen, chunk, offset);
            }

            Opcode::MatchArity => {
                let mut pm_ctx = handlers::PatternMatchingHandlerContext {
                    module: &mut self.module,
                    pattern_match_func_id: self.pattern_matching.pattern_match_func_id,
                    pattern_match_bind_func_id: self.pattern_matching.pattern_match_bind_func_id,
                    match_head_func_id: self.pattern_matching.match_head_func_id,
                    match_arity_func_id: self.pattern_matching.match_arity_func_id,
                    unify_func_id: self.pattern_matching.unify_func_id,
                    unify_bind_func_id: self.pattern_matching.unify_bind_func_id,
                };
                return handlers::compile_match_arity(&mut pm_ctx, codegen, chunk, offset);
            }

            Opcode::MatchGuard => {
                let mut pm_ctx = handlers::PatternMatchingHandlerContext {
                    module: &mut self.module,
                    pattern_match_func_id: self.pattern_matching.pattern_match_func_id,
                    pattern_match_bind_func_id: self.pattern_matching.pattern_match_bind_func_id,
                    match_head_func_id: self.pattern_matching.match_head_func_id,
                    match_arity_func_id: self.pattern_matching.match_arity_func_id,
                    unify_func_id: self.pattern_matching.unify_func_id,
                    unify_bind_func_id: self.pattern_matching.unify_bind_func_id,
                };
                return handlers::compile_match_guard(&mut pm_ctx, codegen, chunk, offset);
            }

            Opcode::Unify => {
                let mut pm_ctx = handlers::PatternMatchingHandlerContext {
                    module: &mut self.module,
                    pattern_match_func_id: self.pattern_matching.pattern_match_func_id,
                    pattern_match_bind_func_id: self.pattern_matching.pattern_match_bind_func_id,
                    match_head_func_id: self.pattern_matching.match_head_func_id,
                    match_arity_func_id: self.pattern_matching.match_arity_func_id,
                    unify_func_id: self.pattern_matching.unify_func_id,
                    unify_bind_func_id: self.pattern_matching.unify_bind_func_id,
                };
                return handlers::compile_unify(&mut pm_ctx, codegen, offset);
            }

            Opcode::UnifyBind => {
                let mut pm_ctx = handlers::PatternMatchingHandlerContext {
                    module: &mut self.module,
                    pattern_match_func_id: self.pattern_matching.pattern_match_func_id,
                    pattern_match_bind_func_id: self.pattern_matching.pattern_match_bind_func_id,
                    match_head_func_id: self.pattern_matching.match_head_func_id,
                    match_arity_func_id: self.pattern_matching.match_arity_func_id,
                    unify_func_id: self.pattern_matching.unify_func_id,
                    unify_bind_func_id: self.pattern_matching.unify_bind_func_id,
                };
                return handlers::compile_unify_bind(&mut pm_ctx, codegen, offset);
            }

            // =================================================================
            // Phase D: Space Operations (delegated to handlers module)
            // =================================================================

            Opcode::SpaceAdd => {
                let mut space_ctx = handlers::SpaceHandlerContext {
                    module: &mut self.module,
                    space_add_func_id: self.space.space_add_func_id,
                    space_remove_func_id: self.space.space_remove_func_id,
                    space_get_atoms_func_id: self.space.space_get_atoms_func_id,
                    space_match_func_id: self.space.space_match_func_id,
                    new_state_func_id: self.space.new_state_func_id,
                    get_state_func_id: self.space.get_state_func_id,
                    change_state_func_id: self.space.change_state_func_id,
                };
                return handlers::compile_space_add(&mut space_ctx, codegen, offset);
            }

            Opcode::SpaceRemove => {
                let mut space_ctx = handlers::SpaceHandlerContext {
                    module: &mut self.module,
                    space_add_func_id: self.space.space_add_func_id,
                    space_remove_func_id: self.space.space_remove_func_id,
                    space_get_atoms_func_id: self.space.space_get_atoms_func_id,
                    space_match_func_id: self.space.space_match_func_id,
                    new_state_func_id: self.space.new_state_func_id,
                    get_state_func_id: self.space.get_state_func_id,
                    change_state_func_id: self.space.change_state_func_id,
                };
                return handlers::compile_space_remove(&mut space_ctx, codegen, offset);
            }

            Opcode::SpaceGetAtoms => {
                let mut space_ctx = handlers::SpaceHandlerContext {
                    module: &mut self.module,
                    space_add_func_id: self.space.space_add_func_id,
                    space_remove_func_id: self.space.space_remove_func_id,
                    space_get_atoms_func_id: self.space.space_get_atoms_func_id,
                    space_match_func_id: self.space.space_match_func_id,
                    new_state_func_id: self.space.new_state_func_id,
                    get_state_func_id: self.space.get_state_func_id,
                    change_state_func_id: self.space.change_state_func_id,
                };
                return handlers::compile_space_get_atoms(&mut space_ctx, codegen, offset);
            }

            Opcode::SpaceMatch => {
                let mut space_ctx = handlers::SpaceHandlerContext {
                    module: &mut self.module,
                    space_add_func_id: self.space.space_add_func_id,
                    space_remove_func_id: self.space.space_remove_func_id,
                    space_get_atoms_func_id: self.space.space_get_atoms_func_id,
                    space_match_func_id: self.space.space_match_func_id,
                    new_state_func_id: self.space.new_state_func_id,
                    get_state_func_id: self.space.get_state_func_id,
                    change_state_func_id: self.space.change_state_func_id,
                };
                return handlers::compile_space_match(&mut space_ctx, codegen, offset);
            }

            // =================================================================
            // Phase D.1: State Operations (delegated to handlers module)
            // =================================================================

            Opcode::NewState => {
                let mut space_ctx = handlers::SpaceHandlerContext {
                    module: &mut self.module,
                    space_add_func_id: self.space.space_add_func_id,
                    space_remove_func_id: self.space.space_remove_func_id,
                    space_get_atoms_func_id: self.space.space_get_atoms_func_id,
                    space_match_func_id: self.space.space_match_func_id,
                    new_state_func_id: self.space.new_state_func_id,
                    get_state_func_id: self.space.get_state_func_id,
                    change_state_func_id: self.space.change_state_func_id,
                };
                return handlers::compile_new_state(&mut space_ctx, codegen, offset);
            }

            Opcode::GetState => {
                let mut space_ctx = handlers::SpaceHandlerContext {
                    module: &mut self.module,
                    space_add_func_id: self.space.space_add_func_id,
                    space_remove_func_id: self.space.space_remove_func_id,
                    space_get_atoms_func_id: self.space.space_get_atoms_func_id,
                    space_match_func_id: self.space.space_match_func_id,
                    new_state_func_id: self.space.new_state_func_id,
                    get_state_func_id: self.space.get_state_func_id,
                    change_state_func_id: self.space.change_state_func_id,
                };
                return handlers::compile_get_state(&mut space_ctx, codegen, offset);
            }

            Opcode::ChangeState => {
                let mut space_ctx = handlers::SpaceHandlerContext {
                    module: &mut self.module,
                    space_add_func_id: self.space.space_add_func_id,
                    space_remove_func_id: self.space.space_remove_func_id,
                    space_get_atoms_func_id: self.space.space_get_atoms_func_id,
                    space_match_func_id: self.space.space_match_func_id,
                    new_state_func_id: self.space.new_state_func_id,
                    get_state_func_id: self.space.get_state_func_id,
                    change_state_func_id: self.space.change_state_func_id,
                };
                return handlers::compile_change_state(&mut space_ctx, codegen, offset);
            }

            // =================================================================
            // Phase C: Rule Dispatch Operations (delegated to handlers module)
            // =================================================================

            Opcode::DispatchRules => {
                let mut rules_ctx = handlers::RulesHandlerContext {
                    module: &mut self.module,
                    dispatch_rules_func_id: self.rules.dispatch_rules_func_id,
                    try_rule_func_id: self.rules.try_rule_func_id,
                    next_rule_func_id: self.rules.next_rule_func_id,
                    commit_rule_func_id: self.rules.commit_rule_func_id,
                    fail_rule_func_id: self.rules.fail_rule_func_id,
                    lookup_rules_func_id: self.rules.lookup_rules_func_id,
                    apply_subst_func_id: self.rules.apply_subst_func_id,
                    define_rule_func_id: self.rules.define_rule_func_id,
                };
                return handlers::compile_dispatch_rules(&mut rules_ctx, codegen, offset);
            }

            Opcode::TryRule => {
                let mut rules_ctx = handlers::RulesHandlerContext {
                    module: &mut self.module,
                    dispatch_rules_func_id: self.rules.dispatch_rules_func_id,
                    try_rule_func_id: self.rules.try_rule_func_id,
                    next_rule_func_id: self.rules.next_rule_func_id,
                    commit_rule_func_id: self.rules.commit_rule_func_id,
                    fail_rule_func_id: self.rules.fail_rule_func_id,
                    lookup_rules_func_id: self.rules.lookup_rules_func_id,
                    apply_subst_func_id: self.rules.apply_subst_func_id,
                    define_rule_func_id: self.rules.define_rule_func_id,
                };
                return handlers::compile_try_rule(&mut rules_ctx, codegen, chunk, offset);
            }

            Opcode::NextRule => {
                let mut rules_ctx = handlers::RulesHandlerContext {
                    module: &mut self.module,
                    dispatch_rules_func_id: self.rules.dispatch_rules_func_id,
                    try_rule_func_id: self.rules.try_rule_func_id,
                    next_rule_func_id: self.rules.next_rule_func_id,
                    commit_rule_func_id: self.rules.commit_rule_func_id,
                    fail_rule_func_id: self.rules.fail_rule_func_id,
                    lookup_rules_func_id: self.rules.lookup_rules_func_id,
                    apply_subst_func_id: self.rules.apply_subst_func_id,
                    define_rule_func_id: self.rules.define_rule_func_id,
                };
                return handlers::compile_next_rule(&mut rules_ctx, codegen, offset);
            }

            Opcode::CommitRule => {
                let mut rules_ctx = handlers::RulesHandlerContext {
                    module: &mut self.module,
                    dispatch_rules_func_id: self.rules.dispatch_rules_func_id,
                    try_rule_func_id: self.rules.try_rule_func_id,
                    next_rule_func_id: self.rules.next_rule_func_id,
                    commit_rule_func_id: self.rules.commit_rule_func_id,
                    fail_rule_func_id: self.rules.fail_rule_func_id,
                    lookup_rules_func_id: self.rules.lookup_rules_func_id,
                    apply_subst_func_id: self.rules.apply_subst_func_id,
                    define_rule_func_id: self.rules.define_rule_func_id,
                };
                return handlers::compile_commit_rule(&mut rules_ctx, codegen, offset);
            }

            Opcode::FailRule => {
                let mut rules_ctx = handlers::RulesHandlerContext {
                    module: &mut self.module,
                    dispatch_rules_func_id: self.rules.dispatch_rules_func_id,
                    try_rule_func_id: self.rules.try_rule_func_id,
                    next_rule_func_id: self.rules.next_rule_func_id,
                    commit_rule_func_id: self.rules.commit_rule_func_id,
                    fail_rule_func_id: self.rules.fail_rule_func_id,
                    lookup_rules_func_id: self.rules.lookup_rules_func_id,
                    apply_subst_func_id: self.rules.apply_subst_func_id,
                    define_rule_func_id: self.rules.define_rule_func_id,
                };
                return handlers::compile_fail_rule(&mut rules_ctx, codegen, offset);
            }

            Opcode::LookupRules => {
                let mut rules_ctx = handlers::RulesHandlerContext {
                    module: &mut self.module,
                    dispatch_rules_func_id: self.rules.dispatch_rules_func_id,
                    try_rule_func_id: self.rules.try_rule_func_id,
                    next_rule_func_id: self.rules.next_rule_func_id,
                    commit_rule_func_id: self.rules.commit_rule_func_id,
                    fail_rule_func_id: self.rules.fail_rule_func_id,
                    lookup_rules_func_id: self.rules.lookup_rules_func_id,
                    apply_subst_func_id: self.rules.apply_subst_func_id,
                    define_rule_func_id: self.rules.define_rule_func_id,
                };
                return handlers::compile_lookup_rules(&mut rules_ctx, codegen, chunk, offset);
            }

            Opcode::ApplySubst => {
                let mut rules_ctx = handlers::RulesHandlerContext {
                    module: &mut self.module,
                    dispatch_rules_func_id: self.rules.dispatch_rules_func_id,
                    try_rule_func_id: self.rules.try_rule_func_id,
                    next_rule_func_id: self.rules.next_rule_func_id,
                    commit_rule_func_id: self.rules.commit_rule_func_id,
                    fail_rule_func_id: self.rules.fail_rule_func_id,
                    lookup_rules_func_id: self.rules.lookup_rules_func_id,
                    apply_subst_func_id: self.rules.apply_subst_func_id,
                    define_rule_func_id: self.rules.define_rule_func_id,
                };
                return handlers::compile_apply_subst(&mut rules_ctx, codegen, offset);
            }

            Opcode::DefineRule => {
                let mut rules_ctx = handlers::RulesHandlerContext {
                    module: &mut self.module,
                    dispatch_rules_func_id: self.rules.dispatch_rules_func_id,
                    try_rule_func_id: self.rules.try_rule_func_id,
                    next_rule_func_id: self.rules.next_rule_func_id,
                    commit_rule_func_id: self.rules.commit_rule_func_id,
                    fail_rule_func_id: self.rules.fail_rule_func_id,
                    lookup_rules_func_id: self.rules.lookup_rules_func_id,
                    apply_subst_func_id: self.rules.apply_subst_func_id,
                    define_rule_func_id: self.rules.define_rule_func_id,
                };
                return handlers::compile_define_rule(&mut rules_ctx, codegen, chunk, offset);
            }

            // =================================================================
            // Phase E: Special Forms (delegated to handlers module)
            // =================================================================

            Opcode::EvalIf => {
                return handlers::compile_eval_if(codegen);
            }

            Opcode::EvalLet => {
                let mut special_ctx = handlers::SpecialFormsHandlerContext {
                    module: &mut self.module,
                    store_binding_func_id: self.bindings.store_binding_func_id,
                    pattern_match_func_id: self.pattern_matching.pattern_match_func_id,
                    eval_case_func_id: self.special_forms.eval_case_func_id,
                    eval_quote_func_id: self.special_forms.eval_quote_func_id,
                    eval_unquote_func_id: self.special_forms.eval_unquote_func_id,
                    eval_eval_func_id: self.special_forms.eval_eval_func_id,
                    eval_new_func_id: self.special_forms.eval_new_func_id,
                    eval_collapse_func_id: self.special_forms.eval_collapse_func_id,
                    eval_superpose_func_id: self.special_forms.eval_superpose_func_id,
                    eval_memo_func_id: self.special_forms.eval_memo_func_id,
                    eval_memo_first_func_id: self.special_forms.eval_memo_first_func_id,
                    eval_pragma_func_id: self.special_forms.eval_pragma_func_id,
                    eval_function_func_id: self.special_forms.eval_function_func_id,
                    eval_lambda_func_id: self.special_forms.eval_lambda_func_id,
                    eval_apply_func_id: self.special_forms.eval_apply_func_id,
                };
                return handlers::compile_eval_let(&mut special_ctx, codegen, chunk, offset);
            }

            Opcode::EvalLetStar => {
                return handlers::compile_eval_let_star(codegen);
            }

            Opcode::EvalMatch => {
                let mut special_ctx = handlers::SpecialFormsHandlerContext {
                    module: &mut self.module,
                    store_binding_func_id: self.bindings.store_binding_func_id,
                    pattern_match_func_id: self.pattern_matching.pattern_match_func_id,
                    eval_case_func_id: self.special_forms.eval_case_func_id,
                    eval_quote_func_id: self.special_forms.eval_quote_func_id,
                    eval_unquote_func_id: self.special_forms.eval_unquote_func_id,
                    eval_eval_func_id: self.special_forms.eval_eval_func_id,
                    eval_new_func_id: self.special_forms.eval_new_func_id,
                    eval_collapse_func_id: self.special_forms.eval_collapse_func_id,
                    eval_superpose_func_id: self.special_forms.eval_superpose_func_id,
                    eval_memo_func_id: self.special_forms.eval_memo_func_id,
                    eval_memo_first_func_id: self.special_forms.eval_memo_first_func_id,
                    eval_pragma_func_id: self.special_forms.eval_pragma_func_id,
                    eval_function_func_id: self.special_forms.eval_function_func_id,
                    eval_lambda_func_id: self.special_forms.eval_lambda_func_id,
                    eval_apply_func_id: self.special_forms.eval_apply_func_id,
                };
                return handlers::compile_eval_match(&mut special_ctx, codegen, offset);
            }

            Opcode::EvalCase => {
                let mut special_ctx = handlers::SpecialFormsHandlerContext {
                    module: &mut self.module,
                    store_binding_func_id: self.bindings.store_binding_func_id,
                    pattern_match_func_id: self.pattern_matching.pattern_match_func_id,
                    eval_case_func_id: self.special_forms.eval_case_func_id,
                    eval_quote_func_id: self.special_forms.eval_quote_func_id,
                    eval_unquote_func_id: self.special_forms.eval_unquote_func_id,
                    eval_eval_func_id: self.special_forms.eval_eval_func_id,
                    eval_new_func_id: self.special_forms.eval_new_func_id,
                    eval_collapse_func_id: self.special_forms.eval_collapse_func_id,
                    eval_superpose_func_id: self.special_forms.eval_superpose_func_id,
                    eval_memo_func_id: self.special_forms.eval_memo_func_id,
                    eval_memo_first_func_id: self.special_forms.eval_memo_first_func_id,
                    eval_pragma_func_id: self.special_forms.eval_pragma_func_id,
                    eval_function_func_id: self.special_forms.eval_function_func_id,
                    eval_lambda_func_id: self.special_forms.eval_lambda_func_id,
                    eval_apply_func_id: self.special_forms.eval_apply_func_id,
                };
                return handlers::compile_eval_case(&mut special_ctx, codegen, chunk, offset);
            }

            Opcode::EvalChain => {
                return handlers::compile_eval_chain(codegen);
            }

            Opcode::EvalQuote => {
                let mut special_ctx = handlers::SpecialFormsHandlerContext {
                    module: &mut self.module,
                    store_binding_func_id: self.bindings.store_binding_func_id,
                    pattern_match_func_id: self.pattern_matching.pattern_match_func_id,
                    eval_case_func_id: self.special_forms.eval_case_func_id,
                    eval_quote_func_id: self.special_forms.eval_quote_func_id,
                    eval_unquote_func_id: self.special_forms.eval_unquote_func_id,
                    eval_eval_func_id: self.special_forms.eval_eval_func_id,
                    eval_new_func_id: self.special_forms.eval_new_func_id,
                    eval_collapse_func_id: self.special_forms.eval_collapse_func_id,
                    eval_superpose_func_id: self.special_forms.eval_superpose_func_id,
                    eval_memo_func_id: self.special_forms.eval_memo_func_id,
                    eval_memo_first_func_id: self.special_forms.eval_memo_first_func_id,
                    eval_pragma_func_id: self.special_forms.eval_pragma_func_id,
                    eval_function_func_id: self.special_forms.eval_function_func_id,
                    eval_lambda_func_id: self.special_forms.eval_lambda_func_id,
                    eval_apply_func_id: self.special_forms.eval_apply_func_id,
                };
                return handlers::compile_eval_quote(&mut special_ctx, codegen, offset);
            }

            Opcode::EvalUnquote => {
                let mut special_ctx = handlers::SpecialFormsHandlerContext {
                    module: &mut self.module,
                    store_binding_func_id: self.bindings.store_binding_func_id,
                    pattern_match_func_id: self.pattern_matching.pattern_match_func_id,
                    eval_case_func_id: self.special_forms.eval_case_func_id,
                    eval_quote_func_id: self.special_forms.eval_quote_func_id,
                    eval_unquote_func_id: self.special_forms.eval_unquote_func_id,
                    eval_eval_func_id: self.special_forms.eval_eval_func_id,
                    eval_new_func_id: self.special_forms.eval_new_func_id,
                    eval_collapse_func_id: self.special_forms.eval_collapse_func_id,
                    eval_superpose_func_id: self.special_forms.eval_superpose_func_id,
                    eval_memo_func_id: self.special_forms.eval_memo_func_id,
                    eval_memo_first_func_id: self.special_forms.eval_memo_first_func_id,
                    eval_pragma_func_id: self.special_forms.eval_pragma_func_id,
                    eval_function_func_id: self.special_forms.eval_function_func_id,
                    eval_lambda_func_id: self.special_forms.eval_lambda_func_id,
                    eval_apply_func_id: self.special_forms.eval_apply_func_id,
                };
                return handlers::compile_eval_unquote(&mut special_ctx, codegen, offset);
            }

            Opcode::EvalEval => {
                let mut special_ctx = handlers::SpecialFormsHandlerContext {
                    module: &mut self.module,
                    store_binding_func_id: self.bindings.store_binding_func_id,
                    pattern_match_func_id: self.pattern_matching.pattern_match_func_id,
                    eval_case_func_id: self.special_forms.eval_case_func_id,
                    eval_quote_func_id: self.special_forms.eval_quote_func_id,
                    eval_unquote_func_id: self.special_forms.eval_unquote_func_id,
                    eval_eval_func_id: self.special_forms.eval_eval_func_id,
                    eval_new_func_id: self.special_forms.eval_new_func_id,
                    eval_collapse_func_id: self.special_forms.eval_collapse_func_id,
                    eval_superpose_func_id: self.special_forms.eval_superpose_func_id,
                    eval_memo_func_id: self.special_forms.eval_memo_func_id,
                    eval_memo_first_func_id: self.special_forms.eval_memo_first_func_id,
                    eval_pragma_func_id: self.special_forms.eval_pragma_func_id,
                    eval_function_func_id: self.special_forms.eval_function_func_id,
                    eval_lambda_func_id: self.special_forms.eval_lambda_func_id,
                    eval_apply_func_id: self.special_forms.eval_apply_func_id,
                };
                return handlers::compile_eval_eval(&mut special_ctx, codegen, offset);
            }

            Opcode::EvalBind => {
                let mut special_ctx = handlers::SpecialFormsHandlerContext {
                    module: &mut self.module,
                    store_binding_func_id: self.bindings.store_binding_func_id,
                    pattern_match_func_id: self.pattern_matching.pattern_match_func_id,
                    eval_case_func_id: self.special_forms.eval_case_func_id,
                    eval_quote_func_id: self.special_forms.eval_quote_func_id,
                    eval_unquote_func_id: self.special_forms.eval_unquote_func_id,
                    eval_eval_func_id: self.special_forms.eval_eval_func_id,
                    eval_new_func_id: self.special_forms.eval_new_func_id,
                    eval_collapse_func_id: self.special_forms.eval_collapse_func_id,
                    eval_superpose_func_id: self.special_forms.eval_superpose_func_id,
                    eval_memo_func_id: self.special_forms.eval_memo_func_id,
                    eval_memo_first_func_id: self.special_forms.eval_memo_first_func_id,
                    eval_pragma_func_id: self.special_forms.eval_pragma_func_id,
                    eval_function_func_id: self.special_forms.eval_function_func_id,
                    eval_lambda_func_id: self.special_forms.eval_lambda_func_id,
                    eval_apply_func_id: self.special_forms.eval_apply_func_id,
                };
                return handlers::compile_eval_bind(&mut special_ctx, codegen, chunk, offset);
            }

            Opcode::EvalNew => {
                let mut special_ctx = handlers::SpecialFormsHandlerContext {
                    module: &mut self.module,
                    store_binding_func_id: self.bindings.store_binding_func_id,
                    pattern_match_func_id: self.pattern_matching.pattern_match_func_id,
                    eval_case_func_id: self.special_forms.eval_case_func_id,
                    eval_quote_func_id: self.special_forms.eval_quote_func_id,
                    eval_unquote_func_id: self.special_forms.eval_unquote_func_id,
                    eval_eval_func_id: self.special_forms.eval_eval_func_id,
                    eval_new_func_id: self.special_forms.eval_new_func_id,
                    eval_collapse_func_id: self.special_forms.eval_collapse_func_id,
                    eval_superpose_func_id: self.special_forms.eval_superpose_func_id,
                    eval_memo_func_id: self.special_forms.eval_memo_func_id,
                    eval_memo_first_func_id: self.special_forms.eval_memo_first_func_id,
                    eval_pragma_func_id: self.special_forms.eval_pragma_func_id,
                    eval_function_func_id: self.special_forms.eval_function_func_id,
                    eval_lambda_func_id: self.special_forms.eval_lambda_func_id,
                    eval_apply_func_id: self.special_forms.eval_apply_func_id,
                };
                return handlers::compile_eval_new(&mut special_ctx, codegen, offset);
            }

            Opcode::EvalCollapse => {
                let mut special_ctx = handlers::SpecialFormsHandlerContext {
                    module: &mut self.module,
                    store_binding_func_id: self.bindings.store_binding_func_id,
                    pattern_match_func_id: self.pattern_matching.pattern_match_func_id,
                    eval_case_func_id: self.special_forms.eval_case_func_id,
                    eval_quote_func_id: self.special_forms.eval_quote_func_id,
                    eval_unquote_func_id: self.special_forms.eval_unquote_func_id,
                    eval_eval_func_id: self.special_forms.eval_eval_func_id,
                    eval_new_func_id: self.special_forms.eval_new_func_id,
                    eval_collapse_func_id: self.special_forms.eval_collapse_func_id,
                    eval_superpose_func_id: self.special_forms.eval_superpose_func_id,
                    eval_memo_func_id: self.special_forms.eval_memo_func_id,
                    eval_memo_first_func_id: self.special_forms.eval_memo_first_func_id,
                    eval_pragma_func_id: self.special_forms.eval_pragma_func_id,
                    eval_function_func_id: self.special_forms.eval_function_func_id,
                    eval_lambda_func_id: self.special_forms.eval_lambda_func_id,
                    eval_apply_func_id: self.special_forms.eval_apply_func_id,
                };
                return handlers::compile_eval_collapse(&mut special_ctx, codegen, offset);
            }

            Opcode::EvalSuperpose => {
                let mut special_ctx = handlers::SpecialFormsHandlerContext {
                    module: &mut self.module,
                    store_binding_func_id: self.bindings.store_binding_func_id,
                    pattern_match_func_id: self.pattern_matching.pattern_match_func_id,
                    eval_case_func_id: self.special_forms.eval_case_func_id,
                    eval_quote_func_id: self.special_forms.eval_quote_func_id,
                    eval_unquote_func_id: self.special_forms.eval_unquote_func_id,
                    eval_eval_func_id: self.special_forms.eval_eval_func_id,
                    eval_new_func_id: self.special_forms.eval_new_func_id,
                    eval_collapse_func_id: self.special_forms.eval_collapse_func_id,
                    eval_superpose_func_id: self.special_forms.eval_superpose_func_id,
                    eval_memo_func_id: self.special_forms.eval_memo_func_id,
                    eval_memo_first_func_id: self.special_forms.eval_memo_first_func_id,
                    eval_pragma_func_id: self.special_forms.eval_pragma_func_id,
                    eval_function_func_id: self.special_forms.eval_function_func_id,
                    eval_lambda_func_id: self.special_forms.eval_lambda_func_id,
                    eval_apply_func_id: self.special_forms.eval_apply_func_id,
                };
                return handlers::compile_eval_superpose(&mut special_ctx, codegen, offset);
            }

            Opcode::EvalMemo => {
                let mut special_ctx = handlers::SpecialFormsHandlerContext {
                    module: &mut self.module,
                    store_binding_func_id: self.bindings.store_binding_func_id,
                    pattern_match_func_id: self.pattern_matching.pattern_match_func_id,
                    eval_case_func_id: self.special_forms.eval_case_func_id,
                    eval_quote_func_id: self.special_forms.eval_quote_func_id,
                    eval_unquote_func_id: self.special_forms.eval_unquote_func_id,
                    eval_eval_func_id: self.special_forms.eval_eval_func_id,
                    eval_new_func_id: self.special_forms.eval_new_func_id,
                    eval_collapse_func_id: self.special_forms.eval_collapse_func_id,
                    eval_superpose_func_id: self.special_forms.eval_superpose_func_id,
                    eval_memo_func_id: self.special_forms.eval_memo_func_id,
                    eval_memo_first_func_id: self.special_forms.eval_memo_first_func_id,
                    eval_pragma_func_id: self.special_forms.eval_pragma_func_id,
                    eval_function_func_id: self.special_forms.eval_function_func_id,
                    eval_lambda_func_id: self.special_forms.eval_lambda_func_id,
                    eval_apply_func_id: self.special_forms.eval_apply_func_id,
                };
                return handlers::compile_eval_memo(&mut special_ctx, codegen, offset);
            }

            Opcode::EvalMemoFirst => {
                let mut special_ctx = handlers::SpecialFormsHandlerContext {
                    module: &mut self.module,
                    store_binding_func_id: self.bindings.store_binding_func_id,
                    pattern_match_func_id: self.pattern_matching.pattern_match_func_id,
                    eval_case_func_id: self.special_forms.eval_case_func_id,
                    eval_quote_func_id: self.special_forms.eval_quote_func_id,
                    eval_unquote_func_id: self.special_forms.eval_unquote_func_id,
                    eval_eval_func_id: self.special_forms.eval_eval_func_id,
                    eval_new_func_id: self.special_forms.eval_new_func_id,
                    eval_collapse_func_id: self.special_forms.eval_collapse_func_id,
                    eval_superpose_func_id: self.special_forms.eval_superpose_func_id,
                    eval_memo_func_id: self.special_forms.eval_memo_func_id,
                    eval_memo_first_func_id: self.special_forms.eval_memo_first_func_id,
                    eval_pragma_func_id: self.special_forms.eval_pragma_func_id,
                    eval_function_func_id: self.special_forms.eval_function_func_id,
                    eval_lambda_func_id: self.special_forms.eval_lambda_func_id,
                    eval_apply_func_id: self.special_forms.eval_apply_func_id,
                };
                return handlers::compile_eval_memo_first(&mut special_ctx, codegen, offset);
            }

            Opcode::EvalPragma => {
                let mut special_ctx = handlers::SpecialFormsHandlerContext {
                    module: &mut self.module,
                    store_binding_func_id: self.bindings.store_binding_func_id,
                    pattern_match_func_id: self.pattern_matching.pattern_match_func_id,
                    eval_case_func_id: self.special_forms.eval_case_func_id,
                    eval_quote_func_id: self.special_forms.eval_quote_func_id,
                    eval_unquote_func_id: self.special_forms.eval_unquote_func_id,
                    eval_eval_func_id: self.special_forms.eval_eval_func_id,
                    eval_new_func_id: self.special_forms.eval_new_func_id,
                    eval_collapse_func_id: self.special_forms.eval_collapse_func_id,
                    eval_superpose_func_id: self.special_forms.eval_superpose_func_id,
                    eval_memo_func_id: self.special_forms.eval_memo_func_id,
                    eval_memo_first_func_id: self.special_forms.eval_memo_first_func_id,
                    eval_pragma_func_id: self.special_forms.eval_pragma_func_id,
                    eval_function_func_id: self.special_forms.eval_function_func_id,
                    eval_lambda_func_id: self.special_forms.eval_lambda_func_id,
                    eval_apply_func_id: self.special_forms.eval_apply_func_id,
                };
                return handlers::compile_eval_pragma(&mut special_ctx, codegen, offset);
            }

            Opcode::EvalFunction => {
                let mut special_ctx = handlers::SpecialFormsHandlerContext {
                    module: &mut self.module,
                    store_binding_func_id: self.bindings.store_binding_func_id,
                    pattern_match_func_id: self.pattern_matching.pattern_match_func_id,
                    eval_case_func_id: self.special_forms.eval_case_func_id,
                    eval_quote_func_id: self.special_forms.eval_quote_func_id,
                    eval_unquote_func_id: self.special_forms.eval_unquote_func_id,
                    eval_eval_func_id: self.special_forms.eval_eval_func_id,
                    eval_new_func_id: self.special_forms.eval_new_func_id,
                    eval_collapse_func_id: self.special_forms.eval_collapse_func_id,
                    eval_superpose_func_id: self.special_forms.eval_superpose_func_id,
                    eval_memo_func_id: self.special_forms.eval_memo_func_id,
                    eval_memo_first_func_id: self.special_forms.eval_memo_first_func_id,
                    eval_pragma_func_id: self.special_forms.eval_pragma_func_id,
                    eval_function_func_id: self.special_forms.eval_function_func_id,
                    eval_lambda_func_id: self.special_forms.eval_lambda_func_id,
                    eval_apply_func_id: self.special_forms.eval_apply_func_id,
                };
                return handlers::compile_eval_function(&mut special_ctx, codegen, chunk, offset);
            }

            Opcode::EvalLambda => {
                let mut special_ctx = handlers::SpecialFormsHandlerContext {
                    module: &mut self.module,
                    store_binding_func_id: self.bindings.store_binding_func_id,
                    pattern_match_func_id: self.pattern_matching.pattern_match_func_id,
                    eval_case_func_id: self.special_forms.eval_case_func_id,
                    eval_quote_func_id: self.special_forms.eval_quote_func_id,
                    eval_unquote_func_id: self.special_forms.eval_unquote_func_id,
                    eval_eval_func_id: self.special_forms.eval_eval_func_id,
                    eval_new_func_id: self.special_forms.eval_new_func_id,
                    eval_collapse_func_id: self.special_forms.eval_collapse_func_id,
                    eval_superpose_func_id: self.special_forms.eval_superpose_func_id,
                    eval_memo_func_id: self.special_forms.eval_memo_func_id,
                    eval_memo_first_func_id: self.special_forms.eval_memo_first_func_id,
                    eval_pragma_func_id: self.special_forms.eval_pragma_func_id,
                    eval_function_func_id: self.special_forms.eval_function_func_id,
                    eval_lambda_func_id: self.special_forms.eval_lambda_func_id,
                    eval_apply_func_id: self.special_forms.eval_apply_func_id,
                };
                return handlers::compile_eval_lambda(&mut special_ctx, codegen, chunk, offset);
            }

            Opcode::EvalApply => {
                let mut special_ctx = handlers::SpecialFormsHandlerContext {
                    module: &mut self.module,
                    store_binding_func_id: self.bindings.store_binding_func_id,
                    pattern_match_func_id: self.pattern_matching.pattern_match_func_id,
                    eval_case_func_id: self.special_forms.eval_case_func_id,
                    eval_quote_func_id: self.special_forms.eval_quote_func_id,
                    eval_unquote_func_id: self.special_forms.eval_unquote_func_id,
                    eval_eval_func_id: self.special_forms.eval_eval_func_id,
                    eval_new_func_id: self.special_forms.eval_new_func_id,
                    eval_collapse_func_id: self.special_forms.eval_collapse_func_id,
                    eval_superpose_func_id: self.special_forms.eval_superpose_func_id,
                    eval_memo_func_id: self.special_forms.eval_memo_func_id,
                    eval_memo_first_func_id: self.special_forms.eval_memo_first_func_id,
                    eval_pragma_func_id: self.special_forms.eval_pragma_func_id,
                    eval_function_func_id: self.special_forms.eval_function_func_id,
                    eval_lambda_func_id: self.special_forms.eval_lambda_func_id,
                    eval_apply_func_id: self.special_forms.eval_apply_func_id,
                };
                return handlers::compile_eval_apply(&mut special_ctx, codegen, chunk, offset);
            }

            // =====================================================================
            // Phase G: Advanced Nondeterminism (delegated to handlers module)
            // =====================================================================

            Opcode::Cut => {
                let mut nondet_ctx = handlers::NondetHandlerContext {
                    module: &mut self.module,
                    fork_native_func_id: self.nondet.fork_native_func_id,
                    yield_native_func_id: self.nondet.yield_native_func_id,
                    collect_native_func_id: self.nondet.collect_native_func_id,
                    cut_func_id: self.nondet.cut_func_id,
                    guard_func_id: self.nondet.guard_func_id,
                    amb_func_id: self.nondet.amb_func_id,
                    commit_func_id: self.nondet.commit_func_id,
                    backtrack_func_id: self.nondet.backtrack_func_id,
                    begin_nondet_func_id: self.nondet.begin_nondet_func_id,
                    end_nondet_func_id: self.nondet.end_nondet_func_id,
                };
                return handlers::compile_cut(&mut nondet_ctx, codegen, offset);
            }

            Opcode::Guard => {
                let mut nondet_ctx = handlers::NondetHandlerContext {
                    module: &mut self.module,
                    fork_native_func_id: self.nondet.fork_native_func_id,
                    yield_native_func_id: self.nondet.yield_native_func_id,
                    collect_native_func_id: self.nondet.collect_native_func_id,
                    cut_func_id: self.nondet.cut_func_id,
                    guard_func_id: self.nondet.guard_func_id,
                    amb_func_id: self.nondet.amb_func_id,
                    commit_func_id: self.nondet.commit_func_id,
                    backtrack_func_id: self.nondet.backtrack_func_id,
                    begin_nondet_func_id: self.nondet.begin_nondet_func_id,
                    end_nondet_func_id: self.nondet.end_nondet_func_id,
                };
                return handlers::compile_guard(&mut nondet_ctx, codegen, offset);
            }

            Opcode::Amb => {
                let mut nondet_ctx = handlers::NondetHandlerContext {
                    module: &mut self.module,
                    fork_native_func_id: self.nondet.fork_native_func_id,
                    yield_native_func_id: self.nondet.yield_native_func_id,
                    collect_native_func_id: self.nondet.collect_native_func_id,
                    cut_func_id: self.nondet.cut_func_id,
                    guard_func_id: self.nondet.guard_func_id,
                    amb_func_id: self.nondet.amb_func_id,
                    commit_func_id: self.nondet.commit_func_id,
                    backtrack_func_id: self.nondet.backtrack_func_id,
                    begin_nondet_func_id: self.nondet.begin_nondet_func_id,
                    end_nondet_func_id: self.nondet.end_nondet_func_id,
                };
                return handlers::compile_amb(&mut nondet_ctx, codegen, chunk, offset);
            }

            Opcode::Commit => {
                let mut nondet_ctx = handlers::NondetHandlerContext {
                    module: &mut self.module,
                    fork_native_func_id: self.nondet.fork_native_func_id,
                    yield_native_func_id: self.nondet.yield_native_func_id,
                    collect_native_func_id: self.nondet.collect_native_func_id,
                    cut_func_id: self.nondet.cut_func_id,
                    guard_func_id: self.nondet.guard_func_id,
                    amb_func_id: self.nondet.amb_func_id,
                    commit_func_id: self.nondet.commit_func_id,
                    backtrack_func_id: self.nondet.backtrack_func_id,
                    begin_nondet_func_id: self.nondet.begin_nondet_func_id,
                    end_nondet_func_id: self.nondet.end_nondet_func_id,
                };
                return handlers::compile_commit(&mut nondet_ctx, codegen, chunk, offset);
            }

            Opcode::Backtrack => {
                let mut nondet_ctx = handlers::NondetHandlerContext {
                    module: &mut self.module,
                    fork_native_func_id: self.nondet.fork_native_func_id,
                    yield_native_func_id: self.nondet.yield_native_func_id,
                    collect_native_func_id: self.nondet.collect_native_func_id,
                    cut_func_id: self.nondet.cut_func_id,
                    guard_func_id: self.nondet.guard_func_id,
                    amb_func_id: self.nondet.amb_func_id,
                    commit_func_id: self.nondet.commit_func_id,
                    backtrack_func_id: self.nondet.backtrack_func_id,
                    begin_nondet_func_id: self.nondet.begin_nondet_func_id,
                    end_nondet_func_id: self.nondet.end_nondet_func_id,
                };
                return handlers::compile_backtrack(&mut nondet_ctx, codegen, offset);
            }

            // =====================================================================
            // Phase F: Advanced Calls (delegated to handlers module)
            // =====================================================================

            Opcode::CallNative => {
                let mut call_ctx = handlers::CallHandlerContext {
                    module: &mut self.module,
                    call_func_id: self.calls.call_func_id,
                    tail_call_func_id: self.calls.tail_call_func_id,
                    call_n_func_id: self.calls.call_n_func_id,
                    tail_call_n_func_id: self.calls.tail_call_n_func_id,
                    call_native_func_id: self.calls.call_native_func_id,
                    call_external_func_id: self.calls.call_external_func_id,
                    call_cached_func_id: self.calls.call_cached_func_id,
                };
                return handlers::compile_call_native(&mut call_ctx, codegen, chunk, offset);
            }

            Opcode::CallExternal => {
                let mut call_ctx = handlers::CallHandlerContext {
                    module: &mut self.module,
                    call_func_id: self.calls.call_func_id,
                    tail_call_func_id: self.calls.tail_call_func_id,
                    call_n_func_id: self.calls.call_n_func_id,
                    tail_call_n_func_id: self.calls.tail_call_n_func_id,
                    call_native_func_id: self.calls.call_native_func_id,
                    call_external_func_id: self.calls.call_external_func_id,
                    call_cached_func_id: self.calls.call_cached_func_id,
                };
                return handlers::compile_call_external(&mut call_ctx, codegen, chunk, offset);
            }

            Opcode::CallCached => {
                let mut call_ctx = handlers::CallHandlerContext {
                    module: &mut self.module,
                    call_func_id: self.calls.call_func_id,
                    tail_call_func_id: self.calls.tail_call_func_id,
                    call_n_func_id: self.calls.call_n_func_id,
                    tail_call_n_func_id: self.calls.tail_call_n_func_id,
                    call_native_func_id: self.calls.call_native_func_id,
                    call_external_func_id: self.calls.call_external_func_id,
                    call_cached_func_id: self.calls.call_cached_func_id,
                };
                return handlers::compile_call_cached(&mut call_ctx, codegen, chunk, offset);
            }

            // =====================================================================
            // Phase H: MORK Bridge (via runtime calls)
            // =====================================================================

            Opcode::MorkLookup => {
                let mut mork_ctx = handlers::MorkHandlerContext {
                    module: &mut self.module,
                    mork_lookup_func_id: self.mork_lookup_func_id,
                    mork_match_func_id: self.mork_match_func_id,
                    mork_insert_func_id: self.mork_insert_func_id,
                    mork_delete_func_id: self.mork_delete_func_id,
                };
                return handlers::compile_mork_lookup(&mut mork_ctx, codegen, offset);
            }

            Opcode::MorkMatch => {
                let mut mork_ctx = handlers::MorkHandlerContext {
                    module: &mut self.module,
                    mork_lookup_func_id: self.mork_lookup_func_id,
                    mork_match_func_id: self.mork_match_func_id,
                    mork_insert_func_id: self.mork_insert_func_id,
                    mork_delete_func_id: self.mork_delete_func_id,
                };
                return handlers::compile_mork_match(&mut mork_ctx, codegen, offset);
            }

            Opcode::MorkInsert => {
                let mut mork_ctx = handlers::MorkHandlerContext {
                    module: &mut self.module,
                    mork_lookup_func_id: self.mork_lookup_func_id,
                    mork_match_func_id: self.mork_match_func_id,
                    mork_insert_func_id: self.mork_insert_func_id,
                    mork_delete_func_id: self.mork_delete_func_id,
                };
                return handlers::compile_mork_insert(&mut mork_ctx, codegen, offset);
            }

            Opcode::MorkDelete => {
                let mut mork_ctx = handlers::MorkHandlerContext {
                    module: &mut self.module,
                    mork_lookup_func_id: self.mork_lookup_func_id,
                    mork_match_func_id: self.mork_match_func_id,
                    mork_insert_func_id: self.mork_insert_func_id,
                    mork_delete_func_id: self.mork_delete_func_id,
                };
                return handlers::compile_mork_delete(&mut mork_ctx, codegen, offset);
            }

            // =====================================================================
            // Phase I: Debug/Meta (delegated to handlers module)
            // =====================================================================

            Opcode::Trace => {
                let mut debug_ctx = handlers::DebugHandlerContext {
                    module: &mut self.module,
                    trace_func_id: self.debug.trace_func_id,
                    breakpoint_func_id: self.debug.breakpoint_func_id,
                };
                return handlers::compile_trace(&mut debug_ctx, codegen, chunk, offset);
            }

            Opcode::Breakpoint => {
                let mut debug_ctx = handlers::DebugHandlerContext {
                    module: &mut self.module,
                    trace_func_id: self.debug.trace_func_id,
                    breakpoint_func_id: self.debug.breakpoint_func_id,
                };
                return handlers::compile_breakpoint(&mut debug_ctx, codegen, chunk, offset);
            }

            // =====================================================================
            // Phase 1.1: Core Nondeterminism Markers (delegated to handlers module)
            // =====================================================================

            Opcode::Fail => {
                return handlers::compile_fail(codegen);
            }

            Opcode::BeginNondet => {
                let mut nondet_ctx = handlers::NondetHandlerContext {
                    module: &mut self.module,
                    fork_native_func_id: self.nondet.fork_native_func_id,
                    yield_native_func_id: self.nondet.yield_native_func_id,
                    collect_native_func_id: self.nondet.collect_native_func_id,
                    cut_func_id: self.nondet.cut_func_id,
                    guard_func_id: self.nondet.guard_func_id,
                    amb_func_id: self.nondet.amb_func_id,
                    commit_func_id: self.nondet.commit_func_id,
                    backtrack_func_id: self.nondet.backtrack_func_id,
                    begin_nondet_func_id: self.nondet.begin_nondet_func_id,
                    end_nondet_func_id: self.nondet.end_nondet_func_id,
                };
                return handlers::compile_begin_nondet(&mut nondet_ctx, codegen, offset);
            }

            Opcode::EndNondet => {
                let mut nondet_ctx = handlers::NondetHandlerContext {
                    module: &mut self.module,
                    fork_native_func_id: self.nondet.fork_native_func_id,
                    yield_native_func_id: self.nondet.yield_native_func_id,
                    collect_native_func_id: self.nondet.collect_native_func_id,
                    cut_func_id: self.nondet.cut_func_id,
                    guard_func_id: self.nondet.guard_func_id,
                    amb_func_id: self.nondet.amb_func_id,
                    commit_func_id: self.nondet.commit_func_id,
                    backtrack_func_id: self.nondet.backtrack_func_id,
                    begin_nondet_func_id: self.nondet.begin_nondet_func_id,
                    end_nondet_func_id: self.nondet.end_nondet_func_id,
                };
                return handlers::compile_end_nondet(&mut nondet_ctx, codegen, offset);
            }

            // =====================================================================
            // Phase 1.3: Multi-value Return (delegated to handlers module)
            // =====================================================================

            Opcode::ReturnMulti => {
                let mut ctx = handlers::MultiReturnHandlerContext {
                    module: &mut self.module,
                    return_multi_func_id: self.debug.return_multi_func_id,
                    collect_n_func_id: self.debug.collect_n_func_id,
                };
                return handlers::compile_return_multi(&mut ctx, codegen, offset);
            }

            Opcode::CollectN => {
                let mut ctx = handlers::MultiReturnHandlerContext {
                    module: &mut self.module,
                    return_multi_func_id: self.debug.return_multi_func_id,
                    collect_n_func_id: self.debug.collect_n_func_id,
                };
                return handlers::compile_collect_n(&mut ctx, codegen, chunk, offset);
            }

            // =====================================================================
            // Phase 1.4: Multi-way Branch (JumpTable) - Native Switch (delegated to handlers)
            // =====================================================================

            Opcode::JumpTable => {
                return handlers::compile_jump_table(codegen, chunk, offset, offset_to_block);
            }

            // =====================================================================
            // Phase 1.5: Global/Space Access (delegated to handlers module)
            // =====================================================================

            Opcode::LoadGlobal => {
                let mut ctx = handlers::GlobalsHandlerContext {
                    module: &mut self.module,
                    load_global_func_id: self.globals.load_global_func_id,
                    store_global_func_id: self.globals.store_global_func_id,
                    load_space_func_id: self.globals.load_space_func_id,
                    load_upvalue_func_id: self.globals.load_upvalue_func_id,
                };
                return handlers::compile_load_global(&mut ctx, codegen, chunk, offset);
            }

            Opcode::StoreGlobal => {
                let mut ctx = handlers::GlobalsHandlerContext {
                    module: &mut self.module,
                    load_global_func_id: self.globals.load_global_func_id,
                    store_global_func_id: self.globals.store_global_func_id,
                    load_space_func_id: self.globals.load_space_func_id,
                    load_upvalue_func_id: self.globals.load_upvalue_func_id,
                };
                return handlers::compile_store_global(&mut ctx, codegen, chunk, offset);
            }

            Opcode::LoadSpace => {
                let mut ctx = handlers::GlobalsHandlerContext {
                    module: &mut self.module,
                    load_global_func_id: self.globals.load_global_func_id,
                    store_global_func_id: self.globals.store_global_func_id,
                    load_space_func_id: self.globals.load_space_func_id,
                    load_upvalue_func_id: self.globals.load_upvalue_func_id,
                };
                return handlers::compile_load_space(&mut ctx, codegen, chunk, offset);
            }

            // =====================================================================
            // Phase 1.6: Closure Support (delegated to handlers module)
            // =====================================================================

            Opcode::LoadUpvalue => {
                let mut ctx = handlers::GlobalsHandlerContext {
                    module: &mut self.module,
                    load_global_func_id: self.globals.load_global_func_id,
                    store_global_func_id: self.globals.store_global_func_id,
                    load_space_func_id: self.globals.load_space_func_id,
                    load_upvalue_func_id: self.globals.load_upvalue_func_id,
                };
                return handlers::compile_load_upvalue(&mut ctx, codegen, chunk, offset);
            }

            // =====================================================================
            // Phase 1.7: Atom Operations (delegated to handlers module)
            // =====================================================================

            Opcode::DeconAtom => {
                let mut ctx = handlers::AtomOpsHandlerContext {
                    module: &mut self.module,
                    decon_atom_func_id: self.higher_order.decon_atom_func_id,
                    repr_func_id: self.higher_order.repr_func_id,
                };
                return handlers::compile_decon_atom(&mut ctx, codegen, offset);
            }

            Opcode::Repr => {
                let mut ctx = handlers::AtomOpsHandlerContext {
                    module: &mut self.module,
                    decon_atom_func_id: self.higher_order.decon_atom_func_id,
                    repr_func_id: self.higher_order.repr_func_id,
                };
                return handlers::compile_repr(&mut ctx, codegen, offset);
            }

            // =====================================================================
            // Phase 1.8: Higher-Order Operations (delegated to handlers module)
            // =====================================================================

            Opcode::MapAtom => {
                let mut ctx = handlers::HigherOrderOpsHandlerContext {
                    module: &mut self.module,
                    map_atom_func_id: self.higher_order.map_atom_func_id,
                    filter_atom_func_id: self.higher_order.filter_atom_func_id,
                    foldl_atom_func_id: self.higher_order.foldl_atom_func_id,
                };
                return handlers::compile_map_atom(&mut ctx, codegen, chunk, offset);
            }

            Opcode::FilterAtom => {
                let mut ctx = handlers::HigherOrderOpsHandlerContext {
                    module: &mut self.module,
                    map_atom_func_id: self.higher_order.map_atom_func_id,
                    filter_atom_func_id: self.higher_order.filter_atom_func_id,
                    foldl_atom_func_id: self.higher_order.foldl_atom_func_id,
                };
                return handlers::compile_filter_atom(&mut ctx, codegen, chunk, offset);
            }

            Opcode::FoldlAtom => {
                let mut ctx = handlers::HigherOrderOpsHandlerContext {
                    module: &mut self.module,
                    map_atom_func_id: self.higher_order.map_atom_func_id,
                    filter_atom_func_id: self.higher_order.filter_atom_func_id,
                    foldl_atom_func_id: self.higher_order.foldl_atom_func_id,
                };
                return handlers::compile_foldl_atom(&mut ctx, codegen, chunk, offset);
            }

            // =====================================================================
            // Phase 1.9: Meta-Type Operations (delegated to handlers module)
            // =====================================================================

            Opcode::GetMetaType => {
                let mut ctx = handlers::MetaOpsHandlerContext {
                    module: &mut self.module,
                    get_metatype_func_id: self.debug.get_metatype_func_id,
                    bloom_check_func_id: self.debug.bloom_check_func_id,
                };
                return handlers::compile_get_metatype(&mut ctx, codegen, offset);
            }

            // =====================================================================
            // Phase 1.10: MORK and Debug (delegated to handlers module)
            // =====================================================================

            Opcode::BloomCheck => {
                let mut ctx = handlers::MetaOpsHandlerContext {
                    module: &mut self.module,
                    get_metatype_func_id: self.debug.get_metatype_func_id,
                    bloom_check_func_id: self.debug.bloom_check_func_id,
                };
                return handlers::compile_bloom_check(&mut ctx, codegen, offset);
            }

            Opcode::Halt => {
                return handlers::compile_halt(codegen);
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
    pub fn code_size(&self) -> usize {
        // Note: Cranelift doesn't expose this directly, would need tracking
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
