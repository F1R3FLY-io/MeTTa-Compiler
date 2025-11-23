# Justifications: The Core Consensus Primitive

## Overview

**Justifications** are the single most important innovation in Casper CBC consensus. They are simple in concept but profound in impact: each block includes a map showing the latest block this validator has seen from every other validator in the network.

This document provides a comprehensive deep dive into justifications, explaining why they're necessary, how they work, and what they enable.

## The Problem: Distributed Agreement Without Coordination

### Traditional Approaches

In traditional distributed consensus:

**Synchronous Voting (PBFT-style)**:
```
Round 1: Leader proposes
Round 2: All validators vote "prepare"
Round 3: All validators vote "commit"
Result: 3 network round-trips for one decision
```

Problems:
- Requires synchronous communication
- Leader is a single point of failure
- Fixed round structure limits throughput
- Network delays block progress

**Longest Chain (Nakamoto-style)**:
```
Validators independently build chains
Longest chain wins
Forks are resolved probabilistically
```

Problems:
- Finality is only probabilistic
- Energy inefficient (PoW) or requires stake grinding prevention (PoS)
- Linear structure limits throughput

### The Casper CBC Approach

Instead of explicit voting or probabilistic finality, Casper CBC uses **implicit agreement through justifications**:

```
Each validator simply declares:
"Here's what I've seen from everyone else"

From this, the protocol can derive:
- Who agrees with whom
- Which chains have support
- When agreement is irreversible
- Who violated protocol rules
```

No voting rounds. No leader election. No proof-of-work. Just a declaration of observed state.

## What Are Justifications?

### Data Structure

**Protobuf Definition** (`/var/tmp/debug/f1r3node/models/src/main/protobuf/CasperMessage.proto`, lines 70-73):

```protobuf
message JustificationProto {
  bytes validator = 1;        // Validator's public key (identity)
  bytes latestBlockHash = 2;  // Hash of latest block seen from this validator
}
```

**In Each Block** (lines 60-68):

```protobuf
message BlockMessageProto {
  bytes blockHash = 1;
  HeaderProto header = 2;
  BodyProto body = 3;
  repeated JustificationProto justifications = 4;  // ← The key field
  bytes sender = 5;          // Who created this block
  int32 seqNum = 6;          // Sequence number for this validator
  bytes sig = 7;             // Signature
}
```

### Concrete Example

Suppose we have three validators: Alice, Bob, and Charlie.

**Alice creates block A5** (her 5th block):

```rust
Block {
  sender: "alice_pubkey",
  seqNum: 5,
  justifications: [
    { validator: "alice_pubkey", latestBlockHash: "hash_A4" },  // Her previous block
    { validator: "bob_pubkey",   latestBlockHash: "hash_B7" },  // Latest from Bob she's seen
    { validator: "charlie_pubkey", latestBlockHash: "hash_C3" },  // Latest from Charlie
  ],
  ...
}
```

**Bob creates block B8** shortly after:

```rust
Block {
  sender: "bob_pubkey",
  seqNum: 8,
  justifications: [
    { validator: "alice_pubkey", latestBlockHash: "hash_A5" },  // He's seen Alice's new block!
    { validator: "bob_pubkey",   latestBlockHash: "hash_B7" },  // His previous block
    { validator: "charlie_pubkey", latestBlockHash: "hash_C3" },  // Same Charlie block
  ],
  ...
}
```

**Charlie creates block C4**:

```rust
Block {
  sender: "charlie_pubkey",
  seqNum: 4,
  justifications: [
    { validator: "alice_pubkey", latestBlockHash: "hash_A4" },  // Hasn't seen A5 yet
    { validator: "bob_pubkey",   latestBlockHash: "hash_B7" },  // Hasn't seen B8 yet
    { validator: "charlie_pubkey", latestBlockHash: "hash_C3" }, // His previous block
  ],
  ...
}
```

From these justifications, we can infer:
- Bob has seen Alice's latest block (A5)
- Charlie has not seen A5 or B8 yet (network delay or not propagated)
- All three agree on Charlie's C3 block

## What Justifications Enable

### 1. Equivocation Detection

**Direct Equivocation** (Creating two blocks with same sequence number):

```
Alice creates block A5:
  justifications: [
    { validator: "alice_pubkey", latestBlockHash: "hash_A4" },
    ...
  ]

Alice creates conflicting block A5':
  justifications: [
    { validator: "alice_pubkey", latestBlockHash: "hash_A4" },  // Same sequence!
    ...
  ]
```

**Detection** (`/var/tmp/debug/f1r3node/casper/src/main/scala/coop/rchain/casper/EquivocationDetector.scala`, lines 40-50):

```scala
val isEquivocation =
  // Current block's sender justification should match their previous block
  maybeCreatorJustification != maybeLatestMessageOfCreatorHash

if (isEquivocation) {
  // Record equivocation
  // Potentially slash validator
  // Decide if equivocation is admissible
}
```

**Neglected Equivocation** (Failing to acknowledge known equivocation):

```
Validator Bob creates block B10:
  justifications: [
    { validator: "alice_pubkey", latestBlockHash: "hash_A5" },  // Only one A5
    ...
  ]

But Bob has actually seen BOTH A5 and A5' (equivocation).
Protocol requires: Bob MUST include the equivocation in his justifications.
If he doesn't → his block is invalid (neglected equivocation).
```

This ensures equivocations can't be hidden.

### 2. Partial Ordering of All Blocks

Justifications create a happens-before relationship:

```
Block B contains justification pointing to block A
  ⟹  Block A "happened before" Block B from B's creator's perspective
  ⟹  Creates partial order: A ≺ B
```

**Transitive Closure**:

```
A ≺ B  and  B ≺ C  ⟹  A ≺ C
```

This partial order is used for:
- Fork choice (which blocks to build on)
- Safety oracle (which validators agree)
- Finalization (which blocks are irreversible)

### 3. Fork Choice Without Synchronization

**Algorithm** (`/var/tmp/debug/f1r3node/casper/src/main/scala/coop/rchain/casper/Estimator.scala`):

```
1. Collect latest messages (justifications) from all validators
2. Score each block:
   - Start from each latest message
   - Traverse backwards through DAG
   - Add validator's weight to each block in their chain
3. Select highest-scored blocks as parents
```

**Example**:

```
Validators:         Weights:
- Alice             30%
- Bob               25%
- Charlie           45%

Latest messages (from justifications):
- Alice → Block A5
- Bob   → Block B8
- Charlie → Block C4

Both A5 and B8 build on common ancestor X.
C4 builds on different ancestor Y.

Scoring:
  Block X: 30% (Alice) + 25% (Bob) = 55%
  Block Y: 45% (Charlie) = 45%

Fork choice: Build on X (higher cumulative weight)
```

No voting required. Just follow the weight.

### 4. Safety Oracle Computation

**Clique Oracle** (`/var/tmp/debug/f1r3node/casper/src/main/scala/coop/rchain/casper/safety/CliqueOracle.scala`):

```
Goal: Determine if block B is safe to finalize

1. Find all validators whose latest message (from justifications) builds on B
2. Build "agreement graph":
   - Validators are nodes
   - Edge between V1 and V2 if they'll never disagree about B
3. Find maximum clique (largest fully-connected subgraph)
4. Compute fault tolerance:

   FT = (maxCliqueWeight × 2 - totalStake) / totalStake

   If FT > threshold (typically 0): Block is safe
```

**Why This Works**:

```
If validators controlling >50% stake all:
- Have B in their chain (from justifications)
- Won't change their minds (no conflicting justifications)

Then B is irreversible: not enough stake remains to create competing finalized chain.
```

All determined from justifications alone!

### 5. State Synchronization

New nodes joining the network can:

```
1. Request latest blocks from peers
2. Examine their justifications
3. Build DAG backwards:
   - For each justification, request that block
   - Recursively request justifications' blocks
   - Continue until reaching genesis
4. Result: Full DAG reconstruction
```

Justifications provide the dependency graph for synchronization.

## Implementation Deep Dive

### Creating Justifications

**Source**: `/var/tmp/debug/f1r3node/casper/src/main/scala/coop/rchain/casper/blocks/proposer/Proposer.scala`

```scala
def createBlock(deploys: Seq[Deploy]): F[Block] = {
  for {
    // Get current DAG snapshot
    snapshot <- casper.getSnapshot

    // Collect latest messages from all validators
    latestMessages <- snapshot.latestMessages

    // Create justifications from latest messages
    justifications = latestMessages.map { case (validator, blockHash) =>
      Justification(
        validator = validator,
        latestBlockHash = blockHash
      )
    }.toList

    // Include in block
    block = Block(
      sender = myPublicKey,
      seqNum = mySequenceNumber + 1,
      justifications = justifications,
      ...
    )
  } yield block
}
```

### Validating Justifications

**Source**: `/var/tmp/debug/f1r3node/casper/src/rust/interpreter/validate.rs`, lines 300-350

```rust
fn validate_justifications(
    block: &BlockMessage,
    dag: &BlockDagRepresentation,
) -> Result<(), ValidationError> {
    let creator = &block.sender;
    let seq_num = block.seq_num;

    // 1. Creator's justification must point to their previous block
    let creator_justification = block.justifications
        .iter()
        .find(|j| j.validator == *creator)
        .ok_or(ValidationError::MissingCreatorJustification)?;

    let expected_previous = dag.get_block_by_validator_and_seq(creator, seq_num - 1)?;

    if creator_justification.latest_block_hash != expected_previous.hash() {
        // Either equivocation or neglected equivocation
        return Err(ValidationError::InvalidJustification);
    }

    // 2. All justifications must point to valid blocks
    for justification in &block.justifications {
        if !dag.contains(&justification.latest_block_hash) {
            return Err(ValidationError::MissingJustifiedBlock);
        }
    }

    // 3. Check for neglected equivocations
    for (validator, latest_msg) in dag.latest_messages() {
        if let Some(equivocation) = dag.get_equivocation(validator) {
            // If creator has seen this validator's equivocation,
            // they MUST include it in justifications
            if is_ancestor(equivocation, creator_justification, dag) {
                // Creator knows about equivocation
                let includes_equivocation = block.justifications
                    .iter()
                    .any(|j| j.validator == validator &&
                             is_equivocator_block(&j.latest_block_hash, dag));

                if !includes_equivocation {
                    return Err(ValidationError::NeglectedEquivocation);
                }
            }
        }
    }

    Ok(())
}
```

### Using Justifications in Fork Choice

**Source**: `/var/tmp/debug/f1r3node/casper/src/main/scala/coop/rchain/casper/Estimator.scala`, lines 80-150

```scala
def tips(
  dag: BlockDagRepresentation[F],
  genesis: BlockMessage
): F[ForkChoice] = {
  for {
    // 1. Get latest messages from all validators (from justifications)
    latestMessages <- dag.latestMessages

    // 2. Find LCA (Latest Common Ancestor)
    lca <- computeLCA(latestMessages, dag)

    // 3. Score blocks from latest messages down to LCA
    scoreMap <- buildScoreMap(latestMessages, lca, dag)

    // 4. Rank blocks by score
    rankedBlocks = scoreMap.toList.sortBy(-_._2)  // Descending by score

    // 5. Select top-ranked as tips
    tips = rankedBlocks.take(maxParents).map(_._1)

  } yield ForkChoice(tips)
}

def buildScoreMap(
  latestMessages: Map[Validator, BlockHash],
  lca: BlockHash,
  dag: BlockDagRepresentation[F]
): F[Map[BlockHash, Weight]] = {
  // For each latest message
  latestMessages.foldLeft(Map.empty[BlockHash, Weight]) {
    case (scores, (validator, blockHash)) =>
      val weight = dag.getValidatorWeight(validator)

      // Traverse from this block down to LCA
      val path = dag.pathToBlock(blockHash, lca)

      // Add this validator's weight to all blocks in path
      path.foldLeft(scores) { (acc, block) =>
        acc.updated(block, acc.getOrElse(block, 0) + weight)
      }
  }
}
```

## Mathematical Properties

### Theorem: Safety from Justifications

**Claim**: If validators controlling >50% stake all have block B in their chain (via justifications) and won't change their minds, then B is irreversible.

**Proof Sketch**:

```
1. Assume B is in the chain for validators with >50% stake (S1)
2. Their justifications show they've committed to B
3. For B to be orphaned, validators with >50% stake must switch chains (S2)
4. But |S1| > 50% and |S2| > 50%
5. ⟹ |S1 ∩ S2| > 0% (some validators must switch)
6. Switching means creating equivocations (provable via justifications)
7. Honest validators won't equivocate (by assumption <1/3 Byzantine)
8. ⟹ Not enough stake can switch
9. ⟹ B cannot be orphaned
10. ⟹ B is safe

Q.E.D.
```

The key insight: **Justifications make commitment observable and irrevocable**.

### Theorem: Equivocation Detection Completeness

**Claim**: If a validator equivocates, all honest validators will eventually detect it.

**Proof Sketch**:

```
1. Validator V creates equivocating blocks B1 and B2 (same sequence number)
2. Both B1 and B2 get propagated to the network
3. Some validator H (honest) receives both B1 and B2
4. H creates new block H_new with justifications
5. H's justifications must include V's equivocation (or H's block is invalid)
6. H_new propagates to other honest validators
7. They see equivocation in H's justifications
8. Their blocks must also acknowledge it (or invalid)
9. Transitively, all honest validators learn of equivocation
10. Protocol records equivocation permanently in DAG

Q.E.D.
```

The mechanism: **Neglected equivocation detection forces propagation**.

## Design Alternatives (Why This Approach?)

### Why Not Simple Message Log?

**Alternative**: Just keep a log of all messages seen.

**Problem**:
- Unbounded state growth
- No way to determine "latest" message from a validator
- Difficult to detect equivocations
- No clear parent selection

Justifications solve this by:
- Keeping only the **latest** message per validator
- Implicitly defining parent set
- Making equivocations immediately obvious

### Why Not Explicit Vote Messages?

**Alternative**: Validators send explicit "vote for block X" messages.

**Problem**:
- Requires additional message types
- Needs synchronization for vote collection
- Increases network traffic
- Adds protocol complexity

Justifications solve this by:
- Blocks themselves are implicit votes
- No separate vote collection phase
- Single message type (blocks with justifications)

### Why Include All Validators?

**Alternative**: Only include justifications for validators whose blocks are parents.

**Problem**:
- Can't detect neglected equivocations
- Partial ordering is incomplete
- Safety oracle needs all validators' perspectives

Full justifications provide:
- Complete view of validator's knowledge
- Byzantine fault detection
- Accurate safety computation

## Common Misconceptions

### Misconception 1: "Justifications are just parent references"

**Reality**: Parents are a subset of justifications. Justifications declare the latest message from **all** validators, not just those being built upon.

```
Parents (explicit dependency):
  Block B5 directly builds on B3 and B4

Justifications (knowledge declaration):
  Creator of B5 has seen:
  - Block A7 from Alice
  - Block B4 from Bob (parent)
  - Block C9 from Charlie
  - Block D2 from Dave
```

### Misconception 2: "Justifications are optional metadata"

**Reality**: Justifications are **core consensus data**. Remove them and:
- Equivocation detection breaks
- Fork choice becomes arbitrary
- Safety oracle can't compute
- Protocol loses Byzantine fault tolerance

### Misconception 3: "Justifications make blocks larger"

**Reality**: Yes, but minimal. Each justification is ~64 bytes (validator pubkey + block hash). For 100 validators: ~6.4 KB per block. Negligible compared to block contents (deploys, state, etc.).

### Misconception 4: "Latest message = highest sequence number"

**Reality**: Due to equivocations, a validator might have multiple blocks with the same sequence number. "Latest" means "most recent message this validator has seen from that validator", which might include equivocations.

## Practical Considerations

### Performance

**Storage**:
```rust
struct Justification {
    validator: [u8; 32],      // 32 bytes (public key)
    latest_block_hash: [u8; 32]  // 32 bytes (block hash)
}
// Total: 64 bytes per justification
// For 100 validators: 6.4 KB
// For 1000 validators: 64 KB
```

Scales linearly with validator count.

**Validation**:
```
For each justification (N validators):
  - Lookup block in DAG: O(1) hash map lookup
  - Check equivocation: O(1) equivocation tracker lookup

Total: O(N) validation time
```

Acceptable for validator counts up to ~10,000.

### Network Optimization

**Justification Compression**:

Since justifications often overlap between blocks, they can be delta-encoded:

```
Block B8:
  justifications_delta: [
    { validator: "alice", latestBlockHash: "hash_A5" },  // Changed from B7
    // Bob, Charlie unchanged from B7, omitted
  ]
  justifications_base: "hash_B7"  // Reference to previous block's justifications
```

Not currently implemented in RNode, but possible optimization.

### Equivocation Handling

**Admissible Equivocations**:

```rust
// Equivocation that other honest validators have referenced
// Must be included in DAG for accountability
struct AdmissibleEquivocation {
    equivocator: Validator,
    blocks: Vec<BlockHash>,  // Conflicting blocks
    first_detected_by: Vec<BlockHash>,  // Blocks that first referenced it
}
```

**Ignored Equivocations**:

```rust
// Equivocation no honest validator has built on
// Can be dropped to prevent spam
struct IgnoredEquivocation {
    equivocator: Validator,
    blocks: Vec<BlockHash>,
}
```

Source: `/var/tmp/debug/f1r3node/casper/src/main/scala/coop/rchain/casper/EquivocationDetector.scala`, lines 100-150

## Code Example: Building a Block with Justifications

**Complete workflow** from RNode source:

```rust
// 1. Get current state
let dag = casper.get_dag().await?;
let latest_messages = dag.latest_messages();

// 2. Build justifications
let mut justifications = Vec::new();
for (validator, block_hash) in latest_messages {
    justifications.push(Justification {
        validator: validator.clone(),
        latest_block_hash: block_hash.clone(),
    });
}

// 3. Select parents using fork choice (uses justifications internally)
let parents = estimator.tips(&dag).await?;

// 4. Create block
let block = BlockMessage {
    sender: my_validator_key,
    seq_num: my_current_seq + 1,
    justifications,  // ← Include justifications
    header: Header {
        parent_hashes: parents,  // Subset of justifications
        timestamp: current_time(),
        ...
    },
    body: Body {
        deploys: selected_deploys,
        ...
    },
    ...
};

// 5. Sign block
let signature = sign(&block, my_private_key);
block.sig = signature;

// 6. Broadcast
broadcast_block(&block).await?;
```

## Summary

Justifications are deceptively simple but extraordinarily powerful:

**What they are**:
- A map from validators to their latest blocks
- Included in every block

**What they enable**:
1. **Equivocation detection**: Identify Byzantine behavior
2. **Partial ordering**: Create happens-before relationships
3. **Fork choice**: Select parents without voting
4. **Safety oracle**: Compute finalization without synchronization
5. **State sync**: Reconstruct DAG from any starting point

**Why they work**:
- Make validator knowledge explicit
- Create irrevocable commitments
- Enable implicit agreement
- Provide accountability

**Key insight**: Rather than coordinating explicitly (voting, leader election), validators simply declare what they've observed. The protocol mathematics does the rest.

This is the essence of "Correct-by-Construction": prove the properties you want, then construct the protocol to guarantee them. Justifications are the construction that guarantees Byzantine fault tolerance and asynchronous safety.

## Further Reading

- [DAG Structure](dag-structure.md) - How justifications create the DAG
- [Fork Choice Estimator](../02-consensus-protocol/fork-choice-estimator.md) - Using justifications for parent selection
- [Equivocation Detection](../02-consensus-protocol/equivocation-detection.md) - Byzantine fault detection
- [Safety Oracle](../02-consensus-protocol/safety-oracle.md) - Finalization via justifications

---

**Navigation**: [← Back](../README.md) | [Overview →](overview.md) | [DAG Structure →](dag-structure.md)
