//! Native Parallel PathMap Operations
//!
//! Implements parallel set operations using PathMap's native parallelism pattern:
//! - `std::thread::scope` for bounded thread lifetimes
//! - `mpsc::channel` for zipper dispatch
//! - Path-prefix partitioning for exclusive access
//!
//! This avoids Rayon which causes jemalloc segfaults with PathMap.

use super::models::MettaValue;
use super::pathmap_converter::{
    metta_expr_to_pathmap_multiset, pathmap_multiset_to_metta_expr, MultisetCount,
};
use pathmap::zipper::{ZipperIteration, ZipperMoving, ZipperReadOnlyValues, ZipperWriting};
use pathmap::PathMap;
use std::sync::mpsc;
use std::thread;

/// Configuration for parallel operations
#[derive(Debug, Clone, Copy)]
pub struct ParallelConfig {
    /// Number of threads to use (0 = auto-detect)
    pub thread_count: usize,
}

impl Default for ParallelConfig {
    fn default() -> Self {
        Self {
            thread_count: num_cpus(),
        }
    }
}

impl ParallelConfig {
    /// Create config with specific thread count
    pub fn with_threads(thread_count: usize) -> Self {
        Self {
            thread_count: thread_count.max(1),
        }
    }
}

/// Get number of CPU cores (simple fallback)
fn num_cpus() -> usize {
    std::thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(4)
}

// ============================================================================
// Parallel Conversion (Serialization Phase)
// ============================================================================

/// Convert MettaValue elements to path strings in parallel.
/// This parallelizes the expensive `to_path_map_string()` calls.
fn parallel_convert_to_paths(items: &[MettaValue], thread_count: usize) -> Vec<String> {
    if thread_count <= 1 || items.len() < 100 {
        // Sequential for small inputs
        return items.iter().map(|v| v.to_path_map_string()).collect();
    }

    let chunk_size = (items.len() + thread_count - 1) / thread_count;
    let mut results: Vec<String> = Vec::with_capacity(items.len());

    thread::scope(|scope| {
        let mut handles = Vec::with_capacity(thread_count);
        let mut receivers = Vec::with_capacity(thread_count);

        // Spawn worker threads
        for chunk_idx in 0..thread_count {
            let start = chunk_idx * chunk_size;
            let end = (start + chunk_size).min(items.len());
            if start >= items.len() {
                break;
            }

            let chunk = &items[start..end];
            let (tx, rx) = mpsc::channel::<Vec<String>>();
            receivers.push((chunk_idx, rx));

            handles.push(scope.spawn(move || {
                let converted: Vec<String> = chunk.iter().map(|v| v.to_path_map_string()).collect();
                tx.send(converted).expect("Channel send failed");
            }));
        }

        // Collect results in order
        let mut ordered_results: Vec<(usize, Vec<String>)> = Vec::with_capacity(receivers.len());
        for (idx, rx) in receivers {
            if let Ok(chunk_results) = rx.recv() {
                ordered_results.push((idx, chunk_results));
            }
        }
        ordered_results.sort_by_key(|(idx, _)| *idx);

        for (_, chunk_results) in ordered_results {
            results.extend(chunk_results);
        }
    });

    results
}

/// Build PathMap from pre-converted paths (sequential, as PathMap isn't thread-safe for writes)
fn build_pathmap_from_paths(paths: &[String]) -> PathMap<MultisetCount> {
    let mut path_map = PathMap::<MultisetCount>::new();

    for path in paths {
        let path_bytes = path.as_bytes();
        let mut wz = path_map.write_zipper_at_path(path_bytes);

        if let Some(count) = wz.get_val_mut() {
            count.increment();
        } else {
            wz.get_val_or_set_mut(MultisetCount(1));
        }
    }

    path_map
}

// ============================================================================
// Parallel Set Operations
// ============================================================================

/// Parallel intersection using PathMap lattice meet operation.
/// Parallelizes the serialization phase, then applies sequential lattice operation.
pub fn parallel_intersection(
    left: &MettaValue,
    right: &MettaValue,
    config: ParallelConfig,
) -> Result<MettaValue, String> {
    // Extract items from SExpr
    let left_items = match left {
        MettaValue::SExpr(items) => items,
        MettaValue::Nil => return Ok(MettaValue::SExpr(vec![])),
        _ => return Err(format!("Expected SExpr for intersection, got {:?}", left)),
    };

    let right_items = match right {
        MettaValue::SExpr(items) => items,
        MettaValue::Nil => return Ok(MettaValue::SExpr(vec![])),
        _ => return Err(format!("Expected SExpr for intersection, got {:?}", right)),
    };

    // Parallel conversion to paths
    let left_paths = parallel_convert_to_paths(left_items, config.thread_count);
    let right_paths = parallel_convert_to_paths(right_items, config.thread_count);

    // Build PathMaps (sequential - PathMap write operations aren't thread-safe)
    let left_pm = build_pathmap_from_paths(&left_paths);
    let right_pm = build_pathmap_from_paths(&right_paths);

    // Apply lattice meet operation
    let result_pm = left_pm.meet(&right_pm);

    // Convert back to MettaValue
    pathmap_multiset_to_metta_expr(result_pm)
}

/// Parallel subtraction using PathMap lattice subtract operation.
/// Parallelizes the serialization phase, then applies sequential lattice operation.
pub fn parallel_subtraction(
    left: &MettaValue,
    right: &MettaValue,
    config: ParallelConfig,
) -> Result<MettaValue, String> {
    // Extract items from SExpr
    let left_items = match left {
        MettaValue::SExpr(items) => items,
        MettaValue::Nil => return Ok(MettaValue::SExpr(vec![])),
        _ => return Err(format!("Expected SExpr for subtraction, got {:?}", left)),
    };

    let right_items = match right {
        MettaValue::SExpr(items) => items,
        MettaValue::Nil => return Ok(left.clone()),
        _ => return Err(format!("Expected SExpr for subtraction, got {:?}", right)),
    };

    // Parallel conversion to paths
    let left_paths = parallel_convert_to_paths(left_items, config.thread_count);
    let right_paths = parallel_convert_to_paths(right_items, config.thread_count);

    // Build PathMaps (sequential)
    let left_pm = build_pathmap_from_paths(&left_paths);
    let right_pm = build_pathmap_from_paths(&right_paths);

    // Apply lattice subtract operation
    let result_pm = left_pm.subtract(&right_pm);

    // Convert back to MettaValue
    pathmap_multiset_to_metta_expr(result_pm)
}

/// Parallel union using PathMap lattice join operation.
/// Parallelizes the serialization phase, then applies sequential lattice operation.
pub fn parallel_union(
    left: &MettaValue,
    right: &MettaValue,
    config: ParallelConfig,
) -> Result<MettaValue, String> {
    // Extract items from SExpr
    let left_items = match left {
        MettaValue::SExpr(items) => items,
        MettaValue::Nil => return Ok(right.clone()),
        _ => return Err(format!("Expected SExpr for union, got {:?}", left)),
    };

    let right_items = match right {
        MettaValue::SExpr(items) => items,
        MettaValue::Nil => return Ok(left.clone()),
        _ => return Err(format!("Expected SExpr for union, got {:?}", right)),
    };

    // Parallel conversion to paths
    let left_paths = parallel_convert_to_paths(left_items, config.thread_count);
    let right_paths = parallel_convert_to_paths(right_items, config.thread_count);

    // Build PathMaps (sequential)
    let left_pm = build_pathmap_from_paths(&left_paths);
    let right_pm = build_pathmap_from_paths(&right_paths);

    // Apply lattice join operation
    let result_pm = left_pm.join(&right_pm);

    // Convert back to MettaValue
    pathmap_multiset_to_metta_expr(result_pm)
}

// ============================================================================
// Advanced Parallel Pattern: Partitioned Parallelism
// ============================================================================

/// Partition-based parallel intersection that parallelizes more of the pipeline.
/// Uses path-prefix partitioning to enable parallel PathMap construction.
pub fn partitioned_parallel_intersection(
    left: &MettaValue,
    right: &MettaValue,
    config: ParallelConfig,
) -> Result<MettaValue, String> {
    let thread_count = config.thread_count.max(1);

    // Extract items from SExpr
    let left_items = match left {
        MettaValue::SExpr(items) => items,
        MettaValue::Nil => return Ok(MettaValue::SExpr(vec![])),
        _ => return Err(format!("Expected SExpr for intersection, got {:?}", left)),
    };

    let right_items = match right {
        MettaValue::SExpr(items) => items,
        MettaValue::Nil => return Ok(MettaValue::SExpr(vec![])),
        _ => return Err(format!("Expected SExpr for intersection, got {:?}", right)),
    };

    // For small inputs, fall back to sequential
    if left_items.len() < 1000 && right_items.len() < 1000 {
        let left_pm =
            metta_expr_to_pathmap_multiset(left).map_err(|e| format!("Left conversion: {}", e))?;
        let right_pm = metta_expr_to_pathmap_multiset(right)
            .map_err(|e| format!("Right conversion: {}", e))?;
        let result_pm = left_pm.meet(&right_pm);
        return pathmap_multiset_to_metta_expr(result_pm);
    }

    // Partition items by first character of path string (determines trie prefix)
    let mut left_partitions: Vec<Vec<String>> = vec![Vec::new(); thread_count];
    let mut right_partitions: Vec<Vec<String>> = vec![Vec::new(); thread_count];

    // Convert and partition left items
    for item in left_items {
        let path = item.to_path_map_string();
        let partition_idx = if path.is_empty() {
            0
        } else {
            (path.as_bytes()[0] as usize) % thread_count
        };
        left_partitions[partition_idx].push(path);
    }

    // Convert and partition right items
    for item in right_items {
        let path = item.to_path_map_string();
        let partition_idx = if path.is_empty() {
            0
        } else {
            (path.as_bytes()[0] as usize) % thread_count
        };
        right_partitions[partition_idx].push(path);
    }

    // Process partitions in parallel
    let mut result_paths: Vec<String> = Vec::new();

    thread::scope(|scope| {
        let mut handles = Vec::with_capacity(thread_count);
        let mut receivers = Vec::with_capacity(thread_count);

        for partition_idx in 0..thread_count {
            let left_partition = &left_partitions[partition_idx];
            let right_partition = &right_partitions[partition_idx];

            let (tx, rx) = mpsc::channel::<Vec<String>>();
            receivers.push((partition_idx, rx));

            handles.push(scope.spawn(move || {
                // Build partial PathMaps for this partition
                let left_pm = build_pathmap_from_paths(left_partition);
                let right_pm = build_pathmap_from_paths(right_partition);

                // Apply meet on this partition
                let result_pm = left_pm.meet(&right_pm);

                // Extract result paths
                let mut paths = Vec::new();
                let mut rz = result_pm.read_zipper();
                while rz.to_next_val() {
                    if let Ok(path_str) = std::str::from_utf8(rz.path()) {
                        let count = rz.get_val().map(|c| c.count()).unwrap_or(0);
                        for _ in 0..count {
                            paths.push(path_str.to_string());
                        }
                    }
                }

                tx.send(paths).expect("Channel send failed");
            }));
        }

        // Collect results
        let mut ordered_results: Vec<(usize, Vec<String>)> = Vec::with_capacity(receivers.len());
        for (idx, rx) in receivers {
            if let Ok(partition_results) = rx.recv() {
                ordered_results.push((idx, partition_results));
            }
        }
        ordered_results.sort_by_key(|(idx, _)| *idx);

        for (_, partition_results) in ordered_results {
            result_paths.extend(partition_results);
        }
    });

    // Convert result paths back to MettaValues
    let result_items: Result<Vec<MettaValue>, String> = result_paths
        .iter()
        .map(|path| super::pathmap_converter::parse_pathmap_path_public(path))
        .collect();

    Ok(MettaValue::SExpr(result_items?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parallel_intersection_basic() {
        let left = MettaValue::SExpr(vec![
            MettaValue::Atom("a".to_string()),
            MettaValue::Atom("b".to_string()),
            MettaValue::Atom("c".to_string()),
        ]);
        let right = MettaValue::SExpr(vec![
            MettaValue::Atom("b".to_string()),
            MettaValue::Atom("c".to_string()),
            MettaValue::Atom("d".to_string()),
        ]);

        let config = ParallelConfig::with_threads(2);
        let result = parallel_intersection(&left, &right, config).unwrap();

        if let MettaValue::SExpr(items) = result {
            assert_eq!(items.len(), 2); // b and c
            assert!(items.contains(&MettaValue::Atom("b".to_string())));
            assert!(items.contains(&MettaValue::Atom("c".to_string())));
        } else {
            panic!("Expected SExpr result");
        }
    }

    #[test]
    fn test_parallel_subtraction_basic() {
        let left = MettaValue::SExpr(vec![
            MettaValue::Atom("a".to_string()),
            MettaValue::Atom("b".to_string()),
            MettaValue::Atom("c".to_string()),
        ]);
        let right = MettaValue::SExpr(vec![
            MettaValue::Atom("b".to_string()),
            MettaValue::Atom("d".to_string()),
        ]);

        let config = ParallelConfig::with_threads(2);
        let result = parallel_subtraction(&left, &right, config).unwrap();

        if let MettaValue::SExpr(items) = result {
            assert_eq!(items.len(), 2); // a and c
            assert!(items.contains(&MettaValue::Atom("a".to_string())));
            assert!(items.contains(&MettaValue::Atom("c".to_string())));
        } else {
            panic!("Expected SExpr result");
        }
    }

    #[test]
    fn test_parallel_union_basic() {
        let left = MettaValue::SExpr(vec![
            MettaValue::Atom("a".to_string()),
            MettaValue::Atom("b".to_string()),
        ]);
        let right = MettaValue::SExpr(vec![
            MettaValue::Atom("b".to_string()),
            MettaValue::Atom("c".to_string()),
        ]);

        let config = ParallelConfig::with_threads(2);
        let result = parallel_union(&left, &right, config).unwrap();

        if let MettaValue::SExpr(items) = result {
            // Union is multiset sum: a(1) + b(1) + b(1) + c(1) = a(1), b(2), c(1)
            assert_eq!(items.len(), 4);
        } else {
            panic!("Expected SExpr result");
        }
    }

    #[test]
    fn test_parallel_config_default() {
        let config = ParallelConfig::default();
        assert!(config.thread_count >= 1);
    }
}
