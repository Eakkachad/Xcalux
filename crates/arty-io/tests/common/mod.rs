//! Shared helpers for arty-io integration tests: a seeded document
//! generator, in-memory save/load, and document comparison.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;

use ahash::AHashMap;
use arty_core::tile::new_tile;
use arty_core::{
    BlendMode, CompositeScratch, DocParts, Document, Layer, LayerContent, LayerId, LayerProps, TileCoord, TileGrid,
    TileRef,
};
use arty_io::format::{T_MAX, T_MIN};
use arty_io::{CommitMeta, FileWriter, LoadOptions, Loaded, Progress, SaveExtras, SaveOptions, SessionId, Verify};
use rayon::ThreadPool;

pub const UUID: [u8; 16] = [0x5A; 16];
pub const NOW: u64 = 1_750_000_000_000;
pub const ONE: u16 = 1 << 15;

pub fn pool() -> ThreadPool {
    rayon::ThreadPoolBuilder::new().num_threads(3).build().unwrap()
}

pub fn session() -> SessionId {
    SessionId([0xC3; 16])
}

/// Fixed clock and uuid, full verification.
pub fn opts() -> SaveOptions {
    SaveOptions { verify: Verify::Full, now_ms: Some(NOW), uuid: Some(UUID) }
}

pub fn meta(doc: &Document) -> CommitMeta<'static> {
    CommitMeta { session: session(), rev: doc.revision(), src: None, clean: false }
}

/// A single-commit file in memory, byte-identical to what `save_main`
/// writes with [`opts`] and [`session`].
pub fn write(doc: &Document, ex: &SaveExtras, pool: &ThreadPool) -> Vec<u8> {
    let mut w = FileWriter::create(Vec::new(), 0, UUID).unwrap();
    w.commit(doc, ex, &meta(doc), &opts(), pool, &Progress::default()).unwrap();
    w.into_sink()
}

pub fn read(bytes: &[u8], pool: &ThreadPool) -> Loaded {
    read_with(bytes, &LoadOptions::default(), pool).unwrap()
}

pub fn read_with(bytes: &[u8], o: &LoadOptions, pool: &ThreadPool) -> Result<Loaded, arty_io::IoError> {
    arty_io::load_from(bytes, None, o, pool, &Progress::default())
}

/// A fresh, empty directory under the system temp dir.
pub fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("arty-io-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    pub fn chance(&mut self, num: u64, den: u64) -> bool {
        self.below(den) < num
    }

    pub fn pick<T: Copy>(&mut self, xs: &[T]) -> T {
        xs[self.below(xs.len() as u64) as usize]
    }

    /// A valid channel value, sometimes exactly 1.0.
    pub fn channel(&mut self) -> u16 {
        if self.chance(1, 16) { ONE } else { (self.next() as u16) & 0x7FFF }
    }
}

pub fn all_blends() -> Vec<BlendMode> {
    let mut v = BlendMode::LAYER_MODES.to_vec();
    v.push(BlendMode::PassThrough);
    v
}

pub fn filled(px: [u16; 4]) -> TileRef {
    let mut t = new_tile();
    Arc::get_mut(&mut t).unwrap().as_flattened_mut().fill(px);
    t
}

/// A tile of the given shape, all channels in `0..=1<<15`:
/// 0 empty, 1 solid, 2 noise, 3 gradient, 4 line art, 5 colour above
/// alpha (merge output), 6 sparse dots.
pub fn tile(rng: &mut Rng, kind: u64) -> TileRef {
    match kind {
        0 => return filled([0; 4]),
        1 => return filled([rng.channel(), rng.channel(), rng.channel(), rng.channel()]),
        _ => {}
    }
    let mut t = new_tile();
    let base = rng.next();
    let px = Arc::get_mut(&mut t).unwrap();
    for (i, p) in px.as_flattened_mut().iter_mut().enumerate() {
        let (x, y) = ((i % 64) as u64, (i / 64) as u64);
        *p = match kind {
            2 => [rng.channel(), rng.channel(), rng.channel(), rng.channel()],
            3 => {
                let a = ((base & 0xFFF) + x * 170 + y * 90).min(ONE as u64) as u16;
                [a / 2, a / 3, a, a]
            }
            4 => {
                if (x + y * 3 + base) % 17 < 2 { [0, 0, 0, ONE] } else { [0; 4] }
            }
            5 => {
                let a = ((x * 500 + y * 7) as u16).min(ONE - 1);
                [a + 1, a / 2, a, a]
            }
            _ => {
                if rng.chance(1, 40) { [ONE / 2, ONE / 4, 0, ONE] } else { [0; 4] }
            }
        };
    }
    t
}

/// Where tiles land: mostly on the page, sometimes past its edges or near
/// the ends of the coordinate domain.
fn coord(rng: &mut Rng, tw: i32, th: i32) -> TileCoord {
    match rng.below(10) {
        0 => TileCoord::new(-1, -1),
        1 => TileCoord::new(rng.pick(&[T_MIN, T_MAX]), rng.pick(&[T_MIN, T_MAX, 0])),
        2 => TileCoord::new(tw, th),
        3 => TileCoord::new(rng.below(2001) as i32 - 1000, rng.below(2001) as i32 - 1000),
        _ => TileCoord::new(rng.below(tw as u64) as i32, rng.below(th as u64) as i32),
    }
}

fn props(rng: &mut Rng, folder: bool) -> LayerProps {
    const NAMES: [&str; 5] = ["", "Layer", "線画 ✒️", "Ünïcödé\u{1F3A8}", "tab\tand\nnewline"];
    let name = match rng.below(8) {
        // Exactly 4096 bytes of 3-byte chars plus one ASCII byte.
        0 => "あ".repeat(1365) + "a",
        1 => "x".repeat(4096),
        _ => rng.pick(&NAMES).to_owned(),
    };
    let opacity = match rng.below(7) {
        0 => 0.0,
        1 => 1.0,
        2 => f32::from_bits(1), // subnormal
        3 => -0.0,
        4 => f32::from_bits(0x007F_FFFF), // largest subnormal
        _ => (rng.below(1 << 24) as f32) / (1 << 24) as f32,
    };
    let blend = if folder && rng.chance(1, 2) { BlendMode::PassThrough } else { rng.pick(&all_blends()) };
    LayerProps {
        name,
        visible: rng.chance(4, 5),
        opacity,
        blend,
        clip: rng.chance(1, 4),
        lock_alpha: rng.chance(1, 4),
        locked: rng.chance(1, 6),
    }
}

/// A random but valid document: up to 12 layers nested up to 6 deep, id
/// gaps, every flag and blend mode, odd opacities and names, tiles of
/// every shape on and off the page, and Arcs shared within and across
/// layers. Built with `from_parts`, so its revision is 0.
pub fn random_doc(rng: &mut Rng) -> Document {
    let (w, h) = (1 + rng.below(3000) as u32, 1 + rng.below(3000) as u32);
    let (tw, th) = (w.div_ceil(64) as i32, h.div_ceil(64) as i32);
    let n = 1 + rng.below(12) as usize;
    struct Node {
        id: LayerId,
        folder: bool,
        depth: usize,
        children: Vec<LayerId>,
    }
    let mut nodes: Vec<Node> = Vec::new();
    let mut root = Vec::new();
    let mut next = 1 + rng.below(4) as u32;
    for i in 0..n {
        let id = LayerId(next);
        next += 1 + rng.below(3) as u32;
        let parents: Vec<usize> = (0..nodes.len()).filter(|&j| nodes[j].folder && nodes[j].depth < 6).collect();
        let depth = if !parents.is_empty() && rng.chance(2, 3) {
            let p = rng.pick(&parents);
            nodes[p].children.push(id);
            nodes[p].depth + 1
        } else {
            root.push(id);
            1
        };
        // The last layer is always a raster, so there is at least one.
        let folder = i + 1 < n && rng.chance(1, 3);
        nodes.push(Node { id, folder, depth, children: Vec::new() });
    }
    let mut shared: Vec<TileRef> = Vec::new();
    let mut layers = Vec::new();
    for node in nodes {
        let props = props(rng, node.folder);
        let content = if node.folder {
            LayerContent::Folder { children: node.children, expanded: rng.chance(1, 2) }
        } else {
            let mut grid = TileGrid::new();
            for _ in 0..rng.below(9) {
                let t = match rng.below(8) {
                    0 | 1 if !shared.is_empty() => shared[rng.below(shared.len() as u64) as usize].clone(),
                    2 if !shared.is_empty() => {
                        // Same bytes, different Arc.
                        let src = shared[rng.below(shared.len() as u64) as usize].clone();
                        let mut t = new_tile();
                        *Arc::get_mut(&mut t).unwrap() = *src;
                        t
                    }
                    _ => {
                        let kind = rng.below(7);
                        tile(rng, kind)
                    }
                };
                shared.push(t.clone());
                grid.insert(coord(rng, tw, th), t);
            }
            LayerContent::Raster(grid)
        };
        layers.push(Layer { id: node.id, props, content });
    }
    let active = layers[rng.below(layers.len() as u64) as usize].id;
    let paper = rng.chance(1, 2).then(|| [rng.channel(), rng.channel(), rng.channel(), ONE]);
    let parts = DocParts {
        width: w,
        height: h,
        dpi: 1 + rng.below(1200) as u32,
        paper,
        layers,
        root,
        active,
        next_id: next + rng.below(3) as u32,
    };
    Document::from_parts(parts).unwrap_or_else(|e| panic!("generator built an invalid tree: {e:?}"))
}

/// Every id reachable from the root, in pre-order.
pub fn layer_ids(doc: &Document) -> Vec<LayerId> {
    fn walk(doc: &Document, ids: &[LayerId], out: &mut Vec<LayerId>) {
        for &id in ids {
            out.push(id);
            if let Some(c) = doc.layer(id).unwrap().children() {
                walk(doc, c, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(doc, doc.root(), &mut out);
    out
}

/// `b` equals `a` in every saved field, pixel and tile key; every pair of
/// tiles sharing an Arc in `a` shares one in `b`; composites agree.
pub fn assert_same_doc(a: &Document, b: &Document) {
    assert_eq!((a.width(), a.height(), a.dpi()), (b.width(), b.height(), b.dpi()));
    assert_eq!(a.paper(), b.paper());
    assert_eq!(a.root(), b.root());
    assert_eq!(a.active(), b.active());
    assert_eq!(a.next_layer_id(), b.next_layer_id());
    assert_eq!(a.layer_count(), b.layer_count());
    let mut sharing: AHashMap<usize, usize> = AHashMap::new();
    let mut samples = vec![TileCoord::new(0, 0)];
    for id in layer_ids(a) {
        let (la, lb) = (a.layer(id).unwrap(), b.layer(id).expect("layer missing after load"));
        let (pa, pb) = (&la.props, &lb.props);
        assert_eq!(pa.name, pb.name, "layer {id:?}");
        assert_eq!(pa.opacity.to_bits(), pb.opacity.to_bits(), "layer {id:?} opacity");
        assert_eq!(
            (pa.visible, pa.blend, pa.clip, pa.lock_alpha, pa.locked),
            (pb.visible, pb.blend, pb.clip, pb.lock_alpha, pb.locked),
            "layer {id:?}"
        );
        match (&la.content, &lb.content) {
            (LayerContent::Folder { children: ca, expanded: ea }, LayerContent::Folder { children: cb, expanded: eb }) => {
                assert_eq!((ca, ea), (cb, eb), "folder {id:?}");
            }
            (LayerContent::Raster(ga), LayerContent::Raster(gb)) => {
                let mut ka: Vec<_> = ga.coords().collect();
                let mut kb: Vec<_> = gb.coords().collect();
                ka.sort();
                kb.sort();
                assert_eq!(ka, kb, "layer {id:?} tile keys");
                for (c, ta) in ga.iter() {
                    let tb = gb.get_ref(c).unwrap();
                    assert!(**ta == **tb, "layer {id:?} tile {c:?} pixels");
                    let (qa, qb) = (Arc::as_ptr(ta) as usize, Arc::as_ptr(tb) as usize);
                    assert_eq!(*sharing.entry(qa).or_insert(qb), qb, "layer {id:?} tile {c:?} lost sharing");
                    if samples.len() < 6 {
                        samples.push(c);
                    }
                }
            }
            _ => panic!("layer {id:?} changed kind"),
        }
    }
    let (mut sa, mut sb) = (CompositeScratch::new(), CompositeScratch::new());
    let (mut oa, mut ob) = (arty_core::tile::new_tile_box(), arty_core::tile::new_tile_box());
    for c in samples {
        a.composite_tile(c, &mut oa, &mut sa);
        b.composite_tile(c, &mut ob, &mut sb);
        assert!(*oa == *ob, "composite differs at {c:?}");
    }
}

/// Record boundaries of a well-formed file: `(offset, kind, end)`.
pub fn records(file: &[u8]) -> Vec<(u64, u8, u64)> {
    let mut out = Vec::new();
    let mut pos = 64usize;
    while pos < file.len() {
        let h = arty_io::format::RecordHeader::decode(&file[pos..pos + 24], pos as u64).unwrap();
        let end = h.end(pos as u64).unwrap();
        out.push((pos as u64, h.kind, end));
        pos = end as usize;
    }
    out
}
