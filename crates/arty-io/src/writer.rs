//! Writing `.arty` v2 files.
//!
//! [`FileWriter::commit`] appends one commit: Segments of tile blobs, a
//! TileTable per raster layer with tiles, the Manifest, then the 56-byte
//! Commit between two fsync barriers. Within a commit, a tile that appears
//! twice (same `Arc`, or same bytes confirmed by memcmp) is stored once.
//!
//! [`Session::save_main`] saves the main file as a full rewrite: a temp
//! file next to the target, verified by re-reading it, then renamed over
//! the target. The target is never deleted or truncated first.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ahash::{AHashMap, AHashSet};
use arty_core::{Document, Layer, LayerContent, MAX_LAYERS, MAX_TREE_DEPTH, TileCoord, TileRef, TreeError};
use rayon::ThreadPool;
use rayon::prelude::*;

use crate::codec::{BlobCodec, CodecScratch, TileClass, classify, encode_tile};
use crate::error::IoError;
use crate::format::{
    COMMIT_RECORD_LEN, Commit, FileIdentity, HEADER_LEN, Header, LAYER_KIND_FOLDER, LAYER_KIND_RASTER, LF_CLIP,
    LF_EXPANDED, LF_LOCK_ALPHA, LF_LOCKED, LF_VISIBLE, LayerRecord, RECORD_HEADER_LEN, RecordHeader, RecordKind,
    TileCodec, TileEntry, new_file_uuid,
};
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

/// Where a blob was written.
#[derive(Debug, Clone, Copy)]
struct BlobLoc {
    offset: u64,
    stored_len: u32,
    codec: TileCodec,
    raw_crc: u32,
    stored_crc: u32,
}

/// How a distinct tile is stored.
#[derive(Clone, Copy)]
enum Resolved {
    Solid([u16; 4]),
    /// Index into the commit's blob list.
    Blob(usize),
}

/// A layer as it will be written, in pre-order.
struct PlannedLayer<'d> {
    layer: &'d Layer,
    parent: u32,
    name: &'d str,
    /// Sorted by `(ty, tx)`.
    tiles: Vec<(TileCoord, &'d TileRef)>,
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
            let mut tiles: Vec<_> = layer.raster().map(|g| g.iter().collect()).unwrap_or_default();
            tiles.sort_unstable_by_key(|(c, _)| (c.y, c.x));
            out.push(PlannedLayer { layer, parent, name, tiles });
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

/// Writes commits to a file in the v2 container. One writer per physical
/// file; every commit is self-contained (encode-only) for now.
pub struct FileWriter<S: Sink> {
    sink: S,
    /// End of the last complete commit; a failed commit truncates back.
    valid_end: u64,
    /// Offset and sequence number of the last commit.
    last_commit: Option<(u64, u64)>,
    last_manifest_raw: Option<Vec<u8>>,
    /// Offsets of the blobs the last commit encoded.
    encoded: AHashSet<u64>,
}

impl<S: Sink> FileWriter<S> {
    /// Start a new file in `sink` (emptied first) with a fresh header.
    pub fn create(mut sink: S, optional_flags: u32, file_uuid: [u8; 16]) -> Result<Self, IoError> {
        sink.set_len(0).map_err(write_err)?;
        sink.append(&Header::new(optional_flags, file_uuid).encode()).map_err(write_err)?;
        Ok(Self {
            sink,
            valid_end: HEADER_LEN as u64,
            last_commit: None,
            last_manifest_raw: None,
            encoded: AHashSet::new(),
        })
    }

    pub fn sink(&self) -> &S {
        &self.sink
    }

    pub fn into_sink(self) -> S {
        self.sink
    }

    /// Offset of the last commit record.
    pub fn last_commit_offset(&self) -> Option<u64> {
        self.last_commit.map(|(at, _)| at)
    }

    /// Append a commit of `doc`. On failure the sink is truncated back to
    /// the previous commit (best effort), which stays valid.
    pub fn commit(
        &mut self,
        doc: &Document,
        ex: &SaveExtras,
        meta: &CommitMeta<'_>,
        o: &SaveOptions,
        pool: &ThreadPool,
        p: &Progress,
    ) -> Result<SaveStats, IoError> {
        let old_end = self.valid_end;
        let r = self.commit_inner(doc, ex, meta, o, pool, p);
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
        o: &SaveOptions,
        pool: &ThreadPool,
        p: &Progress,
    ) -> Result<SaveStats, IoError> {
        let mut stats = SaveStats::default();
        let start_len = self.sink.len();
        let t = Instant::now();
        p.begin(phase::PLAN, 0);
        let layers = plan_layers(doc, &mut stats.truncated_names)?;

        // Distinct tiles by pointer, classified in parallel.
        let mut unique_of: AHashMap<usize, usize> = AHashMap::new();
        let mut uniques: Vec<&TileRef> = Vec::new();
        for (_, tile) in layers.iter().flat_map(|l| &l.tiles) {
            unique_of.entry(Arc::as_ptr(tile) as usize).or_insert_with(|| {
                uniques.push(tile);
                uniques.len() - 1
            });
            stats.tiles += 1;
        }
        let classes: Vec<TileClass> = pool.install(|| uniques.par_iter().map(|t| classify(t)).collect());
        stats.classified = uniques.len() as u32;

        // Resolve: equal content is stored once. crc32 only proposes a
        // candidate; memcmp decides.
        let mut jobs: Vec<(usize, u32)> = Vec::new();
        let mut by_crc: AHashMap<u32, Vec<usize>> = AHashMap::new();
        let mut resolved = Vec::with_capacity(uniques.len());
        for (u, class) in classes.iter().enumerate() {
            resolved.push(match *class {
                TileClass::Solid(v) => Resolved::Solid(v),
                TileClass::General { raw_crc } => {
                    let cands = by_crc.entry(raw_crc).or_default();
                    let same = cands.iter().copied().find(|&j| **uniques[jobs[j].0] == **uniques[u]);
                    Resolved::Blob(same.unwrap_or_else(|| {
                        jobs.push((u, raw_crc));
                        cands.push(jobs.len() - 1);
                        jobs.len() - 1
                    }))
                }
            });
        }
        stats.ms_plan = ms_since(t);

        // Encode in batches; each batch is one Segment.
        p.begin(phase::ENCODE, jobs.len() as u64);
        let mut locs: Vec<BlobLoc> = Vec::with_capacity(jobs.len());
        let mut staging = Vec::new();
        for batch in jobs.chunks(MAX_SEGMENT_BLOBS) {
            if p.is_cancelled() {
                return Err(IoError::Cancelled);
            }
            let t = Instant::now();
            let blobs: Vec<(TileCodec, Vec<u8>, u32)> = pool.install(|| {
                batch
                    .par_iter()
                    .map_init(CodecScratch::new, |s, &(u, raw_crc)| {
                        let e = encode_tile(uniques[u], raw_crc, BlobCodec::default(), s);
                        (e.codec, e.bytes.to_vec(), e.stored_crc)
                    })
                    .collect()
            });
            stats.ms_encode += ms_since(t);

            let t = Instant::now();
            let payload_len: u64 = blobs.iter().map(|b| b.1.len() as u64).sum();
            let seg_at = self.sink.len();
            let header = RecordHeader { kind: RecordKind::Segment as u8, payload_len, payload_crc: 0 };
            self.append(&header.encode())?;
            let mut offset = seg_at + RECORD_HEADER_LEN as u64;
            for ((codec, bytes, stored_crc), &(_, raw_crc)) in blobs.iter().zip(batch) {
                locs.push(BlobLoc { offset, stored_len: bytes.len() as u32, codec: *codec, raw_crc, stored_crc: *stored_crc });
                offset += bytes.len() as u64;
                staging.extend_from_slice(bytes);
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
            p.advance(batch.len() as u64);
        }
        stats.encoded = jobs.len() as u32;
        let blob_tiles = layers
            .iter()
            .flat_map(|l| &l.tiles)
            .filter(|(_, t)| matches!(resolved[unique_of[&(Arc::as_ptr(t) as usize)]], Resolved::Blob(_)))
            .count();
        stats.reused = (blob_tiles - jobs.len()) as u32;
        let mut live = locs.iter().map(|b| u64::from(b.stored_len)).sum::<u64>();

        // One table per raster layer with tiles.
        let t = Instant::now();
        let mut table_offsets = vec![0u64; layers.len()];
        for (l, table_offset) in layers.iter().zip(&mut table_offsets) {
            if l.tiles.is_empty() {
                continue;
            }
            let entries: Vec<TileEntry> = l
                .tiles
                .iter()
                .map(|&(coord, tile)| match resolved[unique_of[&(Arc::as_ptr(tile) as usize)]] {
                    Resolved::Solid(v) => TileEntry::solid(coord, v),
                    Resolved::Blob(j) => {
                        let b = locs[j];
                        TileEntry {
                            coord,
                            codec: b.codec,
                            stored_len: b.stored_len,
                            raw_crc: b.raw_crc,
                            stored_crc: b.stored_crc,
                            offset: b.offset,
                        }
                    }
                })
                .collect();
            let payload = table::encode(l.layer.id.0, &entries);
            *table_offset = self.write_record(RecordKind::TileTable, &payload)?;
            live += (RECORD_HEADER_LEN + payload.len()) as u64;
            stats.tables_written += 1;
        }

        let raw = build_manifest(doc, &layers, &table_offsets, ex, meta)?;
        if jobs.is_empty() && stats.tables_written == 0 && self.last_manifest_raw.as_deref() == Some(raw.as_slice()) {
            stats.unchanged = true;
            stats.file_len = self.sink.len();
            return Ok(stats);
        }
        let payload = crate::manifest::encode_payload(&raw);
        let manifest_offset = self.write_record(RecordKind::Manifest, &payload)?;
        live += (RECORD_HEADER_LEN + payload.len() + COMMIT_RECORD_LEN) as u64;
        stats.ms_io += ms_since(t);

        // Barrier 1 makes everything the commit points at durable; barrier
        // 2 makes the commit itself durable.
        let t = Instant::now();
        self.sync()?;
        let (prev_commit_offset, commit_seq) = match self.last_commit {
            Some((at, seq)) => (at, seq + 1),
            None => (0, 1),
        };
        let commit = Commit { manifest_offset, prev_commit_offset, commit_seq, unix_ms: o.now_ms.unwrap_or_else(now_ms) };
        let commit_at = self.sink.len();
        self.append(&commit.encode_record())?;
        self.sync()?;
        stats.ms_fsync = ms_since(t);

        self.valid_end = self.sink.len();
        self.last_commit = Some((commit_at, commit_seq));
        self.last_manifest_raw = Some(raw);
        self.encoded = locs.iter().map(|b| b.offset).collect();
        stats.bytes_written = self.valid_end - start_len;
        stats.file_len = self.valid_end;
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
            tile_count: l.tiles.len() as u32,
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

// ----- main file -------------------------------------------------------------

/// The main file this session loaded or last saved.
#[derive(Debug, Clone)]
struct MainFile {
    path: PathBuf,
    id: FileIdentity,
}

/// One open document's file state: the main file it came from or was
/// saved to, and (later) its recovery file.
pub struct Session {
    id: SessionId,
    recovery_dir: Option<PathBuf>,
    main: Option<MainFile>,
}

impl Session {
    pub fn new(id: SessionId, recovery_dir: Option<&Path>) -> Self {
        Self { id, recovery_dir: recovery_dir.map(Path::to_path_buf), main: None }
    }

    pub fn id(&self) -> SessionId {
        self.id
    }

    pub fn recovery_dir(&self) -> Option<&Path> {
        self.recovery_dir.as_deref()
    }

    /// The main file, if the document came from or was saved to one.
    pub fn main_path(&self) -> Option<&Path> {
        self.main.as_ref().map(|m| m.path.as_path())
    }

    /// Take over a document returned by [`crate::load`] from `path`
    /// (`None` for imports, which must be saved under a new name).
    pub fn adopt(&mut self, loaded: &mut Loaded, path: Option<PathBuf>) {
        let source = loaded.source.take();
        self.main = path.zip(source).map(|(path, id)| MainFile { path, id });
    }

    /// Save `doc` to `path` as a new single-commit file: temp file, fsync,
    /// verify, rename. Fails with `ExternallyModified` when `path` is this
    /// session's main file and it changed on disk since, unless
    /// `overwrite_external`. If only the rename fails, the saved data is
    /// kept and `SavedToTemp` names it.
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
            self.main = None;
        }
        let tmp = temp_path(path)?;
        let file = open_temp(&tmp)?;
        let uuid = match o.uuid {
            Some(u) => u,
            None => new_file_uuid()?,
        };
        let written = self.write_temp(file, uuid, doc, ex, o, pool, p, wrap, unwrap);
        // A simulated crash must leave the temp file, as a real one would.
        let (stats, commit_at, commit_seq) = match written {
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
        let mtime = fs::metadata(path).ok().and_then(|m| m.modified().ok());
        let id = FileIdentity { file_uuid: uuid, len: stats.file_len, commit_offset: commit_at, commit_seq, mtime };
        self.main = Some(MainFile { path: path.to_path_buf(), id });
        Ok(stats)
    }

    /// Write and verify the temp file; the handle is closed on return.
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
    ) -> Result<(SaveStats, u64, u64), IoError> {
        let sink = FileSink::new(file).map_err(IoError::io("open temp file"))?;
        let mut w = FileWriter::create(wrap(sink), 0, uuid)?;
        let meta = CommitMeta { session: self.id, rev: doc.revision(), src: None, clean: false };
        let mut stats = w.commit(doc, ex, &meta, o, pool, p)?;
        let (commit_at, commit_seq) = w.last_commit.unwrap_or_default();
        let encoded = std::mem::take(&mut w.encoded);
        let sink = unwrap(w.into_sink());
        if o.verify != Verify::Off {
            // Through the write handle: the temp file is opened unshared.
            // Positional reads may move its cursor; nothing is appended
            // after this.
            let t = Instant::now();
            let r = reader::verify_file(sink.file(), commit_at, &encoded, o.verify == Verify::Full, pool, p);
            p.begin(phase::IDLE, 0);
            r?;
            stats.ms_verify = ms_since(t);
        }
        Ok((stats, commit_at, commit_seq))
    }
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

#[cfg(windows)]
fn is_sharing_violation(e: &io::Error) -> bool {
    // ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION
    matches!(e.raw_os_error(), Some(32 | 33))
}

#[cfg(not(windows))]
fn is_sharing_violation(_: &io::Error) -> bool {
    false
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
