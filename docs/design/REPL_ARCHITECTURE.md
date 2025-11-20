# Enhanced REPL Architecture

## System Overview

```
┌─────────────────────────────────────────────────────────────┐
│                    MeTTaTron Enhanced REPL                   │
└─────────────────────────────────────────────────────────────┘

┌─────────────┐     ┌──────────────┐     ┌─────────────────┐
│   Terminal  │────▶│  Rustyline   │────▶│ State Machine   │
│   (User)    │◀────│   Editor     │◀────│  (Multi-line)   │
└─────────────┘     └──────────────┘     └─────────────────┘
                           │                        │
                           │                        │
                    ┌──────▼──────┐         ┌───────▼────────┐
                    │ Highlighter │         │  Indenter      │
                    │(Tree-Sitter)│         │ (Tree-Sitter)  │
                    └─────────────┘         └────────────────┘
                           │
                    ┌──────▼──────────────────────┐
                    │    Pattern History          │
                    │   (PathMap + MORK)          │
                    └──────┬──────────────────────┘
                           │
                    ┌──────▼──────┐
                    │  Evaluator  │
                    │  (Backend)  │
                    └─────────────┘
```

## Component Details

### 1. Rustyline Editor (Entry Point)

**File**: External crate
**Purpose**: Line editing, history, key bindings

```rust
Editor::new()
  .set_helper(MettaHelper)  // Syntax highlighting
  .readline(prompt)         // Read user input
```

**Features**:
- ↑/↓ for history navigation
- Ctrl-R for search
- Emacs/Vi keybindings
- Character-by-character callbacks

### 2. Query Highlighter

**File**: `src/repl/query_highlighter.rs`
**Purpose**: Syntax highlighting using Tree-Sitter queries

**Data Flow**:
```
Input Text ──▶ Tree-Sitter Parser ──▶ Parse Tree
                                          │
                                          ▼
highlights.scm Query ──▶ Query Execution ──▶ Captures
                                          │
                                          ▼
                               Apply ANSI Colors ──▶ Colored Text
```

**Key Method**:
```rust
impl QueryHighlighter {
    fn highlight_source(&mut self, source: &str) -> String {
        // 1. Parse with Tree-Sitter
        let tree = self.parser.parse(source, None)?;

        // 2. Execute highlights.scm query
        let captures = cursor.matches(&self.query, tree.root_node(), ...);

        // 3. Map captures to colors
        for capture in captures {
            let color = self.capture_colors[capture.index];
            // Apply color at capture position
        }
    }
}
```

### 3. State Machine

**File**: `src/repl/state_machine.rs`
**Purpose**: Handle multi-line input and REPL state

**State Diagram**:
```
        ┌──────────────┐
        │    Ready     │◀────────────────┐
        └──────┬───────┘                 │
               │                         │
       LineSubmitted(complete)    DisplayingResults
               │                         │
               ▼                         │
        ┌──────────────┐           ┌─────────┐
        │  Evaluating  │──────────▶│ Results │
        └──────────────┘           └─────────┘
               │
       LineSubmitted(incomplete)
               │
               ▼
        ┌──────────────┐
        │Continuation  │──┐
        └──────────────┘  │
               │          │
               └──────────┘
           LineSubmitted
         (still incomplete)
```

**Key Methods**:
```rust
impl ReplStateMachine {
    fn handle_event(&mut self, event: ReplEvent) -> StateTransition {
        match (&self.state, event) {
            (Ready, LineSubmitted(line)) => {
                if self.is_complete(line) {
                    StartEvaluation(line)
                } else {
                    AwaitContinuation { indent }
                }
            }
            // ... other transitions
        }
    }

    fn analyze_line(&self, line: &str) -> LineAnalysis {
        // Count open/close parens, braces, strings
        // Determine if expression is complete
    }
}
```

### 4. Smart Indenter

**File**: `src/repl/indenter.rs`
**Purpose**: Calculate indentation for continuation lines

**Query-Based Approach**:
```
Source Code ──▶ Tree-Sitter Parser ──▶ Parse Tree
                                          │
                                          ▼
indents.scm Query ──▶ Query Execution ──▶ @indent/@dedent
                                          │
                                          ▼
                              Calculate Indent Level ──▶ Spaces
```

**Example**:
```metta
(= (fib $n)          # indent level 0
  (+ (fib $x)        # @indent after '(' → level 1 (2 spaces)
     (fib $y)))      # another '(' → level 2 (4 spaces)
```

### 5. Pattern History

**File**: `src/repl/pattern_history.rs`
**Purpose**: Store and search command history using patterns

**Architecture**:
```
Command String ──▶ MeTTa Parser ──▶ MettaValue
                                        │
                                        ▼
                      .to_mork_string() ─────▶ MORK Format
                                        │
                                        ▼
                            MORK Space (PathMap Trie)
                                        │
                                        ▼
                          Pattern Match Query ──▶ Results
```

**Storage Structure**:
```rust
pub struct PatternHistory {
    space: Arc<Mutex<Space>>,       // MORK Space for pattern matching
    raw_commands: Vec<String>,      // Original command strings
    parsed_commands: Vec<MettaValue>, // Parsed expressions
}
```

**Search Flow**:
```
Pattern "(= $lhs $rhs)" ──▶ Parse ──▶ MettaValue Pattern
                                            │
                                            ▼
                            Match against parsed_commands
                                            │
                                            ▼
                          pattern_match(pattern, cmd)
                                            │
                                            ▼
                          Return (idx, raw_cmd, bindings)
```

### 6. History Search Interface

**File**: `src/repl/history_search.rs`
**Purpose**: Interactive pattern search UI

**Interaction Flow**:
```
User types: ?search (= $lhs $rhs)
     │
     ▼
Display matches with bindings
     │
     ▼
User selects: [1]
     │
     ▼
Return selected command
     │
     ▼
Execute in REPL
```

### 7. Helper (Integration Layer)

**File**: `src/repl/helper.rs`
**Purpose**: Integrate components with Rustyline

**Trait Implementation**:
```rust
impl Helper for MettaHelper {}

impl Highlighter for MettaHelper {
    fn highlight(&self, line: &str, pos: usize) -> Cow<str> {
        self.highlighter.highlight(line, pos)
    }
}

impl Hinter for MettaHelper {
    fn hint(&self, line: &str, pos: usize, ctx: &Context) -> Option<String> {
        // TODO: Show hints (matching parens, function signatures)
        None
    }
}

impl Completer for MettaHelper {
    fn complete(&self, line: &str, pos: usize, ctx: &Context)
        -> Result<(usize, Vec<String>)> {
        // TODO: Complete built-in functions
        Ok((0, vec![]))
    }
}

impl Validator for MettaHelper {
    fn validate(&self, ctx: &mut ValidationContext) -> Result<ValidationResult> {
        // TODO: Validate syntax, multi-line support
        Ok(ValidationResult::Valid(None))
    }
}
```

## Data Flow Example

### User Types: `(= (double $x)`

```
1. Rustyline captures keystrokes
        │
        ▼
2. QueryHighlighter.highlight() called for each character
        │
        ▼
3. Tree-Sitter parses "(= (double $x)"
        │
        ▼
4. highlights.scm query matches:
   - "=" → @keyword.operator → Bold Magenta
   - "double" → @function → White
   - "$x" → @variable → Bold Cyan
        │
        ▼
5. ANSI codes inserted: "\x1b[1;35m=\x1b[0m ..."
        │
        ▼
6. User presses Enter
        │
        ▼
7. StateMachine.handle_event(LineSubmitted)
        │
        ▼
8. analyze_line() finds: open_parens=2, close_parens=1
        │
        ▼
9. StateTransition::AwaitContinuation { indent: 2 }
        │
        ▼
10. Display prompt: "  ... "
        │
        ▼
11. User types: "  (* $x 2))"
        │
        ▼
12. accumulated = "(= (double $x)\n  (* $x 2))"
        │
        ▼
13. analyze_line() finds: open_parens=2, close_parens=2 → Complete!
        │
        ▼
14. StateTransition::StartEvaluation(full_input)
        │
        ▼
15. PatternHistory.add(full_input)
    - Parse to MettaValue
    - Store in MORK Space
    - Add to raw_commands
        │
        ▼
16. compile(full_input) → MettaState
        │
        ▼
17. eval(state) → Results
        │
        ▼
18. Display results
        │
        ▼
19. Return to Ready state
```

## Performance Characteristics

| Component | Complexity | Notes |
|-----------|-----------|-------|
| **Syntax Highlighting** | O(n) | Tree-Sitter incremental parsing |
| **Query Execution** | O(k) | k = number of captures |
| **State Machine** | O(1) | State transitions are constant time |
| **Line Analysis** | O(n) | Single pass through input |
| **Pattern History Add** | O(m) | m = expression size for parsing |
| **Pattern Search** | O(k*m) | k = matching commands, m = pattern complexity |
| **PathMap Trie** | O(m) | Prefix matching in trie |

## Memory Usage

```
Component               Estimate per Command
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Raw command string      50-200 bytes
Parsed MettaValue       100-500 bytes
MORK Space entry        200-1000 bytes
Tree-Sitter parse tree  (transient, freed after use)
━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━
Total per command:      ~350-1700 bytes
For 1000 commands:      ~1.7 MB
```

## Thread Safety

**MORK Space**: `Arc<Mutex<Space>>`
- Multiple readers via lock
- Single writer at a time

**Rustyline**: Single-threaded
- No concurrency within REPL loop

**State Machine**: Single-threaded
- State transitions are sequential

## Extension Points

### Future Enhancements

1. **Tab Completion**
   ```rust
   impl Completer for MettaHelper {
       fn complete(&self, line: &str, pos: usize) -> Result<Vec<String>> {
           // Complete function names, variables
           // Query environment for defined symbols
       }
   }
   ```

2. **Semantic Hints**
   ```rust
   impl Hinter for MettaHelper {
       fn hint(&self, line: &str, pos: usize) -> Option<String> {
           // Show matching closing parens
           // Show function signatures
           // Show variable types
       }
   }
   ```

3. **Syntax Validation**
   ```rust
   impl Validator for MettaHelper {
       fn validate(&self, ctx: &mut ValidationContext) -> Result<ValidationResult> {
           // Real-time syntax checking
           // Multi-line detection
           // Error hints
       }
   }
   ```

4. **History Analytics**
   ```rust
   impl PatternHistory {
       fn most_used_functions(&self) -> Vec<(String, usize)>;
       fn command_frequency_by_type(&self) -> HashMap<CommandType, usize>;
       fn suggest_based_on_context(&self, current: &str) -> Vec<String>;
   }
   ```

## Configuration

**File**: `~/.mettatron/repl.toml`

```toml
[repl]
syntax_highlighting = true
multiline_mode = true
auto_indent = true
indent_size = 2

[history]
max_size = 1000
persist = true
path = "~/.mettatron_history"

[colors]
scheme = "default"  # or "dark", "light", "monochrome"

[keybindings]
mode = "emacs"  # or "vi"
```

## Testing Strategy

### Unit Tests
- `query_highlighter.rs`: Test color mapping
- `state_machine.rs`: Test all state transitions
- `indenter.rs`: Test indent calculations
- `pattern_history.rs`: Test pattern matching

### Integration Tests
- Full REPL flow with mock input
- Multi-line expression handling
- Pattern search with various queries
- History persistence

### Manual Tests
- Visual verification of colors
- Interactive multi-line editing
- Performance with large history
- Edge cases (nested expressions, errors)

## Debugging

### Enable Logging

```rust
// Add to Cargo.toml
[dependencies]
env_logger = "0.11"

// In main.rs
env_logger::init();

// In REPL code
log::debug!("State transition: {:?}", transition);
log::info!("Pattern search: {} results", results.len());
```

### Debug Commands

```
?debug state        # Show current state machine state
?debug history      # Show history statistics
?debug colors       # Test color output
```

## Summary

The enhanced REPL architecture provides:

✅ **Modular Design**: Each component is independent and testable
✅ **Performance**: O(k) queries with PathMap, incremental Tree-Sitter parsing
✅ **Extensibility**: Clear extension points for future features
✅ **User Experience**: Syntax highlighting, multi-line, pattern search
✅ **Maintainability**: Declarative queries, state machine, clean interfaces
