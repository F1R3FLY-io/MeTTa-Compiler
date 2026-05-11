//! Process-level interrupt flag used for graceful evaluator shutdown.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

static INTERRUPTED: OnceLock<Arc<AtomicBool>> = OnceLock::new();
static INSTALLED: OnceLock<()> = OnceLock::new();

fn interrupt_flag() -> Arc<AtomicBool> {
    INTERRUPTED
        .get_or_init(|| Arc::new(AtomicBool::new(false)))
        .clone()
}

/// Install the SIGINT handler once for the process.
///
/// The handler only flips an atomic flag. Evaluation code observes the flag at
/// safe loop points and returns normally, so callers can flush traces and drop
/// runtime state without running inside a signal handler.
pub fn install_signal_handler() {
    INSTALLED.get_or_init(|| {
        let flag = interrupt_flag();
        if let Err(err) = signal_hook::flag::register(signal_hook::consts::SIGINT, flag) {
            eprintln!("[interrupt] Failed to install SIGINT handler: {err}");
        }
    });
}

#[inline]
pub fn reset() {
    interrupt_flag().store(false, Ordering::Release);
}

#[inline]
pub fn is_interrupted() -> bool {
    interrupt_flag().load(Ordering::Acquire)
}
