//! Evaluation cache (ARCHITECTURE.md §5).
//!
//! Every node result is stored under a key that hashes everything the result
//! depends on: node type and version, parameters, seed, grid, world, and the
//! keys of its inputs. Editing a node changes its key and therefore the keys of
//! everything downstream, while upstream keys stay the same, so only the edited
//! branch recomputes. Nothing is ever invalidated explicitly; stale entries just
//! stop being asked for and age out of the LRU.
//!
//! With a spill folder set ([`EvalCache::set_spill`]), results pushed out of
//! memory are written there instead of dropped, up to a disk budget, and read
//! back on the next request. Files are written and read outside the lock, so
//! other jobs never wait on the disk.

use std::collections::HashMap;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::error::{CoreError, Result};
use crate::grid::{ColorGrid, Grid, GridSpec};
use crate::node::{Outputs, Value};
use crate::points::PointSet;
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
    /// Node results reused from the cache (from memory or disk).
    pub hits: u64,
    /// Node results computed.
    pub misses: u64,
    /// Entries dropped to stay within the memory budget (including those
    /// moved to disk).
    pub evictions: u64,
    /// Current number of entries in memory.
    pub entries: usize,
    /// Current size of the cached data in memory, in bytes.
    pub bytes: usize,
    /// Results read back from the spill folder.
    pub disk_hits: u64,
    /// Current number of results in the spill folder, and their size.
    pub disk_entries: usize,
    pub disk_bytes: u64,
}

struct Entry {
    outputs: Outputs,
    bytes: usize,
    last_used: u64,
}

/// Where evicted results go.
struct Spill {
    dir: PathBuf,
    budget: u64,
    /// Size and last use of each spilled result.
    files: HashMap<CacheKey, (u64, u64)>,
}

impl Spill {
    fn path(&self, key: CacheKey) -> PathBuf {
        spill_path(&self.dir, key)
    }
}

fn spill_path(dir: &Path, key: CacheKey) -> PathBuf {
    dir.join(format!("{key:032x}.otc"))
}

struct Inner {
    map: HashMap<CacheKey, Entry>,
    budget: usize,
    tick: u64,
    stats: CacheStats,
    spill: Option<Spill>,
}

/// A size-limited, least-recently-used store of node results. Thread-safe;
/// share one between preview and build jobs.
pub struct EvalCache {
    inner: Mutex<Inner>,
}

impl EvalCache {
    /// A cache holding at most `budget_bytes` of grid data in memory.
    pub fn new(budget_bytes: usize) -> Self {
        Self {
            inner: Mutex::new(Inner {
                map: HashMap::new(),
                budget: budget_bytes,
                tick: 0,
                stats: CacheStats::default(),
                spill: None,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Look up a result (counts a hit if found), in memory or on disk.
    pub fn get(&self, key: CacheKey) -> Option<Outputs> {
        let path = {
            let mut inner = self.lock();
            inner.tick += 1;
            let tick = inner.tick;
            if let Some(e) = inner.map.get_mut(&key) {
                e.last_used = tick;
                let found = e.outputs.clone();
                inner.stats.hits += 1;
                return Some(found);
            }
            // On disk: take it out of the index (it comes back to memory).
            let spill = inner.spill.as_mut()?;
            let (size, _) = spill.files.remove(&key)?;
            let path = spill.path(key);
            inner.stats.disk_bytes -= size;
            path
        };
        let read = read_outputs(&path);
        let _ = std::fs::remove_file(&path);
        let outputs = read.ok()?;
        {
            let mut inner = self.lock();
            inner.stats.hits += 1;
            inner.stats.disk_hits += 1;
        }
        self.store(key, &outputs);
        Some(outputs)
    }

    /// Store a freshly computed result (counts a miss), evicting the least
    /// recently used entries if over budget. Results bigger than the whole
    /// budget are not stored.
    pub fn insert(&self, key: CacheKey, outputs: &Outputs) {
        self.lock().stats.misses += 1;
        self.store(key, outputs);
    }

    fn store(&self, key: CacheKey, outputs: &Outputs) {
        // GPU results count too (without reading them back): they hold device memory.
        let bytes: usize = outputs.values().map(|v| v.bytes()).sum();
        let evicted = {
            let mut inner = self.lock();
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
            inner.evict()
        };
        self.spill(evicted);
    }

    /// Write evicted results to the spill folder (outside the lock), then
    /// list them, dropping the oldest spilled results over the disk budget.
    fn spill(&self, evicted: Vec<(CacheKey, Outputs, u64)>) {
        if evicted.is_empty() {
            return;
        }
        let Some(dir) = self.lock().spill.as_ref().map(|s| s.dir.clone()) else {
            return;
        };
        for (key, outputs, last_used) in evicted {
            // GPU results would need reading back first: let them go.
            if outputs.values().any(|v| matches!(v, Value::Gpu(..))) {
                continue;
            }
            let path = spill_path(&dir, key);
            let size = match write_outputs(&path, &outputs) {
                Ok(size) => size,
                Err(_) => {
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
            };
            let mut inner = self.lock();
            let inner = &mut *inner;
            let Some(spill) = inner.spill.as_mut().filter(|s| s.dir == dir) else {
                let _ = std::fs::remove_file(&path);
                return;
            };
            if size > spill.budget {
                let _ = std::fs::remove_file(&path);
                continue;
            }
            if let Some((old, _)) = spill.files.insert(key, (size, last_used)) {
                inner.stats.disk_bytes -= old;
            }
            inner.stats.disk_bytes += size;
            while inner.stats.disk_bytes > spill.budget {
                let Some(oldest) = spill.files.iter().min_by_key(|(_, (_, t))| *t).map(|(k, _)| *k) else {
                    break;
                };
                if let Some((s, _)) = spill.files.remove(&oldest) {
                    let _ = std::fs::remove_file(spill.path(oldest));
                    inner.stats.disk_bytes -= s;
                }
            }
        }
    }

    pub fn stats(&self) -> CacheStats {
        let inner = self.lock();
        CacheStats {
            entries: inner.map.len(),
            disk_entries: inner.spill.as_ref().map_or(0, |s| s.files.len()),
            ..inner.stats
        }
    }

    /// Change the memory budget (evicts immediately if needed).
    pub fn set_budget(&self, budget_bytes: usize) {
        let evicted = {
            let mut inner = self.lock();
            inner.budget = budget_bytes;
            inner.evict()
        };
        self.spill(evicted);
    }

    /// Spill results pushed out of memory into `dir` (created if needed),
    /// keeping at most `budget_bytes` there; `None` turns spilling off.
    /// Results already spilled elsewhere are deleted.
    pub fn set_spill(&self, dir: Option<PathBuf>, budget_bytes: u64) -> Result<()> {
        if let Some(d) = &dir {
            std::fs::create_dir_all(d)
                .map_err(|e| CoreError::Project(format!("cache folder {}: {e}", d.display())))?;
        }
        let mut inner = self.lock();
        if let (Some(old), Some(new)) = (&mut inner.spill, &dir)
            && old.dir == *new
        {
            old.budget = budget_bytes;
        } else {
            inner.drop_spill();
            if let Some(d) = &dir {
                remove_stale(d);
            }
            inner.spill = dir.map(|dir| Spill {
                dir,
                budget: budget_bytes,
                files: HashMap::new(),
            });
        }
        // Trim to the new budget.
        let inner = &mut *inner;
        if let Some(spill) = inner.spill.as_mut() {
            while inner.stats.disk_bytes > spill.budget {
                let Some(oldest) = spill.files.iter().min_by_key(|(_, (_, t))| *t).map(|(k, _)| *k) else {
                    break;
                };
                if let Some((s, _)) = spill.files.remove(&oldest) {
                    let _ = std::fs::remove_file(spill.path(oldest));
                    inner.stats.disk_bytes -= s;
                }
            }
        }
        Ok(())
    }

    /// Drop everything, in memory and on disk.
    pub fn clear(&self) {
        let mut inner = self.lock();
        inner.map.clear();
        inner.stats.bytes = 0;
        inner.stats.entries = 0;
        if let Some(spill) = inner.spill.as_mut() {
            for key in spill.files.keys() {
                let _ = std::fs::remove_file(spill_path(&spill.dir, *key));
            }
            spill.files.clear();
        }
        inner.stats.disk_bytes = 0;
    }
}

impl Inner {
    /// Drop least recently used entries until within budget. Returns them
    /// (with their last use) for the spill folder, if there is one.
    fn evict(&mut self) -> Vec<(CacheKey, Outputs, u64)> {
        let mut out = Vec::new();
        while self.stats.bytes > self.budget {
            let Some(oldest) = self.map.iter().min_by_key(|(_, e)| e.last_used).map(|(k, _)| *k) else {
                break;
            };
            if let Some(e) = self.map.remove(&oldest) {
                self.stats.bytes -= e.bytes;
                self.stats.evictions += 1;
                if self.spill.is_some() {
                    out.push((oldest, e.outputs, e.last_used));
                }
            }
        }
        self.stats.entries = self.map.len();
        out
    }

    /// Delete every spilled result and stop spilling.
    fn drop_spill(&mut self) {
        if let Some(spill) = self.spill.take() {
            for key in spill.files.keys() {
                let _ = std::fs::remove_file(spill.path(*key));
            }
        }
        self.stats.disk_bytes = 0;
    }
}

impl Drop for EvalCache {
    fn drop(&mut self) {
        self.lock().drop_spill();
    }
}

impl Default for EvalCache {
    /// 1 GiB: a few hundred preview grids, or a handful at 8K.
    fn default() -> Self {
        Self::new(1 << 30)
    }
}

// ---- spill files ------------------------------------------------------------

/// Delete spill files in `dir` untouched for an hour: left by a session
/// that ended without cleaning up. (One still listed by a running session
/// just fails to read back there, and is recomputed.)
fn remove_stale(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let hour = std::time::Duration::from_secs(3600);
    for e in entries.flatten() {
        let path = e.path();
        let old = e
            .metadata()
            .and_then(|m| m.modified())
            .is_ok_and(|t| t.elapsed().is_ok_and(|age| age > hour));
        if old && path.extension().is_some_and(|x| x == "otc") {
            let _ = std::fs::remove_file(path);
        }
    }
}

const MAGIC: &[u8; 8] = b"OTSCACH1";

/// Write `outputs` to `path`; returns the file size.
fn write_outputs(path: &Path, outputs: &Outputs) -> Result<u64> {
    let mut w = BufWriter::new(std::fs::File::create(path)?);
    w.write_all(MAGIC)?;
    put_u64(&mut w, outputs.len() as u64)?;
    for (port, value) in outputs {
        put_bytes(&mut w, port.as_bytes())?;
        let (tag, spec) = match value {
            Value::Heightfield(g) => (0u8, g.spec),
            Value::Mask(g) => (1, g.spec),
            Value::ColorMap(c) => (2, c.spec),
            Value::Points(p) => (3, p.spec),
            Value::Gpu(..) => return Err(CoreError::Project("GPU results aren't spilled".into())),
        };
        w.write_all(&[tag])?;
        put_bytes(&mut w, serde_json::to_string(&spec)?.as_bytes())?;
        match value {
            Value::Points(p) => {
                put_f32s(&mut w, &p.data)?;
                put_u64(&mut w, p.species_index.len() as u64)?;
                for s in &p.species_index {
                    w.write_all(&s.to_le_bytes())?;
                }
                put_u64(&mut w, p.species.len() as u64)?;
                for s in &p.species {
                    put_bytes(&mut w, s.as_bytes())?;
                }
            }
            _ => put_f32s(&mut w, value.samples())?,
        }
    }
    w.flush()?;
    Ok(std::fs::metadata(path)?.len())
}

fn read_outputs(path: &Path) -> Result<Outputs> {
    let bad = || CoreError::Project(format!("{} is not a cache file", path.display()));
    let mut r = BufReader::new(std::fs::File::open(path)?);
    let mut magic = [0u8; 8];
    r.read_exact(&mut magic)?;
    if &magic != MAGIC {
        return Err(bad());
    }
    let mut outputs = Outputs::new();
    for _ in 0..get_u64(&mut r)? {
        let port = String::from_utf8(get_bytes(&mut r)?).map_err(|_| bad())?;
        let mut tag = [0u8];
        r.read_exact(&mut tag)?;
        let spec: GridSpec = serde_json::from_slice(&get_bytes(&mut r)?)?;
        let value = match tag[0] {
            0 => Value::Heightfield(Arc::new(Grid {
                spec,
                data: get_f32s(&mut r)?,
            })),
            1 => Value::Mask(Arc::new(Grid {
                spec,
                data: get_f32s(&mut r)?,
            })),
            2 => Value::ColorMap(Arc::new(ColorGrid {
                spec,
                data: get_f32s(&mut r)?,
            })),
            3 => {
                let data = get_f32s(&mut r)?;
                let n = get_u64(&mut r)? as usize;
                let mut species_index = Vec::with_capacity(n);
                for _ in 0..n {
                    let mut b = [0u8; 2];
                    r.read_exact(&mut b)?;
                    species_index.push(u16::from_le_bytes(b));
                }
                let mut species = Vec::new();
                for _ in 0..get_u64(&mut r)? {
                    species.push(String::from_utf8(get_bytes(&mut r)?).map_err(|_| bad())?);
                }
                Value::Points(Arc::new(PointSet {
                    spec,
                    data,
                    species_index,
                    species,
                }))
            }
            _ => return Err(bad()),
        };
        outputs.insert(port, value);
    }
    Ok(outputs)
}

fn put_u64(w: &mut impl Write, v: u64) -> Result<()> {
    Ok(w.write_all(&v.to_le_bytes())?)
}

fn get_u64(r: &mut impl Read) -> Result<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b)?;
    Ok(u64::from_le_bytes(b))
}

fn put_bytes(w: &mut impl Write, b: &[u8]) -> Result<()> {
    put_u64(w, b.len() as u64)?;
    Ok(w.write_all(b)?)
}

fn get_bytes(r: &mut impl Read) -> Result<Vec<u8>> {
    let n = get_u64(r)? as usize;
    let mut b = vec![0u8; n];
    r.read_exact(&mut b)?;
    Ok(b)
}

fn put_f32s(w: &mut impl Write, data: &[f32]) -> Result<()> {
    put_u64(w, data.len() as u64)?;
    for chunk in data.chunks(1 << 16) {
        let bytes: Vec<u8> = chunk.iter().flat_map(|v| v.to_le_bytes()).collect();
        w.write_all(&bytes)?;
    }
    Ok(())
}

fn get_f32s(r: &mut impl Read) -> Result<Vec<f32>> {
    let n = get_u64(r)? as usize;
    let mut bytes = vec![0u8; n * 4];
    r.read_exact(&mut bytes)?;
    Ok(bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|b| f32::from_le_bytes(*b))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::points::Point;

    fn grid_outputs(n: u32, v: f32) -> Outputs {
        let spec = GridSpec::new(n, n, [0.0, 0.0], [100.0, 100.0]);
        Outputs::from([("out".to_string(), Value::Mask(Arc::new(Grid::filled(spec, v))))])
    }

    #[test]
    fn evicted_results_come_back_from_disk() {
        let dir = std::env::temp_dir().join(format!("ots-spill-{}", std::process::id()));
        // Room for one 64² grid (16 KiB) in memory.
        let cache = EvalCache::new(20_000);
        cache.set_spill(Some(dir.clone()), 1 << 20).unwrap();
        cache.insert(1, &grid_outputs(64, 1.0));
        cache.insert(2, &grid_outputs(64, 2.0)); // pushes 1 to disk
        let s = cache.stats();
        assert_eq!((s.entries, s.disk_entries), (1, 1));
        assert!(spill_path(&dir, 1).exists());

        let back = cache.get(1).expect("read back from disk");
        assert_eq!(back["out"].samples()[0], 1.0);
        let s = cache.stats();
        assert_eq!(s.disk_hits, 1);
        // 1 is back in memory, 2 went to disk in its place.
        assert!(!spill_path(&dir, 1).exists() && spill_path(&dir, 2).exists());
        assert_eq!(cache.get(2).unwrap()["out"].samples()[0], 2.0);

        // Points and colour maps survive the round trip.
        let spec = GridSpec::new(4, 4, [0.0, 0.0], [3.0, 3.0]);
        let mut points = PointSet::new(spec, "pine");
        points.push(Point {
            x_m: 1.0,
            y_m: 2.0,
            z_m: 3.0,
            rotation_deg: 45.0,
            scale: 1.5,
            species: 0,
        });
        let colour = ColorGrid::from_fn_indexed(spec, |i, _, _| [i as f32, 0.5, 0.25, 1.0]);
        let mixed = Outputs::from([
            ("points".to_string(), Value::Points(Arc::new(points.clone()))),
            ("colour".to_string(), Value::ColorMap(Arc::new(colour.clone()))),
        ]);
        let path = dir.join("mixed.otc");
        write_outputs(&path, &mixed).unwrap();
        let read = read_outputs(&path).unwrap();
        assert_eq!(**read["points"].points().unwrap(), points);
        assert_eq!(**read["colour"].color().unwrap(), colour);

        // The disk budget holds: only the newest spilled result stays.
        cache.set_spill(Some(dir.clone()), 20_000).unwrap();
        cache.insert(3, &grid_outputs(64, 3.0));
        cache.insert(4, &grid_outputs(64, 4.0));
        assert!(cache.stats().disk_bytes <= 20_000);
        drop(cache);
        let left: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter(|e| e.as_ref().unwrap().path().extension().is_some_and(|x| x == "otc"))
            .map(|e| e.unwrap().file_name())
            .filter(|n| n != "mixed.otc")
            .collect();
        assert!(left.is_empty(), "spill files left behind: {left:?}");
        let _ = std::fs::remove_file(path);
    }
}
