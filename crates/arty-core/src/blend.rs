//! Layer blend modes on premultiplied fix15 tiles.
//!
//! Separable modes follow the W3C compositing spec:
//! `co = cs·(1−αb) + cb·(1−αs) + αs·αb·B(Cb, Cs)` (premultiplied form).
//! `Normal` has an integer fast path since it dominates real documents.

use serde::{Deserialize, Serialize};

use crate::fix15::{self, ONE};
use crate::tile::TilePixels;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    LinearBurn,
    /// Linear dodge — SAI's "Luminosity" / CSP "Add (Glow)".
    Add,
    SoftLight,
    HardLight,
    Difference,
    /// Folders only: children blend straight into the backdrop.
    PassThrough,
}

impl BlendMode {
    pub const LAYER_MODES: [BlendMode; 13] = [
        BlendMode::Normal,
        BlendMode::Multiply,
        BlendMode::Screen,
        BlendMode::Overlay,
        BlendMode::Darken,
        BlendMode::Lighten,
        BlendMode::ColorDodge,
        BlendMode::ColorBurn,
        BlendMode::LinearBurn,
        BlendMode::Add,
        BlendMode::SoftLight,
        BlendMode::HardLight,
        BlendMode::Difference,
    ];

    pub fn label(self) -> &'static str {
        match self {
            BlendMode::Normal => "Normal",
            BlendMode::Multiply => "Multiply",
            BlendMode::Screen => "Screen",
            BlendMode::Overlay => "Overlay",
            BlendMode::Darken => "Darken",
            BlendMode::Lighten => "Lighten",
            BlendMode::ColorDodge => "Color Dodge",
            BlendMode::ColorBurn => "Color Burn",
            BlendMode::LinearBurn => "Linear Burn",
            BlendMode::Add => "Add (Glow)",
            BlendMode::SoftLight => "Soft Light",
            BlendMode::HardLight => "Hard Light",
            BlendMode::Difference => "Difference",
            BlendMode::PassThrough => "Pass Through",
        }
    }
}

/// How the source participates in the result.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Porter {
    /// Regular layer compositing.
    Over,
    /// Clipping: source only lands where the backdrop already has alpha,
    /// and the backdrop's alpha is preserved.
    Atop,
}

/// Composite `src` onto `dst` with `opacity` (0..=1) and `mode`.
pub fn blend_tile(dst: &mut TilePixels, src: &TilePixels, opacity: f32, mode: BlendMode) {
    dispatch(dst, src, opacity, mode, Porter::Over);
}

/// Composite `src` onto `dst` clipped to `dst`'s alpha (clipping layers).
pub fn blend_tile_atop(dst: &mut TilePixels, src: &TilePixels, opacity: f32, mode: BlendMode) {
    dispatch(dst, src, opacity, mode, Porter::Atop);
}

/// `dst = dst + (src − dst)·t` — used for pass-through folder opacity.
pub fn lerp_tile(dst: &mut TilePixels, src: &TilePixels, t: f32) {
    let t = fix15::from_f32(t) as u32;
    let inv = ONE - t;
    for (d, s) in dst.as_flattened_mut().iter_mut().zip(src.as_flattened()) {
        for c in 0..4 {
            d[c] = (fix15::mul(s[c] as u32, t) + fix15::mul(d[c] as u32, inv)) as u16;
        }
    }
}

fn dispatch(dst: &mut TilePixels, src: &TilePixels, opacity: f32, mode: BlendMode, p: Porter) {
    if opacity <= 0.0 {
        return;
    }
    match mode {
        BlendMode::Normal | BlendMode::PassThrough => normal(dst, src, opacity, p),
        BlendMode::Multiply => general(dst, src, opacity, p, |b, s| b * s),
        BlendMode::Screen => general(dst, src, opacity, p, screen),
        BlendMode::Overlay => general(dst, src, opacity, p, |b, s| hard_light(s, b)),
        BlendMode::Darken => general(dst, src, opacity, p, f32::min),
        BlendMode::Lighten => general(dst, src, opacity, p, f32::max),
        BlendMode::ColorDodge => general(dst, src, opacity, p, color_dodge),
        BlendMode::ColorBurn => general(dst, src, opacity, p, color_burn),
        BlendMode::LinearBurn => general(dst, src, opacity, p, |b, s| (b + s - 1.0).max(0.0)),
        BlendMode::Add => general(dst, src, opacity, p, |b, s| (b + s).min(1.0)),
        BlendMode::SoftLight => general(dst, src, opacity, p, soft_light),
        BlendMode::HardLight => general(dst, src, opacity, p, hard_light),
        BlendMode::Difference => general(dst, src, opacity, p, |b, s| (b - s).abs()),
    }
}

/// Integer source-over / source-atop.
fn normal(dst: &mut TilePixels, src: &TilePixels, opacity: f32, p: Porter) {
    let op = fix15::from_f32(opacity) as u32;
    for (d, s) in dst.as_flattened_mut().iter_mut().zip(src.as_flattened()) {
        if s[3] == 0 {
            continue;
        }
        let sa = fix15::mul(s[3] as u32, op);
        let inv = ONE - sa;
        match p {
            Porter::Over => {
                for c in 0..4 {
                    let v = fix15::mul(s[c] as u32, op) + fix15::mul(d[c] as u32, inv);
                    d[c] = v.min(ONE) as u16;
                }
            }
            Porter::Atop => {
                let da = d[3] as u32;
                for c in 0..3 {
                    let v = fix15::mul(fix15::mul(s[c] as u32, op), da)
                        + fix15::mul(d[c] as u32, inv);
                    d[c] = v.min(ONE) as u16;
                }
            }
        }
    }
}

#[inline(always)]
fn general<F: Fn(f32, f32) -> f32>(
    dst: &mut TilePixels,
    src: &TilePixels,
    opacity: f32,
    p: Porter,
    f: F,
) {
    const K: f32 = 1.0 / ONE as f32;
    for (d, s) in dst.as_flattened_mut().iter_mut().zip(src.as_flattened()) {
        if s[3] == 0 {
            continue;
        }
        let sa = s[3] as f32 * K * opacity;
        let da = d[3] as f32 * K;
        let inv_sa = 1.0 / sa;
        let inv_da = if da > 0.0 { 1.0 / da } else { 0.0 };
        for c in 0..3 {
            let sc = s[c] as f32 * K * opacity;
            let dc = d[c] as f32 * K;
            let mixed = sa * da * f(dc * inv_da, (sc * inv_sa).min(1.0));
            let out = match p {
                Porter::Over => sc * (1.0 - da) + dc * (1.0 - sa) + mixed,
                Porter::Atop => dc * (1.0 - sa) + mixed,
            };
            d[c] = fix15::from_f32(out);
        }
        if p == Porter::Over {
            d[3] = fix15::from_f32(sa + da - sa * da);
        }
    }
}

#[inline(always)]
fn screen(b: f32, s: f32) -> f32 {
    b + s - b * s
}

#[inline(always)]
fn hard_light(b: f32, s: f32) -> f32 {
    if s <= 0.5 {
        b * 2.0 * s
    } else {
        screen(b, 2.0 * s - 1.0)
    }
}

#[inline(always)]
fn color_dodge(b: f32, s: f32) -> f32 {
    if b <= 0.0 {
        0.0
    } else if s >= 1.0 {
        1.0
    } else {
        (b / (1.0 - s)).min(1.0)
    }
}

#[inline(always)]
fn color_burn(b: f32, s: f32) -> f32 {
    if b >= 1.0 {
        1.0
    } else if s <= 0.0 {
        0.0
    } else {
        1.0 - ((1.0 - b) / s).min(1.0)
    }
}

#[inline(always)]
fn soft_light(b: f32, s: f32) -> f32 {
    if s <= 0.5 {
        b - (1.0 - 2.0 * s) * b * (1.0 - b)
    } else {
        let d = if b <= 0.25 {
            ((16.0 * b - 12.0) * b + 4.0) * b
        } else {
            b.sqrt()
        };
        b + (2.0 * s - 1.0) * (d - b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fix15::ONE_U16;
    use crate::tile::{fill_tile, new_tile_box};

    const HALF: u16 = ONE_U16 / 2;

    fn tile(v: [u16; 4]) -> Box<TilePixels> {
        let mut t = new_tile_box();
        fill_tile(&mut t, v);
        t
    }

    fn close(a: u16, b: u16) -> bool {
        (a as i32 - b as i32).abs() <= 2
    }

    #[test]
    fn normal_opaque_replaces() {
        let mut d = tile([ONE_U16, 0, 0, ONE_U16]);
        blend_tile(&mut d, &tile([0, 0, ONE_U16, ONE_U16]), 1.0, BlendMode::Normal);
        assert_eq!(d[5][5], [0, 0, ONE_U16, ONE_U16]);
    }

    #[test]
    fn normal_half_opacity_mixes() {
        let mut d = tile([ONE_U16, ONE_U16, ONE_U16, ONE_U16]);
        blend_tile(&mut d, &tile([0, 0, 0, ONE_U16]), 0.5, BlendMode::Normal);
        assert!(close(d[0][0][0], HALF));
        assert_eq!(d[0][0][3], ONE_U16);
    }

    #[test]
    fn transparent_source_is_noop_for_all_modes() {
        for mode in BlendMode::LAYER_MODES {
            let mut d = tile([HALF, HALF, HALF, ONE_U16]);
            blend_tile(&mut d, &tile([0; 4]), 1.0, mode);
            assert_eq!(d[0][0], [HALF, HALF, HALF, ONE_U16], "{mode:?}");
        }
    }

    #[test]
    fn multiply_on_white_is_source() {
        let mut d = tile([ONE_U16; 4]);
        blend_tile(&mut d, &tile([HALF, 0, ONE_U16, ONE_U16]), 1.0, BlendMode::Multiply);
        assert!(close(d[0][0][0], HALF));
        assert!(close(d[0][0][1], 0));
        assert!(close(d[0][0][2], ONE_U16));
    }

    #[test]
    fn general_mode_over_empty_backdrop_is_source() {
        // With αb = 0 every separable mode degenerates to plain source.
        let mut d = tile([0; 4]);
        blend_tile(&mut d, &tile([HALF, 0, HALF, HALF]), 1.0, BlendMode::Screen);
        assert!(close(d[0][0][0], HALF) && close(d[0][0][3], HALF));
    }

    #[test]
    fn atop_preserves_backdrop_alpha() {
        let mut d = tile([0; 4]);
        d[0][0] = [ONE_U16, ONE_U16, ONE_U16, ONE_U16];
        let s = tile([0, 0, ONE_U16, ONE_U16]);
        blend_tile_atop(&mut d, &s, 1.0, BlendMode::Normal);
        assert_eq!(d[0][0], [0, 0, ONE_U16, ONE_U16]);
        assert_eq!(d[0][1], [0; 4], "clipped where backdrop is empty");

        let mut d2 = tile([0; 4]);
        d2[0][0] = [ONE_U16; 4];
        blend_tile_atop(&mut d2, &s, 1.0, BlendMode::Multiply);
        assert_eq!(d2[0][1], [0; 4]);
        assert!(close(d2[0][0][0], 0) && close(d2[0][0][2], ONE_U16));
    }
}
