//! Configuration and statistics for hybrid JIT/VM execution.

use super::super::TieredStats;
use super::constants::*;
use crate::backend::bytecode::VmConfig;

/// Hybrid executor configuration
#[derive(Debug, Clone)]
pub struct HybridConfig {
    /// Bytecode VM configuration
    pub vm_config: VmConfig,
    /// JIT value stack capacity
    pub jit_stack_capacity: usize,
    /// JIT choice point capacity
    pub jit_choice_point_capacity: usize,
    /// JIT results buffer capacity
    pub jit_results_capacity: usize,
    /// JIT binding frames capacity
    pub jit_binding_frames_capacity: usize,
    /// JIT cut markers capacity
    pub jit_cut_markers_capacity: usize,
    /// Whether to enable JIT compilation
    pub jit_enabled: bool,
    /// Whether to enable execution tracing
    pub trace: bool,
}

impl Default for HybridConfig {
    fn default() -> Self {
        Self {
            vm_config: VmConfig::default(),
            jit_stack_capacity: JIT_STACK_CAPACITY,
            jit_choice_point_capacity: JIT_CHOICE_POINT_CAPACITY,
            jit_results_capacity: JIT_RESULTS_CAPACITY,
            jit_binding_frames_capacity: JIT_BINDING_FRAMES_CAPACITY,
            jit_cut_markers_capacity: JIT_CUT_MARKERS_CAPACITY,
            jit_enabled: super::super::JIT_ENABLED,
            trace: false,
        }
    }
}

impl HybridConfig {
    /// Create a configuration with tracing enabled
    pub fn with_trace(mut self) -> Self {
        self.trace = true;
        self.vm_config.trace = true;
        self
    }

    /// Create a configuration with JIT disabled (bytecode-only)
    pub fn bytecode_only() -> Self {
        Self {
            jit_enabled: false,
            ..Default::default()
        }
    }
}

/// Statistics for hybrid execution
#[derive(Debug, Clone, Default)]
pub struct HybridStats {
    /// Total number of run() calls
    pub total_runs: u64,
    /// Number of runs that used JIT
    pub jit_runs: u64,
    /// Number of runs that used bytecode VM
    pub vm_runs: u64,
    /// Number of JIT bailouts
    pub jit_bailouts: u64,
    /// Number of successful JIT compilations
    pub jit_compilations: u64,
    /// Number of failed JIT compilations
    pub jit_compile_failures: u64,
    /// Tiered compilation statistics
    pub tiered_stats: TieredStats,
}

impl HybridStats {
    /// Get the JIT hit rate as a percentage
    pub fn jit_hit_rate(&self) -> f64 {
        if self.total_runs == 0 {
            0.0
        } else {
            (self.jit_runs as f64 / self.total_runs as f64) * 100.0
        }
    }

    /// Get the bailout rate as a percentage of JIT runs
    pub fn bailout_rate(&self) -> f64 {
        if self.jit_runs == 0 {
            0.0
        } else {
            (self.jit_bailouts as f64 / self.jit_runs as f64) * 100.0
        }
    }
}
