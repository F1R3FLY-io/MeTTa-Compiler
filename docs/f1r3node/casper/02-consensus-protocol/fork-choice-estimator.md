# Fork Choice Estimator

## Overview

The **fork choice estimator** (also called "estimator" or "parent selection algorithm") is the algorithm that determines which blocks a validator should build upon when creating a new block. It's one of the three critical components of Casper CBC consensus, alongside justifications and the safety oracle.

**Purpose**: Ensure all validators converge on a common preferred history without explicit coordination.

**Core Idea**: Score blocks by the cumulative weight of validators supporting each chain. Build on the highest-scored blocks.

## The Problem: Selecting Parents

### Why This Is Non-Trivial

In a multi-parent DAG, multiple valid blocks may be available as potential parents:

```
Current DAG state:

         B7 (Alice)
        /
    B4 ──── B6 (Bob)
        \
         B5 (Charlie)

Validator Dave wants to create B8.
Which blocks should be B8's parents?

Options:
1. Just B7 (latest from Alice)
2. Just B6 (latest from Bob)
3. B7 + B6 (merge Alice and Bob)
4. All three: B7 + B6 + B5
```

**Bad choices lead to**:
- Network fragmentation (validators diverge)
- Security vulnerabilities (attackers can influence fork choice)
- Poor performance (building on stale blocks)

**Good choices result in**:
- Convergence (all validators prefer same history)
- Security (Byzantine validators cannot manipulate)
- Performance (newest blocks with most state changes)

## The Solution: Weight-Based Scoring

### High-Level Algorithm

```
1. Get latest messages from all validators (from justifications)
2. Find Latest Common Ancestor (LCA) of all latest messages
3. Score each block from LCA to tips:
   - For each validator's latest message
   - Add validator's weight to all blocks in their chain
4. Select top-K highest-scored blocks as parents
   (where K is typically 1-5, configurable)
```

### Why This Works

**Intuition**: Validators with more stake (weight) have more influence. If validators controlling >50% of stake all build on block B, then B should be preferred.

**Mathematical Property**: This is a **greedy algorithm** that approximates the "most supported" blocks in the DAG.

**Convergence**: Honest validators using the same algorithm with the same view (justifications) will select the same parents.

## Implementation Deep Dive

### Source Code Reference

**Primary Implementation**: `/var/tmp/debug/f1r3node/casper/src/main/scala/coop/rchain/casper/Estimator.scala`

Key functions:
- `tips()` (lines 80-150): Main entry point
- `buildScoreMap()` (lines 200-250): Scoring algorithm
- `computeLCA()` (lines 300-350): Latest Common Ancestor

### Data Structures

**Latest Messages Map**:

```scala
type LatestMessages = Map[Validator, BlockHash]

// Example:
latestMessages = Map(
  "alice_pubkey" -> "hash_A7",
  "bob_pubkey"   -> "hash_B6",
  "charlie_pubkey" -> "hash_C5"
)
```

From justifications in latest blocks seen by this validator.

**Validator Weights**:

```scala
type ValidatorWeights = Map[Validator, Weight]

// Example (stake amounts):
weights = Map(
  "alice_pubkey" -> 30,   // 30% stake
  "bob_pubkey"   -> 25,   // 25% stake
  "charlie_pubkey" -> 45  // 45% stake
)
```

From bond table in latest finalized block.

**Score Map**:

```scala
type ScoreMap = Map[BlockHash, Weight]

// Example:
scoreMap = Map(
  "hash_B4" -> 100,  // All validators support B4
  "hash_B7" -> 30,   // Only Alice's chain
  "hash_B6" -> 55,   // Alice + Bob's chains
  "hash_B5" -> 45    // Only Charlie's chain
)
```

### Algorithm Steps

#### Step 1: Get Latest Messages

```scala
def tips(
  dag: BlockDagRepresentation[F],
  genesis: BlockMessage
): F[ForkChoice] = {
  for {
    // Get latest messages from all validators
    latestMessages <- dag.latestMessages

    // latestMessages = Map(
    //   validator1 -> latest_block_from_v1,
    //   validator2 -> latest_block_from_v2,
    //   ...
    // )
  } yield ...
}
```

These come from justifications in recently seen blocks.

#### Step 2: Compute Latest Common Ancestor (LCA)

```scala
def computeLCA(
  latestMessages: Map[Validator, BlockHash],
  dag: BlockDagRepresentation[F]
): F[BlockHash] = {
  // Find the most recent block that is an ancestor of ALL latest messages

  // Simple approach: find block with highest height that all latest messages build on
  val allAncestors = latestMessages.values.map(block => dag.ancestors(block))
  val commonAncestors = allAncestors.reduce((a, b) => a.intersect(b))

  commonAncestors.maxBy(block => dag.height(block))
}
```

**Example**:

```
Latest messages: A7, B6, C5

     A7
    /
   A6
  /  \
 A5  B6
  \ / \
  A4  C5
   \ /
   A3  ← LCA (most recent common ancestor)
    |
   ...
```

LCA is the "frontier" - blocks below this are agreed upon, blocks above are still being decided.

#### Step 3: Build Score Map

```scala
def buildScoreMap(
  latestMessages: Map[Validator, BlockHash],
  lca: BlockHash,
  dag: BlockDagRepresentation[F]
): F[ScoreMap] = {

  // Start with empty scores
  var scoreMap = Map.empty[BlockHash, Weight]

  // For each latest message
  for ((validator, blockHash) <- latestMessages) {
    val weight = dag.getValidatorWeight(validator)

    // Find path from this block back to LCA
    val path = dag.pathFromTo(blockHash, lca)  // [blockHash, parent, parent's parent, ..., LCA]

    // Add this validator's weight to all blocks in path
    for (block <- path) {
      scoreMap = scoreMap.updated(
        block,
        scoreMap.getOrElse(block, 0) + weight
      )
    }
  }

  scoreMap
}
```

**Example Walkthrough**:

```
Validators and weights:
- Alice (30 stake): latest message = A7
- Bob (25 stake): latest message = B6
- Charlie (45 stake): latest message = C5

LCA = A3

Paths:
- A7 → A6 → A5 → A4 → A3
- B6 → A4 → A3
- C5 → A4 → A3

Scoring:
1. Process Alice (weight 30):
   - A7: 0 + 30 = 30
   - A6: 0 + 30 = 30
   - A5: 0 + 30 = 30
   - A4: 0 + 30 = 30
   - A3: 0 + 30 = 30

2. Process Bob (weight 25):
   - B6: 0 + 25 = 25
   - A4: 30 + 25 = 55  ← cumulative
   - A3: 30 + 25 = 55

3. Process Charlie (weight 45):
   - C5: 0 + 45 = 45
   - A4: 55 + 45 = 100  ← all validators
   - A3: 55 + 45 = 100

Final ScoreMap:
  A3: 100 (all validators)
  A4: 100 (all validators)
  A5: 30  (Alice only)
  A6: 30  (Alice only)
  A7: 30  (Alice only)
  B6: 25  (Bob only)
  C5: 45  (Charlie only)
```

#### Step 4: Select Top-Scored Blocks

```scala
def selectParents(
  scoreMap: ScoreMap,
  maxParents: Int = 5
): List[BlockHash] = {
  // Sort blocks by score (descending)
  val rankedBlocks = scoreMap.toList.sortBy(-_._2)

  // Take top K blocks
  val topBlocks = rankedBlocks.take(maxParents).map(_._1)

  // Filter: remove blocks that are ancestors of other selected blocks
  // (no point including both parent and grandparent)
  val filtered = topBlocks.filterNot { block =>
    topBlocks.exists(other => other != block && dag.isAncestor(block, other))
  }

  filtered
}
```

**Example**:

```
Ranked blocks:
1. A4: 100
2. A3: 100
3. C5: 45
4. A7: 30
5. A6: 30
6. A5: 30
7. B6: 25

Take top 5:
- A4 (100)
- A3 (100)
- C5 (45)
- A7 (30)
- A6 (30)

Filter ancestors:
- A3 is ancestor of A4, A7, A6, C5 → remove A3
- A4 is ancestor of A7, A6 → remove A4
- A6 is ancestor of A7 → remove A6

Final parents: [A7, C5]
(or maybe [A7, B6, C5] depending on threshold)
```

### Complete Implementation

```scala
def tips(
  dag: BlockDagRepresentation[F],
  genesis: BlockMessage
): F[ForkChoice] = {
  for {
    // 1. Get latest messages
    latestMessages <- dag.latestMessages

    // Handle edge case: no latest messages (empty DAG)
    _ <- if (latestMessages.isEmpty) {
      F.pure(ForkChoice(List(genesis.blockHash)))
    } else F.unit

    // 2. Compute LCA
    lca <- computeLCA(latestMessages, dag)

    // 3. Build score map
    scoreMap <- buildScoreMap(latestMessages, lca, dag)

    // 4. Select parents
    parents = selectParents(scoreMap, maxParents = 5)

    // 5. Return fork choice
  } yield ForkChoice(parents)
}
```

## Theoretical Properties

### Theorem: Convergence

**Claim**: If all honest validators use the fork choice estimator with the same view (latest messages), they will select the same parents.

**Proof**:

```
1. Assume all honest validators have the same latest messages (via gossip)
2. LCA computation is deterministic (max height of common ancestors)
3. Score map computation is deterministic (sum of weights along paths)
4. Parent selection is deterministic (sort by score, filter ancestors)
5. Therefore, all honest validators compute the same parents

Q.E.D.
```

**Caveat**: Validators may have slightly different views due to network delays. This is acceptable - they will converge as they see each other's blocks.

### Theorem: Security Against Weight-Based Attacks

**Claim**: An attacker controlling <50% of stake cannot force honest validators to build on the attacker's chain.

**Proof**:

```
1. Assume attacker controls weight W_a < 50%
2. Honest validators control weight W_h > 50%
3. Attacker creates block B_attack
4. Honest validators create competing block B_honest
5. Score(B_attack) ≤ W_a < 50%
6. Score(B_honest) ≥ W_h > 50%
7. Fork choice selects B_honest (higher score)
8. Honest validators build on B_honest, not B_attack

Q.E.D.
```

**Key insight**: Weight-based scoring ensures majority stake determines preferred history.

### Theorem: Greedy Approximation

**Claim**: Fork choice estimator approximates the "most supported" blocks in polynomial time.

**Proof sketch**:

```
Finding globally optimal set of parents is NP-hard (maximum weight independent set).
Fork choice uses greedy heuristic:
- Sort by cumulative weight
- Select top-K
- Filter ancestors

This runs in O(N log N + N*D) where:
- N = number of blocks
- D = DAG depth

Greedy algorithm provides good approximation in practice.
```

## Edge Cases and Handling

### Edge Case 1: No Latest Messages (Empty DAG)

```scala
if (latestMessages.isEmpty) {
  // First block after genesis
  return ForkChoice(List(genesis.blockHash))
}
```

**Scenario**: Node just started, no blocks seen yet.

**Solution**: Build on genesis.

### Edge Case 2: Tie Scores

```scala
val rankedBlocks = scoreMap.toList
  .sortBy { case (hash, score) => (-score, hash.toString) }  // Lexicographic tie-breaking
```

**Scenario**: Multiple blocks have same score.

**Solution**: Break ties deterministically (lexicographic order of block hashes).

### Edge Case 3: Equivocations

```scala
// If validator has equivocated, their latest "message" may be multiple blocks
latestMessages = Map(
  "alice_pubkey" -> Set("hash_A5", "hash_A5'")  // Equivocation!
)

// Handle: Include all equivocating blocks in scoring
// They will be weighted equally and compete
```

**Scenario**: Validator created two blocks with same sequence number.

**Solution**: Include both in scoring. Safety oracle will detect equivocation later.

### Edge Case 4: Very Deep DAG

```scala
// If DAG is too deep, limit search depth
val maxDepth = 1000

def pathFromTo(start: BlockHash, end: BlockHash): List[BlockHash] = {
  var path = List(start)
  var current = start
  var depth = 0

  while (current != end && depth < maxDepth) {
    current = dag.parent(current).getOrElse(return path)
    path = path :+ current
    depth += 1
  }

  if (depth >= maxDepth) {
    log.warn(s"Path search exceeded max depth $maxDepth")
  }

  path
}
```

**Scenario**: Very long chains (100,000+ blocks).

**Solution**: Limit search depth to prevent performance degradation.

## Performance Analysis

### Time Complexity

```
N = number of blocks in DAG
V = number of validators
D = average DAG depth (height from LCA to tips)
K = max parents

Algorithm steps:
1. Get latest messages: O(V) - hash map lookup
2. Compute LCA: O(V * D) - traverse from each latest message to LCA
3. Build score map: O(V * D) - traverse paths, update scores
4. Select parents: O(N log N) - sort all blocks by score
5. Filter ancestors: O(K * D) - check ancestry relationships

Total: O(N log N + V * D)

Typical values:
- N = 10,000 blocks
- V = 100 validators
- D = 100 depth
- K = 5 parents

Total: ~10,000 * log(10,000) + 100 * 100 = ~140,000 operations

This is acceptable for block creation (happens every ~5-10 seconds).
```

### Space Complexity

```
ScoreMap: O(N) - one entry per block
LatestMessages: O(V) - one entry per validator
Paths: O(V * D) - temporary storage during scoring

Total: O(N + V * D)

Typical: ~10,000 + 100 * 100 = ~20,000 entries
```

### Optimizations

**1. Caching**:

```scala
// Cache score map between block creations
var cachedScoreMap: Option[(LatestMessages, ScoreMap)] = None

def buildScoreMapCached(latestMessages: LatestMessages): ScoreMap = {
  cachedScoreMap match {
    case Some((oldMessages, oldScores)) if oldMessages == latestMessages =>
      oldScores  // Reuse cached result
    case _ =>
      val newScores = buildScoreMap(latestMessages, ...)
      cachedScoreMap = Some((latestMessages, newScores))
      newScores
  }
}
```

**2. Incremental Updates**:

```scala
// When new block arrives, only update affected scores
def updateScoresIncremental(
  oldScores: ScoreMap,
  newBlock: BlockMessage
): ScoreMap = {
  val validator = newBlock.sender
  val weight = dag.getValidatorWeight(validator)

  // Remove old latest message's contribution
  val oldPath = dag.pathFromTo(oldLatestMessage(validator), lca)
  var scores = oldScores
  for (block <- oldPath) {
    scores = scores.updated(block, scores(block) - weight)
  }

  // Add new latest message's contribution
  val newPath = dag.pathFromTo(newBlock.blockHash, lca)
  for (block <- newPath) {
    scores = scores.updated(block, scores.getOrElse(block, 0) + weight)
  }

  scores
}
```

**3. Parallel Computation**:

```scala
// Compute paths in parallel
val paths = latestMessages.par.map { case (validator, blockHash) =>
  (validator, dag.pathFromTo(blockHash, lca))
}.seq

// Aggregate scores (requires synchronization)
val scoreMap = paths.foldLeft(Map.empty[BlockHash, Weight]) {
  case (scores, (validator, path)) =>
    val weight = weights(validator)
    path.foldLeft(scores) { (acc, block) =>
      acc.updated(block, acc.getOrElse(block, 0) + weight)
    }
}
```

## Examples

### Example 1: Simple Linear Chain

```
DAG:
Genesis ← B1 ← B2 ← B3 ← B4

Latest messages:
- Alice (30): B4
- Bob (25): B4
- Charlie (45): B4

LCA: B3 (or maybe Genesis, depending on definition)

Scores:
- B4: 100 (all validators)
- B3: 100
- B2: 100
- B1: 100

Fork choice: [B4]

Result: All validators build on B4 (convergence!)
```

### Example 2: Parallel Chains

```
DAG:
        A5 (Alice)
       /
   A2 ── B4 (Bob)
       \
        C3 (Charlie)

Weights:
- Alice: 30
- Bob: 25
- Charlie: 45

Latest messages:
- Alice: A5
- Bob: B4
- Charlie: C3

LCA: A2

Scores:
- A2: 100 (all)
- A5: 30  (Alice only)
- B4: 25  (Bob only)
- C3: 45  (Charlie only)

Ranked: C3 (45) > A5 (30) > B4 (25)

Fork choice: [C3, A5] (top 2, or maybe just [C3])

Result: Validators prefer Charlie's chain (most stake).
```

### Example 3: Merge Opportunity

```
DAG:
     A7
    /
   A6
  /  \
 A5  B6
  \ /
  A4

Weights: Alice 50, Bob 50

Latest messages:
- Alice: A7
- Bob: B6

LCA: A4

Scores:
- A4: 100
- A5: 50 (Alice path)
- A6: 50 (Alice path)
- A7: 50 (Alice path)
- B6: 50 (Bob path)

Ranked (after filtering ancestors):
- A7: 50
- B6: 50

Fork choice: [A7, B6] (tie, take both)

Result: Next block merges both chains!
```

## Integration with Justifications

Fork choice and justifications work together:

**Justifications provide input**:
```
Block B includes justifications:
{
  "alice": "hash_A7",
  "bob": "hash_B6",
  "charlie": "hash_C5"
}

These become the "latest messages" for fork choice.
```

**Fork choice selects parents**:
```
Run estimator on latest messages:
  scores = buildScoreMap(latestMessages, ...)
  parents = selectParents(scores)

Block B's header includes:
  parent_hashes: [selected parents from fork choice]
```

**Relationship**:
- Justifications declare knowledge (Byzantine fault detection)
- Parents declare dependency (state merging)
- Fork choice ensures parents maximize validator support

## Common Misconceptions

### Misconception 1: "Fork choice is like longest chain"

**Reality**: Not quite. Longest chain considers depth. Fork choice considers cumulative validator weight along chains. A short chain with lots of validator support beats a long chain with little support.

### Misconception 2: "Highest-scored block is always the parent"

**Reality**: Multiple high-scored blocks may be selected. This enables DAG merging. Also, ancestors are filtered (no point building on both grandparent and grandchild).

### Misconception 3: "Fork choice prevents all forks"

**Reality**: Fork choice ensures convergence, not absence of forks. Temporary forks due to network delays are normal. Fork choice ensures validators eventually agree on preferred history.

### Misconception 4: "Attackers can manipulate by creating lots of blocks"

**Reality**: Number of blocks doesn't matter, only validator weight. An attacker with 10% stake creating 1000 blocks still only contributes 10% to scores.

## Debugging Fork Choice Issues

### Issue 1: Validators Diverging

**Symptom**: Validators building on different chains.

**Debug**:
```scala
log.debug(s"Latest messages: $latestMessages")
log.debug(s"LCA: ${lca.show}")
log.debug(s"Score map: $scoreMap")
log.debug(s"Selected parents: $parents")
```

**Common causes**:
- Network partition (validators have different views)
- Bug in LCA computation
- Non-deterministic tie-breaking

### Issue 2: Deep Forks

**Symptom**: Forks lasting many blocks.

**Debug**:
```scala
// Check if equivocations are causing split
val equivocators = dag.getEquivocators
log.warn(s"Active equivocators: $equivocators")

// Check validator weight distribution
val topChain = ...
val topChainWeight = scoreMap(topChain)
log.info(s"Top chain weight: $topChainWeight / $totalStake = ${topChainWeight.toDouble / totalStake}")
```

**Common causes**:
- Equivocations splitting validator set
- Network partition
- Insufficient validator connectivity

### Issue 3: Performance Degradation

**Symptom**: Fork choice taking >1 second.

**Debug**:
```scala
val start = System.currentTimeMillis()
val lca = computeLCA(...)
val lcaTime = System.currentTimeMillis() - start
log.info(s"LCA computation: ${lcaTime}ms")

val scoreMap = buildScoreMap(...)
val scoreTime = System.currentTimeMillis() - lcaTime
log.info(s"Score map: ${scoreTime}ms")
```

**Common causes**:
- Very deep DAG (limit search depth)
- Too many validators (optimize path finding)
- Large score map (use incremental updates)

## Summary

The fork choice estimator is a **weight-based greedy algorithm** that ensures validator convergence:

**Algorithm**:
1. Get latest messages (from justifications)
2. Find Latest Common Ancestor
3. Score blocks by cumulative validator weight
4. Select top-K highest-scored blocks as parents

**Properties**:
- **Convergence**: Honest validators with same view select same parents
- **Security**: Attackers with <50% stake cannot force their chain
- **Performance**: O(N log N + V*D) time complexity

**Integration**:
- Takes latest messages from justifications
- Provides parent selection for new blocks
- Works with safety oracle for finalization

**Key Insight**: Weight-based scoring ensures validators controlling majority stake determine preferred history, without explicit voting or coordination.

## Further Reading

- [Justifications](../01-fundamentals/justifications.md) - Provides latest messages input
- [DAG Structure](../01-fundamentals/dag-structure.md) - Why multiple parents
- [Safety Oracle](safety-oracle.md) - Finalization using estimator output
- [Block Creation](block-creation.md) - Complete block proposal flow

---

**Navigation**: [← DAG Structure](../01-fundamentals/dag-structure.md) | [Block Validation →](block-validation.md) | [Safety Oracle →](safety-oracle.md)
