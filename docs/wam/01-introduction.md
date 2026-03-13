# 1. Introduction

## What is a Warren Abstract Machine?

The Warren Abstract Machine (WAM) is a virtual machine architecture designed by
David H. D. Warren in 1983 for efficient execution of Prolog programs [Warren 1983].
It remains, over four decades later, the foundational architecture for nearly every
compiled logic programming system. Hassan Ait-Kaci's tutorial reconstruction
[Ait-Kaci 1991] made the WAM accessible to a broader audience and established the
standard pedagogical presentation that most implementations reference.

The WAM's core insight is that the fundamental operations of logic programming --
unification, backtracking, and environment management -- can be decomposed into a
small, orthogonal instruction set operating on a register machine with specialized
memory areas. This decomposition enables compilation of declarative logic programs
into efficient imperative instruction sequences.

## Why a WAM for MeTTa?

MeTTa is a language with LISP-like S-expression syntax that supports pattern matching
rules, nondeterministic evaluation, and grounded operations. The MeTTaTron evaluator
uses a trampoline-based evaluation engine with three tiers:

1. **Tree-walking interpreter** -- baseline evaluator
2. **Bytecode VM** -- compiled evaluation for hot paths
3. **JIT compilation** -- NaN-boxed native execution

Within this tiered architecture, **rule dispatch** -- the process of matching an
expression against all defined rules and extracting variable bindings -- is a
critical performance bottleneck. Prior to the WAM integration, rule dispatch used
two mechanisms:

- **MORK/PathMap trie traversal** -- prefix-based candidate filtering
- **StructuralMatcher** -- compiled check sequences that validate candidates

The StructuralMatcher approach, while effective, has a fundamental inefficiency:
each structural check re-navigates the expression tree from the root via
`MatchPath::navigate()`. For a pattern `(f (g $x) $y)`, verifying the inner `g`
requires traversing `root -> child[1] -> child[0]` for every check at that depth.

The WAM eliminates this redundancy through **register-based decomposition**: a
`GetArg` instruction extracts a child into a register once, and all subsequent
checks on that child operate on the register value (O(1) access). This transforms
O(depth) per-check navigation into O(1) per-check register reads.

## Design Philosophy

The MeTTa-WAM is not a full Prolog WAM. It is a **selective adaptation** that
adopts specific WAM concepts where they provide clear performance advantages over
the existing evaluation infrastructure:

| WAM Concept | Adopted? | Rationale |
|-------------|----------|-----------|
| Register-based decomposition | Yes | Eliminates redundant tree navigation |
| Trail-based binding undo | Yes | Eliminates binding map cloning (93% single-match case: zero undo cost) |
| Indexed binding frames | Yes | O(1) slot access vs O(n) name lookup |
| Choice points | Yes, modified | All-solutions semantics (not Prolog's depth-first single solution) |
| Environment frames | No | Trampoline manages evaluation stack |
| Heap cells / structure sharing | No | Slab-allocated MettaValues with 'static lifetime |
| WAM unification algorithm | No | MeTTa uses one-directional pattern matching, not bidirectional unification |
| `cut` (!) | No | MeTTa explores all alternatives |
| Continuation passing | No | Trampoline handles continuation stack |

The WAM engine operates as a **rule dispatch accelerator** inside the existing
trampoline. It handles pattern matching and variable binding, then returns results
to the trampoline for evaluation of rule RHS templates and special forms.

```
                         eval_trampoline_generic()
                                    |
                           eval_step_generic()
                              identifies S-expr with rules
                                    |
                    +---------------+---------------+
                    |                               |
                    v                               v
             WAM dispatch                  Existing structural/MORK
          (wam_dispatch_rules)          (try_match_all_rules_generic)
                    |                               |
                    +----------- merge ------------+
                                    |
                         dispatch_rule_matches()
```

## Performance Profile

The WAM provides two categories of performance improvement:

**1. Single-match fast path (93% of rule dispatches)**

In the common case where exactly one rule matches an expression, the WAM:
- Performs register-based matching (no tree re-navigation)
- Writes binding slots + trail entries (no HashMap/SmallVec allocation)
- Converts binding frame to `GenericBindings` only on success
- Never unwinds the trail (entries are discarded)

**2. Multi-match nondeterministic path**

When multiple rules match (7% of dispatches, common in PLN inference):
- Trail unwinding restores only the modified slots (O(changed) vs O(all_slots) clone)
- Choice points track alternatives without cloning the entire binding state
- All alternatives are explored and results accumulated (all-solutions semantics)

## Document Scope

This documentation covers the MeTTa-WAM as implemented in `src/backend/eval/wam/`.
It does not cover:
- The trampoline evaluation engine (see `src/backend/eval/`)
- The bytecode VM or JIT compiler (see `src/backend/eval/bytecode/` and `src/backend/eval/jit/`)
- The MORK/PathMap trie (see `src/backend/mork_convert.rs`)
- The StructuralMatcher (see `src/backend/environment/rule_management.rs`)

## References

- Warren, D. H. D. (1983). *An Abstract Prolog Instruction Set*. Technical Note 309, SRI International.
- Ait-Kaci, H. (1991). *Warren's Abstract Machine: A Tutorial Reconstruction*. MIT Press.
