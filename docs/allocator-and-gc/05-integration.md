# Chapter 5: System Integration

This chapter describes how the slab allocator and garbage collector connect to the rest of MeTTaTron — the value type, the factory, the evaluation loop, the JIT compiler, and deserialization.

## MettaValue — The Value Type

`MettaValue` is MeTTaTron's primary value type. It wraps a single pointer to a `MettaValueInner` stored in the slab allocator.

```rust
// metta_value.rs
#[derive(Clone, Copy)]
pub struct MettaValue {
    inner: &'static MettaValueInner,
}
```

### Properties

| Property | Value |
|----------|-------|
| Size | 8 bytes (one pointer) |
| Clone | `Copy` — bit-for-bit copy, zero cost |
| Thread safety | `Send + Sync` (values are immutable after allocation) |
| Lifetime | `'static` (global allocator outlives all values) |
| GC interaction | `inner_ptr()` exposes raw pointer for slot identification |

### MettaValueInner Variants

```rust
// metta_value.rs
pub enum MettaValueInner {
    Atom(&'static str),                 // Symbol, variable, or literal name
    Bool(bool),                         // Boolean literal
    Long(i64),                          // Integer literal
    Float(f64),                         // Floating-point literal
    String(&'static str),               // String literal
    SExpr(&'static [MettaValue]),       // S-expression (list of values)
    Error(&'static str, MettaValue),    // Error with message and details
    Type(MettaValue),                   // First-class type wrapper
    Conjunction(&'static [MettaValue]), // Logical AND of goals
    Space(SpaceHandle),                 // First-class space reference
    State(u64),                         // Mutable state cell reference
    Unit,                               // Unit value (empty expression)
    Memo(MemoHandle),                   // Memoization table reference
    Empty,                              // Empty sentinel
}
```

All variants are the same size (Rust enum = largest variant). Variable-length data — strings (`&str`), slices (`&[MettaValue]`), error messages — are stored separately in data class pages and referenced by pointer.

### Memory Layout

```
MettaValue (8 bytes):
┌──────────────────────────────┐
│  &'static MettaValueInner    │──→ Value page slot (80 bytes)
└──────────────────────────────┘    ┌─────────────────────────────┐
                                    │ discriminant + variant data  │
                                    │ e.g., SExpr(&[...])         │──→ Data page slot
                                    │                             │    ┌──────────────┐
                                    └─────────────────────────────┘    │ [ptr, ptr, …] │
                                                                       └──────────────┘
```

A `MettaValue` is just a pointer. Cloning it copies the pointer. There is no reference count to increment, no allocation to perform, and no atomic operation. This is why `MettaValue` is `Copy`.

### Thread Safety

`MettaValue` is `Send + Sync` because:
1. The global `SlabAllocator` is thread-safe (lock-free Treiber stack + atomic bump)
2. Values are immutable after allocation — `MettaValue` exposes no mutation methods
3. The `'static` lifetime ensures the referenced data lives until GC reclaims it

These guarantees are upheld by `unsafe impl Send/Sync` blocks with documented safety invariants (see `metta_value.rs`).

## GcFactory — The Value Factory

`GcFactory` is the user-facing API for creating `MettaValue`s. It implements the `MettaValueFactory<MettaValue>` trait.

```rust
// gc_allocator.rs:1738-1741
#[derive(Clone, Copy)]
pub struct GcFactory {
    alloc: &'static SlabAllocator,
}
```

### Properties

| Property | Value |
|----------|-------|
| Size | 8 bytes (one pointer) |
| Clone | `Copy` — zero cost |
| Thread safety | `Send + Sync` |
| Default | `global_factory()` — backed by the global allocator |

### Factory Methods

Each factory method allocates the necessary data (strings, slices) and then allocates the `MettaValueInner` value:

```rust
// Example: creating an Atom
fn atom(&self, s: &str) -> MettaValue {
    let s: &'static str = self.alloc.alloc_str(s);      // Copy string into data page
    MettaValue::from_inner(self.alloc.alloc_value(
        MettaValueInner::Atom(s)                          // Write value into value page
    ))
}

// Example: creating an SExpr
fn sexpr(&self, items: Vec<MettaValue>) -> MettaValue {
    if items.is_empty() { return self.unit(); }
    let slice = self.alloc.alloc_slice_from_iter(items);  // Copy slice into data page
    MettaValue::from_inner(self.alloc.alloc_value(
        MettaValueInner::SExpr(slice)                      // Write value into value page
    ))
}
```

The `MettaValueFactory` trait provides methods for all variant types: `atom()`, `bool()`, `long()`, `float()`, `string()`, `sexpr()`, `sexpr_from_slice()`, `error()`, `type_value()`, `conjunction()`, `space()`, `state()`, `unit()`, `memo()`, `empty()`, and `deserialize()`.

## Global Singletons

The system uses three global singletons managed by `OnceLock` for lazy initialization:

```
┌─────────────────────────────────────────────────────────────────┐
│                    Global Singletons                             │
│                                                                  │
│  GLOBAL_ALLOCATOR: OnceLock<SlabAllocator>                       │
│    → Lazily initialized on first call to global_allocator()      │
│    → Lives for entire program duration                           │
│    → Never dropped (OnceLock ensures single initialization)      │
│                                                                  │
│  GLOBAL_GC_THREAD: OnceLock<Mutex<GcThread>>                     │
│    → Lazily spawned on first call to global_gc_thread()          │
│    → Mutex because GcThread contains mpsc::Receiver (!Sync)      │
│    → Joined on Drop (sends Shutdown, waits for thread exit)      │
│                                                                  │
│  ROOT_REGISTRY: OnceLock<RwLock<Vec<Weak<dyn RootProvider>>>>   │
│    → Lazily initialized on first registration                    │
│    → Weak refs auto-prune dead providers                         │
│    → RwLock: writes rare (env creation), reads during GC only    │
│                                                                  │
│  GC_REQUESTED: AtomicBool (static, not OnceLock)                 │
│    → Set by cron manager or explicit request_gc()                │
│    → Cleared by maybe_trigger_gc() via CAS                       │
│    → Coordination flag between cron and eval threads             │
└─────────────────────────────────────────────────────────────────┘
```

### Initialization Order

The singletons initialize lazily and independently. There is no required initialization order:

1. `global_allocator()` is typically called first (when the first `GcFactory` is created)
2. `ROOT_REGISTRY` initializes when the first environment registers itself
3. `GLOBAL_GC_THREAD` initializes when the first `maybe_trigger_gc()` or `trigger_gc_cycle()` is called
4. `GC_REQUESTED` is a static `AtomicBool` — always available, no initialization needed

## Evaluation Loop Integration

The evaluation loop uses `MettaState` and `SessionContext` to bridge the allocator to the generic trampoline engine.

### MettaState

`MettaState` is a simple session container holding compiled source expressions and evaluation output:

```rust
// metta_state.rs
pub struct MettaState {
    source: Vec<MettaValue>,
    output: Vec<MettaValue>,
}
```

It delegates to `global_factory()` for all allocation. There are no per-session arenas -- all allocations go through the global slab allocator.

### SessionContext

`SessionContext` implements `EvalContext`, the trait required by the generic trampoline engine:

```rust
// session_context.rs
pub struct SessionContext<'s> {
    state: &'s MettaState,
    factory: GcFactory,
}
```

Key methods:

```rust
impl<'s> EvalContext for SessionContext<'s> {
    type Value = MettaValue;
    type Factory = GcFactory;

    fn factory(&self) -> &GcFactory { &self.factory }

    fn maybe_gc(&self) {
        crate::backend::models::gc_allocator::maybe_trigger_gc();
    }
}
```

The `maybe_gc()` method is called every 256 trampoline iterations by the generic trampoline engine. It is the primary integration point between evaluation and garbage collection.

### Evaluation Flow

```
compile(source_text)
    -> Parse MeTTa source into MettaValue expressions
    -> Store in MettaState.source

eval(&MettaState)
    -> Create SessionContext from MettaState
    -> For each source expression:
        -> Run generic trampoline engine
        -> Every 256 iterations: SessionContext::maybe_gc()
            -> maybe_trigger_gc()
            -> If GC response pending: process it (epoch filter, free dead, release pages)
            -> If GC_REQUESTED: build snapshot, send to GC thread
        -> Collect results into MettaState.output
```

## Environment Root Registration

`GenericEnvironmentShared<MettaValue>` implements `RootProvider` to expose its held values as GC roots. Registration happens automatically in several environment lifecycle methods:

```rust
// Called from GenericEnvironment::new(), make_owned(),
// fork_for_nondeterminism(), union(), union_all()
try_register_env_roots(&shared);
```

`try_register_env_roots()` uses `Any` downcasting so it compiles for any value type `V` but only actually registers when `V = MettaValue`:

```rust
pub fn try_register_env_roots<V>(shared: &Arc<GenericEnvironmentShared<V>>) {
    let any: Arc<dyn Any + Send + Sync> = shared.clone();
    if let Ok(metta_shared) =
        any.downcast::<GenericEnvironmentShared<MettaValue>>()
    {
        let provider: Arc<dyn RootProvider> = metta_shared;
        register_root_provider(&provider);
    }
    // For non-MettaValue types: no-op
}
```

The `Weak` reference in the registry ensures automatic cleanup:

```
Environment creation:
    Arc<GenericEnvironmentShared> created (strong count = 1+)
    Weak registered in ROOT_REGISTRY
    collect_all_roots() can upgrade Weak → Strong, collect roots

Environment drop:
    Arc dropped → strong count = 0
    Next collect_all_roots() call:
        Weak::upgrade() returns None
        Entry pruned from registry via retain()
```

## JIT Integration

The JIT compiler uses the slab allocator through an opaque pointer stored in `JitContext`:

```rust
// JitContext stores:
pub fn arena_ptr(&self) -> *const () {
    // Returns opaque pointer to SlabAllocator
}
```

JIT runtime functions cast the opaque pointer back to a `SlabAllocator` reference and create a `GcFactory`:

```rust
// Example from value_creation.rs:66-77
unsafe {
    let arena_ptr = ctx_ref.arena_ptr();
    assert!(!arena_ptr.is_null(), "arena_ptr must be set in arena mode");
    let alloc: &'static SlabAllocator = &*(arena_ptr as *const SlabAllocator);
    let factory = GcFactory::new(alloc);
    // Use factory to create values...
}
```

The JIT uses a `JitValueMode` enum to distinguish between value allocation strategies:

```rust
pub enum JitValueMode {
    Heap,   // Values are Arc-wrapped -- legacy mode
    Arena,  // Values are MettaValue -- slab allocator
}
```

JIT-compiled code never allocates directly from the slab. It always calls into runtime helper functions (`jit_runtime_create_sexpr`, `jit_runtime_create_atom`, etc.) which use the `GcFactory`.

## Deserialization

`deserialize_slab_value()` reads values from a binary format and allocates them directly into the slab allocator:

```rust
// gc_allocator.rs
fn deserialize_slab_value(
    factory: &GcFactory,
    bytes: &[u8],
) -> Result<(MettaValue, usize), String> {
    let tag = bytes[0];
    let rest = &bytes[1..];

    match tag {
        ATOM => {
            let (len, varint_size) = read_varint(rest)?;
            let s = std::str::from_utf8(&rest[varint_size..varint_size + len])?;
            Ok((factory.atom(s), 1 + varint_size + len))
        }
        BOOL => Ok((factory.bool(rest[0] != 0), 2)),
        LONG => {
            let n = i64::from_le_bytes(rest[..8].try_into().unwrap());
            Ok((factory.long(n), 9))
        }
        SEXPR => {
            let (count, varint_size) = read_varint(rest)?;
            let mut items = Vec::with_capacity(count);
            let mut offset = 1 + varint_size;
            for _ in 0..count {
                let (item, consumed) = deserialize_slab_value(factory, &bytes[offset..])?;
                items.push(item);
                offset += consumed;
            }
            Ok((factory.sexpr(items), offset))
        }
        UNIT_LEGACY => Ok((factory.unit(), 1)),  // Wire compat with old Nil tag
        UNIT => Ok((factory.unit(), 1)),
        // ... Float, String, Error, Type, Conjunction, Space, State, Memo, Empty
    }
}
```

This replaces the old implementation that leaked a `bumpalo::Bump` arena per deserialization call. Values are now allocated directly into the global slab, where the GC can track and reclaim them.

Wire compatibility is preserved via legacy tag constants — the `UNIT_LEGACY` tag (formerly `Nil`) decodes to the same `Unit` value as the current `UNIT` tag.

## Component Data Flow Diagram

```
┌─────────────────────────────────────────────────────────────────────┐
│                        MeTTaTron Evaluation                         │
│                                                                     │
│  ┌──────────┐    ┌─────────────┐    ┌──────────────┐                │
│  │ compile()│───→│ MettaState  │───→│ SessionCtx   │                │
│  │          │    │ source: Vec │    │ factory: Gc  │                │
│  └──────────┘    │ output: Vec │    │ maybe_gc()   │                │
│                  └─────────────┘    └──────┬───────┘                │
│                                            │                        │
│                    ┌───────────────────────┘                        │
│                    ▼                                                │
│  ┌────────────────────────────┐     ┌────────────────────┐         │
│  │   GcFactory (Copy, 8 B)   │────→│   SlabAllocator    │         │
│  │   atom(), sexpr(), ...    │     │   alloc_value()    │         │
│  └────────────────────────────┘     │   alloc_str()     │         │
│                                     │   alloc_slice()   │         │
│  ┌────────────────────────────┐     └───────┬────────────┘         │
│  │   JIT Runtime Functions   │──────────────┘                      │
│  │   arena_ptr → GcFactory   │                                     │
│  └────────────────────────────┘                                     │
│                                                                     │
│  ┌────────────────────────────┐     ┌────────────────────┐         │
│  │   GenericEnvironment      │────→│   ROOT_REGISTRY    │         │
│  │   implements RootProvider │     │   Vec<Weak<dyn RP>>│         │
│  │   try_register_env_roots()│     └────────┬───────────┘         │
│  └────────────────────────────┘              │                     │
│                                              │ collect_all_roots() │
│                                              ▼                     │
│  ┌────────────────────────────┐     ┌────────────────────┐         │
│  │   maybe_trigger_gc()      │────→│   GcThread         │         │
│  │   build_snapshot()        │     │   mark_snapshot()  │         │
│  │   process_gc_response()   │←────│   sweep_snapshot() │         │
│  └────────────────────────────┘     └────────────────────┘         │
│                                                                     │
│  ┌────────────────────────────┐                                     │
│  │   GcCronHandle            │     Sets GC_REQUESTED when          │
│  │   mettatron-gc-cron thread│     allocation rate > 100k/s        │
│  └────────────────────────────┘                                     │
└─────────────────────────────────────────────────────────────────────┘
```

## What's Next

[Chapter 6](06-formal-verification.md) describes the TLA+ formal verification models that exhaustively explored all thread interleavings and discovered four concurrency bugs — all of which were fixed and verified.
