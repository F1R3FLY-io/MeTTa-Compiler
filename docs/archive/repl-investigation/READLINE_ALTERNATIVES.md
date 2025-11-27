# Readline Library Alternatives for MeTTaTron

## Current State: Rustyline

**Library**: rustyline v14.0
**Status**: Working well, but limited extensibility
**Limitation**: Cannot auto-insert indentation on multi-line continuation

## Alternative 1: Reedline ⭐ RECOMMENDED

**Website**: https://github.com/nushell/reedline
**Used By**: Nushell REPL
**License**: MIT

### Advantages

✅ **Better Architecture**
- Event-driven design with `ReedlineEvent` enum
- Can intercept and customize key handling
- Modern, built specifically for Rust REPLs

✅ **Feature-Rich**
- Built-in syntax highlighting support
- History with fuzzy search
- Vi and Emacs modes
- Completions with custom rendering
- Hints (like rustyline)

✅ **Active Development**
- Well-maintained by Nushell team
- Regular updates and improvements
- Good documentation

✅ **Similar API to Rustyline**
- Validator trait (multi-line detection)
- Highlighter trait (syntax highlighting)
- Hinter trait (inline suggestions)
- Migration would be straightforward

### Potential for Auto-Indentation

Reedline's event system **might** allow custom Enter handling:

```rust
// Hypothetical approach (needs verification)
use reedline::{Reedline, ReedlineEvent, EditCommand};

let mut keybindings = default_emacs_keybindings();

// Custom Enter handler
keybindings.add_binding(
    KeyModifiers::NONE,
    KeyCode::Enter,
    ReedlineEvent::Custom("check_and_indent".into())
);

// In read_line loop, handle custom event:
match line_editor.read_line(&prompt) {
    Ok(Signal::Custom(name)) if name == "check_and_indent" => {
        let buffer = line_editor.current_buffer();
        if !is_complete(buffer) {
            let indent = calculate_indent(buffer);
            line_editor.insert_str(&" ".repeat(indent));
        } else {
            // Submit
        }
    }
    // ...
}
```

**Research Needed**: Investigate if reedline supports:
1. Accessing current buffer from custom events
2. Inserting text programmatically
3. Multi-line prompt customization

### Migration Effort

**Estimated Time**: 4-8 hours

**Changes Needed**:
1. Update Cargo.toml dependency
2. Update trait names (mostly compatible)
3. Update prompt API
4. Test all REPL features

**Risk**: Low (APIs are similar)

## Alternative 2: Custom Implementation

**Approach**: Build our own using `crossterm` or `termion`

### Advantages

✅ **Full Control**
- Complete control over all behavior
- Can implement auto-indent exactly as we want
- No dependency on external readline library

✅ **No Limitations**
- Custom keybindings
- Custom cursor behavior
- Exact UX we design

### Disadvantages

❌ **Significant Development Effort**
- Estimated 1,850 lines of code
- 2-3 weeks of development
- Platform-specific code (Windows vs Unix)

❌ **Ongoing Maintenance**
- We own all terminal I/O code
- Bug fixes and edge cases
- Platform compatibility testing

### Implementation Outline

```rust
use crossterm::{terminal, cursor, event, style};
use crossterm::event::{Event, KeyCode, KeyModifiers, KeyEvent};

struct CustomRepl {
    history: Vec<String>,
    history_pos: usize,
    buffer: String,
    cursor_pos: usize,
    // ... more state
}

impl CustomRepl {
    fn read_line(&mut self) -> Result<String, io::Error> {
        terminal::enable_raw_mode()?;

        loop {
            match event::read()? {
                Event::Key(KeyEvent { code, modifiers, .. }) => {
                    match (code, modifiers) {
                        // AUTO-INDENT ON ENTER!
                        (KeyCode::Enter, KeyModifiers::NONE) => {
                            if self.is_complete() {
                                break;
                            } else {
                                self.buffer.push('\n');
                                let indent = self.calculate_indent();
                                self.buffer.push_str(&" ".repeat(indent));
                                self.redraw_with_indent(indent);
                            }
                        }

                        (KeyCode::Char(c), KeyModifiers::NONE) => {
                            self.insert_char(c);
                        }

                        (KeyCode::Backspace, _) => {
                            self.delete_char_before();
                        }

                        (KeyCode::Left, _) => {
                            self.move_cursor_left();
                        }

                        (KeyCode::Right, _) => {
                            self.move_cursor_right();
                        }

                        (KeyCode::Up, _) => {
                            self.history_prev();
                        }

                        (KeyCode::Down, _) => {
                            self.history_next();
                        }

                        (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                            self.clear_line();
                        }

                        (KeyCode::Char('d'), KeyModifiers::CONTROL) => {
                            return Err(io::ErrorKind::Interrupted.into());
                        }

                        // ... many more keybindings
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        terminal::disable_raw_mode()?;
        Ok(std::mem::take(&mut self.buffer))
    }
}
```

**Feature Completeness Breakdown**:

| Feature | Lines of Code | Complexity | Time Estimate |
|---------|--------------|------------|---------------|
| Basic input loop | 100 | Low | 2 hours |
| Cursor movement (left/right/home/end) | 200 | Medium | 4 hours |
| History navigation (up/down) | 150 | Medium | 3 hours |
| Editing (insert/delete/backspace) | 100 | Low | 2 hours |
| **Multi-line + Auto-indent** | **50** | **Low** | **1 hour** |
| Completion UI (popup menu) | 300 | High | 8 hours |
| Syntax highlighting | 150 | Medium | 4 hours |
| History search (Ctrl-R) | 300 | High | 8 hours |
| Platform support (Windows/Unix) | 400 | High | 12 hours |
| Word movement (Alt-Left/Right) | 100 | Medium | 2 hours |
| Kill/yank (Ctrl-K/Ctrl-Y) | 150 | Medium | 3 hours |
| **Total** | **~2,000** | | **~49 hours** |

### Libraries for Custom Implementation

**crossterm** (Recommended):
- Cross-platform (Windows + Unix)
- Modern, well-maintained
- Good API design
- Used by many TUI apps

**termion**:
- Unix-only
- Lightweight
- Less features than crossterm

## Alternative 3: Other Libraries

### linefeed
- Status: Maintenance mode
- Not recommended (unmaintained)

### rustyline-async
- Fork of rustyline with async support
- Same limitations as rustyline
- Not actively maintained

## Recommendation Matrix

| Criterion | Rustyline (Current) | Reedline | Custom |
|-----------|-------------------|----------|---------|
| **Auto-indent** | ❌ No | ❓ Maybe | ✅ Yes |
| **Development effort** | ✅ 0 hours | ⚠️ 4-8 hours | ❌ 49+ hours |
| **Maintenance burden** | ✅ Low | ✅ Low | ❌ High |
| **Feature completeness** | ✅ Complete | ✅ Complete | ⚠️ We build it |
| **Platform support** | ✅ Good | ✅ Good | ⚠️ We handle it |
| **Control/flexibility** | ⚠️ Limited | ✅ Good | ✅ Complete |
| **Proven/stable** | ✅ Very | ✅ Yes (Nushell) | ❌ Unknown |

## Recommended Action Plan

### Phase 1: Research Reedline (2-4 hours)

1. Create spike branch: `spike/reedline-investigation`
2. Add reedline to Cargo.toml
3. Create minimal test REPL with reedline
4. Test if custom events + buffer insertion works
5. Verify auto-indent is possible

**Decision Point**: If auto-indent works → proceed to Phase 2. Otherwise, reconsider.

### Phase 2: Prototype Migration (4-8 hours)

1. Migrate main REPL to reedline
2. Port existing helpers (Highlighter, Validator, Hinter)
3. Implement auto-indent via custom event handling
4. Test all REPL features

**Decision Point**: If migration successful → proceed to Phase 3. Otherwise, stick with rustyline.

### Phase 3: Polish & Document (2-4 hours)

1. Update documentation
2. Add tests for new behavior
3. Update examples
4. Merge to main

**Total Time**: 8-16 hours (1-2 days)

### Alternative: Custom Implementation (Only if Reedline Fails)

If reedline doesn't support our needs:

1. Evaluate if auto-indent is worth ~49 hours of work
2. Consider staged approach:
   - Phase 1: Basic REPL (8 hours)
   - Phase 2: History + editing (8 hours)
   - Phase 3: Auto-indent (1 hour)
   - Phase 4: Completion UI (8 hours)
   - Phase 5: Search + polish (24 hours)

## Code Examples

### Current (Rustyline)

```rust
let mut editor = Editor::new()?;
editor.set_helper(Some(helper));

loop {
    match editor.readline(&prompt) {
        Ok(line) => { /* ... */ }
        Err(ReadlineError::Interrupted) => { /* ... */ }
        Err(ReadlineError::Eof) => break,
        Err(err) => { /* ... */ }
    }
}
```

### Proposed (Reedline)

```rust
let mut line_editor = Reedline::create()
    .with_validator(Box::new(validator))
    .with_highlighter(Box::new(highlighter))
    .with_hinter(Box::new(hinter))
    .with_history(history);

loop {
    match line_editor.read_line(&prompt)? {
        Signal::Success(buffer) => { /* ... */ }
        Signal::CtrlC => continue,
        Signal::CtrlD => break,
    }
}
```

### Custom Implementation Skeleton

See full implementation outline above in "Alternative 2" section.

## Conclusion

**Short term**: Stay with rustyline (works well)
**Medium term**: Investigate reedline migration (best balance)
**Long term**: Consider custom implementation only if absolutely necessary

**Next Steps**:
1. Create `spike/reedline-investigation` branch
2. Test reedline for 2-4 hours
3. Report findings
4. Decide: migrate, stay, or build custom

The SmartIndenter component we've built will work with **any** of these options - it's library-agnostic!
