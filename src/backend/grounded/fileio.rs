//! File-IO grounded operations.
//!
//! HE-bisim source: `hyperon-experimental/lib/src/metta/runner/builtin_mods/fileio.rs`.
//!
//! Operations registered:
//!   - `(file-open! "path" "options") -> FileHandle`
//!         options: "r" read, "w" write, "c" create, "a" append, "t" truncate
//!   - `(file-read-to-string! $fh) -> String`
//!         read from current cursor to EOF
//!   - `(file-write! $fh "content") -> ()`
//!   - `(file-seek! $fh $offset) -> ()`
//!   - `(file-read-exact! $fh $nbytes) -> String`
//!   - `(file-get-size! $fh) -> Number`
//!
//! HE stores the `std::fs::File` inside a `Rc<RefCell<File>>` wrapped in a
//! `FileHandle` grounded atom. MeTTaTron values are immutable persistent
//! slab pointers, so we represent a FileHandle as the symbolic shape
//! `(FileHandle <id>)` where `<id>` is a `Long` index into a global
//! handle registry. The actual `std::fs::File` lives in the registry
//! protected by a `Mutex`.
//!
//! Handles are never freed during a session — sessions are short-lived
//! (one mettatron invocation), and OS file descriptors are released when
//! the process exits. A future enhancement could expose `(file-close! $fh)`
//! to drop a handle eagerly.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use super::state::{find_error, GroundedState, GroundedWork};
use super::traits::GroundedOperationTCO;
use crate::backend::grounded::ExecError;
use crate::backend::models::{MettaValueFactory, MettaValueTrait};

/// Global file-handle registry. Mutex-protected for thread safety because
/// MeTTaTron evaluates expressions across worker threads.
static FILE_HANDLES: OnceLock<Mutex<HashMap<u64, File>>> = OnceLock::new();
static NEXT_HANDLE_ID: AtomicU64 = AtomicU64::new(1);

fn handles() -> &'static Mutex<HashMap<u64, File>> {
    FILE_HANDLES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Build a `(FileHandle <id>)` SExpr to represent a handle value.
fn handle_value<V, F>(id: u64, factory: &F) -> V
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    factory.sexpr(vec![factory.atom("FileHandle"), factory.long(id as i64)])
}

/// Extract the handle id from a `(FileHandle <id>)` value, or return None.
fn extract_handle_id<V: MettaValueTrait>(value: &V) -> Option<u64> {
    let items = value.as_sexpr()?;
    if items.len() != 2 {
        return None;
    }
    if items[0].as_atom() != Some("FileHandle") {
        return None;
    }
    let id = items[1].as_long()?;
    if id < 0 {
        return None;
    }
    Some(id as u64)
}

/// `(file-open! <String:path> <String:options>) -> FileHandle`.
pub struct FileOpenOp;

impl<V: MettaValueTrait + Clone> GroundedOperationTCO<V> for FileOpenOp {
    fn name(&self) -> &str {
        "file-open!"
    }

    fn execute_step<F: MettaValueFactory<V>>(
        &self,
        state: &mut GroundedState<V>,
        factory: &F,
    ) -> GroundedWork<V> {
        match state.step {
            0 => {
                if state.args.len() != 2 {
                    return GroundedWork::Error(ExecError::Tagged(
                        "IncorrectNumberOfArguments",
                    ));
                }
                state.step = 1;
                GroundedWork::EvalArg {
                    arg_idx: 0,
                    state: state.clone(),
                }
            }
            1 => {
                if let Some(err) = find_error(state.get_arg(0).expect("arg 0")) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }
                state.step = 2;
                GroundedWork::EvalArg {
                    arg_idx: 1,
                    state: state.clone(),
                }
            }
            2 => {
                let path_arg = state.get_arg(0).expect("arg 0 should be evaluated");
                let opt_arg = state.get_arg(1).expect("arg 1 should be evaluated");
                if let Some(err) = find_error(opt_arg) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                // Cartesian product of evaluated arguments (matches the
                // pattern used by other grounded ops).
                let mut results: Vec<(V, Option<_>)> =
                    Vec::with_capacity(path_arg.len() * opt_arg.len());
                for pv in path_arg {
                    let Some(path) = pv.as_string() else {
                        return GroundedWork::Error(ExecError::BadArgType {
                            pos: 1,
                            expected: "String",
                            got: pv.friendly_type_name().to_string(),
                        });
                    };
                    for ov in opt_arg {
                        let Some(options) = ov.as_string() else {
                            return GroundedWork::Error(ExecError::BadArgType {
                                pos: 2,
                                expected: "String",
                                got: ov.friendly_type_name().to_string(),
                            });
                        };

                        // Translate option-string flags to OpenOptions.
                        // HE source: builtin_mods/fileio.rs::FileHandle::open.
                        let mut opts = OpenOptions::new();
                        opts.read(options.contains('r'))
                            .write(options.contains('w'))
                            .create(options.contains('c'))
                            .append(options.contains('a'))
                            .truncate(options.contains('t'));

                        match opts.open(path) {
                            Ok(file) => {
                                let id = NEXT_HANDLE_ID.fetch_add(1, Ordering::Relaxed);
                                handles()
                                    .lock()
                                    .expect("file handle registry mutex poisoned")
                                    .insert(id, file);
                                results.push((handle_value(id, factory), None));
                            }
                            Err(_) => {
                                return GroundedWork::Error(ExecError::Runtime(format!(
                                    "Failed to open file with provided path={} and options={}",
                                    path, options
                                )));
                            }
                        }
                    }
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for file-open! operation", state.step),
        }
    }
}

/// `(file-read-to-string! <FileHandle>) -> String`.
pub struct FileReadToStringOp;

impl<V: MettaValueTrait + Clone> GroundedOperationTCO<V> for FileReadToStringOp {
    fn name(&self) -> &str {
        "file-read-to-string!"
    }

    fn execute_step<F: MettaValueFactory<V>>(
        &self,
        state: &mut GroundedState<V>,
        factory: &F,
    ) -> GroundedWork<V> {
        match state.step {
            0 => {
                if state.args.len() != 1 {
                    return GroundedWork::Error(ExecError::Tagged(
                        "IncorrectNumberOfArguments",
                    ));
                }
                state.step = 1;
                GroundedWork::EvalArg {
                    arg_idx: 0,
                    state: state.clone(),
                }
            }
            1 => {
                let arg = state.get_arg(0).expect("arg 0 should be evaluated");
                if let Some(err) = find_error(arg) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut results = Vec::with_capacity(arg.len());
                for value in arg {
                    let Some(id) = extract_handle_id(value) else {
                        return GroundedWork::Error(ExecError::Runtime(
                            "file-read-to-string! expects filehandle as an argument"
                                .to_string(),
                        ));
                    };

                    let mut registry = handles()
                        .lock()
                        .expect("file handle registry mutex poisoned");
                    let file = registry.get_mut(&id).ok_or_else(|| {
                        ExecError::Runtime(format!(
                            "file-read-to-string!: unknown FileHandle id {}",
                            id
                        ))
                    });
                    let file = match file {
                        Ok(f) => f,
                        Err(e) => return GroundedWork::Error(e),
                    };

                    let mut contents = String::new();
                    if let Err(message) = file.read_to_string(&mut contents) {
                        return GroundedWork::Error(ExecError::Runtime(format!(
                            "Failed to read file contents: {}",
                            message
                        )));
                    }
                    results.push((factory.string(&contents), None));
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!(
                "Invalid step {} for file-read-to-string! operation",
                state.step
            ),
        }
    }
}

/// `(file-write! <FileHandle> <String:content>) -> ()`.
pub struct FileWriteOp;

impl<V: MettaValueTrait + Clone> GroundedOperationTCO<V> for FileWriteOp {
    fn name(&self) -> &str {
        "file-write!"
    }

    fn execute_step<F: MettaValueFactory<V>>(
        &self,
        state: &mut GroundedState<V>,
        factory: &F,
    ) -> GroundedWork<V> {
        match state.step {
            0 => {
                if state.args.len() != 2 {
                    return GroundedWork::Error(ExecError::Tagged(
                        "IncorrectNumberOfArguments",
                    ));
                }
                state.step = 1;
                GroundedWork::EvalArg {
                    arg_idx: 0,
                    state: state.clone(),
                }
            }
            1 => {
                if let Some(err) = find_error(state.get_arg(0).expect("arg 0")) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }
                state.step = 2;
                GroundedWork::EvalArg {
                    arg_idx: 1,
                    state: state.clone(),
                }
            }
            2 => {
                let fh_arg = state.get_arg(0).expect("arg 0 should be evaluated");
                let content_arg = state.get_arg(1).expect("arg 1 should be evaluated");
                if let Some(err) = find_error(content_arg) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut results: Vec<(V, Option<_>)> =
                    Vec::with_capacity(fh_arg.len() * content_arg.len());
                for fhv in fh_arg {
                    let Some(id) = extract_handle_id(fhv) else {
                        return GroundedWork::Error(ExecError::Runtime(
                            "file-write! expects filehandle and content (string atom) as an arguments"
                                .to_string(),
                        ));
                    };
                    for cv in content_arg {
                        let Some(content) = cv.as_string() else {
                            return GroundedWork::Error(ExecError::BadArgType {
                                pos: 2,
                                expected: "String",
                                got: cv.friendly_type_name().to_string(),
                            });
                        };

                        let mut registry = handles()
                            .lock()
                            .expect("file handle registry mutex poisoned");
                        let file = match registry.get_mut(&id) {
                            Some(f) => f,
                            None => {
                                return GroundedWork::Error(ExecError::Runtime(format!(
                                    "file-write!: unknown FileHandle id {}",
                                    id
                                )));
                            }
                        };

                        if let Err(message) = file.write_all(content.as_bytes()) {
                            return GroundedWork::Error(ExecError::Runtime(format!(
                                "Failed to write content to file: {}",
                                message
                            )));
                        }
                        results.push((factory.unit(), None));
                    }
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for file-write! operation", state.step),
        }
    }
}

/// `(file-seek! <FileHandle> <Number:offset>) -> ()`.
pub struct FileSeekOp;

impl<V: MettaValueTrait + Clone> GroundedOperationTCO<V> for FileSeekOp {
    fn name(&self) -> &str {
        "file-seek!"
    }

    fn execute_step<F: MettaValueFactory<V>>(
        &self,
        state: &mut GroundedState<V>,
        factory: &F,
    ) -> GroundedWork<V> {
        match state.step {
            0 => {
                if state.args.len() != 2 {
                    return GroundedWork::Error(ExecError::Tagged(
                        "IncorrectNumberOfArguments",
                    ));
                }
                state.step = 1;
                GroundedWork::EvalArg {
                    arg_idx: 0,
                    state: state.clone(),
                }
            }
            1 => {
                if let Some(err) = find_error(state.get_arg(0).expect("arg 0")) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }
                state.step = 2;
                GroundedWork::EvalArg {
                    arg_idx: 1,
                    state: state.clone(),
                }
            }
            2 => {
                let fh_arg = state.get_arg(0).expect("arg 0 should be evaluated");
                let off_arg = state.get_arg(1).expect("arg 1 should be evaluated");
                if let Some(err) = find_error(off_arg) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut results: Vec<(V, Option<_>)> =
                    Vec::with_capacity(fh_arg.len() * off_arg.len());
                for fhv in fh_arg {
                    let Some(id) = extract_handle_id(fhv) else {
                        return GroundedWork::Error(ExecError::Runtime(
                            "file-seek! expects filehandle and start byte (number) as an arguments"
                                .to_string(),
                        ));
                    };
                    for ov in off_arg {
                        let Some(offset) = ov.as_long() else {
                            return GroundedWork::Error(ExecError::BadArgType {
                                pos: 2,
                                expected: "Number",
                                got: ov.friendly_type_name().to_string(),
                            });
                        };

                        let mut registry = handles()
                            .lock()
                            .expect("file handle registry mutex poisoned");
                        let file = match registry.get_mut(&id) {
                            Some(f) => f,
                            None => {
                                return GroundedWork::Error(ExecError::Runtime(format!(
                                    "file-seek!: unknown FileHandle id {}",
                                    id
                                )));
                            }
                        };

                        // HE source seeks from Start; treat negative offsets
                        // as 0 (HE casts to u64 with no bounds check, but
                        // that's effectively the same on 64-bit platforms).
                        let pos = if offset < 0 { 0 } else { offset as u64 };
                        let _ = file.seek(SeekFrom::Start(pos));
                        results.push((factory.unit(), None));
                    }
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for file-seek! operation", state.step),
        }
    }
}

/// `(file-read-exact! <FileHandle> <Number:nbytes>) -> String`.
pub struct FileReadExactOp;

impl<V: MettaValueTrait + Clone> GroundedOperationTCO<V> for FileReadExactOp {
    fn name(&self) -> &str {
        "file-read-exact!"
    }

    fn execute_step<F: MettaValueFactory<V>>(
        &self,
        state: &mut GroundedState<V>,
        factory: &F,
    ) -> GroundedWork<V> {
        match state.step {
            0 => {
                if state.args.len() != 2 {
                    return GroundedWork::Error(ExecError::Tagged(
                        "IncorrectNumberOfArguments",
                    ));
                }
                state.step = 1;
                GroundedWork::EvalArg {
                    arg_idx: 0,
                    state: state.clone(),
                }
            }
            1 => {
                if let Some(err) = find_error(state.get_arg(0).expect("arg 0")) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }
                state.step = 2;
                GroundedWork::EvalArg {
                    arg_idx: 1,
                    state: state.clone(),
                }
            }
            2 => {
                let fh_arg = state.get_arg(0).expect("arg 0 should be evaluated");
                let n_arg = state.get_arg(1).expect("arg 1 should be evaluated");
                if let Some(err) = find_error(n_arg) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut results: Vec<(V, Option<_>)> =
                    Vec::with_capacity(fh_arg.len() * n_arg.len());
                for fhv in fh_arg {
                    let Some(id) = extract_handle_id(fhv) else {
                        return GroundedWork::Error(ExecError::Runtime(
                            "file-read-exact! expects filehandle and number of bytes to read (number) as an arguments"
                                .to_string(),
                        ));
                    };
                    for nv in n_arg {
                        let Some(nbytes) = nv.as_long() else {
                            return GroundedWork::Error(ExecError::BadArgType {
                                pos: 2,
                                expected: "Number",
                                got: nv.friendly_type_name().to_string(),
                            });
                        };

                        if nbytes < 0 {
                            return GroundedWork::Error(ExecError::Runtime(format!(
                                "file-read-exact!: byte count must be non-negative, got {}",
                                nbytes
                            )));
                        }

                        let mut buf = vec![0u8; nbytes as usize];
                        let mut registry = handles()
                            .lock()
                            .expect("file handle registry mutex poisoned");
                        let file = match registry.get_mut(&id) {
                            Some(f) => f,
                            None => {
                                return GroundedWork::Error(ExecError::Runtime(format!(
                                    "file-read-exact!: unknown FileHandle id {}",
                                    id
                                )));
                            }
                        };

                        let n = match file.read(&mut buf) {
                            Ok(n) => n,
                            Err(message) => {
                                return GroundedWork::Error(ExecError::Runtime(format!(
                                    "Read exact failed: {}",
                                    message
                                )));
                            }
                        };
                        buf.truncate(n);
                        // HE used `String::from_utf8(buf)` and propagated
                        // utf8 errors; replicate that behavior verbatim.
                        let s = match String::from_utf8(buf) {
                            Ok(s) => s,
                            Err(message) => {
                                return GroundedWork::Error(ExecError::Runtime(format!(
                                    "Read exact failed: {}",
                                    message
                                )));
                            }
                        };
                        results.push((factory.string(&s), None));
                    }
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!(
                "Invalid step {} for file-read-exact! operation",
                state.step
            ),
        }
    }
}

/// `(file-get-size! <FileHandle>) -> Number`.
pub struct FileGetSizeOp;

impl<V: MettaValueTrait + Clone> GroundedOperationTCO<V> for FileGetSizeOp {
    fn name(&self) -> &str {
        "file-get-size!"
    }

    fn execute_step<F: MettaValueFactory<V>>(
        &self,
        state: &mut GroundedState<V>,
        factory: &F,
    ) -> GroundedWork<V> {
        match state.step {
            0 => {
                if state.args.len() != 1 {
                    return GroundedWork::Error(ExecError::Tagged(
                        "IncorrectNumberOfArguments",
                    ));
                }
                state.step = 1;
                GroundedWork::EvalArg {
                    arg_idx: 0,
                    state: state.clone(),
                }
            }
            1 => {
                let arg = state.get_arg(0).expect("arg 0 should be evaluated");
                if let Some(err) = find_error(arg) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut results = Vec::with_capacity(arg.len());
                for value in arg {
                    let Some(id) = extract_handle_id(value) else {
                        return GroundedWork::Error(ExecError::Runtime(
                            "file-get-size! expects filehandle as an argument".to_string(),
                        ));
                    };

                    let registry = handles()
                        .lock()
                        .expect("file handle registry mutex poisoned");
                    let file = match registry.get(&id) {
                        Some(f) => f,
                        None => {
                            return GroundedWork::Error(ExecError::Runtime(format!(
                                "file-get-size!: unknown FileHandle id {}",
                                id
                            )));
                        }
                    };

                    let size = match file.metadata() {
                        Ok(meta) => meta.len(),
                        Err(message) => {
                            return GroundedWork::Error(ExecError::Runtime(format!(
                                "Get size failed: {}",
                                message
                            )));
                        }
                    };
                    results.push((factory.long(size as i64), None));
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for file-get-size! operation", state.step),
        }
    }
}
