# Tiered Compiler Implementation Guide

A comprehensive guide for implementing a tiered JIT compilation system, based on MeTTaTron's architecture. This document provides sufficient detail to port the design to other languages such as Rholang.

## Table of Contents

1. [Architecture Overview](#architecture-overview)
2. [Profiling Infrastructure](#profiling-infrastructure)
3. [Value Representation: NaN-Boxing](#value-representation-nan-boxing)
4. [Cranelift Integration](#cranelift-integration)
5. [HybridExecutor Pattern](#hybridexecutor-pattern)
6. [JitContext Design](#jitcontext-design)
7. [Runtime Helper Functions](#runtime-helper-functions)
8. [Caching Strategy](#caching-strategy)
9. [Portability Considerations for Rholang](#portability-considerations-for-rholang)
10. [Implementation Checklist](#implementation-checklist)

---

## Architecture Overview

The tiered execution system progressively optimizes code based on execution frequency:

```text
┌─────────────────────────────────────────────────────────────────────────────┐
│                         Tiered Execution Model                              │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                             │
│   Tier 0: Interpreter (Cold)    Tier 1: Bytecode VM (Warm)                  │
│   ┌─────────────────────────┐   ┌─────────────────────────┐                 │
│   │ • 0-1 executions        │   │ • 2+ executions         │                 │
│   │ • Tree-walker           │   │ • Stack-based VM        │                 │
│   │ • Maximum flexibility   │   │ • Lower overhead        │                 │
│   │ • Zero compilation      │   │ • Simple compilation    │                 │
│   └───────────┬─────────────┘   └───────────┬─────────────┘                 │
│               │                             │                               │
│               │ count >= 2                  │ count >= 100                  │
│               └─────────────────────────────┼───────────────────────────────┤
│                                             ▼                               │
│   Tier 3: JIT Stage 2 (Very Hot)    Tier 2: JIT Stage 1 (Hot)               │
│   ┌─────────────────────────┐       ┌─────────────────────────┐             │
│   │ • 500+ executions       │       │ • 100+ executions       │             │
│   │ • Full optimization     │◄──────│ • Basic native code     │             │
│   │ • Runtime helpers       │ 500   │ • Arithmetic/boolean    │             │
│   │ • All opcodes           │       │ • Stack operations      │             │
│   └─────────────────────────┘       └─────────────────────────┘             │
│                                                                             │
└─────────────────────────────────────────────────────────────────────────────┘
```

### Core Design Principles

1. **Progressive optimization**: Invest compilation effort proportional to execution frequency
2. **Transparent fallback**: Higher tiers fall back to lower tiers gracefully
3. **Thread-safe compilation**: Multiple threads may trigger compilation; only one wins
4. **Zero allocation hot path**: Pre-allocated buffers for JIT execution context

### State Machine

Each compilable code unit tracks its JIT state:

```text
          ┌───────────────────────────────────────────────────────────┐
          │                  JIT State Machine                        │
          └───────────────────────────────────────────────────────────┘

                              ┌────────┐
                              │  Cold  │ ◄─── Initial state
                              │ n < 10 │
                              └───┬────┘
                                  │ n >= WARM_THRESHOLD (10)
                                  ▼
                              ┌────────┐
                              │Warming │
                              │ n < 100│
                              └───┬────┘
                                  │ n >= HOT_THRESHOLD (100)
                                  ▼
                              ┌────────┐
                    ┌────────►│  Hot   │◄────────┐
                    │         └───┬────┘         │
                    │             │              │
                    │     try_start_compiling()  │
                    │             │              │
              (loser)      ┌──────┴──────┐      (wait for
                    │      │             │       completion)
                    │     WIN          LOSE──────┘
                    │      │
                    │      ▼
                    │  ┌──────────┐
                    │  │Compiling │
                    │  └────┬─────┘
                    │       │
                    │  ┌────┴────┐
                    │  │         │
                    │ OK      FAIL
                    │  │         │
                    │  ▼         ▼
                    │ ┌────┐   ┌────────┐
                    └─│Jitted│  │ Failed │
                      └────┘   └────────┘
```

---

## Profiling Infrastructure

Accurate profiling is essential for effective tiering.

### Execution Counter

Each code unit has an atomic execution counter:

```rust
// From src/backend/bytecode/jit/profile.rs:80-93
#[derive(Debug)]
pub struct JitProfile {
    execution_count: AtomicU32,
    state: AtomicU8,
    native_code: AtomicPtr<()>,
    code_size: AtomicU32,
}
```

### Recording Executions

The `record_execution` method handles state transitions atomically:

```rust
// From src/backend/bytecode/jit/profile.rs:106-152
#[inline]
pub fn record_execution(&self) -> bool {
    let current_state = self.state();

    match current_state {
        JitState::Cold => {
            let count = self.execution_count.fetch_add(1, Ordering::Relaxed) + 1;
            if count >= WARM_THRESHOLD {
                // Try to transition to Warming
                let _ = self.state.compare_exchange(
                    JitState::Cold as u8,
                    JitState::Warming as u8,
                    Ordering::Release,
                    Ordering::Relaxed,
                );
            }
            false
        }
        JitState::Warming => {
            let count = self.execution_count.fetch_add(1, Ordering::Relaxed) + 1;
            if count >= HOT_THRESHOLD {
                // Try to transition to Hot - return true if we won the race
                self.state
                    .compare_exchange(
                        JitState::Warming as u8,
                        JitState::Hot as u8,
                        Ordering::Release,
                        Ordering::Relaxed,
                    )
                    .is_ok()
            } else {
                false
            }
        }
        JitState::Hot | JitState::Compiling | JitState::Jitted | JitState::Failed => {
            // Keep counting for statistics but don't trigger transitions
            if self.execution_count.load(Ordering::Relaxed) < MAX_EXECUTION_COUNT {
                self.execution_count.fetch_add(1, Ordering::Relaxed);
            }
            false
        }
    }
}
```

### Winner-Take-All Compilation

Only one thread compiles:

```rust
// From src/backend/bytecode/jit/profile.rs:178-192
pub fn try_start_compiling(&self) -> bool {
    self.state
        .compare_exchange(
            JitState::Hot as u8,
            JitState::Compiling as u8,
            Ordering::AcqRel,
            Ordering::Relaxed,
        )
        .is_ok()
}
```

The compare-exchange ensures exactly one thread wins the compilation race. Losing threads can either wait for completion or fall back to the bytecode VM.

### Thresholds

```rust
// From src/backend/bytecode/jit/profile.rs:17-28
pub const HOT_THRESHOLD: u32 = 100;
pub const WARM_THRESHOLD: u32 = 10;
pub const MAX_EXECUTION_COUNT: u32 = u32::MAX - 1;
```

These thresholds are tunable. Lower values increase JIT coverage but may not amortize compilation cost; higher values miss optimization opportunities.

---

## Value Representation: NaN-Boxing

NaN-boxing encodes type and value in a single 64-bit word, enabling efficient type checks without pointer indirection.

### IEEE 754 Quiet NaN Layout

```text
IEEE 754 double-precision:
┌───────────────────────────────────────────────────────────────────────────┐
│ Sign (1) │ Exponent (11)  │ Mantissa (52)                                 │
│   [63]   │ [62:52]        │ [51:0]                                        │
└───────────────────────────────────────────────────────────────────────────┘

Quiet NaN: Exponent = 0x7FF, Mantissa bit 51 = 1

NaN-boxed layout:
┌───────────────────────────────────────────────────────────────────────────┐
│ 0 │ 0x7FF │ 1 │ Tag (3 bits) │ Payload (48 bits)                          │
│   │       │   │ [50:48]      │ [47:0]                                     │
└───────────────────────────────────────────────────────────────────────────┘
```

### Tag Definitions

```rust
// From src/backend/bytecode/jit/types/constants.rs:24-49
pub(super) const QNAN: u64 = 0x7FF8_0000_0000_0000;

pub const TAG_LONG: u64 = QNAN | (0 << 48); // 0x7FF8: 48-bit signed integer
pub const TAG_BOOL: u64 = QNAN | (1 << 48); // 0x7FF9: Boolean (0/1)
pub const TAG_NIL: u64 = QNAN | (2 << 48);  // 0x7FFA: Nil
pub const TAG_UNIT: u64 = QNAN | (3 << 48); // 0x7FFB: Unit ()
pub const TAG_HEAP: u64 = QNAN | (4 << 48); // 0x7FFC: Heap pointer
pub const TAG_ERROR: u64 = QNAN | (5 << 48);// 0x7FFD: Error value
pub const TAG_ATOM: u64 = QNAN | (6 << 48); // 0x7FFE: Symbol/atom
pub const TAG_VAR: u64 = QNAN | (7 << 48);  // 0x7FFF: Variable

pub const TAG_MASK: u64 = 0xFFFF_0000_0000_0000;
pub const PAYLOAD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
```

### Type Checking in JIT Code

Type checks become cheap bitwise operations:

```rust
// Check type: 2-3 CPU cycles
fn is_long(v: u64) -> bool {
    (v & TAG_MASK) == TAG_LONG
}

// Extract value: 2-4 CPU cycles (with sign extension)
fn get_long(v: u64) -> i64 {
    let payload = v & PAYLOAD_MASK;
    // Sign extend from 48 bits to 64 bits
    if payload & 0x0000_8000_0000_0000 != 0 {
        (payload | 0xFFFF_0000_0000_0000) as i64
    } else {
        payload as i64
    }
}
```

### JitValue Implementation

```rust
// From src/backend/bytecode/jit/types/value.rs:32-56
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
pub struct JitValue(pub u64);

impl JitValue {
    #[inline(always)]
    pub const fn from_long(n: i64) -> Self {
        let payload = (n as u64) & PAYLOAD_MASK;
        JitValue(TAG_LONG | payload)
    }

    #[inline(always)]
    pub const fn from_bool(b: bool) -> Self {
        JitValue(TAG_BOOL | (b as u64))
    }

    #[inline(always)]
    pub const fn nil() -> Self {
        JitValue(TAG_NIL)
    }

    #[inline(always)]
    pub fn from_heap_ptr(ptr: *const MettaValue) -> Self {
        let addr = ptr as u64;
        JitValue(TAG_HEAP | (addr & PAYLOAD_MASK))
    }
}
```

### 48-bit Pointer Compatibility

Modern x86-64 uses 48-bit canonical addresses. Heap pointers fit in the 48-bit payload:

```rust
// From src/backend/bytecode/jit/types/value.rs:81-89
pub fn from_heap_ptr(ptr: *const MettaValue) -> Self {
    let addr = ptr as u64;
    debug_assert!(
        addr & TAG_MASK == 0,
        "Pointer uses more than 48 bits: {:#x}",
        addr
    );
    JitValue(TAG_HEAP | (addr & PAYLOAD_MASK))
}
```

---

## Cranelift Integration

Cranelift is a fast code generator suitable for JIT compilation.

### JIT Module Setup

```rust
// Conceptual structure (from src/backend/bytecode/jit/compiler/mod.rs)
pub struct JitCompiler {
    builder_context: FunctionBuilderContext,
    module: JITModule,
    runtime_func_ids: RuntimeFuncIds,
}

impl JitCompiler {
    pub fn new() -> Result<Self, JitError> {
        // 1. Create ISA for current platform
        let isa = cranelift_native::builder()
            .expect("Host platform not supported")
            .finish(settings::Flags::new(settings::builder()))
            .expect("Failed to create ISA");

        // 2. Create JIT module
        let builder = JITBuilder::with_isa(isa, default_libcall_names());
        let module = JITModule::new(builder);

        // 3. Register runtime functions
        let runtime_func_ids = register_runtime_symbols(&mut module)?;

        Ok(JitCompiler {
            builder_context: FunctionBuilderContext::new(),
            module,
            runtime_func_ids,
        })
    }
}
```

### Runtime Function Registration

Runtime helpers are declared using `extern "C"`:

```rust
// Step 1: Define helper functions with C ABI
#[no_mangle]
pub extern "C" fn jit_runtime_pow(ctx: *mut JitContext, base: i64, exp: i64) -> i64 {
    if exp < 0 {
        return JitValue::nil().to_bits() as i64;
    }
    let result = (base as i64).pow(exp as u32);
    JitValue::from_long(result).to_bits() as i64
}

// Step 2: Register with Cranelift
fn register_runtime_symbols(module: &mut JITModule) -> Result<RuntimeFuncIds, JitError> {
    // Declare function signature
    let mut sig = module.make_signature();
    sig.params.push(AbiParam::new(types::I64)); // ctx
    sig.params.push(AbiParam::new(types::I64)); // base
    sig.params.push(AbiParam::new(types::I64)); // exp
    sig.returns.push(AbiParam::new(types::I64)); // result

    // Declare and define
    let func_id = module
        .declare_function("jit_runtime_pow", Linkage::Import, &sig)?;

    let ptr = jit_runtime_pow as *const u8;
    module.define_function_bytes(
        func_id,
        &[],  // No relocations
        &[],  // No constant data
        std::slice::from_raw_parts(ptr, 1), // Symbol resolution
    )?;

    Ok(RuntimeFuncIds { pow_func: func_id })
}
```

### Compiling a Function

```rust
pub fn compile(&mut self, chunk: &BytecodeChunk) -> Result<*const (), JitError> {
    // 1. Create function signature: fn(*mut JitContext) -> i64
    let mut sig = self.module.make_signature();
    sig.params.push(AbiParam::new(types::I64)); // ctx pointer
    sig.returns.push(AbiParam::new(types::I64)); // NaN-boxed result

    // 2. Declare function
    let func_id = self.module.declare_anonymous_function(&sig)?;

    // 3. Build function body
    let mut ctx = self.module.make_context();
    ctx.func.signature = sig;

    {
        let mut builder = FunctionBuilder::new(&mut ctx.func, &mut self.builder_context);

        // Entry block
        let entry_block = builder.create_block();
        builder.append_block_params_for_function_params(entry_block);
        builder.switch_to_block(entry_block);
        builder.seal_block(entry_block);

        let ctx_ptr = builder.block_params(entry_block)[0];

        // Generate IR for each bytecode instruction
        let mut codegen = CodegenContext::new(&mut builder, ctx_ptr);
        for (offset, opcode) in chunk.iter_opcodes() {
            compile_opcode(&mut codegen, opcode, offset)?;
        }

        builder.finalize();
    }

    // 4. Compile to native code
    self.module.define_function(func_id, &mut ctx)?;
    self.module.finalize_definitions()?;

    // 5. Get executable pointer
    let code_ptr = self.module.get_finalized_function(func_id);
    Ok(code_ptr as *const ())
}
```

### FuncId Management

Organize runtime functions by category for maintainability:

```rust
pub struct RuntimeFuncIds {
    // Arithmetic
    pub pow_func: FuncId,
    pub sqrt_func: FuncId,

    // Pattern matching
    pub match_pattern_func: FuncId,
    pub unify_func: FuncId,

    // Space operations
    pub space_add_func: FuncId,
    pub space_match_func: FuncId,

    // Nondeterminism
    pub fork_func: FuncId,
    pub yield_func: FuncId,
    pub fail_func: FuncId,

    // ... more categories
}
```

---

## HybridExecutor Pattern

The HybridExecutor handles tier dispatch and graceful fallback.

### Structure

```rust
// From src/backend/bytecode/jit/hybrid/executor.rs:37-78
pub struct HybridExecutor {
    // Shared caches
    jit_cache: Arc<JitCache>,
    tiered_compiler: Arc<TieredCompiler>,

    // Configuration and statistics
    config: HybridConfig,
    stats: HybridStats,

    // Pre-allocated JIT buffers (zero allocation hot path)
    jit_stack: Vec<JitValue>,
    jit_choice_points: Vec<JitChoicePoint>,
    jit_results: Vec<JitValue>,
    jit_binding_frames: Vec<JitBindingFrame>,
    jit_cut_markers: Vec<usize>,
    heap_tracker: Vec<*mut MettaValue>,

    // Optional runtime context
    bridge: Option<Arc<MorkBridge>>,
    external_registry: Option<*const ()>,
    memo_cache: Option<*const ()>,
    space_registry: Option<*mut ()>,
    env: Option<*mut ()>,
}
```

### Execution Flow

```rust
// From src/backend/bytecode/jit/hybrid/executor.rs:291-338
pub fn run(&mut self, chunk: &Arc<BytecodeChunk>) -> VmResult<Vec<MettaValue>> {
    self.stats.total_runs += 1;

    if !self.config.jit_enabled {
        return self.run_vm(chunk);
    }

    let chunk_id = ChunkId::from_chunk(chunk);

    // Fast path: Check cache first
    if let Some(native_ptr) = self.jit_cache.get(&chunk_id) {
        return self.execute_jit(chunk, native_ptr);
    }

    // Record execution and check tier
    self.tiered_compiler.record_execution(chunk);
    let tier = self.tiered_compiler.get_tier(chunk);

    match tier {
        Tier::Interpreter | Tier::Bytecode => {
            // Not hot enough - use VM
            self.run_vm(chunk)
        }
        Tier::JitStage1 | Tier::JitStage2 => {
            // Hot enough - try to compile
            if let Some(native_ptr) = self.try_compile(chunk, &chunk_id, tier) {
                self.execute_jit(chunk, native_ptr)
            } else {
                // Compilation failed - use VM
                self.run_vm(chunk)
            }
        }
    }
}
```

### Bailout Handling

When JIT code cannot handle an operation, it bails out:

```rust
// From src/backend/bytecode/jit/hybrid/executor.rs:566-597
if ctx.bailout {
    self.stats.jit_bailouts += 1;

    // Transfer JIT stack to VM
    let mut vm_stack = Vec::with_capacity(ctx.sp);
    for i in 0..ctx.sp {
        let jit_val = unsafe { *ctx.value_stack.add(i) };
        let metta_val = unsafe { jit_val.to_metta() };
        vm_stack.push(metta_val);
    }

    // Cleanup heap allocations
    unsafe { ctx.cleanup_heap_allocations(); }

    // Resume from bailout point
    let mut vm = BytecodeVM::with_config(Arc::clone(chunk), self.config.vm_config.clone());
    return vm.resume_from_bailout(ctx.bailout_ip, vm_stack);
}
```

---

## JitContext Design

JitContext provides the runtime context for JIT-compiled code.

### C-Compatible Layout

```rust
// Use #[repr(C)] for predictable memory layout
#[repr(C)]
pub struct JitContext {
    // Value stack
    pub value_stack: *mut JitValue,
    pub sp: usize,
    pub stack_cap: usize,

    // Constant pool
    pub constants: *const MettaValue,
    pub constants_len: usize,

    // Bailout handling
    pub bailout: bool,
    pub bailout_ip: usize,
    pub bailout_reason: JitBailoutReason,

    // Nondeterminism
    pub choice_points: *mut JitChoicePoint,
    pub choice_point_count: usize,
    pub choice_point_cap: usize,
    pub results: *mut JitValue,
    pub results_count: usize,
    pub results_cap: usize,

    // Binding frames for pattern matching
    pub binding_frames: *mut JitBindingFrame,
    pub binding_frames_count: usize,
    pub binding_frames_cap: usize,

    // Cut markers for Prolog-style cut
    pub cut_markers: *mut usize,
    pub cut_marker_count: usize,
    pub cut_marker_cap: usize,

    // External resources
    pub bridge_ptr: *const (),
    pub external_registry: *const (),
    pub memo_cache: *const (),
    pub space_registry: *mut (),
    pub env: *mut (),
    pub current_chunk: *const (),

    // Optimization caches
    pub state_cache: [(u64, u64); STATE_CACHE_SIZE],
    pub var_index_cache: [(u64, u32); VAR_INDEX_CACHE_SIZE],
    pub stack_save_pool: *mut JitValue,
    pub stack_save_pool_cap: usize,
    pub stack_save_pool_next: usize,

    // Heap tracking
    heap_tracker: *mut Vec<*mut MettaValue>,
}
```

### Stack Operations

```rust
impl JitContext {
    #[inline(always)]
    pub unsafe fn push(&mut self, value: JitValue) {
        debug_assert!(self.sp < self.stack_cap, "JIT stack overflow");
        *self.value_stack.add(self.sp) = value;
        self.sp += 1;
    }

    #[inline(always)]
    pub unsafe fn pop(&mut self) -> JitValue {
        debug_assert!(self.sp > 0, "JIT stack underflow");
        self.sp -= 1;
        *self.value_stack.add(self.sp)
    }

    #[inline(always)]
    pub unsafe fn peek(&self) -> JitValue {
        debug_assert!(self.sp > 0, "JIT stack underflow");
        *self.value_stack.add(self.sp - 1)
    }
}
```

### Bailout Signaling

```rust
impl JitContext {
    pub fn trigger_bailout(&mut self, ip: usize, reason: JitBailoutReason) {
        self.bailout = true;
        self.bailout_ip = ip;
        self.bailout_reason = reason;
    }
}

#[derive(Debug, Clone, Copy)]
pub enum JitBailoutReason {
    UnsupportedOperation,
    TypeError,
    StackOverflow,
    DivisionByZero,
    PatternMatchRequired,
}
```

### Choice Points for Nondeterminism

```rust
#[repr(C)]
#[derive(Clone)]
pub struct JitChoicePoint {
    /// Saved stack pointer
    pub saved_sp: usize,

    /// Resume instruction pointer (for backtracking)
    pub resume_ip: usize,

    /// Inline alternatives (avoid heap allocation)
    pub alternatives: [u32; MAX_ALTERNATIVES_INLINE],
    pub alternatives_count: usize,
    pub current_alternative: usize,

    /// Pointer to saved stack values
    pub saved_stack: *const JitValue,
    pub saved_stack_len: usize,
}
```

---

## Runtime Helper Functions

Runtime helpers bridge between JIT code and complex operations.

### Function Signature Conventions

All runtime helpers use `extern "C"` with this pattern:

```rust
#[no_mangle]
pub extern "C" fn jit_runtime_<category>_<operation>(
    ctx: *mut JitContext,
    // ... additional parameters as i64 (NaN-boxed)
) -> i64 // NaN-boxed result
```

### Categories

| Category | Purpose | Examples |
|----------|---------|----------|
| arithmetic | Extended math ops | `pow`, `sqrt`, `log` |
| bindings | Variable binding | `bind_var`, `lookup_var` |
| calls | Function dispatch | `call_external`, `call_cached` |
| nondet | Nondeterminism | `fork`, `yield_result`, `fail`, `cut` |
| pattern_matching | Unification | `match_pattern`, `unify` |
| rules | Rule dispatch | `match_all_rules`, `apply_rule` |
| space | Space operations | `space_add`, `space_match`, `space_remove` |
| special_forms | Control flow | `if_eval`, `quote_eval` |
| type_ops | Type system | `get_type`, `check_type` |
| sexpr | S-expression ops | `make_sexpr`, `get_car`, `get_cdr` |
| higher_order | Higher-order | `map`, `fold`, `filter` |
| globals | Global state | `get_state`, `set_state` |
| debug | Development | `print_stack`, `trace_exec` |

### Example: Nondeterminism Fork

```rust
#[no_mangle]
pub extern "C" fn jit_runtime_fork_native(
    ctx: *mut JitContext,
    alternatives_ptr: *const JitValue,
    alternatives_count: usize,
) -> i64 {
    let ctx = unsafe { &mut *ctx };

    if alternatives_count == 0 {
        // No alternatives - fail
        return JIT_SIGNAL_FAIL;
    }

    if alternatives_count == 1 {
        // Single alternative - no choice point needed
        let alt = unsafe { *alternatives_ptr };
        unsafe { ctx.push(alt); }
        return JIT_SIGNAL_OK;
    }

    // Create choice point
    if ctx.choice_point_count >= ctx.choice_point_cap {
        ctx.trigger_bailout(0, JitBailoutReason::StackOverflow);
        return JIT_SIGNAL_BAILOUT;
    }

    let cp = unsafe { &mut *ctx.choice_points.add(ctx.choice_point_count) };
    cp.saved_sp = ctx.sp;
    cp.alternatives_count = alternatives_count;
    cp.current_alternative = 0;

    // Copy alternatives inline
    for i in 0..alternatives_count.min(MAX_ALTERNATIVES_INLINE) {
        let alt = unsafe { *alternatives_ptr.add(i) };
        cp.alternatives[i] = alt.to_bits() as u32;
    }

    ctx.choice_point_count += 1;

    // Push first alternative
    let first = unsafe { *alternatives_ptr };
    unsafe { ctx.push(first); }

    JIT_SIGNAL_OK
}
```

---

## Caching Strategy

Effective caching minimizes repeated compilation and improves cache hit rates.

### ChunkId: Hash-Based Keying

```rust
// From src/backend/bytecode/jit/tiered.rs
#[derive(Clone, Copy, Hash, Eq, PartialEq)]
pub struct ChunkId(u64);

impl ChunkId {
    pub fn from_chunk(chunk: &BytecodeChunk) -> Self {
        use std::hash::{Hash, Hasher};
        use std::collections::hash_map::DefaultHasher;

        let mut hasher = DefaultHasher::new();
        chunk.code().hash(&mut hasher);
        ChunkId(hasher.finish())
    }
}
```

### JitCache with LRU Eviction

```rust
pub struct JitCache {
    entries: RwLock<HashMap<ChunkId, CacheEntry>>,
    max_entries: usize,
    max_code_bytes: usize,
    current_code_bytes: AtomicUsize,
}

pub struct CacheEntry {
    pub native_code: *const (),
    pub code_size: usize,
    pub profile: Arc<JitProfile>,
    pub tier: Tier,
    pub last_access: Instant,
}

impl JitCache {
    pub fn get(&self, id: &ChunkId) -> Option<*const ()> {
        let entries = self.entries.read().unwrap();
        entries.get(id).map(|e| {
            // Update access time (approximate LRU)
            e.last_access = Instant::now();
            e.native_code
        })
    }

    pub fn insert(&self, id: ChunkId, entry: CacheEntry) {
        let mut entries = self.entries.write().unwrap();

        // Evict if at capacity
        while entries.len() >= self.max_entries {
            self.evict_lru(&mut entries);
        }

        self.current_code_bytes.fetch_add(entry.code_size, Ordering::Relaxed);
        entries.insert(id, entry);
    }

    fn evict_lru(&self, entries: &mut HashMap<ChunkId, CacheEntry>) {
        if let Some((oldest_id, oldest_entry)) = entries
            .iter()
            .min_by_key(|(_, e)| e.last_access)
            .map(|(id, e)| (*id, e.clone()))
        {
            self.current_code_bytes.fetch_sub(oldest_entry.code_size, Ordering::Relaxed);
            entries.remove(&oldest_id);
        }
    }
}
```

### Profile Survival Across Eviction

When code is evicted from cache, profiles survive to preserve execution counts:

```rust
// Profiles are stored in Arc, shared between cache and chunk
pub profile: Arc<JitProfile>,

// When evicted, profile remains in the BytecodeChunk
// Re-compilation sees the accumulated execution count
```

---

## Portability Considerations for Rholang

Adapting this architecture for Rholang requires understanding both what to keep and what to adapt.

### What to Keep

1. **Tiered execution model**: The 4-tier progression (Interpreter → Bytecode → JIT Stage 1 → JIT Stage 2) applies universally.

2. **NaN-boxing**: 48-bit payloads work for any language. Rholang can use:
   - TAG_LONG: Process identifiers
   - TAG_ATOM: Channel names
   - TAG_HEAP: Complex processes, patterns

3. **Atomic profiling**: The lock-free state machine works regardless of language semantics.

4. **Cranelift integration**: Platform-independent code generation.

5. **HybridExecutor pattern**: Graceful fallback from JIT to VM.

### What to Adapt

1. **Process representation**: Rholang's core types differ from MeTTa:

   ```rust
   // Rholang-specific NaN-boxing
   pub const TAG_PROCESS: u64 = QNAN | (0 << 48);  // Process reference
   pub const TAG_CHANNEL: u64 = QNAN | (1 << 48);  // Channel name
   pub const TAG_NAME: u64 = QNAN | (2 << 48);     // Quoted channel
   pub const TAG_PAR: u64 = QNAN | (3 << 48);      // Parallel composition
   pub const TAG_SEND: u64 = QNAN | (4 << 48);     // Send operation
   pub const TAG_RECEIVE: u64 = QNAN | (5 << 48);  // Receive pattern
   pub const TAG_HEAP: u64 = QNAN | (6 << 48);     // Complex structures
   pub const TAG_NIL: u64 = QNAN | (7 << 48);      // Nil process
   ```

2. **Runtime helpers**: Rholang-specific operations:

   ```rust
   // Channel operations
   extern "C" fn jit_runtime_send(ctx: *mut RhoContext, channel: i64, data: i64) -> i64;
   extern "C" fn jit_runtime_receive(ctx: *mut RhoContext, pattern: i64) -> i64;

   // Parallel composition
   extern "C" fn jit_runtime_par(ctx: *mut RhoContext, left: i64, right: i64) -> i64;

   // Name operations
   extern "C" fn jit_runtime_new_channel(ctx: *mut RhoContext) -> i64;
   extern "C" fn jit_runtime_quote(ctx: *mut RhoContext, process: i64) -> i64;
   ```

3. **Concurrency model**: Rholang's concurrency differs fundamentally:

   - MeTTa: Nondeterministic choice (superposition)
   - Rholang: True concurrent processes on channels

   The JitContext needs:
   ```rust
   // Rholang process context
   pub struct RhoContext {
       // Standard fields
       pub value_stack: *mut JitValue,
       pub sp: usize,

       // Rholang-specific
       pub pending_sends: *mut SendQueue,
       pub pending_receives: *mut ReceiveQueue,
       pub channel_registry: *mut ChannelRegistry,
       pub reduction_rules: *mut ReductionRules,
   }
   ```

4. **Compilation triggers**: Rholang may want different thresholds:

   ```rust
   // Rholang might use per-channel hotness
   pub const CHANNEL_HOT_THRESHOLD: u32 = 50;  // Frequent communication
   pub const PROCESS_HOT_THRESHOLD: u32 = 100; // Frequently spawned
   ```

### Shared Rayon Scheduler Compatibility

Both MeTTaTron and Rholang use Rayon for parallelism. They can share a scheduler:

```rust
// Initialize once at startup
rayon::ThreadPoolBuilder::new()
    .num_threads(num_cpus::get())
    .build_global()
    .expect("Failed to initialize Rayon");

// Both compilers use rayon::spawn() for background compilation
#[cfg(feature = "hybrid-p2-priority-scheduler")]
{
    if is_sequential_mode() {
        rayon::spawn(compile_task);
    } else {
        global_priority_eval_pool().spawn_with_priority(
            compile_task,
            priority_levels::BACKGROUND_COMPILE,
            TaskTypeId::JitCompile,
        );
    }
}
```

### Integration Points

1. **Shared cache**: Rholang and MeTTa can share a unified JIT cache if they share a Cranelift module.

2. **Cross-language calls**: If Rholang calls MeTTa (or vice versa), bailout to a common interpreter layer.

3. **Unified profiling**: The P2 scheduler can prioritize compilation across both languages.

---

## Implementation Checklist

### Phase 1: Foundation

- [ ] Define NaN-boxing constants for target language
- [ ] Implement `JitValue` type with constructors and accessors
- [ ] Implement `JitProfile` with atomic state machine
- [ ] Define bytecode instruction set if not already present

### Phase 2: Cranelift Setup

- [ ] Add Cranelift dependencies to `Cargo.toml`
- [ ] Implement `JitCompiler::new()` with ISA detection
- [ ] Define `RuntimeFuncIds` structure
- [ ] Implement `register_runtime_symbols()`

### Phase 3: Code Generation

- [ ] Implement `compile_opcode()` for basic operations
- [ ] Implement stack operations (push, pop, dup)
- [ ] Implement arithmetic operations
- [ ] Implement comparison operations
- [ ] Implement control flow (jump, branch)

### Phase 4: HybridExecutor

- [ ] Implement `HybridExecutor` structure
- [ ] Implement tier dispatch logic
- [ ] Implement bailout handling
- [ ] Implement stack transfer (JIT → VM)

### Phase 5: Runtime Helpers

- [ ] Implement arithmetic helpers (pow, sqrt, etc.)
- [ ] Implement nondeterminism helpers (fork, yield, fail)
- [ ] Implement pattern matching helpers
- [ ] Implement language-specific helpers

### Phase 6: Caching

- [ ] Implement `ChunkId` hashing
- [ ] Implement `JitCache` with LRU eviction
- [ ] Add memory pressure monitoring
- [ ] Test cache hit rates

### Phase 7: Testing

- [ ] Unit tests for JitValue
- [ ] Unit tests for JitProfile state machine
- [ ] Integration tests for compilation
- [ ] Benchmark against interpreter baseline

### Phase 8: Optimization

- [ ] Profile hot paths
- [ ] Add optimization caches (state cache, var cache)
- [ ] Implement pre-allocation pools
- [ ] Tune thresholds based on benchmarks

---

## References

- **Source files**:
  - `src/backend/bytecode/jit/profile.rs` - Profiling infrastructure
  - `src/backend/bytecode/jit/types/` - JitValue, JitContext
  - `src/backend/bytecode/jit/compiler/` - Cranelift code generation
  - `src/backend/bytecode/jit/hybrid/` - HybridExecutor

- **Related documentation**:
  - `docs/optimization/jit/JIT_PIPELINE_ARCHITECTURE.md` - Pedagogical overview
  - `docs/guides/BYTECODE_JIT_IMPLEMENTATION.md` - Step-by-step guide
  - `docs/optimization/jit/BYTECODE_AND_JIT_IR_SPEC.md` - Opcode specification
  - `docs/architecture/HYBRID_P2_PRIORITY_SCHEDULER.md` - Background compilation scheduling

- **External resources**:
  - [Cranelift Documentation](https://cranelift.dev/)
  - [IEEE 754 NaN-boxing](https://piotrduperas.com/posts/nan-boxing)
