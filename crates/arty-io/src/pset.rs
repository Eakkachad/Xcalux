//! The `PSET` section: the page setup (SAFE_TO_COPY, 44-byte body; later
//! minor versions may append bytes).
//!
//! ```text
//! u8 ver=1, u8 unit, u16 flags=0, f32×4 trim (x, y, w, h), f32 bleed,
//! f32 safe, f32×4 inner (w == 0: none)
//! ```

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

use arty_core::{PageSetup, RectF};

use crate::error::LoadWarning;
use crate::format::ByteReader;

pub const PSET_VERSION: u8 = 1;
pub const PSET_LEN: usize = 44;

/// The `PSET` body, when the document has a page setup.
pub fn encode(p: Option<&PageSetup>) -> Option<Vec<u8>> {
    let p = p?;
    let mut b = Vec::with_capacity(PSET_LEN);
    b.extend_from_slice(&[PSET_VERSION, p.unit, 0, 0]);
    let r = |b: &mut Vec<u8>, r: RectF| {
        for v in [r.x, r.y, r.w, r.h] {
            b.extend_from_slice(&v.to_le_bytes());
        }
    };
    r(&mut b, p.trim);
    b.extend_from_slice(&p.bleed.to_le_bytes());
    b.extend_from_slice(&p.safe.to_le_bytes());
    r(&mut b, p.inner);
    debug_assert_eq!(b.len(), PSET_LEN);
    Some(b)
}

/// The page setup in a `PSET` body, sanitized for a `w`×`h` page; `None`
/// (with `PageSetupDropped`) when invalid.
pub fn decode(b: &[u8], w: u32, h: u32, warn: &mut Vec<LoadWarning>) -> Option<PageSetup> {
    let p = parse(b).and_then(|p| p.sanitized(w, h));
    if p.is_none() {
        warn.push(LoadWarning::PageSetupDropped);
    }
    p
}

fn parse(b: &[u8]) -> Option<PageSetup> {
    let mut r = ByteReader::new(b, "truncated PSET section", 0);
    if r.u8().ok()? != PSET_VERSION {
        return None;
    }
    let unit = r.u8().ok()?;
    r.skip(2).ok()?;
    let trim = rect(&mut r)?;
    let (bleed, safe) = (r.f32().ok()?, r.f32().ok()?);
    let inner = rect(&mut r)?;
    Some(PageSetup { trim, bleed, safe, inner, unit })
}

fn rect(r: &mut ByteReader<'_>) -> Option<RectF> {
    Some(RectF { x: r.f32().ok()?, y: r.f32().ok()?, w: r.f32().ok()?, h: r.f32().ok()? })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_layout() {
        let p = PageSetup {
            trim: RectF { x: 1.0, y: 2.0, w: 3.0, h: 4.0 },
            bleed: 5.0,
            safe: 6.0,
            inner: RectF { x: 7.0, y: 8.0, w: 9.0, h: 10.0 },
            unit: 2,
        };
        let b = encode(Some(&p)).unwrap();
        assert_eq!(b.len(), 44);
        assert_eq!(&b[..4], &[1, 2, 0, 0]);
        assert_eq!(&b[4..8], &1.0f32.to_le_bytes());
        assert_eq!(&b[40..44], &10.0f32.to_le_bytes());
        assert_eq!(parse(&b), Some(p));
        assert_eq!(encode(None), None);
        // A later minor version's appended bytes are ignored.
        let mut longer = b.clone();
        longer.extend_from_slice(&[0xAA; 12]);
        assert_eq!(parse(&longer), Some(p));
        assert_eq!(parse(&b[..43]), None);
        let mut v2 = b;
        v2[0] = 2;
        assert_eq!(parse(&v2), None);
    }
}
