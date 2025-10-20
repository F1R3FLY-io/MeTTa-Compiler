# MeTTa Built-in Functions Implementation TODO

This document tracks the implementation status of MeTTa built-in functions in MeTTaTron against the official Hyperon implementation.

**Reference Repository:** https://github.com/trueagi-io/hyperon-experimental

**Primary Sources:**
- [stdlib.metta](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta) - Standard library definitions and documentation
- [core.rs](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/core.rs) - Core operations (match, if-equal, superpose, etc.)
- [atom.rs](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/atom.rs) - Atom operations (get-type, min-atom, etc.)
- [space.rs](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/space.rs) - Space operations
- [arithmetics.rs](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/arithmetics.rs) - Math operations

## Core Special Forms

### Pattern Matching & Rules
- [x] **`=`** - Define reduction rules for expressions
  - Location: `src/backend/eval.rs:52`
  - Reference: [stdlib.metta:7](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L7)

### Evaluation Control
- [x] **`!`** - Force evaluation (custom prefix operator in MeTTaTron)
  - Location: `src/backend/eval.rs:74`
  - Note: In official MeTTa, this is implicit; MeTTaTron uses `!` as explicit eval operator

- [x] **`eval`** - Evaluates input atom (one step)
  - Location: `src/backend/eval.rs:130`
  - Reference: [stdlib.metta:56-61](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L56-L61)

- [ ] **`evalc`** - Evaluates atom in specific space context
  - Reference: [stdlib.metta:63-69](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L63-L69)

- [ ] **`function`** - Evaluates until `(return <result>)`
  - Reference: [stdlib.metta:49-54](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L49-L54)

- [ ] **`return`** - Returns value from function expressions
  - Reference: [stdlib.metta:42-47](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L42-L47)

- [ ] **`chain`** - Evaluates first arg, binds to variable, evaluates third arg
  - Reference: [stdlib.metta:71-78](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L71-L78)

- [ ] **`unify`** - Pattern matches two args, returns third if matched, fourth otherwise
  - Reference: [stdlib.metta:80-88](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L80-L88)

### Quoting
- [x] **`quote`** - Prevents atom from being reduced
  - Location: `src/backend/eval.rs:88`
  - Reference: [stdlib.metta:598-604](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L598-L604)

- [ ] **`unquote`** - Unquotes quoted atom
  - Reference: [stdlib.metta:606-612](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L606-L612)

## Control Flow

- [x] **`if`** - Conditional: `(if cond then else)`
  - Location: `src/backend/eval.rs:101`
  - Reference: [stdlib.metta:510-519](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L510-L519)

- [ ] **`case`** - Pattern matching with multiple conditions
  - Reference: [stdlib.metta:1193-1204](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1193-L1204)

- [ ] **`switch`** - Similar to case but handles Empty differently
  - Reference: [stdlib.metta:339-347](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L339-L347)

## Variable Binding

- [x] **`let`** - Unifies two args and applies result to third arg
  - Location: `src/backend/eval.rs:178`
  - Reference: [stdlib.metta:541-550](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L541-L550)

- [ ] **`let*`** - Sequential let with list of pairs
  - Reference: [stdlib.metta:552-565](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L552-L565)

## Type System

- [x] **`:`** - Type assertion `(: expr type)`
  - Location: `src/backend/eval.rs:184`
  - Note: Adds type assertions to environment and MORK Space

- [x] **`get-type`** - Returns type of an atom
  - Location: `src/backend/eval.rs:222`
  - Reference: [stdlib.metta:961-965](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L961-L965), [atom.rs:448](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/atom.rs#L448)

- [ ] **`get-type-space`** - Returns type relative to a specific space
  - Reference: [stdlib.metta:967-972](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L967-L972), [atom.rs:453](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/atom.rs#L453)

- [ ] **`get-metatype`** - Returns metatype (Symbol, Expression, Grounded, Variable)
  - Reference: [stdlib.metta:974-978](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L974-L978), [atom.rs:455](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/atom.rs#L455)

- [x] **`check-type`** - Checks if atom matches expected type
  - Location: `src/backend/eval.rs:238`
  - Reference: [stdlib.metta](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta)

- [ ] **`type-cast`** - Casts atom to a type
  - Reference: [stdlib.metta:387-403](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L387-L403)

- [ ] **`is-function`** - Checks if type is a function type
  - Reference: [stdlib.metta:371-385](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L371-L385)

- [ ] **`match-types`** - Checks if two types can be unified
  - Reference: [stdlib.metta:405-422](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L405-L422)

## Expression Manipulation

- [ ] **`cons-atom`** - Constructs expression from head and tail
  - Reference: [stdlib.metta:90-96](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L90-L96)

- [ ] **`decons-atom`** - Deconstructs expression into head and tail
  - Reference: [stdlib.metta:98-103](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L98-L103)

- [ ] **`car-atom`** - Extracts first atom of expression
  - Reference: [stdlib.metta:576-585](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L576-L585)

- [ ] **`cdr-atom`** - Extracts tail of expression
  - Reference: [stdlib.metta:587-596](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L587-L596)

- [ ] **`size-atom`** - Returns size of expression
  - Reference: [stdlib.metta:123-127](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L123-L127), [atom.rs:461](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/atom.rs#L461)

- [ ] **`index-atom`** - Returns atom at given index in expression
  - Reference: [stdlib.metta:129-134](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L129-L134), [atom.rs:463](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/atom.rs#L463)

## Arithmetic Operations

- [x] **`+`** - Addition
  - Location: `src/backend/eval.rs:500`
  - Reference: [stdlib.metta:1278-1283](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1278-L1283)

- [x] **`-`** - Subtraction
  - Location: `src/backend/eval.rs:501`
  - Reference: [stdlib.metta:1285-1290](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1285-L1290)

- [x] **`*`** - Multiplication
  - Location: `src/backend/eval.rs:502`
  - Reference: [stdlib.metta:1292-1297](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1292-L1297)

- [x] **`/`** - Division
  - Location: `src/backend/eval.rs:503`
  - Reference: [stdlib.metta:1299-1304](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1299-L1304)

- [ ] **`%`** - Modulo
  - Reference: [stdlib.metta:1306-1311](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1306-L1311)

## Math Functions

- [ ] **`pow-math`** - Power function (base ^ power)
  - Reference: [stdlib.metta:136-141](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L136-L141)

- [ ] **`sqrt-math`** - Square root
  - Reference: [stdlib.metta:143-147](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L143-L147)

- [ ] **`abs-math`** - Absolute value
  - Reference: [stdlib.metta:149-153](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L149-L153)

- [ ] **`log-math`** - Logarithm
  - Reference: [stdlib.metta:155-160](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L155-L160)

- [ ] **`min-atom`** - Minimum value in expression
  - Reference: [stdlib.metta:111-115](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L111-L115), [atom.rs:457](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/atom.rs#L457)

- [ ] **`max-atom`** - Maximum value in expression
  - Reference: [stdlib.metta:117-121](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L117-L121), [atom.rs:459](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/atom.rs#L459)

### Rounding Functions
- [ ] **`trunc-math`** - Integer part of value
  - Reference: [stdlib.metta:162-166](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L162-L166)

- [ ] **`ceil-math`** - Smallest integer >= value
  - Reference: [stdlib.metta:168-172](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L168-L172)

- [ ] **`floor-math`** - Smallest integer <= value
  - Reference: [stdlib.metta:174-178](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L174-L178)

- [ ] **`round-math`** - Nearest integer to value
  - Reference: [stdlib.metta:180-184](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L180-L184)

### Trigonometric Functions
- [ ] **`sin-math`** - Sine function
  - Reference: [stdlib.metta:186-190](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L186-L190)

- [ ] **`asin-math`** - Arcsine function
  - Reference: [stdlib.metta:192-196](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L192-L196)

- [ ] **`cos-math`** - Cosine function
  - Reference: [stdlib.metta:198-202](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L198-L202)

- [ ] **`acos-math`** - Arccosine function
  - Reference: [stdlib.metta:204-208](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L204-L208)

- [ ] **`tan-math`** - Tangent function
  - Reference: [stdlib.metta:210-214](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L210-L214)

- [ ] **`atan-math`** - Arctangent function
  - Reference: [stdlib.metta:216-220](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L216-L220)

### Special Value Checks
- [ ] **`isnan-math`** - Returns True if value is NaN
  - Reference: [stdlib.metta:222-226](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L222-L226)

- [ ] **`isinf-math`** - Returns True if value is ±infinity
  - Reference: [stdlib.metta:228-232](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L228-L232)

## Comparison Operations

- [x] **`<`** - Less than
  - Location: `src/backend/eval.rs:504`
  - Reference: [stdlib.metta:1313-1318](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1313-L1318)

- [x] **`<=`** - Less than or equal
  - Location: `src/backend/eval.rs:505`
  - Reference: [stdlib.metta:1327-1332](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1327-L1332)

- [x] **`>`** - Greater than
  - Location: `src/backend/eval.rs:506`
  - Reference: [stdlib.metta:1320-1325](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1320-L1325)

- [x] **`>=`** - Greater than or equal
  - Location: `src/backend/eval.rs:507`
  - Reference: [stdlib.metta:1334-1339](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1334-L1339)

- [x] **`==`** - Equality check
  - Location: `src/backend/eval.rs:508`
  - Reference: [stdlib.metta:1341-1346](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1341-L1346), [core.rs:279](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/core.rs#L279)

- [x] **`!=`** - Not equal (MeTTaTron extension)
  - Location: `src/backend/eval.rs:509`
  - Note: Not in official MeTTa stdlib

- [ ] **`=alpha`** - Alpha equality check
  - Reference: [stdlib.metta:1056-1061](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1056-L1061)

- [ ] **`if-equal`** - Checks equality without matching
  - Reference: [stdlib.metta:980-987](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L980-L987), [core.rs:271](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/core.rs#L271)

## Logical Operations

- [ ] **`and`** - Logical conjunction
  - Reference: [stdlib.metta:528-533](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L528-L533)

- [ ] **`or`** - Logical disjunction
  - Reference: [stdlib.metta:521-526](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L521-L526)

- [ ] **`not`** - Logical negation
  - Reference: [stdlib.metta:535-539](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L535-L539)

- [ ] **`xor`** - Logical exclusive or
  - Reference: [stdlib.metta:1348-1353](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1348-L1353)

## Error Handling

- [x] **`error`** - Error constructor
  - Location: `src/backend/eval.rs:106`
  - Reference: [stdlib.metta:34-40](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L34-L40)

- [x] **`catch`** - Error recovery `(catch expr default)`
  - Location: `src/backend/eval.rs:124`
  - Note: MeTTaTron implementation; official MeTTa uses different error handling

- [x] **`is-error`** - Checks if value is an error
  - Location: `src/backend/eval.rs:150`
  - Note: MeTTaTron implementation; official MeTTa uses `if-error`

- [ ] **`if-error`** - Checks if atom is error
  - Reference: [stdlib.metta:300-317](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L300-L317)

- [ ] **`return-on-error`** - Returns on error or continues
  - Reference: [stdlib.metta:319-329](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L319-L329)

### Error Types
- [ ] **`BadType`** - BadType error constructor
  - Reference: [stdlib.metta:15-21](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L15-L21)

- [ ] **`BadArgType`** - BadArgType error constructor
  - Reference: [stdlib.metta:23-30](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L23-L30)

- [ ] **`IncorrectNumberOfArguments`** - Error type
  - Reference: [stdlib.metta:32](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L32)

## Non-deterministic Operations

- [ ] **`superpose`** - Turns tuple into non-deterministic result
  - Reference: [stdlib.metta:1167-1171](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1167-L1171), [core.rs:286](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/core.rs#L286)

- [ ] **`collapse`** - Converts non-deterministic result into tuple
  - Reference: [stdlib.metta:1173-1184](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1173-L1184)

- [ ] **`collapse-bind`** - Returns all alternative evaluations with bindings
  - Reference: [stdlib.metta:234-239](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L234-L239)

- [ ] **`superpose-bind`** - Complement to collapse-bind
  - Reference: [stdlib.metta:241-246](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L241-L246)

## Set Operations

- [ ] **`unique`** / **`unique-atom`** - Returns unique elements
  - Reference: [stdlib.metta:630-636](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L630-L636), [stdlib.metta:1355-1359](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1355-L1359), [atom.rs:465](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/atom.rs#L465)

- [ ] **`union`** / **`union-atom`** - Union of two sets
  - Reference: [stdlib.metta:638-647](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L638-L647), [stdlib.metta:1361-1366](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1361-L1366), [atom.rs:471](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/atom.rs#L471)

- [ ] **`intersection`** / **`intersection-atom`** - Intersection of two sets
  - Reference: [stdlib.metta:649-658](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L649-L658), [stdlib.metta:1368-1373](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1368-L1373), [atom.rs:469](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/atom.rs#L469)

- [ ] **`subtraction`** / **`subtraction-atom`** - Set subtraction
  - Reference: [stdlib.metta:660-669](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L660-L669), [stdlib.metta:1375-1380](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1375-L1380), [atom.rs:467](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/atom.rs#L467)

## Space Operations

- [x] **`match`** - Searches space for pattern (partial implementation)
  - Location: `src/backend/eval.rs:171`
  - Reference: [stdlib.metta:1031-1037](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1031-L1037), [core.rs:275](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/core.rs#L275)
  - Note: MeTTaTron uses MORK Space for pattern matching

- [ ] **`add-atom`** - Adds atom to space without reducing
  - Reference: [stdlib.metta:954-959](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L954-L959)

- [ ] **`add-reduct`** - Reduces and adds atom to space
  - Reference: [stdlib.metta:567-574](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L567-L574)

- [ ] **`add-reducts`** - Adds multiple atoms after evaluation
  - Reference: [stdlib.metta:671-679](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L671-L679)

- [ ] **`add-atoms`** - Adds multiple atoms without reducing
  - Reference: [stdlib.metta:681-689](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L681-L689)

- [ ] **`remove-atom`** - Removes atom from space
  - Reference: [stdlib.metta:994-999](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L994-L999)

- [ ] **`get-atoms`** - Returns all atoms in space
  - Reference: [stdlib.metta:1001-1005](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1001-L1005)

- [ ] **`new-space`** - Creates new atomspace
  - Reference: [stdlib.metta:989-992](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L989-L992)

- [ ] **`context-space`** - Returns current context space
  - Reference: [stdlib.metta:105-109](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L105-L109)

## List Operations

- [ ] **`map-atom`** - Maps function over list
  - Reference: [stdlib.metta:467-483](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L467-L483)

- [ ] **`filter-atom`** - Filters list with predicate
  - Reference: [stdlib.metta:447-465](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L447-L465)

- [ ] **`foldl-atom`** - Left fold over list
  - Reference: [stdlib.metta:485-504](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L485-L504)

## Testing & Assertions

- [ ] **`assert`** - Basic assertion
  - Reference: [stdlib.metta:706-710](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L706-L710)

- [ ] **`assertEqual`** - Compares evaluation results
  - Reference: [stdlib.metta:1063-1074](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1063-L1074)

- [ ] **`assertEqualMsg`** - assertEqual with custom message
  - Reference: [stdlib.metta:1076-1088](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1076-L1088)

- [ ] **`assertEqualToResult`** - Compares with unevaluated expected result
  - Reference: [stdlib.metta:1117-1127](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1117-L1127)

- [ ] **`assertEqualToResultMsg`** - assertEqualToResult with custom message
  - Reference: [stdlib.metta:1129-1140](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1129-L1140)

- [ ] **`assertAlphaEqual`** - Alpha equality comparison
  - Reference: [stdlib.metta:1090-1101](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1090-L1101)

- [ ] **`assertAlphaEqualMsg`** - assertAlphaEqual with custom message
  - Reference: [stdlib.metta:1103-1115](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1103-L1115)

- [ ] **`assertAlphaEqualToResult`** - Alpha equality with unevaluated result
  - Reference: [stdlib.metta:1142-1152](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1142-L1152)

- [ ] **`assertAlphaEqualToResultMsg`** - assertAlphaEqualToResult with message
  - Reference: [stdlib.metta:1154-1165](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1154-L1165)

- [ ] **`assertIncludes`** - Checks if content included in results
  - Reference: [stdlib.metta:691-704](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L691-L704)

## I/O & System

- [ ] **`println!`** - Prints to console
  - Reference: [stdlib.metta:1250-1254](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1250-L1254)

- [ ] **`trace!`** - Prints first arg and returns second
  - Reference: [stdlib.metta:1243-1248](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1243-L1248)

- [ ] **`format-args`** - String formatting with `{}`
  - Reference: [stdlib.metta:1256-1261](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1256-L1261)

- [ ] **`help!`** - Shows documentation
  - Reference: [stdlib.metta:882-914](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L882-L914)

- [ ] **`pragma!`** - Changes global settings
  - Reference: [stdlib.metta:1212-1221](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1212-L1221), [core.rs:270](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/core.rs#L270)

## Module System

- [ ] **`import!`** - Imports module
  - Reference: [stdlib.metta:1223-1228](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1223-L1228)

- [ ] **`include`** - Includes file in current space
  - Reference: [stdlib.metta:1230-1234](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1230-L1234)

- [ ] **`bind!`** - Registers token replacement
  - Reference: [stdlib.metta:1236-1241](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1236-L1241)

- [ ] **`register-module!`** - Loads module from filesystem
  - Reference: [stdlib.metta:1039-1043](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1039-L1043)

- [ ] **`mod-space!`** - Returns module space
  - Reference: [stdlib.metta:1045-1049](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1045-L1049)

- [ ] **`print-mods!`** - Prints all modules
  - Reference: [stdlib.metta:1051-1054](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1051-L1054)

- [ ] **`git-module!`** - Access remote git repo module
  - Reference: [stdlib.metta:1382-1386](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1382-L1386)

## Utility Functions

- [ ] **`id`** - Identity function (returns argument)
  - Reference: [stdlib.metta:257-263](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L257-L263)

- [ ] **`noeval`** - Returns argument without evaluation
  - Reference: [stdlib.metta:265-271](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L265-L271)

- [ ] **`nop`** - No operation (returns unit)
  - Reference: [stdlib.metta:616-621](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L616-L621), [core.rs:273](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/core.rs#L273)

- [ ] **`empty`** - Returns Empty atom
  - Reference: [stdlib.metta:623-628](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L623-L628)

- [ ] **`atom-subst`** - Substitutes variable in template
  - Reference: [stdlib.metta:273-282](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L273-L282)

- [ ] **`sealed`** - Creates locally scoped variables
  - Reference: [stdlib.metta:1263-1268](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1263-L1268), [core.rs:277](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/core.rs#L277)

- [ ] **`capture`** - Wraps atom and captures current space
  - Reference: [stdlib.metta:1206-1210](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1206-L1210), [core.rs:291](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/core.rs#L291)

- [ ] **`metta`** - Runs MeTTa interpreter on atom
  - Reference: [stdlib.metta:248-255](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L248-L255)

- [ ] **`if-decons-expr`** - Conditional deconstruction
  - Reference: [stdlib.metta:284-298](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L284-L298)

- [ ] **`first-from-pair`** - Gets first atom from pair
  - Reference: [stdlib.metta:424-433](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L424-L433)

- [ ] **`match-type-or`** - OR operation for type matching
  - Reference: [stdlib.metta:435-445](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L435-L445)

- [ ] **`noreduce-eq`** - Checks equality without reducing
  - Reference: [stdlib.metta:941-948](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L941-L948)

- [ ] **`for-each-in-atom`** - Applies function to each atom in expression
  - Reference: [stdlib.metta:926-939](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L926-L939)

## State Management

- [ ] **`new-state`** - Creates state atom
  - Reference: [stdlib.metta:1007-1016](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1007-L1016)

- [ ] **`get-state`** - Gets wrapped value from state
  - Reference: [stdlib.metta:1025-1029](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1025-L1029)

- [ ] **`change-state!`** - Changes state value
  - Reference: [stdlib.metta:1018-1023](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L1018-L1023)

## Documentation System

- [ ] **`@doc`** - Function/atom documentation
  - Reference: [stdlib.metta:715-724](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L715-L724)

- [ ] **`@desc`** - Description field
  - Reference: [stdlib.metta:726-731](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L726-L731)

- [ ] **`@param`** - Parameter description
  - Reference: [stdlib.metta:733-739](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L733-L739)

- [ ] **`@return`** - Return value description
  - Reference: [stdlib.metta:741-747](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L741-L747)

- [ ] **`@doc-formal`** - Formal documentation structure
  - Reference: [stdlib.metta:749-761](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L749-L761)

- [ ] **`@item`** - Documentation item marker
  - Reference: [stdlib.metta:763-768](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L763-L768)

- [ ] **`@kind`** - Entity kind (function/atom)
  - Reference: [stdlib.metta:773-779](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L773-L779)

- [ ] **`@type`** - Type annotation
  - Reference: [stdlib.metta:781-786](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L781-L786)

- [ ] **`@params`** - Parameters list container
  - Reference: [stdlib.metta:788-793](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L788-L793)

- [ ] **`get-doc`** - Retrieves documentation
  - Reference: [stdlib.metta:795-805](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L795-L805)

- [ ] **`get-doc-atom`** - Gets documentation for atom
  - Reference: [stdlib.metta:867-880](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L867-L880)

- [ ] **`get-doc-function`** - Gets documentation for function
  - Reference: [stdlib.metta:820-833](https://github.com/trueagi-io/hyperon-experimental/blob/main/lib/src/metta/runner/stdlib/stdlib.metta#L820-L833)

## Summary Statistics

- **Total Functions:** 147
- **Implemented:** 22 (15.0%)
- **Not Implemented:** 125 (85.0%)

## Implementation Notes

### MeTTaTron-Specific Features
- Uses `!` as an explicit evaluation operator (not in official MeTTa)
- Implements `!=` (not equal) comparison operator
- Uses MORK Space for pattern matching and rule storage
- Supports multiply-defined patterns/rules with Cartesian products

### Key Differences from Official MeTTa
1. **Evaluation Model:** MeTTaTron uses explicit `!` operator; official MeTTa has implicit evaluation
2. **Error Handling:** MeTTaTron has `is-error`; official MeTTa uses `if-error`
3. **Space Integration:** MeTTaTron deeply integrates with MORK Space
4. **Type System:** Basic type support in MeTTaTron; official MeTTa has more comprehensive type checking

### Priority Implementation Targets
For maximum compatibility with official MeTTa, prioritize:
1. Expression manipulation: `cons-atom`, `decons-atom`, `car-atom`, `cdr-atom`
2. Non-deterministic ops: `superpose`, `collapse`
3. Control flow: `case`, `switch`
4. List operations: `map-atom`, `filter-atom`, `foldl-atom`
5. Logical operations: `and`, `or`, `not`
6. Advanced evaluation: `chain`, `unify`, `function`, `return`

---

Last Updated: 2025-10-20
