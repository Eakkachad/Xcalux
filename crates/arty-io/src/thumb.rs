//! The page thumbnail of the `THUM` section: the page (its trim, when it has
//! a page setup) flattened over its paper, or white where there is none, and
//! reduced by a box filter to at most `MAX_THUMB_SIDE` pixels.
//!
//! [`from_overview`] cuts it from the app's eighth-scale copy of the page,
//! which the canvas keeps for display anyway; [`make`] flattens the page
//! itself, for a save the copy is not current for.

use arty_core::{CompositeScratch, Document, TILE_SIZE, TileCoord, fix15, tile::new_tile_box};
use rayon::ThreadPool;
use rayon::prelude::*;

use crate::limits::MAX_THUMB_SIDE;

/// `(w, h, RGBA8)`, opaque. `None` for an empty page rectangle.
pub type Thumbnail = (u16, u16, Vec<u8>);

/// What the thumbnail shows: the trim rectangle inside the page, else all of it.
fn region(doc: &Document) -> (u32, u32, u32, u32) {
    let (w, h) = (doc.width(), doc.height());
    let Some(page) = doc.page_setup() else { return (0, 0, w, h) };
    let clamp = |v: f32, max: u32| (v.round().max(0.0) as u32).min(max);
    let r = page.trim;
    let (x0, x1) = (clamp(r.x, w), clamp(r.x + r.w, w));
    let (y0, y1) = (clamp(r.y, h), clamp(r.y + r.h, h));
    if x1 <= x0 || y1 <= y0 { (0, 0, w, h) } else { (x0, y0, x1 - x0, y1 - y0) }
}

/// Size of the thumbnail of a `w` × `h` region: the long side is
/// `MAX_THUMB_SIDE` (never enlarged), the aspect is kept.
pub fn thumb_size(w: u32, h: u32) -> (u32, u32) {
    let max = u32::from(MAX_THUMB_SIDE);
    let long = w.max(h).max(1);
    if long <= max {
        return (w.max(1), h.max(1));
    }
    let scale = |s: u32| ((u64::from(s) * u64::from(max) + u64::from(long) / 2) / u64::from(long)).max(1) as u32;
    (scale(w), scale(h))
}

/// The thumbnail of `doc` taken from `image`, the page at `1 / scale` (premultiplied
/// RGBA8, `width` × `height`, tiles rounded up). It is never larger than that image.
pub fn from_overview(doc: &Document, width: usize, height: usize, image: &[u8], scale: u32) -> Option<Thumbnail> {
    let (x0, y0, w, h) = region(doc);
    let s = scale.max(1) as usize;
    let (cx0, cy0) = (x0 as usize / s, y0 as usize / s);
    let (cx1, cy1) = (((x0 + w) as usize).div_ceil(s).min(width), ((y0 + h) as usize).div_ceil(s).min(height));
    if cx1 <= cx0 || cy1 <= cy0 || image.len() < width * height * 4 {
        return None;
    }
    let (cw, ch) = (cx1 - cx0, cy1 - cy0);
    let (tw, th) = thumb_size(w, h);
    let (tw, th) = ((tw as usize).min(cw), (th as usize).min(ch));
    let mut sums = vec![[0u32; 4]; tw * th];
    let mut counts = vec![0u32; tw * th];
    for y in 0..ch {
        let row = &image[((cy0 + y) * width + cx0) * 4..][..cw * 4];
        let o = y * th / ch * tw;
        for (x, p) in row.chunks_exact(4).enumerate() {
            let i = o + x * tw / cw;
            // Over white: the missing alpha is white in every channel.
            let gap = 255 - u32::from(p[3]);
            for (s, &c) in sums[i].iter_mut().zip(p) {
                *s += u32::from(c) + gap;
            }
            counts[i] += 1;
        }
    }
    let mut px = Vec::with_capacity(tw * th * 4);
    for (s, &n) in sums.iter().zip(&counts) {
        let n = n.max(1);
        px.extend(s[..3].iter().map(|&c| ((c + n / 2) / n).min(255) as u8));
        px.push(255);
    }
    Some((tw as u16, th as u16, px))
}

/// Box sums of the output rows `first..first + sums.len() / width` that one band of the page touches.
struct Band {
    first: u32,
    sums: Vec<[u32; 4]>,
    counts: Vec<u32>,
}

/// Flatten the page and reduce it, one band of tile rows per task on `pool`.
pub fn make(doc: &Document, pool: &ThreadPool) -> Option<Thumbnail> {
    let (x0, y0, w, h) = region(doc);
    if w == 0 || h == 0 {
        return None;
    }
    let (ow, oh) = thumb_size(w, h);
    let t = TILE_SIZE as u32;
    let (tx0, tx1) = (x0 / t, (x0 + w - 1) / t);
    // Output index of region offset `d` along an axis of `len` pixels reduced to `out`.
    let map = |d: u32, len: u32, out: u32| (u64::from(d) * u64::from(out) / u64::from(len)) as u32;
    let bands: Vec<u32> = (y0 / t..=(y0 + h - 1) / t).collect();
    let parts: Vec<Band> = pool.install(|| {
        bands
            .into_par_iter()
            .map_init(
                || (CompositeScratch::new(), new_tile_box()),
                |(scratch, tile), ty| {
                    let (ya, yb) = ((ty * t).max(y0), ((ty + 1) * t).min(y0 + h));
                    let (first, last) = (map(ya - y0, h, oh), map(yb - 1 - y0, h, oh));
                    let rows = (last - first + 1) as usize;
                    let mut band = Band { first, sums: vec![[0; 4]; rows * ow as usize], counts: vec![0; rows * ow as usize] };
                    for tx in tx0..=tx1 {
                        doc.composite_tile(TileCoord::new(tx as i32, ty as i32), tile, scratch);
                        let (xa, xb) = ((tx * t).max(x0), ((tx + 1) * t).min(x0 + w));
                        for y in ya..yb {
                            let row = &tile[(y - ty * t) as usize];
                            let at = (map(y - y0, h, oh) - first) as usize * ow as usize;
                            for x in xa..xb {
                                let p = row[(x - tx * t) as usize];
                                // Over white: premultiplied white adds the missing alpha to every channel.
                                let gap = fix15::ONE - u32::from(p[3]);
                                let i = at + map(x - x0, w, ow) as usize;
                                for (s, c) in band.sums[i].iter_mut().zip(p) {
                                    *s += u32::from(c) + gap;
                                }
                                band.counts[i] += 1;
                            }
                        }
                    }
                    band
                },
            )
            .collect()
    });
    let mut sums = vec![[0u32; 4]; ow as usize * oh as usize];
    let mut counts = vec![0u32; sums.len()];
    for b in parts {
        let at = b.first as usize * ow as usize;
        for (i, (s, c)) in b.sums.iter().zip(&b.counts).enumerate() {
            for (d, v) in sums[at + i].iter_mut().zip(s) {
                *d += v;
            }
            counts[at + i] += c;
        }
    }
    let mut px = Vec::with_capacity(sums.len() * 4);
    for (s, &n) in sums.iter().zip(&counts) {
        let n = n.max(1);
        for &c in &s[..3] {
            px.push(fix15::to_u8((c / n).min(fix15::ONE) as u16));
        }
        px.push(255);
    }
    Some((ow as u16, oh as u16, px))
}

#[cfg(test)]
mod tests {
    use super::*;
    use arty_core::{PageSetup, RectF};

    fn pool() -> ThreadPool {
        rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap()
    }

    fn at(t: &Thumbnail, x: usize, y: usize) -> [u8; 4] {
        let i = (y * usize::from(t.0) + x) * 4;
        t.2[i..i + 4].try_into().unwrap()
    }

    /// Paint the whole tile at `c` in layer `id` with `v`.
    fn fill_tile(doc: &mut Document, c: TileCoord, v: [u16; 4]) {
        let id = doc.active();
        let tile = doc.paint_target(id).unwrap().0.get_mut_or_create(c);
        for row in tile.iter_mut() {
            row.fill(v);
        }
    }

    #[test]
    fn sizes_keep_the_aspect_and_never_grow() {
        assert_eq!(thumb_size(2894, 4093), (181, 256));
        assert_eq!(thumb_size(800, 12800), (16, 256));
        assert_eq!(thumb_size(4093, 2894), (256, 181));
        assert_eq!(thumb_size(100, 50), (100, 50));
        assert_eq!(thumb_size(256, 256), (256, 256));
        assert_eq!(thumb_size(100_000, 10), (256, 1));
    }

    #[test]
    fn a_blank_page_is_white() {
        let doc = Document::new(1000, 600, 72);
        let t = make(&doc, &pool()).unwrap();
        assert_eq!((t.0, t.1, t.2.len()), (256, 154, 256 * 154 * 4));
        assert!(t.2.chunks(4).all(|p| p == [255, 255, 255, 255]));
    }

    #[test]
    fn paint_lands_where_it_is_and_is_averaged() {
        // 512 × 512: 8 × 8 tiles, reduced 2:1 to 256 × 256. A red tile at (2, 1) covers 32 × 32 thumbnail pixels at (64, 32).
        let mut doc = Document::new(512, 512, 72);
        fill_tile(&mut doc, TileCoord::new(2, 1), [fix15::ONE as u16, 0, 0, fix15::ONE as u16]);
        let t = make(&doc, &pool()).unwrap();
        assert_eq!((t.0, t.1), (256, 256));
        assert_eq!(at(&t, 64, 32), [255, 0, 0, 255]);
        assert_eq!(at(&t, 95, 63), [255, 0, 0, 255]);
        assert_eq!(at(&t, 63, 32), [255, 255, 255, 255]);
        assert_eq!(at(&t, 96, 63), [255, 255, 255, 255]);
        assert_eq!(at(&t, 64, 64), [255, 255, 255, 255]);
        // Half-transparent black over white is mid grey.
        let mut doc = Document::new(64, 64, 72);
        fill_tile(&mut doc, TileCoord::new(0, 0), [0, 0, 0, (fix15::ONE / 2) as u16]);
        let g = at(&make(&doc, &pool()).unwrap(), 10, 10);
        assert!((127..=128).contains(&g[0]) && g[0] == g[1] && g[1] == g[2] && g[3] == 255, "{g:?}");
    }

    /// An eighth-scale image of `doc`: `paint(x, y)` per pixel.
    fn overview(doc: &Document, paint: impl Fn(usize, usize) -> [u8; 4]) -> (usize, usize, Vec<u8>) {
        let (w, h) = (doc.tiles_wide() as usize * 8, doc.tiles_high() as usize * 8);
        let px = (0..w * h).flat_map(|i| paint(i % w, i / w)).collect();
        (w, h, px)
    }

    #[test]
    fn an_overview_gives_the_thumbnail_over_white() {
        // 256 × 192: a 32 × 24 overview, shown 1:1 (never enlarged).
        let doc = Document::new(256, 192, 72);
        let (w, h, px) = overview(&doc, |x, y| match (x < 16, y < 12) {
            (true, true) => [255, 0, 0, 255],
            (true, false) => [0, 0, 0, 128],
            _ => [0; 4],
        });
        let t = from_overview(&doc, w, h, &px, 8).unwrap();
        assert_eq!((t.0, t.1), (32, 24));
        assert_eq!(at(&t, 0, 0), [255, 0, 0, 255]);
        assert_eq!(at(&t, 20, 5), [255, 255, 255, 255], "transparent is white");
        assert_eq!(at(&t, 5, 20), [127, 127, 127, 255], "half black over white is grey");

        // 4096 square: a 512 overview is reduced 2:1 to 256, a checker averages to grey.
        let doc = Document::new(4096, 4096, 72);
        let (w, h, px) = overview(&doc, |x, y| if (x + y) % 2 == 0 { [0, 0, 0, 255] } else { [255; 4] });
        let t = from_overview(&doc, w, h, &px, 8).unwrap();
        assert_eq!((t.0, t.1), (256, 256));
        assert!(t.2.chunks(4).all(|p| p == [128, 128, 128, 255]), "{:?}", &t.2[..4]);
    }

    #[test]
    fn an_overview_of_a_page_setup_shows_the_trim() {
        // 512 × 512 with the trim at x 256..512: the overview's right half.
        let mut doc = Document::new(512, 512, 72);
        let page = PageSetup { trim: RectF { x: 256.0, y: 0.0, w: 256.0, h: 512.0 }, bleed: 0.0, safe: 0.0, inner: RectF::default(), unit: 0 };
        doc.set_page_unrecorded(Some(page));
        let (w, h, px) = overview(&doc, |x, _| if x < 32 { [255, 0, 0, 255] } else { [0, 0, 255, 255] });
        let t = from_overview(&doc, w, h, &px, 8).unwrap();
        assert_eq!((t.0, t.1), (32, 64));
        assert!(t.2.chunks(4).all(|p| p == [0, 0, 255, 255]));
        // A short image (not the page's) is refused.
        assert!(from_overview(&doc, w, h, &px[..100], 8).is_none());
    }

    #[test]
    fn a_page_setup_shows_the_trim() {
        // A 400 × 400 page whose trim is the right half: red tiles outside it never show.
        let mut doc = Document::new(400, 400, 72);
        fill_tile(&mut doc, TileCoord::new(0, 0), [fix15::ONE as u16, 0, 0, fix15::ONE as u16]);
        let page = PageSetup { trim: RectF { x: 200.0, y: 0.0, w: 200.0, h: 400.0 }, bleed: 0.0, safe: 0.0, inner: RectF::default(), unit: 0 };
        doc.set_page_unrecorded(Some(page));
        let t = make(&doc, &pool()).unwrap();
        assert_eq!((t.0, t.1), (128, 256));
        assert!(t.2.chunks(4).all(|p| p == [255, 255, 255, 255]));
    }
}
