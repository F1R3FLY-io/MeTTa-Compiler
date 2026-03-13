# 2. Classical WAM Architecture

This chapter presents the classical Warren Abstract Machine as described by
Warren [1983] and reconstructed by Ait-Kaci [1991]. Understanding the classical
design is essential for appreciating the adaptations made for MeTTa in Chapter 3.

## Overview

The WAM is a register-based abstract machine with five memory areas, a fixed
register file, and an instruction set designed for Prolog execution. Its key
innovation is decomposing unification -- the central operation of logic
programming -- into sequential instruction sequences that operate on registers
and memory cells.

## Memory Architecture

The classical WAM organizes memory into five distinct areas:

```
  Address Space
  +============================================================+
  |                                                            |
  |  +--------------+                   +--------------+       |
  |  |    HEAP      |                   | STACK (AND/OR)|      |
  |  |  (Global     | ----grows--->     | <---grows---- |      |
  |  |   Stack)     |                   |  Environment  |      |
  |  |              |                   |  & Choice Pt  |      |
  |  +--------------+                   +--------------+       |
  |        ^                                   ^               |
  |        | H (heap top)           B/E (choice/env top)       |
  |                                                            |
  |  +--------------+                   +--------------+       |
  |  |    TRAIL     | ----grows--->     |     PDL       |      |
  |  |  (Binding    |                   |  (Push-Down   |      |
  |  |   Undo Log)  |                   |   List for    |      |
  |  |              |                   |   Unification) |      |
  |  +--------------+                   +--------------+       |
  |        ^                                   ^               |
  |        | TR (trail top)            (temporary)             |
  |                                                            |
  +============================================================+
```

### Heap (Global Stack)

The heap stores compound terms (structures and lists) built during execution.
Terms are represented as tagged cells:

```
  Heap Cell (word-sized)
  +--------+-------------------------------------------+
  |  Tag   |              Value / Address               |
  +--------+-------------------------------------------+

  Tag values:
    REF  -- reference (variable, points to another cell)
    STR  -- structure (points to functor cell)
    FUN  -- functor/arity pair (f/n)
    INT  -- integer constant
    CON  -- atom constant
    LIS  -- list (points to head cell; tail is next cell)
```

A compound term `f(a, g(X))` is stored as:

```
  Heap
  +------+-----+-------------------------------------------+
  |  H0  | STR |  H1                                       |
  +------+-----+-------------------------------------------+
  |  H1  | FUN |  f/2                                      |
  +------+-----+-------------------------------------------+
  |  H2  | CON |  a                                        |
  +------+-----+-------------------------------------------+
  |  H3  | STR |  H4                                       |
  +------+-----+-------------------------------------------+
  |  H4  | FUN |  g/1                                      |
  +------+-----+-------------------------------------------+
  |  H5  | REF |  H5  (unbound variable, self-referencing) |
  +------+-----+-------------------------------------------+
```

Unbound variables are represented as self-referencing REF cells: a REF cell
whose value field points to its own address. Binding a variable changes the
value field to point to the bound term.

### Registers

The WAM has a fixed bank of argument registers A1, A2, ..., An used for:
- Passing arguments to predicates
- Holding intermediate values during unification
- Decomposing compound terms

Warren's original design also introduces **temporary registers** (X registers)
that alias the argument registers, and **permanent variables** (Y registers)
that live in environment frames on the stack.

```
  Register File
  +------+------+------+------+------+------+----+------+
  |  A1  |  A2  |  A3  |  A4  |  A5  |  A6  |... |  An  |
  +------+------+------+------+------+------+----+------+
    ^                                                ^
    |                                                |
  First argument                          Last argument
  (also X1)                               (also Xn)
```

### Stack (And/Or Stack)

The stack interleaves two types of frames:

**Environment frames** store permanent variables for a clause:

```
  Environment Frame
  +==========================+
  |  CE  (continuation env)  |  -- pointer to caller's environment
  +==========================+
  |  CP  (continuation point)|  -- return address
  +==========================+
  |  Y1  (perm var 1)        |
  +--------------------------+
  |  Y2  (perm var 2)        |
  +--------------------------+
  |  ...                     |
  +--------------------------+
  |  Yn  (perm var n)        |
  +==========================+
```

**Choice point frames** save the machine state for backtracking:

```
  Choice Point Frame
  +==================================+
  |  n   (arity / num saved args)    |
  +==================================+
  |  A1  (saved register 1)          |
  +----------------------------------+
  |  A2  (saved register 2)          |
  +----------------------------------+
  |  ...                             |
  +----------------------------------+
  |  An  (saved register n)          |
  +==================================+
  |  E   (saved environment pointer) |
  +----------------------------------+
  |  CP  (saved continuation point)  |
  +----------------------------------+
  |  B   (previous choice point)     |
  +----------------------------------+
  |  BP  (next alternative clause)   |
  +----------------------------------+
  |  TR  (saved trail pointer)       |
  +----------------------------------+
  |  H   (saved heap pointer)        |
  +==================================+
```

### Trail

The trail is a stack of heap addresses that records which variables were bound
during the current branch of execution. On backtracking, the trail is unwound
to reset variables to their unbound state.

```
  Trail
  +------+------+------+------+------+
  | addr | addr | addr | addr | addr | ...
  +------+------+------+------+------+
    TR=0   TR=1   TR=2   TR=3   TR=4
```

**Conditional trailing**: A variable at address `a` is trailed only if
`a < HB` (the heap backtrack point saved in the current choice point).
Variables created after the choice point do not need trailing because
backtracking will reclaim that portion of the heap.

### Push-Down List (PDL)

A temporary stack used by the `unify` instruction to manage recursive
structure unification. Pairs of addresses are pushed onto the PDL, and
the unification algorithm processes them iteratively rather than recursively.

## Machine Registers

Beyond the argument registers, the WAM maintains these special-purpose registers:

| Register | Name | Purpose |
|----------|------|---------|
| `P` | Program counter | Current instruction address |
| `CP` | Continuation pointer | Return address after call |
| `E` | Environment pointer | Top of environment stack |
| `B` | Backtrack pointer | Top of choice point stack |
| `A` | Stack top | Top of combined and/or stack |
| `H` | Heap top | Next free heap cell |
| `HB` | Heap backtrack | H at last choice point (for conditional trailing) |
| `TR` | Trail top | Next free trail entry |
| `S` | Structure pointer | Current position in structure being unified |
| `mode` | Read/Write | Unification mode flag |

## Instruction Set

The classical WAM instruction set is organized into categories:

### Put Instructions (Load Argument Registers for Call)

| Instruction | Semantics |
|-------------|-----------|
| `put_variable Xn, Ai` | Create unbound var on heap, store in Xn and Ai |
| `put_variable Yn, Ai` | Initialize Yn in environment, copy to Ai |
| `put_value Xn, Ai` | Copy Xn to Ai |
| `put_value Yn, Ai` | Copy Yn to Ai |
| `put_structure f/n, Ai` | Push STR cell on heap, set Ai |
| `put_constant c, Ai` | Set Ai to constant c |
| `put_list Ai` | Push LIS cell on heap, set Ai |

### Get Instructions (Unify Argument Registers)

| Instruction | Semantics |
|-------------|-----------|
| `get_variable Xn, Ai` | Copy Ai to Xn |
| `get_variable Yn, Ai` | Copy Ai to Yn |
| `get_value Xn, Ai` | Unify Xn with Ai |
| `get_value Yn, Ai` | Unify Yn with Ai |
| `get_structure f/n, Ai` | Match or build structure f/n from Ai |
| `get_constant c, Ai` | Unify Ai with constant c |
| `get_list Ai` | Match or build list from Ai |

### Unify Instructions (Structure Argument Processing)

| Instruction | Semantics |
|-------------|-----------|
| `unify_variable Xn` | In read mode: copy S^ to Xn. In write mode: push unbound var |
| `unify_variable Yn` | Same, but into permanent variable Yn |
| `unify_value Xn` | In read mode: unify S^ with Xn. In write mode: push Xn to heap |
| `unify_value Yn` | Same, but from permanent variable Yn |
| `unify_constant c` | In read mode: check S^ = c. In write mode: push c to heap |

### Control Instructions

| Instruction | Semantics |
|-------------|-----------|
| `allocate` | Create new environment frame |
| `deallocate` | Remove top environment frame |
| `call P/n` | Call predicate P with arity n |
| `proceed` | Return from clause (continue at CP) |

### Choice Instructions (Nondeterminism)

| Instruction | Semantics |
|-------------|-----------|
| `try_me_else L` | Create choice point, next alternative at L |
| `retry_me_else L` | Update choice point, next alternative at L |
| `trust_me` | Remove choice point (last alternative) |
| `try L` | Variant: allocate choice point and jump to L |
| `retry L` | Variant: reset and jump to L |
| `trust L` | Variant: deallocate choice point and jump to L |

### Indexing Instructions

| Instruction | Semantics |
|-------------|-----------|
| `switch_on_term Lv, Lc, Ll, Ls` | Branch on first argument type |
| `switch_on_constant Table` | Hash-table dispatch on constants |
| `switch_on_structure Table` | Hash-table dispatch on functors |

## Execution Model

### Forward Execution

During forward execution, the WAM:
1. Fetches the instruction at `P`
2. Executes it (possibly modifying registers, heap, stack)
3. Advances `P` to the next instruction (unless a `call` or branch changes it)

### Unification

Unification is split into two modes controlled by the `mode` register:

- **Read mode**: The structure already exists on the heap. Instructions compare
  arguments positionally using the `S` (structure pointer) register.
- **Write mode**: The structure is being built. Instructions create new heap cells.

The `get_structure f/n, Ai` instruction determines the mode:
- If Ai is an unbound variable: enter write mode, push `STR f/n` to heap, bind Ai
- If Ai points to `STR f/n`: enter read mode, set S to first argument
- Otherwise: fail

### Backtracking

When unification fails or all clauses for a predicate have been tried:
1. Restore registers from the most recent choice point at `B`
2. Unwind the trail from `TR` back to the saved `TR` in the choice point
3. Reset `H` to the saved `HB` (reclaiming heap allocated during failed branch)
4. Set `P` to the next alternative clause address (`BP`)
5. If this was the last alternative (`trust_me`), remove the choice point

### Prolog's Depth-First Search

The classical WAM implements Prolog's **depth-first, left-to-right** search:
- Clauses are tried in program order
- The first successful derivation is returned
- `fail` or unsatisfied goals trigger backtracking
- The `cut` (!) operator prunes remaining alternatives

This yields a **single-solution** model: the programmer uses `findall/3` or
explicit backtracking to collect multiple answers.

## Complexity Analysis

| Operation | Complexity |
|-----------|-----------|
| Register access | O(1) |
| Heap allocation | O(1) amortized (bump pointer) |
| Variable binding | O(1) (write cell + trail entry) |
| Trail undo (backtrack) | O(k) where k = bindings since choice point |
| Structure unification | O(n) where n = term size |
| Choice point creation | O(a) where a = arity (register save) |
| First-argument indexing | O(1) amortized (hash table) |

## Limitations for MeTTa

Several aspects of the classical WAM do not apply to MeTTa:

1. **Bidirectional unification**: Prolog unifies query terms with clause heads
   bidirectionally. MeTTa uses one-directional pattern matching (the query is
   ground; only the rule's LHS contains pattern variables).

2. **Single-solution search**: Prolog returns the first solution and backtracks
   on demand. MeTTa requires *all* matching rules to fire and results to be
   accumulated as an unordered set.

3. **No `cut`**: MeTTa has no mechanism to prune the search space. All
   alternatives are always explored.

4. **No heap-allocated terms**: MeTTa values are slab-allocated with `'static`
   lifetime. There is no WAM heap with bump-pointer allocation and backtrack
   reclamation.

5. **No environment frames**: MeTTa's evaluation stack is managed by the
   trampoline's continuation stack, not WAM environment frames.

6. **Special forms**: MeTTa has `if`, `let`, `let*`, `chain`, `case`, `match`,
   and other special forms that are not predicates and require dedicated
   evaluation -- they cannot be compiled to WAM unification sequences.

These differences motivate the MeTTa-specific adaptations described in Chapter 3.

## References

- Warren, D. H. D. (1983). *An Abstract Prolog Instruction Set*. Technical Note 309, SRI International.
- Ait-Kaci, H. (1991). *Warren's Abstract Machine: A Tutorial Reconstruction*. MIT Press.
- Clocksin, W. F. and Mellish, C. S. (2003). *Programming in Prolog*. 5th ed., Springer.
