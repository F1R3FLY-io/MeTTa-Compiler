// massif_parser.rs — Parse Valgrind massif output files.
//
// Input format: line-oriented text with `snapshot=N`, `time=T`, `mem_heap_B=N`,
// `heap_tree=empty|detailed`, and indented `nN: BYTES ADDR: function (location)` tree nodes.

use std::fs;

use crate::function_map::extract_leaf_name;

/// Complete parsed massif output.
pub struct MassifProfile {
    /// All snapshots in order.
    pub snapshots: Vec<MassifSnapshot>,
    /// Index of the peak snapshot (max mem_heap_bytes), if any.
    pub peak_snapshot_idx: Option<usize>,
    /// The command line from the `cmd:` header.
    pub command: String,
    /// Time unit from `time_unit:` header ("ms", "i", or "B").
    pub time_unit: String,
}

/// A single massif snapshot.
pub struct MassifSnapshot {
    pub snapshot_num: u32,
    pub time: u64,
    pub mem_heap_bytes: u64,
    pub mem_heap_extra_bytes: u64,
    /// `None` if `heap_tree=empty`, `Some(root)` if `heap_tree=detailed`.
    pub heap_tree: Option<MassifHeapNode>,
}

/// A node in the heap allocation tree.
pub struct MassifHeapNode {
    pub bytes: u64,
    /// Function name (empty string for root/aggregate nodes like "(heap allocation functions)").
    pub function_name: String,
    /// Source file:line if available.
    pub source_location: Option<String>,
    pub children: Vec<MassifHeapNode>,
}

/// Parser state machine states.
#[derive(Debug, PartialEq)]
enum State {
    Header,
    BetweenSnapshots,
    InSnapshot,
    InHeapTree,
}

/// Parse a massif output file into a `MassifProfile`.
pub fn parse_massif_output(path: &str) -> Result<MassifProfile, String> {
    let content = fs::read_to_string(path)
        .map_err(|e| format!("Failed to read massif file '{}': {}", path, e))?;

    if content.trim().is_empty() {
        return Err(format!("Massif file '{}' is empty", path));
    }

    let mut command = String::new();
    let mut time_unit = String::from("i");
    let mut snapshots: Vec<MassifSnapshot> = Vec::new();
    let mut state = State::Header;

    // Current snapshot being built
    let mut cur_snapshot_num: u32 = 0;
    let mut cur_time: u64 = 0;
    let mut cur_heap_bytes: u64 = 0;
    let mut cur_heap_extra_bytes: u64 = 0;
    let mut cur_tree_lines: Vec<String> = Vec::new();
    let mut in_detailed_tree = false;

    for line in content.lines() {
        let trimmed = line.trim();

        // Header lines
        if trimmed.starts_with("desc:") {
            continue; // skip desc lines
        }
        if let Some(cmd) = trimmed.strip_prefix("cmd:") {
            command = cmd.trim().to_string();
            continue;
        }
        if let Some(tu) = trimmed.strip_prefix("time_unit:") {
            time_unit = tu.trim().to_string();
            continue;
        }

        // Snapshot boundary
        if let Some(num_str) = trimmed.strip_prefix("snapshot=") {
            // If we were in a snapshot, finalize it
            if state == State::InSnapshot || state == State::InHeapTree {
                let heap_tree = if in_detailed_tree && !cur_tree_lines.is_empty() {
                    Some(parse_heap_tree(&cur_tree_lines)?)
                } else {
                    None
                };
                snapshots.push(MassifSnapshot {
                    snapshot_num: cur_snapshot_num,
                    time: cur_time,
                    mem_heap_bytes: cur_heap_bytes,
                    mem_heap_extra_bytes: cur_heap_extra_bytes,
                    heap_tree,
                });
            }

            cur_snapshot_num = num_str
                .trim()
                .parse()
                .map_err(|_| format!("Invalid snapshot number: '{}'", num_str.trim()))?;
            cur_time = 0;
            cur_heap_bytes = 0;
            cur_heap_extra_bytes = 0;
            cur_tree_lines.clear();
            in_detailed_tree = false;
            state = State::InSnapshot;
            continue;
        }

        // Snapshot scalar fields
        if state == State::InSnapshot || state == State::InHeapTree {
            if let Some(val) = trimmed.strip_prefix("time=") {
                cur_time = val.trim().parse().unwrap_or(0);
                continue;
            }
            if let Some(val) = trimmed.strip_prefix("mem_heap_B=") {
                cur_heap_bytes = val.trim().parse().unwrap_or(0);
                continue;
            }
            if let Some(val) = trimmed.strip_prefix("mem_heap_extra_B=") {
                cur_heap_extra_bytes = val.trim().parse().unwrap_or(0);
                continue;
            }
            if trimmed.starts_with("mem_stacks_B=") {
                continue; // skip stack bytes
            }

            if trimmed == "heap_tree=empty" {
                in_detailed_tree = false;
                continue;
            }
            if trimmed == "heap_tree=detailed" || trimmed == "heap_tree=peak" {
                in_detailed_tree = true;
                state = State::InHeapTree;
                continue;
            }

            // Tree lines start with 'n'
            if state == State::InHeapTree && trimmed.starts_with('n') {
                cur_tree_lines.push(line.to_string()); // preserve indentation
                continue;
            }
        }

        if trimmed.is_empty() {
            if state == State::Header {
                state = State::BetweenSnapshots;
            }
            continue;
        }
    }

    // Finalize last snapshot
    if state == State::InSnapshot || state == State::InHeapTree {
        let heap_tree = if in_detailed_tree && !cur_tree_lines.is_empty() {
            Some(parse_heap_tree(&cur_tree_lines)?)
        } else {
            None
        };
        snapshots.push(MassifSnapshot {
            snapshot_num: cur_snapshot_num,
            time: cur_time,
            mem_heap_bytes: cur_heap_bytes,
            mem_heap_extra_bytes: cur_heap_extra_bytes,
            heap_tree,
        });
    }

    // Find peak snapshot
    let peak_snapshot_idx = if snapshots.is_empty() {
        None
    } else {
        Some(
            snapshots
                .iter()
                .enumerate()
                .max_by_key(|(_, s)| s.mem_heap_bytes)
                .map(|(i, _)| i)
                .unwrap_or(0),
        )
    };

    Ok(MassifProfile {
        snapshots,
        peak_snapshot_idx,
        command,
        time_unit,
    })
}

/// Parse the heap tree from indented `nN: BYTES ...` lines.
///
/// Format per line (indentation = depth):
///   `nCHILD_COUNT: BYTES (ADDR: function_name (file:line))`
///   or `nCHILD_COUNT: BYTES (ADDR: function_name)`
///   or `nCHILD_COUNT: BYTES in N places, all below massif's threshold (X%)`
///   or `nCHILD_COUNT: BYTES (below threshold)`
fn parse_heap_tree(lines: &[String]) -> Result<MassifHeapNode, String> {
    if lines.is_empty() {
        return Err("Empty heap tree".to_string());
    }

    // Build nodes with depth tracking
    let mut nodes: Vec<(usize, MassifHeapNode)> = Vec::new();

    for line in lines {
        let depth = count_leading_spaces(line);
        let trimmed = line.trim();

        let node = parse_tree_line(trimmed)?;
        nodes.push((depth, node));
    }

    // Build tree from flat list using depth as structure
    build_tree_from_flat(&nodes)
}

/// Count leading space characters (each space = 1 depth unit).
fn count_leading_spaces(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Parse a single tree line like `n3: 1024 0x12345: function_name (file.c:42)`.
fn parse_tree_line(line: &str) -> Result<MassifHeapNode, String> {
    // Skip the 'n' prefix and parse child count
    let after_n = line
        .strip_prefix('n')
        .ok_or_else(|| format!("Tree line doesn't start with 'n': {}", line))?;

    let colon_idx = after_n
        .find(':')
        .ok_or_else(|| format!("No colon after child count: {}", line))?;

    let _child_count: u32 = after_n[..colon_idx]
        .trim()
        .parse()
        .map_err(|_| format!("Invalid child count in: {}", line))?;

    let rest = after_n[colon_idx + 1..].trim();

    // Parse bytes (first token)
    let (bytes_str, remainder) = match rest.find(|c: char| !c.is_ascii_digit()) {
        Some(idx) => (&rest[..idx], rest[idx..].trim()),
        None => (rest, ""),
    };

    let bytes: u64 = bytes_str
        .parse()
        .map_err(|_| format!("Invalid byte count '{}' in: {}", bytes_str, line))?;

    // Parse function name and location from remainder
    let (function_name, source_location) = parse_function_and_location(remainder);

    Ok(MassifHeapNode {
        bytes,
        function_name,
        source_location,
        children: Vec::new(),
    })
}

/// Extract function name and optional source location from the remainder of a tree line.
///
/// Formats:
///   `0x12345: function_name (file.c:42)` — function with location
///   `0x12345: function_name`             — function without location
///   `in N places, all below ...`         — threshold aggregate
///   `(heap allocation functions) ...`    — special aggregate
///   empty                                — root node
fn parse_function_and_location(s: &str) -> (String, Option<String>) {
    if s.is_empty() {
        return (String::new(), None);
    }

    // "(below threshold)" or "in N places, all below ..."
    if s.starts_with("in ") || s.starts_with("(below") {
        return ("(below threshold)".to_string(), None);
    }

    // Strip address prefix: `0xHEX: `
    let after_addr = if s.starts_with("0x") || s.starts_with("0X") {
        if let Some(colon_idx) = s.find(": ") {
            s[colon_idx + 2..].trim()
        } else {
            s
        }
    } else {
        s
    };

    // Check for source location in parentheses at end: `function_name (file:line)`
    if let Some(paren_start) = after_addr.rfind(" (") {
        let paren_end = after_addr.len();
        if after_addr.ends_with(')') {
            let func = after_addr[..paren_start].trim();
            let loc = &after_addr[paren_start + 2..paren_end - 1];
            return (extract_leaf_name(func).to_string(), Some(loc.to_string()));
        }
    }

    // No location — just function name
    (extract_leaf_name(after_addr.trim()).to_string(), None)
}

/// Build a tree from a flat list of (depth, node) pairs.
fn build_tree_from_flat(nodes: &[(usize, MassifHeapNode)]) -> Result<MassifHeapNode, String> {
    if nodes.is_empty() {
        return Err("Empty node list".to_string());
    }

    // Clone the root node
    let mut root = MassifHeapNode {
        bytes: nodes[0].1.bytes,
        function_name: nodes[0].1.function_name.clone(),
        source_location: nodes[0].1.source_location.clone(),
        children: Vec::new(),
    };

    if nodes.len() == 1 {
        return Ok(root);
    }

    let root_depth = nodes[0].0;

    // Use a stack of (depth, &mut node) to build the tree
    // We process children iteratively using depth to determine parentage
    build_children(&mut root, &nodes[1..], root_depth);

    Ok(root)
}

/// Recursively build children for a parent node from a flat node list.
fn build_children(
    parent: &mut MassifHeapNode,
    nodes: &[(usize, MassifHeapNode)],
    parent_depth: usize,
) {
    let mut i = 0;
    while i < nodes.len() {
        let (depth, _) = &nodes[i];

        // If depth <= parent_depth, this node belongs to a higher-level parent
        if *depth <= parent_depth {
            break;
        }

        // Direct child: depth == parent_depth + 1
        // (massif uses 1-space indentation per level)
        let mut child = MassifHeapNode {
            bytes: nodes[i].1.bytes,
            function_name: nodes[i].1.function_name.clone(),
            source_location: nodes[i].1.source_location.clone(),
            children: Vec::new(),
        };

        // Find the range of sub-children for this child
        let child_depth = *depth;
        let mut j = i + 1;
        while j < nodes.len() && nodes[j].0 > child_depth {
            j += 1;
        }

        // Recursively build sub-children
        if j > i + 1 {
            build_children(&mut child, &nodes[i + 1..j], child_depth);
        }

        parent.children.push(child);
        i = j;
    }
}

/// Walk the peak snapshot's heap tree and return `(function_name, bytes, pct_of_peak)`
/// sorted by bytes descending. Only includes leaf-level allocations (functions that
/// directly allocate, not their parents).
pub fn walk_peak_allocations(profile: &MassifProfile) -> Vec<(String, u64, f64)> {
    let peak_idx = match profile.peak_snapshot_idx {
        Some(idx) => idx,
        None => return Vec::new(),
    };

    let snapshot = &profile.snapshots[peak_idx];
    let tree = match &snapshot.heap_tree {
        Some(t) => t,
        None => return Vec::new(),
    };

    let peak_bytes = snapshot.mem_heap_bytes.max(1); // avoid div by zero

    let mut allocations: Vec<(String, u64, f64)> = Vec::new();
    collect_leaf_allocations(tree, peak_bytes, &mut allocations);

    // Sort by bytes descending
    allocations.sort_by(|a, b| b.1.cmp(&a.1));
    allocations
}

/// Collect leaf-level allocations from the tree. A leaf is a node with no children
/// (it directly allocates) or a node whose children sum to less than its bytes
/// (it has its own direct allocations).
fn collect_leaf_allocations(
    node: &MassifHeapNode,
    peak_bytes: u64,
    out: &mut Vec<(String, u64, f64)>,
) {
    if node.children.is_empty() {
        // Leaf node — direct allocator
        if node.bytes > 0 && !node.function_name.is_empty() {
            let pct = node.bytes as f64 / peak_bytes as f64 * 100.0;
            out.push((node.function_name.clone(), node.bytes, pct));
        }
    } else {
        // Internal node — recurse into children
        for child in &node.children {
            collect_leaf_allocations(child, peak_bytes, out);
        }
    }
}

/// Format bytes into human-readable form.
pub fn format_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{} B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1} KiB", bytes as f64 / 1024.0)
    } else if bytes < 1024 * 1024 * 1024 {
        format!("{:.1} MiB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.2} GiB", bytes as f64 / (1024.0 * 1024.0 * 1024.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn write_temp_file(content: &str) -> String {
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = format!("/tmp/test_massif_{}_{}.txt", std::process::id(), id);
        let mut f = fs::File::create(&path).expect("create temp file");
        f.write_all(content.as_bytes()).expect("write temp file");
        path
    }

    const SAMPLE_MASSIF: &str = "\
desc: --tool=massif --massif-out-file=massif.out
cmd: ./target/release/mettatron examples/simple.metta
time_unit: i

snapshot=0
time=0
mem_heap_B=0
mem_heap_extra_B=0
mem_stacks_B=0
heap_tree=empty

snapshot=1
time=1000000
mem_heap_B=4096
mem_heap_extra_B=512
mem_stacks_B=0
heap_tree=empty

snapshot=2
time=2000000
mem_heap_B=8192
mem_heap_extra_B=1024
mem_stacks_B=0
heap_tree=detailed
n2: 8192 (heap allocation functions) malloc/new/new[], --alloc-fns, etc.
 n1: 6144 0xABCDEF: mettatron::backend::eval::eval_inner (eval.rs:100)
  n0: 6144 0x123456: alloc::alloc::exchange_malloc (alloc.rs:50)
 n1: 2048 0xFEDCBA: mettatron::slab::SlabAllocator::alloc_value (slab.rs:200)
  n0: 2048 0x654321: core::ptr::write (ptr.rs:10)

snapshot=3
time=3000000
mem_heap_B=4000
mem_heap_extra_B=500
mem_stacks_B=0
heap_tree=empty
";

    #[test]
    fn test_parse_massif_basic() {
        let path = write_temp_file(SAMPLE_MASSIF);
        let profile = parse_massif_output(&path).expect("parse should succeed");
        fs::remove_file(&path).ok();

        assert_eq!(
            profile.command,
            "./target/release/mettatron examples/simple.metta"
        );
        assert_eq!(profile.time_unit, "i");
        assert_eq!(profile.snapshots.len(), 4);

        // Peak should be snapshot 2 (8192 bytes)
        assert_eq!(profile.peak_snapshot_idx, Some(2));
        assert_eq!(profile.snapshots[2].mem_heap_bytes, 8192);
        assert_eq!(profile.snapshots[2].mem_heap_extra_bytes, 1024);

        // Snapshot 2 should have a detailed tree
        assert!(profile.snapshots[2].heap_tree.is_some());
        // Snapshot 0, 1, 3 should not
        assert!(profile.snapshots[0].heap_tree.is_none());
        assert!(profile.snapshots[1].heap_tree.is_none());
        assert!(profile.snapshots[3].heap_tree.is_none());
    }

    #[test]
    fn test_walk_peak_allocations() {
        let path = write_temp_file(SAMPLE_MASSIF);
        let profile = parse_massif_output(&path).expect("parse should succeed");
        fs::remove_file(&path).ok();

        let allocs = walk_peak_allocations(&profile);
        // Should have leaf nodes: exchange_malloc (6144) and write (2048)
        assert_eq!(allocs.len(), 2);
        assert_eq!(allocs[0].0, "exchange_malloc");
        assert_eq!(allocs[0].1, 6144);
        assert_eq!(allocs[1].0, "write");
        assert_eq!(allocs[1].1, 2048);

        // Percentages should add up to 100%
        let total_pct: f64 = allocs.iter().map(|a| a.2).sum();
        assert!((total_pct - 100.0).abs() < 0.1);
    }

    #[test]
    fn test_empty_file_error() {
        let path = write_temp_file("");
        let result = parse_massif_output(&path);
        fs::remove_file(&path).ok();
        assert!(result.is_err());
    }

    #[test]
    fn test_no_detailed_snapshots() {
        let content = "\
desc: test
cmd: test
time_unit: ms

snapshot=0
time=0
mem_heap_B=1024
mem_heap_extra_B=0
mem_stacks_B=0
heap_tree=empty
";
        let path = write_temp_file(content);
        let profile = parse_massif_output(&path).expect("parse should succeed");
        fs::remove_file(&path).ok();

        assert_eq!(profile.snapshots.len(), 1);
        assert_eq!(profile.peak_snapshot_idx, Some(0));
        assert!(profile.snapshots[0].heap_tree.is_none());

        let allocs = walk_peak_allocations(&profile);
        assert!(allocs.is_empty());
    }

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(1536), "1.5 KiB");
        assert_eq!(format_bytes(1048576), "1.0 MiB");
        assert_eq!(format_bytes(1073741824), "1.00 GiB");
    }

    #[test]
    fn test_parse_function_and_location() {
        let (name, loc) = parse_function_and_location("0xABCDEF: eval_inner (eval.rs:100)");
        assert_eq!(name, "eval_inner");
        assert_eq!(loc, Some("eval.rs:100".to_string()));

        let (name, loc) = parse_function_and_location("0x123: alloc::alloc::exchange_malloc");
        assert_eq!(name, "exchange_malloc");
        assert!(loc.is_none());

        let (name, _loc) =
            parse_function_and_location("in 5 places, all below massif's threshold (1.00%)");
        assert_eq!(name, "(below threshold)");

        let (name, _loc) = parse_function_and_location("");
        assert_eq!(name, "");
    }
}
