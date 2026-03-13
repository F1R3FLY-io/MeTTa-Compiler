# 3. MeTTa-WAM Design

This chapter describes how the MeTTaTron WAM adaptation diverges from the classical
WAM to serve MeTTa's evaluation semantics. The MeTTa-WAM is not a general-purpose
Prolog machine; it is a **rule dispatch accelerator** that handles pattern matching
and variable binding, then delegates to the trampoline for RHS evaluation.

## Architectural Position

The WAM engine sits within the evaluation pipeline as a fast path for rule dispatch:

```
  eval_trampoline_generic()
          |
          v
  eval_step_generic()
    identifies S-expr with head + arity
          |
          +--- Has compiled WamCode? -----+
          |           |                    |
          |           v                    v
          |    wam_dispatch_rules()    try_match_all_rules_generic()
          |           |                    |
          |           +--- results --------+
          |                    |
          v                    v
          +---- dispatch_rule_matches() ---+
                       |
               Continuation stack
               (trampoline manages
                RHS evaluation)
```

The WAM engine is invoked when a rule group has pre-compiled `WamCode` (stored in
`RuleEntry.wam_code`). If compilation was not possible (unsupported patterns), the
existing StructuralMatcher/MORK path is used as a fallback.

## Side-by-Side Comparison

| Aspect | Classical WAM (Prolog) | MeTTa-WAM |
|--------|------------------------|-----------|
| **Search strategy** | Depth-first, single solution | All solutions accumulated |
| **Unification** | Bidirectional | One-directional pattern matching |
| **Variables** | Both sides unify | Only LHS (rule) has variables |
| **Heap** | Bump-allocated, backtrack-reclaimed | Slab-allocated, GC-managed (`'static`) |
| **Environment frames** | On stack, managed by `allocate`/`deallocate` | None (trampoline manages eval stack) |
| **Continuation** | `CP` register + `call`/`proceed` | Trampoline continuation stack |
| **`cut` (!)** | Supported (prune alternatives) | Not supported (all alternatives explored) |
| **Trail entries** | Heap addresses (conditional trailing) | Slot index + previous value (unconditional) |
| **Binding storage** | Heap REF cells (self-referencing) | SmallVec slots, indexed by compile-time assignment |
| **Register count** | Varies by implementation (typically 256+) | 16 fixed (A0..A15) |
| **Instruction encoding** | Bytecode (1-4 bytes per operand) | Rust enum (8-32 bytes, discriminant-dispatched) |
| **Indexing** | First-argument indexing (`switch_on_term`) | External (MORK trie / bloom filter) |
| **Result delivery** | Success/failure + register state | `Vec<WamMatchResult>` accumulated |

## Core Design Decisions

### D1: All-Solutions Semantics

The most fundamental departure from the classical WAM. In Prolog, `try_me_else`
creates a choice point and execution proceeds with the first alternative; if
`proceed` succeeds, execution returns the result to the caller. The choice point
is left on the stack for potential backtracking via `fail`.

In MeTTa, **all** matching rules must fire. The MeTTa-WAM implements this by:

1. After each successful match (`TailEval`), the result is accumulated in
   `WamState.match_results`
2. Execution then falls through to a `Fail` instruction
3. `Fail` triggers backtracking to the next alternative
4. Only when all alternatives are exhausted does execution terminate
5. The accumulated results are returned as a `Vec`

```
  Classical WAM                    MeTTa-WAM
  +==========+                     +==========+
  | match?   |--yes--> return      | match?   |--yes--> accumulate result
  +==========+                     +==========+              |
       |                                |                    v
       no                               no              [always]
       |                                |                    |
       v                                v                    v
  backtrack                         backtrack            backtrack
  (on demand)                    (to next alt)         (to next alt)
       |                                |                    |
       v                                v                    v
  next alt                          next alt             next alt
  or fail                           or done              or done
```

### D2: One-Directional Pattern Matching

Classical WAM unification is bidirectional: both the query term and the clause
head may contain variables, and the algorithm must handle all four combinations
(var-var, var-nonvar, nonvar-var, nonvar-nonvar).

In MeTTa rule dispatch, the **input expression is always ground** (fully
evaluated). Only the rule's LHS pattern contains variables. This simplification
eliminates:

- The read/write mode distinction
- The `put_*` instruction family (no need to build terms on a heap)
- The PDL (push-down list) for recursive unification
- Occurs-check (no cyclic terms possible with one-directional matching)

Pattern matching reduces to: **decompose the ground input, check structural
constraints, and bind variables**.

### D3: No WAM Heap

Classical WAM stores terms on a heap with bump-pointer allocation. Backtracking
reclaims heap space by resetting the heap pointer to the saved `HB`.

MeTTaTron allocates all values in a **slab allocator** with `'static` lifetime.
Values are never deallocated by backtracking; they are reclaimed by the
garbage collector during quiescent periods. This means:

- No `H` (heap top) or `HB` (heap backtrack) registers
- No heap cell tags (REF, STR, FUN, etc.)
- No structure sharing via heap pointers
- `MettaValue` is an 8-byte `Copy` type (pointer to slab-allocated inner)

### D4: Indexed Binding Frames Instead of Heap Variables

Classical WAM represents variables as self-referencing REF cells on the heap.
Binding a variable mutates the REF cell to point elsewhere.

MeTTa-WAM uses **binding frames**: fixed-size arrays where each slot corresponds
to a pattern variable, assigned at compile time:

```
  WamBindingFrame
  +=========+=========+=========+=========+
  | slot 0  | slot 1  | slot 2  | slot 3  |
  | ($x)    | ($y)    | ($z)    | (unused) |
  | UNBOUND | Long(42)| UNBOUND | UNBOUND  |
  +=========+=========+=========+=========+
        ^         ^
        |         |
  unbound    bound to 42

  Backed by: SmallVec<[MettaValue; 8]>
  UNBOUND sentinel: MettaValue::inline_empty()
```

Compile-time slot assignment enables O(1) binding and lookup:

```rust
// Compile time: $x -> slot 0, $y -> slot 1
// At runtime:
frame.slots[0] = value;  // O(1) bind
let x = frame.slots[0];  // O(1) lookup
```

Compare with `GenericBindings<V>`:

```rust
// At runtime:
bindings.insert("$x", value);   // O(n) scan for existing, O(1) amortized insert
let x = bindings.get("$x");     // O(n) linear scan
```

### D5: Unconditional Trailing

Classical WAM uses **conditional trailing**: a variable is trailed only if
its heap address `a < HB` (created before the current choice point). Variables
created after the choice point are on heap space that will be reclaimed anyway.

MeTTa-WAM uses **unconditional trailing**: every `BindSlot` instruction pushes
a trail entry regardless of when the slot was created. This is simpler and
correct because:

1. Binding frames are not on a heap that can be reclaimed
2. The frame persists across alternatives (same frame, different bindings)
3. Trail entries store `(slot_index, previous_value)` -- the previous value
   must be restored on backtrack regardless of creation time

```
  TrailEntry (12 bytes)
  +==============+==================+
  | slot_index   | previous_value   |
  | (u16)        | (MettaValue, 8B) |
  +==============+==================+
```

### D6: No Environment Frames or Continuation Pointer

The classical WAM manages clause-body evaluation through environment frames
(holding permanent variables) and the `CP` register (return address). The
`allocate`/`deallocate` instructions create and destroy these frames.

MeTTaTron's trampoline manages the entire evaluation lifecycle through its
continuation stack. The WAM engine only handles **rule LHS matching** -- it
does not evaluate clause bodies. After a successful match:

1. WAM produces `(rhs_template, bindings)` pairs
2. The trampoline's `dispatch_rule_matches()` takes over
3. RHS templates are evaluated via the normal continuation-based mechanism

This means the WAM instruction set has no `allocate`, `deallocate`, `call`,
or standard `proceed` (in the Prolog sense of "return to caller").

### D7: No Indexing Instructions

The classical WAM uses `switch_on_term`, `switch_on_constant`, and
`switch_on_structure` to efficiently dispatch to the correct clause based on
the first argument's type and value.

MeTTa-WAM delegates indexing to the existing infrastructure:

- **MORK trie**: Prefix-based candidate filtering
- **Bloom filter**: `may_have_rules_for(op, arity)` fast rejection
- **RuleIndex**: `HashMap<(head, arity), Vec<RuleEntry>>` grouping

By the time `wam_dispatch_rules()` is called, the candidate rule group has
already been narrowed by the index. The WAM code for a rule group chains
alternatives with `TryMeElse`/`RetryMeElse`/`TrustMe` -- linear scanning
through a small set of pre-filtered candidates.

## Pseudocode: Core Algorithms

### Algorithm 1: WAM Execution Loop

```rust
fn execute_wam(state: &mut WamState) {
    loop {
        if state.ip >= state.code.instructions.len() {
            break;  // All instructions executed
        }

        let instruction = state.code.instructions[state.ip];
        state.ip += 1;

        match instruction {
            // Structural checks: on failure, call wam_fail()
            GetArity { reg, expected } => {
                if registers[reg].arity() != expected {
                    wam_fail(state);
                    continue;
                }
            }
            GetAtom { reg, expected } => {
                if registers[reg].as_atom() != Some(expected) {
                    wam_fail(state);
                    continue;
                }
            }

            // Decomposition: extract child into register
            GetArg { source, index, target } => {
                registers[target] = registers[source].child(index);
            }

            // Binding: record on trail, write to slot
            BindSlot { reg, slot } => {
                trail.push(TrailEntry { slot, previous: frame[slot] });
                frame[slot] = registers[reg];
            }

            // Repeated variable: check equality
            EqualCheck { reg, slot } => {
                if registers[reg] != frame[slot] {
                    wam_fail(state);
                    continue;
                }
            }

            // Choice point management
            TryMeElse { next } => {
                choice_points.push(ChoicePoint {
                    trail_mark: trail.mark(),
                    next_alternative: next,
                });
            }
            RetryMeElse { next } => {
                choice_points.top().next_alternative = next;
            }
            TrustMe => {
                choice_points.top().next_alternative = SENTINEL;
            }

            // Match success: accumulate result, then backtrack
            TailEval { rhs_index, has_variables } => {
                let bindings = if has_variables {
                    frame.to_generic_bindings()
                } else {
                    GenericBindings::Empty
                };
                match_results.push(WamMatchResult { rhs_index, bindings });
                // Fall through to Fail (next instruction)
            }

            Fail => {
                wam_fail(state);
                continue;
            }

            Proceed => {
                state.matched = true;
                break;
            }
        }
    }
}
```

### Algorithm 2: Backtracking (wam_fail)

```rust
fn wam_fail(state: &mut WamState) {
    loop {
        match state.choice_points.last() {
            None => {
                // No more alternatives -- terminate
                state.ip = END;
                return;
            }
            Some(cp) if cp.next_alternative == SENTINEL => {
                // TrustMe: last alternative exhausted, pop choice point
                let cp = state.choice_points.pop();
                trail.unwind_to(cp.trail_mark, &mut frame);
                registers.reset();
                registers.load_input(original_input);
                continue;  // Try previous choice point
            }
            Some(cp) => {
                // More alternatives: unwind and jump
                let next_ip = cp.next_alternative;
                trail.unwind_to(cp.trail_mark, &mut frame);
                registers.reset();
                registers.load_input(original_input);
                state.ip = next_ip;
                return;
            }
        }
    }
}
```

### Algorithm 3: Trail Unwinding

```rust
fn unwind_to(trail: &mut Trail, mark: usize, frame: &mut WamBindingFrame) {
    // LIFO order: last binding undone first
    while trail.len() > mark {
        let entry = trail.pop();
        frame.slots[entry.slot_index] = entry.previous;
    }
}
```

The LIFO unwinding order is critical for correctness when the same slot is
bound multiple times within a single alternative (e.g., through nested patterns):

```
  Trail (forward):    [slot=0, prev=UNBOUND] [slot=0, prev=Long(10)]
  Frame after:        slot 0 = Long(20)

  Unwind (reverse):   restore slot 0 = Long(10)  (undo second bind)
                      restore slot 0 = UNBOUND   (undo first bind)
```

## Data Flow: End-to-End Match

The following diagram traces data flow through a complete match of expression
`(f (g 42) 99)` against rules with compiled WAM code:

```
  Input: (f (g 42) 99)
         |
         v
  wam_dispatch_rules(value, code, env)
         |
         v
  WamState::new(code, value)
    registers.A0 = (f (g 42) 99)
    frame = [UNBOUND, UNBOUND]     // slots for $x, $y
    trail = []
    choice_points = []
         |
         v
  execute_wam(&mut state)
    +--------------------------------------------------+
    | ip=0: GetArity A0, 3                             |
    |   A0 = (f (g 42) 99), len=3 == 3  --> pass       |
    +--------------------------------------------------+
    | ip=1: GetArg A0, 0, A1                           |
    |   A1 = f                                          |
    +--------------------------------------------------+
    | ip=2: GetAtom A1, "f"                            |
    |   A1 = f == "f"  --> pass                         |
    +--------------------------------------------------+
    | ip=3: GetArg A0, 1, A2                           |
    |   A2 = (g 42)                                     |
    +--------------------------------------------------+
    | ip=4: GetArity A2, 2                             |
    |   A2 = (g 42), len=2 == 2  --> pass               |
    +--------------------------------------------------+
    | ip=5: GetArg A2, 0, A3                           |
    |   A3 = g                                          |
    +--------------------------------------------------+
    | ip=6: GetAtom A3, "g"                            |
    |   A3 = g == "g"  --> pass                         |
    +--------------------------------------------------+
    | ip=7: GetArg A2, 1, A4                           |
    |   A4 = 42                                         |
    +--------------------------------------------------+
    | ip=8: BindSlot A4, 0                             |
    |   trail: [(slot=0, prev=UNBOUND)]                 |
    |   frame: [Long(42), UNBOUND]                      |
    +--------------------------------------------------+
    | ip=9: GetArg A0, 2, A5                           |
    |   A5 = 99                                         |
    +--------------------------------------------------+
    | ip=10: BindSlot A5, 1                            |
    |   trail: [(slot=0, prev=UNBOUND), (slot=1, prev=UNBOUND)] |
    |   frame: [Long(42), Long(99)]                     |
    +--------------------------------------------------+
    | ip=11: TailEval rhs=0, has_variables=true        |
    |   bindings = {$x: Long(42), $y: Long(99)}        |
    |   match_results.push(...)                         |
    +--------------------------------------------------+
    | ip=12: Fail                                      |
    |   wam_fail: no choice points --> terminate         |
    +--------------------------------------------------+
         |
         v
  return vec![(rhs_template, {$x: 42, $y: 99}, rhs_type, has_vars)]
```

## Memory Layout: WamState

```
  WamState (stack-allocated per dispatch)
  +===========================================================+
  |  registers: WamRegisters                                   |
  |    +------+------+------+------+------+---+------+        |
  |    |  A0  |  A1  |  A2  |  A3  | ...  |   | A15  |        |
  |    +------+------+------+------+------+---+------+        |
  |    128 bytes (16 x 8-byte MettaValue)                      |
  |    arity: u8 (number of valid registers)                   |
  |    scratch: MettaValue (8 bytes)                           |
  +===========================================================+
  |  frame: WamBindingFrame                                    |
  |    slots: SmallVec<[MettaValue; 8]>                        |
  |           (inline for <= 8 slots, spills to heap)          |
  |    trail_mark: usize                                       |
  |    names: SmallVec<[&'static str; 8]>                      |
  +===========================================================+
  |  trail: Trail                                              |
  |    entries: Vec<TrailEntry>                                 |
  |             (pre-allocated capacity 64)                     |
  |             ~768 bytes (64 x 12 bytes)                     |
  +===========================================================+
  |  choice_points: Vec<WamChoicePoint>                        |
  |    (pre-allocated capacity 4)                              |
  +===========================================================+
  |  match_results: Vec<WamMatchResult>                        |
  +===========================================================+
  |  ip: usize (instruction pointer)                           |
  +===========================================================+
  |  code: Arc<WamCode> (shared reference to compiled code)    |
  +===========================================================+
  |  matched: bool                                             |
  +===========================================================+
```

## References

- Warren, D. H. D. (1983). *An Abstract Prolog Instruction Set*. Technical Note 309, SRI International.
- Ait-Kaci, H. (1991). *Warren's Abstract Machine: A Tutorial Reconstruction*. MIT Press.
