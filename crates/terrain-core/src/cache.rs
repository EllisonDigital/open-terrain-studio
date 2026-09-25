//! Evaluation cache (ARCHITECTURE.md §5).
//!
//! Every node result is stored under a key that hashes everything the result
//! depends on: node type and version, parameters, seed, grid, world, and the
//! keys of its inputs. Editing a node changes its key and therefore the keys of
//! everything downstream, while upstream keys stay the same, so only the edited
//! branch recomputes. Nothing is ever invalidated explicitly; stale entries just
//! stop being asked for and age out of the LRU.

use std::collections::HashMap;
use std::sync::Mutex;

use crate::node::Outputs;
use crate::seed::{fnv1a64, mix64};

/// A 128-bit cache key.
pub type CacheKey = u128;

/// Hash a canonical description into a cache key. Two independent 64-bit
/// hashes, so accidental collisions are out of the question in practice.
pub fn key_of(text: &str) -> CacheKey {
    let bytes = text.as_bytes();
    let a = fnv1a64(bytes);
    let mut b: u64 = 0x243f_6a88_85a3_08d3 ^ bytes.len() as u64;
    for chunk in bytes.chunks(8) {
        let mut word = [0u8; 8];
        word[..chunk.len()].copy_from_slice(chunk);
        b = mix64(b ^ u64::from_le_bytes(word));
    }
    ((a as u128) << 64) | b as u128
}

/// Counters, for tests and the status bar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    /// Node results reused from the cache.
    pub hits: u64,
    /// Node results computed.
    pub misses: u64,
    /// Entries dropped to stay within the memory budget.
    pub evictions: u64,
    /// Current number of entries.
    pub entries: usize,
    /// Current size of the cached data, in bytes.
    pub bytes: usize,
}

struct Entry {
    outputs: Outputs,
    bytes: usize,
    last_used: u64,
}

struct Inner {
    map: HashMap<CacheKey, Entry>,
    budget: usize,
    tick: u64,
    stats: CacheStats,
}

/// A size-limited, least-recently-used store of node results. Thread-safe;
/// share one between preview and build jobs.
pub struct EvalCache {
    inner: Mutex<Inner>,
}

impl EvalCache {
    /// A cache holding at most `budget_bytes` of grid data.
    pub fn new(budget_bytes: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                map: HashMap::new(),
                budget: budget_bytes,
                tick: 0,
                stats: CacheStats::default(),
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Look up a result (counts a hit if found).
    pub fn get(&self, key: CacheKey) -> Option<Outputs> {
        let mut inner = self.lock();
        inner.tick += 1;
        let tick = inner.tick;
        let found = inner.map.get_mut(&key).map(|e| {
            e.last_used = tick;
            e.outputs.clone()
        });
        if found.is_some() {
            inner.stats.hits += 1;
        }
        found
    }

    /// Store a freshly computed result (counts a miss), evicting the least
    /// recently used entries if over budget. Results bigger than the whole
    /// budget are not stored.
    pub fn insert(&self, key: CacheKey, outputs: &Outputs) {
        let bytes: usize = outputs.values().map(|v| v.grid().data.len() * 4).sum();
        let mut inner = self.lock();
        inner.stats.misses += 1;
        if bytes > inner.budget {
            return;
        }
        inner.tick += 1;
        let tick = inner.tick;
        if let Some(old) = inner.map.insert(
            key,
            Entry {
                outputs: outputs.clone(),
                bytes,
                last_used: tick,
            },
        ) {
            inner.stats.bytes -= old.bytes;
        }
        inner.stats.bytes += bytes;
        inner.evict();
    }

    pub fn stats(&self) -> CacheStats {
        let inner = self.lock();
        CacheStats {
            entries: inner.map.len(),
            ..inner.stats
        }
    }

    /// Change the memory budget (evicts immediately if needed).
    pub fn set_budget(&self, budget_bytes: usize) {
        let mut inner = self.lock();
        inner.budget = budget_bytes;
        inner.evict();
    }

    /// Drop everything.
    pub fn clear(&self) {
        let mut inner = self.lock();
        inner.map.clear();
        inner.stats.bytes = 0;
        inner.stats.entries = 0;
    }
}

impl Inner {
    /// Drop least recently used entries until within budget.
    fn evict(&mut self) {
        while self.stats.bytes > self.budget {
            let Some(oldest) = self.map.iter().min_by_key(|(_, e)| e.last_used).map(|(k, _)| *k) else {
                break;
            };
            if let Some(e) = self.map.remove(&oldest) {
                self.stats.bytes -= e.bytes;
                self.stats.evictions += 1;
            }
        }
        self.stats.entries = self.map.len();
    }
}

impl Default for EvalCache {
    /// 1 GiB: a few hundred preview grids, or a handful at 8K.
    fn default() -> Self {
        Self::new(1 << 30)
    }
}
