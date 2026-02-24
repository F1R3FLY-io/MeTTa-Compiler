# Debugging Parallel Test Hangs in Rust

A practical guide for diagnosing thread deadlocks and lock contention when
`cargo test --lib` hangs indefinitely under parallel execution but passes with
`--test-threads=1`.

---

## Table of Contents

1. [Symptoms](#1-symptoms)
2. [Tools Required](#2-tools-required)
3. [Step-by-Step Diagnosis](#3-step-by-step-diagnosis)
4. [Common Deadlock Patterns in Rust](#4-common-deadlock-patterns-in-rust)
5. [BCC / bpftrace Tools (Advanced)](#5-bcc--bpftrace-tools-advanced)
6. [Prevention Strategies](#6-prevention-strategies)

---

## 1. Symptoms

- `cargo test --lib` hangs indefinitely when run with the default parallel
  thread count, but completes successfully with `--test-threads=1`.
- Individual tests show **"has been running for over 60 seconds"** warnings
  from the Rust test harness.
- The process is not consuming CPU (it is blocked, not spinning), which you can
  confirm with `top` or `htop` showing near-zero CPU for the test binary.
- Ctrl-C may not produce useful output because the test harness's panic handler
  does not fire for sleeping threads.

These symptoms strongly indicate a **deadlock** or **severe lock contention**
among parallel test threads competing for shared global state.

---

## 2. Tools Required

| Tool | Package | Purpose |
|------|---------|---------|
| `eu-stack` | `elfutils` | Captures user-space stack traces of all threads in a running process |
| `rustfilt` | `cargo install rustfilt` | Demangles Rust symbol names into readable form |
| `sudo` | (system) | Required for `eu-stack -p PID` on processes you did not directly launch |
| `pgrep` | `procps-ng` | Finds process IDs by name/pattern |
| `timeout` | `coreutils` | Prevents `cargo test` from running forever during diagnosis |

### Installation

```bash
# Arch Linux
sudo pacman -S elfutils procps-ng

# Debian/Ubuntu
sudo apt install elfutils procps

# Fedora/RHEL
sudo dnf install elfutils procps-ng

# Install rustfilt
cargo install rustfilt
```

---

## 3. Step-by-Step Diagnosis

### Step 1: Build the test binary without running it

Building first ensures the binary exists and avoids conflating compilation time
with hang time.

```bash
cargo test --lib --no-run
```

The output will show the path to the compiled test binary, for example:

```
Executable unittests src/lib.rs (target/debug/deps/mettatron-d881eae2dc783ac5)
```

Note this path -- you will use it to identify the correct process later.

### Step 2: Start the hung test in the background

Use `timeout` to ensure the process eventually terminates even if diagnosis
takes a while:

```bash
timeout 120 cargo test --lib 2>&1 &
```

This gives you 120 seconds to capture diagnostics before the process is killed.

### Step 3: Wait for the hang, then find the test binary PID

Wait approximately 30 seconds for tests to reach the deadlock point, then
locate the actual test binary process (not the `cargo` wrapper):

```bash
# Show all matching processes with full command lines
pgrep -fa 'mettatron-'

# Capture the PID of the test binary
TEST_PID=$(pgrep -f 'mettatron-[a-f0-9]' | head -1)
echo "Test binary PID: $TEST_PID"
```

**Important**: `cargo test` spawns the test binary as a child process. You need
the PID of the test binary itself (e.g., `mettatron-d881eae2dc783ac5`), not the
PID of `cargo` or `rustc`.

### Step 4: Capture stack traces of all threads

```bash
sudo eu-stack -p "$TEST_PID" -m | rustfilt > /tmp/eu_stack_output.txt
```

Flags:
- `-p PID` -- attach to the given process
- `-m` -- show module/library names alongside addresses

The `rustfilt` pipe demangles Rust symbols so you see
`mettatron::backend::models::gc_allocator::SlabAllocator::alloc` instead of
`_ZN9mettatron7backend6models12gc_allocator14SlabAllocator5allocE`.

### Step 5: Analyze the stack traces

#### Count total threads

```bash
grep -c '^Thread ' /tmp/eu_stack_output.txt
```

A typical parallel test run has N test threads plus a few runtime threads. If
the thread count is much higher than expected, a thread pool may be spawning
unboundedly.

#### Find threads blocked on locks

```bash
grep -B1 -A20 \
  'lock_exclusive\|lock_shared\|futex_wait\|park\|condvar\|Mutex.*lock\|RwLock.*write\|RwLock.*read' \
  /tmp/eu_stack_output.txt | head -200
```

This shows the full stack context around any thread that is waiting on a
synchronization primitive.

#### Identify the most contended locks

```bash
grep -oE '[A-Za-z_:]+::(write|read|lock)\b' /tmp/eu_stack_output.txt \
  | sort | uniq -c | sort -rn
```

Example output:

```
     12 ROOT_REGISTRY::write
      8 GLOBAL_GC_THREAD::lock
      3 SlabAllocator::lock
```

This tells you which lock is the bottleneck. In the example above,
`ROOT_REGISTRY::write` is contended by 12 threads -- that is the primary
suspect.

#### Categorize threads by their first meaningful Rust frame

This is the single most diagnostic command. Frame `#7` is typically the first
meaningful Rust frame after the futex/parking_lot/condvar boilerplate:

```bash
grep '#7 ' /tmp/eu_stack_output.txt | rustfilt | sort | uniq -c | sort -rn | head -20
```

Example output from a real deadlock:

```
     36 #7  EvalGuard::enter           -- test threads blocked on GC_IN_PROGRESS
     17 #7  WorkerPark::wait_if_parked  -- idle work pool workers (normal)
      3 #7  execute_session_release     -- GC workers waiting for quiescence
      1 #7  Mutex<Vec<MettaValue>>::lock -- GC worker blocked on data mutex
```

This instantly shows: 36 threads blocked on `EvalGuard`, 3 GC workers in
session release, and 1 thread blocked on a data mutex. The deadlock is between
the GC worker (holding `GC_IN_PROGRESS`, blocked on the data mutex) and the
test threads (blocked on `GC_IN_PROGRESS`).

#### Examine the call stacks of blocked threads

```bash
grep -B30 'futex_wait\|lock_exclusive' /tmp/eu_stack_output.txt \
  | grep '#[0-9]' | head -50
```

This extracts the numbered stack frames leading up to the blocking call. Look
for patterns like:

- **Thread A** holds lock X, waiting for lock Y
- **Thread B** holds lock Y, waiting for lock X
- This is a classic **ABBA deadlock**

Or:

- **Thread A** holds `RwLock::write()` and is doing expensive work
- **Threads B through N** are all blocked on `RwLock::write()` or
  `RwLock::read()` for the same lock
- This is **writer starvation / convoy**

### Step 6: Alternative -- kernel stacks via /proc (no sudo needed)

If `/proc/$PID/task/*/stack` is readable (some kernels restrict this), you can
inspect kernel-level wait states without `sudo`:

```bash
for tid in /proc/$TEST_PID/task/*/; do
    tid_num=$(basename "$tid")
    stack=$(cat "$tid/stack" 2>/dev/null)
    if echo "$stack" | grep -q futex_wait; then
        echo "=== TID $tid_num (BLOCKED) ==="
        echo "$stack"
        echo
    fi
done
```

This shows which threads are in `futex_wait` (the kernel-level syscall
underlying `Mutex`, `RwLock`, and `Condvar`). It does not show Rust function
names (only kernel frames), so it is less detailed than `eu-stack` but can
confirm whether threads are genuinely blocked vs. spinning.

---

## 4. Common Deadlock Patterns in Rust

### 4.1 RwLock Writer Starvation

**Symptom**: Many threads blocked on `RwLock::write()`. One thread holds the
write lock for a long time because it performs expensive work inside the
critical section.

**Root Cause**: A function acquires `RwLock::write()` and then iterates over a
collection, calling expensive methods on each element -- all while holding the
lock.

**Example from this project**: `collect_all_roots()` held `ROOT_REGISTRY.write()`
while calling `collect_roots()` on every registered environment. Since
`collect_roots()` itself acquires other locks, this created a lock convoy where
all test threads queued behind the writer.

**Fix**: Minimize lock hold time. Split long critical sections into two phases:

1. **Snapshot under lock**: Acquire the lock, clone/upgrade the data you need
   into a local collection, release the lock.
2. **Process without lock**: Iterate over the local collection and do the
   expensive work lock-free.

```rust
// BEFORE (bad): holds write lock during expensive iteration
fn collect_all_roots() -> Vec<Root> {
    let registry = ROOT_REGISTRY.write();  // blocks all other writers AND readers
    registry.providers.iter()
        .filter_map(|weak| weak.upgrade())
        .flat_map(|provider| provider.collect_roots())  // expensive!
        .collect()
}

// AFTER (good): minimal lock hold time
fn collect_all_roots() -> Vec<Root> {
    // Phase 1: snapshot under lock
    let providers: Vec<Arc<dyn RootProvider>> = {
        let registry = ROOT_REGISTRY.write();
        registry.providers.iter()
            .filter_map(|weak| weak.upgrade())
            .collect()
    };  // lock released here

    // Phase 2: expensive work without any lock held
    providers.iter()
        .flat_map(|provider| provider.collect_roots())
        .collect()
}
```

### 4.2 Global Singleton Contention

**Symptom**: All test threads compete for the same global lock (e.g., a
`static` `OnceLock`, `Mutex`, or `RwLock` used by a singleton).

**Root Cause**: The production code uses a single global instance (appropriate
for the application), but tests run in parallel within the same process and all
contend on that singleton.

**Fix**: Parameterize constructors so tests can create local, independent
instances instead of sharing the global singleton.

```rust
// BEFORE: all tests use the global pool
static WORK_POOL: OnceLock<WorkPool> = OnceLock::new();

fn get_pool() -> &'static WorkPool {
    WORK_POOL.get_or_init(|| WorkPool::new())
}

// AFTER: production code uses the global, tests create local instances
impl WorkPool {
    /// Create a pool with explicit thread counts (for testing)
    pub fn with_threads(min: usize, max: usize) -> Self {
        WorkPool { min, max, /* ... */ }
    }
}

#[test]
fn test_pool_behavior() {
    let pool = WorkPool::with_threads(1, 2);  // local, no contention
    // ...
}
```

### 4.3 Condvar Wait Without Wakeup

**Symptom**: A thread calls `condvar.wait()` and never wakes up. The thread
that should call `condvar.notify_one()` (or `notify_all()`) is itself blocked
on a lock held by the waiting thread.

**Root Cause**: Circular dependency between the condvar's associated mutex and
another lock in the system.

**Fix**:

1. Add **timeouts** to all condvar waits so threads do not sleep forever:
   ```rust
   let (lock, timeout_result) = condvar.wait_timeout(guard, Duration::from_millis(100))?;
   if timeout_result.timed_out() {
       // Handle timeout: retry, log warning, or bail out
   }
   ```
2. Check for **circular dependencies** in lock ordering. Draw a graph of which
   locks are held when other locks are acquired. Any cycle is a potential
   deadlock.

### 4.4 GC_IN_PROGRESS Blocking Evaluators

**Symptom**: `EvalGuard::enter()` spins or blocks because `GC_IN_PROGRESS` is
set to `true` and never cleared. The GC thread that should clear it has either
panicked or is blocked on another lock.

**Root Cause**: The GC completion path does not clear `GC_IN_PROGRESS` on all
exit paths (especially panic/error paths).

**Fix**: Use an **RAII guard** to ensure the flag is always cleared:

```rust
struct GcInProgressGuard;

impl GcInProgressGuard {
    fn enter() -> Self {
        GC_IN_PROGRESS.store(true, Ordering::Release);
        GcInProgressGuard
    }
}

impl Drop for GcInProgressGuard {
    fn drop(&mut self) {
        GC_IN_PROGRESS.store(false, Ordering::Release);
        // Wake any threads waiting for GC to complete
        GC_CONDVAR.notify_all();
    }
}

fn perform_gc() {
    let _guard = GcInProgressGuard::enter();
    // ... GC work ...
    // Flag is cleared automatically when _guard is dropped,
    // even if this function panics.
}
```

### 4.5 GC Root Collection Blocking on Data Mutexes

**Symptom**: GC thread holds a global progress flag (`GC_IN_PROGRESS`) and
calls `collect_roots()` on each registered root provider. One provider's
`collect_roots()` blocks on a data `Mutex` held by a thread that is itself
blocked on `GC_IN_PROGRESS`.

**Root Cause**: `collect_roots()` uses `mutex.lock()` (blocking) to read data
protected by a `Mutex`. If another thread is holding that mutex (e.g.,
mid-compilation pushing values into a `Vec<MettaValue>`), and that other thread
subsequently needs `EvalGuard::enter()` (which blocks on `GC_IN_PROGRESS`), a
deadlock forms:

```
GC thread:  holds GC_IN_PROGRESS --> calls collect_roots() --> blocks on Mutex
Test thread: holds Mutex (pushing values) --> calls EvalGuard::enter() --> blocks on GC_IN_PROGRESS
```

**Fix**: Use `try_lock()` in `collect_roots()` instead of `lock()`. If the
mutex is currently held, skip that provider. The skipped values are transient
(actively being written) and will be collected on the next GC cycle:

```rust
impl RootProvider for MyStateGcRoots {
    fn collect_roots(&self, roots: &mut Vec<Value>) {
        if let Some(source) = self.source.try_lock() {
            roots.extend(source.iter().copied());
        }
        // Skipping a locked provider is safe: the values being actively
        // written are transient and will be collected next cycle.
    }
}
```

### 4.6 Thread Pool Permanent Capacity Loss

**Symptom**: After some tests run, the thread pool has fewer live worker
threads than expected. Subsequent tests that need pool capacity hang waiting
for a worker that will never become available.

**Root Cause**: A worker thread panicked (e.g., due to a test assertion
failure propagating into the pool), and the pool did not replace it.

**Fix**: Wrap the worker's task execution in `std::panic::catch_unwind()`:

```rust
fn worker_loop(receiver: Receiver<Task>) {
    while let Ok(task) = receiver.recv() {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            task.run();
        }));
        if let Err(panic_info) = result {
            eprintln!("Worker thread caught panic: {:?}", panic_info);
            // Worker continues to the next task instead of dying
        }
    }
}
```

---

## 5. BCC / bpftrace Tools (Advanced)

These tools provide deeper visibility into lock contention and off-CPU time but
require root access and BPF support in the kernel.

### 5.1 offcputime (BCC Python version)

`offcputime` traces time spent sleeping/blocked, grouped by stack trace. This
is the single best tool for understanding **why** threads are not making
progress.

```bash
sudo /usr/share/bcc/tools/offcputime -p $TEST_PID -u 15 > /tmp/offcpu.stacks
```

Flags:
- `-p PID` -- trace only this process
- `-u` -- user-space stacks only
- `15` -- trace for 15 seconds

**Important**: The `libbpf-tools` version of `offcputime` (typically at
`/usr/bin/offcputime`) may crash with "stack smashing detected" on some
systems. Use the **Python BCC version** at `/usr/share/bcc/tools/offcputime`
instead. You can verify which version you have:

```bash
file /usr/share/bcc/tools/offcputime   # Should be a Python script
file /usr/bin/offcputime               # May be a compiled binary (libbpf-tools)
```

The output is a series of folded stacks with microsecond counts:

```
thread_park;RwLock::write;collect_all_roots;gc_thread_main  850000
futex_wait;Mutex::lock;SlabAllocator::alloc;eval_step       120000
```

Pipe through `rustfilt` for readable names, or generate a flamegraph:

```bash
cat /tmp/offcpu.stacks | rustfilt | flamegraph.pl > /tmp/offcpu.svg
```

### 5.2 Using perf

```bash
sudo perf record -g -p $TEST_PID -- sleep 15
sudo perf report
```

**Caveat**: `perf record` captures **on-CPU** samples. For deadlocks where
threads are sleeping (off-CPU), `perf` will show very few samples for the
blocked threads. Use `eu-stack` or `offcputime` for off-CPU analysis. `perf` is
most useful for diagnosing **spinlocks** or **busy-wait loops** where the
thread is on-CPU but not making progress.

### 5.3 bpftrace one-liners

```bash
# Trace all futex calls for the test process (shows which threads are waiting)
sudo bpftrace -e 'tracepoint:syscalls:sys_enter_futex /pid == '$TEST_PID'/ {
    printf("tid=%d op=%d\n", tid, args->op);
}'

# Histogram of time spent in futex_wait (in microseconds)
sudo bpftrace -e '
    tracepoint:syscalls:sys_enter_futex /pid == '$TEST_PID' && args->op == 0/ { @start[tid] = nsecs; }
    tracepoint:syscalls:sys_exit_futex  /pid == '$TEST_PID' && @start[tid]/ {
        @us = hist((nsecs - @start[tid]) / 1000);
        delete(@start[tid]);
    }
'
```

---

## 6. Prevention Strategies

### 6.1 Minimize lock hold times

Never perform I/O, allocation, or expensive computation while holding a lock.
Snapshot the data you need under the lock, release it, then process:

```rust
// Acquire lock, snapshot, release, process
let snapshot = {
    let guard = lock.read();
    guard.clone()  // or .iter().map(...).collect()
};
// Process snapshot without holding any lock
process(snapshot);
```

### 6.2 Use local instances in tests

Parameterize constructors so tests can create isolated instances. Global
singletons are appropriate for production but cause contention in parallel
tests:

```rust
impl MyService {
    pub fn new() -> Self { /* uses global config */ }
    pub fn with_config(config: Config) -> Self { /* parameterized */ }
}
```

### 6.3 Consistent lock ordering

Document the lock ordering for all locks in the system and enforce it in code
reviews. If lock A must always be acquired before lock B, any code path that
acquires B then A is a deadlock risk.

Example documentation:

```
Lock ordering (acquire in this order):
  1. ROOT_REGISTRY (RwLock)
  2. GC_STATE (Mutex)
  3. SLAB_PAGES (Mutex)
  4. Per-environment locks

Never acquire a lower-numbered lock while holding a higher-numbered one.
```

### 6.4 Prefer atomics over locks

For simple flags, counters, and state machines, use `AtomicBool`, `AtomicU32`,
`AtomicU64`, or `AtomicUsize` instead of `Mutex<bool>`, `Mutex<u32>`, etc.
Atomics are lock-free and cannot deadlock:

```rust
// Instead of Mutex<bool>
static GC_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

// Instead of Mutex<u32>
static ACTIVE_EVALUATORS: AtomicU32 = AtomicU32::new(0);
```

### 6.5 Use `catch_unwind` in worker pools

Wrap task execution in `std::panic::catch_unwind()` to prevent panicking tasks
from killing worker threads. A dead worker thread permanently reduces pool
capacity, which can cause subsequent tasks to hang waiting for a worker that
will never become available.

### 6.6 Add `--test-threads=1` to CI as a fallback

If parallel tests are unstable due to global state contention that cannot be
easily eliminated, run sequentially as a reliability backstop:

```bash
# In CI pipeline
cargo test --lib -- --test-threads=1
```

This is a **workaround**, not a fix. The underlying contention should still be
addressed, but sequential testing prevents CI from hanging indefinitely while
the fix is developed.

### 6.7 Use RAII guards for all state transitions

Any global flag or state that must be set/cleared as a pair should use an RAII
guard to guarantee cleanup on all exit paths (including panics):

```rust
struct StateGuard<'a> {
    flag: &'a AtomicBool,
}

impl<'a> StateGuard<'a> {
    fn set(flag: &'a AtomicBool) -> Self {
        flag.store(true, Ordering::Release);
        StateGuard { flag }
    }
}

impl<'a> Drop for StateGuard<'a> {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
    }
}
```

### 6.8 Add timeouts to all blocking waits

Never call `condvar.wait()` or `receiver.recv()` without a timeout in
production code. Unbounded waits turn any missed notification into a permanent
hang:

```rust
// Bad: hangs forever if notify is missed
let guard = condvar.wait(guard)?;

// Good: retries periodically, can detect and recover from missed notifications
loop {
    let (guard, timeout) = condvar.wait_timeout(guard, Duration::from_secs(1))?;
    if timeout.timed_out() {
        if should_give_up() { break; }
        continue;
    }
    // Notification received
    break;
}
```

---

## Appendix: Quick Reference Command Sheet

```bash
# === Setup ===
cargo test --lib --no-run                           # Build test binary
timeout 120 cargo test --lib 2>&1 &                 # Start tests with timeout

# === Find PID ===
pgrep -fa 'mettatron-'                              # List matching processes
TEST_PID=$(pgrep -f 'mettatron-[a-f0-9]' | head -1) # Capture PID

# === Capture stacks ===
sudo eu-stack -p "$TEST_PID" -m | rustfilt > /tmp/eu_stack_output.txt

# === Analyze ===
grep -c '^Thread ' /tmp/eu_stack_output.txt          # Thread count
grep -B1 -A20 'futex_wait\|lock_exclusive' /tmp/eu_stack_output.txt | head -200
grep -oE '[A-Za-z_:]+::(write|read|lock)\b' /tmp/eu_stack_output.txt | sort | uniq -c | sort -rn

# === Advanced ===
sudo /usr/share/bcc/tools/offcputime -p $TEST_PID -u 15 > /tmp/offcpu.stacks
```
