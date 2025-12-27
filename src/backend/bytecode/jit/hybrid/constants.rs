//! Constants for hybrid JIT/VM execution.

/// Default stack capacity for JIT execution
pub const JIT_STACK_CAPACITY: usize = 1024;

/// Default capacity for choice points in JIT nondeterminism
pub const JIT_CHOICE_POINT_CAPACITY: usize = 64;

/// Default capacity for results buffer in JIT nondeterminism
pub const JIT_RESULTS_CAPACITY: usize = 256;

/// Default capacity for binding frames in JIT
pub const JIT_BINDING_FRAMES_CAPACITY: usize = 32;

/// Default capacity for cut markers in JIT
pub const JIT_CUT_MARKERS_CAPACITY: usize = 16;
