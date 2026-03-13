# Bailout Mechanisms

MeTTaTron's JIT cannot compile every MeTTa expression. When the JIT encounters
an expression it cannot handle, it must **bail out** -- transfer control back
to a lower tier (bytecode VM or tree-walker) with enough state to continue
execution correctly. This document describes the three bailout layers:

1. **Static compilability check** -- reject before compilation begins
2. **Runtime bailout** -- abort during JIT execution and fall back to VM
3. **Partial inline with FFI fallback** -- degrade individual operations, not
   the whole function

---

## 1. Static Compilability Check

Before any Cranelift IR is generated, `can_compile_stage1(chunk)` scans the
entire bytecode chunk to determine whether it is JIT-compilable.

### 1.1 Nondeterminism Fast-Path Rejection

```rust
pub fn can_compile_stage1(chunk: &BytecodeChunk) -> bool {
    // Fast path: reject nondeterministic chunks immediately
    if chunk.has_nondeterminism() {
        return false;
    }
    // ... opcode scan follows
}
```

`has_nondeterminism()` is a flag set during bytecode compilation when the chunk
contains `Fork`, `Yield`, or `Collect` opcodes. These require coroutine-style
control flow that cannot be represented as a single Cranelift function.

### 1.2 Opcode-by-Opcode Scan

The scan iterates every opcode in the chunk. Supported opcodes are enumerated
exhaustively in match arms; the catch-all `_ => return false` rejects anything
unknown:

```rust
while offset < code.len() {
    let Some(op) = chunk.read_opcode(offset) else {
        return false;  // unreadable opcode byte
    };
    match op {
        // Stack ops, arithmetic, boolean, comparisons,
        // control flow, locals, type ops, pattern matching,
        // rule dispatch, space ops, special forms,
        // nondeterminism markers, MORK bridge, debug, ...
        Opcode::Nop | Opcode::Pop | ... => {}

        // Anything else is not compilable
        _ => return false,
    }
    offset += 1 + op.immediate_size();
}
true
```

The supported opcode set covers Stages 1-14 and Phases A-J of the JIT
roadmap, comprising the full bytecode instruction set with the exception of
the deprecated `TypeCast` opcode.

### 1.3 Bytecode-Only Variant

`can_compile_stage1_bytecode(code: &[u8])` performs the same scan but on raw
bytecode bytes rather than a `BytecodeChunk`. This is used for generic bytecode
chunks (`GenericBytecodeChunk<MettaValue>`) where the bytecode structure is
type-parameter-independent.

This variant does **not** check the nondeterminism flag -- the caller must
handle that separately.

### 1.4 What Happens on Rejection

When `can_compile_stage1` returns `false`, `JitCompiler::compile()` returns:

```rust
Err(JitError::NotCompilable("Chunk contains non-Stage-1 opcodes"))
```

The `TieredCache` marks the JIT stage as `Failed`:

```rust
state.set_jit1_failed();
```

The expression permanently stays at the bytecode tier. No further JIT
compilation attempts are made for that expression.

---

## 2. Runtime Bailout

Even when a chunk passes static analysis, runtime conditions can make JIT
execution impossible. The `JitContext` provides a bailout protocol:

### 2.1 Bailout Fields in JitContext

```rust
#[repr(C)]
pub struct JitContext {
    // ...
    pub bailout: bool,                    // flag: JIT cannot continue
    pub bailout_ip: usize,               // bytecode offset to resume at
    pub bailout_reason: JitBailoutReason, // error classification
    // ...
}
```

### 2.2 JitBailoutReason

```rust
pub enum JitBailoutReason {
    None,           // no bailout
    TypeError,      // type mismatch (e.g., adding String + Bool)
    DivisionByZero, // integer division by zero
    StackOverflow,  // value stack overflow
    Overflow,       // arithmetic overflow (e.g., abs(i64::MIN))
    Unsupported,    // operation requires features not available in JIT
}
```

### 2.3 Signaling a Bailout from Cranelift IR

Type guards emit a conditional branch to a bailout block:

```
                    ┌───────────────────────┐
                    │ guard_long(val, ip)   │
                    │                       │
                    │ tag = band(val, MASK) │
                    │ exp = iconst(TAG_LONG)│
                    │ ok = icmp(eq,tag,exp) │
                    │ brif ok ->           │
                    │   continue_block,     │
                    │   bailout_block       │
                    └───┬────────────┬──────┘
                        │            │
               ┌────────▼──┐  ┌─────▼────────────────┐
               │ continue  │  │ bailout_block         │
               │ (normal   │  │                       │
               │  flow)    │  │ if error_func_refs:   │
               └───────────┘  │   call type_error(    │
                              │     ctx, ip, expected) │
                              │   return_(0)           │
                              │ else:                  │
                              │   trap(user_1)         │
                              └────────────────────────┘
```

When `ErrorFuncRefs` are available (the normal case), the bailout block:

1. Calls the appropriate runtime error handler:
   - `jit_runtime_type_error(ctx, ip, expected)` for type errors
   - `jit_runtime_div_by_zero(ctx, ip)` for division by zero
   - `jit_runtime_stack_overflow(ctx, ip)` for overflow
2. Returns `0` from the JIT function

The runtime error handler sets `ctx.bailout = true` and records the IP:

```rust
pub fn signal_bailout(&mut self, ip: usize) {
    self.bailout = true;
    self.bailout_ip = ip;
}

pub fn signal_error(&mut self, ip: usize, reason: JitBailoutReason) {
    self.bailout = true;
    self.bailout_ip = ip;
    self.bailout_reason = reason;
}
```

### 2.4 Bailout Without ErrorFuncRefs (Legacy)

If `ErrorFuncRefs` are not available (backward compatibility), the bailout
block emits a `trap()` instruction:

```rust
self.builder.ins().trap(TrapCode::unwrap_user(1));  // type error
self.builder.ins().trap(TrapCode::unwrap_user(2));  // div by zero
self.builder.ins().trap(TrapCode::unwrap_user(3));  // overflow
```

`trap()` compiles to x86-64 `ud2`, which raises SIGILL. This is caught by
the process signal handler. The `ErrorFuncRefs` path avoids this entirely.

### 2.5 State Transfer After Bailout

After JIT execution returns, the `HybridExecutor` checks:

```rust
if ctx.has_bailout() {
    let ip = ctx.bailout_ip;
    let reason = ctx.bailout_reason;
    ctx.clear_bailout();
    // Resume execution in bytecode VM at `ip`
    // Or fall back to tree-walker for full generality
}
```

The state transfer includes:
- **Stack contents**: The `JitContext::value_stack` contains all values pushed
  before the bailout. These are converted from `JitValue` back to `MettaValue`
  via `to_metta()`.
- **Instruction pointer**: `bailout_ip` tells the VM exactly where to resume.
- **Binding frames**: If binding operations were executed before the bailout,
  they persist in the `JitContext` binding frame buffer.

---

## 3. Partial Inline with FFI Fallback

This is the most common form of "bailout" -- it happens at the granularity of
a single operation, not the whole function. The JIT-compiled function continues
executing after the FFI call returns.

### 3.1 The Integer Fast-Path Pattern

Binary arithmetic operations (Add, Sub, Mul, etc.) emit:

```
entry:
    both_long = (tag(a) == TAG_LONG) AND (tag(b) == TAG_LONG)
    brif both_long -> int_path, runtime_path

int_path:
    result = iadd(extract_long(a), extract_long(b))
    boxed = box_long(result)
    jmp merge(boxed)

runtime_path:                          // <-- "partial bailout"
    rt_result = call jit_runtime_numeric_add(a, b)
    jmp merge(rt_result)

merge(result):
    push(result)
```

The `runtime_path` is not a bailout in the traditional sense -- it does not
abort the JIT function. It is a **local degradation** to FFI for one operation.
The merge block resumes JIT execution with the result.

### 3.2 What the Runtime Fallback Handles

The runtime functions handle all type combinations that the inline fast-path
does not:

| Fast-path covers | Runtime fallback covers |
|------------------|------------------------|
| Long + Long | Float + Float |
| | Long + Float |
| | Float + Long |
| | Type error (signals flag) |

From `runtime/arithmetic.rs`:

```rust
pub unsafe extern "C" fn jit_runtime_numeric_add(a: u64, b: u64) -> u64 {
    let a_mv = JitValue::from_raw(a).to_metta();
    let b_mv = JitValue::from_raw(b).to_metta();

    match (a_mv.view(), b_mv.view()) {
        (ValueView::Long(x), ValueView::Long(y)) => box_long(x.wrapping_add(y)),
        (ValueView::Float(x), ValueView::Float(y)) =>
            metta_to_jit(&MettaValue::Float(x + y)).to_bits(),
        (ValueView::Long(x), ValueView::Float(y)) =>
            metta_to_jit(&MettaValue::Float(x as f64 + y)).to_bits(),
        (ValueView::Float(x), ValueView::Long(y)) =>
            metta_to_jit(&MettaValue::Float(x + y as f64)).to_bits(),
        _ => {
            signal_jit_type_error();
            box_long(0)  // dummy; discarded after flag check
        }
    }
}
```

### 3.3 Division Zero-Check: Guard + Fallback

Division combines **both** mechanisms. The integer fast-path has an explicit
`guard_nonzero` that bails out to a div-by-zero error handler. The runtime
fallback has its own independent zero-check:

```
int_path:
    a_val = extract_long(a)
    b_val = extract_long(b)
    guard_nonzero(b_val, offset)     // -> bailout block if b == 0
    result = sdiv(a_val, b_val)
    ...

runtime_path:
    rt_result = call jit_runtime_numeric_div(a, b)  // handles Float and zero
    ...
```

In `jit_runtime_numeric_div`:

```rust
(ValueView::Long(x), ValueView::Long(y)) => {
    if y == 0 {
        return make_jit_error("Division by zero");
    }
    box_long(x.wrapping_div(y))
}
```

### 3.4 Equality: Bit Identity + FFI

Equality uses a different fast-path strategy. Instead of checking types, it
compares the raw 64-bit NaN-boxed representations:

- **Identical bits** -> definitely equal (fast true)
- **Different bits** -> might still be equal cross-type (FFI check)

This is sound because NaN-boxing is injective for inline types: two different
`TAG_LONG` values always have different bits.

Cross-type equality (e.g., `Long(2) == Float(2.0)`) requires the runtime
`numeric_equal()` function, which uses epsilon-based floating-point comparison.

---

## 4. Thread-Local Type Error Flag

For FFI functions that cannot propagate errors through their return value
(because the return type is a NaN-boxed value, not a Result), a thread-local
flag mechanism is used:

```
┌─────────────────────────────────────────────────┐
│  JIT Function Execution                         │
│                                                 │
│  1. call jit_runtime_numeric_add(a, b)          │
│     -> type mismatch detected                   │
│     -> signal_jit_type_error()                  │
│     -> returns box_long(0) (dummy)              │
│                                                 │
│  2. JIT continues with dummy value              │
│     (subsequent operations may produce garbage) │
│                                                 │
│  3. JIT function returns normally               │
└─────────────────────────┬───────────────────────┘
                          │
                          ▼
┌─────────────────────────────────────────────────┐
│  HybridExecutor Post-Check                      │
│                                                 │
│  if check_and_clear_jit_type_error() {          │
│      // Discard JIT result                      │
│      // Re-evaluate via tree-walker             │
│  }                                              │
└─────────────────────────────────────────────────┘
```

The flag is thread-local (`Cell<bool>`), so it has no synchronization overhead:

```rust
thread_local! {
    static JIT_TYPE_ERROR_FLAG: Cell<bool> = const { Cell::new(false) };
}
```

**Why not bail out immediately?** A type error in a runtime FFI call happens
deep in the call stack (Rust runtime function -> JIT-generated code ->
HybridExecutor). The JIT-generated code has no mechanism for early return from
the middle of a basic block. The flag defers the error to the natural function
return, where the HybridExecutor can handle it cleanly.

---

## 5. Compilation Failure Handling

When `JitCompiler::compile()` fails (either from `can_compile_stage1` returning
false or a Cranelift error), the tiered cache records the failure permanently:

```rust
// In TieredCache background compilation task:
match compiler.compile(&chunk) {
    Ok(code_ptr) => {
        state.set_jit1_ready(Arc::new(NativeCode {
            ptr: code_ptr,
            code_size: /* ... */,
        }));
    }
    Err(e) => {
        if is_jit_debug() {
            eprintln!("JIT1 compilation failed: {}", e);
        }
        state.set_jit1_failed();
    }
}
```

The `set_jit1_failed()` stores `TierStatusKind::Failed` atomically:

```rust
pub fn set_jit1_failed(&self) {
    self.jit1_status.store(TierStatusKind::Failed as u8, Ordering::Release);
}
```

Once `Failed`, no further compilation attempts are made for that tier. The
expression runs at the bytecode tier indefinitely. This avoids repeated
compilation failures for expressions that are fundamentally incompatible with
JIT (e.g., those using `TypeCast`).

### 5.1 Revert on Backpressure

If a compilation task is dropped before completion (e.g., due to WorkPool
backpressure), the status is reverted from `Compiling` back to `NotStarted`:

```rust
pub fn revert_jit1_to_not_started(&self) {
    self.jit1_status.compare_exchange(
        TierStatusKind::Compiling as u8,
        TierStatusKind::NotStarted as u8,
        Ordering::AcqRel,
        Ordering::Relaxed,
    ).ok();
}
```

This allows a future execution to re-trigger compilation when system load
decreases.

---

## 6. Error Types

The `JitError` enum (in `src/backend/bytecode/jit/types/error.rs`) covers all
failure modes:

| Variant | Trigger | Recovery |
|---------|---------|----------|
| `NotCompilable(String)` | Static analysis rejects chunk | Permanent fallback to bytecode |
| `CompilationError(String)` | Cranelift codegen failure | Permanent fallback to bytecode |
| `TypeError { expected, got }` | Runtime type guard failure | Bailout to VM at `bailout_ip` |
| `StackOverflow` | Value stack full | Bailout to VM |
| `StackUnderflow` | Value stack empty | Bailout to VM |
| `DivisionByZero` | Integer divide/modulo by zero | Bailout block calls error handler |
| `InvalidOpcode(u8)` | Unknown bytecode byte | Permanent fallback to bytecode |
| `Bailout { ip, reason }` | Generic bailout | Resume VM at `ip` |
| `InvalidLocalIndex(usize)` | Local variable out of bounds | Bailout to VM |
| `InvalidBinding(String)` | Variable not found | Bailout to VM |
| `BindingFrameOverflow` | Too many nested binding frames | Bailout to VM |

---

## 7. Summary: Bailout Flow Diagram

```
Expression execution begins
        │
        ▼
┌───────────────────────┐
│ TieredCache.dispatch()│
│ Best tier = ?         │
└────┬──────┬──────┬────┘
     │      │      │
     ▼      ▼      ▼
  Tier 0  Tier 1  Tier 2/3
  (tree)  (VM)    (JIT)
                    │
         ┌──────────┼──────────┐
         │          │          │
    ┌────▼─────┐    │    ┌─────▼──────────┐
    │ Static   │    │    │ Inline         │
    │ reject   │    │    │ fast-path      │
    │ (before  │    │    │ succeeds       │
    │ compile) │    │    │ (no bailout)   │
    └──┬───────┘    │    └────────────────┘
       │            │
       ▼            ▼
  set_failed()    ┌──────────────────┐
  forever at      │ Runtime bailout  │
  Tier 1          │ (type guard or   │
                  │  FFI type error) │
                  └────┬─────────────┘
                       │
              ┌────────┼────────┐
              │                 │
    ┌─────────▼──┐    ┌────────▼─────────┐
    │ ctx.bailout│    │ Thread-local      │
    │ = true     │    │ type error flag   │
    │ Resume at  │    │ Re-evaluate via   │
    │ bailout_ip │    │ tree-walker       │
    └────────────┘    └──────────────────┘
```
