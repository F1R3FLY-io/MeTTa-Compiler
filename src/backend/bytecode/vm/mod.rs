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

use std::ops::ControlFlow;
use std::sync::Arc;
use tracing::trace;

use super::chunk::BytecodeChunk;
use super::external_registry::ExternalRegistry;
#[cfg(test)]
use super::memo_cache::CacheStats;
use super::memo_cache::MemoCache;
use super::mork_bridge::MorkBridge;
use super::native_registry::NativeRegistry;
use super::opcodes::Opcode;
use crate::backend::models::MettaValue;
use crate::backend::Environment;

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

// === Re-exports ===

pub use pattern::{pattern_match_bind, pattern_matches, unify};
pub use types::{Alternative, BindingFrame, CallFrame, ChoicePoint, VmConfig, VmError, VmResult};

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
    pub(super) env: Option<Environment>,
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
    pub fn with_env(chunk: Arc<BytecodeChunk>, env: Environment) -> Self {
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
        env: Environment,
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
    pub fn with_environment(mut self, env: Environment) -> Self {
        self.env = Some(env);
        self
    }

    // === Environment Accessors ===

    /// Get a reference to the environment, if present.
    pub fn environment(&self) -> Option<&Environment> {
        self.env.as_ref()
    }

    /// Take ownership of the environment, returning it.
    ///
    /// This is used to return the modified environment after execution.
    pub fn take_environment(&mut self) -> Option<Environment> {
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
            self.value_stack.resize(local_count, MettaValue::Nil);
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
    pub fn run_with_env(&mut self) -> VmResult<(Vec<MettaValue>, Option<Environment>)> {
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
    ///
    /// Note: JIT compilation is not yet implemented in this PR. This function
    /// always returns Ok(None) to fall back to the interpreter.
    #[allow(dead_code)]
    fn try_jit_execute(&mut self) -> VmResult<Option<Vec<MettaValue>>> {
        // JIT compiler not yet available - always use interpreter
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
            Opcode::PushNil => self.push(MettaValue::Nil),
            Opcode::PushTrue => self.push(MettaValue::Bool(true)),
            Opcode::PushFalse => self.push(MettaValue::Bool(false)),
            Opcode::PushUnit => self.push(MettaValue::Unit),
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
            Opcode::Fork => self.op_fork()?,
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
            let value = self.pop().unwrap_or(MettaValue::Nil);
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
                self.results.append(&mut self.value_stack);
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
