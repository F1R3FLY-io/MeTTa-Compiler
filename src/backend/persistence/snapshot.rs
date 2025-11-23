// Snapshot module: Serialize/deserialize knowledge bases using PathMap ACT format
//
// This module provides instant O(1) loading of knowledge bases via memory-mapped files.
// The key insight: Instead of parsing on load, we pre-compute the trie structure
// and memory-map it directly.

use crate::backend::models::MettaValue;
use crate::backend::persistence::TermStore;
use pathmap::arena_compact::ArenaCompactTree;
use pathmap::zipper::{Zipper, ZipperValues};
use std::path::Path;
use std::io;
use memmap2::Mmap;

/// Snapshot metadata stored alongside ACT file
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SnapshotMetadata {
    /// Version of the snapshot format
    pub format_version: u32,
    /// Timestamp when snapshot was created
    pub created_at: u64,
    /// Number of unique terms in the knowledge base
    pub num_terms: usize,
    /// Number of paths (rules/facts) in the knowledge base
    pub num_paths: usize,
    /// Whether merkleization was applied
    pub merkleized: bool,
}

impl SnapshotMetadata {
    pub fn new(num_terms: usize, num_paths: usize, merkleized: bool) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let created_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        SnapshotMetadata {
            format_version: 1,
            created_at,
            num_terms,
            num_paths,
            merkleized,
        }
    }
}

/// Create a snapshot from a PathMap zipper
///
/// # Arguments
/// * `zipper` - PathMap zipper to serialize
/// * `term_store` - Term store for encoding MettaValue as u64
/// * `tree_path` - Output path for ACT file (e.g., "kb.tree")
/// * `metadata_path` - Output path for metadata file (e.g., "kb.meta")
/// * `merkleize` - Whether merkleization was applied (for metadata only)
///
/// # Note
/// The `merkleize` parameter is for metadata tracking only. Actual merkleization
/// must be performed on the PathMap/TrieMap before creating the zipper by calling
/// `.merkleize()` on the map. This reduces file size by ~70% via structural sharing.
///
/// # Returns
/// Snapshot metadata on success
pub fn create_snapshot<V, Z>(
    zipper: Z,
    term_store: &TermStore,
    tree_path: impl AsRef<Path>,
    metadata_path: impl AsRef<Path>,
    merkleize: bool,
) -> io::Result<SnapshotMetadata>
where
    V: Clone + Send + Sync + Unpin,
    Z: pathmap::morphisms::Catamorphism<V>,
{
    // Count paths for metadata
    let num_paths = count_paths(&zipper);

    // Create metadata
    let metadata = SnapshotMetadata::new(
        term_store.len(),
        num_paths,
        merkleize,
    );

    // Serialize ACT file
    // Map values using term store (MettaValue -> u64)
    ArenaCompactTree::dump_from_zipper(
        zipper,
        |_value: &V| {
            // For now, we'll use a placeholder mapping
            // In the full implementation, this would use term_store
            0u64
        },
        tree_path,
    )?;

    // Serialize metadata as bincode
    let metadata_bytes = bincode::serialize(&metadata)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
    std::fs::write(metadata_path, metadata_bytes)?;

    Ok(metadata)
}

/// Load a snapshot from disk (O(1) via mmap)
///
/// # Arguments
/// * `tree_path` - Path to ACT file
/// * `metadata_path` - Path to metadata file
///
/// # Returns
/// Tuple of (ACT tree, metadata)
pub fn load_snapshot(
    tree_path: impl AsRef<Path>,
    metadata_path: impl AsRef<Path>,
) -> io::Result<(ArenaCompactTree<Mmap>, SnapshotMetadata)> {
    // Load ACT file via mmap (O(1))
    let tree = ArenaCompactTree::open_mmap(tree_path)?;

    // Load metadata
    let metadata_bytes = std::fs::read(metadata_path)?;
    let metadata: SnapshotMetadata = bincode::deserialize(&metadata_bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

    Ok((tree, metadata))
}

/// Count number of paths in a zipper (for metadata)
fn count_paths<V, Z>(zipper: &Z) -> usize
where
    V: Clone,
    Z: pathmap::morphisms::Catamorphism<V>,
{
    // This is a placeholder - actual implementation would traverse the zipper
    // For now, return 0 to allow compilation
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_snapshot_metadata() {
        let metadata = SnapshotMetadata::new(100, 50, true);

        assert_eq!(metadata.format_version, 1);
        assert_eq!(metadata.num_terms, 100);
        assert_eq!(metadata.num_paths, 50);
        assert_eq!(metadata.merkleized, true);
        assert!(metadata.created_at > 0);
    }

    #[test]
    fn test_metadata_serialization() {
        let metadata = SnapshotMetadata::new(100, 50, true);

        // Serialize
        let bytes = bincode::serialize(&metadata).unwrap();

        // Deserialize
        let deserialized: SnapshotMetadata = bincode::deserialize(&bytes).unwrap();

        assert_eq!(metadata.format_version, deserialized.format_version);
        assert_eq!(metadata.num_terms, deserialized.num_terms);
        assert_eq!(metadata.num_paths, deserialized.num_paths);
        assert_eq!(metadata.merkleized, deserialized.merkleized);
        assert_eq!(metadata.created_at, deserialized.created_at);
    }
}
