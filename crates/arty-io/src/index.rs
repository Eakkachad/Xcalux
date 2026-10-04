//! Change detection and dedup across commits.
//!
//! - [`TileCache`] maps tile pointers to their class (SOLID value or
//!   `raw_crc`) and holds a strong ref to every tile of the last snapshot
//!   written, so a pointer key cannot be reused for other pixels.
//! - [`FileIndex`] says where each blob of a file is: by pointer (only for
//!   pointers the cache holds) and by `raw_crc` (pins nothing).
//! - [`resolve`] decides for each distinct tile of a commit whether it is
//!   already in the destination, the same as another tile of the commit,
//!   copyable from a source file, or must be encoded. crc32 only proposes
//!   candidates; memcmp decides, so correctness never depends on a hash.

use std::fs::File;
use std::path::PathBuf;
use std::sync::Arc;

use ahash::AHashMap;
use arty_core::tile::new_tile_box;
use arty_core::{TileCoord, TileGrid, TileRef};
use rayon::ThreadPool;
use rayon::prelude::*;
use smallvec::SmallVec;

use crate::codec::{CodecScratch, TileClass, classify, decode_tile};
use crate::format::{Commit, FileIdentity, HEADER_LEN, TILE_BYTES, TileCodec, TileEntry};
use crate::readat::ReadAt;
use crate::reader;

/// The cache and index key of a tile.
#[inline]
pub(crate) fn ptr_of(t: &TileRef) -> usize {
    Arc::as_ptr(t) as usize
}

pub(crate) struct CachedTile {
    tile: TileRef,
    class: TileClass,
}

/// Classes of the tiles of the last snapshot written, keyed by pointer.
///
/// Invariant: the pixels behind a cached pointer cannot change while the
/// cache holds its strong ref. `TileGrid` writes go through
/// `Arc::make_mut`, which copies a tile whose count is 2 or more, so a
/// painted tile always gets a new pointer. A pointer is therefore a valid
/// key exactly while it is cached, and every [`FileIndex::by_ptr`] key must
/// be a cached pointer: evictions are removed from every index.
#[derive(Default)]
pub struct TileCache {
    tiles: AHashMap<usize, CachedTile>,
    /// The other half of the double buffer (keeps its capacity).
    spare: AHashMap<usize, CachedTile>,
}

impl TileCache {
    pub fn new() -> Self {
        Self::default()
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.tiles.len()
    }

    pub(crate) fn class(&self, ptr: usize) -> Option<TileClass> {
        self.tiles.get(&ptr).map(|c| c.class)
    }

    /// Add a tile whose class is already known (seeding after a load).
    pub(crate) fn insert(&mut self, tile: TileRef, class: TileClass) {
        self.tiles.insert(ptr_of(&tile), CachedTile { tile, class });
    }

    /// Make the cache hold exactly `tiles` (the snapshot about to be
    /// written), classifying the ones it does not know in parallel.
    /// Returns how many were classified and the evicted pointers, which the
    /// caller must remove from every index.
    pub(crate) fn update<'a>(&mut self, tiles: impl Iterator<Item = &'a TileRef>, pool: &ThreadPool) -> (u32, Vec<usize>) {
        debug_assert!(self.spare.is_empty());
        let mut misses: Vec<TileRef> = Vec::new();
        for t in tiles {
            let p = ptr_of(t);
            if self.spare.contains_key(&p) {
                continue;
            }
            match self.tiles.remove(&p) {
                Some(hit) => {
                    self.spare.insert(p, hit);
                }
                None => {
                    // A placeholder class, replaced below.
                    self.spare.insert(p, CachedTile { tile: t.clone(), class: TileClass::Solid([0; 4]) });
                    misses.push(t.clone());
                }
            }
        }
        let classes: Vec<TileClass> = pool.install(|| misses.par_iter().map(|t| classify(t)).collect());
        for (t, class) in misses.iter().zip(classes) {
            if let Some(c) = self.spare.get_mut(&ptr_of(t)) {
                c.class = class;
            }
        }
        let evicted = self.tiles.keys().copied().collect();
        self.tiles.clear();
        std::mem::swap(&mut self.tiles, &mut self.spare);
        (misses.len() as u32, evicted)
    }

    /// Drop the tiles nothing else references (`strong_count == 1`: no
    /// other thread holds or can obtain them). Returns their pointers,
    /// which the caller must remove from every index.
    pub(crate) fn trim(&mut self) -> Vec<usize> {
        let gone: Vec<usize> = self.tiles.iter().filter(|(_, c)| Arc::strong_count(&c.tile) == 1).map(|(&p, _)| p).collect();
        for p in &gone {
            self.tiles.remove(p);
        }
        gone
    }
}

/// Where a blob is in a file and what it holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BlobLoc {
    pub offset: u64,
    pub stored_len: u32,
    pub codec: TileCodec,
    pub raw_crc: u32,
    pub stored_crc: u32,
}

impl BlobLoc {
    pub fn of(e: &TileEntry) -> Self {
        Self { offset: e.offset, stored_len: e.stored_len, codec: e.codec, raw_crc: e.raw_crc, stored_crc: e.stored_crc }
    }

    pub fn entry(&self, coord: TileCoord) -> TileEntry {
        TileEntry {
            coord,
            codec: self.codec,
            stored_len: self.stored_len,
            raw_crc: self.raw_crc,
            stored_crc: self.stored_crc,
            offset: self.offset,
        }
    }
}

/// A layer as the destination's last commit stored it.
pub(crate) struct CommittedLayer {
    /// O(1) clone of the grid written; `shares_storage` with the next
    /// snapshot's grid means the layer is unchanged.
    pub grid: TileGrid,
    pub table_offset: u64,
    /// Record length (header and payload).
    pub table_len: u64,
    pub entries: Vec<TileEntry>,
}

/// What is known about one file written or read by this session.
pub(crate) struct FileIndex {
    pub path: PathBuf,
    pub id: FileIdentity,
    /// Blobs of the newest commit, for cached pointers only.
    pub by_ptr: AHashMap<usize, BlobLoc>,
    /// Every blob in the file by `raw_crc` (for an append target, also
    /// those of earlier commits: they stay in the file until it is
    /// rewritten).
    pub by_crc: AHashMap<u32, SmallVec<[BlobLoc; 1]>>,
    /// Layers of the newest commit with tiles (append targets).
    pub layers: AHashMap<u32, CommittedLayer>,
    /// End of the newest complete commit.
    pub valid_end: u64,
    /// Offset and contents of the newest commit.
    pub last_commit: Option<(u64, Commit)>,
    /// Bytes the newest commit references.
    pub live_bytes: u64,
    pub commits_since_compact: u32,
    pub last_manifest_raw: Option<Vec<u8>>,
}

impl FileIndex {
    /// A file holding only its header.
    pub fn new(file_uuid: [u8; 16]) -> Self {
        let valid_end = HEADER_LEN as u64;
        Self {
            path: PathBuf::new(),
            id: FileIdentity { file_uuid, len: valid_end, commit_offset: 0, commit_seq: 0, mtime: None },
            by_ptr: AHashMap::new(),
            by_crc: AHashMap::new(),
            layers: AHashMap::new(),
            valid_end,
            last_commit: None,
            live_bytes: 0,
            commits_since_compact: 0,
            last_manifest_raw: None,
        }
    }

    pub fn forget(&mut self, ptrs: &[usize]) {
        if self.by_ptr.is_empty() {
            return;
        }
        for p in ptrs {
            self.by_ptr.remove(p);
        }
    }

    pub fn add_blob(&mut self, loc: BlobLoc) {
        let same = self.by_crc.entry(loc.raw_crc).or_default();
        if !same.iter().any(|b| b.offset == loc.offset) {
            same.push(loc);
        }
    }

    /// True when `src` still holds this file up to its newest commit: the
    /// same header uuid, at least `valid_end` bytes, and the same commit
    /// record ending there. Bytes past `valid_end` (a torn append) are
    /// allowed.
    pub fn still_matches<R: ReadAt + ?Sized>(&self, src: &R) -> bool {
        let Some((at, commit)) = self.last_commit else { return false };
        let Ok(len) = src.len() else { return false };
        len >= self.valid_end
            && reader::read_header(src, len).is_ok_and(|h| h.file_uuid == self.id.file_uuid)
            && reader::commit_at(src, at, self.valid_end).is_ok_and(|c| c == commit)
    }
}

/// A file blobs may be copied from.
pub(crate) struct Source<'a> {
    pub index: &'a FileIndex,
    pub file: &'a File,
}

/// How a distinct tile of a commit is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Resolution {
    Solid([u16; 4]),
    /// A blob already in the destination file.
    Existing(BlobLoc),
    /// The same bytes as an earlier distinct tile of this commit.
    Same(usize),
    /// The stored bytes of `sources[src]` at `loc`.
    Copy { src: usize, loc: BlobLoc },
    Encode { raw_crc: u32 },
}

/// Decide how each of `uniques` (distinct by pointer) is stored when
/// writing to `dest`. In order, a general tile is:
/// 1. at its pointer's location in `dest` (append targets);
/// 2. the same bytes (memcmp) as an earlier tile of this commit;
/// 3. copied from the first source that has its pointer;
/// 4. found by `raw_crc` in `dest` or a source, confirmed by decoding the
///    candidate and memcmp (the rare path: after a trim or an undo past
///    the cache);
/// 5. encoded.
pub(crate) fn resolve(
    uniques: &[&TileRef],
    cache: &TileCache,
    dest: &FileIndex,
    dest_reader: Option<&(dyn ReadAt + Sync)>,
    sources: &[Source<'_>],
    pool: &ThreadPool,
) -> Vec<Resolution> {
    let mut out = Vec::with_capacity(uniques.len());
    // raw_crc → tiles of this commit that store their own bytes.
    let mut this_commit: AHashMap<u32, SmallVec<[usize; 1]>> = AHashMap::new();
    let mut pending: Vec<(usize, u32)> = Vec::new();
    for (u, &t) in uniques.iter().enumerate() {
        let p = ptr_of(t);
        let class = cache.class(p).unwrap_or_else(|| {
            debug_assert!(false, "tile missing from the cache");
            classify(t)
        });
        let raw_crc = match class {
            TileClass::Solid(v) => {
                out.push(Resolution::Solid(v));
                continue;
            }
            TileClass::General { raw_crc } => raw_crc,
        };
        let same = this_commit.entry(raw_crc).or_default();
        let copy = |(src, s): (usize, &Source<'_>)| s.index.by_ptr.get(&p).map(|&loc| Resolution::Copy { src, loc });
        let r = if let Some(&loc) = dest.by_ptr.get(&p) {
            Resolution::Existing(loc)
        } else if let Some(&first) = same.iter().find(|&&j| **uniques[j] == **t) {
            out.push(Resolution::Same(first));
            continue;
        } else if let Some(r) = sources.iter().enumerate().find_map(copy) {
            r
        } else {
            pending.push((u, raw_crc));
            Resolution::Encode { raw_crc }
        };
        same.push(u);
        out.push(r);
    }

    // Content matches in files, checked in parallel.
    let candidates = |raw_crc: u32| {
        let in_dest = dest_reader
            .into_iter()
            .flat_map(move |r| dest.by_crc.get(&raw_crc).into_iter().flatten().map(move |l| (None, r, *l)));
        let in_sources = sources.iter().enumerate().flat_map(move |(i, s)| {
            s.index.by_crc.get(&raw_crc).into_iter().flatten().map(move |l| (Some(i), s.file as &(dyn ReadAt + Sync), *l))
        });
        in_dest.chain(in_sources)
    };
    pending.retain(|&(_, raw_crc)| candidates(raw_crc).next().is_some());
    if pending.is_empty() {
        return out;
    }
    let init = || (CodecScratch::new(), new_tile_box(), vec![0u8; TILE_BYTES]);
    let found: Vec<Option<Resolution>> = pool.install(|| {
        pending
            .par_iter()
            .map_init(init, |(scratch, tile, buf), &(u, raw_crc)| {
                candidates(raw_crc).find_map(|(src, file, loc)| {
                    let stored = buf.get_mut(..loc.stored_len as usize)?;
                    file.read_exact_at(stored, loc.offset).ok()?;
                    let clamped = decode_tile(&loc.entry(TileCoord::new(0, 0)), stored, tile, scratch).ok()?;
                    (clamped == 0 && **tile == **uniques[u]).then_some(match src {
                        None => Resolution::Existing(loc),
                        Some(src) => Resolution::Copy { src, loc },
                    })
                })
            })
            .collect()
    });
    for (&(u, _), r) in pending.iter().zip(found) {
        if let Some(r) = r {
            out[u] = r;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use arty_core::Document;

    use super::*;

    fn pool() -> ThreadPool {
        rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap()
    }

    fn noise(seed: u16) -> TileRef {
        let mut t = arty_core::tile::new_tile();
        for (i, p) in Arc::get_mut(&mut t).unwrap().as_flattened_mut().iter_mut().enumerate() {
            let v = (i as u16).wrapping_mul(seed) & 0x7FFF;
            *p = [v, v / 2, v / 3, 0x8000];
        }
        t
    }

    fn tiles(doc: &Document) -> Vec<TileRef> {
        doc.active_layer().raster().unwrap().iter().map(|(_, t)| t.clone()).collect()
    }

    /// I-5: a cached tile cannot change under its pointer, and trimming
    /// drops only tiles nobody else holds.
    #[test]
    fn cached_pointers_stay_valid_until_trimmed() {
        let pool = pool();
        let mut doc = Document::new(256, 256, 72);
        let id = doc.active();
        let c0 = TileCoord::new(0, 0);
        let c1 = TileCoord::new(1, 0);
        {
            let (g, _) = doc.paint_target(id).unwrap();
            g.insert(c0, noise(3));
            g.insert(c1, noise(5));
        }
        let mut cache = TileCache::new();
        let snap = doc.snapshot();
        let (classified, evicted) = cache.update(tiles(&snap).iter(), &pool);
        assert_eq!((classified, evicted.len()), (2, 0));
        drop(snap);
        let mut index = FileIndex::new([0; 16]);
        let old = doc.layer(id).unwrap().raster().unwrap().get_ref(c0).unwrap().clone();
        let before = *old;
        let (p0, p1) = (ptr_of(&old), ptr_of(doc.layer(id).unwrap().raster().unwrap().get_ref(c1).unwrap()));
        let loc = BlobLoc { offset: 64, stored_len: 10, codec: TileCodec::Lz4Shuf, raw_crc: 1, stored_crc: 2 };
        index.by_ptr.insert(p0, loc);
        index.by_ptr.insert(p1, BlobLoc { offset: 74, ..loc });
        drop(old);

        // Painting the cached tile copies it: new pointer, old pixels kept.
        let (g, _) = doc.paint_target(id).unwrap();
        g.get_mut_or_create(c0)[0][0] = [1, 2, 3, 4];
        let now = doc.layer(id).unwrap().raster().unwrap().get_ref(c0).unwrap();
        assert_ne!(ptr_of(now), p0);
        assert_eq!(cache.class(p0), Some(classify(&before)));
        assert!(*cache.tiles[&p0].tile == before, "the cached tile is pinned and unchanged");

        // Only the replaced tile is referenced by nothing but the cache.
        let gone = cache.trim();
        assert_eq!(gone, [p0]);
        index.forget(&gone);
        assert!(!index.by_ptr.contains_key(&p0) && index.by_ptr.contains_key(&p1));
        assert_eq!(cache.len(), 1);
        assert!(cache.trim().is_empty());

        // The next snapshot evicts what it no longer holds.
        let (g, _) = doc.paint_target(id).unwrap();
        g.replace(c1, None);
        let (classified, evicted) = cache.update(tiles(&doc).iter(), &pool);
        assert_eq!((classified, evicted), (1, vec![p1]));
    }

    #[test]
    fn resolve_prefers_destination_then_this_commit_then_sources() {
        let pool = pool();
        let (a, b, c) = (noise(3), noise(5), noise(7));
        let a_copy = {
            let mut t = arty_core::tile::new_tile();
            *Arc::get_mut(&mut t).unwrap() = *a;
            t
        };
        let solid = {
            let mut t = arty_core::tile::new_tile();
            Arc::get_mut(&mut t).unwrap().as_flattened_mut().fill([1, 1, 1, 1]);
            t
        };
        let all = [&a, &a_copy, &b, &c, &solid];
        let mut cache = TileCache::new();
        cache.update(all.iter().copied(), &pool);
        let loc = |offset| BlobLoc { offset, stored_len: 10, codec: TileCodec::Lz4Shuf, raw_crc: 0, stored_crc: 0 };
        let mut dest = FileIndex::new([0; 16]);
        dest.by_ptr.insert(ptr_of(&b), loc(100));
        let mut src = FileIndex::new([1; 16]);
        src.by_ptr.insert(ptr_of(&c), loc(200));
        src.by_ptr.insert(ptr_of(&b), loc(300));
        let file = File::open(std::env::current_exe().unwrap()).unwrap();
        let sources = [Source { index: &src, file: &file }];
        let r = resolve(&all, &cache, &dest, None, &sources, &pool);
        assert_eq!(
            r,
            [
                Resolution::Encode { raw_crc: crc32fast::hash(bytemuck::bytes_of(&*a)) },
                Resolution::Same(0),
                Resolution::Existing(loc(100)),
                Resolution::Copy { src: 0, loc: loc(200) },
                Resolution::Solid([1, 1, 1, 1]),
            ]
        );
    }
}
