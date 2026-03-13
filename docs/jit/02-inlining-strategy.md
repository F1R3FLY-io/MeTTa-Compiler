# Inlining Strategy

The JIT compiler classifies every bytecode opcode into one of three code
generation strategies:

1. **Fully inlined** -- The operation is emitted as pure Cranelift IR with no
   function calls. The register allocator can schedule it freely.

2. **Inline fast-path with FFI fallback** -- A type guard checks whether the
   operands satisfy the fast-path precondition (e.g., both are `TAG_LONG`).
   On success, a short inline sequence executes. On failure, a runtime FFI
   function handles the general case.

3. **Pure FFI** -- The entire operation is a call to a `#[no_mangle] extern "C"`
   runtime function. Used for operations too complex to inline (pattern
   matching, rule dispatch, space operations).

---

## 1. Decision Criteria

| Criterion | Inline | Fast-path + Fallback | Pure FFI |
|-----------|--------|---------------------|----------|
| Operand types known statically? | Yes (e.g., `PushTrue`) | Partially (integer likely) | No |
| Operation involves allocation? | No | Fallback may allocate | Often |
| Operation touches environment? | No | No | Yes |
| Cranelift IR cost (instructions) | 1-5 | 10-30 | 1 call |
| Latency (cycles, approximate) | 1-3 | 5-15 (fast path) | 30-200+ |

The key design principle: **hot arithmetic stays in registers**. Integer
addition of two `TAG_LONG` values completes in ~5 Cranelift IR instructions
(2 tag extractions, 1 `iadd`, 1 rebox, plus the guard branch). The FFI
fallback handles Float operands, mixed Long/Float promotion, and type errors.

---

## 2. Fully Inlined Operations

### 2.1 Boolean Operations (And, Or, Not, Xor)

Boolean operations are always inlined because both operands must be `TAG_BOOL`
(enforced by type guards). No runtime fallback is needed.

**`And` IR pattern:**

```
                         ┌──────────────┐
                         │  entry       │
                         │  a = pop()   │
                         │  b = pop()   │
                         └──────┬───────┘
                                │
                  ┌─────────────┼─────────────┐
                  │ guard_bool  │ guard_bool   │
                  │    (a)      │    (b)       │
                  └──────┬──────┴──────┬───────┘
                         │             │
                         ▼             ▼
                   ┌─────────────────────────┐
                   │  a_val = extract_bool(a) │
                   │  b_val = extract_bool(b) │
                   │  result = band(a_val,    │
                   │                b_val)    │
                   │  boxed = box_bool(result)│
                   │  push(boxed)             │
                   └─────────────────────────┘
```

From `handlers/comparison.rs`:

```rust
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
```

**`Not` IR pattern:**

```rust
let a_val = codegen.extract_bool(a);       // band(a, 1)
let one = codegen.builder.ins().iconst(types::I64, 1);
let result = codegen.builder.ins().bxor(a_val, one);  // flip bit 0
let boxed = codegen.box_bool(result);      // bor(result, TAG_BOOL)
```

Not is `XOR 1` on the extracted boolean bit -- a single instruction flip.

### 2.2 Structural Equality (StructEq)

Structural equality compares the raw 64-bit NaN-boxed representations directly.
If the bits are identical, the values are structurally equal. No unboxing or
runtime call is needed.

```rust
Opcode::StructEq => {
    let b = codegen.pop()?;
    let a = codegen.pop()?;
    let cmp = codegen.builder.ins().icmp(IntCC::Equal, a, b);
    let result = codegen.builder.ins().uextend(types::I64, cmp);
    let boxed = codegen.box_bool(result);
    codegen.push(boxed)?;
}
```

Total: 3 Cranelift instructions (icmp, uextend, bor).

### 2.3 EvalIf (Conditional)

`EvalIf` is inlined using Cranelift's `select` instruction. The truthiness
check follows MeTTa semantics: only `TAG_BOOL_FALSE` and `TAG_UNIT` are falsy.

```rust
pub fn compile_eval_if(codegen: &mut CodegenContext) -> JitResult<()> {
    let else_val = codegen.pop()?;
    let then_val = codegen.pop()?;
    let condition = codegen.pop()?;

    let tag_bool_false = codegen.const_bool(false);
    let tag_unit = codegen.const_unit();

    let is_false = codegen.builder.ins().icmp(IntCC::Equal, condition, tag_bool_false);
    let is_unit  = codegen.builder.ins().icmp(IntCC::Equal, condition, tag_unit);
    let is_falsy = codegen.builder.ins().bor(is_false, is_unit);

    let result = codegen.builder.ins().select(is_falsy, else_val, then_val);
    codegen.push(result)?;
    Ok(())
}
```

This is branch-free: `select` compiles to a conditional move (CMOVcc on x86-64).

### 2.4 EvalChain (Sequencing)

Chain discards the first operand and keeps the second. Zero instructions beyond
stack manipulation:

```rust
pub fn compile_eval_chain(codegen: &mut CodegenContext) -> JitResult<()> {
    let second = codegen.pop()?;
    let _first = codegen.pop()?;  // discard
    codegen.push(second)?;
    Ok(())
}
```

### 2.5 EvalLet / EvalBind

Both call `jit_runtime_store_binding` to install the binding, then inline
the `Unit` return value:

```rust
// Store binding returns status (ignored), we always push Unit
codegen.builder.ins().call(func_ref, &[ctx_ptr, name_idx_val, value, ip_val]);

// Push Unit result (inline, no function call needed)
let unit_val = codegen.const_unit();
codegen.push(unit_val)?;
```

The binding store is an FFI call, but the result construction is inlined.

### 2.6 EvalLetStar

Sequential let-bindings are handled by the bytecode compiler. The opcode is a
no-op marker that pushes Unit:

```rust
pub fn compile_eval_let_star(codegen: &mut CodegenContext) -> JitResult<()> {
    let unit_val = codegen.const_unit();
    codegen.push(unit_val)?;
    Ok(())
}
```

### 2.7 Stack Operations

`Nop`, `Pop`, `Dup`, `Swap`, `Rot3`, `Over`, `DupN`, `PopN` are purely
simulated-stack manipulations. They generate no machine code at all -- the
register allocator sees them as SSA value rearrangements.

### 2.8 Constant Creation

`PushUnit`, `PushTrue`, `PushFalse`, `PushLongSmall` emit a single `iconst`:

```rust
Opcode::PushUnit      => { let v = codegen.const_unit(); codegen.push(v)?; }
Opcode::PushTrue      => { let v = codegen.const_bool(true); codegen.push(v)?; }
Opcode::PushFalse     => { let v = codegen.const_bool(false); codegen.push(v)?; }
Opcode::PushLongSmall => {
    let n = chunk.read_byte(offset + 1).unwrap_or(0) as i8 as i64;
    let v = codegen.const_long(n);
    codegen.push(v)?;
}
```

---

## 3. Inline Fast-Path with FFI Fallback

### 3.1 Binary Arithmetic (Add, Sub, Mul)

The most important optimization: integer arithmetic stays in registers on the
fast path. The general pattern from `handlers/arithmetic.rs`:

```
                    ┌──────────────────────┐
                    │  entry               │
                    │  b = pop()           │
                    │  a = pop()           │
                    │  a_tag = band(a, MASK)│
                    │  b_tag = band(b, MASK)│
                    │  a_long = icmp(eq,   │
                    │    a_tag, TAG_LONG)   │
                    │  b_long = icmp(eq,   │
                    │    b_tag, TAG_LONG)   │
                    │  both = band(a_long, │
                    │              b_long)  │
                    │  brif both ->        │
                    │    int_path,          │
                    │    runtime_path       │
                    └───┬────────────┬──────┘
                        │            │
               ┌────────▼──┐   ┌────▼───────────┐
               │ int_path  │   │ runtime_path   │
               │ a_v=      │   │ rt = call      │
               │  extract  │   │  jit_runtime_  │
               │  _long(a) │   │  numeric_add   │
               │ b_v=      │   │  (a, b)        │
               │  extract  │   │ jmp merge(rt)  │
               │  _long(b) │   └────────────────┘
               │ r = iadd  │
               │  (a_v,b_v)│
               │ box=      │
               │  box_long │
               │  (r)      │
               │ jmp       │
               │  merge    │
               │  (box)    │
               └─────┬─────┘
                     │
                ┌────▼──────────┐
                │ merge_block   │
                │ (result: i64) │  <-- block parameter (SSA phi)
                │ push(result)  │
                └───────────────┘
```

For `Add`, the integer fast-path is `iadd`. For `Sub`, `isub`. For `Mul`,
`imul`. All three share the `emit_binary_arith_with_fallback` helper.

### 3.2 Division and Modulo (Div, Mod, FloorDiv)

Division and modulo add a **zero-check guard** on the integer fast-path:

```rust
// Integer fast-path with zero-check
codegen.builder.switch_to_block(int_path);
let a_val = codegen.extract_long(a);
let b_val = codegen.extract_long(b);
codegen.guard_nonzero(b_val, offset)?;     // bailout if b == 0
let int_result = codegen.builder.ins().sdiv(a_val, b_val);
let boxed = codegen.box_long(int_result);
```

The `guard_nonzero` emits a conditional branch to a bailout block that calls
`jit_runtime_div_by_zero(ctx, ip)`. The runtime fallback
(`jit_runtime_numeric_div`) also checks for zero independently, since it
handles Float/Float and mixed-type division.

### 3.3 Unary Arithmetic (Neg, Abs)

Unary operations use `emit_unary_arith_with_fallback`:

**Neg fast-path:** `ineg(a_val)`

**Abs fast-path:**
```rust
|cg, a| {
    let zero = cg.builder.ins().iconst(types::I64, 0);
    let is_neg = cg.builder.ins().icmp(IntCC::SignedLessThan, a, zero);
    let negated = cg.builder.ins().ineg(a);
    cg.builder.ins().select(is_neg, negated, a)
}
```

Branch-free abs via `select` (CMOVcc).

### 3.4 Ordered Comparisons (Lt, Le, Gt, Ge)

Ordered comparisons share `emit_comparison_with_fallback`:

```
int_path:
    a_val = extract_long(a)
    b_val = extract_long(b)
    cmp   = icmp(<int_cc>, a_val, b_val)    // e.g., SignedLessThan
    ext   = uextend(i64, cmp)                // bool -> i64
    boxed = box_bool(ext)
    jmp merge(boxed)

runtime_path:
    rt = call jit_runtime_numeric_lt(a, b)   // handles Float, mixed
    jmp merge(rt)
```

The `int_cc` is mapped directly:

| Opcode | `IntCC` |
|--------|---------|
| `Lt` | `SignedLessThan` |
| `Le` | `SignedLessThanOrEqual` |
| `Gt` | `SignedGreaterThan` |
| `Ge` | `SignedGreaterThanOrEqual` |

### 3.5 Equality (Eq)

Equality uses a **bit-level identity check** as its fast-path instead of tag
extraction. If two NaN-boxed values have identical 64-bit representations, they
are definitionally equal regardless of type:

```
                    ┌───────────────────┐
                    │ entry             │
                    │ b = pop()         │
                    │ a = pop()         │
                    │ raw_eq = icmp(eq, │
                    │          a, b)    │
                    │ brif raw_eq ->    │
                    │   fast_true,      │
                    │   slow_check      │
                    └──┬──────────┬─────┘
                       │          │
              ┌────────▼──┐  ┌───▼──────────────┐
              │ fast_true │  │ slow_check       │
              │ result =  │  │ rt = call        │
              │  box_bool │  │  jit_runtime_    │
              │  (true)   │  │  numeric_eq(a,b) │
              │ jmp merge │  │ jmp merge(rt)    │
              └───────────┘  └──────────────────┘
                       │          │
                  ┌────▼──────────▼──┐
                  │  merge(result)   │
                  │  push(result)    │
                  └──────────────────┘
```

The slow path calls `jit_runtime_numeric_eq`, which uses `numeric_equal()` for
MeTTa HE-compatible cross-type equality (e.g., `Long(2) == Float(2.0)` is
`True`).

### 3.6 Inequality (Ne)

The negation of equality. If bits are identical, the answer is immediately
`False`. Otherwise, call `jit_runtime_numeric_eq` and negate:

```rust
// Slow check: negate eq result
let eq_bool = codegen.extract_bool(eq_result);
let one = codegen.builder.ins().iconst(types::I64, 1);
let neq_bool = codegen.builder.ins().bxor(eq_bool, one);
let boxed_neq = codegen.box_bool(neq_bool);
```

---

## 4. Pure FFI Operations

Operations that are too complex or environment-dependent for inlining:

### 4.1 Pattern Matching

All pattern matching opcodes delegate to runtime:

```rust
// Match: pattern match without binding
let call_inst = codegen.builder.ins().call(
    func_ref, &[ctx_ptr, pattern, value, ip_val]);
let result = codegen.builder.inst_results(call_inst)[0];
codegen.push(result)?;
```

Opcodes: `Match`, `MatchBind`, `MatchHead`, `MatchArity`, `MatchGuard`,
`Unify`, `UnifyBind`.

### 4.2 Rule Dispatch

`DispatchRules`, `TryRule`, `NextRule`, `CommitRule`, `FailRule`,
`LookupRules`, `ApplySubst`, `DefineRule` -- all pure FFI. These interact with
the MORK trie, rule index, and environment.

### 4.3 Space and State Operations

`SpaceAdd`, `SpaceRemove`, `SpaceGetAtoms`, `SpaceMatch`, `NewState`,
`GetState`, `ChangeState` -- all pure FFI. State operations use the
`state_cache` in `JitContext` for amortized O(1) access.

### 4.4 Special Forms (Complex)

`EvalCase`, `EvalCollapse`, `EvalSuperpose`, `EvalMemo`, `EvalFunction`,
`EvalLambda`, `EvalApply` -- all pure FFI. These require loop constructs,
recursive evaluation, or closure allocation that cannot be expressed in a
single straight-line Cranelift block.

### 4.5 Extended Math

`Sqrt`, `Log`, `Sin`, `Cos`, `Tan`, `Asin`, `Acos`, `Atan`, `IsNan`, `IsInf`,
`Trunc`, `Ceil`, `FloorMath`, `Round` -- all pure FFI. These operate on
`JitValue` via `JitValue::from_raw(val).to_metta()`, perform the operation on
the extracted numeric value, and return a new `JitValue`.

### 4.6 Pow

`Pow` is always an FFI call to `jit_runtime_pow`, which implements binary
exponentiation for integers and handles negative exponents:

```rust
pub fn compile_pow(ctx: &mut ArithmeticHandlerContext, codegen: &mut CodegenContext)
    -> JitResult<()>
{
    let exp = codegen.pop()?;
    let base = codegen.pop()?;
    let func_ref = ctx.module.declare_func_in_func(ctx.pow_func_id, codegen.builder.func);
    let call_inst = codegen.builder.ins().call(func_ref, &[base, exp]);
    let result = codegen.builder.inst_results(call_inst)[0];
    codegen.push(result)?;
    Ok(())
}
```

---

## 5. Type Error Signaling in Runtime Functions

When a runtime function receives operands of incompatible types (e.g., adding a
String to an Atom), it cannot return a meaningful result. The signaling
mechanism uses a **thread-local flag**:

```rust
thread_local! {
    static JIT_TYPE_ERROR_FLAG: Cell<bool> = const { Cell::new(false) };
}

pub fn signal_jit_type_error() {
    JIT_TYPE_ERROR_FLAG.with(|f| f.set(true));
}

pub fn check_and_clear_jit_type_error() -> bool {
    JIT_TYPE_ERROR_FLAG.with(|f| {
        let had_error = f.get();
        f.set(false);
        had_error
    })
}
```

The `HybridExecutor` checks this flag after JIT execution completes. If set,
the result is discarded and the expression is re-evaluated via the tree-walker.

Runtime functions that encounter a type error:
1. Call `signal_jit_type_error()`
2. Return a dummy value (`box_long(0)`)

The dummy value is never observed because the flag causes the result to be
discarded.

---

## 6. Summary Table

| Category | Inlining | Example |
|----------|----------|---------|
| Bool ops | Fully inline | `And`, `Or`, `Not`, `Xor` |
| StructEq | Fully inline | `icmp(eq, a, b)` |
| EvalIf | Fully inline | `select(is_falsy, else, then)` |
| EvalChain | Fully inline | Pop first, keep second |
| EvalLetStar | Fully inline | Push Unit |
| Stack ops | Fully inline (zero-cost) | `Nop`, `Dup`, `Swap`, `Pop` |
| Constants | Fully inline | `PushUnit`, `PushTrue`, `PushLongSmall` |
| Int arithmetic | Fast-path + fallback | `Add`, `Sub`, `Mul` |
| Int div/mod | Fast-path + zero guard + fallback | `Div`, `Mod`, `FloorDiv` |
| Unary arith | Fast-path + fallback | `Neg`, `Abs` |
| Ordered cmp | Fast-path + fallback | `Lt`, `Le`, `Gt`, `Ge` |
| Equality | Bit identity + fallback | `Eq`, `Ne` |
| Let/Bind | FFI + inline Unit | `EvalLet`, `EvalBind` |
| Pattern match | Pure FFI | `Match`, `Unify`, `MatchHead` |
| Rule dispatch | Pure FFI | `DispatchRules`, `TryRule` |
| Space/State | Pure FFI | `SpaceAdd`, `NewState` |
| Complex forms | Pure FFI | `EvalCase`, `EvalCollapse` |
| Extended math | Pure FFI | `Sqrt`, `Sin`, `Log` |
| Pow | Pure FFI | `jit_runtime_pow` |
