# 6. Execution Engine

This chapter describes the WAM execution engine: the instruction dispatch loop,
state machine transitions, backtracking mechanics, and integration with the
MeTTaTron trampoline.

Source: `src/backend/eval/wam/engine.rs`

## Entry Points

The engine provides two entry points:

### wam_dispatch_rules (Primary)

All-solutions rule dispatch. Returns a vector of `(rhs_template, bindings,
rhs_type, has_vars)` tuples, one per successful match, expanded by multiplicity.

```rust
pub fn wam_dispatch_rules(
    value: MettaValue,
    code: &Arc<WamCode>,
    _env: &MettaEnvironment,
) -> Vec<(MettaValue, GenericBindings<MettaValue>, Option<MettaValue>, bool)>
```

**Integration point**: Called from `dispatch_rule_matches()` in the trampoline
when `RuleEntry.wam_code` is present.

### wam_try_match (Single Rule)

Drop-in replacement for `StructuralMatcher::try_match()`. Returns `Some(bindings)`
if the LHS pattern matches, `None` otherwise.

```rust
pub fn wam_try_match(
    expr: &MettaValue,
    code: &Arc<WamCode>,
) -> Option<GenericBindings<MettaValue>>
```

## State Machine

The execution engine operates as a state machine with four states:

```
                     +===========+
                     |  RUNNING  |<-----+
                     +===========+      |
                       |     |          |
            check      |     | check    | backtrack
            passes     |     | fails    | succeeds
                       |     |          |
                       v     v          |
                  +-----+ +------+      |
                  |     | | FAIL |------+
                  |     | +------+
                  |     |    |
                  |     |    | no more
                  |     |    | choice points
                  |     |    |
                  v     v    v
               +=====+ +=========+
               |MATCH| |FINISHED |
               +=====+ +=========+
```

**RUNNING**: The execution loop fetches and dispatches instructions sequentially.
Head-matching and binding instructions execute here. Control transitions to
FAIL when a check instruction fails.

**FAIL**: The `wam_fail()` function searches for a choice point with remaining
alternatives. If found, it unwinds the trail, resets registers, and transitions
back to RUNNING at the next alternative's instruction offset. If no choice points
remain, transitions to FINISHED.

**MATCH**: A `Proceed` instruction was reached (standalone LHS compilation).
Sets `state.matched = true` and terminates the loop. Only used by
`wam_try_match()`.

**FINISHED**: All alternatives exhausted. Execution terminates. The accumulated
`match_results` vector contains all successful matches.

## Execution Loop

The core execution loop in `execute_wam()`:

```
  +---> fetch instruction at ip
  |         |
  |         v
  |     dispatch on instruction type
  |         |
  |     +---+---+---+---+---+---+---+---+
  |     |   |   |   |   |   |   |   |   |
  |     v   v   v   v   v   v   v   v   v
  |    Get  Get Get Get Get Get Bind Eql ...
  |    Ari  Arg Atm Lng Bol Flt Slot Chk
  |     |   |   |   |   |   |   |   |
  |     +---+   +---+---+---+   |   +---> fail? --> wam_fail()
  |     pass    pass            pass               |
  |     |       |               |                  |
  |     v       v               v              +---+---+
  |   ip+=1   ip+=1           ip+=1            | has   | no
  |     |       |               |              | choice|---> terminate
  +-----+-------+---------------+              | point?|
                                               +---+---+
                                                   | yes
                                                   v
                                            unwind trail
                                            reset registers
                                            jump to next alt
                                                   |
                                                   +---> RUNNING
```

### Instruction Fetch

```rust
loop {
    if state.ip >= state.code.instructions.len() {
        break;  // Past end of code
    }
    let instruction = state.code.instructions[state.ip].clone();
    state.ip += 1;
    // ... dispatch ...
}
```

The instruction is cloned (cheap for small enum variants) and the IP is advanced
*before* dispatch. This means that after a check failure, `wam_fail()` does not
need to adjust IP -- it simply sets IP to the next alternative's offset.

### Check Instruction Pattern

All head-matching instructions follow the same pattern:

```rust
WamInstruction::GetXxx { reg, expected } => {
    let val = state.registers.get(reg);
    match val.as_xxx() {
        Some(v) if v == expected => {
            // Pass: continue to next instruction (ip already advanced)
        }
        _ => {
            // Fail: trigger backtracking
            wam_fail(state);
            continue;  // Re-enter fetch loop at new ip
        }
    }
}
```

The `continue` after `wam_fail()` is essential: `wam_fail()` sets the IP to the
next alternative (or past the end), and the loop must re-fetch from the new IP.

### GetAtom Optimization

Atom comparison uses a two-stage strategy:

```rust
WamInstruction::GetAtom { reg, expected } => {
    let val = state.registers.get(reg);
    match val.as_atom() {
        Some(a) if std::ptr::eq(a, expected) || a == expected => {
            // Match
        }
        _ => { wam_fail(state); continue; }
    }
}
```

1. **Pointer comparison** (`std::ptr::eq`): Atoms are interned by the slab
   allocator, so atoms from the same allocation epoch share string addresses.
   This is a single-instruction comparison.
2. **String comparison** (fallback): For atoms from different interning epochs
   (e.g., rule compiled in one session, input from another), full string
   comparison is used.

### GetArg Safety

`GetArg` relies on a prior `GetArity` check to guarantee the S-expression has
sufficient children:

```rust
WamInstruction::GetArg { source_reg, child_index, target_reg } => {
    let val = state.registers.get(source_reg);
    let items = val.as_sexpr()
        .expect("GetArg: source must be S-expr (verified by GetArity)");
    let child = items[child_index as usize];
    state.registers.set(target_reg, child);
}
```

The `.expect()` will panic if `GetArg` is emitted without a preceding `GetArity`.
The compiler guarantees this invariant by always emitting `GetArity` before any
`GetArg` on the same register.

### BindSlot with Trail

The `BindSlot` instruction performs an unconditional trail push:

```rust
WamInstruction::BindSlot { reg, slot } => {
    let val = state.registers.get(reg);
    let prev = state.frame.get_slot(slot);
    state.trail.push(TrailEntry {
        slot_index: slot,
        previous: prev,
    });
    state.frame.set_slot_unchecked(slot, val);
}
```

Trail entries are always pushed, even for the first binding to an UNBOUND slot.
This simplifies the unwinding logic (no conditional trailing) at the cost of
slightly more trail memory in the single-match case. Since 93% of dispatches
match a single rule, the trail entries are simply discarded without unwinding.

### TailEval with Per-Rule Names

The `TailEval` instruction is the most complex, handling binding extraction
with per-rule slot name mapping:

```rust
WamInstruction::TailEval { rhs_index, has_variables } => {
    let rhs_info = state.code.rhs_templates[rhs_index as usize].clone();

    let bindings = if has_variables {
        // Swap in per-rule slot names
        let saved_names = std::mem::replace(
            &mut state.frame.names,
            SmallVec::from_slice(&rhs_info.slot_names),
        );
        let b = state.frame.to_generic_bindings();
        state.frame.names = saved_names;
        b
    } else {
        GenericBindings::Empty  // Ground RHS: skip allocation
    };

    state.match_results.push(WamMatchResult { rhs_info, bindings });
    // Fall through to next instruction (Fail)
}
```

**Ground RHS optimization**: When `has_variables` is `false`, the RHS template
contains no pattern variables. The `to_generic_bindings()` call is skipped
entirely, saving a SmallVec allocation and O(n) scan of slot values.

## Backtracking

The `wam_fail()` function implements backtracking:

```rust
fn wam_fail(state: &mut WamState) {
    loop {
        match state.choice_points.last() {
            None => {
                // No choice points: terminate
                state.ip = state.code.instructions.len();
                return;
            }
            Some(cp) if cp.next_alternative == usize::MAX => {
                // TrustMe sentinel: last alternative exhausted
                let cp = state.choice_points.pop()
                    .expect("checked non-empty");
                state.trail.unwind_to(cp.trail_mark, &mut state.frame);
                let input = state.registers.args[0];
                state.registers.reset();
                state.registers.load_input(input);
                continue;  // Pop and try previous choice point
            }
            Some(_) => {
                // More alternatives: unwind and jump
                let cp = state.choice_points.last()
                    .expect("checked non-empty");
                let next_ip = cp.next_alternative;
                let trail_mark = cp.trail_mark;
                state.trail.unwind_to(trail_mark, &mut state.frame);
                let input = state.registers.args[0];
                state.registers.reset();
                state.registers.load_input(input);
                state.ip = next_ip;
                return;
            }
        }
    }
}
```

### Backtracking Steps

1. **Find choice point**: Check the top of the choice point stack.

2. **Trail unwinding**: Restore binding frame slots to their state at choice
   point creation by popping trail entries in LIFO order:

   ```
   Before unwind:
     trail: [e0, e1, e2, e3]    (mark = 2)
     frame: [Long(42), "hello"]

   After unwind to mark 2:
     trail: [e0, e1]
     frame: [UNBOUND, UNBOUND]  (restored from e3.previous, e2.previous)
   ```

3. **Register reset**: Clear all registers and reload the original input
   expression into A0. The input is always preserved in A0 (the first
   instruction of every alternative reads from A0).

4. **IP jump**: Set the instruction pointer to the next alternative's offset.

### TrustMe Sentinel

When `TrustMe` executes, it sets `next_alternative = usize::MAX` as a sentinel.
When `wam_fail()` encounters this sentinel, it knows:
- This was the last alternative for this choice point
- The choice point should be removed
- Backtracking should continue to the previous choice point (if any)

This differs from the classical WAM where `trust_me` immediately removes the
choice point. The MeTTa-WAM defers removal because:
1. The trail mark is still needed for unwinding
2. The all-solutions model may need to unwind through multiple choice point levels

## State Initialization

`WamState::new()` creates the execution state:

```rust
pub fn new(code: Arc<WamCode>, input: MettaValue) -> Self {
    let mut registers = WamRegisters::new();
    registers.load_input(input);  // A0 = input

    let frame = WamBindingFrame::with_names(&code.slot_names, 0);

    WamState {
        registers,
        frame,
        trail: Trail::new(),            // capacity 64
        choice_points: Vec::with_capacity(4),
        match_results: Vec::new(),
        ip: 0,
        code,
        matched: false,
    }
}
```

**Pre-allocation**: The trail is pre-allocated with capacity 64 (covering 10-20
rules with 2-4 variables each). Choice points are pre-allocated with capacity 4
(sufficient for most rule groups; PLN rarely has more than 4 matching rules per
dispatch).

## Result Conversion

After execution, `wam_dispatch_rules()` converts `WamMatchResult` entries to the
format expected by `dispatch_rule_matches()`:

```rust
let mut results = Vec::with_capacity(state.match_results.len());
for match_result in state.match_results {
    let multiplicity = match_result.rhs_info.multiplicity as usize;
    if multiplicity == 0 { continue; }

    let template = match_result.rhs_info.template;
    let rhs_type = match_result.rhs_info.rhs_type;
    let has_vars = match_result.rhs_info.has_variables;

    // Clone for extra copies (multiplicity > 1)
    for _ in 1..multiplicity {
        results.push((template, match_result.bindings.clone(), rhs_type, has_vars));
    }
    // Move bindings for the last copy (no clone)
    results.push((template, match_result.bindings, rhs_type, has_vars));
}
```

**Multiplicity optimization**: For multiplicity 1 (the common case), the bindings
are moved, not cloned. For higher multiplicities, only the extra copies are cloned;
the last copy still moves.

## Trace Example: Multi-Rule Nondeterministic Match

Given rules and input:

```metta
(= (color red)   "warm")
(= (color blue)  "cool")
(= (color $c)    "unknown")
```

Input: `(color blue)`

### Compiled Code

```
  0:  TryMeElse(7)
  1:  GetArity A0, 2
  2:  GetArg A0, 0, A1
  3:  GetAtom A1, "color"
  4:  GetArg A0, 1, A2
  5:  GetAtom A2, "red"
  6:  TailEval(0, false)        ; RHS: "warm"
  7:  Fail

  8:  RetryMeElse(15)
  9:  GetArity A0, 2
  10: GetArg A0, 0, A1
  11: GetAtom A1, "color"
  12: GetArg A0, 1, A2
  13: GetAtom A2, "blue"
  14: TailEval(1, false)        ; RHS: "cool"
  15: Fail

  16: TrustMe
  17: GetArity A0, 2
  18: GetArg A0, 0, A1
  19: GetAtom A1, "color"
  20: GetArg A0, 1, A2
  21: BindSlot A2, 0            ; $c
  22: TailEval(2, true)         ; RHS: "unknown"
  23: Fail
```

### Execution Trace

```
  State: registers=[], frame=[UNBOUND], trail=[], cp=[], results=[]

  ip=0:  TryMeElse(8)
         push cp {trail_mark=0, next=8}
         cp=[{trail_mark=0, next=8}]

  ip=1:  GetArity A0, 2
         A0=(color blue), len=2 == 2 -> pass

  ip=2:  GetArg A0, 0, A1
         A1 = color

  ip=3:  GetAtom A1, "color"
         "color" == "color" -> pass

  ip=4:  GetArg A0, 1, A2
         A2 = blue

  ip=5:  GetAtom A2, "red"
         "blue" != "red" -> FAIL

  --- wam_fail() ---
  cp.top has next=8
  unwind trail to mark 0 (nothing to undo)
  reset registers, A0 = (color blue)
  jump to ip=8

  ip=8:  RetryMeElse(16)
         update cp: next=16
         cp=[{trail_mark=0, next=16}]

  ip=9:  GetArity A0, 2 -> pass

  ip=10: GetArg A0, 0, A1 -> A1 = color

  ip=11: GetAtom A1, "color" -> pass

  ip=12: GetArg A0, 1, A2 -> A2 = blue

  ip=13: GetAtom A2, "blue"
         "blue" == "blue" -> pass

  ip=14: TailEval(1, false)
         bindings = GenericBindings::Empty (no variables)
         results = [("cool", {})]

  ip=15: Fail

  --- wam_fail() ---
  cp.top has next=16
  unwind trail to mark 0 (nothing to undo)
  reset registers, A0 = (color blue)
  jump to ip=16

  ip=16: TrustMe
         mark cp as last: next=SENTINEL
         cp=[{trail_mark=0, next=SENTINEL}]

  ip=17: GetArity A0, 2 -> pass

  ip=18: GetArg A0, 0, A1 -> A1 = color

  ip=19: GetAtom A1, "color" -> pass

  ip=20: GetArg A0, 1, A2 -> A2 = blue

  ip=21: BindSlot A2, 0
         trail: [(slot=0, prev=UNBOUND)]
         frame: [Atom("blue")]

  ip=22: TailEval(2, true)
         bindings = {$c: Atom("blue")}
         results = [("cool", {}), ("unknown", {$c: blue})]

  ip=23: Fail

  --- wam_fail() ---
  cp.top has next=SENTINEL -> pop cp
  unwind trail to mark 0: restore slot 0 = UNBOUND
  reset registers
  no more choice points -> terminate

  Final results: [("cool", {}), ("unknown", {$c: blue})]
```

Two results are returned: the exact match `"cool"` and the variable match
`"unknown"` with `$c` bound to `blue`. Both rules fire per MeTTa's all-solutions
semantics. The first rule `(color red)` fails at the `GetAtom` check and is
correctly skipped.

## Performance Characteristics

| Operation | Cost |
|-----------|------|
| Instruction fetch + dispatch | O(1) -- array index + enum match |
| GetArity check | O(1) -- length comparison |
| GetAtom check | O(1) expected (pointer eq), O(n) worst (string cmp) |
| GetArg extraction | O(1) -- slice index |
| BindSlot | O(1) -- slot write + trail push |
| EqualCheck | O(1) for scalars, O(n) for S-expressions |
| Trail unwind | O(k) where k = bindings since mark |
| Register reset | O(16) -- fixed array clear |
| Choice point push | O(1) -- Vec push |
| Result accumulation | O(1) -- Vec push |

**Single-match fast path** (93% of dispatches):
- Execute matching instructions: O(pattern_size)
- TailEval: O(num_vars) for binding extraction
- No backtracking, no trail unwinding
- Total: O(pattern_size + num_vars)

**Multi-match path** (7% of dispatches):
- Per alternative: O(pattern_size + num_vars)
- Between alternatives: O(changed_slots) trail unwind + O(16) register reset
- Total: O(num_alternatives x (pattern_size + num_vars + changed_slots))

## References

- Warren, D. H. D. (1983). *An Abstract Prolog Instruction Set*. Technical Note 309, SRI International.
- Ait-Kaci, H. (1991). *Warren's Abstract Machine: A Tutorial Reconstruction*. MIT Press.
