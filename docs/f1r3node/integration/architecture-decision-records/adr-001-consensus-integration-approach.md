# ADR-001: Consensus Integration Approach for MeTTaTron

## Status

**Accepted** - 2025-11-22

## Context

MeTTaTron is a high-performance MeTTa language evaluator already integrated with RNode's Rholang interpreter via direct Rust linking. We need to decide how to extend this integration to enable interaction with RNode's Casper consensus protocol.

### Current State

**Existing Integration** (Phase 0):
- MeTTaTron compiled as Rust crate dependency in RNode
- Rholang system processes expose MeTTa compilation at fixed channels:
  - `rho:metta:compile` (channel 200)
  - `rho:metta:compile:sync` (channel 201)
- PathMap Par conversion enables bidirectional MeTTa ↔ Rholang data exchange
- **Location**: `/home/dylon/Workspace/f1r3fly.io/f1r3node/rholang/src/rust/interpreter/system_processes.rs`

**User Need**:
User wants MeTTaTron to be "compatible with the Casper consensus protocol" but hasn't specified exact requirements.

### Possible Interpretations

1. **Execute Consensus in MeTTa**: Implement Casper consensus algorithms directly in MeTTa language
   - **Challenge**: MeTTa is functional/declarative; consensus needs stateful operations
   - **Risk**: Very high (touching consensus is dangerous)
   - **Value**: Unclear (Casper is already well-implemented in Rust/Scala)

2. **Query Consensus State**: Allow MeTTa code to read Casper consensus state (blocks, validators, finality)
   - **Challenge**: Design clean API, minimal changes to Casper
   - **Risk**: Low (read-only access)
   - **Value**: High (enables finality-aware smart contracts)

3. **Define Consensus Rules**: Use MeTTa as DSL for consensus parameters and validation rules
   - **Challenge**: Design expressive DSL, ensure correctness
   - **Risk**: Medium (incorrect config could affect consensus)
   - **Value**: High (declarative, verifiable configuration)

4. **Formal Verification**: Use MeTTa for consensus property specification and verification
   - **Challenge**: Link to existing Coq proofs, design verification framework
   - **Risk**: Low (verification separate from runtime)
   - **Value**: Very high (mathematical safety guarantees)

## Decision

We will implement a **three-phase incremental integration** approach:

### Phase 1: Read-Only Consensus State Access

**Goal**: Enable MeTTa code to query Casper consensus state without modifying it.

**Scope**:
- Query block finalization status
- Get validator information (bonded stake, status)
- Read consensus parameters (safety threshold, max parents)
- Access network state (latest finalized block, DAG height)

**Implementation**:
- Extend Rholang system process pattern (proven approach)
- Add new channels in `rho:casper:*` namespace
- Direct Rust API calls from system processes to Casper
- PathMap Par conversion for results

**Risk**: **Low**
- Read-only operations cannot break consensus
- Existing system process pattern is well-tested
- Minimal changes to Casper codebase

**Timeline**: 1-2 weeks

**Deliverables**:
- Working `(is-finalized)` function in MeTTa
- Example contracts using finality checks
- API documentation and tests

### Phase 2: Consensus Configuration DSL

**Goal**: Define consensus parameters declaratively in MeTTa.

**Scope**:
- Safety threshold for finalization
- Maximum parents per block
- Validator set configuration
- Gas/phlogiston limits

**Implementation**:
- MeTTa DSL for consensus config
- Compile to Rholang-compatible format
- Validation via MeTTa's type system
- Load at node startup or via governance contracts

**Risk**: **Medium**
- Incorrect configuration could affect consensus behavior
- Requires careful validation and testing
- Backward compatibility with existing configs

**Timeline**: 2-4 weeks (after Phase 1)

**Deliverables**:
- MeTTa consensus config DSL
- Compiler to Rholang/Casper format
- Validation framework
- Migration guide from existing configs

### Phase 3: Formal Verification Bridge

**Goal**: Use MeTTa for formal specification and verification of consensus properties.

**Scope**:
- Encode consensus invariants (safety, liveness) in MeTTa
- Verify properties hold for current DAG state
- Link to existing Coq proofs
- Generate test cases from specifications

**Implementation**:
- MeTTa → Coq proof translation
- Property checking against live DAG
- Integration with CI/CD for regression testing
- Documentation of verified properties

**Risk**: **Low**
- Verification doesn't affect runtime consensus
- Complements existing Coq formal verification
- Failures are informational, not critical

**Timeline**: 4-8 weeks (after Phase 2)

**Deliverables**:
- MeTTa property specification language
- Coq proof generator
- Automated verification suite
- Verified consensus properties documentation

## Rationale

### Why Incremental (Phased) Approach?

1. **Risk Management**:
   - Start with low-risk read-only access
   - Gain confidence before touching consensus parameters
   - Formal verification last (lowest risk, highest complexity)

2. **Value Delivery**:
   - Phase 1 provides immediate value (finality-aware contracts)
   - Each phase builds on previous
   - Can stop at any point if requirements change

3. **Learning**:
   - Phase 1 teaches us integration patterns
   - Discover edge cases before higher-risk phases
   - Refine APIs based on real usage

4. **Resources**:
   - Can allocate resources incrementally
   - Pause between phases for other priorities
   - Each phase is independently useful

### Why Extend System Process Pattern?

**Alternatives Considered**:

1. **FFI (Foreign Function Interface)**:
   - **Pros**: Standard approach for language interop
   - **Cons**: Overhead, complexity, already have direct Rust linking
   - **Decision**: Rejected (unnecessary when both are Rust)

2. **New RPC API**:
   - **Pros**: Clean separation, versioning
   - **Cons**: Network overhead, auth complexity, separate service
   - **Decision**: Rejected (overkill for in-process integration)

3. **Modify Casper Directly**:
   - **Pros**: Tightest integration
   - **Cons**: High risk, tight coupling, hard to test
   - **Decision**: Rejected (too invasive)

4. **Extend System Processes** ✅:
   - **Pros**: Proven pattern, clean API, testable, minimal changes
   - **Cons**: Limited to Rholang interface (acceptable)
   - **Decision**: **Accepted** (best trade-offs)

**System Process Pattern Benefits**:
- Already used for MeTTa compilation
- Well-understood by team
- Clean separation of concerns
- Easy to test (mock system processes)
- Follows RNode conventions

### Why Not Execute Consensus in MeTTa?

**Reasons**:

1. **Semantic Mismatch**:
   - MeTTa is declarative/functional
   - Consensus requires stateful, imperative operations
   - Forcing consensus into MeTTa would be unnatural

2. **Performance**:
   - Consensus is performance-critical (microseconds matter)
   - Current Rust/Scala implementation is highly optimized
   - MeTTa interpretation would add overhead

3. **Risk**:
   - Consensus is Byzantine fault tolerant (BFT)
   - Any bugs could compromise network security
   - Rewriting in new language is extremely risky

4. **Existing Investment**:
   - Casper CBC has years of development
   - Formal verification in Coq
   - Battle-tested in production
   - No compelling reason to replace

**Better Use of MeTTa**:
- **Configuration**: Declarative is perfect fit
- **Verification**: Type system and logic useful
- **Observation**: Read-only queries safe and valuable

## Consequences

### Positive

1. **Low Risk Start**:
   - Phase 1 (read-only) cannot break consensus
   - Build confidence incrementally
   - Early value delivery

2. **Proven Patterns**:
   - Extends existing system process integration
   - Leverages MeTTaTron's existing deployment
   - No new infrastructure required

3. **Flexibility**:
   - Can adapt based on user feedback
   - Can stop at any phase if sufficient
   - Easy to extend further if needed

4. **Documentation-Driven**:
   - Forces clear thinking about integration
   - Creates valuable reference material
   - Enables community contribution

### Negative

1. **Delayed Advanced Features**:
   - Formal verification comes last (Phase 3)
   - Configuration DSL waits for Phase 2
   - Some users may want these earlier

   **Mitigation**: Clear roadmap, prioritization based on user needs

2. **API Evolution**:
   - Early APIs may change based on learnings
   - Potential breaking changes between phases

   **Mitigation**: Version APIs, deprecation warnings, migration guides

3. **Integration Overhead**:
   - System processes add indirection
   - Some performance cost vs. direct calls

   **Mitigation**: Benchmarks show <5ms latency (acceptable for queries)

### Risks

1. **Casper API Instability**:
   - If Casper internals change, integration may break
   - **Mitigation**: Use stable public APIs, encapsulate internal details

2. **Complexity Creep**:
   - Each phase adds complexity
   - **Mitigation**: Strict scope per phase, comprehensive testing

3. **User Expectations**:
   - User may want full consensus in MeTTa despite risks
   - **Mitigation**: Clear communication, document rationale

## Implementation Notes

### Phase 1 Technical Details

**New System Processes**:

```rust
// In system_processes.rs

// Channel 210: Block finality
pub const CASPER_FINALITY_CHANNEL: u32 = 210;

// Channel 211: Validator info
pub const CASPER_VALIDATOR_CHANNEL: u32 = 211;

// Channel 212: Consensus params
pub const CASPER_PARAMS_CHANNEL: u32 = 212;

pub fn register_casper_processes(
    registry: &mut ProcessRegistry,
    casper: Arc<Mutex<Casper>>,
) {
    registry.register(
        CASPER_FINALITY_CHANNEL,
        Box::new(move |args| handle_finality_query(args, &casper)),
    );
    // ... more registrations
}

fn handle_finality_query(
    args: Vec<Par>,
    casper: &Arc<Mutex<Casper>>,
) -> Result<Par, InterpreterError> {
    let block_hash = parse_block_hash(&args[0])?;
    let casper = casper.lock().unwrap();

    let is_finalized = casper.is_finalized(&block_hash)?;
    let metadata = if is_finalized {
        casper.get_finalization_metadata(&block_hash)?
    } else {
        None
    };

    Ok(Par::from_map(vec![
        ("finalized", Par::from_bool(is_finalized)),
        ("metadata", metadata.map(Par::from).unwrap_or(Par::empty())),
    ]))
}
```

**MeTTa API**:

```metta
; Define builtin
(= (is-finalized $block-hash)
   (call-rholang "rho:casper:finality" $block-hash))

; Usage
(let $result (is-finalized "0xABC123...")
  (if (get-field $result "finalized")
    (log "Block is finalized")
    (log "Block not yet finalized")))
```

### Testing Strategy

**Unit Tests**:
- PathMap Par conversion (MeTTa ↔ Rholang)
- Query parsing and validation
- Error handling edge cases

**Integration Tests**:
- End-to-end: MeTTa → Rholang → Casper → response
- Concurrent queries (stress test)
- Invalid inputs (fuzz testing)

**Property Tests**:
- Round-trip conversion identity
- Query results consistent with DAG state
- Monotonicity (finalized stays finalized)

### Performance Requirements

**Phase 1 Targets**:
- Query latency: <5ms (p95)
- Throughput: >1000 queries/second
- Memory: <100MB additional overhead

**Benchmarking**:
```bash
# Before integration
cargo bench --bench casper_queries > before.txt

# After integration
cargo bench --bench casper_queries > after.txt

# Compare
diff before.txt after.txt
```

## Alternatives Considered

### Alternative 1: All-at-Once Integration

**Approach**: Implement all three phases simultaneously.

**Pros**:
- Complete feature set immediately
- No API evolution concerns
- Single large effort

**Cons**:
- High risk (no incremental validation)
- Long development time before any value
- Difficult to scope and estimate
- All-or-nothing commitment

**Rejected**: Too risky, violates incremental delivery principles.

### Alternative 2: MeTTa-Native Consensus

**Approach**: Reimplement Casper CBC in MeTTa language.

**Pros**:
- Pure MeTTa solution
- Could leverage MeTTa's strengths (pattern matching, etc.)

**Cons**:
- Massive effort (months-years)
- Extremely high risk (BFT correctness critical)
- Performance unknown
- Abandons existing investment

**Rejected**: Unrealistic, unjustified risk.

### Alternative 3: Fork Choice Only

**Approach**: Integrate only fork choice algorithm, leave rest of Casper unchanged.

**Pros**:
- Narrower scope than full integration
- Fork choice is well-defined algorithm
- Could demonstrate MeTTa capabilities

**Cons**:
- Still touches consensus (high risk)
- Unclear value (existing fork choice works)
- Partial solution unsatisfying

**Rejected**: Risk not justified by value.

### Alternative 4: Monitoring/Observability Only

**Approach**: Use MeTTa purely for consensus monitoring and metrics.

**Pros**:
- Zero risk to consensus
- Useful for debugging and analysis
- Clear separation of concerns

**Cons**:
- Limited functionality
- Doesn't enable smart contract integration
- Underutilizes MeTTa capabilities

**Partially Accepted**: Phase 1 includes this, plus more.

## References

- [Casper CBC Paper](https://github.com/cbc-casper/cbc-casper-paper)
- [RNode Casper Implementation](/var/tmp/debug/f1r3node/casper/)
- [MeTTaTron Integration](/home/dylon/Workspace/f1r3fly.io/f1r3node/rholang/src/rust/interpreter/system_processes.rs)
- [RNode Formal Verification](/var/tmp/debug/f1r3node/docs/formal-verification/)

## Related ADRs

- **ADR-002**: MeTTa-Rholang Bridge Design (Proposed)
- **ADR-003**: Consensus State Representation in MeTTa (Proposed)

## Changelog

- **2025-11-22**: Initial version, accepted
- **Future**: Will update with implementation learnings

---

**Navigation**: [← Integration README](../README.md) | [Phase 1 Guide →](../../implementation-guide/phase-1-read-only-access.md)
