# MeTTa Backend REPL Usage Guide

## Starting the REPL

```bash
cargo run --example backend_interactive
```

## MeTTa Syntax

### 1. Rule Definition

Define pattern matching rules with `=`:

```metta
metta[1]> (= (f) 42)
Nil

metta[2]> (= (double $x) (* $x 2))
Nil
```

**Note**: Rule definitions return `Nil` to indicate the rule was added to the environment.

### 2. Evaluation

Evaluate expressions with the `!` prefix operator:

```metta
metta[3]> !(f)
Long(42)

metta[4]> !(double 21)
Long(42)
```

**Syntax**: `!(expr)` is equivalent to `(! expr)`

The prefix operator `!` automatically wraps the following expression:
- `!(f)` → parsed as `(! (f))`
- `!(double 5)` → parsed as `(! (double 5))`

### 3. Direct Evaluation

Expressions without `!` are also evaluated, but rules defined with `=` are not automatically applied:

```metta
metta[5]> (+ 10 5)
Long(15)

metta[6]> (* 3 4)
Long(12)
```

### 4. Pattern Matching with Variables

```metta
metta[7]> (= (factorial 0) 1)
Nil

metta[8]> (= (factorial $n) (* $n (factorial (- $n 1))))
Nil

metta[9]> !(factorial 5)
Long(120)
```

### 5. Control Flow

```metta
metta[10]> !(if (< 5 10) "less" "greater")
String("less")

metta[11]> (= (safe-div $x $y) (if (== $y 0) (error "div by zero" $y) (div $x $y)))
Nil

metta[12]> !(safe-div 10 2)
Long(5)

metta[13]> !(safe-div 10 0)
Error("div by zero", Long(0))
```

### 6. Quote (Prevent Evaluation)

```metta
metta[14]> (quote (+ 1 2))
SExpr([Atom("add"), Long(1), Long(2)])

metta[15]> (+ 1 2)
Long(3)
```

### 7. Reduction Prevention (Error Recovery)

```metta
metta[16]> (catch (error "fail" 0) 42)
Long(42)

metta[17]> (catch (+ 1 2) "default")
Long(3)

metta[18]> (is-error (error "test" 0))
Bool(true)

metta[19]> (is-error 42)
Bool(false)

metta[20]> (eval (quote (+ 1 2)))
Long(3)
```

**Reduction Prevention**: These features allow you to control evaluation and handle errors gracefully:
- `catch` - Prevents error propagation by providing a default value
- `is-error` - Checks if a value is an error for conditional logic
- `eval` - Forces evaluation of quoted expressions

## Complete Example Session

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

## Special Forms

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

## Built-in Operations

### Arithmetic
- `+` (add), `-` (sub), `*` (mul), `/` (div)

Example:
```metta
metta> (+ 1 2)
Long(3)

metta> (* (+ 2 3) 4)
Long(20)
```

### Comparison
- `<` (lt), `<=` (lte), `>` (gt), `==` (eq)

Example:
```metta
metta> (< 5 10)
Bool(true)

metta> (== 42 42)
Bool(true)
```

## Variables

Variables start with `$`, `&`, or `'`:
- `$x` - Standard variable
- `&y` - Reference variable (same matching behavior)
- `'z` - Quote variable (same matching behavior)

Wildcard:
- `_` - Matches anything without binding

## Tips

1. **Rule definitions return `Nil`**: This is expected and indicates success.

2. **Use `!` for explicit evaluation**: When you want to apply rules, use the `!` operator.

3. **Environment is persistent**: Rules you define stay in the environment for the entire session.

4. **Prefix syntax**: `!(expr)` is more concise than `(! expr)` and they're equivalent.

5. **Errors propagate**: If an error occurs in a subexpression, the entire evaluation returns the error.

## Troubleshooting

**Problem**: `!(f)` returns two results: `Atom("!")` and `SExpr([Atom("f")])`

**Solution**: Update to the latest version. This was a parser issue that's now fixed. The parser now correctly handles `!(f)` as a single expression `(! (f))`.

**Problem**: Defined rule doesn't work

**Solution**: Make sure you're using `!` to evaluate: `!(f)` not just `(f)`.

**Problem**: "Error: Unexpected character"

**Solution**: Check your syntax. Common issues:
- Unmatched parentheses
- Special characters that aren't supported
- Use `(= lhs rhs)` for rules, not just `= lhs rhs`

## Exiting

Type `exit` or `quit` to exit the REPL.

```metta
metta[N]> exit
Goodbye!
```
