# MeTTaTron Future Enhancements Roadmap

**Date**: 2025-11-24
**Status**: Planning Document
**Purpose**: Comprehensive roadmap for optional improvements to MeTTaTron

This document outlines potential enhancements to MeTTaTron organized by priority and implementation complexity. All items are **optional** and can be tackled independently based on project needs.

---

## Priority Levels

| Priority | Focus Area | Timeline | Risk Level |
|----------|-----------|----------|------------|
| **P1** | Performance Optimization | Weeks 1-2 | Low |
| **P2** | LSP Server Development | Weeks 3-6 | Medium |
| **P3** | Debugging Infrastructure | Weeks 7-8 | Low |
| **P4** | Type System Extensions | Weeks 9-10 | High |
| **P5** | Standard Library | Weeks 11-12 | Low |
| **P6** | Error Handling | Week 13 | Low |
| **P7** | Documentation Generator | Week 14 | Low |
| **P8** | Build System Integration | Week 15 | Medium |
| **P9** | Testing Framework | Week 16 | Low |
| **P10** | Advanced Language Features | Weeks 17+ | High |

---

## P1: Performance Optimization (Weeks 1-2)

### 1.1 Memoization for Pattern Matching

**Problem**: Repeated pattern matching on identical expressions wastes CPU cycles.

**Solution**: Cache pattern match results using expression hash as key.

**Implementation**:
```rust
// src/backend/eval/mod.rs
use std::collections::HashMap;

pub struct PatternCache {
    cache: HashMap<u64, bool>,  // hash → matches
}

impl PatternCache {
    fn check_match(&mut self, pattern: &MettaValue, value: &MettaValue) -> bool {
        let key = compute_hash((pattern, value));
        if let Some(&result) = self.cache.get(&key) {
            return result;
        }
        let result = pattern_match_impl(pattern, value, &mut HashMap::new());
        self.cache.insert(key, result);
        result
    }
}
```

**Effort**: 2-3 days
**Impact**: 15-30% speedup on rule-heavy code
**Dependencies**: None

---

### 1.2 Parallel Rule Evaluation

**Problem**: Independent rules evaluated sequentially, underutilizing multi-core CPUs.

**Solution**: Use Rayon to parallelize rule application when rules don't interfere.

**Implementation**:
```rust
// src/backend/eval/evaluation.rs
use rayon::prelude::*;

fn apply_rules_parallel(expr: &MettaValue, rules: &[Rule]) -> Vec<MettaValue> {
    rules
        .par_iter()
        .filter_map(|rule| {
            if pattern_match(&rule.pattern, expr) {
                Some(eval(rule.body.clone()))
            } else {
                None
            }
        })
        .collect()
}
```

**Effort**: 3-5 days
**Impact**: 2-4x speedup on multi-core systems
**Dependencies**: Add `rayon` crate
**Caveat**: Requires thread-safe environment cloning

---

### 1.3 Indexed Space Lookup

**Problem**: Linear search through space for pattern matching is O(n).

**Solution**: Build inverted index on first element of expressions.

**Implementation**:
```rust
// src/backend/environment.rs
use std::collections::HashMap;

pub struct IndexedSpace {
    by_head: HashMap<String, Vec<MettaValue>>,  // head → expressions
    all: Vec<MettaValue>,                        // fallback for variables
}

impl IndexedSpace {
    fn query(&self, pattern: &MettaValue) -> impl Iterator<Item = &MettaValue> {
        match pattern {
            MettaValue::SExpression(items, _) if !items.is_empty() => {
                match &items[0] {
                    MettaValue::Atom(head) => {
                        self.by_head.get(head).into_iter().flatten()
                    }
                    _ => self.all.iter()
                }
            }
            _ => self.all.iter()
        }
    }
}
```

**Effort**: 4-6 days
**Impact**: O(n) → O(k) for space lookups (k = expressions with same head)
**Dependencies**: None

---

### 1.4 Lazy Evaluation Optimizations

**Problem**: Some evaluations force computation that's never used.

**Solution**: Delay evaluation of expensive operations until result accessed.

**Implementation**:
```rust
// src/backend/models/metta_value.rs
pub enum MettaValue {
    // ... existing variants ...
    Thunk(Box<dyn Fn() -> MettaValue + Send + Sync>),
}

// Force evaluation when needed
fn force(value: MettaValue) -> MettaValue {
    match value {
        MettaValue::Thunk(f) => force(f()),
        other => other,
    }
}
```

**Effort**: 5-7 days
**Impact**: Memory savings, avoids unnecessary computation
**Dependencies**: None
**Caveat**: Requires careful handling of side effects

---

## P2: LSP Server Development (Weeks 3-6)

### 2.1 Core LSP Infrastructure

**Goal**: Implement MeTTa Language Server Protocol for IDE integration.

**Features**:
- Syntax highlighting (semantic tokens)
- Hover documentation
- Go-to-definition
- Find references
- Auto-completion
- Signature help

**Implementation Steps**:

1. **Parse with Comments** (Week 3, Days 1-2):
```rust
// src/tree_sitter_parser.rs
pub struct MettaDocumentIR {
    pub ast: Vec<SExpr>,
    pub comments: Vec<CommentNode>,
    pub tree: Tree,
}

impl TreeSitterMettaParser {
    pub fn parse_with_comments(&mut self, source: &str) -> Result<MettaDocumentIR, String> {
        let tree = self.parser.parse(source, None)?;
        let root = tree.root_node();

        // Extract comments from CST
        let comments = self.extract_comments(root, source);

        // Convert to AST
        let ast = self.convert_source_file(root, source)?;

        Ok(MettaDocumentIR { ast, comments, tree })
    }
}
```

2. **LSP Server Scaffold** (Week 3, Days 3-5):
```rust
// metta-lsp/src/server.rs
use tower_lsp::{LspService, Server, LanguageServer};

struct MettaLanguageServer {
    documents: Arc<RwLock<HashMap<Url, MettaDocumentIR>>>,
}

#[tower_lsp::async_trait]
impl LanguageServer for MettaLanguageServer {
    async fn initialize(&self, _: InitializeParams) -> Result<InitializeResult> {
        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::INCREMENTAL,
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                completion_provider: Some(CompletionOptions::default()),
                definition_provider: Some(OneOf::Left(true)),
                // ... more capabilities ...
            },
            // ...
        })
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        // Implementation in 2.2
    }
}
```

3. **Hover Documentation** (Week 4, Days 1-3):
```rust
impl MettaLanguageServer {
    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = params.text_document_position_params.text_document.uri;
        let position = params.text_document_position_params.position;

        let docs = self.documents.read().await;
        let doc_ir = docs.get(&uri)?;

        // Find node at position
        let node = self.find_node_at_position(&doc_ir.tree, position)?;

        // Get doc comments above this node
        let doc_comments = self.extract_doc_comments(&doc_ir.comments, node.start_position());

        Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: format_doc_comments(&doc_comments),
            }),
            range: None,
        })
    }
}
```

4. **Go-to-Definition** (Week 4, Days 4-5):
```rust
impl MettaLanguageServer {
    async fn goto_definition(&self, params: GotoDefinitionParams) -> Result<Option<GotoDefinitionResponse>> {
        // Build symbol table from AST
        let symbol_table = self.build_symbol_table(&doc_ir.ast);

        // Find definition location
        let node_text = self.get_node_text(node);
        if let Some(def_location) = symbol_table.get(node_text) {
            return Some(GotoDefinitionResponse::Scalar(Location {
                uri: uri.clone(),
                range: def_location.range,
            }));
        }
        None
    }
}
```

5. **Auto-Completion** (Week 5):
```rust
impl MettaLanguageServer {
    async fn completion(&self, params: CompletionParams) -> Result<Option<CompletionResponse>> {
        // Context-aware completion
        let context = self.analyze_completion_context(&doc_ir, position);

        let mut items = vec![];

        // Add keywords
        items.extend(self.complete_keywords(&context));

        // Add symbols in scope
        items.extend(self.complete_symbols(&symbol_table, &context));

        // Add grounded functions
        items.extend(self.complete_builtins(&context));

        Some(CompletionResponse::Array(items))
    }
}
```

6. **Integration Testing** (Week 6):
   - Test with VSCode extension
   - Test with NeoVim client
   - Performance profiling

**Effort**: 4 weeks
**Impact**: Major IDE integration improvement
**Dependencies**: `tower-lsp`, Tree-Sitter queries
**Reference**: Your Rholang LSP implementation

---

### 2.2 Documentation Comment Conventions

**Goal**: Define and parse structured documentation comments.

**Convention**:
```metta
;;; Doubles the given number
;;;
;;; @param $x - The number to double
;;; @return The doubled value
;;; @example
;;;   !(double 21)  ; → 42
(= (double $x) (* $x 2))
```

**Parser**:
```rust
// metta-lsp/src/doc_parser.rs
pub struct DocComment {
    pub summary: String,
    pub params: Vec<(String, String)>,  // name, description
    pub return_desc: Option<String>,
    pub examples: Vec<String>,
}

impl DocComment {
    pub fn parse(comments: &[CommentNode]) -> Option<Self> {
        let text = comments.iter()
            .filter(|c| c.text.starts_with(";;;"))
            .map(|c| c.text.trim_start_matches(";;;").trim())
            .collect::<Vec<_>>()
            .join("\n");

        // Parse @tags
        // ...
    }
}
```

**Effort**: 3-4 days
**Impact**: Rich hover tooltips, API documentation generation

---

## P3: Debugging Infrastructure (Weeks 7-8)

### 3.1 Evaluation Trace Visualization

**Goal**: Show step-by-step evaluation for debugging.

**Implementation**:
```rust
// src/backend/eval/trace.rs
pub struct EvalTrace {
    steps: Vec<EvalStep>,
}

pub struct EvalStep {
    expr: MettaValue,
    rule_applied: Option<String>,
    result: MettaValue,
    depth: usize,
    timestamp: Instant,
}

impl Evaluator {
    fn eval_with_trace(&self, expr: MettaValue, trace: &mut EvalTrace) -> MettaValue {
        let start = Instant::now();

        // Record input
        trace.push_input(expr.clone(), self.depth);

        // Evaluate
        let result = self.eval(expr);

        // Record output
        trace.push_output(result.clone(), start.elapsed());

        result
    }
}
```

**Output Format** (JSON):
```json
{
  "steps": [
    {
      "input": "(+ 1 2)",
      "rule": "builtin:+",
      "output": "3",
      "depth": 0,
      "duration_us": 15
    }
  ]
}
```

**Effort**: 4-5 days
**Impact**: Easier debugging, performance profiling

---

### 3.2 Breakpoint Support

**Goal**: Pause evaluation at specific expressions.

**Implementation**:
```rust
// src/backend/eval/debugger.rs
pub struct Debugger {
    breakpoints: HashSet<MettaValue>,
    enabled: bool,
}

impl Debugger {
    fn should_break(&self, expr: &MettaValue) -> bool {
        self.enabled && self.breakpoints.contains(expr)
    }
}

// In evaluator:
if debugger.should_break(&expr) {
    // Pause, send to LSP, wait for continue
}
```

**Effort**: 3-4 days
**Impact**: Interactive debugging in IDEs

---

## P4: Type System Extensions (Weeks 9-10)

### 4.1 Dependent Types

**Goal**: Types that depend on values.

**Example**:
```metta
; Vector parameterized by length
(: (Vec $n $t) Type)

; Append preserves length
(: (append (Vec $n $t) (Vec $m $t)) (Vec (+ $n $m) $t))
```

**Implementation**:
```rust
// src/backend/eval/types.rs
fn infer_dependent_type(expr: &MettaValue, env: &Environment) -> MettaValue {
    match expr {
        MettaValue::SExpression(items, _) if items[0] == atom("Vec") => {
            // Evaluate length parameter
            let len = eval(items[1].clone(), env);
            let elem_type = items[2].clone();
            sexpr![atom("Vec"), len, elem_type]
        }
        // ...
    }
}
```

**Effort**: 7-10 days
**Impact**: Stronger type safety
**Risk**: High complexity, requires careful design

---

### 4.2 Polymorphic Type Inference

**Goal**: Infer type variables automatically.

**Example**:
```metta
; map should infer: ∀a b. (a → b) → [a] → [b]
(= (map $f Nil) Nil)
(= (map $f (Cons $x $xs)) (Cons ($f $x) (map $f $xs)))
```

**Implementation**: Hindley-Milner type inference with unification.

**Effort**: 10-14 days
**Impact**: Less type annotation burden
**Risk**: High complexity

---

## P5: Standard Library (Weeks 11-12)

### 5.1 List Operations

**Functions**:
- `(map $f $list)` - Apply function to each element
- `(filter $pred $list)` - Keep elements matching predicate
- `(fold $f $init $list)` - Reduce list to single value
- `(zip $list1 $list2)` - Combine two lists
- `(take $n $list)` - First n elements
- `(drop $n $list)` - Skip n elements
- `(reverse $list)` - Reverse order
- `(sort $list)` - Sort elements
- `(unique $list)` - Remove duplicates

**Implementation**:
```rust
// src/backend/stdlib/lists.rs
pub fn register_list_ops(env: &mut Environment) {
    env.add_grounded("map", |args| {
        let f = &args[0];
        let list = &args[1];
        // Apply f to each element
    });

    env.add_grounded("filter", |args| {
        // Keep elements where predicate returns True
    });

    // ... more operations ...
}
```

**Effort**: 4-5 days
**Impact**: Essential functionality for practical programs

---

### 5.2 String Operations

**Functions**:
- `(str-concat $s1 $s2)` - Concatenate strings
- `(str-split $sep $s)` - Split on separator
- `(str-trim $s)` - Remove whitespace
- `(str-upper $s)` / `(str-lower $s)` - Case conversion
- `(str-replace $pattern $replacement $s)` - String replacement
- `(str-match $pattern $s)` - Pattern matching
- `(str-length $s)` - String length

**Effort**: 3-4 days

---

### 5.3 Math/Logic Utilities

**Functions**:
- `(abs $x)` - Absolute value
- `(min $x $y)` / `(max $x $y)` - Min/max
- `(sqrt $x)` / `(pow $x $y)` - Math operations
- `(and $p $q)` / `(or $p $q)` / `(not $p)` - Logic operations
- `(all $pred $list)` / `(any $pred $list)` - Quantifiers

**Effort**: 2-3 days

---

## P6: Error Handling Improvements (Week 13)

### 6.1 Better Error Messages

**Current**:
```
Error: Pattern match failed
```

**Improved**:
```
Error: Pattern match failed at line 42, column 10
  Pattern: (foo $x $y)
  Value:   (foo 1)
  Reason:  Expected 3 elements, found 2

  Context:
    40 |   (= (bar $z) (foo $z $z))
    41 |
    42 |   !(bar 1)
       |     ^^^^^ pattern match failed here
```

**Implementation**:
```rust
// src/backend/eval/errors.rs
pub struct DetailedError {
    message: String,
    span: Span,
    context: Vec<String>,  // surrounding source lines
    reason: String,
}

impl DetailedError {
    pub fn format(&self) -> String {
        format!(
            "Error: {} at line {}, column {}\n  {}\n  Reason: {}\n\n  Context:\n{}",
            self.message,
            self.span.start.line,
            self.span.start.column,
            self.format_snippet(),
            self.reason,
            self.format_context(),
        )
    }
}
```

**Effort**: 4-5 days
**Impact**: Much easier debugging

---

### 6.2 Warning System

**Goal**: Non-fatal warnings for suspicious code.

**Examples**:
- Unused variables
- Shadowed bindings
- Non-exhaustive pattern matches
- Deprecated features

**Implementation**:
```rust
// src/backend/eval/warnings.rs
pub enum Warning {
    UnusedVariable(String, Span),
    ShadowedBinding(String, Span, Span),  // name, new, old
    NonExhaustiveMatch(Span),
}

pub struct WarningCollector {
    warnings: Vec<Warning>,
    enabled: bool,
}
```

**Effort**: 3-4 days

---

## P7: Documentation Generator (Week 14)

### 7.1 Extract API Documentation

**Goal**: Generate HTML/Markdown docs from doc comments.

**Usage**:
```bash
mettatron doc src/ --output docs/api/
```

**Output**:
```markdown
# API Documentation

## Functions

### `double`

Doubles the given number

**Parameters:**
- `$x` - The number to double

**Returns:** The doubled value

**Example:**
```metta
!(double 21)  ; → 42
```

**Definition:**
```metta
(= (double $x) (* $x 2))
```
```

**Implementation**:
```rust
// src/doc_gen/mod.rs
pub fn generate_docs(input_dir: &Path, output_dir: &Path) -> Result<()> {
    let mut parser = TreeSitterMettaParser::new()?;

    for file in find_metta_files(input_dir) {
        let source = fs::read_to_string(file)?;
        let doc_ir = parser.parse_with_comments(&source)?;

        // Extract definitions with doc comments
        let docs = extract_documented_symbols(&doc_ir);

        // Generate markdown
        let markdown = format_as_markdown(&docs);

        // Write output
        let output_file = output_dir.join(file.with_extension("md"));
        fs::write(output_file, markdown)?;
    }

    // Generate index
    generate_index(output_dir, &all_symbols)?;

    Ok(())
}
```

**Effort**: 5-6 days
**Impact**: Professional API documentation

---

## P8: Build System Integration (Week 15)

### 8.1 Package Manager

**Goal**: Manage MeTTa dependencies.

**Configuration** (`metta.toml`):
```toml
[package]
name = "my-project"
version = "0.1.0"
authors = ["You <you@example.com>"]

[dependencies]
stdlib = { version = "1.0" }
logic-utils = { git = "https://github.com/example/logic-utils" }

[dev-dependencies]
test-framework = { version = "0.5" }
```

**Commands**:
```bash
mettatron init my-project     # Create new project
mettatron add stdlib           # Add dependency
mettatron build                # Build project
mettatron test                 # Run tests
mettatron publish              # Publish to registry
```

**Implementation**:
```rust
// src/package/mod.rs
pub struct Package {
    manifest: Manifest,
    dependencies: Vec<Dependency>,
}

impl Package {
    pub fn resolve_dependencies(&self) -> Result<DependencyGraph> {
        // Fetch dependencies from registry/git
        // Resolve version constraints
        // Build dependency graph
    }

    pub fn build(&self) -> Result<()> {
        // Load all dependencies
        // Compile MeTTa modules
        // Generate artifacts
    }
}
```

**Effort**: 7-10 days
**Impact**: Essential for ecosystem growth

---

### 8.2 Module System

**Goal**: Organize code into modules with imports/exports.

**Example**:
```metta
; Module definition
(module math-utils
  ; Exports
  (export (double $x) (triple $x))

  ; Private helpers
  (= (internal-helper $x) (* $x 2))

  ; Public functions
  (= (double $x) (internal-helper $x))
  (= (triple $x) (* $x 3))
)

; Import in another file
(import math-utils (double triple))

!(double 21)  ; Works
!(internal-helper 5)  ; Error: not exported
```

**Implementation**:
```rust
// src/backend/modules.rs
pub struct Module {
    name: String,
    exports: HashSet<String>,
    environment: Environment,
}

impl Module {
    pub fn import(&self, symbols: &[String], target_env: &mut Environment) -> Result<()> {
        for symbol in symbols {
            if !self.exports.contains(symbol) {
                return Err(format!("Symbol '{}' not exported by module '{}'", symbol, self.name));
            }
            // Copy rules/bindings to target environment
        }
        Ok(())
    }
}
```

**Effort**: 6-8 days

---

## P9: Testing Framework (Week 16)

### 9.1 Property-Based Testing

**Goal**: Generate test cases automatically.

**Example**:
```metta
; Property: reversing twice is identity
(property (forall $list
  (= (reverse (reverse $list)) $list)))

; Generator: random lists
(generator random-list
  (sized 0 10
    (one-of
      Nil
      (Cons (random-int -100 100) (random-list)))))

; Run tests
!(check-property reverse-twice-identity
                random-list
                num-tests: 100)
```

**Implementation**: Integration with QuickCheck-style library.

**Effort**: 5-7 days

---

### 9.2 Benchmark Infrastructure

**Goal**: Measure and track performance.

**Example**:
```metta
; Benchmark definition
(benchmark fibonacci-recursive
  (fib 20))

(benchmark fibonacci-iterative
  (fib-iter 20))
```

**Commands**:
```bash
mettatron bench                # Run all benchmarks
mettatron bench --compare      # Compare with baseline
mettatron bench --flamegraph   # Generate flamegraph
```

**Implementation**:
```rust
// src/bench/mod.rs
pub fn run_benchmark(name: &str, expr: MettaValue, iterations: usize) -> BenchResult {
    let mut times = Vec::with_capacity(iterations);

    for _ in 0..iterations {
        let start = Instant::now();
        eval(expr.clone());
        times.push(start.elapsed());
    }

    BenchResult {
        name: name.to_string(),
        mean: mean(&times),
        median: median(&times),
        std_dev: std_dev(&times),
        min: *times.iter().min().unwrap(),
        max: *times.iter().max().unwrap(),
    }
}
```

**Effort**: 4-5 days

---

## P10: Advanced Language Features (Weeks 17+)

### 10.1 Macro System

**Goal**: Compile-time code generation.

**Example**:
```metta
; Define macro
(macro when ($cond $body)
  `(if ,cond ,body Nil))

; Usage
!(when (> x 0) (print "positive"))

; Expands to:
!(if (> x 0) (print "positive") Nil)
```

**Implementation**: AST transformation with quasiquotation.

**Effort**: 10-14 days
**Risk**: High complexity, hygiene issues

---

### 10.2 Compile-Time Evaluation

**Goal**: Run code at compile time.

**Example**:
```metta
; Compile-time constant
(define-const PI (comptime (atan2 0 -1)))

; Compile-time list
(define-const PRIMES
  (comptime (sieve (range 2 1000))))
```

**Effort**: 7-10 days

---

### 10.3 Embedded DSLs

**Goal**: Support domain-specific languages within MeTTa.

**Example**:
```metta
; SQL-like DSL
(dsl sql
  (select name age
    (from users)
    (where (> age 18))
    (order-by age desc)))
```

**Implementation**: Custom parsers + AST transformation.

**Effort**: 14+ days
**Risk**: Very high complexity

---

## Implementation Guidelines

### General Principles

1. **Incremental Development**: Implement features in small, testable chunks
2. **Documentation First**: Write design docs before coding
3. **Test Coverage**: Maintain >80% test coverage for new code
4. **Backward Compatibility**: Don't break existing code
5. **Performance Awareness**: Benchmark before and after optimizations

### Code Review Checklist

- [ ] Tests added for new functionality
- [ ] Documentation updated
- [ ] No clippy warnings
- [ ] Code formatted with `cargo fmt`
- [ ] Examples provided
- [ ] Performance impact measured (if optimization)
- [ ] Error messages are clear
- [ ] Edge cases handled

### Dependency Policy

- Prefer standard library when possible
- Audit new dependencies for security
- Pin versions in `Cargo.toml`
- Document reason for each dependency

---

## Metrics and Success Criteria

### Performance Targets

| Metric | Current | Target |
|--------|---------|--------|
| Parse time (1000 LOC) | ~50ms | <30ms |
| Eval time (simple rules) | ~100µs | <50µs |
| Memory usage (10K rules) | ~50MB | <30MB |
| LSP response time | N/A | <100ms |

### Quality Metrics

- Test coverage: >80%
- Benchmark suite: >50 benchmarks
- Documentation: Every public API documented
- Examples: >100 example files

---

## References

- **Tree-Sitter Documentation**: https://tree-sitter.github.io/tree-sitter/
- **LSP Specification**: https://microsoft.github.io/language-server-protocol/
- **Rholang LSP** (reference impl): `/home/dylon/Workspace/f1r3fly.io/rholang-language-server/`
- **MORK Documentation**: `/home/dylon/Workspace/f1r3fly.io/MORK/`

---

## Change Log

| Date | Priority | Item | Status |
|------|----------|------|--------|
| 2025-11-24 | - | Initial roadmap created | ✅ Complete |

---

**Note**: This roadmap is a living document. Update it as priorities shift or new features are identified.
