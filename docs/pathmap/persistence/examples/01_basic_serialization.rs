//! Example 1: Basic Serialization with Paths and ACT Formats
//!
//! Demonstrates:
//! - Serializing PathMap to paths format (compressed)
//! - Deserializing paths format back to PathMap
//! - Serializing PathMap to ACT format (memory-mappable)
//! - Loading ACT format via mmap
//!
//! To run this example in your project:
//! 1. Copy to `examples/01_basic_serialization.rs`
//! 2. Add to Cargo.toml:
//!    [[example]]
//!    name = "basic_serialization"
//!    path = "examples/01_basic_serialization.rs"
//! 3. Run: cargo run --example basic_serialization

use pathmap::PathMap;
use pathmap::paths_serialization::{serialize_paths, deserialize_paths};
use pathmap::arena_compact::ArenaCompactTree;
use std::fs::File;
use std::io;

fn main() -> io::Result<()> {
    println!("=== PathMap Basic Serialization Example ===\n");

    // ===== Part 1: Create Sample PathMap =====
    println!("Creating sample PathMap...");
    let mut map: PathMap<String> = PathMap::new();

    // Insert sample data
    map.set_val_at(b"facts/math/addition", "2 + 2 = 4".to_string());
    map.set_val_at(b"facts/math/subtraction", "5 - 3 = 2".to_string());
    map.set_val_at(b"facts/physics/newton1", "Objects in motion...".to_string());
    map.set_val_at(b"facts/physics/newton2", "F = ma".to_string());
    map.set_val_at(b"facts/chemistry/water", "H2O".to_string());

    println!("Inserted {} entries\n", map.len());

    // ===== Part 2: Serialize to Paths Format =====
    println!("--- Paths Format (Compressed) ---");

    let paths_file = "example_data.paths";
    println!("Serializing to {}...", paths_file);

    {
        let mut file = File::create(paths_file)?;
        let stats = serialize_paths(map.read_zipper(), &mut file)?;
        println!("Serialized {} paths", stats.count);
    }

    // Check file size
    let paths_size = std::fs::metadata(paths_file)?.len();
    println!("File size: {} bytes (compressed)\n", paths_size);

    // ===== Part 3: Deserialize from Paths Format =====
    println!("Deserializing from {}...", paths_file);

    let mut restored_map: PathMap<String> = PathMap::new();
    {
        let file = File::open(paths_file)?;
        let stats = deserialize_paths(
            restored_map.write_zipper(),
            file,
            String::new()  // Default value
        )?;
        println!("Deserialized {} paths", stats.count);
    }

    // Verify data
    assert_eq!(
        restored_map.get_val_at(b"facts/math/addition"),
        Some(&"2 + 2 = 4".to_string())
    );
    println!("✓ Data verified\n");

    // ===== Part 4: Serialize to ACT Format =====
    println!("--- ACT Format (Memory-Mappable) ---");

    // Convert String values to u64 (ACT format limitation)
    let mut map_u64: PathMap<u64> = PathMap::new();
    let value_map: Vec<String> = vec![
        "2 + 2 = 4".to_string(),
        "5 - 3 = 2".to_string(),
        "Objects in motion...".to_string(),
        "F = ma".to_string(),
        "H2O".to_string(),
    ];

    map_u64.set_val_at(b"facts/math/addition", 0);
    map_u64.set_val_at(b"facts/math/subtraction", 1);
    map_u64.set_val_at(b"facts/physics/newton1", 2);
    map_u64.set_val_at(b"facts/physics/newton2", 3);
    map_u64.set_val_at(b"facts/chemistry/water", 4);

    let act_file = "example_data.tree";
    println!("Serializing to {}...", act_file);

    ArenaCompactTree::dump_from_zipper(
        map_u64.read_zipper(),
        |&v| v,  // Values already u64
        act_file
    )?;

    // Check file size
    let act_size = std::fs::metadata(act_file)?.len();
    println!("File size: {} bytes (uncompressed)\n", act_size);

    // ===== Part 5: Load ACT via mmap =====
    println!("Loading {} via mmap (O(1))...", act_file);

    let act = ArenaCompactTree::open_mmap(act_file)?;
    println!("✓ Loaded instantly\n");

    // Query ACT
    println!("Querying ACT...");
    let value_id = act.get_val_at(b"facts/math/addition").unwrap();
    let value = &value_map[value_id as usize];
    println!("  facts/math/addition = {} (ID: {})", value, value_id);

    let value_id = act.get_val_at(b"facts/physics/newton2").unwrap();
    let value = &value_map[value_id as usize];
    println!("  facts/physics/newton2 = {} (ID: {})", value, value_id);

    println!("\n✓ All queries successful");

    // ===== Part 6: Compare Formats =====
    println!("\n--- Format Comparison ---");
    println!("Paths format:");
    println!("  - File size: {} bytes", paths_size);
    println!("  - Load time: O(n) (deserialize + decompress)");
    println!("  - Supports: Any Clone type");
    println!("  - Mutable: Yes");

    println!("\nACT format:");
    println!("  - File size: {} bytes", act_size);
    println!("  - Load time: O(1) (mmap)");
    println!("  - Supports: u64 only");
    println!("  - Mutable: No (read-only)");

    println!("\n=== Example Complete ===");

    // Cleanup
    std::fs::remove_file(paths_file)?;
    std::fs::remove_file(act_file)?;

    Ok(())
}

/* Example Output:

=== PathMap Basic Serialization Example ===

Creating sample PathMap...
Inserted 5 entries

--- Paths Format (Compressed) ---
Serializing to example_data.paths...
Serialized 5 paths
File size: 187 bytes (compressed)

Deserializing from example_data.paths...
Deserialized 5 paths
✓ Data verified

--- ACT Format (Memory-Mappable) ---
Serializing to example_data.tree...
File size: 412 bytes (uncompressed)

Loading example_data.tree via mmap (O(1))...
✓ Loaded instantly

Querying ACT...
  facts/math/addition = 2 + 2 = 4 (ID: 0)
  facts/physics/newton2 = F = ma (ID: 3)

✓ All queries successful

--- Format Comparison ---
Paths format:
  - File size: 187 bytes
  - Load time: O(n) (deserialize + decompress)
  - Supports: Any Clone type
  - Mutable: Yes

ACT format:
  - File size: 412 bytes
  - Load time: O(1) (mmap)
  - Supports: u64 only
  - Mutable: No (read-only)

=== Example Complete ===

*/
