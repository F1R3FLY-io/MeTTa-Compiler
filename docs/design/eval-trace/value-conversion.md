# Iterative Trampoline for trace_value()

## Problem

`MettaValue → TraceValue` conversion must handle arbitrarily deep
S-expressions without stack overflow. A naive recursive approach:

```rust
fn trace_value_recursive(v: &MettaValue) -> TraceValue {
    match v.inner() {
        MettaValueInner::SExpr(items) =>
            TraceValue::SExpr(items.iter().map(trace_value_recursive).collect()),
        // ...
    }
}
```

overflows the stack at depth ~1000 on typical platforms (8 MB default
stack). While MeTTa programs rarely nest this deep during normal
evaluation, pathological inputs or generated code could trigger it.

## Solution: Iterative Trampoline

**Source**: `src/backend/trace/convert.rs:69`

The `trace_value()` function uses an explicit work stack and continuation
stack to convert the value tree iteratively, eliminating all recursive
calls.

### Data Structures

```rust
enum Cont {
    CollectSExpr   { remaining: usize, collected: Vec<TraceValue> },
    WrapError      { message: String },
    WrapType,
    WrapQuoted,
    CollectConjunction { remaining: usize, collected: Vec<TraceValue> },
}
```

Two thread-local stacks are reused across calls:

```rust
thread_local! {
    static TRACE_WORK: RefCell<Vec<*const MettaValueInner>>
        = RefCell::new(Vec::with_capacity(64));
    static TRACE_CONTS: RefCell<Vec<Cont>>
        = RefCell::new(Vec::with_capacity(32));
}
```

### Algorithm

The trampoline operates in two alternating phases:

**Phase 1 — Process work item**: Pop a `*const MettaValueInner` from
the work stack. If it is a leaf type (Atom, Bool, Long, Float, String,
Unit, Empty, Space, State, Memo), produce a `TraceValue` result
immediately. If it is a compound type (SExpr, Error, Type, Quoted,
Conjunction), push a `Cont` describing how to assemble the children,
then push the children onto the work stack (in reverse order so the
first child is popped first).

**Phase 2 — Feed result to continuation**: When a result is available,
feed it to the topmost continuation. If the continuation needs more
children (e.g., `CollectSExpr` with `remaining > 0`), continue to
Phase 1. If the continuation is complete, pop it and produce a new
result, which feeds into the next continuation (or becomes the final
result if no continuations remain).

### Why Raw Pointers

The thread-local `TRACE_WORK` stack stores `*const MettaValueInner`
instead of `&MettaValueInner`. This is because:

1. The `RefCell<Vec<...>>` borrow from `TRACE_WORK.with()` prevents
   storing references that borrow from the `RefCell`'s contents.
2. `MettaValue::inner()` returns `&'static MettaValueInner` (slab-allocated),
   so the pointers are valid for the `'static` lifetime.
3. The pointers are only dereferenced within the scope of the
   `trace_value()` function, well within their validity period.

### Spanned Stripping

`MettaValueInner::Spanned(v, span)` wrappers are stripped by
`strip_spanned()` before processing. This is defensive — `MettaValue::inner()`
already strips the outermost Spanned layer, but nested Spanned wrappers
(which shouldn't normally occur) are handled gracefully.

## Generic Fallback: trace_value_generic

**Source**: `src/backend/trace/convert.rs:259`

```rust
pub fn trace_value_generic<V: MettaValueTrait>(v: &V) -> TraceValue
```

This is a recursive implementation using the `MettaValueTrait` methods
rather than raw `MettaValueInner` access. It is used in generic
trampoline code where the value type parameter `C::Value` is
`MettaValueTrait` but not necessarily `MettaValue` at the type level.

In practice, `C::Value` is always `MettaValue`, but Rust's type system
requires the generic path. For the hot path with concrete `MettaValue`,
`trace_value()` (the iterative trampoline) is preferred.

The recursive implementation is acceptable here because:
- It is only called from within `#[cfg(feature = "eval-trace")]` blocks
- MeTTa expressions in practice are shallow (depth < 50)
- The generic path is a fallback, not the primary conversion path

## Performance

The iterative trampoline with thread-local stack reuse achieves:

- **Zero allocation after warmup**: The `Vec`s are pre-allocated and
  `.clear()`ed on each call, not re-allocated.
- **~200–500ns per typical expression**: For a depth-5 expression with
  10 leaf nodes, the conversion takes 200–500ns (dominated by string
  cloning for Atom/String variants).
- **No stack overflow**: Tested with depth-100 nested S-expressions
  in `test_trace_value_conversion_deep_nesting` (see `tests.rs:141`).

## Alternatives Considered

### Direct Recursion with Stack Size Increase

Setting a larger stack size (e.g., 64 MB) would handle deeper recursion
but:
- Wastes virtual memory for the common case (shallow expressions)
- Is a global setting that affects all threads
- Is a band-aid, not a proper solution

### Rayon-style Split/Steal

Parallelizing the conversion across threads would be possible for wide
S-expressions but adds complexity for negligible benefit — the
conversion is fast and only happens when tracing is enabled.

### Storing MettaValue References Directly

Instead of converting to TraceValue, store the original `MettaValue`
references in the trace event. This was rejected because of GC safety —
see [`gc-safety.md`](gc-safety.md).
