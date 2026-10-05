//! Manuscript page setup: trim, bleed, safe area and inner frame guides.
//!
//! Lengths are stored in document px. Millimetres exist only in the UI
//! (`px = mm / 25.4 × dpi`).

use crate::geom::RectF;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PageSetup {
    /// The finished (cut) page.
    pub trim: RectF,
    /// Bleed past the trim on every side.
    pub bleed: f32,
    /// Safe margin inside the trim on every side.
    pub safe: f32,
    /// Inner frame (基本枠); `w == 0` means none.
    pub inner: RectF,
    /// Display unit: 0 mm, 1 in, 2 px.
    pub unit: u8,
}

/// A New-dialog manuscript preset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MangaPreset {
    pub name: &'static str,
    /// Canvas size; `None` = trim + 2 × bleed.
    pub paper_mm: Option<(f32, f32)>,
    pub trim_mm: (f32, f32),
    pub bleed_mm: f32,
    pub safe_mm: f32,
    pub inner_mm: (f32, f32),
    pub dpi: u32,
}

/// Typical starting points; users should check them against their
/// printer's spec. `dpi` is the suggested resolution.
pub const MANGA_PRESETS: &[MangaPreset] = &[
    MangaPreset {
        name: "JP commercial B4",
        paper_mm: Some((257.0, 364.0)),
        trim_mm: (220.0, 310.0),
        bleed_mm: 5.0,
        safe_mm: 5.0,
        inner_mm: (180.0, 270.0),
        dpi: 600,
    },
    MangaPreset {
        name: "Doujin B5",
        paper_mm: None,
        trim_mm: (182.0, 257.0),
        bleed_mm: 3.0,
        safe_mm: 5.0,
        inner_mm: (150.0, 220.0),
        dpi: 600,
    },
    MangaPreset {
        name: "Doujin A5",
        paper_mm: None,
        trim_mm: (148.0, 210.0),
        bleed_mm: 3.0,
        safe_mm: 5.0,
        inner_mm: (120.0, 180.0),
        dpi: 600,
    },
    MangaPreset {
        name: "A4",
        paper_mm: None,
        trim_mm: (210.0, 297.0),
        bleed_mm: 3.0,
        safe_mm: 5.0,
        inner_mm: (180.0, 267.0),
        dpi: 350,
    },
    MangaPreset {
        name: "US comic",
        paper_mm: None,
        trim_mm: (6.625 * MM_PER_IN, 10.1875 * MM_PER_IN),
        bleed_mm: 0.125 * MM_PER_IN,
        safe_mm: 0.25 * MM_PER_IN,
        inner_mm: (0.0, 0.0),
        dpi: 600,
    },
];

pub const MM_PER_IN: f32 = 25.4;

/// Largest bleed or safe margin kept (px); larger values are clamped.
const MAX_MARGIN: f32 = 100_000.0;

/// Display units of [`PageSetup::unit`].
pub const UNIT_MM: u8 = 0;
pub const UNIT_IN: u8 = 1;
pub const UNIT_PX: u8 = 2;

impl PageSetup {
    /// The trim grown by the bleed, clipped to the `w`×`h` canvas.
    pub fn bleed_rect(&self, w: u32, h: u32) -> RectF {
        let canvas = RectF { x: 0.0, y: 0.0, w: w as f32, h: h as f32 };
        self.trim.expand(self.bleed).intersect(canvas).unwrap_or_default()
    }

    /// The trim shrunk by the safe margin.
    pub fn safe_rect(&self) -> RectF {
        self.trim.expand(-self.safe)
    }

    /// Canvas size and page setup of `p` at `dpi`. Trim and bleed are
    /// rounded to whole px; the trim is centred (`floor((canvas − trim)/2)`)
    /// and the inner frame centred on it.
    pub fn from_mm(p: &MangaPreset, dpi: u32) -> (u32, u32, PageSetup) {
        let dpi = dpi.max(1);
        let px = |mm: f32| mm / MM_PER_IN * dpi as f32;
        let whole = |mm: f32| px(mm).round().max(1.0);
        let (tw, th) = (whole(p.trim_mm.0), whole(p.trim_mm.1));
        let bleed = px(p.bleed_mm).round().max(0.0);
        let (cw, ch) = match p.paper_mm {
            Some((w, h)) => (whole(w).max(tw), whole(h).max(th)),
            None => (tw + 2.0 * bleed, th + 2.0 * bleed),
        };
        let trim = RectF { x: ((cw - tw) / 2.0).floor(), y: ((ch - th) / 2.0).floor(), w: tw, h: th };
        let inner = if p.inner_mm.0 > 0.0 && p.inner_mm.1 > 0.0 {
            let (iw, ih) = (whole(p.inner_mm.0).min(tw), whole(p.inner_mm.1).min(th));
            RectF { x: trim.x + ((tw - iw) / 2.0).floor(), y: trim.y + ((th - ih) / 2.0).floor(), w: iw, h: ih }
        } else {
            RectF::default()
        };
        // A trim in sixteenths of an inch but not whole mm is an inch size.
        let sixteenths = p.trim_mm.0 / MM_PER_IN * 16.0;
        let inches = (sixteenths - sixteenths.round()).abs() < 1e-3 && p.trim_mm.0.fract() != 0.0;
        let unit = if inches { UNIT_IN } else { UNIT_MM };
        (cw as u32, ch as u32, PageSetup { trim, bleed, safe: px(p.safe_mm), inner, unit })
    }

    /// This setup if it is valid for a `w`×`h` canvas, with small fixes:
    /// every value finite, the trim non-empty and inside the canvas.
    /// Bleed and safe are clamped (safe to half the trim's short side), an
    /// inner frame is clipped to the canvas (dropped when nothing is
    /// left), and an unknown unit becomes mm. `None` when invalid.
    pub fn sanitized(self, w: u32, h: u32) -> Option<PageSetup> {
        let finite = |r: RectF| [r.x, r.y, r.w, r.h].iter().all(|v| v.is_finite());
        let t = self.trim;
        if !finite(t) || !finite(self.inner) || !self.bleed.is_finite() || !self.safe.is_finite() {
            return None;
        }
        // Allow float noise from unit conversions at the edges.
        const SLACK: f32 = 1e-3;
        if t.w <= 0.0
            || t.h <= 0.0
            || t.x < -SLACK
            || t.y < -SLACK
            || t.x + t.w > w as f32 + SLACK
            || t.y + t.h > h as f32 + SLACK
        {
            return None;
        }
        let canvas = RectF { x: 0.0, y: 0.0, w: w as f32, h: h as f32 };
        let trim = t.intersect(canvas)?;
        let inner = if self.inner.w > 0.0 && self.inner.h > 0.0 {
            self.inner.intersect(canvas).unwrap_or_default()
        } else {
            RectF::default()
        };
        Some(PageSetup {
            trim,
            bleed: self.bleed.clamp(0.0, MAX_MARGIN),
            safe: self.safe.clamp(0.0, trim.w.min(trim.h) / 2.0),
            inner,
            unit: if self.unit <= UNIT_PX { self.unit } else { UNIT_MM },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guide_rects() {
        let trim = RectF { x: 100.0, y: 50.0, w: 800.0, h: 1200.0 };
        let p = PageSetup { trim, bleed: 60.0, safe: 40.0, inner: RectF::default(), unit: 0 };
        assert_eq!(p.bleed_rect(1000, 1300), RectF { x: 40.0, y: 0.0, w: 920.0, h: 1300.0 }, "clipped to the canvas");
        assert_eq!(p.safe_rect(), RectF { x: 140.0, y: 90.0, w: 720.0, h: 1120.0 });
    }
}
