# MeTTa-Rholang Integration Checklist

Quick reference checklist for integrating MeTTa compiler with Rholang.
See `DEPLOYMENT_GUIDE.md` for detailed instructions.

---

## Pre-Deployment Checklist

- [ ] MeTTa-Compiler repository available at `../MeTTa-Compiler`
- [ ] f1r3node repository checked out on branch `new_parser_path_map_support_full`
- [ ] Rust toolchain installed (1.70+)
- [ ] All MeTTa tests passing: `cd MeTTa-Compiler && cargo test`

---

## Deployment Steps

### ☐ Step 1: Add Dependency (2 minutes)

**File**: `f1r3node/rholang/Cargo.toml`

```toml
[dependencies]
mettatron = { path = "../../../MeTTa-Compiler" }
```

**Verify**:
```bash
cd f1r3node/rholang
cargo check
```

---

### ☐ Step 2: Add FFI Declarations (1 minute)

**File**: `f1r3node/rholang/src/rust/interpreter/system_processes.rs`

**At the top of the file**, add:

```rust
// MeTTa Compiler FFI
extern "C" {
    fn metta_compile(src: *const std::os::raw::c_char) -> *mut std::os::raw::c_char;
    fn metta_free_string(ptr: *mut std::os::raw::c_char);
}
```

---

### ☐ Step 3: Add Handler Function (5 minutes)

**File**: `f1r3node/rholang/src/rust/interpreter/system_processes.rs`

**Inside `impl SystemProcesses`**, add:

```rust
pub async fn metta_compile(
    &self,
    contract_args: (Vec<ListParWithRandom>, bool, Vec<Par>),
) -> Result<Vec<Par>, InterpreterError> {
    let Some((_, _, _, args)) = self.is_contract_call().unapply(contract_args) else {
        return Err(illegal_argument_error("metta_compile"));
    };

    let [arg] = args.as_slice() else {
        return Err(InterpreterError::IllegalArgumentException {
            message: "metta_compile: expected 1 argument (source code string)".to_string(),
        });
    };

    let src = self.pretty_printer.build_string_from_message(arg);

    use std::ffi::{CStr, CString};

    let src_cstr = CString::new(src.as_str())
        .map_err(|_| InterpreterError::IllegalArgumentException {
            message: "Invalid MeTTa source string (contains null byte)".to_string(),
        })?;

    let result_json = unsafe {
        let result_ptr = metta_compile(src_cstr.as_ptr());
        if result_ptr.is_null() {
            return Err(InterpreterError::IllegalArgumentException {
                message: "MeTTa compiler returned null".to_string(),
            });
        }
        let json_str = CStr::from_ptr(result_ptr).to_str()
            .map_err(|_| InterpreterError::IllegalArgumentException {
                message: "Invalid UTF-8 from MeTTa compiler".to_string(),
            })?
            .to_string();
        metta_free_string(result_ptr);
        json_str
    };

    Ok(vec![RhoString::create_par(result_json)])
}
```

**Source**: Copy from `MeTTa-Compiler/rholang_handler.rs`

---

### ☐ Step 4: Add Registry Function (3 minutes)

**File**: `f1r3node/rholang/src/rust/interpreter/system_processes.rs`

**Inside `impl SystemProcesses`**, add:

```rust
pub fn metta_contracts(&self) -> Vec<Definition> {
    vec![
        Definition {
            urn: "rho:metta:compile".to_string(),
            fixed_channel: FixedChannels::byte_name(200),  // ⚠️ Check for conflicts
            arity: 2,
            body_ref: 0,
            handler: {
                let sp = self.clone();
                Box::new(move |args| {
                    let sp = sp.clone();
                    Box::pin(async move { sp.metta_compile(args).await })
                })
            },
            remainder: None,
        },
    ]
}
```

**Source**: Copy from `MeTTa-Compiler/rholang_registry.rs`

**⚠️ Check**: Verify channel 200 is not used:
```bash
grep "FixedChannels::byte_name(200)" system_processes.rs
```

---

### ☐ Step 5: Register at Bootstrap (2 minutes)

**File**: Where system processes are initialized (likely `system_processes.rs`)

**Find** this pattern:
```rust
let mut all_defs = system_processes.test_framework_contracts();
```

**Add** this line after it:
```rust
all_defs.extend(system_processes.metta_contracts());
```

**Full example**:
```rust
let system_processes = SystemProcesses::new(/* ... */);
let mut all_defs = system_processes.test_framework_contracts();
all_defs.extend(system_processes.metta_contracts());  // ← Add this
```

---

### ☐ Step 6: Build (5-10 minutes)

```bash
cd f1r3node/rholang
RUSTFLAGS="-C target-cpu=native" cargo build --release
```

**Expected**:
```
   Compiling mettatron v0.1.0
   Compiling rholang v0.x.x
    Finished `release` profile
```

**Check binary**:
```bash
ls -lh target/release/rholang
```

---

### ☐ Step 7: Test (3 minutes)

**Create** `test_metta.rho`:

```rholang
new compile(`rho:metta:compile`), ack in {
  compile!("(+ 1 2)", *ack) |
  for (@result <- ack) {
    stdoutAck!(result, *ack)
  }
}
```

**Run**:
```bash
./target/release/rholang test_metta.rho
```

**Expected output**:
```json
{"success":true,"exprs":[{"type":"sexpr","items":[{"type":"atom","value":"add"},{"type":"number","value":1},{"type":"number","value":2}]}]}
```

---

## Verification Checklist

After deployment, verify:

- [ ] Build completes without errors
- [ ] Binary exists: `target/release/rholang`
- [ ] Test script produces JSON output
- [ ] Valid MeTTa: `{"success":true,...}`
- [ ] Invalid MeTTa: `{"success":false,"error":"..."}`
- [ ] Service accessible via `@"rho:metta:compile"`

---

## Quick Test Suite

**Test 1: Valid expression**
```rholang
new compile(`rho:metta:compile`), ack in {
  compile!("(+ 1 2)", *ack) |
  for (@result <- ack) { stdoutAck!(result, *ack) }
}
```
Expected: `"success":true`

**Test 2: Nested expression**
```rholang
new compile(`rho:metta:compile`), ack in {
  compile!("(+ 1 (* 2 3))", *ack) |
  for (@result <- ack) { stdoutAck!(result, *ack) }
}
```
Expected: `"success":true` with nested sexpr

**Test 3: Syntax error**
```rholang
new compile(`rho:metta:compile`), ack in {
  compile!("(+ 1 2", *ack) |
  for (@result <- ack) { stdoutAck!(result, *ack) }
}
```
Expected: `"success":false`

---

## Common Issues

### ❌ "undefined reference to `metta_compile`"
**Fix**: Verify `MeTTa-Compiler/Cargo.toml` has:
```toml
[lib]
crate-type = ["rlib", "cdylib"]
```

### ❌ "MeTTa compiler returned null"
**Fix**: Check FFI function names match between:
- `system_processes.rs`: `fn metta_compile(...)`
- `MeTTa-Compiler/src/ffi.rs`: `pub unsafe extern "C" fn metta_compile(...)`

### ❌ Channel conflict
**Fix**: Change channel number:
```rust
fixed_channel: FixedChannels::byte_name(201),  // Try 201, 202, etc.
```

### ❌ Many MORK warnings
**Fix**: Expected and safe to ignore. To suppress:
```bash
RUSTFLAGS="-A warnings -C target-cpu=native" cargo build --release
```

---

## Files Modified

Summary of changes:

1. ✏️ `f1r3node/rholang/Cargo.toml` - Added dependency
2. ✏️ `f1r3node/rholang/src/rust/interpreter/system_processes.rs` - Added:
   - FFI declarations (top of file)
   - `metta_compile` handler (in `impl SystemProcesses`)
   - `metta_contracts` registry (in `impl SystemProcesses`)
   - Registration call (in bootstrap/init)

---

## Time Estimate

- **Step 1-5**: ~15 minutes (code changes)
- **Step 6**: ~5-10 minutes (build)
- **Step 7**: ~3 minutes (testing)
- **Total**: ~20-30 minutes

---

## Reference Files

All code is ready to copy from:
- `MeTTa-Compiler/rholang_handler.rs` - Handler function
- `MeTTa-Compiler/rholang_registry.rs` - Registry function
- `MeTTa-Compiler/examples/metta_rholang_example.rho` - Usage examples

---

## Detailed Documentation

- **Full Guide**: `DEPLOYMENT_GUIDE.md` (comprehensive step-by-step)
- **Summary**: `RHOLANG_INTEGRATION_SUMMARY.md` (quick reference)
- **Technical Details**: `docs/RHOLANG_INTEGRATION.md` (architecture)

---

## Success Criteria

✅ Integration is successful when:

1. Build completes: `cargo build --release`
2. Test passes: Valid MeTTa returns `{"success":true,...}`
3. Errors handled: Invalid MeTTa returns `{"success":false,...}`
4. Service works: `@"rho:metta:compile"!("(+ 1 2)", *result)`

---

**Status**: MeTTa side complete (85/85 tests + 16/16 FFI tests passing)
**Ready**: For Rholang deployment
**Estimated Time**: 20-30 minutes
