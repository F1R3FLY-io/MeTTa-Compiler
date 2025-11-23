# Block Validation

## Overview

**Block validation** is the multi-layer process that ensures every block added to the DAG is well-formed, cryptographically valid, consensus-compliant, and produces correct state transitions. It's the gatekeeper preventing invalid or Byzantine blocks from corrupting the blockchain.

**Purpose**: Maintain Byzantine Fault Tolerance by rejecting invalid blocks while accepting all valid blocks.

**Layers**:
1. **Format validation** - Basic structure and encoding
2. **Cryptographic validation** - Signatures and hashes
3. **Consensus validation** - Justifications, parents, sequence numbers
4. **State transition validation** - Deploy execution and RSpace state
5. **Byzantine validation** - Equivocation detection

## The Validation Pipeline

### High-Level Flow

```
Block received from network
    ↓
[1. Format Validation]
    ├─ Invalid → Reject immediately
    ↓
[2. Cryptographic Validation]
    ├─ Invalid → Reject, possibly ban peer
    ↓
[3. Consensus Validation]
    ├─ Invalid → Reject, record equivocation if applicable
    ↓
[4. Dependencies Check]
    ├─ Missing parents → Request and wait
    ↓
[5. State Transition Validation]
    ├─ Invalid → Reject, possibly slash validator
    ↓
[6. Byzantine Validation]
    ├─ Equivocation → Record, decide if admissible
    ↓
[All checks passed]
    ↓
Add to DAG, update latest messages
    ↓
Run fork choice, check finalization
```

### Source Code Reference

**Primary Implementation**: `/var/tmp/debug/f1r3node/casper/src/rust/interpreter/validate.rs` (35KB file)

Key validation functions:
- `validate_block()` (lines 50-150) - Main entry point
- `validate_format()` (lines 200-250) - Format checks
- `validate_crypto()` (lines 300-400) - Cryptographic checks
- `validate_consensus()` (lines 450-600) - Consensus rules
- `validate_state_transition()` (lines 700-850) - Execution and state

## Layer 1: Format Validation

### Purpose

Ensure block has correct structure before expensive validation.

### Checks

**1. Protobuf Structure**:

```rust
fn validate_format(block: &BlockMessage) -> Result<(), ValidationError> {
    // Block hash exists and is correct length
    if block.block_hash.len() != 32 {
        return Err(ValidationError::InvalidBlockHashLength);
    }

    // Header exists
    let header = block.header.as_ref()
        .ok_or(ValidationError::MissingHeader)?;

    // Body exists
    let body = block.body.as_ref()
        .ok_or(ValidationError::MissingBody)?;

    // Sender public key exists and valid length
    if block.sender.len() != 32 {
        return Err(ValidationError::InvalidSenderKey);
    }

    // Signature exists and valid length
    if block.sig.len() != 64 {
        return Err(ValidationError::InvalidSignature);
    }

    Ok(())
}
```

**2. Field Constraints**:

```rust
// Sequence number must be positive
if block.seq_num < 0 {
    return Err(ValidationError::InvalidSequenceNumber);
}

// Justifications must not be empty (except genesis)
if block.justifications.is_empty() && !is_genesis(block) {
    return Err(ValidationError::MissingJustifications);
}

// Parent hashes must exist
if header.parent_hashes.is_empty() && !is_genesis(block) {
    return Err(ValidationError::MissingParents);
}

// Timestamp must be reasonable (not too far in future)
let now = current_timestamp();
let max_future = now + CLOCK_DRIFT_TOLERANCE;  // e.g., 5 minutes
if header.timestamp > max_future {
    return Err(ValidationError::TimestampTooFarInFuture);
}
```

**3. Size Limits**:

```rust
// Block size must not exceed maximum
let block_size = bincode::serialized_size(block)?;
if block_size > MAX_BLOCK_SIZE {  // e.g., 10 MB
    return Err(ValidationError::BlockTooLarge);
}

// Deploy count must be reasonable
if body.deploys.len() > MAX_DEPLOYS_PER_BLOCK {  // e.g., 1000
    return Err(ValidationError::TooManyDeploys);
}

// Parent count must not exceed maximum
if header.parent_hashes.len() > MAX_PARENTS {  // e.g., 5
    return Err(ValidationError::TooManyParents);
}
```

### Why Format Validation First?

**Performance**: Reject obviously malformed blocks immediately without expensive crypto or state checks.

**DoS Protection**: Prevent attackers from sending garbage that consumes resources.

**Early Exit**: No point validating crypto if structure is already broken.

## Layer 2: Cryptographic Validation

### Purpose

Ensure block is cryptographically authentic and untampered.

### Checks

**1. Block Hash Verification**:

```rust
fn validate_block_hash(block: &BlockMessage) -> Result<(), ValidationError> {
    // Compute hash of block contents (excluding signature)
    let computed_hash = blake2b256(&serialize_for_hash(block));

    // Verify matches claimed hash
    if computed_hash != block.block_hash {
        return Err(ValidationError::BlockHashMismatch);
    }

    Ok(())
}

fn serialize_for_hash(block: &BlockMessage) -> Vec<u8> {
    // Serialize header + body + justifications + sender + seq_num
    // Excluding: block_hash itself and signature
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&serialize(&block.header));
    bytes.extend_from_slice(&serialize(&block.body));
    bytes.extend_from_slice(&serialize(&block.justifications));
    bytes.extend_from_slice(&block.sender);
    bytes.extend_from_slice(&block.seq_num.to_le_bytes());
    bytes
}
```

**2. Signature Verification**:

```rust
fn validate_signature(block: &BlockMessage) -> Result<(), ValidationError> {
    // Extract public key
    let public_key = PublicKey::from_bytes(&block.sender)?;

    // Message to sign is the block hash
    let message = &block.block_hash;

    // Verify signature
    let signature = Signature::from_bytes(&block.sig)?;

    if !public_key.verify(message, &signature) {
        return Err(ValidationError::InvalidSignature);
    }

    Ok(())
}
```

**Why Ed25519?** Fast verification (~50 microseconds), small keys/signatures (32/64 bytes), deterministic.

**3. State Hash Verification** (preliminary):

```rust
// Verify pre-state hash and post-state hash are valid Blake2b256 hashes
if body.pre_state_hash.len() != 32 || body.post_state_hash.len() != 32 {
    return Err(ValidationError::InvalidStateHash);
}

// Actual state transition validation happens later (Layer 5)
```

### Why Cryptographic Validation Second?

**Security**: Reject tampered or forged blocks before consensus checks.

**Cost**: Crypto validation is cheap (~100 microseconds) compared to state execution (milliseconds).

**Attribution**: Invalid signature identifies malicious peer for potential banning.

## Layer 3: Consensus Validation

### Purpose

Ensure block follows Casper CBC consensus rules.

### Checks

**1. Justification Validation**:

```rust
fn validate_justifications(
    block: &BlockMessage,
    dag: &BlockDagRepresentation,
) -> Result<(), ValidationError> {
    let creator = &block.sender;
    let seq_num = block.seq_num;

    // Creator must justify their own previous block
    let creator_justification = block.justifications.iter()
        .find(|j| j.validator == *creator)
        .ok_or(ValidationError::MissingCreatorJustification)?;

    // Expected: previous block from this validator
    let expected_prev = if seq_num > 0 {
        dag.get_block_by_validator_and_seq(creator, seq_num - 1)?
    } else {
        // seq_num == 0: first block from this validator, should justify genesis
        dag.genesis_hash()
    };

    if creator_justification.latest_block_hash != expected_prev {
        // This could be either:
        // 1. Direct equivocation (creating two blocks with same seq_num)
        // 2. Skipped sequence number
        return Err(ValidationError::InvalidCreatorJustification);
    }

    // All justified blocks must exist in DAG
    for justification in &block.justifications {
        if !dag.contains(&justification.latest_block_hash) {
            return Err(ValidationError::UnknownJustifiedBlock(
                justification.latest_block_hash.clone()
            ));
        }
    }

    Ok(())
}
```

See [equivocation-detection.md](equivocation-detection.md) for detailed equivocation handling.

**2. Parent Validation**:

```rust
fn validate_parents(
    block: &BlockMessage,
    dag: &BlockDagRepresentation,
) -> Result<(), ValidationError> {
    let header = block.header.as_ref().unwrap();

    // All parents must exist in DAG
    for parent_hash in &header.parent_hashes {
        if !dag.contains(parent_hash) {
            return Err(ValidationError::UnknownParent(parent_hash.clone()));
        }
    }

    // Parents must be earlier than this block (no cycles)
    for parent_hash in &header.parent_hashes {
        let parent = dag.get_block(parent_hash)?;

        if parent.header.timestamp >= header.timestamp {
            return Err(ValidationError::ParentNotEarlier);
        }
    }

    // Parents should come from fork choice (optional, can log warning)
    let expected_parents = run_fork_choice(dag, &block.justifications)?;
    if !expected_parents.eq(&header.parent_hashes) {
        log::warn!(
            "Block parents {:?} differ from fork choice {:?}",
            header.parent_hashes,
            expected_parents
        );
        // Not an error - validators may use different fork choice
    }

    Ok(())
}
```

**3. Sequence Number Validation**:

```rust
fn validate_sequence_number(
    block: &BlockMessage,
    dag: &BlockDagRepresentation,
) -> Result<(), ValidationError> {
    let creator = &block.sender;
    let seq_num = block.seq_num;

    // Check if we already have a block with this (validator, seq_num)
    if let Some(existing) = dag.get_block_by_validator_and_seq(creator, seq_num) {
        // Two blocks with same sequence number = direct equivocation
        if existing.block_hash != block.block_hash {
            return Err(ValidationError::DirectEquivocation {
                validator: creator.clone(),
                seq_num,
                block1: existing.block_hash.clone(),
                block2: block.block_hash.clone(),
            });
        } else {
            // Same block, already have it
            return Err(ValidationError::DuplicateBlock);
        }
    }

    // Sequence number must be exactly previous + 1 (no skipping)
    if seq_num > 0 {
        let prev_exists = dag.get_block_by_validator_and_seq(creator, seq_num - 1).is_some();
        if !prev_exists {
            return Err(ValidationError::SkippedSequenceNumber { expected: seq_num - 1 });
        }
    }

    Ok(())
}
```

**4. Shard Identifier**:

```rust
// Verify block is for correct shard (in multi-shard setup)
if header.shard_id != expected_shard_id {
    return Err(ValidationError::WrongShard);
}
```

**5. Bonds Cache Consistency**:

```rust
// Verify bonds (validator set) matches expected
// Bonds only change at finalized blocks
let expected_bonds = dag.get_bonds_at_finalized_block()?;
if header.bonds_hash != hash(&expected_bonds) {
    return Err(ValidationError::InvalidBondsCache);
}
```

### Why Consensus Validation Third?

**Logical Order**: Can't check consensus rules without valid structure and crypto.

**Byzantine Detection**: Equivocations need to be identified before state execution.

**Dependencies**: Need to know if parents exist before trying to merge their states.

## Layer 4: Dependencies Check

### Purpose

Ensure all parent blocks are available before attempting state merge.

### Process

```rust
fn check_dependencies(
    block: &BlockMessage,
    dag: &BlockDagRepresentation,
) -> Result<(), ValidationError> {
    let header = block.header.as_ref().unwrap();

    // Check all parents are in DAG
    for parent_hash in &header.parent_hashes {
        if !dag.contains(parent_hash) {
            // Parent not yet received
            return Err(ValidationError::MissingDependency(parent_hash.clone()));
        }
    }

    // Check all justified blocks are in DAG
    for justification in &block.justifications {
        if !dag.contains(&justification.latest_block_hash) {
            return Err(ValidationError::MissingDependency(
                justification.latest_block_hash.clone()
            ));
        }
    }

    Ok(())
}
```

**Handling Missing Dependencies**:

```rust
// In block processor
match validate_block(block, dag) {
    Err(ValidationError::MissingDependency(hash)) => {
        // Request missing dependency from peer
        request_block_from_peer(peer, &hash).await?;

        // Store this block for later validation
        pending_blocks.insert(block.block_hash.clone(), block);

        // Will retry when dependency arrives
        Ok(ValidationStatus::Pending)
    }
    Err(e) => Err(e),
    Ok(()) => Ok(ValidationStatus::Valid),
}
```

### Why Separate Dependencies Check?

**Async Nature**: Dependencies may arrive out of order due to network.

**Buffering**: Need to buffer blocks until dependencies arrive.

**Prevent Cascade**: Don't want to reject block for missing parent when parent is valid but delayed.

## Layer 5: State Transition Validation

### Purpose

Ensure block's claimed state changes match actual execution of deploys.

### Process

**1. Merge Parent States**:

```rust
fn merge_parent_states(
    parents: &[BlockHash],
    rspace: &RSpace,
) -> Result<Blake2b256Hash, ValidationError> {
    // Get post-state hash from each parent
    let parent_states: Vec<_> = parents.iter()
        .map(|p| dag.get_block(p).unwrap().body.post_state_hash.clone())
        .collect();

    // Merge states (RSpace operation)
    let merged_hash = rspace.merge_states(&parent_states)?;

    Ok(merged_hash)
}
```

See [../../rholang/04-rspace-tuplespace/produce-consume.md](../../rholang/04-rspace-tuplespace/produce-consume.md) for RSpace merge semantics.

**2. Verify Pre-State**:

```rust
fn validate_pre_state(
    block: &BlockMessage,
    rspace: &RSpace,
) -> Result<(), ValidationError> {
    let expected_pre_state = merge_parent_states(&block.header.parent_hashes, rspace)?;

    if block.body.pre_state_hash != expected_pre_state {
        return Err(ValidationError::InvalidPreStateHash {
            expected: expected_pre_state,
            actual: block.body.pre_state_hash.clone(),
        });
    }

    Ok(())
}
```

**3. Execute Deploys**:

```rust
fn validate_post_state(
    block: &BlockMessage,
    rspace: &RSpace,
    interpreter: &Interpreter,
) -> Result<(), ValidationError> {
    // Reset RSpace to pre-state
    rspace.reset(&block.body.pre_state_hash)?;

    // Execute each deploy
    for deploy in &block.body.deploys {
        interpreter.execute_deploy(deploy, rspace)?;
    }

    // Create checkpoint (compute post-state hash)
    let checkpoint = rspace.create_checkpoint()?;
    let computed_post_state = checkpoint.root;

    // Verify matches claimed post-state
    if computed_post_state != block.body.post_state_hash {
        return Err(ValidationError::InvalidPostStateHash {
            expected: computed_post_state,
            actual: block.body.post_state_hash.clone(),
        });
    }

    Ok(())
}
```

**4. Deploy Validation**:

```rust
fn validate_deploys(
    block: &BlockMessage,
    dag: &BlockDagRepresentation,
) -> Result<(), ValidationError> {
    let deploy_hashes: HashSet<_> = block.body.deploys.iter()
        .map(|d| hash_deploy(d))
        .collect();

    // No duplicate deploys in this block
    if deploy_hashes.len() != block.body.deploys.len() {
        return Err(ValidationError::DuplicateDeploys);
    }

    // No replayed deploys (already in finalized blocks)
    let finalized_deploys = dag.get_finalized_deploys()?;
    for deploy_hash in &deploy_hashes {
        if finalized_deploys.contains(deploy_hash) {
            return Err(ValidationError::ReplayedDeploy(deploy_hash.clone()));
        }
    }

    // Each deploy has valid signature, phlogiston limit, etc.
    for deploy in &block.body.deploys {
        validate_deploy(deploy)?;
    }

    Ok(())
}
```

### Why State Transition Last?

**Cost**: Most expensive validation (milliseconds vs. microseconds).

**Dependencies**: Requires all parents' states to be available.

**Byzantine**: Invalid state transitions may indicate Byzantine behavior requiring slashing.

## Layer 6: Byzantine Validation

### Purpose

Detect and handle Byzantine behavior (equivocations, neglected equivocations).

### Checks

**1. Direct Equivocation**:

Already covered in Layer 3 (sequence number validation).

**2. Neglected Equivocation**:

```rust
fn check_neglected_equivocations(
    block: &BlockMessage,
    dag: &BlockDagRepresentation,
) -> Result<(), ValidationError> {
    // For each validator
    for validator in dag.validators() {
        // Check if this validator has equivocated
        if let Some(equivocation) = dag.get_equivocation(validator) {
            // Has block creator seen this equivocation?
            let creator_sees_equivocation = block.justifications.iter()
                .any(|j| j.validator == *validator &&
                         is_descendant_of_equivocation(&j.latest_block_hash, equivocation, dag));

            if creator_sees_equivocation {
                // Creator MUST include equivocation in justifications
                let includes_equivocation = block.justifications.iter()
                    .any(|j| j.validator == *validator &&
                             is_equivocator_message(&j.latest_block_hash, dag));

                if !includes_equivocation {
                    return Err(ValidationError::NeglectedEquivocation {
                        neglector: block.sender.clone(),
                        equivocator: validator.clone(),
                    });
                }
            }
        }
    }

    Ok(())
}
```

See [equivocation-detection.md](equivocation-detection.md) for complete details.

**3. Admissible vs. Ignorable Equivocations**:

```rust
fn classify_equivocation(
    block: &BlockMessage,  // Equivocating block
    dag: &BlockDagRepresentation,
) -> EquivocationClass {
    // Check if any honest validator has built on this equivocation
    let referenced_by_honest = dag.blocks_iter()
        .filter(|b| !dag.is_equivocator(&b.sender))
        .any(|b| b.justifications.iter()
            .any(|j| j.latest_block_hash == block.block_hash));

    if referenced_by_honest {
        EquivocationClass::Admissible  // Include in DAG for accountability
    } else {
        EquivocationClass::Ignorable   // Drop to prevent spam
    }
}
```

## Validation Performance

### Typical Timings

```
Layer 1 (Format):              ~10 μs
Layer 2 (Crypto):              ~100 μs (signature verification)
Layer 3 (Consensus):           ~500 μs (DAG lookups, checks)
Layer 4 (Dependencies):        ~50 μs (hash map lookups)
Layer 5 (State Transition):    ~10 ms (deploy execution)
Layer 6 (Byzantine):           ~200 μs (equivocation checks)

Total: ~11 ms per block
```

**Bottleneck**: State transition (deploy execution) dominates.

### Optimizations

**1. Parallel Validation**:

```rust
// Validate multiple blocks concurrently
let validations: Vec<_> = blocks.par_iter()
    .map(|block| validate_block(block, dag))
    .collect();
```

**Limitation**: State transition must be sequential (depends on parent states).

**2. Lazy State Validation**:

```rust
// Validate format, crypto, consensus immediately
// Defer state validation until block is in fork choice path
fn validate_lazy(block: &BlockMessage, dag: &DAG) -> Result<(), Error> {
    validate_format(block)?;
    validate_crypto(block)?;
    validate_consensus(block, dag)?;
    check_dependencies(block, dag)?;

    // Mark block as "tentatively valid"
    dag.add_tentative(block);

    // Validate state only when block becomes parent candidate
    Ok(())
}
```

**Benefit**: Don't execute deploys for blocks that won't be built upon.

**3. State Transition Caching**:

```rust
// Cache execution results
struct StateTransitionCache {
    cache: LruCache<(PreStateHash, Vec<DeployHash>), PostStateHash>,
}

impl StateTransitionCache {
    fn get_or_compute(
        &mut self,
        pre_state: &Blake2b256Hash,
        deploys: &[Deploy],
        executor: impl FnOnce() -> Result<Blake2b256Hash, Error>,
    ) -> Result<Blake2b256Hash, Error> {
        let key = (pre_state.clone(), deploys.iter().map(hash_deploy).collect());

        if let Some(post_state) = self.cache.get(&key) {
            return Ok(post_state.clone());
        }

        let post_state = executor()?;
        self.cache.put(key, post_state.clone());
        Ok(post_state)
    }
}
```

**Benefit**: If multiple blocks have same pre-state and deploys, reuse result.

## Error Handling

### Validation Errors

```rust
pub enum ValidationError {
    // Format errors
    InvalidBlockHashLength,
    MissingHeader,
    BlockTooLarge,

    // Crypto errors
    BlockHashMismatch,
    InvalidSignature,

    // Consensus errors
    MissingCreatorJustification,
    UnknownParent(BlockHash),
    DirectEquivocation { validator: PublicKey, seq_num: i32, block1: BlockHash, block2: BlockHash },
    NeglectedEquivocation { neglector: PublicKey, equivocator: PublicKey },

    // State errors
    InvalidPreStateHash { expected: Blake2b256Hash, actual: Blake2b256Hash },
    InvalidPostStateHash { expected: Blake2b256Hash, actual: Blake2b256Hash },
    DeployExecutionFailed(String),

    // Dependency errors
    MissingDependency(BlockHash),
}
```

### Handling Strategy

```rust
match validate_block(block, dag) {
    Ok(()) => {
        // Add to DAG
        dag.add_block(block)?;
        log::info!("Block {} validated and added", block.block_hash);
        Ok(ValidationStatus::Valid)
    }

    Err(ValidationError::MissingDependency(hash)) => {
        // Request dependency, buffer block
        request_block(hash).await?;
        pending_blocks.insert(block.block_hash.clone(), block);
        Ok(ValidationStatus::Pending)
    }

    Err(ValidationError::DirectEquivocation { validator, .. }) => {
        // Record equivocation for slashing
        dag.record_equivocation(validator, block)?;
        log::warn!("Equivocation detected from {}", validator);
        Ok(ValidationStatus::Equivocation)
    }

    Err(ValidationError::NeglectedEquivocation { neglector, equivocator }) => {
        // Reject block, possibly ban peer
        log::error!(
            "Block {} neglects equivocation from {}",
            block.block_hash, equivocator
        );
        Err(ValidationError::NeglectedEquivocation { neglector, equivocator })
    }

    Err(e) => {
        // Invalid block, reject
        log::error!("Block {} invalid: {:?}", block.block_hash, e);
        Err(e)
    }
}
```

## Integration with Network Layer

### Block Reception Flow

```
Network receives BlockHashMessage:
    ↓
Check if already have block:
    ├─ Yes → Ignore
    ↓ No
Request full BlockMessage:
    ↓
Receive BlockMessage:
    ↓
Validate block (all layers):
    ├─ Valid → Add to DAG
    ├─ Pending → Buffer, request dependencies
    └─ Invalid → Reject, possibly ban peer
```

### Peer Reputation

```rust
struct PeerReputation {
    invalid_blocks_sent: u32,
    equivocations_sent: u32,
    valid_blocks_sent: u32,
}

fn update_reputation(peer: &PeerId, result: ValidationResult) {
    match result {
        Ok(ValidationStatus::Valid) => {
            peer.reputation.valid_blocks_sent += 1;
        }
        Err(ValidationError::InvalidSignature) |
        Err(ValidationError::BlockHashMismatch) => {
            // Severe: cryptographic forgery
            peer.reputation.invalid_blocks_sent += 10;
            if peer.reputation.invalid_blocks_sent > 50 {
                ban_peer(peer);
            }
        }
        Err(ValidationError::DirectEquivocation { .. }) => {
            // Moderate: Byzantine behavior
            peer.reputation.equivocations_sent += 1;
        }
        _ => {
            // Minor: honest mistakes, network issues
            peer.reputation.invalid_blocks_sent += 1;
        }
    }
}
```

## Testing Block Validation

### Unit Tests

```rust
#[test]
fn test_invalid_signature() {
    let mut block = create_test_block();
    // Corrupt signature
    block.sig[0] ^= 0xFF;

    let result = validate_crypto(&block);
    assert!(matches!(result, Err(ValidationError::InvalidSignature)));
}

#[test]
fn test_direct_equivocation() {
    let validator = create_test_validator();
    let block1 = create_block(validator, seq_num: 5, ...);
    let block2 = create_block(validator, seq_num: 5, ...);  // Same seq_num!

    let mut dag = BlockDagRepresentation::new();
    dag.add_block(block1).unwrap();

    let result = validate_consensus(&block2, &dag);
    assert!(matches!(result, Err(ValidationError::DirectEquivocation { .. })));
}
```

### Integration Tests

```rust
#[tokio::test]
async fn test_full_validation_pipeline() {
    let rspace = RSpace::new(...);
    let dag = BlockDagRepresentation::new();

    // Create valid block
    let block = create_valid_block(&dag, &rspace);

    // Validate
    let result = validate_block(&block, &dag, &rspace).await;
    assert!(result.is_ok());

    // Add to DAG
    dag.add_block(block).unwrap();

    // Verify in DAG
    assert!(dag.contains(&block.block_hash));
}
```

### Property-Based Tests

```rust
#[quickcheck]
fn prop_valid_blocks_always_validate(block: ArbitraryValidBlock) -> bool {
    let dag = setup_dag_with_dependencies(&block);
    let rspace = setup_rspace_with_state(&block);

    validate_block(&block.0, &dag, &rspace).is_ok()
}

#[quickcheck]
fn prop_invalid_signature_always_rejected(mut block: ArbitraryBlock) -> bool {
    // Corrupt signature
    block.sig[0] ^= 0xFF;

    validate_crypto(&block).is_err()
}
```

## Summary

Block validation is a **multi-layer pipeline** ensuring Byzantine fault tolerance:

**Layers**:
1. **Format** - Structure and encoding (~10 μs)
2. **Cryptographic** - Signatures and hashes (~100 μs)
3. **Consensus** - Justifications, parents, sequence numbers (~500 μs)
4. **Dependencies** - Parent availability (~50 μs)
5. **State Transition** - Deploy execution (~10 ms) ← bottleneck
6. **Byzantine** - Equivocation detection (~200 μs)

**Total**: ~11 ms per block (dominated by state execution)

**Properties**:
- **Safety**: Invalid blocks are rejected
- **Liveness**: Valid blocks are accepted
- **Byzantine Tolerance**: Equivocations detected and handled
- **Performance**: Optimized for common case (valid blocks)

**Key Insights**:
- Cheap validations first (fail fast)
- Expensive state execution last (skip if earlier checks fail)
- Dependencies handled asynchronously (buffer and retry)
- Byzantine behavior recorded for accountability (slashing)

## Further Reading

- [Justifications](../01-fundamentals/justifications.md) - Consensus validation details
- [Equivocation Detection](equivocation-detection.md) - Byzantine behavior handling
- [Fork Choice Estimator](fork-choice-estimator.md) - Parent selection validation
- [RSpace State Transitions](../../rholang/04-rspace-tuplespace/produce-consume.md) - State merge and execution

---

**Navigation**: [← Fork Choice](fork-choice-estimator.md) | [Equivocation Detection →](equivocation-detection.md) | [Safety Oracle →](safety-oracle.md)
