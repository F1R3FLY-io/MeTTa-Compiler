# Evaluation Engine Implementation Guide for MORK

**Version**: 1.0
**Date**: 2025-11-13
**Target**: MeTTaTron Compiler
**Hardware Reference**: Intel Xeon E5-2699 v3 @ 2.30GHz (36 cores, 72 threads), 252 GB DDR4-2133 ECC

---

## Table of Contents

1. [Introduction](#introduction)
2. [MeTTa Evaluation Semantics](#metta-evaluation-semantics)
3. [Minimal Operation Set](#minimal-operation-set)
4. [Evaluation Architecture](#evaluation-architecture)
5. [Eval Operation](#eval-operation)
6. [Chain Operation](#chain-operation)
7. [Unify Operation](#unify-operation)
8. [Cons/Decons Operations](#consdecons-operations)
9. [Function/Return Operations](#functionreturn-operations)
10. [Rewrite Rules](#rewrite-rules)
11. [Non-Determinism and Backtracking](#non-determinism-and-backtracking)
12. [Grounded Functions](#grounded-functions)
13. [Performance Optimization](#performance-optimization)
14. [Implementation Examples](#implementation-examples)
15. [Testing Strategy](#testing-strategy)
16. [Performance Benchmarks](#performance-benchmarks)

---

## Introduction

This document provides a comprehensive guide to implementing MeTTa's evaluation engine on top of MORK. The evaluation engine is the core of the MeTTaTron compiler, responsible for:

- **Expression reduction**: Evaluating MeTTa expressions to normal form
- **Pattern matching**: Applying rewrite rules
- **Non-deterministic search**: Managing multiple evaluation paths
- **Grounded execution**: Interfacing with native functions
- **Type checking**: Verifying type constraints

### Key Design Goals

1. **Correctness**: Preserve MeTTa's evaluation semantics exactly
2. **Performance**: Leverage MORK's efficient operations
3. **Composability**: Enable modular evaluation strategies
4. **Debuggability**: Provide clear execution traces
5. **Extensibility**: Support custom evaluation rules

### MORK Advantages for Evaluation

- **Fast pattern matching**: O(log N) queries via structural sharing
- **Efficient rule storage**: Prefix compression for similar rules
- **COW semantics**: O(1) space snapshots for backtracking
- **Parallelizable**: Independent evaluation paths can run concurrently

---

## MeTTa Evaluation Semantics

### Evaluation Model

MeTTa uses a **non-deterministic rewriting** model:

1. **Match** expression against space patterns
2. **Instantiate** matching rules
3. **Reduce** expression using instantiated rules
4. **Repeat** until no more rules apply (normal form)

### Example Evaluation

```metta
; Space contains:
(= (parent Alice) Bob)
(= (parent Bob) Carol)
(= (grandparent $x) (parent (parent $x)))

; Evaluate:
!(eval (grandparent Alice))

; Step 1: Match (grandparent Alice) against rules
;         Matches: (= (grandparent $x) (parent (parent $x)))
;         Bindings: {$x → Alice}

; Step 2: Instantiate rule
;         (parent (parent Alice))

; Step 3: Evaluate inner (parent Alice)
;         Matches: (= (parent Alice) Bob)
;         Result: Bob

; Step 4: Evaluate (parent Bob)
;         Matches: (= (parent Bob) Carol)
;         Result: Carol

; Final result: Carol
```

### Non-Determinism

Multiple rules may match, yielding multiple results:

```metta
; Space:
(= (child Alice) Bob)
(= (child Alice) Carol)

; Evaluate:
!(eval (child Alice))

; Results: [Bob, Carol]  ; Both are valid
```

### Normal Forms

An expression is in **normal form** if no rules match:

```metta
; foo has no rules → already in normal form
!(eval foo)  → foo

; (bar baz) has no matching rules → normal form
!(eval (bar baz))  → (bar baz)
```

---

## Minimal Operation Set

MeTTa's evaluation can be implemented using a minimal set of operations:

### 1. eval

Evaluate an expression to normal form(s):

```metta
!(eval <expr>)
```

### 2. chain

Chain evaluation results:

```metta
!(chain <expr> <var> <template>)
; Equivalent to: for each result of eval(<expr>), bind to <var> and eval(<template>)
```

### 3. unify

Bidirectional pattern matching:

```metta
!(unify <pattern1> <pattern2> <template> <else>)
; If patterns unify, eval template with bindings; otherwise eval else
```

### 4. cons-atom / decons-atom

Construct/deconstruct expressions:

```metta
!(cons-atom <head> <tail>)   ; Build expression
!(decons-atom <expr>)         ; Destructure expression
```

### 5. function / return

Define and return from functions:

```metta
!(function <body>)           ; Define grounded function
!(return <value>)            ; Return value from function
```

---

## Evaluation Architecture

### High-Level Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                     Evaluation Engine                       │
├─────────────────────────────────────────────────────────────┤
│  • Evaluator      - Main evaluation loop                    │
│  • RuleIndex      - Fast rule lookup                        │
│  • EvalCache      - Memoization cache                       │
│  • BacktrackStack - Non-determinism management              │
└────────────┬────────────────────────────────────────────────┘
             │
             ▼
┌─────────────────────────────────────────────────────────────┐
│                   Pattern Matcher                           │
├─────────────────────────────────────────────────────────────┤
│  • PatternMatcher - Match expressions against rules         │
│  • BindingsSet    - Manage multiple bindings                │
└────────────┬────────────────────────────────────────────────┘
             │
             ▼
┌─────────────────────────────────────────────────────────────┐
│                      MorkSpace                              │
├─────────────────────────────────────────────────────────────┤
│  • Space operations (query, add, remove)                    │
│  • Rule storage                                             │
└─────────────────────────────────────────────────────────────┘
```

### Data Flow

```
Expression ──→ Evaluator ──→ RuleIndex ──→ PatternMatcher ──→ Space
                   ↑                                             │
                   └──────────── Bindings ←──────────────────────┘

Bindings ──→ Instantiate Rule ──→ New Expression ──→ Evaluator (recursive)
```

---

## Eval Operation

### Semantics

```metta
!(eval <expr>)
; Returns: all normal forms of <expr>
```

### Implementation

```rust
use mork_space::MorkSpace;
use mork_pattern::{PatternMatcher, BindingsSet};
use metta_atom::Atom;

pub struct Evaluator {
    space: Arc<MorkSpace>,
    cache: Option<Arc<RwLock<EvalCache>>>,
    max_depth: usize,
}

impl Evaluator {
    pub fn new(space: Arc<MorkSpace>) -> Self {
        Self {
            space,
            cache: None,
            max_depth: 1000,
        }
    }

    pub fn with_cache(space: Arc<MorkSpace>) -> Self {
        Self {
            space,
            cache: Some(Arc::new(RwLock::new(EvalCache::new()))),
            max_depth: 1000,
        }
    }

    /// Evaluate an atom to normal form(s)
    pub fn eval(&self, atom: &Atom) -> Result<Vec<Atom>, EvalError> {
        self.eval_with_depth(atom, 0)
    }

    fn eval_with_depth(&self, atom: &Atom, depth: usize) -> Result<Vec<Atom>, EvalError> {
        if depth > self.max_depth {
            return Err(EvalError::MaxDepthExceeded);
        }

        // Check cache
        if let Some(cache) = &self.cache {
            if let Some(results) = cache.read().unwrap().get(atom) {
                return Ok(results.clone());
            }
        }

        // Evaluate based on atom type
        let results = match atom {
            Atom::Symbol(_) | Atom::Variable(_) => {
                // Symbols and variables evaluate to themselves
                vec![atom.clone()]
            }

            Atom::Grounded(g) => {
                // Check if grounded function
                if g.is_function() {
                    // Execute grounded function
                    self.eval_grounded_function(g, depth)?
                } else {
                    // Grounded values evaluate to themselves
                    vec![atom.clone()]
                }
            }

            Atom::Expression(expr) => {
                self.eval_expression(expr, depth)?
            }
        };

        // Update cache
        if let Some(cache) = &self.cache {
            cache.write().unwrap().put(atom.clone(), results.clone());
        }

        Ok(results)
    }

    fn eval_expression(
        &self,
        expr: &ExpressionAtom,
        depth: usize,
    ) -> Result<Vec<Atom>, EvalError> {
        // Step 1: Try to match against rewrite rules
        let rule_pattern = expr!(sym!("="), var!("$pattern"), var!("$template"));
        let matcher = PatternMatcher::new(Arc::clone(&self.space));
        let rule_matches = matcher.match_pattern(&rule_pattern)?;

        let mut results = Vec::new();

        for rule_bindings in rule_matches.alternatives() {
            let pattern = rule_bindings.get("$pattern").unwrap();
            let template = rule_bindings.get("$template").unwrap();

            // Try to match expression against rule pattern
            let expr_atom = Atom::Expression(expr.clone());
            let match_result = self.match_atoms(&expr_atom, pattern)?;

            for match_bindings in match_result.alternatives() {
                // Instantiate template with bindings
                let instantiated = match_bindings.apply(template);

                // Recursively evaluate instantiated template
                let evaluated = self.eval_with_depth(&instantiated, depth + 1)?;
                results.extend(evaluated);
            }
        }

        // Step 2: If no rules matched, evaluate children and reconstruct
        if results.is_empty() {
            results = self.eval_children(expr, depth)?;
        }

        // Step 3: If still no results, return original expression (normal form)
        if results.is_empty() {
            results.push(Atom::Expression(expr.clone()));
        }

        Ok(results)
    }

    fn eval_children(
        &self,
        expr: &ExpressionAtom,
        depth: usize,
    ) -> Result<Vec<Atom>, EvalError> {
        let children = expr.children();

        if children.is_empty() {
            return Ok(vec![Atom::Expression(expr.clone())]);
        }

        // Evaluate each child
        let mut child_results: Vec<Vec<Atom>> = Vec::new();

        for child in children {
            let results = self.eval_with_depth(child, depth + 1)?;
            child_results.push(results);
        }

        // Cartesian product of child results
        let combinations = cartesian_product(&child_results);

        // Reconstruct expressions with all combinations
        let results: Vec<Atom> = combinations.into_iter()
            .map(|combo| {
                Atom::Expression(ExpressionAtom::new(combo))
            })
            .collect();

        Ok(results)
    }

    fn match_atoms(
        &self,
        atom: &Atom,
        pattern: &Atom,
    ) -> Result<BindingsSet, EvalError> {
        // Create temporary space with atom
        let mut temp_space = MorkSpace::new();
        temp_space.add(atom)?;

        // Match pattern against temp space
        let matcher = PatternMatcher::new(Arc::new(temp_space));
        let bindings = matcher.match_pattern(pattern)?;

        Ok(bindings)
    }

    fn eval_grounded_function(
        &self,
        func: &Grounded,
        depth: usize,
    ) -> Result<Vec<Atom>, EvalError> {
        // Execute grounded function
        // (implementation depends on grounded function interface)
        todo!("Implement grounded function execution")
    }
}

/// Cartesian product of vectors
fn cartesian_product<T: Clone>(vecs: &[Vec<T>]) -> Vec<Vec<T>> {
    if vecs.is_empty() {
        return vec![vec![]];
    }

    if vecs.len() == 1 {
        return vecs[0].iter().map(|x| vec![x.clone()]).collect();
    }

    let mut result = Vec::new();
    let rest = cartesian_product(&vecs[1..]);

    for item in &vecs[0] {
        for rest_vec in &rest {
            let mut combo = vec![item.clone()];
            combo.extend(rest_vec.iter().cloned());
            result.push(combo);
        }
    }

    result
}
```

### Eval Cache

```rust
use std::collections::HashMap;

pub struct EvalCache {
    cache: HashMap<Atom, Vec<Atom>>,
    max_size: usize,
}

impl EvalCache {
    pub fn new() -> Self {
        Self::with_capacity(10000)
    }

    pub fn with_capacity(max_size: usize) -> Self {
        Self {
            cache: HashMap::new(),
            max_size,
        }
    }

    pub fn get(&self, key: &Atom) -> Option<&Vec<Atom>> {
        self.cache.get(key)
    }

    pub fn put(&mut self, key: Atom, value: Vec<Atom>) {
        if self.cache.len() >= self.max_size {
            // Simple eviction: clear cache when full
            self.cache.clear();
        }
        self.cache.insert(key, value);
    }

    pub fn clear(&mut self) {
        self.cache.clear();
    }
}
```

---

## Chain Operation

### Semantics

```metta
!(chain <expr> <var> <template>)
; Equivalent to: flat_map(eval(<expr>), |result| {
;     bind <var> to result;
;     eval(<template>)
; })
```

### Example

```metta
; Space:
(= (numbers) (1 2 3))
(= (square $x) (* $x $x))

; Chain:
!(chain (numbers) $n (square $n))

; Evaluation:
; 1. eval (numbers) → [1, 2, 3]
; 2. For each result:
;    - bind $n = 1, eval (square 1) → 1
;    - bind $n = 2, eval (square 2) → 4
;    - bind $n = 3, eval (square 3) → 9
; Result: [1, 4, 9]
```

### Implementation

```rust
impl Evaluator {
    pub fn chain(
        &self,
        expr: &Atom,
        var: &str,
        template: &Atom,
    ) -> Result<Vec<Atom>, EvalError> {
        // Step 1: Evaluate expression
        let expr_results = self.eval(expr)?;

        // Step 2: For each result, bind variable and evaluate template
        let mut final_results = Vec::new();

        for result in expr_results {
            // Create bindings
            let mut bindings = Bindings::new();
            bindings.add_binding(var, result);

            // Apply bindings to template
            let instantiated = bindings.apply(template);

            // Evaluate instantiated template
            let template_results = self.eval(&instantiated)?;
            final_results.extend(template_results);
        }

        Ok(final_results)
    }
}
```

---

## Unify Operation

### Semantics

```metta
!(unify <pattern1> <pattern2> <then-template> <else-template>)
; If pattern1 and pattern2 unify:
;   eval then-template with bindings
; Else:
;   eval else-template
```

### Example

```metta
!(unify (parent Alice $x) (parent $y Bob)
    (matched $x $y)
    (no-match))

; pattern1: (parent Alice $x)
; pattern2: (parent $y Bob)

; Unification:
; parent = parent ✓
; Alice = $y  →  $y = Alice
; $x = Bob    →  $x = Bob

; Bindings: {$x → Bob, $y → Alice}
; Result: (matched Bob Alice)
```

### Implementation

```rust
impl Evaluator {
    pub fn unify(
        &self,
        pattern1: &Atom,
        pattern2: &Atom,
        then_template: &Atom,
        else_template: &Atom,
    ) -> Result<Vec<Atom>, EvalError> {
        // Attempt unification
        match self.unify_patterns(pattern1, pattern2)? {
            Some(bindings) => {
                // Unification succeeded - evaluate then branch
                let instantiated = bindings.apply(then_template);
                self.eval(&instantiated)
            }
            None => {
                // Unification failed - evaluate else branch
                self.eval(else_template)
            }
        }
    }

    fn unify_patterns(
        &self,
        pattern1: &Atom,
        pattern2: &Atom,
    ) -> Result<Option<Bindings>, EvalError> {
        let mut bindings = Bindings::new();

        if self.unify_recursive(pattern1, pattern2, &mut bindings)? {
            Ok(Some(bindings))
        } else {
            Ok(None)
        }
    }

    fn unify_recursive(
        &self,
        atom1: &Atom,
        atom2: &Atom,
        bindings: &mut Bindings,
    ) -> Result<bool, EvalError> {
        match (atom1, atom2) {
            // Both variables
            (Atom::Variable(v1), Atom::Variable(v2)) => {
                let name1 = v1.name();
                let name2 = v2.name();

                match (bindings.get(name1), bindings.get(name2)) {
                    (Some(val1), Some(val2)) => {
                        // Both bound - check equality
                        Ok(val1 == val2)
                    }
                    (Some(val1), None) => {
                        // v1 bound, v2 free - bind v2
                        bindings.add_binding(name2, val1.clone());
                        Ok(true)
                    }
                    (None, Some(val2)) => {
                        // v1 free, v2 bound - bind v1
                        bindings.add_binding(name1, val2.clone());
                        Ok(true)
                    }
                    (None, None) => {
                        // Both free - bind v1 to v2
                        bindings.add_binding(name1, atom2.clone());
                        Ok(true)
                    }
                }
            }

            // One variable, one non-variable
            (Atom::Variable(v), other) | (other, Atom::Variable(v)) => {
                let name = v.name();

                if let Some(bound) = bindings.get(name) {
                    // Variable already bound - check equality
                    Ok(bound == other)
                } else {
                    // Variable free - bind it
                    bindings.add_binding(name, other.clone());
                    Ok(true)
                }
            }

            // Both symbols
            (Atom::Symbol(s1), Atom::Symbol(s2)) => {
                Ok(s1.name() == s2.name())
            }

            // Both expressions
            (Atom::Expression(e1), Atom::Expression(e2)) => {
                let children1 = e1.children();
                let children2 = e2.children();

                if children1.len() != children2.len() {
                    return Ok(false);
                }

                // Recursively unify children
                for (child1, child2) in children1.iter().zip(children2.iter()) {
                    if !self.unify_recursive(child1, child2, bindings)? {
                        return Ok(false);
                    }
                }

                Ok(true)
            }

            // Both grounded
            (Atom::Grounded(g1), Atom::Grounded(g2)) => {
                // Grounded atoms unify if equal
                Ok(g1 == g2)
            }

            // Different types
            _ => Ok(false),
        }
    }
}
```

---

## Cons/Decons Operations

### cons-atom

Build an expression from head and tail:

```metta
!(cons-atom head (a b c))  →  (head a b c)
```

```rust
impl Evaluator {
    pub fn cons_atom(&self, head: &Atom, tail: &Atom) -> Result<Atom, EvalError> {
        match tail {
            Atom::Expression(expr) => {
                let mut children = vec![head.clone()];
                children.extend(expr.children().iter().cloned());
                Ok(Atom::Expression(ExpressionAtom::new(children)))
            }
            _ => {
                // Tail is not an expression - create pair
                Ok(expr!(head.clone(), tail.clone()))
            }
        }
    }
}
```

### decons-atom

Destructure an expression:

```metta
!(decons-atom (foo a b c))  →  (foo (a b c))
```

```rust
impl Evaluator {
    pub fn decons_atom(&self, atom: &Atom) -> Result<Atom, EvalError> {
        match atom {
            Atom::Expression(expr) => {
                let children = expr.children();

                if children.is_empty() {
                    // Empty expression
                    return Err(EvalError::CannotDeconstructEmpty);
                }

                let head = children[0].clone();
                let tail = if children.len() > 1 {
                    Atom::Expression(ExpressionAtom::new(children[1..].to_vec()))
                } else {
                    // Single element - tail is empty expression
                    expr!()
                };

                Ok(expr!(head, tail))
            }
            _ => {
                Err(EvalError::CannotDeconstructNonExpression)
            }
        }
    }
}
```

---

## Function/Return Operations

### function

Define a grounded function:

```metta
!(function (lambda ($x) (* $x $x)))
```

### return

Return value from function:

```metta
!(return 42)
```

### Implementation

```rust
pub struct GroundedFunction {
    body: Atom,
    evaluator: Arc<Evaluator>,
}

impl GroundedFunction {
    pub fn new(body: Atom, evaluator: Arc<Evaluator>) -> Self {
        Self { body, evaluator }
    }

    pub fn call(&self, args: &[Atom]) -> Result<Vec<Atom>, EvalError> {
        // Extract lambda parameters
        if let Atom::Expression(expr) = &self.body {
            let children = expr.children();

            if children.len() >= 3 {
                if let Atom::Symbol(sym) = &children[0] {
                    if sym.name() == "lambda" {
                        // Extract parameters
                        if let Atom::Expression(params_expr) = &children[1] {
                            let params = params_expr.children();

                            if params.len() != args.len() {
                                return Err(EvalError::ArityMismatch {
                                    expected: params.len(),
                                    got: args.len(),
                                });
                            }

                            // Create bindings
                            let mut bindings = Bindings::new();
                            for (param, arg) in params.iter().zip(args.iter()) {
                                if let Atom::Variable(var) = param {
                                    bindings.add_binding(var.name(), arg.clone());
                                } else {
                                    return Err(EvalError::InvalidParameter);
                                }
                            }

                            // Evaluate body with bindings
                            let body = &children[2];
                            let instantiated = bindings.apply(body);

                            return self.evaluator.eval(&instantiated);
                        }
                    }
                }
            }
        }

        Err(EvalError::InvalidFunctionBody)
    }
}

impl Evaluator {
    pub fn function(&self, body: &Atom) -> Result<Atom, EvalError> {
        let func = GroundedFunction::new(body.clone(), Arc::new(self.clone()));

        // Wrap in Grounded atom
        let grounded = Grounded::from_function(func);
        Ok(Atom::Grounded(grounded))
    }

    pub fn return_value(&self, value: &Atom) -> Result<Vec<Atom>, EvalError> {
        // Return simply evaluates to the value
        Ok(vec![value.clone()])
    }
}
```

---

## Rewrite Rules

### Rule Format

Rewrite rules are stored as `(= <pattern> <template>)`:

```metta
(= (factorial 0) 1)
(= (factorial $n) (* $n (factorial (- $n 1))))
```

### Rule Index

For efficient rule lookup:

```rust
use std::collections::HashMap;

pub struct RuleIndex {
    /// Map: head symbol → list of rules
    rules_by_head: HashMap<String, Vec<Rule>>,

    /// Catch-all rules (no specific head)
    generic_rules: Vec<Rule>,
}

pub struct Rule {
    pattern: Atom,
    template: Atom,
}

impl RuleIndex {
    pub fn new() -> Self {
        Self {
            rules_by_head: HashMap::new(),
            generic_rules: Vec::new(),
        }
    }

    pub fn add_rule(&mut self, pattern: Atom, template: Atom) {
        let rule = Rule { pattern: pattern.clone(), template };

        // Index by head symbol if possible
        if let Atom::Expression(expr) = &pattern {
            if let Some(Atom::Symbol(head)) = expr.children().first() {
                self.rules_by_head
                    .entry(head.name().to_string())
                    .or_insert_with(Vec::new)
                    .push(rule);
                return;
            }
        }

        // Otherwise, add to generic rules
        self.generic_rules.push(rule);
    }

    pub fn find_rules(&self, atom: &Atom) -> Vec<&Rule> {
        let mut rules = Vec::new();

        // Add specific rules
        if let Atom::Expression(expr) = atom {
            if let Some(Atom::Symbol(head)) = expr.children().first() {
                if let Some(specific_rules) = self.rules_by_head.get(head.name()) {
                    rules.extend(specific_rules.iter());
                }
            }
        }

        // Add generic rules
        rules.extend(self.generic_rules.iter());

        rules
    }

    pub fn build_from_space(space: &MorkSpace) -> Result<Self, BuildError> {
        let mut index = RuleIndex::new();

        // Query all rules: (= <pattern> <template>)
        let rule_pattern = expr!(sym!("="), var!("$p"), var!("$t"));
        let matches = space.query(&rule_pattern)?;

        for bindings in matches.alternatives() {
            let pattern = bindings.get("$p").unwrap().clone();
            let template = bindings.get("$t").unwrap().clone();
            index.add_rule(pattern, template);
        }

        Ok(index)
    }
}
```

### Using Rule Index

```rust
impl Evaluator {
    pub fn with_rule_index(space: Arc<MorkSpace>) -> Result<Self, BuildError> {
        let rule_index = RuleIndex::build_from_space(&space)?;

        Ok(Self {
            space,
            rule_index: Some(Arc::new(rule_index)),
            cache: None,
            max_depth: 1000,
        })
    }

    fn eval_expression_with_index(
        &self,
        expr: &ExpressionAtom,
        depth: usize,
    ) -> Result<Vec<Atom>, EvalError> {
        let expr_atom = Atom::Expression(expr.clone());

        if let Some(index) = &self.rule_index {
            let rules = index.find_rules(&expr_atom);

            let mut results = Vec::new();

            for rule in rules {
                // Try to match expression against rule pattern
                let match_result = self.match_atoms(&expr_atom, &rule.pattern)?;

                for match_bindings in match_result.alternatives() {
                    // Instantiate template with bindings
                    let instantiated = match_bindings.apply(&rule.template);

                    // Recursively evaluate
                    let evaluated = self.eval_with_depth(&instantiated, depth + 1)?;
                    results.extend(evaluated);
                }
            }

            if !results.is_empty() {
                return Ok(results);
            }
        }

        // Fall back to evaluating children
        self.eval_children(expr, depth)
    }
}
```

---

## Non-Determinism and Backtracking

### Non-Deterministic Evaluation

Multiple evaluation paths may exist:

```metta
; Space:
(= (foo) a)
(= (foo) b)
(= (bar a) 1)
(= (bar b) 2)

!(eval (bar (foo)))

; Paths:
; 1. (foo) → a, (bar a) → 1
; 2. (foo) → b, (bar b) → 2

; Results: [1, 2]
```

### Backtracking Stack

```rust
pub struct BacktrackPoint {
    alternatives: Vec<Atom>,
    continuation: Box<dyn Fn(&Atom) -> Result<Vec<Atom>, EvalError>>,
}

pub struct BacktrackStack {
    stack: Vec<BacktrackPoint>,
}

impl BacktrackStack {
    pub fn new() -> Self {
        Self { stack: Vec::new() }
    }

    pub fn push(&mut self, point: BacktrackPoint) {
        self.stack.push(point);
    }

    pub fn pop(&mut self) -> Option<BacktrackPoint> {
        self.stack.pop()
    }

    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }
}
```

### Depth-First Search Evaluation

```rust
impl Evaluator {
    pub fn eval_dfs(&self, atom: &Atom) -> Result<Vec<Atom>, EvalError> {
        let mut backtrack_stack = BacktrackStack::new();
        let mut results = Vec::new();

        // Initial evaluation
        let initial_results = self.eval(atom)?;

        for result in initial_results {
            self.explore_path(result, &mut backtrack_stack, &mut results)?;
        }

        Ok(results)
    }

    fn explore_path(
        &self,
        current: Atom,
        backtrack_stack: &mut BacktrackStack,
        results: &mut Vec<Atom>,
    ) -> Result<(), EvalError> {
        // Check if in normal form
        let next_results = self.eval(&current)?;

        if next_results.len() == 1 && next_results[0] == current {
            // Normal form reached
            results.push(current);
            return Ok(());
        }

        // Multiple paths - explore each
        for next in next_results {
            self.explore_path(next, backtrack_stack, results)?;
        }

        Ok(())
    }
}
```

---

## Grounded Functions

### Grounded Function Interface

```rust
pub trait GroundedFunctionTrait: Send + Sync {
    fn execute(&self, args: &[Atom]) -> Result<Vec<Atom>, EvalError>;
    fn name(&self) -> &str;
}
```

### Example: Arithmetic Functions

```rust
pub struct AddFunction;

impl GroundedFunctionTrait for AddFunction {
    fn execute(&self, args: &[Atom]) -> Result<Vec<Atom>, EvalError> {
        if args.len() != 2 {
            return Err(EvalError::ArityMismatch {
                expected: 2,
                got: args.len(),
            });
        }

        let a = extract_number(&args[0])?;
        let b = extract_number(&args[1])?;

        Ok(vec![Atom::Grounded(Grounded::from_number(a + b))])
    }

    fn name(&self) -> &str {
        "+"
    }
}

pub struct MultiplyFunction;

impl GroundedFunctionTrait for MultiplyFunction {
    fn execute(&self, args: &[Atom]) -> Result<Vec<Atom>, EvalError> {
        if args.len() != 2 {
            return Err(EvalError::ArityMismatch {
                expected: 2,
                got: args.len(),
            });
        }

        let a = extract_number(&args[0])?;
        let b = extract_number(&args[1])?;

        Ok(vec![Atom::Grounded(Grounded::from_number(a * b))])
    }

    fn name(&self) -> &str {
        "*"
    }
}

fn extract_number(atom: &Atom) -> Result<i64, EvalError> {
    match atom {
        Atom::Grounded(g) => {
            if let Some(n) = g.as_number() {
                Ok(n)
            } else {
                Err(EvalError::TypeMismatch {
                    expected: "Number",
                    got: g.type_name(),
                })
            }
        }
        _ => Err(EvalError::ExpectedGrounded),
    }
}
```

### Grounded Function Registry

```rust
use std::collections::HashMap;

pub struct FunctionRegistry {
    functions: RwLock<HashMap<String, Arc<dyn GroundedFunctionTrait>>>,
}

impl FunctionRegistry {
    pub fn new() -> Self {
        Self {
            functions: RwLock::new(HashMap::new()),
        }
    }

    pub fn register(&self, func: Arc<dyn GroundedFunctionTrait>) {
        self.functions.write().unwrap().insert(func.name().to_string(), func);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn GroundedFunctionTrait>> {
        self.functions.read().unwrap().get(name).cloned()
    }

    pub fn standard_library() -> Self {
        let registry = Self::new();

        registry.register(Arc::new(AddFunction));
        registry.register(Arc::new(MultiplyFunction));
        // ... register more functions

        registry
    }
}
```

### Evaluating Grounded Functions

```rust
impl Evaluator {
    pub fn with_functions(
        space: Arc<MorkSpace>,
        function_registry: Arc<FunctionRegistry>,
    ) -> Self {
        Self {
            space,
            function_registry: Some(function_registry),
            cache: None,
            max_depth: 1000,
            rule_index: None,
        }
    }

    fn eval_expression_with_functions(
        &self,
        expr: &ExpressionAtom,
        depth: usize,
    ) -> Result<Vec<Atom>, EvalError> {
        let children = expr.children();

        if children.is_empty() {
            return Ok(vec![Atom::Expression(expr.clone())]);
        }

        // Check if head is a grounded function
        if let Atom::Symbol(head) = &children[0] {
            if let Some(registry) = &self.function_registry {
                if let Some(func) = registry.get(head.name()) {
                    // Evaluate arguments first
                    let mut arg_results = Vec::new();

                    for arg in &children[1..] {
                        let results = self.eval_with_depth(arg, depth + 1)?;
                        arg_results.push(results);
                    }

                    // Cartesian product of argument results
                    let arg_combinations = cartesian_product(&arg_results);

                    // Execute function for each combination
                    let mut results = Vec::new();

                    for args in arg_combinations {
                        let func_results = func.execute(&args)?;
                        results.extend(func_results);
                    }

                    return Ok(results);
                }
            }
        }

        // Not a grounded function - continue with normal evaluation
        self.eval_expression(expr, depth)
    }
}
```

---

## Performance Optimization

### Memoization

Cache evaluation results:

```rust
impl Evaluator {
    pub fn eval_with_memo(&self, atom: &Atom) -> Result<Vec<Atom>, EvalError> {
        if let Some(cache) = &self.cache {
            if let Some(results) = cache.read().unwrap().get(atom) {
                return Ok(results.clone());
            }
        }

        let results = self.eval(atom)?;

        if let Some(cache) = &self.cache {
            cache.write().unwrap().put(atom.clone(), results.clone());
        }

        Ok(results)
    }
}
```

### Parallel Evaluation

Evaluate independent branches in parallel:

```rust
#[cfg(feature = "parallel")]
impl Evaluator {
    pub fn eval_parallel(&self, atoms: &[Atom]) -> Result<Vec<Vec<Atom>>, EvalError> {
        use rayon::prelude::*;

        atoms.par_iter()
            .map(|atom| self.eval(atom))
            .collect()
    }

    fn eval_children_parallel(
        &self,
        expr: &ExpressionAtom,
        depth: usize,
    ) -> Result<Vec<Atom>, EvalError> {
        use rayon::prelude::*;

        let children = expr.children();

        // Evaluate children in parallel
        let child_results: Vec<Vec<Atom>> = children.par_iter()
            .map(|child| self.eval_with_depth(child, depth + 1))
            .collect::<Result<Vec<_>, _>>()?;

        // Cartesian product
        let combinations = cartesian_product(&child_results);

        Ok(combinations.into_iter()
            .map(|combo| Atom::Expression(ExpressionAtom::new(combo)))
            .collect())
    }
}
```

### Tail Call Optimization

```rust
impl Evaluator {
    fn eval_tail_recursive(&self, atom: &Atom) -> Result<Vec<Atom>, EvalError> {
        let mut current = atom.clone();
        let mut depth = 0;

        loop {
            if depth > self.max_depth {
                return Err(EvalError::MaxDepthExceeded);
            }

            let results = self.eval_one_step(&current)?;

            if results.len() == 1 && results[0] == current {
                // Normal form
                return Ok(results);
            }

            if results.len() == 1 {
                // Single result - continue tail recursion
                current = results[0].clone();
                depth += 1;
            } else {
                // Multiple results - evaluate each
                let mut final_results = Vec::new();

                for result in results {
                    let sub_results = self.eval_tail_recursive(&result)?;
                    final_results.extend(sub_results);
                }

                return Ok(final_results);
            }
        }
    }

    fn eval_one_step(&self, atom: &Atom) -> Result<Vec<Atom>, EvalError> {
        // Perform one evaluation step only
        // (similar to eval, but doesn't recurse)
        todo!("Implement one-step evaluation")
    }
}
```

---

## Implementation Examples

### Complete Evaluation Example

```rust
use mork_eval::{Evaluator, FunctionRegistry};
use mork_space::MorkSpace;
use metta_atom::{atom, expr, sym, var};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create space
    let mut space = MorkSpace::new();

    // Add rules
    space.add(&expr!(sym!("="), expr!(sym!("factorial"), sym!("0")), sym!("1")))?;
    space.add(&expr!(
        sym!("="),
        expr!(sym!("factorial"), var!("$n")),
        expr!(
            sym!("*"),
            var!("$n"),
            expr!(sym!("factorial"), expr!(sym!("-"), var!("$n"), sym!("1")))
        )
    ))?;

    // Create evaluator with functions
    let func_registry = FunctionRegistry::standard_library();
    let evaluator = Evaluator::with_functions(Arc::new(space), Arc::new(func_registry));

    // Evaluate factorial(5)
    let expr = expr!(sym!("factorial"), sym!("5"));
    let results = evaluator.eval(&expr)?;

    println!("factorial(5) = {:?}", results);
    // Expected: [120]

    Ok(())
}
```

---

## Testing Strategy

### Unit Tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_eval_symbol() {
        let space = MorkSpace::new();
        let evaluator = Evaluator::new(Arc::new(space));

        let atom = atom!("foo");
        let results = evaluator.eval(&atom).unwrap();

        assert_eq!(results, vec![atom!("foo")]);
    }

    #[test]
    fn test_eval_simple_rule() {
        let mut space = MorkSpace::new();
        space.add(&expr!(sym!("="), sym!("foo"), sym!("bar"))).unwrap();

        let evaluator = Evaluator::new(Arc::new(space));

        let atom = atom!("foo");
        let results = evaluator.eval(&atom).unwrap();

        assert_eq!(results, vec![atom!("bar")]);
    }

    #[test]
    fn test_chain() {
        let mut space = MorkSpace::new();
        space.add(&expr!(sym!("="), expr!(sym!("numbers")), expr!(sym!("1"), sym!("2"), sym!("3")))).unwrap();

        let evaluator = Evaluator::new(Arc::new(space));

        let results = evaluator.chain(
            &expr!(sym!("numbers")),
            "$n",
            &var!("$n"),
        ).unwrap();

        assert_eq!(results.len(), 3);
    }
}
```

---

## Performance Benchmarks

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn bench_eval(c: &mut Criterion) {
    let mut space = MorkSpace::new();

    // Add factorial rules
    space.add(&expr!(sym!("="), expr!(sym!("fac"), sym!("0")), sym!("1"))).unwrap();
    space.add(&expr!(
        sym!("="),
        expr!(sym!("fac"), var!("$n")),
        expr!(sym!("*"), var!("$n"), expr!(sym!("fac"), expr!(sym!("-"), var!("$n"), sym!("1"))))
    )).unwrap();

    let func_registry = FunctionRegistry::standard_library();
    let evaluator = Evaluator::with_functions(Arc::new(space), Arc::new(func_registry));

    c.bench_function("eval_factorial_10", |b| {
        let expr = expr!(sym!("fac"), sym!("10"));
        b.iter(|| {
            evaluator.eval(black_box(&expr)).unwrap()
        });
    });
}

criterion_group!(benches, bench_eval);
criterion_main!(benches);
```

---

## Summary

This evaluation engine guide provides:

1. **Complete Evaluation Implementation**: eval, chain, unify, cons/decons, function/return
2. **Rule Management**: Efficient rule indexing and lookup
3. **Non-Determinism**: Backtracking and multiple results
4. **Grounded Functions**: Interface for native functions
5. **Performance Optimization**: Memoization, parallelization, tail call optimization

### Key Takeaways

- **Use rule indexing** for efficient pattern lookup
- **Memoize frequently evaluated expressions**
- **Parallelize independent evaluations** when possible
- **Implement tail call optimization** for recursive functions
- **Profile evaluation hot paths** and optimize bottlenecks

---

**Document Version**: 1.0
**Last Updated**: 2025-11-13
**Next Review**: After initial implementation and benchmarking
