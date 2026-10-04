//! Loading `.arty` v2 files.
//!
//! 1. Validate the header.
//! 2. Find the newest valid commit: the last 56 bytes, else a forward scan
//!    over record headers, plus a backward scan for the `"ArRc"` magic
//!    when the forward scan stops at damage.
//! 3. Read and parse the manifest, then each layer's TileTable.
//! 4. Check the pixel budget, then decode unique blobs in parallel over
//!    offset-sorted, coalesced runs (one file handle per task).
//! 5. Rebuild the document, sharing one `Arc` per blob and per SOLID value.
//!
//! When the chosen commit is damaged, `fallback_to_previous` follows
//! `prev_commit_offset`, and `salvage` loads it with damaged tiles blank.
//! Every count and length from the file is checked before allocating.
//!
//! A load also seeds the session's tile cache and the file's index from
//! the verified entries, so the first save after opening classifies
//! nothing and copies every unchanged blob.

#![cfg_attr(
    not(test),
    deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::arithmetic_side_effects
    )
)]

use std::collections::hash_map::Entry;
use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use ahash::{AHashMap, AHashSet};
use arty_core::tile::{TILE_PIXELS, new_tile, new_tile_box};
use arty_core::{
    BlendMode, DocParts, Document, Layer, LayerContent, LayerId, LayerProps, MAX_TREE_DEPTH, TileGrid, TilePixels,
    TileRef, TreeError,
};
use rayon::ThreadPool;
use rayon::prelude::*;

use crate::codec::{CodecScratch, TileClass, decode_tile, sanitize_pixel};
use crate::error::{IoError, LoadWarning};
use crate::format::{
    COMMIT_RECORD_LEN, Commit, FileIdentity, HEADER_LEN, Header, LAYER_KIND_FOLDER, LAYER_KIND_RASTER, LF_CLIP,
    LF_EXPANDED, LF_LOCK_ALPHA, LF_LOCKED, LF_VISIBLE, LayerRecord, REC_MAGIC, RECORD_HEADER_LEN, RecordHeader,
    RecordKind, TILE_BYTES, TileCodec, TileEntry, unpack_solid,
};
use crate::index::{BlobLoc, FileIndex, TileCache};
use crate::limits::{LoadLimits, MAX_FALLBACK_COMMITS, MAX_LAYER_COUNT, MAX_MANIFEST_STORED, SCAN_WINDOW};
use crate::manifest::{self, AppSection, LayerExt, ManifestView};
use crate::names::blend_from_id;
use crate::readat::ReadAt;
use crate::{FileKind, Progress, phase, sniff, table};

/// Blobs closer than this are read in one run.
const RUN_GAP: u64 = 64 << 10;
/// Largest run read at once (per decode task).
const RUN_MAX: u64 = 4 << 20;

#[derive(Debug, Clone)]
pub struct LoadOptions {
    pub limits: LoadLimits,
    /// Load a damaged commit with its damaged tiles left blank (lossy).
    pub salvage: bool,
    /// Try up to 16 earlier commits when the newest one is damaged
    /// (recovery files).
    pub fallback_to_previous: bool,
    /// Resolution given to v1 files, which stored none.
    pub legacy_dpi: u32,
}

impl Default for LoadOptions {
    fn default() -> Self {
        Self { limits: LoadLimits::default(), salvage: false, fallback_to_previous: false, legacy_dpi: 350 }
    }
}

/// What the header, commit and manifest say about a file.
#[derive(Debug, Clone, PartialEq)]
pub struct FileInfo {
    pub kind: FileKind,
    pub commit_seq: u64,
    /// When the loaded commit was written (Unix ms).
    pub saved_ms: u64,
    /// The newest commit was found by scanning past a damaged tail.
    pub recovered: bool,
    pub width: u32,
    pub height: u32,
    pub dpi: u32,
    pub layer_count: u32,
    pub meta: Vec<(String, String)>,
    /// `(w, h, RGBA8 straight sRGB)`.
    pub thumb: Option<(u16, u16, Vec<u8>)>,
}

/// A loaded document plus everything the app keeps for re-saving it.
pub struct Loaded {
    pub doc: Document,
    /// Unknown SAFE_TO_COPY sections, re-emitted on save.
    pub extra_sections: Vec<AppSection>,
    pub layer_ext: Vec<LayerExt>,
    pub view: Option<Vec<u8>>,
    pub warnings: Vec<LoadWarning>,
    /// Set when saving over the file would lose data; Save then becomes
    /// Save As.
    pub read_only_reason: Option<String>,
    pub info: FileInfo,
    /// The classes of the loaded tiles and where the file stores them,
    /// taken over by `Session::adopt`.
    pub(crate) seed: Option<(TileCache, FileIndex)>,
}

/// Open and load the file at `path`.
pub fn load(path: &Path, o: &LoadOptions, pool: &ThreadPool, p: &Progress) -> Result<Loaded, IoError> {
    let file = File::open(path).map_err(IoError::io("open"))?;
    let mtime = file.metadata().ok().and_then(|m| m.modified().ok());
    let mut loaded = load_from(&file, Some(path), o, pool, p)?;
    if let Some((_, index)) = loaded.seed.as_mut() {
        index.path = path.to_path_buf();
        index.id.mtime = mtime;
    }
    Ok(loaded)
}

/// Load from any positional source. When `path` is given, decode tasks
/// open their own handles to it (it must be the file `src` reads).
pub fn load_from<R: ReadAt + Sync + ?Sized>(
    src: &R,
    path: Option<&Path>,
    o: &LoadOptions,
    pool: &ThreadPool,
    p: &Progress,
) -> Result<Loaded, IoError> {
    let len = src.len().map_err(IoError::io("read"))?;
    let header = read_header(src, len)?;
    p.begin(phase::READ, 0);
    let newest = locate_commit(src, len)?;
    let mut warnings = Vec::new();
    if newest.recovered {
        warnings.push(LoadWarning::RecoveredTornTail);
    }
    let (mut asm, used) = match load_commit(src, path, &newest, o, false, pool, p) {
        Ok(asm) => (asm, newest),
        Err(e) if recoverable(&e) => {
            let fallback = if o.fallback_to_previous { fall_back(src, path, len, &newest, o, pool, p)? } else { None };
            match fallback {
                Some((asm, used)) => {
                    warnings.push(LoadWarning::FellBackToCommit { seq: used.commit.commit_seq });
                    (asm, used)
                }
                None if o.salvage => (load_commit(src, path, &newest, o, true, pool, p)?, newest),
                None => return Err(e),
            }
        }
        Err(e) => return Err(e),
    };
    p.begin(phase::IDLE, 0);
    warnings.extend(asm.warnings);
    let read_only_reason = (!asm.lossy.is_empty()).then(|| format!("Saving would lose data: {}.", asm.lossy.join("; ")));
    let info = FileInfo {
        kind: FileKind::V2 { minor: header.minor },
        commit_seq: used.commit.commit_seq,
        saved_ms: used.commit.unix_ms,
        recovered: newest.recovered,
        width: asm.doc.width(),
        height: asm.doc.height(),
        dpi: asm.doc.dpi(),
        layer_count: asm.layer_count,
        meta: asm.meta,
        thumb: asm.thumb,
    };
    // The identity is the newest valid commit, even after a fallback: that
    // is what the file still looks like when it is saved over.
    let index = &mut asm.seed.1;
    index.id = FileIdentity {
        file_uuid: header.file_uuid,
        len,
        commit_offset: newest.at,
        commit_seq: newest.commit.commit_seq,
        mtime: None,
    };
    index.valid_end = newest.at.saturating_add(COMMIT_RECORD_LEN as u64);
    index.last_commit = Some((newest.at, newest.commit));
    Ok(Loaded {
        doc: asm.doc,
        extra_sections: asm.extras,
        layer_ext: asm.layer_ext,
        view: asm.view,
        warnings,
        read_only_reason,
        info,
        seed: Some(asm.seed),
    })
}

/// Header, newest commit and manifest of a file, without its pixels or
/// tables (recovery prompt, recent files).
pub fn read_info(path: &Path) -> Result<FileInfo, IoError> {
    let file = File::open(path).map_err(IoError::io("open"))?;
    let len = file.len().map_err(IoError::io("read"))?;
    let header = read_header(&file, len)?;
    let located = locate_commit(&file, len)?;
    let payload = read_manifest(&file, &located.commit)?;
    let raw = manifest::decode_payload(&payload, located.commit.manifest_offset)?;
    let m = manifest::parse(&raw, located.commit.manifest_offset, MAX_LAYER_COUNT)?;
    Ok(FileInfo {
        kind: FileKind::V2 { minor: header.minor },
        commit_seq: located.commit.commit_seq,
        saved_ms: located.commit.unix_ms,
        recovered: located.recovered,
        width: m.doc.width,
        height: m.doc.height,
        dpi: m.doc.dpi,
        layer_count: m.doc.layer_count,
        meta: m.meta,
        thumb: m.thumb.map(|(w, h, px)| (w, h, px.to_vec())),
    })
}

/// The identity of the v2 file at `path` as it is now, or `None` when it
/// is missing or not a readable v2 file.
pub(crate) fn current_identity(path: &Path) -> Option<FileIdentity> {
    identity_of(&File::open(path).ok()?)
}

/// The identity of the open v2 file `file`, or `None` when it is not one.
pub(crate) fn identity_of(file: &File) -> Option<FileIdentity> {
    let meta = file.metadata().ok()?;
    let len = meta.len();
    let header = read_header(file, len).ok()?;
    let located = locate_commit(file, len).ok()?;
    Some(FileIdentity {
        file_uuid: header.file_uuid,
        len,
        commit_offset: located.at,
        commit_seq: located.commit.commit_seq,
        mtime: meta.modified().ok(),
    })
}

/// Errors that a different commit or salvage may get around.
fn recoverable(e: &IoError) -> bool {
    matches!(e, IoError::Corrupt { .. } | IoError::Io { .. })
}

// ----- reading records -------------------------------------------------------

fn read_at<R: ReadAt + ?Sized>(src: &R, buf: &mut [u8], at: u64) -> Result<(), IoError> {
    src.read_exact_at(buf, at).map_err(|e| match e.kind() {
        std::io::ErrorKind::UnexpectedEof => IoError::corrupt("truncated file", at),
        _ => IoError::Io { op: "read", source: e },
    })
}

pub(crate) fn read_header<R: ReadAt + ?Sized>(src: &R, len: u64) -> Result<Header, IoError> {
    let mut head = [0u8; HEADER_LEN];
    let n = usize::try_from(len).unwrap_or(HEADER_LEN).min(HEADER_LEN);
    let head = head.get_mut(..n).unwrap_or_default();
    read_at(src, head, 0)?;
    match sniff(head) {
        FileKind::V2 { .. } => Header::decode(head),
        // Import of v1 files lands with `legacy.rs`.
        FileKind::LegacyV1 => Err(IoError::UnsupportedFeature { tag: *b"ARv1" }),
        FileKind::Newer { major } => Err(IoError::NewerFormat { major }),
        FileKind::Unknown => Err(IoError::NotArty),
    }
}

/// Read and check the record of `kind` at `at`: its payload must be at
/// most `max_payload` bytes and end by `end_limit`.
fn read_record<R: ReadAt + ?Sized>(
    src: &R,
    at: u64,
    kind: RecordKind,
    max_payload: u64,
    end_limit: u64,
) -> Result<Vec<u8>, IoError> {
    let mut hb = [0u8; RECORD_HEADER_LEN];
    read_at(src, &mut hb, at)?;
    let h = RecordHeader::decode(&hb, at)?;
    if h.kind != kind as u8 {
        return Err(IoError::corrupt("record kind", at));
    }
    if h.payload_len > max_payload {
        return Err(IoError::corrupt("record length", at));
    }
    if h.end(at)? > end_limit {
        return Err(IoError::corrupt("record range", at));
    }
    let mut payload = vec![0u8; h.payload_len as usize];
    read_at(src, &mut payload, at.saturating_add(RECORD_HEADER_LEN as u64))?;
    h.check_payload(&payload, at)?;
    Ok(payload)
}

fn read_manifest<R: ReadAt + ?Sized>(src: &R, c: &Commit) -> Result<Vec<u8>, IoError> {
    read_record(src, c.manifest_offset, RecordKind::Manifest, MAX_MANIFEST_STORED, u64::MAX)
}

// ----- locating the commit ---------------------------------------------------

/// A valid commit and where it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Located {
    pub commit: Commit,
    pub at: u64,
    /// Found by a scan rather than at the very end of the file.
    pub recovered: bool,
}

/// The commit record at `at`, if valid: CRCs, offsets, and a manifest
/// record header of the right kind that ends before the commit.
pub(crate) fn commit_at<R: ReadAt + ?Sized>(src: &R, at: u64, len: u64) -> Result<Commit, IoError> {
    let end = at.checked_add(COMMIT_RECORD_LEN as u64).ok_or(IoError::corrupt("commit offset", at))?;
    if at < HEADER_LEN as u64 || end > len {
        return Err(IoError::corrupt("commit offset", at));
    }
    let mut b = [0u8; COMMIT_RECORD_LEN];
    read_at(src, &mut b, at)?;
    let c = Commit::decode_record(&b, at)?;
    let mut hb = [0u8; RECORD_HEADER_LEN];
    read_at(src, &mut hb, c.manifest_offset)?;
    let h = RecordHeader::decode(&hb, c.manifest_offset)?;
    if h.kind != RecordKind::Manifest as u8 || h.payload_len > MAX_MANIFEST_STORED || h.end(c.manifest_offset)? > at {
        return Err(IoError::corrupt("commit manifest", at));
    }
    Ok(c)
}

/// Keep the candidate with the highest `commit_seq` (later offset on ties).
fn keep_best(best: &mut Option<(Commit, u64)>, c: Commit, at: u64) {
    if best.is_none_or(|(b, b_at)| (c.commit_seq, at) > (b.commit_seq, b_at)) {
        *best = Some((c, at));
    }
}

pub(crate) fn locate_commit<R: ReadAt + ?Sized>(src: &R, len: u64) -> Result<Located, IoError> {
    if let Some(at) = len.checked_sub(COMMIT_RECORD_LEN as u64)
        && let Ok(commit) = commit_at(src, at, len)
    {
        return Ok(Located { commit, at, recovered: false });
    }
    // Forward scan over record headers until EOF or the first damage.
    let mut best = None;
    let mut pos = HEADER_LEN as u64;
    let mut hb = [0u8; RECORD_HEADER_LEN];
    let stopped_early = loop {
        if pos == len {
            break false;
        }
        if read_at(src, &mut hb, pos).is_err() {
            break true;
        }
        let Ok(h) = RecordHeader::decode(&hb, pos) else { break true };
        let Some(end) = h.end(pos).ok().filter(|&end| end <= len) else { break true };
        if h.kind == RecordKind::Commit as u8
            && let Ok(c) = commit_at(src, pos, len)
        {
            keep_best(&mut best, c, pos);
        }
        pos = end;
    };
    if stopped_early {
        backward_scan(src, len, &mut best)?;
    }
    let (commit, at) = best.ok_or(IoError::corrupt("no valid commit", len))?;
    Ok(Located { commit, at, recovered: true })
}

/// Look for `"ArRc"` at every byte, from the end back to the header, and
/// keep the valid commit with the highest sequence number whose manifest
/// checks out. Memory: one window buffer.
fn backward_scan<R: ReadAt + ?Sized>(src: &R, len: u64, best: &mut Option<(Commit, u64)>) -> Result<(), IoError> {
    let floor = HEADER_LEN as u64;
    let mut buf = vec![0u8; SCAN_WINDOW];
    let mut hi = len;
    while hi > floor {
        let lo = hi.saturating_sub(SCAN_WINDOW as u64).max(floor);
        let n = usize::try_from(hi.saturating_sub(lo)).unwrap_or(SCAN_WINDOW).min(SCAN_WINDOW);
        let window = buf.get_mut(..n).unwrap_or_default();
        read_at(src, window, lo)?;
        for (i, w) in window.windows(REC_MAGIC.len()).enumerate().rev() {
            let at = lo.saturating_add(i as u64);
            if w == REC_MAGIC
                && let Ok(c) = commit_at(src, at, len)
                && best.is_none_or(|(b, _)| c.commit_seq > b.commit_seq)
                && read_manifest(src, &c).is_ok()
            {
                keep_best(best, c, at);
            }
        }
        if lo == floor {
            break;
        }
        // Overlap so a magic split across windows is still seen.
        hi = lo.saturating_add((REC_MAGIC.len() as u64).saturating_sub(1));
    }
    Ok(())
}

/// Follow `prev_commit_offset` from `newest` for up to 16 commits and load
/// the first one that is intact.
fn fall_back<R: ReadAt + Sync + ?Sized>(
    src: &R,
    path: Option<&Path>,
    len: u64,
    newest: &Located,
    o: &LoadOptions,
    pool: &ThreadPool,
    p: &Progress,
) -> Result<Option<(Assembled, Located)>, IoError> {
    let mut c = newest.commit;
    for _ in 0..MAX_FALLBACK_COMMITS {
        if c.prev_commit_offset == 0 {
            break;
        }
        let at = c.prev_commit_offset;
        let Ok(prev) = commit_at(src, at, len) else { break };
        c = prev;
        let here = Located { commit: c, at, recovered: newest.recovered };
        match load_commit(src, path, &here, o, false, pool, p) {
            Ok(asm) => return Ok(Some((asm, here))),
            Err(e) if recoverable(&e) => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(None)
}

// ----- tables and blobs ------------------------------------------------------

/// The entries of each layer's table (`None` for layers without tiles), in
/// `LAYR` order.
fn read_tables<R: ReadAt + ?Sized>(
    src: &R,
    m: &ManifestView<'_>,
    manifest_offset: u64,
    limits: &LoadLimits,
) -> Result<Vec<Option<Vec<TileEntry>>>, IoError> {
    let mut total = 0u64;
    let mut used = AHashSet::new();
    let mut out = Vec::with_capacity(m.layers.len());
    for rec in &m.layers {
        let bad = |what| IoError::corrupt(what, rec.table_offset);
        if rec.tile_count == 0 {
            if rec.table_offset != 0 {
                return Err(bad("layer table offset"));
            }
            out.push(None);
            continue;
        }
        if rec.kind == LAYER_KIND_FOLDER {
            return Err(bad("folder with tiles"));
        }
        total = total.saturating_add(rec.tile_count.into());
        if total > limits.max_entries {
            return Err(IoError::limit("tile entries", total, limits.max_entries));
        }
        if rec.table_offset < HEADER_LEN as u64 || !used.insert(rec.table_offset) {
            return Err(bad("layer table offset"));
        }
        let max = table::max_payload_len(rec.tile_count);
        let payload = read_record(src, rec.table_offset, RecordKind::TileTable, max, manifest_offset)?;
        out.push(Some(table::parse(&payload, rec.table_offset, rec.id, rec.tile_count)?));
    }
    Ok(out)
}

/// Unique blobs sorted by offset, after checking that entries sharing an
/// offset agree and that the decoded pixels fit the budget.
fn plan_blobs(tables: &[Option<Vec<TileEntry>>], limits: &LoadLimits) -> Result<Vec<TileEntry>, IoError> {
    let mut by_offset: AHashMap<u64, TileEntry> = AHashMap::new();
    let mut solids = AHashSet::new();
    for e in tables.iter().flatten().flatten() {
        if e.codec == TileCodec::Solid {
            solids.insert(e.offset);
            continue;
        }
        match by_offset.entry(e.offset) {
            Entry::Occupied(o) => {
                let a = o.get();
                if (a.codec, a.stored_len, a.raw_crc, a.stored_crc) != (e.codec, e.stored_len, e.raw_crc, e.stored_crc) {
                    return Err(IoError::corrupt("shared tile blob disagrees", e.offset));
                }
            }
            Entry::Vacant(v) => {
                v.insert(*e);
            }
        }
    }
    let unique = (by_offset.len() as u64).saturating_add(solids.len() as u64);
    let bytes = unique.saturating_mul(TILE_BYTES as u64);
    if bytes > limits.max_decoded_bytes {
        return Err(IoError::limit("decoded pixels", bytes, limits.max_decoded_bytes));
    }
    let mut blobs: Vec<TileEntry> = by_offset.into_values().collect();
    blobs.sort_unstable_by_key(|e| e.offset);
    Ok(blobs)
}

/// Blobs `first..last` of the sorted list, read as one range.
struct Run {
    start: u64,
    end: u64,
    first: usize,
    last: usize,
}

fn coalesce(blobs: &[TileEntry]) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    for (i, e) in blobs.iter().enumerate() {
        let end = e.blob_end().unwrap_or(e.offset);
        let next = i.saturating_add(1);
        match runs.last_mut() {
            // Hostile entries may overlap: the run ends at the furthest end.
            Some(r) if e.offset <= r.end.saturating_add(RUN_GAP) && end.max(r.end).saturating_sub(r.start) <= RUN_MAX => {
                r.end = r.end.max(end);
                r.last = next;
            }
            _ => runs.push(Run { start: e.offset, end, first: i, last: next }),
        }
    }
    runs
}

/// Per-task buffers of [`for_each_blob`].
pub(crate) struct BlobScratch {
    pub codec: CodecScratch,
    pub tile: Box<TilePixels>,
}

/// Run `op` on the stored bytes of every blob, in parallel over coalesced
/// runs, and return the results in blob order. Each task opens its own
/// handle to `path` when given (Windows serializes I/O on one handle).
/// A run that cannot be read fails all of its blobs; a cancelled run
/// returns `Cancelled` for them.
pub(crate) fn for_each_blob<R, T, F>(
    src: &R,
    path: Option<&Path>,
    blobs: &[TileEntry],
    pool: &ThreadPool,
    p: &Progress,
    op: F,
) -> Vec<Result<T, IoError>>
where
    R: ReadAt + Sync + ?Sized,
    T: Send,
    F: Fn(&TileEntry, &[u8], &mut BlobScratch) -> Result<T, IoError> + Sync,
{
    let runs = coalesce(blobs);
    let init = || {
        let own = path.and_then(|path| File::open(path).ok());
        (own, Vec::new(), BlobScratch { codec: CodecScratch::new(), tile: new_tile_box() })
    };
    let per_run: Vec<Vec<Result<T, IoError>>> = pool.install(|| {
        runs.par_iter()
            .map_init(init, |(own, buf, scratch), run| {
                let mine = blobs.get(run.first..run.last).unwrap_or_default();
                if p.is_cancelled() {
                    return mine.iter().map(|_| Err(IoError::Cancelled)).collect();
                }
                buf.resize(usize::try_from(run.end.saturating_sub(run.start)).unwrap_or(0), 0);
                let read = match own {
                    Some(f) => f.read_exact_at(buf, run.start),
                    None => src.read_exact_at(buf, run.start),
                };
                let out = match read {
                    Err(_) => mine.iter().map(|e| Err(IoError::corrupt("unreadable tile data", e.offset))).collect(),
                    Ok(()) => mine
                        .iter()
                        .map(|e| {
                            let from = usize::try_from(e.offset.saturating_sub(run.start)).unwrap_or(usize::MAX);
                            let stored = from
                                .checked_add(e.stored_len as usize)
                                .and_then(|to| buf.get(from..to))
                                .ok_or(IoError::corrupt("tile blob range", e.offset))?;
                            op(e, stored, &mut *scratch)
                        })
                        .collect(),
                };
                p.advance(mine.len() as u64);
                out
            })
            .collect()
    });
    per_run.into_iter().flatten().collect()
}

// ----- building the document -------------------------------------------------

/// One commit's content, before it becomes a `Loaded`.
struct Assembled {
    doc: Document,
    warnings: Vec<LoadWarning>,
    lossy: Vec<String>,
    extras: Vec<AppSection>,
    layer_ext: Vec<LayerExt>,
    view: Option<Vec<u8>>,
    meta: Vec<(String, String)>,
    thumb: Option<(u16, u16, Vec<u8>)>,
    layer_count: u32,
    /// The index has blob locations only; the caller adds the identity.
    seed: (TileCache, FileIndex),
}

/// A layer before its pixels are attached.
struct Node {
    id: LayerId,
    props: LayerProps,
    /// `Some((children, expanded))` for folders.
    folder: Option<(Vec<LayerId>, bool)>,
}

struct Tree {
    nodes: Vec<Node>,
    root: Vec<LayerId>,
    active: LayerId,
    next_id: u32,
}

fn load_commit<R: ReadAt + Sync + ?Sized>(
    src: &R,
    path: Option<&Path>,
    at: &Located,
    o: &LoadOptions,
    salvage: bool,
    pool: &ThreadPool,
    p: &Progress,
) -> Result<Assembled, IoError> {
    let c = &at.commit;
    let payload = read_manifest(src, c)?;
    let raw = manifest::decode_payload(&payload, c.manifest_offset)?;
    let m = manifest::parse(&raw, c.manifest_offset, o.limits.max_layers)?;
    let tables = read_tables(src, &m, c.manifest_offset, &o.limits)?;
    let mut warnings = m.warnings.clone();
    let mut lossy = m.lossy.clone();
    let tree = build_tree(&m, &mut warnings, &mut lossy)?;
    let blobs = plan_blobs(&tables, &o.limits)?;

    p.begin(phase::DECODE, blobs.len() as u64);
    let decoded = for_each_blob(src, path, &blobs, pool, p, |e, stored, s| {
        let mut tile = new_tile();
        let px = Arc::get_mut(&mut tile).ok_or(IoError::corrupt("tile allocation", e.offset))?;
        let clamped = decode_tile(e, stored, px, &mut s.codec)?;
        Ok((tile, clamped))
    });
    if p.is_cancelled() {
        return Err(IoError::Cancelled);
    }
    let mut clamped = 0u64;
    let mut slots: Vec<Option<TileRef>> = Vec::with_capacity(decoded.len());
    let mut cache = TileCache::new();
    let mut index = FileIndex::new([0; 16]);
    for (r, e) in decoded.into_iter().zip(&blobs) {
        match r {
            Ok((tile, n)) => {
                clamped = clamped.saturating_add(n);
                // A clamped tile no longer matches its blob's raw_crc; the
                // first save classifies and encodes it instead.
                if n == 0 {
                    let loc = BlobLoc::of(e);
                    index.by_ptr.insert(crate::index::ptr_of(&tile), loc);
                    index.add_blob(loc);
                    cache.insert(tile.clone(), TileClass::General { raw_crc: e.raw_crc });
                }
                slots.push(Some(tile));
            }
            Err(e) if salvage && recoverable(&e) => slots.push(None),
            Err(e) => return Err(e),
        }
    }

    // Attach pixels: one Arc per blob and per SOLID value, so sharing in
    // the file is sharing in memory.
    let mut solids: AHashMap<u64, TileRef> = AHashMap::new();
    let mut damaged = 0u32;
    let mut grids: Vec<Option<TileGrid>> = Vec::with_capacity(tables.len());
    for entries in &tables {
        let Some(entries) = entries else {
            grids.push(None);
            continue;
        };
        let mut grid = TileGrid::with_capacity(entries.len());
        for e in entries {
            let tile = if e.codec == TileCodec::Solid {
                match solids.entry(e.offset) {
                    Entry::Occupied(o) => Some(o.get().clone()),
                    Entry::Vacant(v) => {
                        let (px, n) = sanitize_pixel(unpack_solid(e.offset));
                        clamped = clamped.saturating_add(n.saturating_mul(TILE_PIXELS as u64));
                        let t = filled(px);
                        cache.insert(t.clone(), TileClass::Solid(px));
                        Some(v.insert(t).clone())
                    }
                }
            } else {
                blobs.binary_search_by_key(&e.offset, |b| b.offset).ok().and_then(|i| slots.get(i)).cloned().flatten()
            };
            match tile {
                Some(t) => {
                    grid.insert(e.coord, t);
                }
                None => damaged = damaged.saturating_add(1),
            }
        }
        grids.push(Some(grid));
    }

    let paper = m.doc.paper.map(|v| {
        let (px, n) = sanitize_pixel(v);
        clamped = clamped.saturating_add(n);
        px
    });
    if clamped > 0 {
        warnings.push(LoadWarning::ClampedPixels { count: clamped });
    }
    if damaged > 0 {
        warnings.push(LoadWarning::DamagedTiles { count: damaged });
        lossy.push(format!("{damaged} damaged tiles were left blank"));
    }

    // Nodes line up with tables; an added missing raster comes last.
    let mut grids = grids.into_iter();
    let layers = tree
        .nodes
        .into_iter()
        .map(|n| {
            let grid = grids.next().flatten();
            let content = match n.folder {
                Some((children, expanded)) => LayerContent::Folder { children, expanded },
                None => LayerContent::Raster(grid.unwrap_or_default()),
            };
            Layer { id: n.id, props: n.props, content }
        })
        .collect();
    let doc = Document::from_parts(DocParts {
        width: m.doc.width,
        height: m.doc.height,
        dpi: m.doc.dpi,
        paper,
        layers,
        root: tree.root,
        active: tree.active,
        next_id: tree.next_id,
    })?;
    Ok(Assembled {
        doc,
        warnings,
        lossy,
        extras: m.extras,
        layer_ext: m.layer_ext,
        view: m.view.map(<[u8]>::to_vec),
        meta: m.meta,
        thumb: m.thumb.map(|(w, h, px)| (w, h, px.to_vec())),
        layer_count: m.doc.layer_count,
        seed: (cache, index),
    })
}

fn filled(v: [u16; 4]) -> TileRef {
    let mut t = new_tile();
    if let Some(px) = Arc::get_mut(&mut t) {
        px.as_flattened_mut().fill(v);
    }
    t
}

/// Check the `LAYR` tree (ids, parents earlier and folders, depth) and turn
/// records into layers, fixing what can be fixed with a warning.
fn build_tree(m: &ManifestView<'_>, warnings: &mut Vec<LoadWarning>, lossy: &mut Vec<String>) -> Result<Tree, IoError> {
    let mut index: AHashMap<u32, (usize, usize)> = AHashMap::with_capacity(m.layers.len());
    let mut nodes: Vec<Node> = Vec::with_capacity(m.layers.len());
    let mut root = Vec::new();
    let mut top_raster = None;
    for rec in &m.layers {
        let id = LayerId(rec.id);
        if rec.id == 0 {
            return Err(TreeError::ZeroId.into());
        }
        if index.contains_key(&rec.id) {
            return Err(TreeError::DuplicateId(id).into());
        }
        let depth = if rec.parent_id == 0 {
            root.push(id);
            1
        } else {
            // Parents come first, so the tree is acyclic by construction.
            let &(pi, pd) = index.get(&rec.parent_id).ok_or(TreeError::Orphan(id))?;
            match nodes.get_mut(pi).and_then(|n| n.folder.as_mut()) {
                Some((children, _)) => children.push(id),
                None => return Err(TreeError::ChildOfRaster(LayerId(rec.parent_id)).into()),
            }
            pd.saturating_add(1)
        };
        if depth > MAX_TREE_DEPTH {
            return Err(TreeError::TooDeep.into());
        }
        index.insert(rec.id, (nodes.len(), depth));
        let folder = (rec.kind == LAYER_KIND_FOLDER).then(|| (Vec::new(), rec.flags & LF_EXPANDED != 0));
        if folder.is_none() {
            // Later records are higher in the stack (pre-order, bottom → top).
            top_raster = Some(id);
        }
        nodes.push(Node { id, props: layer_props(rec, warnings, lossy), folder });
    }

    let max_id = m.layers.iter().map(|r| r.id).max().unwrap_or(0);
    let mut next_id = m.doc.next_id;
    if next_id <= max_id {
        next_id = max_id.checked_add(1).ok_or(TreeError::BadNextId)?;
        warnings.push(LoadWarning::FixedNextId);
    }
    let top_raster = match top_raster {
        Some(id) => id,
        None => {
            let id = LayerId(next_id);
            next_id = next_id.checked_add(1).ok_or(TreeError::BadNextId)?;
            nodes.push(Node { id, props: LayerProps::named(format!("Layer {}", id.0)), folder: None });
            root.push(id);
            warnings.push(LoadWarning::AddedMissingRaster);
            id
        }
    };
    let active = if index.contains_key(&m.doc.active) {
        LayerId(m.doc.active)
    } else {
        warnings.push(LoadWarning::FixedActiveLayer);
        top_raster
    };
    Ok(Tree { nodes, root, active, next_id })
}

fn layer_props(rec: &LayerRecord<'_>, warnings: &mut Vec<LoadWarning>, lossy: &mut Vec<String>) -> LayerProps {
    let name = match std::str::from_utf8(rec.name) {
        Ok(s) => s.to_owned(),
        Err(_) => {
            warnings.push(LoadWarning::LossyName { layer: rec.id });
            String::from_utf8_lossy(rec.name).into_owned()
        }
    };
    // Bits are kept exactly when in range (including -0 and subnormals).
    let raw = f32::from_bits(rec.opacity_bits);
    let opacity = if (0.0..=1.0).contains(&raw) {
        raw
    } else {
        warnings.push(LoadWarning::OpacityFixed { layer: rec.id });
        if raw.is_finite() { raw.clamp(0.0, 1.0) } else { 1.0 }
    };
    let blend = blend_from_id(rec.blend).unwrap_or_else(|| {
        warnings.push(LoadWarning::UnknownBlend { layer: rec.id, id: rec.blend });
        lossy.push(format!("layer {} uses an unknown blend mode", rec.id));
        BlendMode::Normal
    });
    let mut locked = rec.flags & LF_LOCKED != 0;
    if rec.kind != LAYER_KIND_RASTER && rec.kind != LAYER_KIND_FOLDER {
        warnings.push(LoadWarning::UnsupportedLayerKind { layer: rec.id, kind: rec.kind });
        lossy.push(format!("layer {} is of a kind this version cannot edit", rec.id));
        locked = true;
    }
    LayerProps {
        name,
        visible: rec.flags & LF_VISIBLE != 0,
        opacity,
        blend,
        clip: rec.flags & LF_CLIP != 0,
        lock_alpha: rec.flags & LF_LOCK_ALPHA != 0,
        locked,
    }
}

// ----- save verification -----------------------------------------------------

/// Re-read a file just written with its commit at `expect_at` (which must
/// be the tail): header, commit, manifest, tables and every blob's
/// `stored_crc`. Blobs at `decode` offsets (all of them with `full`) are
/// also decoded and checked against `raw_crc`. Builds no document.
pub(crate) fn verify_file<R: ReadAt + Sync + ?Sized>(
    src: &R,
    expect_at: u64,
    decode: &AHashSet<u64>,
    full: bool,
    pool: &ThreadPool,
    p: &Progress,
) -> Result<(), IoError> {
    let len = src.len().map_err(IoError::io("read"))?;
    read_header(src, len)?;
    let at = len.checked_sub(COMMIT_RECORD_LEN as u64).filter(|&at| at == expect_at);
    let at = at.ok_or(IoError::corrupt("written commit is not at the end", expect_at))?;
    let c = commit_at(src, at, len)?;
    let payload = read_manifest(src, &c)?;
    let raw = manifest::decode_payload(&payload, c.manifest_offset)?;
    let m = manifest::parse(&raw, c.manifest_offset, MAX_LAYER_COUNT)?;
    let unlimited = LoadLimits { max_decoded_bytes: u64::MAX, max_entries: u64::MAX, max_layers: MAX_LAYER_COUNT };
    let tables = read_tables(src, &m, c.manifest_offset, &unlimited)?;
    let blobs = plan_blobs(&tables, &unlimited)?;
    p.begin(phase::VERIFY, blobs.len() as u64);
    let results = for_each_blob(src, None, &blobs, pool, p, |e, stored, s| {
        if crc32fast::hash(stored) != e.stored_crc {
            return Err(IoError::corrupt("tile stored crc", e.offset));
        }
        if full || decode.contains(&e.offset) {
            decode_tile(e, stored, &mut s.tile, &mut s.codec)?;
        }
        Ok(())
    });
    results.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::format::RecordKind;

    fn pool() -> ThreadPool {
        rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap()
    }

    #[test]
    fn coalesces_close_blobs_into_runs() {
        let e = |offset: u64, len: u32| TileEntry {
            coord: arty_core::TileCoord::new(0, 0),
            codec: TileCodec::Lz4Shuf,
            stored_len: len,
            raw_crc: 0,
            stored_crc: 0,
            offset,
        };
        let blobs = [e(100, 50), e(150, 50), e(200 + RUN_GAP, 10), e(300 + 2 * RUN_GAP, 10), e(305 + 2 * RUN_GAP, 100)];
        let runs = coalesce(&blobs);
        let spans: Vec<_> = runs.iter().map(|r| (r.start, r.end, r.first, r.last)).collect();
        assert_eq!(spans, [(100, 210 + RUN_GAP, 0, 3), (300 + 2 * RUN_GAP, 405 + 2 * RUN_GAP, 3, 5)]);
        // A run never grows past RUN_MAX.
        let many: Vec<_> = (0..300).map(|i| e(64 + i * 32768, 32767)).collect();
        for r in coalesce(&many) {
            assert!(r.end - r.start <= RUN_MAX);
        }
    }

    #[test]
    fn locate_needs_a_commit() {
        let mut file = Header::new(0, [1; 16]).encode().to_vec();
        assert!(matches!(locate_commit(&file[..], file.len() as u64), Err(IoError::Corrupt { what: "no valid commit", .. })));
        file.extend_from_slice(&RecordHeader::for_payload(RecordKind::Segment, &[]).encode());
        file.extend_from_slice(b"junk");
        let o = LoadOptions::default();
        assert!(load_from(&file[..], None, &o, &pool(), &Progress::default()).is_err());
    }
}
