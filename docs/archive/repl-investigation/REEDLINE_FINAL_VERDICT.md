# Reedline Auto-Indentation Investigation - Final Verdict

**Date**: 2025-10-24
**Branch**: `spike/reedline-investigation`
**Investigator**: Claude Code
**Status**: ❌ **INVESTIGATION COMPLETE - AUTO-INDENT NOT POSSIBLE**

## Executive Summary

After thorough investigation of reedline as an alternative to rustyline for auto-indentation support, the verdict is:

**❌ Reedline CANNOT support automatic indentation with its current API.**

The fundamental limitation is identical to rustyline: **keybindings are static and cannot compute EditCommands dynamically based on buffer state.**

## What We Investigated

### Test 1: Custom Prompt ✅
**Result**: WORKS

Reedline supports custom prompts via the `Prompt` trait:

```rust
struct MettaPrompt {
    indent_level: usize,
}

impl Prompt for MettaPrompt {
    fn render_prompt_indicator(&self, _mode: PromptEditMode) -> Cow<str> {
        if self.indent_level > 0 {
            Cow::Owned(format!("...{}", " ".repeat(self.indent_level)))
        } else {
            Cow::Borrowed("...")
        }
    }
}
```

**Limitation**: Visual only - doesn't insert spaces into buffer

### Test 2: Validator ✅
**Result**: WORKS (same as rustyline)

```rust
impl Validator for MettaValidator {
    fn validate(&self, line: &str) -> ValidationResult {
        if has_unclosed_delimiters(line) {
            ValidationResult::Incomplete
        } else {
            ValidationResult::Complete
        }
    }
}
```

Works identically to rustyline's Validator.

### Test 3: SubmitOrNewline ✅
**Result**: WORKS

`ReedlineEvent::SubmitOrNewline` respects the Validator:
- If text is complete → submits
- If text is incomplete → inserts newline

This is useful but doesn't solve auto-indentation.

### Test 4: EditCommand::InsertString ✅
**Result**: WORKS!

**Critical Discovery**: Reedline DOES have text insertion APIs:

```rust
EditCommand::InsertString(String)
EditCommand::InsertNewline
```

We CAN insert text programmatically! For example:

```rust
keybindings.add_binding(
    KeyModifiers::ALT,
    KeyCode::Enter,
    ReedlineEvent::Edit(vec![
        EditCommand::InsertNewline,
        EditCommand::InsertString("  ".to_string()),  // 2 spaces
    ]),
);
```

**This proves the insertion mechanism exists!**

### Test 5: Dynamic Auto-Indentation ❌
**Result**: BLOCKED

**The Fatal Flaw**: Keybindings are STATIC

```rust
// What we NEED (doesn't exist):
keybindings.add_dynamic_binding(
    KeyCode::Enter,
    |buffer: &str| {  // <-- Callback with buffer access
        let indent = calculate_indent(buffer);
        vec![
            EditCommand::InsertNewline,
            EditCommand::InsertString(" ".repeat(indent)),
        ]
    }
);

// What we HAVE (static only):
keybindings.add_binding(
    KeyModifiers::NONE,
    KeyCode::Enter,
    ReedlineEvent::Edit(vec![  // <-- Fixed at definition time
        EditCommand::InsertNewline,
        EditCommand::InsertString("  ".to_string()),  // Always 2 spaces
    ])
);
```

**The Problem**:
1. Keybindings are defined once at startup
2. `ReedlineEvent::Edit(Vec<EditCommand>)` is a static vec of commands
3. No callback or closure to compute commands dynamically
4. No hook that runs before executing Edit commands

## API Comparison: Rustyline vs Reedline

| Feature | Rustyline | Reedline | Notes |
|---------|-----------|----------|-------|
| **Validator** | ✅ | ✅ | Same |
| **Highlighter** | ✅ | ✅ | Same |
| **Completer** | ✅ | ✅ | Same |
| **Hinter** | ✅ | ✅ | Same |
| **Text insertion** | ❌ No public API | ✅ EditCommand::InsertString | Reedline wins |
| **Dynamic keybindings** | ❌ No | ❌ No | Both blocked |
| **Event system** | ⚠️ Basic | ✅ Comprehensive | Reedline better |
| **Architecture** | ⚠️ Older | ✅ Modern | Reedline better |

## What Reedline Has (But Doesn't Help)

✅ **Better Architecture**:
- Event-driven design
- `ReedlineEvent` enum
- More extensible than rustyline

✅ **Text Insertion APIs**:
- `EditCommand::InsertString(String)`
- `EditCommand::InsertNewline`
- `ReedlineEvent::Edit(Vec<EditCommand>)`

✅ **More Features**:
- `SubmitOrNewline` event
- Better prompt customization
- Vi and Emacs modes
- Menu system

## What Reedline Lacks (Critical for Auto-Indent)

❌ **No Dynamic Keybindings**:
- Cannot compute EditCommands based on buffer state
- Keybindings are static, defined once
- No closure/callback system

❌ **No Pre-Edit Hook**:
- No callback before executing EditCommands
- No way to intercept and modify commands
- No buffer access during event handling

❌ **No Buffer Manipulation API**:
- Can't call `insert_string()` from outside keybindings
- No public `get_buffer() -> &str` method
- No public `set_insertion_point(usize)` method

## The Missing Piece

What we need for auto-indentation:

```rust
// Hypothetical API (DOES NOT EXIST):
pub trait DynamicKeybinding: Send + Sync {
    fn handle_key(&self, buffer: &str, cursor: usize) -> Vec<EditCommand>;
}

// Usage:
struct AutoIndentHandler;

impl DynamicKeybinding for AutoIndentHandler {
    fn handle_key(&self, buffer: &str, _cursor: usize) -> Vec<EditCommand> {
        let indent = calculate_indent(buffer) * 2;
        vec![
            EditCommand::InsertNewline,
            EditCommand::InsertString(" ".repeat(indent)),
        ]
    }
}

keybindings.add_dynamic_binding(
    KeyCode::Enter,
    Box::new(AutoIndentHandler),
);
```

## Feature Request Potential

**Could this be added to reedline?**

Possibly! The groundwork exists:
- ✅ EditCommand system is flexible
- ✅ Event-driven architecture
- ✅ Active development

**Proposed Addition**:

```rust
pub enum ReedlineEvent {
    // ... existing variants

    /// Execute edit commands computed by a callback
    DynamicEdit(Box<dyn Fn(&str, usize) -> Vec<EditCommand> + Send + Sync>),
}
```

**Alternative**:

```rust
pub trait PreEditHook: Send + Sync {
    /// Called before executing EditCommands
    /// Can modify or replace the commands
    fn pre_edit(&self, buffer: &str, commands: Vec<EditCommand>) -> Vec<EditCommand>;
}
```

## Recommendations

### Option 1: Stay with Rustyline ✅
**Recommended for now**

- Already works
- Proven, stable
- No migration cost
- Same limitations as reedline

**Verdict**: **Best current choice**

### Option 2: Migrate to Reedline (Without Auto-Indent)
**Only if other features are valuable**

Benefits:
- Better architecture
- More modern
- Easier to extend in future
- Active development

Cost:
- 6-10 hours migration
- No auto-indent gain
- Potential API changes

**Verdict**: Not worth it for just architecture

### Option 3: File Reedline Feature Request
**Worth doing regardless**

Actions:
1. File issue on reedline GitHub
2. Propose `DynamicEdit` or `PreEditHook` API
3. Reference our use case
4. Offer to contribute if accepted

**Verdict**: Do this, but don't block on it

### Option 4: Custom Implementation
**Last resort**

- ~2 weeks effort
- Full control
- High maintenance

**Verdict**: Not worth it for auto-indent alone

## Conclusion

### Final Answer

**Q**: Can reedline support auto-indentation?
**A**: ❌ **NO**, with current API

**Q**: Why not?
**A**: Keybindings are static - cannot compute EditCommands dynamically

**Q**: Is reedline better than rustyline?
**A**: ✅ **Architecture yes**, ❌ **auto-indent capability no**

**Q**: Should we migrate?
**A**: ❌ **NO**, not for auto-indentation

**Q**: Should we file feature request?
**A**: ✅ **YES**, it's a reasonable ask

### Recommendation

**Short term**: **Stay with rustyline**
- No change needed
- Works well for our use case
- SmartIndenter still valuable for documentation and future use

**Medium term**: **Monitor reedline development**
- Watch for dynamic keybinding features
- Track feature request progress
- Re-evaluate if API improves

**Long term**: **Consider custom if critical**
- Only if auto-indent becomes essential
- ~2 week investment
- Full control over behavior

### Integration Status

The SmartIndenter we built is **library-agnostic** and remains valuable:
- ✅ Works with any readline library
- ✅ Provides indent calculation API
- ✅ Ready for external tooling
- ✅ Demonstrates feasibility
- ✅ Can be used if reedline adds dynamic bindings later

## Files Created

- ✅ `examples/reedline_spike.rs` - Basic validation
- ✅ `examples/reedline_autoindent_test.rs` - Advanced investigation
- ✅ `docs/REEDLINE_INVESTIGATION.md` - Initial findings
- ✅ `docs/REEDLINE_FINAL_VERDICT.md` - This document
- ✅ `docs/READLINE_ALTERNATIVES.md` - Comprehensive comparison
- ✅ `docs/INDENTATION_INTEGRATION.md` - SmartIndenter integration

## Next Steps

1. **Document findings** ✅ (this file)
2. **Close spike branch** (after user review)
3. **File reedline feature request** (optional but recommended)
4. **Stay with rustyline** (decision made)
5. **Keep SmartIndenter** (useful for future/tooling)

## Branch Cleanup

```bash
# After review:
git checkout main
git branch -D spike/reedline-investigation

# Remove reedline from Cargo.toml
# It was only added as optional dependency
```

---

**Investigation Time**: ~4 hours
**Lines of Investigation Code**: ~330
**Result**: Clear answer, no ambiguity
**Value**: Confirmed the right decision is to stay with rustyline
