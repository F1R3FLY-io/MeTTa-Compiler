# MeTTaTron I/O & Network Operations Reference

A complete inventory of every way the MeTTaTron evaluator touches the
**filesystem**, the **network**, and **external processes** — plus the
module-import machinery layered on top of that I/O.

> **Verified:** 2026-06-01 against branch `feature/petta-semantics`, by reading
> source directly. All entries carry `file:line` citations so they can be
> re-checked.

**Status legend:** ✅ real & callable · ⚠️ placeholder (parsed, returns a stub
value) · 🚫 absent.

---

## Headline

MeTTaTron has real **file I/O** (six `file-*` grounded ops), out-of-core
**persistence** to `/dev/shm`, and a full **module-import** system. It links
**no in-process network stack** (no HTTP client, no sockets) — but it *does*
reach the network and execute arbitrary subprocesses through exactly one path:
`git-import!` / `git-module!`, which shell out to `git clone` and `sh -c`.

---

## 1. File I/O — MeTTa-surface grounded operations

Implemented in `src/backend/grounded/fileio.rs`; type signatures in
`src/backend/builtin_signatures.rs:354-384`; dispatched in
`src/backend/grounded/registry.rs:154+`. This is an HE-bisim of Hyperon's
`lib/src/metta/runner/builtin_mods/fileio.rs`. **All six are real and callable.**

| MeTTa op                                           | Rust (`fileio.rs`)        | Underlying call                  | Notes                                                               |
|----------------------------------------------------|---------------------------|----------------------------------|---------------------------------------------------------------------|
| `(file-open! "path" "opts")` → `(FileHandle <id>)` | `FileOpenOp` @77          | `OpenOptions::open`              | opts: `r` read · `w` write · `c` create · `a` append · `t` truncate |
| `(file-read-to-string! $fh)` → `String`            | `FileReadToStringOp` @173 | `File::read_to_string`           | cursor → EOF                                                        |
| `(file-read-exact! $fh $n)` → `String`             | `FileReadExactOp` @419    | `File::read` into `vec![0u8; n]` | UTF-8 decoded                                                       |
| `(file-write! $fh "content")` → `()`               | `FileWriteOp` @244        | `File::write_all`                |                                                                     |
| `(file-seek! $fh $off)` → `()`                     | `FileSeekOp` @332         | `File::seek(SeekFrom::Start)`    | absolute offset                                                     |
| `(file-get-size! $fh)` → `Long`                    | `FileGetSizeOp` @530      | `metadata().len()`               |                                                                     |

- **Handle registry:** global `static FILE_HANDLES: OnceLock<Mutex<HashMap<u64, File>>>`
  (`fileio.rs:40`); ids from an `AtomicU64`. A handle is the symbolic value
  `(FileHandle <id>)`.
- **No `file-close!`** — handles are never freed mid-session; the OS closes the
  file descriptors at process exit (documented at `fileio.rs:22-25`).
- **No path sandboxing** — arbitrary paths are accepted; only OS permissions apply.

## 2. Out-of-core persistence — ACT (ArenaCompactTree)

Surface dispatch at `src/backend/eval/step/sexpr.rs:3189-3376`; implementation in
`src/backend/environment/act_persistence.rs` and `act_tiered.rs`. Files live under
MORK's `ACT_PATH` = **`/dev/shm/`** (tmpfs; documented `act_tiered.rs:85`), as
`<name>.act`, `<name>.wide.act` (wide facts, arity ≥ 64), and `<name>.sm` (symbol
mapping, cross-run faithful). Reads are memory-mapped.

| MeTTa op                      | `sexpr.rs` | Effect                                                                                 |
|-------------------------------|------------|----------------------------------------------------------------------------------------|
| `(save-space! "name")`        | @3189      | dump literal-fact trie → `/dev/shm/<name>.{act,wide.act,sm}`                           |
| `(load-space! "name")`        | @3221      | mmap snapshot, decode + re-add facts (multiplicity-faithful)                           |
| `(query-act "name" pat tmpl)` | @3256      | query `/dev/shm/<name>.act` (trie-pruned join, or leaf-scan fallback); empty if absent |
| `(attach-act-base! "name")`   | @3296      | attach `.act` as immutable LSM base; the in-memory trie becomes an overlay             |
| `(detach-act-base!)`          | @3330      | stop tiering (base-only facts are dropped, not materialized)                           |
| `(compact-space! "name")`     | @3351      | fold `overlay + (base − tombstones)` → fresh ACT, atomic-rename over the live files    |

## 3. CLI / source-loading I/O

In `src/main.rs`:

- **Input:** positional `<INPUT>` file via `fs::read_to_string`, or `-` for stdin
  (`io::stdin().read_to_string`).
- **Output:** `-o <FILE>` → `fs::File::create` + `write_all`; otherwise stdout.
- **Modes:** `--sexpr` (parse-only dump), `--repl`, `--trace <FILE>`.
- **Module root:** the input file's parent directory seeds `current_module_path`
  for relative `include` / `import!`.

## 4. Module import system

Handlers in `src/backend/eval/modules.rs`; git in `src/backend/eval/git_import.rs`;
resolution in `src/backend/modules/path.rs`. Dispatch in `sexpr.rs`
(`git-import!` / `git-module!` @4279). All are parsed as ordinary S-expressions —
no grammar extension is required.

| Op                            | Status | Implementation                               | Behavior                                                                                                                                                                                                   |
|-------------------------------|--------|----------------------------------------------|------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `(include "path")`            | ✅     | `eval_include_generic` `modules.rs:40`       | `fs::read_to_string` → parse → eval **inline** in the current env; `!`-forms force-eval (enables transitive includes); hash-based cycle detection                                                          |
| `(import! [&self] path)`      | ✅     | `eval_import_generic` `modules.rs:198`       | load into the module registry; **silent-fail** on a missing file (PeTTa-canonical); native modules `stdlib`/`corelib`/`random`/`fileio`/`json`/`das`/`catalog`/`math`/`concurrency` are compiled-in no-ops |
| `(register-module! "path")`   | ✅     | → same handler as `import!`                  | HE-canonical alias                                                                                                                                                                                         |
| `(git-import! "url" ["cmd"])` | ✅     | `eval_git_import_generic` `git_import.rs:54` | **subprocess + network** — see §5                                                                                                                                                                          |
| `(git-module! "url")`         | ✅     | → same handler                               | HE-canonical alias (`sexpr.rs:4279`)                                                                                                                                                                       |
| `(bind! tok expr)`            | ✅     | `sexpr.rs` `StartBind` → `register_token`    | evaluate `expr`, store the token in the env                                                                                                                                                                |
| `(get-modules)`               | ✅     | `eval_get_modules_generic` `modules.rs:441`  | walks `module_registry` via `ModuleRegistry::iter()` → a name tuple                                                                                                                                        |
| `(mod-space! name)`           | ⚠️      | `modules.rs:382`                             | returns symbolic `(space <name>)`; no real resolution                                                                                                                                                      |
| `(print-mods!)`               | ⚠️      | `modules.rs:419`                             | returns `(modules none)`                                                                                                                                                                                   |

**Resolution & search paths** (`modules/path.rs`):

- Env vars: `METTA_MODULE_PATH`, `METTA_LIBRARY_PATH`, `METTA_GIT_CACHE`.
- `LIBRARY_PATHS` registry (`RwLock<Vec<PathBuf>>`) seeded with the bundled
  `stdlib`, `CARGO_MANIFEST_DIR/stdlib`, and auto-discovered `./repos/*/` from
  prior clones.
- `(library X)` → `<base>/X.metta`; `(library X Y)` → sibling-repo
  `<base>/../X/Y.metta`.
- Importer-relative ancestor walk, up to 16 levels.
- Package manifests: `_pkg-info.metta` (HE format) → fallback `metta.toml`.

## 5. Network I/O & subprocess execution

**MeTTaTron links no in-process network stack.** `Cargo.toml` has no `reqwest` /
`hyper` / `tonic` (client) / `ureq` / `curl`; `tokio` is `features = ["rt", "sync"]`
(no `net`); there is no `std::net` / `tokio::net` / socket usage anywhere. URIs
(`` `…` `` backtick literals) are **opaque atoms** — never dereferenced.

**But it does reach the network — transitively — and it does spawn subprocesses,**
exclusively through `git-import!` / `git-module!` (`src/backend/eval/git_import.rs`):

- `Command::new("git").args(["clone", "--depth", "1", &url]).arg(&local)` —
  **`git_import.rs:136`**. This is the **only network egress**: `git` pulls from
  an arbitrary remote URL.
- `Command::new("sh").arg("-c").arg(cmd).current_dir(&local)` —
  **`git_import.rs:168`**. An optional **build step runs an arbitrary shell
  command** in the freshly cloned directory.
- Cache dir: `METTA_GIT_CACHE` env var, else `./repos/` (CWD-relative). Idempotent
  (the clone is skipped if the directory already exists). All failures return a
  graceful `MettaValue` error — it never panics.

These are the **only two `Command::new` sites** in production code (test and
conformance runners aside). Correct framing: **no direct sockets; network access
plus arbitrary-code execution via the `git` / `sh` subprocess path of
`git-import!`.**

## 6. Rholang integration — in-process only

`src/rholang_integration.rs` and `src/pathmap_par_integration.rs` are **pure Rust
linking** (direct function calls: `compile_safe`, `run_state[_async]`,
`metta_value_to_par` / `par_to_metta_value`). There is no RPC / gRPC / IPC /
socket; the protobuf types in the optional `models` crate are for in-memory data
shapes only. **MeTTaTron does not talk to an f1r3node over the network.**

## 7. Other filesystem access (not MeTTa-callable)

- **Trace:** `--trace <FILE>` → `BufWriter<File>` binary event log
  (`src/backend/trace/collector.rs`), feature-gated.
- **GC allocator:** `libc::mmap` / `munmap` of anonymous pages
  (`src/backend/models/gc_allocator.rs`) — memory management, not I/O.
- **Diagnostics (Linux):** `/proc/self/statm` (RSS, `work_pool.rs:1534`),
  `/proc/self/task` (thread list, `diagnostics.rs`).
- **Signals:** SIGTERM / SIGILL / SIGUSR1 handlers (`diagnostics.rs`) — no I/O is
  performed from the handlers themselves.

## 8. Security / sandboxing posture

> These are observations about the current behavior, not recommendations.

- File ops (`file-*`), `include`, and `import!` accept **arbitrary,
  unvalidated, un-canonicalized paths** — full read/write within process
  permissions; symlink traversal is possible.
- `git-import!` clones **arbitrary URLs** and its optional build step executes
  **arbitrary shell** — a hostile repo plus build command is remote code
  execution by design (this mirrors PeTTa's `git-import!`).
- ACT ops are implicitly confined to `/dev/shm/` (tmpfs), but the `<name>`
  component is caller-controlled.
- There is **no opt-out / capability flag** to disable file, git, or subprocess
  I/O.

---

## Appendix — quick verification

Re-checked this session by reading source: `git_import.rs` (full), `fileio.rs`
(head + dispatch), `modules.rs:382-441`, and greps confirming dispatch wiring
(`registry.rs:154`, `builtin_signatures.rs:354`, `sexpr.rs:3189` / `4279`) plus
`ACT_PATH = /dev/shm/` (`act_tiered.rs:85`).

To exercise end-to-end (after `cargo build --release`):

```metta
; File I/O
!(let $fh (file-open! "/tmp/t.txt" "wc") (file-write! $fh "hi"))
!(let $fh (file-open! "/tmp/t.txt" "r")  (file-read-to-string! $fh))

; Module include
!(include "examples/module_lib.metta")

; Git import (network + subprocess); override the cache with METTA_GIT_CACHE
!(git-import! "https://github.com/<small-repo>.git")
```

Subprocess failures (e.g. `git` not on `PATH`) surface as `MettaValue` errors
rather than panics.
