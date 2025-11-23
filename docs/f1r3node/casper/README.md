# Casper CBC Consensus Protocol

## Overview

This directory contains comprehensive documentation of RNode's **Casper CBC (Correct-by-Construction)** consensus protocol, a Byzantine Fault Tolerant consensus mechanism that enables distributed agreement on blockchain state across untrusted nodes.

## What is Casper CBC?

Casper CBC is a family of consensus protocols that provide **mathematically provable safety guarantees** through careful protocol construction. Unlike traditional blockchain consensus (Proof-of-Work, traditional BFT), Casper CBC:

- Uses a **multi-parent DAG** instead of a linear chain
- Employs **justifications** as the core consensus primitive
- Achieves **asynchronous safety** (safe even with network delays)
- Provides **accountable Byzantine fault tolerance** (provable who violated protocol)
- Supports **partial synchrony** for liveness

### Key Innovation: Justifications

Every block includes a **justification map** showing the latest block seen from each validator. This simple mechanism enables:

```
Block by ValidatorA includes:
{
  justifications: {
    "ValidatorA": "latest_block_from_A",
    "ValidatorB": "latest_block_from_B",  // What A knows about B
    "ValidatorC": "latest_block_from_C"   // What A knows about C
  }
}
```

From justifications alone, the protocol can:
1. Detect Byzantine behavior (validators creating conflicting blocks)
2. Compute agreement without synchronous voting rounds
3. Determine safety through clique detection
4. Select optimal parents through weight-based fork choice

## Architecture Overview

### High-Level Flow

```
┌─────────────────┐
│ Validator Nodes │
└────────┬────────┘
         │
         ├─► 1. Create Block
         │      - Select parents (fork choice)
         │      - Include justifications
         │      - Execute deploys
         │      - Sign block
         │
         ├─► 2. Broadcast Block
         │      - Send BlockHashMessage to peers
         │      - Peers request full block
         │
         ├─► 3. Validate Block
         │      - Format validation
         │      - Cryptographic validation
         │      - Consensus rules validation
         │      - Equivocation detection
         │      - State execution validation
         │
         ├─► 4. Add to DAG
         │      - Update latest messages
         │      - Incorporate into fork choice
         │
         └─► 5. Check Finalization
                - Run safety oracle
                - If threshold exceeded → finalize
                - Publish finalization event
```

### Multi-Parent DAG Structure

Unlike linear blockchains, Casper CBC uses a DAG where blocks can have multiple parents:

```
Traditional Blockchain:    Casper CBC DAG:

     B5                         B5──┐
      │                         │   B5'
     B4                         │  / │
      │                        B4  B4'
     B3                         │ /  │
      │                        B3  B3'
     B2                         │ /
      │                        B2
     B1                         │
                               B1
```

Benefits:
- **Concurrency**: Multiple validators propose simultaneously
- **Throughput**: More blocks per unit time
- **Flexibility**: Independent branches can merge
- **Resilience**: No single point of failure (no leader)

## Source Code References

### Primary Implementation

**Rust Implementation**:
- `/var/tmp/debug/f1r3node/casper/src/rust/` - Core Rust consensus logic
  - `casper.rs` - Main Casper implementation
  - `multi_parent_casper_impl.rs` - Multi-parent protocol
  - `validate.rs` - Block validation (35KB, most complex)
  - `estimator.rs` - Fork choice algorithm
  - `safety_oracle.rs` - Safety computation
  - `equivocation_detector.rs` - Byzantine detection
  - `finality/` - Finalization logic

**Scala Implementation**:
- `/var/tmp/debug/f1r3node/casper/src/main/scala/coop/rchain/casper/` - Scala consensus
  - `blocks/` - Block creation and processing
  - `engine/` - Consensus engine
  - `safety/` - Safety oracle
  - `protocol/` - Protocol definitions

**Protobuf Schemas**:
- `/var/tmp/debug/f1r3node/models/src/main/protobuf/CasperMessage.proto` - Block structure
- `/var/tmp/debug/f1r3node/models/src/main/protobuf/RhoTypes.proto` - Rholang types

### Key Data Structures

See [04-implementation/data-structures.md](04-implementation/data-structures.md) for complete protobuf schemas.

## Documentation Structure

### [01-fundamentals/](01-fundamentals/)

Core concepts required to understand the protocol:

- **[overview.md](01-fundamentals/overview.md)**: High-level architecture and design principles
- **[dag-structure.md](01-fundamentals/dag-structure.md)**: Multi-parent DAG vs linear blockchain
- **[justifications.md](01-fundamentals/justifications.md)**: The core consensus primitive (most important!)
- **[byzantine-fault-tolerance.md](01-fundamentals/byzantine-fault-tolerance.md)**: BFT properties and guarantees

Start here if you're new to Casper CBC.

### [02-consensus-protocol/](02-consensus-protocol/)

Detailed protocol mechanics:

- **[block-creation.md](02-consensus-protocol/block-creation.md)**: How validators propose blocks
- **[block-validation.md](02-consensus-protocol/block-validation.md)**: Multi-layer validation pipeline
- **[fork-choice-estimator.md](02-consensus-protocol/fork-choice-estimator.md)**: Parent selection algorithm
- **[equivocation-detection.md](02-consensus-protocol/equivocation-detection.md)**: Byzantine behavior detection
- **[safety-oracle.md](02-consensus-protocol/safety-oracle.md)**: Clique-based safety computation
- **[finalization.md](02-consensus-protocol/finalization.md)**: Block finalization process

Read after understanding fundamentals.

### [03-network-layer/](03-network-layer/)

Distributed coordination mechanisms:

- **[peer-discovery.md](03-network-layer/peer-discovery.md)**: Kademlia DHT for finding peers
- **[message-protocols.md](03-network-layer/message-protocols.md)**: gRPC message types and flow
- **[state-synchronization.md](03-network-layer/state-synchronization.md)**: Bootstrap and sync procedures
- **[validator-coordination.md](03-network-layer/validator-coordination.md)**: Bonding, slashing, active set management

Essential for understanding distributed operation.

### [04-implementation/](04-implementation/)

Implementation details and code organization:

- **[code-organization.md](04-implementation/code-organization.md)**: Codebase structure and module purposes
- **[data-structures.md](04-implementation/data-structures.md)**: Protobuf schemas explained in detail
- **[algorithms.md](04-implementation/algorithms.md)**: Pseudocode for key algorithms
- **[performance-considerations.md](04-implementation/performance-considerations.md)**: Optimization strategies

For developers implementing or modifying consensus.

### [05-formal-properties/](05-formal-properties/)

Mathematical foundations and proofs:

- **[safety-proof-sketch.md](05-formal-properties/safety-proof-sketch.md)**: Safety guarantees explanation
- **[liveness-conditions.md](05-formal-properties/liveness-conditions.md)**: When the protocol makes progress
- **[consensus-parameters.md](05-formal-properties/consensus-parameters.md)**: Tunable parameters and their effects

For formal verification and theoretical understanding.

## Key Concepts Quick Reference

### Justifications

A map from validators to their latest blocks, included in every block:

```rust
justifications: Vec<Justification> = [
  { validator: "pubkey_A", latestBlockHash: "hash_123" },
  { validator: "pubkey_B", latestBlockHash: "hash_456" },
  ...
]
```

**Purpose**: Creates partial ordering enabling consensus without synchronous communication.

See: [01-fundamentals/justifications.md](01-fundamentals/justifications.md)

### Fork Choice (Estimator)

Algorithm for selecting which blocks to build upon:

1. Get latest messages from all validators
2. Score blocks by validator weight supporting each chain
3. Select highest-scored blocks as parents

**Purpose**: Ensures validators converge on preferred history.

See: [02-consensus-protocol/fork-choice-estimator.md](02-consensus-protocol/fork-choice-estimator.md)

### Equivocation Detection

Identifying when validators create conflicting blocks:

- **Direct**: Same sequence number, different blocks
- **Admissible**: Referenced by honest validators
- **Ignorable**: Not referenced by anyone
- **Neglected**: Validator failed to report known equivocation

**Purpose**: Byzantine fault tolerance and accountability.

See: [02-consensus-protocol/equivocation-detection.md](02-consensus-protocol/equivocation-detection.md)

### Safety Oracle

Computes fault tolerance for a block:

1. Find validators agreeing on the block
2. Build agreement graph (who won't disagree)
3. Find maximum clique
4. Calculate: `(maxCliqueWeight * 2 - totalStake) / totalStake`

**Purpose**: Determines when blocks are safe to finalize.

See: [02-consensus-protocol/safety-oracle.md](02-consensus-protocol/safety-oracle.md)

### Finalization

Process of marking blocks as irreversible:

1. Identify finalization candidates (>50% stake agreement)
2. Run safety oracle on each
3. First to exceed threshold becomes Last Finalized Block (LFB)

**Purpose**: Provides transaction finality for applications.

See: [02-consensus-protocol/finalization.md](02-consensus-protocol/finalization.md)

## Protocol Properties

### Safety

**Guarantee**: Two honest validators will never finalize conflicting blocks.

**Condition**: Less than 1/3 of stake is Byzantine.

**Mechanism**: Safety oracle ensures >50% stake agreement before finalization.

### Liveness

**Guarantee**: Network continues making progress.

**Condition**: More than 2/3 of stake is honest and can communicate.

**Mechanism**: Fork choice ensures validators converge on common history.

### Accountable Byzantine Fault Tolerance

**Guarantee**: Byzantine behavior is detected and provable.

**Mechanism**: Equivocation detection creates evidence of protocol violations.

### Partial Synchrony

**Safety**: Guaranteed even with arbitrary network delays.

**Liveness**: Guaranteed under bounded network delays (partial synchrony assumption).

## Integration with Rholang

Casper consensus orchestrates Rholang smart contract execution:

```
Block contains:
├─ Pre-state hash (RSpace root before execution)
├─ Deploys (Rholang smart contracts)
└─ Post-state hash (RSpace root after execution)

Validation ensures:
  execute(pre_state, deploys) = post_state
```

All nodes execute deploys independently and verify state transitions match. This provides:
- **Deterministic execution**: Same inputs → same outputs
- **Byzantine detection**: Nodes producing wrong state are identified
- **Concurrent execution**: RSpace enables parallel smart contract execution

See: [../rholang/05-distributed-execution/consensus-integration.md](../rholang/05-distributed-execution/consensus-integration.md)

## Common Questions

### Q: Why multi-parent DAG instead of linear blockchain?

**A**: Higher throughput. Multiple validators can propose concurrently without conflicts. Independent state changes can execute in parallel and merge later.

### Q: How do validators agree without voting rounds?

**A**: Justifications create implicit voting. Each block declares what the validator has seen. Fork choice algorithm scores chains by cumulative validator weight, ensuring convergence.

### Q: What happens if a validator creates two conflicting blocks?

**A**: Equivocation detection identifies this. The validator can be slashed (lose their stake). Other validators must acknowledge the equivocation or their blocks are rejected.

### Q: When is a transaction truly final?

**A**: When its containing block is finalized (exceeds safety threshold). Finalization typically takes 1-3 minutes depending on network conditions and validator count.

### Q: Can the network fork permanently?

**A**: No, if >2/3 stake is honest. Fork choice ensures validators converge on the heaviest chain. Even during network partitions, the protocol maintains safety.

### Q: How does this compare to Ethereum's Casper FFG?

**A**: Both are Casper family protocols, but:
- **CBC**: Correct-by-Construction, proves safety first, then adds liveness
- **FFG**: Friendly Finality Gadget, overlays finality on existing blockchain (like Ethereum)
- RNode implements CBC for full protocol integration

## Performance Characteristics

### Throughput

- **Blocks per second**: Depends on network size and validator count
- **Transactions per second**: Depends on Rholang contract complexity
- **Parallel execution**: RSpace enables concurrent deploy execution

### Latency

- **Block creation**: 5-10 seconds typical
- **Block propagation**: 1-3 seconds across network
- **Finalization**: 1-3 minutes (depends on validator responsiveness)

### Scalability

- **Validator count**: Tested with 10-100 validators
- **Network size**: DHT-based discovery scales to thousands of nodes
- **State size**: LMDB backend supports multi-GB state

See: [04-implementation/performance-considerations.md](04-implementation/performance-considerations.md)

## Related Documentation

- **Rholang Integration**: [../rholang/05-distributed-execution/consensus-integration.md](../rholang/05-distributed-execution/consensus-integration.md)
- **MeTTaTron Integration**: [../integration/specifications/consensus-state-queries.md](../integration/specifications/consensus-state-queries.md)
- **Formal Verification**: `/var/tmp/debug/f1r3node/docs/formal-verification/`
- **Original Casper CBC Paper**: https://github.com/cbc-casper/cbc-casper-paper

## Recommended Reading Order

**For Understanding**:
1. [01-fundamentals/overview.md](01-fundamentals/overview.md) - Start here
2. [01-fundamentals/justifications.md](01-fundamentals/justifications.md) - Most important concept
3. [01-fundamentals/dag-structure.md](01-fundamentals/dag-structure.md) - Why DAG not chain
4. [02-consensus-protocol/fork-choice-estimator.md](02-consensus-protocol/fork-choice-estimator.md) - How validators converge
5. [02-consensus-protocol/safety-oracle.md](02-consensus-protocol/safety-oracle.md) - How finalization works

**For Implementation**:
1. [04-implementation/code-organization.md](04-implementation/code-organization.md) - Find your way around
2. [04-implementation/data-structures.md](04-implementation/data-structures.md) - Understand data formats
3. [02-consensus-protocol/block-validation.md](02-consensus-protocol/block-validation.md) - Validation pipeline
4. [04-implementation/algorithms.md](04-implementation/algorithms.md) - Key algorithms

**For Verification**:
1. [05-formal-properties/safety-proof-sketch.md](05-formal-properties/safety-proof-sketch.md)
2. [05-formal-properties/liveness-conditions.md](05-formal-properties/liveness-conditions.md)
3. [01-fundamentals/byzantine-fault-tolerance.md](01-fundamentals/byzantine-fault-tolerance.md)

## Next Steps

After reading this overview:

1. **Understand the fundamentals**: Read [01-fundamentals/justifications.md](01-fundamentals/justifications.md)
2. **See it in action**: Explore [02-consensus-protocol/](02-consensus-protocol/)
3. **Dive into code**: Check [04-implementation/code-organization.md](04-implementation/code-organization.md)
4. **Integrate**: Read [../integration/README.md](../integration/README.md) for MeTTaTron integration

---

**Navigation**: [← Back](../README.md) | [Rholang →](../rholang/README.md) | [Integration →](../integration/README.md)
