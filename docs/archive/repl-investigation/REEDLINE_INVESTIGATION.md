# Reedline Investigation - Findings

**Branch**: `spike/reedline-investigation`
**Date**: 2025-10-24
**Status**: ✅ Initial investigation complete

## Summary

Reedline is **viable** as a rustyline replacement with similar API and potentially better extensibility for auto-indentation.

## Compilation Status

✅ **Success** - Reedline 0.43 compiles with our project

**Dependencies added**:
```toml
reedline = { version = "0.43", optional = true }
```

**Build time**: ~2 minutes (initial compilation with dependencies)

## API Comparison

### Rustyline vs Reedline

| Feature | Rustyline | Reedline | Compatible? |
|---------|-----------|----------|-------------|
| **Validator trait** | ✅ `ValidationResult::Complete/Incomplete/Invalid` | ⚠️ `ValidationResult::Complete/Incomplete` (no Invalid) | Mostly |
| **Multi-line support** | ✅ Via Validator | ✅ Via Validator | ✅ Yes |
| **Syntax highlighting** | ✅ Highlighter trait | ✅ Highlighter trait | ✅ Yes |
| **Completions** | ✅ Completer trait | ✅ Completer trait | ✅ Yes |
| **Hints** | ✅ Hinter trait | ✅ Hinter trait | ✅ Yes |
| **History** | ✅ Built-in | ✅ FileBackedHistory | ✅ Yes |
| **Helper trait** | ✅ Combined trait | ❌ Individual traits | ⚠️ Different pattern |

### Key API Differences

1. **No `Invalid` validation state**:
   - Rustyline: `ValidationResult::Invalid(Some(message))`
   - Reedline: Only `Complete` or `Incomplete`
   - Impact: Extra closing delimiters can't be flagged during input, only during evaluation

2. **No combined Helper trait**:
   - Rustyline: Single `Helper` trait combining all features
   - Reedline: Individual traits configured separately
   - Impact: More verbose setup, but more flexible

3. **Different prompt API**:
   - Rustyline: `editor.readline(prompt_str)`
   - Reedline: `line_editor.read_line(&prompt)` where prompt is a `Prompt` trait object
   - Impact: Need to implement custom Prompt trait

## Test Results

### What Works

✅ **Compilation**: No blockers
✅ **Validator trait**: Multi-line detection works
✅ **Basic REPL**: Input/output loop functional
✅ **Indentation calculation**: Our SmartIndenter logic works

### Example Code

See: `examples/reedline_spike.rs`

```rust
use reedline::{DefaultPrompt, Reedline, Signal, ValidationResult, Validator};

struct MettaValidator;

impl Validator for MettaValidator {
    fn validate(&self, line: &str) -> ValidationResult {
        if has_unclosed_delimiters(line) {
            ValidationResult::Incomplete
        } else {
            ValidationResult::Complete
        }
    }
}

let mut line_editor = Reedline::create()
    .with_validator(Box::new(MettaValidator));
```

## Auto-Indentation Investigation

###  Current Understanding

**Key Question**: Can we intercept Enter and insert indentation?

**Reedline's Event System**:
- Uses `ReedlineEvent` enum for keybindings
- Custom events possible via `ReedlineEvent::ExecuteHostCommand(name)`
- But unclear if we can access/modify buffer from custom events

**Potential Approaches**:

1. **Custom Keybinding** (needs testing):
   ```rust
   keybindings.add_binding(
       KeyModifiers::NONE,
       KeyCode::Enter,
       ReedlineEvent::ExecuteHostCommand("check_indent".to_string())
   );

   // Then handle in read_line loop:
   match line_editor.read_line(&prompt) {
       Ok(Signal::ExecuteHostCommand(cmd)) if cmd == "check_indent" => {
           // Can we access buffer here?
           // Can we insert text?
       }
   }
   ```

2. **Prompt-based** (current approach):
   - Custom `Prompt` trait implementation
   - Provide indented prompt visually
   - User still types manually

3. **Editor Modification** (fork reedline):
   - Add `insert_str()` method to public API
   - Expose buffer access during events
   - Upstream contribution?

### Next Steps for Auto-Indent Testing

**Phase 1**: Test custom Enter handling (2 hours)
- [ ] Implement custom keybinding for Enter
- [ ] Check if `ExecuteHostCommand` provides buffer access
- [ ] Test if we can call `insert_str()` or similar

**Phase 2**: Investigate reedline internals (2 hours)
- [ ] Read reedline source for buffer manipulation APIs
- [ ] Check if `Reedline::read_line()` exposes buffer
- [ ] Look for `insert_text()` or `get_buffer()` methods

**Phase 3**: Test or fork (2-4 hours)
- [ ] If API exists: Implement auto-indent
- [ ] If API missing: Consider fork or feature request upstream

## Migration Effort Estimate

### If Auto-Indent Works

**Total Time**: 6-10 hours

| Task | Effort | Risk |
|------|--------|------|
| Update Cargo.toml | 5 min | Low |
| Create custom Prompt | 1 hour | Low |
| Port MettaHelper traits | 2 hours | Low |
| Migrate main.rs REPL | 1 hour | Low |
| Implement auto-indent | 1-2 hours | Medium |
| Testing | 2-3 hours | Medium |
| Documentation | 1 hour | Low |

### If Auto-Indent Doesn't Work

**Options**:
1. **Use reedline without auto-indent** (still better architecture)
2. **Fork reedline** and add API (high maintenance)
3. **Stay with rustyline** (current state)

## Advantages of Reedline

Even **without** auto-indent:

✅ **Better Architecture**
- Event-driven design
- Easier to extend
- More modern codebase

✅ **Better Documentation**
- Used by Nushell (battle-tested)
- Active community
- Good examples

✅ **Future-Proof**
- Active development
- Modern Rust idioms
- Likely to add features we need

## Disadvantages

⚠️ **No Invalid state**
- Can't flag syntax errors during input
- Only during evaluation

⚠️ **More setup code**
- Individual trait configuration
- Custom Prompt implementation
- More verbose than rustyline

⚠️ **Newer library**
- Less stable API (but Nushell uses it in production)
- Potential breaking changes

## Recommendation

###  Continue Investigation

**Next Step**: Test auto-indentation capability (2-4 hours)

**Decision Tree**:
```
Can reedline support auto-indent?
├─ YES → Migrate to reedline ✅
├─ NO, but has good API →
│  ├─ Submit feature request/PR
│  └─ Use reedline without auto-indent (still better than rustyline)
└─ NO, and API is locked →
   ├─ Fork reedline (high effort)
   └─ Stay with rustyline (current state)
```

###  Immediate Action

**Create**: `examples/reedline_autoindent_test.rs`
- Test custom Enter handling
- Check buffer access
- Verify text insertion

**Timeline**: 2-4 hours for complete answer

## Code Samples

### Current Rustyline Setup

```rust
let mut editor = Editor::new()?;
editor.set_helper(Some(helper));

loop {
    match editor.readline(&prompt) {
        Ok(line) => { /* ... */ }
        Err(ReadlineError::Interrupted) => continue,
        Err(ReadlineError::Eof) => break,
        Err(err) => { /* ... */ }
    }
}
```

### Proposed Reedline Setup

```rust
let mut line_editor = Reedline::create()
    .with_validator(Box::new(validator))
    .with_highlighter(Box::new(highlighter))
    .with_hinter(Box::new(hinter))
    .with_history(Box::new(history));

loop {
    match line_editor.read_line(&prompt)? {
        Signal::Success(buffer) => { /* ... */ }
        Signal::CtrlC => continue,
        Signal::CtrlD => break,
        Signal::ExecuteHostCommand(cmd) => {
            // Custom command handling
        }
    }
}
```

## Files Created

- ✅ `examples/reedline_spike.rs` - Basic multi-line test
- 🔜 `examples/reedline_autoindent_test.rs` - Auto-indent testing
- ✅ `docs/REEDLINE_INVESTIGATION.md` - This document

## Branch Status

**Current**: `spike/reedline-investigation`
**Commits**: None yet (exploratory only)
**Merge**: Do not merge until auto-indent capability confirmed

## Next Session Tasks

1. **Test auto-indentation** (2 hours)
   - Custom Enter keybinding
   - Buffer access testing
   - Text insertion API

2. **Decision Point** (30 min)
   - Go/No-go on reedline migration
   - Document findings
   - Update recommendation

3. **If GO**: Start migration (6-8 hours)
4. **If NO-GO**: Document and close spike

## References

- Reedline docs: https://docs.rs/reedline/
- Reedline repo: https://github.com/nushell/reedline
- Nushell REPL: https://github.com/nushell/nushell (reference implementation)
- Related: `docs/READLINE_ALTERNATIVES.md`
- Related: `docs/INDENTATION_INTEGRATION.md`

## Conclusion

**Verdict**: 🟡 **Promising but needs more testing**

Reedline is a **viable alternative** with similar API and potentially better extensibility. The key blocker is confirming auto-indentation capability.

**Recommendation**: Invest 2-4 more hours to definitively answer the auto-indent question, then decide on migration.
