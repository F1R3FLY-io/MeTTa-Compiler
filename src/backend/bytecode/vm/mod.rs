//! Bytecode Virtual Machine
//!
//! The VM executes compiled bytecode using a stack-based architecture with
//! support for nondeterminism via choice points and backtracking.
//!
//! This module is organized into submodules by functionality:
//! - `types`: Core type definitions (VmError, VmConfig, CallFrame, etc.)
//! - `pattern`: Pattern matching helpers
//! - `stack`: Stack manipulation operations
//! - `arithmetic`: Arithmetic and math operations
//! - `comparison`: Comparison and boolean operations
//! - `value_ops`: Value creation and variable operations
//! - `control_flow`: Jumps, calls, and returns
//! - `nondeterminism`: Fork, fail, choice points
//! - `expression_ops`: Expression manipulation and higher-order operations
//! - `advanced_calls`: Native, external, and cached calls
//! - `environment_ops`: Rule definition and dispatch
//! - `space_ops`: Space operations
//! - `state_ops`: State cell operations
//! - `debug_ops`: Debugging operations

use std::fmt;
use std::marker::Unpin;
use std::ops::ControlFlow;
use std::sync::Arc;
use tracing::{debug, trace, warn};

use super::chunk::BytecodeChunk;
use super::external_registry::ExternalRegistry;
use super::memo_cache::{CacheStats, MemoCache};
use super::mork_bridge::MorkBridge;
use super::native_registry::NativeRegistry;
use super::opcodes::Opcode;
use crate::backend::models::{MettaValue, MettaValueInner};
use crate::backend::HeapEnvironment;

// === Submodules ===

mod advanced_calls;
mod arithmetic;
mod comparison;
mod control_flow;
mod debug_ops;
mod environment_ops;
mod expression_ops;
mod nondeterminism;
mod pattern;
mod space_ops;
mod stack;
mod state_ops;
mod types;
mod value_ops;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod proptests;

// === Re-exports ===

pub use pattern::{pattern_match_bind, pattern_matches, unify};
pub use types::{Alternative, BindingFrame, CallFrame, ChoicePoint, VmConfig, VmError, VmResult};
// Generic types for zero-conversion support
pub use types::{
    GenericAlternative, GenericBindingFrame, GenericCallFrame, GenericChoicePoint,
    HeapAlternative, HeapBindingFrame, HeapCallFrame, HeapChoicePoint,
};

// Export generic VM
// Note: GenericBytecodeVM and HeapGenericBytecodeVM are defined at bottom of this file

// === BytecodeVM Struct ===

/// The Bytecode Virtual Machine
#[derive(Debug)]
pub struct BytecodeVM {
    /// Value stack for operands and results
    pub(super) value_stack: Vec<MettaValue>,

    /// Call stack for function frames
    pub(super) call_stack: Vec<CallFrame>,

    /// Bindings stack for pattern variables
    pub(super) bindings_stack: Vec<BindingFrame>,

    /// Choice points for nondeterminism
    pub(super) choice_points: Vec<ChoicePoint>,

    /// Collected results (for nondeterministic evaluation)
    pub(super) results: Vec<MettaValue>,

    /// Current instruction pointer
    pub(super) ip: usize,

    /// Current bytecode chunk
    pub(super) chunk: Arc<BytecodeChunk>,

    /// VM configuration
    pub(super) config: VmConfig,

    /// Optional bridge to MORK for rule dispatch
    pub(super) bridge: Option<Arc<MorkBridge>>,

    /// Native function registry for CallNative opcode
    pub(super) native_registry: Arc<NativeRegistry>,

    /// Memoization cache for CallCached opcode
    pub(super) memo_cache: Arc<MemoCache>,

    /// External function registry for CallExternal opcode
    pub(super) external_registry: Arc<ExternalRegistry>,

    /// Optional environment for rule definitions and lookups
    /// When present, enables DefineRule and RuntimeCall opcodes
    pub(super) env: Option<HeapEnvironment>,
}

impl BytecodeVM {
    // === Constructors ===

    /// Create a new VM with the given chunk
    pub fn new(chunk: Arc<BytecodeChunk>) -> Self {
        Self::with_config(chunk, VmConfig::default())
    }

    /// Create a new VM with custom configuration
    pub fn with_config(chunk: Arc<BytecodeChunk>, config: VmConfig) -> Self {
        Self {
            value_stack: Vec::with_capacity(256),
            call_stack: Vec::with_capacity(64),
            bindings_stack: vec![BindingFrame::new(0)],
            choice_points: Vec::new(),
            results: Vec::new(),
            ip: 0,
            chunk,
            config,
            bridge: None,
            native_registry: Arc::new(NativeRegistry::with_stdlib()),
            memo_cache: Arc::new(MemoCache::default()),
            external_registry: Arc::new(ExternalRegistry::default()),
            env: None,
        }
    }

    /// Create a new VM with a bridge for rule dispatch
    pub fn with_bridge(chunk: Arc<BytecodeChunk>, bridge: Arc<MorkBridge>) -> Self {
        let mut vm = Self::new(chunk);
        vm.bridge = Some(bridge);
        vm
    }

    /// Create a new VM with custom configuration and bridge
    pub fn with_config_and_bridge(
        chunk: Arc<BytecodeChunk>,
        config: VmConfig,
        bridge: Arc<MorkBridge>,
    ) -> Self {
        Self {
            value_stack: Vec::with_capacity(256),
            call_stack: Vec::with_capacity(64),
            bindings_stack: vec![BindingFrame::new(0)],
            choice_points: Vec::new(),
            results: Vec::new(),
            ip: 0,
            chunk,
            config,
            bridge: Some(bridge),
            native_registry: Arc::new(NativeRegistry::with_stdlib()),
            memo_cache: Arc::new(MemoCache::default()),
            external_registry: Arc::new(ExternalRegistry::default()),
            env: None,
        }
    }

    /// Create a new VM with an environment for rule definitions and lookups.
    ///
    /// This enables the DefineRule and RuntimeCall opcodes to interact with
    /// the MeTTa environment for rule-based evaluation.
    pub fn with_env(chunk: Arc<BytecodeChunk>, env: HeapEnvironment) -> Self {
        Self {
            value_stack: Vec::with_capacity(256),
            call_stack: Vec::with_capacity(64),
            bindings_stack: vec![BindingFrame::new(0)],
            choice_points: Vec::new(),
            results: Vec::new(),
            ip: 0,
            chunk,
            config: VmConfig::default(),
            bridge: None,
            native_registry: Arc::new(NativeRegistry::with_stdlib()),
            memo_cache: Arc::new(MemoCache::default()),
            external_registry: Arc::new(ExternalRegistry::default()),
            env: Some(env),
        }
    }

    /// Create a new VM with custom configuration and environment.
    pub fn with_config_and_env(
        chunk: Arc<BytecodeChunk>,
        config: VmConfig,
        env: HeapEnvironment,
    ) -> Self {
        Self {
            value_stack: Vec::with_capacity(256),
            call_stack: Vec::with_capacity(64),
            bindings_stack: vec![BindingFrame::new(0)],
            choice_points: Vec::new(),
            results: Vec::new(),
            ip: 0,
            chunk,
            config,
            bridge: None,
            native_registry: Arc::new(NativeRegistry::with_stdlib()),
            memo_cache: Arc::new(MemoCache::default()),
            external_registry: Arc::new(ExternalRegistry::default()),
            env: Some(env),
        }
    }

    /// Set the external function registry
    ///
    /// This allows registering external functions before VM execution.
    pub fn with_external_registry(mut self, registry: Arc<ExternalRegistry>) -> Self {
        self.external_registry = registry;
        self
    }

    /// Set the environment for rule operations.
    ///
    /// This is a builder-style method for setting environment after construction.
    pub fn with_environment(mut self, env: HeapEnvironment) -> Self {
        self.env = Some(env);
        self
    }

    // === Environment Accessors ===

    /// Get a reference to the environment, if present.
    pub fn environment(&self) -> Option<&HeapEnvironment> {
        self.env.as_ref()
    }

    /// Take ownership of the environment, returning it.
    ///
    /// This is used to return the modified environment after execution.
    pub fn take_environment(&mut self) -> Option<HeapEnvironment> {
        self.env.take()
    }

    // === Initial Value Setup ===

    /// Push an initial value onto the stack before execution.
    ///
    /// This is used for template execution where a binding value
    /// needs to be available as local slot 0.
    #[inline]
    pub fn push_initial_value(&mut self, value: MettaValue) {
        self.value_stack.push(value);
    }

    // === Execution Methods ===

    /// Resume VM execution after JIT bailout for non-determinism.
    ///
    /// This allows JIT to compile deterministic parts of bytecode and then
    /// bail out to VM for Fork/Choice opcodes that require backtracking.
    ///
    /// # Arguments
    /// * `bailout_ip` - The instruction pointer where JIT bailed out
    /// * `value_stack` - The value stack state at bailout time
    ///
    /// # Returns
    /// The results of completing execution from the bailout point
    pub fn resume_from_bailout(
        &mut self,
        bailout_ip: usize,
        value_stack: Vec<MettaValue>,
    ) -> VmResult<Vec<MettaValue>> {
        self.ip = bailout_ip;
        self.value_stack = value_stack;
        self.run_without_jit()
    }

    /// Run the VM to completion without attempting JIT execution.
    /// Used for resuming after JIT bailout.
    fn run_without_jit(&mut self) -> VmResult<Vec<MettaValue>> {
        loop {
            match self.step()? {
                ControlFlow::Continue(()) => continue,
                ControlFlow::Break(results) => return Ok(results),
            }
        }
    }

    /// Run the VM to completion, returning all results
    pub fn run(&mut self) -> VmResult<Vec<MettaValue>> {
        // Pre-allocate local variable slots on the stack.
        // The VM stores locals ON the stack at positions [base, base+local_count).
        // StoreLocal pops a value and stores it at stack[base+index], so
        // slots must exist before the first StoreLocal executes.
        let local_count = self.chunk.local_count() as usize;
        if local_count > 0 && self.value_stack.len() < local_count {
            self.value_stack.resize(local_count, MettaValue::Nil());
        }

        // JIT execution path

        if let Some(result) = self.try_jit_execute()? {
            return Ok(result);
        }

        loop {
            match self.step()? {
                ControlFlow::Continue(()) => continue,
                ControlFlow::Break(results) => return Ok(results),
            }
        }
    }

    /// Run the VM to completion, returning results and the modified environment.
    ///
    /// This is the primary entry point for environment-aware bytecode execution.
    /// It returns both the evaluation results and the (possibly modified) environment,
    /// enabling rule definitions to persist across evaluations.
    ///
    /// # Returns
    /// A tuple of (results, environment) where environment is the modified state
    /// after execution (e.g., with newly defined rules).
    pub fn run_with_env(&mut self) -> VmResult<(Vec<MettaValue>, Option<HeapEnvironment>)> {
        let results = self.run()?;
        let env = self.env.take();
        Ok((results, env))
    }

    /// Try to execute the chunk using JIT-compiled code
    ///
    /// Returns:
    /// - `Ok(Some(results))` if JIT execution completed successfully
    /// - `Ok(None)` if JIT is not available or bailed out (fall back to interpreter)
    /// - `Err(_)` if an error occurred

    fn try_jit_execute(&mut self) -> VmResult<Option<Vec<MettaValue>>> {
        use super::jit::{JitBailoutReason, JitCompiler, JitContext, JitValue};

        // Record execution for profiling
        let should_compile = self.chunk.record_jit_execution();

        // Try to compile if hot
        if should_compile && self.chunk.can_jit_compile() {
            if self.chunk.jit_profile().try_start_compiling() {
                // We won the race to compile
                match JitCompiler::new() {
                    Ok(mut compiler) => {
                        match compiler.compile(&self.chunk) {
                            Ok(code_ptr) => {
                                unsafe {
                                    // Code size is not tracked separately for now
                                    self.chunk.jit_profile().set_compiled(code_ptr, 0);
                                }
                            }
                            Err(_e) => {
                                // Compilation failed - mark as failed so we don't try again
                                self.chunk.jit_profile().set_failed();
                            }
                        }
                    }
                    Err(_e) => {
                        // Could not create compiler - mark as failed
                        self.chunk.jit_profile().set_failed();
                    }
                }
            }
        }

        // Execute JIT code if available
        if !self.chunk.has_jit_code() {
            return Ok(None);
        }

        // Set up JIT context with appropriately sized stack
        // Use the bytecode length as a conservative upper bound for stack depth
        // (each push adds at most 1, and typical ops consume before producing)
        let required_stack = self.chunk.code().len().max(64).min(4096);
        let constants = self.chunk.constants();
        let mut stack: Vec<JitValue> = vec![JitValue::nil(); required_stack];

        // SAFETY: stack is valid for the lifetime of this function call
        let mut ctx = unsafe {
            JitContext::new(
                stack.as_mut_ptr(),
                required_stack,
                constants.as_ptr(),
                constants.len(),
            )
        };

        // Get and execute native code
        if let Some(native_fn) = unsafe { self.chunk.jit_profile().get_native_fn() } {
            let jit_result = unsafe { native_fn(&mut ctx as *mut JitContext) };

            // Check for bailout
            if ctx.bailout {
                // JIT execution bailed out - set interpreter IP to bailout point
                self.ip = ctx.bailout_ip;
                // Transfer any values from JIT stack to interpreter stack
                for i in 0..ctx.sp {
                    let jit_val = unsafe { *ctx.value_stack.add(i) };
                    let metta_val = unsafe { jit_val.to_metta() };
                    self.push(metta_val);
                }
                return Ok(None); // Fall back to interpreter
            }

            // JIT execution completed - collect results
            // Priority: collected results > return value > stack
            let mut results = Vec::new();

            // Check for collected results (from nondeterminism)
            if ctx.results_count > 0 {
                results.reserve(ctx.results_count);
                for i in 0..ctx.results_count {
                    let jit_val = unsafe { *ctx.results.add(i) };
                    let metta_val = unsafe { jit_val.to_metta() };
                    results.push(metta_val);
                }
            } else if jit_result != 0 {
                // Use the function return value (NaN-boxed JitValue)
                let jit_val = JitValue::from_raw(jit_result as u64);
                let metta_val = unsafe { jit_val.to_metta() };
                results.push(metta_val);
            } else {
                // Fallback to stack
                results.reserve(ctx.sp);
                for i in 0..ctx.sp {
                    let jit_val = unsafe { *ctx.value_stack.add(i) };
                    let metta_val = unsafe { jit_val.to_metta() };
                    results.push(metta_val);
                }
            }

            if results.is_empty() {
                results.push(MettaValue::Unit());
            }

            return Ok(Some(results));
        }

        Ok(None)
    }

    /// Execute a single instruction
    pub fn step(&mut self) -> VmResult<ControlFlow<Vec<MettaValue>>> {
        // Bounds check
        if self.ip >= self.chunk.len() {
            // End of chunk - return results or value on stack
            return self.handle_chunk_end();
        }

        // Read opcode
        let opcode_byte = self
            .chunk
            .read_byte(self.ip)
            .ok_or(VmError::IpOutOfBounds)?;
        let opcode = Opcode::from_byte(opcode_byte).ok_or(VmError::InvalidOpcode(opcode_byte))?;

        // Trace if enabled
        if self.config.trace {
            let (disasm, _) = self.chunk.disassemble_instruction(self.ip);
            trace!(target: "mettatron::vm::step", ip = self.ip, mnemonic = %disasm, stack_depth = self.value_stack.len());
        }

        // Advance IP past opcode
        self.ip += 1;

        // Execute opcode
        match opcode {
            // Stack operations
            Opcode::Nop => {}
            Opcode::Pop => {
                self.pop()?;
            }
            Opcode::Dup => self.op_dup()?,
            Opcode::Swap => self.op_swap()?,
            Opcode::Rot3 => self.op_rot3()?,
            Opcode::Over => self.op_over()?,
            Opcode::DupN => self.op_dup_n()?,
            Opcode::PopN => self.op_pop_n()?,

            // Value creation
            Opcode::PushNil => self.push(MettaValue::Nil()),
            Opcode::PushTrue => self.push(MettaValue::Bool(true)),
            Opcode::PushFalse => self.push(MettaValue::Bool(false)),
            Opcode::PushUnit => self.push(MettaValue::Unit()),
            Opcode::PushEmpty => self.push(MettaValue::sexpr(vec![])),
            Opcode::PushLongSmall => self.op_push_long_small()?,
            Opcode::PushLong => self.op_push_constant()?,
            Opcode::PushAtom => self.op_push_constant()?,
            Opcode::PushString => self.op_push_constant()?,
            Opcode::PushUri => self.op_push_constant()?,
            Opcode::PushConstant => self.op_push_constant()?,
            Opcode::PushVariable => self.op_push_variable()?,
            Opcode::MakeSExpr => self.op_make_sexpr()?,
            Opcode::MakeSExprLarge => self.op_make_sexpr_large()?,
            Opcode::MakeList => self.op_make_list()?,
            Opcode::MakeQuote => self.op_make_quote()?,

            // Variable operations
            Opcode::LoadLocal => self.op_load_local()?,
            Opcode::StoreLocal => self.op_store_local()?,
            Opcode::LoadLocalWide => self.op_load_local_wide()?,
            Opcode::StoreLocalWide => self.op_store_local_wide()?,
            Opcode::LoadBinding => self.op_load_binding()?,
            Opcode::StoreBinding => self.op_store_binding()?,
            Opcode::HasBinding => self.op_has_binding()?,
            Opcode::ClearBindings => self.op_clear_bindings(),
            Opcode::PushBindingFrame => self.op_push_binding_frame(),
            Opcode::PopBindingFrame => self.op_pop_binding_frame()?,
            Opcode::LoadUpvalue => self.op_load_upvalue()?,

            // Control flow
            Opcode::Jump => self.op_jump()?,
            Opcode::JumpIfFalse => self.op_jump_if_false()?,
            Opcode::JumpIfTrue => self.op_jump_if_true()?,
            Opcode::JumpIfNil => self.op_jump_if_nil()?,
            Opcode::JumpIfError => self.op_jump_if_error()?,
            Opcode::JumpShort => self.op_jump_short()?,
            Opcode::JumpIfFalseShort => self.op_jump_if_false_short()?,
            Opcode::JumpIfTrueShort => self.op_jump_if_true_short()?,
            Opcode::JumpTable => self.op_jump_table()?,
            Opcode::Call => self.op_call()?,
            Opcode::TailCall => self.op_tail_call()?,
            Opcode::CallN => self.op_call_n()?,
            Opcode::TailCallN => self.op_tail_call_n()?,
            Opcode::Return => return self.op_return(),
            Opcode::ReturnMulti => return self.op_return_multi(),

            // Arithmetic
            Opcode::Add => self.op_add()?,
            Opcode::Sub => self.op_sub()?,
            Opcode::Mul => self.op_mul()?,
            Opcode::Div => self.op_div()?,
            Opcode::Mod => self.op_mod()?,
            Opcode::Neg => self.op_neg()?,
            Opcode::Abs => self.op_abs()?,
            Opcode::FloorDiv => self.op_floor_div()?,
            Opcode::Pow => self.op_pow()?,
            Opcode::Sqrt => self.op_sqrt()?,
            Opcode::Log => self.op_log()?,
            Opcode::Trunc => self.op_trunc()?,
            Opcode::Ceil => self.op_ceil()?,
            Opcode::FloorMath => self.op_floor_math()?,
            Opcode::Round => self.op_round()?,
            Opcode::Sin => self.op_sin()?,
            Opcode::Cos => self.op_cos()?,
            Opcode::Tan => self.op_tan()?,
            Opcode::Asin => self.op_asin()?,
            Opcode::Acos => self.op_acos()?,
            Opcode::Atan => self.op_atan()?,
            Opcode::IsNan => self.op_isnan()?,
            Opcode::IsInf => self.op_isinf()?,

            // Comparison
            Opcode::Lt => self.op_lt()?,
            Opcode::Le => self.op_le()?,
            Opcode::Gt => self.op_gt()?,
            Opcode::Ge => self.op_ge()?,
            Opcode::Eq => self.op_eq()?,
            Opcode::Ne => self.op_ne()?,
            Opcode::StructEq => self.op_struct_eq()?,

            // Boolean
            Opcode::And => self.op_and()?,
            Opcode::Or => self.op_or()?,
            Opcode::Not => self.op_not()?,
            Opcode::Xor => self.op_xor()?,

            // Type operations
            Opcode::GetType => self.op_get_type()?,
            Opcode::CheckType => self.op_check_type()?,
            Opcode::IsType => self.op_is_type()?,
            Opcode::AssertType => self.op_assert_type()?,

            // Pattern matching
            Opcode::Match => self.op_match()?,
            Opcode::MatchBind => self.op_match_bind()?,
            Opcode::MatchHead => self.op_match_head()?,
            Opcode::MatchArity => self.op_match_arity()?,
            Opcode::MatchGuard => self.op_match_guard()?,
            Opcode::Unify => self.op_unify()?,
            Opcode::UnifyBind => self.op_unify_bind()?,
            Opcode::IsVariable => self.op_is_variable()?,
            Opcode::IsSExpr => self.op_is_sexpr()?,
            Opcode::IsSymbol => self.op_is_symbol()?,
            Opcode::GetHead => self.op_get_head()?,
            Opcode::GetTail => self.op_get_tail()?,
            Opcode::GetArity => self.op_get_arity()?,
            Opcode::GetElement => self.op_get_element()?,
            Opcode::DeconAtom => self.op_decon_atom()?,
            Opcode::Repr => self.op_repr()?,
            Opcode::GetMetaType => self.op_get_metatype()?,
            Opcode::ConsAtom => self.op_cons_atom()?,
            Opcode::MapAtom => self.op_map_atom()?,
            Opcode::FilterAtom => self.op_filter_atom()?,
            Opcode::FoldlAtom => self.op_foldl_atom()?,
            Opcode::IndexAtom => self.op_index_atom()?,
            Opcode::MinAtom => self.op_min_atom()?,
            Opcode::MaxAtom => self.op_max_atom()?,

            // Nondeterminism
            Opcode::Fork => return self.op_fork(),
            Opcode::Fail => return self.op_fail(),
            Opcode::Cut => self.op_cut(),
            Opcode::Collect => self.op_collect()?,
            Opcode::CollectN => self.op_collect_n()?,
            Opcode::Yield => return self.op_yield(),
            Opcode::BeginNondet => self.op_begin_nondet(),
            Opcode::EndNondet => self.op_end_nondet()?,
            Opcode::Amb => self.op_amb()?,
            Opcode::Guard => return self.op_guard(),
            Opcode::Commit => self.op_commit(),
            Opcode::Backtrack => return self.op_fail(), // Backtrack is alias for Fail

            // Advanced calls
            Opcode::CallNative => self.op_call_native()?,
            Opcode::CallExternal => self.op_call_external()?,
            Opcode::CallCached => self.op_call_cached()?,

            // Environment operations (require Environment to be set)
            Opcode::DefineRule => self.op_define_rule()?,
            Opcode::LoadGlobal => self.op_load_global()?,
            Opcode::StoreGlobal => self.op_store_global()?,
            Opcode::DispatchRules => self.op_dispatch_rules()?,

            // Space operations
            Opcode::SpaceAdd => self.op_space_add()?,
            Opcode::SpaceRemove => self.op_space_remove()?,
            Opcode::SpaceGetAtoms => self.op_space_get_atoms()?,
            Opcode::SpaceMatch => self.op_space_match()?,
            Opcode::LoadSpace => self.op_load_space()?,

            // State operations
            Opcode::NewState => self.op_new_state()?,
            Opcode::GetState => self.op_get_state()?,
            Opcode::ChangeState => self.op_change_state()?,

            // Debug
            Opcode::Breakpoint => self.op_breakpoint()?,
            Opcode::Trace => self.op_trace()?,
            Opcode::Halt => return Err(VmError::Halted),

            // Not yet implemented
            _ => {
                return Err(VmError::Runtime(format!(
                    "Opcode {} not yet implemented",
                    opcode.mnemonic()
                )));
            }
        }

        Ok(ControlFlow::Continue(()))
    }

    /// Handle reaching the end of a bytecode chunk
    fn handle_chunk_end(&mut self) -> VmResult<ControlFlow<Vec<MettaValue>>> {
        if let Some(frame) = self.call_stack.pop() {
            // Return to caller
            let value = self.pop().unwrap_or(MettaValue::Nil());
            self.ip = frame.return_ip;
            self.chunk = frame.return_chunk;
            self.value_stack.truncate(frame.base_ptr);

            // Pop binding frame pushed by execute_rule_body
            if self.bindings_stack.len() > frame.bindings_base {
                self.bindings_stack.truncate(frame.bindings_base + 1);
                self.bindings_stack.pop();
            }

            self.push(value);
            Ok(ControlFlow::Continue(()))
        } else {
            // End of top-level
            if !self.value_stack.is_empty() {
                self.results.extend(self.value_stack.drain(..));
            }
            Ok(ControlFlow::Break(std::mem::take(&mut self.results)))
        }
    }

    // === Bytecode Reading Helpers ===

    #[inline]
    pub(super) fn read_u8(&mut self) -> VmResult<u8> {
        let byte = self
            .chunk
            .read_byte(self.ip)
            .ok_or(VmError::IpOutOfBounds)?;
        self.ip += 1;
        Ok(byte)
    }

    #[inline]
    pub(super) fn read_i8(&mut self) -> VmResult<i8> {
        Ok(self.read_u8()? as i8)
    }

    #[inline]
    pub(super) fn read_u16(&mut self) -> VmResult<u16> {
        let value = self.chunk.read_u16(self.ip).ok_or(VmError::IpOutOfBounds)?;
        self.ip += 2;
        Ok(value)
    }

    #[inline]
    pub(super) fn read_i16(&mut self) -> VmResult<i16> {
        Ok(self.read_u16()? as i16)
    }

    // === Test Helper Methods ===

    /// Push a value onto the results vector (for testing)
    #[cfg(test)]
    pub fn push_result(&mut self, value: MettaValue) {
        self.results.push(value);
    }

    /// Get the number of choice points (for testing)
    #[cfg(test)]
    pub fn choice_points_len(&self) -> usize {
        self.choice_points.len()
    }

    /// Get the number of entries in the memo cache (for testing)
    #[cfg(test)]
    pub fn memo_cache_len(&self) -> usize {
        self.memo_cache.len()
    }

    /// Get memo cache statistics (for testing)
    #[cfg(test)]
    pub fn memo_cache_stats(&self) -> CacheStats {
        self.memo_cache.stats()
    }
}

// ============================================================================
// Generic Bytecode VM - Zero-Conversion Support
// ============================================================================

use crate::backend::environment::GenericEnvironment;
use crate::backend::models::{GenericBindings, MettaValueFactory, MettaValueTrait};
use super::chunk::GenericBytecodeChunk;

/// Generic bytecode virtual machine that works with any value type.
///
/// This is the generic version of `BytecodeVM` that enables zero-conversion
/// evaluation with both heap-allocated (`MettaValue`) and arena-allocated
/// (`ArenaValue<'static>`) values.
///
/// # Type Parameters
///
/// - `V`: The value type (must implement `MettaValueTrait`)
/// - `F`: The factory type for constructing values
///
/// # Design
///
/// The generic VM uses:
/// - `GenericBytecodeChunk<V>` for constant storage
/// - `GenericBindingFrame<V>` for pattern variable bindings
/// - `GenericChoicePoint<V, GenericBytecodeChunk<V>>` for nondeterminism
/// - `GenericEnvironment<V, F>` for rule storage
/// - `MettaValueFactory<V>` trait methods for value construction
///
/// This ensures NO conversions are needed between value types during execution.
pub struct GenericBytecodeVM<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + PartialEq + 'static,
    F: MettaValueFactory<V> + Clone + Send + Sync + 'static,
{
    /// Value stack for operands and results
    pub(crate) value_stack: Vec<V>,

    /// Call stack for function frames
    pub(crate) call_stack: Vec<GenericCallFrame<GenericBytecodeChunk<V>>>,

    /// Bindings stack for pattern variables
    pub(crate) bindings_stack: Vec<GenericBindingFrame<V>>,

    /// Choice points for nondeterminism
    pub(crate) choice_points: Vec<GenericChoicePoint<V, GenericBytecodeChunk<V>>>,

    /// Collected results (for nondeterministic evaluation)
    pub(crate) results: Vec<V>,

    /// Current instruction pointer
    pub(crate) ip: usize,

    /// Current bytecode chunk
    pub(crate) chunk: Arc<GenericBytecodeChunk<V>>,

    /// VM configuration
    pub(crate) config: VmConfig,

    /// Factory for constructing values
    pub(crate) factory: F,

    /// Optional environment for rule definitions and lookups
    pub(crate) env: Option<GenericEnvironment<V, F>>,

    /// Native function registry for CallNative opcode
    pub(crate) native_registry: Arc<super::native_registry::GenericNativeRegistry<V, F>>,

    /// External function registry for CallExternal opcode
    pub(crate) external_registry: Arc<super::external_registry::GenericExternalRegistry<V, F>>,

    /// Memoization cache for CallCached opcode
    pub(crate) memo_cache: Arc<super::generic_memo_cache::GenericMemoCache<V>>,
}

impl<V, F> fmt::Debug for GenericBytecodeVM<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + PartialEq + fmt::Debug + 'static,
    F: MettaValueFactory<V> + Clone + Send + Sync + fmt::Debug + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GenericBytecodeVM")
            .field("value_stack_len", &self.value_stack.len())
            .field("call_stack_len", &self.call_stack.len())
            .field("bindings_stack_len", &self.bindings_stack.len())
            .field("choice_points_len", &self.choice_points.len())
            .field("results_len", &self.results.len())
            .field("ip", &self.ip)
            .field("config", &self.config)
            .field("has_env", &self.env.is_some())
            .finish()
    }
}

impl<V, F> GenericBytecodeVM<V, F>
where
    V: MettaValueTrait + Clone + Send + Sync + Unpin + PartialEq + 'static,
    F: MettaValueFactory<V> + Clone + Send + Sync + 'static,
{
    /// Create a new generic VM with the given chunk and factory.
    pub fn new(chunk: Arc<GenericBytecodeChunk<V>>, factory: F) -> Self {
        Self::with_config(chunk, VmConfig::default(), factory)
    }

    /// Create a new generic VM with custom configuration.
    pub fn with_config(chunk: Arc<GenericBytecodeChunk<V>>, config: VmConfig, factory: F) -> Self {
        Self {
            value_stack: Vec::with_capacity(256),
            call_stack: Vec::with_capacity(64),
            bindings_stack: vec![GenericBindingFrame::new(0)],
            choice_points: Vec::new(),
            results: Vec::new(),
            ip: 0,
            chunk,
            config,
            factory,
            env: None,
            native_registry: Arc::new(super::native_registry::GenericNativeRegistry::new()),
            external_registry: Arc::new(super::external_registry::GenericExternalRegistry::new()),
            memo_cache: Arc::new(super::generic_memo_cache::GenericMemoCache::default()),
        }
    }

    /// Create a new generic VM with an environment.
    pub fn with_env(
        chunk: Arc<GenericBytecodeChunk<V>>,
        env: GenericEnvironment<V, F>,
        factory: F,
    ) -> Self {
        Self {
            value_stack: Vec::with_capacity(256),
            call_stack: Vec::with_capacity(64),
            bindings_stack: vec![GenericBindingFrame::new(0)],
            choice_points: Vec::new(),
            results: Vec::new(),
            ip: 0,
            chunk,
            config: VmConfig::default(),
            factory,
            env: Some(env),
            native_registry: Arc::new(super::native_registry::GenericNativeRegistry::new()),
            external_registry: Arc::new(super::external_registry::GenericExternalRegistry::new()),
            memo_cache: Arc::new(super::generic_memo_cache::GenericMemoCache::default()),
        }
    }

    /// Create a new generic VM with full configuration including registries.
    pub fn with_registries(
        chunk: Arc<GenericBytecodeChunk<V>>,
        env: GenericEnvironment<V, F>,
        factory: F,
        native_registry: Arc<super::native_registry::GenericNativeRegistry<V, F>>,
        external_registry: Arc<super::external_registry::GenericExternalRegistry<V, F>>,
        memo_cache: Arc<super::generic_memo_cache::GenericMemoCache<V>>,
    ) -> Self {
        Self {
            value_stack: Vec::with_capacity(256),
            call_stack: Vec::with_capacity(64),
            bindings_stack: vec![GenericBindingFrame::new(0)],
            choice_points: Vec::new(),
            results: Vec::new(),
            ip: 0,
            chunk,
            config: VmConfig::default(),
            factory,
            env: Some(env),
            native_registry,
            external_registry,
            memo_cache,
        }
    }

    /// Get the factory reference.
    #[inline]
    pub fn factory(&self) -> &F {
        &self.factory
    }

    /// Get a reference to the environment, if present.
    #[inline]
    pub fn environment(&self) -> Option<&GenericEnvironment<V, F>> {
        self.env.as_ref()
    }

    /// Take ownership of the environment.
    #[inline]
    pub fn take_environment(&mut self) -> Option<GenericEnvironment<V, F>> {
        self.env.take()
    }

    // === Stack Operations ===

    /// Push a value onto the value stack.
    #[inline]
    pub fn push(&mut self, value: V) {
        self.value_stack.push(value);
    }

    /// Pop a value from the value stack.
    #[inline]
    pub fn pop(&mut self) -> VmResult<V> {
        self.value_stack.pop().ok_or(VmError::StackUnderflow)
    }

    /// Peek at the top value on the stack.
    #[inline]
    pub fn peek(&self) -> VmResult<&V> {
        self.value_stack.last().ok_or(VmError::StackUnderflow)
    }

    /// Peek at a value n positions from the top.
    #[inline]
    pub fn peek_n(&self, n: usize) -> VmResult<&V> {
        let len = self.value_stack.len();
        if n >= len {
            return Err(VmError::StackUnderflow);
        }
        Ok(&self.value_stack[len - 1 - n])
    }

    /// Duplicate the top value.
    #[inline]
    pub fn dup(&mut self) -> VmResult<()> {
        let value = self.peek()?.clone();
        self.push(value);
        Ok(())
    }

    /// Swap the top two values.
    #[inline]
    pub fn swap(&mut self) -> VmResult<()> {
        let len = self.value_stack.len();
        if len < 2 {
            return Err(VmError::StackUnderflow);
        }
        self.value_stack.swap(len - 1, len - 2);
        Ok(())
    }

    // === Bytecode Reading Helpers ===

    /// Read a single byte and advance IP.
    #[inline]
    pub fn read_u8(&mut self) -> VmResult<u8> {
        let byte = self
            .chunk
            .read_byte(self.ip)
            .ok_or(VmError::IpOutOfBounds)?;
        self.ip += 1;
        Ok(byte)
    }

    /// Read a signed byte and advance IP.
    #[inline]
    pub fn read_i8(&mut self) -> VmResult<i8> {
        Ok(self.read_u8()? as i8)
    }

    /// Read a u16 and advance IP.
    #[inline]
    pub fn read_u16(&mut self) -> VmResult<u16> {
        let value = self.chunk.read_u16(self.ip).ok_or(VmError::IpOutOfBounds)?;
        self.ip += 2;
        Ok(value)
    }

    /// Read a signed i16 and advance IP.
    #[inline]
    pub fn read_i16(&mut self) -> VmResult<i16> {
        Ok(self.read_u16()? as i16)
    }

    // === Value Construction Helpers ===

    /// Create a nil value using the factory.
    #[inline]
    pub fn make_nil(&self) -> V {
        self.factory.nil()
    }

    /// Create a unit value using the factory.
    #[inline]
    pub fn make_unit(&self) -> V {
        self.factory.unit()
    }

    /// Create a boolean value using the factory.
    #[inline]
    pub fn make_bool(&self, b: bool) -> V {
        self.factory.bool(b)
    }

    /// Create a long value using the factory.
    #[inline]
    pub fn make_long(&self, n: i64) -> V {
        self.factory.long(n)
    }

    /// Create a float value using the factory.
    #[inline]
    pub fn make_float(&self, f: f64) -> V {
        self.factory.float(f)
    }

    /// Create an atom value using the factory.
    #[inline]
    pub fn make_atom(&self, name: &str) -> V {
        self.factory.atom(name)
    }

    /// Create a string value using the factory.
    #[inline]
    pub fn make_string(&self, s: &str) -> V {
        self.factory.string(s)
    }

    /// Create an s-expression using the factory.
    #[inline]
    pub fn make_sexpr(&self, items: Vec<V>) -> V {
        self.factory.sexpr(items)
    }

    /// Create an error value using the factory.
    #[inline]
    pub fn make_error(&self, msg: &str, details: V) -> V {
        self.factory.error(msg, details)
    }

    // === Binding Operations ===

    /// Get a binding by name from the bindings stack.
    pub fn get_binding(&self, name: &str) -> Option<&V> {
        for frame in self.bindings_stack.iter().rev() {
            if let Some(value) = frame.get(name) {
                return Some(value);
            }
        }
        None
    }

    /// Set a binding in the current frame.
    pub fn set_binding(&mut self, name: String, value: V) {
        if let Some(frame) = self.bindings_stack.last_mut() {
            frame.set(name, value);
        }
    }

    /// Push a new binding frame.
    pub fn push_binding_frame(&mut self) {
        let depth = self.bindings_stack.len() as u32;
        self.bindings_stack.push(GenericBindingFrame::new(depth));
    }

    /// Pop the current binding frame.
    pub fn pop_binding_frame(&mut self) {
        if self.bindings_stack.len() > 1 {
            self.bindings_stack.pop();
        }
    }

    // === Full Execution ===

    /// Run the VM to completion, returning all results.
    ///
    /// This is the complete generic implementation that handles all opcodes
    /// using trait methods for value construction and inspection. NO conversions
    /// between value types occur during execution.
    pub fn run(&mut self) -> VmResult<Vec<V>> {
        // Pre-allocate local variable slots
        let local_count = self.chunk.local_count() as usize;
        if local_count > 0 && self.value_stack.len() < local_count {
            let nil = self.make_nil();
            self.value_stack.resize(local_count, nil);
        }

        loop {
            match self.step()? {
                ControlFlow::Continue(()) => continue,
                ControlFlow::Break(results) => return Ok(results),
            }
        }
    }

    /// Run the VM with environment, returning results and modified environment.
    pub fn run_with_env(&mut self) -> VmResult<(Vec<V>, Option<GenericEnvironment<V, F>>)> {
        let results = self.run()?;
        let env = self.env.take();
        Ok((results, env))
    }

    /// Execute a single instruction.
    pub fn step(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        // Bounds check
        if self.ip >= self.chunk.len() {
            return self.handle_chunk_end();
        }

        // Read opcode
        let opcode_byte = self.read_u8()?;
        let opcode = Opcode::from_byte(opcode_byte)
            .ok_or(VmError::InvalidOpcode(opcode_byte))?;

        // Execute opcode
        match opcode {
            // === Stack Operations ===
            Opcode::Nop => {}
            Opcode::Pop => { self.pop()?; }
            Opcode::Dup => self.dup()?,
            Opcode::Swap => self.swap()?,
            Opcode::Rot3 => self.op_rot3()?,
            Opcode::Over => self.op_over()?,
            Opcode::DupN => self.op_dup_n()?,
            Opcode::PopN => self.op_pop_n()?,

            // === Value Creation ===
            Opcode::PushNil => self.push(self.make_nil()),
            Opcode::PushUnit => self.push(self.make_unit()),
            Opcode::PushTrue => self.push(self.make_bool(true)),
            Opcode::PushFalse => self.push(self.make_bool(false)),
            Opcode::PushEmpty => {
                let empty = self.make_sexpr(vec![]);
                self.push(empty);
            }
            Opcode::PushLongSmall => {
                let n = self.read_i8()? as i64;
                self.push(self.make_long(n));
            }
            Opcode::PushLong | Opcode::PushAtom | Opcode::PushString |
            Opcode::PushUri | Opcode::PushConstant => {
                let index = self.read_u16()?;
                let value = self.chunk.get_constant(index)
                    .ok_or(VmError::InvalidConstant(index))?
                    .clone();
                self.push(value);
            }
            Opcode::PushVariable => self.op_push_variable()?,
            Opcode::MakeSExpr => {
                let arity = self.read_u8()? as usize;
                let len = self.value_stack.len();
                if arity > len {
                    return Err(VmError::StackUnderflow);
                }
                let items: Vec<V> = self.value_stack.drain((len - arity)..).collect();
                self.push(self.make_sexpr(items));
            }
            Opcode::MakeSExprLarge => {
                let arity = self.read_u16()? as usize;
                let len = self.value_stack.len();
                if arity > len {
                    return Err(VmError::StackUnderflow);
                }
                let items: Vec<V> = self.value_stack.drain((len - arity)..).collect();
                self.push(self.make_sexpr(items));
            }
            Opcode::MakeList => self.op_make_list()?,
            Opcode::MakeQuote => self.op_make_quote()?,

            // === Variable Operations ===
            Opcode::LoadLocal => {
                let index = self.read_u8()? as usize;
                let value = self.value_stack.get(index)
                    .ok_or(VmError::InvalidLocal(index as u16))?
                    .clone();
                self.push(value);
            }
            Opcode::StoreLocal => {
                let index = self.read_u8()? as usize;
                let value = self.pop()?;
                if index >= self.value_stack.len() {
                    self.value_stack.resize(index + 1, self.make_nil());
                }
                self.value_stack[index] = value;
            }
            Opcode::LoadLocalWide => {
                let index = self.read_u16()? as usize;
                let value = self.value_stack.get(index)
                    .ok_or(VmError::InvalidLocal(index as u16))?
                    .clone();
                self.push(value);
            }
            Opcode::StoreLocalWide => {
                let index = self.read_u16()? as usize;
                let value = self.pop()?;
                if index >= self.value_stack.len() {
                    self.value_stack.resize(index + 1, self.make_nil());
                }
                self.value_stack[index] = value;
            }
            Opcode::LoadBinding => {
                let index = self.read_u16()?;
                let name = self.chunk.get_constant(index)
                    .and_then(|v| v.as_atom().map(|s| s.to_string()))
                    .ok_or(VmError::InvalidConstant(index))?;
                let value = self.get_binding(&name)
                    .cloned()
                    .unwrap_or_else(|| self.make_nil());
                self.push(value);
            }
            Opcode::StoreBinding => {
                let index = self.read_u16()?;
                let name = self.chunk.get_constant(index)
                    .and_then(|v| v.as_atom().map(|s| s.to_string()))
                    .ok_or(VmError::InvalidConstant(index))?;
                let value = self.pop()?;
                self.set_binding(name, value);
            }
            Opcode::HasBinding => {
                let index = self.read_u16()?;
                let name = self.chunk.get_constant(index)
                    .and_then(|v| v.as_atom().map(|s| s.to_string()))
                    .ok_or(VmError::InvalidConstant(index))?;
                let has = self.get_binding(&name).is_some();
                self.push(self.make_bool(has));
            }
            Opcode::ClearBindings => {
                if let Some(frame) = self.bindings_stack.last_mut() {
                    frame.clear();
                }
            }
            Opcode::PushBindingFrame => self.push_binding_frame(),
            Opcode::PopBindingFrame => {
                self.pop_binding_frame();
            }
            Opcode::LoadUpvalue => {
                // Upvalues are stored as constants in parent scopes
                let index = self.read_u16()?;
                let value = self.chunk.get_constant(index)
                    .ok_or(VmError::InvalidConstant(index))?
                    .clone();
                self.push(value);
            }

            // === Control Flow ===
            Opcode::Jump => {
                let offset = self.read_i16()?;
                self.ip = (self.ip as isize + offset as isize) as usize;
            }
            Opcode::JumpIfFalse => {
                let offset = self.read_i16()?;
                let cond = self.pop()?;
                if cond.as_bool() == Some(false) || cond.is_nil() {
                    self.ip = (self.ip as isize + offset as isize) as usize;
                }
            }
            Opcode::JumpIfTrue => {
                let offset = self.read_i16()?;
                let cond = self.pop()?;
                if cond.as_bool() == Some(true) {
                    self.ip = (self.ip as isize + offset as isize) as usize;
                }
            }
            Opcode::JumpIfNil => {
                let offset = self.read_i16()?;
                let value = self.pop()?;
                if value.is_nil() {
                    self.ip = (self.ip as isize + offset as isize) as usize;
                }
            }
            Opcode::JumpIfError => {
                let offset = self.read_i16()?;
                let value = self.peek()?.clone();
                if value.is_error() {
                    self.pop()?;
                    self.ip = (self.ip as isize + offset as isize) as usize;
                }
            }
            Opcode::JumpShort => {
                let offset = self.read_i8()?;
                self.ip = (self.ip as isize + offset as isize) as usize;
            }
            Opcode::JumpIfFalseShort => {
                let offset = self.read_i8()?;
                let cond = self.pop()?;
                if cond.as_bool() == Some(false) || cond.is_nil() {
                    self.ip = (self.ip as isize + offset as isize) as usize;
                }
            }
            Opcode::JumpIfTrueShort => {
                let offset = self.read_i8()?;
                let cond = self.pop()?;
                if cond.as_bool() == Some(true) {
                    self.ip = (self.ip as isize + offset as isize) as usize;
                }
            }
            Opcode::JumpTable => self.op_jump_table()?,
            Opcode::Call => self.op_call()?,
            Opcode::TailCall => self.op_tail_call()?,
            Opcode::CallN => self.op_call_n()?,
            Opcode::TailCallN => self.op_tail_call_n()?,
            Opcode::Return => return self.op_return(),
            Opcode::ReturnMulti => return self.op_return_multi(),

            // === Arithmetic ===
            Opcode::Add => self.op_binary_num(|a, b| a + b, |a, b| a + b)?,
            Opcode::Sub => self.op_binary_num(|a, b| a - b, |a, b| a - b)?,
            Opcode::Mul => self.op_binary_num(|a, b| a * b, |a, b| a * b)?,
            Opcode::Div => {
                let b = self.pop()?;
                let a = self.pop()?;
                match (a.as_long(), b.as_long()) {
                    (Some(x), Some(0)) => return Err(VmError::DivisionByZero),
                    (Some(x), Some(y)) => self.push(self.make_long(x / y)),
                    _ => match (a.as_float(), b.as_float()) {
                        (Some(x), Some(y)) => self.push(self.make_float(x / y)),
                        _ => return Err(VmError::TypeError { expected: "number", got: "other" }),
                    }
                }
            }
            Opcode::Mod => {
                let b = self.pop()?;
                let a = self.pop()?;
                match (a.as_long(), b.as_long()) {
                    (Some(x), Some(0)) => return Err(VmError::DivisionByZero),
                    (Some(x), Some(y)) => {
                        // Check for overflow case (MIN % -1)
                        if x == i64::MIN && y == -1 {
                            return Err(VmError::ArithmeticOverflow);
                        }
                        self.push(self.make_long(x % y));
                    }
                    _ => match (a.as_float(), b.as_float()) {
                        (Some(x), Some(y)) => self.push(self.make_float(x % y)),
                        _ => return Err(VmError::TypeError { expected: "number", got: "other" }),
                    }
                }
            }
            Opcode::Neg => {
                let a = self.pop()?;
                if let Some(x) = a.as_long() {
                    self.push(self.make_long(-x));
                } else if let Some(x) = a.as_float() {
                    self.push(self.make_float(-x));
                } else {
                    return Err(VmError::TypeError { expected: "number", got: "other" });
                }
            }
            Opcode::Abs => {
                let a = self.pop()?;
                if let Some(x) = a.as_long() {
                    self.push(self.make_long(x.abs()));
                } else if let Some(x) = a.as_float() {
                    self.push(self.make_float(x.abs()));
                } else {
                    return Err(VmError::TypeError { expected: "number", got: "other" });
                }
            }
            Opcode::FloorDiv => {
                let b = self.pop()?;
                let a = self.pop()?;
                match (a.as_float(), b.as_float()) {
                    (Some(x), Some(y)) if y != 0.0 => {
                        self.push(self.make_float((x / y).floor()));
                    }
                    (Some(_), Some(_)) => return Err(VmError::DivisionByZero),
                    _ => return Err(VmError::TypeError { expected: "number", got: "other" }),
                }
            }
            Opcode::Pow => {
                let b = self.pop()?;
                let a = self.pop()?;
                match (a.as_float(), b.as_float()) {
                    (Some(x), Some(y)) => self.push(self.make_float(x.powf(y))),
                    _ => match (a.as_long(), b.as_long()) {
                        (Some(x), Some(y)) if y >= 0 => {
                            self.push(self.make_long(x.pow(y as u32)));
                        }
                        _ => return Err(VmError::TypeError { expected: "number", got: "other" }),
                    }
                }
            }
            Opcode::Sqrt => {
                let a = self.pop()?;
                if let Some(x) = a.as_float() {
                    self.push(self.make_float(x.sqrt()));
                } else if let Some(x) = a.as_long() {
                    self.push(self.make_float((x as f64).sqrt()));
                } else {
                    return Err(VmError::TypeError { expected: "number", got: "other" });
                }
            }
            Opcode::Log => {
                let a = self.pop()?;
                if let Some(x) = a.as_float() {
                    self.push(self.make_float(x.ln()));
                } else if let Some(x) = a.as_long() {
                    self.push(self.make_float((x as f64).ln()));
                } else {
                    return Err(VmError::TypeError { expected: "number", got: "other" });
                }
            }
            Opcode::Trunc => {
                let a = self.pop()?;
                if let Some(x) = a.as_float() {
                    self.push(self.make_long(x.trunc() as i64));
                } else if a.as_long().is_some() {
                    self.push(a);
                } else {
                    return Err(VmError::TypeError { expected: "number", got: "other" });
                }
            }
            Opcode::Ceil => {
                let a = self.pop()?;
                if let Some(x) = a.as_float() {
                    self.push(self.make_float(x.ceil()));
                } else if a.as_long().is_some() {
                    self.push(a);
                } else {
                    return Err(VmError::TypeError { expected: "number", got: "other" });
                }
            }
            Opcode::FloorMath => {
                let a = self.pop()?;
                if let Some(x) = a.as_float() {
                    self.push(self.make_float(x.floor()));
                } else if a.as_long().is_some() {
                    self.push(a);
                } else {
                    return Err(VmError::TypeError { expected: "number", got: "other" });
                }
            }
            Opcode::Round => {
                let a = self.pop()?;
                if let Some(x) = a.as_float() {
                    self.push(self.make_float(x.round()));
                } else if a.as_long().is_some() {
                    self.push(a);
                } else {
                    return Err(VmError::TypeError { expected: "number", got: "other" });
                }
            }
            Opcode::Sin => self.op_unary_float(f64::sin)?,
            Opcode::Cos => self.op_unary_float(f64::cos)?,
            Opcode::Tan => self.op_unary_float(f64::tan)?,
            Opcode::Asin => self.op_unary_float(f64::asin)?,
            Opcode::Acos => self.op_unary_float(f64::acos)?,
            Opcode::Atan => self.op_unary_float(f64::atan)?,
            Opcode::IsNan => {
                let a = self.pop()?;
                let is_nan = a.as_float().map(|f| f.is_nan()).unwrap_or(false);
                self.push(self.make_bool(is_nan));
            }
            Opcode::IsInf => {
                let a = self.pop()?;
                let is_inf = a.as_float().map(|f| f.is_infinite()).unwrap_or(false);
                self.push(self.make_bool(is_inf));
            }

            // === Comparison ===
            Opcode::Lt => self.op_comparison(|a, b| a < b, |a, b| a < b)?,
            Opcode::Le => self.op_comparison(|a, b| a <= b, |a, b| a <= b)?,
            Opcode::Gt => self.op_comparison(|a, b| a > b, |a, b| a > b)?,
            Opcode::Ge => self.op_comparison(|a, b| a >= b, |a, b| a >= b)?,
            Opcode::Eq => {
                let b = self.pop()?;
                let a = self.pop()?;
                let equal = a.structurally_equivalent(&b);
                self.push(self.make_bool(equal));
            }
            Opcode::Ne => {
                let b = self.pop()?;
                let a = self.pop()?;
                let not_equal = !a.structurally_equivalent(&b);
                self.push(self.make_bool(not_equal));
            }
            Opcode::StructEq => {
                let b = self.pop()?;
                let a = self.pop()?;
                let equal = a.structurally_equivalent(&b);
                self.push(self.make_bool(equal));
            }

            // === Boolean ===
            Opcode::And => {
                let b = self.pop()?;
                let a = self.pop()?;
                match (a.as_bool(), b.as_bool()) {
                    (Some(x), Some(y)) => self.push(self.make_bool(x && y)),
                    _ => return Err(VmError::TypeError { expected: "bool", got: "other" }),
                }
            }
            Opcode::Or => {
                let b = self.pop()?;
                let a = self.pop()?;
                match (a.as_bool(), b.as_bool()) {
                    (Some(x), Some(y)) => self.push(self.make_bool(x || y)),
                    _ => return Err(VmError::TypeError { expected: "bool", got: "other" }),
                }
            }
            Opcode::Not => {
                let a = self.pop()?;
                match a.as_bool() {
                    Some(x) => self.push(self.make_bool(!x)),
                    None => return Err(VmError::TypeError { expected: "bool", got: "other" }),
                }
            }
            Opcode::Xor => {
                let b = self.pop()?;
                let a = self.pop()?;
                match (a.as_bool(), b.as_bool()) {
                    (Some(x), Some(y)) => self.push(self.make_bool(x ^ y)),
                    _ => return Err(VmError::TypeError { expected: "bool", got: "other" }),
                }
            }

            // === Type Operations ===
            Opcode::GetType => self.op_get_type()?,
            Opcode::CheckType => self.op_check_type()?,
            Opcode::IsType => self.op_is_type()?,
            Opcode::AssertType => self.op_assert_type()?,

            // === Pattern Matching ===
            Opcode::Match => self.op_match()?,
            Opcode::MatchBind => self.op_match_bind()?,
            Opcode::MatchHead => self.op_match_head()?,
            Opcode::MatchArity => self.op_match_arity()?,
            Opcode::MatchGuard => self.op_match_guard()?,
            Opcode::Unify => self.op_unify()?,
            Opcode::UnifyBind => self.op_unify_bind()?,
            Opcode::IsVariable => {
                let a = self.pop()?;
                let is_var = a.is_variable();
                self.push(self.make_bool(is_var));
            }
            Opcode::IsSExpr => {
                let a = self.pop()?;
                let is_sexpr = a.as_sexpr().is_some();
                self.push(self.make_bool(is_sexpr));
            }
            Opcode::IsSymbol => {
                let a = self.pop()?;
                let is_symbol = a.as_atom().is_some();
                self.push(self.make_bool(is_symbol));
            }
            Opcode::GetHead => {
                let a = self.pop()?;
                if let Some(items) = a.as_sexpr() {
                    if let Some(first) = items.first() {
                        self.push(first.clone());
                    } else {
                        self.push(self.make_nil());
                    }
                } else {
                    self.push(self.make_nil());
                }
            }
            Opcode::GetTail => {
                let a = self.pop()?;
                if let Some(items) = a.as_sexpr() {
                    if items.len() > 1 {
                        let tail: Vec<V> = items[1..].to_vec();
                        self.push(self.make_sexpr(tail));
                    } else {
                        self.push(self.make_sexpr(vec![]));
                    }
                } else {
                    self.push(self.make_nil());
                }
            }
            Opcode::GetArity => {
                let a = self.pop()?;
                if let Some(items) = a.as_sexpr() {
                    self.push(self.make_long(items.len() as i64));
                } else {
                    self.push(self.make_long(0));
                }
            }
            Opcode::GetElement => {
                let index = self.pop()?;
                let expr = self.pop()?;
                if let (Some(items), Some(i)) = (expr.as_sexpr(), index.as_long()) {
                    if i >= 0 && (i as usize) < items.len() {
                        self.push(items[i as usize].clone());
                    } else {
                        self.push(self.make_nil());
                    }
                } else {
                    self.push(self.make_nil());
                }
            }
            Opcode::DeconAtom => self.op_decon_atom()?,
            Opcode::Repr => self.op_repr()?,
            Opcode::GetMetaType => self.op_get_metatype()?,
            Opcode::ConsAtom => self.op_cons_atom()?,
            Opcode::MapAtom => self.op_map_atom()?,
            Opcode::FilterAtom => self.op_filter_atom()?,
            Opcode::FoldlAtom => self.op_foldl_atom()?,
            Opcode::IndexAtom => self.op_index_atom()?,
            Opcode::MinAtom => self.op_min_atom()?,
            Opcode::MaxAtom => self.op_max_atom()?,

            // === Nondeterminism ===
            Opcode::Fork => return self.op_fork(),
            Opcode::Fail => return self.op_fail(),
            Opcode::Cut => self.op_cut(),
            Opcode::Collect => self.op_collect()?,
            Opcode::CollectN => self.op_collect_n()?,
            Opcode::Yield => return self.op_yield(),
            Opcode::BeginNondet => self.op_begin_nondet(),
            Opcode::EndNondet => self.op_end_nondet()?,
            Opcode::Amb => self.op_amb()?,
            Opcode::Guard => return self.op_guard(),
            Opcode::Commit => self.op_commit(),
            Opcode::Backtrack => return self.op_fail(),

            // === Advanced Calls ===
            Opcode::CallNative => self.op_call_native()?,
            Opcode::CallExternal => self.op_call_external()?,
            Opcode::CallCached => self.op_call_cached()?,

            // === Environment Operations ===
            Opcode::DefineRule => self.op_define_rule()?,
            Opcode::LoadGlobal => self.op_load_global()?,
            Opcode::StoreGlobal => self.op_store_global()?,
            Opcode::DispatchRules => self.op_dispatch_rules()?,

            // === Space Operations ===
            Opcode::SpaceAdd => self.op_space_add()?,
            Opcode::SpaceRemove => self.op_space_remove()?,
            Opcode::SpaceGetAtoms => self.op_space_get_atoms()?,
            Opcode::SpaceMatch => self.op_space_match()?,
            Opcode::LoadSpace => self.op_load_space()?,

            // === State Operations ===
            Opcode::NewState => self.op_new_state()?,
            Opcode::GetState => self.op_get_state()?,
            Opcode::ChangeState => self.op_change_state()?,

            // === Debug ===
            Opcode::Breakpoint => self.op_breakpoint()?,
            Opcode::Trace => self.op_trace()?,
            Opcode::Halt => return Err(VmError::Halted),

            // Catch-all for any unhandled opcodes
            _ => {
                return Err(VmError::Runtime(format!(
                    "Opcode {:?} not yet implemented in generic VM",
                    opcode
                )));
            }
        }

        Ok(ControlFlow::Continue(()))
    }

    /// Handle reaching the end of a bytecode chunk.
    fn handle_chunk_end(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        if let Some(frame) = self.call_stack.pop() {
            // Return to caller
            let value = self.pop().unwrap_or_else(|_| self.make_nil());
            self.ip = frame.return_ip;
            self.chunk = frame.return_chunk;
            self.value_stack.truncate(frame.base_ptr);

            // Pop binding frame
            if self.bindings_stack.len() > frame.bindings_base {
                self.bindings_stack.truncate(frame.bindings_base + 1);
                self.bindings_stack.pop();
            }

            self.push(value);
            Ok(ControlFlow::Continue(()))
        } else {
            // End of top-level
            if !self.value_stack.is_empty() {
                self.results.extend(self.value_stack.drain(..));
            }
            Ok(ControlFlow::Break(std::mem::take(&mut self.results)))
        }
    }

    // === Helper Methods for Opcodes ===

    /// Binary numeric operation helper.
    #[inline]
    fn op_binary_num(
        &mut self,
        int_op: impl Fn(i64, i64) -> i64,
        float_op: impl Fn(f64, f64) -> f64,
    ) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        match (a.as_long(), b.as_long()) {
            (Some(x), Some(y)) => self.push(self.make_long(int_op(x, y))),
            _ => match (a.as_float(), b.as_float()) {
                (Some(x), Some(y)) => self.push(self.make_float(float_op(x, y))),
                _ => return Err(VmError::TypeError { expected: "number", got: "other" }),
            }
        }
        Ok(())
    }

    /// Unary float operation helper.
    #[inline]
    fn op_unary_float(&mut self, op: impl Fn(f64) -> f64) -> VmResult<()> {
        let a = self.pop()?;
        if let Some(x) = a.as_float() {
            self.push(self.make_float(op(x)));
        } else if let Some(x) = a.as_long() {
            self.push(self.make_float(op(x as f64)));
        } else {
            return Err(VmError::TypeError { expected: "number", got: "other" });
        }
        Ok(())
    }

    /// Comparison operation helper.
    #[inline]
    fn op_comparison(
        &mut self,
        int_cmp: impl Fn(i64, i64) -> bool,
        float_cmp: impl Fn(f64, f64) -> bool,
    ) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        match (a.as_long(), b.as_long()) {
            (Some(x), Some(y)) => self.push(self.make_bool(int_cmp(x, y))),
            _ => match (a.as_float(), b.as_float()) {
                (Some(x), Some(y)) => self.push(self.make_bool(float_cmp(x, y))),
                _ => return Err(VmError::TypeError { expected: "number", got: "other" }),
            }
        }
        Ok(())
    }

    /// Rot3: rotate top 3 stack elements (a b c -> b c a).
    fn op_rot3(&mut self) -> VmResult<()> {
        let len = self.value_stack.len();
        if len < 3 {
            return Err(VmError::StackUnderflow);
        }
        self.value_stack[len - 3..].rotate_left(1);
        Ok(())
    }

    /// Over: copy second element to top (a b -> a b a).
    fn op_over(&mut self) -> VmResult<()> {
        let value = self.peek_n(1)?.clone();
        self.push(value);
        Ok(())
    }

    /// DupN: duplicate n-th element to top.
    fn op_dup_n(&mut self) -> VmResult<()> {
        let n = self.read_u8()? as usize;
        let value = self.peek_n(n)?.clone();
        self.push(value);
        Ok(())
    }

    /// PopN: pop n elements from stack.
    fn op_pop_n(&mut self) -> VmResult<()> {
        let n = self.read_u8()? as usize;
        let len = self.value_stack.len();
        if n > len {
            return Err(VmError::StackUnderflow);
        }
        self.value_stack.truncate(len - n);
        Ok(())
    }

    // === Stub implementations for complex opcodes ===
    // These need full implementation with trait-based logic

    fn op_push_variable(&mut self) -> VmResult<()> {
        let index = self.read_u16()?;
        let var = self.chunk.get_constant(index)
            .ok_or(VmError::InvalidConstant(index))?
            .clone();

        // Check if it's a pattern variable that should be resolved from bindings
        if let Some(name) = var.as_atom() {
            if name.starts_with('$') {
                // Search bindings from innermost to outermost
                for frame in self.bindings_stack.iter().rev() {
                    if let Some(value) = frame.get(name) {
                        self.push(value.clone());
                        return Ok(());
                    }
                }
            }
        }

        // Not found in bindings or not a pattern variable - push as-is
        self.push(var);
        Ok(())
    }

    fn op_make_list(&mut self) -> VmResult<()> {
        let arity = self.read_u8()? as usize;
        let len = self.value_stack.len();
        if arity > len {
            return Err(VmError::StackUnderflow);
        }
        let items: Vec<V> = self.value_stack.drain((len - arity)..).collect();
        self.push(self.make_sexpr(items));
        Ok(())
    }

    fn op_make_quote(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let quote_atom = self.make_atom("quote");
        let quoted = self.make_sexpr(vec![quote_atom, value]);
        self.push(quoted);
        Ok(())
    }

    fn op_jump_table(&mut self) -> VmResult<()> {
        // Read number of cases and default offset
        let num_cases = self.read_u8()? as usize;
        let value = self.pop()?;

        // Find matching case
        for _ in 0..num_cases {
            let case_const_idx = self.read_u16()?;
            let case_offset = self.read_i16()?;
            if let Some(case_val) = self.chunk.get_constant(case_const_idx) {
                if value.structurally_equivalent(case_val) {
                    self.ip = (self.ip as isize + case_offset as isize) as usize;
                    return Ok(());
                }
            }
        }

        // No match - fall through
        Ok(())
    }

    fn op_call(&mut self) -> VmResult<()> {
        let head_idx = self.read_u16()?;
        let arity = self.read_u8()? as usize;

        // Pop arguments in reverse order
        let mut args = Vec::with_capacity(arity);
        for _ in 0..arity {
            args.push(self.pop()?);
        }
        args.reverse();

        // Build call expression from head + args
        let head = self.chunk.get_constant(head_idx).cloned()
            .ok_or(VmError::InvalidConstant(head_idx))?;
        let mut items = Vec::with_capacity(arity + 1);
        items.push(head);
        items.extend(args);
        let expr = self.make_sexpr(items);

        // Dispatch via environment rules
        self.push(expr);
        self.op_dispatch_rules()?;
        Ok(())
    }

    fn op_tail_call(&mut self) -> VmResult<()> {
        let head_idx = self.read_u16()?;
        let arity = self.read_u8()? as usize;

        // Pop arguments in reverse order
        let mut args = Vec::with_capacity(arity);
        for _ in 0..arity {
            args.push(self.pop()?);
        }
        args.reverse();

        // Build call expression from head + args
        let head = self.chunk.get_constant(head_idx).cloned()
            .ok_or(VmError::InvalidConstant(head_idx))?;
        let mut items = Vec::with_capacity(arity + 1);
        items.push(head);
        items.extend(args);
        let expr = self.make_sexpr(items);

        // Dispatch via environment rules (tail call - no new frame)
        self.push(expr);
        self.op_dispatch_rules()?;
        Ok(())
    }

    fn op_call_n(&mut self) -> VmResult<()> {
        // Call with N arguments
        self.op_call()
    }

    fn op_tail_call_n(&mut self) -> VmResult<()> {
        self.op_tail_call()
    }

    fn op_return(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        if let Some(frame) = self.call_stack.pop() {
            let value = self.pop().unwrap_or_else(|_| self.make_nil());
            self.ip = frame.return_ip;
            self.chunk = frame.return_chunk;
            self.value_stack.truncate(frame.base_ptr);
            self.push(value);
            Ok(ControlFlow::Continue(()))
        } else {
            // Top level return
            if !self.value_stack.is_empty() {
                self.results.extend(self.value_stack.drain(..));
            }
            Ok(ControlFlow::Break(std::mem::take(&mut self.results)))
        }
    }

    fn op_return_multi(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        let count = self.read_u8()? as usize;
        let len = self.value_stack.len();
        if count > len {
            return Err(VmError::StackUnderflow);
        }

        if let Some(frame) = self.call_stack.pop() {
            let values: Vec<V> = self.value_stack.drain((len - count)..).collect();
            self.ip = frame.return_ip;
            self.chunk = frame.return_chunk;
            self.value_stack.truncate(frame.base_ptr);
            for v in values {
                self.push(v);
            }
            Ok(ControlFlow::Continue(()))
        } else {
            self.results.extend(self.value_stack.drain(..));
            Ok(ControlFlow::Break(std::mem::take(&mut self.results)))
        }
    }

    fn op_get_type(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let type_name = value.type_name();
        self.push(self.make_atom(type_name));
        Ok(())
    }

    fn op_check_type(&mut self) -> VmResult<()> {
        let type_val = self.pop()?;
        let value = self.peek()?.clone();

        let type_name = if let Some(name) = type_val.as_atom() {
            name
        } else {
            return Err(VmError::TypeError { expected: "atom", got: "other" });
        };

        let matches = value.type_name() == type_name;
        self.push(self.make_bool(matches));
        Ok(())
    }

    fn op_is_type(&mut self) -> VmResult<()> {
        let type_val = self.pop()?;
        let value = self.pop()?;

        let type_name = if let Some(name) = type_val.as_atom() {
            name
        } else {
            return Err(VmError::TypeError { expected: "atom", got: "other" });
        };

        let matches = value.type_name() == type_name;
        self.push(self.make_bool(matches));
        Ok(())
    }

    fn op_assert_type(&mut self) -> VmResult<()> {
        let type_val = self.pop()?;
        let value = self.peek()?.clone();

        let type_name = if let Some(name) = type_val.as_atom() {
            name
        } else {
            return Err(VmError::TypeError { expected: "atom", got: "other" });
        };

        if value.type_name() != type_name {
            let error = self.make_error(
                &format!("Type assertion failed: expected {}", type_name),
                value,
            );
            self.push(error);
        }
        Ok(())
    }

    fn op_match(&mut self) -> VmResult<()> {
        let pattern = self.pop()?;
        let value = self.pop()?;
        let matches = self.pattern_matches_generic(&pattern, &value);
        self.push(self.make_bool(matches));
        Ok(())
    }

    fn op_match_bind(&mut self) -> VmResult<()> {
        let pattern = self.pop()?;
        let value = self.pop()?;

        if let Some(bindings) = self.pattern_match_bind_generic(&pattern, &value) {
            for (name, val) in bindings {
                self.set_binding(name, val);
            }
            self.push(self.make_bool(true));
        } else {
            self.push(self.make_bool(false));
        }
        Ok(())
    }

    fn op_match_head(&mut self) -> VmResult<()> {
        let head_pattern = self.pop()?;
        let value = self.pop()?;

        if let Some(items) = value.as_sexpr() {
            if let Some(head) = items.first() {
                let matches = self.pattern_matches_generic(&head_pattern, head);
                self.push(self.make_bool(matches));
                return Ok(());
            }
        }
        self.push(self.make_bool(false));
        Ok(())
    }

    fn op_match_arity(&mut self) -> VmResult<()> {
        let arity = self.read_u8()? as usize;
        let value = self.pop()?;

        let matches = if let Some(items) = value.as_sexpr() {
            items.len() == arity
        } else {
            false
        };
        self.push(self.make_bool(matches));
        Ok(())
    }

    fn op_match_guard(&mut self) -> VmResult<()> {
        let guard_result = self.pop()?;
        if guard_result.as_bool() != Some(true) {
            // Guard failed - push false
            self.push(self.make_bool(false));
        } else {
            self.push(self.make_bool(true));
        }
        Ok(())
    }

    fn op_unify(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;
        let unified = self.unify_generic(&a, &b).is_some();
        self.push(self.make_bool(unified));
        Ok(())
    }

    fn op_unify_bind(&mut self) -> VmResult<()> {
        let b = self.pop()?;
        let a = self.pop()?;

        if let Some(bindings) = self.unify_generic(&a, &b) {
            for (name, val) in bindings {
                self.set_binding(name, val);
            }
            self.push(self.make_bool(true));
        } else {
            self.push(self.make_bool(false));
        }
        Ok(())
    }

    fn op_decon_atom(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        if let Some(items) = value.as_sexpr() {
            if items.is_empty() {
                return Err(VmError::Runtime("decons-atom: empty expression".to_string()));
            }
            let head = items[0].clone();
            let tail = self.make_sexpr(items[1..].to_vec());
            // Return (head tail) pair - matching BytecodeVM semantics
            self.push(self.factory.sexpr(vec![head, tail]));
        } else {
            // Non-expression: return (atom ()) pair
            let empty_tail = self.make_sexpr(vec![]);
            self.push(self.factory.sexpr(vec![value, empty_tail]));
        }
        Ok(())
    }

    fn op_repr(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let repr = value.friendly_repr();
        self.push(self.make_string(&repr));
        Ok(())
    }

    fn op_get_metatype(&mut self) -> VmResult<()> {
        let value = self.pop()?;
        let metatype = if value.as_sexpr().is_some() {
            "Expression"
        } else if value.is_variable() {
            "Variable"
        } else if value.as_atom().is_some() {
            "Symbol"
        } else {
            "Grounded"
        };
        self.push(self.make_atom(metatype));
        Ok(())
    }

    fn op_cons_atom(&mut self) -> VmResult<()> {
        let tail = self.pop()?;
        let head = self.pop()?;

        let mut items = vec![head];
        if let Some(tail_items) = tail.as_sexpr() {
            items.extend(tail_items.iter().cloned());
        } else {
            items.push(tail);
        }
        self.push(self.make_sexpr(items));
        Ok(())
    }

    /// Map a template chunk over each element of an S-expression.
    /// Operand: u16 chunk_idx
    /// Stack: [list] -> [mapped_list]
    fn op_map_atom(&mut self) -> VmResult<()> {
        let chunk_idx = self.read_u16()?;
        let list = self.pop()?;

        let items = list.as_sexpr().ok_or(VmError::TypeError {
            expected: "list/S-expression",
            got: "other",
        })?;

        let template_chunk = self
            .chunk
            .get_chunk_constant(chunk_idx)
            .ok_or(VmError::InvalidConstant(chunk_idx))?;

        let mut results = Vec::with_capacity(items.len());
        for item in items {
            let result =
                self.execute_generic_template_with_binding(Arc::clone(&template_chunk), item.clone())?;
            results.push(result);
        }

        self.push(self.factory.sexpr(results));
        Ok(())
    }

    /// Filter elements of an S-expression using a predicate chunk.
    /// Operand: u16 chunk_idx
    /// Stack: [list] -> [filtered_list]
    fn op_filter_atom(&mut self) -> VmResult<()> {
        let chunk_idx = self.read_u16()?;
        let list = self.pop()?;

        let items = list.as_sexpr().ok_or(VmError::TypeError {
            expected: "list/S-expression",
            got: "other",
        })?;

        let predicate_chunk = self
            .chunk
            .get_chunk_constant(chunk_idx)
            .ok_or(VmError::InvalidConstant(chunk_idx))?;

        let mut results = Vec::new();
        for item in items {
            let result = self.execute_generic_template_with_binding(
                Arc::clone(&predicate_chunk),
                item.clone(),
            )?;
            // Check if predicate returned true
            if result.as_bool() == Some(true) {
                results.push(item.clone());
            }
        }

        self.push(self.factory.sexpr(results));
        Ok(())
    }

    /// Left fold over an S-expression using a template chunk.
    /// Operand: u16 chunk_idx
    /// Stack: [list, init] -> [result]
    fn op_foldl_atom(&mut self) -> VmResult<()> {
        let chunk_idx = self.read_u16()?;
        let init = self.pop()?;
        let list = self.pop()?;

        let items = list.as_sexpr().ok_or(VmError::TypeError {
            expected: "list/S-expression",
            got: "other",
        })?;

        let op_chunk = self
            .chunk
            .get_chunk_constant(chunk_idx)
            .ok_or(VmError::InvalidConstant(chunk_idx))?;

        let mut acc = init;
        for item in items {
            acc = self.execute_generic_foldl_template(Arc::clone(&op_chunk), acc, item.clone())?;
        }

        self.push(acc);
        Ok(())
    }

    // === Template Execution Helpers ===

    /// Execute a template chunk with a single bound value (for map/filter).
    /// Saves and restores VM state around execution.
    fn execute_generic_template_with_binding(
        &mut self,
        chunk: Arc<GenericBytecodeChunk<V>>,
        binding: V,
    ) -> VmResult<V> {
        // Save state
        let saved_ip = self.ip;
        let saved_chunk = Arc::clone(&self.chunk);
        let saved_stack_base = self.value_stack.len();

        // Setup for template execution
        self.chunk = chunk;
        self.ip = 0;
        self.push(binding); // Push bound value as local slot 0

        // Execute until Return or end of chunk
        loop {
            if self.ip >= self.chunk.len() {
                break;
            }
            let opcode_byte = self
                .chunk
                .read_byte(self.ip)
                .ok_or(VmError::IpOutOfBounds)?;
            let opcode =
                Opcode::from_byte(opcode_byte).ok_or(VmError::InvalidOpcode(opcode_byte))?;

            if opcode == Opcode::Return {
                break;
            }

            match self.step() {
                Ok(ControlFlow::Continue(())) => {}
                Ok(ControlFlow::Break(results)) => {
                    self.ip = saved_ip;
                    self.chunk = saved_chunk;
                    self.value_stack.truncate(saved_stack_base);
                    return Ok(results.into_iter().next().unwrap_or_else(|| self.factory.unit()));
                }
                Err(e) => {
                    self.ip = saved_ip;
                    self.chunk = saved_chunk;
                    self.value_stack.truncate(saved_stack_base);
                    return Err(e);
                }
            }
        }

        // Get result
        let result = self.pop().unwrap_or_else(|_| self.factory.unit());

        // Restore state
        self.ip = saved_ip;
        self.chunk = saved_chunk;

        // Cleanup any remaining stack entries from template
        self.value_stack.truncate(saved_stack_base);

        Ok(result)
    }

    /// Execute a foldl template chunk with accumulator and item bindings.
    /// Saves and restores VM state around execution.
    fn execute_generic_foldl_template(
        &mut self,
        chunk: Arc<GenericBytecodeChunk<V>>,
        acc: V,
        item: V,
    ) -> VmResult<V> {
        // Save state
        let saved_ip = self.ip;
        let saved_chunk = Arc::clone(&self.chunk);
        let saved_stack_base = self.value_stack.len();

        // Setup for template execution
        self.chunk = chunk;
        self.ip = 0;
        self.push(acc); // Local slot 0: accumulator
        self.push(item); // Local slot 1: item

        // Execute until Return or end of chunk
        loop {
            if self.ip >= self.chunk.len() {
                break;
            }
            let opcode_byte = self
                .chunk
                .read_byte(self.ip)
                .ok_or(VmError::IpOutOfBounds)?;
            let opcode =
                Opcode::from_byte(opcode_byte).ok_or(VmError::InvalidOpcode(opcode_byte))?;

            if opcode == Opcode::Return {
                break;
            }

            match self.step() {
                Ok(ControlFlow::Continue(())) => {}
                Ok(ControlFlow::Break(results)) => {
                    self.ip = saved_ip;
                    self.chunk = saved_chunk;
                    self.value_stack.truncate(saved_stack_base);
                    return Ok(results.into_iter().next().unwrap_or_else(|| self.factory.unit()));
                }
                Err(e) => {
                    self.ip = saved_ip;
                    self.chunk = saved_chunk;
                    self.value_stack.truncate(saved_stack_base);
                    return Err(e);
                }
            }
        }

        // Get result
        let result = self.pop().unwrap_or_else(|_| self.factory.unit());

        // Restore state
        self.ip = saved_ip;
        self.chunk = saved_chunk;
        self.value_stack.truncate(saved_stack_base);

        Ok(result)
    }

    fn op_index_atom(&mut self) -> VmResult<()> {
        let index = self.pop()?;
        let expr = self.pop()?;

        if let (Some(items), Some(i)) = (expr.as_sexpr(), index.as_long()) {
            if i >= 0 && (i as usize) < items.len() {
                self.push(items[i as usize].clone());
            } else {
                let error = self.make_error("Index out of bounds", index);
                self.push(error);
            }
        } else {
            self.push(self.make_nil());
        }
        Ok(())
    }

    fn op_min_atom(&mut self) -> VmResult<()> {
        let expr = self.pop()?;
        if let Some(items) = expr.as_sexpr() {
            let mut min: Option<V> = None;
            for item in items {
                if let Some(current_min) = &min {
                    if let (Some(a), Some(b)) = (item.as_long(), current_min.as_long()) {
                        if a < b {
                            min = Some(item.clone());
                        }
                    }
                } else {
                    min = Some(item.clone());
                }
            }
            self.push(min.unwrap_or_else(|| self.make_nil()));
        } else {
            self.push(self.make_nil());
        }
        Ok(())
    }

    fn op_max_atom(&mut self) -> VmResult<()> {
        let expr = self.pop()?;
        if let Some(items) = expr.as_sexpr() {
            let mut max: Option<V> = None;
            for item in items {
                if let Some(current_max) = &max {
                    if let (Some(a), Some(b)) = (item.as_long(), current_max.as_long()) {
                        if a > b {
                            max = Some(item.clone());
                        }
                    }
                } else {
                    max = Some(item.clone());
                }
            }
            self.push(max.unwrap_or_else(|| self.make_nil()));
        } else {
            self.push(self.make_nil());
        }
        Ok(())
    }

    // === Nondeterminism Stubs ===

    fn op_fork(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        // Create choice point with alternatives from sub-chunks
        let num_alts = self.read_u8()? as usize;

        // Zero alternatives means immediate failure
        if num_alts == 0 {
            return self.op_fail();
        }

        let mut alternatives = Vec::with_capacity(num_alts);

        for _ in 0..num_alts {
            let chunk_idx = self.read_u16()?;
            if let Some(chunk) = self.chunk.get_chunk_constant(chunk_idx) {
                alternatives.push(GenericAlternative::Chunk(chunk));
            }
        }

        if !alternatives.is_empty() {
            let choice_point = GenericChoicePoint {
                value_stack_height: self.value_stack.len(),
                call_stack_height: self.call_stack.len(),
                bindings_stack_height: self.bindings_stack.len(),
                ip: self.ip,
                chunk: Arc::clone(&self.chunk),
                alternatives,
            };
            self.choice_points.push(choice_point);
        }
        Ok(ControlFlow::Continue(()))
    }

    fn op_fail(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        // Backtrack to most recent choice point
        if let Some(mut cp) = self.choice_points.pop() {
            if let Some(alt) = cp.alternatives.pop() {
                // Restore state
                self.value_stack.truncate(cp.value_stack_height);
                self.call_stack.truncate(cp.call_stack_height);
                self.bindings_stack.truncate(cp.bindings_stack_height);

                // Put choice point back if more alternatives
                if !cp.alternatives.is_empty() {
                    self.choice_points.push(cp.clone());
                }

                // Execute alternative
                match alt {
                    GenericAlternative::Value(v) => {
                        self.push(v);
                        self.ip = cp.ip;
                        self.chunk = cp.chunk;
                    }
                    GenericAlternative::Chunk(chunk) => {
                        self.chunk = chunk;
                        self.ip = 0;
                    }
                    GenericAlternative::Index(i) => {
                        self.push(self.make_long(i as i64));
                        self.ip = cp.ip;
                        self.chunk = cp.chunk;
                    }
                    GenericAlternative::RuleMatch { chunk, bindings } => {
                        // Apply bindings
                        for (name, val) in bindings.iter() {
                            self.set_binding(name.to_string(), val.clone());
                        }
                        self.chunk = chunk;
                        self.ip = 0;
                    }
                }
                return Ok(ControlFlow::Continue(()));
            }
        }

        // No more alternatives - return results
        Ok(ControlFlow::Break(std::mem::take(&mut self.results)))
    }

    fn op_cut(&mut self) {
        // Remove all choice points
        self.choice_points.clear();
    }

    fn op_collect(&mut self) -> VmResult<()> {
        // Collect all results from nondeterministic evaluation
        let collected = std::mem::take(&mut self.results);
        self.push(self.make_sexpr(collected));
        Ok(())
    }

    fn op_collect_n(&mut self) -> VmResult<()> {
        let n = self.read_u8()? as usize;
        let len = self.results.len();
        let take = len.min(n);
        let collected: Vec<V> = self.results.drain((len - take)..).collect();
        self.push(self.make_sexpr(collected));
        Ok(())
    }

    fn op_yield(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        let value = self.pop()?;
        self.results.push(value);
        // Continue to next alternative
        self.op_fail()
    }

    fn op_begin_nondet(&mut self) {
        // Mark beginning of nondeterministic section
        // Nothing to do in simple implementation
    }

    fn op_end_nondet(&mut self) -> VmResult<()> {
        // End nondeterministic section
        Ok(())
    }

    fn op_amb(&mut self) -> VmResult<()> {
        // Ambiguous choice - create choice point for each value
        let choices = self.pop()?;
        if let Some(items) = choices.as_sexpr() {
            let alternatives: Vec<GenericAlternative<V, GenericBytecodeChunk<V>>> =
                items.iter().map(|v| GenericAlternative::Value(v.clone())).collect();

            if !alternatives.is_empty() {
                let choice_point = GenericChoicePoint {
                    value_stack_height: self.value_stack.len(),
                    call_stack_height: self.call_stack.len(),
                    bindings_stack_height: self.bindings_stack.len(),
                    ip: self.ip,
                    chunk: Arc::clone(&self.chunk),
                    alternatives,
                };
                self.choice_points.push(choice_point);
            }
        }
        Ok(())
    }

    fn op_guard(&mut self) -> VmResult<ControlFlow<Vec<V>>> {
        let guard = self.pop()?;
        if guard.as_bool() != Some(true) {
            return self.op_fail();
        }
        Ok(ControlFlow::Continue(()))
    }

    fn op_commit(&mut self) {
        // Commit to current choice - remove most recent choice point
        self.choice_points.pop();
    }

    // === Advanced Calls ===

    /// Call a native Rust function by ID.
    /// Stack: [arg1, arg2, ..., argN] -> [result]
    fn op_call_native(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::call", ip = self.ip, "call_native (generic)");
        use super::native_registry::GenericNativeContext;

        let func_id = self.read_u16()?;
        let arity = self.read_u8()? as usize;

        // Pop arguments in reverse order
        let mut args = Vec::with_capacity(arity);
        for _ in 0..arity {
            args.push(self.pop()?);
        }
        args.reverse();

        // Create context for native function
        let env = self
            .env
            .clone()
            .unwrap_or_else(|| GenericEnvironment::new(self.factory.clone()));
        let ctx = GenericNativeContext::new(env, self.factory.clone());

        // Call through registry
        let result = self
            .native_registry
            .call(func_id, &args, &ctx)
            .map_err(|e| VmError::Runtime(e.to_string()))?;

        // Push result(s)
        if result.len() == 1 {
            self.push(result.into_iter().next().expect("result has 1 element"));
        } else if result.is_empty() {
            self.push(self.factory.unit());
        } else {
            // Multiple results - push as S-expression
            self.push(self.factory.sexpr(result));
        }

        Ok(())
    }

    /// Call an external FFI function by name.
    /// Stack: [arg1, arg2, ..., argN] -> [result]
    fn op_call_external(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::call", ip = self.ip, "call_external (generic)");
        use super::external_registry::{ExternalError, GenericExternalContext};

        let symbol_idx = self.read_u16()?;
        let arity = self.read_u8()? as usize;

        // Get function name from constant pool
        let func_name = self
            .chunk
            .get_constant(symbol_idx)
            .and_then(|v| v.as_atom().map(|s| s.to_string()))
            .ok_or(VmError::InvalidConstant(symbol_idx))?;

        // Pop arguments in reverse order
        let mut args = Vec::with_capacity(arity);
        for _ in 0..arity {
            args.push(self.pop()?);
        }
        args.reverse();

        // Create context for external function
        let env = self
            .env
            .clone()
            .unwrap_or_else(|| GenericEnvironment::new(self.factory.clone()));
        let ctx = GenericExternalContext::new(env, self.factory.clone());

        // Call through registry
        match self.external_registry.call(&func_name, &args, &ctx) {
            Ok(results) => {
                if results.len() == 1 {
                    self.push(results.into_iter().next().expect("results has 1 element"));
                } else if results.is_empty() {
                    self.push(self.factory.unit());
                } else {
                    self.push(self.factory.sexpr(results));
                }
                Ok(())
            }
            Err(ExternalError::NotFound(_)) => Err(VmError::Runtime(format!(
                "External function '{}' not registered",
                func_name
            ))),
            Err(e) => Err(VmError::Runtime(format!("External call error: {}", e))),
        }
    }

    /// Call a function with memoization.
    /// Stack: [arg1, arg2, ..., argN] -> [result]
    ///
    /// Checks the memo cache first. On miss, builds the call expression,
    /// dispatches via environment rules, and caches the result.
    fn op_call_cached(&mut self) -> VmResult<()> {
        trace!(target: "mettatron::vm::call", ip = self.ip, "call_cached (generic)");

        let head_idx = self.read_u16()?;
        let arity = self.read_u8()? as usize;

        let head = self
            .chunk
            .get_constant(head_idx)
            .cloned()
            .ok_or(VmError::InvalidConstant(head_idx))?;

        // Extract head as string for cache key
        let head_str = head
            .as_atom()
            .map(|s| s.to_string())
            .unwrap_or_else(|| head.type_name().to_string());

        // Pop arguments in reverse order
        let mut args = Vec::with_capacity(arity);
        for _ in 0..arity {
            args.push(self.pop()?);
        }
        args.reverse();

        // Check memo cache first
        if let Some(cached) = self.memo_cache.get(&head_str, &args) {
            self.push(cached);
            return Ok(());
        }

        // Build the call expression
        let mut items = Vec::with_capacity(arity + 1);
        items.push(head);
        items.extend(args.clone());
        let expr = self.factory.sexpr(items);

        // Dispatch via environment rules (push expr, dispatch pops and pushes result)
        self.push(expr);
        self.op_dispatch_rules()?;

        // Cache the result (peek at top of stack)
        if let Some(result) = self.value_stack.last() {
            self.memo_cache.insert(&head_str, &args, result.clone());
        }

        Ok(())
    }

    // === Environment Operations ===

    fn op_define_rule(&mut self) -> VmResult<()> {
        use crate::backend::models::GenericRule;
        trace!(target: "mettatron::vm::rules", ip = self.ip, "define_rule (generic)");

        let body = self.pop()?;
        let pattern = self.pop()?;

        // Environment is required for DefineRule
        let env = self.env.as_mut().ok_or_else(|| {
            VmError::Runtime(
                "DefineRule requires environment (use GenericBytecodeVM::with_env)".to_string(),
            )
        })?;

        // Create and add the generic rule
        let rule = GenericRule::new(pattern, body);
        env.add_generic_rule(rule);

        // Push Unit to indicate success
        self.push(self.make_unit());
        Ok(())
    }

    fn op_load_global(&mut self) -> VmResult<()> {
        let index = self.read_u16()?;
        let name = self.chunk.get_constant(index)
            .and_then(|v| v.as_atom().map(|s| s.to_string()))
            .ok_or(VmError::InvalidConstant(index))?;

        // Try to load from environment using get_binding
        if let Some(env) = &self.env {
            if let Some(value) = env.get_binding(&name) {
                self.push(value);
                return Ok(());
            }
        }
        self.push(self.make_nil());
        Ok(())
    }

    fn op_store_global(&mut self) -> VmResult<()> {
        let index = self.read_u16()?;
        let name = self.chunk.get_constant(index)
            .and_then(|v| v.as_atom().map(|s| s.to_string()))
            .ok_or(VmError::InvalidConstant(index))?;
        let value = self.pop()?;

        // Store in environment using bind
        if let Some(env) = &mut self.env {
            env.bind(&name, value);
        }
        Ok(())
    }

    fn op_dispatch_rules(&mut self) -> VmResult<()> {
        use crate::backend::eval::bindings_generic::{apply_bindings_generic, pattern_match_generic};
        trace!(target: "mettatron::vm::rules", ip = self.ip, "dispatch_rules (generic)");

        // Pop the call expression from the stack
        let expr = self.pop()?;

        // Extract head symbol and arity for indexed rule lookup
        let (head, arity) = if let Some(items) = expr.as_sexpr() {
            if items.is_empty() {
                // Empty expression - return unchanged
                self.push(expr);
                return Ok(());
            }
            if let Some(name) = items[0].as_atom() {
                (name, items.len() - 1)
            } else {
                // Head is not an atom - return expression unchanged
                self.push(expr);
                return Ok(());
            }
        } else if let Some(name) = expr.as_atom() {
            (name, 0)
        } else {
            // Not a callable expression - return unchanged
            self.push(expr);
            return Ok(());
        };

        // Get environment reference
        let env = match &self.env {
            Some(e) => e,
            None => {
                // No environment - return expression unchanged (irreducible)
                self.push(expr);
                return Ok(());
            }
        };

        // Look up matching rules by head symbol and arity using generic method
        let candidate_rules = env.get_matching_rules_vec(head, arity);

        if candidate_rules.is_empty() {
            // No rules match - return expression unchanged
            self.push(expr);
            return Ok(());
        }

        // Try to pattern match each rule against the expression
        let mut matches: Vec<(V, GenericBindings<V>)> = Vec::new();
        for rule in &candidate_rules {
            if let Some(bindings) = pattern_match_generic(&rule.lhs, &expr) {
                // Found a match - apply bindings to the rule body
                let instantiated_body = apply_bindings_generic(&rule.rhs, &bindings, &self.factory);
                matches.push((instantiated_body, bindings));
            }
        }

        if matches.is_empty() {
            // Pattern matching failed for all rules - return expression unchanged
            self.push(expr);
            return Ok(());
        }

        if matches.len() == 1 {
            // Single match - push the instantiated body for further evaluation
            let (body, bindings) = matches.into_iter().next().expect("matches has 1 element");

            // Set up bindings in the current binding frame
            if let Some(frame) = self.bindings_stack.last_mut() {
                for (name, value) in bindings.iter() {
                    frame.set(name.to_string(), value.clone());
                }
            }

            // Push the instantiated body - caller will continue evaluation
            self.push(body);
            return Ok(());
        }

        // Multiple matches - create choice point for nondeterminism
        // First match executes now, others become alternatives
        let mut alternatives: Vec<GenericAlternative<V, GenericBytecodeChunk<V>>> = Vec::with_capacity(matches.len() - 1);
        let mut first_match: Option<(V, GenericBindings<V>)> = None;

        for (body, bindings) in matches {
            if first_match.is_none() {
                first_match = Some((body, bindings));
            } else {
                // Store as alternative value
                alternatives.push(GenericAlternative::Value(body));
            }
        }

        // Create choice point for backtracking to alternatives
        self.choice_points.push(GenericChoicePoint {
            ip: self.ip,
            chunk: Arc::clone(&self.chunk),
            value_stack_height: self.value_stack.len(),
            call_stack_height: self.call_stack.len(),
            bindings_stack_height: self.bindings_stack.len(),
            alternatives,
        });

        // Execute first match
        if let Some((body, bindings)) = first_match {
            // Set up bindings in the current binding frame
            if let Some(frame) = self.bindings_stack.last_mut() {
                for (name, value) in bindings.iter() {
                    frame.set(name.to_string(), value.clone());
                }
            }

            // Push the instantiated body
            self.push(body);
        }

        Ok(())
    }

    // === Space Operations ===

    fn op_space_add(&mut self) -> VmResult<()> {
        let atom = self.pop()?;
        if let Some(env) = &mut self.env {
            env.add_to_space(&atom);
        }
        self.push(self.make_unit());
        Ok(())
    }

    fn op_space_remove(&mut self) -> VmResult<()> {
        let atom = self.pop()?;
        if let Some(env) = &mut self.env {
            // remove_from_space is void, we just push true to indicate success
            env.remove_from_space(&atom);
            self.push(self.make_bool(true));
        } else {
            self.push(self.make_bool(false));
        }
        Ok(())
    }

    fn op_space_get_atoms(&mut self) -> VmResult<()> {
        if let Some(env) = &self.env {
            let atoms = env.get_all_atoms();
            self.push(self.make_sexpr(atoms));
        } else {
            self.push(self.make_sexpr(vec![]));
        }
        Ok(())
    }

    fn op_space_match(&mut self) -> VmResult<()> {
        let template = self.pop()?;
        let pattern = self.pop()?;
        if let Some(env) = &self.env {
            // match_space returns Vec<MultiplicityMatch<V>>, extract the values
            let matches = env.match_space(&pattern, &template);
            let results: Vec<V> = matches.into_iter()
                .flat_map(|m| std::iter::repeat(m.value).take(m.count))
                .collect();
            self.push(self.make_sexpr(results));
        } else {
            self.push(self.make_sexpr(vec![]));
        }
        Ok(())
    }

    fn op_load_space(&mut self) -> VmResult<()> {
        // Load default space atoms - not directly available in GenericEnvironment
        // Return empty list for now
        self.push(self.make_sexpr(vec![]));
        Ok(())
    }

    // === State Operations ===

    fn op_new_state(&mut self) -> VmResult<()> {
        let initial = self.pop()?;

        let env = self.env.as_mut().ok_or_else(|| {
            VmError::Runtime("new-state requires environment".to_string())
        })?;

        let state_id = env.create_state(&initial);
        self.push(self.factory.state(state_id));
        Ok(())
    }

    fn op_get_state(&mut self) -> VmResult<()> {
        let state_ref = self.pop()?;

        if let Some(state_id) = state_ref.as_state() {
            let env = self.env.as_ref().ok_or_else(|| {
                VmError::Runtime("get-state requires environment".to_string())
            })?;

            if let Some(value) = env.get_state(state_id) {
                self.push(value);
                Ok(())
            } else {
                Err(VmError::Runtime(format!("get-state: state {} not found", state_id)))
            }
        } else {
            Err(VmError::TypeError { expected: "State", got: state_ref.type_name() })
        }
    }

    fn op_change_state(&mut self) -> VmResult<()> {
        let new_value = self.pop()?;
        let state_ref = self.pop()?;

        if let Some(state_id) = state_ref.as_state() {
            let env = self.env.as_mut().ok_or_else(|| {
                VmError::Runtime("change-state! requires environment".to_string())
            })?;

            if env.change_state(state_id, &new_value) {
                self.push(self.factory.state(state_id));
                Ok(())
            } else {
                Err(VmError::Runtime(format!("change-state!: state {} not found", state_id)))
            }
        } else {
            Err(VmError::TypeError { expected: "State", got: state_ref.type_name() })
        }
    }

    // === Debug Operations ===

    fn op_breakpoint(&mut self) -> VmResult<()> {
        // Breakpoint - just continue
        Ok(())
    }

    fn op_trace(&mut self) -> VmResult<()> {
        // Trace - print stack top
        if let Ok(value) = self.peek() {
            trace!(target: "mettatron::vm::trace", value = %value.friendly_repr());
        }
        Ok(())
    }

    // === Pattern Matching Helpers ===

    /// Check if pattern matches value using trait methods.
    fn pattern_matches_generic(&self, pattern: &V, value: &V) -> bool {
        // Variable matches anything
        if pattern.is_variable() {
            return true;
        }

        // Wildcard matches anything
        if pattern.as_atom() == Some("_") {
            return true;
        }

        // Check structural equality for non-expressions
        if pattern.as_sexpr().is_none() && value.as_sexpr().is_none() {
            return pattern.structurally_equivalent(value);
        }

        // Match s-expressions recursively
        if let (Some(p_items), Some(v_items)) = (pattern.as_sexpr(), value.as_sexpr()) {
            if p_items.len() != v_items.len() {
                return false;
            }
            for (p, v) in p_items.iter().zip(v_items.iter()) {
                if !self.pattern_matches_generic(p, v) {
                    return false;
                }
            }
            return true;
        }

        false
    }

    /// Pattern match with binding extraction.
    fn pattern_match_bind_generic(&self, pattern: &V, value: &V) -> Option<Vec<(String, V)>> {
        let mut bindings = Vec::new();
        if self.pattern_match_bind_recursive(pattern, value, &mut bindings) {
            Some(bindings)
        } else {
            None
        }
    }

    fn pattern_match_bind_recursive(
        &self,
        pattern: &V,
        value: &V,
        bindings: &mut Vec<(String, V)>,
    ) -> bool {
        // Variable captures value
        if pattern.is_variable() {
            if let Some(name) = pattern.as_atom() {
                bindings.push((name.to_string(), value.clone()));
            }
            return true;
        }

        // Wildcard matches but doesn't bind
        if pattern.as_atom() == Some("_") {
            return true;
        }

        // Non-expressions must match exactly
        if pattern.as_sexpr().is_none() && value.as_sexpr().is_none() {
            return pattern.structurally_equivalent(value);
        }

        // Match s-expressions recursively
        if let (Some(p_items), Some(v_items)) = (pattern.as_sexpr(), value.as_sexpr()) {
            if p_items.len() != v_items.len() {
                return false;
            }
            for (p, v) in p_items.iter().zip(v_items.iter()) {
                if !self.pattern_match_bind_recursive(p, v, bindings) {
                    return false;
                }
            }
            return true;
        }

        false
    }

    /// Unification with binding extraction.
    fn unify_generic(&self, a: &V, b: &V) -> Option<Vec<(String, V)>> {
        let mut bindings = Vec::new();
        if self.unify_recursive(a, b, &mut bindings) {
            Some(bindings)
        } else {
            None
        }
    }

    fn unify_recursive(&self, a: &V, b: &V, bindings: &mut Vec<(String, V)>) -> bool {
        // Variables unify with anything
        if a.is_variable() {
            if let Some(name) = a.as_atom() {
                bindings.push((name.to_string(), b.clone()));
            }
            return true;
        }
        if b.is_variable() {
            if let Some(name) = b.as_atom() {
                bindings.push((name.to_string(), a.clone()));
            }
            return true;
        }

        // Non-expressions must match
        if a.as_sexpr().is_none() && b.as_sexpr().is_none() {
            return a.structurally_equivalent(b);
        }

        // Unify s-expressions recursively
        if let (Some(a_items), Some(b_items)) = (a.as_sexpr(), b.as_sexpr()) {
            if a_items.len() != b_items.len() {
                return false;
            }
            for (x, y) in a_items.iter().zip(b_items.iter()) {
                if !self.unify_recursive(x, y, bindings) {
                    return false;
                }
            }
            return true;
        }

        false
    }
}

// ============================================================================
// Type Aliases for Backwards Compatibility
// ============================================================================

/// Type alias for heap-based generic VM.
///
/// This is equivalent to the non-generic `BytecodeVM` but uses the generic
/// infrastructure. Useful for testing generic code paths with heap values.
pub type HeapGenericBytecodeVM = GenericBytecodeVM<MettaValue, crate::backend::models::HeapMettaValueFactory>;
