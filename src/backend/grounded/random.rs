//! Random-number grounded operations.
//!
//! HE-bisim source: `hyperon-experimental/lib/src/metta/runner/builtin_mods/random.rs`.
//!
//! Operations registered:
//!   - `(new-random-generator <Number:seed>) -> RandomGenerator`
//!   - `(random-int <RandomGenerator> <Number:start> <Number:end>) -> Number`
//!   - `(random-float <RandomGenerator> <Number:start> <Number:end>) -> Number`
//!   - `(set-random-seed <RandomGenerator> <Number:seed>) -> ()`
//!   - `(reset-random-generator <RandomGenerator>) -> ()`
//!   - `(flip) -> Bool`
//!
//! HE uses `rand::rngs::StdRng` (ChaCha-based) wrapped in `Rc<RefCell<_>>`.
//! MeTTaTron's value model is immutable persistent slab pointers, so we
//! represent a RandomGenerator as `(RandomGenerator <id>)` where `<id>` is
//! a `Long` index into a global registry of `StdRng` instances.
//!
//! For HE-determinism: `new-random-generator` uses `StdRng::seed_from_u64`,
//! matching HE's `RandomGenerator::from_seed_u64`. Two generators created
//! from the same seed produce the same sequence of values.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use super::state::{find_error, GroundedState, GroundedWork};
use super::traits::GroundedOperationTCO;
use crate::backend::grounded::ExecError;
use crate::backend::models::{MettaValueFactory, MettaValueTrait};

/// Global RandomGenerator registry. Mutex-protected for thread safety.
static RNG_REGISTRY: OnceLock<Mutex<HashMap<u64, StdRng>>> = OnceLock::new();
static NEXT_RNG_ID: AtomicU64 = AtomicU64::new(1);

fn registry() -> &'static Mutex<HashMap<u64, StdRng>> {
    RNG_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Build a `(RandomGenerator <id>)` SExpr to represent a generator value.
fn rng_value<V, F>(id: u64, factory: &F) -> V
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    factory.sexpr(vec![
        factory.atom("RandomGenerator"),
        factory.long(id as i64),
    ])
}

/// Extract the generator id from a `(RandomGenerator <id>)` value.
fn extract_rng_id<V: MettaValueTrait>(value: &V) -> Option<u64> {
    let items = value.as_sexpr()?;
    if items.len() != 2 {
        return None;
    }
    if items[0].as_atom() != Some("RandomGenerator") {
        return None;
    }
    let id = items[1].as_long()?;
    if id < 0 {
        return None;
    }
    Some(id as u64)
}

/// Create a fresh seeded RandomGenerator value (for `&rng` token init,
/// Plan Phase J.2 (2026-05-20)). Mirrors `NewRandomGeneratorOp`'s body
/// but exposes a synchronous API that env init can call. No recursion
/// (single `StdRng::seed_from_u64` + registry insert) — stack-safe per
/// [[feedback-stack-safety-mandate]].
pub fn create_seeded_generator<V, F>(seed: u64, factory: &F) -> V
where
    V: MettaValueTrait + Clone,
    F: MettaValueFactory<V>,
{
    let rng = StdRng::seed_from_u64(seed);
    let id = NEXT_RNG_ID.fetch_add(1, Ordering::Relaxed);
    registry()
        .lock()
        .expect("rng registry mutex poisoned")
        .insert(id, rng);
    rng_value(id, factory)
}

/// `(new-random-generator <Number:seed>) -> RandomGenerator`.
pub struct NewRandomGeneratorOp;

impl<V: MettaValueTrait + Clone> GroundedOperationTCO<V> for NewRandomGeneratorOp {
    fn name(&self) -> &str {
        "new-random-generator"
    }

    fn execute_step<F: MettaValueFactory<V>>(
        &self,
        state: &mut GroundedState<V>,
        factory: &F,
    ) -> GroundedWork<V> {
        match state.step {
            0 => {
                if state.args.len() != 1 {
                    return GroundedWork::Error(ExecError::Tagged("IncorrectNumberOfArguments"));
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
                    let Some(seed) = value.as_long() else {
                        return GroundedWork::Error(ExecError::BadArgType {
                            pos: 1,
                            expected: "Number",
                            got: value.friendly_type_name().to_string(),
                        });
                    };
                    let rng = StdRng::seed_from_u64(seed as u64);
                    let id = NEXT_RNG_ID.fetch_add(1, Ordering::Relaxed);
                    registry()
                        .lock()
                        .expect("rng registry mutex poisoned")
                        .insert(id, rng);
                    results.push((rng_value(id, factory), None));
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!(
                "Invalid step {} for new-random-generator operation",
                state.step
            ),
        }
    }
}

/// `(random-int <RandomGenerator> <Number:start> <Number:end>) -> Number`.
pub struct RandomIntOp;

impl<V: MettaValueTrait + Clone> GroundedOperationTCO<V> for RandomIntOp {
    fn name(&self) -> &str {
        "random-int"
    }

    fn execute_step<F: MettaValueFactory<V>>(
        &self,
        state: &mut GroundedState<V>,
        factory: &F,
    ) -> GroundedWork<V> {
        match state.step {
            0 => {
                if state.args.len() != 3 {
                    return GroundedWork::Error(ExecError::Tagged("IncorrectNumberOfArguments"));
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
                if let Some(err) = find_error(state.get_arg(1).expect("arg 1")) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }
                state.step = 3;
                GroundedWork::EvalArg {
                    arg_idx: 2,
                    state: state.clone(),
                }
            }
            3 => {
                let rng_arg = state.get_arg(0).expect("arg 0 should be evaluated");
                let start_arg = state.get_arg(1).expect("arg 1 should be evaluated");
                let end_arg = state.get_arg(2).expect("arg 2 should be evaluated");
                if let Some(err) = find_error(end_arg) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut results: Vec<(V, Option<_>)> =
                    Vec::with_capacity(rng_arg.len() * start_arg.len() * end_arg.len());
                for rngv in rng_arg {
                    let Some(id) = extract_rng_id(rngv) else {
                        return GroundedWork::Error(ExecError::Runtime(
                            "random-int expects a random generator as its argument".to_string(),
                        ));
                    };
                    for sv in start_arg {
                        let Some(start) = sv.as_long() else {
                            return GroundedWork::Error(ExecError::BadArgType {
                                pos: 2,
                                expected: "Number",
                                got: sv.friendly_type_name().to_string(),
                            });
                        };
                        for ev in end_arg {
                            let Some(end) = ev.as_long() else {
                                return GroundedWork::Error(ExecError::BadArgType {
                                    pos: 3,
                                    expected: "Number",
                                    got: ev.friendly_type_name().to_string(),
                                });
                            };
                            if start >= end {
                                return GroundedWork::Error(ExecError::Tagged("RangeIsEmpty"));
                            }

                            let mut reg = registry().lock().expect("rng registry mutex poisoned");
                            let rng = match reg.get_mut(&id) {
                                Some(r) => r,
                                None => {
                                    return GroundedWork::Error(ExecError::Runtime(format!(
                                        "random-int: unknown RandomGenerator id {}",
                                        id
                                    )));
                                }
                            };
                            let val: i64 = rng.gen_range(start..end);
                            results.push((factory.long(val), None));
                        }
                    }
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for random-int operation", state.step),
        }
    }
}

/// `(random-float <RandomGenerator> <Number:start> <Number:end>) -> Number`.
pub struct RandomFloatOp;

impl<V: MettaValueTrait + Clone> GroundedOperationTCO<V> for RandomFloatOp {
    fn name(&self) -> &str {
        "random-float"
    }

    fn execute_step<F: MettaValueFactory<V>>(
        &self,
        state: &mut GroundedState<V>,
        factory: &F,
    ) -> GroundedWork<V> {
        match state.step {
            0 => {
                if state.args.len() != 3 {
                    return GroundedWork::Error(ExecError::Tagged("IncorrectNumberOfArguments"));
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
                if let Some(err) = find_error(state.get_arg(1).expect("arg 1")) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }
                state.step = 3;
                GroundedWork::EvalArg {
                    arg_idx: 2,
                    state: state.clone(),
                }
            }
            3 => {
                let rng_arg = state.get_arg(0).expect("arg 0 should be evaluated");
                let start_arg = state.get_arg(1).expect("arg 1 should be evaluated");
                let end_arg = state.get_arg(2).expect("arg 2 should be evaluated");
                if let Some(err) = find_error(end_arg) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut results: Vec<(V, Option<_>)> =
                    Vec::with_capacity(rng_arg.len() * start_arg.len() * end_arg.len());
                for rngv in rng_arg {
                    let Some(id) = extract_rng_id(rngv) else {
                        return GroundedWork::Error(ExecError::Runtime(
                            "random-float expects a random generator as its argument".to_string(),
                        ));
                    };
                    for sv in start_arg {
                        let start = if let Some(f) = sv.as_float() {
                            f
                        } else if let Some(i) = sv.as_long() {
                            i as f64
                        } else {
                            return GroundedWork::Error(ExecError::BadArgType {
                                pos: 2,
                                expected: "Number",
                                got: sv.friendly_type_name().to_string(),
                            });
                        };
                        for ev in end_arg {
                            let end = if let Some(f) = ev.as_float() {
                                f
                            } else if let Some(i) = ev.as_long() {
                                i as f64
                            } else {
                                return GroundedWork::Error(ExecError::BadArgType {
                                    pos: 3,
                                    expected: "Number",
                                    got: ev.friendly_type_name().to_string(),
                                });
                            };
                            if start >= end {
                                return GroundedWork::Error(ExecError::Tagged("RangeIsEmpty"));
                            }

                            let mut reg = registry().lock().expect("rng registry mutex poisoned");
                            let rng = match reg.get_mut(&id) {
                                Some(r) => r,
                                None => {
                                    return GroundedWork::Error(ExecError::Runtime(format!(
                                        "random-float: unknown RandomGenerator id {}",
                                        id
                                    )));
                                }
                            };
                            let val: f64 = rng.gen_range(start..end);
                            results.push((factory.float(val), None));
                        }
                    }
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for random-float operation", state.step),
        }
    }
}

/// `(set-random-seed <RandomGenerator> <Number:seed>) -> ()`.
pub struct SetRandomSeedOp;

impl<V: MettaValueTrait + Clone> GroundedOperationTCO<V> for SetRandomSeedOp {
    fn name(&self) -> &str {
        "set-random-seed"
    }

    fn execute_step<F: MettaValueFactory<V>>(
        &self,
        state: &mut GroundedState<V>,
        factory: &F,
    ) -> GroundedWork<V> {
        match state.step {
            0 => {
                if state.args.len() != 2 {
                    return GroundedWork::Error(ExecError::Tagged("IncorrectNumberOfArguments"));
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
                let rng_arg = state.get_arg(0).expect("arg 0 should be evaluated");
                let seed_arg = state.get_arg(1).expect("arg 1 should be evaluated");
                if let Some(err) = find_error(seed_arg) {
                    return GroundedWork::Done(vec![(err.clone(), None)]);
                }

                let mut results: Vec<(V, Option<_>)> =
                    Vec::with_capacity(rng_arg.len() * seed_arg.len());
                for rngv in rng_arg {
                    let Some(id) = extract_rng_id(rngv) else {
                        return GroundedWork::Error(ExecError::Runtime(
                            "set-random-seed expects a random generator as its argument"
                                .to_string(),
                        ));
                    };
                    for sv in seed_arg {
                        let Some(seed) = sv.as_long() else {
                            return GroundedWork::Error(ExecError::BadArgType {
                                pos: 2,
                                expected: "Number",
                                got: sv.friendly_type_name().to_string(),
                            });
                        };

                        let mut reg = registry().lock().expect("rng registry mutex poisoned");
                        let entry = reg.get_mut(&id);
                        match entry {
                            Some(r) => {
                                *r = StdRng::seed_from_u64(seed as u64);
                            }
                            None => {
                                return GroundedWork::Error(ExecError::Runtime(format!(
                                    "set-random-seed: unknown RandomGenerator id {}",
                                    id
                                )));
                            }
                        }
                        results.push((factory.unit(), None));
                    }
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!("Invalid step {} for set-random-seed operation", state.step),
        }
    }
}

/// `(reset-random-generator <RandomGenerator>) -> ()`.
///
/// HE seeds from `StdRng::from_os_rng()`. We do the same — non-deterministic
/// reset is part of the documented behavior even in HE.
pub struct ResetRandomGeneratorOp;

impl<V: MettaValueTrait + Clone> GroundedOperationTCO<V> for ResetRandomGeneratorOp {
    fn name(&self) -> &str {
        "reset-random-generator"
    }

    fn execute_step<F: MettaValueFactory<V>>(
        &self,
        state: &mut GroundedState<V>,
        factory: &F,
    ) -> GroundedWork<V> {
        match state.step {
            0 => {
                if state.args.len() != 1 {
                    return GroundedWork::Error(ExecError::Tagged("IncorrectNumberOfArguments"));
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
                    let Some(id) = extract_rng_id(value) else {
                        return GroundedWork::Error(ExecError::Runtime(
                            "reset-random-generator expects a random generator as its argument"
                                .to_string(),
                        ));
                    };

                    let mut reg = registry().lock().expect("rng registry mutex poisoned");
                    let entry = reg.get_mut(&id);
                    match entry {
                        Some(r) => {
                            // rand 0.8 doesn't have `from_os_rng`; use
                            // `from_entropy` which is the equivalent OS-seeded
                            // constructor in this version.
                            *r = StdRng::from_entropy();
                        }
                        None => {
                            return GroundedWork::Error(ExecError::Runtime(format!(
                                "reset-random-generator: unknown RandomGenerator id {}",
                                id
                            )));
                        }
                    }
                    results.push((factory.unit(), None));
                }
                GroundedWork::Done(results)
            }
            _ => unreachable!(
                "Invalid step {} for reset-random-generator operation",
                state.step
            ),
        }
    }
}

/// `(flip) -> Bool`.
///
/// HE empirical: uses `rand::random()` (process-default RNG, non-deterministic).
/// MeTTaTron uses `rand::thread_rng()` likewise; the spec does not require
/// determinism for `flip` (it's documented as "uniformly distributed random
/// boolean value").
pub struct FlipOp;

impl<V: MettaValueTrait + Clone> GroundedOperationTCO<V> for FlipOp {
    fn name(&self) -> &str {
        "flip"
    }

    fn execute_step<F: MettaValueFactory<V>>(
        &self,
        state: &mut GroundedState<V>,
        factory: &F,
    ) -> GroundedWork<V> {
        match state.step {
            0 => {
                if !state.args.is_empty() {
                    return GroundedWork::Error(ExecError::Tagged("IncorrectNumberOfArguments"));
                }
                let val = rand::thread_rng().gen::<bool>();
                GroundedWork::Done(vec![(factory.bool(val), None)])
            }
            _ => unreachable!("Invalid step {} for flip operation", state.step),
        }
    }
}
