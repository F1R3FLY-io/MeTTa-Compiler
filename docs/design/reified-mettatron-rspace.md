# Reified RSpaces Integration Plan: Expose MeTTaTron as ISpace

## Overview

Expose MeTTaTron's MORK-based Environment to Rholang (in f1r3node-reified-rspaces) as a Reified RSpace by implementing the `ISpace` trait. This replaces the current Par serialization bridge with direct, zero-copy API access via library linking.

**Communication Flow:**
- **Current:** Rholang → Par serialization → MeTTaTron → Par serialization → Rholang
- **New:** Rholang → `ISpace::produce/consume` → MorkSpaceAgent → direct response (same process)

**Key Goals:**
1. Implement `ISpace` trait for MORK Environment (zero-copy, in-process)
2. Expose MeTTaTron as a linked library for Rholang to consume
3. Deprecate/remove Par serialization bridge
4. Support prefix semantics for PathMap-based channel hierarchies

---

## Phase 1: ISpace Trait Implementation for MORK

**Objective:** Implement `ISpace<C, P, A, K>` trait from `f1r3node-reified-rspaces/rspace++/src/rspace/rspace_interface.rs` for MeTTaTron's MORK Environment.

### 1.1 Create MorkSpaceAgent Wrapper

**New file:** `src/backend/spaces/mork_space_agent.rs`

```rust
use rspace_plus_plus::rspace::rspace_interface::{
    ISpace, RSpaceResult, ContResult, MaybeConsumeResult, MaybeProduceResult,
    Checkpoint, SoftCheckpoint,
};
use crate::backend::environment::Environment;
use crate::backend::models::MettaValue;

/// MeTTa channel type - MettaValue representing a path in PathMap
pub type MettaChannel = MettaValue;

/// MeTTa pattern type - MettaValue with variable semantics ($x, _, etc.)
pub type MettaPattern = MettaValue;

/// MeTTa data type - arbitrary MettaValue
pub type MettaData = MettaValue;

/// MeTTa continuation - wraps evaluation context
pub struct MettaContinuation {
    pub body: MettaValue,
    pub bindings: HashMap<String, MettaValue>,
}

/// Wrapper exposing MORK Environment as ISpace
pub struct MorkSpaceAgent {
    /// The underlying MeTTaTron environment
    environment: Environment,
    /// Continuation storage (MORK stores data, not continuations)
    continuations: ContinuationRegistry,
    /// Name counter for gensym
    gensym_counter: AtomicU64,
}
```

### 1.2 Implement Core ISpace Methods

**File:** `src/backend/spaces/mork_space_agent.rs`

```rust
impl ISpace<MettaChannel, MettaPattern, MettaData, MettaContinuation> for MorkSpaceAgent {
    /// Store data at channel, wake matching continuations
    fn produce(
        &mut self,
        channel: MettaChannel,
        data: MettaData,
        persist: bool,
        priority: Option<usize>,
    ) -> Result<MaybeProduceResult<...>, RSpaceError> {
        // 1. Look for waiting continuations that match this data
        // 2. If match found: return continuation + matched data
        // 3. If no match: store data in Environment via add_to_space()
        // 4. Handle prefix semantics for PathMap channels
    }

    /// Search for data matching patterns, store continuation if no match
    fn consume(
        &mut self,
        channels: Vec<MettaChannel>,
        patterns: Vec<MettaPattern>,
        continuation: MettaContinuation,
        persist: bool,
        peeks: BTreeSet<i32>,
    ) -> Result<MaybeConsumeResult<...>, RSpaceError> {
        // 1. Use Environment::match_space() to find matching data
        // 2. If match found: return continuation + matched data
        // 3. If no match: store continuation in ContinuationRegistry
        // 4. Handle peek semantics (don't consume peeked channels)
    }

    fn get_data(&self, channel: &MettaChannel) -> Vec<Datum<MettaData>> {
        // Query Environment for data at channel
    }

    fn get_waiting_continuations(&self, channels: Vec<MettaChannel>)
        -> Vec<WaitingContinuation<MettaPattern, MettaContinuation>> {
        // Query ContinuationRegistry
    }

    fn get_joins(&self, channel: MettaChannel) -> Vec<Vec<MettaChannel>> {
        // Query join patterns from ContinuationRegistry
    }

    // Checkpoint methods delegate to Environment's CoW semantics
    fn create_checkpoint(&mut self) -> Result<Checkpoint, RSpaceError>;
    fn create_soft_checkpoint(&mut self) -> SoftCheckpoint<...>;
    fn revert_to_soft_checkpoint(&mut self, cp: SoftCheckpoint<...>) -> Result<(), RSpaceError>;
    fn reset(&mut self, root: &Blake2b256Hash) -> Result<(), RSpaceError>;

    // Replay methods (for deterministic replay)
    fn rig_and_reset(&mut self, start_root: Blake2b256Hash, log: Log) -> Result<(), RSpaceError>;
    fn rig(&self, log: Log) -> Result<(), RSpaceError>;
    fn check_replay_data(&self) -> Result<(), RSpaceError>;
    fn is_replay(&self) -> bool;
    fn update_produce(&mut self, produce: Produce);
}
```

### 1.3 Continuation Registry

**New file:** `src/backend/spaces/continuations.rs`

MORK Environment stores data (facts, rules) but not waiting continuations. We need a separate registry:

```rust
use dashmap::DashMap;

pub struct ContinuationRegistry {
    /// Map: channel combination -> waiting continuations
    waiting: DashMap<Vec<MettaChannel>, Vec<WaitingContinuation<MettaPattern, MettaContinuation>>>,

    /// Join index: channel -> channel combinations that include it
    joins: DashMap<MettaChannel, Vec<Vec<MettaChannel>>>,

    /// Installed (persistent) continuations
    installed: DashMap<Vec<MettaChannel>, Vec<WaitingContinuation<MettaPattern, MettaContinuation>>>,
}

impl ContinuationRegistry {
    pub fn put_continuation(&mut self, channels: &[MettaChannel], wc: WaitingContinuation<...>);
    pub fn find_and_remove(&mut self, channels: &[MettaChannel]) -> Option<WaitingContinuation<...>>;
    pub fn get_waiting(&self, channels: &[MettaChannel]) -> Vec<WaitingContinuation<...>>;
    pub fn add_join(&mut self, channel: &MettaChannel, join: Vec<MettaChannel>);
    pub fn remove_join(&mut self, channel: &MettaChannel, join: &[MettaChannel]);
}
```

---

## Phase 2: PathMap Prefix Semantics

**Objective:** Implement hierarchical channel matching per Reified RSpaces design.

From design doc: Data at `@[0,1,2]` consumed at prefix `@[0,1]` returns `[2, "data"]` (suffix key prepended).

### 2.1 Prefix Matching in produce()

```rust
fn produce(&mut self, channel: MettaChannel, data: MettaData, ...) -> Result<...> {
    // Add to Environment at exact path
    self.environment.add_to_space(&data)?;

    // Check continuations at all prefix paths
    let path = channel.to_path_bytes();
    for prefix_len in 0..=path.len() {
        let prefix = &path[..prefix_len];
        let suffix = &path[prefix_len..];

        if let Some(wcs) = self.continuations.get_waiting_at_prefix(prefix) {
            for wc in wcs {
                if self.pattern_matches(&wc.patterns, &data) {
                    // Found match - wrap data with suffix key
                    let wrapped = wrap_with_suffix_key(suffix, &data);
                    return Ok(Some((cont_result, vec![rspace_result_with_suffix], produce)));
                }
            }
        }
    }

    // No match - data stored, return None
    Ok(None)
}
```

### 2.2 Prefix Matching in consume()

```rust
fn consume(&mut self, channels: Vec<MettaChannel>, patterns: Vec<MettaPattern>, ...) -> Result<...> {
    // For each channel, search at path AND all descendants (prefix semantics)
    for (i, (channel, pattern)) in channels.iter().zip(patterns.iter()).enumerate() {
        let matches = self.environment.match_space_with_prefix(pattern, channel)?;

        for (data, suffix_key) in matches {
            // Wrap matched data with suffix key if not exact match
            let wrapped = if suffix_key.is_empty() {
                data.clone()
            } else {
                wrap_with_suffix_key(&suffix_key, &data)
            };

            // Check if all channels have matches...
        }
    }

    // If no match, store continuation
    self.continuations.put_continuation(&channels, wc);
    Ok(None)
}
```

---

## Phase 3: Library Interface for Rholang

**Objective:** Expose MeTTaTron as a library that Rholang can link against.

### 3.1 Public Library API

**File:** `src/lib.rs`

Add exports for Rholang integration:

```rust
// Re-export ISpace implementation
pub mod spaces {
    pub use crate::backend::spaces::mork_space_agent::{
        MorkSpaceAgent,
        MettaChannel,
        MettaPattern,
        MettaData,
        MettaContinuation,
    };
}

// Factory function for creating MorkSpaceAgent
pub fn create_mork_space() -> MorkSpaceAgent {
    MorkSpaceAgent::new()
}

pub fn create_mork_space_with_env(env: Environment) -> MorkSpaceAgent {
    MorkSpaceAgent::with_environment(env)
}
```

### 3.2 Cargo.toml Configuration

**File:** `Cargo.toml`

```toml
[lib]
name = "mettatron"
crate-type = ["lib", "cdylib", "staticlib"]  # Support various linking modes

[dependencies]
# Add Reified RSpaces dependency for ISpace trait
rspace_plus_plus = { path = "../f1r3node-reified-rspaces/rspace++" }

[features]
default = ["async"]
async = ["tokio"]
# Feature for building without Rholang deps (standalone mode)
standalone = []
```

### 3.3 Integration in Rholang

**In f1r3node-reified-rspaces:** (for reference, not edited)

```rust
// Rholang can create and use MeTTaTron space
use mettatron::{create_mork_space, MorkSpaceAgent};
use rspace_plus_plus::rspace::rspace_interface::ISpace;

fn use_metta_space() {
    let mut metta_space: MorkSpaceAgent = create_mork_space();

    // Direct ISpace method calls - no serialization
    let result = metta_space.produce(channel, data, false, None)?;
    let result = metta_space.consume(channels, patterns, cont, false, peeks)?;
}
```

---

## Phase 4: Deprecate Par Serialization Bridge

**Objective:** Remove old serialization-based integration.

### 4.1 Deprecate pathmap_par_integration.rs

**File:** `src/pathmap_par_integration.rs`

```rust
#[deprecated(since = "0.x.0", note = "Use MorkSpaceAgent ISpace implementation instead")]
pub fn metta_value_to_par(value: &MettaValue) -> Par { ... }

#[deprecated(since = "0.x.0", note = "Use MorkSpaceAgent ISpace implementation instead")]
pub fn par_to_metta_value(par: &Par) -> Result<MettaValue, String> { ... }

#[deprecated(since = "0.x.0", note = "Use MorkSpaceAgent ISpace implementation instead")]
pub fn environment_to_par(env: &Environment) -> Par { ... }
```

### 4.2 Update rholang_integration.rs

**File:** `src/rholang_integration.rs`

Keep `run_state` and `run_state_async` for standalone MeTTa evaluation, but mark Par-dependent code paths as deprecated:

```rust
/// Run MeTTa state evaluation (standalone mode)
/// For Rholang integration, use MorkSpaceAgent::produce/consume directly
pub fn run_state(accumulated: MettaState, compiled: MettaState) -> Result<MettaState, String> {
    // This remains for standalone MeTTa CLI usage
    // Rholang should use ISpace methods directly
}
```

---

## Phase 5: Pattern Matching Integration

**Objective:** Connect ISpace pattern matching to MORK's existing match infrastructure.

### 5.1 Pattern Matching Adapter

MORK Environment already has `match_space()` which does pattern matching. We need to adapt this for ISpace semantics:

```rust
impl MorkSpaceAgent {
    /// Match pattern against data using MORK's pattern matcher
    fn pattern_matches(&self, pattern: &MettaPattern, data: &MettaData) -> bool {
        // Use existing pattern matching from Environment
        !self.environment.match_pattern(pattern, data).is_empty()
    }

    /// Match with bindings extraction
    fn match_with_bindings(&self, pattern: &MettaPattern, data: &MettaData)
        -> Option<HashMap<String, MettaValue>> {
        self.environment.match_pattern(pattern, data)
            .into_iter()
            .next()
    }
}
```

### 5.2 Variable Binding Propagation

When a consume matches, bindings need to propagate to the continuation:

```rust
fn execute_continuation(
    continuation: MettaContinuation,
    bindings: HashMap<String, MettaValue>,
) -> MettaData {
    // Apply bindings to continuation body
    let bound_body = substitute_bindings(&continuation.body, &bindings);
    // Could trigger MeTTa evaluation if needed
    bound_body
}
```

---

## Critical Files Summary

| File | Action | Purpose |
|------|--------|---------|
| `src/backend/spaces/mod.rs` | Create | Module organization |
| `src/backend/spaces/mork_space_agent.rs` | Create | ISpace impl for MORK |
| `src/backend/spaces/continuations.rs` | Create | Continuation storage |
| `src/lib.rs` | Modify | Export MorkSpaceAgent, factory functions |
| `src/pathmap_par_integration.rs` | Modify | Deprecate Par conversions |
| `src/rholang_integration.rs` | Modify | Document ISpace as preferred integration |
| `Cargo.toml` | Modify | Add rspace++ dependency, library config |

**Files in f1r3node-reified-rspaces to update:** (for reference)
- Register MorkSpaceAgent as available space type
- Add MeTTaTron as workspace dependency

---

## Dependencies

**Add to `Cargo.toml`:**
```toml
[dependencies]
rspace_plus_plus = { path = "../f1r3node-reified-rspaces/rspace++" }
dashmap = "6"  # For ContinuationRegistry
```

---

## Verification Plan

### Unit Tests

1. **MorkSpaceAgent::produce()** - data storage, continuation wake-up
2. **MorkSpaceAgent::consume()** - pattern matching, continuation storage
3. **Prefix semantics** - suffix key generation, hierarchical matching
4. **Checkpoint/replay** - state snapshot and restoration
5. **ContinuationRegistry** - add/remove/lookup operations

### Integration Tests

1. **Round-trip:** Rholang produce → MeTTa consume → Rholang receive
2. **Pattern matching:** Variable binding propagation
3. **Prefix matching:** Data at `@[0,1,2]` visible at `@[0,1]`
4. **Persistence:** Persistent produces/consumes stick
5. **Joins:** Multi-channel patterns

### Performance Benchmarks

```bash
# Compare old vs new integration
cargo bench --bench rholang_integration

# Metrics to measure:
# - Latency: produce/consume round-trip time
# - Throughput: operations per second
# - Memory: eliminated serialization buffers
```

### End-to-End Validation

```rust
// In Rholang test
#[test]
fn test_metta_space_integration() {
    let mut metta_space = mettatron::create_mork_space();

    // Add facts via ISpace
    metta_space.produce(
        MettaValue::Atom("channel".into()),
        MettaValue::SExpr(vec![
            MettaValue::Atom("Parent".into()),
            MettaValue::Atom("Tom".into()),
            MettaValue::Atom("Bob".into()),
        ]),
        false,
        None,
    ).unwrap();

    // Query via ISpace
    let result = metta_space.consume(
        vec![MettaValue::Atom("channel".into())],
        vec![MettaValue::SExpr(vec![
            MettaValue::Atom("Parent".into()),
            MettaValue::Atom("$parent".into()),
            MettaValue::Atom("Bob".into()),
        ])],
        continuation,
        false,
        BTreeSet::new(),
    ).unwrap();

    assert!(result.is_some());
}
```

---

## Architectural Decisions

| Question | Decision | Rationale |
|----------|----------|-----------|
| Communication mechanism? | Direct library linking (same process) | Zero-copy, maximum efficiency |
| Type mappings? | MettaValue for all (Channel, Pattern, Data) | Natural fit, no conversion needed |
| Continuation storage? | Separate ContinuationRegistry | MORK stores data, not continuations |
| Pattern matching? | Reuse Environment::match_space() | Existing, tested implementation |
| Prefix semantics? | Implement in MorkSpaceAgent | Required by Reified RSpaces design |
| Checkpoint support? | Delegate to Environment CoW | Existing fork_for_nondeterminism() |
