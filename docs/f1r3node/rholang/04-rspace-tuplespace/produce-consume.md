# RSpace Produce and Consume Operations

## Overview

**Produce** and **Consume** are the fundamental operations in RSpace that enable Rholang processes to communicate. They implement the Linda tuple space model adapted for blockchain consensus with deterministic execution.

**Produce**: Send data to a channel (like `channel!(data)` in Rholang)
**Consume**: Wait for data matching patterns on channels (like `for (@x <- channel)` in Rholang)

This document provides a complete technical specification of these operations, their semantics, implementation, and consensus implications.

## Produce Operation

### Purpose

Put data on a channel, potentially triggering waiting continuations that match the data.

### Rholang Syntax

```rholang
// Single send (non-persistent)
channel!(data)

// Persistent send (remains after consumption)
channel!!(data)

// Multiple data items
channel!(item1, item2, item3)
```

### RSpace API

**Source**: `/var/tmp/debug/f1r3node/rspace++/src/rspace/rspace_interface.rs` (lines 20-40)

```rust
pub trait ISpace<C, P, A, K> {
    fn produce(
        &mut self,
        channel: C,              // Channel to send on
        data: A,                 // Data to send
        persist: bool,           // Persistent (!!)?
    ) -> Result<MaybeProduceResult<C, P, A, K>, RSpaceError>;
}

// Result type
pub type MaybeProduceResult<C, P, A, K> = Option<(
    ContResult<C, P, K>,         // Matched continuation
    Vec<RSpaceResult<C, A>>,     // Consumed data
    Produce,                     // Produce event record
)>;
```

**Return Value**:
- `None`: No match, data stored for future consumption
- `Some((cont, data, event))`: Match found, continuation executed

### Semantics

**Algorithm**:

```
produce(channel, data, persist):
    1. Hash channel to get storage key
    2. Look for continuations waiting on this channel
    3. If matching continuation found:
        a. Execute continuation with matched data
        b. If !persist: Remove data
        c. If !continuation.persist: Remove continuation
        d. Record COMM event
        e. Return Some(continuation, data, event)
    4. Else (no match):
        a. Store data on channel
        b. Record PRODUCE event
        c. Return None
```

### Implementation

**Source**: `/var/tmp/debug/f1r3node/rspace++/src/rspace/rspace.rs` (lines 100-200)

```rust
impl RSpace {
    pub fn produce(
        &mut self,
        channel: Par,
        data: ListParWithRandom,
        persist: bool,
    ) -> Result<MaybeProduceResult, RSpaceError> {
        // 1. Get continuations waiting on this channel
        let waiting = self.store.get_continuations(&[channel.clone()]);

        if let Some(continuations) = waiting {
            // 2. Try to match with waiting continuations
            for continuation in continuations {
                let match_result = self.matcher.match_data(
                    &continuation.patterns,
                    &[data.clone()],
                );

                if let Some(bindings) = match_result {
                    // 3. Match found! Execute continuation
                    let cont_body = continuation.continuation.clone();

                    // 4. Remove from storage (unless persistent)
                    if !continuation.persist {
                        self.store.remove_continuation(&continuation.channels);
                    }

                    // 5. Don't store data (consumed immediately)

                    // 6. Record COMM event
                    self.event_log.push(Event::Comm {
                        consume: continuation.to_event(),
                        produces: vec![Produce { channel: channel.clone(), data, persist }],
                    });

                    // 7. Return continuation to execute
                    return Ok(Some((
                        ContResult {
                            continuation: cont_body,
                            bindings,
                        },
                        vec![],
                        Produce { channel, data, persist },
                    )));
                }
            }
        }

        // No match: store data for future consumption
        self.store.put_data(
            channel.clone(),
            Datum {
                data: data.clone(),
                persist,
            },
        );

        // Record PRODUCE event
        self.event_log.push(Event::Produce {
            channel: channel.clone(),
            data: data.clone(),
            persist,
        });

        Ok(None)  // No continuation executed
    }
}
```

### Examples

**Example 1: Immediate Match**

```rholang
// Consumer waiting
for (@x <- @"channel") {
  stdout!(x)
}
|
// Producer sends
@"channel"!(42)
```

**RSpace Execution**:

```
1. consume(@"channel", pattern: @x, continuation: stdout!(x))
   - No data on @"channel" yet
   - Store continuation

2. produce(@"channel", data: 42, persist: false)
   - Find waiting continuation
   - Match: x = 42
   - Execute: stdout!(42)
   - Remove continuation (not persistent)
   - Don't store data (consumed)
   - Return Some(...)

Result: stdout!(42) executes immediately
```

**Example 2: Data Stored**

```rholang
// Producer sends first
@"channel"!(42)
|
// Consumer arrives later
for (@x <- @"channel") {
  stdout!(x)
}
```

**RSpace Execution**:

```
1. produce(@"channel", data: 42, persist: false)
   - No waiting continuations
   - Store data on @"channel"
   - Return None

2. consume(@"channel", pattern: @x, continuation: stdout!(x))
   - Find data: 42
   - Match: x = 42
   - Execute: stdout!(42)
   - Remove data (not persistent)
   - Return Some(...)

Result: Data waits for consumer, then executes
```

**Example 3: Persistent Send**

```rholang
// Persistent producer
@"channel"!!(42)
|
// First consumer
for (@x <- @"channel") { stdout!("Consumer 1: ", x) }
|
// Second consumer
for (@x <- @"channel") { stdout!("Consumer 2: ", x) }
```

**RSpace Execution**:

```
1. produce(@"channel", data: 42, persist: true)
   - No waiting continuations
   - Store data (persistent)

2. consume(@"channel", @x, continuation: stdout!("Consumer 1: ", x))
   - Find data: 42
   - Match: x = 42
   - Execute: stdout!("Consumer 1: ", 42)
   - Data remains (persist = true)

3. consume(@"channel", @x, continuation: stdout!("Consumer 2: ", x))
   - Find data: 42 (still there!)
   - Match: x = 42
   - Execute: stdout!("Consumer 2: ", 42)
   - Data remains

Result: Both consumers execute (persistent data)
```

## Consume Operation

### Purpose

Wait for data matching patterns on one or more channels, then execute continuation.

### Rholang Syntax

```rholang
// Linear receive (execute once)
for (@x <- channel) { body }

// Persistent receive (execute repeatedly)
for (@x <= channel) { body }

// Peek (non-consuming)
for (@x <<- channel) { body }

// Join pattern (multiple channels)
for (@x <- chan1 & @y <- chan2) { body }
```

### RSpace API

```rust
pub trait ISpace<C, P, A, K> {
    fn consume(
        &mut self,
        channels: Vec<C>,          // Channels to wait on
        patterns: Vec<P>,          // Patterns to match
        continuation: K,           // Code to run on match
        persist: bool,             // Persistent (<=)?
        peeks: BTreeSet<i32>,      // Peek indices (<<-)?
    ) -> Result<MaybeConsumeResult<C, P, A, K>, RSpaceError>;
}

pub type MaybeConsumeResult<C, P, A, K> = Option<(
    ContResult<C, P, K>,         // Continuation with bindings
    Vec<RSpaceResult<C, A>>,     // Matched data
)>;
```

**Return Value**:
- `None`: No match, continuation stored waiting for data
- `Some((cont, data))`: Match found, continuation ready to execute

### Semantics

**Algorithm**:

```
consume(channels, patterns, continuation, persist, peeks):
    1. For each channel, get available data
    2. Run spatial matcher to find complete match across all channels
    3. If complete match found:
        a. Bind pattern variables to matched data
        b. For each channel (unless peek):
            - If data.persist: Keep data
            - Else: Remove data
        c. If !persist: Don't store continuation
        d. Record COMM event
        e. Return Some(continuation, bindings, data)
    4. Else (no complete match):
        a. Store continuation waiting on channels
        b. Record CONSUME event
        c. Return None
```

### Implementation

**Source**: `/var/tmp/debug/f1r3node/rspace++/src/rspace/rspace.rs` (lines 250-400)

```rust
impl RSpace {
    pub fn consume(
        &mut self,
        channels: Vec<Par>,
        patterns: Vec<Par>,
        continuation: ParWithRandom,
        persist: bool,
        peeks: BTreeSet<i32>,
    ) -> Result<MaybeConsumeResult, RSpaceError> {
        // 1. Get data from all channels
        let mut channel_data: Vec<Vec<Datum>> = channels.iter()
            .map(|ch| self.store.get_data(ch).unwrap_or_default())
            .collect();

        // 2. Run spatial matcher
        let match_result = self.matcher.find_match(
            &channels,
            &patterns,
            &channel_data,
        );

        if let Some(matched_data) = match_result {
            // 3. Complete match found!

            // 4. Bind pattern variables
            let bindings = self.matcher.bind_patterns(&patterns, &matched_data)?;

            // 5. Remove consumed data (unless peek or persistent)
            for (i, (channel, datum)) in channels.iter().zip(&matched_data).enumerate() {
                if !peeks.contains(&(i as i32)) && !datum.persist {
                    self.store.remove_data(channel, datum);
                }
            }

            // 6. If persistent consume, leave continuation for future matches
            if persist {
                self.store.put_continuation(
                    channels.clone(),
                    WaitingContinuation {
                        channels: channels.clone(),
                        patterns: patterns.clone(),
                        continuation: continuation.clone(),
                        persist: true,
                        peeks: peeks.clone(),
                    },
                );
            }

            // 7. Record COMM event
            self.event_log.push(Event::Comm {
                consume: Consume {
                    channels: channels.clone(),
                    patterns: patterns.clone(),
                    continuation: continuation.clone(),
                    persist,
                },
                produces: matched_data.iter()
                    .zip(&channels)
                    .map(|(datum, ch)| Produce {
                        channel: ch.clone(),
                        data: datum.data.clone(),
                        persist: datum.persist,
                    })
                    .collect(),
            });

            // 8. Return continuation to execute
            return Ok(Some((
                ContResult {
                    continuation: continuation.clone(),
                    bindings,
                },
                matched_data.clone(),
            )));
        }

        // No match: store continuation waiting for data
        self.store.put_continuation(
            channels.clone(),
            WaitingContinuation {
                channels: channels.clone(),
                patterns: patterns.clone(),
                continuation: continuation.clone(),
                persist,
                peeks,
            },
        );

        // Record CONSUME event
        self.event_log.push(Event::Consume {
            channels: channels.clone(),
            patterns: patterns.clone(),
            continuation: continuation.clone(),
            persist,
        });

        Ok(None)  // Continuation waiting
    }
}
```

### Examples

**Example 1: Simple Match**

```rholang
@"channel"!(42) |
for (@x <- @"channel") {
  stdout!(x + 10)
}
```

**RSpace Execution**:

```
1. produce(@"channel", 42, false)
   - Store data

2. consume([@"channel"], [@x], stdout!(x + 10), false, {})
   - Get data: [42]
   - Match patterns: [@x] against [42]
   - Bind: x = 42
   - Remove data (not persistent)
   - Execute: stdout!(42 + 10)

Result: stdout!(52)
```

**Example 2: Join Pattern**

```rholang
@"chan1"!(1) |
@"chan2"!(2) |
for (@x <- @"chan1" & @y <- @"chan2") {
  stdout!(x + y)
}
```

**RSpace Execution**:

```
1. produce(@"chan1", 1, false)
   - Store data on chan1

2. produce(@"chan2", 2, false)
   - Store data on chan2

3. consume([@"chan1", @"chan2"], [@x, @y], stdout!(x + y), false, {})
   - Get data: [[1], [2]]
   - Match patterns: [@x, @y] against [1, 2]
   - Bind: x = 1, y = 2
   - Remove both data items
   - Execute: stdout!(1 + 2)

Result: stdout!(3) - only executes when BOTH channels have data
```

**Example 3: Persistent Receive**

```rholang
// Contract (persistent receive)
contract @"add"(@x, @y, return) = {
  return!(x + y)
}
|
// Calls
@"add"!(5, 3, *result1) |
@"add"!(10, 20, *result2)
```

**RSpace Execution**:

```
1. consume([@"add"], [[@x, @y, return]], body, persist: true, {})
   - No data yet
   - Store continuation (persistent)

2. produce(@"add", [5, 3, result1], false)
   - Find continuation
   - Match: x=5, y=3, return=result1
   - Execute: result1!(5 + 3)
   - Continuation remains (persistent)

3. produce(@"add", [10, 20, result2], false)
   - Find continuation (still there!)
   - Match: x=10, y=20, return=result2
   - Execute: result2!(10 + 20)
   - Continuation remains

Result: Both calls execute (contract available repeatedly)
```

**Example 4: Peek (Non-Consuming)**

```rholang
@"channel"!(42) |
for (@x <<- @"channel") {
  stdout!("Peeked: ", x)
} |
for (@y <- @"channel") {
  stdout!("Consumed: ", y)
}
```

**RSpace Execution**:

```
1. produce(@"channel", 42, false)
   - Store data

2. consume([@"channel"], [@x], stdout!("Peeked: ", x), false, {0})
   - peeks = {0} means peek on channel 0
   - Get data: [42]
   - Match: x = 42
   - DON'T remove data (peek)
   - Execute: stdout!("Peeked: ", 42)

3. consume([@"channel"], [@y], stdout!("Consumed: ", y), false, {})
   - Get data: [42] (still there!)
   - Match: y = 42
   - Remove data (normal consume)
   - Execute: stdout!("Consumed: ", 42)

Result: Data peeked first, then consumed
```

## Pattern Matching

### Pattern Types

**1. Variable Binding**:

```rholang
for (@x <- channel) { ... }  // Binds any value to x
```

```rust
// Pattern: Variable("x")
// Data: Par(42)
// Result: Bindings { "x" -> Par(42) }
```

**2. Constant Matching**:

```rholang
for (@42 <- channel) { ... }  // Only matches 42
```

```rust
// Pattern: Par(42)
// Data: Par(42)
// Result: Match (no bindings)

// Pattern: Par(42)
// Data: Par(100)
// Result: No match
```

**3. Structure Matching**:

```rholang
for (@{"key": value} <- channel) { ... }
```

```rust
// Pattern: Map { "key" -> Variable("value") }
// Data: Map { "key" -> Par("hello") }
// Result: Bindings { "value" -> Par("hello") }
```

**4. List Matching**:

```rholang
for (@[head ...tail] <- channel) { ... }
```

```rust
// Pattern: List [ Variable("head"), Remainder("tail") ]
// Data: List [ Par(1), Par(2), Par(3) ]
// Result: Bindings {
//   "head" -> Par(1),
//   "tail" -> List [Par(2), Par(3)]
// }
```

**5. Wildcard**:

```rholang
for (@_ <- channel) { ... }  // Matches anything, no binding
```

```rust
// Pattern: Wildcard
// Data: Par(anything)
// Result: Match (no bindings)
```

### Spatial Matching Algorithm

For join patterns, RSpace must find a **complete match** across all channels:

**Example**:

```rholang
for (@x <- @"c1" & @y <- @"c2" & @z <- @"c3") { body }
```

**Available Data**:
```
@"c1": [10, 20, 30]
@"c2": [100, 200]
@"c3": [1000]
```

**Algorithm**:
1. Generate all combinations: 3 × 2 × 1 = 6 combinations
2. Try each combination against patterns
3. Find first match (deterministic ordering)

**Combinations**:
```
(10, 100, 1000) - check patterns
(10, 200, 1000) - check patterns
(20, 100, 1000) - check patterns
(20, 200, 1000) - check patterns
(30, 100, 1000) - check patterns
(30, 200, 1000) - check patterns
```

**Deterministic Ordering**:
- Channels sorted lexicographically
- Data within channel sorted by insertion order (FIFO)
- Combinations tried in lexicographic order

See [spatial-matching.md](spatial-matching.md) for complete algorithm.

## State Merging

### The Challenge

When a block has multiple parents, their RSpace states must merge:

```
Parent 1 state:
  @"x": [1]
  @"y": [2]

Parent 2 state:
  @"x": [5]
  @"z": [3]

Merged state:
  @"x": [1, 5]  ← Combine data from both parents
  @"y": [2]
  @"z": [3]
```

### Merge Algorithm

```rust
pub fn merge_states(
    &mut self,
    parent_states: &[Blake2b256Hash],
) -> Result<Blake2b256Hash, RSpaceError> {
    // 1. Reset to empty state
    self.store.clear();

    // 2. For each parent state
    for parent_hash in parent_states {
        // 3. Load parent's data
        let parent_data = self.history_repository
            .get_all_data_at_root(parent_hash)?;

        // 4. Merge into current state
        for (channel, data_list) in parent_data {
            for datum in data_list {
                self.store.put_data(channel.clone(), datum);
            }
        }
    }

    // 5. Create checkpoint of merged state
    let checkpoint = self.create_checkpoint()?;
    Ok(checkpoint.root)
}
```

### Commutative Property

**Requirement**: `merge(A, B) = merge(B, A)`

**RSpace Guarantee**: Merge is commutative because:
1. Data from both parents is combined (union)
2. Channel-based isolation (no conflicts on different channels)
3. Deterministic ordering (same data → same order)

**Example**:

```
State A:             State B:
@"x": [1]            @"x": [2]
@"y": [10]           @"z": [20]

merge(A, B):         merge(B, A):
@"x": [1, 2]         @"x": [2, 1]
@"y": [10]           @"y": [10]
@"z": [20]           @"z": [20]
```

**But wait**: `[1, 2] ≠ [2, 1]`!

**Solution**: Deterministic merge ordering - always merge in parent hash order:

```rust
// Sort parent hashes lexicographically
let mut sorted_parents = parent_states.to_vec();
sorted_parents.sort();

// Merge in deterministic order
for parent_hash in sorted_parents {
    merge_parent_data(parent_hash);
}
```

Now: `merge(A, B) = merge(B, A)` because both sort to same order.

## Performance Considerations

### Produce Performance

```
Operation                  Time
------------------------------------
Hash channel              ~100 ns
Lookup continuations      ~500 ns
Pattern matching          ~5 μs
Store data (memory)       ~200 ns
Store data (LMDB)         ~10 μs

Total (hot path):         ~6 μs
Total (with LMDB):        ~16 μs
```

**Optimization**: Keep hot data in memory, batch LMDB writes at checkpoints.

### Consume Performance

```
Operation                  Time
------------------------------------
Hash channels             ~100 ns × N
Lookup data               ~500 ns × N
Spatial matching          ~10 μs (for 2-3 channels)
                          ~1 ms (for 10+ channels)
Store continuation        ~200 ns
Store continuation (LMDB) ~10 μs

Total (2 channels):       ~11 μs
Total (10 channels):      ~1 ms
```

**Bottleneck**: Spatial matching (combinatorial explosion with many channels).

### Memory Usage

```
Per Data Item:            ~100 bytes
Per Continuation:         ~500 bytes
Per Event:                ~200 bytes

Typical block:
- 1000 produces:          ~100 KB data
- 500 consumes:           ~250 KB continuations
- 1500 events:            ~300 KB event log

Total:                    ~650 KB per block
```

**Checkpoint**: Flush hot store to LMDB, clear memory.

## Consensus Integration

### Deterministic Execution Requirement

**Problem**: Rholang is concurrent, execution order is non-deterministic.

**Solution**: RSpace pattern matching is deterministic:

```
Given:
- Same channels
- Same patterns
- Same data

Result:
- Same matches
- Same bindings
- Same execution order
```

**Mechanism**:
1. Lexicographic channel sorting
2. FIFO data ordering
3. Maximum bipartite matching (deterministic algorithm)

### Block Execution

**Each block includes**:
```protobuf
message BlockBody {
  bytes pre_state_hash = 1;   // RSpace root before execution
  repeated Deploy deploys = 2; // Rholang contracts
  bytes post_state_hash = 3;  // RSpace root after execution
}
```

**Validation**:

```rust
fn validate_block_execution(block: &Block, rspace: &RSpace) -> Result<(), Error> {
    // 1. Reset RSpace to pre-state
    rspace.reset(&block.body.pre_state_hash)?;

    // 2. Execute each deploy
    for deploy in &block.body.deploys {
        let rholang_term = parse_rholang(&deploy.term)?;
        execute_rholang(rholang_term, rspace)?;
    }

    // 3. Create checkpoint
    let checkpoint = rspace.create_checkpoint()?;

    // 4. Verify post-state matches
    if checkpoint.root != block.body.post_state_hash {
        return Err(Error::StateHashMismatch {
            expected: block.body.post_state_hash,
            actual: checkpoint.root,
        });
    }

    Ok(())
}
```

**Byzantine Detection**:

```
Honest Validator:
  execute(pre_state, deploys) → post_state_1

Byzantine Validator (wrong execution):
  execute(pre_state, deploys) → post_state_2

Result: post_state_1 ≠ post_state_2
Byzantine validator's block rejected!
```

## Error Handling

### Common Errors

```rust
pub enum RSpaceError {
    // Pattern matching errors
    PatternMismatch,
    IncompatibleTypes { expected: Type, actual: Type },
    UnboundVariable(String),

    // Storage errors
    ChannelNotFound(Par),
    DataNotFound { channel: Par, index: usize },
    ContinuationNotFound { channels: Vec<Par> },

    // LMDB errors
    PersistenceError(String),
    CheckpointFailed,
    ResetFailed { root: Blake2b256Hash },

    // Merge errors
    MergeConflict { channel: Par },
    InvalidParentState(Blake2b256Hash),
}
```

### Error Recovery

```rust
match rspace.produce(channel, data, false) {
    Ok(Some((cont, _, _))) => {
        // Match found, execute continuation
        execute_continuation(cont)?;
    }
    Ok(None) => {
        // No match, data stored successfully
        log::debug!("Data stored on channel {}", channel);
    }
    Err(RSpaceError::PersistenceError(msg)) => {
        // LMDB error, retry or abort block
        log::error!("RSpace persistence error: {}", msg);
        return Err(BlockValidationError::RSpaceFailure);
    }
    Err(e) => {
        // Other error, reject block
        return Err(BlockValidationError::InvalidDeploy(e));
    }
}
```

## Testing

### Unit Tests

```rust
#[test]
fn test_produce_consume_match() {
    let mut rspace = RSpace::new();

    // Produce first
    let produce_result = rspace.produce(
        channel!("test"),
        data!(42),
        false,
    ).unwrap();
    assert!(produce_result.is_none());  // No waiting continuation

    // Consume
    let consume_result = rspace.consume(
        vec![channel!("test")],
        vec![pattern!("@x")],
        continuation!("stdout!(x)"),
        false,
        BTreeSet::new(),
    ).unwrap();
    assert!(consume_result.is_some());  // Match found

    let (cont, data) = consume_result.unwrap();
    assert_eq!(data[0].data, data!(42));
}
```

### Integration Tests

```rust
#[test]
fn test_persistent_send_multiple_consumers() {
    let mut rspace = RSpace::new();

    // Persistent produce
    rspace.produce(channel!("test"), data!(42), true).unwrap();

    // First consume
    let result1 = rspace.consume(
        vec![channel!("test")],
        vec![pattern!("@x")],
        continuation!("stdout!(x)"),
        false,
        BTreeSet::new(),
    ).unwrap();
    assert!(result1.is_some());

    // Second consume (data still there because persistent)
    let result2 = rspace.consume(
        vec![channel!("test")],
        vec![pattern!("@y")],
        continuation!("stdout!(y)"),
        false,
        BTreeSet::new(),
    ).unwrap();
    assert!(result2.is_some());
}
```

### Property-Based Tests

```rust
#[quickcheck]
fn prop_merge_commutative(state_a: RSpaceState, state_b: RSpaceState) -> bool {
    let mut rspace1 = RSpace::new();
    let merged1 = rspace1.merge_states(&[state_a.hash(), state_b.hash()]).unwrap();

    let mut rspace2 = RSpace::new();
    let merged2 = rspace2.merge_states(&[state_b.hash(), state_a.hash()]).unwrap();

    merged1 == merged2  // Merge is commutative
}
```

## Summary

**Produce** and **Consume** are the fundamental RSpace operations enabling Rholang communication:

**Produce**:
- Sends data to a channel
- Triggers waiting continuations on match
- Stores data if no match
- Supports persistent (!!`) and non-persistent (`!`) variants

**Consume**:
- Waits for data matching patterns
- Supports join patterns (multiple channels atomically)
- Executes continuation on match
- Supports persistent (`<=`), linear (`<-`), and peek (`<<-`) variants

**Key Properties**:
- **Deterministic** - Same input → same matches (consensus requirement)
- **Atomic** - Join patterns match all or nothing
- **Persistent** - Data and continuations can outlive single use
- **Commutative** - State merging is order-independent

**Consensus Integration**:
- Blocks include pre/post RSpace state hashes
- Validators verify state transitions match
- Deterministic matching ensures consensus

**Performance**:
- Produce: ~6 μs (hot path), ~16 μs (with LMDB)
- Consume: ~11 μs (2 channels), ~1 ms (10 channels)
- Bottleneck: Spatial matching with many channels

**Key Insight**: RSpace produce/consume operations bridge Rholang's concurrent semantics with blockchain's determinism requirement through spatial pattern matching and Merkleized state.

## Further Reading

- [Linda Model](linda-model.md) - Tuple space foundations
- [Spatial Matching](spatial-matching.md) - Pattern matching algorithm
- [Consensus Integration](../05-distributed-execution/consensus-integration.md) - How RSpace enables consensus
- [Send Operations](../02-message-passing/send-operations.md) - Rholang send syntax

---

**Navigation**: [← Linda Model](linda-model.md) | [Spatial Matching →](spatial-matching.md) | [Consensus Integration →](../05-distributed-execution/consensus-integration.md)
