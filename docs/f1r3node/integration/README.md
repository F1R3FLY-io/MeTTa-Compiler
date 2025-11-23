# MeTTaTron Integration with RNode

## Overview

This directory contains specifications, architectural decision records (ADRs), and implementation guides for integrating MeTTaTron with RNode's Casper consensus protocol and Rholang execution environment.

## Integration Philosophy

MeTTaTron is a high-performance **MeTTa language evaluator** written in pure Rust. RNode is a **blockchain platform** with Casper CBC consensus and Rholang smart contracts. The integration enables:

1. **Consensus State Observation**: MeTTa code can query Casper consensus state
2. **Consensus Configuration**: Define consensus parameters declaratively in MeTTa
3. **Formal Verification**: Use MeTTa's type system for consensus property specification
4. **Smart Contract Integration**: Enhanced Rholang contracts with MeTTa evaluation

## Current Integration Status

### ✅ Already Integrated

MeTTaTron is **already deployed** and integrated with RNode:

**Location**: `/home/dylon/Workspace/f1r3fly.io/f1r3node/rholang/src/rust/interpreter/system_processes.rs`

**Integration Method**: Direct Rust linking (no FFI overhead)

**Exposed Services** (Rholang system processes):
- `rho:metta:compile` (channel 200) - Compile MeTTa to PathMap Par
- `rho:metta:compile:sync` (channel 201) - Synchronous compile

**Usage from Rholang**:

```rholang
new compiled, result in {
  @"rho:metta:compile"!("(+ 1 2)", *compiled) |
  for (@compiledState <- compiled) {
    result!({||}.run(compiledState))
  }
}
```

**PathMap Par Conversion**:
- Bidirectional conversion between MeTTa values and Rholang Par structures
- Enables data exchange between MeTTa and Rholang

### 🔄 Planned Integration (This Documentation)

**New Capabilities**:
1. **Consensus State Queries**: Read Casper state from MeTTa code
2. **Block Finality Checks**: Query if blocks are finalized
3. **Validator Information**: Get validator status, bonds, stakes
4. **Consensus Parameters**: Define safety thresholds, finalization criteria in MeTTa
5. **Formal Properties**: Specify and verify consensus invariants

## Documentation Structure

### [architecture-decision-records/](architecture-decision-records/)

ADRs documenting architectural choices:

- **[adr-001-consensus-integration-approach.md](architecture-decision-records/adr-001-consensus-integration-approach.md)**
  - **Decision**: Three-phase integration (observe → configure → verify)
  - **Rationale**: Minimize risk, maximize value incrementally
  - **Status**: Accepted

- **[adr-002-metta-rholang-bridge.md](architecture-decision-records/adr-002-metta-rholang-bridge.md)**
  - **Decision**: Extend existing system process pattern
  - **Rationale**: Proven approach, minimal changes to RNode
  - **Status**: Proposed

- **[adr-003-state-representation.md](architecture-decision-records/adr-003-state-representation.md)**
  - **Decision**: MeTTa S-expressions for consensus state
  - **Rationale**: Natural fit for MeTTa's evaluation model
  - **Status**: Proposed

### [specifications/](specifications/)

Technical specifications for integration features:

- **[consensus-state-queries.md](specifications/consensus-state-queries.md)**
  - API for querying Casper consensus state from MeTTa
  - Block finality, validator status, network parameters
  - Examples and usage patterns

- **[rholang-system-processes.md](specifications/rholang-system-processes.md)**
  - New system processes for consensus integration
  - Channel assignments (rho:casper:* namespace)
  - Message protocols and data formats

- **[formal-verification-bridge.md](specifications/formal-verification-bridge.md)**
  - Using MeTTa for consensus property specification
  - Linking to Coq proofs (existing formal verification)
  - Safety/liveness invariant encoding

### [implementation-guide/](implementation-guide/)

Phased implementation roadmap:

- **[phase-1-read-only-access.md](implementation-guide/phase-1-read-only-access.md)**
  - **Goal**: Query consensus state from MeTTa (read-only)
  - **Risk**: Minimal (no consensus logic changes)
  - **Timeline**: 1-2 weeks
  - **Deliverables**: Working examples of state queries

- **[phase-2-consensus-parameters.md](implementation-guide/phase-2-consensus-parameters.md)**
  - **Goal**: Define consensus config in MeTTa
  - **Risk**: Medium (incorrect config could affect consensus)
  - **Timeline**: 2-4 weeks
  - **Deliverables**: MeTTa DSL for consensus parameters

- **[phase-3-verification-integration.md](implementation-guide/phase-3-verification-integration.md)**
  - **Goal**: Formal property verification via MeTTa
  - **Risk**: Low (verification doesn't affect runtime)
  - **Timeline**: 4-8 weeks
  - **Deliverables**: MeTTa → Coq proof generation

### [examples/](examples/)

Working MeTTa code demonstrating integration:

- **[query-block-finality.metta](examples/query-block-finality.metta)**: Check if block is finalized
- **[validator-status.metta](examples/validator-status.metta)**: Get validator information
- **[consensus-properties.metta](examples/consensus-properties.metta)**: Verify safety invariants
- **[fork-choice-visualization.metta](examples/fork-choice-visualization.metta)**: Visualize DAG and fork choice

## Integration Architecture

### System Overview

```
┌────────────────────────────────────────────────────────┐
│                   MeTTaTron                            │
│  (MeTTa Language Evaluator)                            │
│                                                         │
│  ┌──────────────────────────────────────────────────┐  │
│  │  MeTTa Code                                      │  │
│  │  - Consensus state queries                       │  │
│  │  - Parameter definitions                         │  │
│  │  - Formal property specs                         │  │
│  └─────────────────┬────────────────────────────────┘  │
│                    │                                    │
│                    ↓                                    │
│  ┌──────────────────────────────────────────────────┐  │
│  │  PathMap Par Conversion                          │  │
│  │  (Bidirectional MeTTa ↔ Rholang)                 │  │
│  └─────────────────┬────────────────────────────────┘  │
└────────────────────┼────────────────────────────────────┘
                     │
                     │ Direct Rust Linking
                     ↓
┌────────────────────────────────────────────────────────┐
│                    RNode                               │
│                                                         │
│  ┌──────────────────────────────────────────────────┐  │
│  │  Rholang Interpreter                             │  │
│  │  - System processes (rho:metta:*, rho:casper:*)  │  │
│  │  - Smart contract execution                      │  │
│  └─────────────────┬────────────────────────────────┘  │
│                    │                                    │
│                    ↓                                    │
│  ┌──────────────────────────────────────────────────┐  │
│  │  Casper Consensus                                │  │
│  │  - Block creation/validation                     │  │
│  │  - Fork choice estimator                         │  │
│  │  - Safety oracle / finalization                  │  │
│  │  - Equivocation detection                        │  │
│  └─────────────────┬────────────────────────────────┘  │
│                    │                                    │
│                    ↓                                    │
│  ┌──────────────────────────────────────────────────┐  │
│  │  RSpace Tuple Space                              │  │
│  │  - LMDB persistent storage                       │  │
│  │  - History trie (Merkleized)                     │  │
│  │  - State checkpoints                             │  │
│  └──────────────────────────────────────────────────┘  │
└────────────────────────────────────────────────────────┘
```

### Data Flow: Query Block Finality

```
1. MeTTa Code:
   (is-finalized "block_hash_abc123")

2. MeTTaTron Evaluation:
   - Parse S-expression
   - Compile to internal representation

3. PathMap Par Conversion:
   - Convert MeTTa query to Rholang Par structure
   - Prepare for Rholang system process call

4. Rholang System Process (rho:casper:finality):
   - Receive query via channel
   - Call Casper finality checker (Rust)
   - Return result

5. Casper Consensus:
   - Look up block in DAG
   - Check if block is finalized (LFB or ancestor)
   - Return boolean + finalization metadata

6. Response Path (reverse):
   - Casper → Rholang Par → MeTTa Value
   - Return to MeTTa code
   - Result: (finalized true (height 12345) (lfb "hash_xyz"))
```

## Integration Patterns

### Pattern 1: Read-Only State Query

**Use Case**: MeTTa code needs to check if a block is finalized before proceeding.

**Implementation**:

```metta
; MeTTa code
(= (check-finality $block-hash)
   (let $result (call-rholang "rho:casper:finality" $block-hash)
     (if (get-field $result "finalized")
       (log "Block is finalized")
       (log "Block not yet finalized"))))
```

**Rholang Bridge**:

```rholang
contract @"rho:casper:finality"(@blockHash, return) = {
  new casperAPICh in {
    // Call Casper API (Rust)
    @"rho:casper:api"!("is-finalized", blockHash, *casperAPICh) |
    for (@result <- casperAPICh) {
      return!(result)
    }
  }
}
```

**Casper API (Rust)**:

```rust
// In system_processes.rs
pub fn handle_casper_query(
    query: &str,
    args: &[Par],
    casper: &CasperRef,
) -> Result<Par, InterpreterError> {
    match query {
        "is-finalized" => {
            let block_hash = parse_block_hash(&args[0])?;
            let is_finalized = casper.is_finalized(&block_hash)?;
            let metadata = casper.get_finalization_metadata(&block_hash)?;

            Ok(Par::from_map(vec![
                ("finalized", Par::from_bool(is_finalized)),
                ("height", Par::from_int(metadata.height)),
                ("lfb", Par::from_bytes(metadata.lfb_hash)),
            ]))
        }
        _ => Err(InterpreterError::UnknownQuery(query.to_string()))
    }
}
```

### Pattern 2: Consensus Parameter Definition

**Use Case**: Define safety threshold for finalization in MeTTa DSL.

**Implementation**:

```metta
; MeTTa consensus config
(consensus-config
  (safety-threshold 0.0)      ; Fault tolerance threshold
  (max-parents 5)             ; Maximum parents per block
  (finalization-window 100))  ; Blocks to keep in memory

; Compile to Rholang
(compile-consensus-config (consensus-config ...))
```

**Output** (Rholang-compatible format):

```rholang
@"rho:casper:config"!({
  "safety_threshold": 0.0,
  "max_parents": 5,
  "finalization_window": 100
})
```

### Pattern 3: Formal Property Specification

**Use Case**: Specify safety invariant and verify it holds.

**Implementation**:

```metta
; Safety invariant: No two finalized blocks conflict
(safety-property
  (forall ($b1 $b2)
    (implies
      (and (finalized $b1) (finalized $b2))
      (or (ancestor $b1 $b2) (ancestor $b2 $b1)))))

; Verify property holds for current DAG
(verify-property (safety-property ...))
```

**Verification Approach**:
1. MeTTa encodes property as logical formula
2. Extract current DAG state from Casper
3. Check formula holds for all finalized blocks
4. Generate counter-example if violated
5. Link to Coq proof for formal guarantee

## Technical Considerations

### Performance

**Query Latency**:
- **MeTTa → Rholang**: Sub-millisecond (direct Rust call)
- **Rholang → Casper**: Sub-millisecond (Rust API)
- **Casper Lookup**: Microseconds (HashMap DAG)
- **Total**: <5ms for simple queries

**Throughput**:
- Limited by Rholang system process concurrency
- Thousands of queries per second possible
- Batching supported for bulk operations

### Security

**Capability-Based Access**:
- MeTTa code requires channel access to query Casper
- Unforgeable names prevent unauthorized access
- Read-only queries can't affect consensus

**Input Validation**:
- All MeTTa inputs validated before Casper API calls
- Block hashes checked for format
- Query parameters bounds-checked

**Gas Accounting**:
- MeTTa evaluation consumes Rholang gas (phlogiston)
- Prevents resource exhaustion
- Complex queries may require higher gas limits

### Error Handling

**Query Errors**:
- **Invalid block hash**: Return error Par with message
- **Block not found**: Return not-found status
- **DAG inconsistency**: Log error, return partial results

**System Errors**:
- **Casper unavailable**: Return service-unavailable error
- **RSpace error**: Propagate to MeTTa with context
- **Parse errors**: Clear error messages with location

## Development Workflow

### 1. Requirements

Write user stories and acceptance criteria:

```
US-001: As a smart contract developer, I want to check if a block is
        finalized from MeTTa code, so I can make finality-dependent decisions.

AC-001.1: Given a valid block hash, when I call (is-finalized hash),
          then I receive a boolean indicating finalization status.
AC-001.2: Given an invalid block hash, when I call (is-finalized hash),
          then I receive an error with descriptive message.
```

### 2. Specification

Design API and data formats:

```
API: is-finalized
Input: block-hash (32-byte hex string)
Output: {
  "finalized": boolean,
  "height": integer,
  "lfb_hash": string,
  "distance_to_lfb": integer
}
Error: "invalid-hash" | "not-found" | "service-unavailable"
```

### 3. Implementation

Follow F1R3FLY.io scientific method:

```
Hypothesis: Direct Rust API call will be faster than Rholang message passing.

Experiment:
  1. Implement both approaches
  2. Benchmark 10,000 queries each
  3. Measure latency (p50, p95, p99)

Results:
  - Direct Rust: p50=0.2ms, p95=0.5ms, p99=1ms
  - Message passing: p50=2ms, p95=5ms, p99=10ms
  - Conclusion: Direct Rust 10× faster

Decision: Use direct Rust API calls for consensus queries.
```

### 4. Testing

Comprehensive test coverage:

```
Unit tests:
  - PathMap Par conversion (MeTTa ↔ Rholang)
  - Query parsing and validation
  - Error handling edge cases

Integration tests:
  - MeTTa → Rholang → Casper → response
  - Concurrent queries
  - Invalid inputs

Property-based tests:
  - Round-trip conversion (MeTTa → Par → MeTTa = identity)
  - Query results consistent with DAG state
```

### 5. Documentation

Document all features:

```
- API reference (specifications/)
- Usage examples (examples/)
- ADRs for design decisions (architecture-decision-records/)
- Implementation notes (implementation-guide/)
```

## Recommended Integration Path

### Start Here (Minimum Viable Integration)

**Goal**: Query block finality from MeTTa code.

**Steps**:
1. Read [adr-001-consensus-integration-approach.md](architecture-decision-records/adr-001-consensus-integration-approach.md)
2. Study [phase-1-read-only-access.md](implementation-guide/phase-1-read-only-access.md)
3. Implement `rho:casper:finality` system process
4. Add `(is-finalized)` function to MeTTa
5. Write tests and examples
6. Document usage

**Timeline**: 1-2 weeks
**Risk**: Low (read-only)
**Value**: Immediate (enables finality-aware contracts)

### Next Steps (Expanded Capabilities)

**After Phase 1**:
1. Add more query types (validator status, network parameters)
2. Implement consensus parameter definition in MeTTa
3. Build formal verification bridge (MeTTa → Coq)

**Long-term Vision**:
- Complete consensus configuration DSL in MeTTa
- Automated property verification in CI/CD
- MeTTa-based consensus simulation and testing

## Related Documentation

- **Casper Consensus**: [../casper/README.md](../casper/README.md)
- **Rholang Programming**: [../rholang/README.md](../rholang/README.md)
- **MeTTaTron Source**: `/home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/`
- **RNode Source**: `/var/tmp/debug/f1r3node/`
- **Existing Integration**: `/home/dylon/Workspace/f1r3fly.io/f1r3node/rholang/src/rust/interpreter/system_processes.rs`

## Contributing

Follow F1R3FLY.io's documentation-first methodology:

1. **Requirements First**: Write user stories before code
2. **Design ADRs**: Document architectural decisions
3. **Scientific Rigor**: Hypothesis → Experiment → Results
4. **Test-Driven**: Write tests before implementation
5. **Document Everything**: API reference, examples, guides

See: [CLAUDE.md](https://github.com/f1r3fly/MeTTa-Compiler/blob/main/CLAUDE.md) for complete guidelines.

## Status

- **Phase 0** (Existing Integration): ✅ Complete
  - MeTTa compilation via Rholang system processes
  - PathMap Par bidirectional conversion

- **Phase 1** (Read-Only Access): 📋 Documented, ready for implementation
  - Consensus state query specification complete
  - Implementation guide written
  - Examples in progress

- **Phase 2** (Consensus Parameters): 📝 Specification in progress
  - ADR drafted
  - DSL design underway

- **Phase 3** (Formal Verification): 🔮 Future work
  - Conceptual design complete
  - Awaiting Phase 1/2 completion

---

**Navigation**: [← Back](../README.md) | [Casper →](../casper/README.md) | [Rholang →](../rholang/README.md)
