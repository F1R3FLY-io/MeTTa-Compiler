# AGENTS.md — MeTTa-Compiler (MeTTaTron) project-level mandates

Canonical, durable statement of the **project-level mandates** that govern work in this repository.
Read this before non-trivial work. pgmcp indexes this file as a mandate source (`mandate_context`); it
complements — does not replace — these detail sources:

- **`.claude/CLAUDE.md`** — build/run/test commands, evaluation pipeline, code organization, MeTTa surface.
- **`~/.claude/CLAUDE.md`** — the user's global directives (apply to every project; summarized below where they
  govern this one).
- **`~/.claude/plans/help-me-complete-the-shimmying-mochi.md`** — the active GC-migration plan (Phases A–F).
- **`docs/cesk-gc/`** — the GC design docs (per-phase). **auto-memory `MEMORY.md`** — standing directives +
  the live workstream ledger.

Enforcement is advisory via MCP; hard gates live in verification scripts (`scripts/a5_greenwall.sh`,
`scripts/*_asan.sh`), the per-rung gate below, and review.

## Process mandates (standing — the user has reinforced these repeatedly)

- **Commit at EVERY stable point.** Commit verified/green work immediately, on the feature branch, BEFORE
  dispatching the next agent or starting the next increment. Never accumulate uncommitted stable work; never
  bury a stable point under later WIP.
- **Cap ALL heavy ops under `systemd-run`; NEVER background an uncapped build.** Machine has **125 GiB** RAM.
  Every cargo build / ASAN / nextest / stress / massif runs under
  `systemd-run --user --scope -p MemoryMax=… -p MemorySwapMax=0 -p CPUQuota=…`, FOREGROUND. ASAN
  (`-Zbuild-std`) ≤ 32 G, low `-j`. Check `free -h` + other sessions' builds first. (An uncapped backgrounded
  ASAN+stress run once OOM-crashed the machine.)
- **NEVER busy-wait poll** for background tasks. Foreground `sleep` is blocked → end the turn and wait for the
  `<task-notification>`; for external state use `ScheduleWakeup`. No `until [cond]; do :; done` loops.
- **Use Explore/Plan agents whenever needed or useful; never wing complex decisions.** When a task is more
  complicated than predicted, design it with a Plan agent rather than taking shortcuts.
- **Never make destructive changes without explicit approval** — no `git reset`, `git stash`, revert, or
  clobbering files without a recoverable path. Prefer `git show` to inspect history. Never revert or
  second-guess explicit instructions / change a plan's implementation without surfacing it for approval first.
- **Never disable code by deleting it** — comment it out with a reason. Delete only what the user asks to delete.
- **Do not modify sibling projects** (`PathMap`, `MORK`, `hyperon-experimental`, `rholang-rs`) unless explicitly
  requested.
- **Be data-driven when optimizing** — benchmark/profile (perf, `time`/`hyperfine`, VTune, massif) before
  optimizing; follow the scientific method; pin CPU affinity + max frequency when benchmarking; `tee` long runs.
- **Preallocate** when the size is known (a best practice, not premature optimization).
- **Prefer `.expect("…")` over `unwrap()`** (useful panic messages). Prefer pattern matching to predicate
  conditioning.
- **Document designs thoroughly** so they can be reconstructed from scratch; track progress + results as a
  scientific ledger (in `docs/`, intuitively organized — never clutter the repo root).
- **Prefer `mcp__pgmcp__*` over built-in Grep/Glob** for conceptual/cross-project/graph/health queries; when
  dispatching subagents, instruct them to prefer pgmcp and explain why (project/graph/semantic awareness).
- **Formal verification**: use Rocq/Z3/TLA+ and validate models with the tools; Rocq/Coq proofs carry no
  assumptions/axioms/admits (well-known lemmas may be cited); cap Coq builds under `systemd-run`.

## GC-migration mandates (active workstream — a GENUINE CESK collector)

The value heap is being migrated to a genuine CESK-machine garbage collector + integrated allocator
(`--features index-gc`), NOT an amendment of the old slab GC nor a collector merely influenced by CESK
vocabulary. These are load-bearing:

- **Genuine architecture, not retrofit.** Designs must embody the named architecture's guaranteed-by-construction
  properties. Red-team critical designs to convergence (until an adversarial round reverses nothing). Verify the
  implementation *realizes* the property against source before trusting any framing.
- **Roots are STRUCTURAL: `σ|_Reachable(⟨C,E,K⟩) ∪ reach(E₀)`**, read from the reified machine registers. **NEVER
  re-introduce a root registry or a discovery side-channel** (ROOT_REGISTRY / RootProvider / frame_chain /
  publish-by-value — all deleted in Phase A). Per-worker roots = that worker's machine registers, read
  structurally (the worker self-collects; the collector never reads another thread's thread-locals).
- **Soundness is mechanically discharged, not argued.** A collector that narrows the marked set turns a wrong
  "dead" classification into a UAF — discharge it with differential tests + coupling asserts + ASAN + TLA+, not
  prose.
- **Byte-identical until ship.** The index GC must keep conformance **483/0** (+ M11-pt 221, M11-he 40)
  byte-identical to slab, and 20-run-deterministic, until the final ship phase (F3) flips the default.
- **The slab (default) build stays green on every commit** (`cargo nextest run --release`). Every index-only arm
  is `gc_mode_is_index()`-guarded or `cfg(feature = "index-gc")`-walled so no gate leaks into slab.
- **Never flip a concurrency gate without the full gate**: ASAN (0 UAF, forcing cycles) + ≥20-run determinism +
  mmverify "Correct proof" + the relevant TLA+ invariant + loom/TSan for concurrent rungs.
- **Benchmark FANOUT=0 with the collector ON** for the single-threaded-collector win (the parallel collector's
  win is measured at FANOUT>0 only after Phase D's gate flip). Welch: index+JIT vs slab+JIT, ≥30 replicate,
  CPU-pinned/freq-locked.
- **Reuse existing machinery before building new; prefer the minimal mechanism set.** Commit at every green
  sub-increment.

## Per-rung gate (the standing verification bar)

`0 ASAN-UAF` + byte-identical conformance (483/221/40, cycles>0) + ≥20-run determinism + mmverify "Correct
proof" + HE-bisim 40/40 + PLN budgets + the relevant TLA+ invariant; concurrent rungs add loom + TSan; CESK
rungs keep the machine-equivalence oracle green. Harness: `scripts/a5_greenwall.sh <label> [--with-oracle]`.
