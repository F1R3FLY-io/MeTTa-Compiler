# MeTTa-WAM Documentation

This directory contains comprehensive documentation for the Warren Abstract Machine
(WAM) adaptation used in the MeTTaTron evaluator for accelerated pattern matching
and rule dispatch.

## Reading Guide

The documents are ordered to build understanding incrementally:

| Document | Title | Audience |
|----------|-------|----------|
| [01-introduction.md](01-introduction.md) | Introduction | Everyone -- motivation and historical context |
| [02-classical-wam.md](02-classical-wam.md) | Classical WAM Architecture | Those unfamiliar with the WAM |
| [03-metta-wam-design.md](03-metta-wam-design.md) | MeTTa-WAM Design | Core document -- how the MeTTa adaptation diverges from the classical WAM |
| [04-instruction-set.md](04-instruction-set.md) | Instruction Set Reference | Implementers and contributors |
| [05-compilation.md](05-compilation.md) | Compilation | How MeTTa rules become WAM instructions |
| [06-execution.md](06-execution.md) | Execution Engine | The instruction dispatch loop and backtracking model |
| [07-gc-integration.md](07-gc-integration.md) | GC Integration | How garbage collection interacts with WAM state |
| [08-mettail-connections.md](08-mettail-connections.md) | MeTTaIL Connections | How MeTTaIL automata patterns relate to WAM components |

## Quick Reference

### Source Files

All WAM source code resides under `src/backend/eval/wam/`:

```
src/backend/eval/wam/
  mod.rs            Module root and re-exports
  trail.rs          Trail (undo log) for binding restoration
  binding_frame.rs  Stack-allocated variable binding storage
  choice_point.rs   All-solutions nondeterministic branching
  registers.rs      Argument register file (A0..A15)
  instructions.rs   Instruction enum and opcode table
  compiler.rs       LHS pattern -> WAM instruction compiler
  engine.rs         Instruction dispatch loop and state management
```

### Key Types

| Type | File | Purpose |
|------|------|---------|
| `WamInstruction` | `instructions.rs` | Instruction enum (17 variants) |
| `WamCode` | `compiler.rs` | Compiled instruction sequence + metadata |
| `WamState` | `engine.rs` | Complete execution state for one dispatch |
| `WamRegisters` | `registers.rs` | 16-register argument file |
| `WamBindingFrame` | `binding_frame.rs` | Indexed variable slot storage |
| `Trail` / `TrailEntry` | `trail.rs` | Undo log for backtracking |
| `WamChoicePoint` | `choice_point.rs` | Nondeterministic branching state |
| `WamMatchResult` | `engine.rs` | Result of a successful match |

### Entry Points

| Function | File | Purpose |
|----------|------|---------|
| `wam_dispatch_rules()` | `engine.rs` | All-solutions rule dispatch (primary entry) |
| `wam_try_match()` | `engine.rs` | Single-rule match (drop-in for StructuralMatcher) |
| `compile_rule_group()` | `compiler.rs` | Compile a group of rules to WAM code |
| `compile_rule_lhs()` | `compiler.rs` | Compile a single LHS pattern |

## References

- Warren, D. H. D. (1983). *An Abstract Prolog Instruction Set*. Technical Note 309, SRI International.
- Ait-Kaci, H. (1991). *Warren's Abstract Machine: A Tutorial Reconstruction*. MIT Press.
