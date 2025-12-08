# MeTTaTron Logging Strategy

This document provides a comprehensive guide to the logging strategy implemented in MeTTaTron, including filtering patterns, target hierarchies, and practical examples for debugging and monitoring.

## Table of Contents

1. [Overview](#overview)
2. [Log Levels](#log-levels)
3. [Target Hierarchy](#target-hierarchy)
4. [Filtering Strategies](#filtering-strategies)
5. [Common Use Cases](#common-use-cases)
6. [Examples](#examples)
7. [Performance Considerations](#performance-considerations)
8. [Best Practices](#best-practices)

## Overview

MeTTaTron uses structured logging via the `tracing` crate to provide visibility into evaluation processes, error handling, and system behavior. The logging system is designed to be:

- **Hierarchical**: Targets follow module structure for easy filtering
- **Granular**: Specific function-level targets for precise debugging
- **Performance-aware**: Minimal overhead in release builds
- **Context-rich**: Structured data with relevant evaluation context

## Log Levels

### TRACE
- **Purpose**: Function entry/exit and execution flow
- **When to use**: High-frequency operations, detailed execution paths
- **Target audience**: Deep debugging, performance analysis

```rust
trace!(target: "mettatron::eval::control_flow", ?items, ?args);
```

### DEBUG
- **Purpose**: Decision points, user errors, validation failures
- **When to use**: Conditional logic, error handling, user input validation
- **Target audience**: Development debugging, troubleshooting user issues

```rust
debug!(target: "mettatron::eval::error_handling", is_error = is_err, "error check result");
```

### WARN
- **Purpose**: Handled system issues, recoverable problems
- **When to use**: System degradation, fallback mechanisms, resource issues
- **Target audience**: Operations monitoring, system health

```rust
warn!(target: "mettatron::backend::eval", "pattern matching limit exceeded, using fallback");
```

### ERROR
- **Purpose**: Unhandled failures, system errors
- **When to use**: Unrecoverable errors, system corruption, critical failures
- **Target audience**: Alert systems, error tracking

```rust
error!(target: "mettatron::backend", error = %e, "critical evaluation failure");
```

## Target Hierarchy

### Primary Targets

#### `mettatron::backend::eval`
Main evaluation engine targets for high-level filtering:

- `mettatron::backend::eval::process_collected_sexpr`
- `mettatron::backend::eval::eval_trampoline`

#### `mettatron::eval` 
Evaluation function families for modular filtering:

- `mettatron::eval::control_flow` - Conditional logic (`eval_if`, `eval_case`, `eval_switch`)
- `mettatron::eval::error_handling` - Error operations (`eval_error`, `eval_catch`, `eval_if_error`)
- `mettatron::eval::list_ops` - List operations (`eval_map_atom`, `eval_filter_atom`, `eval_foldl_atom`)
- `mettatron::eval::bindings` - Variable binding (`eval_let`)
- `mettatron::eval::quoting` - Quote/unquote operations
- `mettatron::eval::types` - Type checking and assertions

#### `mettatron::integration`
External system integration:

- `mettatron::integration::rholang` - Rholang Par integration
- `mettatron::integration::pathmap` - PathMap conversions

### Function-Specific Targets

For precise debugging of individual functions:

```rust
// Direct function targeting
trace!(target: "eval_if", ?condition, ?true_branch, ?false_branch);
trace!(target: "eval_catch", ?expr, ?default);
trace!(target: "metta_to_par", ?value);
```

## Filtering Strategies

### Development Debugging

#### Full Evaluation Tracing
```bash
env RUST_LOG=mettatron::eval=trace cargo run --example backend_usage
```
Shows all evaluation function calls and decisions.

#### Control Flow Only
```bash
env RUST_LOG=mettatron::eval::control_flow=debug cargo run --example backend_usage
```
Focus on conditional logic and branching decisions.

#### Error Handling Focus
```bash
env RUST_LOG=mettatron::eval::error_handling=trace cargo run --example backend_usage
```
Detailed error construction, checking, and recovery.

### Production Monitoring

#### Error and Warning Only
```bash
env RUST_LOG=warn cargo run --example backend_usage
```
System health monitoring without debug noise.

#### Backend Operations
```bash
env RUST_LOG=mettatron::backend=info cargo run --example backend_usage
```
High-level backend operations and system events.

### Performance Analysis

#### MORK Integration
```bash
env RUST_LOG=mettatron::backend::eval::mork_integration=trace cargo run --example backend_usage
```
Deep dive into pattern matching and kernel operations.

#### Function-Specific Profiling
```bash
env RUST_LOG=eval_map_atom=trace,eval_filter_atom=trace cargo run --example backend_usage
```
Profile specific high-frequency functions.

## Common Use Cases

### 1. User Program Not Working

**Symptom**: User's MeTTa program produces unexpected results

**Strategy**:
```bash
# Start with control flow to understand execution path
env RUST_LOG=mettatron::eval::control_flow=debug cargo run --example backend_usage

# If still unclear, add error handling
env RUST_LOG=mettatron::eval::control_flow=debug,mettatron::eval::error_handling=debug cargo run --example backend_usage

# For complex programs, full evaluation trace
env RUST_LOG=mettatron::eval=trace cargo run --example backend_usage
```

### 2. Performance Issues

**Symptom**: Slow evaluation, excessive memory usage

**Strategy**:
```bash
# Check pattern matching performance
env RUST_LOG=mettatron::backend::eval::mork_integration=debug cargo run --example backend_usage

# Monitor list operations for efficiency
env RUST_LOG=mettatron::eval::list_ops=debug cargo run --example backend_usage

# Full backend trace for bottleneck identification
env RUST_LOG=mettatron::backend=trace cargo run --example backend_usage
```

### 3. Integration Problems

**Symptom**: Issues with Rholang/PathMap integration

**Strategy**:
```bash
# Focus on integration layers
env RUST_LOG=mettatron::integration=trace cargo run --example backend_usage

# Specific Rholang debugging
env RUST_LOG=mettatron::integration::rholang=trace cargo run --example backend_usage
```

### 4. System Stability

**Symptom**: Crashes, memory leaks, resource exhaustion

**Strategy**:
```bash
# Error and warning monitoring
env RUST_LOG=warn cargo run --example backend_usage

# Include backend warnings for system health
env RUST_LOG=mettatron::backend=warn cargo run --example backend_usage
```

## Examples

### Basic Debugging Session

```bash
# User reports: "My if-statement always goes to else branch"
$ env RUST_LOG=mettatron::eval::control_flow=debug cargo run --example backend_usage

DEBUG mettatron::eval::control_flow: condition_result=Bool(false) branch="false_branch" eval_if
DEBUG mettatron::eval::control_flow: evaluating false branch eval_if
```

This immediately shows the condition is evaluating to `false`, directing investigation to the condition logic.

### Performance Investigation

```bash
# User reports: "List processing is very slow"
$ env RUST_LOG=mettatron::eval::list_ops=debug cargo run --example backend_usage

DEBUG mettatron::eval::list_ops: input_length=10000 predicate="complex_check" eval_filter_atom
DEBUG mettatron::eval::list_ops: filtered_count=8500 eval_filter_atom
DEBUG mettatron::eval::list_ops: input_length=8500 mapper="expensive_transform" eval_map_atom
WARN  mettatron::eval::list_ops: large list operation may impact performance eval_map_atom
```

Shows large list operations and performance warnings.

### Error Recovery Debugging

```bash
# User asks: "Why doesn't my catch block work?"
$ env RUST_LOG=mettatron::eval::error_handling=trace cargo run --example backend_usage

TRACE mettatron::eval::error_handling: items=[SExpr([Atom("catch"), Atom("risky-op"), Atom("default")])] eval_catch
DEBUG mettatron::eval::error_handling: error_count=1 non_error_count=0 "all results were errors, evaluating default" eval_catch
TRACE mettatron::eval::error_handling: items=[Atom("default")] eval_catch
DEBUG mettatron::eval::error_handling: "returning default value" eval_catch
```

Shows the catch mechanism working correctly - error detected and default evaluated.

### Integration Tracing

```bash
# Debugging Rholang integration
$ env RUST_LOG=mettatron::integration::rholang=trace cargo run --example backend_usage

TRACE mettatron::integration::rholang: metta_value=Atom("hello") metta_value_to_par
DEBUG mettatron::integration::rholang: par_type="string_expr" "converted MeTTa atom to Par"
TRACE mettatron::integration::rholang: par=Par{exprs=[Expr{g_string="hello"}]} par_to_metta_value
DEBUG mettatron::integration::rholang: metta_value=String("hello") "converted Par to MeTTa"
```

Shows bidirectional conversion between MeTTa values and Rholang Par structures.

### Multi-Target Filtering

```bash
# Complex debugging: control flow + error handling + integration
$ env RUST_LOG='mettatron::eval::control_flow=debug,mettatron::eval::error_handling=debug,mettatron::integration=trace' cargo run --example backend_usage

TRACE mettatron::integration::rholang: starting_conversion metta_value_to_par
DEBUG mettatron::eval::control_flow: condition_result=Error("type_mismatch") branch="error_handling" eval_if
DEBUG mettatron::eval::error_handling: error_count=1 "routing to error handler" eval_catch
TRACE mettatron::integration::rholang: error_converted par_conversion_failed
```

Multi-layer view showing integration → control flow → error handling pipeline.


This logging strategy provides comprehensive visibility into MeTTaTron's operation while maintaining performance and allowing precise filtering for specific debugging scenarios.