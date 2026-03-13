# 4. Instruction Set Reference

This chapter provides a complete reference for the MeTTa-WAM instruction set.
Every instruction is documented with its encoding, operational semantics, failure
conditions, and usage examples.

Source: `src/backend/eval/wam/instructions.rs`

## Instruction Encoding

Instructions are represented as a Rust enum (`WamInstruction`) with 17 variants.
Each variant carries its operands inline. At the bytecode level (reserved for
future use), each instruction has a unique opcode byte:

```
  WamOpcode (u8)
  +======+=================+==========+
  | Code | Instruction     | Category |
  +======+=================+==========+
  | 0x01 | GetArity        | Match    |
  | 0x02 | GetAtom         | Match    |
  | 0x03 | GetLong         | Match    |
  | 0x04 | GetBool         | Match    |
  | 0x05 | GetFloat        | Match    |
  | 0x06 | GetString       | Match    |
  +------+-----------------+----------+
  | 0x10 | GetArg          | Decomp.  |
  +------+-----------------+----------+
  | 0x20 | BindSlot        | Binding  |
  | 0x21 | EqualCheck      | Binding  |
  | 0x22 | LoadSlot        | Binding  |
  +------+-----------------+----------+
  | 0x30 | TryMeElse       | Control  |
  | 0x31 | RetryMeElse     | Control  |
  | 0x32 | TrustMe         | Control  |
  | 0x33 | Proceed         | Control  |
  | 0x34 | Fail            | Control  |
  +------+-----------------+----------+
  | 0x40 | TailEval        | Eval     |
  | 0x41 | YieldToTrampoline| Eval    |
  +======+=================+==========+
```

The Rust enum representation uses 8-32 bytes per instruction (enum discriminant +
largest variant with padding). The `size_of::<WamInstruction>()` is verified by
unit tests to be at most 32 bytes.

## Category 1: Head Matching

These instructions check structural properties of register values. On failure,
they invoke `wam_fail()` to trigger backtracking.

### GetArity

**Syntax**: `GetArity reg, expected`

**Operands**:
- `reg: u8` -- Register containing the value to check
- `expected: u16` -- Expected number of children in the S-expression

**Semantics**:

```
  if registers[reg] is SExpr(items) AND items.len() == expected:
      continue to next instruction
  else:
      wam_fail()
```

**Failure condition**: The register value is not an S-expression, or its arity
(number of children) does not match `expected`.

**Usage**: Always emitted as the first check for an S-expression pattern node.
Subsequent `GetArg` instructions rely on the arity being verified.

**Example**:
```
  Pattern: (f $x $y)
  Code:    GetArity A0, 3     ; (f $x $y) has 3 elements
```

### GetAtom

**Syntax**: `GetAtom reg, expected`

**Operands**:
- `reg: u8` -- Register containing the value to check
- `expected: &'static str` -- Expected atom string (interned)

**Semantics**:

```
  if registers[reg] is Atom(name) AND (ptr_eq(name, expected) OR name == expected):
      continue to next instruction
  else:
      wam_fail()
```

**Optimization**: Since atom strings are interned by the slab allocator, the
comparison first tries pointer equality (`std::ptr::eq`), which is a single
comparison instruction. String comparison is the fallback for atoms from different
interning epochs.

**Failure condition**: The register value is not an atom, or the atom name does
not match `expected`.

**Example**:
```
  Pattern: (f $x)
  Code:    GetArity A0, 2
           GetArg A0, 0, A1
           GetAtom A1, "f"    ; check head is atom "f"
```

### GetLong

**Syntax**: `GetLong reg, expected`

**Operands**:
- `reg: u8` -- Register containing the value to check
- `expected: i64` -- Expected integer value

**Semantics**:

```
  if registers[reg] is Long(n) AND n == expected:
      continue to next instruction
  else:
      wam_fail()
```

**Example**:
```
  Pattern: (f 0)
  Code:    GetArity A0, 2
           GetArg A0, 0, A1
           GetAtom A1, "f"
           GetArg A0, 1, A2
           GetLong A2, 0      ; check second element is integer 0
```

### GetBool

**Syntax**: `GetBool reg, expected`

**Operands**:
- `reg: u8` -- Register containing the value to check
- `expected: bool` -- Expected boolean value

**Semantics**:

```
  if registers[reg] is Bool(b) AND b == expected:
      continue to next instruction
  else:
      wam_fail()
```

**Example**:
```
  Pattern: (if True $x $y)
  Code:    ...
           GetArg A0, 1, A2
           GetBool A2, true   ; check condition is True
```

### GetFloat

**Syntax**: `GetFloat reg, expected_bits`

**Operands**:
- `reg: u8` -- Register containing the value to check
- `expected_bits: u64` -- Expected float value as raw IEEE 754 bits

**Semantics**:

```
  if registers[reg] is Float(f) AND f.to_bits() == expected_bits:
      continue to next instruction
  else:
      wam_fail()
```

**Note**: Bitwise comparison is used instead of `==` to avoid NaN comparison
issues. `NaN != NaN` in IEEE 754, but `NaN.to_bits() == NaN.to_bits()` for the
same NaN representation.

### GetString

**Syntax**: `GetString reg, expected`

**Operands**:
- `reg: u8` -- Register containing the value to check
- `expected: &'static str` -- Expected string value (interned)

**Semantics**:

```
  if registers[reg] is String(s) AND s == expected:
      continue to next instruction
  else:
      wam_fail()
```

**Note**: String values in MeTTa (double-quoted `"..."`) are distinct from atom
values (unquoted identifiers). The compiler interns the expected string via
`global_allocator().alloc_str()` at compile time.

## Category 2: Argument Decomposition

### GetArg

**Syntax**: `GetArg source_reg, child_index, target_reg`

**Operands**:
- `source_reg: u8` -- Register containing the parent S-expression
- `child_index: u8` -- Zero-based index of the child to extract
- `target_reg: u8` -- Register to store the extracted child

**Semantics**:

```
  let items = registers[source_reg].as_sexpr()  // always valid after GetArity
  registers[target_reg] = items[child_index]
```

**Precondition**: A prior `GetArity` instruction has verified that `source_reg`
contains an S-expression with sufficient arity. Violating this precondition
triggers a panic in debug builds.

**Example**:
```
  Input: (f (g 42) 99)

  GetArity A0, 3          ; A0 = (f (g 42) 99), verify 3 elements
  GetArg   A0, 0, A1      ; A1 = f
  GetArg   A0, 1, A2      ; A2 = (g 42)
  GetArg   A0, 2, A3      ; A3 = 99
```

**Performance**: This is the key instruction that eliminates redundant tree
navigation. In the StructuralMatcher approach, checking the inner `g` atom
requires navigating `root -> child[1] -> child[0]` from scratch. With GetArg,
each level is extracted once and stored in a register.

```
  StructuralMatcher path navigation:
    check_atom_at([1, 0], "g")  --> navigate(root, [1, 0])
    check_var_at([1, 1])        --> navigate(root, [1, 1])
    check_var_at([2])           --> navigate(root, [2])

    Total navigations: 3 (each O(depth))

  WAM register decomposition:
    GetArg A0, 1, A2            ; extract child[1] once
    GetArg A2, 0, A3            ; extract grandchild[0] once
    GetAtom A3, "g"             ; O(1) register read
    GetArg A2, 1, A4            ; extract grandchild[1] once

    Total navigations: 0 (all O(1) register reads after extraction)
```

## Category 3: Variable Binding

### BindSlot

**Syntax**: `BindSlot reg, slot`

**Operands**:
- `reg: u8` -- Register containing the value to bind
- `slot: u16` -- Binding frame slot index

**Semantics**:

```
  let previous = frame.slots[slot]
  trail.push(TrailEntry { slot_index: slot, previous })
  frame.slots[slot] = registers[reg]
```

**Trail interaction**: The previous value at the slot is recorded on the trail
*before* the new value is written. This enables the trail to restore the slot
to its pre-binding state on backtrack.

**First occurrence**: Emitted when a variable is first encountered during LHS
compilation. The slot index is assigned by the compiler's `alloc_slot()`.

**Example**:
```
  Pattern: (f $x $y)
  Slot assignment: $x -> slot 0, $y -> slot 1

  GetArg A0, 1, A2
  BindSlot A2, 0      ; bind $x = value at A2
  GetArg A0, 2, A3
  BindSlot A3, 1      ; bind $y = value at A3
```

### EqualCheck

**Syntax**: `EqualCheck reg, slot`

**Operands**:
- `reg: u8` -- Register containing the value to check
- `slot: u16` -- Binding frame slot index with the expected value

**Semantics**:

```
  if registers[reg] == frame.slots[slot]:
      continue to next instruction
  else:
      wam_fail()
```

**Repeated variables**: Emitted when a variable appears more than once in the
LHS pattern. The first occurrence uses `BindSlot`; subsequent occurrences use
`EqualCheck` to verify consistency.

**Example**:
```
  Pattern: (f $x $x)    ; $x must match in both positions
  Slot assignment: $x -> slot 0

  GetArg A0, 1, A2
  BindSlot A2, 0        ; first $x: bind slot 0 = A2
  GetArg A0, 2, A3
  EqualCheck A3, 0      ; second $x: verify A3 == slot 0
```

### LoadSlot

**Syntax**: `LoadSlot slot, target_reg`

**Operands**:
- `slot: u16` -- Binding frame slot index to read
- `target_reg: u8` -- Register to store the value

**Semantics**:

```
  registers[target_reg] = frame.slots[slot]
```

**Usage**: Reserved for future phases where RHS evaluation occurs within the
WAM engine (currently, RHS evaluation is delegated to the trampoline). Would
be used to load bound variable values into registers for use in RHS computation.

## Category 4: Control Flow

### TryMeElse

**Syntax**: `TryMeElse next_alternative`

**Operands**:
- `next_alternative: u16` -- Instruction index of the next alternative

**Semantics**:

```
  choice_points.push(WamChoicePoint {
      trail_mark: trail.mark(),
      frame_slots: frame.num_slots(),
      next_alternative: next_alternative,
      ...
  })
  // Continue with next instruction (first alternative)
```

**Role**: Emitted before the first alternative in a multi-rule group. Creates a
choice point that saves the current state for backtracking.

**Classical WAM correspondence**: Equivalent to `try_me_else L` in Warren's
instruction set, but without saving registers (the input expression in A0 is
preserved across backtracking by the `wam_fail` function).

### RetryMeElse

**Syntax**: `RetryMeElse next_alternative`

**Operands**:
- `next_alternative: u16` -- Instruction index of the next alternative

**Semantics**:

```
  choice_points.top().next_alternative = next_alternative
  // Continue with next instruction (current alternative)
```

**Role**: Emitted before intermediate alternatives (neither first nor last).
Updates the choice point's backtrack target.

### TrustMe

**Syntax**: `TrustMe`

**Operands**: None

**Semantics**:

```
  choice_points.top().next_alternative = SENTINEL  // usize::MAX
  // Continue with next instruction (last alternative)
```

**Role**: Emitted before the last alternative. Marks the choice point for
removal when backtracking reaches it (no more alternatives to try).

**Difference from classical WAM**: In the classical WAM, `trust_me` immediately
removes the choice point. In MeTTa-WAM, the choice point is removed by
`wam_fail()` when it detects the `SENTINEL` value, because the all-solutions
model requires the trail mark for proper unwinding.

### Proceed

**Syntax**: `Proceed`

**Operands**: None

**Semantics**:

```
  state.matched = true
  // Terminate execution loop
```

**Role**: Terminal instruction for standalone LHS compilation (via
`compile_rule_lhs`). In practice, `compile_rule_group` replaces `Proceed` with
`TailEval` for actual rule dispatch.

**Classical WAM correspondence**: In Prolog, `proceed` transfers control to the
continuation point (caller). In MeTTa-WAM, it simply signals that the pattern
match succeeded.

### Fail

**Syntax**: `Fail`

**Operands**: None

**Semantics**:

```
  wam_fail(state)
  // Control transferred to next alternative or execution terminates
```

**Role**: Triggers backtracking. In multi-rule code, `Fail` is emitted after
each `TailEval` to force exploration of the next alternative (all-solutions).

**Classical WAM correspondence**: Implicit in the classical WAM (any failed check
triggers backtracking). Made explicit in MeTTa-WAM to implement the
"accumulate-and-continue" all-solutions model.

## Category 5: Evaluation

### TailEval

**Syntax**: `TailEval rhs_index, has_variables`

**Operands**:
- `rhs_index: u16` -- Index into `WamCode.rhs_templates[]`
- `has_variables: bool` -- Whether the RHS template contains variables

**Semantics**:

```
  let rhs_info = code.rhs_templates[rhs_index]

  let bindings = if has_variables {
      // Swap in per-rule slot names for correct variable mapping
      frame.names = rhs_info.slot_names
      let b = frame.to_generic_bindings()
      frame.names = original_names  // restore
      b
  } else {
      GenericBindings::Empty  // skip allocation for ground RHS
  }

  match_results.push(WamMatchResult { rhs_info, bindings })
  // Fall through to next instruction (typically Fail)
```

**Optimization**: When `has_variables` is `false`, the RHS template is a ground
term (no pattern variables). In this case, `to_generic_bindings()` is skipped
entirely, avoiding SmallVec allocation and linear scan.

**Per-rule slot names**: In multi-rule groups, each rule may use different
variable names at different slot indices. The `rhs_info.slot_names` field
provides the correct mapping for converting the binding frame back to named
bindings. The frame's `names` field is temporarily swapped during conversion.

### YieldToTrampoline

**Syntax**: `YieldToTrampoline`

**Operands**: None

**Semantics**:

```
  // Terminate WAM execution, return control to trampoline
  break
```

**Status**: Reserved for future use (Phase 4: WAM-native special forms). When
the WAM encounters an expression that it cannot evaluate natively (Tier 2 special
forms like `if`, `let*`, `case`), it would yield to the trampoline with the
partially-evaluated expression in register A0.

## Instruction Layout in Multi-Rule Code

For a rule group with N rules, the compiled instruction sequence has this layout:

```
  +=============================================================+
  | TryMeElse(alt_2_offset)                                     |
  +-------------------------------------------------------------+
  |   <Rule 1 LHS matching instructions>                        |
  |   GetArity A0, ...                                          |
  |   GetArg A0, 0, A1                                          |
  |   GetAtom A1, ...                                           |
  |   ...                                                       |
  +-------------------------------------------------------------+
  |   TailEval(rhs_index=0, has_variables=...)                  |
  +-------------------------------------------------------------+
  |   Fail                                                      |
  +=============================================================+
  | RetryMeElse(alt_3_offset)           <-- alt_2_offset        |
  +-------------------------------------------------------------+
  |   <Rule 2 LHS matching instructions>                        |
  +-------------------------------------------------------------+
  |   TailEval(rhs_index=1, has_variables=...)                  |
  +-------------------------------------------------------------+
  |   Fail                                                      |
  +=============================================================+
  |   ...                                                       |
  +=============================================================+
  | TrustMe                             <-- alt_N_offset        |
  +-------------------------------------------------------------+
  |   <Rule N LHS matching instructions>                        |
  +-------------------------------------------------------------+
  |   TailEval(rhs_index=N-1, has_variables=...)                |
  +-------------------------------------------------------------+
  |   Fail                                                      |
  +=============================================================+
```

## Worked Example: Multi-Rule Matching

Given the rules:

```metta
(= (f 0)  "zero")
(= (f $n) "other")
```

And input `(f 0)`, the compiled instruction sequence and execution trace:

```
  Instructions:
    0: TryMeElse(7)           ; alt 2 starts at instruction 7
    1: GetArity A0, 2
    2: GetArg A0, 0, A1
    3: GetAtom A1, "f"
    4: GetArg A0, 1, A2
    5: GetLong A2, 0          ; rule 1: check second arg is 0
    6: TailEval(0, false)     ; rule 1 RHS: "zero" (no variables)
    7: Fail

    8: TrustMe                ; alt 2 (last)
    9: GetArity A0, 2
   10: GetArg A0, 0, A1
   11: GetAtom A1, "f"
   12: GetArg A0, 1, A2
   13: BindSlot A2, 0         ; rule 2: bind $n
   14: TailEval(1, true)      ; rule 2 RHS: "other" (has $n)
   15: Fail

  Execution trace for input (f 0):
    ip=0:  TryMeElse(8) -> push choice point {trail_mark=0, next=8}
    ip=1:  GetArity A0, 2 -> (f 0) has 2 elements -> pass
    ip=2:  GetArg A0, 0, A1 -> A1 = f
    ip=3:  GetAtom A1, "f" -> f == "f" -> pass
    ip=4:  GetArg A0, 1, A2 -> A2 = Long(0)
    ip=5:  GetLong A2, 0 -> 0 == 0 -> pass
    ip=6:  TailEval(0, false) -> accumulate ("zero", {})
    ip=7:  Fail -> backtrack
              unwind trail to mark 0 (nothing to undo)
              reset registers, reload A0 = (f 0)
              jump to ip=8
    ip=8:  TrustMe -> mark choice point as last
    ip=9:  GetArity A0, 2 -> pass
    ip=10: GetArg A0, 0, A1 -> A1 = f
    ip=11: GetAtom A1, "f" -> pass
    ip=12: GetArg A0, 1, A2 -> A2 = Long(0)
    ip=13: BindSlot A2, 0 -> trail: [(0, UNBOUND)], frame: [Long(0)]
    ip=14: TailEval(1, true) -> accumulate ("other", {$n: 0})
    ip=15: Fail -> backtrack
              choice point is SENTINEL (TrustMe) -> pop it
              unwind trail to mark 0: restore slot 0 = UNBOUND
              no more choice points -> terminate

  Results: [("zero", {}), ("other", {$n: 0})]
```

Both rules match -- this is MeTTa's all-solutions semantics in action.

## References

- Warren, D. H. D. (1983). *An Abstract Prolog Instruction Set*. Technical Note 309, SRI International.
- Ait-Kaci, H. (1991). *Warren's Abstract Machine: A Tutorial Reconstruction*. MIT Press.
