# MeTTaTron Enhanced REPL Features

## Overview

The MeTTaTron REPL has been enhanced with advanced features including syntax highlighting, multi-line input support, intelligent auto-indentation, and powerful history search capabilities.

## Starting the REPL

```bash
# Start the enhanced REPL
./target/release/mettatron --repl

# Or during development
cargo run -- --repl
```

## Core Features

### 1. Syntax Highlighting

The REPL provides real-time syntax highlighting as you type, using Tree-Sitter queries for accurate token classification.

**Highlighted Elements:**
- **Comments** (gray): `;`, `//`, `/* */`
- **Strings** (green): `"hello world"`
- **Numbers** (yellow): `42`, `3.14`
- **Booleans** (yellow): `true`, `false`
- **Variables** (blue): `$x`, `&y`, `'z`
- **Operators** (magenta): `+`, `-`, `*`, `/`, `==`, `!=`
- **Special forms** (cyan): `if`, `match`, `case`, `quote`, `eval`
- **Keywords** (cyan): `=`, `->`, `:`, `!`
- **URIs** (green): `` `https://example.com` ``

**Example:**
```metta
metta[1]> (= (fib $n) (if (< $n 2) $n (+ (fib (- $n 1)) (fib (- $n 2)))))
metta[2]> !(fib 10)
```

**Colorized Prompt:**
The prompt itself is also colorized for better visual distinction:
- **metta** - Cyan
- **[line number]** - Bright white
- **>** - Magenta

**Colorized Output:**
Evaluation results are also syntax-highlighted using the same color scheme, making complex nested expressions easier to read.

**TTY Detection:**
Colors are automatically disabled when output is redirected to files or pipes, ensuring clean output for scripting.

### 2. Multi-Line Input Support

The REPL automatically detects incomplete expressions and continues accepting input until the expression is complete.

**Features:**
- **Automatic detection** of unclosed parentheses, braces, and strings
- **Continuation prompt** (same as main prompt) for additional lines
- **Smart validation** that distinguishes incomplete from invalid input
- **Ctrl-C** to cancel multi-line input and start fresh

**Example:**
```metta
metta[1]> (= (factorial $n)
...>   (if (== $n 0)
...>     1
...>     (* $n (factorial (- $n 1)))))
```

**Invalid Input Detection:**
```metta
metta[1]> (+ 1 2))
Error: Unexpected closing parenthesis ')'
```

### 3. Smart Indentation Support

The REPL includes a SmartIndenter component that calculates proper indentation based on syntax structure.

**Current Capabilities:**
- ✅ **Indentation calculation** - Determines correct indent level based on unclosed delimiters
- ✅ **Tree-Sitter parsing** - Uses syntax-aware parsing for accuracy
- ✅ **Comment-aware** - Ignores delimiters in comments
- ✅ **String-aware** - Ignores delimiters in strings
- ✅ **Configurable** - 2-space indentation per nesting level (adjustable)
- ✅ **API available** - `helper.calculate_indent(buffer)` for external use

**Limitation:**
- ⚠️ **Manual indentation required** - Due to rustyline's API, the REPL cannot auto-insert spaces into the input buffer
- The SmartIndenter is integrated but automatic insertion isn't supported by the underlying readline library

**Usage:**
Users should manually indent multi-line expressions. The indenter is available for:
- Future rustyline enhancements
- External tooling integration
- Editor plugins
- Custom REPL implementations

**Example** (manual indentation):
```metta
metta[1]> (let (
...>        ($x 5)      # User manually indents
...>        ($y 10))
...>      (+ $x $y))
```

### 4. Auto-Completion

Tab completion for MeTTa keywords, functions, and operators.

**What You Can Complete:**
- **Grounded functions**: `+`, `-`, `*`, `/`, `%`, `<`, `<=`, `>`, `>=`, `==`, `!=`
- **Special forms**: `if`, `match`, `case`, `let`, `let*`, `quote`, `eval`, `error`, `catch`
- **Type operations**: `:`, `get-type`, `check-type`
- **Control flow**: `=`, `!`, `->`

**Usage:**
```metta
metta[1]> (i<TAB>         # Completes to (if
metta[1]> (+<TAB>         # Shows: +, -, *, /, etc.
metta[1]> (mat<TAB>       # Completes to (match
```

**How It Works:**
- Press `Tab` to see completions
- Press `Tab` again to cycle through options
- Completions are context-aware and filter as you type

### 5. Inline Hints

Suggestions based on your command history appear as you type.

**Features:**
- **Auto-suggest** - Shows the rest of a previously used command
- **Recent first** - Prioritizes most recent matches
- **Dimmed display** - Hints appear in gray text
- **Accept hint** - Press `→` (right arrow) to accept

**Example:**
```metta
metta[1]> (+ 1 2)
[3]

metta[2]> (+          # Shows hint: " 1 2)" in gray
metta[2]> (+ 1 2)     # Press → to accept the hint
```

**Smart Matching:**
- Hints only appear when typing at end of line
- Matches commands that start with your current input
- Up to 100 most recent commands remembered

### 6. Command History

The REPL maintains persistent command history across sessions.

**Features:**
- **Up/Down arrows** - Navigate through command history
- **Ctrl-R** - Reverse search through history
- **History file** - Stored at `~/.config/mettatron/history.txt` (Linux/macOS) or `%APPDATA%\mettatron\history.txt` (Windows)
- **Pattern-based search** - Search by substring, prefix, regex, or function name

**Navigation:**
- `↑` - Previous command
- `↓` - Next command
- `Ctrl-R` - Search history (then type to filter)
- `Home` / `Ctrl-A` - Beginning of line
- `End` / `Ctrl-E` - End of line
- `→` - Accept inline hint

### 7. Line Editing

Standard readline-style editing is supported:

**Cursor Movement:**
- `←` / `→` - Move cursor left/right
- `Ctrl-A` - Beginning of line
- `Ctrl-E` - End of line
- `Alt-B` - Back one word
- `Alt-F` - Forward one word

**Editing:**
- `Backspace` - Delete previous character
- `Delete` - Delete character at cursor
- `Ctrl-K` - Kill to end of line
- `Ctrl-U` - Kill to beginning of line
- `Ctrl-W` - Kill previous word

**History:**
- `Ctrl-P` / `↑` - Previous history entry
- `Ctrl-N` / `↓` - Next history entry
- `Ctrl-R` - Reverse search

### 8. Control Keys

**Special Commands:**
- `Ctrl-C` - Cancel current input (clear line buffer)
- `Ctrl-D` - Exit REPL (at empty prompt)
- `exit` or `quit` - Exit REPL

## Advanced Features

### Pattern-Based History Search

The REPL maintains a separate pattern history that stores both the source text and the parsed AST of each command. This enables advanced search capabilities.

**Search Modes:**

1. **Substring Search** (default with Ctrl-R)
   ```
   Search for commands containing "fib"
   ```

2. **Prefix Search**
   ```
   Find all commands starting with "(="
   ```

3. **Function Search**
   ```
   Find all commands that use the '+' function
   ```

4. **Regex Search**
   ```
   Search with regular expressions like "\(= \(\w+ \$.*"
   ```

### State Machine Architecture

The REPL uses a formal state machine to manage input:

**States:**
- **Ready** - Waiting for new input
- **Continuation** - Accumulating multi-line input
- **Evaluating** - Processing complete expression
- **DisplayingResults** - Showing evaluation results
- **Error** - Handling invalid input

**Events:**
- **LineSubmitted** - User pressed Enter
- **Interrupted** - User pressed Ctrl-C
- **Eof** - User pressed Ctrl-D
- **EvaluationComplete** - Expression evaluated successfully
- **EvaluationFailed** - Evaluation error occurred
- **ResultsDisplayed** - Output shown to user

## Implementation Details

### Architecture

The enhanced REPL consists of several integrated components:

1. **QueryHighlighter** (`src/repl/query_highlighter.rs`)
   - Uses Tree-Sitter highlight queries
   - Applies ANSI color codes in real-time
   - O(k) complexity based on captures, not O(n) node traversal

2. **ReplStateMachine** (`src/repl/state_machine.rs`)
   - Manages REPL state transitions
   - Detects expression completeness
   - Handles error states gracefully

3. **SmartIndenter** (`src/repl/indenter.rs`)
   - Calculates indentation based on unclosed delimiters
   - Comment and string aware
   - Generates continuation prompts with proper spacing

4. **PatternHistory** (`src/repl/pattern_history.rs`)
   - Stores commands with parsed AST
   - Supports multiple search modes
   - Fixed-size circular buffer (default 1000 entries)

5. **HistorySearchInterface** (`src/repl/history_search.rs`)
   - Interactive search with position tracking
   - Next/previous navigation with wrap-around
   - Multiple search modes

6. **MettaHelper** (`src/repl/helper.rs`)
   - Integrates all components with rustyline
   - Implements Highlighter, Validator, Completer, Hinter traits

### Dependencies

- **rustyline** (14.0) - Line editing and history
- **tree-sitter** (0.25) - Parsing for syntax highlighting
- **regex** (1.11) - Regular expression search
- **dirs** (5.0) - Platform-independent config directory lookup

## Examples

### Basic Arithmetic
```metta
metta[1]> (+ 1 2)
[3]

metta[2]> (* (+ 2 3) (- 10 4))
[30]
```

### Defining Rules
```metta
metta[1]> (= (double $x) (* 2 $x))

metta[2]> !(double 21)
[42]
```

### Multi-Line Functions
```metta
metta[1]> (= (fibonacci $n)
...>   (if (< $n 2)
...>     $n
...>     (+ (fibonacci (- $n 1))
...>        (fibonacci (- $n 2)))))

metta[2]> !(fibonacci 8)
[21]
```

### Using Variables
```metta
metta[1]> (= $x 42)

metta[2]> (= $y (+ $x 8))

metta[3]> !(* $x $y)
[2100]
```

### Conditional Logic
```metta
metta[1]> (= (classify $n)
...>   (if (< $n 0)
...>     "negative"
...>     (if (== $n 0)
...>       "zero"
...>       "positive")))

metta[2]> !(classify -5)
["negative"]
```

## Troubleshooting

### Syntax Highlighting Not Working

If syntax highlighting doesn't appear:
1. Ensure your terminal supports ANSI color codes
2. Check that `TERM` environment variable is set (e.g., `xterm-256color`)
3. The REPL will fall back to basic mode if initialization fails

### History Not Persisting

History is saved when you exit normally (Ctrl-D or `exit`/`quit`). If you force-quit (Ctrl-C at the prompt level), history may not be saved.

**History location:**
- Linux/macOS: `~/.config/mettatron/history.txt`
- Windows: `%APPDATA%\mettatron\history.txt`
- Fallback: `.mettatron_history` in current directory

### Multi-Line Input Issues

If multi-line detection isn't working:
- Check for balanced delimiters (parentheses, braces)
- Ensure strings are properly closed with `"`
- Use Ctrl-C to cancel and start over

## Future Enhancements

Planned features for future releases:

1. **Completion System**
   - Function name completion
   - Variable completion from environment
   - Path completion for file operations

2. **Inline Hints**
   - Show recent history matches while typing
   - Display function signatures on hover
   - Type information display

3. **Advanced Search**
   - Full pattern matching with variables
   - Structural pattern queries (e.g., "all rule definitions")
   - Search results browser

4. **Customization**
   - Configurable key bindings
   - Custom color schemes
   - Adjustable indent width
   - Prompt customization

## Technical Reference

### Query-Based Highlighting

The highlighter uses Tree-Sitter's query system for accurate syntax highlighting:

```scheme
; From tree-sitter-metta/queries/highlights.scm
(comment) @comment
(string) @string
(number) @number
(variable) @variable
(operator) @operator
```

### Completeness Detection Algorithm

The state machine uses a streaming parser to detect expression completeness:

1. **Track delimiter depth** - Count open/close parens and braces
2. **String state tracking** - Monitor quote and escape sequences
3. **Comment awareness** - Ignore delimiters in comments
4. **Negative depth detection** - Catch extra closing delimiters immediately

### Performance Characteristics

- **Syntax highlighting**: O(k) where k = number of query captures
- **Completeness checking**: O(n) single-pass scan
- **History search**: O(h) where h = history size
- **Pattern matching**: O(h × m) where m = pattern complexity

## See Also

- [MeTTa Language Specification](https://metta-lang.dev)
- [Tree-Sitter Documentation](https://tree-sitter.github.io/tree-sitter/)
- [Rustyline Documentation](https://docs.rs/rustyline/)
