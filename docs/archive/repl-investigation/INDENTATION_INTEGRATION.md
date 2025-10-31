# SmartIndenter Integration

## Overview

The SmartIndenter component has been integrated into the MeTTaTron REPL, providing intelligent indentation calculation based on syntax structure.

## Status: ✅ Integrated

- **Component**: `src/repl/indenter.rs` (SmartIndenter)
- **Integration**: `src/repl/helper.rs` (MettaHelper)
- **Tests**: 14 tests passing in `repl::helper`
- **Example**: `examples/indenter_demo.rs`

## Capabilities

### What It Does

✅ **Calculates proper indentation** based on:
- Unclosed parentheses `(` and `)`
- Unclosed braces `{` and `}`
- Nesting depth (2 spaces per level, configurable)

✅ **Syntax-aware parsing**:
- Uses Tree-Sitter for accurate parsing
- Ignores delimiters inside strings
- Ignores delimiters in comments (`//`, `/* */`, `;`)

✅ **Fully tested**:
```bash
cargo test --lib repl::helper  # 14 tests pass
cargo run --example indenter_demo  # Demo example
```

✅ **API available**:
```rust
// From MettaHelper
let mut helper = MettaHelper::new()?;
let indent = helper.calculate_indent("(+ 1");  // Returns 2

// Adjust indent width
helper.indenter_mut().set_indent_width(4);

// Generate continuation prompt
let prompt = helper.indenter_mut()
    .continuation_prompt("(foo", "...> ");
// Returns: "...>   " (with 2 spaces)
```

### What It Doesn't Do

⚠️ **No automatic space insertion** - Due to rustyline's API limitations:
- Rustyline's `Validator` trait detects incomplete expressions
- But there's no hook to insert spaces into the input buffer
- No way to dynamically change the prompt per line

This means:
- ❌ Can't auto-insert spaces when user presses RETURN on incomplete expression
- ❌ Can't dynamically adjust continuation prompt based on nesting
- ✅ Can only provide visual prompt indentation (fixed per session)

## Architecture

### Component Structure

```
SmartIndenter (indenter.rs)
├── Tree-Sitter parser for syntax analysis
├── count_indent_level() - Delimiter counting heuristic
├── calculate_indent() - Returns number of spaces
└── continuation_prompt() - Formats prompt with indent

MettaHelper (helper.rs)
├── Contains SmartIndenter instance
├── Exposes indenter() and indenter_mut()
└── Provides calculate_indent() convenience method
```

### Why Delimiter Counting vs Tree-Sitter Queries?

The implementation uses **delimiter counting** instead of Tree-Sitter indent queries:

**Tree-Sitter Queries** (`indents.scm`):
- Designed for editor integration (Neovim, Emacs, VSCode)
- Use capture names: `@indent`, `@dedent`, `@branch`
- Editors interpret these to adjust buffer indentation
- Not suitable for calculating numeric indent levels

**Delimiter Counting**:
- Direct algorithm: count unclosed `(` and `{`
- Simple, fast, and accurate for S-expressions
- Returns exact number of spaces needed
- Perfect for REPL use case

The Tree-Sitter parser is still used for accurate tokenization (to identify strings and comments).

## Usage

### In REPL Code

```rust
use mettatron::repl::MettaHelper;

let mut helper = MettaHelper::new().unwrap();

// User types: "(let ("
let buffer = "(let (";
let indent = helper.calculate_indent(buffer);
println!("Need {} spaces of indentation", indent);  // 2
```

### External Tools

The SmartIndenter can be used by:
- **Editor plugins** - Calculate indent for MeTTa files
- **Custom REPLs** - Implement auto-indentation if using a different readline library
- **Formatters** - Auto-format MeTTa code with proper indentation
- **LSP servers** - Provide indentation hints to editors

### Example

See `examples/indenter_demo.rs`:
```bash
cargo run --example indenter_demo
```

Output demonstrates:
- Simple incomplete expressions
- Nested expressions
- String-aware parsing
- Comment-aware parsing
- Custom indent width
- Continuation prompt generation

## Future Enhancements

Potential improvements if rustyline adds API support:

1. **Dynamic continuation prompts**:
   ```metta
   metta[1]> (let (
   ...>        ($x 5)       # Auto-indented 2 spaces
   ...>        ($y (+ 1     # Auto-indented 2 spaces
   ...>              2)))   # Auto-indented 4 spaces (nested)
   ```

2. **Auto-insert spaces** on RETURN for incomplete expressions

3. **Smart re-indentation** as user types closing delimiters

4. **Copy-paste auto-indent** for multi-line code blocks

## Implementation Notes

### Integration Points

1. **`src/repl/helper.rs:39`** - SmartIndenter field added to MettaHelper
2. **`src/repl/helper.rs:50`** - Initialized in `MettaHelper::new()`
3. **`src/repl/helper.rs:73-86`** - Accessor methods added
4. **`src/repl/helper.rs:345-359`** - Integration test added

### Dependencies

- `tree-sitter` (0.25) - Syntax parsing
- `tree-sitter-metta` - MeTTa grammar and queries

### Configuration

Default indent width: **2 spaces**

Change via:
```rust
helper.indenter_mut().set_indent_width(4);
```

Or create with custom width:
```rust
let indenter = SmartIndenter::with_indent_width(4)?;
```

## Testing

Run tests:
```bash
# All REPL tests (including indenter)
cargo test --lib repl::helper

# Specific indenter test
cargo test --lib repl::helper::tests::test_indenter_access

# All indenter unit tests
cargo test --lib repl::indenter

# Demo example
cargo run --example indenter_demo
```

All 282 library tests pass, including:
- ✅ Indenter creation and initialization
- ✅ Indent calculation for various expressions
- ✅ String-aware parsing
- ✅ Comment-aware parsing
- ✅ Custom indent width
- ✅ Integration with MettaHelper

## Related Documentation

- `docs/REPL_FEATURES.md` - Complete REPL feature documentation
- `src/repl/indenter.rs` - SmartIndenter implementation
- `src/repl/helper.rs` - MettaHelper integration
- `tree-sitter-metta/queries/indents.scm` - Tree-Sitter indent queries (for editor use)

## API Reference

### SmartIndenter

```rust
pub struct SmartIndenter { /* ... */ }

impl SmartIndenter {
    /// Create with default 2-space indent
    pub fn new() -> Result<Self, String>

    /// Create with custom indent width
    pub fn with_indent_width(width: usize) -> Result<Self, String>

    /// Calculate indentation level (in spaces)
    pub fn calculate_indent(&mut self, buffer: &str) -> usize

    /// Get current indent width
    pub fn indent_width(&self) -> usize

    /// Set indent width
    pub fn set_indent_width(&mut self, width: usize)

    /// Generate continuation prompt with indentation
    pub fn continuation_prompt(&mut self, buffer: &str, base_prompt: &str) -> String
}
```

### MettaHelper

```rust
impl MettaHelper {
    /// Get reference to indenter
    pub fn indenter(&self) -> &SmartIndenter

    /// Get mutable reference to indenter
    pub fn indenter_mut(&mut self) -> &mut SmartIndenter

    /// Calculate indentation for current buffer (convenience method)
    pub fn calculate_indent(&mut self, buffer: &str) -> usize
}
```

## Summary

**SmartIndenter is fully integrated** and provides accurate indentation calculation for MeTTa expressions. While automatic space insertion isn't possible due to rustyline's API, the component is:

- ✅ Fully functional for programmatic use
- ✅ Available via MettaHelper API
- ✅ Thoroughly tested (14 tests)
- ✅ Ready for external tool integration
- ✅ Documented with working example

Users should manually indent multi-line expressions in the REPL, but tools and plugins can use the SmartIndenter API for automatic indentation in other contexts.
