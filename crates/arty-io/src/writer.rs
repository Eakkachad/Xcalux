//! Writing `.arty` v2 files.
//!
//! [`FileWriter::commit`] appends one commit: Segments of tile blobs, a
//! TileTable per raster layer with tiles, the Manifest, then the 56-byte
//! Commit between two fsync barriers. Within a commit, a tile that appears
//! twice (same `Arc`, or same bytes confirmed by memcmp) is stored once.
//!
//! Unchanged tiles are not encoded again: an append reuses the blobs
//! already in the file, and a rewrite copies them from the files the
//! session knows (see [`crate::index`]). A layer whose grid is unchanged
//! keeps its TileTable.
//!
//! [`Session::save_main`] saves the main file as a full rewrite: a temp
//! file next to the target, verified by re-reading it, then renamed over
//! the target. The target is never deleted or truncated first.
//! [`Session::autosave`] appends to the session's recovery file, and
//! rewrites it (compaction) when it grows past twice its live data.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ahash::{AHashMap, AHashSet};
use arty_core::{Document, Layer, LayerContent, MAX_LAYERS, MAX_TREE_DEPTH, TileCoord, TileGrid, TileRef, TreeError};
use rayon::ThreadPool;
use rayon::prelude::*;

use crate::codec::{BlobCodec, CodecScratch, encode_tile};
use crate::error::IoError;
use crate::format::{
    COMMIT_RECORD_LEN, Commit, Header, LAYER_KIND_FOLDER, LAYER_KIND_RASTER, LF_CLIP, LF_EXPANDED, LF_LOCK_ALPHA,
    LF_LOCKED, LF_VISIBLE, LayerRecord, OPT_RECOVERY_FILE, RECORD_HEADER_LEN, RecordHeader, RecordKind, TileCodec,
    TileEntry, new_file_uuid,
};
use crate::index::{self, BlobLoc, CommittedLayer, FileIndex, Resolution, Source, TileCache};
use crate::limits::{
    MAX_EXTRA_TOTAL, MAX_LEXT_ENTRIES, MAX_LEXT_ENTRY, MAX_LEXT_TOTAL, MAX_MANIFEST_RAW, MAX_NAME_LEN, MAX_SEGMENT_BLOBS,
    MAX_VIEW_BYTES,
};
use crate::manifest::{
    AppSection, DocFields, KNOWN_TAGS, LayerExt, SEC_CRITICAL, SEC_SAFE_TO_COPY, SectionWriter, TAG_DOC, TAG_LAYR,
    TAG_LEXT, TAG_META, TAG_VIEW, lext_body, meta_body, truncate_str,
};
use crate::names::blend_id;
use crate::reader::{self, Loaded};
use crate::recovery::{self, SessionLock, is_sharing_violation};
use crate::sink::{FileSink, Sink};
use crate::{FileKind, Progress, phase, sniff, table};

/// Blob bytes are gathered into appends of about this size.
const STAGING: usize = 4 << 20;
/// Waits between attempts to rename the temp file over the target, which
/// antivirus and sync tools may briefly hold open.
const RENAME_RETRY_MS: [u64; 5] = [100, 200, 400, 800, 1600];

/// Identifies one app instance's editing session (recovery files, `META`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SessionId(pub [u8; 16]);

impl SessionId {
    pub fn random() -> Result<Self, IoError> {
        new_file_uuid().map(Self)
    }

    pub fn hex(&self) -> String {
        self.0.iter().map(|b| format!("{b:02x}")).collect()
    }
}

/// App data saved alongside the document.
#[derive(Debug, Clone, Default)]
pub struct SaveExtras {
    /// `VIEW` bytes (zoom, rotation, …), at most 64 KiB.
    pub view: Option<Vec<u8>>,
    /// Unknown SAFE_TO_COPY sections from the loaded file.
    pub sections: Vec<AppSection>,
    /// `LEXT` entries; those whose layer no longer exists are dropped.
    pub layer_ext: Vec<LayerExt>,
    pub title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Verify {
    Off,
    /// Check every blob's stored CRC and decode the blobs encoded by this
    /// save.
    #[default]
    Fast,
    /// Decode every blob.
    Full,
}

#[derive(Debug, Clone, Default)]
pub struct SaveOptions {
    pub verify: Verify,
    /// Commit time; the clock when `None` (tests fix it for determinism).
    pub now_ms: Option<u64>,
    /// File uuid; random when `None` (tests).
    pub uuid: Option<[u8; 16]>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SaveStats {
    /// Nothing changed since the last commit, so nothing was written.
    pub unchanged: bool,
    /// Tile entries written.
    pub tiles: u32,
    /// Distinct tiles classified.
    pub classified: u32,
    pub encoded: u32,
    pub copied: u32,
    /// Blob tiles stored by reference to a blob written for another tile.
    pub reused: u32,
    pub healed: u32,
    pub tables_written: u32,
    pub bytes_written: u64,
    pub file_len: u64,
    /// Bytes the newest commit references (blobs, tables, manifest, commit).
    pub live_bytes: u64,
    /// Layer names cut to 4096 bytes.
    pub truncated_names: u32,
    pub ms_plan: f32,
    pub ms_encode: f32,
    pub ms_io: f32,
    pub ms_fsync: f32,
    pub ms_verify: f32,
}

/// What a commit writes into `META` besides the app version and title.
#[derive(Debug, Clone, Copy)]
pub struct CommitMeta<'a> {
    pub session: SessionId,
    /// Document revision the commit holds.
    pub rev: u64,
    /// Recovery files: path of the main file.
    pub src: Option<&'a str>,
    /// Recovery files: the state equals the main file just saved.
    pub clean: bool,
}

fn ms_since(t: Instant) -> f32 {
    t.elapsed().as_secs_f32() * 1000.0
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn write_err(source: io::Error) -> IoError {
    IoError::Io { op: "write", source }
}

/// A layer as it will be written, in pre-order.
struct PlannedLayer<'d> {
    layer: &'d Layer,
    parent: u32,
    name: &'d str,
    /// Raster layers only.
    grid: Option<&'d TileGrid>,
}

impl PlannedLayer<'_> {
    fn tile_count(&self) -> usize {
        self.grid.map_or(0, TileGrid::len)
    }
}

/// Walk the tree in pre-order (parent first, siblings bottom → top),
/// truncating long names. Depth and layer count are enforced by core, so
/// failing here means a core invariant broke.
fn plan_layers<'d>(doc: &'d Document, truncated: &mut u32) -> Result<Vec<PlannedLayer<'d>>, IoError> {
    fn walk<'d>(
        doc: &'d Document,
        ids: &[arty_core::LayerId],
        parent: u32,
        depth: usize,
        truncated: &mut u32,
        out: &mut Vec<PlannedLayer<'d>>,
    ) -> Result<(), IoError> {
        if depth > MAX_TREE_DEPTH {
            debug_assert!(false, "core lets the tree grow past MAX_TREE_DEPTH");
            return Err(TreeError::TooDeep.into());
        }
        for &id in ids {
            let layer = doc.layer(id).ok_or(TreeError::MissingLayer(id))?;
            let name = truncate_str(&layer.props.name, MAX_NAME_LEN);
            if name.len() < layer.props.name.len() {
                *truncated += 1;
            }
            out.push(PlannedLayer { layer, parent, name, grid: layer.raster() });
            if let Some(children) = layer.children() {
                walk(doc, children, id.0, depth + 1, truncated, out)?;
            }
        }
        Ok(())
    }
    let mut out = Vec::with_capacity(doc.layer_count());
    walk(doc, doc.root(), 0, 1, truncated, &mut out)?;
    if out.len() > MAX_LAYERS {
        debug_assert!(false, "core lets the document grow past MAX_LAYERS");
        return Err(TreeError::TooManyLayers.into());
    }
    Ok(out)
}

/// Point `cache` at the tiles of `doc`. Returns how many tiles were
/// classified and the evicted pointers, which must leave every index.
fn update_cache(cache: &mut TileCache, doc: &Document, pool: &ThreadPool) -> Result<(u32, Vec<usize>), IoError> {
    let layers = plan_layers(doc, &mut 0)?;
    let tiles = layers.iter().filter_map(|l| l.grid).flat_map(|g| g.iter().map(|(_, t)| t));
    Ok(cache.update(tiles, pool))
}

/// A blob this commit writes: tile `u`, copied or encoded.
struct Job {
    u: usize,
    raw_crc: u32,
    copy: Option<(usize, BlobLoc)>,
}

/// The stored bytes of a [`Job`].
struct Blob {
    codec: TileCodec,
    bytes: Vec<u8>,
    stored_crc: u32,
    encoded: bool,
}

/// Get the bytes of a batch of jobs: copies are read per source in offset
/// order over coalesced runs and checked against `stored_crc`; a copy that
/// fails the check (or cannot be read) is encoded from memory instead
/// (healed). Encodes run in parallel.
fn gather_blobs(
    batch: &[Job],
    uniques: &[&TileRef],
    sources: &[Source<'_>],
    pool: &ThreadPool,
    p: &Progress,
    stats: &mut SaveStats,
) -> Result<Vec<Blob>, IoError> {
    let mut out: Vec<Option<Blob>> = batch.iter().map(|_| None).collect();
    for (si, src) in sources.iter().enumerate() {
        let mut want: Vec<(usize, TileEntry)> = batch
            .iter()
            .enumerate()
            .filter_map(|(k, j)| match j.copy {
                Some((s, loc)) if s == si => Some((k, loc.entry(TileCoord::new(0, 0)))),
                _ => None,
            })
            .collect();
        if want.is_empty() {
            continue;
        }
        want.sort_unstable_by_key(|(_, e)| e.offset);
        let entries: Vec<TileEntry> = want.iter().map(|(_, e)| *e).collect();
        let path = Some(src.index.path.as_path()).filter(|p| !p.as_os_str().is_empty());
        let read = reader::for_each_blob(src.file, path, &entries, pool, p, |e, stored, _| {
            if crc32fast::hash(stored) == e.stored_crc {
                Ok(stored.to_vec())
            } else {
                Err(IoError::corrupt("tile stored crc", e.offset))
            }
        });
        if p.is_cancelled() {
            return Err(IoError::Cancelled);
        }
        for ((k, e), r) in want.into_iter().zip(read) {
            match r {
                Ok(bytes) => {
                    out[k] = Some(Blob { codec: e.codec, bytes, stored_crc: e.stored_crc, encoded: false });
                    stats.copied += 1;
                }
                Err(err) => {
                    log::warn!("copying a tile from {}: {err}; encoding it again", src.index.path.display());
                    stats.healed += 1;
                }
            }
        }
    }
    let todo: Vec<usize> = (0..batch.len()).filter(|&k| out[k].is_none()).collect();
    let encoded: Vec<Blob> = pool.install(|| {
        todo.par_iter()
            .map_init(CodecScratch::new, |s, &k| {
                let j = &batch[k];
                let e = encode_tile(uniques[j.u], j.raw_crc, BlobCodec::default(), s);
                Blob { codec: e.codec, bytes: e.bytes.to_vec(), stored_crc: e.stored_crc, encoded: true }
            })
            .collect()
    });
    p.advance(todo.len() as u64);
    stats.encoded += todo.len() as u32;
    for (k, b) in todo.into_iter().zip(encoded) {
        out[k] = Some(b);
    }
    Ok(out.into_iter().flatten().collect())
}

/// Writes commits to a file in the v2 container. One writer per physical
/// file. A commit stores each tile once: unchanged tiles keep their blobs
/// in this file (appends), are copied from source files, or are encoded.
pub struct FileWriter<S: Sink> {
    sink: S,
    index: FileIndex,
    /// Used by [`FileWriter::commit`]; sessions pass their own.
    cache: TileCache,
    /// Offsets of the blobs the last commit encoded.
    encoded: AHashSet<u64>,
}

impl<S: Sink> FileWriter<S> {
    /// Start a new file in `sink` (emptied first) with a fresh header.
    pub fn create(mut sink: S, optional_flags: u32, file_uuid: [u8; 16]) -> Result<Self, IoError> {
        sink.set_len(0).map_err(write_err)?;
        sink.append(&Header::new(optional_flags, file_uuid).encode()).map_err(write_err)?;
        Ok(Self::resume(sink, FileIndex::new(file_uuid)))
    }

    /// Append to a file described by `index`; `sink` must end at its last
    /// commit.
    pub(crate) fn resume(sink: S, index: FileIndex) -> Self {
        debug_assert_eq!(sink.len(), index.valid_end);
        Self { sink, index, cache: TileCache::new(), encoded: AHashSet::new() }
    }

    pub fn sink(&self) -> &S {
        &self.sink
    }

    pub fn into_sink(self) -> S {
        self.sink
    }

    pub(crate) fn into_parts(self) -> (S, FileIndex) {
        (self.sink, self.index)
    }

    /// Offset of the last commit record.
    pub fn last_commit_offset(&self) -> Option<u64> {
        self.index.last_commit.map(|(at, _)| at)
    }

    /// Append a commit of `doc`, reusing what earlier commits of this
    /// writer stored. On failure the sink is truncated back to the
    /// previous commit (best effort), which stays valid.
    pub fn commit(
        &mut self,
        doc: &Document,
        ex: &SaveExtras,
        meta: &CommitMeta<'_>,
        o: &SaveOptions,
        pool: &ThreadPool,
        p: &Progress,
    ) -> Result<SaveStats, IoError> {
        let t = Instant::now();
        let mut cache = std::mem::take(&mut self.cache);
        let r = update_cache(&mut cache, doc, pool).and_then(|(classified, evicted)| {
            self.index.forget(&evicted);
            let ms = ms_since(t);
            let mut stats = self.commit_from(doc, ex, meta, &cache, &[], o, pool, p)?;
            stats.classified = classified;
            stats.ms_plan += ms;
            Ok(stats)
        });
        self.cache = cache;
        r
    }

    /// [`FileWriter::commit`] with a cache already updated for `doc` and
    /// files to copy unchanged blobs from.
    pub(crate) fn commit_from(
        &mut self,
        doc: &Document,
        ex: &SaveExtras,
        meta: &CommitMeta<'_>,
        cache: &TileCache,
        sources: &[Source<'_>],
        o: &SaveOptions,
        pool: &ThreadPool,
        p: &Progress,
    ) -> Result<SaveStats, IoError> {
        let old_end = self.index.valid_end;
        let r = self.commit_inner(doc, ex, meta, cache, sources, o, pool, p);
        if r.is_err() && self.sink.len() != old_end {
            let _ = self.sink.set_len(old_end);
        }
        p.begin(phase::IDLE, 0);
        r
    }

    fn append(&mut self, b: &[u8]) -> Result<(), IoError> {
        self.sink.append(b).map_err(write_err)
    }

    fn sync(&mut self) -> Result<(), IoError> {
        self.sink.sync().map_err(|source| IoError::Io { op: "flush to disk", source })
    }

    /// Append a record; returns its offset.
    fn write_record(&mut self, kind: RecordKind, payload: &[u8]) -> Result<u64, IoError> {
        let at = self.sink.len();
        self.append(&RecordHeader::for_payload(kind, payload).encode())?;
        self.append(payload)?;
        Ok(at)
    }

    fn commit_inner(
        &mut self,
        doc: &Document,
        ex: &SaveExtras,
        meta: &CommitMeta<'_>,
        cache: &TileCache,
        sources: &[Source<'_>],
        o: &SaveOptions,
        pool: &ThreadPool,
        p: &Progress,
    ) -> Result<SaveStats, IoError> {
        let mut stats = SaveStats::default();
        let start_len = self.sink.len();
        let t = Instant::now();
        p.begin(phase::PLAN, 0);
        let layers = plan_layers(doc, &mut stats.truncated_names)?;

        // A layer whose grid map is the one last committed keeps its table
        // with no per-tile work. Others get their tiles sorted by (ty, tx).
        let mut rebuilt: Vec<Option<Vec<(TileCoord, &TileRef)>>> = Vec::with_capacity(layers.len());
        for l in &layers {
            stats.tiles += l.tile_count() as u32;
            let kept = |g: &TileGrid| self.index.layers.get(&l.layer.id.0).is_some_and(|c| c.grid.shares_storage(g));
            rebuilt.push(l.grid.filter(|g| !g.is_empty() && !kept(g)).map(|g| {
                let mut tiles: Vec<_> = g.iter().collect();
                tiles.sort_unstable_by_key(|(c, _)| (c.y, c.x));
                tiles
            }));
        }

        // Distinct tiles of the rebuilt layers, by pointer.
        let mut unique_of: AHashMap<usize, usize> = AHashMap::new();
        let mut uniques: Vec<&TileRef> = Vec::new();
        for (_, tile) in rebuilt.iter().flatten().flatten() {
            unique_of.entry(index::ptr_of(tile)).or_insert_with(|| {
                uniques.push(tile);
                uniques.len() - 1
            });
        }
        let res = index::resolve(&uniques, cache, &self.index, self.sink.read_back(), sources, pool);
        // New blobs in first-use order, so equal input gives equal bytes
        // whether a blob is copied or encoded.
        let mut jobs: Vec<Job> = Vec::new();
        let mut job_of = vec![usize::MAX; uniques.len()];
        for (u, r) in res.iter().enumerate() {
            let job = match *r {
                Resolution::Copy { src, loc } => Job { u, raw_crc: loc.raw_crc, copy: Some((src, loc)) },
                Resolution::Encode { raw_crc } => Job { u, raw_crc, copy: None },
                _ => continue,
            };
            job_of[u] = jobs.len();
            jobs.push(job);
        }
        stats.ms_plan = ms_since(t);

        // Batches of up to 1024 blobs; each batch is one Segment.
        p.begin(phase::ENCODE, jobs.len() as u64);
        let mut locs: Vec<BlobLoc> = Vec::with_capacity(jobs.len());
        let mut encoded = AHashSet::new();
        let mut staging = Vec::new();
        for batch in jobs.chunks(MAX_SEGMENT_BLOBS) {
            if p.is_cancelled() {
                return Err(IoError::Cancelled);
            }
            let t = Instant::now();
            let blobs = gather_blobs(batch, &uniques, sources, pool, p, &mut stats)?;
            stats.ms_encode += ms_since(t);

            let t = Instant::now();
            let payload_len: u64 = blobs.iter().map(|b| b.bytes.len() as u64).sum();
            let seg_at = self.sink.len();
            let header = RecordHeader { kind: RecordKind::Segment as u8, payload_len, payload_crc: 0 };
            self.append(&header.encode())?;
            let mut offset = seg_at + RECORD_HEADER_LEN as u64;
            for (b, job) in blobs.iter().zip(batch) {
                let stored_len = b.bytes.len() as u32;
                locs.push(BlobLoc { offset, stored_len, codec: b.codec, raw_crc: job.raw_crc, stored_crc: b.stored_crc });
                if b.encoded {
                    encoded.insert(offset);
                }
                offset += b.bytes.len() as u64;
                staging.extend_from_slice(&b.bytes);
                if staging.len() >= STAGING {
                    self.append(&staging)?;
                    staging.clear();
                }
            }
            if !staging.is_empty() {
                self.append(&staging)?;
                staging.clear();
            }
            stats.ms_io += ms_since(t);
        }
        let mut loc_of: Vec<Option<BlobLoc>> = Vec::with_capacity(uniques.len());
        for (u, r) in res.iter().enumerate() {
            let loc = match *r {
                Resolution::Solid(_) => None,
                Resolution::Existing(loc) => Some(loc),
                Resolution::Same(first) => loc_of[first],
                Resolution::Copy { .. } | Resolution::Encode { .. } => Some(locs[job_of[u]]),
            };
            loc_of.push(loc);
        }

        // One table per raster layer with tiles; one whose entries did not
        // change is kept.
        let t = Instant::now();
        let mut table_offsets = vec![0u64; layers.len()];
        let mut tables: Vec<Option<CommittedLayer>> = Vec::with_capacity(layers.len());
        let mut blob_entries = 0usize;
        for ((l, tiles), table_offset) in layers.iter().zip(&rebuilt).zip(&mut table_offsets) {
            let old = self.index.layers.get(&l.layer.id.0);
            let Some(tiles) = tiles else {
                // Unchanged (moved over on success), or no tiles.
                *table_offset = old.filter(|_| l.tile_count() > 0).map_or(0, |c| c.table_offset);
                tables.push(None);
                continue;
            };
            let entries: Vec<TileEntry> = tiles
                .iter()
                .map(|&(coord, tile)| {
                    let u = unique_of[&index::ptr_of(tile)];
                    match (res[u], loc_of[u]) {
                        (Resolution::Solid(v), _) => TileEntry::solid(coord, v),
                        (_, Some(loc)) => loc.entry(coord),
                        (_, None) => unreachable!("a general tile always has a blob"),
                    }
                })
                .collect();
            blob_entries += entries.iter().filter(|e| e.codec.has_blob()).count();
            let grid = l.grid.cloned().unwrap_or_default();
            let committed = match old.filter(|c| c.entries == entries) {
                Some(c) => CommittedLayer { grid, table_offset: c.table_offset, table_len: c.table_len, entries },
                None => {
                    let payload = table::encode(l.layer.id.0, &entries);
                    let at = self.write_record(RecordKind::TileTable, &payload)?;
                    stats.tables_written += 1;
                    CommittedLayer { grid, table_offset: at, table_len: (RECORD_HEADER_LEN + payload.len()) as u64, entries }
                }
            };
            *table_offset = committed.table_offset;
            tables.push(Some(committed));
        }
        stats.reused = (blob_entries - jobs.len()) as u32;

        let raw = build_manifest(doc, &layers, &table_offsets, ex, meta)?;
        if jobs.is_empty() && stats.tables_written == 0 && self.index.last_manifest_raw.as_deref() == Some(raw.as_slice()) {
            stats.unchanged = true;
            stats.file_len = self.sink.len();
            stats.live_bytes = self.index.live_bytes;
            return Ok(stats);
        }
        let payload = crate::manifest::encode_payload(&raw);
        let manifest_offset = self.write_record(RecordKind::Manifest, &payload)?;
        stats.ms_io += ms_since(t);

        // Barrier 1 makes everything the commit points at durable; barrier
        // 2 makes the commit itself durable.
        let t = Instant::now();
        self.sync()?;
        let (prev_commit_offset, commit_seq) = match self.index.last_commit {
            Some((at, c)) => (at, c.commit_seq + 1),
            None => (0, 1),
        };
        let commit = Commit { manifest_offset, prev_commit_offset, commit_seq, unix_ms: o.now_ms.unwrap_or_else(now_ms) };
        let commit_at = self.sink.len();
        self.append(&commit.encode_record())?;
        self.sync()?;
        stats.ms_fsync = ms_since(t);

        // The commit is durable: record what the file now holds.
        let idx = &mut self.index;
        let mut committed = AHashMap::with_capacity(tables.len());
        for (l, table) in layers.iter().zip(tables) {
            let id = l.layer.id.0;
            match table {
                Some(c) => {
                    committed.insert(id, c);
                }
                None if l.tile_count() > 0 => {
                    if let Some(c) = idx.layers.remove(&id) {
                        committed.insert(id, c);
                    }
                }
                None => {}
            }
        }
        idx.layers = committed;
        for (t, loc) in uniques.iter().zip(&loc_of) {
            let p = index::ptr_of(t);
            // Only cached pointers are valid keys.
            if let Some(loc) = loc
                && cache.class(p).is_some()
            {
                idx.by_ptr.insert(p, *loc);
            }
        }
        for loc in &locs {
            idx.add_blob(*loc);
        }
        let mut live_blobs = AHashSet::new();
        let mut live = (RECORD_HEADER_LEN + payload.len() + COMMIT_RECORD_LEN) as u64;
        for c in idx.layers.values() {
            live += c.table_len;
            for e in c.entries.iter().filter(|e| e.codec.has_blob()) {
                if live_blobs.insert(e.offset) {
                    live += u64::from(e.stored_len);
                }
            }
        }
        idx.valid_end = self.sink.len();
        idx.last_commit = Some((commit_at, commit));
        idx.last_manifest_raw = Some(raw);
        idx.live_bytes = live;
        idx.commits_since_compact += 1;
        idx.id.len = idx.valid_end;
        idx.id.commit_offset = commit_at;
        idx.id.commit_seq = commit_seq;
        self.encoded = encoded;
        stats.bytes_written = idx.valid_end - start_len;
        stats.file_len = idx.valid_end;
        stats.live_bytes = live;
        Ok(stats)
    }
}

fn layer_flags(l: &Layer) -> u8 {
    let p = &l.props;
    let mut f = 0;
    for (on, bit) in [(p.visible, LF_VISIBLE), (p.clip, LF_CLIP), (p.lock_alpha, LF_LOCK_ALPHA), (p.locked, LF_LOCKED)] {
        if on {
            f |= bit;
        }
    }
    if let LayerContent::Folder { expanded: true, .. } = l.content {
        f |= LF_EXPANDED;
    }
    f
}

/// The manifest body: DOC, LAYR, META, then VIEW, LEXT and kept sections
/// when present, in that order (so equal input gives equal bytes).
fn build_manifest(
    doc: &Document,
    layers: &[PlannedLayer<'_>],
    table_offsets: &[u64],
    ex: &SaveExtras,
    meta: &CommitMeta<'_>,
) -> Result<Vec<u8>, IoError> {
    let mut w = SectionWriter::default();
    let fields = DocFields {
        width: doc.width(),
        height: doc.height(),
        dpi: doc.dpi(),
        paper: doc.paper(),
        active: doc.active().0,
        next_id: doc.next_layer_id(),
        layer_count: layers.len() as u32,
    };
    w.push(TAG_DOC, SEC_CRITICAL, &fields.encode());

    let mut layr = (layers.len() as u32).to_le_bytes().to_vec();
    for (l, &table_offset) in layers.iter().zip(table_offsets) {
        LayerRecord {
            id: l.layer.id.0,
            parent_id: l.parent,
            kind: if l.layer.is_folder() { LAYER_KIND_FOLDER } else { LAYER_KIND_RASTER },
            flags: layer_flags(l.layer),
            blend: blend_id(l.layer.props.blend),
            opacity_bits: l.layer.props.opacity.to_bits(),
            tile_count: l.tile_count() as u32,
            table_offset,
            name: l.name.as_bytes(),
        }
        .encode_into(&mut layr);
    }
    w.push(TAG_LAYR, SEC_CRITICAL, &layr);

    let app = concat!("ARTY ", env!("CARGO_PKG_VERSION"));
    let (session, rev) = (meta.session.hex(), meta.rev.to_string());
    let mut entries = vec![("app", app)];
    if !ex.title.is_empty() {
        entries.push(("title", ex.title.as_str()));
    }
    entries.extend([("session", session.as_str()), ("rev", rev.as_str())]);
    if let Some(src) = meta.src {
        entries.push(("src", src));
    }
    if meta.clean {
        entries.push(("clean", "1"));
    }
    w.push(TAG_META, 0, &meta_body(&entries));

    // Readers refuse oversized sections, so never write one.
    if let Some(view) = &ex.view {
        if view.len() > MAX_VIEW_BYTES {
            return Err(IoError::limit("VIEW section", view.len() as u64, MAX_VIEW_BYTES as u64));
        }
        w.push(TAG_VIEW, SEC_SAFE_TO_COPY, view);
    }
    let ids: AHashSet<u32> = layers.iter().map(|l| l.layer.id.0).collect();
    let ext: Vec<&LayerExt> = ex.layer_ext.iter().filter(|e| ids.contains(&e.layer)).collect();
    if !ext.is_empty() {
        let total: u64 = ext.iter().map(|e| e.bytes.len() as u64).sum();
        let largest = ext.iter().map(|e| e.bytes.len() as u64).max().unwrap_or(0);
        if ext.len() > MAX_LEXT_ENTRIES as usize || largest > MAX_LEXT_ENTRY || total > MAX_LEXT_TOTAL {
            return Err(IoError::limit("LEXT section", total, MAX_LEXT_TOTAL));
        }
        w.push(TAG_LEXT, SEC_SAFE_TO_COPY, &lext_body(ext.into_iter()));
    }
    let mut seen: AHashSet<[u8; 4]> = KNOWN_TAGS.into_iter().collect();
    let mut extra_total = 0u64;
    for s in &ex.sections {
        if seen.insert(s.tag) {
            extra_total += s.bytes.len() as u64;
            if extra_total > MAX_EXTRA_TOTAL {
                return Err(IoError::limit("unknown sections", extra_total, MAX_EXTRA_TOTAL));
            }
            w.push(s.tag, s.flags, &s.bytes);
        }
    }
    let raw = w.into_raw();
    if raw.len() as u64 > MAX_MANIFEST_RAW {
        return Err(IoError::limit("manifest size", raw.len() as u64, MAX_MANIFEST_RAW));
    }
    Ok(raw)
}

// ----- sessions --------------------------------------------------------------

/// When the recovery file is rewritten instead of appended to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Compaction {
    /// Rewrite once the file is larger than twice its live bytes plus this.
    pub slack: u64,
    /// Rewrite after this many commits since the last rewrite.
    pub max_commits: u32,
}

impl Default for Compaction {
    fn default() -> Self {
        Self { slack: 64 << 20, max_commits: 512 }
    }
}

impl Compaction {
    fn due(&self, f: &FileIndex) -> bool {
        f.valid_end > f.live_bytes.saturating_mul(2).saturating_add(self.slack) || f.commits_since_compact >= self.max_commits
    }
}

/// One open document's file state: the tile cache, the main file it came
/// from or was saved to, and its recovery file.
///
/// The recovery file `{session}.arty` in the recovery folder is
/// self-contained. Autosaves append to it; the first one after opening a
/// file copies every blob from the main file, and an explicit save copies
/// from the recovery file, so neither re-encodes unchanged tiles. While the
/// session has a recovery file it holds `{session}.lock` open, unshared.
pub struct Session {
    id: SessionId,
    recovery_dir: Option<PathBuf>,
    cache: TileCache,
    main: Option<FileIndex>,
    recovery: Option<FileIndex>,
    /// A restored recovery file of an earlier session: a copy source until
    /// this session first saves or autosaves.
    restored: Option<FileIndex>,
    /// The main file of a restored document, which has no index here.
    origin: Option<PathBuf>,
    lock: Option<SessionLock>,
    compaction: Compaction,
}

impl Session {
    pub fn new(id: SessionId, recovery_dir: Option<&Path>) -> Self {
        Self {
            id,
            recovery_dir: recovery_dir.map(Path::to_path_buf),
            cache: TileCache::new(),
            main: None,
            recovery: None,
            restored: None,
            origin: None,
            lock: None,
            compaction: Compaction::default(),
        }
    }

    pub fn id(&self) -> SessionId {
        self.id
    }

    pub fn recovery_dir(&self) -> Option<&Path> {
        self.recovery_dir.as_deref()
    }

    /// The main file, if the document came from or was saved to one.
    pub fn main_path(&self) -> Option<&Path> {
        self.main.as_ref().map(|m| m.path.as_path()).or(self.origin.as_deref())
    }

    /// `{session}.arty` in the recovery folder (it may not exist yet).
    pub fn recovery_path(&self) -> Option<PathBuf> {
        self.recovery_dir.as_ref().map(|d| d.join(format!("{}.arty", self.id.hex())))
    }

    /// `{session}.lock` in the recovery folder.
    pub fn lock_path(&self) -> Option<PathBuf> {
        self.recovery_dir.as_ref().map(|d| d.join(format!("{}.lock", self.id.hex())))
    }

    pub fn set_compaction(&mut self, c: Compaction) {
        self.compaction = c;
    }

    /// Take over a document returned by [`crate::load`] from `path`
    /// (`None` for imports, which must be saved under a new name). The
    /// load's tile classes and blob locations seed this session, so the
    /// next save or autosave copies instead of encoding.
    pub fn adopt(&mut self, loaded: &mut Loaded, path: Option<PathBuf>) {
        self.restored = None;
        self.origin = None;
        let Some((cache, mut index)) = loaded.seed.take() else {
            self.main = None;
            return;
        };
        // Pointers of the old cache are no longer pinned.
        if let Some(r) = &mut self.recovery {
            r.by_ptr.clear();
        }
        self.cache = cache;
        self.main = path.map(|path| {
            index.path = path;
            index
        });
    }

    /// Take over a document restored from the recovery file `from` of an
    /// earlier session, whose main file was `src`. The first save or
    /// autosave copies unchanged blobs from `from`; until the document is
    /// saved, autosaves name `src` as its main file.
    pub fn adopt_restored(&mut self, loaded: &mut Loaded, from: &Path, src: Option<PathBuf>) {
        self.adopt(loaded, Some(from.to_path_buf()));
        self.restored = self.main.take();
        self.origin = src;
    }

    /// Unpin cached tiles nothing else references any more (call
    /// periodically; memory held is then bounded by the live document).
    pub fn trim(&mut self) {
        let gone = self.cache.trim();
        for f in self.main.iter_mut().chain(self.recovery.iter_mut()).chain(self.restored.iter_mut()) {
            f.forget(&gone);
        }
    }

    /// End the session: release the lock, and delete the recovery file
    /// when `discard_recovery` (after a clean save, or the user chose not
    /// to keep it).
    pub fn close(mut self, discard_recovery: bool) -> Result<(), IoError> {
        let mut r = Ok(());
        if discard_recovery && let Some(path) = self.recovery_path() {
            for p in [temp_path(&path)?, path] {
                match fs::remove_file(&p) {
                    Err(e) if e.kind() != io::ErrorKind::NotFound => {
                        r = Err(IoError::Io { op: "delete the recovery file", source: e });
                    }
                    _ => {}
                }
            }
        }
        if let Some(lock) = self.lock.take() {
            lock.release();
        }
        r
    }

    /// Point the cache at `doc` and drop evicted pointers from every index.
    fn update_cache(&mut self, doc: &Document, pool: &ThreadPool) -> Result<(u32, f32), IoError> {
        let t = Instant::now();
        let (classified, evicted) = update_cache(&mut self.cache, doc, pool)?;
        for f in self.main.iter_mut().chain(self.recovery.iter_mut()).chain(self.restored.iter_mut()) {
            f.forget(&evicted);
        }
        Ok((classified, ms_since(t)))
    }

    /// Save `doc` to `path` as a new single-commit file: temp file, fsync,
    /// verify, rename. Unchanged blobs are copied from the main file (if
    /// nobody changed it since) and the recovery file. Fails with
    /// `ExternallyModified` when `path` is this session's main file and it
    /// changed on disk since, unless `overwrite_external`. If only the
    /// rename fails, the saved data is kept and `SavedToTemp` names it.
    /// Afterwards the recovery file gets a commit marked clean.
    pub fn save_main(
        &mut self,
        doc: &Document,
        ex: &SaveExtras,
        path: &Path,
        overwrite_external: bool,
        o: &SaveOptions,
        pool: &ThreadPool,
        p: &Progress,
    ) -> Result<SaveStats, IoError> {
        self.save_main_with(doc, ex, path, overwrite_external, o, pool, p, |s| s, |s| s, false)
    }

    /// [`Session::save_main`] with a simulated crash after `crash_after`
    /// bytes of the temp file (see [`crate::sink::FailAfter`]). A crash
    /// leaves the temp file behind, as a real one would.
    #[cfg(any(test, feature = "fault-injection"))]
    pub fn save_main_crashing(
        &mut self,
        doc: &Document,
        ex: &SaveExtras,
        path: &Path,
        o: &SaveOptions,
        pool: &ThreadPool,
        p: &Progress,
        crash_after: u64,
        lose_unsynced: bool,
    ) -> Result<SaveStats, IoError> {
        use crate::sink::FailAfter;
        let wrap = |s| FailAfter::new(s, crash_after, lose_unsynced);
        self.save_main_with(doc, ex, path, false, o, pool, p, wrap, FailAfter::into_inner, true)
    }

    fn save_main_with<S: Sink>(
        &mut self,
        doc: &Document,
        ex: &SaveExtras,
        path: &Path,
        overwrite_external: bool,
        o: &SaveOptions,
        pool: &ThreadPool,
        p: &Progress,
        wrap: impl FnOnce(FileSink) -> S,
        unwrap: impl FnOnce(S) -> FileSink,
        simulated: bool,
    ) -> Result<SaveStats, IoError> {
        if let Some(main) = &self.main
            && main.path == path
            && reader::current_identity(path) != Some(main.id)
        {
            if !overwrite_external {
                return Err(IoError::ExternallyModified);
            }
            // Never copy from a file someone else wrote.
            self.main = None;
        }
        let tmp = temp_path(path)?;
        let file = open_temp(&tmp)?;
        let uuid = match o.uuid {
            Some(u) => u,
            None => new_file_uuid()?,
        };
        let written = self.update_cache(doc, pool).and_then(|(classified, ms)| {
            let (mut stats, index) = self.write_temp(file, uuid, doc, ex, o, pool, p, wrap, unwrap)?;
            stats.classified = classified;
            stats.ms_plan += ms;
            Ok((stats, index))
        });
        // A simulated crash must leave the temp file, as a real one would.
        let (stats, mut index) = match written {
            Ok(w) => w,
            Err(e) => {
                if !simulated {
                    let _ = fs::remove_file(&tmp);
                }
                return Err(e);
            }
        };
        if sniff_path(path) == Some(FileKind::LegacyV1)
            && let Err(e) = backup_v1(path)
        {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
        replace_file(&tmp, path)?;
        index.path = path.to_path_buf();
        index.id.mtime = fs::metadata(path).ok().and_then(|m| m.modified().ok());
        // Only a copy source from now on.
        index.layers.clear();
        index.last_manifest_raw = None;
        self.main = Some(index);
        self.restored = None;
        self.origin = None;
        self.mark_recovery_clean(doc, ex, path, pool);
        Ok(stats)
    }

    /// Write and verify the temp file; every handle is closed on return.
    fn write_temp<S: Sink>(
        &self,
        file: File,
        uuid: [u8; 16],
        doc: &Document,
        ex: &SaveExtras,
        o: &SaveOptions,
        pool: &ThreadPool,
        p: &Progress,
        wrap: impl FnOnce(FileSink) -> S,
        unwrap: impl FnOnce(S) -> FileSink,
    ) -> Result<(SaveStats, FileIndex), IoError> {
        let main = self.main.as_ref().and_then(open_unchanged);
        let recovery = self.recovery.as_ref().and_then(open_matching);
        let restored = self.restored.as_ref().and_then(open_matching);
        let sources = sources([
            (self.main.as_ref(), main.as_ref()),
            (self.recovery.as_ref(), recovery.as_ref()),
            (self.restored.as_ref(), restored.as_ref()),
        ]);
        let sink = FileSink::new(file).map_err(IoError::io("open temp file"))?;
        let mut w = FileWriter::create(wrap(sink), 0, uuid)?;
        let meta = CommitMeta { session: self.id, rev: doc.revision(), src: None, clean: false };
        let mut stats = w.commit_from(doc, ex, &meta, &self.cache, &sources, o, pool, p)?;
        let encoded = std::mem::take(&mut w.encoded);
        let (sink, index) = w.into_parts();
        let sink = unwrap(sink);
        if o.verify != Verify::Off {
            // Through the write handle: the temp file is opened unshared.
            let t = Instant::now();
            let at = index.id.commit_offset;
            let r = reader::verify_file(sink.file(), at, &encoded, o.verify == Verify::Full, pool, p);
            p.begin(phase::IDLE, 0);
            r?;
            stats.ms_verify = ms_since(t);
        }
        Ok((stats, index))
    }

    /// After a save: a recovery commit saying its state is the main file's,
    /// so a crash from here on does not offer it for recovery. Usually only
    /// a manifest. Failing here does not fail the save.
    fn mark_recovery_clean(&mut self, doc: &Document, ex: &SaveExtras, path: &Path, pool: &ThreadPool) {
        let (Some(rec_path), Some(rec)) = (self.recovery_path(), self.recovery.take()) else { return };
        let Some(file) = open_for_append(&rec_path, &rec) else {
            log::warn!("{} changed on disk; it will be rewritten", rec_path.display());
            return;
        };
        let src = path.to_string_lossy();
        let meta = CommitMeta { session: self.id, rev: doc.revision(), src: Some(&src), clean: true };
        if let Err(e) = self.append_recovery(file, rec, doc, ex, &meta, pool, &Progress::default(), |s| s) {
            log::warn!("marking {} clean: {e}", rec_path.display());
        }
    }

    /// Write the current state to the recovery file: an append (after
    /// dropping any torn tail), or a rewrite when there is no recovery
    /// file yet, it changed on disk, or it is due for compaction. `rev`
    /// is the document revision recorded in `META`.
    pub fn autosave(
        &mut self,
        doc: &Document,
        ex: &SaveExtras,
        rev: u64,
        pool: &ThreadPool,
        p: &Progress,
    ) -> Result<SaveStats, IoError> {
        self.autosave_with(doc, ex, rev, false, pool, p, |s| s, false)
    }

    /// [`Session::autosave`] that always rewrites (compacts) the recovery
    /// file.
    pub fn compact(
        &mut self,
        doc: &Document,
        ex: &SaveExtras,
        rev: u64,
        pool: &ThreadPool,
        p: &Progress,
    ) -> Result<SaveStats, IoError> {
        self.autosave_with(doc, ex, rev, true, pool, p, |s| s, false)
    }

    /// [`Session::autosave`] (or [`Session::compact`] when `compact`) with
    /// a simulated crash after `crash_after` bytes are written to the
    /// recovery file or its replacement.
    #[cfg(any(test, feature = "fault-injection"))]
    pub fn autosave_crashing(
        &mut self,
        doc: &Document,
        ex: &SaveExtras,
        rev: u64,
        compact: bool,
        pool: &ThreadPool,
        p: &Progress,
        crash_after: u64,
        lose_unsynced: bool,
    ) -> Result<SaveStats, IoError> {
        use crate::sink::FailAfter;
        let wrap = |s| FailAfter::new(s, crash_after, lose_unsynced);
        self.autosave_with(doc, ex, rev, compact, pool, p, wrap, true)
    }

    fn autosave_with<S: Sink>(
        &mut self,
        doc: &Document,
        ex: &SaveExtras,
        rev: u64,
        force_compact: bool,
        pool: &ThreadPool,
        p: &Progress,
        wrap: impl FnOnce(FileSink) -> S,
        simulated: bool,
    ) -> Result<SaveStats, IoError> {
        let (Some(dir), Some(path)) = (self.recovery_dir.clone(), self.recovery_path()) else {
            let source = io::Error::new(io::ErrorKind::NotFound, "this session has no recovery folder");
            return Err(IoError::Io { op: "autosave", source });
        };
        if self.lock.is_none() {
            self.lock = Some(recovery::acquire_lock(&dir, &self.id.hex())?);
        }
        let (classified, ms) = self.update_cache(doc, pool)?;
        let src = self.main_path().map(|p| p.to_string_lossy().into_owned());
        let meta = CommitMeta { session: self.id, rev, src: src.as_deref(), clean: false };
        let compact = force_compact || self.recovery.as_ref().is_some_and(|r| self.compaction.due(r));
        let r = match self.recovery.take() {
            Some(rec) if !compact => match open_for_append(&path, &rec) {
                Some(file) => self.append_recovery(file, rec, doc, ex, &meta, pool, p, wrap),
                // Changed under us: start a new file.
                None => self.rewrite_recovery(&path, None, doc, ex, &meta, pool, p, wrap, simulated),
            },
            rec => self.rewrite_recovery(&path, rec, doc, ex, &meta, pool, p, wrap, simulated),
        };
        r.map(|mut stats| {
            stats.classified = classified;
            stats.ms_plan += ms;
            stats
        })
    }

    /// Append a commit to the recovery file `file` described by `rec`.
    /// Afterwards `self.recovery` is the updated index (unchanged on
    /// failure: the previous commit stays the valid end).
    fn append_recovery<S: Sink>(
        &mut self,
        file: File,
        rec: FileIndex,
        doc: &Document,
        ex: &SaveExtras,
        meta: &CommitMeta<'_>,
        pool: &ThreadPool,
        p: &Progress,
        wrap: impl FnOnce(FileSink) -> S,
    ) -> Result<SaveStats, IoError> {
        let mut sink = match FileSink::new(file) {
            Ok(s) => s,
            Err(e) => {
                self.recovery = Some(rec);
                return Err(IoError::Io { op: "open the recovery file", source: e });
            }
        };
        // Drop a torn tail. Safe: the lock makes us the only writer.
        if sink.len() != rec.valid_end
            && let Err(e) = sink.set_len(rec.valid_end)
        {
            self.recovery = Some(rec);
            return Err(write_err(e));
        }
        let main = self.main.as_ref().and_then(open_unchanged);
        let sources = sources([(self.main.as_ref(), main.as_ref())]);
        let mut w = FileWriter::resume(wrap(sink), rec);
        let o = SaveOptions { verify: Verify::Off, now_ms: None, uuid: None };
        let r = w.commit_from(doc, ex, meta, &self.cache, &sources, &o, pool, p);
        let (_, index) = w.into_parts();
        self.recovery = Some(index);
        r
    }

    /// Write the recovery file anew (temp file, fsync, rename), copying
    /// from `old` (compaction) and the main file. The lock file stays held
    /// throughout.
    fn rewrite_recovery<S: Sink>(
        &mut self,
        path: &Path,
        old: Option<FileIndex>,
        doc: &Document,
        ex: &SaveExtras,
        meta: &CommitMeta<'_>,
        pool: &ThreadPool,
        p: &Progress,
        wrap: impl FnOnce(FileSink) -> S,
        simulated: bool,
    ) -> Result<SaveStats, IoError> {
        let tmp = temp_path(path)?;
        let written = (|| {
            let file = open_temp(&tmp)?;
            let sink = FileSink::new(file).map_err(IoError::io("open temp file"))?;
            let old_file = old.as_ref().and_then(open_matching);
            let main = self.main.as_ref().and_then(open_unchanged);
            let restored = self.restored.as_ref().and_then(open_matching);
            let sources = sources([
                (old.as_ref(), old_file.as_ref()),
                (self.main.as_ref(), main.as_ref()),
                (self.restored.as_ref(), restored.as_ref()),
            ]);
            let mut w = FileWriter::create(wrap(sink), OPT_RECOVERY_FILE, new_file_uuid()?)?;
            let o = SaveOptions { verify: Verify::Off, now_ms: None, uuid: None };
            let stats = w.commit_from(doc, ex, meta, &self.cache, &sources, &o, pool, p)?;
            Ok((stats, w.into_parts().1))
        })();
        let replaced = written.and_then(|w| {
            replace_file(&tmp, path).map_err(|e| match e {
                IoError::SavedToTemp(_) => {
                    let _ = fs::remove_file(&tmp);
                    let source = io::Error::new(io::ErrorKind::PermissionDenied, "the recovery file is held open");
                    IoError::Io { op: "replace the recovery file", source }
                }
                e => e,
            })?;
            Ok(w)
        });
        match replaced {
            Ok((stats, mut index)) => {
                index.path = path.to_path_buf();
                index.commits_since_compact = 0;
                self.recovery = Some(index);
                // Everything restored is in the new file now.
                self.restored = None;
                Ok(stats)
            }
            Err(e) => {
                // The old file is untouched and still ours.
                self.recovery = old;
                if !simulated {
                    let _ = fs::remove_file(&tmp);
                }
                Err(e)
            }
        }
    }
}

/// `index`'s file, opened, if nobody changed it since (main files: same
/// uuid, length, tail commit and modification time).
fn open_unchanged(index: &FileIndex) -> Option<File> {
    let file = File::open(&index.path).ok()?;
    (reader::identity_of(&file) == Some(index.id)).then_some(file)
}

/// `index`'s file, opened for reading, if it still holds our last commit
/// (recovery files may have a torn tail).
fn open_matching(index: &FileIndex) -> Option<File> {
    let file = File::open(&index.path).ok()?;
    index.still_matches(&file).then_some(file)
}

/// The recovery file at `path`, opened for appending, if it still holds
/// `index`'s last commit.
fn open_for_append(path: &Path, index: &FileIndex) -> Option<File> {
    let file = OpenOptions::new().read(true).write(true).open(path).ok()?;
    index.still_matches(&file).then_some(file)
}

/// Copy sources from `(index, opened file)` pairs, skipping files that
/// could not be used.
fn sources<'a, const N: usize>(files: [(Option<&'a FileIndex>, Option<&'a File>); N]) -> Vec<Source<'a>> {
    files.into_iter().filter_map(|(index, file)| Some(Source { index: index?, file: file? })).collect()
}

/// `<path>.saving~`
fn temp_path(path: &Path) -> Result<PathBuf, IoError> {
    let name = path.file_name().ok_or_else(|| IoError::Io {
        op: "save",
        source: io::Error::new(io::ErrorKind::InvalidInput, "the path has no file name"),
    })?;
    let mut tmp = OsString::from(name);
    tmp.push(".saving~");
    Ok(path.with_file_name(tmp))
}

/// Create the temp file, unshared on Windows so no one else can open it
/// while it is being written.
fn open_temp(tmp: &Path) -> Result<File, IoError> {
    let mut o = OpenOptions::new();
    o.read(true).write(true).create(true).truncate(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        o.share_mode(0);
    }
    o.open(tmp).map_err(|e| if is_sharing_violation(&e) { IoError::Busy } else { IoError::Io { op: "create temp file", source: e } })
}

fn sniff_path(path: &Path) -> Option<FileKind> {
    let mut head = [0u8; 12];
    let mut f = File::open(path).ok()?;
    let mut n = 0;
    while n < head.len() {
        match f.read(&mut head[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    Some(sniff(&head[..n]))
}

/// Copy a v1 file to `<stem>.v1-backup.arty` (or `-1`, `-2`, … when taken)
/// before it is replaced. A copy, so the original never goes missing.
fn backup_v1(path: &Path) -> Result<(), IoError> {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    for i in 0..1000 {
        let name = if i == 0 { format!("{stem}.v1-backup.arty") } else { format!("{stem}.v1-backup-{i}.arty") };
        let backup = path.with_file_name(name);
        if !backup.exists() {
            return fs::copy(path, &backup).map(|_| ()).map_err(IoError::io("back up the v1 file"));
        }
    }
    Err(IoError::Io { op: "back up the v1 file", source: io::Error::from(io::ErrorKind::AlreadyExists) })
}

/// Errors from antivirus or sync tools holding the target for a moment.
fn is_transient(e: &io::Error) -> bool {
    #[cfg(windows)]
    if matches!(e.raw_os_error(), Some(5 | 32 | 33)) {
        return true;
    }
    e.kind() == io::ErrorKind::PermissionDenied
}

/// Atomically replace `path` with `tmp`, retrying while the target is held.
/// The target is never deleted first; on failure the data stays in `tmp`.
fn replace_file(tmp: &Path, path: &Path) -> Result<(), IoError> {
    let mut waits = RENAME_RETRY_MS.iter();
    loop {
        match fs::rename(tmp, path) {
            Ok(()) => {
                sync_dir(path);
                return Ok(());
            }
            Err(e) if is_transient(&e) => match waits.next() {
                Some(&ms) => std::thread::sleep(Duration::from_millis(ms)),
                None => break,
            },
            Err(e) => {
                log::warn!("rename {} -> {}: {e}", tmp.display(), path.display());
                break;
            }
        }
    }
    Err(IoError::SavedToTemp(tmp.to_path_buf()))
}

/// Make the rename durable (Unix needs the directory synced).
fn sync_dir(path: &Path) {
    #[cfg(unix)]
    if let Some(dir) = path.parent()
        && let Ok(d) = File::open(dir)
    {
        let _ = d.sync_all();
    }
    #[cfg(not(unix))]
    let _ = path;
}
