# F1R3FLY.io RNode Documentation for MeTTaTron

This documentation provides comprehensive technical coverage of RNode's distributed consensus and execution architecture, specifically tailored for MeTTaTron integration and development.

## Overview

**RNode** is F1R3FLY.io's blockchain platform implementing Byzantine Fault Tolerant consensus with concurrent smart contract execution via the Rholang programming language. This documentation serves three primary purposes:

1. **Educational Reference**: Deep technical understanding of Casper CBC consensus and Rholang's distributed programming model
2. **Integration Specification**: Architectural guidelines for MeTTaTron compatibility with RNode protocols
3. **Implementation Guide**: Practical roadmap for MeTTa-based consensus components and formal verification

## Documentation Structure

### [Casper Consensus](casper/README.md)

Complete documentation of RNode's Casper CBC (Correct-by-Construction) consensus protocol:

- **[01-fundamentals](casper/01-fundamentals/)**: Core concepts (DAG structure, justifications, BFT properties)
- **[02-consensus-protocol](casper/02-consensus-protocol/)**: Protocol mechanics (block creation, validation, finalization)
- **[03-network-layer](casper/03-network-layer/)**: Distributed coordination (peer discovery, message protocols, synchronization)
- **[04-implementation](casper/04-implementation/)**: Code organization, data structures, algorithms
- **[05-formal-properties](casper/05-formal-properties/)**: Safety proofs, liveness conditions, parameters

### [Rholang Distributed Programming](rholang/README.md)

In-depth coverage of Rholang's concurrent, distributed execution model:

- **[01-process-calculus](rholang/01-process-calculus/)**: Rho-calculus foundations, reflection, parallel composition
- **[02-message-passing](rholang/02-message-passing/)**: Channels, send/receive operations, join patterns
- **[03-capability-security](rholang/03-capability-security/)**: Unforgeable names, bundles, security patterns
- **[04-rspace-tuplespace](rholang/04-rspace-tuplespace/)**: Linda model, produce/consume, spatial matching, persistence
- **[05-distributed-execution](rholang/05-distributed-execution/)**: Cross-node coordination, consensus integration
- **[06-examples](rholang/06-examples/)**: Annotated code examples demonstrating key patterns

### [MeTTaTron Integration](integration/README.md)

Specifications and implementation guides for MeTTaTron-RNode integration:

- **[architecture-decision-records](integration/architecture-decision-records/)**: ADRs documenting integration approach
- **[specifications](integration/specifications/)**: Technical specs for consensus queries, system processes, verification
- **[implementation-guide](integration/implementation-guide/)**: Phased roadmap (read-only access → parameters → verification)
- **[examples](integration/examples/)**: Working MeTTa code demonstrating integration patterns

## Quick Navigation

### By Topic

**Understanding Consensus:**
1. Start: [Casper Overview](casper/README.md)
2. Core Primitive: [Justifications](casper/01-fundamentals/justifications.md)
3. Key Algorithm: [Fork Choice Estimator](casper/02-consensus-protocol/fork-choice-estimator.md)
4. Safety: [Safety Oracle](casper/02-consensus-protocol/safety-oracle.md)
5. Byzantine Detection: [Equivocation Detection](casper/02-consensus-protocol/equivocation-detection.md)

**Understanding Rholang:**
1. Start: [Rholang Overview](rholang/README.md)
2. Foundations: [Rho-Calculus](rholang/01-process-calculus/rho-calculus-foundations.md)
3. Communication: [Message Passing](rholang/02-message-passing/channels-and-names.md)
4. Coordination: [RSpace Tuple Space](rholang/04-rspace-tuplespace/linda-model.md)
5. Distribution: [Distributed Execution](rholang/05-distributed-execution/consensus-integration.md)

**Integration Development:**
1. Start: [Integration Overview](integration/README.md)
2. Architecture: [ADR-001: Consensus Integration Approach](integration/architecture-decision-records/adr-001-consensus-integration-approach.md)
3. Phase 1: [Read-Only Access](integration/implementation-guide/phase-1-read-only-access.md)
4. Examples: [Query Block Finality](integration/examples/query-block-finality.metta)

### By Use Case

**I want to understand how distributed consensus works**
→ Read [Casper Consensus](casper/README.md) section top-to-bottom

**I want to write Rholang smart contracts**
→ Read [Rholang Examples](rholang/06-examples/) then [Message Passing](rholang/02-message-passing/)

**I want to integrate MeTTaTron with RNode consensus**
→ Read [Integration Guide](integration/README.md) and relevant ADRs

**I want to verify consensus properties formally**
→ Read [Formal Properties](casper/05-formal-properties/) and [Verification Bridge](integration/specifications/formal-verification-bridge.md)

**I want to understand the codebase**
→ Read [Implementation](casper/04-implementation/code-organization.md)

## Key Architectural Insights

### Casper Consensus Model

RNode uses a **multi-parent DAG** (Directed Acyclic Graph) structure rather than a linear blockchain:

```
Linear Blockchain:        Multi-Parent DAG:

   Block N+1                Block N+1    Block N+1'
      |                         |    \   /    |
   Block N                   Block N   Block N'
      |                         |    \ / \    |
   Block N-1                 Block N-1  Block N-1'
```

Advantages:
- **Higher throughput**: Multiple validators can propose blocks concurrently
- **Parallel execution**: Independent state changes can execute in parallel
- **Merge blocks**: Combine independent branches
- **Faster finality**: More data points for safety oracle

### Justifications: The Core Innovation

Each block includes a **justification map** showing the latest block seen from every validator:

```
Block created by Validator A:
{
  justifications: {
    "ValidatorA": "hash_A_123",
    "ValidatorB": "hash_B_456",
    "ValidatorC": "hash_C_789"
  }
}
```

This creates a partial ordering enabling:
- Detection of Byzantine behavior (equivocations)
- Computation of agreement without synchronous communication
- Safety oracle calculations for finalization
- Fork choice through validator weight scoring

### Rholang Process Calculus

Rholang implements the **ρ-calculus** (reflective higher-order calculus), extending π-calculus:

```rholang
new channel in {           // Create unforgeable name
  channel!("data") |       // Send (parallel composition)
  for (msg <- channel) {   // Receive
    // Process message
  }
}
```

Key properties:
- **Reflection**: Names and processes are interchangeable via `@` and `*`
- **Concurrency**: `|` operator for true parallelism
- **Join patterns**: Multi-channel atomic synchronization
- **Capability security**: Possession of name = permission to use

### RSpace: Distributed Tuple Space

RSpace provides Linda-style coordination across distributed nodes:

- **Produce**: Send data to channel (like `channel!` in Rholang)
- **Consume**: Wait for pattern match (like `for` in Rholang)
- **Spatial Matching**: Atomic pattern matching across multiple channels
- **History Trie**: Merkleized radix tree for consensus verification
- **Checkpoints**: Create consensus points with state hash

## Integration Philosophy

MeTTaTron integrates with RNode through **three phases**:

### Phase 1: Read-Only Observation (Recommended Start)
- Query consensus state via Rholang system processes
- No consensus logic in MeTTaTron
- MeTTa code reads: block finality, validator status, network parameters
- **Risk**: Minimal (read-only)
- **Complexity**: Low
- **Value**: Immediate (enables consensus-aware MeTTa applications)

### Phase 2: Consensus Configuration
- Define consensus parameters as MeTTa data structures
- Compile to Rholang for execution in system contracts
- Use MeTTa's type system for validation
- **Risk**: Medium (incorrect config could affect consensus)
- **Complexity**: Medium
- **Value**: High (declarative consensus configuration)

### Phase 3: Formal Verification Bridge
- Specify consensus properties in MeTTa
- Generate Coq proofs from MeTTa specs
- Link to existing formal verification work
- **Risk**: Low (verification doesn't affect runtime)
- **Complexity**: High (requires formal methods expertise)
- **Value**: Very High (mathematical safety guarantees)

## Related Documentation

- **RNode Source**: `/var/tmp/debug/f1r3node/`
- **MeTTaTron Source**: `/home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/`
- **Formal Verification**: `/var/tmp/debug/f1r3node/docs/formal-verification/`
- **Rholang Examples**: `/var/tmp/debug/f1r3node/rholang/examples/`
- **Integration Templates**: `/home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/integration/templates/`

## Contributing to This Documentation

This documentation follows F1R3FLY.io's **documentation-first development methodology**:

1. **Requirements First**: Document user stories and acceptance criteria before implementation
2. **Scientific Rigor**: Use scientific method (hypothesis → test → results) for optimization and design decisions
3. **Completeness**: Ensure proofs and algorithms are rigorous, with no hand-waving
4. **Digestibility**: Break large topics into focused files (300-800 lines ideal)
5. **Code References**: Link to source files with line numbers for traceability
6. **Examples**: Provide working code examples, not pseudocode

When adding or updating documentation:
- Place in intuitively named directories matching topic hierarchy
- Cross-reference related documents
- Include both conceptual and implementation perspectives
- Provide clear navigation paths for different use cases
- Maintain consistency with existing documentation style

## Changelog

- **2025-11-22**: Initial documentation structure created
  - Complete directory hierarchy established
  - Top-level README with navigation and philosophy
  - Prepared for systematic documentation of Casper and Rholang

## License

This documentation is part of the F1R3FLY.io MeTTaTron project and follows the same license as the source code.

---

**Navigation**: [Casper Consensus](casper/README.md) | [Rholang Programming](rholang/README.md) | [Integration Guide](integration/README.md)
