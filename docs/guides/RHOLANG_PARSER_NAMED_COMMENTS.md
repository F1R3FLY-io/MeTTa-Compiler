# Rholang Parser Named Comments Feature

## Overview

The Rholang Tree-Sitter grammar supports an optional `named-comments` feature that controls whether comments appear as named nodes in the syntax tree.

## When to Enable Named Comments

### ✅ **Enable for Language Tools**
- **LSP servers** (rholang-language-server)
- **IDE integrations**
- **Code formatters**
- **Documentation generators**
- Any tool that needs to preserve or process comments

### ❌ **Disable for Runtime**
- **Runtime interpreters** (f1r3node, rholang-cli)
- **Compilers**
- **Execution engines**
- Any production code that doesn't need comment information

## Performance Impact

Enabling named comments creates a **performance bottleneck**:
- Parser must process comment nodes in the syntax tree
- Additional filtering required for normal operations
- Increased memory usage
- Slower parsing performance

For runtime/execution use cases, this overhead is unnecessary.

## Configuration

### For Language Tools (rholang-language-server)

**Cargo.toml:**
```toml
rholang-parser = { path = "...", features = ["named-comments"] }
rholang-tree-sitter = { path = "...", features = ["named-comments"] }
```

**build.rs:**
```rust
// Automatically regenerate grammar with named comments
fn ensure_rholang_parser_with_named_comments() {
    // Set RHOLANG_NAMED_COMMENTS=1
    // Run tree-sitter generate
    // Verify named comments are enabled
}
```

### For Runtime (f1r3node, rholang-cli)

**Cargo.toml:**
```toml
# NO features = ["named-comments"]
rholang-parser = { path = "..." }
```

**No build.rs needed** - use default grammar generation

## Grammar Regeneration

The grammar must be regenerated based on which mode you need:

### With Named Comments (for LSP)
```bash
cd rholang-rs/rholang-tree-sitter
RHOLANG_NAMED_COMMENTS=1 tree-sitter generate
# Creates .named_comments_enabled marker
```

### Without Named Comments (for runtime)
```bash
cd rholang-rs/rholang-tree-sitter
RHOLANG_NAMED_COMMENTS=0 tree-sitter generate
# or
tree-sitter generate  # (unset = disabled)
```

## Why This Matters

The grammar file (`grammar.js`) uses the environment variable to conditionally include comments:

```javascript
const namedComments = process.env.RHOLANG_NAMED_COMMENTS === '1';

module.exports = grammar({
    extras: $ => [
        namedComments ? $.line_comment : $._line_comment,  // Named or unnamed
        namedComments ? $.block_comment : $._block_comment,
        /\s/,
    ],
    // ...
});
```

- `$.line_comment` = named node (visible in tree, causes parser to handle it)
- `$._line_comment` = unnamed node (filtered out, better performance)

## Issue We Encountered

The integration tests were failing because:

1. ✅ `rholang-language-server` was built with `RHOLANG_NAMED_COMMENTS=1`
2. ✅ This regenerated the grammar with named comments enabled
3. ❌ `f1r3node` tried to use this grammar WITHOUT the `named-comments` feature
4. ❌ Mismatch caused parser to hit `unimplemented!()` code path

## Solution

1. **Regenerate grammar without named comments** for default/runtime use:
   ```bash
   cd rholang-rs/rholang-tree-sitter
   RHOLANG_NAMED_COMMENTS=0 tree-sitter generate
   ```

2. **Only enable `named-comments` feature for tools that need it**:
   - rholang-language-server: ✅ uses `features = ["named-comments"]`
   - f1r3node/rholang: ❌ does NOT use the feature

3. **Use build automation for LSP** (rholang-language-server has build.rs)
   - Automatically regenerates with RHOLANG_NAMED_COMMENTS=1
   - Ensures consistency for language tools

4. **Use default grammar for runtime** (f1r3node has no build.rs)
   - Uses pre-generated grammar without named comments
   - Better performance for execution

## Best Practices

1. **Default State**: Grammar should be generated WITHOUT named comments (better performance)

2. **LSP Tools**: Use build.rs to automatically enable named comments during build

3. **Shared Codebase**: If multiple projects use rholang-rs:
   - Language tools: build.rs regenerates with their settings
   - Runtime tools: use default grammar (no regeneration)

4. **Testing**: Ensure grammar state matches your feature flags:
   ```bash
   # Check current state
   ls rholang-tree-sitter/.named_comments_enabled

   # If present, named comments are enabled
   # If absent, named comments are disabled
   ```

## Project Configuration Summary

| Project | Named Comments | build.rs | Reason |
|---------|---------------|----------|---------|
| rholang-language-server | ✅ Enabled | ✅ Yes | LSP needs comment info |
| f1r3node/rholang | ❌ Disabled | ❌ No | Runtime doesn't need it |
| MeTTa-Compiler tests | ❌ Disabled | ❌ No | Uses f1r3node's parser |

## Verification

After regeneration, verify tests pass:

```bash
# Integration tests
cargo test --test rholang_integration

# Library tests
cargo test --lib

# All tests
cargo test
```

**Expected Result**: All 472 tests passing (385 lib + 69 integration + 13 binary + 5 other)
