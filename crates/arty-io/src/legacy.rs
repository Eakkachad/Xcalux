//! Import of v1 files (ARTY 0.1, written by `legacy/src/save.rs`).
//!
//! Layout: `"ARTY"`, `u32 1`, `u64 json_off`, `u64 dir_off`; raw-deflate
//! tiles (32768 bytes of `[y][x][rgba]` LE u16 each); JSON metadata in
//! `[json_off, dir_off)`; then 24-byte directory entries
//! `{u32 layer, i32 tx, i32 ty, u64 offset, u32 csize}` to the end.
//!
//! v1 lists layers top → bottom (`layer_order`, `folder_child_ids`), so
//! both are reversed. Broken trees (cycles, second parents, unlisted
//! layers) are repaired with a warning, vector layers become raster layers
//! built from their saved tiles, and blend modes v2 lacks map to the
//! closest one. Tiles decode in parallel, and equal tiles share one `Arc`
//! (v1 deep-copied duplicated layers). Every length is checked before
//! allocating, as in the v2 reader.

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

use std::path::Path;
use std::sync::Arc;

use ahash::{AHashMap, AHashSet};
use arty_core::tile::new_tile;
use arty_core::{
    BlendMode, DocParts, Document, Layer, LayerContent, LayerId, LayerProps, MAX_TREE_DEPTH, PAPER_WHITE, TileCoord,
    TileGrid, TileRef,
};
use flate2::{Decompress, FlushDecompress, Status};
use rayon::ThreadPool;
use serde::Deserialize;
use smallvec::SmallVec;

use crate::codec::{TileClass, classify, sanitize};
use crate::error::{IoError, LoadWarning};
use crate::format::{ByteReader, TILE_BYTES, TileCodec, TileEntry, coord_in_domain};
use crate::index::{FileIndex, TileCache};
use crate::limits::{MAX_DPI, MAX_LAYER_COUNT, MAX_PAGE_SIDE};
use crate::readat::ReadAt;
use crate::reader::{FileInfo, LoadOptions, Loaded, for_each_blob, read_at};
use crate::{FileKind, Progress, phase};

/// `"ARTY"`, version, `json_off`, `dir_off`.
const V1_HEADER_LEN: u64 = 24;
const DIR_ENTRY_LEN: u64 = 24;
/// JSON metadata size.
pub const MAX_V1_JSON: u64 = 64 << 20;
/// Directory entries.
pub const MAX_V1_ENTRIES: u64 = 4_194_304;
/// Compressed size of one tile.
pub const MAX_V1_TILE: u32 = 1 << 20;
/// Ids above this are renumbered, so fresh ids can never overflow.
const MAX_KEPT_ID: u32 = u32::MAX - (1 << 17);
/// v1 drew folders nested at most this deep; deeper ones were blank.
const V1_FOLDER_DEPTH: usize = 3;

/// The v1 JSON block. Unknown fields (and `vector_strokes`, which is never
/// imported) are skipped by serde without being built.
#[derive(Deserialize, Default)]
#[serde(default)]
struct Meta {
    canvas_width: u32,
    canvas_height: u32,
    /// Top → bottom.
    layer_order: Vec<u32>,
    layers: Vec<MetaLayer>,
}

#[derive(Deserialize)]
#[serde(default)]
struct MetaLayer {
    id: u32,
    name: String,
    /// `null` where v1 saved a NaN.
    opacity: Option<f32>,
    visible: bool,
    lock_alpha: bool,
    is_clipping: bool,
    blend_mode: String,
    kind: String,
    /// Top → bottom.
    folder_child_ids: Vec<u32>,
}

impl Default for MetaLayer {
    fn default() -> Self {
        Self {
            id: 0,
            name: String::new(),
            opacity: Some(1.0),
            visible: true,
            lock_alpha: false,
            is_clipping: false,
            blend_mode: String::new(),
            kind: String::new(),
            folder_child_ids: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Raster,
    Folder,
    Vector,
}

impl Kind {
    /// v1 loaded unknown kinds as raster layers; so do we.
    fn of(s: &str) -> Kind {
        match s {
            "Folder" => Kind::Folder,
            "Vector" => Kind::Vector,
            _ => Kind::Raster,
        }
    }
}

struct DirEntry {
    layer: u32,
    coord: TileCoord,
    offset: u64,
    csize: u32,
}

/// Header offsets, checked: `24 ≤ json_off ≤ dir_off ≤ len`, JSON ≤ 64 MiB.
fn read_offsets<R: ReadAt + ?Sized>(src: &R, len: u64) -> Result<(u64, u64), IoError> {
    if len < V1_HEADER_LEN {
        return Err(IoError::corrupt("truncated v1 header", 0));
    }
    let mut b = [0u8; V1_HEADER_LEN as usize];
    read_at(src, &mut b, 0)?;
    let mut r = ByteReader::new(&b, "truncated v1 header", 0);
    r.skip(8)?;
    let (json_off, dir_off) = (r.u64()?, r.u64()?);
    if !(V1_HEADER_LEN <= json_off && json_off <= dir_off && dir_off <= len) {
        return Err(IoError::corrupt("v1 header offsets", 8));
    }
    let json_len = dir_off.saturating_sub(json_off);
    if json_len > MAX_V1_JSON {
        return Err(IoError::limit("v1 metadata", json_len, MAX_V1_JSON));
    }
    Ok((json_off, dir_off))
}

fn read_meta<R: ReadAt + ?Sized>(src: &R, json_off: u64, dir_off: u64) -> Result<Meta, IoError> {
    let n = usize::try_from(dir_off.saturating_sub(json_off)).map_err(|_| IoError::corrupt("v1 metadata", json_off))?;
    let mut json = vec![0u8; n];
    read_at(src, &mut json, json_off)?;
    // serde_json's recursion limit stays on: nesting cannot blow the stack.
    let meta: Meta = serde_json::from_slice(&json).map_err(|_| IoError::corrupt("v1 metadata", json_off))?;
    let side = 1..=MAX_PAGE_SIDE;
    if !side.contains(&meta.canvas_width) || !side.contains(&meta.canvas_height) {
        return Err(IoError::corrupt("page size", json_off));
    }
    Ok(meta)
}

fn read_dir<R: ReadAt + ?Sized>(
    src: &R,
    dir_off: u64,
    len: u64,
    max_entries: u64,
    warnings: &mut Vec<LoadWarning>,
) -> Result<Vec<DirEntry>, IoError> {
    let bytes = len.saturating_sub(dir_off);
    let n = bytes.checked_div(DIR_ENTRY_LEN).unwrap_or(0);
    if bytes.checked_rem(DIR_ENTRY_LEN).unwrap_or(0) != 0 {
        warnings.push(LoadWarning::LegacyDroppedTiles { count: 1, reason: "incomplete directory entry" });
    }
    let max = MAX_V1_ENTRIES.min(max_entries);
    if n > max {
        return Err(IoError::limit("tile entries", n, max));
    }
    let mut buf = vec![0u8; usize::try_from(n.saturating_mul(DIR_ENTRY_LEN)).unwrap_or(0)];
    read_at(src, &mut buf, dir_off)?;
    let mut r = ByteReader::new(&buf, "truncated v1 directory", dir_off);
    let mut out = Vec::with_capacity(usize::try_from(n).unwrap_or(0));
    while !r.is_empty() {
        let layer = r.u32()?;
        let coord = TileCoord::new(r.i32()?, r.i32()?);
        out.push(DirEntry { layer, coord, offset: r.u64()?, csize: r.u32()? });
    }
    Ok(out)
}

/// The repaired layer tree.
struct Tree {
    /// Per metadata record: its id (kept or fresh) and kind.
    ids: Vec<LayerId>,
    kinds: Vec<Kind>,
    /// Folders' children, bottom → top, as placed by the walk.
    children: Vec<Vec<LayerId>>,
    root: Vec<LayerId>,
    /// First metadata record of each v1 id (what tiles refer to).
    by_old: AHashMap<u32, usize>,
}

/// Places layers depth-first, parent before children. A layer reached a
/// second time (second parent, cycle, repeated listing) is skipped.
struct Walk<'a> {
    meta: &'a [MetaLayer],
    ids: &'a [LayerId],
    kinds: &'a [Kind],
    /// Child records of each folder, bottom → top.
    kids: &'a [Vec<usize>],
    visited: Vec<bool>,
    children: Vec<Vec<LayerId>>,
    root: Vec<LayerId>,
    /// Folders v1 never drew (nested deeper than `V1_FOLDER_DEPTH`).
    deep: u32,
    /// `(record, parent record or root, depth)`. A stack rather than
    /// recursion: hostile files can chain any number of folders.
    stack: Vec<(usize, Option<usize>, usize)>,
}

impl Walk<'_> {
    fn run(&mut self, warnings: &mut Vec<LoadWarning>) {
        while let Some((i, parent, depth)) = self.stack.pop() {
            let (Some(seen), Some(&id), Some(&kind)) = (self.visited.get_mut(i), self.ids.get(i), self.kinds.get(i)) else {
                continue;
            };
            if *seen {
                let layer = self.meta.get(i).map_or(0, |m| m.id);
                warnings.push(LoadWarning::LegacyDuplicateRef { layer });
                continue;
            }
            *seen = true;
            match parent.and_then(|p| self.children.get_mut(p)) {
                Some(siblings) => siblings.push(id),
                None => self.root.push(id),
            }
            if kind != Kind::Folder {
                continue;
            }
            if depth > V1_FOLDER_DEPTH {
                self.deep = self.deep.saturating_add(1);
            }
            // Past the depth limit, descendants are flattened into the
            // deepest folder allowed.
            let (cp, cd) = if depth < MAX_TREE_DEPTH { (Some(i), depth.saturating_add(1)) } else { (parent, depth) };
            for &c in self.kids.get(i).into_iter().flatten().rev() {
                if c == i {
                    warnings.push(LoadWarning::LegacyDuplicateRef { layer: self.meta.get(i).map_or(0, |m| m.id) });
                    continue;
                }
                self.stack.push((c, cp, cd));
            }
        }
    }
}

fn build_tree(meta: &Meta, max_layers: u32, warnings: &mut Vec<LoadWarning>) -> Result<Tree, IoError> {
    let n = meta.layers.len();
    let limit = max_layers.min(MAX_LAYER_COUNT);
    if n > limit as usize {
        return Err(IoError::limit("layers", n as u64, limit.into()));
    }
    // Ids are kept when valid and unique; the rest get fresh ones above.
    let mut by_old: AHashMap<u32, usize> = AHashMap::with_capacity(n);
    let mut keep = Vec::with_capacity(n);
    for (i, l) in meta.layers.iter().enumerate() {
        let first = match by_old.entry(l.id) {
            std::collections::hash_map::Entry::Vacant(v) => {
                v.insert(i);
                true
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                warnings.push(LoadWarning::LegacyDuplicateRef { layer: l.id });
                false
            }
        };
        keep.push(first && (1..=MAX_KEPT_ID).contains(&l.id));
    }
    let max_kept = meta.layers.iter().zip(&keep).filter(|(_, k)| **k).map(|(l, _)| l.id).max().unwrap_or(0);
    let mut next = max_kept.saturating_add(1);
    let ids: Vec<LayerId> = meta
        .layers
        .iter()
        .zip(&keep)
        .map(|(l, &k)| {
            if k {
                LayerId(l.id)
            } else {
                let id = LayerId(next);
                next = next.saturating_add(1);
                id
            }
        })
        .collect();
    let kinds: Vec<Kind> = meta.layers.iter().map(|l| Kind::of(&l.kind)).collect();

    // v1 lists are top → bottom; missing ids are skipped.
    let resolve = |old: &u32| by_old.get(old).copied();
    let kids: Vec<Vec<usize>> = meta
        .layers
        .iter()
        .zip(&kinds)
        .map(|(l, &k)| match k {
            Kind::Folder => l.folder_child_ids.iter().rev().filter_map(resolve).collect(),
            _ => Vec::new(),
        })
        .collect();
    let claimed: AHashSet<usize> =
        kids.iter().enumerate().flat_map(|(i, c)| c.iter().copied().filter(move |&c| c != i)).collect();

    let mut walk = Walk {
        meta: &meta.layers,
        ids: &ids,
        kinds: &kinds,
        kids: &kids,
        visited: vec![false; n],
        children: vec![Vec::new(); n],
        root: Vec::new(),
        deep: 0,
        stack: Vec::new(),
    };
    // A layer that is both listed at the top level and a folder's child
    // belongs to the folder.
    for old in meta.layer_order.iter() {
        if let Some(i) = resolve(old)
            && claimed.contains(&i)
        {
            warnings.push(LoadWarning::LegacyDuplicateRef { layer: *old });
        }
    }
    let top: Vec<usize> = meta.layer_order.iter().filter_map(resolve).filter(|i| !claimed.contains(i)).collect();
    // Pushed top first, so the bottom layer is placed first.
    walk.stack.extend(top.into_iter().map(|i| (i, None, 1)));
    walk.run(warnings);
    // Unlisted layers go on top, listed order kept (the first listed ends
    // up highest). Folders first take the children they claim.
    for unclaimed_only in [true, false] {
        for i in (0..n).rev() {
            if walk.visited.get(i) == Some(&false) && !(unclaimed_only && claimed.contains(&i)) {
                if let Some(id) = ids.get(i) {
                    warnings.push(LoadWarning::LegacyOrphan { layer: id.0 });
                }
                walk.stack.push((i, None, 1));
                walk.run(warnings);
            }
        }
    }
    if walk.deep > 0 {
        warnings.push(LoadWarning::LegacyDeepFolders { count: walk.deep });
    }
    let (children, root) = (walk.children, walk.root);
    Ok(Tree { ids, kinds, children, root, by_old })
}

fn props_of(l: &MetaLayer, id: LayerId, warnings: &mut Vec<LoadWarning>) -> LayerProps {
    let opacity = match l.opacity {
        Some(v) if (0.0..=1.0).contains(&v) => v,
        v => {
            warnings.push(LoadWarning::OpacityFixed { layer: id.0 });
            v.filter(|v| v.is_finite()).map_or(1.0, |v| v.clamp(0.0, 1.0))
        }
    };
    let mapped = |to, warnings: &mut Vec<LoadWarning>| {
        warnings.push(LoadWarning::LegacyBlendMapped { layer: id.0, from: l.blend_mode.clone() });
        to
    };
    // Folders keep their mode: v1 groups were always isolated, so they
    // never become PassThrough.
    let blend = match l.blend_mode.as_str() {
        "Normal" => BlendMode::Normal,
        "Multiply" => BlendMode::Multiply,
        "Screen" => BlendMode::Screen,
        "Overlay" => BlendMode::Overlay,
        "Luminosity" => mapped(BlendMode::Add, warnings),
        "Shade" => mapped(BlendMode::LinearBurn, warnings),
        other => {
            warnings.push(LoadWarning::LegacyUnknownBlend { name: other.to_owned() });
            BlendMode::Normal
        }
    };
    LayerProps {
        name: l.name.clone(),
        visible: l.visible,
        opacity,
        blend,
        clip: l.is_clipping,
        lock_alpha: l.lock_alpha,
        locked: false,
    }
}

/// Inflate one raw-deflate tile into `dst`, which it must fill exactly.
fn inflate(src: &[u8], dst: &mut [u8], d: &mut Decompress) -> bool {
    d.reset(false);
    matches!(d.decompress(src, dst, FlushDecompress::Finish), Ok(Status::StreamEnd))
        && d.total_out() == dst.len() as u64
}

/// Errors that `salvage` loads around (the tile is left out).
fn recoverable(e: &IoError) -> bool {
    matches!(e, IoError::Corrupt { .. } | IoError::Io { .. })
}

/// Import a v1 file. When `path` is given, decode tasks open their own
/// handles to it (it must be the file `src` reads). The result has no
/// path: it is saved under a new name, and overwriting the v1 file backs
/// it up first.
pub fn import_v1<R: ReadAt + Sync + ?Sized>(
    src: &R,
    path: Option<&Path>,
    o: &LoadOptions,
    pool: &ThreadPool,
    p: &Progress,
) -> Result<Loaded, IoError> {
    p.begin(phase::READ, 0);
    let len = src.len().map_err(IoError::io("read"))?;
    let (json_off, dir_off) = read_offsets(src, len)?;
    let meta = read_meta(src, json_off, dir_off)?;
    let mut warnings = Vec::new();
    let dir = read_dir(src, dir_off, len, o.limits.max_entries, &mut warnings)?;
    let tree = build_tree(&meta, o.limits.max_layers, &mut warnings)?;

    // Keep the last entry of each (layer, tile); drop tiles no raster
    // layer can hold.
    let mut slot_of: AHashMap<(usize, TileCoord), usize> = AHashMap::with_capacity(dir.len());
    let (mut unknown, mut folder, mut outside, mut repeated) = (0u32, 0u32, 0u32, 0u32);
    for (k, e) in dir.iter().enumerate() {
        let Some(&i) = tree.by_old.get(&e.layer) else {
            unknown = unknown.saturating_add(1);
            continue;
        };
        if tree.kinds.get(i) == Some(&Kind::Folder) {
            folder = folder.saturating_add(1);
        } else if !coord_in_domain(e.coord) {
            outside = outside.saturating_add(1);
        } else if slot_of.insert((i, e.coord), k).is_some() {
            repeated = repeated.saturating_add(1);
        }
    }
    for (count, reason) in [
        (unknown, "their layer is missing"),
        (folder, "they belong to a folder"),
        (outside, "outside the canvas range"),
        (repeated, "stored twice; the last copy was kept"),
    ] {
        if count > 0 {
            warnings.push(LoadWarning::LegacyDroppedTiles { count, reason });
        }
    }

    // Blobs in offset order, each with the (layer, tile) it fills.
    let mut damaged = 0u32;
    let mut kept: Vec<(TileEntry, (usize, TileCoord))> = Vec::with_capacity(slot_of.len());
    for (&owner, &k) in &slot_of {
        let Some(e) = dir.get(k) else { continue };
        let in_range = e.offset >= V1_HEADER_LEN
            && (1..=MAX_V1_TILE).contains(&e.csize)
            && e.offset.checked_add(e.csize.into()).is_some_and(|end| end <= json_off);
        if !in_range {
            if !o.salvage {
                return Err(IoError::corrupt("v1 tile range", e.offset));
            }
            damaged = damaged.saturating_add(1);
            continue;
        }
        let blob = TileEntry {
            coord: e.coord,
            codec: TileCodec::Raw,
            stored_len: e.csize,
            raw_crc: 0,
            stored_crc: 0,
            offset: e.offset,
        };
        kept.push((blob, owner));
    }
    kept.sort_unstable_by_key(|(b, (i, c))| (b.offset, *i, c.y, c.x));
    let bytes = (kept.len() as u64).saturating_mul(TILE_BYTES as u64);
    if bytes > o.limits.max_decoded_bytes {
        return Err(IoError::limit("decoded pixels", bytes, o.limits.max_decoded_bytes));
    }

    let blobs: Vec<TileEntry> = kept.iter().map(|(b, _)| *b).collect();
    p.begin(phase::DECODE, blobs.len() as u64);
    let decoded = for_each_blob(src, path, &blobs, pool, p, |e, stored, s| {
        // Inflated into scratch first: a SOLID tile (a third of a typical
        // page) is then rebuilt from its value and never allocated.
        let d = s.inflate.get_or_insert_with(|| Decompress::new(false));
        if !inflate(stored, bytemuck::bytes_of_mut(&mut *s.tile), d) {
            return Err(IoError::corrupt("v1 tile data", e.offset));
        }
        let clamped = sanitize(&mut s.tile);
        let class = classify(&s.tile);
        let general = match class {
            TileClass::Solid(_) => None,
            TileClass::General { .. } => {
                let mut tile = new_tile();
                let px = Arc::get_mut(&mut tile).ok_or(IoError::corrupt("tile allocation", e.offset))?;
                *px = *s.tile;
                Some(tile)
            }
        };
        Ok((general, clamped, class))
    });
    if p.is_cancelled() {
        return Err(IoError::Cancelled);
    }

    // SOLID values share one Arc; general tiles share one when their bytes
    // are equal (crc proposes, memcmp decides).
    let mut grids: Vec<Option<TileGrid>> = tree.kinds.iter().map(|&k| (k != Kind::Folder).then(TileGrid::new)).collect();
    let mut solids: AHashMap<[u16; 4], TileRef> = AHashMap::new();
    let mut by_crc: AHashMap<u32, SmallVec<[TileRef; 1]>> = AHashMap::new();
    let mut cache = TileCache::new();
    let mut clamped = 0u64;
    for (r, (_, (i, coord))) in decoded.into_iter().zip(&kept) {
        let (tile, n, class) = match r {
            Ok(t) => t,
            Err(e) if o.salvage && recoverable(&e) => {
                damaged = damaged.saturating_add(1);
                continue;
            }
            Err(e) => return Err(e),
        };
        clamped = clamped.saturating_add(n);
        let tile = match (class, tile) {
            (TileClass::Solid(v), _) => solids
                .entry(v)
                .or_insert_with(|| {
                    let mut t = new_tile();
                    if let Some(px) = Arc::get_mut(&mut t) {
                        px.as_flattened_mut().fill(v);
                    }
                    cache.insert(t.clone(), class);
                    t
                })
                .clone(),
            (TileClass::General { .. }, None) => continue,
            (TileClass::General { raw_crc }, Some(tile)) => {
                let same = by_crc.entry(raw_crc).or_default();
                match same.iter().find(|t| ***t == *tile) {
                    Some(t) => t.clone(),
                    None => {
                        cache.insert(tile.clone(), class);
                        same.push(tile.clone());
                        tile
                    }
                }
            }
        };
        if let Some(Some(g)) = grids.get_mut(*i) {
            g.insert(*coord, tile);
        }
    }
    if clamped > 0 {
        warnings.push(LoadWarning::ClampedPixels { count: clamped });
    }
    let mut lossy = None;
    if damaged > 0 {
        warnings.push(LoadWarning::DamagedTiles { count: damaged });
        lossy = Some(format!("Saving would lose data: {damaged} damaged tiles were left blank."));
    }

    let Tree { ids, kinds, mut children, mut root, .. } = tree;
    let mut layers = Vec::with_capacity(ids.len().saturating_add(1));
    for (i, (l, (&id, &kind))) in meta.layers.iter().zip(ids.iter().zip(&kinds)).enumerate() {
        let props = props_of(l, id, &mut warnings);
        let content = match kind {
            Kind::Folder => LayerContent::Folder {
                children: children.get_mut(i).map(std::mem::take).unwrap_or_default(),
                expanded: true,
            },
            Kind::Raster | Kind::Vector => {
                LayerContent::Raster(grids.get_mut(i).and_then(Option::take).unwrap_or_default())
            }
        };
        if kind == Kind::Vector {
            warnings.push(LoadWarning::LegacyVectorRasterized { name: l.name.clone() });
        }
        layers.push(Layer { id, props, content });
    }
    let mut next_id = ids.iter().map(|id| id.0).max().unwrap_or(0).saturating_add(1);
    let active = match topmost_raster(&layers, &root) {
        Some(id) => id,
        None => {
            let id = LayerId(next_id);
            next_id = next_id.saturating_add(1);
            layers.push(Layer { id, props: LayerProps::named("Layer 1"), content: LayerContent::Raster(TileGrid::new()) });
            root.push(id);
            warnings.push(LoadWarning::AddedMissingRaster);
            id
        }
    };
    let dpi = o.legacy_dpi.clamp(1, MAX_DPI);
    warnings.push(LoadWarning::LegacyDefaultDpi);
    let doc = Document::from_parts(DocParts {
        width: meta.canvas_width,
        height: meta.canvas_height,
        dpi,
        paper: Some(PAPER_WHITE),
        layers,
        root,
        active,
        next_id,
    })?;
    p.begin(phase::IDLE, 0);
    let info = FileInfo {
        kind: FileKind::LegacyV1,
        commit_seq: 0,
        saved_ms: 0,
        recovered: false,
        width: doc.width(),
        height: doc.height(),
        dpi,
        layer_count: doc.layer_count() as u32,
        meta: Vec::new(),
        thumb: None,
    };
    Ok(Loaded {
        doc,
        extra_sections: Vec::new(),
        layer_ext: Vec::new(),
        view: None,
        warnings,
        read_only_reason: lossy,
        info,
        // Tile classes are known, so the first save classifies nothing;
        // there is no v2 file to copy blobs from.
        seed: Some((cache, FileIndex::new([0; 16]))),
    })
}

/// The last raster layer in pre-order (parent first, siblings bottom →
/// top), which is the topmost one on screen.
fn topmost_raster(layers: &[Layer], root: &[LayerId]) -> Option<LayerId> {
    let by_id: AHashMap<LayerId, &Layer> = layers.iter().map(|l| (l.id, l)).collect();
    let mut top = None;
    let mut stack: Vec<LayerId> = root.iter().rev().copied().collect();
    while let Some(id) = stack.pop() {
        let Some(l) = by_id.get(&id) else { continue };
        match l.children() {
            Some(c) => stack.extend(c.iter().rev()),
            None => top = Some(id),
        }
    }
    top
}

/// What a v1 file's header and metadata say (no tiles are read).
pub(crate) fn read_info_v1<R: ReadAt + ?Sized>(src: &R, legacy_dpi: u32) -> Result<FileInfo, IoError> {
    let len = src.len().map_err(IoError::io("read"))?;
    let (json_off, dir_off) = read_offsets(src, len)?;
    let meta = read_meta(src, json_off, dir_off)?;
    Ok(FileInfo {
        kind: FileKind::LegacyV1,
        commit_seq: 0,
        saved_ms: 0,
        recovered: false,
        width: meta.canvas_width,
        height: meta.canvas_height,
        dpi: legacy_dpi,
        layer_count: meta.layers.len() as u32,
        meta: Vec::new(),
        thumb: None,
    })
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use flate2::Compression;
    use flate2::write::DeflateEncoder;

    use super::*;

    fn deflate(data: &[u8]) -> Vec<u8> {
        let mut e = DeflateEncoder::new(Vec::new(), Compression::default());
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    #[test]
    fn inflate_needs_exactly_one_tile() {
        let mut d = Decompress::new(false);
        let mut dst = vec![0u8; TILE_BYTES];
        let tile: Vec<u8> = (0..TILE_BYTES).map(|i| (i % 251) as u8).collect();
        assert!(inflate(&deflate(&tile), &mut dst, &mut d));
        assert_eq!(dst, tile);
        for n in [0, 100, TILE_BYTES - 1, TILE_BYTES + 1, 40_000] {
            let data: Vec<u8> = (0..n).map(|i| (i % 7) as u8).collect();
            assert!(!inflate(&deflate(&data), &mut dst, &mut d), "{n} bytes");
        }
        assert!(!inflate(&[0xFF; 50], &mut dst, &mut d), "garbage");
        assert!(!inflate(&[], &mut dst, &mut d), "empty");
        // The state is reset between tiles.
        assert!(inflate(&deflate(&tile), &mut dst, &mut d));
    }

    fn layer(id: u32, kind: &str, children: &[u32]) -> MetaLayer {
        MetaLayer { id, kind: kind.into(), folder_child_ids: children.to_vec(), ..Default::default() }
    }

    fn tree(layer_order: &[u32], layers: Vec<MetaLayer>) -> (Tree, Vec<LoadWarning>) {
        let meta = Meta { canvas_width: 64, canvas_height: 64, layer_order: layer_order.to_vec(), layers };
        let mut w = Vec::new();
        (build_tree(&meta, MAX_LAYER_COUNT, &mut w).unwrap(), w)
    }

    #[test]
    fn trees_are_reversed_and_repaired() {
        // Top → bottom: 3, folder 2 [5, 4], 1.
        let (t, w) = tree(&[3, 2, 1], vec![
            layer(3, "Raster", &[]),
            layer(2, "Folder", &[5, 4]),
            layer(1, "Raster", &[]),
            layer(5, "Raster", &[]),
            layer(4, "Raster", &[]),
        ]);
        assert_eq!(t.root, [LayerId(1), LayerId(2), LayerId(3)]);
        assert_eq!(t.children[1], [LayerId(4), LayerId(5)]);
        assert!(w.is_empty(), "{w:?}");

        // A cycle, a self-reference, a second parent and an unlisted layer.
        let (t, w) = tree(&[1, 2], vec![
            layer(1, "Folder", &[2, 1]),
            layer(2, "Folder", &[1, 3]),
            layer(3, "Raster", &[]),
            layer(4, "Folder", &[3]),
            layer(9, "Raster", &[]),
        ]);
        assert!(t.visited_all());
        assert!(w.contains(&LoadWarning::LegacyDuplicateRef { layer: 1 }));
        assert!(w.contains(&LoadWarning::LegacyOrphan { layer: 9 }));
        assert!(w.contains(&LoadWarning::LegacyOrphan { layer: 4 }));

        // Ids 0 and duplicates are renumbered above the largest kept id.
        let (t, w) = tree(&[0, 7], vec![layer(0, "Raster", &[]), layer(7, "Raster", &[]), layer(7, "Raster", &[])]);
        assert_eq!(t.ids, [LayerId(8), LayerId(7), LayerId(9)]);
        assert_eq!(t.by_old[&0], 0);
        assert_eq!(t.by_old[&7], 1);
        assert!(w.contains(&LoadWarning::LegacyDuplicateRef { layer: 7 }));
        assert!(w.contains(&LoadWarning::LegacyOrphan { layer: 9 }));
    }

    #[test]
    fn deep_chains_flatten_without_recursion() {
        let chain = |n: u32| {
            let mut layers: Vec<MetaLayer> = (1..n).map(|i| layer(i, "Folder", &[i + 1])).collect();
            layers.push(layer(n, "Raster", &[]));
            layers
        };
        let meta = Meta { canvas_width: 64, canvas_height: 64, layer_order: vec![1], layers: chain(70_000) };
        let err = build_tree(&meta, MAX_LAYER_COUNT, &mut Vec::new()).err();
        assert!(matches!(err, Some(IoError::LimitExceeded { what: "layers", .. })));

        // 60k nested folders: no recursion, so no stack overflow.
        let (t, _) = tree(&[1], chain(60_000));
        assert!(t.visited_all());

        // Below folder 63 (depth 63), everything is flattened into it.
        let n = 200u32;
        let (t, w) = tree(&[1], chain(n));
        assert!(t.visited_all());
        let limit = MAX_TREE_DEPTH as u32;
        assert_eq!(t.children[MAX_TREE_DEPTH - 2], (limit..=n).map(LayerId).collect::<Vec<_>>());
        assert!(t.children[MAX_TREE_DEPTH - 1..].iter().all(Vec::is_empty));
        assert!(w.contains(&LoadWarning::LegacyDeepFolders { count: n - 1 - V1_FOLDER_DEPTH as u32 }));
    }

    impl Tree {
        /// Every record placed exactly once.
        fn visited_all(&self) -> bool {
            let mut placed: Vec<LayerId> = self.root.iter().chain(self.children.iter().flatten()).copied().collect();
            placed.sort();
            let mut all = self.ids.clone();
            all.sort();
            placed == all
        }
    }
}
