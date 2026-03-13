# JIT Architecture

## 1. Compilation Pipeline

```
MeTTa Source
    │
    ▼
┌──────────────────┐
│  Tree-Sitter     │  Parse to S-expression IR (SExpr)
│  Parser          │
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│  compile()       │  SExpr  -->  MettaValue AST
│  (compile.rs)    │
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│  Bytecode        │  MettaValue AST  -->  BytecodeChunk
│  Compiler        │  (compile_arc in bytecode/compiler.rs)
│  (Tier 1)        │
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│  Cranelift JIT   │  BytecodeChunk  -->  native x86-64
│  Compiler        │  (JitCompiler in jit/compiler/mod.rs)
│  (Tier 2/3)      │
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│  Native Code     │  fn(*mut JitContext) -> i64
│  Execution       │
└──────────────────┘
```

The JIT compiler operates on **bytecode**, not on the AST directly. This
isolates the JIT from parsing concerns and guarantees that every expression
entering the JIT has already been validated by the bytecode compiler.

### 1.1 Cranelift Translation Steps

Given a `BytecodeChunk`, the `JitCompiler::compile()` method proceeds as
follows:

1. **Compilability check** -- `can_compile_stage1(chunk)` verifies that every
   opcode in the chunk is supported and the chunk has no nondeterminism flag.

2. **Function declaration** -- A unique Cranelift function with signature
   `fn(i64) -> i64` is declared in the `JITModule`. The single `i64` parameter
   is the `*mut JitContext` pointer; the `i64` return is a signal code.

3. **Block analysis** -- `find_block_info(chunk)` pre-scans bytecode to locate
   all jump targets and count predecessors. Merge points (predecessor count > 1)
   receive an `i64` block parameter for SSA phi values.

4. **IR construction** -- `build_function()` iterates bytecodes linearly.
   `translate_opcode()` dispatches each opcode to its handler module. Handlers
   emit Cranelift IR into a `CodegenContext` which tracks a simulated value
   stack of SSA `Value` handles.

5. **Finalization** -- `module.define_function()` runs Cranelift's register
   allocator, instruction selection, and machine code emission.
   `module.finalize_definitions()` links the code.
   `module.get_finalized_function()` returns the executable function pointer.

### 1.2 Function Signature

Every JIT-compiled function has the C calling convention:

```rust
unsafe extern "C" fn jit_chunk_N(ctx: *mut JitContext) -> i64
```

Return values (signal codes):

| Constant | Value | Meaning |
|----------|-------|---------|
| `JIT_SIGNAL_OK` | 0 | Normal completion |
| `JIT_SIGNAL_YIELD` | 2 | Result saved, try next alternative |
| `JIT_SIGNAL_FAIL` | 3 | Current path failed, backtrack |
| `JIT_SIGNAL_ERROR` | -1 | Error occurred |
| `JIT_SIGNAL_HALT` | -2 | Explicit halt requested |
| `JIT_SIGNAL_BAILOUT` | -3 | JIT cannot continue, fall back to VM |

The actual MeTTa result value is communicated through the `JitContext`'s value
stack, not through the return value.

---

## 2. NaN-Boxing Value Representation

All values flowing through JIT-compiled code are represented as 64-bit
NaN-boxed integers (`JitValue`). The scheme exploits the IEEE 754
double-precision quiet NaN encoding:

```
Bit Layout (64 bits):
┌─────────────────────┬───────┬───────┬──────────────────────────────────────────────────┐
│  Exponent (11 bits) │ Quiet │  Tag  │              Payload (48 bits)                   │
│  0x7FF              │  1    │ 3 bit │                                                  │
├─────────────────────┴───────┴───────┴──────────────────────────────────────────────────┤
│ 63                  52  51   50  48  47                                               0│
└────────────────────────────────────────────────────────────────────────────────────────┘
```

### 2.1 Tag Assignments

| Tag | Hex Prefix | Type | Payload |
|-----|------------|------|---------|
| 0 | `0x7FF8` | Long | 48-bit signed integer (sign-extended) |
| 1 | `0x7FF9` | Bool | bit 0: 0 = false, 1 = true |
| 2 | `0x7FFA` | Empty | (no payload) zero-result marker |
| 3 | `0x7FFB` | Unit | (no payload) |
| 4 | `0x7FFC` | Ptr | 48-bit pointer to `*const MettaValueInner` |
| 5 | `0x7FFD` | Error | 48-bit pointer to error `MettaValueInner` |
| 6 | `0x7FFE` | Atom | 48-bit pointer to interned `String` |
| 7 | `0x7FFF` | Var | 48-bit pointer to variable name `String` |

Constants from `src/backend/bytecode/jit/types/constants.rs`:

```rust
pub const TAG_LONG:  u64 = 0x7FF8_0000_0000_0000;
pub const TAG_BOOL:  u64 = 0x7FF9_0000_0000_0000;
pub const TAG_EMPTY: u64 = 0x7FFA_0000_0000_0000;
pub const TAG_UNIT:  u64 = 0x7FFB_0000_0000_0000;
pub const TAG_PTR:   u64 = 0x7FFC_0000_0000_0000;
pub const TAG_ERROR: u64 = 0x7FFD_0000_0000_0000;
pub const TAG_ATOM:  u64 = 0x7FFE_0000_0000_0000;
pub const TAG_VAR:   u64 = 0x7FFF_0000_0000_0000;

pub const TAG_MASK:     u64 = 0xFFFF_0000_0000_0000;
pub const PAYLOAD_MASK: u64 = 0x0000_FFFF_FFFF_FFFF;
```

### 2.2 Type Check in Cranelift IR

Checking whether a value `v` is a Long:

```
tag      = band(v, 0xFFFF_0000_0000_0000)
expected = iconst(0x7FF8_0000_0000_0000)
is_long  = icmp(eq, tag, expected)
```

Extracting the signed 48-bit integer from a Long:

```
payload = band(v, 0x0000_FFFF_FFFF_FFFF)
shifted = ishl_imm(payload, 16)
signed  = sshr_imm(shifted, 16)        // arithmetic shift for sign extension
```

Boxing an `i64` result as Long:

```
masked = band(result, 0x0000_FFFF_FFFF_FFFF)
boxed  = bor(masked, 0x7FF8_0000_0000_0000)
```

### 2.3 Conversion to/from MettaValue

`JitValue` (defined in `src/backend/bytecode/jit/types/value.rs`) provides:

- `JitValue::try_from_metta(value: &MettaValue) -> Option<Self>` -- converts
  inline types (Long within 48-bit range, Bool, Unit, Empty). Returns `None` for
  values requiring heap allocation (floats, S-expressions, large integers).

- `unsafe JitValue::to_metta(self) -> MettaValue` -- reconstructs a MettaValue.
  For `TAG_PTR`, dereferences the slab pointer to recover
  `MettaValue::from_inner(&*ptr)`.

---

## 3. JitContext Layout

`JitContext` is a `#[repr(C)]` struct passed as a raw pointer to every
JIT-compiled function. It provides the runtime interface between generated
native code and the Rust host.

```
JitContext (#[repr(C)])
┌─────────────────────────────────┐
│ value_stack: *mut JitValue      │  Operand/result stack
│ sp: usize                       │  Stack pointer (next free slot)
│ stack_cap: usize                │  Stack capacity
├─────────────────────────────────┤
│ constants: *const MettaValue    │  Constant pool for PushConstant
│ constants_len: usize            │
├─────────────────────────────────┤
│ bailout: bool                   │  Bailout flag
│ bailout_ip: usize               │  IP to resume in VM
│ bailout_reason: JitBailoutReason│  Error classification
├─────────────────────────────────┤
│ choice_points: *mut JitChoice.. │  Nondeterminism: choice point stack
│ choice_point_count: usize       │
│ choice_point_cap: usize         │
│ results: *mut JitValue          │  Collected results buffer
│ results_count / results_cap     │
├─────────────────────────────────┤
│ bridge_ptr: *const ()           │  MorkBridge for rule dispatch
│ current_chunk: *const ()        │  Active BytecodeChunk
├─────────────────────────────────┤
│ current_rules: *mut ()          │  Vec<CompiledRule> from dispatch
│ current_rule_idx: usize         │
├─────────────────────────────────┤
│ resume_ip: usize                │  Re-entry IP after backtracking
│ in_nondet_mode: bool            │
│ fork_depth: usize               │
│ saved_stack: *mut JitValue      │  Stack snapshot for backtrack
│ saved_stack_count/cap           │
├─────────────────────────────────┤
│ binding_frames: *mut JitBind..  │  Variable binding frames
│ binding_frames_count/cap        │
├─────────────────────────────────┤
│ external_registry: *const ()    │  External function registry
│ memo_cache: *const ()           │  Memoization cache
│ space_registry: *mut ()         │  Named space registry
├─────────────────────────────────┤
│ grounded_spaces: *const *const()│  Pre-resolved [&self, &kb, &stack]
│ grounded_spaces_count: usize    │
│ template_results / cap          │
├─────────────────────────────────┤
│ cut_markers: *mut usize         │  Choice point count at cut entry
│ cut_marker_count / cap          │
├─────────────────────────────────┤
│ env_ptr: *mut ()                │  Environment for state ops
├─────────────────────────────────┤
│ state_cache: [(u64,u64); 8]     │  Direct-mapped state value cache
│ state_cache_valid: u8           │  Validity bitmask
├─────────────────────────────────┤
│ stack_save_pool: *mut JitValue  │  Ring buffer for fork stack saves
│ stack_save_pool_cap / next      │  (POOL_SIZE * MAX_VALUES each)
├─────────────────────────────────┤
│ var_index_cache: [(u64,u32);32] │  Variable name -> constant index
├─────────────────────────────────┤
│ arena_constants: *const ()      │  Arena-mode constant pool
│ arena_constants_len: usize      │
│ arena: *const ()                │  Slab allocator pointer
├─────────────────────────────────┤
│ type_registry_ptr: *const       │  TypeSignatureRegistry for
│   TypeSignatureRegistry         │  type-driven applicative eval
└─────────────────────────────────┘
```

Runtime functions receive `ctx: *mut JitContext` as their first argument and
use its fields to access the stack, constant pool, environment, and binding
frames. The `#[repr(C)]` layout guarantees that field offsets are stable and
predictable from generated code.

---

## 4. Runtime Function Interface

Operations that cannot be fully inlined (pattern matching, space operations,
rule dispatch, etc.) are implemented as `#[no_mangle] pub unsafe extern "C"`
functions in `src/backend/bytecode/jit/runtime/`. These are registered with the
Cranelift `JITBuilder` by name and imported into each compiled function via
`module.declare_func_in_func()`.

### 4.1 Registration Flow

```rust
// 1. Register symbol address with JITBuilder
builder.symbol("jit_runtime_numeric_add",
               runtime::jit_runtime_numeric_add as *const u8);

// 2. Declare import signature in JITModule
let mut sig = module.make_signature();
sig.params.push(AbiParam::new(types::I64));  // operand a
sig.params.push(AbiParam::new(types::I64));  // operand b
sig.returns.push(AbiParam::new(types::I64)); // result
let func_id = module.declare_function(
    "jit_runtime_numeric_add", Linkage::Import, &sig)?;

// 3. Import into current function during IR build
let func_ref = module.declare_func_in_func(func_id, builder.func);

// 4. Emit call instruction
let call = builder.ins().call(func_ref, &[a, b]);
let result = builder.inst_results(call)[0];
```

### 4.2 Handler Contexts

Each opcode category has a dedicated handler context struct that bundles the
`JITModule` reference with pre-declared `FuncId`s:

| Context Struct | Category | Example FuncIds |
|----------------|----------|-----------------|
| `ArithmeticHandlerContext` | `+`, `-`, `*`, `/`, `%`, `neg`, `abs`, `pow` | `numeric_add_func_id`, `pow_func_id` |
| `ComparisonHandlerContext` | `<`, `<=`, `>`, `>=`, `==`, `!=` | `numeric_lt_func_id`, `numeric_eq_func_id` |
| `PatternMatchingHandlerContext` | `Match`, `Unify`, `MatchHead`, `MatchArity` | `pattern_match_func_id`, `unify_func_id` |
| `SpecialFormsHandlerContext` | `if`, `let`, `case`, `quote`, `eval`, etc. | `eval_case_func_id`, `eval_quote_func_id` |
| `SExprHandlerContext` | `GetHead`, `GetTail`, `MakeSExpr`, `ConsAtom` | `get_head_func_id`, `make_sexpr_func_id` |
| `CallHandlerContext` | `Call`, `TailCall`, `CallN`, `CallNative` | `call_func_id`, `call_native_func_id` |
| `NondetHandlerContext` | `Fork`, `Yield`, `Collect`, `Cut`, `Guard` | `fork_native_func_id`, `cut_func_id` |

### 4.3 Grouped Initialization

`JitCompiler` organizes its ~130 `FuncId` declarations into trait-based init
groups (defined in `src/backend/bytecode/jit/compiler/init.rs`):

```rust
pub struct JitCompiler {
    module: JITModule,
    arithmetic:        ArithmeticFuncIds,
    bindings:          BindingFuncIds,
    calls:             CallFuncIds,
    nondet:            NondetFuncIds,
    pattern_matching:  PatternMatchingFuncIds,
    rules:             RulesFuncIds,
    space:             SpaceFuncIds,
    special_forms:     SpecialFormsFuncIds,
    type_ops:          TypeOpsFuncIds,
    sexpr:             SExprFuncIds,
    higher_order:      HigherOrderFuncIds,
    globals:           GlobalsFuncIds,
    debug:             DebugFuncIds,
    errors:            ErrorFuncIds,
    set_ops:           SetOpsFuncIds,
    // ... miscellaneous individual FuncIds
}
```

Each group has a corresponding `register_*_symbols(builder)` method and a
`declare_*_funcs(module)` method, providing zero-cost static dispatch through
traits.

---

## 5. Tiered Compilation Thresholds and State Machine

### 5.1 Thresholds

Defined in `src/backend/bytecode/tiered_cache.rs`:

```rust
pub const BYTECODE_THRESHOLD: u32 = 5;     // Tier 0 -> Tier 1
pub const JIT1_THRESHOLD:     u32 = 200;   // Tier 1 -> Tier 2
pub const JIT2_THRESHOLD:     u32 = 2_000; // Tier 2 -> Tier 3
```

These are modeled after V8's tiering:

| V8 Equivalent | MeTTaTron Tier | Threshold |
|---------------|----------------|-----------|
| Ignition -> Sparkplug | Tier 0 -> 1 | ~1-5 (MeTTaTron: 5) |
| Sparkplug -> Maglev | Tier 1 -> 2 | ~100-400 (MeTTaTron: 200) |
| Maglev -> Turbofan | Tier 2 -> 3 | ~1000-6000 (MeTTaTron: 2000) |

### 5.2 Per-Expression State Machine

Each expression is tracked by `ExprCompilationState`, keyed by structural hash
(xxh3) in a `DashMap<u64, Arc<ExprCompilationState>>`:

```
                      ┌──────────┐
                      │NotStarted│
                      └────┬─────┘
                           │ count >= threshold
                           │ CAS(NotStarted -> Compiling)
                           ▼
                      ┌──────────┐
                      │Compiling │  (background task running)
                      └──┬───┬───┘
                  success│   │failure
                         ▼   ▼
                  ┌──────┐ ┌──────┐
                  │Ready │ │Failed│
                  └──────┘ └──────┘
```

Each tier (bytecode, jit1, jit2) has its own independent `AtomicU8` status
field and `OnceLock` artifact slot. The transitions use `compare_exchange` for
lock-free races; only the CAS winner spawns the background compilation task.

### 5.3 Execution Counter Infrastructure

Execution counts are stored per-slot in slab page metadata
(`ValuePage::exec_counts: [AtomicU32]`) rather than in a global HashMap. The
hot path uses a thread-local page cache (`EXEC_PAGE_CACHE`) for O(1) counter
increments:

```
Thread-Local Cache
┌──────────────────────────────┐
│ base: usize                  │  Page data start
│ end: usize                   │  base + PAGE_SIZE
│ counters_ptr: *const AtomicU32│ Direct pointer to exec_counts[0]
│ hashes_ptr: *const AtomicU64 │  Compilation hash array
│ slot_size: usize             │  For index computation
│ generation: u64              │  Invalidation epoch
└──────────────────────────────┘
```

Hot path cost: pointer arithmetic + `atomic fetch_add` = ~10-15 cycles on cache
hit. No hash computation, no map probe, no lock.

### 5.4 Dispatch Logic

On each sub-expression evaluation, the tiered cache determines which tier to
use:

```
fn best_tier(state: &ExprCompilationState) -> ExecutionTier {
    if state.jit2_status() == Ready { return JitStage2 }
    if state.jit1_status() == Ready { return JitStage1 }
    if state.bytecode_status() == Ready { return Bytecode }
    return Interpreter
}
```

Compilation is **non-blocking**: when a threshold is crossed and the previous
tier is Ready, a background task is spawned via the priority scheduler
(`BACKGROUND_COMPILE` priority). The current tier continues executing until the
next tier becomes Ready.

---

## 6. CodegenContext

`CodegenContext` (in `src/backend/bytecode/jit/codegen.rs`) wraps a Cranelift
`FunctionBuilder` with higher-level operations:

### 6.1 Simulated Value Stack

Values are tracked as Cranelift SSA `Value` handles in a `Vec<Value>`, not in
memory. This enables the register allocator to keep values in registers.

```rust
pub struct CodegenContext<'a, 'b> {
    builder: &'a mut FunctionBuilder<'b>,
    ctx_ptr: Value,                   // JitContext pointer
    value_stack: Vec<Value>,          // SSA value stack
    terminated: bool,                 // Block termination flag
    locals: Vec<Option<Value>>,       // Local variable slots
    error_func_refs: Option<ErrorFuncRefs>, // Bailout handlers
}
```

### 6.2 Error Function References

When available, bailout code calls runtime error handlers instead of emitting
`trap()` (which generates `ud2` and causes SIGILL):

```rust
pub struct ErrorFuncRefs {
    pub type_error: FuncRef,   // fn(ctx, ip, expected) -> ()
    pub div_by_zero: FuncRef,  // fn(ctx, ip) -> ()
    pub overflow: FuncRef,     // fn(ctx, ip) -> ()
}
```

After calling the error handler, the bailout block returns `0` to the caller.
The VM checks `ctx.bailout` to detect the error and resumes at `ctx.bailout_ip`.

### 6.3 Provided Operations

| Method | Purpose |
|--------|---------|
| `push(val)` / `pop()` / `peek()` | Simulated stack manipulation |
| `const_unit()` / `const_bool(b)` / `const_long(n)` | NaN-boxed constant creation |
| `extract_long(v)` / `extract_bool(v)` / `extract_tag(v)` | Unboxing |
| `box_long(v)` / `box_bool(v)` | Boxing |
| `guard_long(v, ip)` / `guard_bool(v, ip)` | Type guards with bailout |
| `guard_nonzero(v, ip)` | Division-by-zero guard |
| `guard_not_i64_min(v, ip)` | Overflow guard for `abs(i64::MIN)` |
| `init_locals(n)` / `load_local(i)` / `store_local(i)` | Local variable support |

---

## 7. ISA Configuration

The Cranelift ISA is configured for maximum performance:

```rust
fn create_validated_isa() -> JitResult<Arc<dyn TargetIsa>> {
    let mut flag_builder = settings::builder();
    flag_builder.set("opt_level", "speed")?;
    let isa_builder = cranelift_native::builder()?; // auto-detect CPU
    let flags = settings::Flags::new(flag_builder);
    isa_builder.finish(flags)
}
```

`cranelift_native::builder()` auto-detects the host CPU's instruction set
extensions (SSE, AVX, BMI, etc.). Under CPU affinity (`taskset`), the
auto-detection may report features unavailable on the pinned cores, causing
SIGILL. The workaround is `METTATRON_DISABLE_JIT=1`.
