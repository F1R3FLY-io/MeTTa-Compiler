# JIT and Bytecode Debug Guide

This guide covers the debugging features available in MeTTaTron's JIT compiler and bytecode VM, including how to use them effectively for troubleshooting and performance analysis.

## Overview

MeTTaTron provides several debugging mechanisms across its execution tiers:

| Feature | Status | Description |
|---------|--------|-------------|
| Execution Tracing | Fully Supported | Log execution flow via `RUST_LOG` |
| Trace Opcode | Fully Supported | Log values during bytecode/JIT execution |
| Breakpoint Opcode | Stub Only | Logs breakpoint hit but doesn't pause |
| Debug Print/Stack | Fully Supported | JIT runtime helpers for value/stack inspection |
| Statistics/Profiling | Fully Supported | Comprehensive execution metrics |
| Bailout Tracking | Fully Supported | 16 detailed bailout reason codes |
| Bytecode Disassembly | Fully Supported | Human-readable bytecode output |

## Quick Start

### Enable Execution Tracing

The fastest way to debug execution is to enable tracing via the `RUST_LOG` environment variable:

```bash
# Trace all JIT and VM execution
RUST_LOG=trace ./target/release/mettatron input.metta

# Trace only JIT runtime events
RUST_LOG=mettatron::jit::runtime=trace ./target/release/mettatron input.metta

# Debug level for less verbose output
RUST_LOG=mettatron::jit=debug ./target/release/mettatron input.metta
```

### Programmatic Tracing

When using the Rust API, enable tracing via configuration:

```rust
use mettatron::backend::bytecode::jit::HybridConfig;

// Enable execution tracing
let config = HybridConfig::default().with_trace();
let executor = HybridExecutor::new(config)?;
```

---

## Debug Features Reference

### 1. Execution Tracing

Execution tracing provides visibility into the hybrid executor's operation, including tier selection, JIT compilation, and bailouts.

#### Configuration

```rust
// Enable tracing in HybridConfig
let config = HybridConfig {
    trace: true,
    ..Default::default()
};

// Or use the builder method
let config = HybridConfig::default().with_trace();
```

#### What Gets Traced

When tracing is enabled, the following events are logged:

- **Chunk execution**: Which chunk is being executed and at which tier
- **JIT entry/exit**: Native code pointer and result counts
- **Bailouts**: Bailout IP and reason code
- **Backtracking**: Choice point exploration and result collection
- **Tier selection**: Why a particular execution tier was chosen

#### Example Output

```
TRACE mettatron::jit::hybrid::execute: Executing chunk chunk_id=42 tier=JitStage1
TRACE mettatron::jit::hybrid::execute: JIT entry native_ptr=0x7f123456
TRACE mettatron::jit::hybrid::execute: JIT completed results_count=3
DEBUG mettatron::jit::hybrid::backtrack: Backtracking iteration=2 choice_points=1 results=5
```

### 2. Trace Opcode (0xFE)

The `Trace` opcode emits trace events with the current value on top of the stack.

#### How It Works

- **VM Behavior**: Peeks at the stack top and logs via `trace!` macro
- **JIT Behavior**: Calls `jit_runtime_trace()` which converts the NaN-boxed value and logs it

#### Trace Targets

- **VM**: `mettatron::vm::trace`
- **JIT**: `mettatron::jit::runtime::trace`

#### Example Usage

To see trace output, enable the trace log level:

```bash
RUST_LOG=mettatron::vm::trace=trace,mettatron::jit::runtime::trace=trace ./target/release/mettatron input.metta
```

#### Example Output

```
TRACE mettatron::jit::runtime::trace: ip=42 msg_idx=0 metta_val=Long(123) Trace
TRACE mettatron::vm::trace: value=Long(456)
```

### 3. Breakpoint Opcode (0xFD)

The `Breakpoint` opcode is intended for debugger integration but currently operates as a logging stub only.

#### Current Limitations

**The breakpoint opcode does NOT pause execution.** Both the VM and JIT implementations simply log the breakpoint hit and continue:

- **VM**: Logs at debug level, continues execution
- **JIT**: Calls `jit_runtime_breakpoint()`, which always returns 0 (continue)

#### Current Behavior

```rust
// VM implementation (vm.rs)
fn op_breakpoint(&mut self) -> VmResult<()> {
    // TODO: Implement breakpoint handling
    debug!(target: "mettatron::vm::breakpoint", ip = self.ip);
    Ok(())
}

// JIT runtime (runtime.rs)
pub unsafe extern "C" fn jit_runtime_breakpoint(...) -> i64 {
    debug!(target: "mettatron::jit::runtime::breakpoint", bp_id, ip, "Breakpoint hit");
    0 // Always continue
}
```

#### Using for Logging

While breakpoints don't pause, they can still be useful for logging specific points in execution:

```bash
RUST_LOG=mettatron::vm::breakpoint=debug,mettatron::jit::runtime::breakpoint=debug ./target/release/mettatron input.metta
```

### 4. Debug Print/Stack Functions

The JIT runtime provides two debug functions for inspecting values and stack state.

#### `jit_runtime_debug_print(val: u64)`

Prints a single NaN-boxed value via the tracing infrastructure.

```rust
// Signature
pub extern "C" fn jit_runtime_debug_print(val: u64)

// Output target
// mettatron::jit::runtime::debug
```

#### `jit_runtime_debug_stack(ctx: *const JitContext)`

Dumps the entire JIT stack contents with indices.

```rust
// Signature
pub unsafe extern "C" fn jit_runtime_debug_stack(ctx: *const JitContext)

// Output target
// mettatron::jit::runtime::debug
```

#### Example Output

```
TRACE mettatron::jit::runtime::debug: jv=Long(42) "Debug print"
TRACE mettatron::jit::runtime::debug: sp=3 "Stack dump"
TRACE mettatron::jit::runtime::debug: index=0 val=0x7ff8000000000001 "  Stack slot"
TRACE mettatron::jit::runtime::debug: index=1 val=0x7ff800000000002a "  Stack slot"
TRACE mettatron::jit::runtime::debug: index=2 val=0x7ff8000000000000 "  Stack slot"
```

#### Enabling Debug Function Output

```bash
RUST_LOG=mettatron::jit::runtime::debug=trace ./target/release/mettatron input.metta
```

---

## Statistics and Profiling

MeTTaTron provides comprehensive statistics for understanding execution behavior across tiers.

### HybridStats

The `HybridStats` struct tracks high-level execution metrics:

```rust
pub struct HybridStats {
    /// Total number of run() calls
    pub total_runs: u64,
    /// Number of runs that used JIT
    pub jit_runs: u64,
    /// Number of runs that used bytecode VM
    pub vm_runs: u64,
    /// Number of JIT bailouts
    pub jit_bailouts: u64,
    /// Number of successful JIT compilations
    pub jit_compilations: u64,
    /// Number of failed JIT compilations
    pub jit_compile_failures: u64,
    /// Tiered compilation statistics
    pub tiered_stats: TieredStats,
}
```

#### Accessing Statistics

```rust
let executor = HybridExecutor::new(config)?;
// ... run some code ...

let stats = executor.stats();
println!("JIT hit rate: {:.1}%", stats.jit_hit_rate());
println!("Bailout rate: {:.1}%", stats.bailout_rate());
```

#### Key Metrics

| Method | Description |
|--------|-------------|
| `jit_hit_rate()` | Percentage of runs using JIT code |
| `bailout_rate()` | Percentage of JIT runs that bailed out |

### TieredStats

Detailed breakdown of execution across tiers:

```rust
pub struct TieredStats {
    /// Number of interpreter executions (Tier 0)
    pub interpreter_runs: u64,
    /// Number of bytecode VM executions (Tier 1)
    pub bytecode_runs: u64,
    /// Number of JIT Stage 1 executions (Tier 2 - arithmetic/boolean)
    pub jit_stage1_runs: u64,
    /// Number of JIT Stage 2 executions (Tier 3 - full native)
    pub jit_stage2_runs: u64,
    /// Number of successful JIT compilations
    pub jit_compilations: u64,
    /// Number of failed JIT compilations
    pub jit_failures: u64,
    /// Total bytes of JIT compiled code
    pub total_jit_bytes: u64,
    /// Number of cache hits
    pub cache_hits: u64,
    /// Number of cache misses
    pub cache_misses: u64,
}
```

#### Key Metrics

| Method | Description |
|--------|-------------|
| `total_executions()` | Sum of all tier executions |
| `jit_percentage()` | Percentage of executions using JIT |
| `cache_hit_rate()` | JIT cache effectiveness |

### JitStats

Global JIT profiling statistics:

```rust
pub struct JitStats {
    /// Total number of chunks profiled
    pub total_chunks: usize,
    /// Number of chunks that reached Hot state
    pub hot_chunks: usize,
    /// Number of successfully JIT compiled chunks
    pub jitted_chunks: usize,
    /// Number of chunks where JIT compilation failed
    pub failed_chunks: usize,
    /// Total bytes of generated native code
    pub total_code_bytes: usize,
    /// Total execution count across all chunks
    pub total_executions: u64,
}
```

#### JIT Coverage

```rust
impl JitStats {
    /// Calculate JIT coverage percentage
    pub fn coverage_percent(&self) -> f64 {
        if self.total_chunks == 0 {
            0.0
        } else {
            (self.jitted_chunks as f64 / self.total_chunks as f64) * 100.0
        }
    }
}
```

---

## Bailout Tracking

When JIT code encounters an operation it cannot handle natively, it "bails out" to the bytecode VM. Understanding bailouts is crucial for performance optimization.

### JitBailoutReason Enum

There are 17 bailout reason codes (0-16):

| Code | Name | Description |
|------|------|-------------|
| 0 | `None` | No bailout occurred (normal completion) |
| 1 | `TypeError` | Type mismatch during operation |
| 2 | `DivisionByZero` | Attempted division by zero |
| 3 | `StackOverflow` | JIT stack capacity exceeded |
| 4 | `StackUnderflow` | Pop from empty stack |
| 5 | `InvalidOpcode` | Unknown or unsupported opcode encountered |
| 6 | `UnsupportedOperation` | Operation requires bytecode VM |
| 7 | `IntegerOverflow` | Numeric overflow detected |
| 8 | `NonDeterminism` | Fork/Choice opcodes require VM |
| 9 | `Call` | Rule dispatch needs VM |
| 10 | `TailCall` | Tail call needs VM for rule dispatch |
| 11 | `Fork` | Fork needs VM for choice point management |
| 12 | `Yield` | Yield needs VM for backtracking |
| 13 | `Collect` | Collect needs VM to gather results |
| 14 | `InvalidBinding` | Variable not found in any scope |
| 15 | `BindingFrameOverflow` | Too many nested binding frames |
| 16 | `HigherOrderOp` | Map/filter/fold need VM |

### Common Bailout Causes and Solutions

#### NonDeterminism (8), Fork (11), Yield (12), Collect (13)

**Cause**: Your code uses non-deterministic features (multiple results, backtracking).

**Solution**: This is expected behavior. The JIT handles the deterministic portions and the VM handles backtracking. Consider restructuring code to minimize non-deterministic sections if performance is critical.

#### Call (9), TailCall (10)

**Cause**: User-defined rule invocations require the VM for rule dispatch.

**Solution**: The JIT compiles the body of rules but rule selection is handled by the VM. This is a fundamental limitation of the current JIT architecture.

#### HigherOrderOp (16)

**Cause**: Higher-order operations like `map`, `filter`, `fold` require the VM.

**Solution**: These operations involve arbitrary MeTTa expressions as callbacks. Consider using explicit recursion if JIT performance is critical.

#### TypeError (1)

**Cause**: Runtime type mismatch (e.g., arithmetic on non-numbers).

**Solution**: Add type guards or ensure type consistency in your code.

### Monitoring Bailouts

```bash
# Log bailout events
RUST_LOG=mettatron::jit::hybrid::execute=trace ./target/release/mettatron input.metta
```

Example output:
```
WARN mettatron::jit::hybrid::execute: JIT bailout bailout_ip=42 reason=NonDeterminism
```

---

## Bytecode Disassembly

The `BytecodeChunk::disassemble()` method produces human-readable bytecode output for debugging.

### Using Disassembly

```rust
use mettatron::backend::bytecode::{BytecodeCompiler, BytecodeChunk};

// Compile MeTTa to bytecode
let chunk = compiler.compile(expression)?;

// Get human-readable disassembly
let disasm = chunk.disassemble();
println!("{}", disasm);
```

### Disassembly Output Format

```
=== main ===
locals: 2, upvalues: 0, arity: 0

0000    load_const       0    ; 42
0003    load_const       1    ; 10
0006    add
0007    store_local      0
0009    load_local       0
0011    return
```

### Understanding the Output

- **Header**: Chunk name, local variable count, upvalue count, arity
- **Offset**: Byte offset of the instruction
- **Mnemonic**: Human-readable instruction name
- **Operands**: Immediate values (if any)
- **Comments**: Constant values, jump targets, etc.

### Per-Instruction Disassembly

For fine-grained inspection:

```rust
let (instruction_str, next_offset) = chunk.disassemble_instruction(offset);
println!("{}", instruction_str);
```

---

## Debugging Workflows

### Workflow 1: Tracing Program Execution

When you need to understand what your MeTTa program is doing:

```bash
# Step 1: Enable comprehensive tracing
RUST_LOG=mettatron=trace ./target/release/mettatron input.metta 2>&1 | head -500

# Step 2: Filter to specific targets if too verbose
RUST_LOG=mettatron::jit::hybrid::execute=trace ./target/release/mettatron input.metta

# Step 3: Look for patterns in the output
RUST_LOG=mettatron=trace ./target/release/mettatron input.metta 2>&1 | grep "Bailout\|Error"
```

### Workflow 2: Investigating JIT Performance

When JIT isn't providing expected speedup:

```rust
// Step 1: Collect statistics
let stats = executor.stats();

// Step 2: Check JIT hit rate
if stats.jit_hit_rate() < 50.0 {
    println!("Low JIT coverage - check for bailouts");
}

// Step 3: Check bailout rate
if stats.bailout_rate() > 20.0 {
    println!("High bailout rate - examine bailout reasons");
}

// Step 4: Enable bailout tracing
// RUST_LOG=mettatron::jit::hybrid::execute=debug
```

### Workflow 3: Debugging Bailouts

When you're seeing too many bailouts:

```bash
# Step 1: Enable bailout logging
RUST_LOG=mettatron::jit::hybrid::execute=trace ./target/release/mettatron input.metta 2>&1 | grep "bailout"

# Step 2: Identify bailout IPs
# Look for patterns like: bailout_ip=42 reason=NonDeterminism

# Step 3: Disassemble bytecode at those offsets
# Use BytecodeChunk::disassemble_instruction(ip) to see what triggered bailout
```

### Workflow 4: Profiling Hot Paths

When optimizing performance-critical code:

```rust
// Step 1: Run with profiling enabled
let executor = HybridExecutor::new(HybridConfig::default())?;
for _ in 0..1000 {
    executor.run(&chunk)?;
}

// Step 2: Collect tiered stats
let tiered = &executor.stats().tiered_stats;
println!("JIT Stage 1 runs: {}", tiered.jit_stage1_runs);
println!("JIT Stage 2 runs: {}", tiered.jit_stage2_runs);
println!("Bytecode runs: {}", tiered.bytecode_runs);
println!("JIT percentage: {:.1}%", tiered.jit_percentage());

// Step 3: Check JIT state transitions
// Chunks progress: Cold -> Warming (10 runs) -> Hot (100 runs) -> Jitted
```

---

## Log Target Reference

### Complete Target List

| Target | Level | Description |
|--------|-------|-------------|
| `mettatron::jit::hybrid::execute` | trace/debug | Hybrid executor events |
| `mettatron::jit::hybrid::compile` | debug | JIT compilation events |
| `mettatron::jit::hybrid::backtrack` | trace | Backtracking and choice points |
| `mettatron::jit::runtime::debug` | trace | Debug print/stack dumps |
| `mettatron::jit::runtime::trace` | trace | Trace opcode events |
| `mettatron::jit::runtime::breakpoint` | debug | Breakpoint hits |
| `mettatron::jit::compiler::ir` | trace | Generated Cranelift IR |
| `mettatron::vm::trace` | trace | VM trace opcode events |
| `mettatron::vm::breakpoint` | debug | VM breakpoint events |
| `mettatron::backend::eval` | trace/debug | Main evaluator events |

### Recommended Filter Combinations

```bash
# General debugging (moderate verbosity)
RUST_LOG=mettatron=debug

# JIT performance analysis
RUST_LOG=mettatron::jit::hybrid=debug

# Execution flow tracing (very verbose)
RUST_LOG=mettatron::jit::hybrid::execute=trace,mettatron::jit::hybrid::backtrack=trace

# Bailout investigation
RUST_LOG=mettatron::jit::hybrid::execute=trace

# JIT compilation issues
RUST_LOG=mettatron::jit::hybrid::compile=debug,mettatron::jit::compiler=debug

# Value inspection
RUST_LOG=mettatron::jit::runtime::debug=trace,mettatron::jit::runtime::trace=trace
```

---

## Troubleshooting

### Common Issues

#### Q: No output from trace logging

**A**: Ensure you're using the correct target and level:
```bash
# Correct
RUST_LOG=mettatron::jit::hybrid::execute=trace

# Wrong (missing level)
RUST_LOG=mettatron::jit::hybrid::execute
```

Also ensure your logging subscriber is initialized (e.g., `tracing_subscriber::fmt::init()`).

#### Q: JIT hit rate is 0%

**A**: Check if JIT is enabled:
```rust
let config = HybridConfig::default();
assert!(config.jit_enabled);  // Should be true
```

Also check if code is reaching the hot threshold (100 executions).

#### Q: All runs are bailouts

**A**: Your code likely uses features that require the VM:
- Non-determinism (multiple results)
- User-defined rules (Call/TailCall)
- Higher-order operations

This is expected behavior, not an error.

#### Q: Can't pause at breakpoints

**A**: The breakpoint opcode is currently a stub only. It logs breakpoint hits but does not pause execution. Full debugger support is planned for a future release.

---

## Future: Full Breakpoint Support

The breakpoint opcode currently only logs; it doesn't pause execution. Full support would require:

### Debugger State Management
- `debugger_enabled` flag in VM/JitContext
- Breakpoint table with enable/disable per breakpoint
- Conditional breakpoint expression evaluation

### Pause/Resume Mechanism
- Cooperative yield from JIT to debugger REPL
- State preservation across pause
- Continue/step/next commands

### CLI/REPL Integration
- `--debug` flag to enable debugger mode
- REPL commands: `break`, `continue`, `step`, `next`, `print`, `backtrace`
- Stack frame inspection

This functionality is tracked for future implementation.
