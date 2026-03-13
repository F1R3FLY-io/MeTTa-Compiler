# 5. Compilation

This chapter describes how MeTTa rule patterns are compiled into WAM instruction
sequences. The compiler analyzes each rule's LHS pattern and produces a `WamCode`
structure containing instructions, slot metadata, and RHS template references.

Source: `src/backend/eval/wam/compiler.rs`

## Compilation Pipeline

```
  MeTTa Rule
  (= (f (g $x) $y) rhs)
          |
          v
  Rule LHS: (f (g $x) $y)
          |
          v
  compile_rule_lhs(&lhs)
    1. Create CompilerState
    2. Recursive compile_node() from A0
    3. Emit Proceed
    4. Return WamCode
          |
          v
  compile_rule_group(&[entries])
    1. Compile each rule's LHS
    2. Replace Proceed with TailEval
    3. Chain with TryMeElse/RetryMeElse/TrustMe
    4. Patch forward references
    5. Return Arc<WamCode>
          |
          v
  WamCode stored in RuleEntry.wam_code
```

## Compiler State

The `CompilerState` structure tracks resources during compilation:

```rust
struct CompilerState {
    instructions: Vec<WamInstruction>,  // emitted instructions
    next_reg: u8,                        // next free register (starts at 1; A0 = root)
    seen_vars: SmallVec<[(&'static str, u16); 8]>,  // variable -> slot mapping
    next_slot: u16,                      // next free binding slot
    slot_names: Vec<&'static str>,       // slot index -> variable name
}
```

### Register Allocation

Registers are allocated **left-to-right, depth-first** during pattern tree
traversal. Register A0 is always reserved for the root input expression.
Each `GetArg` instruction allocates the next available register for its target.

```
  Pattern: (f (g $x) $y)

  Traversal order:
    A0 = (f (g $x) $y)     [root, pre-allocated]
    A1 = f                  [child 0 of A0]
    A2 = (g $x)             [child 1 of A0]
    A3 = g                  [child 0 of A2]
    A4 = $x                 [child 1 of A2]
    A5 = $y                 [child 2 of A0]
```

Since MeTTa patterns are trees (no sharing), each register is written exactly
once (by `GetArg`) and read at most twice (once for type/value check, once for
further decomposition). The maximum register count is `MAX_REGISTERS = 16`,
sufficient for patterns up to ~15 nodes deep.

**Register exhaustion**: If a pattern requires more than 16 registers,
`compile_node()` returns `false` and the compilation falls back to the
StructuralMatcher path. In practice, PLN rules have maximum observed arity of 6,
so 16 registers are more than sufficient.

### Variable Classification

The compiler classifies pattern variables into three categories:

| Category | Detection | Instruction | Example |
|----------|-----------|-------------|---------|
| First occurrence | Not in `seen_vars` | `BindSlot` | `$x` in `(f $x $y)` |
| Repeated | Already in `seen_vars` | `EqualCheck` | Second `$x` in `(f $x $x)` |
| Wildcard | Name is `"_"` | (none) | `_` in `(f _ $y)` |

Variable prefixes recognized by the compiler:
- `$` -- standard pattern variables (e.g., `$x`, `$var`)
- `'` -- quoted variables (e.g., `'a`, `'type`)
- `&` -- reference variables (e.g., `&self`, when `len > 1`)

The wildcard `_` matches anything without binding. No instruction is emitted,
and no slot is allocated.

### Slot Assignment

Slots are assigned sequentially to variables in the order they are first
encountered during left-to-right, depth-first traversal:

```
  Pattern: (f $x (g $y $x) $z)

  Traversal and slot assignment:
    $x first seen at position [1] -> slot 0
    $y first seen at position [2,1] -> slot 1
    $x seen again at position [2,2] -> EqualCheck (no new slot)
    $z first seen at position [3] -> slot 2

  slot_names = ["$x", "$y", "$z"]
  num_slots = 3
```

## Node Compilation Algorithm

The `compile_node()` function is called recursively for each node in the
LHS pattern tree. It dispatches on the node type:

### S-expression Nodes

```
  compile_node(sexpr, reg, state):
      emit GetArity(reg, len(sexpr))
      for each child at index i:
          child_reg = alloc_reg()
          emit GetArg(reg, i, child_reg)
          compile_node(child, child_reg, state)  // recurse
```

### Atom Nodes

```
  compile_node(atom, reg, state):
      if atom is variable ($, ', &):
          if atom in seen_vars:
              emit EqualCheck(reg, seen_vars[atom].slot)
          else:
              slot = alloc_slot(atom)
              record_var(atom, slot)
              emit BindSlot(reg, slot)
      else if atom == "_":
          // wildcard: no instruction
      else:
          emit GetAtom(reg, atom)
```

### Literal Nodes

```
  compile_node(long, reg, state):
      emit GetLong(reg, long.value)

  compile_node(bool, reg, state):
      emit GetBool(reg, bool.value)

  compile_node(float, reg, state):
      emit GetFloat(reg, float.to_bits())

  compile_node(string, reg, state):
      emit GetString(reg, intern(string.value))
```

### Unsupported Nodes

The following node types cause `compile_node()` to return `false`, aborting
compilation and falling back to the StructuralMatcher:

- `Type` nodes -- type assertions within patterns
- `Conjunction` nodes -- multiple constraint patterns
- `Error` nodes -- error patterns
- `Quoted` nodes -- quote-wrapped patterns

## Single-Rule Compilation

`compile_rule_lhs()` compiles a single rule's LHS into a standalone instruction
sequence ending with `Proceed`:

```
  Input:  LHS = (f $x)
  Output: WamCode {
      instructions: [
          GetArity(A0, 2),
          GetArg(A0, 0, A1),
          GetAtom(A1, "f"),
          GetArg(A0, 1, A2),
          BindSlot(A2, 0),
          Proceed
      ],
      num_slots: 1,
      slot_names: ["$x"],
      rhs_templates: []    // filled by compile_rule_group
  }
```

## Rule Group Compilation

`compile_rule_group()` handles both single-rule and multi-rule cases.

### Single Rule (No Choice Points)

For a single rule, the `Proceed` instruction is replaced with `TailEval` and
a `Fail` is appended:

```
  Rule: (= (f $x) "result")

  WamCode {
      instructions: [
          GetArity(A0, 2),
          GetArg(A0, 0, A1),
          GetAtom(A1, "f"),
          GetArg(A0, 1, A2),
          BindSlot(A2, 0),
          TailEval(rhs_index=0, has_variables=true)
      ],
      num_slots: 1,
      slot_names: ["$x"],
      rhs_templates: [
          RhsInfo { template: "result", has_variables: true, ... }
      ]
  }
```

Note: no `Fail` after `TailEval` for single rules -- execution simply terminates
after the match.

### Multiple Rules (Choice Point Chain)

For N rules, each rule's LHS is compiled independently, then the instructions are
interleaved with choice point management:

**Two-pass algorithm**:

1. **First pass**: Compile each rule's LHS via `compile_rule_lhs()`. Track the
   maximum slot count across all rules. Collect `RhsInfo` for each rule.

2. **Second pass**: Interleave compiled sequences with choice instructions and
   patch forward references.

```
  Rules:
    (= (f 0)  "zero")
    (= (f $n) "other")

  First pass:
    Rule 1 LHS: [GetArity A0 2, GetArg A0 0 A1, GetAtom A1 "f",
                  GetArg A0 1 A2, GetLong A2 0, Proceed]
                  num_slots = 0

    Rule 2 LHS: [GetArity A0 2, GetArg A0 0 A1, GetAtom A1 "f",
                  GetArg A0 1 A2, BindSlot A2 0, Proceed]
                  num_slots = 1

    max_slots = 1

  Second pass (interleave + patch):
    Offset 0:  TryMeElse(8)        <- patched: alt 2 starts at 8
    Offset 1:  GetArity A0, 2
    Offset 2:  GetArg A0, 0, A1
    Offset 3:  GetAtom A1, "f"
    Offset 4:  GetArg A0, 1, A2
    Offset 5:  GetLong A2, 0
    Offset 6:  TailEval(0, false)
    Offset 7:  Fail
    Offset 8:  TrustMe             <- alt 2 (last alternative)
    Offset 9:  GetArity A0, 2
    Offset 10: GetArg A0, 0, A1
    Offset 11: GetAtom A1, "f"
    Offset 12: GetArg A0, 1, A2
    Offset 13: BindSlot A2, 0
    Offset 14: TailEval(1, true)
    Offset 15: Fail
```

### Forward Reference Patching

The `TryMeElse` and `RetryMeElse` instructions contain forward references to
the start of the next alternative. These cannot be known until all alternatives
are laid out. The compiler uses a two-step process:

1. Emit placeholder values (`next_alternative = 0`) during layout
2. Record the offset of each alternative's start
3. Patch the placeholders with actual offsets after layout

```rust
// Record offsets during layout
let mut alt_offsets: Vec<usize> = Vec::with_capacity(n);
// ... emit instructions, recording offset of each alternative ...

// Patch forward references
for i in 0..n - 1 {
    let next_offset = alt_offsets[i + 1] as u16;
    match &mut all_instructions[alt_offsets[i]] {
        WamInstruction::TryMeElse { next_alternative } => {
            *next_alternative = next_offset;
        }
        WamInstruction::RetryMeElse { next_alternative } => {
            *next_alternative = next_offset;
        }
        _ => unreachable!(),
    }
}
```

## Per-Rule Slot Names

In a multi-rule group, different rules may use different variable names at
different slot indices. For example:

```metta
(= (f $a $b) rhs1)     ; $a -> slot 0, $b -> slot 1
(= (f $x $y) rhs2)     ; $x -> slot 0, $y -> slot 1
```

When `TailEval` converts the binding frame to `GenericBindings`, it needs the
correct variable names for the *current* rule, not a global name mapping. Each
`RhsInfo` carries its own `slot_names` vector:

```rust
pub struct RhsInfo {
    pub template: MettaValue,
    pub has_variables: bool,
    pub rhs_type: Option<MettaValue>,
    pub multiplicity: u64,
    pub slot_names: Vec<&'static str>,  // per-rule mapping
}
```

At `TailEval` execution time, the engine temporarily swaps the frame's `names`
field with the per-rule `slot_names`:

```rust
let saved_names = std::mem::replace(
    &mut state.frame.names,
    SmallVec::from_slice(&rhs_info.slot_names),
);
let bindings = state.frame.to_generic_bindings();
state.frame.names = saved_names;  // restore
```

## WamCode Structure

The compiled output is stored in a `WamCode` struct, wrapped in `Arc` for
shared ownership:

```rust
pub struct WamCode {
    pub instructions: Vec<WamInstruction>,   // instruction sequence
    pub num_slots: u16,                       // max binding frame slots
    pub slot_names: Vec<&'static str>,        // global slot name mapping
    pub rhs_templates: Vec<RhsInfo>,          // RHS info per rule
}
```

The `Arc<WamCode>` is stored in `RuleEntry.wam_code` and shared across all
evaluations of that rule group. The `WamState` clones the `Arc` (O(1) reference
count increment) rather than the `WamCode` contents.

## Worked Example: PLN Rule Compilation

Consider a PLN (Probabilistic Logic Network) inference rule:

```metta
(= (|- ($A $TVA) ($B $TVB))
   (deduce $A $TVA $B $TVB))
```

LHS pattern: `(|- ($A $TVA) ($B $TVB))`

### Traversal Order

```
  A0  = (|- ($A $TVA) ($B $TVB))
  A1  = |-
  A2  = ($A $TVA)
  A3  = $A
  A4  = $TVA
  A5  = ($B $TVB)
  A6  = $B
  A7  = $TVB
```

### Slot Assignment

| Variable | First Seen | Slot |
|----------|-----------|------|
| `$A` | A3 | 0 |
| `$TVA` | A4 | 1 |
| `$B` | A6 | 2 |
| `$TVB` | A7 | 3 |

### Compiled Instructions

```
  0: GetArity   A0, 3       ; (|- _ _) has 3 elements
  1: GetArg     A0, 0, A1
  2: GetAtom    A1, "|-"    ; head is "|-"
  3: GetArg     A0, 1, A2
  4: GetArity   A2, 2       ; ($A $TVA) has 2 elements
  5: GetArg     A2, 0, A3
  6: BindSlot   A3, 0       ; $A -> slot 0
  7: GetArg     A2, 1, A4
  8: BindSlot   A4, 1       ; $TVA -> slot 1
  9: GetArg     A0, 2, A5
 10: GetArity   A5, 2       ; ($B $TVB) has 2 elements
 11: GetArg     A5, 0, A6
 12: BindSlot   A6, 2       ; $B -> slot 2
 13: GetArg     A5, 1, A7
 14: BindSlot   A7, 3       ; $TVB -> slot 3
 15: TailEval   (0, true)   ; RHS: (deduce $A $TVA $B $TVB)
```

### Register Usage

```
  A0  A1  A2  A3  A4  A5  A6  A7  A8..A15
  |   |   |   |   |   |   |   |   (unused)
  v   v   v   v   v   v   v   v
  root |- pair $A $TVA pair $B $TVB
            ^               ^
            |               |
         child[1]        child[2]
```

8 of 16 registers used. Total instructions: 16 (including TailEval). Compare
with StructuralMatcher which would need 7 path navigations from root.

## Compilation Failures and Fallback

The compiler returns `None` for patterns it cannot handle, triggering a fallback
to the existing StructuralMatcher/MORK path:

| Pattern Feature | Supported? | Reason |
|----------------|-----------|--------|
| S-expression decomposition | Yes | Core WAM operation |
| Atom matching | Yes | `GetAtom` |
| Integer matching | Yes | `GetLong` |
| Boolean matching | Yes | `GetBool` |
| Float matching | Yes | `GetFloat` |
| String matching | Yes | `GetString` |
| Variable binding | Yes | `BindSlot` |
| Repeated variables | Yes | `EqualCheck` |
| Wildcards | Yes | No instruction emitted |
| Type patterns | No | Requires type system integration |
| Conjunction patterns | No | Multiple constraints per position |
| Error patterns | No | Error matching semantics |
| Quoted patterns | No | Quote-level tracking needed |
| Register exhaustion (>16 nodes) | No | Fixed register limit |

## References

- Warren, D. H. D. (1983). *An Abstract Prolog Instruction Set*. Technical Note 309, SRI International.
- Ait-Kaci, H. (1991). *Warren's Abstract Machine: A Tutorial Reconstruction*. MIT Press.
