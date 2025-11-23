# Rholang: Concurrent Distributed Programming

## Overview

**Rholang** (Reflective Higher-Order Language) is a concurrent programming language based on the **ρ-calculus** (rho-calculus), a reflective variant of the π-calculus. It provides a foundation for writing distributed smart contracts with strong mathematical semantics and capability-based security.

This documentation covers Rholang's distributed programming model, focusing on how processes communicate, coordinate, and execute across RNode's distributed network.

## What is Rholang?

Rholang is fundamentally different from traditional programming languages:

### Traditional Languages (Imperative)

```javascript
// Sequential execution
let x = 5;
let y = 10;
let z = x + y;
console.log(z);  // Output: 15
```

**Model**: Sequential instructions, shared mutable state

### Rholang (Concurrent Process Calculus)

```rholang
new x, y, result in {
  x!(5) |                    // Send 5 on channel x (parallel)
  y!(10) |                   // Send 10 on channel y (parallel)
  for (@a <- x & @b <- y) {  // Wait for both (join pattern)
    result!(a + b)           // Send sum on result channel
  }
}
```

**Model**: Concurrent processes, message passing, channel-based communication

## Key Characteristics

### 1. Process-Oriented

Everything in Rholang is a **process**:

```rholang
// Simple process
stdout!("Hello, World!")

// Parallel composition of processes
process1 | process2 | process3

// Process that creates new processes
new channel in {
  channel!("data") |
  for (msg <- channel) { /* handle message */ }
}
```

### 2. Message Passing via Channels

Communication happens through **channels** (names):

```rholang
new channel in {
  // Send process
  channel!("message") |

  // Receive process
  for (msg <- channel) {
    // Process message
  }
}
```

### 3. Unforgeable Names (Capability Security)

Channels created with `new` are **unforgeable** - cryptographically unique:

```rholang
new privateChannel in {
  // Only code with access to privateChannel can use it
  privateChannel!("secret")
  // No one else can guess or forge this channel name
}
```

**Security**: Possession of name = permission to use.

### 4. Reflection (Quotation and Unquotation)

Names and processes are interchangeable:

```rholang
@process  // Quote: Convert process to name
*name     // Unquote: Convert name to process

new x in {
  x!(*x)  // Send the name x on channel x (self-reference)
}
```

### 5. Spatial Pattern Matching

Receive can wait for data from **multiple channels atomically**:

```rholang
for (@x <- chan1 & @y <- chan2 & @z <- chan3) {
  // Executes only when data is available on ALL three channels
  // Atomic: all or nothing
}
```

## Source Code References

### Primary Implementation

**Rust Interpreter**:
- `/var/tmp/debug/f1r3node/rholang/src/rust/interpreter/` - Core interpreter
  - `reduce.rs` - Main evaluation engine (eval, send, receive)
  - `compiler/normalizer/` - Parsing and normalization
  - `matcher/spatial_matcher.rs` - Pattern matching algorithm
  - `rho_runtime.rs` - Runtime system and built-in functions

**RSpace (Tuple Space)**:
- `/var/tmp/debug/f1r3node/rspace++/src/rspace/` - Tuple space implementation
  - `rspace.rs` - Main RSpace structure
  - `rspace_interface.rs` - Produce/consume API
  - `history_repository.rs` - Persistent storage with history trie

**Protocol Definitions**:
- `/var/tmp/debug/f1r3node/models/src/main/protobuf/RhoTypes.proto` - Data structures

### Example Contracts

- `/var/tmp/debug/f1r3node/rholang/examples/` - Real-world examples
  - `tut-hello.rho` - Hello world
  - `tut-philosophers.rho` - Dining philosophers (synchronization)
  - Token contracts, registries, and more

## Documentation Structure

### [01-process-calculus/](01-process-calculus/)

Theoretical foundations of Rholang:

- **[rho-calculus-foundations.md](01-process-calculus/rho-calculus-foundations.md)**: π-calculus extensions, formal semantics
- **[reflection-and-quotation.md](01-process-calculus/reflection-and-quotation.md)**: `@` and `*` operators
- **[parallel-composition.md](01-process-calculus/parallel-composition.md)**: `|` operator semantics
- **[name-equivalence.md](01-process-calculus/name-equivalence.md)**: Structural equivalence rules

Essential for understanding the mathematical foundations.

### [02-message-passing/](02-message-passing/)

Core communication primitives:

- **[channels-and-names.md](02-message-passing/channels-and-names.md)**: Channel creation with `new`
- **[send-operations.md](02-message-passing/send-operations.md)**: `!` and `!!` semantics
- **[receive-operations.md](02-message-passing/receive-operations.md)**: `for`, `<-`, `<=`, `<<-`
- **[join-patterns.md](02-message-passing/join-patterns.md)**: Multi-channel synchronization
- **[pattern-matching.md](02-message-passing/pattern-matching.md)**: Structural patterns

Start here for practical Rholang programming.

### [03-capability-security/](03-capability-security/)

Security model and best practices:

- **[unforgeable-names.md](03-capability-security/unforgeable-names.md)**: Cryptographic security
- **[bundles.md](03-capability-security/bundles.md)**: Read/write permission wrappers
- **[security-patterns.md](03-capability-security/security-patterns.md)**: Best practices

Critical for writing secure smart contracts.

### [04-rspace-tuplespace/](04-rspace-tuplespace/)

The distributed coordination layer:

- **[linda-model.md](04-rspace-tuplespace/linda-model.md)**: Tuple space concepts
- **[produce-consume.md](04-rspace-tuplespace/produce-consume.md)**: Core operations
- **[spatial-matching.md](04-rspace-tuplespace/spatial-matching.md)**: Pattern matching algorithm
- **[persistence-and-checkpoints.md](04-rspace-tuplespace/persistence-and-checkpoints.md)**: LMDB backend
- **[deterministic-replay.md](04-rspace-tuplespace/deterministic-replay.md)**: Event log and replay

Essential for understanding distributed execution.

### [05-distributed-execution/](05-distributed-execution/)

How Rholang executes across nodes:

- **[local-vs-distributed.md](05-distributed-execution/local-vs-distributed.md)**: Execution models
- **[state-synchronization.md](05-distributed-execution/state-synchronization.md)**: Cross-node coordination
- **[consensus-integration.md](05-distributed-execution/consensus-integration.md)**: RSpace ↔ Casper integration
- **[deploy-execution.md](05-distributed-execution/deploy-execution.md)**: Smart contract lifecycle

Connects Rholang to Casper consensus.

### [06-examples/](06-examples/)

Annotated working examples:

- **[hello-world.rho.md](06-examples/hello-world.rho.md)**: Simple message passing
- **[dining-philosophers.rho.md](06-examples/dining-philosophers.rho.md)**: Synchronization patterns
- **[token-contract.rho.md](06-examples/token-contract.rho.md)**: Stateful contracts
- **[cross-contract-calls.rho.md](06-examples/cross-contract-calls.rho.md)**: Inter-contract communication

Learn by example.

## Core Concepts Quick Reference

### Processes

Basic unit of computation:

```rholang
// Nil process (does nothing)
Nil

// Send process
channel!("data")

// Receive process
for (msg <- channel) { body }

// Parallel composition
process1 | process2

// New name creation
new x in { process }
```

### Channels (Names)

Communication endpoints:

```rholang
// Unforgeable name (cryptographically unique)
new privateChannel in { ... }

// Registry name (human-readable, registered)
@"rho:io:stdout"!("Hello")

// Quoted process (name derived from process)
@{process}
```

### Send Operations

Put data on channels:

```rholang
// Single send (removed after one receive)
channel!("data")

// Persistent send (remains after receives)
channel!!("data")

// Multiple data items
channel!("item1", "item2", "item3")
```

### Receive Operations

Wait for data on channels:

```rholang
// Linear receive (execute once)
for (@data <- channel) { body }

// Persistent receive (execute repeatedly)
for (@data <= channel) { body }

// Peek (non-consuming read)
for (@data <<- channel) { body }

// Join pattern (atomic multi-channel)
for (@x <- ch1 & @y <- ch2) { body }
```

### Patterns

Match structure of received data:

```rholang
// Variable binding
for (@x <- channel) { /* x is bound */ }

// Constant matching
for (@"specific_value" <- channel) { /* exact match */ }

// Structure matching
for (@{"key": value} <- channel) { /* bind value */ }

// List matching
for (@[head ...tail] <- channel) { /* destructure */ }

// Wildcard (match anything, don't bind)
for (@_ <- channel) { /* ignore value */ }
```

## Integration with Casper Consensus

Rholang executes within RNode's Casper consensus:

```
┌─────────────────────────────────────┐
│         Casper Consensus            │
│  (Byzantine Fault Tolerant DAG)     │
└──────────────┬──────────────────────┘
               │
               │ Blocks contain deploys
               ↓
┌─────────────────────────────────────┐
│      Rholang Interpreter            │
│   (Execute smart contracts)         │
└──────────────┬──────────────────────┘
               │
               │ Produce/Consume operations
               ↓
┌─────────────────────────────────────┐
│         RSpace Tuple Space          │
│  (Distributed coordination layer)   │
│  - LMDB persistent storage          │
│  - History trie (Merkleized)        │
│  - Event log (replay capability)    │
└─────────────────────────────────────┘
```

**Execution Flow**:

1. **Deploy**: User submits Rholang contract
2. **Block Creation**: Validator includes deploy in block
3. **Consensus**: Network agrees on block via Casper
4. **Execution**: All nodes execute deploy independently
   - Interpreter evaluates Rholang processes
   - RSpace handles channel operations (produce/consume)
   - State transitions recorded with pre/post hashes
5. **Validation**: Nodes verify state hash matches
6. **Finalization**: Block becomes finalized via safety oracle

See: [05-distributed-execution/consensus-integration.md](05-distributed-execution/consensus-integration.md)

## Key Properties

### Deterministic Execution

Same input → same output:

```
Pre-state hash + Deploy = Post-state hash

All honest validators compute the same post-state.
Byzantine validators producing different states are detected.
```

### Concurrent Semantics

Parallel composition has true concurrency:

```rholang
process1 | process2 | process3

Executes simultaneously (not sequential).
Order is non-deterministic unless synchronized via channels.
```

### Spatial Pattern Matching

Join patterns are atomic:

```rholang
for (@x <- ch1 & @y <- ch2) { body }

Either both channels have data and body executes,
or neither and process waits.
No partial matching (all or nothing).
```

### Capability-Based Security

Unforgeable names provide security:

```
new secret in {
  secret!("classified")  // Only code with 'secret' can read
}

No global namespace.
No way to guess or enumerate names.
Access = possession of name (capability).
```

## Performance Characteristics

### Throughput

- **Deploys per block**: 100-1000s (depends on complexity)
- **Concurrent execution**: RSpace enables parallel process execution
- **State size**: Multi-GB supported via LMDB

### Latency

- **Send/receive**: Microseconds (in-memory RSpace)
- **Persistent channels**: Milliseconds (LMDB write)
- **Cross-contract calls**: Sub-second (if channels are registered)

### Scalability

- **Channels**: Billions supported (LMDB capacity)
- **Processes**: Limited by gas (phlogiston) system
- **State growth**: Linear with usage, prunable after finalization

## Comparison with Other Smart Contract Languages

### vs. Ethereum Solidity

| Feature | Rholang | Solidity |
|---------|---------|----------|
| **Paradigm** | Concurrent process calculus | Object-oriented imperative |
| **Concurrency** | Native (parallel composition) | None (sequential EVM) |
| **Communication** | Message passing (channels) | Function calls |
| **State** | Distributed tuple space | Global state trie |
| **Security** | Capability-based | Access control lists |
| **Determinism** | Spatial matching | Transaction ordering |

### vs. Cardano Plutus

| Feature | Rholang | Plutus |
|---------|---------|--------|
| **Paradigm** | Process calculus | Functional (Haskell-like) |
| **Model** | UTXO-like (channels) | EUTXO |
| **Concurrency** | Native | Limited |
| **Validation** | On-chain | On-chain + off-chain |

### vs. Polkadot Ink!

| Feature | Rholang | Ink! |
|---------|---------|------|
| **Language** | Domain-specific | Rust embedded DSL |
| **Execution** | Process calculus interpreter | Wasm VM |
| **Concurrency** | Process-level | Thread-level (async) |
| **Interoperability** | Channels | Cross-chain messaging |

## Common Use Cases

### 1. Token Contracts

```rholang
contract @"MyToken"(@"transfer", @from, @to, @amount, return) = {
  new balancesCh in {
    for (@balances <- balancesCh) {
      match (balances.get(from) >= amount) {
        true => {
          balances.set(from, balances.get(from) - amount) |
          balances.set(to, balances.get(to) + amount) |
          balancesCh!(balances) |
          return!(true)
        }
        false => {
          balancesCh!(balances) |
          return!(false)
        }
      }
    }
  }
}
```

### 2. Multi-Signature Wallets

```rholang
contract @"MultiSig"(@"propose", @transaction, return) = {
  new proposalCh in {
    proposalCh!({"tx": transaction, "sigs": []}) |
    return!(*proposalCh)
  }
}

contract @"MultiSig"(@"sign", @proposalName, @signature, return) = {
  for (@proposal <- proposalName) {
    match (proposal.sigs.length + 1 >= threshold) {
      true => {
        // Execute transaction
        execute!(proposal.tx) |
        return!(true)
      }
      false => {
        proposalName!(proposal.set("sigs", proposal.sigs.append(signature))) |
        return!(false)
      }
    }
  }
}
```

### 3. Atomic Swaps

```rholang
contract @"AtomicSwap"(@aliceAsset, @bobAsset, @timeout) = {
  new lock in {
    for (@assetA <- aliceAsset & @assetB <- bobAsset) {
      // Both assets locked atomically
      lock!({"a": assetA, "b": assetB})
    } |
    for (@assets <- lock) {
      // Swap
      aliceAsset!(assets.b) |
      bobAsset!(assets.a)
    }
  } |
  // Timeout refund
  new timer in {
    timer!(*timeout) |
    for (_ <- timer) {
      for (@assetA <- aliceAsset) { aliceAsset!(assetA) } |
      for (@assetB <- bobAsset) { bobAsset!(assetB) }
    }
  }
}
```

## Common Questions

### Q: How does Rholang achieve determinism with non-deterministic concurrency?

**A**: Determinism comes from **spatial pattern matching** in RSpace. While process execution order is non-deterministic, pattern matching is deterministic (lexicographic channel sorting, maximum bipartite matching algorithm). Same channels + same patterns = same matches = same state transitions.

### Q: What prevents race conditions?

**A**: **Atomic channel operations**. Send and receive on channels are atomic. Join patterns match all channels simultaneously or not at all. No partial state visible between operations.

### Q: How does Rholang handle state?

**A**: Through **RSpace tuple space**. Channels hold data (tuples). Send puts data on channel. Receive takes data from channel. State is the sum of all data on all channels at any point in time.

### Q: Can Rholang contracts call each other?

**A**: Yes, via **shared channels**. Contracts can publish their interfaces on known channels (e.g., registry). Other contracts send messages to those channels.

### Q: What prevents infinite loops?

**A**: **Gas system** (phlogiston). Every operation costs gas. When gas runs out, execution halts. Prevents resource exhaustion attacks.

## Learning Path

**Beginner**:
1. Read [02-message-passing/channels-and-names.md](02-message-passing/channels-and-names.md)
2. Study [06-examples/hello-world.rho.md](06-examples/hello-world.rho.md)
3. Experiment with send/receive basics

**Intermediate**:
1. Learn [02-message-passing/join-patterns.md](02-message-passing/join-patterns.md)
2. Study [06-examples/dining-philosophers.rho.md](06-examples/dining-philosophers.rho.md)
3. Understand [03-capability-security/unforgeable-names.md](03-capability-security/unforgeable-names.md)

**Advanced**:
1. Deep dive [04-rspace-tuplespace/spatial-matching.md](04-rspace-tuplespace/spatial-matching.md)
2. Study [05-distributed-execution/consensus-integration.md](05-distributed-execution/consensus-integration.md)
3. Read [01-process-calculus/rho-calculus-foundations.md](01-process-calculus/rho-calculus-foundations.md)

## Related Documentation

- **Casper Consensus**: [../casper/README.md](../casper/README.md)
- **MeTTaTron Integration**: [../integration/README.md](../integration/README.md)
- **RNode Source**: `/var/tmp/debug/f1r3node/rholang/`
- **Examples**: `/var/tmp/debug/f1r3node/rholang/examples/`

## Next Steps

After reading this overview:

1. **Understand message passing**: [02-message-passing/send-operations.md](02-message-passing/send-operations.md)
2. **Learn RSpace**: [04-rspace-tuplespace/linda-model.md](04-rspace-tuplespace/linda-model.md)
3. **See examples**: [06-examples/](06-examples/)
4. **Explore integration**: [../integration/README.md](../integration/README.md)

---

**Navigation**: [← Back](../README.md) | [Casper Consensus →](../casper/README.md) | [Integration →](../integration/README.md)
