# Complete RootProvider Implementation Map — Phase A4.2a

**Branch**: feature/petta-semantics  
**Commit**: HEAD  
**Date**: 2026-05-29  

## Verified Inventory

All 8 implementations found via `rg 'impl RootProvider for'`:

---

## GLOBAL SINGLETON ANCHORS (a) — for collect_global_anchors()

### 1. TieredCacheRoots
| Aspect | Details |
|--------|---------|
| **Impl Block** | `src/backend/bytecode/tiered_cache.rs:1929` |
| **Impl Body** | Delegates to `global_tiered_cache().collect_roots_into(roots)` |
| **Inherent Collector** | `TieredCache::collect_roots_into(&self, roots: &mut Vec<MettaValue>)` at `src/backend/bytecode/tiered_cache.rs:1104` |
| **Global Accessor** | `pub fn global_tiered_cache() -> &'static TieredCache` at `src/backend/bytecode/tiered_cache.rs:1873` |
| **Status** | ✓ Already delegates; ready for direct call |
| **Call in collect_global_anchors** | `global_tiered_cache().collect_roots_into(out)` |

### 2. SpaceRegistryRoots
| Aspect | Details |
|--------|---------|
| **Impl Block** | `src/backend/bytecode/space_registry.rs:165` |
| **Impl Body** | Delegates to `crate::backend::bytecode::global_space_registry().collect_all_gc_values(roots)` |
| **Inherent Collector** | `SpaceRegistry::collect_all_gc_values(&self, roots: &mut Vec<MettaValue>)` at `src/backend/bytecode/space_registry.rs:146` |
| **Global Accessor** | `pub fn global_space_registry() -> &'static SpaceRegistry` at `src/backend/bytecode/mod.rs:285` |
| **Status** | ✓ Already delegates; ready for direct call |
| **Call in collect_global_anchors** | `global_space_registry().collect_all_gc_values(out)` |

### 3. MemoCacheRoots
| Aspect | Details |
|--------|---------|
| **Impl Block** | `src/backend/bytecode/memo_cache.rs:247` |
| **Impl Body** | Delegates to `GLOBAL_MEMO_CACHE.collect_all_values(roots)` |
| **Inherent Collector** | `MemoCache<MettaValue>::collect_all_values(&self, out: &mut Vec<V>)` at `src/backend/bytecode/memo_cache.rs:198` |
| **Global Accessor** | `pub fn global_memo_cache() -> &'static Arc<MemoCache<MettaValue>>` at `src/backend/bytecode/memo_cache.rs:273` |
| **Status** | ✓ Already delegates; ready for direct call |
| **Call in collect_global_anchors** | `global_memo_cache().collect_all_values(out)` |

### 4. BytecodeCacheRoots
| Aspect | Details |
|--------|---------|
| **Impl Block** | `src/backend/bytecode/cache.rs:242` |
| **Impl Body** | `let cache = BYTECODE_CACHE.read(); for (_, chunk) in cache.iter() { collect_chunk_constants(chunk, roots); }` |
| **Inherent Collector** | **NEEDS EXTRACTION** — no inherent method exists |
| **Global Accessor** | `static BYTECODE_CACHE: LazyLock<RwLock<LruCache<...>>>` at `src/backend/bytecode/cache.rs:99` |
| **Status** | ⚠️ Needs extract-and-delegate pattern (like TieredCacheRoots) |
| **Call in collect_global_anchors** | Will call: `collect_bytecode_cache_roots(out)` or similar (after extraction) |

### 5. CompilerAtomRoots
| Aspect | Details |
|--------|---------|
| **Impl Block** | `src/backend/bytecode/compiler/iterative.rs:49` |
| **Impl Body** | Intrinsic: checks 3 OnceLock<MettaValue> statics (ATOM_EQUALS, ATOM_PRINTLN, ATOM_IF); if Some, pushes to roots |
| **Inherent Collector** | **NEEDS EXTRACTION** — marker type with no inherent methods |
| **Global Accessor** | OnceLock statics: `ATOM_EQUALS`, `ATOM_PRINTLN`, `ATOM_IF` at `src/backend/bytecode/compiler/iterative.rs:15–24` |
| **Status** | ⚠️ Needs extraction into inherent method (e.g., `CompilerAtomRoots::collect_roots_into`) |
| **Call in collect_global_anchors** | Will call: `CompilerAtomRoots.collect_roots_into(out)` or similar (after extraction) |

---

## PER-ENV / PER-INSTANCE (b) — NOT collected globally

### 6. GenericEnvironmentShared<MettaValue>
| Aspect | Details |
|--------|---------|
| **Impl Block** | `src/backend/environment/core.rs:2268` |
| **Impl Body** | Delegates to `self.collect_roots_into(roots)` |
| **Inherent Collector** | `pub(crate) fn collect_roots_into(&self, roots: &mut Vec<MettaValue>)` at `src/backend/environment/core.rs:2168` |
| **Global Accessor** | None — per-env, accessed as E₀ from MettaState or caller |
| **Status** | ✓ Already delegates; handled separately as part of E₀ traversal |
| **Classification** | (b) Per-environment; NOT a global anchor |

### 7. MettaStateGcRoots
| Aspect | Details |
|--------|---------|
| **Impl Block** | `src/backend/models/metta_state.rs:25` |
| **Impl Body** | Intrinsic: locks `source` and `output` Mutex fields; extends roots with both vectors' contents |
| **Inherent Collector** | Intrinsic (no separate method) |
| **Global Accessor** | None — embedded in MettaState instances via Arc<MettaStateGcRoots> |
| **Registration** | Per-instance: called in `MettaState::from_parts(...)` at `src/backend/models/metta_state.rs:~150` |
| **Status** | ✓ Auto-registered on MettaState creation; no global anchor |
| **Classification** | (b) Per-instance; NOT a global anchor |

### 8. CurrentIterRootProvider
| Aspect | Details |
|--------|---------|
| **Impl Block** | `src/backend/eval/trampoline/current_iter_root.rs:88` |
| **Impl Body** | Intrinsic: loads `self.tagged` AtomicUsize (Acquire); if non-zero, reconstructs and pushes MettaValue |
| **Inherent Collector** | Intrinsic (no separate method) |
| **Global Accessor** | Thread-local `CURRENT_ITER_CELL: OnceCell<Arc<CurrentIterRootProvider>>` at `src/backend/eval/trampoline/current_iter_root.rs:120` |
| **Lazy Init** | `get_or_init()` at current_iter_root.rs:124; calls `register_root_provider()` on first access per thread |
| **Status** | ✓ Auto-registered per-thread; no global singleton |
| **Classification** | (b) Per-thread, auto-registered; NOT a global anchor |

---

## Summary: Ready for collect_global_anchors()

### Immediately usable (3 providers):
```rust
pub(crate) fn collect_global_anchors(out: &mut Vec<MettaValue>) {
    // TieredCacheRoots
    global_tiered_cache().collect_roots_into(out);
    
    // SpaceRegistryRoots
    global_space_registry().collect_all_gc_values(out);
    
    // MemoCacheRoots
    global_memo_cache().collect_all_values(out);
    
    // BytecodeCacheRoots (after extraction)
    // CompilerAtomRoots (after extraction)
}
```

### Requires extraction (2 providers):

1. **BytecodeCacheRoots** — extract impl logic into inherent method on a wrapper or direct function
2. **CompilerAtomRoots** — extract impl logic into inherent method on marker type

### NOT global (3 providers):
- **MettaStateGcRoots** — per-instance, auto-registered on creation
- **GenericEnvironmentShared<MettaValue>** — per-env, reachable from E₀
- **CurrentIterRootProvider** — per-thread, auto-registered on first use

---

## File Locations Reference

| File | Key Items |
|------|-----------|
| `src/backend/bytecode/tiered_cache.rs` | TieredCacheRoots impl (1929), TieredCache::collect_roots_into (1104), global_tiered_cache (1873) |
| `src/backend/bytecode/space_registry.rs` | SpaceRegistryRoots impl (165), SpaceRegistry::collect_all_gc_values (146), ensure registration (179) |
| `src/backend/bytecode/memo_cache.rs` | MemoCacheRoots impl (247), MemoCache::collect_all_values (198), global_memo_cache (273) |
| `src/backend/bytecode/cache.rs` | BytecodeCacheRoots impl (242), BYTECODE_CACHE static (99), helper collect_chunk_constants (~257) |
| `src/backend/bytecode/compiler/iterative.rs` | CompilerAtomRoots impl (49), ATOM_* OnceLock statics (15–24), ensure registration (71) |
| `src/backend/bytecode/mod.rs` | global_space_registry (285) |
| `src/backend/environment/core.rs` | GenericEnvironmentShared impl (2268), collect_roots_into (2168) |
| `src/backend/models/metta_state.rs` | MettaStateGcRoots impl (25), MettaState from_parts registration (~150) |
| `src/backend/eval/trampoline/current_iter_root.rs` | CurrentIterRootProvider impl (88), CURRENT_ITER_CELL thread-local (120), get_or_init (124) |

