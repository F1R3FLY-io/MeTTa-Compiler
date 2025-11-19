// PathMap jemalloc Diagnostics Module
//
// Purpose: Diagnostic tools for monitoring jemalloc behavior with PathMap allocations
// Reference: PATHMAP_JEMALLOC_ANALYSIS.md Section 7
//
// Usage:
//   1. Copy this file to src/backend/diagnostics.rs
//   2. Add to lib.rs: pub mod diagnostics;
//   3. Enable jemalloc feature in Cargo.toml
//   4. Use in tests/benchmarks for monitoring
//
// Example:
//   use crate::diagnostics::{AllocMonitor, test_arena_creation_limit};
//
//   let mut monitor = AllocMonitor::new();
//   // ... your code ...
//   monitor.report();

#![allow(dead_code)]

use std::fmt;

#[cfg(feature = "jemalloc")]
use tikv_jemalloc_ctl::{arenas, epoch, stats, thread};

// ============================================================================
// Allocation Monitoring
// ============================================================================

/// Real-time allocation tracker
///
/// Monitors jemalloc statistics and reports allocation deltas.
pub struct AllocMonitor {
    last_allocated: usize,
    last_resident: usize,
    last_active: usize,
    snapshots: Vec<AllocSnapshot>,
}

impl AllocMonitor {
    /// Create a new allocation monitor
    pub fn new() -> Self {
        Self {
            last_allocated: 0,
            last_resident: 0,
            last_active: 0,
            snapshots: Vec::new(),
        }
    }

    /// Take a snapshot of current allocation statistics
    #[cfg(feature = "jemalloc")]
    pub fn snapshot(&mut self) -> AllocSnapshot {
        // Advance jemalloc epoch to refresh statistics
        let _ = epoch::mib().map(|e| e.advance());

        let allocated = stats::allocated::read().unwrap_or(0);
        let resident = stats::resident::read().unwrap_or(0);
        let active = stats::active::read().unwrap_or(0);
        let metadata = stats::metadata::read().unwrap_or(0);

        let delta_allocated = allocated.saturating_sub(self.last_allocated);
        let delta_resident = resident.saturating_sub(self.last_resident);
        let delta_active = active.saturating_sub(self.last_active);

        self.last_allocated = allocated;
        self.last_resident = resident;
        self.last_active = active;

        let snapshot = AllocSnapshot {
            allocated,
            resident,
            active,
            metadata,
            delta_allocated,
            delta_resident,
            delta_active,
        };

        self.snapshots.push(snapshot.clone());
        snapshot
    }

    #[cfg(not(feature = "jemalloc"))]
    pub fn snapshot(&mut self) -> AllocSnapshot {
        AllocSnapshot::default()
    }

    /// Print a report of all snapshots
    pub fn report(&self) {
        println!("\n=== Allocation Monitor Report ===\n");

        if self.snapshots.is_empty() {
            println!("No snapshots taken");
            return;
        }

        println!("{:<6} {:>12} {:>12} {:>12} {:>12}",
                 "Snap", "Allocated", "Δ Alloc", "Resident", "Metadata");
        println!("{:-<60}", "");

        for (i, snap) in self.snapshots.iter().enumerate() {
            println!("{:<6} {:>12} {:>12} {:>12} {:>12}",
                     i,
                     format_bytes(snap.allocated),
                     format_bytes(snap.delta_allocated),
                     format_bytes(snap.resident),
                     format_bytes(snap.metadata));
        }

        println!("\nTotal allocation delta: {}",
                 format_bytes(self.snapshots.last().unwrap().allocated -
                              self.snapshots.first().unwrap().allocated));
    }

    /// Get all snapshots
    pub fn snapshots(&self) -> &[AllocSnapshot] {
        &self.snapshots
    }
}

impl Default for AllocMonitor {
    fn default() -> Self {
        Self::new()
    }
}

/// Snapshot of allocation statistics at a point in time
#[derive(Debug, Clone, Default)]
pub struct AllocSnapshot {
    pub allocated: usize,       // Bytes allocated by application
    pub resident: usize,        // Bytes in physical memory (RSS)
    pub active: usize,          // Bytes in active pages
    pub metadata: usize,        // Bytes used for jemalloc metadata
    pub delta_allocated: usize, // Bytes allocated since last snapshot
    pub delta_resident: usize,  // Resident growth since last snapshot
    pub delta_active: usize,    // Active page growth since last snapshot
}

impl fmt::Display for AllocSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AllocSnapshot {{\n")?;
        write!(f, "  allocated: {} ({} delta)\n",
               format_bytes(self.allocated), format_bytes(self.delta_allocated))?;
        write!(f, "  resident:  {} ({} delta)\n",
               format_bytes(self.resident), format_bytes(self.delta_resident))?;
        write!(f, "  active:    {} ({} delta)\n",
               format_bytes(self.active), format_bytes(self.delta_active))?;
        write!(f, "  metadata:  {}\n", format_bytes(self.metadata))?;
        write!(f, "  efficiency: {:.1}%\n",
               100.0 * self.allocated as f64 / self.resident.max(1) as f64)?;
        write!(f, "}}")
    }
}

// ============================================================================
// Arena Management Diagnostics
// ============================================================================

/// Test jemalloc arena creation limits
///
/// Creates arenas until failure or safety limit reached.
/// Reports memory overhead per arena.
///
/// **Warning**: Created arenas cannot be destroyed and persist until process exit.
#[cfg(feature = "jemalloc")]
pub fn test_arena_creation_limit(max_arenas: usize) {
    println!("Testing arena creation limits (max: {})...\n", max_arenas);

    let metadata_before = stats::metadata::read().unwrap_or(0);
    let mut created_arenas = Vec::new();
    let mut last_report = 0;

    loop {
        match arenas::create() {
            Ok(arena_idx) => {
                created_arenas.push(arena_idx);

                // Report every 100 arenas
                if created_arenas.len() - last_report >= 100 {
                    let metadata_now = stats::metadata::read().unwrap_or(0);
                    let overhead = metadata_now - metadata_before;
                    println!("Created {} arenas (latest: {}, overhead: {})",
                             created_arenas.len(),
                             arena_idx,
                             format_bytes(overhead));
                    last_report = created_arenas.len();
                }

                // Safety limit
                if created_arenas.len() >= max_arenas {
                    println!("\n✅ Successfully created {} arenas (reached limit)",
                             created_arenas.len());
                    break;
                }
            }
            Err(e) => {
                println!("\n❌ Arena creation failed after {} arenas", created_arenas.len());
                println!("Error: {:?}", e);
                break;
            }
        }
    }

    // Calculate memory overhead
    let metadata_after = stats::metadata::read().unwrap_or(0);
    let total_overhead = metadata_after - metadata_before;
    let overhead_per_arena = if created_arenas.len() > 0 {
        total_overhead / created_arenas.len()
    } else {
        0
    };

    println!("\nMemory overhead analysis:");
    println!("  Total metadata: {}", format_bytes(total_overhead));
    println!("  Per arena: {}", format_bytes(overhead_per_arena));
    println!("  Estimated max arenas (1 GB overhead): ~{}",
             if overhead_per_arena > 0 {
                 1_073_741_824 / overhead_per_arena
             } else {
                 0
             });

    // Warning about arena persistence
    println!("\n⚠️  Warning: Created {} arenas that cannot be destroyed", created_arenas.len());
    println!("   These will persist until process exit.");
}

#[cfg(not(feature = "jemalloc"))]
pub fn test_arena_creation_limit(_max_arenas: usize) {
    println!("❌ jemalloc feature not enabled");
}

/// Print current jemalloc configuration
#[cfg(feature = "jemalloc")]
pub fn print_jemalloc_config() {
    use tikv_jemalloc_ctl::opt;

    println!("\n=== jemalloc Configuration ===\n");

    if let Ok(narenas) = opt::narenas::read() {
        println!("narenas: {}", narenas);
    }

    if let Ok(tcache) = opt::tcache::read() {
        println!("tcache: {}", tcache);
    }

    if let Ok(lg_tcache_max) = opt::lg_tcache_max::read() {
        println!("lg_tcache_max: {} (max size: {})",
                 lg_tcache_max, format_bytes(1 << lg_tcache_max));
    }

    if let Ok(dirty_decay_ms) = opt::dirty_decay_ms::read() {
        println!("dirty_decay_ms: {} ms", dirty_decay_ms);
    }

    if let Ok(muzzy_decay_ms) = opt::muzzy_decay_ms::read() {
        println!("muzzy_decay_ms: {} ms", muzzy_decay_ms);
    }

    println!("\n=== Current Statistics ===\n");

    if let Ok(allocated) = stats::allocated::read() {
        println!("allocated: {}", format_bytes(allocated));
    }

    if let Ok(resident) = stats::resident::read() {
        println!("resident: {}", format_bytes(resident));
    }

    if let Ok(metadata) = stats::metadata::read() {
        println!("metadata: {}", format_bytes(metadata));
    }

    if let Ok(narenas_actual) = arenas::narenas::read() {
        println!("actual arenas: {}", narenas_actual);
    }
}

#[cfg(not(feature = "jemalloc"))]
pub fn print_jemalloc_config() {
    println!("❌ jemalloc feature not enabled");
}

/// Assign current thread to a specific arena
#[cfg(feature = "jemalloc")]
pub fn assign_thread_to_arena(arena_idx: usize) -> Result<(), String> {
    thread::write(arena_idx).map_err(|e| format!("Failed to assign thread to arena: {:?}", e))
}

#[cfg(not(feature = "jemalloc"))]
pub fn assign_thread_to_arena(_arena_idx: usize) -> Result<(), String> {
    Err("jemalloc feature not enabled".to_string())
}

/// Create a new arena and return its index
#[cfg(feature = "jemalloc")]
pub fn create_arena() -> Result<usize, String> {
    arenas::create().map_err(|e| format!("Failed to create arena: {:?}", e))
}

#[cfg(not(feature = "jemalloc"))]
pub fn create_arena() -> Result<usize, String> {
    Err("jemalloc feature not enabled".to_string())
}

// ============================================================================
// Crash Analysis Tools
// ============================================================================

/// Dump jemalloc state to stderr
///
/// Useful for crash handlers to capture state before exit.
#[cfg(feature = "jemalloc")]
pub fn dump_state_on_crash() {
    eprintln!("\n=== jemalloc State Dump (Crash) ===\n");

    if let Ok(narenas) = arenas::narenas::read() {
        eprintln!("Active arenas: {}", narenas);
    }

    if let Ok(allocated) = stats::allocated::read() {
        eprintln!("Allocated: {}", format_bytes(allocated));
    }

    if let Ok(resident) = stats::resident::read() {
        eprintln!("Resident: {}", format_bytes(resident));
    }

    if let Ok(metadata) = stats::metadata::read() {
        eprintln!("Metadata: {}", format_bytes(metadata));
    }

    // Attempt heap dump if profiling enabled
    #[cfg(feature = "jemalloc")]
    {
        use tikv_jemalloc_ctl::prof;
        let _ = prof::dump::mib().and_then(|mib| {
            mib.write("crash.heap").map(|_| {
                eprintln!("Heap dump written to: crash.heap");
            })
        });
    }
}

#[cfg(not(feature = "jemalloc"))]
pub fn dump_state_on_crash() {
    eprintln!("❌ jemalloc feature not enabled");
}

// ============================================================================
// Benchmark Helpers
// ============================================================================

/// Measure memory allocation for a closure
pub fn measure_allocation<F, R>(label: &str, f: F) -> (R, AllocSnapshot)
where
    F: FnOnce() -> R,
{
    let mut monitor = AllocMonitor::new();

    #[cfg(feature = "jemalloc")]
    {
        println!("\n=== Measuring: {} ===", label);
        monitor.snapshot(); // Initial snapshot
    }

    let result = f();

    #[cfg(feature = "jemalloc")]
    {
        let snap = monitor.snapshot();
        println!("\nResults for '{}':", label);
        println!("  Allocated: {}", format_bytes(snap.delta_allocated));
        println!("  Resident:  {}", format_bytes(snap.delta_resident));
        println!("  Metadata:  {}", format_bytes(snap.metadata));
        (result, snap)
    }

    #[cfg(not(feature = "jemalloc"))]
    {
        (result, AllocSnapshot::default())
    }
}

/// Profile a code section with heap dumps
#[cfg(feature = "jemalloc")]
pub fn profile_section<F, R>(label: &str, f: F) -> R
where
    F: FnOnce() -> R,
{
    use tikv_jemalloc_ctl::prof;

    // Dump heap before
    let before_file = format!("before_{}.heap", label);
    let _ = prof::dump::mib().and_then(|mib| mib.write(&before_file));
    println!("Heap dump: {}", before_file);

    // Run code
    let result = f();

    // Dump heap after
    let after_file = format!("after_{}.heap", label);
    let _ = prof::dump::mib().and_then(|mib| mib.write(&after_file));
    println!("Heap dump: {}", after_file);

    println!("\nAnalyze with:");
    println!("  jeprof --text ./target/release/<binary> {}", before_file);
    println!("  jeprof --text ./target/release/<binary> {}", after_file);

    result
}

#[cfg(not(feature = "jemalloc"))]
pub fn profile_section<F, R>(_label: &str, f: F) -> R
where
    F: FnOnce() -> R,
{
    f()
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Format bytes as human-readable string
fn format_bytes(bytes: usize) -> String {
    const KB: usize = 1024;
    const MB: usize = KB * 1024;
    const GB: usize = MB * 1024;

    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_alloc_monitor() {
        let mut monitor = AllocMonitor::new();
        let snap1 = monitor.snapshot();

        // Allocate some memory
        let _vec: Vec<u8> = vec![0u8; 1024 * 1024]; // 1 MB

        let snap2 = monitor.snapshot();

        #[cfg(feature = "jemalloc")]
        {
            assert!(snap2.delta_allocated > 0, "Should show allocation");
            assert!(snap2.allocated > snap1.allocated, "Total should increase");
        }

        monitor.report();
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.00 KB");
        assert_eq!(format_bytes(1024 * 1024), "1.00 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.00 GB");
    }

    #[test]
    #[cfg(feature = "jemalloc")]
    fn test_arena_creation() {
        let arena = create_arena().expect("Should create arena");
        println!("Created arena: {}", arena);

        assign_thread_to_arena(arena).expect("Should assign thread to arena");
        println!("Assigned thread to arena {}", arena);
    }

    #[test]
    fn test_measure_allocation() {
        let (result, snap) = measure_allocation("allocate_vector", || {
            vec![0u8; 10 * 1024 * 1024] // 10 MB
        });

        assert_eq!(result.len(), 10 * 1024 * 1024);

        #[cfg(feature = "jemalloc")]
        {
            assert!(snap.delta_allocated > 0, "Should show allocation");
        }
    }
}

// ============================================================================
// Example Usage
// ============================================================================

#[allow(dead_code)]
fn example_usage() {
    // 1. Monitor allocations
    let mut monitor = AllocMonitor::new();
    monitor.snapshot();

    // Your code here
    let _data = vec![0u8; 1024 * 1024];

    monitor.snapshot();
    monitor.report();

    // 2. Test arena limits
    test_arena_creation_limit(1000);

    // 3. Print config
    print_jemalloc_config();

    // 4. Measure specific section
    let (_result, _snap) = measure_allocation("my_function", || {
        // Your code here
        vec![0u8; 10 * 1024]
    });

    // 5. Profile with heap dumps
    profile_section("important_section", || {
        // Your code here
    });
}
