# Rholang Parser Build Automation

## The Challenge

The Rholang parser has two states that must match:

1. **Grammar State** (Tree-Sitter): Generated with or without `RHOLANG_NAMED_COMMENTS=1`
2. **Parser State** (Rust): Compiled with or without `features = ["named-comments"]`

**Mismatch causes parser panics**:
- Grammar WITH named comments + Parser WITHOUT feature = `unimplemented!()` panic
- Grammar WITHOUT named comments + Parser WITH feature = No panic, but wasted code

## Solution: Conditional Build Automation

Since Cargo features are resolved before `build.rs` runs, we can't conditionally enable the feature. However, we CAN:

### Option 1: Auto-Regenerate Grammar to Match Feature (Recommended for LSP)

```rust
// build.rs for rholang-language-server
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Detect if named-comments feature is enabled
    let named_comments_enabled = cfg!(feature = "named-comments");

    ensure_grammar_matches_feature(named_comments_enabled)?;
    Ok(())
}

fn ensure_grammar_matches_feature(enable_named_comments: bool) -> Result<(), Box<dyn std::error::Error>> {
    let marker_path = "../rholang-rs/rholang-tree-sitter/.named_comments_enabled";
    let grammar_has_named_comments = Path::new(marker_path).exists();

    // Check if regeneration needed
    if enable_named_comments != grammar_has_named_comments {
        println!("cargo:warning=Grammar state doesn't match feature flag, regenerating...");

        let env_value = if enable_named_comments { "1" } else { "0" };

        Command::new("tree-sitter")
            .args(&["generate"])
            .current_dir("../rholang-rs/rholang-tree-sitter")
            .env("RHOLANG_NAMED_COMMENTS", env_value)
            .status()?;

        if enable_named_comments {
            fs::write(marker_path, "named-comments-enabled\n")?;
        } else {
            let _ = fs::remove_file(marker_path);
        }
    }

    Ok(())
}
```

**Pros**:
- Automatically keeps grammar and feature in sync
- Works for language tools that need comments
- No manual intervention

**Cons**:
- Can cause unexpected regeneration
- Requires tree-sitter CLI
- May conflict with other projects using same rholang-rs

### Option 2: Detect and Error on Mismatch (Recommended for Runtime)

```rust
// build.rs for f1r3node/rholang
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let named_comments_enabled = cfg!(feature = "named-comments");
    verify_grammar_matches_feature(named_comments_enabled)?;
    Ok(())
}

fn verify_grammar_matches_feature(feature_enabled: bool) -> Result<(), Box<dyn std::error::Error>> {
    let marker_path = "../../rholang-rs/rholang-tree-sitter/.named_comments_enabled";
    let grammar_has_named_comments = Path::new(marker_path).exists();

    if feature_enabled != grammar_has_named_comments {
        let error_msg = format!(
            "GRAMMAR MISMATCH DETECTED!\n\
             Grammar has named comments: {}\n\
             Rust feature enabled: {}\n\n\
             To fix, regenerate the grammar:\n\
             cd rholang-rs/rholang-tree-sitter\n\
             RHOLANG_NAMED_COMMENTS={} tree-sitter generate\n",
            grammar_has_named_comments,
            feature_enabled,
            if feature_enabled { "1" } else { "0" }
        );
        return Err(error_msg.into());
    }

    Ok(())
}
```

**Pros**:
- Prevents mismatches explicitly
- Doesn't auto-modify shared dependencies
- Clear error messages guide user

**Cons**:
- Requires manual fix
- Stops build on mismatch

### Option 3: Emit Custom cfg Flag (Advanced)

```rust
// build.rs
fn main() {
    let marker_exists = Path::new("../.named_comments_enabled").exists();

    if marker_exists {
        println!("cargo:rustc-cfg=grammar_named_comments");
    }
}
```

Then in code:
```rust
#[cfg(all(feature = "named-comments", grammar_named_comments))]
// Handle named comment nodes

#[cfg(not(grammar_named_comments))]
// Skip comment handling code
```

**Pros**:
- Maximum flexibility
- Can handle mismatches at runtime

**Cons**:
- More complex code
- Runtime checks instead of compile-time

## Recommended Approach

### For rholang-language-server (LSP Tools)
Use **Option 1** - Auto-regenerate to match feature:
- Always builds with `features = ["named-comments"]`
- build.rs ensures grammar is regenerated with RHOLANG_NAMED_COMMENTS=1
- Isolated build, doesn't affect other projects

### For f1r3node (Runtime)
Use **Option 2** - Verify and error on mismatch:
- Never uses `features = ["named-comments"]`
- build.rs verifies grammar was NOT generated with named comments
- Fails fast with clear error if mismatch detected

### For Shared Development
Add a **marker check** to prevent accidental misuse:

```rust
// Both projects can include this
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let feature_enabled = cfg!(feature = "named-comments");
    let marker_exists = Path::new("../../rholang-rs/rholang-tree-sitter/.named_comments_enabled").exists();

    if feature_enabled && !marker_exists {
        return Err("named-comments feature enabled but grammar wasn't generated with it!".into());
    }

    if !feature_enabled && marker_exists {
        return Err("Grammar has named comments but feature is disabled!".into());
    }

    Ok(())
}
```

## Current State

After fixing the tests:

- ✅ f1r3node: NO `named-comments` feature, grammar generated WITHOUT named comments
- ✅ rholang-language-server: HAS `named-comments` feature, has build.rs that regenerates grammar
- ✅ Tests: All 472 passing

## Summary

**Short Answer**: Yes, you can conditionally check grammar state in build.rs and either:
1. **Auto-regenerate** grammar to match the feature (for LSP tools)
2. **Validate and error** on mismatch (for runtime tools)
3. **Emit cfg flags** for runtime detection (advanced)

The marker file (`.named_comments_enabled`) serves as the grammar state indicator.
