# 8. MeTTaIL Connections

## Overview

MeTTaIL (Meta Type Talk Intermediate Language) is a companion project to MeTTaTron
that provides a **theory-level metalanguage** for defining computational calculi via
Graph-Structured Lambda Theories (GSLTs). Where MeTTaTron operates at the level of
individual MeTTa expressions -- parsing, pattern matching, and evaluating them --
MeTTaIL operates one level up: it defines the *presentations* of algebraic theories
from which the grammars, equations, and rewrite rules of a language are generated.

This chapter documents how MeTTaIL's automata patterns (VPA, WPDS, predicate
dispatch, Ascent codegen, SCC triangularization) relate to WAM components in
MeTTaTron. The two systems are complementary: MeTTaIL generates the *specification*
of a rewrite system, and MeTTaTron (via the WAM) *executes* the pattern matching
and rule dispatch that the specification entails.

```
  MeTTaIL                                   MeTTaTron
  +=================================================+===================================+
  |  GSLT Presentation                              |  WAM Execution Engine              |
  |    shapes, fn symbols, equations, rewrites      |    registers, trail, choice points |
  +-------------------------+-----------------------+---+-------------------------------+
  |  Theory Algebra         |  BNFC Codegen         |   |  Rule Dispatch                |
  |    disjunction (\/),    |    monomorphize        |   |    wam_dispatch_rules()       |
  |    conjunction (/\),    |    arrows + products   |   |    GetArity, GetAtom,         |
  |    subtraction (\),     |                        |   |    BindSlot, TailEval         |
  |    replacements         |                        |   |                               |
  +-----------+-------------+----------+-------------+---+-------------------------------+
  |  Hypercube Transform    |  dag-triangular        |   |  Trampoline / Continuation    |
  |    type-lifting         |    SCC detection       |   |    eval_trampoline_generic()  |
  |    modal types          |    topological sort    |   |    dispatch_rule_matches()    |
  |                         |    wavefront schedule  |   |                               |
  +-----------+-------------+----------+-------------+---+-------------------------------+
  |  Gillespie Extension    |  Automata Module       |   |  Nondeterministic Eval        |
  |    augmented rewrites   |    Automaton -> matrix  |   |    all-solutions semantics    |
  |    stochastic rates     |    CommEvent labels    |   |    choice point backtracking  |
  |    quantum amplitudes   |                        |   |                               |
  +=========================+========================+===+===============================+
```

## MeTTaIL Architecture

MeTTaIL consists of four sub-projects, each addressing a different layer of the
compilation pipeline:

### Sub-Project 1: MeTTaIL Core (Scala)

**Source**: `MeTTaIL/src/main/scala/io/f1r3fly/mettail/`

The core compiler processes `.module` files written in the GSLT metalanguage. A
module defines *theories* (algebraic structures with shapes, function symbols,
equations, and rewrite rules) and instantiates them through a rich algebra of
theory combinators.

The compilation pipeline is defined in `Pipeline.scala` as a sequence of passes:

```
  LoadModules -> DumpASTs -> DumpLinear -> FindFinalInst
       |
       v
  CheckInterpret -> Interpret -> DesugarBinders
       |
       +--- [optional] HypercubePass ---+
       |                                |
       v                                v
  GenerateBNFC <------------------------+
```

**Key source files**:

| File | Role |
|------|------|
| `Pipeline.scala` | Pass sequencing, `Context` threading |
| `ModuleProcessor.scala` | Module loading, import resolution, dotted path resolution |
| `InstInterpreter.scala` | Theory instantiation interpreter |
| `InstInterpreterCases.scala` | Handlers for each `TheoryInst` variant (disjunction, conjunction, subtraction, exports, replacements, terms, equations, rewrites, constructors, references, recursion, free theories) |
| `Hypercube.scala` | Type-lifting transform: generates typed variants of function symbols |
| `DesugarBinds.scala` | Converts binder syntax to arrow-category form |
| `BNFCRenderer.scala` | Monomorphizes arrow and product categories into BNFC grammar rules |
| `AddEqRwHelpers.scala` | Category inference, consistency checking for equations and rewrites |
| `TheoryEnv.scala` | Theory environment: maps dotted paths to `(TheoryDecl, TheoryEnv)` |
| `BasePresOps.scala` | Operations on `BasePres` (the core presentation data structure) |
| `ASTHelpers.scala` | AST traversal utilities: variable extraction, rewrite decomposition |

### Sub-Project 2: MeTTaIL2Matrix (Rust)

**Source**: `MeTTaIL2Matrix/src/`

Compiles directed graphs (including automata from rholang synchronization trees)
into **upper triangular adjacency matrices** via SCC condensation and topological
sorting. This is the `automata -> triangularize -> GPU` step.

| File | Role |
|------|------|
| `lib.rs` | Graph triangularization: Kosaraju SCC, condensation, toposort, matrix construction |
| `automata.rs` | `Automaton`, `CommEvent`, `AutomatonBuilder`, `automaton_to_triangular()` |
| `error.rs` | Error types for triangularization failures |

### Sub-Project 3: MeTTaIL-Gillespie (Rust)

**Source**: `MeTTaIL-Gillespie/src/`

Extends MeTTaIL's rewrite rules with **rate maps** that associate spatial behavior
terms with either real probabilities (classical Gillespie SSA) or complex amplitudes
(quantum CTMC model checking).

| File | Role |
|------|------|
| `augmented_rule.rs` | `AugmentedRewriteRule`: `(LHS_term, LHS_rate_map) ~> (RHS_term, RHS_rate_map)` |
| `gillespie.rs` | Classical Gillespie SSA (stochastic pi-machine style) |
| `quantum.rs` | Quantum Gillespie with complex amplitudes and interference |
| `spatial_behavior.rs` | `SpatialBehavior` enum: Null, Local, Interaction, Parallel, Sequential, Guard, Replicated, Custom |
| `rate_map.rs` | `RateMap`: spatial behavior -> rate/amplitude mapping |
| `rate_value.rs` | `RateValue`: real probability or complex amplitude |
| `simulator.rs` | Unified `Simulator` facade for classical/quantum modes |
| `language_ext.rs` | `RuleBuilder` for ergonomic augmented rule construction |

### Sub-Project 4: GSLT Grammar

**Source**: `GSLT/src/main/bnfc/metta_venus.cf`

The BNFC grammar definition for the MeTTaIL module language itself. Defines the
syntax for modules, theories, theory instances, spaces, processes, and all
supporting constructs. This grammar is compiled by BNFC into ANTLR4 lexer/parser
code consumed by the Scala core.


## GSLT Presentations and WAM Rule Groups

### The BasePres Data Structure

A GSLT presentation is the fundamental output of MeTTaIL's interpretation pipeline.
It corresponds directly to the information a WAM compiler needs to generate
matching code:

```
  BasePres
  +===============================================+
  |  listcat_: [Cat]         -- exported shapes    |
  |  listdef_: [Def]         -- function symbols   |
  |  listequation_: [Eq]     -- equational axioms  |
  |  listrewritedecl_: [Rw]  -- rewrite rules      |
  |  listmapentry_: [MapE]   -- references         |
  +===============================================+
```

The `listdef_` field contains BNFC `Rule` objects, each defining a labeled
production:

```
  Rule
  +===================================+
  |  label_: Label    -- e.g., "PSend" |
  |  cat_:   Cat      -- e.g., "Proc"  |
  |  listitem_: [Item]  -- RHS items   |
  +===================================+
```

This is the grammar-level analog of MeTTaTron's `RuleEntry`:

```rust
// MeTTaTron: src/backend/environment/rule_management.rs
pub struct RuleEntry {
    pub lhs:          MettaValue,     // LHS pattern
    pub rhs:          MettaValue,     // RHS template
    pub wam_code:     Option<Arc<WamCode>>,  // compiled WAM code
    pub multiplicity: u64,
    // ...
}
```

The connection is structural: a MeTTaIL `Rule` defines a **syntactic form** (how
terms of a shape are constructed), while a MeTTaTron `RuleEntry` defines a
**rewrite rule** (how terms matching a pattern are transformed). When MeTTaIL
generates BNFC grammar rules, these define the *constructors* that MeTTaTron's WAM
must subsequently decompose during pattern matching.

### Theory Algebra as Rule Group Management

MeTTaIL's theory combinators -- disjunction (`\/`), conjunction (`/\`),
subtraction (`\`), exports, replacements, terms, equations, and rewrites -- define
how presentations are composed and refined. Each combinator produces a new
`BasePres` that adjusts the set of available rules. This maps to how MeTTaTron's
`RuleIndex` organizes rules into groups for WAM compilation:

```
  MeTTaIL Theory Combinators          MeTTaTron Rule Management
  +================================+  +==============================+
  |  A \/ B   (disjunction)        |  |  Union of rule groups        |
  |  = union exports, terms,       |  |  All rules from both sources |
  |    equations, rewrites         |  |  available for matching       |
  +================================+  +==============================+
  |  A /\ B   (conjunction)        |  |  Intersection filtering      |
  |  = intersect, filter by        |  |  Only rules mentioning       |
  |    common exports              |  |  shared categories match     |
  +================================+  +==============================+
  |  A \ B    (subtraction)        |  |  Rule removal                |
  |  = remove B's contributions    |  |  Excluded patterns never     |
  |    from A                      |  |  compiled to WAM code        |
  +================================+  +==============================+
  |  A Replacements { ... }        |  |  Rule mutation + AST rewrite |
  |  = substitute function symbols |  |  Permute and rename patterns |
  |    with syntax-preserving map  |  |  in compiled WAM sequences   |
  +================================+  +==============================+
```

**Example**: The Rholang module (from `GSLT/src/test/module/Rholang.module`) builds
a complete rho-calculus presentation through compositional theory instantiation:

```
-- MeTTaIL: Rholang.module
Theory FreeRholang() {
  let s  = u.EmptySet() in (
  let m  = u.Monoid(s) in (
  let cm = u.CommutativeMonoid(m) in (
  let pm = ParMonoid(cm) in (
  let qd = QuoteDropCalc(pm) in (
  let nr = NewReplCalc(pm) in (
  let rc = RhoCalc(qd) in (
  let rl = Rholang(nr, rc) in (
  rl
  ))))))))
}
```

Each `let` binding interprets a theory constructor, producing a `BasePres`. The
`ParMonoid(cm)` step, for instance, takes a `CommutativeMonoid` presentation and:

1. Renames exports: `Elem => Proc`
2. Replaces function symbols: `Zero => PZero`, `Plus => PPar`
3. Adds rewrite rules: `RPar1`, `RPar2` (parallel reduction context)

The resulting presentation contains the complete grammar and rewrite rules for the
rho-calculus. If these rules were compiled into MeTTaTron, each rewrite declaration
(`RDecl`) would become a `RuleEntry` with an associated `WamCode`.


## Rewrite Rules: MeTTaIL to WAM

### GSLT Rewrite Declarations

A MeTTaIL rewrite declaration consists of a name, optional hypothetical contexts,
and a base rewrite:

```
  RewriteDecl                  Grammar (from metta_venus.cf)
  +===========================+
  |  RDecl . RewriteDecl      |  -- name : rewrite
  |    ident_: String          |  -- e.g., "RComm"
  |    rewrite_: Rewrite       |  -- the rewrite body
  +===========================+
  |  RewriteBase . Rewrite    |  -- AST ~> AST
  |    ast_1: AST   (LHS)     |
  |    ast_2: AST   (RHS)     |
  +===========================+
  |  RewriteContext . Rewrite  |  -- let Hyp in Rewrite
  |    hypothesis_: Hypothesis |  -- DottedPath ~> DottedPath
  |    rewrite_: Rewrite       |
  +===========================+
```

The AST nodes use the same three-variant structure throughout:

```
  ASTSExp  -- ( Label [AST] )   : constructor application
  ASTVar   -- DottedPath         : variable reference
  ASTSubst -- ( Subst AST AST DottedPath ) : explicit substitution
```

### Mapping to WAM Instructions

A GSLT `RewriteBase` AST on the left-hand side decomposes into the same structural
checks that the WAM compiler generates from a MeTTa rule LHS pattern. Consider the
rho-calculus COMM rule:

```
-- MeTTaIL: GSLT rewrite declaration
RComm : ( PPar ( PRecv y x P ) ( PSend x Q ) ) ~> ( Subst P ( NQuote Q ) y )
```

The LHS pattern `( PPar ( PRecv y x P ) ( PSend x Q ) )` is an `ASTSExp` with
label `PPar` and two children. The WAM compiler would decompose this into:

```
  MeTTaIL AST                          WAM Instructions
  +=================================+  +====================================+
  | (PPar                           |  | GetArity  A0, 3                    |
  |       ^--- label = PPar         |  | GetArg    A0, 0, A1               |
  |                                 |  | GetAtom   A1, "PPar"              |
  |   (PRecv y x P)                 |  | GetArg    A0, 1, A2               |
  |       ^--- label = PRecv        |  | GetArity  A2, 4                   |
  |            args = [y, x, P]     |  | GetArg    A2, 0, A3               |
  |                                 |  | GetAtom   A3, "PRecv"             |
  |   (PSend x Q)                   |  | GetArg    A2, 1, A4  ; y          |
  |       ^--- label = PSend        |  | BindSlot  A4, 0      ; bind $y    |
  |            args = [x, Q]        |  | GetArg    A2, 2, A5  ; x (first)  |
  | )                               |  | BindSlot  A5, 1      ; bind $x    |
  |                                 |  | GetArg    A2, 3, A6  ; P          |
  |                                 |  | BindSlot  A6, 2      ; bind $P    |
  |                                 |  | GetArg    A0, 2, A7               |
  |                                 |  | GetArity  A7, 3                   |
  |                                 |  | GetArg    A7, 0, A8               |
  |                                 |  | GetAtom   A8, "PSend"             |
  |                                 |  | GetArg    A7, 1, A9  ; x (second) |
  |                                 |  | EqualCheck A9, 1     ; x == $x    |
  |                                 |  | GetArg    A7, 2, A10 ; Q          |
  |                                 |  | BindSlot  A10, 3     ; bind $Q    |
  +=================================+  +====================================+
```

The critical observation is that the variable `x` appears twice in the LHS (`PRecv`
and `PSend`). The first occurrence is bound via `BindSlot`; the second occurrence
generates an `EqualCheck` against the same slot. This is exactly MeTTaIL's
`freeVarsInAST` analysis detecting repeated variables (see `Hypercube.scala`), and
exactly the WAM's trail-based handling of repeated pattern variables:

```scala
// MeTTaIL: Hypercube.scala — detecting repeated variables
def freeVarsInAST(ast: AST): Map[String, Set[ASTSExp]] = {
  // Returns var name -> set of ASTSExp nodes directly containing it
  // If a var appears in multiple S-expressions, it is "repeated"
  // and needs equality checking during pattern matching
  ...
}
```

```rust
// MeTTaTron: src/backend/eval/wam/compiler.rs — handling repeated vars
// First occurrence of $x: BindSlot { reg, slot }
// Subsequent occurrences: EqualCheck { reg, slot }
```

### Hypothetical Rewrites and Context Rules

MeTTaIL's `RewriteContext` adds hypothetical assumptions of the form
`let Src ~> Tgt in Rewrite`. These express that a sub-expression may reduce,
enabling context rules like:

```
-- Context rule: reduction under parallel composition
RPar1 : let Src ~> Tgt in
        ( PPar Src Q ) ~> ( PPar Tgt Q )
```

This has no direct WAM analog -- the WAM handles only LHS pattern matching, not
hypothetical reasoning. In MeTTaTron, context rules would be expressed as standard
rules where `Src` and `Tgt` are pattern variables that the trampoline's evaluation
loop instantiates through recursive evaluation. The WAM matches the structural
pattern `(PPar $Src $Q)` and binds `$Src` and `$Q`; the trampoline then evaluates
whether `$Src` can step to some `$Tgt`.


## SCC Triangularization and WAM Choice Points

### The dag-triangular Algorithm

MeTTaIL2Matrix (`MeTTaIL2Matrix/src/lib.rs`) implements a six-step graph
triangularization pipeline:

```
  Input: DiGraph<N, E>
       |
       v
  Step 1: Kosaraju SCC detection
       |  kosaraju_scc(&graph) -> Vec<Vec<NodeIndex>>
       v
  Step 2: Node -> SCC index mapping
       |  node_to_scc: HashMap<NodeIndex, usize>
       v
  Step 3: Condensed DAG construction
       |  SCC nodes collapsed, duplicate edges removed
       v
  Step 4: Topological sort of condensed DAG
       |  toposort(&condensed) -> Vec<NodeIndex>
       v
  Step 5: Position mapping
       |  scc_to_pos: Vec<usize>  (SCC index -> matrix row/col)
       v
  Step 6: Upper triangular matrix construction
       |  matrix[(r, c)] = 1.0 where r < c for DAG edges
       v
  Output: TriangularResult { matrix, groups, node_to_group }
```

### Structural Parallel with WAM Choice Points

The SCC condensation step has a deep structural parallel with WAM choice point
management. In both systems, the core problem is: given a set of states with
potential cycles (mutual dependencies), how do we organize traversal to explore
all alternatives while maintaining correctness?

```
  SCC in dag-triangular                     Choice Points in WAM
  +=================================+      +=================================+
  | States in a cycle are collapsed |      | Rules in a group are chained    |
  | into a single matrix entry.     |      | with TryMeElse/RetryMeElse/    |
  | The condensed DAG is acyclic.   |      | TrustMe.                        |
  +=================================+      +=================================+
  | Topological order determines    |      | Alternative ordering determines |
  | which groups can execute first  |      | which rule is tried first       |
  | (no incoming dependencies).     |      | (first alternative in chain).   |
  +=================================+      +=================================+
  | Wavefront schedule: groups at   |      | All-solutions: every matching   |
  | the same level execute in       |      | rule accumulates a result       |
  | parallel on the GPU.            |      | before backtracking.            |
  +=================================+      +=================================+
```

The wavefront parallel schedule (`TriangularAutomatonResult::wavefront_schedule()`)
computes which groups can execute concurrently based on the in-degree of each node
in the upper triangular matrix:

```rust
// MeTTaIL2Matrix: src/automata.rs
pub fn wavefront_schedule(&self) -> Vec<Vec<usize>> {
    // Compute in-degree from upper triangular matrix
    let mut in_degree = vec![0usize; n];
    for i in 0..n {
        for j in (i + 1)..n {
            if self.triangular.matrix[(i, j)] > 0.0 {
                in_degree[j] += 1;
            }
        }
    }
    // Iteratively collect zero-in-degree nodes as "waves"
    // Each wave can execute in parallel
    ...
}
```

This is analogous to MeTTaTron's `dispatch_rule_matches()` which fans out all
matching rule results for parallel evaluation on the trampoline's work pool:

```rust
// MeTTaTron: src/backend/eval/evaluation.rs (conceptual)
// After WAM produces Vec<WamMatchResult>:
fn dispatch_rule_matches(results: Vec<WamMatchResult>, ...) {
    // Each result is a (rhs_template, bindings) pair
    // All results are dispatched concurrently
    // This is the "wavefront" of rule application
}
```


## Automata and Pattern Matching

### CommEvent-Labeled Automata

MeTTaIL's automata module (`MeTTaIL2Matrix/src/automata.rs`) models rholang
synchronization trees as NFAs whose transitions are labeled with communication
events:

```
  CommEvent
  +=========================================+
  | Send    { channel, data: Vec<RhoData> } |  -- channel!(data)
  | Receive { channel, patterns }           |  -- for(p <- channel)
  | Comm    { channel, data }               |  -- tau @ channel(data)
  | Peek    { channel, patterns }           |  -- for(p <= channel)
  | Tau     { description }                 |  -- internal step
  +=========================================+
```

Each `CommEvent` variant carries structured data that must be pattern-matched
against incoming expressions. This is directly analogous to MeTTaTron's WAM
matching against structured `MettaValue` expressions:

```
  MeTTaIL CommEvent Matching          MeTTaTron WAM Matching
  +================================+  +================================+
  | Match on variant discriminant  |  | GetArity: check S-expr length  |
  | (Send vs Receive vs Comm)      |  | GetAtom: check head symbol     |
  +================================+  +================================+
  | Extract channel name           |  | GetArg: extract child register |
  | Extract data payloads          |  | GetArg: extract child register |
  +================================+  +================================+
  | Match data patterns            |  | GetLong/GetBool/GetString:     |
  | (RhoData::Wildcard, etc.)      |  |   literal checks               |
  +================================+  +================================+
  | Bind pattern variables         |  | BindSlot: write to frame slot  |
  | (RhoData::Symbolic)            |  |                                |
  +================================+  +================================+
```

### RhoData and MettaValue Correspondence

The `RhoData` enum in MeTTaIL's automata module mirrors MeTTaTron's `MettaValue`
with domain-specific types for rholang:

| RhoData Variant | MettaValue Equivalent | Notes |
|----------------|----------------------|-------|
| `Int(i64)` | `Long(i64)` | Direct mapping |
| `Bool(bool)` | `Bool(bool)` | Direct mapping |
| `Str(String)` | `String(&'static str)` | MeTTaTron uses interned strings |
| `List(Vec<RhoData>)` | `SExpr(&'static [MettaValue])` | Nested S-expression |
| `Name(ChannelName)` | `Atom(&'static str)` | Channel names as atoms |
| `Wildcard` | Pattern variable (`$_`) | WAM: any slot binding |
| `Symbolic(String)` | Pattern variable (`$x`) | WAM: `BindSlot` target |

### Visibly Pushdown Automata (VPA) Connection

MeTTaIL's AST structure (`ASTSExp`, `ASTVar`, `ASTSubst`) implicitly defines a
visibly pushdown language: each `ASTSExp` node with label and children corresponds
to a call (push) on the automaton stack, and returning from children corresponds
to a return (pop). The GSLT grammar's category system (`Cat`) provides the type
constraints that a VPA would enforce at each stack level.

The WAM's register-based decomposition can be viewed as a VPA execution strategy
where:

- **GetArg**: push (descend into a child)
- **GetArity/GetAtom/GetLong/...**: verify the stack symbol
- **BindSlot**: record state at the current stack depth
- **Fail + choice point restore**: backtrack (pop and retry)

The key advantage of the WAM's register approach over a literal VPA implementation
is that the registers provide O(1) random access to any previously decomposed
node, whereas a VPA can only access the top of the stack. This is why the WAM
pre-decomposes the entire pattern tree into registers before checking constraints.


## Hypercube Transform and Type-Driven Optimization

### The Type-Lifting Transform

MeTTaIL's Hypercube pass (`Hypercube.scala`) implements the type-lifting
transformation described in `transformation.md`. For each function symbol
`f: product_i A_i -> B`, a type-lifted variant `f': product_i T(A_i) -> T(B)` is
generated, where the type transformation `T` operates as:

```
  T(G)         = G                             (generating shape)
  T(A1 x A2)   = T(A1) x T(A2)                (product)
  T(A1 -> A2)  = T(A1) x (T(A1) -> T(A2))     (arrow -- note duplication)
```

The arrow case introduces an extra parameter, because arrow types in a GSLT carry
both the domain and the function itself. When a base reduction has a variable
duplicated across function symbols in its LHS source, the type-lifted versions gain
extra parameters for the shared variable (the `extend` function in
`Hypercube.scala`).

### Connection to WAM Type-Driven Optimization

MeTTaTron's WAM compiler implements type-driven optimizations (documented in
`docs/wam/05-compilation.md` and implemented across `src/backend/eval/`) that use
type information to prune impossible matches early. The Hypercube transform provides
the formal justification for this: if a term has a type-lifted annotation
`f': T(A_i) -> T(B)`, then the WAM can use the type information to:

1. **Skip arity checks** when the type guarantees a particular structure
2. **Narrow register allocation** when the type constrains child shapes
3. **Eliminate redundant equality checks** when types enforce that shared
   variables must have the same shape

This is precisely the `expected_type` propagation in MeTTaTron's evaluation engine
(`src/backend/eval/types.rs`), where knowing the expected type of a sub-expression
allows the evaluator to short-circuit pattern matching.


## Stochastic Rewriting and WAM Nondeterminism

### Augmented Rewrite Rules

MeTTaIL-Gillespie (`MeTTaIL-Gillespie/src/augmented_rule.rs`) extends standard
rewrite rules with **rate maps**:

```
  Standard:    LHS_term  ~>  RHS_term
  Augmented:  (LHS_term, LHS_rate_map) ~> (RHS_term, RHS_rate_map)
```

Where a rate map associates `SpatialBehavior` terms with `RateValue`s (real
probabilities or complex amplitudes):

```rust
// MeTTaIL-Gillespie: src/augmented_rule.rs
pub struct AugmentedRewriteRule {
    pub name: String,
    pub lhs_term: TermRef,
    pub lhs_rate_map: RateMap,         // spatial behavior -> rate/amplitude
    pub rhs: AugmentedRhs,             // (term, rate_map)
    pub condition: Option<String>,     // optional structural guard
}
```

### Mapping to WAM All-Solutions Semantics

The Gillespie algorithm's rule selection step directly parallels the WAM's
all-solutions semantics:

```
  Gillespie SSA (MeTTaIL)                WAM All-Solutions (MeTTaTron)
  +=====================================+  +====================================+
  | 1. Compute propensity a_i for each  |  | 1. Execute WAM code for each rule  |
  |    matching rule r_i                |  |    in the group (TryMeElse chain)  |
  +=====================================+  +====================================+
  | 2. Sample rule r_mu proportional    |  | 2. Accumulate all matching results |
  |    to a_mu / sum(a_i)              |  |    in match_results Vec            |
  +=====================================+  +====================================+
  | 3. Within r_mu, sample spatial      |  | 3. Each result carries bindings    |
  |    behavior proportional to rates   |  |    (WamMatchResult)               |
  +=====================================+  +====================================+
  | 4. Fire selected rule, produce      |  | 4. dispatch_rule_matches() fans   |
  |    (RHS_term, RHS_rate_map)        |  |    out all results for evaluation  |
  +=====================================+  +====================================+
```

In the classical Gillespie mode, rule selection is probabilistic (weighted by
propensity). In MeTTaTron's all-solutions mode, *all* matching rules fire. The
structural analogy is:

- **Gillespie propensity** <-> **WAM match success/failure per alternative**
- **Cumulative selection** <-> **TryMeElse/RetryMeElse/TrustMe chain traversal**
- **Rate map on RHS** <-> **Bindings in WamMatchResult (carried to RHS evaluation)**

A potential future integration would allow MeTTaTron to use rate annotations from
MeTTaIL to **prioritize** which rule results are evaluated first in the trampoline,
without changing the all-solutions semantics (all results are still produced, but
high-propensity ones are scheduled earlier).

### Quantum Mode and Choice Point Superposition

In quantum mode, MeTTaIL-Gillespie models amplitudes that can interfere:

```rust
// MeTTaIL-Gillespie: src/rhocalc_stochastic.rs
// Two paths with interfering amplitudes:
// Path 1: amplitude 1/sqrt(2)
// Path 2: amplitude i/sqrt(2) (90-degree phase shift)
```

This is a deep structural analog of WAM choice points, where multiple alternatives
coexist until observation (result collection). In the WAM:

1. `TryMeElse` creates a choice point -- a "superposition" of alternatives
2. Each alternative is explored, producing bindings
3. All results are accumulated in `match_results`
4. The trail undoes bindings between alternatives (maintaining isolation)

The quantum analog: each alternative has an amplitude, and the `match_results`
vector would carry amplitude annotations rather than Boolean success/failure.
This is exactly what `AugmentedRewriteRule.lhs_rate_map` provides.


## Theory Algebra Operations as WAM Compilation Strategies

### Disjunction as Rule Group Union

MeTTaIL's theory disjunction (`\/`) takes the union of all components:

```scala
// MeTTaIL: InstInterpreterCases.scala
def handleDisj(interpreter, env, disj): BasePres = {
  val presA = interpreter.interpret(env, disj.theoryinst_1)
  val presB = interpreter.interpret(env, disj.theoryinst_2)
  // Union exports, terms, equations, rewrites (with deduplication)
  ...
}
```

In WAM terms, this corresponds to compiling two rule groups into a single
`WamCode` with `TryMeElse`/`RetryMeElse` chains that span both groups:

```
  Rules from A         Rules from B
  +==============+     +==============+
  | TryMeElse L1 |     | RetryMeElse  |
  | ... match A1 |     | ... match B1 |
  | TailEval 0   |     | TailEval 2   |
  | Fail         |     | Fail         |
  +==============+     +==============+
  | RetryMeElse  |     | TrustMe      |
  | ... match A2 |     | ... match B2 |
  | TailEval 1   |     | TailEval 3   |
  | Fail         |     | Fail         |
  +==============+     +==============+
```

### Conjunction as Pre-Filter

Theory conjunction (`/\`) intersects presentations, keeping only function symbols
and rules whose categories appear in both theories. This is analogous to
MeTTaTron's bloom filter pre-check (`may_have_rules_for(op, arity)`) which rejects
expressions before WAM dispatch when no rules match:

```
  MeTTaIL conjunction filter         MeTTaTron bloom filter
  +===============================+  +==============================+
  | Keep rule if all mentioned    |  | may_have_rules_for(op, arity)|
  | categories are in the         |  | returns false if no rules    |
  | intersection of exports       |  | exist for this head + arity  |
  +===============================+  +==============================+
```

### Subtraction as Rule Exclusion

Theory subtraction (`\`) removes rules from a presentation. In MeTTaTron, this
corresponds to dynamically removing rules via `remove-atom` or simply not
compiling excluded rules into `WamCode`.

### Replacements as WAM Code Rewriting

The `Replacements` combinator is particularly interesting. It performs a
permutation-aware substitution of function symbols:

```
-- MeTTaIL: Rholang.module
Replacements {
  [] Zero.Proc => PZero.Proc ::= "0";
  [0, 1] Plus.Proc => PPar.Proc ::= "(" Proc "|" Proc ")";
}
```

The `[0, 1]` is a permutation specifying how arguments are reordered. After
replacement, all ASTs in equations and rewrites are updated to use the new label
and argument order.

In WAM terms, this is a **compile-time instruction rewrite**: if a WAM sequence
was compiled for `Plus`, the replacement to `PPar` with permutation `[0, 1]`
(identity in this case) would:

1. Change `GetAtom A1, "Plus"` to `GetAtom A1, "PPar"`
2. Reorder the `GetArg` sequence to match the permutation
3. Update slot assignments for any variables that moved

This is a form of partial evaluation -- the replacement is resolved before any
runtime matching occurs.


## Explicit Substitution and WAM Binding

### ASTSubst: The Substitution Node

MeTTaIL's AST includes an explicit substitution construct:

```
ASTSubst . AST ::= "(" "Subst" AST AST DottedPath ")"
```

This represents `AST_1[AST_2 / DottedPath]` -- substituting `AST_2` for the
variable named by `DottedPath` in `AST_1`. The COMM rule in the rho-calculus uses
this directly:

```
RComm : ( PPar ( PRecv y x P ) ( PSend x Q ) )
    ~> ( Subst P ( NQuote Q ) y )
```

The RHS `( Subst P ( NQuote Q ) y )` means "substitute `@Q` for `y` in `P`".

### WAM Binding Frame as Substitution Environment

The WAM's `WamBindingFrame` serves exactly the same purpose as MeTTaIL's explicit
substitution -- it records which variables are bound to which values. The
`apply_bindings` operation in MeTTaTron performs the same computation as evaluating
an `ASTSubst`:

```rust
// MeTTaTron: binding application (conceptual)
fn apply_bindings(template: MettaValue, bindings: &WamBindingFrame) -> MettaValue {
    // Walk the template tree
    // When a variable $x is found, look up bindings.slots[slot_for_x]
    // Replace the variable with the bound value
    // This is exactly ASTSubst semantics
}
```

```scala
// MeTTaIL: AddEqRwHelpers.scala — substitution during interpretation
private def findAndReplace(
  replacement: AST,
  ident: String,
  defs: Map[Label, Rule]
)(ast: AST): AST = {
  ast match {
    case astVar: ASTVar =>
      if (dottedPathToString(astVar.dottedpath_) == ident)
        replacement
      else
        astVar
    case astSExp: ASTSExp => /* recurse, respecting binders */
    case astSubst: ASTSubst => /* compose substitutions */
  }
}
```

The WAM's indexed binding frame provides O(1) access to bound values, compared
to MeTTaIL's tree-walking substitution. This is the fundamental efficiency gain
of the WAM approach: substitution is deferred until needed, and when performed,
variable lookup is a direct slot access rather than a name-based search.


## Binder Desugaring and Register Allocation

### MeTTaIL Binder Syntax

MeTTaIL supports binder syntax for lambda-like constructs:

```
-- GSLT grammar
BindNTerminal . Item ::= "(" "Bind" Ident Cat ")" ;
AbsNTerminal  . Item ::= "(" Ident ")" Item ;
```

The `DesugarBinds` pass converts these into arrow categories:

```scala
// MeTTaIL: DesugarBinds.scala
// Before: PRecv . Proc ::= "for" "(" (Bind x Name) "<-" Name ")" "{" (x)Proc "}"
// After:  PRecvToArrow . Proc ::= "PRecvToArrow" "(" (Name -> Proc) Name ")"
```

This desugaring eliminates binders from the surface syntax, replacing them with
explicit arrow types that the BNFC renderer can monomorphize into concrete
categories.

### WAM Register Allocation for Arrow Types

When arrow types appear in patterns (after desugaring), the WAM must allocate
registers for the function argument. In MeTTaTron, this corresponds to patterns
with nested S-expressions:

```
  MeTTaIL desugared:                 WAM registers:
  PRecvToArrow(F, Name)              A0 = (PRecvToArrow F Name)
                                     A1 = PRecvToArrow  (head)
                                     A2 = F             (arrow arg)
                                     A3 = Name          (channel)
```

The arrow category `(Name -> Proc)` becomes a concrete category
`ArrowCC<Name>_<Proc>DD` after monomorphization in `BNFCRenderer.scala`. The WAM
treats this no differently from any other S-expression -- it is decomposed into
registers and matched structurally.

```scala
// MeTTaIL: BNFCRenderer.scala — monomorphizing arrows
def idCatPairToMangledArrow(src: IdCat, tgt: IdCat): IdCat =
  new IdCat(s"ArrowCC${src.ident_}_${tgt.ident_}DD")

// Generates constructors:
//   AppCCName_ProcDD   . Proc   ::= "alpha" "{" ArrowCCName_ProcDD "(" Name ")" "}"
//   IdentCCName_ProcDD . ArrowCCName_ProcDD ::= Ident
//   LamCCName_ProcDD   . ArrowCCName_ProcDD ::= "lambda" "{" "(" Ident ")" "=>" Proc "}"
```


## Wavefront Parallelism and MeTTaTron's Work Pool

### MeTTaIL2Matrix Wavefront Schedule

The wavefront schedule produced by `automaton_to_triangular` partitions state
groups into waves that respect dependency ordering:

```
  Wave 0: [init]               -> GPU kernel 0
  Wave 1: [branch_a | branch_b] -> GPU kernels 1a, 1b  (parallel)
  Wave 2: [join_point]          -> GPU kernel 2
```

Groups within the same wave have no data dependencies and can execute concurrently.

### MeTTaTron's WorkPool Hill Climber

MeTTaTron's work pool (`src/backend/eval/`) uses a similar wavefront concept for
parallel rule application. When multiple rules match (the 7% multi-match case),
the trampoline dispatches all matching RHS evaluations concurrently:

```
  Wave 0: WAM pattern matching (single thread)
       |
       v  All matches accumulated
  Wave 1: RHS evaluation (parallel on work pool)
       |  result_1, result_2, ..., result_n
       v
  Wave 2: Result collection and continuation
```

The hill climber dynamically adjusts work pool size based on workload, analogous
to how the wavefront schedule's wave depths determine GPU kernel parallelism.

### Potential Integration

A future integration could use MeTTaIL's triangularization to **precompute** the
dependency structure of a set of rewrite rules, then use the wavefront schedule to
inform MeTTaTron's work pool about which rule groups can be dispatched in parallel
without data dependencies. Currently, MeTTaTron discovers this dynamically; a
static precomputation would eliminate runtime dependency checking for known rule
sets.


## Gillespie-Augmented Pattern Matching

### Spatial Behavior as Type Refinement

MeTTaIL-Gillespie's `SpatialBehavior` enum refines the type of a rewrite rule's
LHS. Each variant describes a different mode of spatial interaction:

```rust
// MeTTaIL-Gillespie: src/spatial_behavior.rs
pub enum SpatialBehavior {
    Null,                                          // trivial
    Local(ChannelId),                              // localized
    Interaction { input_channel, output_channel },  // COMM
    Parallel(Vec<SpatialBehavior>),                 // concurrent
    Sequential(Box<SB>, Box<SB>),                   // ordered
    Guard { condition, body },                      // conditional
    Replicated(Box<SpatialBehavior>),               // persistent
    Custom { tag, children },                       // extensible
}
```

In WAM terms, a spatial behavior annotation could serve as an additional
**indexing criterion** for rule dispatch. Currently, MeTTaTron indexes rules by
head symbol and arity. With spatial behavior annotations, the WAM could add a
pre-dispatch filter:

```
  Current WAM dispatch:
    RuleIndex[("PPar", 3)] -> [rule_1, rule_2, rule_3]
    -> try all three via TryMeElse chain

  Spatial-behavior-augmented dispatch:
    RuleIndex[("PPar", 3, Interaction("x", "x"))] -> [rule_1]
    RuleIndex[("PPar", 3, Local("x"))]            -> [rule_2]
    -> skip non-matching spatial behaviors entirely
```

This would be a form of **multi-argument indexing** -- the WAM equivalent of the
classical WAM's `switch_on_term` but extended with spatial behavior discriminants.


## Category Inference and WAM Type Checking

### MeTTaIL's Category Inference

MeTTaIL performs category inference on ASTs to verify that equations and rewrites
are well-typed. The `catOfIdentInAST` function (`AddEqRwHelpers.scala`) infers the
category (type) of a variable from its position in a constructor application:

```scala
// MeTTaIL: AddEqRwHelpers.scala
def catOfIdentInAST(
  ident: String,
  defs: Map[Label, Rule],
  context: Option[Cat],
  ast: AST
): CatOfIdentInASTResult = {
  // For ASTSExp: look up the Rule for the label, zip children with
  // non-terminal positions, recursively infer category of ident
  // For ASTVar: return the context category if ident matches
  // For ASTSubst: compose substitutions and check consistency
}
```

This is structurally identical to the WAM compiler's slot assignment, where each
pattern variable is assigned a binding frame slot and the type of the slot is
determined by the position of the variable in the pattern:

```rust
// MeTTaTron: WAM compiler (conceptual)
// Pattern: (f $x (g $y))
// Rule for f: f . Result ::= Atom Result SubExpr
// -> $x has type Result (position 1 in f's arguments)
// -> $y has type SubExpr (position 1 in g's arguments)
// -> slot 0 ($x): type = Result
// -> slot 1 ($y): type = SubExpr
```

### Consistency Checking

MeTTaIL's `consistentCategory` function verifies that the same variable has the
same category everywhere it appears:

```scala
// MeTTaIL: AddEqRwHelpers.scala
def consistentCategory(ast, defs, prettyStructure): Either[String, Map[String, Cat]]
```

This is the compile-time analog of the WAM's `EqualCheck` instruction, which
verifies at runtime that a repeated variable binds to the same value. The
difference is one of phase:

| Phase | MeTTaIL | MeTTaTron WAM |
|-------|---------|---------------|
| Compile time | Verify category consistency | Assign binding slots, generate EqualCheck |
| Runtime | N/A (categories are erased) | Execute EqualCheck: compare register to slot |


## GSLT Module System and MeTTaTron Imports

### MeTTaIL Module Resolution

MeTTaIL's module system (`ModuleProcessor.scala`) resolves imports recursively,
building a map from canonical file paths to parsed module ASTs:

```scala
// MeTTaIL: ModuleProcessor.scala
def resolveModules(entryPath: String): Map[String, Module] = {
  // 1. Parse module file
  // 2. Cache in loaded map
  // 3. Recursively resolve ImportModuleAs and ImportFromModule
}
```

Theory references within modules are resolved via dotted paths:

```scala
def resolveDottedPath(
  resolvedModules: Map[String, Module],
  currentModulePath: String,
  dottedPath: DottedPath
): Either[String, (String, TheoryDecl)]
```

### MeTTaTron Module System Comparison

MeTTaTron's module system (`src/backend/eval/evaluation.rs`) handles `import!` and
`include!` via `eval_import_generic` and `eval_include_generic`. The parallel is:

| MeTTaIL | MeTTaTron |
|---------|-----------|
| `import "file.module" as alias` | `!(import! &self module-name)` |
| `import Name from "file.module"` | `!(include! &self "file.metta")` |
| `ModuleProcessor.resolveModules()` | `METTA_MODULE_PATH` search + cycle detection |
| `TheoryEnv.build(module)` | `env.mark_module_loading()` / `env.unmark_module_loading()` |
| Theory declarations in module scope | Rules added to `RuleIndex` + `MORK/PathMap` |

When MeTTaIL generates a complete presentation via theory interpretation, that
presentation could be compiled directly into MeTTaTron's rule format:
each `Def` (grammar rule) becomes a pattern schema, each `RewriteDecl` becomes a
`RuleEntry` with WAM code compiled from its LHS.


## Formal Correspondence Summary

The following table summarizes the formal correspondences between MeTTaIL
components and MeTTaTron WAM components:

| MeTTaIL Component | WAM Equivalent | Relationship |
|-------------------|----------------|--------------|
| `BasePres` | `Vec<RuleEntry>` + `RuleIndex` | Presentation defines the rule set |
| `ASTSExp(label, children)` | `GetAtom` + `GetArity` + `GetArg` | Constructor decomposition |
| `ASTVar(path)` | `BindSlot(reg, slot)` | Variable binding |
| `ASTSubst(body, repl, var)` | `apply_bindings(template, frame)` | Substitution execution |
| Repeated variable detection | `EqualCheck(reg, slot)` | Shared variable constraint |
| `catOfIdentInAST` | Slot type assignment | Positional type inference |
| `consistentCategory` | Compile-time EqualCheck placement | Type consistency verification |
| Theory disjunction (`\/`) | Rule group union + TryMeElse chain | Nondeterministic alternatives |
| Theory conjunction (`/\`) | Bloom filter pre-check | Category-based filtering |
| Theory subtraction (`\`) | Rule exclusion / remove-atom | Rule set pruning |
| Replacements with permutation | WAM code rewriting (compile-time) | Label + argument remapping |
| `RewriteContext` hypothesis | Trampoline recursive evaluation | Conditional reduction |
| `EquationImpl` (LHS == RHS) | Equational axioms (not WAM) | Normalization rules |
| Kosaraju SCC condensation | Choice point backtracking | Cycle detection + exploration |
| Topological sort | TryMeElse chain ordering | Dependency-respecting traversal |
| Wavefront parallel schedule | WorkPool concurrent dispatch | Parallelism extraction |
| `CommEvent` labels | WAM instruction categories | Structured transition types |
| `SpatialBehavior` | Rule indexing criterion | Dispatch refinement |
| `RateValue` (probability) | Match success (Boolean) | Quantitative vs qualitative |
| Gillespie SSA selection | All-solutions accumulation | Probabilistic vs exhaustive |
| Quantum amplitude interference | Choice point superposition | Phase-aware vs phase-free |
| Hypercube type-lifting | `expected_type` propagation | Type-driven optimization |
| BNFC monomorphization | Atom interning + arity checks | Concrete pattern encoding |
| `DesugarBinds` | Register allocation for arrows | Lambda elimination |

## References

- Warren, D. H. D. (1983). *An Abstract Prolog Instruction Set*. Technical Note 309, SRI International.
- Ait-Kaci, H. (1991). *Warren's Abstract Machine: A Tutorial Reconstruction*. MIT Press.
- Arkor, N. and McDermott, D. (2024). *The formal theory of relative monads*. Journal of Pure and Applied Algebra.
- Phillips, A. and Cardelli, L. (2007). *Efficient, correct simulation of biological processes in the stochastic pi-machine*. CMSB.
- Gillespie, D. T. (1977). *Exact stochastic simulation of coupled chemical reactions*. Journal of Physical Chemistry 81(25).
- Tarjan, R. E. (1972). *Depth-first search and linear graph algorithms*. SIAM Journal on Computing 1(2).
