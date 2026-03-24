# Compile-on-Add: Next Steps for Sub-1s PLN Robot

## Current State: 1.45s (target <1.0s)

### What's Done
- Compile-on-add: rule RHS bodies pre-compiled at `add_rule` time → `RuleEntry.compiled_rhs`
- VM compiled RHS execution via call frame switching in `op_dispatch_rules`
- Bytecode compiler extended: `case`, `collapse`, `trace!` support
- TieredCache caching for environment-aware bytecode
- WFST-gated parallelism, speculative matching fix, structural sharing

### What's Blocking Sub-1s

1. **EvalCase trampoline fallback** (VM delegates to `eval_sub_expr_vm`)
   - The iterative compiler already has jump-based case compilation using `MatchBind` + `JumpIfFalse`
   - Port this to the generic compiler to eliminate the EvalCase → trampoline round-trip
   - The VM has `MatchBind` (0x71) with fast-path optimization for variables, wildcards, ground types

2. **EvalCollapse trampoline fallback** (VM delegates to `eval_sub_expr_vm`)
   - The VM's choice point mechanism tracks alternatives
   - Need a "collect mode" that exhausts all choice points into a Vec
   - Currently bails out to trampoline for full nondeterministic exploration

3. **Pre-compiled built-in bytecode** (user's architectural requirement)
   - Built-in operations (`+`, `if`, `cons-atom`, etc.) should be ready at runtime
   - Only user-defined expressions should require compilation
   - Create `BuiltinBytecodeRegistry` with lazy-initialized pre-compiled chunks
   - Map `(head, arity)` → `Arc<BytecodeChunk>` for all ~60 built-in operations

### Architecture for Pre-compiled Built-ins

```rust
// New file: src/backend/bytecode/builtin_chunks.rs
static BUILTIN_REGISTRY: OnceLock<HashMap<(&str, u8), Arc<BytecodeChunk>>> = OnceLock::new();

pub fn get_builtin_chunk(head: &str, arity: u8) -> Option<Arc<BytecodeChunk>> {
    BUILTIN_REGISTRY.get_or_init(|| {
        let mut map = HashMap::new();
        // Pre-compile all built-in operations
        register_arithmetic(&mut map);  // +, -, *, /, etc.
        register_comparison(&mut map);  // <, >, ==, etc.
        register_list_ops(&mut map);    // car-atom, cdr-atom, cons-atom, etc.
        register_control_flow(&mut map); // if, let, let*, case, etc.
        map
    }).get(&(head, arity)).cloned()
}
```

### Jump-based Case Compilation (from iterative compiler)

```
; (case scrutinee ((pattern1 body1) (pattern2 body2)))
compile scrutinee
Dup
PushConstant pattern1    ; quoted pattern
MatchBind                ; returns bool, populates bindings on match
JumpIfFalse → next1
Pop                      ; remove scrutinee
compile body1
Jump → end
next1:
Dup
PushConstant pattern2
MatchBind
JumpIfFalse → next2
Pop
compile body2
Jump → end
next2:
Pop                      ; no match
PushEmpty
end:
```

### Key Files

- `src/backend/bytecode/compiler/generic.rs` — port jump-based case from iterative
- `src/backend/bytecode/vm/mod.rs` — native collapse collect-all-results
- `src/backend/bytecode/builtin_chunks.rs` (new) — pre-compiled built-in registry
- `src/backend/eval/mod.rs` — use builtin registry in eval_inner
