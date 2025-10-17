# MeTTaTron Examples

This directory contains example code demonstrating various features of the MeTTaTron evaluator.

## MeTTa Language Examples

### Basic Examples
- **simple.metta** - Basic MeTTa language features (arithmetic, rules, evaluation)
- **advanced.metta** - Advanced patterns including pattern matching and control flow
- **mvp_test.metta** - MVP feature demonstrations
- **type_system_demo.metta** - Type system examples
- **pathmap_demo.metta** - PathMap operations

### Rust Backend Examples

#### Direct API Usage
- **backend_usage.rs** - Direct usage of the backend API
  ```bash
  cargo run --example backend_usage
  ```

- **backend_interactive.rs** - Interactive REPL implementation
  ```bash
  cargo run --example backend_interactive
  ```

- **mvp_complete.rs** - Complete MVP demonstration
  ```bash
  cargo run --example mvp_complete
  ```

#### Optimization Examples
- **test_zipper_optimization.rs** - MORK zipper optimization demonstration
  ```bash
  cargo run --example test_zipper_optimization
  ```

- **threading_config.rs** - Threading configuration examples
  ```bash
  cargo run --example threading_config
  ```

## Rholang Integration Examples

- **metta_rholang_example.rho** - Using MeTTa from Rholang via the registry pattern
- **robot_planning.rho** - Robot planning domain implemented in Rholang/MeTTa

## Running Examples

### MeTTa Files
```bash
# Build the CLI first
cargo build --release

# Run a MeTTa file
./target/release/mettatron examples/simple.metta

# Or use cargo run
cargo run --release -- examples/simple.metta
```

### Rust Examples
```bash
# List all examples
cargo run --example

# Run a specific example
cargo run --example backend_usage
```

## See Also

- Main documentation: [`../docs/`](../docs/)
- Integration guides: [`../integration/`](../integration/)
- API reference: [`../docs/reference/`](../docs/reference/)
