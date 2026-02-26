# Architecture Documentation

Technical architecture documents for MeTTaTron's internal systems.

## Tiered Compilation

**[TIERED_COMPILER_IMPLEMENTATION_GUIDE.md](TIERED_COMPILER_IMPLEMENTATION_GUIDE.md)** - Tiered Compiler Architecture
- Tree-walker, bytecode VM, and JIT compilation tiers
- Tier promotion thresholds and execution counting
- Compilation pipeline and code generation

**[HYBRID_P2_PRIORITY_SCHEDULER.md](HYBRID_P2_PRIORITY_SCHEDULER.md)** - Hybrid P2 Priority Scheduler
- Priority-based scheduling for tiered compilation
- Work stealing and load balancing
- Compilation queue management

## Evaluation Tracing

**[eval-trace/](eval-trace/README.md)** - Evaluation Trace System Architecture
- System overview and crate layout
- Trace event data model (TraceEvent, TraceValue, TraceEventKind)
- Binary file format specification (`.mtrace`)
- TraceCollector buffering and batched I/O
- Instrumentation points across all three evaluation tiers
- Threading model (EvalContext vs thread-local sink)
