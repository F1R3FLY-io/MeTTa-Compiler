# Understanding MeTTaTron's JIT Compilation Pipeline

## A Pedagogical Guide to Tiered Execution

---

## Table of Contents

1. [Introduction: What is JIT Compilation?](#1-introduction-what-is-jit-compilation)
2. [The Problem: Why Not Just Interpret Everything?](#2-the-problem-why-not-just-interpret-everything)
3. [The Solution: Tiered Execution](#3-the-solution-tiered-execution)
4. [Tier 0: The Tree-Walking Interpreter](#4-tier-0-the-tree-walking-interpreter)
5. [Tier 1: The Bytecode Virtual Machine](#5-tier-1-the-bytecode-virtual-machine)
6. [Tier 2 & 3: JIT Compilation with Cranelift](#6-tier-2--3-jit-compilation-with-cranelift)
7. [The Journey of an Expression: From Source to Native Code](#7-the-journey-of-an-expression-from-source-to-native-code)
8. [Hotness Detection: How the System Knows What to Compile](#8-hotness-detection-how-the-system-knows-what-to-compile)
9. [JIT Code Storage and Caching](#9-jit-code-storage-and-caching)
10. [Subsequent Executions: The Fast Path](#10-subsequent-executions-the-fast-path)
11. [Bailout: When JIT Code Needs Help](#11-bailout-when-jit-code-needs-help)
12. [Key Data Structures](#12-key-data-structures)
13. [Putting It All Together: Complete Execution Flow](#13-putting-it-all-together-complete-execution-flow)
14. [File Reference](#14-file-reference)

---

## 1. Introduction: What is JIT Compilation?

**Just-In-Time (JIT) compilation** is a technique where code is compiled to native machine instructions *during* program execution, rather than ahead of time. This allows the runtime to make optimization decisions based on actual program behavior.

Think of it like a translator at a conference:

- **Ahead-of-Time (AOT) Compilation**: Translate the entire speech before the conference starts
- **Interpretation**: Translate each sentence as the speaker says it, word by word
- **JIT Compilation**: Start translating sentence-by-sentence, but if you notice the speaker repeats certain phrases often, write them down so you can say them instantly next time

MeTTaTron uses JIT compilation to automatically accelerate frequently-executed code paths, achieving speedups of **700-1500x** over interpretation for hot code.

---

## 2. The Problem: Why Not Just Interpret Everything?

Consider this simple MeTTa expression:

```metta
(+ (+ (+ (+ 1 2) 3) 4) 5)
```

With a **tree-walking interpreter**, each evaluation requires:

1. Examine the root node: Is it an S-expression? Yes
2. Get the operator: `+`
3. Recursively evaluate the first argument...
   - Is it an S-expression? Yes
   - Get the operator: `+`
   - Recursively evaluate...
     - (And so on, all the way down)
4. Finally perform the addition
5. Return back up through all the recursive calls

This involves:
- Many function calls (each with stack frame overhead)
- Type checking at every step
- Pattern matching to identify operations
- Dynamic dispatch based on value types

For code that runs **once**, this overhead is acceptable. But for code in a loop that runs **millions of times**, this overhead dominates execution time.

**Benchmark evidence:**

| Depth | Tree-Walker | Bytecode VM | JIT Native |
|-------|-------------|-------------|------------|
| 10 | 2.92 µs | 231 ns | 40.7 ns |
| 100 | 27.8 µs | 1.05 µs | 41.3 ns |
| 200 | 53.8 µs | 1.67 µs | 42.9 ns |

At depth 200, the tree-walker is **1,254x slower** than native JIT code.

---

## 3. The Solution: Tiered Execution

MeTTaTron solves this with **tiered execution** - a progressive optimization strategy where code is promoted to faster execution tiers based on how frequently it runs.

```
                                 ┌─────────────────────────────────────┐
                                 │         Execution Tiers             │
                                 └─────────────────────────────────────┘

        ┌───────────────────────────────────────────────────────────────────┐
        │                                                                   │
        │   Tier 0: Interpreter          Tier 1: Bytecode VM                │
        │   ┌─────────────────┐          ┌─────────────────┐                │
        │   │  Immediate      │   10     │  Fast           │                │
        │   │  execution,     │ ───────► │  interpretation,│                │
        │   │  no compilation │   calls  │  lower overhead │                │
        │   └─────────────────┘          └─────────────────┘                │
        │                                        │                          │
        │                                        │ 100 calls                │
        │                                        ▼                          │
        │   Tier 3: JIT Stage 2          Tier 2: JIT Stage 1                │
        │   ┌─────────────────┐          ┌─────────────────┐                │
        │   │  Full native    │   500    │  Basic native   │                │
        │   │  with runtime   │ ◄─────── │  (arithmetic/   │                │
        │   │  support        │   calls  │   boolean only) │                │
        │   └─────────────────┘          └─────────────────┘                │
        │                                                                   │
        └───────────────────────────────────────────────────────────────────┘
```

**Key insight**: Most code is "cold" (runs rarely). Only a small fraction is "hot" (runs frequently). By investing compilation effort only in hot code, we get the best of both worlds:

- Cold code runs immediately (no compilation delay)
- Hot code runs as fast as hand-optimized native code

---

## 4. Tier 0: The Tree-Walking Interpreter

The tree-walking interpreter is the simplest and most flexible evaluation mode.

**How it works:**

```
Expression: (+ 1 2)

Step 1: Parse into MettaValue tree
        ┌───────────┐
        │  SExpr    │
        │  [+, 1, 2]│
        └─────┬─────┘
              │
    ┌─────────┼─────────┐
    ▼         ▼         ▼
┌──────┐  ┌──────┐  ┌──────┐
│Atom  │  │Long  │  │Long  │
│ "+"  │  │  1   │  │  2   │
└──────┘  └──────┘  └──────┘

Step 2: Pattern match on operator
        Match "+" → arithmetic addition

Step 3: Evaluate arguments recursively
        1 → 1 (already a value)
        2 → 2 (already a value)

Step 4: Apply operation
        1 + 2 = 3

Step 5: Return Long(3)
```

**Location in code**: `src/backend/eval/evaluation.rs`

**Characteristics:**
- Zero startup cost
- Maximum flexibility (can evaluate anything)
- Highest per-operation overhead
- Used for: First execution, rarely-run code, complex expressions

---

## 5. Tier 1: The Bytecode Virtual Machine

The bytecode VM compiles MeTTa expressions to a stack-based bytecode format, then interprets the bytecode.

**Why bytecode is faster than tree-walking:**

1. **Flat representation**: No recursive tree traversal
2. **Dense encoding**: Operations are single bytes, not complex match statements
3. **Better cache locality**: Sequential memory access vs. pointer chasing
4. **Type specialization**: Common paths can be optimized

**Bytecode compilation example:**

```
Expression: (+ 1 2)

Compilation:
┌─────────────────────────────────────────────────────┐
│ Offset │ Opcode      │ Operand │ Description        │
├────────┼─────────────┼─────────┼────────────────────┤
│ 0      │ PUSH_CONST  │ 0       │ Push constant[0]=1 │
│ 2      │ PUSH_CONST  │ 1       │ Push constant[1]=2 │
│ 4      │ ADD         │ -       │ Pop 2, push sum    │
│ 5      │ HALT        │ -       │ End execution      │
└─────────────────────────────────────────────────────┘

Constant Pool: [Long(1), Long(2)]
```

**Execution:**

```
Step 1: PUSH_CONST 0
        Stack: [1]

Step 2: PUSH_CONST 1
        Stack: [1, 2]

Step 3: ADD
        Pop 2, Pop 1, Push 3
        Stack: [3]

Step 4: HALT
        Return Stack[0] = 3
```

**Location in code**: `src/backend/bytecode/vm.rs`

**The BytecodeVM struct:**

```rust
pub struct BytecodeVM {
    chunk: Arc<BytecodeChunk>,  // Compiled bytecode
    stack: Vec<MettaValue>,     // Operand stack
    ip: usize,                  // Instruction pointer
    // ...
}
```

---

## 6. Tier 2 & 3: JIT Compilation with Cranelift

JIT compilation transforms bytecode into native machine code that runs directly on the CPU without any interpretation overhead.

**The compilation pipeline:**

```
┌──────────────────────────────────────────────────────────────────────────┐
│                     JIT Compilation Pipeline                             │
└──────────────────────────────────────────────────────────────────────────┘

  BytecodeChunk                    Cranelift IR                Native Code
  ┌───────────┐                    ┌───────────┐              ┌───────────┐
  │ PUSH 1    │                    │ v0 = 1    │              │ mov eax,1 │
  │ PUSH 2    │   JitCompiler      │ v1 = 2    │   Cranelift  │ mov ebx,2 │
  │ ADD       │ ───────────────►   │ v2 = add  │ ──────────►  │ add eax,  │
  │ HALT      │                    │     v0,v1 │              │     ebx   │
  └───────────┘                    │ return v2 │              │ ret       │
                                   └───────────┘              └───────────┘

                                   Intermediate                Machine
                                   Representation              Instructions
```

### Stage 1 JIT (Tier 2): Basic Native Compilation

Stage 1 JIT handles **pure arithmetic and boolean operations**:
- Arithmetic: `+`, `-`, `*`, `/`, `%`, `pow`
- Comparison: `<`, `>`, `<=`, `>=`, `==`, `!=`
- Boolean: `and`, `or`, `not`
- Constants and stack operations

**Why only these?** These operations:
- Have no side effects
- Don't require runtime support (environments, pattern matching)
- Map directly to CPU instructions

**Location in code**: `src/backend/bytecode/jit/compiler.rs`

### Stage 2 JIT (Tier 3): Full Native with Runtime Support

Stage 2 adds support for complex operations via **runtime helper functions**:

```
┌─────────────────────────────────────────────────────────────────────────┐
│                    Stage 2 JIT Architecture                             │
└─────────────────────────────────────────────────────────────────────────┘

     Native JIT Code                         Runtime Functions
     ┌───────────────┐                       ┌────────────────────────┐
     │ mov rax, 42   │    ┌─────────────────►│ jit_runtime_fork       │
     │ call fork ────┼────┘                  │ jit_runtime_yield      │
     │ test rax, rax │                       │ jit_runtime_cons       │
     │ call load ────┼──────────────────────►│ jit_runtime_load_const │
     │ ret           │                       │ jit_runtime_call       │
     └───────┬───────┘                       └──────────┬─────────────┘
             │                                          │
             │◄─────────── return ──────────────────────┘
             │
             ▼
        Continue execution
        (or return results)
```

**Execution flow:**
- Fast path: Native code runs arithmetic/boolean ops directly
- Slow path: `call` instructions invoke runtime helpers, which return back to native code

**Runtime functions** (in `src/backend/bytecode/jit/runtime.rs`):

| Function | Purpose |
|----------|---------|
| `jit_runtime_fork_native` | Handle nondeterminism (superpose) |
| `jit_runtime_yield_native` | Yield a result in collect |
| `jit_runtime_load_constant` | Load complex constants |
| `jit_runtime_make_sexpr` | Construct S-expressions |
| `jit_runtime_call` | Call into interpreted code |

---

## 7. The Journey of an Expression: From Source to Native Code

Let's follow a single expression through its entire lifecycle:

```metta
!(+ 1 (+ 2 3))
```

### Phase 1: Parsing (Once)

```
Source Text: "!(+ 1 (+ 2 3))"
                    │
                    ▼
            ┌───────────────┐
            │  Tree-Sitter  │
            │    Parser     │
            └───────┬───────┘
                    │
                    ▼
            ┌───────────────┐
            │   MettaValue  │
            │    Tree       │
            └───────────────┘

Result:
  SExpr[
    Atom("!"),
    SExpr[
      Atom("+"),
      Long(1),
      SExpr[
        Atom("+"),
        Long(2),
        Long(3)
      ]
    ]
  ]
```

### Phase 2: Bytecode Compilation (Once)

```
MettaValue Tree                    BytecodeChunk
┌────────────────┐                 ┌─────────────────────────────┐
│ SExpr[+, 1,    │   compile()     │ Code:                       │
│   SExpr[+,2,3]]│ ──────────────► │   0: PUSH_CONST 2  ; Long(2)│
└────────────────┘                 │   2: PUSH_CONST 3  ; Long(3)│
                                   │   4: ADD                    │
                                   │   5: PUSH_CONST 1  ; Long(1)│
                                   │   7: ADD                    │
                                   │   8: EVAL                   │
                                   │   9: HALT                   │
                                   │                             │
                                   │ Constants: [Long(2), Long(3)│
                                   │            Long(1)]         │
                                   │                             │
                                   │ JitProfile: {               │
                                   │   count: 0,                 │
                                   │   state: Cold,              │
                                   │   native_code: null         │
                                   │ }                           │
                                   └─────────────────────────────┘
```

### Phase 3: First 9 Executions (Tier 0/1 - Bytecode VM)

```
Execution 1:
┌──────────────────────────────────────────────────────────────────────┐
│                                                                      │
│   BytecodeVM::run()                                                  │
│        │                                                             │
│        ├─► record_jit_execution()                                    │
│        │        └─► count = 1, state = Cold                          │
│        │        └─► return false (don't compile)                     │
│        │                                                             │
│        └─► run_without_jit()                                         │
│             └─► Execute bytecode in interpretation loop              │
│             └─► Return result: Long(6)                               │
│                                                                      │
└──────────────────────────────────────────────────────────────────────┘

Executions 2-9: Same flow, count increments each time
```

### Phase 4: Execution 10 (Transition to Warming)

```
Execution 10:
┌──────────────────────────────────────────────────────────────────────┐
│                                                                      │
│   BytecodeVM::run()                                                  │
│        │                                                             │
│        ├─► record_jit_execution()                                    │
│        │        └─► count = 10                                       │
│        │        └─► count >= WARM_THRESHOLD (10)? YES                │
│        │        └─► compare_exchange(Cold → Warming) → SUCCESS       │
│        │        └─► state = Warming                                  │
│        │        └─► return false (still don't compile yet)           │
│        │                                                             │
│        └─► run_without_jit()                                         │
│             └─► Execute bytecode                                     │
│             └─► Return result: Long(6)                               │
│                                                                      │
└──────────────────────────────────────────────────────────────────────┘
```

### Phase 5: Executions 11-99 (Warming)

```
Each execution:
- count increments
- state remains Warming
- Still using bytecode VM
```

### Phase 6: Execution 100 (JIT Compilation Triggered!)

```
Execution 100:
┌──────────────────────────────────────────────────────────────────────┐
│                                                                      │
│   BytecodeVM::run()                                                  │
│        │                                                             │
│        ├─► record_jit_execution()                                    │
│        │        └─► count = 100                                      │
│        │        └─► count >= HOT_THRESHOLD (100)? YES                │
│        │        └─► compare_exchange(Warming → Hot) → SUCCESS        │
│        │        └─► return TRUE  ◄─── TRIGGERS COMPILATION           │
│        │                                                             │
│        ├─► try_jit_execute()                                         │
│        │        │                                                    │
│        │        ├─► should_compile = true                            │
│        │        ├─► chunk.can_jit_compile()? YES (only +, consts)    │
│        │        ├─► profile.try_start_compiling()                    │
│        │        │        └─► CAS(Hot → Compiling) → SUCCESS          │
│        │        │                                                    │
│        │        ├─► JitCompiler::new()                               │
│        │        │        └─► Initialize Cranelift JIT module         │
│        │        │        └─► Import runtime functions                │
│        │        │                                                    │
│        │        ├─► compiler.compile(&chunk)                         │
│        │        │        │                                           │
│        │        │        ├─► Create Cranelift function               │
│        │        │        ├─► For each bytecode instruction:          │
│        │        │        │     PUSH_CONST 2 → v0 = iconst.i64 2      │
│        │        │        │     PUSH_CONST 3 → v1 = iconst.i64 3      │
│        │        │        │     ADD          → v2 = iadd v0, v1       │
│        │        │        │     PUSH_CONST 1 → v3 = iconst.i64 1      │
│        │        │        │     ADD          → v4 = iadd v3, v2       │
│        │        │        │     HALT         → return v4              │
│        │        │        ├─► Run Cranelift optimization passes       │
│        │        │        ├─► Generate native x86-64 code             │
│        │        │        └─► Return function pointer                 │
│        │        │                                                    │
│        │        ├─► profile.set_compiled(code_ptr, code_size)        │
│        │        │        └─► native_code = code_ptr                  │
│        │        │        └─► state = Jitted                          │
│        │        │                                                    │
│        │        └─► Execute native code (first JIT execution!)       │
│        │             └─► Call native_fn(&mut ctx)                    │
│        │             └─► Return result: Long(6)                      │
│        │                                                             │
│        └─► Return Ok(Some(results))                                  │
│                                                                      │
└──────────────────────────────────────────────────────────────────────┘
```

---

## 8. Hotness Detection: How the System Knows What to Compile

The hotness detection system uses **atomic counters and state machines** to track execution frequency without locking.

### The State Machine

```
                    ┌─────────────────────────────────────────────────────┐
                    │              JIT Profile State Machine              │
                    └─────────────────────────────────────────────────────┘

                                 ┌─────────┐
                                 │  Cold   │ Initial state
                                 │ count=0 │
                                 └────┬────┘
                                      │ count reaches 10
                                      ▼
                                 ┌─────────┐
                                 │ Warming │ Bytecode VM, counting
                                 │count<100│
                                 └────┬────┘
                                      │ count reaches 100
                                      ▼
            ┌───────────────────┬─────────┬───────────────────┐
            │                   │   Hot   │                   │
            │                   │ count≥100                   │
            │                   └────┬────┘                   │
            │                        │                        │
            │ try_start_compiling()  │                        │
            │ returns true           │ try_start_compiling()  │
            │ (winner)               │ returns false (loser)  │
            ▼                        ▼                        │
       ┌──────────┐            ┌──────────┐                   │
       │Compiling │            │ (wait)   │                   │
       │ in prog  │            │          │                   │
       └────┬─────┘            └──────────┘                   │
            │                                                 │
            ├──── Success ────►  ┌────────┐                   │
            │                    │ Jitted │ ◄─────────────────┘
            │                    │ native │   (observe Jitted)
            │                    └────────┘
            │
            └──── Failure ────►  ┌────────┐
                                 │ Failed │ Permanent, use bytecode
                                 └────────┘
```

### Atomic Operations for Thread Safety

```rust
// File: src/backend/bytecode/jit/profile.rs

pub fn record_execution(&self) -> bool {
    // Atomically increment counter (no locking!)
    let prev = self.execution_count.fetch_add(1, Ordering::Relaxed);
    let count = prev + 1;

    // Check for tier transitions
    if count == HOT_THRESHOLD {
        // Try to transition Warming → Hot
        // Only ONE thread will succeed (CAS operation)
        self.state.compare_exchange(
            JitState::Warming as u8,
            JitState::Hot as u8,
            Ordering::AcqRel,
            Ordering::Acquire,
        ).is_ok()
    } else if count == WARM_THRESHOLD {
        // Try to transition Cold → Warming
        let _ = self.state.compare_exchange(
            JitState::Cold as u8,
            JitState::Warming as u8,
            Ordering::Release,
            Ordering::Relaxed,
        );
        false
    } else {
        false
    }
}
```

**Why atomics instead of locks?**

1. **No contention**: Multiple threads can increment simultaneously
2. **No blocking**: Threads never wait for each other
3. **Cache efficient**: Only the counter cache line is bounced between cores
4. **Exactly-once compilation**: CAS ensures only one thread compiles

---

## 9. JIT Code Storage and Caching

Once code is JIT-compiled, it needs to be stored for fast retrieval.

### The Three-Level Cache Hierarchy

```
┌──────────────────────────────────────────────────────────────────────────┐
│                     JIT Code Cache Hierarchy                             │
└──────────────────────────────────────────────────────────────────────────┘

Level 1: Profile-Embedded Pointer (FASTEST)
┌─────────────────────────────────────────────────────────────────────────┐
│ BytecodeChunk                                                           │
│ ├── code: Vec<u8>                                                       │
│ ├── constants: Vec<MettaValue>                                          │
│ └── jit_profile: JitProfile                                             │
│     └── native_code: AtomicPtr<()>  ◄─── Direct pointer to native code  │
│                                                                          │
│ Access: atomic load (single instruction, ~1 nanosecond)                 │
└─────────────────────────────────────────────────────────────────────────┘
                                │
                                │ If not found in profile
                                ▼
Level 2: Tiered Cache (HashMap with LRU)
┌─────────────────────────────────────────────────────────────────────────┐
│ JitCache                                                                │
│ ├── entries: RwLock<HashMap<ChunkId, CacheEntry>>                       │
│ ├── max_entries: 1024                                                   │
│ └── max_code_bytes: 64 MB                                               │
│                                                                          │
│ CacheEntry:                                                             │
│ ├── native_code: *const ()                                              │
│ ├── code_size: usize                                                    │
│ ├── tier: Tier                                                          │
│ └── last_access: Instant  ◄─── For LRU eviction                         │
│                                                                          │
│ Access: RwLock read + HashMap lookup (~100-500 nanoseconds)             │
└─────────────────────────────────────────────────────────────────────────┘
                                │
                                │ Shared across executors
                                ▼
Level 3: Hybrid Executor Cache
┌─────────────────────────────────────────────────────────────────────────┐
│ HybridExecutor                                                          │
│ ├── jit_cache: Arc<JitCache>       ◄─── Shared reference                │
│ ├── tiered_compiler: Arc<TieredCompiler>                                │
│ └── Pre-allocated buffers for JIT execution                             │
│                                                                          │
│ Purpose: Amortize cache/compiler setup across many executions           │
└─────────────────────────────────────────────────────────────────────────┘
```

### ChunkId: Identifying Compiled Code

```rust
// File: src/backend/bytecode/jit/tiered.rs

pub struct ChunkId(u64);

impl ChunkId {
    pub fn from_chunk(chunk: &BytecodeChunk) -> Self {
        // Hash the bytecode to create unique identifier
        let mut hasher = DefaultHasher::new();
        chunk.code().hash(&mut hasher);
        ChunkId(hasher.finish())
    }
}
```

### LRU Eviction

When the cache is full, least-recently-used entries are evicted:

```rust
fn evict_lru(&self) {
    let mut entries = self.entries.write().unwrap();

    // Find oldest entry
    let oldest = entries.iter()
        .min_by_key(|(_, entry)| entry.last_access)
        .map(|(id, _)| *id);

    if let Some(id) = oldest {
        entries.remove(&id);
    }
}
```

---

## 10. Subsequent Executions: The Fast Path

After JIT compilation, subsequent executions bypass bytecode interpretation entirely:

```
┌──────────────────────────────────────────────────────────────────────────┐
│           Fast Path: JIT-Compiled Execution (Execution 101+)             │
└──────────────────────────────────────────────────────────────────────────┘

BytecodeVM::run()
     │
     ▼
try_jit_execute()
     │
     ├─► record_jit_execution()
     │        └─► count++ (no state change, already Jitted)
     │        └─► return false
     │
     ├─► has_jit_code()?
     │        └─► profile.native_code.load(Acquire)
     │        └─► returns Some(fn_ptr)  ◄─── YES!
     │
     ├─► Initialize JitContext
     │        ┌───────────────────────────────────────┐
     │        │ JitContext {                          │
     │        │   value_stack: pre-allocated buffer,  │
     │        │   sp: 0,                              │
     │        │   constants: chunk.constants.as_ptr(),│
     │        │   bailout: false,                     │
     │        │   ...                                 │
     │        │ }                                     │
     │        └───────────────────────────────────────┘
     │
     ├─► Call native function
     │        │
     │        │   native_fn(&mut ctx)
     │        │
     │        │   ┌─────────────────────────────────┐
     │        │   │  Native x86-64 code executes:   │
     │        │   │                                 │
     │        │   │  mov rax, 2                     │
     │        │   │  mov rbx, 3                     │
     │        │   │  add rax, rbx   ; rax = 5       │
     │        │   │  mov rbx, 1                     │
     │        │   │  add rax, rbx   ; rax = 6       │
     │        │   │  ; Store result in ctx.stack    │
     │        │   │  ret                            │
     │        │   │                                 │
     │        │   │  Total: ~5-10 nanoseconds!      │
     │        │   └─────────────────────────────────┘
     │        │
     │        ▼
     │
     ├─► Check bailout flag
     │        └─► ctx.bailout == false? YES (no bailout)
     │
     ├─► Collect results from ctx.value_stack
     │        └─► results = vec![Long(6)]
     │
     └─► Return Ok(Some(results))
```

**Performance breakdown:**

| Step | Time |
|------|------|
| record_jit_execution | ~5 ns (atomic increment) |
| has_jit_code check | ~1 ns (atomic load) |
| Initialize JitContext | ~10 ns (pointer setup) |
| Native code execution | ~5-40 ns (depends on complexity) |
| Result collection | ~10 ns |
| **Total** | **~30-70 ns** |

Compare to tree-walker for depth-100 expression: **27,800 ns** (27.8 µs)

---

## 11. Bailout: When JIT Code Needs Help

Not all operations can be compiled to native code. When JIT code encounters something it can't handle, it **bails out** to the bytecode VM.

### Why Bailout Exists

Some operations require:
- **Pattern matching** with complex patterns
- **Rule dispatch** that needs environment lookup
- **Dynamic type checks** that aren't predictable
- **Nondeterminism** with unbounded alternatives
- **External calls** to interpreted code

### The Bailout Mechanism

```
┌──────────────────────────────────────────────────────────────────────────┐
│                         Bailout Flow                                     │
└──────────────────────────────────────────────────────────────────────────┘

JIT Code Executing
     │
     ├── Arithmetic operations...
     │        └── Native code, no problem
     │
     ├── Comparison operations...
     │        └── Native code, no problem
     │
     └── Encounters complex operation (e.g., pattern match)
              │
              ▼
         jit_runtime_type_error(ctx, ip, expected)
              │
              ├─► ctx.bailout = true
              ├─► ctx.bailout_ip = current_instruction
              ├─► ctx.bailout_reason = UnsupportedOperation
              └─► return  (exit native function)

Back in BytecodeVM::run()
     │
     ├─► Check ctx.bailout
     │        └─► true!
     │
     ├─► Transfer JIT stack → VM stack
     │        │
     │        │   for i in 0..ctx.sp {
     │        │       let jit_val = ctx.value_stack[i];
     │        │       vm.stack.push(jit_val.to_metta());
     │        │   }
     │        │
     │        └─► VM stack now has same values as JIT stack
     │
     ├─► Set VM instruction pointer
     │        └─► vm.ip = ctx.bailout_ip
     │
     └─► Resume bytecode interpretation
              │
              └─► run_without_jit() continues from bailout point
```

### Bailout is Transparent

The program doesn't know a bailout happened - it just continues running correctly:

```
Before bailout:
  JIT executed instructions 0-15
  Stack: [value1, value2, value3]

After bailout:
  VM resumes at instruction 16
  Stack: [value1, value2, value3]  ◄── Same state!

Result: Correct output, just slower for the bailed-out portion
```

---

## 12. Key Data Structures

### JitValue: NaN-Boxed Values

JIT code uses **NaN-boxing** to store both type and value in a single 64-bit word:

```
┌──────────────────────────────────────────────────────────────────────────┐
│                     JitValue NaN-Boxing Layout                           │
└──────────────────────────────────────────────────────────────────────────┘

IEEE 754 double-precision float:
┌─────────────────────────────────────────────────────────────────────────┐
│ Sign │    Exponent (11 bits)    │        Mantissa (52 bits)             │
│  1   │ [62:52]                  │ [51:0]                                │
└─────────────────────────────────────────────────────────────────────────┘

NaN has exponent = all 1s (0x7FF)

Quiet NaN (used for boxing):
┌─────────────────────────────────────────────────────────────────────────┐
│ 0 │ 11111111111 │ 1 │  Tag (3 bits)  │     Payload (48 bits)            │
│   │   0x7FF     │   │                │                                  │
└─────────────────────────────────────────────────────────────────────────┘

Tag values:
  0x7FF8: Long    (signed 48-bit integer, fits most MeTTa numbers)
  0x7FF9: Bool    (0 = false, 1 = true)
  0x7FFA: Nil     (unit type)
  0x7FFB: Unit    (empty result)
  0x7FFC: Heap    (pointer to MettaValue on heap)
  0x7FFD: Error   (pointer to error value)
  0x7FFE: Atom    (pointer to symbol string)
  0x7FFF: Var     (pointer to variable)
```

**Why NaN-boxing?**

```rust
// Type check is a single AND + compare
fn is_long(v: u64) -> bool {
    (v & 0xFFFF_0000_0000_0000) == 0x7FF8_0000_0000_0000
}

// Extract value is a single AND
fn get_long(v: u64) -> i64 {
    // Sign-extend from 48 bits
    ((v as i64) << 16) >> 16
}
```

Single register, single instruction type checks, no pointer chasing for primitives.

### JitContext: Runtime State

```rust
// File: src/backend/bytecode/jit/types.rs

#[repr(C)]  // Fixed memory layout for FFI
pub struct JitContext {
    // ═══════════════════════════════════════
    // Value Stack (operands and results)
    // ═══════════════════════════════════════
    pub value_stack: *mut JitValue,  // Stack buffer pointer
    pub sp: usize,                    // Current stack pointer
    pub stack_cap: usize,             // Stack capacity

    // ═══════════════════════════════════════
    // Constant Pool (from BytecodeChunk)
    // ═══════════════════════════════════════
    pub constants: *const MettaValue,
    pub constants_len: usize,

    // ═══════════════════════════════════════
    // Bailout Handling
    // ═══════════════════════════════════════
    pub bailout: bool,           // Set to true to trigger bailout
    pub bailout_ip: usize,       // Instruction to resume at
    pub bailout_reason: JitBailoutReason,

    // ═══════════════════════════════════════
    // Non-determinism (Superpose/Collapse)
    // ═══════════════════════════════════════
    pub choice_points: *mut JitChoicePoint,
    pub choice_point_count: usize,
    pub results: *mut JitValue,
    pub results_count: usize,

    // ═══════════════════════════════════════
    // Optimizations
    // ═══════════════════════════════════════
    pub state_cache: [(u64, u64); 8],        // State lookup cache
    pub var_index_cache: [(u64, u32); 32],   // Variable index cache
    pub stack_save_pool: *mut JitValue,       // Pre-allocated stack saves
}
```

### JitProfile: Execution Tracking

```rust
// File: src/backend/bytecode/jit/profile.rs

pub struct JitProfile {
    // Thread-safe execution counter
    execution_count: AtomicU32,

    // State machine: Cold → Warming → Hot → Compiling → Jitted
    state: AtomicU8,

    // Pointer to compiled native code (null if not compiled)
    native_code: AtomicPtr<()>,

    // Size of generated code (for cache management)
    code_size: AtomicU32,
}
```

---

## 13. Putting It All Together: Complete Execution Flow

Here's the complete decision tree for every execution:

```
┌──────────────────────────────────────────────────────────────────────────┐
│                    Complete Execution Flow Chart                         │
└──────────────────────────────────────────────────────────────────────────┘

                              BytecodeVM::run()
                                     │
                                     ▼
                      ┌─────────────────────────────┐
                      │ Is JIT feature enabled?     │
                      └──────────────┬──────────────┘
                                     │
                        ┌────────────┴────────────┐
                        │                         │
                       YES                        NO
                        │                         │
                        ▼                         │
               try_jit_execute()                  │
                        │                         │
                        ▼                         │
           ┌─────────────────────┐                │
           │ record_jit_execution│                │
           │ (increment counter) │                │
           └──────────┬──────────┘                │
                      │                           │
                      ▼                           │
           ┌─────────────────────┐                │
           │ Just became hot?    │                │
           │ (count hit 100)     │                │
           └──────────┬──────────┘                │
                      │                           │
             ┌────────┴────────┐                  │
             │                 │                  │
            YES                NO ────────────────┼──┐
             │                                    │  │
             ▼                                    │  │
      ┌───────────────────┐                       │  │
      │ Can JIT compile?  │                       │  │
      │ (supported ops?)  │                       │  │
      └────────┬──────────┘                       │  │
               │                                  │  │
          ┌────┴────┐                             │  │
          │         │                             │  │
         YES        NO ───────────────────────────┼──┤
          │                                       │  │
          ▼                                       │  │
   ┌─────────────┐                                │  │
   │ Try to win  │                                │  │
   │ compilation │                                │  │
   │ race (CAS)  │                                │  │
   └──────┬──────┘                                │  │
          │                                       │  │
     ┌────┴────┐                                  │  │
     │         │                                  │  │
    WON      LOST ────────────────────────────────┼──┤
     │                                            │  │
     ▼                                            │  │
   ┌─────────────┐                                │  │
   │ Compile to  │                                │  │
   │ native code │                                │  │
   │ (Cranelift) │                                │  │
   └──────┬──────┘                                │  │
          │                                       │  │
          ▼                                       │  │
      ┌─────────────────────────┐                 │  │
      │ Has native code ready?  │◄────────────────┘  │
      └────────────┬────────────┘                    │
                   │                                 │
          ┌────────┴────────┐                        │
          │                 │                        │
         YES                NO ──────────────────────┤
          │                                          │
          ▼                                          │
      ┌─────────────────┐                            │
      │ Init JitContext │                            │
      └────────┬────────┘                            │
               │                                     │
               ▼                                     │
      ┌─────────────────┐                            │
      │ Call native fn  │                            │
      └────────┬────────┘                            │
               │                                     │
               ▼                                     │
      ┌─────────────────┐                            │
      │ Bailed out?     │                            │
      └────────┬────────┘                            │
               │                                     │
          ┌────┴────┐                                │
          │         │                                │
         YES        NO                               │
          │         │                                │
          │         ▼                                │
          │    ┌────────────┐                        │
          │    │ Return JIT │                        │
          │    │ results    │ ◄── FAST PATH (done)   │
          │    └────────────┘                        │
          │                                          │
          ▼                                          │
   ┌───────────┐                                     │
   │ Transfer  │                                     │
   │ stack,    │                                     │
   │ set VM.ip │                                     │
   └─────┬─────┘                                     │
         │                                           │
         ▼                                           │
  ┌──────────────────────────┐                       │
  │     run_without_jit()    │◄──────────────────────┘
  │  (bytecode VM execution) │
  └───────────┬──────────────┘
              │
              ▼
        Return results
```

---

## 14. File Reference

| File | Purpose |
|------|---------|
| `src/backend/bytecode/vm.rs` | Main VM with JIT dispatch (lines 408-544) |
| `src/backend/bytecode/jit/profile.rs` | Hotness tracking, state machine |
| `src/backend/bytecode/jit/tiered.rs` | Tier definitions, JitCache, LRU |
| `src/backend/bytecode/jit/types.rs` | JitValue, JitContext, JitChoicePoint |
| `src/backend/bytecode/jit/compiler.rs` | Cranelift integration, code generation |
| `src/backend/bytecode/jit/codegen.rs` | IR building for each opcode |
| `src/backend/bytecode/jit/runtime.rs` | Runtime helper functions |
| `src/backend/bytecode/jit/hybrid.rs` | HybridExecutor wrapper |

---

## Summary

MeTTaTron's JIT pipeline provides **automatic, transparent optimization** of hot code paths:

1. **Progressive optimization**: Code starts interpreted, graduates to bytecode, then native
2. **Zero manual configuration**: Thresholds (10/100/500) work well for most workloads
3. **Thread-safe**: Atomic operations prevent races without locking
4. **Safe fallback**: Bailout mechanism handles unsupported operations
5. **Efficient caching**: Three-level cache hierarchy minimizes lookup overhead
6. **Performance gains**: 700-1500x speedup for hot arithmetic/control-flow code

The key insight is that most code is cold and doesn't need optimization, while the small fraction that's hot gets the full benefit of native compilation - achieving the best of both worlds.

---

## 15. Implementing for Other Languages

This tiered JIT architecture is designed to be portable. Here's how to adapt it for other languages like Rholang.

### What's Portable

1. **Core infrastructure** - The following components work for any language:
   - `JitProfile` state machine (Cold → Warming → Hot → Compiling → Jitted)
   - Atomic profiling with compare-exchange for thread safety
   - `HybridExecutor` pattern for tier dispatch and bailout
   - Cranelift code generation infrastructure
   - LRU cache for compiled code

2. **NaN-boxing scheme** - The 8 tag types with 48-bit payloads can represent any language's primitives:
   - Redefine `TAG_*` constants for your language's types
   - 48-bit pointers cover modern x86-64 addresses
   - Type checks remain 2-3 CPU cycles

3. **Background compilation** - The P2 priority scheduler works for any compilation task:
   - Feature-gated via `hybrid-p2-priority-scheduler`
   - Falls back to Rayon when disabled
   - Compatible with shared thread pools

### What to Adapt

1. **Value representation** - Define NaN-boxing tags for your types:
   ```rust
   // Example for Rholang
   pub const TAG_PROCESS: u64 = QNAN | (0 << 48);
   pub const TAG_CHANNEL: u64 = QNAN | (1 << 48);
   pub const TAG_NAME: u64 = QNAN | (2 << 48);
   ```

2. **Runtime helpers** - Implement `extern "C"` functions for language-specific operations:
   - Pattern matching semantics
   - Concurrency primitives
   - Type system operations

3. **JitContext fields** - Add language-specific context:
   - Rholang: channel registry, pending sends/receives
   - Prolog: unification stack, clause database
   - Datalog: fact database, stratification info

### Implementation Guide

For detailed implementation steps, see:
- `docs/architecture/TIERED_COMPILER_IMPLEMENTATION_GUIDE.md` - Comprehensive porting guide
- `docs/architecture/HYBRID_P2_PRIORITY_SCHEDULER.md` - Background compilation scheduling

### Shared Rayon Compatibility

If your project already uses Rayon, the JIT infrastructure integrates seamlessly:

```rust
// Both languages share the global Rayon pool
rayon::ThreadPoolBuilder::new()
    .num_threads(num_cpus::get())
    .build_global()
    .expect("Failed to initialize Rayon");

// Compilation tasks use rayon::spawn() by default
// Enable P2 scheduler with: --features hybrid-p2-priority-scheduler
```
