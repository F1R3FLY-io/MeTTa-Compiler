//! Tiered Compilation for JIT
//!
//! This module implements a multi-tier compilation strategy for MeTTa expressions:
//!
//! ```text
//! Tier 0: Tree-walking interpreter (cold code, first execution)
//! Tier 1: Bytecode VM (warm code, 10+ executions)
//! Tier 2: JIT Stage 1 (hot code, 100+ executions, arithmetic/boolean)
//! Tier 3: JIT Stage 2 (very hot code, 500+ executions, full native)
//! ```
//!
//! The tiered approach balances compilation overhead against runtime performance:
//! - Cold code runs immediately without compilation delay
//! - Warm code gets bytecode compilation amortized over many runs
//! - Hot code gets JIT compiled for maximum performance

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, RwLock};

use super::compiler::JitCompiler;
use super::profile::{JitProfile, JitState, HOT_THRESHOLD, WARM_THRESHOLD};
use crate::backend::bytecode::chunk::BytecodeChunk;

/// Threshold for Stage 2 JIT (full native with runtime calls)
pub const STAGE2_THRESHOLD: u32 = 500;

/// Execution tier for a bytecode chunk
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Tier {
    /// First execution - use tree-walking interpreter
    Interpreter = 0,

    /// 10+ executions - use bytecode VM
    Bytecode = 1,

    /// 100+ executions - JIT Stage 1 (arithmetic/boolean only)
    JitStage1 = 2,

    /// 500+ executions - JIT Stage 2 (full native with runtime calls)
    JitStage2 = 3,
}

impl Tier {
    /// Get the tier for a given execution count
    #[inline]
    pub fn from_count(count: u32) -> Self {
        if count >= STAGE2_THRESHOLD {
            Tier::JitStage2
        } else if count >= HOT_THRESHOLD {
            Tier::JitStage1
        } else if count >= WARM_THRESHOLD {
            Tier::Bytecode
        } else {
            Tier::Interpreter
        }
    }

    /// Get the minimum execution count for this tier
    #[inline]
    pub fn threshold(&self) -> u32 {
        match self {
            Tier::Interpreter => 0,
            Tier::Bytecode => WARM_THRESHOLD,
            Tier::JitStage1 => HOT_THRESHOLD,
            Tier::JitStage2 => STAGE2_THRESHOLD,
        }
    }

    /// Check if this tier uses JIT compilation
    #[inline]
    pub fn is_jit(&self) -> bool {
        matches!(self, Tier::JitStage1 | Tier::JitStage2)
    }
}

/// Unique identifier for a bytecode chunk
///
/// Used as a key in the JIT cache to look up compiled native code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkId(u64);

impl ChunkId {
    /// Create a new chunk ID from a bytecode chunk
    pub fn from_chunk(chunk: &BytecodeChunk) -> Self {
        use std::collections::hash_map::DefaultHasher;
        let mut hasher = DefaultHasher::new();

        // Hash the bytecode content
        chunk.code().hash(&mut hasher);

        // Include constant pool size to differentiate chunks
        chunk.constant_count().hash(&mut hasher);

        ChunkId(hasher.finish())
    }

    /// Create a chunk ID from a raw u64
    pub fn from_raw(id: u64) -> Self {
        ChunkId(id)
    }

    /// Get the raw u64 value
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

/// Entry in the JIT cache containing compiled native code
pub struct CacheEntry {
    /// The compiled native code function pointer
    pub native_code: *const (),

    /// Size of the compiled code in bytes
    pub code_size: usize,

    /// JIT profile for this chunk
    pub profile: Arc<JitProfile>,

    /// Tier at which this was compiled
    pub tier: Tier,

    /// Last access time (for LRU eviction)
    pub last_access: std::time::Instant,
}

// Safety: CacheEntry is Send because the native code pointer is a function pointer
// that can be safely sent between threads. The JitProfile is thread-safe via atomics.
unsafe impl Send for CacheEntry {}
unsafe impl Sync for CacheEntry {}

/// Thread-safe JIT cache for compiled native code
///
/// Uses an LRU eviction strategy when the cache is full.
pub struct JitCache {
    /// Map from chunk ID to compiled code entry
    entries: RwLock<HashMap<ChunkId, CacheEntry>>,

    /// Maximum number of entries in the cache
    max_entries: usize,

    /// Total bytes of compiled code (for memory tracking)
    total_code_bytes: RwLock<usize>,

    /// Maximum bytes of compiled code before eviction
    max_code_bytes: usize,
}

impl JitCache {
    /// Create a new JIT cache with default limits
    pub fn new() -> Self {
        JitCache {
            entries: RwLock::new(HashMap::new()),
            max_entries: 1024,
            total_code_bytes: RwLock::new(0),
            max_code_bytes: 64 * 1024 * 1024, // 64 MB default
        }
    }

    /// Create a new JIT cache with custom limits
    pub fn with_limits(max_entries: usize, max_code_bytes: usize) -> Self {
        JitCache {
            entries: RwLock::new(HashMap::new()),
            max_entries,
            total_code_bytes: RwLock::new(0),
            max_code_bytes,
        }
    }

    /// Get a cached entry, updating its last access time
    pub fn get(&self, id: &ChunkId) -> Option<*const ()> {
        let mut entries = self.entries.write().ok()?;
        if let Some(entry) = entries.get_mut(id) {
            entry.last_access = std::time::Instant::now();
            Some(entry.native_code)
        } else {
            None
        }
    }

    /// Check if a chunk is cached
    pub fn contains(&self, id: &ChunkId) -> bool {
        self.entries
            .read()
            .map(|e| e.contains_key(id))
            .unwrap_or(false)
    }

    /// Insert a compiled entry into the cache
    pub fn insert(&self, id: ChunkId, entry: CacheEntry) {
        // Evict if necessary
        self.maybe_evict();

        let code_size = entry.code_size;
        if let Ok(mut entries) = self.entries.write() {
            entries.insert(id, entry);
        }
        if let Ok(mut total) = self.total_code_bytes.write() {
            *total += code_size;
        }
    }

    /// Remove an entry from the cache
    pub fn remove(&self, id: &ChunkId) -> Option<CacheEntry> {
        if let Ok(mut entries) = self.entries.write() {
            if let Some(entry) = entries.remove(id) {
                if let Ok(mut total) = self.total_code_bytes.write() {
                    *total = total.saturating_sub(entry.code_size);
                }
                return Some(entry);
            }
        }
        None
    }

    /// Get the number of cached entries
    pub fn len(&self) -> usize {
        self.entries.read().map(|e| e.len()).unwrap_or(0)
    }

    /// Check if the cache is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get the total bytes of compiled code
    pub fn total_code_bytes(&self) -> usize {
        self.total_code_bytes.read().map(|t| *t).unwrap_or(0)
    }

    /// Clear the entire cache
    pub fn clear(&self) {
        if let Ok(mut entries) = self.entries.write() {
            entries.clear();
        }
        if let Ok(mut total) = self.total_code_bytes.write() {
            *total = 0;
        }
    }

    /// Evict least recently used entries if cache is full
    fn maybe_evict(&self) {
        let should_evict = {
            let len = self.len();
            let total = self.total_code_bytes();
            len >= self.max_entries || total >= self.max_code_bytes
        };

        if should_evict {
            if let Ok(mut entries) = self.entries.write() {
                // Find the LRU entry
                let lru_id = entries
                    .iter()
                    .min_by_key(|(_, e)| e.last_access)
                    .map(|(id, _)| *id);

                if let Some(id) = lru_id {
                    if let Some(entry) = entries.remove(&id) {
                        if let Ok(mut total) = self.total_code_bytes.write() {
                            *total = total.saturating_sub(entry.code_size);
                        }
                    }
                }
            }
        }
    }
}

impl Default for JitCache {
    fn default() -> Self {
        JitCache::new()
    }
}

/// Statistics about tiered compilation
#[derive(Debug, Clone, Default)]
pub struct TieredStats {
    /// Number of interpreter executions
    pub interpreter_runs: u64,

    /// Number of bytecode VM executions
    pub bytecode_runs: u64,

    /// Number of JIT Stage 1 executions
    pub jit_stage1_runs: u64,

    /// Number of JIT Stage 2 executions
    pub jit_stage2_runs: u64,

    /// Number of successful JIT compilations
    pub jit_compilations: u64,

    /// Number of failed JIT compilations
    pub jit_failures: u64,

    /// Total bytes of JIT compiled code
    pub total_jit_bytes: u64,

    /// Number of cache hits
    pub cache_hits: u64,

    /// Number of cache misses
    pub cache_misses: u64,
}

impl TieredStats {
    /// Create new empty statistics
    pub const fn new() -> Self {
        TieredStats {
            interpreter_runs: 0,
            bytecode_runs: 0,
            jit_stage1_runs: 0,
            jit_stage2_runs: 0,
            jit_compilations: 0,
            jit_failures: 0,
            total_jit_bytes: 0,
            cache_hits: 0,
            cache_misses: 0,
        }
    }

    /// Record an execution at a given tier
    pub fn record_execution(&mut self, tier: Tier) {
        match tier {
            Tier::Interpreter => self.interpreter_runs += 1,
            Tier::Bytecode => self.bytecode_runs += 1,
            Tier::JitStage1 => self.jit_stage1_runs += 1,
            Tier::JitStage2 => self.jit_stage2_runs += 1,
        }
    }

    /// Total number of executions across all tiers
    pub fn total_executions(&self) -> u64 {
        self.interpreter_runs + self.bytecode_runs + self.jit_stage1_runs + self.jit_stage2_runs
    }

    /// Percentage of executions that used JIT
    pub fn jit_percentage(&self) -> f64 {
        let total = self.total_executions();
        if total == 0 {
            0.0
        } else {
            ((self.jit_stage1_runs + self.jit_stage2_runs) as f64 / total as f64) * 100.0
        }
    }

    /// Cache hit rate as a percentage
    pub fn cache_hit_rate(&self) -> f64 {
        let total = self.cache_hits + self.cache_misses;
        if total == 0 {
            0.0
        } else {
            (self.cache_hits as f64 / total as f64) * 100.0
        }
    }
}

/// Tiered compiler that manages execution tier selection
///
/// This struct coordinates between the interpreter, bytecode VM, and JIT compiler
/// to select the optimal execution path for each bytecode chunk.
pub struct TieredCompiler {
    /// JIT cache for compiled native code
    cache: JitCache,

    /// Per-chunk profiles (execution counts and JIT state)
    profiles: RwLock<HashMap<ChunkId, Arc<JitProfile>>>,

    /// JIT compiler instance (when jit feature is enabled)
    jit_compiler: RwLock<Option<JitCompiler>>,

    /// Statistics about tiered compilation
    stats: RwLock<TieredStats>,
}

impl TieredCompiler {
    /// Create a new tiered compiler
    pub fn new() -> Self {
        TieredCompiler {
            cache: JitCache::new(),
            profiles: RwLock::new(HashMap::new()),

            jit_compiler: RwLock::new(None),
            stats: RwLock::new(TieredStats::new()),
        }
    }

    /// Create a new tiered compiler with custom cache limits
    pub fn with_cache_limits(max_entries: usize, max_code_bytes: usize) -> Self {
        TieredCompiler {
            cache: JitCache::with_limits(max_entries, max_code_bytes),
            profiles: RwLock::new(HashMap::new()),

            jit_compiler: RwLock::new(None),
            stats: RwLock::new(TieredStats::new()),
        }
    }

    /// Get or create a profile for a chunk
    pub fn get_or_create_profile(&self, chunk: &BytecodeChunk) -> Arc<JitProfile> {
        let id = ChunkId::from_chunk(chunk);

        // Try read-only access first
        if let Ok(profiles) = self.profiles.read() {
            if let Some(profile) = profiles.get(&id) {
                return profile.clone();
            }
        }

        // Need to create a new profile
        let mut profiles = self
            .profiles
            .write()
            .expect("Failed to acquire profile lock");
        profiles
            .entry(id)
            .or_insert_with(|| Arc::new(JitProfile::new()))
            .clone()
    }

    /// Get the execution tier for a chunk based on its profile
    pub fn get_tier(&self, chunk: &BytecodeChunk) -> Tier {
        let profile = self.get_or_create_profile(chunk);
        let count = profile.execution_count();
        let state = profile.state();

        // If already JIT compiled, use JIT
        if state == JitState::Jitted {
            if count >= STAGE2_THRESHOLD {
                return Tier::JitStage2;
            } else {
                return Tier::JitStage1;
            }
        }

        // Otherwise, determine tier by execution count
        Tier::from_count(count)
    }

    /// Record an execution and potentially trigger JIT compilation
    ///
    /// Returns the tier that should be used for this execution.
    pub fn record_execution(&self, chunk: &BytecodeChunk) -> Tier {
        let profile = self.get_or_create_profile(chunk);
        let triggered_hot = profile.record_execution();
        let tier = self.get_tier(chunk);

        // Record stats
        if let Ok(mut stats) = self.stats.write() {
            stats.record_execution(tier);
        }

        // Check if we should JIT compile
        if triggered_hot && tier.is_jit() {
            self.maybe_compile(chunk, &profile, tier);
        }

        tier
    }

    /// Attempt to JIT compile a chunk
    fn maybe_compile(&self, chunk: &BytecodeChunk, profile: &Arc<JitProfile>, tier: Tier) {
        // Try to start compilation (only one thread will win)
        if !profile.try_start_compiling() {
            return;
        }

        // Check if chunk is compilable
        if !JitCompiler::can_compile_stage1(chunk) {
            profile.set_failed();
            if let Ok(mut stats) = self.stats.write() {
                stats.jit_failures += 1;
            }
            return;
        }

        // Create or get JIT compiler
        let compile_result = {
            let mut compiler_lock = self
                .jit_compiler
                .write()
                .expect("Failed to acquire JIT compiler lock");
            let compiler = compiler_lock
                .get_or_insert_with(|| JitCompiler::new().expect("Failed to create JIT compiler"));
            compiler.compile(chunk)
        };

        match compile_result {
            Ok(code_ptr) => {
                // Get code size (estimate based on chunk size)
                let code_size = chunk.code().len() * 8; // Rough estimate

                // Store in profile
                unsafe {
                    profile.set_compiled(code_ptr, code_size as u32);
                }

                // Store in cache
                let id = ChunkId::from_chunk(chunk);
                let entry = CacheEntry {
                    native_code: code_ptr,
                    code_size,
                    profile: profile.clone(),
                    tier,
                    last_access: std::time::Instant::now(),
                };
                self.cache.insert(id, entry);

                // Update stats
                if let Ok(mut stats) = self.stats.write() {
                    stats.jit_compilations += 1;
                    stats.total_jit_bytes += code_size as u64;
                }
            }
            Err(_) => {
                profile.set_failed();
                if let Ok(mut stats) = self.stats.write() {
                    stats.jit_failures += 1;
                }
            }
        }
    }

    /// Get cached native code for a chunk
    pub fn get_cached(&self, chunk: &BytecodeChunk) -> Option<*const ()> {
        let id = ChunkId::from_chunk(chunk);
        let result = self.cache.get(&id);

        // Update cache stats
        if let Ok(mut stats) = self.stats.write() {
            if result.is_some() {
                stats.cache_hits += 1;
            } else {
                stats.cache_misses += 1;
            }
        }

        result
    }

    /// Get the native code function pointer from a profile
    pub fn get_native_code(&self, chunk: &BytecodeChunk) -> Option<*const ()> {
        let profile = self.get_or_create_profile(chunk);
        profile.native_code()
    }

    /// Get a copy of the current statistics
    pub fn stats(&self) -> TieredStats {
        self.stats.read().map(|s| s.clone()).unwrap_or_default()
    }

    /// Reset statistics
    pub fn reset_stats(&self) {
        if let Ok(mut stats) = self.stats.write() {
            *stats = TieredStats::new();
        }
    }

    /// Get the JIT cache
    pub fn cache(&self) -> &JitCache {
        &self.cache
    }

    /// Clear all cached compilations
    pub fn clear_cache(&self) {
        self.cache.clear();
    }
}

impl Default for TieredCompiler {
    fn default() -> Self {
        TieredCompiler::new()
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::bytecode::chunk::ChunkBuilder;
    use crate::backend::bytecode::opcodes::Opcode;

    #[test]
    fn test_tier_from_count() {
        assert_eq!(Tier::from_count(0), Tier::Interpreter);
        assert_eq!(Tier::from_count(5), Tier::Interpreter);
        assert_eq!(Tier::from_count(10), Tier::Bytecode);
        assert_eq!(Tier::from_count(50), Tier::Bytecode);

        {
            assert_eq!(Tier::from_count(100), Tier::JitStage1);
            assert_eq!(Tier::from_count(200), Tier::JitStage1);
            assert_eq!(Tier::from_count(500), Tier::JitStage2);
            assert_eq!(Tier::from_count(1000), Tier::JitStage2);
        }
    }

    #[test]
    fn test_tier_threshold() {
        assert_eq!(Tier::Interpreter.threshold(), 0);
        assert_eq!(Tier::Bytecode.threshold(), WARM_THRESHOLD);
        assert_eq!(Tier::JitStage1.threshold(), HOT_THRESHOLD);
        assert_eq!(Tier::JitStage2.threshold(), STAGE2_THRESHOLD);
    }

    #[test]
    fn test_tier_is_jit() {
        assert!(!Tier::Interpreter.is_jit());
        assert!(!Tier::Bytecode.is_jit());
        assert!(Tier::JitStage1.is_jit());
        assert!(Tier::JitStage2.is_jit());
    }

    #[test]
    fn test_chunk_id() {
        let mut builder1 = ChunkBuilder::new("test1");
        builder1.emit(Opcode::PushTrue);
        builder1.emit(Opcode::Halt);
        let chunk1 = builder1.build();

        let mut builder2 = ChunkBuilder::new("test2");
        builder2.emit(Opcode::PushTrue);
        builder2.emit(Opcode::Halt);
        let chunk2 = builder2.build();

        let mut builder3 = ChunkBuilder::new("test3");
        builder3.emit(Opcode::PushFalse);
        builder3.emit(Opcode::Halt);
        let chunk3 = builder3.build();

        let id1 = ChunkId::from_chunk(&chunk1);
        let id2 = ChunkId::from_chunk(&chunk2);
        let id3 = ChunkId::from_chunk(&chunk3);

        // Same bytecode should have same ID
        assert_eq!(id1, id2);

        // Different bytecode should have different ID
        assert_ne!(id1, id3);
    }

    #[test]
    fn test_jit_cache_basic() {
        let cache = JitCache::new();
        assert!(cache.is_empty());

        let id = ChunkId::from_raw(12345);
        let entry = CacheEntry {
            native_code: std::ptr::null(),
            code_size: 100,
            profile: Arc::new(JitProfile::new()),
            tier: Tier::JitStage1,
            last_access: std::time::Instant::now(),
        };

        cache.insert(id, entry);
        assert!(!cache.is_empty());
        assert_eq!(cache.len(), 1);
        assert!(cache.contains(&id));

        let ptr = cache.get(&id);
        assert!(ptr.is_some());
    }

    #[test]
    fn test_jit_cache_eviction() {
        let cache = JitCache::with_limits(2, 1024 * 1024);

        // Insert 3 entries
        for i in 0..3 {
            let id = ChunkId::from_raw(i);
            let entry = CacheEntry {
                native_code: std::ptr::null(),
                code_size: 100,
                profile: Arc::new(JitProfile::new()),
                tier: Tier::JitStage1,
                last_access: std::time::Instant::now(),
            };
            cache.insert(id, entry);
        }

        // Should have evicted one
        assert!(cache.len() <= 2);
    }

    #[test]
    fn test_tiered_stats() {
        let mut stats = TieredStats::new();
        assert_eq!(stats.total_executions(), 0);

        stats.record_execution(Tier::Interpreter);
        stats.record_execution(Tier::Bytecode);
        stats.record_execution(Tier::JitStage1);
        stats.record_execution(Tier::JitStage2);

        assert_eq!(stats.interpreter_runs, 1);
        assert_eq!(stats.bytecode_runs, 1);
        assert_eq!(stats.jit_stage1_runs, 1);
        assert_eq!(stats.jit_stage2_runs, 1);
        assert_eq!(stats.total_executions(), 4);
        assert_eq!(stats.jit_percentage(), 50.0);
    }

    #[test]
    fn test_tiered_compiler_profile() {
        let compiler = TieredCompiler::new();

        let mut builder = ChunkBuilder::new("test");
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::Halt);
        let chunk = builder.build();

        let profile1 = compiler.get_or_create_profile(&chunk);
        let profile2 = compiler.get_or_create_profile(&chunk);

        // Should return the same profile
        assert!(Arc::ptr_eq(&profile1, &profile2));
    }

    #[test]
    fn test_tiered_compiler_tier_progression() {
        let compiler = TieredCompiler::new();

        let mut builder = ChunkBuilder::new("test");
        builder.emit(Opcode::PushTrue);
        builder.emit(Opcode::Halt);
        let chunk = builder.build();

        // Initial tier should be Interpreter
        assert_eq!(compiler.get_tier(&chunk), Tier::Interpreter);

        // Record executions up to warm threshold
        for _ in 0..WARM_THRESHOLD {
            compiler.record_execution(&chunk);
        }

        // Should now be Bytecode tier
        assert_eq!(compiler.get_tier(&chunk), Tier::Bytecode);

        // Record executions up to hot threshold
        for _ in WARM_THRESHOLD..HOT_THRESHOLD {
            compiler.record_execution(&chunk);
        }

        // Should now be JitStage1 tier (or Bytecode if JIT not enabled)

        assert!(
            compiler.get_tier(&chunk) == Tier::JitStage1
                || compiler.get_tier(&chunk) == Tier::Bytecode
        );
    }
}
