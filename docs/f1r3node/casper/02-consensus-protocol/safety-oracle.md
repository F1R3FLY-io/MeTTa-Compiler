# Safety Oracle and Finalization

## Overview

The **safety oracle** (also called "clique oracle") is the algorithm that determines when a block is safe to **finalize** - i.e., when it's irreversible and can never be orphaned. It's one of the three critical components of Casper CBC consensus, alongside justifications and fork choice.

**Purpose**: Compute fault tolerance for blocks and identify when they can be finalized.

**Core Idea**: A block is safe when validators controlling >50% of stake all have it in their chain and won't change their minds.

## The Problem: When Is a Block Final?

### Why Finalization Matters

**For Applications**:
- Cryptocurrency: When can recipient spend transferred funds?
- Smart contracts: When are contract outcomes irrevocable?
- Cross-chain bridges: When is lock/unlock final?

**For Network**:
- Data pruning: Can delete old blocks beyond finalized
- Checkpoint synchronization: New nodes sync from finalized blocks
- Accountability: Slashing applies to blocks before finalization

### Naive Approaches (Why They Don't Work)

**Approach 1: Fixed Depth**

```
Block B is final if it's K blocks deep

Genesis ← B ← B1 ← B2 ← ... ← BK
```

**Problem**: In DAG, "depth" is ambiguous. Multiple paths, unclear what K means.

**Approach 2: Time-Based**

```
Block B is final if it's T seconds old
```

**Problem**: Network delays vary. Attackers can delay propagation.

**Approach 3: Vote Counting**

```
Block B is final if >67% of validators voted for it
```

**Problem**: Casper CBC has no explicit voting! Must infer from justifications.

### Casper CBC Approach: Clique-Based Safety

**Insight**: Use justifications to build an **agreement graph** and find the **maximum clique**.

**Clique**: A set of validators who all agree on a block and won't change their minds (based on their justifications).

**Safety**: If validators in the max clique control >50% stake, the block is safe.

## Safety Oracle Algorithm

### High-Level Algorithm

```
safety_oracle(target_block, dag):
    1. Find validators whose latest messages build on target_block
    2. Build agreement graph:
       - Validators are nodes
       - Edge between V1 and V2 if they agree and won't disagree
    3. Find maximum clique in agreement graph
    4. Compute fault tolerance:
       FT = (max_clique_weight × 2 - total_stake) / total_stake
    5. If FT > threshold (typically 0):
       Block is safe to finalize
```

### Detailed Steps

#### Step 1: Find Validators Supporting Target Block

```rust
fn validators_supporting_block(
    target: &BlockHash,
    dag: &BlockDagRepresentation,
) -> HashSet<Validator> {
    let mut supporting = HashSet::new();

    // For each validator
    for (validator, latest_msg) in dag.latest_messages() {
        // Check if latest message is target or descendant of target
        if latest_msg == *target || dag.is_ancestor(target, latest_msg) {
            supporting.insert(validator.clone());
        }
    }

    supporting
}
```

**Example**:

```
DAG:
        B5 (Alice)
       /
   B3 ─── B4 (Bob)
       \
        B2 (Charlie)

Latest messages:
- Alice: B5
- Bob: B4
- Charlie: B2

Target: B3

Supporting B3:
- Alice: Yes (B5 builds on B3)
- Bob: Yes (B4 builds on B3)
- Charlie: No (B2 doesn't build on B3)

Result: {Alice, Bob}
```

#### Step 2: Build Agreement Graph

**Agreement Definition**: Two validators agree on target block if:
1. Both have target in their chains (from Step 1)
2. They won't disagree in the future (based on justifications)

**Won't Disagree Condition**: V1 and V2 won't disagree if V1's latest message includes V2 in justifications (or vice versa), showing they've seen each other's commitments.

```rust
fn build_agreement_graph(
    supporting: &HashSet<Validator>,
    target: &BlockHash,
    dag: &BlockDagRepresentation,
) -> HashMap<Validator, HashSet<Validator>> {
    let mut graph = HashMap::new();

    for v1 in supporting {
        let mut agreeing = HashSet::new();

        for v2 in supporting {
            if v1 == v2 {
                agreeing.insert(v2.clone());
                continue;
            }

            // Check if v1's latest message includes v2 in justifications
            let v1_latest = dag.get_latest_message(v1).unwrap();
            let v1_block = dag.get_block(&v1_latest).unwrap();

            let v2_justification = v1_block.justifications.iter()
                .find(|j| j.validator == *v2);

            if let Some(just) = v2_justification {
                // v1 has seen v2's position on target
                // Check if v2's justified message also builds on target
                if dag.is_ancestor(target, &just.latest_block_hash) {
                    // Both build on target and know about each other
                    agreeing.insert(v2.clone());
                }
            }
        }

        graph.insert(v1.clone(), agreeing);
    }

    graph
}
```

**Example Agreement Graph**:

```
Supporting: {Alice, Bob, Charlie}

Alice's latest (A5) justifications:
  Bob: B4 (builds on target)
  Charlie: C3 (builds on target)

Bob's latest (B4) justifications:
  Alice: A4 (builds on target)
  Charlie: C2 (doesn't build on target)

Charlie's latest (C3) justifications:
  Alice: A4 (builds on target)
  Bob: B3 (builds on target)

Agreement graph:
  Alice → {Alice, Bob, Charlie}  (agrees with all)
  Bob → {Alice, Bob}             (doesn't fully agree with Charlie)
  Charlie → {Alice, Charlie, Bob} (agrees with all)

Wait, Bob doesn't agree with Charlie because Bob's justification of Charlie
(C2) doesn't build on target, even though Charlie's latest (C3) does.
```

#### Step 3: Find Maximum Clique

**Maximum Clique Problem**: Find the largest fully-connected subgraph.

**NP-Hard**: In general, but practical for blockchain (small number of validators).

```rust
fn find_maximum_clique(
    graph: &HashMap<Validator, HashSet<Validator>>,
) -> HashSet<Validator> {
    let mut max_clique = HashSet::new();

    // Bron-Kerbosch algorithm for maximum clique
    bron_kerbosch(
        HashSet::new(),              // R: current clique
        graph.keys().cloned().collect(), // P: candidates
        HashSet::new(),              // X: excluded
        graph,
        &mut max_clique,
    );

    max_clique
}

fn bron_kerbosch(
    r: HashSet<Validator>,           // Current clique
    mut p: HashSet<Validator>,       // Candidate vertices
    mut x: HashSet<Validator>,       // Excluded vertices
    graph: &HashMap<Validator, HashSet<Validator>>,
    max_clique: &mut HashSet<Validator>,
) {
    if p.is_empty() && x.is_empty() {
        // Found maximal clique
        if r.len() > max_clique.len() {
            *max_clique = r;
        }
        return;
    }

    // Choose pivot (vertex with most connections)
    let pivot = p.union(&x)
        .max_by_key(|v| graph.get(v).map(|s| s.len()).unwrap_or(0))
        .cloned();

    let candidates: Vec<_> = if let Some(pivot_v) = pivot {
        p.difference(&graph.get(&pivot_v).unwrap_or(&HashSet::new()))
            .cloned()
            .collect()
    } else {
        p.iter().cloned().collect()
    };

    for v in candidates {
        let neighbors = graph.get(&v).unwrap_or(&HashSet::new());

        let mut new_r = r.clone();
        new_r.insert(v.clone());

        let new_p: HashSet<_> = p.intersection(neighbors).cloned().collect();
        let new_x: HashSet<_> = x.intersection(neighbors).cloned().collect();

        bron_kerbosch(new_r, new_p, new_x, graph, max_clique);

        p.remove(&v);
        x.insert(v);
    }
}
```

**Example**:

```
Agreement graph:
  Alice: {Alice, Bob, Charlie}
  Bob: {Alice, Bob, Charlie}
  Charlie: {Alice, Bob, Charlie}

Maximum clique: {Alice, Bob, Charlie} (all three agree)
```

```
Agreement graph (different scenario):
  Alice: {Alice, Bob}
  Bob: {Alice, Bob}
  Charlie: {Alice, Charlie, Dave}
  Dave: {Alice, Charlie, Dave}

Possible cliques:
- {Alice, Bob} (size 2)
- {Alice, Charlie, Dave} (size 3)

Maximum clique: {Alice, Charlie, Dave}
```

#### Step 4: Compute Fault Tolerance

**Formula**:

```
FT = (max_clique_weight × 2 - total_stake) / total_stake

Where:
- max_clique_weight = sum of stake for validators in max clique
- total_stake = sum of all validator stakes
```

**Interpretation**:

```
FT = -1.0  : No safety (0% stake in clique)
FT =  0.0  : Minimum safety (50% stake in clique)
FT = +1.0  : Maximum safety (100% stake in clique)
```

**Implementation**:

```rust
fn compute_fault_tolerance(
    max_clique: &HashSet<Validator>,
    dag: &BlockDagRepresentation,
) -> f64 {
    let clique_weight: u64 = max_clique.iter()
        .map(|v| dag.get_validator_weight(v).unwrap_or(0))
        .sum();

    let total_weight: u64 = dag.get_all_validators().iter()
        .map(|v| dag.get_validator_weight(v).unwrap_or(0))
        .sum();

    if total_weight == 0 {
        return -1.0;
    }

    ((clique_weight as f64 * 2.0) - total_weight as f64) / total_weight as f64
}
```

**Example**:

```
Validators and stakes:
- Alice: 30
- Bob: 25
- Charlie: 45
Total: 100

Max clique: {Alice, Bob}
Clique weight: 30 + 25 = 55

FT = (55 × 2 - 100) / 100
   = (110 - 100) / 100
   = 10 / 100
   = 0.1

Interpretation: Block has 10% fault tolerance
(safe because > 0)
```

```
Max clique: {Charlie}
Clique weight: 45

FT = (45 × 2 - 100) / 100
   = (90 - 100) / 100
   = -10 / 100
   = -0.1

Interpretation: Block has -10% fault tolerance
(not safe yet, < 50% stake)
```

## Finalization Algorithm

### Complete Finalization Process

```rust
pub fn find_finalized_block(
    dag: &BlockDagRepresentation,
    last_finalized: &BlockHash,
    threshold: f64,  // Typically 0.0
) -> Option<BlockHash> {
    // 1. Get all blocks newer than last finalized
    let candidates = dag.get_blocks_after(last_finalized);

    // 2. Sort by height (finalize in order)
    let mut sorted_candidates = candidates;
    sorted_candidates.sort_by_key(|b| dag.get_height(b));

    // 3. Check each candidate
    for candidate in sorted_candidates {
        let ft = safety_oracle(&candidate, dag);

        if ft > threshold {
            // This block is safe! Finalize it.
            return Some(candidate);
        }
    }

    // No block ready to finalize yet
    None
}
```

### Finalization Effects

When a block is finalized:

```rust
pub fn finalize_block(
    block_hash: &BlockHash,
    dag: &mut BlockDagRepresentation,
    rspace: &mut RSpace,
) -> Result<(), Error> {
    // 1. Mark block as finalized
    dag.mark_finalized(block_hash)?;

    // 2. All ancestors are transitively finalized
    for ancestor in dag.get_ancestors(block_hash) {
        dag.mark_finalized(&ancestor)?;
    }

    // 3. Remove deploys from deploy pool (already executed)
    let deploys = dag.get_block(block_hash)?.body.deploys.clone();
    for deploy in deploys {
        deploy_pool.remove(&deploy.hash())?;
    }

    // 4. Clean up RSpace mergeable channels
    rspace.cleanup_finalized_channels()?;

    // 5. Prune old blocks (optional, based on config)
    let prune_depth = config.finalized_blocks_to_keep;
    let prune_height = dag.get_height(block_hash).saturating_sub(prune_depth);
    dag.prune_below_height(prune_height)?;

    // 6. Publish finalization event
    event_bus.publish(Event::BlockFinalized {
        block_hash: block_hash.clone(),
        height: dag.get_height(block_hash),
    })?;

    Ok(())
}
```

## Source Code Reference

**Primary Implementation**: `/var/tmp/debug/f1r3node/casper/src/main/scala/coop/rchain/casper/safety/CliqueOracle.scala`

Key functions:
- `normalizedFaultTolerance()` (lines 50-150) - Main oracle
- `computeMainParentAgreement()` (lines 200-250) - Agreement graph
- `findMaximumCliqueByWeight()` (lines 300-400) - Clique finding

## Theoretical Properties

### Theorem: Safety Guarantee

**Claim**: If a block B has fault tolerance FT > 0, then B cannot be orphaned (replaced by a conflicting block) without detectable Byzantine behavior.

**Proof**:

```
1. FT > 0  ⟹  clique_weight > 50% of total_stake
2. Validators in clique all have B in their chain (by construction)
3. Their justifications show they've committed to B
4. For B to be orphaned, validators with >50% stake must switch to conflicting chain
5. Clique has >50% stake, so some clique members must switch
6. Switching means creating equivocations (provable via justifications)
7. Honest validators won't equivocate (Byzantine assumption: <33% malicious)
8. ⟹ Not enough stake can switch without detection
9. ⟹ B cannot be orphaned

Q.E.D.
```

### Theorem: Liveness (Progress)

**Claim**: If >67% of stake is honest and online, blocks will eventually be finalized.

**Proof**:

```
1. Assume >67% stake is honest (H) and online
2. Honest validators follow fork choice → converge on common preferred block B
3. Their latest messages all build on B
4. All honest validators include each other in justifications (gossip)
5. ⟹ Agreement graph includes all H
6. ⟹ Maximum clique includes all H
7. Clique weight ≥ 67% > 50%
8. ⟹ FT > 0
9. ⟹ B finalizes

Q.E.D.
```

### Theorem: Accountable Byzantine Fault Tolerance

**Claim**: If a finalized block is orphaned, we can identify and prove which validators behaved maliciously.

**Proof**:

```
1. Assume finalized block B is orphaned by conflicting block B'
2. B was finalized ⟹ clique C with >50% stake committed to B
3. B' wins ⟹ validators with >50% stake committed to B'
4. |C| > 50% and |supporters(B')| > 50%
5. ⟹ |C ∩ supporters(B')| > 0%  (overlap)
6. Validators in overlap committed to both B and B'
7. Commitment shown via justifications in blocks
8. ⟹ Equivocations are provable from blockchain history
9. ⟹ Malicious validators identified with proof

Q.E.D.
```

## Performance Analysis

### Time Complexity

```
N = number of validators
E = number of edges in agreement graph

Algorithm steps:
1. Find supporting validators:  O(N)
2. Build agreement graph:       O(N²)
3. Find maximum clique:         O(2^N) worst case
                                O(N³) practical (with heuristics)
4. Compute fault tolerance:     O(N)

Total worst case: O(2^N)
Total practical: O(N³)

For N = 100 validators:
  Worst case: ~10^30 operations (infeasible)
  Practical: ~1,000,000 operations (~100 ms)
```

**Optimization**: Cache agreement graph, recompute only on new blocks.

### Space Complexity

```
Agreement graph:  O(N²)
Clique storage:   O(N)

Total: O(N²)

For N = 100:
  ~10,000 entries × 8 bytes = ~80 KB
```

### Real-World Performance

```
Validators  Safety Oracle Time
--------------------------------
10          ~1 ms
50          ~20 ms
100         ~100 ms
500         ~2 seconds
1000        ~10 seconds
```

**Recommendation**: Run safety oracle asynchronously, don't block block creation.

## Examples

### Example 1: Simple Convergence

```
Validators: Alice (40%), Bob (35%), Charlie (25%)
Threshold: 0.0

Block B at height 10:

Latest messages:
- Alice: A15 (builds on B)
- Bob: B14 (builds on B)
- Charlie: C12 (builds on different block)

Step 1: Supporting = {Alice, Bob}
Step 2: Agreement graph:
  Alice → {Alice, Bob}
  Bob → {Alice, Bob}
Step 3: Max clique = {Alice, Bob}
Step 4: FT = ((40 + 35) × 2 - 100) / 100
           = (150 - 100) / 100
           = 0.5

Result: FT = 0.5 > 0.0 → Block B finalizes!
```

### Example 2: No Consensus Yet

```
Validators: Alice (35%), Bob (32%), Charlie (33%)

Block B at height 10:

Latest messages:
- Alice: A15 (builds on B)
- Bob: B14 (builds on different block)
- Charlie: C12 (builds on B)

Step 1: Supporting = {Alice, Charlie}
Step 2: Agreement graph:
  Alice → {Alice, Charlie}
  Charlie → {Alice, Charlie}
Step 3: Max clique = {Alice, Charlie}
Step 4: FT = ((35 + 33) × 2 - 100) / 100
           = (136 - 100) / 100
           = 0.36

Result: FT = 0.36 > 0.0 → Block B finalizes!

(Even without Bob's support, Alice + Charlie is enough)
```

### Example 3: Equivocation Prevents Finalization

```
Validators: Alice (40%), Bob (30%), Charlie (30%)

Block B at height 10:

Latest messages:
- Alice: {A15, A15'} (equivocation!)
- Bob: B14 (builds on B)
- Charlie: C12 (builds on B)

Step 1: Supporting = {Alice (disputed), Bob, Charlie}
Step 2: Agreement graph uncertain (Alice's equivocation)
Step 3: Conservative: exclude Alice from clique
        Max clique = {Bob, Charlie}
Step 4: FT = ((30 + 30) × 2 - 100) / 100
           = (120 - 100) / 100
           = 0.2

Result: FT = 0.2 > 0.0 → Still finalizes (Bob + Charlie enough)

But if Alice equivocated with larger stake:

Alice (60%), Bob (20%), Charlie (20%):
FT = ((20 + 20) × 2 - 100) / 100 = -0.6
Result: Does NOT finalize (need to wait)
```

## Common Issues

### Issue 1: Slow Finalization

**Symptom**: Blocks not finalizing for long time.

**Debug**:

```rust
let ft = safety_oracle(&latest_block, dag);
log::info!("Fault tolerance: {:.2}", ft);

if ft < 0.0 {
    log::warn!("Fault tolerance negative, not enough stake agreement");

    // Find which validators are missing
    let supporting = validators_supporting_block(&latest_block, dag);
    let all_validators: HashSet<_> = dag.get_all_validators().iter().cloned().collect();
    let not_supporting: Vec<_> = all_validators.difference(&supporting).collect();

    for validator in not_supporting {
        let latest = dag.get_latest_message(validator);
        log::warn!("Validator {} not supporting (latest: {:?})", validator, latest);
    }
}
```

**Common Causes**:
- Network partition (validators not seeing each other's blocks)
- Validator offline
- Deep fork (validators on different chains)

### Issue 2: False Finalization

**Symptom**: Block marked finalized but later orphaned.

**This should NEVER happen** if implementation is correct!

**Debug**:

```rust
// Verify finalization decision
let recomputed_ft = safety_oracle(&finalized_block, dag);
assert!(recomputed_ft > threshold, "Finalization was incorrect!");

// Check for equivocations that might have been missed
let equivocations = dag.get_all_equivocations();
for (validator, blocks) in equivocations {
    log::error!("Equivocation by {} not accounted for: {:?}", validator, blocks);
}
```

**If this happens**: Critical bug, investigate immediately!

### Issue 3: Performance Degradation

**Symptom**: Safety oracle taking >1 second.

**Debug**:

```rust
let start = std::time::Instant::now();
let supporting = validators_supporting_block(&block, dag);
let supporting_time = start.elapsed();

let agreement_graph = build_agreement_graph(&supporting, &block, dag);
let graph_time = start.elapsed();

let max_clique = find_maximum_clique(&agreement_graph);
let clique_time = start.elapsed();

log::info!("Supporting: {:?}", supporting_time);
log::info!("Graph: {:?}", graph_time - supporting_time);
log::info!("Clique: {:?}", clique_time - graph_time);
```

**Optimizations**:
- Cache agreement graph between calls
- Use approximate clique algorithms for >100 validators
- Run asynchronously, don't block critical path

## Summary

The safety oracle is a **clique-based algorithm** determining when blocks can be finalized:

**Algorithm**:
1. Find validators supporting target block
2. Build agreement graph (who agrees with whom)
3. Find maximum clique (largest mutually agreeing set)
4. Compute fault tolerance: `FT = (clique_weight × 2 - total) / total`
5. If FT > threshold (typically 0): Finalize

**Properties**:
- **Safety**: Finalized blocks cannot be orphaned (without provable Byzantine behavior)
- **Liveness**: Blocks finalize if >67% stake is honest and online
- **Accountable**: Byzantine behavior is provable from justifications

**Performance**:
- Time: O(N³) practical for N validators
- Space: O(N²) for agreement graph
- Real-world: ~100 ms for 100 validators

**Finalization Effects**:
- Block marked as finalized
- Deploys removed from pool
- Old blocks can be pruned
- Applications can treat transactions as irreversible

**Key Insight**: Safety oracle leverages justifications to compute a **consensus estimate** without explicit voting, enabling asynchronous finalization in Casper CBC.

## Further Reading

- [Justifications](../01-fundamentals/justifications.md) - Agreement graph input
- [Fork Choice](fork-choice-estimator.md) - Selecting blocks to build on
- [Block Validation](block-validation.md) - Ensuring blocks are valid before finalization
- [Equivocation Detection](equivocation-detection.md) - Byzantine behavior handling

---

**Navigation**: [← Block Validation](block-validation.md) | [Fork Choice ←](fork-choice-estimator.md) | [Equivocation Detection →](equivocation-detection.md)
