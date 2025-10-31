# MeTTa REPL Guide

## Overview

The MeTTaTron REPL (Read-Eval-Print Loop) provides an interactive environment for experimenting with MeTTa code. The REPL has been enhanced with advanced features including syntax highlighting, multi-line input support, intelligent auto-indentation, powerful history search capabilities, tab completion, and inline hints. These features make the REPL a powerful tool for learning MeTTa, prototyping algorithms, and testing ideas interactively.

## Getting Started

### Starting the REPL

```bash
# Start the enhanced REPL (production)
./target/release/mettatron --repl

# Or during development
cargo run -- --repl

# Alternative: Run the backend interactive example
cargo run --example backend_interactive
```

The REPL will display a welcome message and a colorized prompt:

```metta
=== MeTTa Backend REPL ===
Enter MeTTa expressions. Type 'exit' to quit.

metta[1]>
```

**Colorized Prompt:**
The prompt is colorized for better visual distinction:
- **metta** - Cyan
- **[line number]** - Bright white
- **>** - Magenta

### Basic Usage

At the prompt, you can:
- Type MeTTa expressions and press Enter to evaluate them
- Define rules that persist throughout the session
- Use multi-line input for complex expressions
- Access command history with arrow keys
- Exit with `Ctrl-D`, `exit`, or `quit`

**Simple Example:**
```metta
metta[1]> (+ 1 2)
[3]

metta[2]> (* 5 6)
[30]
```

## MeTTa Language Basics

### 1. Rule Definition

Define pattern matching rules with `=`:

```metta
metta[1]> (= (f) 42)
Nil

metta[2]> (= (double $x) (* $x 2))
Nil

metta[3]> (= (factorial 0) 1)
Nil

metta[4]> (= (factorial $n) (* $n (factorial (- $n 1))))
Nil
```

**Note**: Rule definitions return `Nil` to indicate the rule was added to the environment successfully.

### 2. Evaluation

Evaluate expressions with the `!` prefix operator:

```metta
metta[5]> !(f)
Long(42)

metta[6]> !(double 21)
Long(42)

metta[7]> !(factorial 5)
Long(120)
```

**Syntax**: `!(expr)` is equivalent to `(! expr)` - the prefix operator automatically wraps the following expression:
- `!(f)` → parsed as `(! (f))`
- `!(double 5)` → parsed as `(! (double 5))`

### 3. Direct Evaluation

Expressions without `!` are also evaluated, but rules defined with `=` are not automatically applied:

```metta
metta[8]> (+ 10 5)
Long(15)

metta[9]> (* 3 4)
Long(12)
```

### 4. Pattern Matching with Variables

Variables start with `$`, `&`, or `'`:

```metta
metta[10]> (= (abs $x) (if (< $x 0) (- 0 $x) $x))
Nil

metta[11]> !(abs -5)
Long(5)

metta[12]> !(abs 5)
Long(5)
```

**Variable Prefixes:**
- `$x` - Standard variable
- `&y` - Reference variable (same matching behavior)
- `'z` - Quote variable (same matching behavior)

**Wildcard:**
- `_` - Matches anything without binding

### 5. Control Flow

The `if` special form provides conditional evaluation with lazy branches:

```metta
metta[13]> !(if (< 5 10) "less" "greater")
String("less")

metta[14]> (= (safe-div $x $y)
...>   (if (== $y 0)
...>     (error "div by zero" $y)
...>     (/ $x $y)))
Nil

metta[15]> !(safe-div 10 2)
Long(5)

metta[16]> !(safe-div 10 0)
Error("div by zero", Long(0))
```

### 6. Quote and Eval

Prevent or force evaluation:

```metta
metta[17]> (quote (+ 1 2))
SExpr([Atom("add"), Long(1), Long(2)])

metta[18]> (+ 1 2)
Long(3)

metta[19]> (eval (quote (+ 1 2)))
Long(3)
```

### 7. Error Handling (Reduction Prevention)

Control error propagation and recovery:

```metta
metta[20]> (catch (error "fail" 0) 42)
Long(42)

metta[21]> (catch (+ 1 2) "default")
Long(3)

metta[22]> (is-error (error "test" 0))
Bool(true)

metta[23]> (is-error 42)
Bool(false)
```

**Error Operations:**
- `catch` - Prevents error propagation by providing a default value
- `is-error` - Checks if a value is an error for conditional logic
- `error` - Creates an error value explicitly

### Special Forms Reference

| Form | Description | Example |
|------|-------------|---------|
| `=` | Define rule | `(= (f $x) (* $x 2))` |
| `!` | Force evaluation | `!(f 5)` |
| `if` | Conditional | `(if cond then else)` |
| `quote` | Prevent evaluation | `(quote expr)` |
| `eval` | Force evaluation | `(eval (quote (+ 1 2)))` |
| `error` | Create error | `(error "msg" details)` |
| `catch` | Error recovery | `(catch expr default)` |
| `is-error` | Error check | `(is-error expr)` |

### Built-in Operations

**Arithmetic:**
- `+` (add), `-` (sub), `*` (mul), `/` (div), `%` (modulo)

```metta
metta> (+ 1 2)
[3]

metta> (* (+ 2 3) 4)
[20]
```

**Comparison:**
- `<` (lt), `<=` (lte), `>` (gt), `>=` (gte), `==` (eq), `!=` (neq)

```metta
metta> (< 5 10)
[true]

metta> (== 42 42)
[true]
```

### Data Types

**Ground Types:**
- `Bool` - `true`, `false`
- `Long` - `42`, `-10`
- `String` - `"hello world"`
- `URI` - `` `https://example.com` ``
- `Nil` - Represents no value

**Literals:**
```metta
metta> true
[true]

metta> 42
[42]

metta> "hello"
["hello"]

metta> `uri`
[`uri`]
```

## Enhanced REPL Features

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

metta[2]> (= (fibonacci $n)
...>   (if (< $n 2)
...>     $n
...>     (+ (fibonacci (- $n 1))
...>        (fibonacci (- $n 2)))))

metta[3]> !(fibonacci 8)
[21]
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
- **History file** - Stored at platform-specific location
- **Pattern-based search** - Search by substring, prefix, regex, or function name

**Navigation:**
- `↑` or `Ctrl-P` - Previous command
- `↓` or `Ctrl-N` - Next command
- `Ctrl-R` - Search history (then type to filter)
- `Home` / `Ctrl-A` - Beginning of line
- `End` / `Ctrl-E` - End of line
- `→` - Accept inline hint

**History File Location:**
- **Linux/macOS**: `~/.config/mettatron/history.txt`
- **Windows**: `%APPDATA%\mettatron\history.txt`
- **Fallback**: `.mettatron_history` in current directory

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

## Advanced Usage

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

### Keyboard Shortcuts Summary

| Action | Keys | Description |
|--------|------|-------------|
| **Navigation** |
| Move left/right | `←` / `→` | Move cursor by character |
| Move by word | `Alt-B` / `Alt-F` | Move cursor by word |
| Start of line | `Home` or `Ctrl-A` | Jump to beginning |
| End of line | `End` or `Ctrl-E` | Jump to end |
| **Editing** |
| Delete char | `Backspace` / `Delete` | Remove characters |
| Kill to end | `Ctrl-K` | Delete to end of line |
| Kill to start | `Ctrl-U` | Delete to start of line |
| Kill word | `Ctrl-W` | Delete previous word |
| **History** |
| Previous | `↑` or `Ctrl-P` | Navigate to previous command |
| Next | `↓` or `Ctrl-N` | Navigate to next command |
| Search | `Ctrl-R` | Reverse search history |
| Accept hint | `→` (at end) | Accept inline suggestion |
| **Completion** |
| Complete | `Tab` | Show/cycle completions |
| **Control** |
| Cancel | `Ctrl-C` | Clear current line |
| Exit | `Ctrl-D` | Exit REPL (empty line) |
| Exit | `exit` or `quit` | Exit REPL |

### State Machine Architecture

The REPL uses a formal state machine to manage input flow:

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

This state machine ensures smooth transitions between different REPL states and graceful error handling.

### Tips and Best Practices

1. **Rule definitions return `Nil`**: This is expected and indicates success.

2. **Use `!` for explicit evaluation**: When you want to apply rules, use the `!` operator.

3. **Environment is persistent**: Rules you define stay in the environment for the entire session.

4. **Prefix syntax**: `!(expr)` is more concise than `(! expr)` and they're equivalent.

5. **Errors propagate**: If an error occurs in a subexpression, the entire evaluation returns the error.

6. **Multi-line editing**: For complex functions, use multi-line input with proper indentation for readability.

7. **History search**: Use `Ctrl-R` to quickly find and reuse previous commands.

8. **Tab completion**: Save time by using Tab completion for function names and keywords.

## Technical Details

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

For detailed architecture information, see [docs/design/REPL_ARCHITECTURE.md](/home/dylon/Workspace/f1r3fly.io/MeTTa-Compiler/docs/design/REPL_ARCHITECTURE.md).

### Dependencies

- **rustyline** (14.0) - Line editing and history
- **tree-sitter** (0.25) - Parsing for syntax highlighting
- **regex** (1.11) - Regular expression search
- **dirs** (5.0) - Platform-independent config directory lookup

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

## Examples

### Basic Arithmetic

```metta
metta[1]> (+ 1 2)
[3]

metta[2]> (* (+ 2 3) (- 10 4))
[30]

metta[3]> (/ 100 5)
[20]
```

### Defining Rules

```metta
metta[1]> (= (double $x) (* 2 $x))
Nil

metta[2]> !(double 21)
[42]

metta[3]> (= (square $x) (* $x $x))
Nil

metta[4]> !(square 7)
[49]
```

### Multi-Line Functions

```metta
metta[1]> (= (fibonacci $n)
...>   (if (< $n 2)
...>     $n
...>     (+ (fibonacci (- $n 1))
...>        (fibonacci (- $n 2)))))
Nil

metta[2]> !(fibonacci 8)
[21]

metta[3]> (= (factorial $n)
...>   (if (== $n 0)
...>     1
...>     (* $n (factorial (- $n 1)))))
Nil

metta[4]> !(factorial 5)
[120]
```

### Using Variables

```metta
metta[1]> (= $x 42)
Nil

metta[2]> (= $y (+ $x 8))
Nil

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
Nil

metta[2]> !(classify -5)
["negative"]

metta[3]> !(classify 0)
["zero"]

metta[4]> !(classify 42)
["positive"]
```

### Pattern Matching

```metta
metta[1]> (= (factorial 0) 1)
Nil

metta[2]> (= (factorial $n) (* $n (factorial (- $n 1))))
Nil

metta[3]> !(factorial 5)
[120]
```

### Error Handling

```metta
metta[1]> (= (safe-div $x $y)
...>   (if (== $y 0)
...>     (error "div by zero" $y)
...>     (/ $x $y)))
Nil

metta[2]> !(safe-div 10 2)
[5]

metta[3]> !(safe-div 10 0)
Error("div by zero", Long(0))

metta[4]> (catch (safe-div 10 0) -1)
[-1]
```

### Complete Example Session

```metta
=== MeTTa Backend REPL ===
Enter MeTTa expressions. Type 'exit' to quit.

# Define a simple function
metta[1]> (= (f) 42)
Nil

# Evaluate it
metta[2]> !(f)
Long(42)

# Define a function with parameters
metta[3]> (= (double $x) (* $x 2))
Nil

# Evaluate with argument
metta[4]> !(double 21)
Long(42)

# Define with conditionals
metta[5]> (= (abs $x) (if (< $x 0) (- 0 $x) $x))
Nil

# Test both branches
metta[6]> !(abs -5)
Long(5)

metta[7]> !(abs 5)
Long(5)

# Direct arithmetic (no rules needed)
metta[8]> (+ 10 (* 2 3))
Long(16)

# Exit
metta[9]> exit
Goodbye!
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

### Common Syntax Errors

**Problem**: `!(f)` returns unexpected results

**Solution**: Update to the latest version. The parser now correctly handles `!(f)` as a single expression `(! (f))`.

**Problem**: Defined rule doesn't work

**Solution**: Make sure you're using `!` to evaluate: `!(f)` not just `(f)`.

**Problem**: "Error: Unexpected character"

**Solution**: Check your syntax. Common issues:
- Unmatched parentheses
- Special characters that aren't supported
- Use `(= lhs rhs)` for rules, not just `= lhs rhs`

### Terminal Issues

**Problem**: Colors appear as escape codes

**Solution**: Your terminal may not support ANSI colors. Set `TERM=xterm-256color` or use a modern terminal emulator.

**Problem**: Backspace doesn't work correctly

**Solution**: This is a terminal configuration issue. Try setting `stty erase ^H` or configure your terminal's backspace key.

## Future Enhancements

Planned features for future releases:

1. **Enhanced Completion System**
   - Variable completion from environment
   - Path completion for file operations
   - Context-aware function suggestions

2. **Advanced Inline Hints**
   - Display function signatures on hover
   - Type information display
   - Parameter hints for functions

3. **Advanced Search**
   - Full pattern matching with variables
   - Structural pattern queries (e.g., "all rule definitions")
   - Search results browser

4. **Customization**
   - Configurable key bindings
   - Custom color schemes
   - Adjustable indent width
   - Prompt customization

5. **Debugging Features**
   - Step-through evaluation
   - Breakpoints in expressions
   - Environment inspection

## See Also

- [REPL Architecture](../design/REPL_ARCHITECTURE.md) - Detailed technical architecture
- [MeTTa Language Specification](https://metta-lang.dev) - Official language documentation
- [Tree-Sitter Documentation](https://tree-sitter.github.io/tree-sitter/) - Parsing library
- [Rustyline Documentation](https://docs.rs/rustyline/) - Readline library
