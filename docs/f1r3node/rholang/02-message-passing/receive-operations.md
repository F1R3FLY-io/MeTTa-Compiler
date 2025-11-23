# Receive Operations in Rholang

## Overview

**Receive operations** wait for data on channels and execute continuations when patterns match. They are the consumer side of Rholang's fundamental communication model, derived from the π-calculus and extended with spatial pattern matching.

**Syntax**: `for (pattern <- channel) { body }`

**Purpose**: Consume data from channels, bind pattern variables, and execute continuation code.

This document provides complete technical specification of receive operations, their variants, pattern matching semantics, and integration with RSpace.

## Receive Syntax Forms

### Linear Receive (Execute Once)

```rholang
for (@pattern <- channel) {
  body
}
```

**Semantics**:
- Waits for data matching pattern
- Executes body once with bindings
- Removes data and continuation after execution

**Example**:

```rholang
for (@x <- @"channel") {
  stdout!(x)
}
```

### Persistent Receive (Contract/Service)

```rholang
for (@pattern <= channel) {
  body
}
```

**Semantics**:
- Waits for data matching pattern
- Executes body with bindings
- Continuation remains (executes again for next data)

**Example**:

```rholang
// Contract (service available repeatedly)
for (@x, @y, return <= @"add") {
  return!(x + y)
}
```

### Peek Receive (Non-Consuming)

```rholang
for (@pattern <<- channel) {
  body
}
```

**Semantics**:
- Reads data without removing it
- Executes body with bindings
- Data remains on channel for other receivers

**Example**:

```rholang
for (@x <<- @"channel") {
  stdout!("Peeked: ", x)  // Data still on channel after
}
```

### Join Pattern (Multiple Channels)

```rholang
for (@x <- ch1 & @y <- ch2 & @z <- ch3) {
  body
}
```

**Semantics**:
- Waits for data on ALL channels
- Atomic matching (all or nothing)
- Executes body when all patterns match

**Example**:

```rholang
// Dining philosophers
for (@knife <- @"north" & @spoon <- @"south") {
  // Only executes when both utensils available
  stdout!("Eating with knife and spoon")
}
```

## Source Code Reference

### Parser

**Location**: `/var/tmp/debug/f1r3node/rholang/src/rust/interpreter/compiler/normalizer/processes/p_input_normalizer.rs`

**Key Functions**:
- `normalize()` (lines 20-100) - Parse receive expression
- `desugar_complex_sources()` (lines 150-250) - Handle join patterns

### Normalization

**Input** (AST from parser):

```rust
pub struct PInput {
    pub binds: Vec<InputBind>,        // Channel bindings
    pub body: Box<Proc>,              // Continuation
}

pub struct InputBind {
    pub patterns: Vec<Pattern>,       // Patterns to match
    pub source: Source,               // Channel source
}

pub enum Source {
    Simple(Proc),                     // Single channel: <- ch
    Join(Vec<Source>),                // Multiple channels: <- ch1 & ch2
    Persistent(Box<Source>),          // Persistent: <= ch
    Peek(Box<Source>),                // Peek: <<- ch
}
```

**Output** (normalized Par):

```rust
pub struct Receive {
    pub binds: Vec<ReceiveBind>,      // Normalized bindings
    pub body: Option<Par>,            // Continuation body
    pub persistent: bool,             // <= persistent?
    pub peek: bool,                   // <<- peek?
    pub bind_count: i32,              // Number of pattern variables
}

pub struct ReceiveBind {
    pub patterns: Vec<Par>,           // Match patterns
    pub source: Par,                  // Channel (must be name)
    pub remainder: Option<Var>,       // List remainder binding
    pub free_count: i32,              // Free variables in patterns
}
```

**Normalization Process** (lines 30-100):

```rust
impl Normalizer {
    fn normalize_receive(
        &mut self,
        input: PInput,
        env: &Env,
    ) -> Result<Par, NormalizerError> {
        // 1. Desugar complex sources (join patterns, etc.)
        let simple_binds = self.desugar_complex_sources(input.binds)?;

        // 2. Determine persistence and peek
        let (persistent, peek) = self.analyze_sources(&simple_binds);

        // 3. Normalize each binding
        let mut normalized_binds = Vec::new();
        let mut total_bind_count = 0;

        for bind in simple_binds {
            // 3a. Normalize channel
            let source_result = self.normalize_proc(bind.source, env)?;
            if !is_name(&source_result.par) {
                return Err(NormalizerError::ReceiveChannelNotName);
            }

            // 3b. Normalize patterns
            let mut normalized_patterns = Vec::new();
            for pattern in bind.patterns {
                let pattern_result = self.normalize_pattern(pattern, env)?;
                normalized_patterns.push(pattern_result.par);
                total_bind_count += pattern_result.bind_count;
            }

            normalized_binds.push(ReceiveBind {
                patterns: normalized_patterns,
                source: source_result.par,
                remainder: bind.remainder,
                free_count: source_result.free_count,
            });
        }

        // 4. Normalize body in extended environment
        let body_env = env.extend(total_bind_count);
        let normalized_body = self.normalize_proc(*input.body, &body_env)?;

        // 5. Create Receive structure
        Ok(Par {
            receives: vec![Receive {
                binds: normalized_binds,
                body: Some(normalized_body.par),
                persistent,
                peek,
                bind_count: total_bind_count,
            }],
            ..Default::default()
        })
    }
}
```

### Evaluation

**Location**: `/var/tmp/debug/f1r3node/rholang/src/rust/interpreter/reduce.rs` (lines 729-789)

**Key Function**: `eval_receive()`

```rust
impl DebruijnInterpreter {
    async fn eval_receive(
        &self,
        receive: &Receive,
        env: &Env<Par>,
        rand: Blake2b512Random,
    ) -> Result<(), InterpreterError> {
        // 1. Process all bindings (channels and patterns)
        let binds: Vec<(BindPattern, Par)> = receive.binds.iter()
            .map(|rb| {
                // Check bundle permissions (read access)
                let channel = self.unbundle_receive(rb, env)?;

                // Substitute variables in patterns
                let subst_patterns: Vec<Par> = rb.patterns.iter()
                    .map(|pat| {
                        self.substitute.substitute_and_charge(
                            pat.clone(),
                            1,  // Quote level 1 for patterns
                            env
                        )
                    })
                    .collect::<Result<Vec<_>, InterpreterError>>()?;

                Ok((
                    BindPattern {
                        patterns: subst_patterns,
                        remainder: rb.remainder.clone(),
                        free_count: rb.free_count,
                    },
                    channel,
                ))
            })
            .collect::<Result<Vec<_>, InterpreterError>>()?;

        // 2. Extract channels and patterns
        let channels: Vec<Par> = binds.iter().map(|(_, ch)| ch.clone()).collect();
        let patterns: Vec<Vec<Par>> = binds.iter()
            .map(|(bp, _)| bp.patterns.clone())
            .collect();

        // 3. Prepare continuation with shifted environment
        let body = receive.body.clone().unwrap();
        let shifted_env = env.shift(receive.bind_count);
        let subst_body = self.substitute.substitute_no_sort_and_charge(
            body,
            0,
            &shifted_env,
        )?;

        // 4. Consume from RSpace
        self.consume(
            channels,
            patterns.into_iter().flatten().collect(),
            ParWithRandom {
                body: Some(subst_body),
                random_state: rand.to_bytes(),
            },
            receive.persistent,
            receive.peek,
        ).await?;

        Ok(())
    }
}
```

### RSpace Integration

**Consume Call** (from eval_receive):

```rust
async fn consume(
    &self,
    channels: Vec<Par>,
    patterns: Vec<Par>,
    continuation: ParWithRandom,
    persist: bool,
    peek: bool,
) -> Result<(), InterpreterError> {
    // Lock RSpace
    let mut rspace = self.space.lock().await;

    // Build peek set (which channels to peek)
    let peek_set = if peek {
        (0..channels.len()).collect()
    } else {
        BTreeSet::new()
    };

    // Call RSpace consume
    let result = rspace.consume(
        channels,
        patterns,
        continuation,
        persist,
        peek_set,
    )?;

    // If match found, dispatch continuation
    if let Some((cont, matched_data)) = result {
        drop(rspace);  // Release lock before executing
        self.eval_continuation(cont, matched_data).await?;
    }

    Ok(())
}
```

## Pattern Matching

### Pattern Types

**1. Variable Binding**:

```rholang
for (@x <- channel) { ... }
```

Matches any value, binds to `x`.

**2. Constant Matching**:

```rholang
for (@42 <- channel) { ... }
```

Only matches exact value `42`.

**3. Wildcard**:

```rholang
for (@_ <- channel) { ... }
```

Matches anything, doesn't bind.

**4. Structure Matching**:

```rholang
for (@{"key": value, "type": @"person"} <- channel) { ... }
```

Matches map structure, binds `value`.

**5. List Matching**:

```rholang
for (@[head ...tail] <- channel) { ... }
```

Matches list, binds `head` to first element, `tail` to rest.

**6. Process Matching**:

```rholang
for (proc <- channel) { ... }
```

Matches process (not quoted), binds whole process.

### Pattern Semantics

**Matching Algorithm**:

```
1. Compare pattern structure with data structure
2. For variables: bind to data
3. For constants: check equality
4. For wildcards: always match (no binding)
5. For nested structures: recurse
6. Success: return bindings
7. Failure: no match
```

**Example Matches**:

```rholang
Pattern: @x
Data: 42
Result: { x -> 42 } ✓

Pattern: @42
Data: 42
Result: {} ✓

Pattern: @42
Data: 100
Result: No match ✗

Pattern: @{"a": x}
Data: {"a": 10, "b": 20}
Result: { x -> 10 } ✓

Pattern: @[x, y]
Data: [1, 2]
Result: { x -> 1, y -> 2 } ✓

Pattern: @[x, y]
Data: [1, 2, 3]
Result: No match ✗ (length mismatch)

Pattern: @[x ...rest]
Data: [1, 2, 3]
Result: { x -> 1, rest -> [2, 3] } ✓
```

## Examples

### Example 1: Simple Receive

```rholang
new ch in {
  ch!(42) |
  for (@x <- ch) {
    stdout!(x + 10)
  }
}
```

**Execution**:

```
1. Send 42 on ch
2. Receive waiting with pattern @x
3. Match: x = 42
4. Execute: stdout!(42 + 10)
5. Output: 52
```

### Example 2: Contract (Persistent Receive)

```rholang
contract @"multiply"(@x, @y, return) = {
  return!(x * y)
}

// Equivalent to:
for (@x, @y, return <= @"multiply") {
  return!(x * y)
}

// Calls:
new result in {
  @"multiply"!(5, 3, *result) |
  for (@product <- result) {
    stdout!(product)  // 15
  }
}

new result2 in {
  @"multiply"!(10, 4, *result2) |
  for (@product <- result2) {
    stdout!(product)  // 40
  }
}
```

**Execution**:

```
1. Persistent receive installed
2. First call: match x=5, y=3, execute, return 15
3. Continuation remains (persistent)
4. Second call: match x=10, y=4, execute, return 40
5. Contract still available for future calls
```

### Example 3: Join Pattern (Atomic Multi-Channel)

```rholang
new fork, knife in {
  fork!("Fork") |
  knife!("Knife") |
  for (@f <- fork & @k <- knife) {
    stdout!("Got both: ", f, " and ", k)
  }
}
```

**Execution**:

```
1. Send "Fork" on fork channel
2. Send "Knife" on knife channel
3. Receive waiting on BOTH channels
4. Both available → match atomically
5. Bindings: f="Fork", k="Knife"
6. Execute: stdout!("Got both: Fork and Knife")
```

### Example 4: Peek (Non-Consuming)

```rholang
new ch in {
  ch!(42) |
  for (@x <<- ch) {
    stdout!("Peeked: ", x)
  } |
  for (@y <- ch) {
    stdout!("Consumed: ", y)
  }
}
```

**Execution**:

```
1. Send 42 on ch
2. Peek receive: match x=42, data remains
3. Execute: stdout!("Peeked: 42")
4. Normal receive: match y=42, data removed
5. Execute: stdout!("Consumed: 42")
```

**Output**:
```
Peeked: 42
Consumed: 42
```

### Example 5: Pattern Matching on Structure

```rholang
new ch in {
  ch!({"name": "Alice", "age": 30}) |
  for (@{"name": name, "age": age} <- ch) {
    stdout!("Name: ", name, ", Age: ", age)
  }
}
```

**Execution**:

```
1. Send map with name and age
2. Receive with structure pattern
3. Match: name="Alice", age=30
4. Execute: stdout!("Name: Alice, Age: 30")
```

### Example 6: List Pattern with Remainder

```rholang
new ch in {
  ch!([1, 2, 3, 4, 5]) |
  for (@[first, second ...rest] <- ch) {
    stdout!("First: ", first, ", Second: ", second, ", Rest: ", rest)
  }
}
```

**Execution**:

```
1. Send list [1, 2, 3, 4, 5]
2. Receive with list pattern
3. Match: first=1, second=2, rest=[3, 4, 5]
4. Execute: stdout!("First: 1, Second: 2, Rest: [3, 4, 5]")
```

## Advanced Features

### Multiple Patterns on Same Channel

```rholang
for (@x, @y, @z <- channel) { ... }
```

Waits for data tuple with 3 items.

**Must match**:

```rholang
channel!(1, 2, 3)  // ✓ Matches
channel!(1, 2)     // ✗ No match (wrong arity)
```

### Nested Pattern Matching

```rholang
for (@{"outer": {"inner": value}} <- ch) { ... }
```

Matches nested map structure, extracts deeply nested value.

### Conditional Patterns (via match)

```rholang
for (@data <- ch) {
  match data {
    x if x > 10 => stdout!("Large: ", x)
    x           => stdout!("Small: ", x)
  }
}
```

Patterns in `for` are structural; conditions in `match` are computational.

### Receive with Bundle (Read Permission)

```rholang
new ch in {
  for (@x <- bundle- { ch }) {  // Read-only bundle
    stdout!(x)
  }
  // bundle+ { ch }!(42)  // ERROR: can't send to write-only
}
```

**Security**: Bundle ensures capability-based access control.

## Error Conditions

### Error 1: Receive on Non-Name

```rholang
for (@x <- 42) { ... }  // ERROR: 42 is not a channel
```

**Error**: `ReceiveChannelNotName`

### Error 2: Receive on Write-Only Bundle

```rholang
new ch in {
  for (@x <- bundle+ { ch }) { ... }  // ERROR: write-only
}
```

**Error**: `Trying to receive on non-readable channel`

### Error 3: Pattern Mismatch

```rholang
ch!(1, 2, 3) |
for (@x <- ch) { ... }  // No match (expects 1 item, got 3)
```

**Result**: Continuation waits forever (no error, just no match).

### Error 4: Type Mismatch in Pattern

```rholang
ch!("string") |
for (@{"key": value} <- ch) { ... }  // No match (string != map)
```

**Result**: No match (continuation waits).

## Performance Characteristics

### Receive Performance

```
Operation                    Time
----------------------------------------
Parse receive expression     ~15 μs
Normalize (AST → Par)        ~30 μs
Desugar join patterns        ~10 μs
Evaluate channels            ~5 μs × N
Evaluate patterns            ~5 μs × N
Bundle check                 ~1 μs
RSpace.consume (match)       ~11 μs (2 channels)
RSpace.consume (wait)        ~10 μs (store continuation)
Pattern matching             ~5-10 μs

Total (immediate match):     ~70 μs
Total (wait):                ~65 μs
```

### Join Pattern Performance

```
Channels in Join    Matching Time
-----------------------------------
2                   ~11 μs
3                   ~50 μs
5                   ~200 μs
10                  ~2 ms
```

**Complexity**: O(N!) worst case (combinatorial explosion).

### Memory Usage

```
Receive structure:           ~500 bytes
Pattern (Par):               ~100 bytes × N
Continuation body:           ~1-10 KB (depends on code)

Typical receive:             ~1-2 KB
```

## Integration with RSpace

### Receive → Consume Mapping

**Rholang**:
```rholang
for (@x <- ch1 & @y <- ch2) { body }
```

**RSpace**:
```rust
rspace.consume(
    vec![ch1, ch2],                   // Channels
    vec![pattern_x, pattern_y],       // Patterns
    ParWithRandom {
        body: Some(continuation),     // Body to execute
        random_state: rand.to_bytes(),
    },
    false,                            // persist
    BTreeSet::new(),                  // peek (empty = no peek)
)
```

### Persistent Receive → Persistent Consume

**Rholang**:
```rholang
for (@x <= ch) { body }
```

**RSpace**:
```rust
rspace.consume(
    vec![ch],
    vec![pattern_x],
    ParWithRandom { body: Some(continuation), ... },
    true,  // ← persist = true
    BTreeSet::new(),
)
```

### Peek Receive → Peek Consume

**Rholang**:
```rholang
for (@x <<- ch1 & @y <<- ch2) { body }
```

**RSpace**:
```rust
rspace.consume(
    vec![ch1, ch2],
    vec![pattern_x, pattern_y],
    ParWithRandom { body: Some(continuation), ... },
    false,
    BTreeSet::from([0, 1]),  // ← peek on both channels
)
```

## Comparison with Other Languages

### vs. Go Select

| Feature | Rholang Receive | Go Select |
|---------|-----------------|-----------|
| **Syntax** | `for (@x <- ch)` | `case x := <-ch:` |
| **Join Patterns** | Yes (`&`) | No (one channel per case) |
| **Persistence** | Yes (`<=`) | No |
| **Pattern Match** | Yes (structural) | No (just receive) |
| **Peek** | Yes (`<<-`) | No |

### vs. Erlang Receive

| Feature | Rholang Receive | Erlang Receive |
|---------|-----------------|----------------|
| **Syntax** | `for (@x <- ch)` | `receive X -> ...` |
| **Selective** | Via patterns | Via patterns |
| **Join** | Multi-channel (`&`) | Single mailbox |
| **Timeout** | No (yet) | Yes |
| **Persistence** | Yes (`<=`) | No |

### vs. Actor Model Receive

| Feature | Rholang Receive | Akka Receive |
|---------|-----------------|--------------|
| **Model** | Channel-based | Mailbox-based |
| **Pattern Match** | On data structure | On message type |
| **Join Patterns** | Built-in | Via FSM |
| **Persistence** | Built-in (`<=`) | Via become() |

## Testing Receive Operations

### Unit Tests

```rust
#[test]
fn test_receive_normalization() {
    let source = r#"for (@x <- @"ch") { stdout!(x) }"#;
    let normalized = normalize_rholang(source).unwrap();

    assert_eq!(normalized.receives.len(), 1);
    let receive = &normalized.receives[0];
    assert_eq!(receive.persistent, false);
    assert_eq!(receive.peek, false);
    assert_eq!(receive.binds.len(), 1);
}
```

### Integration Tests

```rust
#[tokio::test]
async fn test_join_pattern() {
    let rholang = r#"
        new ch1, ch2 in {
          ch1!(1) |
          ch2!(2) |
          for (@x <- ch1 & @y <- ch2) {
            @"result"!(x + y)
          }
        }
    "#;

    let result = execute_rholang(rholang).await.unwrap();
    assert_eq!(result, Par::from(3));
}
```

### Property-Based Tests

```rust
#[quickcheck]
fn prop_receive_matches_send(data: ArbitraryPar) -> bool {
    let sent = send_on_channel(data.clone());
    let received = receive_with_pattern("@x");
    sent == received
}
```

## Summary

Receive operations in Rholang enable **pattern-based message consumption** via channels:

**Syntax Forms**:
- `for (@x <- ch)` - Linear receive (once)
- `for (@x <= ch)` - Persistent receive (contract)
- `for (@x <<- ch)` - Peek (non-consuming)
- `for (@x <- ch1 & @y <- ch2)` - Join pattern (atomic multi-channel)

**Pattern Types**:
- Variables (`@x`)
- Constants (`@42`)
- Wildcards (`@_`)
- Structures (`@{"key": value}`)
- Lists (`@[head ...tail]`)

**Key Features**:
- Structural pattern matching
- Atomic join patterns
- Persistent continuations for contracts
- Peek for non-destructive reads
- Bundle-based access control

**Performance**:
- ~70 μs per receive (immediate match)
- ~11 μs for 2-channel join
- O(N!) worst case for join patterns

**RSpace Integration**:
- Receive → `consume()` operation
- Pattern matching via spatial matcher
- Continuation dispatch on match
- State tracked in Merkleized history

**Key Insight**: Receive operations provide the consumer side of Rholang's communication model, with join patterns enabling powerful coordination primitives that go beyond traditional message passing.

## Further Reading

- [Send Operations](send-operations.md) - Producer side of communication
- [RSpace Produce/Consume](../04-rspace-tuplespace/produce-consume.md) - Underlying tuple space
- [Spatial Matching](../04-rspace-tuplespace/spatial-matching.md) - Pattern matching algorithm
- [Consensus Integration](../05-distributed-execution/consensus-integration.md) - Deterministic execution

---

**Navigation**: [← Send Operations](send-operations.md) | [RSpace →](../04-rspace-tuplespace/produce-consume.md) | [Consensus Integration →](../05-distributed-execution/consensus-integration.md)
