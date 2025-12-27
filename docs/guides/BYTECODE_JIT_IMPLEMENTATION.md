# Guide: Adding Bytecode/JIT Support for New MeTTa Features

This guide describes the step-by-step process for adding bytecode VM and JIT compiler support for new MeTTa operations.

---

## Overview

MeTTaTron has three execution tiers:
1. **Tree-Walker** - Direct AST interpretation (slowest, most flexible)
2. **Bytecode VM** - Stack-based virtual machine (faster, compiled)
3. **JIT Compiler** - Native code via Cranelift (fastest, compiled)

When adding a new MeTTa feature, you typically implement the tree-walker first, then add bytecode/JIT support for performance.

---

## Files to Modify

| File | Purpose |
|------|---------|
| `src/backend/bytecode/opcodes.rs` | Define new opcode(s) |
| `src/backend/bytecode/compiler.rs` | Compile MeTTa AST to bytecode |
| `src/backend/bytecode/vm.rs` | Execute opcodes in the VM |
| `src/backend/bytecode/jit/compiler.rs` | JIT compilation support |
| `src/backend/bytecode/jit/runtime.rs` | JIT runtime functions |

---

## Step 1: Define the Opcode

**File:** `src/backend/bytecode/opcodes.rs`

### 1.1 Add to the Opcode enum

Find an available opcode value (check existing values to avoid collisions):

```rust
#[repr(u8)]
pub enum Opcode {
    // ... existing opcodes ...

    /// my-operation: [a, b] -> [result]
    /// Stack effect: pops 2 values, pushes 1
    MyOperation = 0xNN,  // Use next available value
}
```

**Stack Effect Documentation:** Always document the stack effect using the notation `[inputs] -> [outputs]`.

### 1.2 Update `immediate_size()`

If your opcode has immediate operands (inline data), add a match arm:

```rust
pub fn immediate_size(&self) -> usize {
    match self {
        // Opcodes with no immediate data
        Self::MyOperation => 0,

        // Opcodes with immediate data (e.g., constant index)
        Self::MyOperationWithArg => 2,  // u16 argument
        // ...
    }
}
```

### 1.3 Update `mnemonic()`

Add a human-readable name for disassembly:

```rust
pub fn mnemonic(&self) -> &'static str {
    match self {
        Self::MyOperation => "MY_OP",
        // ...
    }
}
```

### 1.4 Update `OPCODE_TABLE`

Add the opcode to the static lookup table:

```rust
static OPCODE_TABLE: [Option<Opcode>; 256] = {
    let mut table = [None; 256];
    // ...
    table[0xNN] = Some(Opcode::MyOperation);
    // ...
    table
};
```

---

## Step 2: Bytecode Compiler

**File:** `src/backend/bytecode/compiler.rs`

Add a case to `try_compile_builtin()` to recognize and compile your operation:

```rust
fn try_compile_builtin(&mut self, head: &str, args: &[MettaValue]) -> CompileResult<Option<()>> {
    match head {
        // ... existing cases ...

        "my-operation" => {
            // 1. Check arity
            self.check_arity("my-operation", args.len(), 2)?;

            // 2. Compile arguments (left-to-right, first arg pushed first)
            self.compile(&args[0])?;  // First argument
            self.compile(&args[1])?;  // Second argument

            // 3. Emit the opcode
            self.builder.emit(Opcode::MyOperation);

            Ok(Some(()))
        }

        // ... more cases ...
        _ => Ok(None),  // Not a builtin, fall through
    }
}
```

**Key Points:**
- Use `check_arity()` to validate argument count
- Compile arguments in order (stack is LIFO, so first arg is deepest)
- For opcodes with immediates, use `emit_with_u16()` or similar

---

## Step 3: VM Execution

**File:** `src/backend/bytecode/vm.rs`

### 3.1 Add dispatch in `run()`

Add a match arm in the main dispatch loop:

```rust
fn run(&mut self) -> VmResult<()> {
    loop {
        let opcode = self.fetch_opcode()?;
        match opcode {
            // ... existing cases ...

            Opcode::MyOperation => self.op_my_operation()?,

            // ...
        }
    }
}
```

### 3.2 Implement the handler method

```rust
fn op_my_operation(&mut self) -> VmResult<()> {
    // 1. Pop arguments (reverse order - last arg first)
    let b = self.pop()?;
    let a = self.pop()?;

    // 2. Type check and compute result
    let result = match (&a, &b) {
        (MettaValue::Long(x), MettaValue::Long(y)) => {
            MettaValue::Long(x + y)  // Example operation
        }
        (MettaValue::Float(x), MettaValue::Float(y)) => {
            MettaValue::Float(x + y)
        }
        _ => {
            return Err(VmError::TypeError {
                expected: "Long or Float",
                got: "other",
            });
        }
    };

    // 3. Push result
    self.push(result);
    Ok(())
}
```

### 3.3 Add VmError variants if needed

If your operation can fail in new ways, add error variants:

```rust
pub enum VmError {
    // ... existing variants ...

    /// Custom error for my operation
    MyOperationError { reason: String },
}
```

Don't forget to update the `Display` impl for the new variant.

---

## Step 4: JIT Compiler Support

**File:** `src/backend/bytecode/jit/compiler.rs`

### 4.1 Add function ID field to `JitCompiler` struct

```rust
pub struct JitCompiler {
    // ... existing fields ...

    /// Imported function ID for jit_runtime_my_operation
    #[cfg(feature = "jit")]
    my_operation_func_id: FuncId,
}
```

### 4.2 Declare the function in `new()`

```rust
pub fn new() -> JitResult<Self> {
    // ... existing declarations ...

    // jit_runtime_my_operation: fn(a: u64, b: u64) -> u64
    let mut my_op_sig = module.make_signature();
    my_op_sig.params.push(AbiParam::new(types::I64));  // a (NaN-boxed)
    my_op_sig.params.push(AbiParam::new(types::I64));  // b (NaN-boxed)
    my_op_sig.returns.push(AbiParam::new(types::I64)); // result (NaN-boxed)
    let my_operation_func_id = module
        .declare_function("jit_runtime_my_operation", Linkage::Import, &my_op_sig)
        .map_err(|e| {
            JitError::CompilationError(format!("Failed to declare jit_runtime_my_operation: {}", e))
        })?;

    Ok(JitCompiler {
        // ... existing fields ...
        my_operation_func_id,
    })
}
```

**Function Signature Patterns:**
- Simple math ops: `fn(value: u64) -> u64`
- Binary ops: `fn(a: u64, b: u64) -> u64`
- Context-aware ops: `fn(ctx: *mut JitContext, value: u64, ip: u64) -> u64`

### 4.3 Register the symbol

In `register_runtime_symbols()`:

```rust
fn register_runtime_symbols(builder: &mut JITBuilder) {
    // ... existing registrations ...

    builder.symbol(
        "jit_runtime_my_operation",
        super::runtime::jit_runtime_my_operation as *const u8,
    );
}
```

### 4.4 Mark opcode as compilable

In `can_compile_stage1()`, add your opcode to the match:

```rust
fn can_compile_stage1(chunk: &BytecodeChunk) -> bool {
    // ...
    match opcode {
        // ... existing opcodes ...

        Opcode::MyOperation => {}  // Empty arm = compilable

        // ...
        _ => return false,  // Unknown = not compilable
    }
}
```

### 4.5 Add translation in `translate_opcode()`

```rust
fn translate_opcode(&mut self, ...) -> JitResult<()> {
    match opcode {
        // ... existing cases ...

        Opcode::MyOperation => {
            // Pop arguments from JIT stack (reverse order)
            let b = codegen.pop()?;
            let a = codegen.pop()?;

            // Get function reference
            let func_ref = self
                .module
                .declare_func_in_func(self.my_operation_func_id, codegen.builder.func);

            // Call runtime function
            let call_inst = codegen.builder.ins().call(func_ref, &[a, b]);
            let result = codegen.builder.inst_results(call_inst)[0];

            // Push result
            codegen.push(result)?;
        }

        // ...
    }
}
```

**For context-aware operations** (that need access to JitContext):

```rust
Opcode::MyContextAwareOp => {
    let value = codegen.pop()?;

    let func_ref = self
        .module
        .declare_func_in_func(self.my_op_func_id, codegen.builder.func);

    let ctx_ptr = codegen.ctx_ptr();
    let ip_val = codegen.builder.ins().iconst(types::I64, offset as i64);

    let call_inst = codegen.builder.ins().call(func_ref, &[ctx_ptr, value, ip_val]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
}
```

---

## Step 5: JIT Runtime Functions

**File:** `src/backend/bytecode/jit/runtime.rs`

### 5.1 Simple operations (no context needed)

```rust
/// My operation: computes something
///
/// # Safety
/// The inputs must be valid NaN-boxed values.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_my_operation(a: u64, b: u64) -> u64 {
    // Convert from NaN-boxed to MettaValue
    let a_jv = JitValue::from_raw(a);
    let b_jv = JitValue::from_raw(b);
    let a_mv = a_jv.to_metta();
    let b_mv = b_jv.to_metta();

    // Compute result
    let result = match (&a_mv, &b_mv) {
        (MettaValue::Long(x), MettaValue::Long(y)) => {
            MettaValue::Long(x + y)
        }
        (MettaValue::Float(x), MettaValue::Float(y)) => {
            MettaValue::Float(x + y)
        }
        _ => MettaValue::Nil,  // Type error fallback
    };

    // Convert back to NaN-boxed
    metta_to_jit(&result).to_bits()
}
```

### 5.2 Context-aware operations (can set errors/bailout)

```rust
/// My operation that can fail
///
/// # Safety
/// The context pointer and inputs must be valid.
#[no_mangle]
pub unsafe extern "C" fn jit_runtime_my_fallible_op(
    ctx: *mut JitContext,
    value: u64,
    ip: u64,
) -> u64 {
    let jv = JitValue::from_raw(value);
    let mv = jv.to_metta();

    match mv {
        MettaValue::SExpr(items) => {
            // Success case
            let result = process_items(&items);
            metta_to_jit(&result).to_bits()
        }
        _ => {
            // Error case - set bailout
            if let Some(ctx_ref) = ctx.as_mut() {
                ctx_ref.bailout = true;
                ctx_ref.bailout_ip = ip as usize;
                ctx_ref.bailout_reason = JitBailoutReason::TypeError;
            }
            JitValue::nil().to_bits()
        }
    }
}
```

---

## Step 6: Update Bytecode Specification

**File:** `docs/optimization/jit/BYTECODE_AND_JIT_IR_SPEC.md`

When adding new opcodes, update the bytecode specification to maintain documentation parity:

### 6.1 Add to Opcode Reference Table

Find the appropriate section (3.1-3.15) and add an entry:

| Hex | Mnemonic | Imm | Stack | Description |
|-----|----------|-----|-------|-------------|
| 0xNN | my_op | 0 | [a, b] → [result] | Brief description |

### 6.2 Section Categories

| Section | Category |
|---------|----------|
| 3.1 | Stack Operations |
| 3.2 | Value Creation |
| 3.3 | Variable Operations |
| 3.4 | Environment Operations |
| 3.5 | Control Flow |
| 3.6 | Pattern Matching |
| 3.7 | Rule Dispatch |
| 3.8 | Special Forms |
| 3.9 | Grounded Arithmetic |
| 3.10 | Grounded Comparison |
| 3.10.1 | Trigonometric Operations |
| 3.10.2 | Float Classification |
| 3.11 | Grounded Boolean |
| 3.12 | Type Operations |
| 3.13 | Nondeterminism |
| 3.14 | MORK Bridge |
| 3.15 | Debug/Meta |

### 6.3 Documentation Format

- **Hex**: Opcode value in hexadecimal (0x00-0xFF)
- **Mnemonic**: Human-readable name (matches `Opcode::mnemonic()`)
- **Imm**: Immediate byte count (0, 1, 2, or 4)
- **Stack**: Stack effect notation `[inputs] → [outputs]`
- **Description**: Brief explanation of operation

---

## Step 7: Testing

### 7.1 VM Unit Tests

Add tests in the appropriate test module:

```rust
#[test]
fn test_vm_my_operation() {
    let mut vm = create_test_vm();

    // Push arguments
    vm.push(MettaValue::Long(2));
    vm.push(MettaValue::Long(3));

    // Execute opcode
    vm.execute_opcode(Opcode::MyOperation).unwrap();

    // Check result
    let result = vm.pop().unwrap();
    assert_eq!(result, MettaValue::Long(5));
}
```

### 7.2 Integration Tests

Test the full pipeline from MeTTa source:

```rust
#[test]
fn test_my_operation_integration() {
    let result = eval("!(my-operation 2 3)");
    assert_eq!(result, vec![MettaValue::Long(5)]);
}
```

### 7.3 JIT Equivalence Tests

Ensure JIT produces same results as VM:

```rust
#[test]
fn test_my_operation_jit_equivalence() {
    let source = "(my-operation 2 3)";

    let vm_result = eval_bytecode(source);
    let jit_result = eval_jit(source);

    assert_eq!(vm_result, jit_result);
}
```

---

## NaN-Boxing Reference

The JIT uses NaN-boxing to represent values in 64 bits:

| Type | Tag | Payload |
|------|-----|---------|
| Long | `TAG_LONG` (0x7FF8) | 48-bit signed integer |
| Bool | `TAG_BOOL` (0x7FF9) | 0 = false, 1 = true |
| Nil | `TAG_NIL` (0x7FFA) | ignored |
| Unit | `TAG_UNIT` (0x7FFB) | ignored |
| Heap | `TAG_HEAP` (0x7FFC) | 48-bit pointer to MettaValue |
| Error | `TAG_ERROR` (0x7FFD) | pointer to error |
| Atom | `TAG_ATOM` (0x7FFE) | pointer to string |
| Var | `TAG_VAR` (0x7FFF) | pointer to variable |

**Key Functions:**
- `JitValue::from_raw(bits)` - Create JitValue from raw u64
- `jv.to_metta()` - Convert to MettaValue
- `metta_to_jit(&mv)` - Convert MettaValue to JitValue
- `jv.to_bits()` - Get raw u64 for return

---

## Checklist

- [ ] **opcodes.rs**: Add opcode to enum with doc comment
- [ ] **opcodes.rs**: Update `immediate_size()` if needed
- [ ] **opcodes.rs**: Update `mnemonic()`
- [ ] **opcodes.rs**: Update `OPCODE_TABLE`
- [ ] **compiler.rs**: Add case in `try_compile_builtin()`
- [ ] **vm.rs**: Add dispatch case in `run()`
- [ ] **vm.rs**: Implement handler method
- [ ] **vm.rs**: Add error variants if needed
- [ ] **jit/compiler.rs**: Add function ID field to struct
- [ ] **jit/compiler.rs**: Declare function signature in `new()`
- [ ] **jit/compiler.rs**: Add to struct initialization in `new()`
- [ ] **jit/compiler.rs**: Register symbol in `register_runtime_symbols()`
- [ ] **jit/compiler.rs**: Add opcode to `can_compile_stage1()`
- [ ] **jit/compiler.rs**: Add translation in `translate_opcode()`
- [ ] **jit/runtime.rs**: Implement runtime function
- [ ] **docs/BYTECODE_AND_JIT_IR_SPEC.md**: Add opcode to reference table
- [ ] **tests**: Add VM unit tests
- [ ] **tests**: Add integration tests
- [ ] **tests**: Add JIT equivalence tests
- [ ] Run `cargo build` - verify no errors
- [ ] Run `cargo test` - verify all tests pass

---

## Example: Complete Implementation

See the implementation of `sqrt-math` as a reference:

1. **Opcode**: `Sqrt = 0xC9` in `opcodes.rs`
2. **Compiler**: `"sqrt-math"` case in `compiler.rs`
3. **VM**: `op_sqrt()` method in `vm.rs`
4. **JIT Compiler**: `sqrt_func_id` and translation in `jit/compiler.rs`
5. **JIT Runtime**: `jit_runtime_sqrt()` in `jit/runtime.rs`

---

## Cross-Project Portability

This bytecode/JIT infrastructure is designed to be portable to other languages. Here's how to adapt it.

### Portable Components

The following can be reused with minimal changes:

| Component | What it does | Portability |
|-----------|--------------|-------------|
| NaN-boxing | Value representation | Redefine `TAG_*` constants |
| `JitProfile` | Hotness tracking | Works as-is |
| `JitCache` | Code caching with LRU | Works as-is |
| `HybridExecutor` | Tier dispatch | Adapt value conversions |
| Cranelift setup | ISA detection, module creation | Works as-is |

### Adaptation Steps for New Language

1. **Define your value tags**

   ```rust
   // Example: Rholang process calculus
   pub const TAG_PROCESS: u64 = QNAN | (0 << 48);
   pub const TAG_CHANNEL: u64 = QNAN | (1 << 48);
   pub const TAG_NAME: u64 = QNAN | (2 << 48);
   pub const TAG_PAR: u64 = QNAN | (3 << 48);
   pub const TAG_SEND: u64 = QNAN | (4 << 48);
   pub const TAG_RECEIVE: u64 = QNAN | (5 << 48);
   pub const TAG_HEAP: u64 = QNAN | (6 << 48);
   pub const TAG_NIL: u64 = QNAN | (7 << 48);
   ```

2. **Define your opcode set**

   Design opcodes for your language's primitives. Consider:
   - Stack operations (push, pop, dup, swap)
   - Language-specific operations
   - Control flow
   - Runtime calls for complex operations

3. **Implement JitContext fields**

   Add language-specific context fields:
   ```rust
   #[repr(C)]
   pub struct RhoContext {
       // Standard fields
       pub value_stack: *mut JitValue,
       pub sp: usize,

       // Rholang-specific
       pub channel_registry: *mut (),
       pub pending_comms: *mut (),
       pub reduction_ctx: *mut (),
   }
   ```

4. **Implement runtime helpers**

   Create `extern "C"` functions for operations that can't be inlined:
   ```rust
   #[no_mangle]
   pub extern "C" fn jit_runtime_rho_send(
       ctx: *mut RhoContext,
       channel: u64,
       data: u64,
   ) -> i64 {
       // Language-specific implementation
   }
   ```

### Shared Scheduler Integration

If your project uses Rayon, the P2 priority scheduler integrates seamlessly:

```rust
// In your build configuration
#[cfg(feature = "hybrid-p2-priority-scheduler")]
{
    // Use P2 scheduler for smart ordering
    use mettatron::priority_scheduler::{
        global_priority_eval_pool, priority_levels, TaskTypeId,
    };

    global_priority_eval_pool().spawn_with_priority(
        compile_task,
        priority_levels::BACKGROUND_COMPILE,
        TaskTypeId::JitCompile,
    );
}

#[cfg(not(feature = "hybrid-p2-priority-scheduler"))]
{
    // Fall back to Rayon
    rayon::spawn(compile_task);
}
```

### Further Reading

- `docs/architecture/TIERED_COMPILER_IMPLEMENTATION_GUIDE.md` - Comprehensive porting guide
- `docs/architecture/HYBRID_P2_PRIORITY_SCHEDULER.md` - Background compilation scheduling
- `docs/optimization/jit/JIT_PIPELINE_ARCHITECTURE.md` - Architecture overview with portability section
