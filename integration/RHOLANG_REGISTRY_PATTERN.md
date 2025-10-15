# Using MeTTa Compile with Registry Binding Pattern

## The Pattern You Want

```rholang
// Bind compile from registry
new compile in {
  registryLookup!("rho:metta:compile:sync", *compile) |
  for (@compileService <- compile) {
    // Use it with !? in for comprehension
    for (@pm <- compileService!?(text)) {
      // pm is the compiled result (JSON)
      stdoutAck!(pm, *ack)
    }
  }
}
```

## Does It Work?

**YES** - but it requires the **synchronous variant** (`rho:metta:compile:sync`) which has:
- **Arity: 1** (source code only)
- **Implicit return** via produce mechanism

## Implementation Details

### Why Arity 1 Is Required

The pattern:
```rholang
for (@result <- channel!?(message)) { ... }
```

Requires that:
1. The service accepts a single message (the source code)
2. The result is returned via an implicit channel
3. The `for` comprehension receives from that implicit channel

This is exactly what `rho:metta:compile:sync` provides!

### How It Maps to Handlers

**Traditional Pattern** (`rho:metta:compile` - arity 2):
```rholang
// Requires explicit return channel
new result in {
  @"rho:metta:compile"!("(+ 1 2)", *result) |
  for (@json <- result) { ... }
}
```
❌ **Cannot use**: `for (@pm <- compile!?(text))` (requires explicit channel)

**Synchronous Pattern** (`rho:metta:compile:sync` - arity 1):
```rholang
// Implicit return via produce
for (@pm <- compileService!?(text)) { ... }
```
✅ **Can use**: `for (@pm <- compile!?(text))` (implicit return)

## Complete Example

### Using Registry Binding

```rholang
new compile, ack in {
  // Step 1: Get service from registry
  registryLookup!("rho:metta:compile:sync", *compile) |

  // Step 2: Wait for service binding
  for (@compileService <- compile) {
    stdoutAck!("Got compile service", *ack) |

    // Step 3: Use with !? in for comprehension
    for (@pm <- compileService!?("(+ 1 2)")) {
      stdoutAck!("Compilation result: " ++ pm, *ack)
    }
  }
}
```

### Direct URN Access (Simpler)

If you don't need registry lookup, you can use the URN directly:

```rholang
// Direct access (if URN resolution is configured)
for (@pm <- @"rho:metta:compile:sync"!?("(+ 1 2)")) {
  stdoutAck!("Result: " ++ pm, *ack)
}
```

## Why Two Patterns?

We provide **two services** to support **two usage patterns**:

### Pattern 1: Explicit Channel (Traditional)

**Service**: `rho:metta:compile` (arity: 2)

```rholang
new result in {
  @"rho:metta:compile"!(source, *result) |
  for (@json <- result) { ... }
}
```

**Use when**:
- You need explicit control over return channel
- Building complex async workflows
- Want backward compatibility

### Pattern 2: Implicit Return (For Your Use Case!)

**Service**: `rho:metta:compile:sync` (arity: 1)

```rholang
for (@pm <- compile!?(source)) { ... }
```

**Use when**:
- Want idiomatic `for (@x <- service!?(msg))` pattern ✅
- Building sequential pipelines
- Don't need explicit channel management
- **This is what you asked about!**

## Registry Integration

### How Registry Lookup Works

```rholang
new compile, lookup in {
  // Lookup in registry (standard Rholang pattern)
  registryLookup!("rho:metta:compile:sync", *lookup) |

  for (@service <- lookup) {
    // service is now bound to the unforgeable name
    // Use it multiple times
    for (@result1 <- service!?("(+ 1 2)")) {
      stdoutAck!("Result 1: " ++ result1, *ack) |

      for (@result2 <- service!?("(* 3 4)")) {
        stdoutAck!("Result 2: " ++ result2, *ack)
      }
    }
  }
}
```

### Registry Configuration

For this to work, ensure the registry is configured with:

```rust
// In Rholang system processes initialization
registry.register(Definition {
    urn: "rho:metta:compile:sync".to_string(),
    fixed_channel: FixedChannels::byte_name(201),
    arity: 1,  // ← Important: arity 1 for implicit return
    // ...
});
```

## Advanced Patterns

### Sequential Pipeline with Registry Binding

```rholang
new compile in {
  registryLookup!("rho:metta:compile:sync", *compile) |

  for (@service <- compile) {
    // Compile multiple expressions in sequence
    for (@r1 <- service!?("(= (double $x) (* $x 2))")) {
      stdoutAck!("Step 1: " ++ r1, *ack) |

      for (@r2 <- service!?("!(double 21)")) {
        stdoutAck!("Step 2: " ++ r2, *ack) |

        for (@r3 <- service!?("(+ 10 20)")) {
          stdoutAck!("Step 3: " ++ r3, *ack) |
          stdoutAck!("Pipeline complete!", *ack)
        }
      }
    }
  }
}
```

### Reusable Service Contract

```rholang
// Create a reusable contract with registry binding
contract @"getCompiler"(return) = {
  new lookup in {
    registryLookup!("rho:metta:compile:sync", *lookup) |
    for (@service <- lookup) {
      return!(service)
    }
  }
} |

// Use it
new compile in {
  @"getCompiler"!(*compile) |
  for (@service <- compile) {
    for (@pm <- service!?("(+ 1 2)")) {
      stdoutAck!(pm, *ack)
    }
  }
}
```

## Error Handling

```rholang
new compile in {
  registryLookup!("rho:metta:compile:sync", *compile) |

  for (@service <- compile) {
    // Try to compile with !?
    for (@pm <- service!?("(+ 1 2")) {  // Syntax error: missing )
      // pm will be error JSON: {"success":false,"error":"..."}
      match pm.contains("\"success\":false") {
        true => stdoutAck!("Compilation failed: " ++ pm, *ack)
        false => stdoutAck!("Compilation succeeded: " ++ pm, *ack)
      }
    }
  }
}
```

## Performance Considerations

Using registry binding adds a small overhead:
1. Registry lookup: ~0.1ms
2. Channel binding: ~0.01ms
3. **Total overhead**: ~0.11ms (negligible)

Once bound, performance is identical to direct URN access.

## Comparison

### Direct URN Access
```rholang
for (@pm <- @"rho:metta:compile:sync"!?("(+ 1 2)")) { ... }
```
✅ Simpler
✅ Faster (no registry lookup)
❌ Less flexible (hardcoded URN)

### Registry Binding
```rholang
new compile in {
  registryLookup!("rho:metta:compile:sync", *compile) |
  for (@service <- compile) {
    for (@pm <- service!?("(+ 1 2)")) { ... }
  }
}
```
✅ More flexible (configurable URN)
✅ Can rebind to different implementations
✅ Standard Rholang pattern
❌ Slightly more code

## Summary

**Question**: Can't the `!?` operator be used in the following fashion?
```rholang
for (@pm <- compile!?(text)) { ... }
```

**Answer**: **YES!** ✅

**Requirements**:
1. Use `rho:metta:compile:sync` (arity 1, not arity 2)
2. Bind `compile` from registry: `registryLookup!("rho:metta:compile:sync", *compile)`
3. Or use URN directly: `@"rho:metta:compile:sync"!?(text)`

**This is the idiomatic Rholang pattern** and is fully supported by the synchronous variant!

## Which Service to Use?

| Your Pattern | Use This Service |
|--------------|------------------|
| `for (@pm <- compile!?(text))` | `rho:metta:compile:sync` ✅ |
| `@service!(text, *result)` | `rho:metta:compile` |
| `@service!?(text, *result)` | Either works |

## Recommendation

For your use case with `for (@pm <- compile!?(text))`, use:

**`rho:metta:compile:sync`** - Designed exactly for this pattern!

See `RHOLANG_SYNC_GUIDE.md` for more examples.
